//! Registro de agentes: resolução dos executáveis oficiais, montagem do comando
//! interativo (PTY) e instruções da ponte (equivalente a `AGENTS`, `resolve*`, `peers`,
//! `bridgeInstructions` e `pty:spawn` do main.js).

use std::collections::BTreeMap;
use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use parking_lot::Mutex;

use crate::instances::Ident;
use crate::models;
use crate::Core;

/// Item de menu de agente (equivalente ao retorno de `agents:list`).
#[derive(Debug, Clone, PartialEq)]
pub struct AgentInfo {
    /// `claude`, `antigravity`, `codex`, ..., `shell`.
    pub id: String,
    pub label: String,
    /// Cor `#rrggbb`.
    pub color: String,
    pub installed: bool,
    pub peer: bool,
}

/// Comando completo para abrir uma aba interativa num pseudo-terminal.
#[derive(Debug, Clone)]
pub struct SpawnSpec {
    pub program: PathBuf,
    pub args: Vec<String>,
    pub cwd: PathBuf,
    /// Ambiente COMPLETO a usar (já derivado do ambiente do processo, com PATH/AGENTDECK_* injetados
    /// e CLAUDECODE & cia removidos). A UI deve limpar o ambiente herdado e aplicar este.
    pub env: BTreeMap<String, String>,
    /// Rótulo da aba (ex.: "Claude").
    pub label: String,
    pub color: String,
    /// `kind` original pedido.
    pub kind: String,
    /// Se `true`, a entrega de uma mensagem à aba deve mandar um `\r` extra após 600 ms quando o
    /// texto contém `@` (autocomplete de arquivo do Claude/Antigravity).
    pub double_enter_on_mention: bool,
    /// Modelo de LLM da instância (já resolvido e gravado em `tab_models`); `None` = o padrão do CLI.
    pub model: Option<String>,
}

/// Cores oficiais (usadas também pela UI como fallback).
pub const COLOR_CLAUDE: &str = "#d97757";
pub const COLOR_ANTIGRAVITY: &str = "#4f8cf7";
pub const COLOR_SHELL: &str = "#9aa0a6";

/// Definição estática de um agente (equivalente a uma entrada de `AGENTS`).
#[derive(Debug, Clone, Copy)]
pub struct AgentDef {
    pub id: &'static str,
    pub label: &'static str,
    pub color: &'static str,
    pub peer: bool,
    /// Mensagem de "não instalado" (com `\r\n`, pois vai direto para o terminal).
    pub missing: &'static str,
}

/// O instalador do Antigravity é um script de PowerShell no Windows; nos outros sistemas, o do site.
#[cfg(windows)]
const ANTIGRAVITY_MISSING: &str =
    "Antigravity CLI (agy) não encontrado. Instale no PowerShell com:\r\n  irm https://antigravity.google/cli/install.ps1 | iex";
#[cfg(not(windows))]
const ANTIGRAVITY_MISSING: &str =
    "Antigravity CLI (agy) não encontrado. Veja o instalador para o seu sistema em:\r\n  https://antigravity.google";

/// Todos os agentes, na ordem dos menus.
pub const AGENTS: &[AgentDef] = &[
    AgentDef {
        id: "claude",
        label: "Claude",
        color: COLOR_CLAUDE,
        peer: true,
        missing: "Claude Code CLI (claude) não encontrado. Instale com:\r\n  npm install -g @anthropic-ai/claude-code",
    },
    AgentDef { id: "antigravity", label: "Antigravity", color: COLOR_ANTIGRAVITY, peer: true, missing: ANTIGRAVITY_MISSING },
    AgentDef {
        id: "codex",
        label: "Codex",
        color: "#10a37f",
        peer: true,
        missing: "Codex CLI não encontrado. Instale com:\r\n  npm install -g @openai/codex",
    },
    AgentDef {
        id: "opencode",
        label: "OpenCode",
        color: "#f97316",
        peer: true,
        missing: "OpenCode não encontrado. Instale com:\r\n  npm install -g opencode-ai",
    },
    AgentDef {
        id: "aider",
        label: "Aider",
        color: "#22c55e",
        peer: true,
        missing: "Aider CLI não encontrado. Instale com:\r\n  pip install aider-chat",
    },
    AgentDef {
        id: "goose",
        label: "Goose",
        color: "#10b981",
        peer: true,
        missing: "Goose CLI não encontrado. Instale a partir de: https://github.com/block/goose",
    },
    AgentDef {
        id: "copilot",
        label: "Copilot",
        color: "#6e40c9",
        peer: true,
        missing: "GitHub Copilot CLI não encontrado. Instale com:\r\n  gh extension install github/gh-copilot",
    },
    AgentDef { id: "q", label: "Amazon Q", color: "#ff9900", peer: true, missing: "Amazon Q Developer CLI não encontrado." },
    AgentDef {
        id: "interpreter",
        label: "Interpreter",
        color: "#8b5cf6",
        peer: true,
        missing: "Open Interpreter não encontrado. Instale com:\r\n  pip install open-interpreter",
    },
    AgentDef { id: "shell", label: "Terminal", color: COLOR_SHELL, peer: false, missing: "Shell não encontrado." },
];

/// Apelidos aceitos pela ponte (`ask-agent agy ...`).
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

/// Resolve um apelido (`agy` → `antigravity`); nomes desconhecidos voltam iguais.
pub fn resolve_alias(name: &str) -> &str {
    ALIASES.iter().find(|(a, _)| *a == name).map(|(_, t)| *t).unwrap_or(name)
}

/// Executável resolvido + argumentos fixos iniciais (ex.: `cmd.exe /d /c claude`).
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Resolved {
    pub program: PathBuf,
    pub pre: Vec<String>,
    /// Windows: o agente é um shim `.cmd` (npm/pip), aberto via `cmd.exe /d /c <nome>`. O cmd.exe
    /// interpreta `& | < > ^ % "` e quebras de linha nos argumentos: nada de texto livre neles.
    pub shim: Option<PathBuf>,
}

impl Resolved {
    fn direct(p: PathBuf) -> Self {
        Self { program: p, pre: Vec::new(), shim: None }
    }

    /// `cmd.exe /d /e:on /v:off /c <nome> [extra…]` para o shim em `path`. Extensões ligadas e
    /// expansão atrasada desligada mesmo que o registro diga o contrário: o escape de
    /// [`cmd_safe_arg`] conta com isso (`%cd:~,%` e `!` literal).
    fn via_cmd(path: PathBuf, name: &str, extra: &[&str]) -> Self {
        let mut pre: Vec<String> = CMD_FLAGS.map(String::from).to_vec();
        pre.push(name.to_owned());
        pre.extend(extra.iter().map(|s| s.to_string()));
        Self { program: "cmd.exe".into(), pre, shim: Some(path) }
    }

    /// Programa e argumentos fixos para `std::process::Command`. Um shim `.cmd` é chamado
    /// direto: assim a std do Rust aplica o escape próprio de arquivos em lote (e recusa o que
    /// não dá para escapar), o que não acontece com um `cmd.exe /c` explícito.
    pub fn command(&self) -> (PathBuf, Vec<String>) {
        match &self.shim {
            Some(path) => (path.clone(), self.pre.iter().skip(CMD_FLAGS.len() + 1).cloned().collect()),
            None => (self.program.clone(), self.pre.clone()),
        }
    }
}

/// Opções do `cmd.exe` que abre um shim `.cmd` numa aba (antes do nome do shim).
const CMD_FLAGS: [&str; 4] = ["/d", "/e:on", "/v:off", "/c"];

/// Escapa um argumento para o `cmd.exe /c <shim>` das abas no Windows. O texto passa por dois
/// interpretadores do cmd: a linha do `cmd.exe /c` e a linha do shim que repassa `%*`.
/// - o portable-pty põe entre aspas o argumento com espaço ou tabulação (e só ele): entre aspas,
///   `& | < > ^ ( )` já são literais nas duas passagens;
/// - sem aspas, cada um desses vira `^^^c` (a primeira passagem deixa `^c`, a segunda, `c`);
/// - `%` vira `%%cd:~,%` (como na std do Rust), que a linha de comando expande para `%` e não é
///   expandido de novo no `%*`; `!` fica literal com `/v:off`;
/// - aspas são removidas (não aparecem em caminhos) e quebras de linha viram espaço.
///
/// Só para textos gerados pelo app (instruções, caminhos como `C:\Users\Ana&Bia`): texto livre
/// de outros agentes nunca vai pela linha de comando de um shim.
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

/// Opções de descoberta (substituíveis para testes/embutir).
#[derive(Debug, Clone)]
pub(crate) struct Discovery {
    pub search_path: Option<OsString>,
    pub home: PathBuf,
    /// Procurar CLIs nas pastas do sistema (`APPDATA`, `LOCALAPPDATA`, `ProgramFiles`). Desligado
    /// nos testes: essas pastas passam a ser procuradas dentro de `home`.
    pub system_dirs: bool,
}

const COPILOT_CACHE_TTL: Duration = Duration::from_secs(30);

/// Estado do registro: cache da detecção do Copilot.
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

    /// Pasta do sistema da variável `key` (`APPDATA`…). Nos testes (`system_dirs = false`), fica
    /// dentro de `home`, para o resultado não depender do que está instalado na máquina.
    pub(crate) fn system_dir(&self, key: &str) -> Option<PathBuf> {
        if self.disc.system_dirs {
            std::env::var_os(key).filter(|v| !v.is_empty()).map(PathBuf::from)
        } else {
            Some(self.home().join(key))
        }
    }

    // ---------- Resolução dos executáveis ----------

    /// Resolve um agente base.
    pub fn resolve(&self, id: &str) -> Option<Resolved> {
        match id {
            "copilot" => self.resolve_copilot(),
            "shell" => Some(shell_resolved()),
            _ if agent_def(id).is_some() => self.resolve_cli(id),
            _ => None,
        }
    }

    /// Resolução padrão: candidatos conhecidos → `where`/`which` → shim `.cmd` via `cmd.exe`.
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
            // Shims do npm/pip (`.cmd`) precisam passar pelo cmd.exe.
            let shim =
                if id == "antigravity" { self.which("agy.cmd") } else { self.which(&format!("{name}.cmd")).or_else(|| self.which(name)) };
            shim.map(|p| {
                if p.extension().is_some_and(|e| e.eq_ignore_ascii_case("exe")) {
                    Resolved::direct(p)
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

    /// GitHub Copilot: `gh` com a extensão `gh-copilot` (resultado em cache por 30 s,
    /// pois exige rodar `gh extension list`).
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

    // ---------- Pares e instruções ----------

    /// Só os agentes instalados e habilitados como peer entram na ponte.
    pub fn peers(&self) -> Vec<String> {
        AGENTS.iter().filter(|a| a.peer && self.resolve(a.id).is_some()).map(|a| a.id.to_owned()).collect()
    }

    /// Como [`Registry::bridge_instructions`], para a instância `ident`, com o cargo dela.
    pub fn bridge_instructions_for(&self, ident: &Ident) -> String {
        let mut text = self.bridge_instructions(Some(&ident.id()));
        if let Some(role) = &ident.role {
            text.push_str(&format!("\nSeu cargo nesta instância: {role}. Aja de acordo com ele."));
        }
        text
    }

    /// As instruções do AgentDeck, a "memória geral" igual para todos os agentes. Com `self_id`, leva o nome da
    /// instância e a lista dos outros agentes; sem ele (texto de arquivo, igual para qualquer aba), manda ler o
    /// nome na variável de ambiente e não lista ninguém, para o arquivo não mudar de máquina para máquina.
    pub fn bridge_instructions(&self, self_id: Option<&str>) -> String {
        const INTRO: &str = "Você está rodando dentro do AgentDeck, um ambiente que conecta vários agentes de IA.";
        let (first, listed) = match self_id {
            Some(id) => {
                let others: Vec<String> = self.peers().into_iter().filter(|p| p != id).collect();
                (format!("{INTRO} Seu nome aqui é \"{id}\"."), format!(" (agentes: {})", others.join(",")))
            }
            None => (
                format!("{INTRO} Seu nome e cargo estão nas variáveis AGENTDECK_AGENT e AGENTDECK_ROLE; nunca pergunte a si mesmo."),
                String::new(),
            ),
        };
        [
            first,
            format!("Para falar com outro agente, rode no shell: ask-agent <agente> \"mensagem\"{listed}. Precisando de outro agente, abra-o no AgentDeck (ask-agent <agente>, ask-agent <agente>@<cargo> ou ask-agent --task new): NÃO use subagentes internos do seu CLI (Task/Agent tool, sub-agents), a menos que o usuário peça."),
            "A resposta sai no stdout e pode levar minutos (timeout de até 600000 ms). Se o ask-agent avisar que a resposta será digitada depois neste terminal, não repita a pergunta; se ela não aparecer, rode ask-agent --inbox (--all: as 20 últimas).".to_owned(),
            "ask-agent --models [<agente>] lista os agentes, os modelos desta máquina e a cota de cada um: escolha um modelo com cota (a ponte recusa o que está esgotado). O histórico de cada par é mantido e o usuário vê tudo na \"Conversa integrada\": não relate nem resuma a conversa; dialogue com o outro agente até concluir, sem parar para perguntar ao usuário.".to_owned(),
            "Cada aba é uma instância <modelo>[@<cargo>[@<nome>]] (claude, claude@dev, claude@dev@ana): ask-agent claude@dev \"tarefa\" abre a aba se não existir; --new abre outra do mesmo cargo; --model <modelo> escolhe o LLM ao abrir. Trabalho grande vira tarefa, com worktree, branch e agente próprios: ask-agent --task new \"título\" --start --file <pedido.md>.".to_owned(),
            "Texto com acento vai num arquivo (--file <arquivo>): pela entrada padrão o shell do Windows troca os acentos por \"?\".".to_owned(),
            "Quando outro agente lhe mandar uma mensagem, responda com o comando indicado nela (ask-agent --reply <número> \"resposta\" ou --file <arquivo>): só assim a resposta chega a ele.".to_owned(),
            "Os agentes formam uma equipe (chefe e subordinados; o usuário pode reorganizá-la). Antes de delegar ou reportar, veja a sua posição com ask-agent --team; delegue de preferência à equipe e, ao concluir algo que o chefe pediu, reporte com ask-agent --boss \"mensagem\".".to_owned(),
            "Memória do projeto: memory/aprendizados/, um .md por armadilha, decisão ou convenção que custou tempo e o código não mostra. Formato: cabeçalho --- titulo: … / resumo: uma linha / touches: [globs dos arquivos; opcional] --- e o corpo curto, com Por quê e Como aplicar. Antes de mexer em arquivos, rode ask-agent --learn <arquivo>... (sem argumentos: o índice e a situação das tarefas; com arquivos, avisa se outra tarefa mexe neles); ao descobrir algo assim, crie o arquivo. ask-agent --outline <arquivo>... mostra as assinaturas com a linha, para ler só o trecho necessário de um arquivo grande (offset).".to_owned(),
        ]
        .join("
")
    }

    /// Lista para menus (equivalente a `agents:list`).
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

/// Shell da aba "Terminal": PowerShell no Windows; `$SHELL` (ou /bin/bash) nos demais.
fn shell_resolved() -> Resolved {
    if cfg!(windows) {
        // No Windows, força UTF-8 sem BOM no console e no pipeline para evitar que
        // pipes para o ask-agent ou outros executáveis troquem acentos por "?".
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

// ---------- Ambiente e comando interativo ----------

/// Separador de PATH da plataforma.
pub const PATH_SEP: &str = if cfg!(windows) { ";" } else { ":" };

/// Ambiente dos agentes (equivalente a `agentEnv`): herda o do processo, remove marcas de
/// sessões aninhadas do Claude Code, põe a pasta dos binários no início do PATH e injeta
/// as variáveis da ponte.
/// `token` é o do processo (ver [`Core::tab_token`] e [`Core::headless_token`]), nunca o mestre.
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
    // `OPENCODE_CONFIG_CONTENT` é a configuração (modelo) da aba do OpenCode que abriu este processo; a de cada
    // aba nova é inserida depois deste filtro (`spawn_spec_for`, via `models::opencode_tui`).
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
        // No Windows as variáveis não diferenciam maiúsculas ("Path" vs "PATH").
        let is_path = if cfg!(windows) { k.eq_ignore_ascii_case("PATH") } else { k == "PATH" };
        if is_path {
            if old_path.is_empty() {
                old_path = v;
            }
            continue;
        }
        let removed = if cfg!(windows) { REMOVE.iter().any(|r| r.eq_ignore_ascii_case(&k)) } else { REMOVE.contains(&k.as_str()) }
            // Nada do AgentDeck que abriu este app (ex.: o app rodando dentro de uma aba de
            // outro AgentDeck) vaza para os agentes: cargo, pasta de dados, depuração…
            || (crate::env::is_app_var(&k) && !crate::env::is_app_var_named(&k, "ASK_WAIT"));
        if !removed {
            env.insert(k, v);
        }
    }
    env.insert("PATH".into(), format!("{}{PATH_SEP}{old_path}", bin_dir.display()));
    env.insert("AGENTDECK_PORT".into(), port.to_string());
    env.insert("AGENTDECK_TOKEN".into(), token.to_owned());
    env.insert("AGENTDECK_AGENT".into(), agent_id.to_owned());
    env.insert("AGENTDECK_DEPTH".into(), depth.to_string());
    env.insert("TERM".into(), "xterm-256color".into());
    env.insert("COLORTERM".into(), "truecolor".into());
    if cfg!(windows) {
        env.insert("PYTHONUTF8".into(), "1".into());
        env.insert("PYTHONIOENCODING".into(), "utf-8".into());
        // O Git Bash (o do Claude Code) reescreve argumentos que começam com "/" como caminhos
        // (`ask-agent claude@dev "/review src"` viraria `C:/Program Files/Git/review src`).
        env.insert("MSYS_NO_PATHCONV".into(), "1".into());
        env.insert("MSYS2_ARG_CONV_EXCL".into(), "*".into());
    }
    env
}

/// Grava `dados/hooks/claude-settings.json`: hooks que avisam a ponte quando o Claude começa
/// (`UserPromptSubmit`) e termina (`Stop`) um turno, que se somam às configurações do usuário, e
/// a statusline (`ask-agent --usage`), que manda o uso do plano à ponte. Esta substitui a do
/// usuário, por isso o `ask-agent --usage` roda a dele e mostra a saída dela; o `padding` dele,
/// se houver, é mantido aqui (quem o aplica é o Claude Code, não a nossa linha).
fn write_claude_hooks(data_dir: &Path, project_dir: &Path) -> std::io::Result<PathBuf> {
    let config_dir = std::env::var_os("CLAUDE_CONFIG_DIR")
        .filter(|v| !v.is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(|| crate::util::home_dir().join(".claude"));
    write_claude_hooks_with(data_dir, project_dir, &config_dir)
}

/// Como [`write_claude_hooks`], com a pasta de configuração do Claude explícita (teste).
fn write_claude_hooks_with(data_dir: &Path, project_dir: &Path, config_dir: &Path) -> std::io::Result<PathBuf> {
    let dir = data_dir.join("hooks");
    std::fs::create_dir_all(&dir)?;
    let path = dir.join("claude-settings.json");
    let hook = |state: &str| serde_json::json!([{ "hooks": [{ "type": "command", "command": format!("ask-agent --signal {state}") }] }]);
    let padding = user_statusline_padding(project_dir, config_dir);
    let settings = serde_json::json!({
        "hooks": { "UserPromptSubmit": hook("busy"), "Stop": hook("idle") },
        "statusLine": { "type": "command", "command": "ask-agent --usage", "padding": padding },
    });
    let text = serde_json::to_string_pretty(&settings).unwrap_or_default();
    if std::fs::read_to_string(&path).ok().as_deref() != Some(text.as_str()) {
        std::fs::write(&path, text)?;
    }
    Ok(path)
}

/// `padding` da statusline própria do usuário, na ordem de precedência do Claude Code (projeto
/// local, projeto e usuário). Sem statusline dele, 0 (o padrão).
fn user_statusline_padding(project_dir: &Path, config_dir: &Path) -> serde_json::Value {
    let files = [
        project_dir.join(".claude").join("settings.local.json"),
        project_dir.join(".claude").join("settings.json"),
        config_dir.join("settings.json"),
    ];
    files.iter().find_map(|f| user_statusline_in(f)).and_then(|sl| sl.get("padding").cloned()).unwrap_or_else(|| serde_json::Value::from(0))
}

/// `statusLine` de um arquivo de configuração do Claude Code, quando é um comando do usuário (a
/// nossa própria `ask-agent --usage` não conta).
fn user_statusline_in(file: &Path) -> Option<serde_json::Value> {
    let v: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(file).ok()?).ok()?;
    let sl = v.get("statusLine")?;
    if sl.get("type").and_then(serde_json::Value::as_str).is_some_and(|t| t != "command") {
        return None;
    }
    let cmd = sl.get("command")?.as_str()?.trim();
    (!cmd.is_empty() && !cmd.contains("ask-agent --usage")).then(|| sl.clone())
}

/// Nome do arquivo (em `dados/`) com as instruções do AgentDeck, para os CLIs que leem instruções de um arquivo.
const INSTRUCTIONS_FILE: &str = "agentdeck-instructions.md";

/// Grava `dados/agentdeck-instructions.md` (só se mudou) e devolve o caminho. Fica fora do repositório do usuário.
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

/// Argumentos que passam as instruções do AgentDeck a `kind`, sem mexer no repositório do usuário. O Claude
/// (`--append-system-prompt`) e o OpenCode (configuração, ver [`models::opencode_tui`]) são montados à parte.
/// - Codex: `-c developer_instructions=…` (o valor é TOML; num shim `.cmd` as aspas não sobrevivem e o texto vai
///   cru, que o Codex aceita como texto quando não é TOML válido);
/// - Aider: `--read <arquivo>` (arquivo só de leitura);
/// - Open Interpreter: `--custom_instructions` (acrescenta ao prompt de sistema).
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

/// CLIs sem opção de instruções que só leem o `AGENTS.md` da pasta onde rodam: sem o arquivo, não sabem do
/// `ask-agent`. Para eles o app cria o `AGENTS.md` só com o bloco dele quando a pasta não tem um (ver
/// [`Core::ensure_agents_file`]). O Antigravity saiu daqui: ele lê o `AGENTS.md` de uma pasta passada em `--add-dir`
/// (ver [`write_agy_instructions`]).
fn reads_only_agents_md(kind: &str) -> bool {
    matches!(kind, "goose" | "copilot")
}

/// Grava `dados/agy/<instância>/AGENTS.md` com as instruções da instância (só se mudou) e devolve a pasta, para o
/// `--add-dir` do Antigravity: o `agy` lê o `AGENTS.md` das pastas adicionadas junto com o do projeto (conferido em
/// 2026-10-24 com o 1.4.7), então nada é criado no repositório do usuário.
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

/// Monta o comando de uma aba interativa (equivalente a `pty:spawn`).
pub(crate) fn spawn_spec(core: &Core, project: &str, kind: &str) -> Result<SpawnSpec, String> {
    spawn_spec_for(core, project, &Ident::main(kind))
}

/// Como [`spawn_spec`], para uma instância: `AGENTDECK_AGENT` leva o id dela e o cargo vai
/// em `AGENTDECK_ROLE` e no prompt de sistema. O terminal abre na pasta do projeto `project`
/// (na primeira aba de um projeto que não é o ativo, o AGENTS.md dele é escrito).
pub(crate) fn spawn_spec_for(core: &Core, project: &str, ident: &Ident) -> Result<SpawnSpec, String> {
    let project_dir = core.project_path(project).ok_or_else(|| format!("Projeto desconhecido: {project}"))?;
    let kind = ident.kind.as_str();
    let id = ident.id();
    // A instância de uma tarefa trabalha na worktree dela, não na pasta do projeto.
    let cwd = match core.task_cwd(project, &id) {
        Some(dir) if dir.is_dir() => dir,
        Some(dir) => return Err(format!("A pasta da tarefa não existe mais ({}). Descarte a tarefa na mesa de tarefas.", dir.display())),
        None => project_dir.clone(),
    };
    let reg = &core.inner.agents;
    let def = agent_def(kind).ok_or_else(|| format!("Agente desconhecido: {kind}"))?;
    let data_dir = core.paths().data_dir.to_string_lossy().into_owned();
    // Nenhum CLI depende do `AGENTS.md` do usuário: cada um recebe as instruções pelo canal que ele tem.
    let instructions = reg.bridge_instructions_for(ident);
    let generic_file = write_instructions_file(&core.paths().data_dir, &reg.bridge_instructions(None))
        .map_err(|e| log::warn!("não consegui gravar o arquivo de instruções do AgentDeck: {e}"))
        .ok();
    let mut env = agent_env(core, &id, 0, &core.tab_token(project, &id));
    if let Some(role) = &ident.role {
        env.insert("AGENTDECK_ROLE".into(), role.clone());
    }

    // Pedido > gravado > padrão do cargo; fica gravado para a interface e para reabrir o app.
    let model = core.assign_model(project, ident, None);
    let resolved = reg.resolve(kind).ok_or_else(|| def.missing.to_owned())?;
    let mut args = Vec::new();
    // Prompt inicial de uma tarefa recém-aberta (ver `Core::set_boot_prompt`). No Claude é posicional e vai
    // primeiro: `--add-dir` e `--allowedTools` aceitam vários valores e engoliriam o texto.
    let boot = models::boot_prompt_flag(kind).and_then(|flag| core.take_boot_prompt(project, &id).map(|p| (flag, p)));
    if let Some((None, prompt)) = &boot {
        args.push(prompt.clone());
    }
    if kind == "claude" {
        args.push("--append-system-prompt".into());
        args.push(instructions.clone());
        // Responder a outro agente é rodar `ask-agent`: libera só esse comando, sem perguntar.
        args.push("--allowedTools".into());
        args.push("Bash(ask-agent:*)".into());
        // Hooks avisam o início e o fim de cada turno (ver `delivery.rs`).
        match write_claude_hooks(&core.paths().data_dir, &project_dir) {
            Ok(path) => {
                args.push("--settings".into());
                args.push(path.to_string_lossy().into_owned());
            }
            Err(e) => log::warn!("não consegui gravar os hooks do Claude: {e}"),
        }
    }
    if kind == "codex" && resolved.shim.is_none() {
        // Fim de turno do Codex → `ask-agent --signal idle <json>`. Pelo cmd.exe o valor TOML
        // (com aspas) não sobrevive, então só com o executável direto.
        args.push("-c".into());
        args.push(r#"notify=["ask-agent","--signal","idle"]"#.into());
    }
    args.extend(instruction_args(kind, &instructions, generic_file.as_deref(), resolved.shim.is_some()));
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
            Err(e) => log::warn!("não consegui gravar as instruções do Antigravity: {e}"),
        }
    }
    if kind == "opencode" {
        // O TUI não tem --model nem --system: servidor próprio + modelo e instruções na configuração
        // (ver `models::opencode_tui`).
        let (extra, (key, value)) = models::opencode_tui(model.as_deref(), generic_file.as_deref());
        args.extend(extra);
        env.insert(key, value);
    } else if let Some(m) = &model {
        args.extend(models::cli_args(kind, m));
    }
    // Esforço da tarefa (o do Antigravity já está no nome do modelo; ver `Task::effort`).
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
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn registry() -> Registry {
        Registry::new(Discovery { search_path: Some(OsString::new()), home: PathBuf::from("/h"), system_dirs: false })
    }

    /// As instruções não crescem além do necessário (eram 2580 caracteres antes da t38; a t40 acrescentou o `--outline`) e
    /// trazem a regra dos subagentes, a ponte e a memória geral.
    #[test]
    fn the_instructions_carry_the_rules_and_stay_short() {
        let reg = registry();
        let text = reg.bridge_instructions(Some("claude@dev"));
        assert!(text.chars().count() <= 2740, "{} caracteres", text.chars().count());
        for needle in [
            "NÃO use subagentes internos do seu CLI (Task/Agent tool, sub-agents), a menos que o usuário peça",
            "ask-agent <agente>@<cargo> ou ask-agent --task new",
            "memory/aprendizados/",
            "titulo:",
            "resumo:",
            "touches:",
            "ask-agent --learn",
            "ask-agent --outline <arquivo>",
            "ask-agent --reply",
            "ask-agent --boss",
        ] {
            assert!(text.contains(needle), "falta {needle:?}");
        }
        assert!(text.contains("Seu nome aqui é \"claude@dev\"") && text.contains("(agentes: )"), "{text}");
    }

    /// O texto de arquivo (igual para qualquer aba) não lista agentes: não muda de máquina para máquina.
    #[test]
    fn the_generic_instructions_do_not_list_the_installed_agents() {
        let reg = registry();
        let text = reg.bridge_instructions(None);
        assert!(text.contains("AGENTDECK_AGENT") && text.contains("AGENTDECK_ROLE") && !text.contains("(agentes:"), "{text}");
    }

    #[test]
    fn each_cli_gets_its_own_instruction_channel() {
        let file = Path::new("/dados/agentdeck-instructions.md");
        let text = "Você \"é\"
ok";
        // Codex: valor TOML (string básica) com aspas e quebra de linha escapadas; num shim, o texto cru.
        assert_eq!(instruction_args("codex", text, Some(file), false), ["-c", r#"developer_instructions="Você \"é\"\nok""#]);
        assert_eq!(
            instruction_args("codex", text, Some(file), true),
            [
                "-c",
                "developer_instructions=Você \"é\"
ok"
            ]
        );
        assert_eq!(instruction_args("aider", text, Some(file), false), ["--read", "/dados/agentdeck-instructions.md"]);
        assert!(instruction_args("aider", text, None, false).is_empty());
        assert_eq!(instruction_args("interpreter", text, Some(file), false), ["--custom_instructions", text]);
        // Claude e OpenCode são montados à parte; o Antigravity lê o AGENTS.md.
        for kind in ["claude", "opencode", "antigravity", "goose", "shell"] {
            assert!(instruction_args(kind, text, Some(file), false).is_empty(), "{kind}");
        }
        assert!(["goose", "copilot"].iter().all(|k| reads_only_agents_md(k)));
        assert!(["claude", "codex", "opencode", "aider", "interpreter", "shell", "antigravity"].iter().all(|k| !reads_only_agents_md(k)));
    }

    #[test]
    fn the_instructions_file_is_rewritten_only_when_it_changes() {
        let tmp = tempfile::tempdir().expect("tmp");
        let dados = tmp.path().join("dados");
        let path = write_instructions_file(&dados, "um").expect("gravar");
        assert_eq!(path, dados.join(INSTRUCTIONS_FILE));
        assert_eq!(
            std::fs::read_to_string(&path).expect("ler"),
            "um
"
        );
        let before = std::fs::metadata(&path).and_then(|m| m.modified()).expect("mtime");
        write_instructions_file(&dados, "um").expect("de novo");
        assert_eq!(std::fs::metadata(&path).and_then(|m| m.modified()).expect("mtime"), before);
        write_instructions_file(&dados, "dois").expect("muda");
        assert_eq!(
            std::fs::read_to_string(&path).expect("ler"),
            "dois
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
            // Herdados de um AgentDeck que abriu este (app rodando dentro de uma aba).
            ("AGENTDECK_ROLE".to_owned(), "dev".to_owned()),
            ("AGENTDECK_HOME".to_owned(), "/outro".to_owned()),
            ("AGENTDECK_TOKEN".to_owned(), "velho".to_owned()),
            ("AGENTDECK_DEBUG_UI".to_owned(), "1".to_owned()),
            // Configuração do usuário: continua valendo.
            ("AGENTDECK_ASK_WAIT".to_owned(), "300".to_owned()),
            // Modelo da aba do OpenCode que abriu este app: não passa para os agentes.
            ("OPENCODE_CONFIG_CONTENT".to_owned(), r#"{"model":"herdado"}"#.to_owned()),
        ];
        let env = build_agent_env(base, Path::new("/app/bin"), 1234, "tok", "claude", 1);
        for k in ["AGENTDECK_ROLE", "AGENTDECK_HOME", "AGENTDECK_DEBUG_UI"] {
            assert!(!env.contains_key(k), "{k} herdado");
        }
        assert_eq!(env["AGENTDECK_TOKEN"], "tok");
        assert_eq!(env["AGENTDECK_ASK_WAIT"], "300");
        assert!(!env.contains_key("CLAUDECODE"));
        assert!(!env.contains_key("CLAUDE_CODE_ENTRYPOINT"));
        assert!(!env.contains_key("OPENCODE_CONFIG_CONTENT"), "a configuração da aba do OpenCode que abriu o app vazou");
        assert_eq!(env["HOME"], "/h");
        assert_eq!(env["PATH"], format!("/app/bin{PATH_SEP}/usr/bin"));
        assert_eq!(env["AGENTDECK_PORT"], "1234");
        assert_eq!(env["AGENTDECK_TOKEN"], "tok");
        assert_eq!(env["AGENTDECK_AGENT"], "claude");
        assert_eq!(env["AGENTDECK_DEPTH"], "1");
        assert_eq!(env["TERM"], "xterm-256color");
        assert_eq!(env["COLORTERM"], "truecolor");
    }

    #[test]
    fn cmd_args_are_escaped_not_mangled() {
        // Com espaço o portable-pty põe aspas: só `%`, aspas e quebras de linha mudam.
        assert_eq!(cmd_safe_arg("a \"b\" & calc | x <y> ^z %PATH% !v!\r\nfim\n"), "a b & calc | x <y> ^z %%cd:~,%PATH%%cd:~,% !v! fim ");
        assert_eq!(cmd_safe_arg(r"C:\Users\ana\dados"), r"C:\Users\ana\dados");
        // Sem espaço, sem aspas: circunflexos para as duas passagens do cmd.
        assert_eq!(cmd_safe_arg(r"C:\Users\Ana&Bia\dados"), r"C:\Users\Ana^^^&Bia\dados");
        assert_eq!(cmd_safe_arg("Bash(ask-agent:*)"), "Bash^^^(ask-agent:*^^^)");
        assert_eq!(cmd_safe_arg("50%!"), "50%%cd:~,%!");
    }

    /// Linha de comando como o portable-pty monta (`append_quoted`) para argumentos sem aspas.
    #[cfg(windows)]
    fn pty_quoted(arg: &str) -> String {
        if !arg.is_empty() && !arg.contains([' ', '\t', '\n', '\x0b', '"']) {
            return arg.to_owned();
        }
        let trailing = arg.len() - arg.trim_end_matches('\\').len();
        format!("\"{arg}{}\"", "\\".repeat(trailing))
    }

    /// Caminhos e textos do app atravessam o `cmd.exe /c` e o `%*` de um shim (os dois formatos do
    /// npm: linha única e bloco `IF … ( … ) ELSE ( … )`) sem quebrar nem expandir nada.
    #[cfg(windows)]
    #[test]
    fn escaped_args_reach_the_shim_intact() {
        use std::os::windows::process::CommandExt;
        let tmp = tempfile::tempdir().expect("tmp");
        std::fs::write(tmp.path().join("linha.cmd"), "@ECHO off\r\nSETLOCAL\r\necho(%*\r\n").expect("shim");
        std::fs::write(tmp.path().join("bloco.cmd"), "@IF EXIST \"%~dp0\\nada.exe\" (\r\n  echo(%*\r\n) ELSE (\r\n  echo(%*\r\n)\r\n")
            .expect("shim");
        let args = [
            r"C:\Users\Ana&Bia\dados",
            r"C:\Users\Ana Bia&X (1)\dados\",
            "Bash(ask-agent:*)",
            "50%PATH%!PATH!^x|y<z>w",
            "texto com %PATH% & aspas \"x\" | fim",
        ];
        let expected: Vec<String> = args.iter().map(|a| pty_quoted(&a.replace('"', ""))).collect();
        for shim in ["linha", "bloco"] {
            let mut tail: Vec<String> = CMD_FLAGS.map(String::from).to_vec();
            tail.push(format!(r".\{shim}")); // sem depender de NoDefaultCurrentDirectoryInExePath
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
            write_claude_hooks_with(&tmp.path().join("dados"), &tmp.path().join("projeto"), &tmp.path().join("claude")).expect("hooks");
        let v: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(&path).expect("ler")).expect("json");
        assert_eq!(v["hooks"]["Stop"][0]["hooks"][0]["command"], "ask-agent --signal idle");
        assert_eq!(v["hooks"]["UserPromptSubmit"][0]["hooks"][0]["command"], "ask-agent --signal busy");
        assert_eq!(v["statusLine"], serde_json::json!({ "type": "command", "command": "ask-agent --usage", "padding": 0 }));
        assert_eq!(
            write_claude_hooks_with(&tmp.path().join("dados"), &tmp.path().join("projeto"), &tmp.path().join("claude")).expect("de novo"),
            path
        );
    }

    #[test]
    fn claude_hooks_keep_the_user_statusline_padding() {
        let tmp = tempfile::tempdir().expect("tmp");
        let data = tmp.path().join("dados");
        let project = tmp.path().join("projeto");
        let config = tmp.path().join("claude");
        let statusline = |padding: serde_json::Value| serde_json::json!({ "statusLine": { "type": "command", "command": "minha-status", "padding": padding } });
        let write = |file: &Path, json: &serde_json::Value| {
            std::fs::create_dir_all(file.parent().expect("pai")).expect("mkdir");
            std::fs::write(file, json.to_string()).expect("write");
        };
        let statusline_padding = || {
            let path = write_claude_hooks_with(&data, &project, &config).expect("hooks");
            let v: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(&path).expect("ler")).expect("json");
            v["statusLine"]["padding"].clone()
        };

        // Sem statusline do usuário: 0, como antes.
        assert_eq!(statusline_padding(), serde_json::json!(0));

        // O padding do usuário é o da nossa statusline (quem o aplica é o Claude Code).
        write(&config.join("settings.json"), &statusline(serde_json::json!(2)));
        assert_eq!(statusline_padding(), serde_json::json!(2));

        // O projeto local vale mais que o do usuário.
        write(&project.join(".claude").join("settings.local.json"), &statusline(serde_json::json!(1)));
        assert_eq!(statusline_padding(), serde_json::json!(1));

        // A nossa própria statusline (`ask-agent --usage`) não conta como a do usuário.
        write(
            &project.join(".claude").join("settings.local.json"),
            &serde_json::json!({ "statusLine": { "type": "command", "command": "ask-agent --usage", "padding": 7 } }),
        );
        assert_eq!(statusline_padding(), serde_json::json!(2));
    }

    #[cfg(windows)]
    #[test]
    fn env_path_casing_windows() {
        let base = vec![("Path".to_owned(), "C:\\Windows".to_owned())];
        let env = build_agent_env(base, Path::new("C:\\app"), 1, "t", "shell", 0);
        assert!(!env.contains_key("Path"));
        assert_eq!(env["PATH"], "C:\\app;C:\\Windows");
        // O Git Bash não converte "/review" em caminho.
        assert_eq!((env["MSYS_NO_PATHCONV"].as_str(), env["MSYS2_ARG_CONV_EXCL"].as_str()), ("1", "*"));
    }
}
