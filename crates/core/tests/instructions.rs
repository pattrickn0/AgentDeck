//! How the Holt instructions reach each CLI, in a user project that does not have an `AGENTS.md` yet:
//! each tab gets them through its own CLI's channel, and only what has no other way gets a new `AGENTS.md`.

use std::path::{Path, PathBuf};

use crossbeam_channel::Receiver;
use holt_core::paths::AppPaths;
use holt_core::{Config, Core, CoreEvent, FeedKind, Ident, StartOptions};

struct Env {
    _tmp: tempfile::TempDir,
    /// Where the user's projects live: outside the app folder (`root`), which the app treats as its own.
    users: tempfile::TempDir,
    core: Core,
    rx: Receiver<CoreEvent>,
    root: PathBuf,
}

/// `Core` in temporary folders with fake CLIs (they only need to exist: resolution does not run them).
fn start(agents: &[&str]) -> Env {
    let tmp = tempfile::tempdir().expect("tmp");
    let root = tmp.path().to_path_buf();
    let paths = AppPaths {
        app_dir: root.clone(),
        bin_dir: root.join("bin"),
        data_dir: root.join("data"),
        default_workspace: root.join("workspace"),
        whisper_dir: root.join("whisper"),
        config_file: root.join("cfg").join("config.json"),
    };
    let fake_bin = root.join("fake-bin");
    std::fs::create_dir_all(&fake_bin).expect("mkdir");
    for name in agents {
        write_fake(&fake_bin, name);
    }
    let opts = StartOptions {
        search_path: Some(fake_bin.into_os_string()),
        home_dir: Some(root.join("home")),
        watch_workspace: false,
        config: Some(Config::with_project(paths.default_workspace.clone())),
        system_dirs: false,
    };
    let (core, rx) = Core::start_with(paths, opts).expect("start");
    Env { _tmp: tmp, users: tempfile::tempdir().expect("tmp"), core, rx, root }
}

/// The executable of each agent: Antigravity is `agy`; the others use their own name.
fn exe_name(name: &str) -> &str {
    if name == "antigravity" {
        "agy"
    } else {
        name
    }
}

#[cfg(unix)]
fn write_fake(dir: &Path, name: &str) {
    use std::os::unix::fs::PermissionsExt;
    let path = dir.join(exe_name(name));
    std::fs::write(&path, "#!/bin/sh\n").expect("write");
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).expect("chmod");
}

#[cfg(not(unix))]
fn write_fake(dir: &Path, name: &str) {
    std::fs::write(dir.join(format!("{}.exe", exe_name(name))), "").expect("write");
}

impl Env {
    /// New user folder (outside the app folder), without an `AGENTS.md`, as a project opened now.
    fn user_project(&self, name: &str) -> (String, PathBuf) {
        let dir = self.users.path().join(name);
        std::fs::create_dir_all(&dir).expect("mkdir");
        (self.core.create_project(&dir, Some(name)).id, dir)
    }

    fn spec(&self, project: &str, ident: &str) -> holt_core::SpawnSpec {
        self.core.spawn_spec_for(project, &Ident::parse(ident).expect("id")).expect("spec")
    }

    fn feed_texts(&self) -> Vec<String> {
        self.rx
            .try_iter()
            .filter_map(|ev| match ev {
                CoreEvent::Feed(f) if f.kind == FeedKind::System => Some(f.text),
                _ => None,
            })
            .collect()
    }
}

fn arg_after(args: &[String], flag: &str) -> String {
    let i = args.iter().position(|a| a == flag).unwrap_or_else(|| panic!("no {flag} in {args:?}"));
    args[i + 1].clone()
}

#[test]
fn every_cli_gets_the_instructions_without_touching_the_users_repository() {
    let env = start(&["claude", "codex", "opencode", "aider", "interpreter", "antigravity"]);
    let (project, dir) = env.user_project("new");

    // Claude: system prompt, with the subagent rule and the general memory.
    let claude = env.spec(&project, "claude");
    let text = arg_after(&claude.args, "--append-system-prompt");
    assert!(text.contains("do NOT use your CLI's internal subagents"), "{text}");
    assert!(text.contains("memory/learnings/") && text.contains("ask-agent --learn"), "{text}");
    assert!(text.contains("ask-agent --task new"), "{text}");

    // Codex: `-c developer_instructions=<TOML text>`.
    let codex = env.spec(&project, "codex");
    let value = codex.args.iter().find(|a| a.starts_with("developer_instructions=")).expect("-c developer_instructions");
    let toml = value.strip_prefix("developer_instructions=").expect("key");
    let decoded: String = serde_json::from_str(toml).expect("TOML/JSON string");
    assert!(decoded.contains("do NOT use your CLI's internal subagents"), "{decoded}");
    assert!(decoded.contains("Your name here is \"codex\""), "{decoded}");

    // OpenCode: instructions file in the configuration (and its own server, which reads it).
    let opencode = env.spec(&project, "opencode");
    assert!(opencode.args.contains(&"--standalone".to_owned()), "{:?}", opencode.args);
    let config: serde_json::Value = serde_json::from_str(&opencode.env["OPENCODE_CONFIG_CONTENT"]).expect("json");
    let file = PathBuf::from(config["instructions"][0].as_str().expect("file"));
    assert!(file.starts_with(env.root.join("data")), "{}", file.display());
    let from_file = std::fs::read_to_string(&file).expect("read");
    assert!(from_file.contains("do NOT use your CLI's internal subagents") && from_file.contains("HOLT_AGENT"), "{from_file}");

    // Aider: the same file, as read-only context.
    let aider = env.spec(&project, "aider");
    assert_eq!(PathBuf::from(arg_after(&aider.args, "--read")), file);

    // Open Interpreter: custom instructions.
    let interpreter = env.spec(&project, "interpreter");
    assert!(arg_after(&interpreter.args, "--custom_instructions").contains("do NOT use your CLI's internal subagents"));

    // Antigravity: an instance `AGENTS.md` in a data folder passed in `--add-dir` (`agy` reads the ones of the added
    // folders together with the project one).
    let agy = env.spec(&project, "antigravity@arquiteto");
    let dirs: Vec<PathBuf> =
        agy.args.iter().zip(agy.args.iter().skip(1)).filter(|(a, _)| *a == "--add-dir").map(|(_, d)| PathBuf::from(d)).collect();
    let own = dirs.iter().find(|d| d.join("AGENTS.md").is_file()).unwrap_or_else(|| panic!("no AGENTS.md in {dirs:?}"));
    assert!(own.starts_with(env.root.join("data")), "{}", own.display());
    let agy_text = std::fs::read_to_string(own.join("AGENTS.md")).expect("read");
    assert!(agy_text.contains("do NOT use your CLI's internal subagents") && agy_text.contains("antigravity@arquiteto"), "{agy_text}");

    // None of that dirties the user's repository.
    assert!(!dir.join("AGENTS.md").exists(), "the app created an AGENTS.md for a CLI that has its own channel");
    assert!(env.feed_texts().is_empty());
}

#[test]
fn a_cli_that_only_reads_agents_md_gets_one_with_just_the_app_block() {
    let env = start(&["goose"]);
    let (project, dir) = env.user_project("new");
    assert!(!dir.join("AGENTS.md").exists());

    env.spec(&project, "goose");
    let text = std::fs::read_to_string(dir.join("AGENTS.md")).expect("AGENTS.md created");
    assert!(text.starts_with("<!-- holt:start -->") && text.trim_end().ends_with("<!-- holt:end -->"), "{text}");
    assert!(text.contains("do NOT use your CLI's internal subagents") && text.contains("memory/learnings/"), "{text}");
    // The user is warned in the Conversation, once.
    let notices = env.feed_texts();
    assert_eq!(notices.len(), 1, "{notices:?}");
    assert!(notices[0].contains("AGENTS.md"), "{notices:?}");

    // Opening it again changes nothing and does not warn again (nothing new in `git status`).
    env.spec(&project, "goose");
    assert_eq!(std::fs::read_to_string(dir.join("AGENTS.md")).expect("read"), text);
    assert!(env.feed_texts().is_empty());
}

#[test]
fn the_users_own_agents_md_is_kept_and_not_announced_as_created() {
    let env = start(&["goose"]);
    let (project, dir) = env.user_project("with-rules");
    std::fs::write(dir.join("AGENTS.md"), "# My project rules\n\nUse tabs.\n").expect("write");

    env.spec(&project, "goose");
    let text = std::fs::read_to_string(dir.join("AGENTS.md")).expect("read");
    assert!(text.starts_with("# My project rules\n\nUse tabs.\n") && text.contains("do NOT use your CLI's internal subagents"), "{text}");
    assert!(env.feed_texts().is_empty());
}
