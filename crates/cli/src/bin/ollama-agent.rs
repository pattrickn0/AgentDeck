//! `ollama-agent` — CLI interativo do agente Ollama do CommandDeck (porta melhorada de
//! bin/ollama-agent.js).
//!
//! Novidades em relação à versão Node: banner com estado real (servidor, modelos, ponte),
//! edição de linha com histórico e Tab para comandos/modelos, resposta em streaming, chamadas
//! de ferramentas mostradas na hora com prévia do resultado, indicador "pensando…",
//! Ctrl+C interrompe só a geração atual, rodapé com tempo e passos e comandos `/`.
//!
//! Uso: ollama-agent [modelo]   (ou COMMANDDECK_OLLAMA_MODEL; padrão: primeiro modelo instalado)

use std::io::{IsTerminal, Write};
use std::path::PathBuf;
use std::process::ExitCode;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use commanddeck_core::agents::DEFAULT_OLLAMA_MODEL;
use commanddeck_core::ollama::{self, AgentEvent, OllamaAgent, RunOptions};
use parking_lot::Mutex;
use rustyline::completion::Completer;
use rustyline::error::ReadlineError;
use rustyline::history::DefaultHistory;
use rustyline::{CompletionType, Editor, Helper, Highlighter, Hinter, Validator};

// ---------- Cores ----------

/// Códigos ANSI (vazios quando a saída não é um terminal ou com `NO_COLOR`).
#[derive(Clone, Copy)]
struct Palette {
    reset: &'static str,
    bold: &'static str,
    dim: &'static str,
    red: &'static str,
    green: &'static str,
    yellow: &'static str,
    cyan: &'static str,
    magenta: &'static str,
}

impl Palette {
    fn detect() -> Self {
        let on = std::io::stdout().is_terminal() && std::env::var_os("NO_COLOR").is_none();
        if on {
            Self {
                reset: "\x1b[0m",
                bold: "\x1b[1m",
                dim: "\x1b[2m",
                red: "\x1b[31m",
                green: "\x1b[32m",
                yellow: "\x1b[33m",
                cyan: "\x1b[36m",
                magenta: "\x1b[35m",
            }
        } else {
            Self { reset: "", bold: "", dim: "", red: "", green: "", yellow: "", cyan: "", magenta: "" }
        }
    }

    fn enabled(&self) -> bool {
        !self.reset.is_empty()
    }
}

// ---------- Saída com indicador de atividade ----------

struct SpinState {
    active: bool,
    label: String,
    since: Instant,
    frame: usize,
}

/// Terminal compartilhado entre a thread principal e a do indicador ("spinner").
/// Toda escrita passa pelo mesmo lock, então o indicador nunca embaralha a resposta.
struct Term {
    c: Palette,
    spin: Mutex<SpinState>,
    /// A última coisa impressa terminou em `\n`.
    at_line_start: AtomicBool,
}

const FRAMES: &[&str] = &["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"];

impl Term {
    fn new(c: Palette) -> Arc<Self> {
        let t = Arc::new(Self {
            c,
            spin: Mutex::new(SpinState { active: false, label: String::new(), since: Instant::now(), frame: 0 }),
            at_line_start: AtomicBool::new(true),
        });
        if c.enabled() {
            let weak = Arc::downgrade(&t);
            let _ = std::thread::Builder::new().name("spinner".into()).spawn(move || loop {
                std::thread::sleep(Duration::from_millis(90));
                let Some(t) = weak.upgrade() else { return };
                let mut st = t.spin.lock();
                if st.active {
                    st.frame = (st.frame + 1) % FRAMES.len();
                    let secs = st.since.elapsed().as_secs();
                    let mut out = std::io::stdout().lock();
                    let _ = write!(out, "\r\x1b[2K{}{}{} {}{} {secs}s{}", t.c.magenta, FRAMES[st.frame], t.c.reset, t.c.dim, st.label, t.c.reset);
                    let _ = out.flush();
                }
            });
        }
        t
    }

    /// Mostra o indicador com `label` (no início de uma linha).
    fn spin(&self, label: &str) {
        if !self.c.enabled() {
            return;
        }
        let mut st = self.spin.lock();
        if !self.at_line_start.load(Ordering::SeqCst) {
            println!();
            self.at_line_start.store(true, Ordering::SeqCst);
        }
        if !st.active {
            st.since = Instant::now();
        }
        st.active = true;
        st.label = label.to_owned();
    }

    fn stop_locked(&self, st: &mut SpinState) {
        if st.active {
            st.active = false;
            print!("\r\x1b[2K");
            let _ = std::io::stdout().flush();
        }
    }

    fn stop(&self) {
        let mut st = self.spin.lock();
        self.stop_locked(&mut st);
    }

    /// Escreve texto (apaga o indicador antes).
    fn write(&self, text: &str) {
        if text.is_empty() {
            return;
        }
        let mut st = self.spin.lock();
        self.stop_locked(&mut st);
        print!("{text}");
        let _ = std::io::stdout().flush();
        self.at_line_start.store(text.ends_with('\n'), Ordering::SeqCst);
    }

    /// Garante que a próxima escrita comece numa linha nova.
    fn newline_if_needed(&self) {
        if !self.at_line_start.load(Ordering::SeqCst) {
            self.write("\n");
        }
    }

    fn line(&self, text: &str) {
        self.newline_if_needed();
        self.write(&format!("{text}\n"));
    }
}

// ---------- Edição de linha (Tab completa comandos e modelos) ----------

const COMMANDS: &[(&str, &str)] = &[
    ("/help", "mostra esta ajuda"),
    ("/models", "lista os modelos instalados (● = ativo)"),
    ("/switch", "<modelo|nº> troca o modelo (atalho: /model <modelo>)"),
    ("/clear", "reinicia a conversa (esquece o histórico)"),
    ("/tools", "lista as ferramentas do agente"),
    ("/workspace", "mostra a pasta de trabalho"),
    ("/history", "tamanho da conversa (mensagens e tokens aproximados)"),
    ("/exit", "encerra (também: /sair, sair, exit)"),
];

#[derive(Helper, Hinter, Highlighter, Validator)]
struct CliHelper {
    models: Vec<String>,
}

impl Completer for CliHelper {
    type Candidate = String;

    fn complete(&self, line: &str, pos: usize, _ctx: &rustyline::Context<'_>) -> rustyline::Result<(usize, Vec<String>)> {
        let head = &line[..pos];
        for prefix in ["/switch ", "/model "] {
            if let Some(rest) = head.strip_prefix(prefix) {
                let found = self.models.iter().filter(|m| m.starts_with(rest)).cloned().collect();
                return Ok((prefix.len(), found));
            }
        }
        if head.starts_with('/') && !head.contains(' ') {
            let mut found: Vec<String> =
                COMMANDS.iter().map(|(c, _)| c.to_string()).chain(["/model".to_owned(), "/sair".to_owned()]).collect();
            found.retain(|c| c.starts_with(head));
            return Ok((0, found));
        }
        Ok((pos, Vec::new()))
    }
}

// ---------- Aplicação ----------

struct App {
    c: Palette,
    term: Arc<Term>,
    agent: OllamaAgent,
    host: String,
    model: String,
    workspace: PathBuf,
    session_key: String,
    cancel: Arc<AtomicBool>,
    generating: Arc<AtomicBool>,
}

fn env_nonempty(k: &str) -> Option<String> {
    std::env::var(k).ok().map(|v| v.trim().to_owned()).filter(|v| !v.is_empty())
}

/// Agentes disponíveis na ponte (`GET /agents`), quando rodando dentro do CommandDeck.
fn bridge_peers() -> Option<Vec<String>> {
    let (port, token) = (env_nonempty("COMMANDDECK_PORT")?, env_nonempty("COMMANDDECK_TOKEN")?);
    let agent: ureq::Agent =
        ureq::Agent::config_builder().proxy(None).timeout_global(Some(Duration::from_millis(1500))).build().into();
    let mut resp = agent.get(format!("http://127.0.0.1:{port}/agents")).header("x-commanddeck-token", &token).call().ok()?;
    let v: serde_json::Value = resp.body_mut().read_json().ok()?;
    let me = env_nonempty("COMMANDDECK_AGENT").unwrap_or_default();
    Some(
        v.get("agents")?
            .as_array()?
            .iter()
            .filter_map(|a| a.as_str())
            .filter(|a| *a != me && *a != "ollama")
            .map(str::to_owned)
            .collect(),
    )
}

fn same_model(installed: &str, wanted: &str) -> bool {
    installed == wanted || installed.strip_suffix(":latest") == Some(wanted)
}

impl App {
    fn models(&self) -> Result<Vec<String>, String> {
        ollama::list_models(&self.host, Duration::from_millis(2500))
    }

    fn banner(&self, models: &Result<Vec<String>, String>) {
        let c = self.c;
        let rule = format!("{}{}{}", c.dim, "━".repeat(66), c.reset);
        println!();
        println!("{}{}🦙 Ollama Agent — CommandDeck Terminal{} {}v{}{}", c.bold, c.yellow, c.reset, c.dim, env!("CARGO_PKG_VERSION"), c.reset);
        println!("{rule}");
        let server = ollama::base_url(&self.host);
        match models {
            Ok(list) => {
                let installed = list.iter().any(|m| same_model(m, &self.model));
                println!("  {}Servidor{}     {server}  {}● online{}", c.bold, c.reset, c.green, c.reset);
                let note = if installed {
                    format!("{}({} modelo(s) instalado(s) — /models){}", c.dim, list.len(), c.reset)
                } else {
                    format!("{}⚠ não instalado — use /models ou `ollama pull {}`{}", c.yellow, self.model, c.reset)
                };
                println!("  {}Modelo{}       {}{}{}  {note}", c.bold, c.reset, c.yellow, self.model, c.reset);
            }
            Err(e) => {
                println!("  {}Servidor{}     {server}  {}● offline{} {}({e}){}", c.bold, c.reset, c.red, c.reset, c.dim, c.reset);
                println!("  {}Modelo{}       {}{}{}", c.bold, c.reset, c.yellow, self.model, c.reset);
            }
        }
        println!("  {}Workspace{}    {}", c.bold, c.reset, self.workspace.display());
        let tools: Vec<&str> = ollama::tool_list().iter().map(|(n, _)| *n).collect();
        println!("  {}Ferramentas{}  {}", c.bold, c.reset, tools.join(" · "));
        match bridge_peers() {
            Some(peers) if !peers.is_empty() => {
                println!("  {}Agentes{}      ask-agent <agente> \"mensagem\"  {}→ {}{}", c.bold, c.reset, c.dim, peers.join(", "), c.reset)
            }
            Some(_) => println!("  {}Agentes{}      {}nenhum outro agente instalado na ponte{}", c.bold, c.reset, c.dim, c.reset),
            None => println!(
                "  {}Agentes{}      {}ponte indisponível (rode dentro do CommandDeck para usar ask-agent){}",
                c.bold, c.reset, c.dim, c.reset
            ),
        }
        println!("{rule}");
        println!("  {}/help comandos · Tab completa · Ctrl+C interrompe a resposta · /exit sai{}", c.dim, c.reset);
        if let Err(e) = models {
            println!();
            println!("  {}✖ {e}{}", c.red, c.reset);
            println!("  {}Dica: inicie o Ollama (`ollama serve` ou o app) e tente de novo; o host vem de OLLAMA_HOST.{}", c.dim, c.reset);
        }
        println!();
    }

    fn prompt(&self) -> (String, String) {
        let raw = format!("ollama ({}) › ", self.model);
        let styled = format!("{}{}ollama{} {}({}){} {}›{} ", self.c.bold, self.c.yellow, self.c.reset, self.c.yellow, self.model, self.c.reset, self.c.bold, self.c.reset);
        (raw, styled)
    }

    fn help(&self) {
        let c = self.c;
        println!("\n{}Comandos:{}", c.bold, c.reset);
        for (cmd, desc) in COMMANDS {
            println!("  {}{cmd:<11}{} {desc}", c.cyan, c.reset);
        }
        println!("\n{}Qualquer outro texto é enviado ao modelo. Ctrl+C interrompe a resposta; Ctrl+C duas vezes no prompt (ou Ctrl+D) sai.{}\n", c.dim, c.reset);
    }

    /// Lista os modelos; devolve a lista para o autocompletar.
    fn list_models(&self) -> Vec<String> {
        let c = self.c;
        match self.models() {
            Ok(list) => {
                println!("\n{}Modelos Ollama instalados:{}", c.bold, c.reset);
                if list.is_empty() {
                    println!("  {}(nenhum — instale com `ollama pull <modelo>`){}", c.dim, c.reset);
                }
                for (i, m) in list.iter().enumerate() {
                    if same_model(m, &self.model) {
                        println!("  {}{:>2}. ● {m}{} {}(ativo){}", c.green, i + 1, c.reset, c.bold, c.reset);
                    } else {
                        println!("  {:>2}. ○ {m}", i + 1);
                    }
                }
                println!("\n{}Para trocar: /switch <modelo> ou /switch <nº>{}\n", c.dim, c.reset);
                list
            }
            Err(e) => {
                println!("\n{}✖ {e}{}", c.red, c.reset);
                println!("  {}● {}{} {}(em uso){}\n", c.green, self.model, c.reset, c.dim, c.reset);
                Vec::new()
            }
        }
    }

    fn switch(&mut self, target: &str) {
        let c = self.c;
        if target.is_empty() {
            println!("{}Uso: /switch <nome-do-modelo>{}", c.yellow, c.reset);
            return;
        }
        let list = self.models().unwrap_or_default();
        let chosen = match target.parse::<usize>() {
            Ok(n) if n >= 1 && n <= list.len() => list[n - 1].clone(),
            _ => target.to_owned(),
        };
        self.model = chosen;
        println!("\n{}✓ Modelo alterado para \"{}\".{}", c.green, self.model, c.reset);
        if !list.is_empty() && !list.iter().any(|m| same_model(m, &self.model)) {
            println!("{}⚠ \"{}\" não aparece entre os modelos instalados (veja /models).{}", c.yellow, self.model, c.reset);
        }
        println!();
    }

    fn history(&self) {
        let c = self.c;
        let s = self.agent.session_stats(&self.session_key);
        let turns = s.messages.saturating_sub(1);
        println!(
            "\n{}Conversa:{} {} mensagem(ns) (+ sistema), ~{} tokens. {}O histórico guarda até {} mensagens.{}\n",
            c.bold,
            c.reset,
            turns,
            s.approx_tokens,
            c.dim,
            ollama::MAX_SESSION_MESSAGES,
            c.reset
        );
    }

    fn tools(&self) {
        let c = self.c;
        println!("\n{}Ferramentas (sempre restritas ao workspace):{}", c.bold, c.reset);
        for (name, desc) in ollama::tool_list() {
            println!("  {}{name:<15}{} {desc}", c.cyan, c.reset);
        }
        println!();
    }

    fn show_workspace(&self) {
        let c = self.c;
        let count = std::fs::read_dir(&self.workspace).map(|r| r.count()).unwrap_or(0);
        println!("\n{}Workspace:{} {} {}({count} itens na raiz){}\n", c.bold, c.reset, self.workspace.display(), c.dim, c.reset);
    }

    /// Envia uma mensagem ao modelo, mostrando o progresso em tempo real.
    fn turn(&mut self, prompt: &str) {
        let c = self.c;
        let term = self.term.clone();
        let mut opts = RunOptions::new(self.model.clone(), prompt, self.workspace.clone());
        opts.session_key = Some(self.session_key.clone());
        opts.stream = true;

        self.cancel.store(false, Ordering::SeqCst);
        self.generating.store(true, Ordering::SeqCst);
        let started = Instant::now();
        let mut printed = false;
        term.spin("pensando…");
        let result = self.agent.run(opts, &self.cancel, &mut |ev| match ev {
            AgentEvent::ContentDelta(t) => {
                printed = true;
                term.write(&t);
            }
            AgentEvent::ThinkingDelta(_) => term.spin("raciocinando…"),
            AgentEvent::ToolCall { name, summary } => {
                let (s, cut) = commanddeck_core::util::truncate_chars(summary.lines().next().unwrap_or(""), 110);
                let more = if cut || summary.lines().count() > 1 { "…" } else { "" };
                term.line(&format!("{}⚙ {name}{} {}{s}{more}{}", c.cyan, c.reset, c.dim, c.reset));
                term.spin(&format!("executando {name}…"));
            }
            AgentEvent::ToolResult { preview, .. } => {
                let first = preview.lines().find(|l| !l.trim().is_empty()).unwrap_or("(vazio)");
                let (s, cut) = commanddeck_core::util::truncate_chars(first.trim(), 120);
                let extra = preview.lines().count().saturating_sub(1);
                let more = if extra > 0 { format!(" (+{extra} linhas)") } else if cut { "…".into() } else { String::new() };
                term.line(&format!("  {}↳ {s}{more}{}", c.dim, c.reset));
                term.spin("pensando…");
            }
            AgentEvent::Done { .. } => term.stop(),
        });
        self.generating.store(false, Ordering::SeqCst);
        term.stop();

        let elapsed = started.elapsed().as_secs_f32();
        match result {
            Ok(out) => {
                if !printed && !out.cancelled {
                    term.write(&out.text);
                }
                term.newline_if_needed();
                if out.cancelled {
                    term.line(&format!("{}⏹ geração interrompida (Ctrl+C).{}", c.yellow, c.reset));
                }
                let tools = match out.tool_calls {
                    0 => "sem ferramentas".to_owned(),
                    1 => "1 ferramenta".to_owned(),
                    n => format!("{n} ferramentas"),
                };
                term.line(&format!("{}── {elapsed:.1}s · {tools} · {} passo(s) · {}{}", c.dim, out.steps, self.model, c.reset));
            }
            Err(e) => {
                term.newline_if_needed();
                let msg = e.to_string();
                term.line(&format!("{}✖ Erro: {msg}{}", c.red, c.reset));
                if msg.contains("não está rodando") {
                    term.line(&format!("{}Dica: inicie o Ollama (`ollama serve` ou o app) — host: {}{}", c.dim, ollama::base_url(&self.host), c.reset));
                } else if msg.contains("not found") {
                    term.line(&format!("{}Dica: veja /models ou baixe com `ollama pull {}`.{}", c.dim, self.model, c.reset));
                }
            }
        }
        println!();
    }
}

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.iter().any(|a| a == "-h" || a == "--help") {
        println!("Uso: ollama-agent [modelo]\n\nModelo: argumento, COMMANDDECK_OLLAMA_MODEL ou o primeiro instalado.\nServidor: COMMANDDECK_OLLAMA_HOST, OLLAMA_HOST ou {}.", ollama::DEFAULT_HOST);
        return ExitCode::SUCCESS;
    }
    let c = Palette::detect();
    let host = env_nonempty("COMMANDDECK_OLLAMA_HOST").or_else(|| env_nonempty("OLLAMA_HOST")).unwrap_or_else(|| ollama::DEFAULT_HOST.into());
    let workspace = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    let agent = OllamaAgent::new(host.clone());
    let models = ollama::list_models(&host, Duration::from_millis(2500));
    let model = args
        .first()
        .filter(|a| !a.starts_with('-'))
        .cloned()
        .or_else(|| env_nonempty("COMMANDDECK_OLLAMA_MODEL"))
        .or_else(|| models.as_ref().ok().and_then(|l| l.first().cloned()))
        .unwrap_or_else(|| DEFAULT_OLLAMA_MODEL.into());

    let cancel = Arc::new(AtomicBool::new(false));
    let generating = Arc::new(AtomicBool::new(false));
    {
        // Ctrl+C durante a geração cancela só a resposta; no prompt o rustyline o recebe.
        let (cancel, generating) = (cancel.clone(), generating.clone());
        let _ = ctrlc::set_handler(move || {
            if generating.load(Ordering::SeqCst) {
                cancel.store(true, Ordering::SeqCst);
            }
        });
    }

    let mut app = App {
        c,
        term: Term::new(c),
        agent,
        host,
        model,
        workspace,
        session_key: format!("interactive-{}", commanddeck_core::util::now_ms()),
        cancel,
        generating,
    };
    app.banner(&models);

    let config = rustyline::Config::builder().auto_add_history(false).completion_type(CompletionType::List).build();
    let mut rl: Editor<CliHelper, DefaultHistory> = match Editor::with_config(config) {
        Ok(rl) => rl,
        Err(e) => {
            eprintln!("ollama-agent: terminal sem suporte a edição de linha: {e}");
            return ExitCode::from(1);
        }
    };
    rl.set_helper(Some(CliHelper { models: models.unwrap_or_default() }));

    let mut exit_armed = false;
    loop {
        let line = match rl.readline(&app.prompt()) {
            Ok(l) => l,
            Err(ReadlineError::Interrupted) => {
                if exit_armed {
                    break;
                }
                exit_armed = true;
                println!("{}(Ctrl+C novamente para sair, ou /exit){}", c.dim, c.reset);
                continue;
            }
            Err(ReadlineError::Eof) => break,
            Err(e) => {
                eprintln!("ollama-agent: {e}");
                return ExitCode::from(1);
            }
        };
        exit_armed = false;
        let text = line.trim();
        if text.is_empty() {
            continue;
        }
        let _ = rl.add_history_entry(text);
        let (cmd, rest) = match text.split_once(char::is_whitespace) {
            Some((a, b)) => (a, b.trim()),
            None => (text, ""),
        };
        match cmd.to_lowercase().as_str() {
            "sair" | "exit" | "/exit" | "/sair" | "/quit" => break,
            "/help" | "/?" => app.help(),
            "/models" => {
                let list = app.list_models();
                if let Some(h) = rl.helper_mut() {
                    h.models = list;
                }
            }
            "/model" | "/switch" if rest.is_empty() => {
                let list = app.list_models();
                if let Some(h) = rl.helper_mut() {
                    h.models = list;
                }
            }
            "/model" | "/switch" => app.switch(rest),
            "/clear" => {
                app.agent.clear_session(&app.session_key);
                println!("{}✓ Conversa reiniciada.{}\n", c.green, c.reset);
            }
            "/tools" => app.tools(),
            "/workspace" => app.show_workspace(),
            "/history" => app.history(),
            other if other.starts_with('/') && !other[1..].contains('/') => {
                println!("{}Comando desconhecido: {cmd} — digite /help.{}", c.yellow, c.reset);
            }
            _ => app.turn(text),
        }
    }
    println!("{}Encerrando sessão do agente Ollama.{}", c.dim, c.reset);
    ExitCode::SUCCESS
}
