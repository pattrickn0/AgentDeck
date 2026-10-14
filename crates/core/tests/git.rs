//! Integração com o git real (pula com aviso se o `git` não estiver instalado). A configuração do
//! usuário é isolada: `GIT_CONFIG_GLOBAL` aponta para um arquivo do teste.

use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::OnceLock;

use commanddeck_core::git::{EntryKind, GitError, RepoState};
use commanddeck_core::paths::AppPaths;
use commanddeck_core::{Config, Core, StartOptions};

/// Pasta da configuração global isolada (criada uma vez para o processo de teste).
fn isolated_config() -> &'static Path {
    static DIR: OnceLock<PathBuf> = OnceLock::new();
    DIR.get_or_init(|| {
        let dir = tempfile::tempdir().expect("tmp").keep();
        std::fs::write(
            dir.join("gitconfig"),
            "[user]\n\tname = Teste\n\temail = teste@example.com\n[init]\n\tdefaultBranch = main\n[commit]\n\tgpgsign = false\n",
        )
        .expect("gitconfig");
        // Só leitura de variáveis pelo git (filhos deste processo); definidas antes de qualquer core.
        std::env::set_var("GIT_CONFIG_GLOBAL", dir.join("gitconfig"));
        std::env::set_var("GIT_CONFIG_NOSYSTEM", "1");
        // O git não sobe além da pasta temporária procurando um repositório.
        std::env::set_var("GIT_CEILING_DIRECTORIES", std::env::temp_dir());
        dir
    })
}

fn git_exe() -> Option<PathBuf> {
    which::which("git").ok()
}

struct Env {
    _tmp: tempfile::TempDir,
    core: Core,
    ws: PathBuf,
    root: PathBuf,
}

/// Core com o workspace em `<tmp>/ws` e o git no PATH de busca (`with_git = false`: sem git).
fn start(with_git: bool) -> Env {
    isolated_config();
    let tmp = tempfile::tempdir().expect("tmp");
    let root = tmp.path().to_path_buf();
    let paths = AppPaths {
        app_dir: root.clone(),
        bin_dir: root.join("bin"),
        data_dir: root.join("dados"),
        default_workspace: root.join("ws"),
        whisper_dir: root.join("whisper"),
        config_file: root.join("cfg").join("config.json"),
    };
    let search = if with_git { git_exe().and_then(|g| g.parent().map(Path::to_path_buf)).unwrap_or_default() } else { root.join("vazio") };
    let opts = StartOptions {
        search_path: Some(search.into_os_string()),
        home_dir: Some(root.join("home")),
        ollama_discovery: false,
        watch_workspace: false,
        config: Some(Config::default_for(&paths)),
        system_dirs: false,
    };
    let (core, _rx) = Core::start_with(paths, opts).expect("start");
    let ws = root.join("ws");
    Env { _tmp: tmp, core, ws, root }
}

/// Roda o git direto (preparação do cenário).
fn git(dir: &Path, args: &[&str]) -> std::process::Output {
    Command::new(git_exe().expect("git")).current_dir(dir).args(args).output().expect("git")
}

macro_rules! need_git {
    () => {
        if git_exe().is_none() {
            eprintln!("git não instalado: teste pulado");
            return;
        }
    };
}

fn paths(st: &commanddeck_core::git::GitStatus) -> Vec<(String, char, char, EntryKind)> {
    st.entries.iter().map(|e| (e.path.clone(), e.index, e.worktree, e.kind)).collect()
}

#[test]
fn without_git_or_without_a_repository() {
    need_git!();
    let env = start(false);
    assert_eq!(env.core.git_status().unwrap_err(), GitError::NotInstalled);
    env.core.shutdown();
    let env = start(true);
    assert_eq!(env.core.git_status().unwrap_err(), GitError::NotARepo);
    env.core.shutdown();
}

#[test]
fn stage_commit_discard_branches_renames_and_conflicts() {
    need_git!();
    let env = start(true);
    let (core, ws) = (&env.core, &env.ws);
    // O app grava o AGENTS.md no workspace ao iniciar; aqui ele só atrapalharia as contas.
    let _ = std::fs::remove_file(ws.join("AGENTS.md"));
    core.git_init().expect("init");
    std::fs::write(ws.join("a b.txt"), "um\ndois\n").expect("w");
    std::fs::create_dir_all(ws.join("ção")).expect("mkdir");
    std::fs::write(ws.join("ção").join("ü.txt"), "x\n").expect("w");

    let st = core.git_status().expect("status");
    assert_eq!((st.branch.as_deref(), st.head.as_deref(), st.prefix.as_str()), (Some("main"), None, ""));
    assert_eq!(paths(&st), [("a b.txt".into(), '?', '?', EntryKind::Untracked), ("ção/".into(), '?', '?', EntryKind::Untracked)]);

    // Diff de um arquivo não rastreado: o conteúdo todo.
    let d = core.git_diff(&st.entries[0], false).expect("diff");
    assert_eq!(d.lines.iter().map(|l| l.text.as_str()).collect::<Vec<_>>(), ["um", "dois"]);

    // Stage de um arquivo, unstage (sem HEAD ainda), stage de tudo.
    core.git_stage(&["a b.txt".into()]).expect("stage");
    assert_eq!(paths(&core.git_status().expect("st"))[0], ("a b.txt".into(), 'A', '.', EntryKind::Ordinary));
    core.git_unstage(&["a b.txt".into()]).expect("unstage sem HEAD");
    assert_eq!(paths(&core.git_status().expect("st"))[0].1, '?');
    core.git_stage_all().expect("stage all");
    let st = core.git_status().expect("st");
    assert!(st.entries.iter().all(|e| e.index == 'A'), "{:?}", paths(&st));
    let staged = core.git_diff(&st.entries[0], true).expect("diff staged");
    assert!(staged.lines.iter().any(|l| l.text == "+um"));

    // Commit, e a última mensagem volta para "corrigir último commit".
    let summary = core.git_commit("primeiro commit\n\ncorpo", false).expect("commit");
    assert!(summary.ends_with(" primeiro commit"), "{summary}");
    assert_eq!(core.git_last_message().as_deref(), Some("primeiro commit\n\ncorpo"));
    let st = core.git_status().expect("st");
    assert!(st.head.is_some() && st.entries.is_empty());
    assert!(core.git_commit("  ", false).is_err(), "mensagem vazia é recusada");

    // Alteração não preparada, diff numerado, descartar.
    std::fs::write(ws.join("a b.txt"), "um\nDOIS\n").expect("w");
    let st = core.git_status().expect("st");
    assert_eq!(paths(&st), [("a b.txt".into(), '.', 'M', EntryKind::Ordinary)]);
    let d = core.git_diff(&st.entries[0], false).expect("diff");
    let changed: Vec<(Option<u32>, Option<u32>, &str)> = d
        .lines
        .iter()
        .filter(|l| l.text.starts_with(['+', '-']) && !l.text.starts_with("+++") && !l.text.starts_with("---"))
        .map(|l| (l.old, l.new, l.text.as_str()))
        .collect();
    assert_eq!(changed, [(Some(2), None, "-dois"), (None, Some(2), "+DOIS")]);
    core.git_discard(&st.entries[0]).expect("discard");
    assert!(core.git_status().expect("st").entries.is_empty());
    assert_eq!(std::fs::read_to_string(ws.join("a b.txt")).expect("r"), "um\ndois\n");

    // Descartar um arquivo não rastreado o apaga.
    std::fs::write(ws.join("lixo.tmp"), "x").expect("w");
    let st = core.git_status().expect("st");
    core.git_discard(&st.entries[0]).expect("discard untracked");
    assert!(!ws.join("lixo.tmp").exists());

    // Branches: criar, listar, trocar; nomes inválidos são recusados.
    core.git_create_branch("feat/x", true).expect("branch");
    assert_eq!(core.git_status().expect("st").branch.as_deref(), Some("feat/x"));
    assert!(core.git_create_branch("-f", true).is_err());
    let branches = core.git_branches().expect("branches");
    assert_eq!(branches.iter().map(|b| (b.name.as_str(), b.current)).collect::<Vec<_>>(), [("feat/x", true), ("main", false)]);

    // Renomeado aparece com o nome antigo.
    assert!(git(ws, &["mv", "a b.txt", "novo nome.txt"]).status.success());
    let st = core.git_status().expect("st");
    assert_eq!(st.entries[0].kind, EntryKind::Renamed);
    assert_eq!((st.entries[0].path.as_str(), st.entries[0].orig_path.as_deref()), ("novo nome.txt", Some("a b.txt")));
    core.git_unstage_all().expect("unstage all");
    assert!(core.git_status().expect("st").entries.iter().all(|e| !e.staged()));
    let _ = git(ws, &["reset", "-q", "--hard"]);
    let _ = git(ws, &["clean", "-fdq"]);

    // Conflito de mesclagem.
    std::fs::write(ws.join("a b.txt"), "um\nfeat\n").expect("w");
    core.git_stage_all().expect("stage");
    core.git_commit("na feat", false).expect("commit");
    let main = branches.iter().find(|b| b.name == "main").expect("main").clone();
    core.git_checkout(&main).expect("checkout main");
    std::fs::write(ws.join("a b.txt"), "um\nmain\n").expect("w");
    core.git_stage_all().expect("stage");
    core.git_commit("na main", false).expect("commit");
    assert!(!git(ws, &["merge", "feat/x"]).status.success(), "o merge deve conflitar");
    let st = core.git_status().expect("st");
    assert_eq!(st.state, RepoState::Merging);
    assert_eq!(paths(&st), [("a b.txt".into(), 'U', 'U', EntryKind::Unmerged)]);
    env.core.shutdown();
}

#[test]
fn push_pull_and_ahead_behind_with_a_local_remote() {
    need_git!();
    let env = start(true);
    let (core, ws) = (&env.core, &env.ws);
    let bare = env.root.join("remoto.git");
    assert!(git(&env.root, &["init", "-q", "--bare", bare.to_str().expect("utf8")]).status.success());
    core.git_init().expect("init");
    std::fs::write(ws.join("x.txt"), "1\n").expect("w");
    core.git_stage_all().expect("stage");
    core.git_commit("c1", false).expect("commit");
    assert!(git(ws, &["remote", "add", "origin", bare.to_str().expect("utf8")]).status.success());

    // Sem upstream: publica no único remoto.
    core.git_push().expect("push -u");
    let st = core.git_status().expect("st");
    assert_eq!((st.upstream.as_deref(), st.ahead, st.behind), (Some("origin/main"), 0, 0));

    std::fs::write(ws.join("x.txt"), "2\n").expect("w");
    core.git_stage_all().expect("stage");
    core.git_commit("c2", false).expect("commit");
    assert_eq!(core.git_status().expect("st").ahead, 1);
    core.git_push().expect("push");
    assert_eq!(core.git_status().expect("st").ahead, 0);

    // Outra cópia envia um commit; aqui: fetch mostra 1 atrás, pull traz.
    let other = env.root.join("outra");
    assert!(git(&env.root, &["clone", "-q", bare.to_str().expect("utf8"), other.to_str().expect("utf8")]).status.success());
    std::fs::write(other.join("y.txt"), "y\n").expect("w");
    assert!(git(&other, &["add", "-A"]).status.success());
    assert!(git(&other, &["commit", "-qm", "de fora"]).status.success());
    assert!(git(&other, &["push", "-q"]).status.success());
    core.git_fetch().expect("fetch");
    assert_eq!(core.git_status().expect("st").behind, 1);
    core.git_pull().expect("pull");
    let st = core.git_status().expect("st");
    assert_eq!((st.ahead, st.behind), (0, 0));
    assert!(ws.join("y.txt").exists());
    assert!(core.git_branches().expect("b").iter().any(|b| b.remote && b.name == "origin/main"));
    env.core.shutdown();
}
