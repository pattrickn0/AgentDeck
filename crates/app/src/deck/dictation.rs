//! Ditado opcional (o Whisper só é baixado se a pessoa quiser).

use super::*;

impl Deck {
    /// Na abertura do app: baixa o que faltar (quem já usa o ditado) ou abre a pergunta (quem nunca foi perguntado).
    pub fn dictation_startup(&mut self) {
        match self.core.dictation_startup() {
            agentdeck_core::whisper::Startup::Ensure => self.core.ensure_whisper_async(),
            agentdeck_core::whisper::Startup::Ask => self.dictation_prompt = true,
            agentdeck_core::whisper::Startup::Skip => {}
        }
    }

    /// Estado do Whisper no disco, para o botão de microfone e as configurações.
    pub fn dictation_setup(&self) -> Setup {
        Setup::of(self.dictation_installed, self.core.dictation_downloading())
    }

    /// "Baixar": grava o "sim" e baixa em segundo plano (a faixa de progresso aparece na barra direita).
    pub fn dictation_accept(&mut self) {
        self.dictation_prompt = false;
        self.core.set_dictation_choice(DictationChoice::Yes);
        self.core.ensure_whisper_async();
        self.refresh_engine();
    }

    /// "Agora não": grava o "não" (não pergunta de novo nem baixa) e fecha a pergunta.
    pub fn dictation_decline(&mut self) {
        self.dictation_prompt = false;
        self.core.set_dictation_choice(DictationChoice::No);
    }

    /// Configurações: apaga o que o ditado baixou.
    pub fn dictation_remove(&mut self) {
        self.dictation_confirm_remove = false;
        if self.mic_phase != MicPhase::Idle {
            self.system("Termine a gravação do ditado antes de remover o Whisper.");
            return;
        }
        match self.core.remove_dictation() {
            Ok(_) => {
                self.dictation_installed = self.core.dictation_installed();
                self.system("Ditado por voz removido do disco. Dá para baixar de novo pelas configurações.");
            }
            Err(e) => self.system(format!("Não foi possível remover o ditado por voz: {e}.")),
        }
    }

    /// Antes de gravar: com o Whisper pronto segue; sem ele, explica ou oferece o download em vez de falhar calado.
    fn mic_ready(&mut self) -> bool {
        // Compilado sem áudio (`--no-default-features`): oferecer o download seria baixar 1,6 GB para nada gravar.
        if !crate::audio::AVAILABLE {
            self.system("O ditado por voz não está disponível: o AgentDeck foi compilado sem o recurso de áudio.");
            return false;
        }
        match self.dictation_setup() {
            Setup::Ready => true,
            Setup::Downloading => {
                self.system("O ditado por voz ainda está baixando (veja a barra direita). Tente de novo quando terminar.");
                false
            }
            Setup::NotInstalled => {
                self.dictation_prompt = true;
                false
            }
        }
    }

    /// Aciona o microfone para `target` (`Tab(uid)`). Abrir o dispositivo pode levar
    /// até 5 s: vai para uma thread, e um segundo clique enquanto isso desiste.
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
                    self.system(format!("Microfone indisponível: {e}"));
                }
            }
        }
    }

    /// `recorder.stop()` → transcreve numa thread.
    pub(super) fn stop_recording(&mut self) {
        let Some(rec) = self.recorder.take() else { return };
        self.recording_since = None;
        crate::dictation::begin_transcribe(&mut self.mic_phase);
        self.spawn_bg("transcribe", move |core| {
            let pcm = rec.stop();
            let res = if pcm.is_empty() {
                Err("nenhum áudio capturado".to_string())
            } else {
                core.transcribe_pcm16k(&pcm).map(|t| t.text).map_err(|e| e.to_string())
            };
            Some(UiMsg::Transcribed(res))
        });
    }

    /// `pty:submit`: cola entre colchetes e confirma com Enter após 250 ms (e de novo após
    /// 600 ms quando há menção `@` em CLIs com autocomplete de arquivo).
    ///
    /// Os Enters saem da thread da interface (em `tick`), na ordem certa em relação ao que o
    /// usuário digitar nesse meio-tempo; uma thread só acorda a interface na hora.
    pub(super) fn submit(&mut self, idx: usize, text: &str) {
        let now = Instant::now();
        let tab = &mut self.tabs[idx];
        // Um envio anterior que ainda esperava o Enter é confirmado antes desta colagem.
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

    /// Mantém o processo de cada aba encerrado ao fechar a janela.
    pub fn shutdown(&mut self) {
        self.persist_tabs_now();
        self.persist_layout_now();
        for t in &mut self.tabs {
            t.session.kill();
        }
    }
}
