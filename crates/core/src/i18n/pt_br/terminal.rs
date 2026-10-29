//! pt-BR translations for the area: Terminal, theme, icons, split layout, audio, crash dialog, dictation, tray and window (crates/app/src).
//!
//! Entries are `(English, Portuguese)`; the English text is the key used in `t!("...")`. See the `i18n` module docs for
//! the conventions and the coverage test.

pub const ENTRIES: &[(&str, &str)] = &[
    // Tray icon (tray.rs)
    ("Open", "Abrir"),
    ("Quit", "Encerrar"),
    ("1 task waiting for your review", "1 tarefa aguardando a sua revisão"),
    ("{count} tasks waiting for your review", "{count} tarefas aguardando a sua revisão"),
    ("1 task awaiting your validation", "1 tarefa aguardando a sua validação"),
    ("{count} tasks awaiting your validation", "{count} tarefas aguardando a sua validação"),
    // Voice dictation (dictation.rs)
    ("about 1.6 GB", "cerca de 1,6 GB"),
    ("Stop recording and transcribe ({shortcut}) (up to {minutes} min)", "Parar gravação e transcrever ({shortcut}) (até {minutes} min)"),
    ("Stop recording and transcribe (up to {minutes} min)", "Parar gravação e transcrever (até {minutes} min)"),
    ("Opening microphone… ({shortcut}) (click to cancel)", "Abrindo o microfone… ({shortcut}) (clique para desistir)"),
    ("Opening microphone… (click to cancel)", "Abrindo o microfone… (clique para desistir)"),
    ("Transcribing audio…", "Transcrevendo áudio…"),
    ("Microphone in use in another tab", "Microfone em uso em outra aba"),
    ("Voice dictation (unavailable: built without audio feature)", "Ditado por voz (indisponível: compilado sem o recurso de áudio)"),
    (
        "Voice dictation not installed: click to download ({size}) ({shortcut})",
        "Ditado por voz não instalado: clique para baixar ({size}) ({shortcut})",
    ),
    ("Voice dictation not installed: click to download ({size})", "Ditado por voz não instalado: clique para baixar ({size})"),
    ("Downloading voice dictation… (see right bar)", "Baixando o ditado por voz… (veja a barra direita)"),
    ("Dictate in this tab ({shortcut} · transcription: {engine})", "Ditar nesta aba ({shortcut} · transcrição: {engine})"),
    ("Dictate in this tab (transcription: {engine})", "Ditar nesta aba (transcrição: {engine})"),
];
