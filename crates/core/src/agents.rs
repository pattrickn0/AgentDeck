//! Agent registry: resolution of the official executables, assembly of the interactive
//! command (PTY) and the bridge instructions (equivalent to `AGENTS`, `resolve*`, `peers`,
//! `bridgeInstructions` and `pty:spawn` in main.js).

use std::collections::BTreeMap;
use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use parking_lot::Mutex;

use crate::instances::Ident;
use crate::models;
use crate::resume;
use crate::t;
use crate::Core;

/// Agent menu item (equivalent to the return of `agents:list`).
#[derive(Debug, Clone, PartialEq)]
pub struct AgentInfo {
    /// `claude`, `antigravity`, `codex`, ..., `shell`.
    pub id: String,
    pub label: String,
    /// `#rrggbb` color.
    pub color: String,
    pub installed: bool,
    pub peer: bool,
}

/// Full command to open an interactive tab in a pseudo-terminal.
#[derive(Debug, Clone)]
pub struct SpawnSpec {
    pub program: PathBuf,
    pub args: Vec<String>,
    pub cwd: PathBuf,
    /// COMPLETE environment to use (already derived from the process environment, with PATH/HOLT_* injected
    /// and CLAUDECODE & co. removed). The UI must clear the inherited environment and apply this one.
    pub env: BTreeMap<String, String>,
    /// Tab label (e.g. "Claude").
    pub label: String,
    pub color: String,
    /// Original `kind` requested.
    pub kind: String,
    /// If `true`, delivering a message to the tab must send an extra `\r` after 600 ms when the
    /// text contains `@` (Claude/Antigravity file autocomplete).
    pub double_enter_on_mention: bool,
    /// LLM model of the instance (already resolved and saved in `tab_models`); `None` = the CLI default.
    pub model: Option<String>,
    /// The launch continues a conversation saved for the tab (`--resume <id>` and the like). If the process ends right
    /// away with a failure, the UI forgets the id and opens the tab again from scratch (see `resume::resume_failed`).
    pub resumed: bool,
    /// Antigravity's `--log-file` for this process: where its conversation id shows up (see `resume::agy_session_in_log`).
    pub session_log: Option<PathBuf>,
}

/// Official colors (also used by the UI as a fallback).
pub const COLOR_CLAUDE: &str = "#d97757";
pub const COLOR_ANTIGRAVITY: &str = "#4f8cf7";
pub const COLOR_SHELL: &str = "#9aa0a6";

/// Static definition of an agent (equivalent to an `AGENTS` entry).
#[derive(Debug, Clone, Copy)]
pub struct AgentDef {
    pub id: &'static str,
    pub label: &'static str,
    pub color: &'static str,
    pub peer: bool,
    /// "Not installed" message (with `\r\n`, since it goes straight to the terminal).
    pub missing: &'static str,
}

/// The Antigravity installer is a PowerShell script on Windows; on the other systems, the one from the site.
#[cfg(windows)]
const ANTIGRAVITY_MISSING: &str =
    "Antigravity CLI (agy) not found. Install from PowerShell with:\r\n  irm https://antigravity.google/cli/install.ps1 | iex";
#[cfg(not(windows))]
const ANTIGRAVITY_MISSING: &str = "Antigravity CLI (agy) not found. See the installer for your system at:\r\n  https://antigravity.google";

/// All agents, in menu order.
pub const AGENTS: &[AgentDef] = &[
    AgentDef {
        id: "claude",
        label: "Claude",
        color: COLOR_CLAUDE,
        peer: true,
        missing: "Claude Code CLI (claude) not found. Install with:\r\n  npm install -g @anthropic-ai/claude-code",
    },
    AgentDef { id: "antigravity", label: "Antigravity", color: COLOR_ANTIGRAVITY, peer: true, missing: ANTIGRAVITY_MISSING },
    AgentDef {
        id: "codex",
        label: "Codex",
        color: "#10a37f",
        peer: true,
        missing: "Codex CLI not found. Install with:\r\n  npm install -g @openai/codex",
    },
    AgentDef {
        id: "opencode",
        label: "OpenCode",
        color: "#f97316",
        peer: true,
        missing: "OpenCode not found. Install with:\r\n  npm install -g opencode-ai",
    },
    AgentDef {
        id: "aider",
        label: "Aider",
        color: "#22c55e",
        peer: true,
        missing: "Aider CLI not found. Install with:\r\n  pip install aider-chat",
    },
    AgentDef {
        id: "goose",
        label: "Goose",
        color: "#10b981",
        peer: true,
        missing: "Goose CLI not found. Install from: https://github.com/block/goose",
    },
    AgentDef {
        id: "copilot",
        label: "Copilot",
        color: "#6e40c9",
        peer: true,
        missing: "GitHub Copilot CLI not found. Install with:\r\n  gh extension install github/gh-copilot",
    },
    AgentDef { id: "q", label: "Amazon Q", color: "#ff9900", peer: true, missing: "Amazon Q Developer CLI not found." },
    AgentDef {
        id: "interpreter",
        label: "Interpreter",
        color: "#8b5cf6",
        peer: true,
        missing: "Open Interpreter not found. Install with:\r\n  pip install open-interpreter",
    },
    AgentDef { id: "shell", label: "Terminal", color: COLOR_SHELL, peer: false, missing: "Shell not found." },
];

impl AgentDef {
    /// The "not installed" message for the app: user-facing text, so it follows the interface language. The agents
    /// get [`AgentDef::missing`] instead, which is always English.
    pub fn install_hint(&self) -> &'static str {
        match self.id {
            "claude" => t!("Claude Code CLI (claude) not found. Install with:\r\n  npm install -g @anthropic-ai/claude-code"),
            "antigravity" => antigravity_hint(),
            "codex" => t!("Codex CLI not found. Install with:\r\n  npm install -g @openai/codex"),
            "opencode" => t!("OpenCode not found. Install with:\r\n  npm install -g opencode-ai"),
            "aider" => t!("Aider CLI not found. Install with:\r\n  pip install aider-chat"),
            "goose" => t!("Goose CLI not found. Install from: https://github.com/block/goose"),
            "copilot" => t!("GitHub Copilot CLI not found. Install with:\r\n  gh extension install github/gh-copilot"),
            "q" => t!("Amazon Q Developer CLI not found."),
            "interpreter" => t!("Open Interpreter not found. Install with:\r\n  pip install open-interpreter"),
            _ => t!("Shell not found."),
        }
    }
}

/// [`AgentDef::install_hint`] for the Antigravity errors, in the wording of [`ANTIGRAVITY_MISSING`].
#[cfg(windows)]
fn antigravity_hint() -> &'static str {
    t!("Antigravity CLI (agy) not found. Install from PowerShell with:\r\n  irm https://antigravity.google/cli/install.ps1 | iex")
}

/// [`AgentDef::install_hint`] for the Antigravity errors, in the wording of [`ANTIGRAVITY_MISSING`].
#[cfg(not(windows))]
fn antigravity_hint() -> &'static str {
    t!("Antigravity CLI (agy) not found. See the installer for your system at:\r\n  https://antigravity.google")
}

/// Aliases accepted by the bridge (`ask-agent agy ...`).
pub const ALIASES: &[(&str, &str)] = &[
    ("agy", "antigravity"),
    ("claudecode", "claude"),
    ("gh-copilot", "copilot"),
    ("amazon-q", "q"),
    ("openai", "codex"),
    ("open-code", "opencode"),
];

pub fn agent_def(id: &str) -> Option<&'static AgentDef> {
    AGENTS.iter().find(|a| a.id == id)
}

/// Resolves an alias (`agy` → `antigravity`); unknown names come back unchanged.
pub fn resolve_alias(name: &str) -> &str {
    ALIASES.iter().find(|(a, _)| *a == name).map(|(_, t)| *t).unwrap_or(name)
}

/// Resolved executable + initial fixed arguments (e.g. `cmd.exe /d /c claude`).
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Resolved {
    pub program: PathBuf,
    pub pre: Vec<String>,
    /// Windows: the agent is a `.cmd` shim (npm/pip), opened via `cmd.exe /d /c <name>`. cmd.exe
    /// interprets `& | < > ^ % "` and line breaks in the arguments: no free text in them.
    pub shim: Option<PathBuf>,
}

impl Resolved {
    fn direct(p: PathBuf) -> Self {
        Self { program: p, pre: Vec::new(), shim: None }
    }

    /// `cmd.exe /d /e:on /v:off /c <name> [extra…]` for the shim at `path`. Extensions enabled and
    /// delayed expansion off even if the registry says otherwise: the escaping of
    /// [`cmd_safe_arg`] relies on that (`%cd:~,%` and a literal `!`).
    fn via_cmd(path: PathBuf, name: &str, extra: &[&str]) -> Self {
        let mut pre: Vec<String> = CMD_FLAGS.map(String::from).to_vec();
        pre.push(name.to_owned());
        pre.extend(extra.iter().map(|s| s.to_string()));
        Self { program: "cmd.exe".into(), pre, shim: Some(path) }
    }

    /// Program and fixed arguments for `std::process::Command`. A `.cmd` shim is called
    /// directly: that way Rust's std applies its own batch-file escaping (and refuses what
    /// cannot be escaped), which does not happen with an explicit `cmd.exe /c`.
    pub fn command(&self) -> (PathBuf, Vec<String>) {
        match &self.shim {
            Some(path) => (path.clone(), self.pre.iter().skip(CMD_FLAGS.len() + 1).cloned().collect()),
            None => (self.program.clone(), self.pre.clone()),
        }
    }
}

/// The native `codex.exe` behind the npm shim `shim` (`<npm>\codex.cmd`). The shim runs `node bin/codex.js`, which spawns
/// the executable of the platform package (`node_modules/@openai/codex-win32-x64/vendor/<triple>/bin/codex.exe`, or
/// `vendor/<triple>/bin` inside `@openai/codex` itself): the same places, in the same order, as `codex.js`.
pub(crate) fn codex_native_exe(shim: &Path) -> Option<PathBuf> {
    let package = shim.parent()?.join("node_modules").join("@openai").join("codex");
    let (platform, triple) = match std::env::consts::ARCH {
        "aarch64" => ("codex-win32-arm64", "aarch64-pc-windows-msvc"),
        _ => ("codex-win32-x64", "x86_64-pc-windows-msvc"),
    };
    [
        package.join("node_modules").join("@openai").join(platform).join("vendor"),
        shim.parent()?.join("node_modules").join("@openai").join(platform).join("vendor"),
        package.join("vendor"),
    ]
    .into_iter()
    .map(|vendor| vendor.join(triple).join("bin").join("codex.exe"))
    .find(|exe| exe.is_file())
}

/// The `notify` setting of Codex: its program is `ask-agent` by its full path when the app's folder has it. A bare name was
/// not found by Codex on Windows (checked with 0.164: the same command with the full path ran).
fn codex_notify_arg(bin_dir: &Path) -> String {
    let exe = bin_dir.join(if cfg!(windows) { "ask-agent.exe" } else { "ask-agent" });
    let program = if exe.is_file() { exe.to_string_lossy().into_owned() } else { "ask-agent".to_owned() };
    // A JSON array of strings is also a TOML array.
    format!("notify={}", serde_json::json!([program, "--signal", "idle"]))
}

/// `cmd.exe` options that open a `.cmd` shim in a tab (before the shim name).
const CMD_FLAGS: [&str; 4] = ["/d", "/e:on", "/v:off", "/c"];

/// Escapes an argument for the `cmd.exe /c <shim>` of the tabs on Windows. The text goes through two
/// cmd interpreters: the `cmd.exe /c` line and the shim line that forwards `%*`.
/// - portable-pty quotes the argument with a space or tab (and only it): inside quotes,
///   `& | < > ^ ( )` are already literal in both passes;
/// - without quotes, each of those becomes `^^^c` (the first pass leaves `^c`, the second, `c`);
/// - `%` becomes `%%cd:~,%` (like Rust's std), which the command line expands to `%` and is not
///   expanded again in `%*`; `!` stays literal with `/v:off`;
/// - quotes are removed (they do not appear in paths) and line breaks become a space.
///
/// Only for text generated by the app (instructions, paths such as `C:\Users\Ana&Bia`): free text
/// from other agents never goes through a shim's command line.
pub(crate) fn cmd_safe_arg(arg: &str) -> String {
    let arg: String = arg.replace("\r\n", " ").replace(['\r', '\n'], " ").chars().filter(|&c| c != '"').collect();
    let quoted = arg.is_empty() || arg.contains([' ', '\t', '\x0b']);
    let mut out = String::with_capacity(arg.len());
    for c in arg.chars() {
        match c {
            '%' => out.push_str("%%cd:~,%"),
            '&' | '|' | '<' | '>' | '^' | '(' | ')' if !quoted => {
                out.push_str("^^^");
                out.push(c);
            }
            c => out.push(c),
        }
    }
    out
}

/// Discovery options (replaceable for tests/embedding).
#[derive(Debug, Clone)]
pub(crate) struct Discovery {
    pub search_path: Option<OsString>,
    pub home: PathBuf,
    /// Look for CLIs in the system folders (`APPDATA`, `LOCALAPPDATA`, `ProgramFiles`). Off
    /// in tests: those folders are then looked up inside `home`.
    pub system_dirs: bool,
}

const COPILOT_CACHE_TTL: Duration = Duration::from_secs(30);

/// Registry state: Copilot detection cache.
pub(crate) struct Registry {
    disc: Discovery,
    copilot: Mutex<Option<(Instant, Option<Resolved>)>>,
}

impl Registry {
    pub fn new(disc: Discovery) -> Self {
        Self { disc, copilot: Mutex::new(None) }
    }

    pub(crate) fn which(&self, name: &str) -> Option<PathBuf> {
        let found = match &self.disc.search_path {
            Some(p) => which::which_in(name, Some(p), std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."))),
            None => which::which(name),
        };
        found.ok()
    }

    fn home(&self) -> &Path {
        &self.disc.home
    }

    /// System folder from the `key` variable (`APPDATA`…). In tests (`system_dirs = false`) it sits
    /// inside `home`, so the result does not depend on what is installed on the machine.
    pub(crate) fn system_dir(&self, key: &str) -> Option<PathBuf> {
        if self.disc.system_dirs {
            std::env::var_os(key).filter(|v| !v.is_empty()).map(PathBuf::from)
        } else {
            Some(self.home().join(key))
        }
    }

    // ---------- Executable resolution ----------

    /// Resolves a base agent.
    pub fn resolve(&self, id: &str) -> Option<Resolved> {
        match id {
            "copilot" => self.resolve_copilot(),
            "shell" => Some(shell_resolved()),
            _ if agent_def(id).is_some() => self.resolve_cli(id),
            _ => None,
        }
    }

    /// Default resolution: known candidates → `where`/`which` → `.cmd` shim via `cmd.exe`.
    fn resolve_cli(&self, id: &str) -> Option<Resolved> {
        let name = if id == "antigravity" { "agy" } else { id };
        let first_existing = |c: Vec<Option<PathBuf>>| c.into_iter().flatten().find(|p| p.is_file());
        if cfg!(windows) {
            let env = |k: &str| self.system_dir(k).unwrap_or_default();
            let home = self.home().to_path_buf();
            let exe = format!("{name}.exe");
            let which_exe = || self.which(&exe);
            let local_bin = Some(home.join(".local").join("bin").join(&exe));
            let candidates = match id {
                "claude" => vec![
                    Some(
                        env("APPDATA")
                            .join("npm")
                            .join("node_modules")
                            .join("@anthropic-ai")
                            .join("claude-code")
                            .join("bin")
                            .join("claude.exe"),
                    ),
                    local_bin,
                    which_exe(),
                ],
                "antigravity" => {
                    let local = env("LOCALAPPDATA");
                    vec![
                        which_exe(),
                        Some(local.join("agy").join("bin").join("agy.exe")),
                        local_bin,
                        Some(local.join("Programs").join("agy").join("agy.exe")),
                        Some(local.join("Programs").join("Antigravity CLI").join("agy.exe")),
                        Some(local.join("antigravity-cli").join("bin").join("agy.exe")),
                        Some(home.join(".antigravity").join("bin").join("agy.exe")),
                    ]
                }
                "aider" => vec![
                    which_exe(),
                    Some(env("APPDATA").join("Python").join("Python311").join("Scripts").join("aider.exe")),
                    Some(env("APPDATA").join("Python").join("Python312").join("Scripts").join("aider.exe")),
                    local_bin,
                ],
                "q" => vec![which_exe(), Some(env("PROGRAMFILES").join("Amazon").join("Q").join("q.exe")), local_bin],
                _ => vec![which_exe(), local_bin],
            };
            if let Some(p) = first_existing(candidates) {
                return Some(Resolved::direct(p));
            }
            // npm/pip shims (`.cmd`) must go through cmd.exe.
            let shim =
                if id == "antigravity" { self.which("agy.cmd") } else { self.which(&format!("{name}.cmd")).or_else(|| self.which(name)) };
            shim.map(|p| {
                if p.extension().is_some_and(|e| e.eq_ignore_ascii_case("exe")) {
                    Resolved::direct(p)
                } else if let Some(exe) = (id == "codex").then(|| codex_native_exe(&p)).flatten() {
                    // The real executable, not the shim: only then does Codex get the `notify` hook (and with it resume).
                    Resolved::direct(exe)
                } else {
                    Resolved::via_cmd(p, name, &[])
                }
            })
        } else {
            let home = self.home().to_path_buf();
            let mut candidates = vec![self.which(name), Some(home.join(".local").join("bin").join(name))];
            match id {
                "claude" => candidates.push(Some(home.join(".claude").join("local").join("claude"))),
                "antigravity" => candidates.push(Some(home.join(".antigravity").join("bin").join("agy"))),
                _ => {}
            }
            first_existing(candidates).map(Resolved::direct)
        }
    }

    /// GitHub Copilot: `gh` with the `gh-copilot` extension (result cached for 30 s,
    /// because it requires running `gh extension list`).
    fn resolve_copilot(&self) -> Option<Resolved> {
        if let Some((at, r)) = self.copilot.lock().as_ref() {
            if at.elapsed() < COPILOT_CACHE_TTL {
                return r.clone();
            }
        }
        let r = self.detect_copilot();
        *self.copilot.lock() = Some((Instant::now(), r.clone()));
        r
    }

    fn detect_copilot(&self) -> Option<Resolved> {
        let gh = if cfg!(windows) {
            self.which("gh.exe").or_else(|| self.which("gh.cmd")).or_else(|| self.which("gh"))
        } else {
            self.which("gh")
        }?;
        let is_cmd = gh.extension().is_some_and(|e| e.eq_ignore_ascii_case("cmd"));
        let gh_resolved = if is_cmd { Resolved::via_cmd(gh.clone(), "gh", &[]) } else { Resolved::direct(gh.clone()) };
        let (program, pre) = gh_resolved.command();
        let mut cmd = std::process::Command::new(&program);
        cmd.args(&pre).args(["extension", "list"]);
        let out = crate::process::run_captured(cmd, None, Duration::from_secs(3), None, None).ok()?;
        if out.stdout.contains("gh-copilot") || out.stdout.contains("copilot") {
            return Some(if is_cmd {
                Resolved::via_cmd(gh, "gh", &["copilot"])
            } else {
                Resolved { program: gh, pre: vec!["copilot".into()], shim: None }
            });
        }
        None
    }

    // ---------- Peers and instructions ----------

    /// Only installed agents enabled as peers join the bridge.
    pub fn peers(&self) -> Vec<String> {
        AGENTS.iter().filter(|a| a.peer && self.resolve(a.id).is_some()).map(|a| a.id.to_owned()).collect()
    }

    /// Like [`Registry::bridge_instructions`], for the instance `ident`, with its role.
    pub fn bridge_instructions_for(&self, ident: &Ident) -> String {
        let mut text = self.bridge_instructions(Some(&ident.id()));
        if let Some(role) = &ident.role {
            text.push_str(&format!("\nYour role in this instance: {role}. Act accordingly."));
        }
        text
    }

    /// The Holt instructions, the "general memory" shared by every agent. With `self_id`, it carries the
    /// instance name and the list of the other agents; without it (file text, the same for any tab), it tells the
    /// agent to read the name from the environment variable and lists nobody, so the file does not change from machine to machine.
    pub fn bridge_instructions(&self, self_id: Option<&str>) -> String {
        const INTRO: &str = "You are running inside Holt, an environment that connects several AI agents.";
        let (first, listed) = match self_id {
            Some(id) => {
                let others: Vec<String> = self.peers().into_iter().filter(|p| p != id).collect();
                (format!("{INTRO} Your name here is \"{id}\"."), format!(" (agents: {})", others.join(",")))
            }
            None => {
                (format!("{INTRO} Your name and role are in the HOLT_AGENT and HOLT_ROLE variables; never ask who you are."), String::new())
            }
        };
        [
            first,
            format!("To talk to another agent, run this in the shell: ask-agent <agent> \"message\"{listed}. If you need another agent, open it in Holt (ask-agent <agent>, ask-agent <agent>@<role> or ask-agent --task new): do NOT use your CLI's internal subagents (Task/Agent tool, sub-agents) unless the user asks for it."),
            "The answer comes on stdout and can take minutes (timeout up to 600000 ms). If ask-agent warns that the answer will be typed later in this terminal, do not repeat the question; if it does not show up, run ask-agent --inbox (--all: the last 20).".to_owned(),
            "ask-agent --models [<agent>] lists the agents, this machine's models and the quota of each one (from the most free to the most spent): before delegating or opening tasks, look at everyone's quota and choose among the models capable of the size of the work, and among them the one with the most free quota, instead of always using the same one (the bridge refuses the exhausted ones). The history of each pair is kept and the user sees everything in the \"Integrated chat\": do not report or summarize the conversation; talk with the other agent until it is done, without stopping to ask the user.".to_owned(),
            "Each tab is an instance <model>[@<role>[@<name>]] (claude, claude@dev, claude@dev@ana): ask-agent claude@dev \"task\" opens the tab if it does not exist; --new opens another one with the same role; --model <model> picks the LLM when opening. Large work becomes a task, with its own worktree, branch and agent: ask-agent --task new \"title\" --start --file <request.md>.".to_owned(),
            "Text with accents goes in a file (--file <file>): through standard input the Windows shell turns accents into \"?\".".to_owned(),
            "When another agent sends you a message, answer with the command it shows (ask-agent --reply <number> \"answer\" or --file <file>): only then does the answer reach it.".to_owned(),
            "The agents form a team (boss and subordinates; the user can reorganize it). Before delegating or reporting, see your position with ask-agent --team; delegate preferably to the team and, when you finish something the boss asked for, report with ask-agent --boss \"message\".".to_owned(),
            "Project memory: memory/learnings/, one .md per trap, decision or convention that cost time and the code does not show. Format: header --- title: … / summary: one line / touches: [globs of the files; optional] --- and a short body, with Why and How to apply. Before touching files, run ask-agent --learn <file>... (without arguments: the index and the state of the tasks; with files, it warns if another task touches them); when you find out something like that, create the file. ask-agent --outline <file>... shows the signatures with the line, so you read only the part of a large file you need (offset).".to_owned(),
        ]
        .join("\n")
    }

    /// List for menus (equivalent to `agents:list`).
    pub fn list(&self) -> Vec<AgentInfo> {
        AGENTS
            .iter()
            .map(|a| AgentInfo {
                id: a.id.into(),
                label: a.label.into(),
                color: a.color.into(),
                installed: self.resolve(a.id).is_some(),
                peer: a.peer,
            })
            .collect()
    }
}

/// Shell of the "Terminal" tab: PowerShell on Windows; `$SHELL` (or /bin/bash) on the others.
fn shell_resolved() -> Resolved {
    if cfg!(windows) {
        // On Windows, force UTF-8 without BOM in the console and in the pipeline to keep
        // pipes to ask-agent or other executables from turning accents into "?".
        Resolved {
            program: "powershell.exe".into(),
            pre: vec![
                "-NoLogo".into(),
                "-NoExit".into(),
                "-Command".into(),
                "$OutputEncoding = [Console]::OutputEncoding = [System.Text.UTF8Encoding]::new($false)".into(),
            ],
            shim: None,
        }
    } else {
        let sh = std::env::var_os("SHELL").filter(|s| !s.is_empty()).map(PathBuf::from).unwrap_or_else(|| "/bin/bash".into());
        Resolved::direct(sh)
    }
}

// ---------- Environment and interactive command ----------

/// The platform's PATH separator.
pub const PATH_SEP: &str = if cfg!(windows) { ";" } else { ":" };

/// Agent environment (equivalent to `agentEnv`): inherits the process one, removes marks of
/// nested Claude Code sessions, puts the binaries folder at the start of PATH and injects
/// the bridge variables.
/// `token` is the process one (see [`Core::tab_token`] and [`Core::headless_token`]), never the master.
pub(crate) fn agent_env(core: &Core, agent_id: &str, depth: i64, token: &str) -> BTreeMap<String, String> {
    let base: Vec<(String, String)> =
        std::env::vars_os().map(|(k, v)| (k.to_string_lossy().into_owned(), v.to_string_lossy().into_owned())).collect();
    build_agent_env(base, &core.paths().bin_dir, core.bridge_port(), token, agent_id, depth)
}

pub(crate) fn build_agent_env(
    base: Vec<(String, String)>,
    bin_dir: &Path,
    port: u16,
    token: &str,
    agent_id: &str,
    depth: i64,
) -> BTreeMap<String, String> {
    // `OPENCODE_CONFIG_CONTENT` is the configuration (model) of the OpenCode tab that opened this process; the one
    // for each new tab is inserted after this filter (`spawn_spec_for`, via `models::opencode_tui`).
    const REMOVE: &[&str] = &[
        "CLAUDECODE",
        "CLAUDE_CODE_ENTRYPOINT",
        "CLAUDE_CODE_CHILD_SESSION",
        "CLAUDE_CODE_SSE_PORT",
        "ELECTRON_RUN_AS_NODE",
        "OPENCODE_CONFIG_CONTENT",
    ];
    let mut env = BTreeMap::new();
    let mut old_path = String::new();
    for (k, v) in base {
        // On Windows variables are not case-sensitive ("Path" vs "PATH").
        let is_path = if cfg!(windows) { k.eq_ignore_ascii_case("PATH") } else { k == "PATH" };
        if is_path {
            if old_path.is_empty() {
                old_path = v;
            }
            continue;
        }
        let removed = if cfg!(windows) { REMOVE.iter().any(|r| r.eq_ignore_ascii_case(&k)) } else { REMOVE.contains(&k.as_str()) }
            // Nothing from the Holt that opened this app (e.g. the app running inside a tab of
            // another Holt) leaks to the agents: role, data folder, debug…
            || (crate::env::is_app_var(&k) && !crate::env::is_app_var_named(&k, "ASK_WAIT"));
        if !removed {
            env.insert(k, v);
        }
    }
    env.insert("PATH".into(), format!("{}{PATH_SEP}{old_path}", bin_dir.display()));
    env.insert("HOLT_PORT".into(), port.to_string());
    env.insert("HOLT_TOKEN".into(), token.to_owned());
    env.insert("HOLT_AGENT".into(), agent_id.to_owned());
    env.insert("HOLT_DEPTH".into(), depth.to_string());
    env.insert("TERM".into(), "xterm-256color".into());
    env.insert("COLORTERM".into(), "truecolor".into());
    if cfg!(windows) {
        env.insert("PYTHONUTF8".into(), "1".into());
        env.insert("PYTHONIOENCODING".into(), "utf-8".into());
        // Git Bash (the one from Claude Code) rewrites arguments that start with "/" as paths
        // (`ask-agent claude@dev "/review src"` would become `C:/Program Files/Git/review src`).
        env.insert("MSYS_NO_PATHCONV".into(), "1".into());
        env.insert("MSYS2_ARG_CONV_EXCL".into(), "*".into());
    }
    env
}

/// Writes `data/hooks/claude-settings.json`: hooks that tell the bridge when Claude starts
/// (`UserPromptSubmit`) and ends (`Stop`) a turn, which add to the user's settings, and
/// the statusline (`ask-agent --usage`), which sends the plan usage to the bridge. Ours replaces the
/// user's, so `ask-agent --usage` runs theirs and shows its output; its `padding`,
/// if any, is kept here (Claude Code applies it, not our line).
fn write_claude_hooks(data_dir: &Path, project_dir: &Path) -> std::io::Result<PathBuf> {
    let config_dir = std::env::var_os("CLAUDE_CONFIG_DIR")
        .filter(|v| !v.is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(|| crate::util::home_dir().join(".claude"));
    write_claude_hooks_with(data_dir, project_dir, &config_dir)
}

/// Like [`write_claude_hooks`], with the Claude configuration folder explicit (test).
fn write_claude_hooks_with(data_dir: &Path, project_dir: &Path, config_dir: &Path) -> std::io::Result<PathBuf> {
    let dir = data_dir.join("hooks");
    std::fs::create_dir_all(&dir)?;
    let path = dir.join("claude-settings.json");
    let hook = |state: &str| serde_json::json!([{ "hooks": [{ "type": "command", "command": format!("ask-agent --signal {state}") }] }]);
    let padding = user_statusline_padding(project_dir, config_dir);
    let settings = serde_json::json!({
        // `SessionStart` also fires after `/clear` (new session id): `ask-agent --signal session` reads the id from stdin.
        "hooks": { "UserPromptSubmit": hook("busy"), "Stop": hook("idle"), "SessionStart": hook("session") },
        "statusLine": { "type": "command", "command": "ask-agent --usage", "padding": padding },
    });
    let text = serde_json::to_string_pretty(&settings).unwrap_or_default();
    if std::fs::read_to_string(&path).ok().as_deref() != Some(text.as_str()) {
        std::fs::write(&path, text)?;
    }
    Ok(path)
}

/// `padding` of the user's own statusline, in Claude Code's order of precedence (local project,
/// project and user). Without one of theirs, 0 (the default).
fn user_statusline_padding(project_dir: &Path, config_dir: &Path) -> serde_json::Value {
    let files = [
        project_dir.join(".claude").join("settings.local.json"),
        project_dir.join(".claude").join("settings.json"),
        config_dir.join("settings.json"),
    ];
    files.iter().find_map(|f| user_statusline_in(f)).and_then(|sl| sl.get("padding").cloned()).unwrap_or_else(|| serde_json::Value::from(0))
}

/// `statusLine` of a Claude Code configuration file, when it is a user command (our own
/// `ask-agent --usage` does not count).
fn user_statusline_in(file: &Path) -> Option<serde_json::Value> {
    let v: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(file).ok()?).ok()?;
    let sl = v.get("statusLine")?;
    if sl.get("type").and_then(serde_json::Value::as_str).is_some_and(|t| t != "command") {
        return None;
    }
    let cmd = sl.get("command")?.as_str()?.trim();
    (!cmd.is_empty() && !cmd.contains("ask-agent --usage")).then(|| sl.clone())
}

/// Name of the file (in `data/`) with the Holt instructions, for the CLIs that read instructions from a file.
const INSTRUCTIONS_FILE: &str = "holt-instructions.md";

/// Writes `data/holt-instructions.md` (only if it changed) and returns the path. It stays outside the user's repository.
fn write_instructions_file(data_dir: &Path, text: &str) -> std::io::Result<PathBuf> {
    std::fs::create_dir_all(data_dir)?;
    let path = data_dir.join(INSTRUCTIONS_FILE);
    let text = format!(
        "{text}
"
    );
    if std::fs::read_to_string(&path).ok().as_deref() != Some(text.as_str()) {
        std::fs::write(&path, text)?;
    }
    Ok(path)
}

/// Arguments that pass the Holt instructions to `kind`, without touching the user's repository. Claude
/// (`--append-system-prompt`) and OpenCode (configuration, see [`models::opencode_tui`]) are assembled separately.
/// - Codex: `-c developer_instructions=…` (the value is TOML; in a `.cmd` shim the quotes do not survive and the text
///   goes raw, which Codex accepts as text when it is not valid TOML);
/// - Aider: `--read <file>` (read-only file);
/// - Open Interpreter: `--custom_instructions` (adds to the system prompt).
fn instruction_args(kind: &str, text: &str, file: Option<&Path>, shim: bool) -> Vec<String> {
    match kind {
        "codex" => {
            let value = if shim { text.to_owned() } else { serde_json::Value::from(text).to_string() };
            vec!["-c".into(), format!("developer_instructions={value}")]
        }
        "aider" => file.map(|f| vec!["--read".into(), f.to_string_lossy().into_owned()]).unwrap_or_default(),
        "interpreter" => vec!["--custom_instructions".into(), text.to_owned()],
        _ => Vec::new(),
    }
}

/// CLIs without an instructions option that only read the `AGENTS.md` of the folder they run in: without the
/// file, they do not know about `ask-agent`. For them the app creates an `AGENTS.md` with only its block when the
/// folder has none (see [`Core::ensure_agents_file`]). Antigravity left this group: it reads the `AGENTS.md` of a folder
/// passed in `--add-dir` (see [`write_agy_instructions`]).
fn reads_only_agents_md(kind: &str) -> bool {
    matches!(kind, "goose" | "copilot")
}

/// Writes `data/agy/<instance>/AGENTS.md` with the instance instructions (only if it changed) and returns the folder, for
/// Antigravity's `--add-dir`: `agy` reads the `AGENTS.md` of the added folders together with the project one (checked on
/// 2026-10-24 with 1.4.7), so nothing is created in the user's repository.
fn write_agy_instructions(data_dir: &Path, id: &str, text: &str) -> std::io::Result<PathBuf> {
    let dir = data_dir.join("agy").join(id);
    std::fs::create_dir_all(&dir)?;
    let path = dir.join("AGENTS.md");
    let text = format!("{text}\n");
    if std::fs::read_to_string(&path).ok().as_deref() != Some(text.as_str()) {
        std::fs::write(&path, text)?;
    }
    Ok(dir)
}

/// Assembles the command of an interactive tab (equivalent to `pty:spawn`).
pub(crate) fn spawn_spec(core: &Core, project: &str, kind: &str) -> Result<SpawnSpec, String> {
    spawn_spec_for(core, project, &Ident::main(kind))
}

/// Like [`spawn_spec`], for an instance: `HOLT_AGENT` gets its id and the role goes
/// in `HOLT_ROLE` and in the system prompt. The terminal opens in the folder of project `project`
/// (on the first tab of a project that is not the active one, its AGENTS.md is written).
pub(crate) fn spawn_spec_for(core: &Core, project: &str, ident: &Ident) -> Result<SpawnSpec, String> {
    let project_dir = core.project_path(project).ok_or_else(|| t!("Unknown project: {project}", project))?;
    let kind = ident.kind.as_str();
    let id = ident.id();
    // A task instance works in its own worktree, not in the project folder.
    let cwd = match core.task_cwd(project, &id) {
        Some(dir) if dir.is_dir() => dir,
        Some(dir) => return Err(t!("The task folder no longer exists ({dir}). Discard the task on the tasks desk.", dir = dir.display())),
        None => project_dir.clone(),
    };
    let reg = &core.inner.agents;
    let def = agent_def(kind).ok_or_else(|| t!("Unknown agent: {kind}", kind))?;
    let data_dir = core.paths().data_dir.to_string_lossy().into_owned();
    // No CLI depends on the user's `AGENTS.md`: each one gets the instructions through the channel it has.
    let instructions = reg.bridge_instructions_for(ident);
    let generic_file = write_instructions_file(&core.paths().data_dir, &reg.bridge_instructions(None))
        .map_err(|e| log::warn!("could not write the Holt instructions file: {e}"))
        .ok();
    let mut env = agent_env(core, &id, 0, &core.tab_token(project, &id));
    if let Some(role) = &ident.role {
        env.insert("HOLT_ROLE".into(), role.clone());
    }

    // Requested > saved > role default; it is saved for the interface and for reopening the app.
    let model = core.assign_model(project, ident, None);
    let resolved = reg.resolve(kind).ok_or_else(|| def.install_hint().to_owned())?;
    let mut args = Vec::new();
    // Initial prompt of a newly opened task (see `Core::set_boot_prompt`). In Claude it is positional and goes
    // first: `--add-dir` and `--allowedTools` accept several values and would swallow the text.
    let boot = models::boot_prompt_flag(kind).and_then(|flag| core.take_boot_prompt(project, &id).map(|p| (flag, p)));
    if let Some((None, prompt)) = &boot {
        args.push(prompt.clone());
    }
    // The tab's own conversation (never "the last one": see `resume.rs`). OpenCode's goes next to `--standalone` below.
    let launch = core.plan_launch(project, ident);
    let resumed = launch.as_ref().is_some_and(|l| l.resumed);
    let launch_args = || launch.iter().flat_map(|l| l.args.iter().cloned());
    let mut session_log = None;
    if kind == "codex" {
        // `resume <id>` is a subcommand: before every other option (Codex has no initial-prompt flag, so nothing precedes it).
        args.extend(launch_args());
    }
    if kind == "claude" {
        args.extend(launch_args());
        args.push("--append-system-prompt".into());
        args.push(instructions.clone());
        // Answering another agent means running `ask-agent`: allow only that command, without asking.
        args.push("--allowedTools".into());
        args.push("Bash(ask-agent:*)".into());
        // Hooks tell the start and the end of each turn (see `delivery.rs`).
        match write_claude_hooks(&core.paths().data_dir, &project_dir) {
            Ok(path) => {
                args.push("--settings".into());
                args.push(path.to_string_lossy().into_owned());
            }
            Err(e) => log::warn!("could not write the Claude hooks: {e}"),
        }
    }
    if kind == "codex" && resolved.shim.is_none() {
        // Codex end of turn → `ask-agent --signal idle <json>`; the JSON carries the thread id that `codex resume` takes.
        // Through cmd.exe the TOML value (with quotes) does not survive, so only with the direct executable.
        args.push("-c".into());
        args.push(codex_notify_arg(&core.paths().bin_dir));
    }
    args.extend(instruction_args(kind, &instructions, generic_file.as_deref(), resolved.shim.is_some()));
    // Without the "Do you trust this folder?" question, which left the tab opened in the background stuck.
    args.extend(crate::trust::prepare(kind, &cwd, &project_dir, &core.inner.home));
    if kind == "antigravity" {
        // The conversation id is not ours to choose: `agy` names it in its log (see `resume::agy_session_in_log`). A new
        // process truncates the file, but a stale one is removed anyway so no old id is ever read.
        let log = resume::agy_log_path(&core.paths().data_dir, &id);
        if std::fs::create_dir_all(log.parent().unwrap_or(&core.paths().data_dir))
            .and_then(|()| match std::fs::remove_file(&log) {
                Err(e) if e.kind() != std::io::ErrorKind::NotFound => Err(e),
                _ => Ok(()),
            })
            .is_ok()
        {
            args.extend(launch_args());
            args.push("--log-file".into());
            args.push(log.to_string_lossy().into_owned());
            session_log = Some(log);
        } else {
            log::warn!("could not prepare the Antigravity log {}: this tab will not resume", log.display());
        }
    }
    if kind == "claude" || kind == "antigravity" {
        args.push("--add-dir".into());
        args.push(data_dir);
    }
    if kind == "antigravity" {
        match write_agy_instructions(&core.paths().data_dir, &id, &instructions) {
            Ok(dir) => {
                args.push("--add-dir".into());
                args.push(dir.to_string_lossy().into_owned());
            }
            Err(e) => log::warn!("could not write the Antigravity instructions: {e}"),
        }
    }
    if kind == "opencode" {
        // The TUI has no --model or --system: its own server + model and instructions in the configuration
        // (see `models::opencode_tui`).
        let (extra, (key, value)) = models::opencode_tui(model.as_deref(), generic_file.as_deref());
        args.extend(extra);
        args.extend(launch.iter().flat_map(|l| l.args.iter().cloned()));
        env.insert(key, value);
    } else if let Some(m) = &model {
        args.extend(models::cli_args(kind, m));
    }
    // Task effort (Antigravity's is already in the model name; see `Task::effort`).
    if let Some(effort) = core.task_of_instance(project, &id).and_then(|t| t.effort) {
        args.extend(models::effort_args(kind, effort));
    }
    if let Some((Some(flag), prompt)) = boot {
        args.push(flag.into());
        args.push(prompt);
    }
    if resolved.shim.is_some() {
        args = args.iter().map(|a| cmd_safe_arg(a)).collect();
    }
    let mut full = resolved.pre;
    full.extend(args);

    core.ensure_project_context(project, &project_dir);
    if reads_only_agents_md(kind) {
        core.ensure_agents_file(project, &cwd);
    }
    Ok(SpawnSpec {
        program: resolved.program,
        args: full,
        cwd,
        env,
        label: def.label.to_owned(),
        color: def.color.to_owned(),
        kind: kind.to_owned(),
        double_enter_on_mention: kind == "claude" || kind == "antigravity",
        model,
        resumed: resumed && (kind != "antigravity" || session_log.is_some()),
        session_log,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn registry() -> Registry {
        Registry::new(Discovery { search_path: Some(OsString::new()), home: PathBuf::from("/h"), system_dirs: false })
    }

    /// The instructions do not grow beyond what is needed (they were 2580 characters before t38; t40 added `--outline`
    /// and t64's English text went a little past the old limit) and carry the subagent rule, the bridge and the general memory.
    #[test]
    fn the_instructions_carry_the_rules_and_stay_short() {
        let reg = registry();
        let text = reg.bridge_instructions(Some("claude@dev"));
        assert!(text.chars().count() <= 2800, "{} characters", text.chars().count());
        for needle in [
            "do NOT use your CLI's internal subagents (Task/Agent tool, sub-agents) unless the user asks for it",
            "ask-agent <agent>@<role> or ask-agent --task new",
            "choose among the models capable of the size of the work, and among them the one with the most free quota",
            "memory/learnings/",
            "title:",
            "summary:",
            "touches:",
            "ask-agent --learn",
            "ask-agent --outline <file>",
            "ask-agent --reply",
            "ask-agent --boss",
        ] {
            assert!(text.contains(needle), "missing {needle:?}");
        }
        assert!(text.contains("Your name here is \"claude@dev\"") && text.contains("(agents: )"), "{text}");
    }

    /// The file text (the same for any tab) does not list agents: it does not change from machine to machine.
    #[test]
    fn the_generic_instructions_do_not_list_the_installed_agents() {
        let reg = registry();
        let text = reg.bridge_instructions(None);
        assert!(text.contains("HOLT_AGENT") && text.contains("HOLT_ROLE") && !text.contains("(agents:"), "{text}");
    }

    #[test]
    fn each_cli_gets_its_own_instruction_channel() {
        let file = Path::new("/data/holt-instructions.md");
        let text = "Você \"é\"
ok";
        // Codex: TOML value (basic string) with quotes and line break escaped; in a shim, the raw text.
        assert_eq!(instruction_args("codex", text, Some(file), false), ["-c", r#"developer_instructions="Você \"é\"\nok""#]);
        assert_eq!(
            instruction_args("codex", text, Some(file), true),
            [
                "-c",
                "developer_instructions=Você \"é\"
ok"
            ]
        );
        assert_eq!(instruction_args("aider", text, Some(file), false), ["--read", "/data/holt-instructions.md"]);
        assert!(instruction_args("aider", text, None, false).is_empty());
        assert_eq!(instruction_args("interpreter", text, Some(file), false), ["--custom_instructions", text]);
        // Claude and OpenCode are assembled separately; Antigravity reads the AGENTS.md.
        for kind in ["claude", "opencode", "antigravity", "goose", "shell"] {
            assert!(instruction_args(kind, text, Some(file), false).is_empty(), "{kind}");
        }
        assert!(["goose", "copilot"].iter().all(|k| reads_only_agents_md(k)));
        assert!(["claude", "codex", "opencode", "aider", "interpreter", "shell", "antigravity"].iter().all(|k| !reads_only_agents_md(k)));
    }

    #[test]
    fn the_instructions_file_is_rewritten_only_when_it_changes() {
        let tmp = tempfile::tempdir().expect("tmp");
        let data_dir = tmp.path().join("data");
        let path = write_instructions_file(&data_dir, "one").expect("write");
        assert_eq!(path, data_dir.join(INSTRUCTIONS_FILE));
        assert_eq!(
            std::fs::read_to_string(&path).expect("ler"),
            "one
"
        );
        let before = std::fs::metadata(&path).and_then(|m| m.modified()).expect("mtime");
        write_instructions_file(&data_dir, "one").expect("again");
        assert_eq!(std::fs::metadata(&path).and_then(|m| m.modified()).expect("mtime"), before);
        write_instructions_file(&data_dir, "two").expect("changes");
        assert_eq!(
            std::fs::read_to_string(&path).expect("read"),
            "two
"
        );
    }

    #[test]
    fn aliases() {
        assert_eq!(resolve_alias("agy"), "antigravity");
        assert_eq!(resolve_alias("amazon-q"), "q");
        assert_eq!(resolve_alias("claude"), "claude");
    }

    #[test]
    fn env_is_sanitized() {
        let base = vec![
            ("CLAUDECODE".to_owned(), "1".to_owned()),
            ("CLAUDE_CODE_ENTRYPOINT".to_owned(), "cli".to_owned()),
            ("HOME".to_owned(), "/h".to_owned()),
            ("PATH".to_owned(), "/usr/bin".to_owned()),
            // Inherited from a Holt that opened this one (app running inside a tab).
            ("HOLT_ROLE".to_owned(), "dev".to_owned()),
            ("HOLT_HOME".to_owned(), "/outro".to_owned()),
            ("HOLT_TOKEN".to_owned(), "old".to_owned()),
            ("HOLT_DEBUG_UI".to_owned(), "1".to_owned()),
            // User's configuration: still valid.
            ("HOLT_ASK_WAIT".to_owned(), "300".to_owned()),
            // Model of the OpenCode tab that opened this app: it does not reach the agents.
            ("OPENCODE_CONFIG_CONTENT".to_owned(), r#"{"model":"herdado"}"#.to_owned()),
        ];
        let env = build_agent_env(base, Path::new("/app/bin"), 1234, "tok", "claude", 1);
        for k in ["HOLT_ROLE", "HOLT_HOME", "HOLT_DEBUG_UI"] {
            assert!(!env.contains_key(k), "{k} inherited");
        }
        assert_eq!(env["HOLT_TOKEN"], "tok");
        assert_eq!(env["HOLT_ASK_WAIT"], "300");
        assert!(!env.contains_key("CLAUDECODE"));
        assert!(!env.contains_key("CLAUDE_CODE_ENTRYPOINT"));
        assert!(!env.contains_key("OPENCODE_CONFIG_CONTENT"), "the configuration of the OpenCode tab that opened the app leaked");
        assert_eq!(env["HOME"], "/h");
        assert_eq!(env["PATH"], format!("/app/bin{PATH_SEP}/usr/bin"));
        assert_eq!(env["HOLT_PORT"], "1234");
        assert_eq!(env["HOLT_TOKEN"], "tok");
        assert_eq!(env["HOLT_AGENT"], "claude");
        assert_eq!(env["HOLT_DEPTH"], "1");
        assert_eq!(env["TERM"], "xterm-256color");
        assert_eq!(env["COLORTERM"], "truecolor");
    }

    #[test]
    fn cmd_args_are_escaped_not_mangled() {
        // With a space portable-pty adds quotes: only `%`, quotes and line breaks change.
        assert_eq!(cmd_safe_arg("a \"b\" & calc | x <y> ^z %PATH% !v!\r\nfim\n"), "a b & calc | x <y> ^z %%cd:~,%PATH%%cd:~,% !v! fim ");
        assert_eq!(cmd_safe_arg(r"C:\Users\ana\data"), r"C:\Users\ana\data");
        // Without a space, without quotes: carets for the two cmd passes.
        assert_eq!(cmd_safe_arg(r"C:\Users\Ana&Bia\data"), r"C:\Users\Ana^^^&Bia\data");
        assert_eq!(cmd_safe_arg("Bash(ask-agent:*)"), "Bash^^^(ask-agent:*^^^)");
        assert_eq!(cmd_safe_arg("50%!"), "50%%cd:~,%!");
    }

    /// Command line as portable-pty assembles it (`append_quoted`) for arguments without quotes.
    #[cfg(windows)]
    fn pty_quoted(arg: &str) -> String {
        if !arg.is_empty() && !arg.contains([' ', '\t', '\n', '\x0b', '"']) {
            return arg.to_owned();
        }
        let trailing = arg.len() - arg.trim_end_matches('\\').len();
        format!("\"{arg}{}\"", "\\".repeat(trailing))
    }

    /// Paths and app text go through the `cmd.exe /c` and the `%*` of a shim (the two npm
    /// formats: single line and `IF … ( … ) ELSE ( … )` block) without breaking or expanding anything.
    #[cfg(windows)]
    #[test]
    fn escaped_args_reach_the_shim_intact() {
        use std::os::windows::process::CommandExt;
        let tmp = tempfile::tempdir().expect("tmp");
        std::fs::write(tmp.path().join("line.cmd"), "@ECHO off\r\nSETLOCAL\r\necho(%*\r\n").expect("shim");
        std::fs::write(tmp.path().join("block.cmd"), "@IF EXIST \"%~dp0\\nothing.exe\" (\r\n  echo(%*\r\n) ELSE (\r\n  echo(%*\r\n)\r\n")
            .expect("shim");
        let args = [
            r"C:\Users\Ana&Bia\data",
            r"C:\Users\Ana Bia&X (1)\data\",
            "Bash(ask-agent:*)",
            "50%PATH%!PATH!^x|y<z>w",
            "text with %PATH% & quotes \"x\" | end",
        ];
        let expected: Vec<String> = args.iter().map(|a| pty_quoted(&a.replace('"', ""))).collect();
        for shim in ["line", "block"] {
            let mut tail: Vec<String> = CMD_FLAGS.map(String::from).to_vec();
            tail.push(format!(r".\{shim}")); // without depending on NoDefaultCurrentDirectoryInExePath
            tail.extend(args.iter().map(|a| pty_quoted(&cmd_safe_arg(a))));
            let out = std::process::Command::new("cmd.exe")
                .raw_arg(tail.join(" "))
                .current_dir(tmp.path())
                .env("PATH", std::env::var_os("PATH").unwrap_or_default())
                .output()
                .expect("cmd");
            let stdout = String::from_utf8_lossy(&out.stdout);
            assert_eq!(stdout.trim_end_matches(['\r', '\n']), expected.join(" "), "{shim}: {}", String::from_utf8_lossy(&out.stderr));
        }
    }

    #[test]
    fn a_cmd_shim_is_called_directly_by_std() {
        let r = Resolved::via_cmd(PathBuf::from(r"C:\npm\gh.cmd"), "gh", &["copilot"]);
        assert_eq!(r.program, PathBuf::from("cmd.exe"));
        assert_eq!(r.pre, ["/d", "/e:on", "/v:off", "/c", "gh", "copilot"]);
        assert_eq!(r.command(), (PathBuf::from(r"C:\npm\gh.cmd"), vec!["copilot".to_owned()]));
        let d = Resolved::direct(PathBuf::from("/bin/claude"));
        assert_eq!(d.command(), (PathBuf::from("/bin/claude"), Vec::new()));
    }

    #[test]
    fn claude_hooks_signal_the_bridge() {
        let tmp = tempfile::tempdir().expect("tmp");
        let path =
            write_claude_hooks_with(&tmp.path().join("data"), &tmp.path().join("project"), &tmp.path().join("claude")).expect("hooks");
        let v: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(&path).expect("ler")).expect("json");
        assert_eq!(v["hooks"]["Stop"][0]["hooks"][0]["command"], "ask-agent --signal idle");
        assert_eq!(v["hooks"]["UserPromptSubmit"][0]["hooks"][0]["command"], "ask-agent --signal busy");
        // `/clear` starts a new conversation: the hook reports its id (the JSON arrives on stdin).
        assert_eq!(v["hooks"]["SessionStart"][0]["hooks"][0]["command"], "ask-agent --signal session");
        assert_eq!(v["statusLine"], serde_json::json!({ "type": "command", "command": "ask-agent --usage", "padding": 0 }));
        assert_eq!(
            write_claude_hooks_with(&tmp.path().join("data"), &tmp.path().join("project"), &tmp.path().join("claude")).expect("again"),
            path
        );
    }

    #[test]
    fn claude_hooks_keep_the_user_statusline_padding() {
        let tmp = tempfile::tempdir().expect("tmp");
        let data = tmp.path().join("data");
        let project = tmp.path().join("project");
        let config = tmp.path().join("claude");
        let statusline = |padding: serde_json::Value| serde_json::json!({ "statusLine": { "type": "command", "command": "my-status", "padding": padding } });
        let write = |file: &Path, json: &serde_json::Value| {
            std::fs::create_dir_all(file.parent().expect("pai")).expect("mkdir");
            std::fs::write(file, json.to_string()).expect("write");
        };
        let statusline_padding = || {
            let path = write_claude_hooks_with(&data, &project, &config).expect("hooks");
            let v: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(&path).expect("ler")).expect("json");
            v["statusLine"]["padding"].clone()
        };

        // Without the user's statusline: 0, as before.
        assert_eq!(statusline_padding(), serde_json::json!(0));

        // The user padding is the one of our statusline (Claude Code applies it).
        write(&config.join("settings.json"), &statusline(serde_json::json!(2)));
        assert_eq!(statusline_padding(), serde_json::json!(2));

        // The local project wins over the user one.
        write(&project.join(".claude").join("settings.local.json"), &statusline(serde_json::json!(1)));
        assert_eq!(statusline_padding(), serde_json::json!(1));

        // Our own statusline (`ask-agent --usage`) does not count as the user's.
        write(
            &project.join(".claude").join("settings.local.json"),
            &serde_json::json!({ "statusLine": { "type": "command", "command": "ask-agent --usage", "padding": 7 } }),
        );
        assert_eq!(statusline_padding(), serde_json::json!(2));
    }

    /// An npm global install of Codex: `<npm>/codex.cmd` and the platform package under `node_modules`.
    fn fake_codex_install(npm: &Path, nested: bool) -> PathBuf {
        let triple = if cfg!(target_arch = "aarch64") { "aarch64-pc-windows-msvc" } else { "x86_64-pc-windows-msvc" };
        let platform = if cfg!(target_arch = "aarch64") { "codex-win32-arm64" } else { "codex-win32-x64" };
        let package = npm.join("node_modules").join("@openai").join("codex");
        let vendor = if nested {
            package.join("node_modules").join("@openai").join(platform).join("vendor")
        } else {
            npm.join("node_modules").join("@openai").join(platform).join("vendor")
        };
        let bin = vendor.join(triple).join("bin");
        std::fs::create_dir_all(&bin).expect("mkdir");
        std::fs::write(npm.join("codex.cmd"), "@echo off\r\n").expect("shim");
        std::fs::write(bin.join("codex.exe"), "").expect("exe");
        bin.join("codex.exe")
    }

    #[test]
    fn the_codex_npm_shim_resolves_to_the_native_executable() {
        // Where npm puts the platform package (nested under `@openai/codex`, as `codex.js` looks first)...
        let tmp = tempfile::tempdir().expect("tmp");
        let exe = fake_codex_install(tmp.path(), true);
        assert_eq!(codex_native_exe(&tmp.path().join("codex.cmd")), Some(exe));
        // ...or hoisted next to it.
        let tmp = tempfile::tempdir().expect("tmp");
        let exe = fake_codex_install(tmp.path(), false);
        assert_eq!(codex_native_exe(&tmp.path().join("codex.cmd")), Some(exe));
        // Nothing there (a different layout): the shim stays, with no `notify`.
        let tmp = tempfile::tempdir().expect("tmp");
        std::fs::write(tmp.path().join("codex.cmd"), "").expect("shim");
        assert_eq!(codex_native_exe(&tmp.path().join("codex.cmd")), None);
    }

    #[cfg(windows)]
    #[test]
    fn on_windows_the_codex_shim_is_launched_as_the_executable_so_it_gets_the_hook() {
        let tmp = tempfile::tempdir().expect("tmp");
        let npm = tmp.path().join("npm");
        let exe = fake_codex_install(&npm, true);
        let reg =
            Registry::new(Discovery { search_path: Some(npm.clone().into_os_string()), home: tmp.path().join("h"), system_dirs: false });
        let r = reg.resolve("codex").expect("codex");
        assert!(r.shim.is_none() && r.program == exe && r.pre.is_empty(), "{r:?}");
        // Without the platform package it is still the shim, through cmd.exe.
        std::fs::remove_dir_all(npm.join("node_modules")).expect("rm");
        let r = reg.resolve("codex").expect("codex");
        assert_eq!(r.shim, Some(npm.join("codex.cmd")));
    }

    #[test]
    fn the_codex_notify_hook_is_ask_agent_by_its_path_when_the_app_has_it() {
        let tmp = tempfile::tempdir().expect("tmp");
        let toml = |bin: &Path| codex_notify_arg(bin).trim_start_matches("notify=").to_owned();
        // No `ask-agent` next to the app (tests, a partial install): the bare name, resolved through the tab's PATH.
        let value: serde_json::Value = serde_json::from_str(&toml(tmp.path())).expect("array");
        assert_eq!(value, serde_json::json!(["ask-agent", "--signal", "idle"]));
        let exe = tmp.path().join(if cfg!(windows) { "ask-agent.exe" } else { "ask-agent" });
        std::fs::write(&exe, "").expect("exe");
        let value: serde_json::Value = serde_json::from_str(&toml(tmp.path())).expect("array");
        assert_eq!(value, serde_json::json!([exe.to_string_lossy(), "--signal", "idle"]));
    }

    #[cfg(windows)]
    #[test]
    fn env_path_casing_windows() {
        let base = vec![("Path".to_owned(), "C:\\Windows".to_owned())];
        let env = build_agent_env(base, Path::new("C:\\app"), 1, "t", "shell", 0);
        assert!(!env.contains_key("Path"));
        assert_eq!(env["PATH"], "C:\\app;C:\\Windows");
        // Git Bash does not turn "/review" into a path.
        assert_eq!((env["MSYS_NO_PATHCONV"].as_str(), env["MSYS2_ARG_CONV_EXCL"].as_str()), ("1", "*"));
    }
}
