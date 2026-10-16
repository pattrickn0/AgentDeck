//! Pasta de trabalho: listagem para a árvore, AGENTS.md com instruções da ponte e watcher.

use std::path::{Component, Path, PathBuf};
use std::sync::Weak;
use std::time::{Duration, Instant};

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

/// Nomes ignorados na árvore (`.gemini` é a pasta de configuração do Antigravity, `~/.gemini/antigravity`).
pub const IGNORED: &[&str] = &["node_modules", ".git", ".gemini", "dados", "$RECYCLE.BIN", "target"];

pub fn read_dir(workspace: &Path, rel_dir: &str) -> Vec<DirEntry> {
    // Só subpastas do workspace (`""` = a raiz): nada de `..`, absoluto ou `C:` no Windows.
    if !rel_dir.is_empty() && !crate::git::safe_rel(rel_dir) {
        return Vec::new();
    }
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
    let old = if file.exists() { std::fs::read_to_string(&file)? } else { String::new() };
    // Mantém o fim de linha do arquivo (CRLF em repositórios com `core.autocrlf`): EOL misto
    // sujaria o `git status` a cada abertura.
    let eol = if old.contains("\r\n") { "\r\n" } else { "\n" };
    let instructions = instructions.replace("\r\n", "\n").replace('\n', eol);
    let block = format!("{MARKER_START}{eol}{instructions}{eol}{MARKER_END}");
    let new = if marker_re().is_match(&old) {
        marker_re().replace(&old, regex::NoExpand(&block)).into_owned()
    } else if old.is_empty() {
        format!("{block}{eol}")
    } else {
        format!("{}{eol}{eol}{block}{eol}", old.trim_end())
    };
    if new == old {
        return Ok(AgentContext::Unchanged);
    }
    write_atomic(&file, new.as_bytes())?;
    Ok(AgentContext::Written)
}

/// Grava ao lado e troca: um agente lendo o AGENTS.md ao mesmo tempo nunca o vê pela metade.
/// Se a troca falhar (no Windows, arquivo aberto sem permissão de troca), grava direto.
fn write_atomic(file: &Path, data: &[u8]) -> std::io::Result<()> {
    let name = file.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    let tmp = file.with_file_name(format!(".{name}.{}.tmp", std::process::id()));
    std::fs::write(&tmp, data)?;
    if std::fs::rename(&tmp, file).is_err() {
        let _ = std::fs::remove_file(&tmp);
        std::fs::write(file, data)?;
    }
    Ok(())
}

// ---------- Watcher ----------

/// Mudanças nestas pastas não recarregam a árvore (`.gemini`: configuração do Antigravity).
const WATCH_IGNORED: &[&str] = &[".git", "node_modules", ".gemini", "dados", "target"];

/// `path` relativo a `base`, comparando por componentes (sem diferença de caixa no Windows:
/// `c:\x` e `C:\x` são a mesma pasta). `None` se estiver fora.
fn rel_within(base: &Path, path: &Path) -> Option<PathBuf> {
    crate::util::path_within(path, base).then(|| path.components().skip(base.components().count()).collect())
}

/// `true` se a mudança em `path` deve ser ignorada. Fora do workspace não se ignora: as pastas
/// acima dele (um projeto em `C:\dados\app`, por exemplo) não contam.
pub(crate) fn watch_ignored(workspace: &Path, path: &Path) -> bool {
    let Some(rel) = rel_within(workspace, path) else { return false };
    rel.components().any(|c| match c {
        Component::Normal(s) => WATCH_IGNORED.iter().any(|i| s == *i),
        _ => false,
    })
}

/// A mudança em `path` é no estado do repositório git do workspace (`.git/HEAD`, `index`, refs,
/// merge/rebase/cherry-pick/revert em andamento…)? Os `*.lock` (temporários do git) não contam.
pub(crate) fn git_state_change(workspace: &Path, path: &Path) -> bool {
    let Some(rel) = rel_within(&workspace.join(".git"), path) else { return false };
    let name = rel.to_string_lossy();
    if name.ends_with(".lock") {
        return false;
    }
    let first = rel.components().next().map(|c| c.as_os_str().to_string_lossy().into_owned()).unwrap_or_default();
    matches!(
        first.as_str(),
        "HEAD"
            | "index"
            | "FETCH_HEAD"
            | "MERGE_HEAD"
            | "ORIG_HEAD"
            | "CHERRY_PICK_HEAD"
            | "REVERT_HEAD"
            | "rebase-merge"
            | "rebase-apply"
            | "packed-refs"
            | "refs"
    )
}

/// Silêncio que encerra uma rajada de eventos.
const DEBOUNCE_QUIET: Duration = Duration::from_millis(300);
/// Teto da espera: sob eventos contínuos (um build, um log) emite ao menos nesse intervalo.
const DEBOUNCE_MAX: Duration = Duration::from_millis(1000);

/// Junta os eventos (`true` = git, `false` = árvore) e chama `emit(árvore, git)` depois de
/// `quiet` sem eventos ou, no máximo, `max` depois do primeiro. Termina quando o canal fecha
/// (emitindo o que estava pendente) ou quando `emit` devolve `false`.
fn debounce(rx: &crossbeam_channel::Receiver<bool>, quiet: Duration, max: Duration, mut emit: impl FnMut(bool, bool) -> bool) {
    while let Ok(first) = rx.recv() {
        let (mut tree, mut git) = (!first, first);
        let deadline = Instant::now() + max;
        let mut open = true;
        while let Some(left) = deadline.checked_duration_since(Instant::now()).filter(|d| !d.is_zero()) {
            match rx.recv_timeout(quiet.min(left)) {
                Ok(is_git) => {
                    git |= is_git;
                    tree |= !is_git;
                }
                Err(crossbeam_channel::RecvTimeoutError::Timeout) => break,
                Err(crossbeam_channel::RecvTimeoutError::Disconnected) => {
                    open = false;
                    break;
                }
            }
        }
        if !emit(tree, git) || !open {
            return;
        }
    }
}

/// Inicia um watcher recursivo com debounce de 300 ms que emite
/// [`crate::CoreEvent::WorkspaceChanged`] (e [`crate::CoreEvent::GitChanged`] quando muda o estado
/// do repositório em `.git/`). O watcher para quando o valor retornado é solto.
pub(crate) fn start_watcher(workspace: &Path, core: Weak<crate::CoreInner>) -> notify::Result<notify::RecommendedWatcher> {
    // `true` = mudança no repositório git; `false` = na árvore de arquivos.
    let (tx, rx) = crossbeam_channel::unbounded::<bool>();
    let ws = workspace.to_path_buf();
    let mut watcher = notify::recommended_watcher(move |res: notify::Result<notify::Event>| {
        let Ok(ev) = res else { return };
        // Leituras (Access) não alteram a árvore — evita laço com a própria listagem.
        if matches!(ev.kind, EventKind::Access(_)) {
            return;
        }
        if ev.paths.iter().any(|p| git_state_change(&ws, p)) {
            let _ = tx.send(true);
        }
        if !ev.paths.is_empty() && ev.paths.iter().all(|p| watch_ignored(&ws, p)) {
            return;
        }
        let _ = tx.send(false);
    })?;
    watcher.watch(workspace, RecursiveMode::Recursive)?;
    std::thread::Builder::new()
        .name("cd-watch-debounce".into())
        .spawn(move || {
            // Termina quando o watcher (dono do `tx`) é solto.
            debounce(&rx, DEBOUNCE_QUIET, DEBOUNCE_MAX, |tree, git| {
                let Some(inner) = core.upgrade() else { return false };
                let core = crate::Core { inner };
                if tree {
                    core.emit(crate::CoreEvent::WorkspaceChanged);
                }
                if git {
                    core.emit(crate::CoreEvent::GitChanged);
                }
                true
            });
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
    fn read_dir_stays_inside_the_workspace() {
        let tmp = tempfile::tempdir().expect("tmp");
        let ws = tmp.path().join("ws");
        std::fs::create_dir_all(ws.join("sub")).expect("mkdir");
        std::fs::write(tmp.path().join("fora.txt"), "x").expect("write");
        std::fs::write(ws.join("sub").join("dentro.txt"), "x").expect("write");
        for bad in ["..", "../", "../x", "a/../b", "sub/../sub", "/x", "C:..\\x", "sub/../.."] {
            assert!(read_dir(&ws, bad).is_empty(), "{bad:?}");
        }
        let abs = tmp.path().to_string_lossy().into_owned();
        assert!(read_dir(&ws, &abs).is_empty(), "absoluto");
        assert_eq!(read_dir(&ws, "./sub")[0].name, "dentro.txt");
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
        std::fs::write(foreign.join("AGENTS.md"), "# Regras\n\n<!-- jwic:start -->\nvelho $1\n<!-- jwic:end -->\n\nfim\n").expect("write");
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
    fn agents_md_keeps_crlf() {
        let tmp = tempfile::tempdir().expect("tmp");
        let (app, home, ws) = (tmp.path().join("app"), tmp.path().join("home"), tmp.path().join("outro"));
        std::fs::create_dir_all(&ws).expect("mkdir");
        let file = ws.join("AGENTS.md");
        let only_crlf = |t: &str| t.matches('\n').count() == t.matches("\r\n").count();
        for start in ["# Regras\r\n\r\nfim\r\n", "# Regras\r\n\r\n<!-- commanddeck:start -->\r\nvelho\r\n<!-- commanddeck:end -->\r\n"] {
            std::fs::write(&file, start).expect("write");
            assert_eq!(ensure_agent_context(&ws, &app, &home, "linha 1\nlinha 2").expect("ok"), AgentContext::Written);
            let text = std::fs::read_to_string(&file).expect("read");
            assert!(only_crlf(&text), "{text:?}");
            assert!(text.contains(&format!("{MARKER_START}\r\nlinha 1\r\nlinha 2\r\n{MARKER_END}\r\n")), "{text:?}");
            // Segunda vez: nada muda.
            assert_eq!(ensure_agent_context(&ws, &app, &home, "linha 1\nlinha 2").expect("ok"), AgentContext::Unchanged);
            assert_eq!(std::fs::read_to_string(&file).expect("read"), text);
        }
        // Nenhum temporário fica para trás.
        let names: Vec<String> =
            std::fs::read_dir(&ws).expect("ls").flatten().map(|e| e.file_name().to_string_lossy().into_owned()).collect();
        assert_eq!(names, ["AGENTS.md"]);
    }

    #[test]
    fn debounce_emits_under_a_continuous_stream_and_on_disconnect() {
        let (tx, rx) = crossbeam_channel::unbounded::<bool>();
        let feeder = std::thread::spawn(move || {
            // Um evento a cada 20 ms por ~700 ms: nunca há 100 ms de silêncio.
            for i in 0..35 {
                let _ = tx.send(i % 5 == 0);
                std::thread::sleep(Duration::from_millis(20));
            }
            // Último evento e o canal fecha logo em seguida: não pode se perder.
            let _ = tx.send(false);
        });
        let started = Instant::now();
        let mut emitted = Vec::new();
        debounce(&rx, Duration::from_millis(100), Duration::from_millis(250), |tree, git| {
            emitted.push((started.elapsed(), tree, git));
            true
        });
        feeder.join().expect("feeder");
        assert!(emitted.len() >= 2, "{emitted:?}");
        assert!(emitted[0].0 < Duration::from_millis(600), "o teto vale sob fluxo contínuo: {emitted:?}");
        assert!(emitted[0].1 && emitted[0].2);
        // `emit` devolvendo `false` encerra.
        let (tx, rx) = crossbeam_channel::unbounded::<bool>();
        tx.send(true).expect("send");
        let mut calls = 0;
        debounce(&rx, Duration::from_millis(10), Duration::from_millis(50), |_, _| {
            calls += 1;
            false
        });
        assert_eq!(calls, 1);
    }

    #[test]
    fn watcher_filter() {
        let ws = Path::new("/w");
        assert!(watch_ignored(ws, Path::new("/w/.git/index")));
        // …mas o estado do repositório vira um aviso próprio (GitChanged), sem os `.lock` do git.
        for p in [
            "/w/.git/HEAD",
            "/w/.git/index",
            "/w/.git/refs/heads/main",
            "/w/.git/FETCH_HEAD",
            "/w/.git/packed-refs",
            "/w/.git/CHERRY_PICK_HEAD",
            "/w/.git/REVERT_HEAD",
            "/w/.git/rebase-merge/done",
            "/w/.git/rebase-apply",
        ] {
            assert!(git_state_change(ws, Path::new(p)), "{p}");
        }
        // Fora do workspace nada é ignorado (uma pasta-mãe chamada `dados` não conta).
        assert!(!watch_ignored(Path::new("/dados/app"), Path::new("/outro/dados/x")));
        assert!(!watch_ignored(Path::new("/dados/app"), Path::new("/dados/app/src/x")));
        if cfg!(windows) {
            // Letra da unidade com outra caixa: é o mesmo repositório.
            assert!(git_state_change(Path::new(r"c:\w"), Path::new(r"C:\W\.git\HEAD")));
            assert!(watch_ignored(Path::new(r"c:\w"), Path::new(r"C:\w\target\x")));
        }
        for p in ["/w/.git/index.lock", "/w/.git/objects/ab/cd", "/w/.git/logs/HEAD", "/w/src/HEAD", "/w/.gitignore"] {
            assert!(!git_state_change(ws, Path::new(p)), "{p}");
        }
        assert!(watch_ignored(ws, Path::new("/w/sub/node_modules/x")));
        assert!(watch_ignored(ws, Path::new("/w/target/debug/x")));
        assert!(!watch_ignored(ws, Path::new("/w/src/main.rs")));
        assert!(!watch_ignored(ws, Path::new("/w/.gitignore")));
    }
}
