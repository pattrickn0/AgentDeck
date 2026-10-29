//! Integration with the real git (skipped with a warning if `git` is not installed). The user's
//! configuration is isolated: `GIT_CONFIG_GLOBAL` points to a file of the test.

use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::OnceLock;

use holt_core::git::{extract_hunk_patch, ConflictChoice, EntryKind, GitError, RepoState};
use holt_core::paths::AppPaths;
use holt_core::{Config, Core, StartOptions};

/// Folder of the isolated global configuration (created once for the test process).
fn isolated_config() -> &'static Path {
    static DIR: OnceLock<PathBuf> = OnceLock::new();
    DIR.get_or_init(|| {
        let dir = tempfile::tempdir().expect("tmp").keep();
        std::fs::write(
            dir.join("gitconfig"),
            "[user]\n\tname = Tester\n\temail = tester@example.com\n[init]\n\tdefaultBranch = main\n[commit]\n\tgpgsign = false\n",
        )
        .expect("gitconfig");
        // Variables are only read by git (children of this process); set before any core.
        std::env::set_var("GIT_CONFIG_GLOBAL", dir.join("gitconfig"));
        std::env::set_var("GIT_CONFIG_NOSYSTEM", "1");
        // Git does not climb above the temporary folder looking for a repository.
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

/// Core with the workspace in `<tmp>/ws` and git in the search PATH (`with_git = false`: no git).
fn start(with_git: bool) -> Env {
    isolated_config();
    let tmp = tempfile::tempdir().expect("tmp");
    let root = tmp.path().to_path_buf();
    let paths = AppPaths {
        app_dir: root.clone(),
        bin_dir: root.join("bin"),
        data_dir: root.join("data"),
        default_workspace: root.join("ws"),
        whisper_dir: root.join("whisper"),
        config_file: root.join("cfg").join("config.json"),
    };
    let search = if with_git { git_exe().and_then(|g| g.parent().map(Path::to_path_buf)).unwrap_or_default() } else { root.join("empty") };
    let opts = StartOptions {
        search_path: Some(search.into_os_string()),
        home_dir: Some(root.join("home")),
        watch_workspace: false,
        config: Some(Config::with_project(paths.default_workspace.clone())),
        system_dirs: false,
    };
    let (core, _rx) = Core::start_with(paths, opts).expect("start");
    let ws = root.join("ws");
    Env { _tmp: tmp, core, ws, root }
}

/// Runs git directly (setting up the scenario).
fn git(dir: &Path, args: &[&str]) -> std::process::Output {
    Command::new(git_exe().expect("git")).current_dir(dir).args(args).output().expect("git")
}

macro_rules! need_git {
    () => {
        if git_exe().is_none() {
            eprintln!("git not installed: test skipped");
            return;
        }
    };
}

fn paths(st: &holt_core::git::GitStatus) -> Vec<(String, char, char, EntryKind)> {
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
    // The app writes AGENTS.md to the workspace on start; here it would only get in the way of the counts.
    let _ = std::fs::remove_file(ws.join("AGENTS.md"));
    core.git_init().expect("init");
    // With no HEAD and an empty index, unstaging everything is not an error.
    core.git_unstage_all().expect("unstage all with an empty index");
    std::fs::write(ws.join("a b.txt"), "one\ntwo\n").expect("w");
    std::fs::create_dir_all(ws.join("naïve")).expect("mkdir");
    std::fs::write(ws.join("naïve").join("ü.txt"), "x\n").expect("w");

    let st = core.git_status().expect("status");
    assert_eq!((st.branch.as_deref(), st.head.as_deref(), st.prefix.as_str()), (Some("main"), None, ""));
    assert_eq!(paths(&st), [("a b.txt".into(), '?', '?', EntryKind::Untracked), ("naïve/".into(), '?', '?', EntryKind::Untracked)]);

    // Diff of an untracked file: the whole content.
    let d = core.git_diff(&st.entries[0], false).expect("diff");
    assert_eq!(d.lines.iter().map(|l| l.text.as_str()).collect::<Vec<_>>(), ["one", "two"]);

    // Stage of one file, unstage (no HEAD yet), stage everything.
    core.git_stage(&["a b.txt".into()]).expect("stage");
    assert_eq!(paths(&core.git_status().expect("st"))[0], ("a b.txt".into(), 'A', '.', EntryKind::Ordinary));
    core.git_unstage(&["a b.txt".into()]).expect("unstage with no HEAD");
    assert_eq!(paths(&core.git_status().expect("st"))[0].1, '?');
    core.git_stage_all().expect("stage all");
    let st = core.git_status().expect("st");
    assert!(st.entries.iter().all(|e| e.index == 'A'), "{:?}", paths(&st));
    let staged = core.git_diff(&st.entries[0], true).expect("diff staged");
    assert!(staged.lines.iter().any(|l| l.text == "+one"));

    // Commit, and the last message comes back for "amend last commit".
    let summary = core.git_commit("first commit\n\nbody", false).expect("commit");
    assert!(summary.ends_with(" first commit"), "{summary}");
    assert_eq!(core.git_last_message().as_deref(), Some("first commit\n\nbody"));
    let st = core.git_status().expect("st");
    assert!(st.head.is_some() && st.entries.is_empty());
    assert!(core.git_commit("  ", false).is_err(), "an empty message is refused");

    // Unstaged change, numbered diff, discard.
    std::fs::write(ws.join("a b.txt"), "one\nTWO\n").expect("w");
    let st = core.git_status().expect("st");
    assert_eq!(paths(&st), [("a b.txt".into(), '.', 'M', EntryKind::Ordinary)]);
    let d = core.git_diff(&st.entries[0], false).expect("diff");
    let changed: Vec<(Option<u32>, Option<u32>, &str)> = d
        .lines
        .iter()
        .filter(|l| l.text.starts_with(['+', '-']) && !l.text.starts_with("+++") && !l.text.starts_with("---"))
        .map(|l| (l.old, l.new, l.text.as_str()))
        .collect();
    assert_eq!(changed, [(Some(2), None, "-two"), (None, Some(2), "+TWO")]);
    core.git_discard(&st.entries[0]).expect("discard");
    assert!(core.git_status().expect("st").entries.is_empty());
    assert_eq!(std::fs::read_to_string(ws.join("a b.txt")).expect("r"), "one\ntwo\n");

    // Discarding an untracked file deletes it; an untracked folder with an inner `.git` is deleted too (`-ffd`).
    std::fs::write(ws.join("junk.tmp"), "x").expect("w");
    let st = core.git_status().expect("st");
    core.git_discard(&st.entries[0]).expect("discard untracked");
    assert!(!ws.join("junk.tmp").exists());

    let sub = ws.join("subrepo");
    std::fs::create_dir_all(&sub).expect("mkdir sub");
    assert!(git(&sub, &["init", "-q"]).status.success());
    std::fs::write(sub.join("f.txt"), "1").expect("w");
    let st = core.git_status().expect("st");
    assert_eq!(st.entries.len(), 1);
    assert_eq!(st.entries[0].kind, EntryKind::Untracked);
    core.git_discard(&st.entries[0]).expect("discard untracked nested .git dir");
    assert!(!sub.exists());

    // Branches: create, list, switch; invalid names are refused.
    core.git_create_branch("feat/x", true).expect("branch");
    assert_eq!(core.git_status().expect("st").branch.as_deref(), Some("feat/x"));
    assert!(core.git_create_branch("-f", true).is_err());
    let branches = core.git_branches().expect("branches");
    assert_eq!(branches.iter().map(|b| (b.name.as_str(), b.current)).collect::<Vec<_>>(), [("feat/x", true), ("main", false)]);

    // A rename appears with the old name.
    assert!(git(ws, &["mv", "a b.txt", "new name.txt"]).status.success());
    let st = core.git_status().expect("st");
    assert_eq!(st.entries[0].kind, EntryKind::Renamed);
    assert_eq!((st.entries[0].path.as_str(), st.entries[0].orig_path.as_deref()), ("new name.txt", Some("a b.txt")));
    // Unstaging only the new name takes the old one along (it does not stay staged as deleted).
    core.git_unstage(&["new name.txt".into()]).expect("unstage renamed");
    let st = core.git_status().expect("st");
    assert_eq!(paths(&st), [("a b.txt".into(), '.', 'D', EntryKind::Ordinary), ("new name.txt".into(), '?', '?', EntryKind::Untracked)]);
    core.git_stage_all().expect("stage");
    assert_eq!(core.git_status().expect("st").entries[0].kind, EntryKind::Renamed);
    core.git_unstage_all().expect("unstage all");
    assert!(core.git_status().expect("st").entries.iter().all(|e| !e.staged()));
    let _ = git(ws, &["reset", "-q", "--hard"]);
    let _ = git(ws, &["clean", "-fdq"]);

    // Merge conflict and discard of a file in conflict (`Unmerged`).
    std::fs::write(ws.join("a b.txt"), "one\nfeat\n").expect("w");
    core.git_stage_all().expect("stage");
    core.git_commit("on feat", false).expect("commit");
    let main = branches.iter().find(|b| b.name == "main").expect("main").clone();
    core.git_checkout(&main).expect("checkout main");
    std::fs::write(ws.join("a b.txt"), "one\nmain\n").expect("w");
    core.git_stage_all().expect("stage");
    core.git_commit("on main", false).expect("commit");
    assert!(!git(ws, &["merge", "feat/x"]).status.success(), "the merge must conflict");
    let st = core.git_status().expect("st");
    assert_eq!(st.state, RepoState::Merging);
    assert_eq!(paths(&st), [("a b.txt".into(), 'U', 'U', EntryKind::Unmerged)]);
    // Discarding an `Unmerged` file restores it to HEAD and takes it out of the conflict.
    core.git_discard(&st.entries[0]).expect("discard unmerged");
    assert!(core.git_status().expect("st").entries.is_empty());
    assert_eq!(std::fs::read_to_string(ws.join("a b.txt")).expect("r"), "one\nmain\n");
    core.git_abort_op().expect("abort merge");
    env.core.shutdown();
}

#[test]
fn push_pull_and_ahead_behind_with_a_local_remote() {
    need_git!();
    let env = start(true);
    let (core, ws) = (&env.core, &env.ws);
    let bare = env.root.join("remote.git");
    assert!(git(&env.root, &["init", "-q", "--bare", bare.to_str().expect("utf8")]).status.success());
    core.git_init().expect("init");
    std::fs::write(ws.join("x.txt"), "1\n").expect("w");
    core.git_stage_all().expect("stage");
    core.git_commit("c1", false).expect("commit");
    assert!(git(ws, &["remote", "add", "origin", bare.to_str().expect("utf8")]).status.success());

    // With no upstream: publishes to the only remote.
    core.git_push().expect("push -u");
    let st = core.git_status().expect("st");
    assert_eq!((st.upstream.as_deref(), st.ahead, st.behind), (Some("origin/main"), 0, 0));

    std::fs::write(ws.join("x.txt"), "2\n").expect("w");
    core.git_stage_all().expect("stage");
    core.git_commit("c2", false).expect("commit");
    assert_eq!(core.git_status().expect("st").ahead, 1);
    core.git_push().expect("push");
    assert_eq!(core.git_status().expect("st").ahead, 0);

    // Another copy pushes a commit; here: fetch shows 1 behind, pull brings it.
    let other = env.root.join("other");
    assert!(git(&env.root, &["clone", "-q", bare.to_str().expect("utf8"), other.to_str().expect("utf8")]).status.success());
    std::fs::write(other.join("y.txt"), "y\n").expect("w");
    assert!(git(&other, &["add", "-A"]).status.success());
    assert!(git(&other, &["commit", "-qm", "from outside"]).status.success());
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
    let bare = env.root.join("remote.git");
    assert!(git(&env.root, &["init", "-q", "--bare", bare.to_str().expect("utf8")]).status.success());
    core.git_init().expect("init");
    std::fs::write(ws.join("x.txt"), "1\n").expect("w");
    core.git_stage_all().expect("stage");
    core.git_commit("c1", false).expect("commit");
    assert!(git(ws, &["remote", "add", "origin", bare.to_str().expect("utf8")]).status.success());
    // `feat` only exists on the remote.
    assert!(git(ws, &["push", "-q", "origin", "main", "main:feat"]).status.success());
    core.git_fetch().expect("fetch");
    let remote = |name: &str| core.git_branches().expect("b").into_iter().find(|b| b.remote && b.name == name).expect("remote");

    // First time: creates the local branch that tracks the remote one.
    core.git_checkout(&remote("origin/feat")).expect("checkout origin/feat");
    let st = core.git_status().expect("st");
    assert_eq!((st.branch.as_deref(), st.upstream.as_deref()), (Some("feat"), Some("origin/feat")));

    // Back to main; on the remote one again: the local `feat` already exists, so it only switches to it.
    let main = core.git_branches().expect("b").into_iter().find(|b| !b.remote && b.name == "main").expect("main");
    core.git_checkout(&main).expect("checkout main");
    core.git_checkout(&remote("origin/feat")).expect("checkout origin/feat again");
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
    let initial = (1..=12).map(|i| format!("line {i}\n")).collect::<String>();
    std::fs::write(ws.join("f.txt"), &initial).expect("w");
    core.git_stage_all().expect("stage");
    core.git_commit("initial", false).expect("commit");

    // Changes the start and the end of the file (two separate hunks).
    let modified = initial.replace("line 1\n", "LINE 1\n").replace("line 12\n", "LINE 12\n");
    std::fs::write(ws.join("f.txt"), &modified).expect("w");
    let st = core.git_status().expect("st");
    let diff = core.git_diff(&st.entries[0], false).expect("diff");
    let p0 = extract_hunk_patch(&diff, 0).expect("hunk 0");
    let p1 = extract_hunk_patch(&diff, 1).expect("hunk 1");

    // Stages only the first hunk: the file stays in Staged and in Changes at the same time.
    core.git_apply_hunk(&p0, false, false).expect("stage hunk 0");
    let st = core.git_status().expect("st");
    assert_eq!(paths(&st), [("f.txt".into(), 'M', 'M', EntryKind::Ordinary)]);

    // Takes the first hunk out of the stage: it goes back to being only unstaged.
    let staged_diff = core.git_diff(&st.entries[0], true).expect("staged diff");
    let sp0 = extract_hunk_patch(&staged_diff, 0).expect("staged hunk 0");
    core.git_apply_hunk(&sp0, true, false).expect("unstage hunk 0");
    let st = core.git_status().expect("st");
    assert_eq!(paths(&st), [("f.txt".into(), '.', 'M', EntryKind::Ordinary)]);

    // Discards only the second hunk in the working copy: only the change to line 1 remains.
    core.git_apply_hunk(&p1, false, true).expect("discard hunk 1");
    let content = std::fs::read_to_string(ws.join("f.txt")).expect("read");
    assert!(content.starts_with("LINE 1\n") && content.ends_with("line 12\n"), "{content}");
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

    // Stash: save (including untracked files), list, see the diff, apply and drop.
    std::fs::write(ws.join("a.txt"), "base 1\nmodified\n").expect("w");
    std::fs::write(ws.join("new.txt"), "draft\n").expect("w");
    core.git_stash_push("my draft", true).expect("stash push");
    assert_eq!(core.git_status().expect("st").stash_count, 1);
    assert!(!ws.join("new.txt").exists());
    let stashes = core.git_stash_list().expect("stash list");
    assert_eq!(stashes.len(), 1);
    assert!(stashes[0].message.contains("my draft"), "{:?}", stashes[0]);
    let sdiff = core.git_stash_diff(0).expect("stash diff");
    assert!(sdiff.lines.iter().any(|l| l.text.contains("modified")));
    core.git_stash_apply(0).expect("stash apply");
    assert!(ws.join("new.txt").exists());
    core.git_stash_drop(0).expect("stash drop");
    assert!(core.git_stash_list().expect("stash list").is_empty());
    // Cleans up what was applied from the stash.
    let _ = git(ws, &["reset", "-q", "--hard"]);
    let _ = git(ws, &["clean", "-fdq"]);

    // Creates branch `feat` with a conflicting commit.
    core.git_create_branch("feat", true).expect("create feat");
    std::fs::write(ws.join("a.txt"), "theirs 1\n\n\n\n\ntheirs 2\n").expect("w");
    core.git_stage_all().expect("stage");
    core.git_commit("c2: feat", false).expect("c2");

    // Goes back to `main` and makes another commit.
    let main = core.git_branches().expect("b").into_iter().find(|b| b.name == "main").expect("main");
    core.git_checkout(&main).expect("checkout main");
    std::fs::write(ws.join("a.txt"), "ours 1\n\n\n\n\nours 2\n").expect("w");
    core.git_stage_all().expect("stage");
    core.git_commit("c3: main", false).expect("c3");

    // Paginated history and commit detail.
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
    assert!(detail.diff.lines.iter().any(|l| l.text == "+ours 1"));

    // Merge with a conflict → resolve block by block → continue the merge.
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
    assert_eq!(std::fs::read_to_string(ws.join("a.txt")).expect("r"), "ours 1\n\n\n\n\ntheirs 2\n");

    // Rebase with a conflict → resolve the whole file (`--theirs`) → abort.
    let feat = core.git_branches().expect("b").into_iter().find(|b| b.name == "feat").expect("feat");
    core.git_checkout(&feat).expect("checkout feat");
    std::fs::write(ws.join("a.txt"), "another conflict\n").expect("w");
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
    // Enables `commit.gpgsign` pointing at a nonexistent program/without an agent: it must fail fast
    // (< 5 s, never waiting for the 120 s COMMIT_TIMEOUT) and guide the user without turning the option off.
    assert!(git(ws, &["config", "commit.gpgsign", "true"]).status.success());
    assert!(git(ws, &["config", "gpg.program", "gpg-missing-holt"]).status.success());
    let t0 = std::time::Instant::now();
    let err = core.git_commit("signed", false).unwrap_err();
    assert!(t0.elapsed() < std::time::Duration::from_secs(10), "took {:?}", t0.elapsed());
    assert!(err.to_string().contains("commit.gpgsign"), "{err}");
    // The repository configuration stays intact.
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

    // No changes: saving to the stash is a clear error; without `-u`, the untracked file stays.
    assert!(core.git_stash_push("nothing", false).is_err());
    std::fs::write(ws.join("b.txt"), "b2\n").expect("w");
    std::fs::write(ws.join("loose.txt"), "x\n").expect("w");
    core.git_stash_push("", false).expect("stash without a message");
    assert!(ws.join("loose.txt").exists());
    assert_eq!(std::fs::read_to_string(ws.join("b.txt")).expect("r"), "b\n");
    let list = core.git_stash_list().expect("list");
    assert_eq!(list.len(), 1);
    core.git_stash_pop(0).expect("pop");
    assert!(core.git_stash_list().expect("list").is_empty());
    assert_eq!(std::fs::read_to_string(ws.join("b.txt")).expect("r"), "b2\n");
    let _ = git(ws, &["reset", "-q", "--hard"]);
    let _ = git(ws, &["clean", "-fdq"]);

    // Branch `feat`: changes a.txt, deletes b.txt, changes c.txt.
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

    core.git_merge("feat").expect("merge returns Ok with conflicts");
    let st = core.git_status().expect("st");
    assert_eq!(st.state, RepoState::Merging);
    let kinds: Vec<(String, char, char)> = st.entries.iter().map(|e| (e.path.clone(), e.index, e.worktree)).collect();
    assert_eq!(kinds, [("a.txt".into(), 'U', 'U'), ("b.txt".into(), 'U', 'D'), ("c.txt".into(), 'U', 'U')]);

    // The diff of a file in conflict (`diff --cc`, `@@@`) has line numbers and no crash.
    let d = core.git_diff(&st.entries[0], false).expect("diff --cc");
    assert!(d.lines.iter().any(|l| l.kind == holt_core::git::DiffKind::Hunk && l.text.starts_with("@@@")));
    assert!(d.lines.iter().any(|l| l.kind == holt_core::git::DiffKind::Add && l.text.contains("FEAT")));
    assert!(extract_hunk_patch(&d, 0).is_none(), "combined diff does not become a hunk patch");

    // a.txt: "both"; c.txt: "theirs" (whole file); b.txt (modify/delete): "theirs" = delete.
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

    // Discarding a modify/delete conflict (the other side deleted it) works too.
    let m2 = core.git_log(0, 1).expect("log").0[0].hash.clone();
    assert!(git(ws, &["reset", "-q", "--hard", &format!("{m2}~1")]).status.success());
    assert!(!git(ws, &["merge", "feat"]).status.success());
    let st = core.git_status().expect("st");
    let b = st.entries.iter().find(|e| e.path == "b.txt").expect("b.txt in conflict");
    assert_eq!(b.kind, EntryKind::Unmerged);
    core.git_discard(b).expect("discard modify/delete");
    assert!(core.git_status().expect("st").entries.iter().all(|e| e.path != "b.txt"));
    core.git_abort_op().expect("abort");
    assert_eq!(core.git_status().expect("st").state, RepoState::Normal);

    // Rebase with a conflict: resolve and continue.
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
    assert!(msg.to_lowercase().contains("conflict"), "{msg}");
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
    assert!(core.git_continue_op().is_err(), "no operation in progress");
    env.core.shutdown();
}

#[test]
fn hunks_work_in_crlf_files() {
    need_git!();
    let env = start(true);
    let (core, ws) = (&env.core, &env.ws);
    let _ = std::fs::remove_file(ws.join("AGENTS.md"));
    core.git_init().expect("init");
    // No end-of-line conversion: CRLF goes to the repository exactly as it is.
    assert!(git(ws, &["config", "core.autocrlf", "false"]).status.success());
    let initial = (1..=12).map(|i| format!("line {i}\r\n")).collect::<String>();
    std::fs::write(ws.join("w.txt"), &initial).expect("w");
    core.git_stage_all().expect("stage");
    core.git_commit("crlf", false).expect("commit");
    let modified = initial.replace("line 1\r\n", "LINE 1\r\n").replace("line 12\r\n", "LINE 12\r\n");
    std::fs::write(ws.join("w.txt"), &modified).expect("w");
    let st = core.git_status().expect("st");
    let diff = core.git_diff(&st.entries[0], false).expect("diff");
    let p0 = extract_hunk_patch(&diff, 0).expect("hunk 0");
    let p1 = extract_hunk_patch(&diff, 1).expect("hunk 1");
    core.git_apply_hunk(&p0, false, false).expect("stage hunk 0 (CRLF)");
    assert_eq!(paths(&core.git_status().expect("st")), [("w.txt".into(), 'M', 'M', EntryKind::Ordinary)]);
    core.git_apply_hunk(&p1, false, true).expect("discard hunk 1 (CRLF)");
    let content = std::fs::read_to_string(ws.join("w.txt")).expect("r");
    assert!(content.starts_with("LINE 1\r\n") && content.ends_with("line 12\r\n"), "{content:?}");
    env.core.shutdown();
}
