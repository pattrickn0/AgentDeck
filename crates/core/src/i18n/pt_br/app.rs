//! pt-BR translations for the area: App shell: sidebar, right bar, terminal, dialogs, tray and everything under crates/app not owned by another area.
//!
//! Entries are `(English, Portuguese)`; the English text is the key used in `t!("...")`. See `super::super` (the `i18n`
//! module docs) for the conventions and the coverage test.

pub const ENTRIES: &[(&str, &str)] = &[
    // Agent tabs and the agent menu
    ("{agent} (not installed)", "{agent} (não instalado)"),
    ("Open {agent}", "Abrir {agent}"),
    // Startup
    ("AgentDeck could not start: {error}", "Não foi possível iniciar o AgentDeck: {error}"),
    // Attachments and drag and drop
    ("Drop to attach to the new task", "Solte para anexar à nova tarefa"),
    ("Copy under data/attachments cited in the request", "Cópia em data/attachments citada no pedido"),
    ("Drop to attach to {tab}", "Solte para anexar a {tab}"),
    ("The path will be inserted into this tab's prompt", "O caminho será inserido no prompt desta aba"),
    ("Open an agent or the new task form to attach files.", "Abra um agente ou o formulário de nova tarefa para anexar arquivos."),
    ("The active tab has no running process.", "A aba ativa não tem processo rodando."),
    // Voice dictation
    ("Finish the dictation recording before removing Whisper.", "Termine a gravação do ditado antes de remover o Whisper."),
    (
        "Voice dictation removed from disk. You can download it again from the settings.",
        "Ditado por voz removido do disco. Você pode baixá-lo de novo nas configurações.",
    ),
    ("Could not remove voice dictation: {error}.", "Não foi possível remover o ditado por voz: {error}."),
    (
        "Voice dictation is unavailable: AgentDeck was built without the audio feature.",
        "O ditado por voz não está disponível: o AgentDeck foi compilado sem o recurso de áudio.",
    ),
    (
        "Voice dictation is still downloading (see the right bar). Try again when it finishes.",
        "O ditado por voz ainda está baixando (veja a barra direita). Tente de novo quando terminar.",
    ),
    ("Microphone unavailable: {error}", "Microfone indisponível: {error}"),
    ("no audio captured", "nenhum áudio capturado"),
    // Attachment and dictation delivery
    ("The tab's process ended before receiving the attachment.", "O processo da aba terminou antes de receber o anexo."),
    ("The attachment's tab was closed before the copy finished.", "A aba do anexo foi fechada antes de a cópia terminar."),
    ("Could not transcribe: {error}.", "Não foi possível transcrever: {error}."),
    (
        "The tab's process ended during the transcription: the text was discarded.",
        "O processo da aba terminou durante a transcrição: o texto foi descartado.",
    ),
    (
        "The dictation's tab was closed before the transcription finished: the text was discarded.",
        "A aba do ditado foi fechada antes de a transcrição terminar: o texto foi descartado.",
    ),
    (
        "The recording reached the {minutes}-minute limit: I stopped and am transcribing what was recorded.",
        "A gravação atingiu o limite de {minutes} minutos: parei e estou transcrevendo o que foi gravado.",
    ),
    ("process ended (code {code}) — press Enter to restart", "processo terminou (código {code}) — pressione Enter para reiniciar"),
    // Projects and team
    ("Could not change the team: {error}.", "Não foi possível mudar a equipe: {error}."),
    ("\n\nOne of its documents has unsaved edits, which will be lost.", "\n\nUm dos documentos dele tem edições não salvas, que serão perdidas."),
    (
        "\n\n{count} of its documents have unsaved edits, which will be lost.",
        "\n\n{count} dos documentos dele têm edições não salvas, que serão perdidas.",
    ),
    ("Remove project", "Remover projeto"),
    ("Do you want to remove \"{name}\" from the project list?{lost}", "Deseja remover \"{name}\" da lista de projetos?{lost}"),
    ("Could not open the project: {error}", "Não foi possível abrir o projeto: {error}"),
    // Tabs
    ("Open a folder as a project before opening an agent.", "Abra uma pasta como projeto antes de abrir um agente."),
    ("Could not open {kind}: {error}.", "Não foi possível abrir {kind}: {error}."),
    ("Opened the {label} tab for task {id}: {title}.", "Abri a aba de {label} para a tarefa {id}: {title}."),
    ("Opened the {label} tab for the conversation with another agent.", "Abri a aba de {label} para a conversa com outro agente."),
    ("Open a folder as a project before sending to an agent.", "Abra uma pasta como projeto antes de enviar para um agente."),
    ("The agent \"{agent}\" is not available.", "O agente \"{agent}\" não está disponível."),
    ("\"{name}\" is ambiguous: {options}. Pick one.", "\"{name}\" é ambíguo: {options}. Escolha uma delas."),
    ("Task {id} is {status}: no agent is running.", "A tarefa {id} está {status}: não há agente rodando."),
    ("The agent \"{agent}\" for this task is not available.", "O agente \"{agent}\" desta tarefa não está disponível."),
    ("press Enter to try again", "pressione Enter para tentar de novo"),
    ("Terminal recording for {id} turned off. The files are in {dir}.", "Gravação do terminal de {id} desligada. Os arquivos ficam em {dir}."),
    (
        "Recording the terminal of {id} from now on to {dir} (three files per recording: .bin, .events.txt and .grid.txt). When the defect shows up, use \"Save grid snapshot now\" in the tab menu (right click) and send those files.",
        "Gravando o terminal de {id} daqui em diante em {dir} (três arquivos por gravação: .bin, .events.txt e .grid.txt). Quando o defeito aparecer, use \"Salvar foto da grade agora\" no menu da aba (botão direito) e mande esses arquivos.",
    ),
    ("Terminal grid snapshot for {id} saved to {path}.", "Foto da grade do terminal de {id} salva em {path}."),
    ("Could not save the terminal grid snapshot for {id}: {error}", "Não consegui salvar a foto da grade do terminal de {id}: {error}"),
    ("tab not found", "aba não encontrada"),
    (
        "invalid format: \"{text}\". Use model@role@name (e.g. {kind}@dev or {kind}@dev@ana)",
        "formato inválido: \"{text}\". Use modelo@cargo@nome (ex.: {kind}@dev ou {kind}@dev@ana)",
    ),
    ("the model cannot be changed from \"{kind}\" to \"{other}\"", "o modelo não pode ser alterado de \"{kind}\" para \"{other}\""),
];
