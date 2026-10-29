//! Fake agent for the bridge tests, in place of the old shell scripts: a real binary that behaves
//! the same on Windows and Unix (it is the `agentdeck-core` test `[[bin]]`, see `Cargo.toml`).
//!
//! The test copies this binary into the search folder under the name the app looks for (`claude.exe`
//! on Windows, `claude` on Unix) and writes the behaviour next to it, in `<name>.fake.json`; the app
//! only runs it in tests. JSON fields (all optional):
//!
//! - `stdout`: printed text (one line);
//! - `stderr`: text printed on error; `exit`: exit code (default 0);
//! - `echo_args`: prints `ARGS: [arg]…`; an argument containing "AgentDeck" (the instructions) becomes `[<sys>]`;
//! - `echo_env`: prints `ENV: <values…>` of the listed variables;
//! - `print_env`: prints only the variable's value;
//! - `echo_stdin`: copies stdin to stdout (like `cat`);
//! - `fail_on_arg`: if an argument equals it, prints `stderr` and exits with `fail_exit` (default 1);
//! - `log`: file where it writes `log_first` before `sleep_ms` and `log_last` after.
//!
//! Stdin is always read to the end: without that `ask-agent` could hang writing the prompt into a
//! pipe nobody reads.

use std::io::{Read, Write};
use std::time::Duration;

use serde_json::Value;

fn main() {
    let spec = read_spec();
    let args: Vec<String> = std::env::args().skip(1).collect();

    if spec["fail_on_arg"].as_str().is_some_and(|bad| args.iter().any(|a| a == bad)) {
        let _ = writeln!(std::io::stderr(), "{}", spec["stderr"].as_str().unwrap_or(""));
        std::process::exit(spec["fail_exit"].as_i64().unwrap_or(1) as i32);
    }

    let mut out = Vec::new();
    if spec["echo_args"].as_bool().unwrap_or(false) {
        out.extend_from_slice(b"ARGS:");
        for arg in &args {
            let text = if arg.contains("AgentDeck") { " [<sys>]".to_owned() } else { format!(" [{arg}]") };
            out.extend_from_slice(text.as_bytes());
        }
        out.push(b'\n');
    }
    if let Some(names) = spec["echo_env"].as_array() {
        let values: Vec<String> = names.iter().filter_map(Value::as_str).map(|n| std::env::var(n).unwrap_or_default()).collect();
        out.extend_from_slice(format!("ENV: {}\n", values.join(" ")).as_bytes());
    }
    if let Some(name) = spec["print_env"].as_str() {
        out.extend_from_slice(format!("{}\n", std::env::var(name).unwrap_or_default()).as_bytes());
    }
    if let Some(text) = spec["stdout"].as_str() {
        out.extend_from_slice(format!("{text}\n").as_bytes());
    }

    if let Some(path) = spec["log"].as_str() {
        append(path, spec["log_first"].as_str().unwrap_or(""));
    }
    if let Some(ms) = spec["sleep_ms"].as_u64() {
        std::thread::sleep(Duration::from_millis(ms));
    }

    let mut stdin = Vec::new();
    let _ = std::io::stdin().read_to_end(&mut stdin);
    if spec["echo_stdin"].as_bool().unwrap_or(false) {
        out.extend_from_slice(&stdin);
    }
    if let Some(path) = spec["log"].as_str() {
        append(path, spec["log_last"].as_str().unwrap_or(""));
    }

    // `process::exit` does not run destructors: the streams must be flushed first.
    let mut stdout = std::io::stdout().lock();
    let _ = stdout.write_all(&out);
    let _ = stdout.flush();
    if let Some(text) = spec["stderr"].as_str() {
        let mut stderr = std::io::stderr().lock();
        let _ = writeln!(stderr, "{text}");
        let _ = stderr.flush();
    }
    std::process::exit(spec["exit"].as_i64().unwrap_or(0) as i32);
}

/// The behaviour lives next to the executable, named after it minus the extension: `claude.fake.json`.
fn read_spec() -> Value {
    let exe = std::env::current_exe().expect("path of the fake agent");
    let path = exe.with_extension("fake.json");
    let text = std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("no behaviour of the fake agent at {}: {e}", path.display()));
    serde_json::from_str(&text).unwrap_or_else(|e| panic!("invalid behaviour at {}: {e}", path.display()))
}

fn append(path: &str, line: &str) {
    let mut file = std::fs::OpenOptions::new().create(true).append(true).open(path).expect("fake agent log");
    let _ = writeln!(file, "{line}");
}
