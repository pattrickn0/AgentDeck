//! Ditado por voz: estados (parado, abrindo o microfone, gravando, transcrevendo) e o destino
//! fixo no início da gravação (compositor ou uma aba específica pelo `uid`).
//!
//! Lógica pura e independente da pintura, com testes, no estilo de [`crate::split`].

use crate::audio;

/// Para onde vai o texto quando a transcrição terminar, fixado no início da gravação.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MicTarget {
    /// O rascunho do compositor da Conversa integrada.
    Composer,
    /// O terminal (PTY) da aba com este `uid` (digitado como colagem, sem Enter).
    Tab(u64),
}

/// Fase do microfone no app inteiro (há no máximo uma gravação por vez).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum MicPhase {
    #[default]
    Idle,
    /// Abrindo o dispositivo de entrada numa thread (pode levar até 5 s).
    Opening(MicTarget),
    /// Capturando áudio para `MicTarget`.
    Recording(MicTarget),
    /// Rodando o Whisper numa thread para `MicTarget`.
    Transcribing(MicTarget),
}

impl MicPhase {
    pub fn is_opening(self) -> bool {
        matches!(self, Self::Opening(_))
    }
}

/// O que um clique no botão de microfone (ou o atalho da aba) pede para fazer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ToggleOutcome {
    /// Começa a abrir o microfone para este destino.
    StartOpening(MicTarget),
    /// O usuário clicou de novo enquanto abria: desiste.
    CancelOpening,
    /// Para a gravação atual e manda transcrever.
    StopAndTranscribe,
    /// Já está transcrevendo (ou gravando em outro lugar): ignora o clique.
    Busy,
}

/// Estado aparente de um botão de microfone específico (`button`), dado o estado global (`phase`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ButtonVisual {
    Idle,
    Opening,
    Recording,
    Transcribing,
    /// Outra aba (ou o compositor) está usando o microfone: este botão fica desabilitado.
    DisabledOther,
}

impl ButtonVisual {
    /// O botão não aceita clique (transcrevendo ou ocupado em outro destino).
    pub fn is_busy(self) -> bool {
        matches!(self, Self::Transcribing | Self::DisabledOther)
    }
}

/// Como o botão de `button` aparece quando o estado global é `phase`.
pub fn button_visual(phase: MicPhase, button: MicTarget) -> ButtonVisual {
    match phase {
        MicPhase::Idle => ButtonVisual::Idle,
        MicPhase::Opening(t) if t == button => ButtonVisual::Opening,
        MicPhase::Recording(t) if t == button => ButtonVisual::Recording,
        MicPhase::Transcribing(t) if t == button => ButtonVisual::Transcribing,
        MicPhase::Opening(_) | MicPhase::Recording(_) | MicPhase::Transcribing(_) => ButtonVisual::DisabledOther,
    }
}

/// Transição pura ao acionar o microfone em `clicked` (botão do compositor, botão da aba ou atalho).
///
/// Só existe uma gravação por vez no app inteiro: se já está abrindo ou gravando em `clicked`, o
/// segundo clique cancela/para; se está ocupado em outro destino ou transcrevendo, devolve `Busy`
/// sem alterar `phase`.
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

/// Resultado de `Recorder::start` chegando da thread: só passa a `Recording` se o usuário ainda
/// estava esperando em `Opening(t)`; caso contrário devolve `false` para descartar o gravador.
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

/// Para a gravação em andamento (limite de tempo ou "Enviar" do compositor) e entra em
/// `Transcribing`, preservando o destino original.
pub fn begin_transcribe(phase: &mut MicPhase) -> Option<MicTarget> {
    match *phase {
        MicPhase::Recording(t) => {
            *phase = MicPhase::Transcribing(t);
            Some(t)
        }
        _ => None,
    }
}

/// Cancela abertura ou gravação em andamento e volta a `Idle`.
pub fn cancel(phase: &mut MicPhase) -> Option<MicTarget> {
    match *phase {
        MicPhase::Opening(t) | MicPhase::Recording(t) => {
            *phase = MicPhase::Idle;
            Some(t)
        }
        _ => None,
    }
}

/// Destino final de uma transcrição concluída para uma aba, conferindo a lista de abas abertas
/// `(uid, alive)`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TabDelivery {
    /// A aba `uid` está aberta no índice `idx` e com o processo rodando: digitar no PTY dela.
    TypeInTab(usize),
    /// A aba ainda existe, mas o processo encerrou antes de a transcrição terminar.
    TabDead,
    /// A aba foi fechada durante a transcrição: descartar o texto com aviso, sem mandar para outra.
    TabClosed,
}

/// Resolve para qual aba (índice) vai o texto transcrito de `target_uid`.
///
/// Nunca cai para a aba ativa nem para outra aba: se `target_uid` não está mais em `tabs`, devolve
/// [`TabDelivery::TabClosed`].
pub fn resolve_tab_delivery(tabs: &[(u64, bool)], target_uid: u64) -> TabDelivery {
    match tabs.iter().position(|&(uid, _)| uid == target_uid) {
        Some(idx) if tabs[idx].1 => TabDelivery::TypeInTab(idx),
        Some(_) => TabDelivery::TabDead,
        None => TabDelivery::TabClosed,
    }
}

/// Conclui a fase `Transcribing`, voltando a `Idle` e devolvendo o destino fixado no início.
pub fn finish_transcribe(phase: &mut MicPhase) -> Option<MicTarget> {
    match *phase {
        MicPhase::Transcribing(t) => {
            *phase = MicPhase::Idle;
            Some(t)
        }
        _ => None,
    }
}

/// Dica (tooltip) do botão de microfone em qualquer lugar (compositor ou cabeçalho da aba).
/// `shortcut` aparece entre parênteses quando é um botão de aba (ex.: `Some("Ctrl+Shift+M")`).
pub fn mic_tooltip(visual: ButtonVisual, available: bool, engine_label: &str, shortcut: Option<&str>) -> String {
    let sc = shortcut.map(|s| format!(" ({s})")).unwrap_or_default();
    match visual {
        ButtonVisual::Recording => format!("Parar gravação e transcrever{sc} (até {} min)", audio::MAX_RECORDING_SECS / 60),
        ButtonVisual::Opening => format!("Abrindo o microfone…{sc} (clique para desistir)"),
        ButtonVisual::Transcribing => "Transcrevendo áudio…".to_owned(),
        ButtonVisual::DisabledOther => "Microfone em uso em outra aba ou na Conversa".to_owned(),
        ButtonVisual::Idle if !available => "Ditado por voz (indisponível: compilado sem o recurso de áudio)".to_owned(),
        ButtonVisual::Idle => {
            let eng = if engine_label.is_empty() { "…" } else { engine_label };
            match shortcut {
                Some(s) => format!("Ditar nesta aba ({s} · transcrição: {eng})"),
                None => format!("Gravar áudio (transcrição: {eng})"),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn target_stays_pinned_to_the_tab_where_recording_started() {
        let mut phase = MicPhase::Idle;
        assert_eq!(toggle(&mut phase, MicTarget::Tab(7)), ToggleOutcome::StartOpening(MicTarget::Tab(7)));
        assert_eq!(phase, MicPhase::Opening(MicTarget::Tab(7)));
        assert!(on_opened(&mut phase, true));
        assert_eq!(phase, MicPhase::Recording(MicTarget::Tab(7)));

        // Mesmo que o usuário troque de aba e tente clicar em outra (uid 9) ou no compositor,
        // a gravação continua presa à aba 7.
        assert_eq!(toggle(&mut phase, MicTarget::Tab(9)), ToggleOutcome::Busy);
        assert_eq!(toggle(&mut phase, MicTarget::Composer), ToggleOutcome::Busy);
        assert_eq!(phase, MicPhase::Recording(MicTarget::Tab(7)));

        // Parar na aba 7 (ou pelo atalho): transcreve para a aba 7.
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
        // Quando a thread terminar de abrir depois da desistência, o gravador é descartado.
        assert!(!on_opened(&mut phase, true));
        assert_eq!(phase, MicPhase::Idle);
    }

    #[test]
    fn closed_or_dead_target_tab_never_falls_back_to_another_tab() {
        // Abas abertas: uid 10 (índice 0, viva) e uid 20 (índice 1, viva).
        let tabs = [(10, true), (20, true)];
        assert_eq!(resolve_tab_delivery(&tabs, 20), TabDelivery::TypeInTab(1));
        // Aba 30 fechou enquanto transcrevia (mesmo com 10 e 20 abertas): descarta.
        assert_eq!(resolve_tab_delivery(&tabs, 30), TabDelivery::TabClosed);
        // Aba 20 encerrou o processo: não digita em 10.
        let tabs_dead = [(10, true), (20, false)];
        assert_eq!(resolve_tab_delivery(&tabs_dead, 20), TabDelivery::TabDead);
    }

    #[test]
    fn button_visuals_reflect_all_four_states_and_lock_out_other_buttons() {
        let tab1 = MicTarget::Tab(1);
        let tab2 = MicTarget::Tab(2);
        let comp = MicTarget::Composer;

        assert_eq!(button_visual(MicPhase::Idle, tab1), ButtonVisual::Idle);
        assert_eq!(button_visual(MicPhase::Opening(tab1), tab1), ButtonVisual::Opening);
        assert_eq!(button_visual(MicPhase::Opening(tab1), tab2), ButtonVisual::DisabledOther);
        assert_eq!(button_visual(MicPhase::Recording(tab1), tab1), ButtonVisual::Recording);
        assert_eq!(button_visual(MicPhase::Recording(tab1), comp), ButtonVisual::DisabledOther);
        assert_eq!(button_visual(MicPhase::Transcribing(tab1), tab1), ButtonVisual::Transcribing);
        assert!(button_visual(MicPhase::Transcribing(tab1), tab1).is_busy());
        assert!(button_visual(MicPhase::Recording(tab1), tab2).is_busy());
        assert!(!button_visual(MicPhase::Recording(tab1), tab1).is_busy());
    }

    #[test]
    fn tooltips_cover_every_state() {
        assert!(mic_tooltip(ButtonVisual::Idle, true, "GPU CUDA", Some("Ctrl+Shift+M")).contains("Ctrl+Shift+M"));
        assert!(mic_tooltip(ButtonVisual::Idle, false, "", None).contains("indisponível"));
        assert!(mic_tooltip(ButtonVisual::Opening, true, "", Some("Ctrl+Shift+M")).contains("Abrindo"));
        assert!(mic_tooltip(ButtonVisual::Recording, true, "", Some("Ctrl+Shift+M")).contains("Parar gravação"));
        assert!(mic_tooltip(ButtonVisual::Transcribing, true, "", None).contains("Transcrevendo"));
        assert!(mic_tooltip(ButtonVisual::DisabledOther, true, "", None).contains("em uso"));
    }
}
