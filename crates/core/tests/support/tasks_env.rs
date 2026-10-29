//! Shared scenario of the tasks desk tests (`tasks.rs`, `tasks_bridge.rs` and `tasks_queue.rs`): a `Core` in
//! temporary folders with a project that is a git repository (branch `main`, one commit), a fake `claude`
//! (and an `aider`) agent installed and the user's git configuration isolated.
#![allow(dead_code)]

use std::io::{Read, Write};
use std::net::TcpStream;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::OnceLock;
use std::time::Duration;

use crossbeam_channel::Receiver;
use holt_core::paths::AppPaths;
use holt_core::{Config, Core, CoreEvent, Ident, StartOptions, Task, TaskSpec};
use serde_json::Value;

pub fn isolated_config() {
    static DIR: OnceLock<PathBuf> = OnceLock::new();
    DIR.get_or_init(|| {
        let dir = tempfile::tempdir().expect("tmp").keep();
        std::fs::write(
            dir.join("gitconfig"),
            "[user]\n\tname = Test\n\temail = test@example.com\n[init]\n\tdefaultBranch = main\n[commit]\n\tgpgsign = false\n",
        )
        .expect("gitconfig");
        // Only read by git (children of this process); set before any core.
        std::env::set_var("GIT_CONFIG_GLOBAL", dir.join("gitconfig"));
        std::env::set_var("GIT_CONFIG_NOSYSTEM", "1");
        // Git does not climb above the temporary folder looking for a repository.
        std::env::set_var("GIT_CEILING_DIRECTORIES", std::env::temp_dir());
        dir
    });
}

pub fn git_exe() -> Option<PathBuf> {
    which::which("git").ok()
}

/// Installed fake agent: a script on the search PATH (Unix) or an empty `.exe` in `~/.local/bin` (Windows).
pub fn install_fake_agent(root: &Path, name: &str) {
    if cfg!(windows) {
        let bin = root.join("home").join(".local").join("bin");
        std::fs::create_dir_all(&bin).expect("mkdir");
        std::fs::write(bin.join(format!("{name}.exe")), b"").expect("write");
    } else {
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let bin = root.join("fake-bin");
            std::fs::create_dir_all(&bin).expect("mkdir");
            let script = bin.join(name);
            std::fs::write(&script, "#!/bin/sh\ncat >/dev/null\n").expect("write");
            std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).expect("chmod");
        }
    }
}

pub struct Env {
    pub _tmp: tempfile::TempDir,
    pub core: Core,
    pub rx: Receiver<CoreEvent>,
    pub root: PathBuf,
    pub ws: PathBuf,
    pub project: String,
    pub config: Config,
}

pub fn paths_in(root: &Path) -> AppPaths {
    AppPaths {
        app_dir: root.to_path_buf(),
        bin_dir: root.join("bin"),
        data_dir: root.join("data"),
        default_workspace: root.join("ws"),
        whisper_dir: root.join("whisper"),
        config_file: root.join("cfg").join("config.json"),
    }
}

pub fn start_core(root: &Path, config: Config, tab_delivery: bool) -> (Core, Receiver<CoreEvent>) {
    let git_dir = git_exe().and_then(|g| g.parent().map(Path::to_path_buf)).unwrap_or_default();
    let search = std::env::join_paths([root.join("fake-bin"), git_dir]).expect("PATH");
    let opts = StartOptions {
        search_path: Some(search),
        home_dir: Some(root.join("home")),
        watch_workspace: false,
        config: Some(config),
        system_dirs: false,
    };
    let (core, rx) = Core::start_with(paths_in(root), opts).expect("start");
    if tab_delivery {
        core.enable_tab_delivery();
    }
    (core, rx)
}

/// Core with the project at `<tmp>/ws`: a repository on the branch `main` with one commit.
pub fn start() -> Env {
    start_with(true, true)
}

pub fn start_with(tab_delivery: bool, repo: bool) -> Env {
    isolated_config();
    let tmp = tempfile::tempdir().expect("tmp");
    // Long name: on the CI's Windows runner TEMP comes short (`RUNNER~1`) and git returns the long one (`runneradmin`).
    let root = holt_core::util::strip_verbatim(&std::fs::canonicalize(tmp.path()).expect("canonicalize"));
    install_fake_agent(&root, "claude");
    install_fake_agent(&root, "aider");
    let config = Config::with_project(root.join("ws"));
    let (core, rx) = start_core(&root, config.clone(), tab_delivery);
    let ws = root.join("ws");
    let project = core.active_project_id();
    if repo {
        git(&ws, &["init", "-q"]);
        std::fs::write(ws.join("a.txt"), "one\ntwo\n").expect("w");
        git(&ws, &["add", "-A"]);
        git(&ws, &["commit", "-q", "-m", "initial"]);
    }
    Env { _tmp: tmp, core, rx, root, ws, project, config }
}

/// Runs git directly (scenario setup and checks); fails if the command fails.
pub fn git(dir: &Path, args: &[&str]) -> String {
    let out = Command::new(git_exe().expect("git")).current_dir(dir).args(args).output().expect("git");
    assert!(out.status.success(), "git {args:?} in {}: {}", dir.display(), String::from_utf8_lossy(&out.stderr));
    String::from_utf8_lossy(&out.stdout).into_owned()
}

pub fn commit_file(dir: &Path, name: &str, content: &str, message: &str) {
    std::fs::write(dir.join(name), content).expect("write");
    git(dir, &["add", "-A"]);
    git(dir, &["commit", "-q", "-m", message]);
}

pub fn spec(title: &str) -> TaskSpec {
    TaskSpec { title: title.into(), brief: format!("Do: {title}\n\nCarefully."), kind: "claude".into(), ..TaskSpec::default() }
}

pub fn events(env: &Env) -> Vec<CoreEvent> {
    env.rx.try_iter().collect()
}

/// The task messages delivered to the tabs (`request: 0`) since the last read.
pub fn deliveries(evs: &[CoreEvent]) -> Vec<(String, String)> {
    evs.iter()
        .filter_map(|e| match e {
            CoreEvent::Deliver(d) if d.request == 0 => Some((d.ident.id(), d.prompt.clone())),
            _ => None,
        })
        .collect()
}

pub fn branches(dir: &Path) -> Vec<String> {
    git(dir, &["branch", "--format=%(refname:short)"]).lines().map(|l| l.trim().to_owned()).filter(|l| !l.is_empty()).collect()
}

pub fn worktree_count(dir: &Path) -> usize {
    git(dir, &["worktree", "list", "--porcelain"]).lines().filter(|l| l.starts_with("worktree ")).count()
}

/// Creates and starts a claude task, with a commit in the worktree, and puts it in review.
pub fn task_in_review(env: &Env, title: &str, file: &str) -> Task {
    let t = env.core.task_create(&env.project, spec(title)).expect("create");
    let t = env.core.task_start(&env.project, &t.id).expect("start");
    commit_file(&t.worktree, file, "content\n", &format!("feat: {title}"));
    env.core.task_done(&env.project, &t.id, "Done.\n\nFiles: all.").expect("done")
}

pub fn http(core: &Core, method: &str, path: &str, token: &str, body: &str) -> (u16, Value) {
    let port = core.bridge_port();
    let mut s = TcpStream::connect(("127.0.0.1", port)).expect("connect");
    s.set_read_timeout(Some(Duration::from_secs(120))).expect("timeout");
    let req = format!(
        "{method} {path} HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nx-holt-token: {token}\r\ncontent-type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    );
    s.write_all(req.as_bytes()).expect("write");
    let mut raw = Vec::new();
    s.read_to_end(&mut raw).expect("read");
    let text = String::from_utf8_lossy(&raw);
    let status: u16 = text.split_whitespace().nth(1).and_then(|c| c.parse().ok()).expect("status");
    let body = text.split_once("\r\n\r\n").map(|(_, b)| b).unwrap_or("");
    (status, serde_json::from_str(body).unwrap_or(Value::Null))
}

pub fn tab_token(env: &Env, id: &str) -> String {
    let ident = Ident::parse(id).expect("id");
    env.core.spawn_spec_for(&env.project, &ident).expect("spec").env["HOLT_TOKEN"].clone()
}
