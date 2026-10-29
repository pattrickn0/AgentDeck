//! Voice dictation: states (idle, opening microphone, recording, transcribing) and the target
//! pinned at the start of recording (a specific tab by `uid`).
//!
//! Pure logic independent of painting, with tests, in the style of [`crate::split`].

use holt_core::t;

use crate::audio;

/// Where transcribed text is delivered when transcription finishes, pinned at the start of recording.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MicTarget {
    /// The terminal (PTY) of the tab with this `uid` (typed as paste, without Enter).
    Tab(u64),
}

/// Microphone phase for the entire app (at most one recording at a time).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum MicPhase {
    #[default]
    Idle,
    /// Opening the input device on a thread (may take up to 5 s).
    Opening(MicTarget),
    /// Capturing audio for `MicTarget`.
    Recording(MicTarget),
    /// Running Whisper on a thread for `MicTarget`.
    Transcribing(MicTarget),
}

impl MicPhase {
    pub fn is_opening(self) -> bool {
        matches!(self, Self::Opening(_))
    }
}

/// What clicking the microphone button (or the tab shortcut) requests to do.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ToggleOutcome {
    /// Starts opening the microphone for this target.
    StartOpening(MicTarget),
    /// User clicked again while opening: cancels.
    CancelOpening,
    /// Stops the current recording and requests transcription.
    StopAndTranscribe,
    /// Already transcribing (or recording in another tab): ignores click.
    Busy,
}

/// Apparent state of a specific microphone button (`button`), given the global state (`phase`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ButtonVisual {
    Idle,
    Opening,
    Recording,
    Transcribing,
    /// Another tab is using the microphone: this button is disabled.
    DisabledOther,
}

impl ButtonVisual {
    /// The button does not accept clicks (transcribing or busy in another target).
    pub fn is_busy(self) -> bool {
        matches!(self, Self::Transcribing | Self::DisabledOther)
    }
}

/// How `button` appears when the global state is `phase`.
pub fn button_visual(phase: MicPhase, button: MicTarget) -> ButtonVisual {
    match phase {
        MicPhase::Idle => ButtonVisual::Idle,
        MicPhase::Opening(t) if t == button => ButtonVisual::Opening,
        MicPhase::Recording(t) if t == button => ButtonVisual::Recording,
        MicPhase::Transcribing(t) if t == button => ButtonVisual::Transcribing,
        MicPhase::Opening(_) | MicPhase::Recording(_) | MicPhase::Transcribing(_) => ButtonVisual::DisabledOther,
    }
}

/// Pure transition when triggering the microphone on `clicked` (panel header button or shortcut).
///
/// Only one recording exists at a time across the whole app: if already opening or recording on
/// `clicked`, the second click cancels/stops; if busy in another target or transcribing, returns `Busy`
/// without altering `phase`.
pub fn toggle(phase: &mut MicPhase, clicked: MicTarget) -> ToggleOutcome {
    match *phase {
        MicPhase::Idle => {
            *phase = MicPhase::Opening(clicked);
            ToggleOutcome::StartOpening(clicked)
        }
        MicPhase::Opening(t) if t == clicked => {
            *phase = MicPhase::Idle;
            ToggleOutcome::CancelOpening
        }
        MicPhase::Recording(t) if t == clicked => {
            *phase = MicPhase::Transcribing(t);
            ToggleOutcome::StopAndTranscribe
        }
        MicPhase::Opening(_) | MicPhase::Recording(_) | MicPhase::Transcribing(_) => ToggleOutcome::Busy,
    }
}

/// Result of `Recorder::start` arriving from the thread: only transitions to `Recording` if the user
/// was still waiting in `Opening(t)`; otherwise returns `false` to discard the recorder.
pub fn on_opened(phase: &mut MicPhase, ok: bool) -> bool {
    match *phase {
        MicPhase::Opening(t) if ok => {
            *phase = MicPhase::Recording(t);
            true
        }
        MicPhase::Opening(_) => {
            *phase = MicPhase::Idle;
            false
        }
        _ => false,
    }
}

/// Stops the ongoing recording (e.g. time limit) and enters `Transcribing`, preserving original target.
pub fn begin_transcribe(phase: &mut MicPhase) -> Option<MicTarget> {
    match *phase {
        MicPhase::Recording(t) => {
            *phase = MicPhase::Transcribing(t);
            Some(t)
        }
        _ => None,
    }
}

/// Final delivery target of a completed transcription for a tab, checking the open tab list `(uid, alive)`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TabDelivery {
    /// Tab `uid` is open at index `idx` with running process: type into its PTY.
    TypeInTab(usize),
    /// Tab still exists, but process exited before transcription finished.
    TabDead,
    /// Tab was closed during transcription: discard text with notice, do not route elsewhere.
    TabClosed,
}

/// Resolves which tab (index) receives transcribed text for `target_uid`.
///
/// Never falls back to the active tab or another tab: if `target_uid` is no longer in `tabs`, returns
/// [`TabDelivery::TabClosed`].
pub fn resolve_tab_delivery(tabs: &[(u64, bool)], target_uid: u64) -> TabDelivery {
    match tabs.iter().position(|&(uid, _)| uid == target_uid) {
        Some(idx) if tabs[idx].1 => TabDelivery::TypeInTab(idx),
        Some(_) => TabDelivery::TabDead,
        None => TabDelivery::TabClosed,
    }
}

/// Finishes the `Transcribing` phase, returning to `Idle` and yielding the pinned target.
pub fn finish_transcribe(phase: &mut MicPhase) -> Option<MicTarget> {
    match *phase {
        MicPhase::Transcribing(t) => {
            *phase = MicPhase::Idle;
            Some(t)
        }
        _ => None,
    }
}

/// Whether Whisper (model and engine) is on disk. Dictation is optional: without it the mic offers download.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Setup {
    /// Ready to record and transcribe.
    Ready,
    /// Downloading (progress bar is in the right panel).
    Downloading,
    /// Not installed: clicking the mic (or `Ctrl+Shift+M`) offers download.
    NotInstalled,
}

impl Setup {
    /// With the model and an engine on disk dictation works, even if an extra engine (CUDA) is still downloading.
    pub fn of(installed: bool, downloading: bool) -> Self {
        if installed {
            Self::Ready
        } else if downloading {
            Self::Downloading
        } else {
            Self::NotInstalled
        }
    }
}

/// Download size text shown in UI (first-time prompt and settings option).
pub fn download_size() -> &'static str {
    t!("about 1.6 GB")
}

/// Tooltip for the microphone button in the panel header.
/// `shortcut` appears in parentheses when applicable (e.g. `Some("Ctrl+Shift+M")`).
pub fn mic_tooltip(visual: ButtonVisual, available: bool, setup: Setup, engine_label: &str, shortcut: Option<&str>) -> String {
    let minutes = audio::MAX_RECORDING_SECS / 60;
    match visual {
        ButtonVisual::Recording => match shortcut {
            Some(shortcut) => t!("Stop recording and transcribe ({shortcut}) (up to {minutes} min)", shortcut, minutes),
            None => t!("Stop recording and transcribe (up to {minutes} min)", minutes),
        },
        ButtonVisual::Opening => match shortcut {
            Some(shortcut) => t!("Opening microphone… ({shortcut}) (click to cancel)", shortcut),
            None => t!("Opening microphone… (click to cancel)").to_owned(),
        },
        ButtonVisual::Transcribing => t!("Transcribing audio…").to_owned(),
        ButtonVisual::DisabledOther => t!("Microphone in use in another tab").to_owned(),
        ButtonVisual::Idle if !available => t!("Voice dictation (unavailable: built without audio feature)").to_owned(),
        ButtonVisual::Idle if setup == Setup::NotInstalled => {
            let size = download_size();
            match shortcut {
                Some(shortcut) => t!("Voice dictation not installed: click to download ({size}) ({shortcut})", size, shortcut),
                None => t!("Voice dictation not installed: click to download ({size})", size),
            }
        }
        ButtonVisual::Idle if setup == Setup::Downloading => t!("Downloading voice dictation… (see right bar)").to_owned(),
        ButtonVisual::Idle => {
            let eng = if engine_label.is_empty() { "…" } else { engine_label };
            match shortcut {
                Some(shortcut) => t!("Dictate in this tab ({shortcut} · transcription: {engine})", shortcut, engine = eng),
                None => t!("Dictate in this tab (transcription: {engine})", engine = eng),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use holt_core::i18n::{self, Lang};

    #[test]
    fn target_stays_pinned_to_the_tab_where_recording_started() {
        let mut phase = MicPhase::Idle;
        assert_eq!(toggle(&mut phase, MicTarget::Tab(7)), ToggleOutcome::StartOpening(MicTarget::Tab(7)));
        assert_eq!(phase, MicPhase::Opening(MicTarget::Tab(7)));
        assert!(on_opened(&mut phase, true));
        assert_eq!(phase, MicPhase::Recording(MicTarget::Tab(7)));

        // Even if the user switches tabs and clicks another (uid 9),
        // recording stays pinned to tab 7.
        assert_eq!(toggle(&mut phase, MicTarget::Tab(9)), ToggleOutcome::Busy);
        assert_eq!(phase, MicPhase::Recording(MicTarget::Tab(7)));

        // Stop in tab 7 (or via shortcut): transcribes for tab 7.
        assert_eq!(toggle(&mut phase, MicTarget::Tab(7)), ToggleOutcome::StopAndTranscribe);
        assert_eq!(phase, MicPhase::Transcribing(MicTarget::Tab(7)));
        assert_eq!(finish_transcribe(&mut phase), Some(MicTarget::Tab(7)));
        assert_eq!(phase, MicPhase::Idle);
    }

    #[test]
    fn second_click_while_opening_cancels_and_discards_late_recorder() {
        let mut phase = MicPhase::Idle;
        assert_eq!(toggle(&mut phase, MicTarget::Tab(3)), ToggleOutcome::StartOpening(MicTarget::Tab(3)));
        assert_eq!(toggle(&mut phase, MicTarget::Tab(3)), ToggleOutcome::CancelOpening);
        assert_eq!(phase, MicPhase::Idle);
        // When the thread finishes opening after canceling, the recorder is discarded.
        assert!(!on_opened(&mut phase, true));
        assert_eq!(phase, MicPhase::Idle);
    }

    #[test]
    fn closed_or_dead_target_tab_never_falls_back_to_another_tab() {
        // Open tabs: uid 10 (index 0, alive) and uid 20 (index 1, alive).
        let tabs = [(10, true), (20, true)];
        assert_eq!(resolve_tab_delivery(&tabs, 20), TabDelivery::TypeInTab(1));
        // Tab 30 closed during transcription (even with 10 and 20 open): discard.
        assert_eq!(resolve_tab_delivery(&tabs, 30), TabDelivery::TabClosed);
        // Tab 20 process exited: do not type into 10.
        let tabs_dead = [(10, true), (20, false)];
        assert_eq!(resolve_tab_delivery(&tabs_dead, 20), TabDelivery::TabDead);
    }

    #[test]
    fn button_visuals_reflect_all_four_states_and_lock_out_other_buttons() {
        let tab1 = MicTarget::Tab(1);
        let tab2 = MicTarget::Tab(2);

        assert_eq!(button_visual(MicPhase::Idle, tab1), ButtonVisual::Idle);
        assert_eq!(button_visual(MicPhase::Opening(tab1), tab1), ButtonVisual::Opening);
        assert_eq!(button_visual(MicPhase::Opening(tab1), tab2), ButtonVisual::DisabledOther);
        assert_eq!(button_visual(MicPhase::Recording(tab1), tab1), ButtonVisual::Recording);
        assert_eq!(button_visual(MicPhase::Recording(tab1), tab2), ButtonVisual::DisabledOther);
        assert_eq!(button_visual(MicPhase::Transcribing(tab1), tab1), ButtonVisual::Transcribing);
        assert!(button_visual(MicPhase::Transcribing(tab1), tab1).is_busy());
        assert!(button_visual(MicPhase::Recording(tab1), tab2).is_busy());
        assert!(!button_visual(MicPhase::Recording(tab1), tab1).is_busy());
    }

    #[test]
    fn tooltips_cover_every_state() {
        let ready = Setup::Ready;
        assert!(mic_tooltip(ButtonVisual::Idle, true, ready, "GPU CUDA", Some("Ctrl+Shift+M")).contains("Ctrl+Shift+M"));
        assert!(mic_tooltip(ButtonVisual::Idle, false, ready, "", None).contains("unavailable"));
        assert!(mic_tooltip(ButtonVisual::Opening, true, ready, "", Some("Ctrl+Shift+M")).contains("Opening"));
        assert!(mic_tooltip(ButtonVisual::Recording, true, ready, "", Some("Ctrl+Shift+M")).contains("Stop recording"));
        assert!(mic_tooltip(ButtonVisual::Transcribing, true, ready, "", None).contains("Transcribing"));
        assert!(mic_tooltip(ButtonVisual::DisabledOther, true, ready, "", None).contains("in use"));

        // pt-BR check
        i18n::with_lang(Lang::PtBr, || {
            assert!(mic_tooltip(ButtonVisual::Idle, false, ready, "", None).contains("indisponível"));
            assert!(mic_tooltip(ButtonVisual::Opening, true, ready, "", Some("Ctrl+Shift+M")).contains("Abrindo"));
            assert!(mic_tooltip(ButtonVisual::Recording, true, ready, "", Some("Ctrl+Shift+M")).contains("Parar gravação"));
            assert!(mic_tooltip(ButtonVisual::Transcribing, true, ready, "", None).contains("Transcrevendo"));
            assert!(mic_tooltip(ButtonVisual::DisabledOther, true, ready, "", None).contains("em uso"));
        });
    }

    #[test]
    fn without_whisper_the_mic_explains_and_offers_the_download() {
        assert_eq!(Setup::of(true, false), Setup::Ready);
        assert_eq!(Setup::of(true, true), Setup::Ready, "CUDA engine downloading does not take dictation offline");
        assert_eq!(Setup::of(false, true), Setup::Downloading);
        assert_eq!(Setup::of(false, false), Setup::NotInstalled);
        let off = mic_tooltip(ButtonVisual::Idle, true, Setup::NotInstalled, "unavailable", Some("Ctrl+Shift+M"));
        assert!(off.contains("not installed") && off.contains("download") && off.contains("about 1.6 GB"), "{off}");
        assert!(!off.contains("transcription:"), "does not show engine for nonexistent dictation: {off}");
        assert!(mic_tooltip(ButtonVisual::Idle, true, Setup::Downloading, "", None).contains("Downloading"));
        // Recording or transcribing, the tip is standard (Whisper was ready when started).
        assert!(mic_tooltip(ButtonVisual::Recording, true, Setup::NotInstalled, "", None).contains("Stop recording"));

        // pt-BR check
        i18n::with_lang(Lang::PtBr, || {
            let off = mic_tooltip(ButtonVisual::Idle, true, Setup::NotInstalled, "indisponível", Some("Ctrl+Shift+M"));
            assert!(off.contains("não instalado") && off.contains("baixar") && off.contains("cerca de 1,6 GB"), "{off}");
            assert!(!off.contains("transcrição:"), "does not show engine for nonexistent dictation: {off}");
            assert!(mic_tooltip(ButtonVisual::Idle, true, Setup::Downloading, "", None).contains("Baixando"));
            assert!(mic_tooltip(ButtonVisual::Recording, true, Setup::NotInstalled, "", None).contains("Parar gravação"));
        });
    }
}
