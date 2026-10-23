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

/// Nomes ignorados na árvore (`.gemini` é a pasta de configuração do Antigravity, `~/.gemini/antigravity`;
/// `.agentdeck` guarda as worktrees das tarefas).
pub const IGNORED: &[&str] = &["node_modules", ".git", ".gemini", ".agentdeck", "dados", "$RECYCLE.BIN", "target"];

fn is_ignored_entry(name: &str) -> bool {
    if cfg!(windows) {
        IGNORED.iter().any(|i| i.eq_ignore_ascii_case(name))
    } else {
        IGNORED.contains(&name)
    }
}

pub fn read_dir(workspace: &Path, rel_dir: &str) -> Vec<DirEntry> {
    // Só subpastas do workspace (`""` = a raiz): nada de `..`, absoluto ou `C:` no Windows.
    // A barra invertida é separador no Windows (é o que `rel_path` usava antes): lá ela vira `/` antes de
    // validar e de juntar. No Unix ela é um caractere válido de nome (`a\b` é uma pasta), e o `rel_path`
    // que volta da árvore traz o nome intacto: trocá-la abriria outra pasta.
    let rel_dir = if cfg!(windows) { rel_dir.replace('\\', "/") } else { rel_dir.to_owned() };
    if !rel_dir.is_empty() && !crate::git::safe_rel(&rel_dir) {
        return Vec::new();
    }
    let target = workspace.join(rel_dir.as_str());
    let Ok(rd) = std::fs::read_dir(&target) else { return Vec::new() };
    let rel_norm = rel_dir.trim_start_matches("./").trim_matches('/');
    let mut items: Vec<DirEntry> = rd
        .flatten()
        .filter_map(|e| {
            let name = e.file_name().to_string_lossy().into_owned();
            if is_ignored_entry(&name) {
                return None;
            }
            let is_dir = e
                .file_type()
                .map(|t| if t.is_symlink() { std::fs::metadata(e.path()).is_ok_and(|m| m.is_dir()) } else { t.is_dir() })
                .unwrap_or(false);
            let rel_path = if rel_norm.is_empty() || rel_norm == "." { name.clone() } else { format!("{rel_norm}/{name}") };
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

/// Limite do que `read_text` lê: acima disso o arquivo não vira texto na tela.
pub const READ_TEXT_MAX: u64 = 1024 * 1024;

/// O que `read_text` encontrou.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TextFile {
    Text(String),
    /// Maior que [`READ_TEXT_MAX`] (tamanho em bytes); o conteúdo não é lido.
    TooLarge(u64),
    /// Tem bytes nulos: não é texto.
    Binary,
}

/// Garante que o caminho acessado esteja estritamente dentro do workspace (normalização
/// lexical: funciona com caminhos inexistentes; case-insensitive no Windows).
pub fn resolve_workspace_path(rel_or_abs: &str, workspace: &Path) -> Result<PathBuf, String> {
    if workspace.as_os_str().is_empty() {
        return Err("Workspace não configurado.".into());
    }
    let ws = crate::util::absolutize(workspace);
    let target = crate::util::normalize_lexical(&ws.join(rel_or_abs));
    if !crate::util::path_within(&target, &ws) {
        return Err(format!("Acesso negado: o caminho \"{rel_or_abs}\" está fora do workspace configurado (\"{}\").", workspace.display()));
    }
    Ok(target)
}

/// Lê um arquivo de texto do workspace para a visualização somente leitura. O caminho precisa ser
/// relativo e seguro (`git::safe_rel`) e, depois de resolver links simbólicos, continuar dentro do
/// workspace. Bytes que não são UTF-8 viram `�` (arquivos antigos em Latin-1 ainda se leem).
pub fn read_text(workspace: &Path, rel: &str) -> Result<TextFile, String> {
    if workspace.as_os_str().is_empty() {
        return Err("nenhum projeto aberto".into());
    }
    if !crate::git::safe_rel(rel) {
        return Err(format!("caminho inválido: {rel}"));
    }
    let root = std::fs::canonicalize(workspace).map_err(|e| format!("a pasta do projeto não abre: {e}"))?;
    let full = std::fs::canonicalize(workspace.join(rel)).map_err(|e| format!("não consegui abrir {rel}: {e}"))?;
    if !crate::util::path_within(&full, &root) {
        return Err(format!("{rel} está fora da pasta do projeto"));
    }
    let meta = std::fs::metadata(&full).map_err(|e| format!("não consegui ler {rel}: {e}"))?;
    if !meta.is_file() {
        return Err(format!("{rel} não é um arquivo"));
    }
    if meta.len() > READ_TEXT_MAX {
        return Ok(TextFile::TooLarge(meta.len()));
    }
    let bytes = std::fs::read(&full).map_err(|e| format!("não consegui ler {rel}: {e}"))?;
    if bytes.contains(&0) {
        return Ok(TextFile::Binary);
    }
    Ok(TextFile::Text(String::from_utf8_lossy(&bytes).into_owned()))
}

/// O fim de linha de um arquivo de texto (o editor trabalha com `\n` e grava de volta com o mesmo).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Eol {
    #[default]
    Lf,
    Crlf,
}

impl Eol {
    /// O texto com `\n` e o fim de linha que ele usava. `None` se mistura os dois (ou tem `\r` solto): trocar tudo
    /// por um só alteraria linhas que o usuário não tocou.
    pub fn normalize(text: &str) -> Option<(String, Eol)> {
        let crlf = text.matches("\r\n").count();
        let lf = text.matches('\n').count();
        if text.replace("\r\n", "").contains('\r') {
            return None;
        }
        match (crlf, lf) {
            (0, _) => Some((text.to_owned(), Eol::Lf)),
            (c, l) if c == l => Some((text.replace("\r\n", "\n"), Eol::Crlf)),
            _ => None,
        }
    }

    /// `text_lf` (só com `\n`) no fim de linha `self`.
    pub fn apply(self, text_lf: &str) -> String {
        match self {
            Eol::Lf => text_lf.to_owned(),
            Eol::Crlf => text_lf.replace('\n', "\r\n"),
        }
    }
}

/// Grava `text_lf` (com `\n`) em `rel`, no fim de linha `eol` e, se `bom`, com o BOM UTF-8 que o arquivo já tinha
/// (nunca acrescenta um). Ao lado e trocando ([`write_atomic`]): um agente lendo o arquivo nunca o vê pela metade.
/// O caminho precisa ser relativo e seguro e ficar dentro do workspace; o arquivo pode já não existir (apagado por
/// um agente enquanto era editado), mas a pasta dele sim.
pub fn write_text(workspace: &Path, rel: &str, text_lf: &str, eol: Eol, bom: bool) -> Result<(), String> {
    if workspace.as_os_str().is_empty() {
        return Err("nenhum projeto aberto".into());
    }
    if !crate::git::safe_rel(rel) {
        return Err(format!("caminho inválido: {rel}"));
    }
    let root = std::fs::canonicalize(workspace).map_err(|e| format!("a pasta do projeto não abre: {e}"))?;
    let joined = workspace.join(rel);
    let target = match std::fs::canonicalize(&joined) {
        Ok(full) => full,
        Err(_) => {
            let name = joined.file_name().ok_or_else(|| format!("caminho inválido: {rel}"))?.to_owned();
            let parent = joined.parent().ok_or_else(|| format!("caminho inválido: {rel}"))?;
            std::fs::canonicalize(parent).map_err(|e| format!("a pasta de {rel} não abre: {e}"))?.join(name)
        }
    };
    if !crate::util::path_within(&target, &root) {
        return Err(format!("{rel} está fora da pasta do projeto"));
    }
    if target.is_dir() {
        return Err(format!("{rel} não é um arquivo"));
    }
    let mut bytes = Vec::with_capacity(text_lf.len() + 3);
    if bom {
        bytes.extend_from_slice(&[0xEF, 0xBB, 0xBF]);
    }
    bytes.extend_from_slice(eol.apply(text_lf).as_bytes());
    write_atomic(&target, &bytes).map_err(|e| format!("não consegui gravar {rel}: {e}"))
}

// ---------- AGENTS.md ----------

pub const MARKER_START: &str = "<!-- agentdeck:start -->";
pub const MARKER_END: &str = "<!-- agentdeck:end -->";

fn marker_re() -> &'static regex::Regex {
    static RE: std::sync::OnceLock<regex::Regex> = std::sync::OnceLock::new();
    RE.get_or_init(|| {
        // Reconhece também os marcadores de nomes mais antigos (`jwic`, `orquestra`) e os troca pelo atual.
        regex::Regex::new(r"<!-- (?:orquestra|jwic|agentdeck):start -->[\s\S]*?<!-- (?:orquestra|jwic|agentdeck):end -->")
            .expect("regex válida")
    })
}

/// Troca o primeiro bloco marcado por `block` e apaga os demais (com as linhas em branco que os
/// separavam do resto): um app de versão anterior que não reconhece o marcador novo acrescenta o
/// seu próprio bloco no fim, e sem isso os dois ficariam para sempre.
fn replace_marker_blocks(old: &str, block: &str) -> String {
    let mut out = String::with_capacity(old.len());
    let mut pos = 0;
    for (i, m) in marker_re().find_iter(old).enumerate() {
        out.push_str(&old[pos..m.start()]);
        if i == 0 {
            out.push_str(block);
        } else {
            for _ in 0..2 {
                if out.ends_with("\r\n") {
                    out.truncate(out.len() - 2);
                } else if out.ends_with('\n') {
                    out.pop();
                }
            }
        }
        pos = m.end();
    }
    out.push_str(&old[pos..]);
    out
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

/// Mantém o bloco marcado do AgentDeck no AGENTS.md do workspace (o Antigravity CLI não
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
        replace_marker_blocks(&old, &block)
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

/// `text` sem os blocos marcados do app e sem diferença de fim de linha: o que sobra é do usuário.
fn without_marker_blocks(text: &str) -> String {
    marker_re().replace_all(&text.replace("\r\n", "\n"), "").trim().to_owned()
}

/// `true` se `current` difere de `committed` só nos blocos marcados do app (fora deles o texto é igual).
pub fn only_marker_blocks_differ(committed: &str, current: &str) -> bool {
    without_marker_blocks(committed) == without_marker_blocks(current)
}

/// Põe em `file` o primeiro bloco marcado de `saved` (o que o app tinha gravado): troca o bloco que o arquivo
/// tem agora ou, se não tiver, acrescenta ao final. Mantém o fim de linha do arquivo. Sem bloco em `saved`,
/// não faz nada. Usada pela mesclagem de tarefas, que devolve o arquivo ao do HEAD para o git mesclar.
pub fn restore_marker_block(file: &Path, saved: &str) -> std::io::Result<()> {
    let Some(found) = marker_re().find(saved) else { return Ok(()) };
    let old = std::fs::read_to_string(file).unwrap_or_default();
    let eol = if old.contains("\r\n") { "\r\n" } else { "\n" };
    let block = found.as_str().replace("\r\n", "\n").replace('\n', eol);
    let new = if marker_re().is_match(&old) {
        replace_marker_blocks(&old, &block)
    } else if old.is_empty() {
        format!("{block}{eol}")
    } else {
        format!("{}{eol}{eol}{block}{eol}", old.trim_end())
    };
    if new == old {
        return Ok(());
    }
    write_atomic(file, new.as_bytes())
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
const WATCH_IGNORED: &[&str] = &[".git", "node_modules", ".gemini", ".agentdeck", "dados", "target"];

/// `path` relativo a `base`, comparando por componentes (sem diferença de caixa nem prefixo
/// verbatim `\\?\` no Windows: `c:\x`, `C:\x` e `\\?\C:\x` são a mesma pasta). `None` se estiver fora.
fn rel_within(base: &Path, path: &Path) -> Option<PathBuf> {
    let base = crate::util::strip_verbatim(base);
    let path = crate::util::strip_verbatim(path);
    crate::util::path_within(&path, &base).then(|| path.components().skip(base.components().count()).collect())
}

/// `true` se a mudança em `path` deve ser ignorada. Fora do workspace não se ignora: as pastas
/// acima dele (um projeto em `C:\dados\app`, por exemplo) não contam.
pub(crate) fn watch_ignored(workspace: &Path, path: &Path) -> bool {
    let Some(rel) = rel_within(workspace, path) else { return false };
    rel.components().any(|c| match c {
        Component::Normal(s) => WATCH_IGNORED.iter().any(|i| crate::util::same_component(s, std::ffi::OsStr::new(i))),
        _ => false,
    })
}

/// A mudança em `path` é no estado do repositório git do workspace (`.git/HEAD`, `index`, refs,
/// merge/rebase/cherry-pick/revert em andamento…)? Os `*.lock` (temporários do git) não contam.
pub(crate) fn git_state_change(workspace: &Path, path: &Path) -> bool {
    git_state_change_in(&workspace.join(".git"), path)
}

/// Como [`git_state_change`], mas comparando diretamente com a pasta `git_dir` (que pode ficar
/// fora de `workspace/.git` em worktrees, submódulos ou subpastas do repositório).
pub(crate) fn git_state_change_in(git_dir: &Path, path: &Path) -> bool {
    let Some(rel) = rel_within(git_dir, path) else { return false };
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

fn resolve_git_path(base: &Path, raw: &str) -> Option<PathBuf> {
    let raw = raw.trim();
    if raw.is_empty() {
        return None;
    }
    let p = Path::new(raw);
    let abs = if p.is_absolute() {
        crate::util::normalize_lexical(p)
    } else {
        crate::util::normalize_lexical(&crate::util::absolutize(base).join(p))
    };
    abs.is_dir().then_some(abs)
}

/// Lê um arquivo `.git` (`gitdir: <caminho>`) de worktree/submódulo quando o binário `git` não
/// está disponível (ex.: ambientes de teste isolados).
fn git_dirs_from_dotgit_file(workspace: &Path) -> Vec<PathBuf> {
    let dotgit = workspace.join(".git");
    if !dotgit.is_file() {
        return Vec::new();
    }
    let Ok(text) = std::fs::read_to_string(&dotgit) else { return Vec::new() };
    let Some(raw) = text.lines().find_map(|l| l.strip_prefix("gitdir:")) else { return Vec::new() };
    let Some(gd) = resolve_git_path(workspace, raw) else { return Vec::new() };
    let mut out = vec![gd.clone()];
    if let Ok(common) = std::fs::read_to_string(gd.join("commondir")) {
        if let Some(cd) = resolve_git_path(&gd, &common) {
            if !crate::util::same_path(&cd, &gd) {
                out.push(cd);
            }
        }
    }
    out
}

/// Descobre o `.git` real do workspace (`git rev-parse --absolute-git-dir --git-common-dir`, sem
/// shell). Chamado fora da thread da interface.
pub(crate) fn discover_git_dirs(core: &crate::Core, workspace: &Path) -> Vec<PathBuf> {
    let mut git = core.inner.agents.which(if cfg!(windows) { "git.exe" } else { "git" });
    if git.is_none() && cfg!(windows) {
        git = core.inner.agents.system_dir("ProgramFiles").map(|p| p.join("Git").join("cmd").join("git.exe")).filter(|p| p.is_file());
    }
    let mut out = Vec::new();
    if let Some(git) = git {
        let mut cmd = std::process::Command::new(git);
        cmd.current_dir(workspace)
            .args(["-c", "core.quotepath=false", "rev-parse", "--absolute-git-dir", "--git-common-dir"])
            .env("GIT_TERMINAL_PROMPT", "0")
            .env("GIT_OPTIONAL_LOCKS", "0");
        for k in ["GIT_DIR", "GIT_WORK_TREE", "GIT_INDEX_FILE", "GIT_PREFIX"] {
            cmd.env_remove(k);
        }
        if let Ok(res) = crate::process::run_captured(cmd, None, Duration::from_secs(5), None, None) {
            if res.exit == crate::process::Exit::Code(Some(0)) {
                for line in res.stdout.lines() {
                    if let Some(p) = resolve_git_path(workspace, line) {
                        if !out.iter().any(|e: &PathBuf| crate::util::same_path(e, &p)) {
                            out.push(p);
                        }
                    }
                }
            }
        }
    }
    if out.is_empty() {
        out = git_dirs_from_dotgit_file(workspace);
    }
    out
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
    let extra_git: std::sync::Arc<parking_lot::RwLock<Vec<PathBuf>>> = std::sync::Arc::default();
    let extra_for_cb = extra_git.clone();
    let mut watcher = notify::recommended_watcher(move |res: notify::Result<notify::Event>| {
        let Ok(ev) = res else { return };
        // Leituras (Access) não alteram a árvore — evita laço com a própria listagem.
        if matches!(ev.kind, EventKind::Access(_)) {
            return;
        }
        let extra = extra_for_cb.read();
        if ev.paths.iter().any(|p| git_state_change(&ws, p) || extra.iter().any(|gd| git_state_change_in(gd, p))) {
            let _ = tx.send(true);
        }
        let in_extra = |p: &Path| extra.iter().any(|gd| rel_within(gd, p).is_some());
        if !ev.paths.is_empty() && ev.paths.iter().all(|p| watch_ignored(&ws, p) || in_extra(p)) {
            return;
        }
        let _ = tx.send(false);
    })?;
    watcher.watch(workspace, RecursiveMode::Recursive)?;
    let ws_for_thread = workspace.to_path_buf();
    std::thread::Builder::new()
        .name("cd-watch-debounce".into())
        .spawn(move || {
            // Descobre o `.git` real fora da thread da UI (worktrees, submódulos, subpasta de repo).
            if let Some(inner) = core.upgrade() {
                let c = crate::Core { inner: inner.clone() };
                let default_git = ws_for_thread.join(".git");
                let dirs: Vec<PathBuf> =
                    discover_git_dirs(&c, &ws_for_thread).into_iter().filter(|gd| !crate::util::same_path(gd, &default_git)).collect();
                if !dirs.is_empty() {
                    *extra_git.write() = dirs.clone();
                    let mut guard = inner.watcher.lock();
                    if crate::util::same_path(&c.workspace(), &ws_for_thread) {
                        if let Some(w) = guard.as_mut() {
                            for gd in &dirs {
                                if !crate::util::path_within(gd, &ws_for_thread) {
                                    let _ = w.watch(gd, RecursiveMode::Recursive);
                                }
                            }
                        }
                    }
                }
            }
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
    fn sandbox_resolution() {
        let tmp = tempfile::tempdir().expect("tmp");
        let ws = tmp.path().join("ws");
        std::fs::create_dir_all(&ws).expect("mkdir");
        assert_eq!(resolve_workspace_path("a/b.txt", &ws).expect("ok"), ws.join("a").join("b.txt"));
        assert_eq!(resolve_workspace_path("novo/inexistente/../x.txt", &ws).expect("ok"), ws.join("novo").join("x.txt"));
        assert_eq!(resolve_workspace_path(".", &ws).expect("ok"), ws);
        assert_eq!(resolve_workspace_path(&ws.join("in.txt").to_string_lossy(), &ws).expect("ok"), ws.join("in.txt"));
        for bad in ["../fora.txt", "a/../../fora", "../ws-irmao/x", "../../../../../../etc/passwd"] {
            let err = resolve_workspace_path(bad, &ws).expect_err(bad);
            assert!(err.starts_with("Acesso negado: o caminho"), "{err}");
        }
        let outside = tmp.path().join("fora.txt");
        assert!(resolve_workspace_path(&outside.to_string_lossy(), &ws).is_err());
        #[cfg(unix)]
        assert!(resolve_workspace_path("/etc/passwd", &ws).is_err());
        #[cfg(windows)]
        {
            let upper = ws.to_string_lossy().to_uppercase();
            assert!(resolve_workspace_path(&format!("{upper}\\x.txt"), &ws).is_ok());
            assert!(resolve_workspace_path("C:\\Windows\\win.ini", &ws).is_err());
        }
    }

    #[test]
    fn read_text_reads_text_and_refuses_big_binary_and_outside_files() {
        let tmp = tempfile::tempdir().expect("tmp");
        let ws = tmp.path().join("ws");
        std::fs::create_dir_all(ws.join("sub")).expect("mkdir");
        std::fs::write(ws.join("sub").join("a.rs"), "fn main() {}\n").expect("write");
        std::fs::write(ws.join("latin.txt"), b"caf\xe9").expect("write");
        std::fs::write(ws.join("bin.dat"), b"MZ\0\0abc").expect("write");
        std::fs::write(ws.join("big.txt"), vec![b'x'; READ_TEXT_MAX as usize + 1]).expect("write");
        std::fs::write(ws.join("limit.txt"), vec![b'x'; READ_TEXT_MAX as usize]).expect("write");
        std::fs::write(tmp.path().join("secret.txt"), "fora").expect("write");

        assert_eq!(read_text(&ws, "sub/a.rs"), Ok(TextFile::Text("fn main() {}\n".into())));
        assert_eq!(read_text(&ws, "latin.txt"), Ok(TextFile::Text("caf\u{fffd}".into())));
        assert_eq!(read_text(&ws, "bin.dat"), Ok(TextFile::Binary));
        assert_eq!(read_text(&ws, "big.txt"), Ok(TextFile::TooLarge(READ_TEXT_MAX + 1)));
        assert!(matches!(read_text(&ws, "limit.txt"), Ok(TextFile::Text(t)) if t.len() as u64 == READ_TEXT_MAX));
        // Fora do workspace, absoluto, pasta, inexistente e sem projeto.
        assert!(read_text(&ws, "../secret.txt").is_err());
        assert!(read_text(&ws, &tmp.path().join("secret.txt").to_string_lossy()).is_err());
        assert!(read_text(&ws, "sub").is_err());
        assert!(read_text(&ws, "nao-existe.txt").is_err());
        assert!(read_text(&ws, "").is_err());
        assert!(read_text(Path::new(""), "sub/a.rs").is_err());
    }

    #[test]
    fn line_endings_are_detected_and_restored() {
        assert_eq!(Eol::normalize("a\nb\n"), Some(("a\nb\n".into(), Eol::Lf)));
        assert_eq!(Eol::normalize("a\r\nb\r\n"), Some(("a\nb\n".into(), Eol::Crlf)));
        assert_eq!(Eol::normalize("sem fim de linha"), Some(("sem fim de linha".into(), Eol::Lf)));
        assert_eq!(Eol::normalize(""), Some((String::new(), Eol::Lf)));
        // Misturado ou com \r solto: não dá para devolver igual.
        assert_eq!(Eol::normalize("a\r\nb\n"), None);
        assert_eq!(Eol::normalize("a\rb\n"), None);
        assert_eq!(Eol::Crlf.apply("a\nb\n"), "a\r\nb\r\n");
        assert_eq!(Eol::Lf.apply("a\nb\n"), "a\nb\n");
        let (lf, eol) = Eol::normalize("x\r\ny\r\n").expect("crlf");
        assert_eq!(eol.apply(&lf), "x\r\ny\r\n");
    }

    #[test]
    fn write_text_keeps_the_line_ending_and_never_adds_a_bom() {
        let tmp = tempfile::tempdir().expect("tmp");
        let ws = tmp.path().join("ws");
        std::fs::create_dir_all(ws.join("sub")).expect("mkdir");
        std::fs::write(tmp.path().join("fora.txt"), "fora").expect("write");
        write_text(&ws, "sub/a.txt", "um\ndois\n", Eol::Crlf, false).expect("grava");
        assert_eq!(std::fs::read(ws.join("sub/a.txt")).expect("read"), b"um\r\ndois\r\n");
        write_text(&ws, "sub/a.txt", "x\n", Eol::Lf, false).expect("regrava");
        assert_eq!(std::fs::read(ws.join("sub/a.txt")).expect("read"), b"x\n");
        // O BOM só volta se o arquivo já tinha um.
        write_text(&ws, "sub/b.txt", "x", Eol::Lf, true).expect("grava");
        assert_eq!(std::fs::read(ws.join("sub/b.txt")).expect("read"), b"\xEF\xBB\xBFx");
        // Nenhum temporário fica para trás.
        let names: Vec<String> =
            std::fs::read_dir(ws.join("sub")).expect("ls").flatten().map(|e| e.file_name().to_string_lossy().into_owned()).collect();
        assert_eq!(names.len(), 2, "{names:?}");
        // Fora do projeto, absoluto, pasta, pasta inexistente e sem projeto.
        assert!(write_text(&ws, "../fora.txt", "x", Eol::Lf, false).is_err());
        assert!(write_text(&ws, &tmp.path().join("fora.txt").to_string_lossy(), "x", Eol::Lf, false).is_err());
        assert!(write_text(&ws, "sub", "x", Eol::Lf, false).is_err());
        assert!(write_text(&ws, "nao/existe/a.txt", "x", Eol::Lf, false).is_err());
        assert!(write_text(Path::new(""), "a.txt", "x", Eol::Lf, false).is_err());
        assert_eq!(std::fs::read_to_string(tmp.path().join("fora.txt")).expect("read"), "fora");
    }

    #[test]
    fn read_dir_orders_and_filters() {
        let tmp = tempfile::tempdir().expect("tmp");
        let ws = tmp.path();
        for d in ["zeta", "Alpha", "node_modules", ".git", "dados", "target"] {
            std::fs::create_dir_all(ws.join(d)).expect("mkdir");
        }
        if cfg!(windows) {
            std::fs::create_dir_all(ws.join("$Recycle.Bin")).expect("mkdir");
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
        // A barra invertida só é separador no Windows; no Unix ela faz parte do nome (ver o teste abaixo).
        if cfg!(windows) {
            let sub_bs = read_dir(ws, "zeta\\");
            assert_eq!(sub_bs[0].rel_path, "zeta/in.json");
        }
        assert!(read_dir(ws, "nao-existe").is_empty());
    }

    #[cfg(unix)]
    #[test]
    fn read_dir_opens_a_unix_folder_with_a_backslash_in_the_name() {
        let tmp = tempfile::tempdir().expect("tmp");
        let ws = tmp.path();
        std::fs::create_dir_all(ws.join("a\\b")).expect("mkdir");
        std::fs::write(ws.join("a\\b").join("in.txt"), "x").expect("write");
        let root = read_dir(ws, "");
        assert_eq!(root[0].rel_path, "a\\b");
        let sub = read_dir(ws, &root[0].rel_path);
        assert_eq!(sub.len(), 1);
        assert_eq!(sub[0].name, "in.txt");
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
        for start in [
            "# Regras\r\n\r\nfim\r\n",
            "# Regras\r\n\r\n<!-- agentdeck:start -->\r\nvelho\r\n<!-- agentdeck:end -->\r\n",
            "# Regras\r\n\r\n<!-- jwic:start -->\r\nvelho\r\n<!-- jwic:end -->\r\n",
        ] {
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

    /// Um AGENTS.md escrito por uma versão antiga (marcadores `jwic`) tem o bloco trocado pelo
    /// novo, sem duplicar e sem tocar no resto do arquivo.
    #[test]
    fn an_old_name_block_is_replaced_not_duplicated() {
        let tmp = tempfile::tempdir().expect("tmp");
        let (app, home, ws) = (tmp.path().join("app"), tmp.path().join("home"), tmp.path().join("outro"));
        std::fs::create_dir_all(&ws).expect("mkdir");
        let file = ws.join("AGENTS.md");
        for eol in ["\n", "\r\n"] {
            let before = format!("# Regras{eol}{eol}Use tabs.{eol}{eol}");
            let after = format!("{eol}## Fim{eol}Obrigado.{eol}");
            let old_block = format!("<!-- jwic:start -->{eol}texto velho{eol}<!-- jwic:end -->");
            std::fs::write(&file, format!("{before}{old_block}{eol}{after}")).expect("write");
            assert_eq!(ensure_agent_context(&ws, &app, &home, "novo").expect("ok"), AgentContext::Written);
            let text = std::fs::read_to_string(&file).expect("read");
            let new_block = format!("{MARKER_START}{eol}novo{eol}{MARKER_END}");
            assert_eq!(text, format!("{before}{new_block}{eol}{after}"), "só o bloco muda");
            assert_eq!(text.matches("<!-- agentdeck:start -->").count(), 1);
            assert!(!text.contains("jwic"), "{text:?}");
            // Com o bloco novo, nada mais muda.
            assert_eq!(ensure_agent_context(&ws, &app, &home, "novo").expect("ok"), AgentContext::Unchanged);
        }
    }

    /// O app antigo não reconhece `agentdeck:` e acrescenta o seu bloco no fim: o app novo troca o
    /// primeiro e apaga os demais, sem deixar linhas em branco acumuladas nem tocar no resto.
    #[test]
    fn extra_blocks_from_an_old_app_are_removed() {
        let tmp = tempfile::tempdir().expect("tmp");
        let (app, home, ws) = (tmp.path().join("app"), tmp.path().join("home"), tmp.path().join("outro"));
        std::fs::create_dir_all(&ws).expect("mkdir");
        let file = ws.join("AGENTS.md");
        for eol in ["\n", "\r\n"] {
            let new_block = format!("{MARKER_START}{eol}novo{eol}{MARKER_END}");
            let new_a = format!("<!-- agentdeck:start -->{eol}velho{eol}<!-- agentdeck:end -->");
            let old_b = format!("<!-- jwic:start -->{eol}antigo{eol}<!-- jwic:end -->");
            // No fim (caso do app antigo), no meio e colado ao primeiro.
            let cases = [
                (format!("# Regras{eol}{eol}{new_a}{eol}{eol}{old_b}{eol}"), format!("# Regras{eol}{eol}{new_block}{eol}")),
                (
                    format!("# Regras{eol}{eol}{new_a}{eol}{eol}{old_b}{eol}{eol}Fim.{eol}"),
                    format!("# Regras{eol}{eol}{new_block}{eol}{eol}Fim.{eol}"),
                ),
                (format!("{new_a}{eol}{old_b}{eol}"), format!("{new_block}{eol}")),
            ];
            for (start, expected) in cases {
                std::fs::write(&file, &start).expect("write");
                assert_eq!(ensure_agent_context(&ws, &app, &home, "novo").expect("ok"), AgentContext::Written);
                assert_eq!(std::fs::read_to_string(&file).expect("read"), expected, "{start:?}");
                assert_eq!(ensure_agent_context(&ws, &app, &home, "novo").expect("ok"), AgentContext::Unchanged);
                assert_eq!(std::fs::read_to_string(&file).expect("read"), expected);
            }
        }
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
        // `.git` externo (worktree, submódulo ou subpasta de repositório).
        let ext_git = Path::new("/repo/.git/worktrees/wt");
        assert!(git_state_change_in(ext_git, Path::new("/repo/.git/worktrees/wt/HEAD")));
        assert!(git_state_change_in(ext_git, Path::new("/repo/.git/worktrees/wt/CHERRY_PICK_HEAD")));
        assert!(!git_state_change_in(ext_git, Path::new("/repo/.git/worktrees/wt/index.lock")));
        // Fora do workspace nada é ignorado (uma pasta-mãe chamada `dados` não conta).
        assert!(!watch_ignored(Path::new("/dados/app"), Path::new("/outro/dados/x")));
        assert!(!watch_ignored(Path::new("/dados/app"), Path::new("/dados/app/src/x")));
        if cfg!(windows) {
            // Letra da unidade com outra caixa e prefixo verbatim `\\?\`: é o mesmo repositório.
            assert!(git_state_change(Path::new(r"c:\w"), Path::new(r"C:\W\.git\HEAD")));
            assert!(git_state_change(Path::new(r"c:\w"), Path::new(r"\\?\C:\W\.git\HEAD")));
            assert!(git_state_change(Path::new(r"\\?\C:\w"), Path::new(r"c:\w\.git\CHERRY_PICK_HEAD")));
            assert!(watch_ignored(Path::new(r"c:\w"), Path::new(r"C:\w\target\x")));
            assert!(watch_ignored(Path::new(r"c:\w"), Path::new(r"\\?\C:\w\Target\x")));
            assert!(git_state_change_in(Path::new(r"c:\repo\.git"), Path::new(r"\\?\C:\REPO\.git\REVERT_HEAD")));
        }
        for p in ["/w/.git/index.lock", "/w/.git/objects/ab/cd", "/w/.git/logs/HEAD", "/w/src/HEAD", "/w/.gitignore"] {
            assert!(!git_state_change(ws, Path::new(p)), "{p}");
        }
        assert!(watch_ignored(ws, Path::new("/w/sub/node_modules/x")));
        assert!(watch_ignored(ws, Path::new("/w/target/debug/x")));
        assert!(!watch_ignored(ws, Path::new("/w/src/main.rs")));
        assert!(!watch_ignored(ws, Path::new("/w/.gitignore")));
    }

    #[test]
    fn watcher_observes_external_git_dir() {
        let tmp = tempfile::tempdir().expect("tmp");
        let root = tmp.path();
        let ws = root.join("worktree");
        let real_git = root.join("repo-git").join("worktrees").join("wt");
        let common_git = root.join("repo-git");
        std::fs::create_dir_all(&ws).expect("mkdir ws");
        std::fs::create_dir_all(real_git.join("rebase-merge")).expect("mkdir git");
        std::fs::create_dir_all(common_git.join("refs").join("heads")).expect("mkdir refs");
        // Simula uma worktree sem depender do binário `git` instalado na máquina.
        std::fs::write(ws.join(".git"), format!("gitdir: {}\n", real_git.display())).expect("dotgit");
        std::fs::write(real_git.join("commondir"), "../..\n").expect("commondir");

        let dirs = git_dirs_from_dotgit_file(&ws);
        assert_eq!(dirs.len(), 2, "{dirs:?}");
        assert!(crate::util::same_path(&dirs[0], &real_git));
        assert!(crate::util::same_path(&dirs[1], &common_git));

        let paths = crate::paths::AppPaths {
            app_dir: root.join("app"),
            bin_dir: root.join("bin"),
            data_dir: root.join("dados"),
            default_workspace: ws.clone(),
            whisper_dir: root.join("whisper"),
            config_file: root.join("cfg").join("config.json"),
        };
        let opts = crate::StartOptions {
            search_path: Some(root.join("vazio").into_os_string()),
            home_dir: Some(root.join("home")),
            watch_workspace: true,
            config: Some(crate::Config::with_project(ws.clone())),
            system_dirs: false,
        };
        let (core, rx) = crate::Core::start_with(paths, opts).expect("start");
        // Aguarda a thread `cd-watch-debounce` registrar o `git_dir` externo no watcher.
        let deadline = Instant::now() + Duration::from_secs(3);
        let mut got_git = false;
        while Instant::now() < deadline && !got_git {
            std::fs::write(real_git.join("CHERRY_PICK_HEAD"), "deadbeef\n").expect("write head");
            while let Ok(ev) = rx.recv_timeout(Duration::from_millis(450)) {
                if matches!(ev, crate::CoreEvent::GitChanged) {
                    got_git = true;
                    break;
                }
            }
        }
        core.shutdown();
        assert!(got_git, "mudança no git_dir externo deveria emitir GitChanged");
    }
}
