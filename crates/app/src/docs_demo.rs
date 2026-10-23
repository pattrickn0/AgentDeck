//! SOMENTE DEPURAÇÃO: documentos fictícios do modo Documentos (`AGENTDECK_DEBUG_UI=docs-demo…`), para
//! capturas de tela sem git, sem agentes e sem arquivos de verdade.
//!
//! - `docs-demo`: o diff de uma tarefa com quatro arquivos, em foco;
//! - `docs-demo-file`, `docs-demo-md`, `docs-demo-report`, `docs-demo-unreadable`: o documento em foco;
//! - `docs-demo-edit` (editando, com alteração não salva), `docs-demo-find` (busca aberta no arquivo), `docs-demo-diff-find`
//!   (busca no diff), `docs-demo-external` (o disco mudou com edição não salva), `docs-demo-confirm` (a pergunta ao fechar a
//!   aba), `docs-demo-large` (arquivo grande, só leitura), `docs-demo-md-code` (markdown como código);
//! - `docs-demo-many`: dez documentos (a faixa de abas rola);
//! - `docs-demo-badge`: os agentes à vista, com o selo da chave (duas abas terminaram, uma trabalha).

use agentdeck_core::git::parse_diff;

use crate::deck::Deck;
use crate::docs::{Badge, CenterView, DiffDoc, DocKind, DocState, MdSource, Source, TextDoc, Unsaved};
use crate::highlight::Lang;

const DIFF: &str = "diff --git a/crates/app/src/docs.rs b/crates/app/src/docs.rs
new file mode 100644
index 0000000..a1b2c3d
--- /dev/null
+++ b/crates/app/src/docs.rs
@@ -0,0 +1,9 @@
+//! Modo Documentos do centro: o estado e a lógica pura.
+
+use std::collections::HashMap;
+
+/// O que o centro mostra.
+pub enum CenterView {
+    Agents,
+    Docs,
+}
diff --git a/crates/app/src/ui/tasks.rs b/crates/app/src/ui/tasks.rs
index 4d5e6f7..8a9b0c1 100644
--- a/crates/app/src/ui/tasks.rs
+++ b/crates/app/src/ui/tasks.rs
@@ -62,7 +62,7 @@ pub fn draw(deck: &mut Deck, ui: &mut Ui, rect: Rect) {
     match deck.tasks.view.clone() {
         TasksView::List => list_view(deck, ui, rect, now, &mut acts),
         TasksView::Detail(id) => match deck.tasks.task(&id).cloned() {
-            Some(task) if deck.tasks.diff.as_ref().is_some_and(|d| d.id == id) => diff_screen(deck, ui, rect, &task, &mut acts),
             Some(task) => detail_view(deck, ui, rect, &task, now, &mut acts),
             None => list_view(deck, ui, rect, now, &mut acts),
         },
@@ -98,7 +97,7 @@ fn apply(deck: &mut Deck, act: Act) {
         Act::Task(id, act) => deck.tasks_act(&id, act),
-        Act::OpenDiff(id) => deck.tasks_open_diff(&id),
+        Act::OpenDiff(id) => deck.docs_open_task_diff(&id),
         Act::DismissNotice => deck.tasks.notice = None,
         Act::ConfirmYes => deck.tasks_confirm_accepted(),
         Act::ConfirmNo => deck.tasks.confirm = None,
diff --git a/crates/app/src/tasks_ui.rs b/crates/app/src/tasks_ui.rs
index 1111111..2222222 100644
--- a/crates/app/src/tasks_ui.rs
+++ b/crates/app/src/tasks_ui.rs
@@ -410,12 +410,6 @@ pub enum Confirm {
     Discard(String),
 }

-/// Diff completo aberto no lugar do detalhe.
-#[derive(Debug, Clone)]
-pub struct OpenDiff {
-    pub id: String,
-    pub diff: Option<Result<GitDiff, String>>,
-}
-
 /// Uma operação do ciclo de vida em andamento.
 #[derive(Debug, Clone, PartialEq, Eq)]
 pub struct Busy {
diff --git a/memory/pendencias.md b/memory/pendencias.md
index 9f8e7d6..5c4b3a2 100644
--- a/memory/pendencias.md
+++ b/memory/pendencias.md
@@ -12,3 +12,4 @@
 - Mesa de tarefas: revisão sem becos sem saída.
-- Pedido e relatório aparecem como texto puro.
+- Editor de arquivos: fase 2 do modo Documentos.
+- Diff lado a lado.
 - Terminal: histórico duplicado no Claude Code.
";

const FILE: &str = "//! Lê o arquivo de texto de um projeto, só para mostrar na tela.

use std::path::Path;

/// O limite do que se lê: acima disso o arquivo não vira texto.
pub const READ_TEXT_MAX: u64 = 1024 * 1024;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TextFile {
    Text(String),
    /// Maior que o limite (tamanho em bytes).
    TooLarge(u64),
    /// Tem bytes nulos: não é texto.
    Binary,
}

pub fn read_text(workspace: &Path, rel: &str) -> Result<TextFile, String> {
    if !crate::git::safe_rel(rel) {
        return Err(format!(\"caminho inválido: {rel}\"));
    }
    let full = std::fs::canonicalize(workspace.join(rel)).map_err(|e| format!(\"não consegui abrir {rel}: {e}\"))?;
    let meta = std::fs::metadata(&full).map_err(|e| format!(\"não consegui ler {rel}: {e}\"))?;
    if meta.len() > READ_TEXT_MAX {
        return Ok(TextFile::TooLarge(meta.len()));
    }
    let bytes = std::fs::read(&full).map_err(|e| format!(\"não consegui ler {rel}: {e}\"))?;
    if bytes.contains(&0) {
        return Ok(TextFile::Binary);
    }
    Ok(TextFile::Text(String::from_utf8_lossy(&bytes).into_owned()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_path_outside_the_workspace_is_refused() {
        assert!(read_text(Path::new(\".\"), \"../segredo.txt\").is_err());
    }
}
";

const REPORT: &str = "# Relatório da t19

Documentos no centro: uma chave **Agentes | Documentos** para ler diff, relatório e arquivos com a largura toda.

## O que foi feito

- a chave na barra de título, com o selo em *Agentes*;
- o diff da tarefa, de um commit e de um arquivo, com a lista de arquivos à esquerda;
- a visualização de arquivo (`Core::read_text`, limite de 1 MB);
- o markdown formatado (pedido, relatório e os `.md` do explorador).

## Conferido

- [x] `cargo fmt --all`
- [x] `cargo clippy --workspace --all-targets -- -D warnings`
- [ ] teste manual no Windows com os agentes reais

## Arquivos

| Arquivo | O que mudou |
| :-- | :-- |
| `crates/app/src/docs.rs` | estado e lógica pura |
| `crates/app/src/ui/docs.rs` | a pintura |
| `crates/core/src/workspace.rs` | `read_text` |

> Os terminais continuam rodando escondidos; o PTY **não** é redimensionado.

```rust
pub fn toggle(&mut self, project: &str) {
    self.view = match self.view {
        CenterView::Docs => CenterView::Agents,
        CenterView::Agents if self.count(project) > 0 => CenterView::Docs,
        CenterView::Agents => CenterView::Agents,
    };
}
```

1. Abra um diff pela mesa.
2. Clique num arquivo da lista.
3. Volte aos agentes com **Ctrl+Shift+D**.
";

const README: &str = "# Guia de manutenção

Este guia é para quem vai manter o AgentDeck, sozinho ou junto com agentes de IA.

## Comandos do dia a dia

```bash
cargo run -p agentdeck
cargo test --workspace
```

Veja também o [GERAL.md](GERAL.md).
";

/// Aplica o estado de captura `item` (um dos `docs-demo*`).
pub fn apply(deck: &mut Deck, item: &str) {
    deck.docs.debug_hold = true;
    let project = deck.active_project.clone();
    let open = |deck: &mut Deck, kind: DocKind, title: &str, subtitle: &str, state: DocState| {
        deck.docs.open(&project, kind, title.to_owned(), subtitle.to_owned(), state);
    };
    open(
        deck,
        DocKind::Markdown(MdSource::TaskReport("t19".into())),
        "t19 · relatório",
        "t19 · relatório: Documentos no centro: chave Agentes | Documentos",
        DocState::Markdown(REPORT.into()),
    );
    open(
        deck,
        DocKind::Markdown(MdSource::File("memory/MANUTENCAO.md".into())),
        "MANUTENCAO.md",
        "memory/MANUTENCAO.md",
        DocState::Source(Source::open(README, Lang::Markdown).expect("cabe no editor")),
    );
    let file_state = if item == "docs-demo-large" {
        DocState::Text(TextDoc::highlighted(FILE, Lang::Rust))
    } else {
        DocState::Source(Source::open(FILE, Lang::Rust).expect("cabe no editor"))
    };
    open(deck, DocKind::File("crates/core/src/workspace.rs".into()), "workspace.rs", "crates/core/src/workspace.rs", file_state);
    open(
        deck,
        DocKind::TaskDiff { id: "t19".into(), merged: false },
        "t19 · diff",
        "t19 · alterações em relação a dev",
        DocState::Diff(DiffDoc::new(parse_diff(DIFF, 5000))),
    );
    if item == "docs-demo-many" {
        for n in 1..=9 {
            let path = format!("crates/app/src/ui/arquivo_{n}.rs");
            let state = DocState::Source(Source::open(FILE, Lang::Rust).expect("cabe no editor"));
            open(deck, DocKind::File(path.clone()), &format!("arquivo_{n}.rs"), &path, state);
        }
    }
    if item == "docs-demo-unreadable" {
        open(
            deck,
            DocKind::File("whisper/models/ggml-base.bin".into()),
            "ggml-base.bin",
            "whisper/models/ggml-base.bin",
            DocState::Unreadable("O arquivo tem 141,1 MB e passa do limite de 1,0 MB. Abra no programa padrão.".into()),
        );
    }
    // O foco: cada variante olha para um documento.
    let focus = match item {
        "docs-demo-file" | "docs-demo-many" | "docs-demo-edit" | "docs-demo-find" | "docs-demo-external" | "docs-demo-confirm"
        | "docs-demo-large" => DocKind::File("crates/core/src/workspace.rs".into()),
        "docs-demo-md" | "docs-demo-md-code" => DocKind::Markdown(MdSource::File("memory/MANUTENCAO.md".into())),
        "docs-demo-report" => DocKind::Markdown(MdSource::TaskReport("t19".into())),
        "docs-demo-unreadable" => DocKind::File("whisper/models/ggml-base.bin".into()),
        _ => DocKind::TaskDiff { id: "t19".into(), merged: false },
    };
    open(deck, focus, "", "", DocState::Loading);
    if item == "docs-demo-badge" {
        deck.docs.view = CenterView::Agents;
        deck.docs.debug_badge = Some(Badge { done: 2, busy: 1 });
    }
    let uid = deck.docs.current_uid(&project);
    let source = |deck: &mut Deck, f: &mut dyn FnMut(&mut Source)| {
        if let Some(DocState::Source(src)) = uid.and_then(|u| deck.docs.get_mut(u)).map(|t| &mut t.state) {
            f(src);
        }
    };
    match item {
        "docs-demo-edit" | "docs-demo-confirm" | "docs-demo-external" => source(deck, &mut |src| {
            src.set_editing(true);
            src.text = src.text.replacen("Ok(TextFile::Text(", "Ok(TextFile::Utf8(", 1) + "\n// anotação nova, ainda sem gravar\n";
            src.touched();
            if item == "docs-demo-external" {
                src.external = Some(src.disk.replacen("READ_TEXT_MAX", "LIMITE_DE_LEITURA", 1));
            }
        }),
        "docs-demo-find" | "docs-demo-diff-find" => {
            if let Some(tab) = uid.and_then(|u| deck.docs.get_mut(u)) {
                tab.find.show();
                tab.find.query = if item == "docs-demo-find" { "read_text".into() } else { "doc".into() };
                tab.find.current = 1;
            }
        }
        "docs-demo-md-code" => source(deck, &mut |src| src.raw = true),
        _ => {}
    }
    if item == "docs-demo-confirm" {
        deck.docs.confirm = uid.map(Unsaved::Tab);
    }
}
