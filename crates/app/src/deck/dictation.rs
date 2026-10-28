//! Optional dictation (Whisper is only downloaded if the person wants it).

use super::*;

impl Deck {
    /// At app startup: downloads what is missing (whoever already uses dictation) or opens the question (whoever was never asked).
    pub fn dictation_startup(&mut self) {
        match self.core.dictation_startup() {
            agentdeck_core::whisper::Startup::Ensure => self.core.ensure_whisper_async(),
            agentdeck_core::whisper::Startup::Ask => self.dictation_prompt = true,
            agentdeck_core::whisper::Startup::Skip => {}
        }
    }

    /// Whisper state on disk, for the microphone button and the settings.
    pub fn dictation_setup(&self) -> Setup {
        Setup::of(self.dictation_installed, self.core.dictation_downloading())
    }

    /// "Download": saves the "yes" and downloads in the background (the progress bar appears in the right bar).
    pub fn dictation_accept(&mut self) {
        self.dictation_prompt = false;
        self.core.set_dictation_choice(DictationChoice::Yes);
        self.core.ensure_whisper_async();
        self.refresh_engine();
    }

    /// "Not now": saves the "no" (does not ask again, does not download) and closes the question.
    pub fn dictation_decline(&mut self) {
        self.dictation_prompt = false;
        self.core.set_dictation_choice(DictationChoice::No);
    }

    /// Settings: deletes what dictation downloaded.
    pub fn dictation_remove(&mut self) {
        self.dictation_confirm_remove = false;
        if self.mic_phase != MicPhase::Idle {
            self.system(t!("Finish the dictation recording before removing Whisper."));
            return;
        }
        match self.core.remove_dictation() {
            Ok(_) => {
                self.dictation_installed = self.core.dictation_installed();
                self.system(t!("Voice dictation removed from disk. You can download it again from the settings."));
            }
            Err(e) => self.system(t!("Could not remove voice dictation: {error}.", error = e)),
        }
    }

    /// Before recording: with Whisper ready it proceeds; without it, explains or offers the download instead of failing silently.
    fn mic_ready(&mut self) -> bool {
        // Built without audio (`--no-default-features`): offering the download would be 1.6 GB for nothing to record.
        if !crate::audio::AVAILABLE {
            self.system(t!("Voice dictation is unavailable: AgentDeck was built without the audio feature."));
            return false;
        }
        match self.dictation_setup() {
            Setup::Ready => true,
            Setup::Downloading => {
                self.system(t!("Voice dictation is still downloading (see the right bar). Try again when it finishes."));
                false
            }
            Setup::NotInstalled => {
                self.dictation_prompt = true;
                false
            }
        }
    }

    /// Triggers the microphone for `target` (`Tab(uid)`). Opening the device can take
    /// up to 5 s: it goes to a thread, and a second click meanwhile gives up.
    pub(super) fn toggle_mic_for(&mut self, target: MicTarget) {
        if self.mic_phase == MicPhase::Idle && !self.mic_ready() {
            return;
        }
        match crate::dictation::toggle(&mut self.mic_phase, target) {
            ToggleOutcome::StartOpening(_) => {
                self.spawn_bg("mic-open", |_| Some(UiMsg::MicOpened(audio::Recorder::start())));
            }
            ToggleOutcome::CancelOpening => {}
            ToggleOutcome::StopAndTranscribe => self.stop_recording(),
            ToggleOutcome::Busy => {}
        }
    }

    pub(super) fn on_mic_opened(&mut self, res: Result<audio::Recorder, String>) {
        let was_opening = self.mic_phase.is_opening();
        match res {
            Ok(r) if self.recorder.is_none() && crate::dictation::on_opened(&mut self.mic_phase, true) => {
                self.recorder = Some(r);
                self.recording_since = Some(Instant::now());
            }
            Ok(r) => {
                crate::dictation::on_opened(&mut self.mic_phase, false);
                discard_recorder(r);
            }
            Err(e) => {
                crate::dictation::on_opened(&mut self.mic_phase, false);
                if was_opening {
                    self.system(t!("Microphone unavailable: {error}", error = e));
                }
            }
        }
    }

    /// `recorder.stop()` → transcribes on a thread.
    pub(super) fn stop_recording(&mut self) {
        let Some(rec) = self.recorder.take() else { return };
        self.recording_since = None;
        crate::dictation::begin_transcribe(&mut self.mic_phase);
        self.spawn_bg("transcribe", move |core| {
            let pcm = rec.stop();
            let res = if pcm.is_empty() {
                Err(t!("no audio captured").to_string())
            } else {
                core.transcribe_pcm16k(&pcm).map(|t| t.text).map_err(|e| e.to_string())
            };
            Some(UiMsg::Transcribed(res))
        });
    }

    /// `pty:submit`: pastes in brackets and confirms with Enter after 250 ms (and again after
    /// 600 ms when there is an `@` mention in CLIs with file autocomplete).
    ///
    /// The Enters leave from the interface thread (in `tick`), in the right order relative to what the
    /// user types meanwhile; a thread only wakes the interface on time.
    pub(super) fn submit(&mut self, idx: usize, text: &str) {
        let now = Instant::now();
        let tab = &mut self.tabs[idx];
        // A previous submit still waiting for the Enter is confirmed before this paste.
        for _ in tab.pending_enter.drain(..) {
            tab.session.write(b"\r");
        }
        if !tab.held_keys.is_empty() {
            let held = std::mem::take(&mut tab.held_keys);
            tab.session.write(&held);
        }
        if !tab.busy {
            tab.busy_output = false;
            tab.idle_deadline = Some(now + NO_OUTPUT_IDLE);
        }
        tab.busy = true;
        tab.turn_confirmed = false;
        tab.typing = false;
        tab.done = false;
        tab.session.write(&input::submit_paste(text));
        let double = text.contains('@') && tab.spec.as_ref().is_some_and(|s| s.double_enter_on_mention);
        let kind = tab.spec.as_ref().map_or("", |s| s.kind.as_str());
        let mut times = vec![now + delivery::enter_delay(kind, text, FIRST_ENTER)];
        if double {
            times.push(now + SECOND_ENTER);
        }
        tab.pending_enter.extend(times.iter().copied());
        let wake = self.wake.clone();
        let _ = std::thread::Builder::new().name("submit-wake".into()).spawn(move || {
            for at in times {
                std::thread::sleep(at.saturating_duration_since(Instant::now()) + Duration::from_millis(2));
                wake();
            }
        });
    }

    /// Keeps each tab's process closed when the window closes.
    pub fn shutdown(&mut self) {
        self.persist_tabs_now();
        self.persist_layout_now();
        for t in &mut self.tabs {
            t.session.kill();
        }
    }
}
