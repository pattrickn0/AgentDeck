//! pt-BR translations for the area: Settings menu (☰), new-agent menu, tab context menu, rename and dictation dialogs,
//! and the title bar (`ui/menus.rs`, `ui/titlebar.rs`).
//!
//! Entries are `(English, Portuguese)`; the English text is the key used in `t!("...")`. See the `i18n` module docs for
//! the conventions and the coverage test.

pub const ENTRIES: &[(&str, &str)] = &[
    // Title bar
    ("Menu", "Menu"),
    ("Expand sidebar (Ctrl+B)", "Expandir barra lateral (Ctrl+B)"),
    ("Collapse sidebar (Ctrl+B)", "Recolher barra lateral (Ctrl+B)"),
    ("Minimize", "Minimizar"),
    ("Maximize", "Maximizar"),
    ("Restore", "Restaurar"),
    // New-agent menu and model steps
    ("Loading models…", "Carregando modelos…"),
    ("No model list: default only", "Sem lista de modelos: só o padrão"),
    ("Choose the model for {agent}", "Escolher o modelo de {agent}"),
    ("free", "grátis"),
    ("1 model", "1 modelo"),
    ("{count} models", "{count} modelos"),
    ("{agent} · model", "{agent} · modelo"),
    ("{agent} · provider", "{agent} · provedor"),
    // Settings menu
    ("SETTINGS", "CONFIGURAÇÕES"),
    ("Notification sounds", "Sons de notificação"),
    ("Open agent tabs", "Abrir abas dos agentes"),
    ("App shortcuts above the terminal", "Atalhos do app acima do terminal"),
    (
        "On: Ctrl+B belongs to the app even when the terminal is focused. Off: it goes to the terminal (tmux prefix).",
        "Ligado: Ctrl+B é do app mesmo com o terminal focado. Desligado: vai para o terminal (prefixo do tmux).",
    ),
    ("Language", "Idioma"),
    ("▶ Test sound", "▶ Testar som"),
    ("Voice dictation", "Ditado por voz"),
    ("installed", "instalado"),
    ("downloading…", "baixando…"),
    ("not installed", "não instalado"),
    ("Progress is shown in the right bar.", "O progresso está na barra direita."),
    ("Download ({size})", "Baixar ({size})"),
    (
        "Downloads Whisper for voice dictation; the audio stays on this computer.",
        "Baixa o Whisper para ditar por voz; o áudio fica no computador.",
    ),
    ("Confirm: delete from disk", "Confirmar: apagar do disco"),
    (
        "Deletes the model and the CUDA engine. You can download them again later.",
        "Apaga o modelo e o motor CUDA. Dá para baixar de novo depois.",
    ),
    ("Remove from disk", "Remover do disco"),
    (
        "Deletes Whisper ({size}) to free up space; you can download it again.",
        "Apaga o Whisper ({size}) para liberar espaço; dá para baixar de novo.",
    ),
    // Tab context menu
    ("Stop recording the terminal", "Parar de gravar o terminal"),
    ("Record the terminal (diagnostics)", "Gravar o terminal (diagnóstico)"),
    (
        "Keeps what this tab's program writes to the terminal, to track down display bugs. Uses disk space while on.",
        "Guarda o que o programa desta aba escreve no terminal, para achar defeitos de tela. Ocupa disco enquanto ligado.",
    ),
    ("Save a snapshot of the grid now", "Salvar foto da grade agora"),
    (
        "Saves the text the terminal holds now (screen and history) and opens the folder. Use it when the screen goes wrong.",
        "Salva o texto que o terminal tem agora (tela e histórico) e abre a pasta. Use quando a tela ficar errada.",
    ),
    ("Model: {model}", "Modelo: {model}"),
    // Rename dialog
    ("Rename Agent", "Renomear Agente"),
    (
        "Format: model@role@name (e.g. claude@dev or antigravity@lead)",
        "Formato: modelo@cargo@nome (ex: claude@dev ou antigravity@lead)",
    ),
    // Dictation dialog
    ("Voice dictation (optional)", "Ditado por voz (opcional)"),
    (
        "Speak instead of typing: AgentDeck transcribes your voice on this computer, without sending the audio over the internet.",
        "Fale em vez de digitar: o AgentDeck transcreve a sua voz aqui no computador, sem enviar o áudio para a internet.",
    ),
    (
        "To do that it downloads Whisper (transcription model and engine): {size}, plus about 700 MB of the CUDA engine if there is an NVIDIA card. The download runs in the background and progress appears in the right bar.",
        "Para isso ele baixa o Whisper (modelo e motor de transcrição): {size}, e mais uns 700 MB do motor CUDA se houver uma placa NVIDIA. O download roda em segundo plano e o progresso aparece na barra direita.",
    ),
    ("You can download or remove dictation later, in Settings.", "Você pode baixar ou remover o ditado depois, em Configurações."),
    ("Not now", "Agora não"),
];
