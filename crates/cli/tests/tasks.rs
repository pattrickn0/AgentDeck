//! `ask-agent --task ...` end to end: the real binary against the real bridge, with real git
//! (skips with a notice if `git` is not installed). The assertions on `ask-agent`'s own output and on the bridge's
//! errors quote texts that belong to other areas (`ask-agent.rs`, `bridge.rs`): they stay as those areas have them.

use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};
use std::sync::OnceLock;

use holt_core::paths::AppPaths;
use holt_core::{Config, Core, Ident, StartOptions};

fn git_exe() -> Option<PathBuf> {
    which::which("git").ok()
}

/// Isolated global git configuration (identity for the commits, `main` branch).
fn isolated_git_config() {
    static DIR: OnceLock<PathBuf> = OnceLock::new();
    DIR.get_or_init(|| {
        let dir = tempfile::tempdir().expect("tmp").keep();
        std::fs::write(
            dir.join("gitconfig"),
            "[user]\n\tname = Test\n\temail = test@example.com\n[init]\n\tdefaultBranch = main\n[commit]\n\tgpgsign = false\n",
        )
        .expect("gitconfig");
        std::env::set_var("GIT_CONFIG_GLOBAL", dir.join("gitconfig"));
        std::env::set_var("GIT_CONFIG_NOSYSTEM", "1");
        std::env::set_var("GIT_CEILING_DIRECTORIES", std::env::temp_dir());
        dir
    });
}

fn git(dir: &Path, args: &[&str]) {
    let out = Command::new(git_exe().expect("git")).current_dir(dir).args(args).output().expect("git");
    assert!(out.status.success(), "git {args:?}: {}", String::from_utf8_lossy(&out.stderr));
}

fn start_core(root: &Path) -> Core {
    let paths = AppPaths {
        app_dir: root.to_path_buf(),
        bin_dir: root.join("bin"),
        data_dir: root.join("data"),
        default_workspace: root.join("workspace"),
        whisper_dir: root.join("whisper"),
        config_file: root.join("cfg").join("config.json"),
    };
    // An installed "claude": a script on Unix; on Windows, the empty .exe where the app looks.
    let fake_bin = root.join("fake-bin");
    std::fs::create_dir_all(&fake_bin).expect("mkdir");
    if cfg!(windows) {
        let bin = root.join("home").join(".local").join("bin");
        std::fs::create_dir_all(&bin).expect("mkdir");
        std::fs::write(bin.join("claude.exe"), b"").expect("write");
    } else {
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let script = fake_bin.join("claude");
            std::fs::write(&script, "#!/bin/sh\ncat >/dev/null\n").expect("write");
            std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).expect("chmod");
        }
    }
    let git_dir = git_exe().and_then(|g| g.parent().map(Path::to_path_buf)).unwrap_or_default();
    let opts = StartOptions {
        search_path: Some(std::env::join_paths([fake_bin, git_dir]).expect("PATH")),
        home_dir: Some(root.join("home")),
        watch_workspace: false,
        config: Some(Config::with_project(paths.default_workspace.clone())),
        system_dirs: false,
    };
    let core = Core::start_with(paths, opts).expect("start").0;
    core.enable_tab_delivery();
    core
}

/// `ask-agent` with the master token (user) or a tab's (`agent = Some((id, token))`).
fn run(core: &Core, agent: Option<(&str, &str)>, args: &[&str], stdin: &str) -> Output {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_ask-agent"));
    cmd.args(args).env_remove("HOLT_DEPTH").env("HOLT_PORT", core.bridge_port().to_string());
    match agent {
        Some((id, token)) => cmd.env("HOLT_TOKEN", token).env("HOLT_AGENT", id),
        None => cmd.env("HOLT_TOKEN", core.bridge_token()).env("HOLT_AGENT", "shell"),
    };
    cmd.stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::piped());
    let mut child = cmd.spawn().expect("spawn ask-agent");
    if let Some(mut pipe) = child.stdin.take() {
        let _ = pipe.write_all(stdin.as_bytes());
    }
    child.wait_with_output().expect("output")
}

fn text(b: &[u8]) -> String {
    String::from_utf8_lossy(b).into_owned()
}

#[test]
fn the_task_commands_drive_the_whole_lifecycle() {
    if git_exe().is_none() {
        eprintln!("git not installed: test skipped");
        return;
    }
    isolated_git_config();
    let tmp = tempfile::tempdir().expect("tmp");
    let core = start_core(tmp.path());
    let ws = tmp.path().join("workspace");
    git(&ws, &["init", "-q"]);
    std::fs::write(ws.join("a.txt"), "um\n").expect("w");
    git(&ws, &["add", "-A"]);
    git(&ws, &["commit", "-q", "-m", "initial"]);
    let project = core.active_project_id();

    let out = run(&core, None, &["--task", "list"], "");
    assert_eq!((out.status.code(), text(&out.stdout).trim()), (Some(0), "No tasks in this project."), "{}", text(&out.stderr));

    // Create and start, with the request on standard input and the options.
    let out = run(&core, None, &["--task", "new", "Fix the login", "--model", "sonnet", "--start", "-"], "Fix the login.\nWith tests.");
    assert_eq!(out.status.code(), Some(0), "{}", text(&out.stderr));
    let shown = text(&out.stdout);
    assert!(shown.contains("Task t1 created: Fix the login") && shown.contains("claude@t1 (sonnet)"), "{shown}");
    assert!(shown.contains("started in the worktree"), "{shown}");
    let t1 = core.task(&project, "t1").expect("t1");
    assert_eq!((t1.brief.as_str(), t1.model.as_deref()), ("Fix the login.\nWith tests.", Some("sonnet")));
    assert!(t1.worktree.is_dir());

    // Only create (request in a file with accents, via `--file`), list, show.
    let brief = tmp.path().join("request.md");
    std::fs::write(&brief, "Do something else: café and naïve.\n").expect("w");
    let out = run(&core, None, &["--task", "new", "Second task", "--file", brief.to_str().expect("utf8")], "");
    assert_eq!(out.status.code(), Some(0), "{}", text(&out.stderr));
    assert!(text(&out.stdout).contains("start it with: ask-agent --task start t2"), "{}", text(&out.stdout));
    assert_eq!(core.task(&project, "t2").expect("t2").brief, "Do something else: café and naïve.");
    let out = run(&core, None, &["--task", "list"], "");
    let list = text(&out.stdout);
    let lines: Vec<&str> = list.lines().collect();
    assert_eq!(lines.len(), 2, "{list}");
    assert!(
        lines[0].starts_with("t1")
            && lines[0].contains("running")
            && lines[0].contains("claude@t1 (sonnet)")
            && lines[0].ends_with("Fix the login"),
        "{list}"
    );
    assert!(lines[1].starts_with("t2") && lines[1].contains("draft"), "{list}");
    let out = run(&core, None, &["--task", "show", "1"], "");
    assert!(text(&out.stdout).contains("Worktree:") && text(&out.stdout).contains("Base: main"), "{}", text(&out.stdout));

    // The task's agent delivers the report (its tab's token); it cannot touch the rest.
    let token = core.spawn_spec_for(&project, &Ident::parse("claude@t1").expect("id")).expect("spec").env["HOLT_TOKEN"].clone();
    let agent = Some(("claude@t1", token.as_str()));
    std::fs::write(t1.worktree.join("login.txt"), "ok\n").expect("w");
    git(&t1.worktree, &["add", "-A"]);
    git(&t1.worktree, &["commit", "-q", "-m", "fix: login"]);
    let out = run(&core, agent, &["--task", "merge", "t1"], "");
    assert_eq!(out.status.code(), Some(1));
    assert!(text(&out.stderr).contains("only reads tasks"), "{}", text(&out.stderr));
    let out = run(&core, agent, &["--task", "done", "t1", "-"], "  \n");
    assert_eq!(out.status.code(), Some(2), "an empty report is a usage error");
    let out = run(&core, agent, &["--task", "done", "t1", "-"], "I fixed the login.\nTests: ok.");
    assert_eq!(out.status.code(), Some(0), "{}", text(&out.stderr));
    assert!(text(&out.stdout).contains("Report of task t1 delivered"), "{}", text(&out.stdout));
    let out = run(&core, None, &["--task", "show", "t1"], "");
    let detail = text(&out.stdout);
    assert!(detail.contains("State: review") && detail.contains("Report:\nI fixed the login.") && detail.contains("login.txt"), "{detail}");

    // Return, deliver again and merge.
    let out = run(&core, None, &["--task", "return", "t1", "A", "test", "is", "missing"], "");
    assert_eq!(out.status.code(), Some(0), "{}", text(&out.stderr));
    assert!(text(&out.stdout).contains("sent back to claude@t1"), "{}", text(&out.stdout));
    // The report also comes from a file (`--file`): it is the path that preserves accents, which the
    // agent's shell (PowerShell 5.1, `$OutputEncoding` in US-ASCII) would replace with "?" on stdin.
    let report = tmp.path().join("report.md");
    std::fs::write(&report, "Now with a test: café and naïve ✓.\n").expect("w");
    let out = run(&core, agent, &["--task", "done", "t1", "--file", report.to_str().expect("utf8")], "");
    assert_eq!(out.status.code(), Some(0), "{}", text(&out.stderr));
    let t1 = core.task(&project, "t1").expect("t1");
    assert_eq!(t1.report.as_deref(), Some("Now with a test: café and naïve ✓."), "accents preserved by --file");
    let out = run(&core, None, &["--task", "merge", "t1"], "");
    assert_eq!(out.status.code(), Some(0), "{}", text(&out.stderr));
    assert!(text(&out.stdout).contains("Task t1 merged into main; waiting for validation"), "{}", text(&out.stdout));
    assert!(ws.join("login.txt").is_file() && t1.worktree.is_dir(), "merging keeps the worktree until validated");
    let out = run(&core, None, &["--task", "show", "t1"], "");
    assert!(text(&out.stdout).contains("State: validating"), "{}", text(&out.stdout));

    // Reject (the reason comes from a file) and deliver again; then validate.
    let out = run(&core, None, &["--task", "reject", "t1"], "");
    assert_eq!(out.status.code(), Some(2), "rejecting without a reason is a usage error");
    let why = tmp.path().join("reason.md");
    std::fs::write(&why, "It does not save the session: café ✗.\n").expect("w");
    let out = run(&core, None, &["--task", "reject", "t1", "--file", why.to_str().expect("utf8")], "");
    assert_eq!(out.status.code(), Some(0), "{}", text(&out.stderr));
    assert!(text(&out.stdout).contains("Task t1 rejected; claude@t1 received the reason"), "{}", text(&out.stdout));
    let out = run(&core, None, &["--task", "validate", "t1"], "");
    assert_eq!(out.status.code(), Some(1), "a task in progress is not validated");
    std::fs::write(t1.worktree.join("login2.txt"), "ok\n").expect("w");
    git(&t1.worktree, &["add", "-A"]);
    git(&t1.worktree, &["commit", "-q", "-m", "fix: session"]);
    assert_eq!(run(&core, agent, &["--task", "done", "t1", "-"], "Session fixed.").status.code(), Some(0));
    assert_eq!(run(&core, None, &["--task", "merge", "t1"], "").status.code(), Some(0));
    let out = run(&core, None, &["--task", "validate", "t1"], "");
    assert_eq!(out.status.code(), Some(0), "{}", text(&out.stderr));
    assert!(text(&out.stdout).contains("Task t1 validated and done"), "{}", text(&out.stdout));
    assert!(ws.join("login2.txt").is_file() && !t1.worktree.exists());

    // Errors: wrong state (1), incorrect usage (2), invalid agent (1), discard.
    let out = run(&core, None, &["--task", "merge", "t1"], "");
    assert_eq!(out.status.code(), Some(1));
    assert!(text(&out.stderr).contains("completed"), "{}", text(&out.stderr));
    assert_eq!(run(&core, None, &["--task", "bogus"], "").status.code(), Some(2));
    assert_eq!(run(&core, None, &["--task"], "").status.code(), Some(2));
    assert_eq!(run(&core, None, &["--task", "show", "xyz"], "").status.code(), Some(2));
    assert_eq!(run(&core, None, &["--task", "show", "t9"], "").status.code(), Some(1));
    let out = run(&core, None, &["--task", "new", "x", "--agent", "shell", "-"], "request");
    assert_eq!(out.status.code(), Some(1));
    assert!(text(&out.stderr).contains("unknown or not installed agent"), "{}", text(&out.stderr));
    let out = run(&core, None, &["--task", "discard", "t2"], "");
    assert_eq!(out.status.code(), Some(0), "{}", text(&out.stderr));
    assert!(text(&out.stdout).contains("Task t2 discarded"), "{}", text(&out.stdout));
    let out = run(&core, None, &["--task", "list"], "");
    assert!(text(&out.stdout).contains("done") && text(&out.stdout).contains("discarded"), "{}", text(&out.stdout));
    // The help mentions the tasks.
    assert!(text(&run(&core, None, &["--help"], "").stdout).contains("--task"));
    core.shutdown();
}

#[test]
fn the_scope_flag_queues_an_overlapping_task_and_shows_it() {
    if git_exe().is_none() {
        eprintln!("git not installed: test skipped");
        return;
    }
    isolated_git_config();
    let tmp = tempfile::tempdir().expect("tmp");
    let core = start_core(tmp.path());
    let ws = tmp.path().join("workspace");
    git(&ws, &["init", "-q"]);
    std::fs::write(ws.join("a.txt"), "um\n").expect("w");
    git(&ws, &["add", "-A"]);
    git(&ws, &["commit", "-q", "-m", "initial"]);
    let project = core.active_project_id();

    // The scope goes up to the next option; the request comes before it (or from the file).
    let out =
        run(&core, None, &["--task", "new", "Touches core", "request", "--scope", "crates/core", "memory/OVERVIEW.md", "--start"], "");
    assert_eq!(out.status.code(), Some(0), "{}", text(&out.stderr));
    let shown = text(&out.stdout);
    assert!(shown.contains("scope: crates/core, memory/OVERVIEW.md") && shown.contains("started in the worktree"), "{shown}");
    assert_eq!(core.task(&project, "t1").expect("t1").brief, "request");

    let out = run(&core, None, &["--task", "new", "Touches git.rs", "request", "--start", "--scope", r"crates\core\src\git.rs"], "");
    assert_eq!(out.status.code(), Some(0), "{}", text(&out.stderr));
    let shown = text(&out.stdout);
    assert!(shown.contains("queued: the scope overlaps that of t1"), "{shown}");
    let t2 = core.task(&project, "t2").expect("t2");
    assert_eq!((t2.status, t2.waiting_for.as_deref()), (holt_core::TaskStatus::Queued, Some("t1")));
    assert_eq!(t2.scope, ["crates/core/src/git.rs"], "the path arrives normalized");

    let list = text(&run(&core, None, &["--task", "list"], "").stdout);
    let line = list.lines().find(|l| l.starts_with("t2")).expect("t2");
    assert!(line.contains("queued") && line.ends_with("Touches git.rs (waits for t1)"), "{list}");
    let detail = text(&run(&core, None, &["--task", "show", "t2"], "").stdout);
    assert!(detail.contains("Queued: waits for task t1") && detail.contains("Scope: crates/core/src/git.rs"), "{detail}");

    // Starting again stays in the queue; discarding the first makes it start by itself.
    let out = run(&core, None, &["--task", "start", "t2"], "");
    assert!(text(&out.stdout).contains("Task t2 queued"), "{}", text(&out.stdout));
    assert_eq!(run(&core, None, &["--task", "discard", "t1"], "").status.code(), Some(0));
    assert_eq!(core.task(&project, "t2").expect("t2").status, holt_core::TaskStatus::Running);

    // Invalid scope: the bridge refuses and nothing is created.
    let out = run(&core, None, &["--task", "new", "Bad", "request", "--scope", "crates/*.rs"], "");
    assert_eq!(out.status.code(), Some(1));
    assert!(text(&out.stderr).contains("not patterns (glob)"), "{}", text(&out.stderr));
    assert_eq!(run(&core, None, &["--task", "new", "Bad", "--scope"], "").status.code(), Some(2));
    assert_eq!(core.tasks(&project).len(), 2);
    core.shutdown();
}
