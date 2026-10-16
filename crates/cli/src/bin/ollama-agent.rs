//! `ollama-agent` — CLI interativo do agente Ollama do CommandDeck (porta melhorada de
//! bin/ollama-agent.js).
//!
//! Novidades em relação à versão Node: banner com estado real (servidor, modelos, ponte),
//! edição de linha com histórico e Tab para comandos/modelos, resposta em streaming, chamadas
//! de ferramentas mostradas na hora com prévia do resultado, indicador "pensando…",
//! Ctrl+C interrompe só a geração atual, rodapé com tempo e passos e comandos `/`.
//!
//! Uso: ollama-agent [modelo]   (ou COMMANDDECK_OLLAMA_MODEL; padrão: primeiro modelo instalado)

use std::borrow::Cow;
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
use rustyline::highlight::Highlighter;
use rustyline::hint::Hinter;
use rustyline::history::DefaultHistory;
use rustyline::{CompletionType, Editor, Helper, Validator};

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
    /// Laranja do Ollama (indicador de atividade e destaques).
    orange: &'static str,
    /// Branco forte (a logo do Ollama).
    white: &'static str,
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
                orange: "\x1b[38;5;208m",
                white: "\x1b[97m",
            }
        } else {
            Self { reset: "", bold: "", dim: "", red: "", green: "", yellow: "", cyan: "", orange: "", white: "" }
        }
    }

    fn enabled(&self) -> bool {
        !self.reset.is_empty()
    }
}

// ---------- Logo do Ollama ----------

/// A cabeça da lhama do Ollama em arte de contorno, 20×20 pixels (`#` = traço branco, `.` = fundo):
/// orelhas, cabeça redonda, os dois olhos e o focinho oval com nariz e boca, como a logo em tema
/// escuro. Simétrica.
const LOGO: [&str; 20] = [
    "...###........###...",
    "..#...#......#...#..",
    "..#...#......#...#..",
    "..#...#......#...#..",
    "..#...#......#...#..",
    ".#....#......#....#.",
    ".#....########....#.",
    ".#................#.",
    "#..................#",
    "#..................#",
    "#....##......##....#",
    "#....##......##....#",
    "#..................#",
    "#.....########.....#",
    "#....#..####..#....#",
    "#....#...##...#....#",
    "#....#........#....#",
    "#.....########.....#",
    "#..................#",
    "#..................#",
];
const LOGO_W: usize = 20;

/// A logo em meio-blocos (`▀ ▄ █`): cada linha do terminal leva duas linhas do desenho, e como a
/// célula é umas duas vezes mais alta que larga os pixels ficam quadrados.
fn llama_logo(c: Palette) -> Vec<String> {
    let on = |row: usize, col: usize| LOGO[row].as_bytes()[col] == b'#';
    (0..LOGO.len() / 2)
        .map(|r| {
            let cells: String = (0..LOGO_W)
                .map(|x| match (on(r * 2, x), on(r * 2 + 1, x)) {
                    (true, true) => '█',
                    (true, false) => '▀',
                    (false, true) => '▄',
                    (false, false) => ' ',
                })
                .collect();
            format!("{}{cells}{}", c.white, c.reset)
        })
        .collect()
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

const FRAMES: &[&str] = &["·", "✢", "✳", "✶", "✻", "✽"];

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
                std::thread::sleep(Duration::from_millis(120));
                let Some(t) = weak.upgrade() else { return };
                let mut st = t.spin.lock();
                if st.active {
                    st.frame = (st.frame + 1) % FRAMES.len();
                    let secs = st.since.elapsed().as_secs();
                    let mut out = std::io::stdout().lock();
                    let _ = write!(
                        out,
                        "\r\x1b[2K{}{}{} {}{}… ({secs}s){}",
                        t.c.orange, FRAMES[st.frame], t.c.reset, t.c.dim, st.label, t.c.reset
                    );
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

/// Largura do terminal em colunas (80 se não der para saber).
fn term_width() -> usize {
    terminal_size::terminal_size().map(|(w, _)| w.0 as usize).filter(|w| *w >= 20).unwrap_or(80)
}

/// Linha horizontal da caixa de digitação. Uma coluna a menos que o terminal: ocupar a última
/// coluna deixaria o cursor "pendurado" e o `rustyline` contaria uma linha a mais.
fn rule(c: Palette) -> String {
    format!("{}{}{}", c.dim, "─".repeat(term_width().saturating_sub(1)), c.reset)
}

#[derive(Helper, Validator)]
struct CliHelper {
    models: Vec<String>,
    /// Modelo em uso (mostrado no rodapé).
    model: String,
    c: Palette,
    /// Só desenha a caixa (linha de baixo e rodapé) num terminal de verdade.
    boxed: bool,
}

impl CliHelper {
    /// Texto **puro** exibido depois da linha digitada, em 3 linhas: placeholder, linha de baixo da
    /// caixa e rodapé. O `rustyline` conta esse texto no layout (sabe que a caixa tem 3 linhas de
    /// altura) e o estilo vem de `highlight_hint`.
    fn bottom(&self, line_is_empty: bool) -> String {
        let width = term_width().saturating_sub(1);
        let left = "  ? /help · Tab completa · Ctrl+C interrompe · /exit sai";
        let right = format!("● {}", self.model);
        let gap = width.saturating_sub(left.chars().count() + right.chars().count());
        let footer = if gap >= 2 { format!("{left}{}{right}", " ".repeat(gap)) } else { left.to_owned() };
        let placeholder = if line_is_empty { "Pergunte algo ao modelo… (ou /help)" } else { "" };
        format!("{placeholder}\n{}\n{footer}", "─".repeat(width))
    }
}

impl Hinter for CliHelper {
    type Hint = String;

    fn hint(&self, line: &str, _pos: usize, _ctx: &rustyline::Context<'_>) -> Option<String> {
        self.boxed.then(|| self.bottom(line.is_empty()))
    }
}

impl Highlighter for CliHelper {
    /// Pinta as 3 linhas do hint: tudo apagado, menos a bolinha laranja do modelo no rodapé.
    fn highlight_hint<'h>(&self, hint: &'h str) -> Cow<'h, str> {
        if !self.boxed {
            return Cow::Borrowed(hint);
        }
        let c = self.c;
        let mut lines = hint.split('\n');
        let (placeholder, rule, footer) = (lines.next().unwrap_or(""), lines.next().unwrap_or(""), lines.next().unwrap_or(""));
        let footer = match footer.rfind("● ") {
            Some(i) => {
                format!("{}{}{}{}●{} {}{}{}", c.dim, &footer[..i], c.reset, c.orange, c.reset, c.dim, &footer[i + "● ".len()..], c.reset)
            }
            None => format!("{}{footer}{}", c.dim, c.reset),
        };
        Cow::Owned(format!("{}{placeholder}{}\n{}{rule}{}\n{footer}", c.dim, c.reset, c.dim, c.reset))
    }
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

/// Quem fez a pergunta, se `text` veio da ponte: `[Mensagem do agente "X" recebida pelo CommandDeck]…`.
fn bridge_asker(text: &str) -> Option<String> {
    let rest = text.trim_start().strip_prefix("[Mensagem do agente \"")?;
    let (asker, tail) = rest.split_once('"')?;
    (tail.starts_with(" recebida pelo CommandDeck]") && !asker.is_empty()).then(|| asker.to_owned())
}

/// Devolve `answer` a `asker` pela ponte (`POST /reply`). `Ok(false)` = não havia pergunta pendente
/// (o modelo já tinha respondido pelo `ask-agent`).
fn reply_to_bridge(asker: &str, answer: &str) -> Result<bool, String> {
    let (port, token) =
        (env_nonempty("COMMANDDECK_PORT").ok_or("fora do CommandDeck")?, env_nonempty("COMMANDDECK_TOKEN").ok_or("fora do CommandDeck")?);
    let me = env_nonempty("COMMANDDECK_AGENT").unwrap_or_default();
    let agent: ureq::Agent = ureq::Agent::config_builder().proxy(None).http_status_as_error(false).build().into();
    let body = serde_json::json!({ "from": me, "to": asker, "message": answer });
    let resp = agent
        .post(format!("http://127.0.0.1:{port}/reply"))
        .header("x-commanddeck-token", &token)
        .send_json(&body)
        .map_err(|e| e.to_string())?;
    match resp.status().as_u16() {
        200 => Ok(true),
        404 => Ok(false),
        other => Err(format!("a ponte respondeu {other}")),
    }
}

/// Resposta à ponte quando o turno não produziu texto: quem perguntou não fica esperando à toa.
fn failure_reply(model: &str, why: &str) -> String {
    format!("[ollama:{model}] não consegui responder: {why}")
}

/// Quem é esta instância, para o prompt de sistema: o modelo não lê `COMMANDDECK_AGENT`.
fn identity_prompt(id: Option<&str>, role: Option<&str>) -> Option<String> {
    let id = id.filter(|s| !s.is_empty())?;
    let mut text = format!(
        "Seu nome de instância no CommandDeck é \"{id}\": é por esse nome que os outros agentes falam com você (ask-agent {id} \"mensagem\")."
    );
    if let Some(role) = role.filter(|s| !s.is_empty()) {
        text.push_str(&format!(" Seu cargo na equipe é \"{role}\"."));
    }
    text.push_str(" Para ver o seu chefe e a sua equipe, rode `ask-agent --team`.");
    Some(text)
}

/// Texto vindo do modelo ou das ferramentas, seguro para o terminal: sem ESC, C1 e outros
/// controles (mantém `\n` e `\t`).
fn printable(text: &str) -> String {
    text.chars().filter(|&c| c == '\n' || c == '\t' || !c.is_control()).collect()
}

fn env_nonempty(k: &str) -> Option<String> {
    std::env::var(k).ok().map(|v| v.trim().to_owned()).filter(|v| !v.is_empty())
}

/// Agentes disponíveis na ponte (`GET /agents`), quando rodando dentro do CommandDeck.
fn bridge_peers() -> Option<Vec<String>> {
    let (port, token) = (env_nonempty("COMMANDDECK_PORT")?, env_nonempty("COMMANDDECK_TOKEN")?);
    let agent: ureq::Agent = ureq::Agent::config_builder().proxy(None).timeout_global(Some(Duration::from_millis(1500))).build().into();
    let mut resp = agent.get(format!("http://127.0.0.1:{port}/agents")).header("x-commanddeck-token", &token).call().ok()?;
    let v: serde_json::Value = resp.body_mut().read_json().ok()?;
    let me = env_nonempty("COMMANDDECK_AGENT").unwrap_or_default();
    Some(v.get("agents")?.as_array()?.iter().filter_map(|a| a.as_str()).filter(|a| *a != me && *a != "ollama").map(str::to_owned).collect())
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
        let (o, r, b, d) = (c.orange, c.reset, c.bold, c.dim);
        let (status, installed) = match models {
            Ok(list) => {
                (format!("{}● online{r}{d} · {} modelo(s){r}", c.green, list.len()), list.iter().any(|m| same_model(m, &self.model)))
            }
            Err(_) => (format!("{}● offline{r}", c.red), true),
        };
        let mut text = vec![
            format!("{b}Ollama Agent{r} {d}v{}{r}", env!("CARGO_PKG_VERSION")),
            format!("{o}{}{r} {d}·{r} {status}", self.model),
            format!("{d}{}{r}", self.workspace.display()),
            match bridge_peers() {
                Some(peers) if !peers.is_empty() => format!("{d}ask-agent <agente> \"mensagem\" → {}{r}", peers.join(", ")),
                Some(_) => format!("{d}nenhum outro agente na ponte{r}"),
                None => format!("{d}ponte indisponível (rode dentro do CommandDeck para usar ask-agent){r}"),
            },
        ];
        if !installed {
            text.push(format!("{}⚠ modelo não instalado — use /models ou `ollama pull {}`{r}", c.yellow, self.model));
        }
        // O texto fica ao lado da logo, a partir da terceira linha dela (as orelhas ficam sozinhas).
        let logo = llama_logo(c);
        println!();
        for i in 0..logo.len().max(text.len() + 2) {
            let art = logo.get(i).cloned().unwrap_or_else(|| " ".repeat(LOGO_W));
            let line = i.checked_sub(2).and_then(|j| text.get(j)).map(String::as_str).unwrap_or("");
            println!(" {art}  {line}");
        }
        if let Err(e) = models {
            println!();
            println!("  {}✖ {e}{r}", c.red);
            println!("  {d}Dica: inicie o Ollama (`ollama serve` ou o app) e tente de novo; o host vem de OLLAMA_HOST.{r}");
        }
        println!();
    }

    /// Prompt da caixa de digitação: linha horizontal e `❯`. A linha de baixo e o rodapé vêm do
    /// `Hinter` (ver `CliHelper::bottom`). Fora de um terminal (stdin/stdout redirecionados) é só `> `.
    fn prompt(&self) -> (String, String) {
        if !self.boxed() {
            return ("> ".to_owned(), "> ".to_owned());
        }
        let width = term_width().saturating_sub(1);
        let raw = format!("{}\n❯ ", "─".repeat(width));
        let styled = format!("{}\n{}❯{} ", rule(self.c), self.c.bold, self.c.reset);
        (raw, styled)
    }

    fn boxed(&self) -> bool {
        self.c.enabled() && std::io::stdin().is_terminal()
    }

    fn help(&self) {
        let c = self.c;
        println!("\n{}Comandos:{}", c.bold, c.reset);
        for (cmd, desc) in COMMANDS {
            println!("  {}{cmd:<11}{} {desc}", c.cyan, c.reset);
        }
        println!(
            "\n{}Qualquer outro texto é enviado ao modelo. Ctrl+C interrompe a resposta; Ctrl+C duas vezes no prompt (ou Ctrl+D) sai.{}\n",
            c.dim, c.reset
        );
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

    /// Envia uma mensagem ao modelo, mostrando o progresso em tempo real. Devolve a resposta final
    /// ou o motivo de não haver uma (interrompida ou erro).
    fn turn(&mut self, prompt: &str) -> Result<String, String> {
        let c = self.c;
        let term = self.term.clone();
        let mut opts = RunOptions::new(self.model.clone(), prompt, self.workspace.clone());
        opts.session_key = Some(self.session_key.clone());
        opts.stream = true;
        opts.extra_system = identity_prompt(env_nonempty("COMMANDDECK_AGENT").as_deref(), env_nonempty("COMMANDDECK_ROLE").as_deref());

        self.cancel.store(false, Ordering::SeqCst);
        self.generating.store(true, Ordering::SeqCst);
        let started = Instant::now();
        let mut printed = false;
        let bullet = format!("{}●{} ", c.bold, c.reset);
        term.spin("pensando");
        let result = self.agent.run(opts, &self.cancel, &mut |ev| match ev {
            AgentEvent::ContentDelta(t) => {
                if !printed {
                    term.write(&bullet);
                }
                printed = true;
                term.write(&printable(&t));
            }
            AgentEvent::ThinkingDelta(_) => term.spin("raciocinando"),
            AgentEvent::ToolCall { name, summary } => {
                let (name, summary) = (printable(&name), printable(&summary));
                let (s, cut) = commanddeck_core::util::truncate_chars(summary.lines().next().unwrap_or(""), 110);
                let more = if cut || summary.lines().count() > 1 { "…" } else { "" };
                term.line(&format!("{}●{} {}{name}{}{}({s}{more}){}", c.green, c.reset, c.bold, c.reset, c.dim, c.reset));
                term.spin(&format!("executando {name}"));
            }
            AgentEvent::ToolResult { preview, .. } => {
                let preview = printable(&preview);
                let first = preview.lines().find(|l| !l.trim().is_empty()).unwrap_or("(vazio)");
                let (s, cut) = commanddeck_core::util::truncate_chars(first.trim(), 120);
                let extra = preview.lines().count().saturating_sub(1);
                let more = if extra > 0 {
                    format!(" (+{extra} linhas)")
                } else if cut {
                    "…".into()
                } else {
                    String::new()
                };
                term.line(&format!("  {}⎿  {s}{more}{}", c.dim, c.reset));
                term.spin("pensando");
            }
            AgentEvent::Done { .. } => term.stop(),
        });
        self.generating.store(false, Ordering::SeqCst);
        term.stop();

        let elapsed = started.elapsed().as_secs_f32();
        let answer;
        match result {
            Ok(out) => {
                answer = if out.cancelled {
                    Err("a geração foi interrompida.".to_owned())
                } else if out.text.trim().is_empty() {
                    Err("o modelo terminou sem nenhum texto.".to_owned())
                } else {
                    Ok(out.text.clone())
                };
                if !printed && !out.cancelled {
                    term.write(&format!("{bullet}{}", printable(&out.text)));
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
                term.line(&format!("{}✻ Respondeu em {elapsed:.1}s · {tools} · {} passo(s){}", c.dim, out.steps, c.reset));
            }
            Err(e) => {
                term.newline_if_needed();
                let msg = printable(&e.to_string());
                term.line(&format!("{}✖ Erro: {msg}{}", c.red, c.reset));
                answer = Err(msg.clone());
                if msg.contains("não está rodando") {
                    term.line(&format!(
                        "{}Dica: inicie o Ollama (`ollama serve` ou o app) — host: {}{}",
                        c.dim,
                        ollama::base_url(&self.host),
                        c.reset
                    ));
                } else if msg.contains("not found") {
                    term.line(&format!("{}Dica: veja /models ou baixe com `ollama pull {}`.{}", c.dim, self.model, c.reset));
                }
            }
        }
        println!();
        answer
    }
}

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.iter().any(|a| a == "-h" || a == "--help") {
        println!("Uso: ollama-agent [modelo]\n\nModelo: argumento, COMMANDDECK_OLLAMA_MODEL ou o primeiro instalado.\nServidor: COMMANDDECK_OLLAMA_HOST, OLLAMA_HOST ou {}.", ollama::DEFAULT_HOST);
        return ExitCode::SUCCESS;
    }
    let c = Palette::detect();
    let host =
        env_nonempty("COMMANDDECK_OLLAMA_HOST").or_else(|| env_nonempty("OLLAMA_HOST")).unwrap_or_else(|| ollama::DEFAULT_HOST.into());
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
    rl.set_helper(Some(CliHelper { models: models.unwrap_or_default(), model: app.model.clone(), c, boxed: app.boxed() }));

    let mut exit_armed = false;
    loop {
        if let Some(h) = rl.helper_mut() {
            h.model = app.model.clone(); // o /switch pode ter trocado o modelo
        }
        let line = match rl.readline(&app.prompt()) {
            Ok(l) => l,
            Err(ReadlineError::Interrupted) => {
                if app.boxed() {
                    print!("\x1b[J"); // apaga a linha de baixo da caixa e o rodapé
                    let _ = std::io::stdout().flush();
                }
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
        if app.boxed() {
            // O cursor ficou no começo da linha de baixo da caixa: apaga ela e o rodapé.
            print!("\x1b[J");
            let _ = std::io::stdout().flush();
        }
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
            _ => {
                let answer = app.turn(text);
                // Pergunta de outro agente (digitada pela ponte): a resposta volta sozinha, sem
                // depender de o modelo lembrar de rodar `ask-agent`. Sem resposta, vai o motivo.
                if let Some(asker) = bridge_asker(text) {
                    let answer = answer.unwrap_or_else(|why| failure_reply(&app.model, &why));
                    match reply_to_bridge(&asker, &answer) {
                        Ok(true) => println!("{}↩ resposta enviada a {asker}.{}\n", c.dim, c.reset),
                        Ok(false) => {} // o modelo já respondeu por conta própria
                        Err(e) => println!("{}⚠ não consegui enviar a resposta a {asker}: {e}{}\n", c.yellow, c.reset),
                    }
                }
            }
        }
    }
    println!("{}Encerrando sessão do agente Ollama.{}", c.dim, c.reset);
    ExitCode::SUCCESS
}

#[cfg(test)]
mod tests {
    use super::*;

    fn plain_helper(boxed: bool) -> CliHelper {
        let none = Palette { reset: "", bold: "", dim: "", red: "", green: "", yellow: "", cyan: "", orange: "", white: "" };
        CliHelper { models: Vec::new(), model: "qwen3.5:4b".into(), c: none, boxed }
    }

    #[test]
    fn the_input_box_bottom_is_three_plain_lines() {
        let helper = plain_helper(true);
        let text = helper.bottom(true);
        let lines: Vec<&str> = text.split('\n').collect();
        assert_eq!(lines.len(), 3, "placeholder, linha de baixo e rodapé");
        assert!(lines[0].contains("Pergunte algo"));
        assert!(!lines[1].is_empty() && lines[1].chars().all(|c| c == '─'));
        assert_eq!(lines[1].chars().count(), term_width() - 1, "uma coluna a menos que o terminal");
        assert!(lines[2].contains("/help") && lines[2].ends_with("● qwen3.5:4b"), "{}", lines[2]);
        // O rustyline conta o hint no layout: precisa ser texto puro (o estilo vem do highlight_hint).
        assert!(!text.contains('\x1b'));
        // Com algo digitado não há placeholder (a linha continua existindo, vazia).
        assert_eq!(helper.bottom(false).split('\n').next(), Some(""));
    }

    #[test]
    fn highlighting_keeps_the_three_lines_and_colors_only_the_model_dot() {
        let helper = {
            let mut h = plain_helper(true);
            h.c = Palette { reset: "<r>", bold: "", dim: "<d>", red: "", green: "", yellow: "", cyan: "", orange: "<o>", white: "" };
            h
        };
        let hint = helper.bottom(true);
        let styled = helper.highlight_hint(&hint).into_owned();
        assert_eq!(styled.split('\n').count(), 3);
        assert!(styled.contains("<o>●<r> <d>qwen3.5:4b<r>"), "{styled}");
        // Fora de um terminal (sem caixa) o hint passa como veio.
        assert_eq!(plain_helper(false).highlight_hint("x"), "x");
    }

    #[test]
    fn the_llama_logo_is_a_symmetric_sprite_of_whole_terminal_rows() {
        assert_eq!(LOGO.len() % 2, 0, "cada linha do terminal leva 2 linhas do desenho");
        for row in LOGO {
            assert_eq!(row.len(), LOGO_W);
            assert_eq!(row.chars().rev().collect::<String>(), row, "simétrica: {row}");
            assert!(row.chars().all(|c| c == '#' || c == '.'));
        }
        let rows = llama_logo(plain_helper(true).c);
        assert_eq!(rows.len(), LOGO.len() / 2);
        assert!(rows.iter().all(|r| r.chars().count() == LOGO_W), "{rows:?}");
        // Orelhas, olhos e nariz ficam vazados/brancos como na logo: o desenho tem os dois tons.
        let all: String = rows.concat();
        assert!(all.contains('█') && all.contains('▀') && all.contains('▄') && all.contains(' '));
    }

    #[test]
    fn recognizes_questions_typed_by_the_bridge() {
        let q = "[Mensagem do agente \"claude@dev\" recebida pelo CommandDeck]\n\nqual a capital?";
        assert_eq!(bridge_asker(q).as_deref(), Some("claude@dev"));
        // O terminal pode colar as quebras de linha como nada.
        assert_eq!(bridge_asker("[Mensagem do agente \"driver\" recebida pelo CommandDeck]Você está num teste").as_deref(), Some("driver"));
        assert_eq!(bridge_asker("  [Mensagem do agente \"x\" recebida pelo CommandDeck] oi").as_deref(), Some("x"));
        // Conversa normal do usuário não é pergunta da ponte.
        assert_eq!(bridge_asker("olá, tudo bem?"), None);
        assert_eq!(bridge_asker("[Mensagem do agente \"\" recebida pelo CommandDeck] oi"), None);
        assert_eq!(bridge_asker("[Mensagem do agente \"x\" qualquer coisa] oi"), None);
    }

    #[test]
    fn a_failed_turn_still_answers_the_bridge() {
        assert_eq!(
            failure_reply("qwen3.5:4b", "a geração foi interrompida."),
            "[ollama:qwen3.5:4b] não consegui responder: a geração foi interrompida."
        );
    }

    #[test]
    fn the_model_knows_its_instance_name_and_role() {
        let t = identity_prompt(Some("ollama:qwen3.5:4b@dev"), Some("dev")).expect("id");
        assert!(
            t.contains("\"ollama:qwen3.5:4b@dev\"") && t.contains("cargo na equipe é \"dev\"") && t.contains("ask-agent --team"),
            "{t}"
        );
        assert!(!identity_prompt(Some("ollama"), None).expect("id").contains("cargo"));
        assert_eq!(identity_prompt(None, Some("dev")), None);
    }

    #[test]
    fn model_text_cannot_drive_the_terminal() {
        assert_eq!(printable("a\x1b]52;c;eA==\x07b\u{9b}2J\r\nc\td"), "a]52;c;eA==b2J\nc\td");
        assert_eq!(printable("ação ✓"), "ação ✓");
    }
}
