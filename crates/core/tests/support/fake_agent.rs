//! Agente falso dos testes da ponte, no lugar dos antigos scripts de shell: um binário de verdade,
//! que roda igual no Windows e no Unix (é o `[[bin]]` de teste do `agentdeck-core`, ver `Cargo.toml`).
//!
//! O teste copia este binário para a pasta de busca com o nome que o app procura (`claude.exe` no
//! Windows, `claude` no Unix) e escreve o comportamento ao lado, em `<nome>.fake.json`; o app só o
//! executa nos testes. Campos do JSON (todos opcionais):
//!
//! - `stdout`: texto impresso (uma linha);
//! - `stderr`: texto impresso no erro; `exit`: código de saída (padrão 0);
//! - `echo_args`: imprime `ARGS: [arg]…`; argumento com "AgentDeck" (as instruções) vira `[<sys>]`;
//! - `echo_env`: imprime `ENV: <valores…>` das variáveis listadas;
//! - `print_env`: imprime só o valor da variável;
//! - `echo_stdin`: copia a entrada padrão para a saída (como `cat`);
//! - `fail_on_arg`: se um argumento for igual a ele, imprime `stderr` e sai com `fail_exit` (padrão 1);
//! - `log`: arquivo onde escreve `log_first` antes do `sleep_ms` e `log_last` depois.
//!
//! A entrada padrão é sempre lida até o fim: sem isso o `ask-agent` poderia travar escrevendo o
//! prompt num pipe que ninguém lê.

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

    // `process::exit` não roda destrutores: os fluxos precisam ser descarregados antes.
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

/// O comportamento fica ao lado do executável, com o nome dele menos a extensão: `claude.fake.json`.
fn read_spec() -> Value {
    let exe = std::env::current_exe().expect("caminho do agente falso");
    let path = exe.with_extension("fake.json");
    let text = std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("sem o comportamento do agente falso em {}: {e}", path.display()));
    serde_json::from_str(&text).unwrap_or_else(|e| panic!("comportamento inválido em {}: {e}", path.display()))
}

fn append(path: &str, line: &str) {
    let mut file = std::fs::OpenOptions::new().create(true).append(true).open(path).expect("log do agente falso");
    let _ = writeln!(file, "{line}");
}
