//! pt-BR translations for the area: Documents mode (docs, docs_demo, ui::docs).
//!
//! Entries are `(English, Portuguese)`; the English text is the key used in `t!("...")`. See `super::super` (the `i18n`
//! module docs) for the conventions and the coverage test.

pub const ENTRIES: &[(&str, &str)] = &[
    ("Text", "Texto"),
    ("The file is empty.", "O arquivo está vazio."),
    ("Workspace is not configured.", "Workspace não configurado."),
    (
        "Access denied: the path \"{path}\" is outside the configured workspace (\"{root}\").",
        "Acesso negado: o caminho \"{path}\" está fora do workspace configurado (\"{root}\").",
    ),
    ("no project open", "nenhum projeto aberto"),
    ("invalid path: {path}", "caminho inválido: {path}"),
    ("the project folder does not open: {error}", "a pasta do projeto não abre: {error}"),
    ("could not open {path}: {error}", "não consegui abrir {path}: {error}"),
    ("{path} is outside the project folder", "{path} está fora da pasta do projeto"),
    ("could not read {path}: {error}", "não consegui ler {path}: {error}"),
    ("{path} is not a file", "{path} não é um arquivo"),
    ("the folder of {path} does not open: {error}", "a pasta de {path} não abre: {error}"),
    ("could not write {path}: {error}", "não consegui gravar {path}: {error}"),
];
