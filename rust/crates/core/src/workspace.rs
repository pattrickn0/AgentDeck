//! Pasta de trabalho: listagem para a árvore, AGENTS.md com instruções da ponte e watcher.

use std::path::Path;

/// Item da árvore de arquivos (equivalente a `workspace:readDir`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DirEntry {
    pub name: String,
    pub is_dir: bool,
    /// Caminho relativo ao workspace, com `/`.
    pub rel_path: String,
    /// Extensão em minúsculas com ponto (`.rs`), vazia para pastas.
    pub ext: String,
}

/// Nomes ignorados na árvore.
pub const IGNORED: &[&str] = &["node_modules", ".git", ".gemini", "dados", "$RECYCLE.BIN", "target"];

pub fn read_dir(workspace: &Path, rel_dir: &str) -> Vec<DirEntry> {
    let target = workspace.join(rel_dir);
    let Ok(rd) = std::fs::read_dir(&target) else { return Vec::new() };
    let mut items: Vec<DirEntry> = rd
        .flatten()
        .filter_map(|e| {
            let name = e.file_name().to_string_lossy().into_owned();
            if IGNORED.contains(&name.as_str()) {
                return None;
            }
            let is_dir = e.file_type().map(|t| t.is_dir()).unwrap_or(false);
            let rel_path = if rel_dir.is_empty() { name.clone() } else { format!("{}/{}", rel_dir.trim_end_matches('/'), name) };
            let ext = if is_dir {
                String::new()
            } else {
                Path::new(&name).extension().map(|x| format!(".{}", x.to_string_lossy().to_lowercase())).unwrap_or_default()
            };
            Some(DirEntry { name, is_dir, rel_path, ext })
        })
        .collect();
    items.sort_by(|a, b| b.is_dir.cmp(&a.is_dir).then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase())));
    items
}
