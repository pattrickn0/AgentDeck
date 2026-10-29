//! pt-BR translations for the area: texts produced by crates/core that the user reads (errors and notices shown in the UI) and that are not owned by another area.
//!
//! Entries are `(English, Portuguese)`; the English text is the key used in `t!("...")`. See `super::super` (the `i18n`
//! module docs) for the conventions and the coverage test.

pub const ENTRIES: &[(&str, &str)] = &[
    (
        "Another Holt window is already using the data folder {dir}.\n\nClose it, or set HOLT_HOME to another folder to open a separate instance.",
        "Outra janela do Holt já está usando a pasta de dados {dir}.\n\nFeche-a, ou defina HOLT_HOME com outra pasta para abrir uma instância separada.",
    ),
    ("\"{who}\" is not open in this project", "\"{who}\" não está aberto neste projeto"),
    ("\"{b}\" is already part of the team of \"{id}\": it cannot be its boss", "\"{b}\" já faz parte da equipe de \"{id}\": não pode ser o chefe dele"),
    (
        "The answer #{request} to {asker} was not typed: {why}; it stays in ask-agent --inbox.",
        "A resposta #{request} para {asker} não foi digitada: {why}; ela fica em ask-agent --inbox.",
    ),
    ("unknown project: {id}", "projeto desconhecido: {id}"),
    (
        "Holt created an AGENTS.md with only its own instructions in this folder, because this CLI only reads instructions from there. It shows up as a new file in git: commit it or delete it as you like.",
        "O Holt criou um AGENTS.md só com as instruções dele nesta pasta, porque este CLI só lê instruções de lá. Ele aparece como arquivo novo no git: faça o commit ou apague-o à vontade.",
    ),
    ("the project no longer exists", "o projeto não existe mais"),
    ("no project open", "nenhum projeto aberto"),
    ("failed to open {path}: {error}", "falha ao abrir {path}: {error}"),
    (
        "only empty folders were left: some process has its current directory inside it (a terminal, the agent, the editor)",
        "só pastas vazias ficaram: algum processo está com o diretório atual dentro dela (um terminal, o agente, o editor)",
    ),
    ("{count} file(s) were left: {files}", "{count} arquivo(s) ficaram: {files}"),
    ("{count} file(s) were left: {files}, and {more} more", "{count} arquivo(s) ficaram: {files} e mais {more}"),
    ("; open by: {holders}", "; aberto por: {holders}"),
];
