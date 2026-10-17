//! Integração com o git real (pula com aviso se o `git` não estiver instalado). A configuração do
//! usuário é isolada: `GIT_CONFIG_GLOBAL` aponta para um arquivo do teste.

use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::OnceLock;

use commanddeck_core::git::{extract_hunk_patch, ConflictChoice, EntryKind, GitError, RepoState};
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
        config: Some(Config::with_project(paths.default_workspace.clone())),
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
    // Sem HEAD e com o índice vazio, tirar tudo do stage não é erro.
    core.git_unstage_all().expect("unstage all com índice vazio");
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

    // Descartar um arquivo não rastreado o apaga; pasta não rastreada com `.git` interno também é apagada (`-ffd`).
    std::fs::write(ws.join("lixo.tmp"), "x").expect("w");
    let st = core.git_status().expect("st");
    core.git_discard(&st.entries[0]).expect("discard untracked");
    assert!(!ws.join("lixo.tmp").exists());

    let sub = ws.join("subrepo");
    std::fs::create_dir_all(&sub).expect("mkdir sub");
    assert!(git(&sub, &["init", "-q"]).status.success());
    std::fs::write(sub.join("f.txt"), "1").expect("w");
    let st = core.git_status().expect("st");
    assert_eq!(st.entries.len(), 1);
    assert_eq!(st.entries[0].kind, EntryKind::Untracked);
    core.git_discard(&st.entries[0]).expect("discard untracked nested .git dir");
    assert!(!sub.exists());

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
    // Tirar do stage só o nome novo leva junto o antigo (que não fica preparado como apagado).
    core.git_unstage(&["novo nome.txt".into()]).expect("unstage renomeado");
    let st = core.git_status().expect("st");
    assert_eq!(paths(&st), [("a b.txt".into(), '.', 'D', EntryKind::Ordinary), ("novo nome.txt".into(), '?', '?', EntryKind::Untracked)]);
    core.git_stage_all().expect("stage");
    assert_eq!(core.git_status().expect("st").entries[0].kind, EntryKind::Renamed);
    core.git_unstage_all().expect("unstage all");
    assert!(core.git_status().expect("st").entries.iter().all(|e| !e.staged()));
    let _ = git(ws, &["reset", "-q", "--hard"]);
    let _ = git(ws, &["clean", "-fdq"]);

    // Conflito de mesclagem e descarte de arquivo em conflito (`Unmerged`).
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
    // Descartar um arquivo `Unmerged` restaura ao HEAD e tira do conflito.
    core.git_discard(&st.entries[0]).expect("discard unmerged");
    assert!(core.git_status().expect("st").entries.is_empty());
    assert_eq!(std::fs::read_to_string(ws.join("a b.txt")).expect("r"), "um\nmain\n");
    core.git_abort_op().expect("abort merge");
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

#[test]
fn checkout_of_a_remote_branch_reuses_the_local_one() {
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
    // `feat` só existe no remoto.
    assert!(git(ws, &["push", "-q", "origin", "main", "main:feat"]).status.success());
    core.git_fetch().expect("fetch");
    let remote = |name: &str| core.git_branches().expect("b").into_iter().find(|b| b.remote && b.name == name).expect("remota");

    // Primeira vez: cria a local que acompanha a remota.
    core.git_checkout(&remote("origin/feat")).expect("checkout origin/feat");
    let st = core.git_status().expect("st");
    assert_eq!((st.branch.as_deref(), st.upstream.as_deref()), (Some("feat"), Some("origin/feat")));

    // De volta à main; de novo na remota: a local `feat` já existe, então só troca para ela.
    let main = core.git_branches().expect("b").into_iter().find(|b| !b.remote && b.name == "main").expect("main");
    core.git_checkout(&main).expect("checkout main");
    core.git_checkout(&remote("origin/feat")).expect("checkout origin/feat de novo");
    assert_eq!(core.git_status().expect("st").branch.as_deref(), Some("feat"));
    env.core.shutdown();
}

#[test]
fn hunk_stage_unstage_and_discard_with_real_git() {
    need_git!();
    let env = start(true);
    let (core, ws) = (&env.core, &env.ws);
    let _ = std::fs::remove_file(ws.join("AGENTS.md"));
    core.git_init().expect("init");
    let initial = (1..=12).map(|i| format!("linha {i}\n")).collect::<String>();
    std::fs::write(ws.join("f.txt"), &initial).expect("w");
    core.git_stage_all().expect("stage");
    core.git_commit("inicial", false).expect("commit");

    // Altera o início e o fim do arquivo (dois hunks separados).
    let modified = initial.replace("linha 1\n", "LINHA 1\n").replace("linha 12\n", "LINHA 12\n");
    std::fs::write(ws.join("f.txt"), &modified).expect("w");
    let st = core.git_status().expect("st");
    let diff = core.git_diff(&st.entries[0], false).expect("diff");
    let p0 = extract_hunk_patch(&diff, 0).expect("hunk 0");
    let p1 = extract_hunk_patch(&diff, 1).expect("hunk 1");

    // Prepara apenas o primeiro hunk: o arquivo fica em Staged e em Changes ao mesmo tempo.
    core.git_apply_hunk(&p0, false, false).expect("stage hunk 0");
    let st = core.git_status().expect("st");
    assert_eq!(paths(&st), [("f.txt".into(), 'M', 'M', EntryKind::Ordinary)]);

    // Tira o primeiro hunk do stage: volta a ficar só não preparado.
    let staged_diff = core.git_diff(&st.entries[0], true).expect("staged diff");
    let sp0 = extract_hunk_patch(&staged_diff, 0).expect("staged hunk 0");
    core.git_apply_hunk(&sp0, true, false).expect("unstage hunk 0");
    let st = core.git_status().expect("st");
    assert_eq!(paths(&st), [("f.txt".into(), '.', 'M', EntryKind::Ordinary)]);

    // Descarta apenas o segundo hunk na cópia de trabalho: sobra só a alteração da linha 1.
    core.git_apply_hunk(&p1, false, true).expect("discard hunk 1");
    let content = std::fs::read_to_string(ws.join("f.txt")).expect("read");
    assert!(content.starts_with("LINHA 1\n") && content.ends_with("linha 12\n"), "{content}");
    env.core.shutdown();
}

#[test]
fn stash_log_merge_rebase_and_conflict_resolution() {
    need_git!();
    let env = start(true);
    let (core, ws) = (&env.core, &env.ws);
    let _ = std::fs::remove_file(ws.join("AGENTS.md"));
    core.git_init().expect("init");
    std::fs::write(ws.join("a.txt"), "base 1\n\n\n\n\nbase 2\n").expect("w");
    core.git_stage_all().expect("stage");
    core.git_commit("c1: base", false).expect("c1");

    // Stash: guardar (incluindo não rastreados), listar, ver diff, aplicar e apagar.
    std::fs::write(ws.join("a.txt"), "base 1\nmodificado\n").expect("w");
    std::fs::write(ws.join("novo.txt"), "rascunho\n").expect("w");
    core.git_stash_push("meu rascunho", true).expect("stash push");
    assert_eq!(core.git_status().expect("st").stash_count, 1);
    assert!(!ws.join("novo.txt").exists());
    let stashes = core.git_stash_list().expect("stash list");
    assert_eq!(stashes.len(), 1);
    assert!(stashes[0].message.contains("meu rascunho"), "{:?}", stashes[0]);
    let sdiff = core.git_stash_diff(0).expect("stash diff");
    assert!(sdiff.lines.iter().any(|l| l.text.contains("modificado")));
    core.git_stash_apply(0).expect("stash apply");
    assert!(ws.join("novo.txt").exists());
    core.git_stash_drop(0).expect("stash drop");
    assert!(core.git_stash_list().expect("stash list").is_empty());
    // Limpa o que foi aplicado do stash.
    let _ = git(ws, &["reset", "-q", "--hard"]);
    let _ = git(ws, &["clean", "-fdq"]);

    // Cria branch `feat` com commit conflitante.
    core.git_create_branch("feat", true).expect("create feat");
    std::fs::write(ws.join("a.txt"), "deles 1\n\n\n\n\ndeles 2\n").expect("w");
    core.git_stage_all().expect("stage");
    core.git_commit("c2: feat", false).expect("c2");

    // Volta para `main` e faz outro commit.
    let main = core.git_branches().expect("b").into_iter().find(|b| b.name == "main").expect("main");
    core.git_checkout(&main).expect("checkout main");
    std::fs::write(ws.join("a.txt"), "nosso 1\n\n\n\n\nnosso 2\n").expect("w");
    core.git_stage_all().expect("stage");
    core.git_commit("c3: main", false).expect("c3");

    // Histórico paginado e detalhe do commit.
    let (log1, more1) = core.git_log(0, 1).expect("log page 1");
    assert_eq!(log1.len(), 1);
    assert!(more1);
    assert_eq!(log1[0].subject, "c3: main");
    let (log2, more2) = core.git_log(1, 5).expect("log page 2");
    assert_eq!(log2.len(), 1);
    assert!(!more2);
    assert_eq!(log2[0].subject, "c1: base");
    let detail = core.git_commit_detail(&log1[0].hash).expect("detail");
    assert_eq!(detail.files, [('M', "a.txt".into())]);
    assert!(detail.diff.lines.iter().any(|l| l.text == "+nosso 1"));

    // Merge com conflito → resolver por blocos → continuar merge.
    core.git_merge("feat").expect("merge feat into main");
    assert_eq!(core.git_status().expect("st").state, RepoState::Merging);
    let cf = core.git_conflict_file("a.txt").expect("conflict file");
    assert_eq!(cf.blocks.len(), 2);
    core.git_resolve_conflict_block("a.txt", 0, ConflictChoice::Ours).expect("resolve block 0");
    let cf = core.git_conflict_file("a.txt").expect("conflict file after block 0");
    assert_eq!(cf.blocks.len(), 1);
    core.git_resolve_conflict_block("a.txt", 0, ConflictChoice::Theirs).expect("resolve block 1");
    assert!(core.git_conflict_file("a.txt").expect("cf").blocks.is_empty());
    core.git_stage(&["a.txt".into()]).expect("stage resolved");
    core.git_continue_op().expect("merge continue");
    assert_eq!(core.git_status().expect("st").state, RepoState::Normal);
    assert_eq!(std::fs::read_to_string(ws.join("a.txt")).expect("r"), "nosso 1\n\n\n\n\ndeles 2\n");

    // Rebase com conflito → resolver o arquivo inteiro (`--theirs`) → abortar.
    let feat = core.git_branches().expect("b").into_iter().find(|b| b.name == "feat").expect("feat");
    core.git_checkout(&feat).expect("checkout feat");
    std::fs::write(ws.join("a.txt"), "outro conflito\n").expect("w");
    core.git_stage_all().expect("stage");
    core.git_commit("c4: feat2", false).expect("c4");
    core.git_rebase("main").expect("rebase onto main");
    assert_eq!(core.git_status().expect("st").state, RepoState::Rebasing);
    core.git_resolve_conflict_file("a.txt", ConflictChoice::Ours).expect("resolve file ours");
    core.git_abort_op().expect("rebase abort");
    assert_eq!(core.git_status().expect("st").state, RepoState::Normal);
    env.core.shutdown();
}

#[test]
fn gpgsign_without_agent_fails_fast_with_explanation() {
    need_git!();
    let env = start(true);
    let (core, ws) = (&env.core, &env.ws);
    let _ = std::fs::remove_file(ws.join("AGENTS.md"));
    core.git_init().expect("init");
    std::fs::write(ws.join("a.txt"), "1\n").expect("w");
    core.git_stage_all().expect("stage");
    // Ativa `commit.gpgsign` apontando para um programa inexistente/sem agente: deve falhar rápido
    // (< 5 s, nunca esperar os 120 s de COMMIT_TIMEOUT) e orientar o usuário sem desligar a opção.
    assert!(git(ws, &["config", "commit.gpgsign", "true"]).status.success());
    assert!(git(ws, &["config", "gpg.program", "gpg-inexistente-commanddeck"]).status.success());
    let t0 = std::time::Instant::now();
    let err = core.git_commit("assinado", false).unwrap_err();
    assert!(t0.elapsed() < std::time::Duration::from_secs(10), "demorou {:?}", t0.elapsed());
    assert!(err.to_string().contains("commit.gpgsign"), "{err}");
    // A configuração do repositório permanece intacta.
    let cfg = git(ws, &["config", "--bool", "--get", "commit.gpgsign"]);
    assert_eq!(String::from_utf8_lossy(&cfg.stdout).trim(), "true");
    env.core.shutdown();
}

#[test]
fn conflict_variants_diff_cc_stash_pop_and_rebase_continue() {
    need_git!();
    let env = start(true);
    let (core, ws) = (&env.core, &env.ws);
    let _ = std::fs::remove_file(ws.join("AGENTS.md"));
    core.git_init().expect("init");
    std::fs::write(ws.join("a.txt"), "l1\nl2\nl3\n").expect("w");
    std::fs::write(ws.join("b.txt"), "b\n").expect("w");
    std::fs::write(ws.join("c.txt"), "c\n").expect("w");
    core.git_stage_all().expect("stage");
    core.git_commit("base", false).expect("base");

    // Sem alterações: guardar no stash é um erro claro; sem `-u`, o não rastreado fica.
    assert!(core.git_stash_push("nada", false).is_err());
    std::fs::write(ws.join("b.txt"), "b2\n").expect("w");
    std::fs::write(ws.join("solto.txt"), "x\n").expect("w");
    core.git_stash_push("", false).expect("stash sem mensagem");
    assert!(ws.join("solto.txt").exists());
    assert_eq!(std::fs::read_to_string(ws.join("b.txt")).expect("r"), "b\n");
    let list = core.git_stash_list().expect("list");
    assert_eq!(list.len(), 1);
    core.git_stash_pop(0).expect("pop");
    assert!(core.git_stash_list().expect("list").is_empty());
    assert_eq!(std::fs::read_to_string(ws.join("b.txt")).expect("r"), "b2\n");
    let _ = git(ws, &["reset", "-q", "--hard"]);
    let _ = git(ws, &["clean", "-fdq"]);

    // Branch `feat`: muda a.txt, apaga b.txt, muda c.txt.
    core.git_create_branch("feat", true).expect("feat");
    std::fs::write(ws.join("a.txt"), "l1\nFEAT\nl3\n").expect("w");
    std::fs::remove_file(ws.join("b.txt")).expect("rm");
    std::fs::write(ws.join("c.txt"), "c-feat\n").expect("w");
    core.git_stage_all().expect("stage");
    core.git_commit("feat", false).expect("feat commit");
    let main = core.git_branches().expect("b").into_iter().find(|b| b.name == "main").expect("main");
    core.git_checkout(&main).expect("main");
    std::fs::write(ws.join("a.txt"), "l1\nMAIN\nl3\n").expect("w");
    std::fs::write(ws.join("b.txt"), "b-main\n").expect("w");
    std::fs::write(ws.join("c.txt"), "c-main\n").expect("w");
    core.git_stage_all().expect("stage");
    core.git_commit("main", false).expect("main commit");

    core.git_merge("feat").expect("merge devolve Ok com conflitos");
    let st = core.git_status().expect("st");
    assert_eq!(st.state, RepoState::Merging);
    let kinds: Vec<(String, char, char)> = st.entries.iter().map(|e| (e.path.clone(), e.index, e.worktree)).collect();
    assert_eq!(kinds, [("a.txt".into(), 'U', 'U'), ("b.txt".into(), 'U', 'D'), ("c.txt".into(), 'U', 'U')]);

    // O diff de um arquivo em conflito (`diff --cc`, `@@@`) tem números de linha e nada de crash.
    let d = core.git_diff(&st.entries[0], false).expect("diff --cc");
    assert!(d.lines.iter().any(|l| l.kind == commanddeck_core::git::DiffKind::Hunk && l.text.starts_with("@@@")));
    assert!(d.lines.iter().any(|l| l.kind == commanddeck_core::git::DiffKind::Add && l.text.contains("FEAT")));
    assert!(extract_hunk_patch(&d, 0).is_none(), "diff combinado não vira patch de trecho");

    // a.txt: "ambos"; c.txt: "deles" (arquivo inteiro); b.txt (modify/delete): "deles" = apagar.
    core.git_resolve_conflict_file("a.txt", ConflictChoice::Both).expect("both");
    assert_eq!(std::fs::read_to_string(ws.join("a.txt")).expect("r"), "l1\nMAIN\nFEAT\nl3\n");
    core.git_resolve_conflict_file("c.txt", ConflictChoice::Theirs).expect("theirs");
    assert_eq!(std::fs::read_to_string(ws.join("c.txt")).expect("r"), "c-feat\n");
    core.git_resolve_conflict_file("b.txt", ConflictChoice::Theirs).expect("theirs delete");
    assert!(!ws.join("b.txt").exists());
    let st = core.git_status().expect("st");
    assert!(st.entries.iter().all(|e| e.kind != EntryKind::Unmerged), "{:?}", paths(&st));
    core.git_continue_op().expect("merge continue");
    assert_eq!(core.git_status().expect("st").state, RepoState::Normal);

    // Descartar um conflito modify/delete (o outro lado apagou) também funciona.
    let m2 = core.git_log(0, 1).expect("log").0[0].hash.clone();
    assert!(git(ws, &["reset", "-q", "--hard", &format!("{m2}~1")]).status.success());
    assert!(!git(ws, &["merge", "feat"]).status.success());
    let st = core.git_status().expect("st");
    let b = st.entries.iter().find(|e| e.path == "b.txt").expect("b.txt em conflito");
    assert_eq!(b.kind, EntryKind::Unmerged);
    core.git_discard(b).expect("discard modify/delete");
    assert!(core.git_status().expect("st").entries.iter().all(|e| e.path != "b.txt"));
    core.git_abort_op().expect("abort");
    assert_eq!(core.git_status().expect("st").state, RepoState::Normal);

    // Rebase com conflito: resolver e continuar.
    core.git_create_branch("feat2", true).expect("feat2");
    std::fs::write(
        ws.join("a.txt"),
        "l1
FEAT2
l3
",
    )
    .expect("w");
    core.git_stage_all().expect("stage");
    core.git_commit("feat2", false).expect("feat2 commit");
    core.git_checkout(&main).expect("main");
    std::fs::write(
        ws.join("a.txt"),
        "l1
MAIN2
l3
",
    )
    .expect("w");
    core.git_stage_all().expect("stage");
    core.git_commit("main2", false).expect("main2");
    let feat2 = core.git_branches().expect("b").into_iter().find(|b| b.name == "feat2").expect("feat2");
    core.git_checkout(&feat2).expect("feat2");
    let msg = core.git_rebase("main").expect("rebase");
    assert!(msg.to_lowercase().contains("conflito"), "{msg}");
    assert_eq!(core.git_status().expect("st").state, RepoState::Rebasing);
    core.git_resolve_conflict_file("a.txt", ConflictChoice::Theirs).expect("resolve");
    core.git_continue_op().expect("rebase continue");
    assert_eq!(core.git_status().expect("st").state, RepoState::Normal);
    assert_eq!(
        std::fs::read_to_string(ws.join("a.txt")).expect("r"),
        "l1
FEAT2
l3
"
    );
    assert!(core.git_continue_op().is_err(), "sem operação em andamento");
    env.core.shutdown();
}

#[test]
fn hunks_work_in_crlf_files() {
    need_git!();
    let env = start(true);
    let (core, ws) = (&env.core, &env.ws);
    let _ = std::fs::remove_file(ws.join("AGENTS.md"));
    core.git_init().expect("init");
    // Sem conversão de fim de linha: o CRLF vai como está para o repositório.
    assert!(git(ws, &["config", "core.autocrlf", "false"]).status.success());
    let initial = (1..=12).map(|i| format!("linha {i}\r\n")).collect::<String>();
    std::fs::write(ws.join("w.txt"), &initial).expect("w");
    core.git_stage_all().expect("stage");
    core.git_commit("crlf", false).expect("commit");
    let modified = initial.replace("linha 1\r\n", "LINHA 1\r\n").replace("linha 12\r\n", "LINHA 12\r\n");
    std::fs::write(ws.join("w.txt"), &modified).expect("w");
    let st = core.git_status().expect("st");
    let diff = core.git_diff(&st.entries[0], false).expect("diff");
    let p0 = extract_hunk_patch(&diff, 0).expect("hunk 0");
    let p1 = extract_hunk_patch(&diff, 1).expect("hunk 1");
    core.git_apply_hunk(&p0, false, false).expect("stage hunk 0 (CRLF)");
    assert_eq!(paths(&core.git_status().expect("st")), [("w.txt".into(), 'M', 'M', EntryKind::Ordinary)]);
    core.git_apply_hunk(&p1, false, true).expect("discard hunk 1 (CRLF)");
    let content = std::fs::read_to_string(ws.join("w.txt")).expect("r");
    assert!(content.starts_with("LINHA 1\r\n") && content.ends_with("linha 12\r\n"), "{content:?}");
    env.core.shutdown();
}
