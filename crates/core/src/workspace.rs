//! Pasta de trabalho: listagem para a árvore, AGENTS.md com instruções da ponte e watcher.

use std::path::{Component, Path};
use std::sync::Weak;
use std::time::Duration;

use notify::{EventKind, RecursiveMode, Watcher};

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

// ---------- AGENTS.md ----------

pub const MARKER_START: &str = "<!-- commanddeck:start -->";
pub const MARKER_END: &str = "<!-- commanddeck:end -->";

fn marker_re() -> &'static regex::Regex {
    static RE: std::sync::OnceLock<regex::Regex> = std::sync::OnceLock::new();
    RE.get_or_init(|| {
        regex::Regex::new(r"<!-- (?:orquestra|jwic|commanddeck):start -->[\s\S]*?<!-- (?:orquestra|jwic|commanddeck):end -->")
            .expect("regex válida")
    })
}

/// Resultado de [`ensure_agent_context`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AgentContext {
    /// Workspace é a pasta pessoal do usuário: nunca escrevemos lá.
    SkippedHome,
    /// Fora da pasta do app e sem AGENTS.md pré-existente.
    SkippedForeign,
    /// Conteúdo já estava atualizado.
    Unchanged,
    Written,
}

/// Mantém o bloco marcado do CommandDeck no AGENTS.md do workspace (o Antigravity CLI não
/// tem flag para anexar instruções ao prompt de sistema). Equivalente a `ensureAgentContext`.
///
/// REGRA: nunca cria AGENTS.md na pasta do usuário; fora da pasta do app só atualiza um
/// AGENTS.md que já exista.
pub fn ensure_agent_context(workspace: &Path, app_dir: &Path, home: &Path, instructions: &str) -> std::io::Result<AgentContext> {
    let ws = crate::util::absolutize(workspace);
    if !home.as_os_str().is_empty() && crate::util::same_path(&ws, &crate::util::absolutize(home)) {
        return Ok(AgentContext::SkippedHome);
    }
    let file = workspace.join("AGENTS.md");
    let is_app_workspace = crate::util::path_within(&ws, &crate::util::absolutize(app_dir));
    if !is_app_workspace && !file.exists() {
        return Ok(AgentContext::SkippedForeign);
    }
    let block = format!("{MARKER_START}\n{instructions}\n{MARKER_END}");
    let old = if file.exists() { std::fs::read_to_string(&file)? } else { String::new() };
    let new = if marker_re().is_match(&old) {
        marker_re().replace(&old, regex::NoExpand(&block)).into_owned()
    } else if old.is_empty() {
        format!("{block}\n")
    } else {
        format!("{}\n\n{block}\n", old.trim_end())
    };
    if new == old {
        return Ok(AgentContext::Unchanged);
    }
    std::fs::write(&file, new)?;
    Ok(AgentContext::Written)
}

// ---------- Watcher ----------

/// Mudanças nestas pastas não recarregam a árvore.
const WATCH_IGNORED: &[&str] = &[".git", "node_modules", ".gemini", "dados", "target"];

/// `true` se a mudança em `path` deve ser ignorada.
pub(crate) fn watch_ignored(workspace: &Path, path: &Path) -> bool {
    let rel = path.strip_prefix(workspace).unwrap_or(path);
    rel.components().any(|c| match c {
        Component::Normal(s) => WATCH_IGNORED.iter().any(|i| s == *i),
        _ => false,
    })
}

/// Inicia um watcher recursivo com debounce de 300 ms que emite
/// [`crate::CoreEvent::WorkspaceChanged`]. O watcher para quando o valor retornado é solto.
pub(crate) fn start_watcher(workspace: &Path, core: Weak<crate::CoreInner>) -> notify::Result<notify::RecommendedWatcher> {
    let (tx, rx) = crossbeam_channel::unbounded::<()>();
    let ws = workspace.to_path_buf();
    let mut watcher = notify::recommended_watcher(move |res: notify::Result<notify::Event>| {
        let Ok(ev) = res else { return };
        // Leituras (Access) não alteram a árvore — evita laço com a própria listagem.
        if matches!(ev.kind, EventKind::Access(_)) {
            return;
        }
        if !ev.paths.is_empty() && ev.paths.iter().all(|p| watch_ignored(&ws, p)) {
            return;
        }
        let _ = tx.send(());
    })?;
    watcher.watch(workspace, RecursiveMode::Recursive)?;
    std::thread::Builder::new()
        .name("cd-watch-debounce".into())
        .spawn(move || {
            // Termina quando o watcher (dono do `tx`) é solto.
            while rx.recv().is_ok() {
                loop {
                    match rx.recv_timeout(Duration::from_millis(300)) {
                        Ok(()) => continue,
                        Err(crossbeam_channel::RecvTimeoutError::Timeout) => break,
                        Err(crossbeam_channel::RecvTimeoutError::Disconnected) => return,
                    }
                }
                match core.upgrade() {
                    Some(inner) => crate::Core { inner }.emit(crate::CoreEvent::WorkspaceChanged),
                    None => return,
                }
            }
        })
        .map_err(|e| notify::Error::generic(&e.to_string()))?;
    Ok(watcher)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn read_dir_orders_and_filters() {
        let tmp = tempfile::tempdir().expect("tmp");
        let ws = tmp.path();
        for d in ["zeta", "Alpha", "node_modules", ".git", "dados", "target"] {
            std::fs::create_dir_all(ws.join(d)).expect("mkdir");
        }
        for f in ["b.TXT", "a.rs", "C.md"] {
            std::fs::write(ws.join(f), "x").expect("write");
        }
        std::fs::write(ws.join("zeta").join("in.json"), "{}").expect("write");
        let items = read_dir(ws, "");
        let names: Vec<&str> = items.iter().map(|i| i.name.as_str()).collect();
        assert_eq!(names, ["Alpha", "zeta", "a.rs", "b.TXT", "C.md"]);
        assert_eq!(items[3].ext, ".txt");
        assert!(items[0].is_dir && items[0].ext.is_empty());
        let sub = read_dir(ws, "zeta");
        assert_eq!(sub[0].rel_path, "zeta/in.json");
        assert!(read_dir(ws, "nao-existe").is_empty());
    }

    #[test]
    fn agents_md_rules() {
        let tmp = tempfile::tempdir().expect("tmp");
        let app = tmp.path().join("app");
        let home = tmp.path().join("home");
        let ws_app = app.join("workspace");
        let foreign = tmp.path().join("outro");
        for d in [&ws_app, &home, &foreign] {
            std::fs::create_dir_all(d).expect("mkdir");
        }

        // Pasta pessoal: nunca.
        assert_eq!(ensure_agent_context(&home, &app, &home, "X").expect("ok"), AgentContext::SkippedHome);
        assert!(!home.join("AGENTS.md").exists());
        // Fora do app, sem AGENTS.md: não cria.
        assert_eq!(ensure_agent_context(&foreign, &app, &home, "X").expect("ok"), AgentContext::SkippedForeign);
        assert!(!foreign.join("AGENTS.md").exists());
        // Dentro do app: cria.
        assert_eq!(ensure_agent_context(&ws_app, &app, &home, "X").expect("ok"), AgentContext::Written);
        let text = std::fs::read_to_string(ws_app.join("AGENTS.md")).expect("read");
        assert_eq!(text, format!("{MARKER_START}\nX\n{MARKER_END}\n"));
        assert_eq!(ensure_agent_context(&ws_app, &app, &home, "X").expect("ok"), AgentContext::Unchanged);

        // Fora do app, AGENTS.md existente com bloco legado: substitui só o bloco.
        std::fs::write(
            foreign.join("AGENTS.md"),
            "# Regras\n\n<!-- jwic:start -->\nvelho $1\n<!-- jwic:end -->\n\nfim\n",
        )
        .expect("write");
        assert_eq!(ensure_agent_context(&foreign, &app, &home, "novo $1").expect("ok"), AgentContext::Written);
        let text = std::fs::read_to_string(foreign.join("AGENTS.md")).expect("read");
        assert_eq!(text, format!("# Regras\n\n{MARKER_START}\nnovo $1\n{MARKER_END}\n\nfim\n"));

        // AGENTS.md existente sem bloco: acrescenta ao final.
        std::fs::write(foreign.join("AGENTS.md"), "# Regras\n\n\n").expect("write");
        ensure_agent_context(&foreign, &app, &home, "Y").expect("ok");
        let text = std::fs::read_to_string(foreign.join("AGENTS.md")).expect("read");
        assert_eq!(text, format!("# Regras\n\n{MARKER_START}\nY\n{MARKER_END}\n"));

        // Pasta "irmã" com prefixo de string igual não conta como dentro do app.
        let sibling = tmp.path().join("app-x");
        std::fs::create_dir_all(&sibling).expect("mkdir");
        assert_eq!(ensure_agent_context(&sibling, &app, &home, "X").expect("ok"), AgentContext::SkippedForeign);
    }

    #[test]
    fn watcher_filter() {
        let ws = Path::new("/w");
        assert!(watch_ignored(ws, Path::new("/w/.git/index")));
        assert!(watch_ignored(ws, Path::new("/w/sub/node_modules/x")));
        assert!(watch_ignored(ws, Path::new("/w/target/debug/x")));
        assert!(!watch_ignored(ws, Path::new("/w/src/main.rs")));
        assert!(!watch_ignored(ws, Path::new("/w/.gitignore")));
    }
}
