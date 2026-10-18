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
    /// `claude`, `antigravity`, `ollama`, `ollama:<modelo>`, `codex`, ..., `shell`.
    pub id: String,
    pub label: String,
    /// Cor `#rrggbb`.
    pub color: String,
    pub installed: bool,
    pub peer: bool,
    /// Modelo Ollama, quando aplicável.
    pub model: Option<String>,
}

/// Comando completo para abrir uma aba interativa num pseudo-terminal.
#[derive(Debug, Clone)]
pub struct SpawnSpec {
    pub program: PathBuf,
    pub args: Vec<String>,
    pub cwd: PathBuf,
    /// Ambiente COMPLETO a usar (já derivado do ambiente do processo, com PATH/COMMANDDECK_* injetados
    /// e CLAUDECODE & cia removidos). A UI deve limpar o ambiente herdado e aplicar este.
    pub env: BTreeMap<String, String>,
    /// Rótulo da aba (ex.: "Claude", "Ollama (qwen3.5:9b)").
    pub label: String,
    pub color: String,
    /// `kind` original pedido.
    pub kind: String,
    /// Se `true`, o envio via compositor deve mandar um `\r` extra após 600 ms quando o
    /// texto contém `@` (autocomplete de arquivo do Claude/Antigravity).
    pub double_enter_on_mention: bool,
    /// Modelo de LLM da instância (já resolvido e gravado em `tab_models`); `None` = o padrão do CLI.
    pub model: Option<String>,
}

/// Cores oficiais (usadas também pela UI como fallback).
pub const COLOR_CLAUDE: &str = "#d97757";
pub const COLOR_ANTIGRAVITY: &str = "#4f8cf7";
pub const COLOR_OLLAMA: &str = "#f59e0b";
pub const COLOR_SHELL: &str = "#9aa0a6";

/// Modelo usado quando nenhum modelo Ollama é conhecido.
pub const DEFAULT_OLLAMA_MODEL: &str = "qwen3.5:9b";

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
        id: "ollama",
        label: "Ollama",
        color: COLOR_OLLAMA,
        peer: true,
        missing: "Ollama CLI não encontrado. Instale em: https://ollama.com/download",
    },
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
    ("qwen", "ollama"),
    ("llama", "ollama"),
    ("local", "ollama"),
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

/// Resultado de `resolveOllama`.
#[derive(Debug, Clone)]
pub(crate) struct OllamaResolved {
    pub model: String,
    pub models: Vec<String>,
}

/// Opções de descoberta (substituíveis para testes/embutir).
#[derive(Debug, Clone)]
pub(crate) struct Discovery {
    pub search_path: Option<OsString>,
    pub home: PathBuf,
    pub ollama: bool,
    /// Procurar CLIs nas pastas do sistema (`APPDATA`, `LOCALAPPDATA`, `ProgramFiles`). Desligado
    /// nos testes: essas pastas passam a ser procuradas dentro de `home`.
    pub system_dirs: bool,
}

#[derive(Default)]
struct OllamaCache {
    models: Vec<String>,
    last_fetch: Option<Instant>,
    /// `None` = ainda não procurado; `Some(None)` = não encontrado.
    exe: Option<Option<PathBuf>>,
}

const OLLAMA_CACHE_TTL: Duration = Duration::from_secs(15);
const COPILOT_CACHE_TTL: Duration = Duration::from_secs(30);

/// Estado do registro: caches de modelos Ollama e da detecção do Copilot.
pub(crate) struct Registry {
    disc: Discovery,
    ollama: Mutex<OllamaCache>,
    copilot: Mutex<Option<(Instant, Option<Resolved>)>>,
}

impl Registry {
    pub fn new(disc: Discovery) -> Self {
        let reg = Self { disc, ollama: Mutex::new(OllamaCache::default()), copilot: Mutex::new(None) };
        // Inicializa o cache a partir do disco (rápido e não bloqueia).
        if reg.disc.ollama {
            let mut c = reg.ollama.lock();
            c.models = models_from_disk(&reg.ollama_model_dirs());
            c.last_fetch = Some(Instant::now());
        }
        reg
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

    /// Resolve um agente base (sem `ollama:`); `ollama` usa o modelo padrão.
    pub fn resolve(&self, id: &str) -> Option<Resolved> {
        match id {
            "ollama" => self.resolve_ollama(None).map(|_| Resolved::direct(self.ollama_exe().unwrap_or_else(|| "ollama".into()))),
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

    // ---------- Ollama ----------

    fn ollama_model_dirs(&self) -> Vec<PathBuf> {
        let mut dirs = Vec::new();
        if let Some(p) = std::env::var_os("OLLAMA_MODELS").filter(|p| !p.is_empty()) {
            dirs.push(PathBuf::from(p));
        }
        dirs.push(self.home().join(".ollama").join("models"));
        dirs
    }

    fn ollama_exe(&self) -> Option<PathBuf> {
        if !self.disc.ollama {
            return None;
        }
        let mut cache = self.ollama.lock();
        if let Some(exe) = &cache.exe {
            return exe.clone();
        }
        let candidates: Vec<Option<PathBuf>> = if cfg!(windows) {
            let env = |k: &str, d: &str| self.system_dir(k).unwrap_or_else(|| PathBuf::from(d));
            vec![
                Some(env("LOCALAPPDATA", "").join("Programs").join("Ollama").join("ollama.exe")),
                Some(env("ProgramFiles", "C:\\Program Files").join("Ollama").join("ollama.exe")),
                Some(env("ProgramFiles(x86)", "C:\\Program Files (x86)").join("Ollama").join("ollama.exe")),
                self.which("ollama.exe"),
                self.which("ollama"),
            ]
        } else {
            vec![
                self.which("ollama"),
                Some(PathBuf::from("/usr/local/bin/ollama")),
                Some(PathBuf::from("/usr/bin/ollama")),
                Some(PathBuf::from("/Applications/Ollama.app/Contents/Resources/ollama")),
            ]
        };
        let exe = candidates.into_iter().flatten().find(|p| p.is_file());
        cache.exe = Some(exe.clone());
        exe
    }

    /// Modelos conhecidos (cache; se vazio, tenta o disco). Equivalente a `getOllamaModels`.
    pub fn ollama_models(&self) -> Vec<String> {
        if !self.disc.ollama {
            return Vec::new();
        }
        let mut c = self.ollama.lock();
        if c.models.is_empty() {
            c.models = models_from_disk(&self.ollama_model_dirs());
        }
        c.models.clone()
    }

    /// Atualiza a lista de modelos via `/api/tags` (cache de 15 s; fallback para o disco).
    /// Retorna `true` se a lista mudou. Equivalente a `refreshOllamaModelsAsync`.
    pub fn refresh_ollama_models(&self, host: &str) -> bool {
        if !self.disc.ollama {
            return false;
        }
        {
            let c = self.ollama.lock();
            if !c.models.is_empty() && c.last_fetch.is_some_and(|t| t.elapsed() < OLLAMA_CACHE_TTL) {
                return false;
            }
        }
        let mut list = crate::ollama::list_models(host, Duration::from_millis(1500)).unwrap_or_default();
        dedup_in_order(&mut list);
        if list.is_empty() {
            list = models_from_disk(&self.ollama_model_dirs());
        }
        let mut c = self.ollama.lock();
        if list.is_empty() {
            return false;
        }
        let changed = c.models != list;
        c.models = list;
        c.last_fetch = Some(Instant::now());
        changed
    }

    /// `resolveOllama(model)`: disponível se houver executável ou modelos.
    pub fn resolve_ollama(&self, specific: Option<&str>) -> Option<OllamaResolved> {
        if !self.disc.ollama {
            return None;
        }
        let exe = self.ollama_exe();
        let models = self.ollama_models();
        let model = specific.map(str::to_owned).or_else(|| models.first().cloned()).unwrap_or_else(|| DEFAULT_OLLAMA_MODEL.to_owned());
        (exe.is_some() || !models.is_empty()).then_some(OllamaResolved { model, models })
    }

    // ---------- Pares e instruções ----------

    /// Só os agentes instalados e habilitados como peer entram na ponte (+ `ollama:<modelo>`).
    pub fn peers(&self) -> Vec<String> {
        let mut out: Vec<String> = AGENTS.iter().filter(|a| a.peer && self.resolve(a.id).is_some()).map(|a| a.id.to_owned()).collect();
        if let Some(o) = self.resolve_ollama(None) {
            out.extend(o.models.iter().map(|m| format!("ollama:{m}")));
        }
        out
    }

    /// Texto injetado no prompt de sistema / AGENTS.md (equivalente a `bridgeInstructions`).
    pub fn bridge_instructions_for(&self, ident: &Ident) -> String {
        let mut text = self.bridge_instructions(Some(&ident.id()));
        if let Some(role) = &ident.role {
            text.push_str(&format!("\nSeu cargo nesta instância: {role}. Aja de acordo com ele."));
        }
        text
    }

    pub fn bridge_instructions(&self, self_id: Option<&str>) -> String {
        let ollama = self.resolve_ollama(None);
        let mut others: Vec<String> = self
            .peers()
            .into_iter()
            .filter(|p| Some(p.as_str()) != self_id && p != "ollama")
            .map(|id| match id.strip_prefix("ollama:") {
                Some(m) => format!("ollama ({m})"),
                None => id,
            })
            .collect();
        if others.is_empty() {
            if let Some(o) = &ollama {
                others.push(format!("ollama ({})", o.model));
            }
        }
        let first = match self_id {
            Some(id) => format!(
                "Você está rodando dentro do CommandDeck Terminal, um terminal que conecta vários agentes de IA. Seu nome aqui é \"{id}\"."
            ),
            None => "Você está rodando dentro do CommandDeck Terminal, um terminal que conecta vários agentes de IA. Seu nome aqui está na variável de ambiente COMMANDDECK_AGENT; nunca pergunte a si mesmo.".to_owned(),
        };
        // `${others}` no JS converte o array com "," (sem espaço) — mantido igual.
        [
            first,
            "Para conversar com outro agente, execute no shell:".to_owned(),
            "  ask-agent <agente> \"sua mensagem\"".to_owned(),
            format!(
                "Agentes disponíveis: {}. A resposta é impressa no stdout e pode levar alguns minutos (use timeout de até 600000 ms). Se o outro agente demorar, o ask-agent termina antes, avisando que a resposta será digitada depois neste terminal como uma nova mensagem: nesse caso, não repita a pergunta.",
                others.join(",")
            ),
            "O histórico de cada par de agentes é mantido entre chamadas. O usuário tem acesso à \"Conversa integrada\" em tempo real no CommandDeck, portanto você NÃO precisa relatar nem resumir a conversa dos agentes para ele.".to_owned(),
            "Ao colaborar com outro agente (ex: Antigravity), mantenha o diálogo diretamente com ele (iterando, tirando dúvidas e alinhando) até concluir a tarefa, em vez de parar no meio para fazer perguntas ao usuário.".to_owned(),
            "Cada aba é uma instância com nome <modelo>[@<cargo>[@<nome>]] (ex.: claude, claude@dev, claude@dev@ana). Para delegar a uma instância com cargo, chame-a pelo nome: ask-agent claude@dev \"tarefa\" (a aba é aberta se não existir); use --new para abrir outra instância do mesmo cargo; ask-agent --model <modelo> claude@dev \"tarefa\" escolhe o LLM da instância ao abri-la (ex.: sonnet). Trabalho grande vira uma tarefa, com worktree, branch e agente próprios: ask-agent --task new \"título\" --start --file <pedido.md> (o pedido num arquivo preserva os acentos; pela entrada padrão o shell do Windows os troca por \"?\").".to_owned(),
            "Quando outro agente lhe mandar uma mensagem, responda executando o comando indicado nela (ask-agent --reply <número> \"sua resposta\"; em textos longos ou com acentos, escreva-os num arquivo e use --file <arquivo>): é só assim que a resposta chega a ele.".to_owned(),
            "Os agentes formam uma equipe: cada um pode ter um chefe e subordinados, e o usuário pode reorganizá-la a qualquer momento. Antes de delegar ou de reportar, consulte a sua posição com ask-agent --team; delegue de preferência à sua equipe e, ao concluir algo que o seu chefe pediu, reporte a ele com ask-agent --boss \"mensagem\".".to_owned(),
        ]
        .join("\n")
    }

    /// Lista para menus (mesma expansão de `agents:list`).
    pub fn list(&self) -> Vec<AgentInfo> {
        let ollama = self.resolve_ollama(None);
        let mut out = Vec::new();
        for a in AGENTS {
            if a.id == "ollama" {
                match &ollama {
                    Some(o) if o.models.len() == 1 => out.push(AgentInfo {
                        id: "ollama".into(),
                        label: format!("Ollama ({})", o.models[0]),
                        color: a.color.into(),
                        installed: true,
                        peer: true,
                        model: Some(o.models[0].clone()),
                    }),
                    Some(o) if !o.models.is_empty() => out.extend(o.models.iter().map(|m| AgentInfo {
                        id: format!("ollama:{m}"),
                        label: format!("Ollama ({m})"),
                        color: a.color.into(),
                        installed: true,
                        peer: true,
                        model: Some(m.clone()),
                    })),
                    _ => out.push(AgentInfo {
                        id: "ollama".into(),
                        label: a.label.into(),
                        color: a.color.into(),
                        installed: ollama.is_some(),
                        peer: ollama.is_some(),
                        model: None,
                    }),
                }
                continue;
            }
            out.push(AgentInfo {
                id: a.id.into(),
                label: a.label.into(),
                color: a.color.into(),
                installed: self.resolve(a.id).is_some(),
                peer: a.peer,
                model: None,
            });
        }
        out
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

fn dedup_in_order(list: &mut Vec<String>) {
    let mut seen = std::collections::HashSet::new();
    list.retain(|m| seen.insert(m.clone()));
}

/// Lê os manifests do Ollama em disco (`manifests[/-v2]/<registro>/library/<modelo>/<tag>`).
/// Equivalente a `getOllamaModelsFromDisk`.
pub fn models_from_disk(bases: &[PathBuf]) -> Vec<String> {
    fn scan(dir: &Path, rel: &mut Vec<String>, found: &mut Vec<String>) {
        let Ok(rd) = std::fs::read_dir(dir) else { return };
        let mut entries: Vec<_> = rd.flatten().collect();
        entries.sort_by_key(|e| e.file_name());
        for ent in entries {
            let name = ent.file_name().to_string_lossy().into_owned();
            let Ok(ft) = ent.file_type() else { continue };
            if ft.is_dir() {
                rel.push(name);
                scan(&ent.path(), rel, found);
                rel.pop();
            } else if ft.is_file() {
                let model = match rel.iter().position(|p| p == "library") {
                    Some(i) if i + 1 < rel.len() => Some(rel[i + 1..].join("/")),
                    _ if rel.len() >= 2 => Some(rel[1..].join("/")),
                    _ => None,
                };
                if let Some(m) = model {
                    let full = format!("{m}:{name}");
                    if !found.contains(&full) {
                        found.push(full);
                    }
                }
            }
        }
    }
    let mut found = Vec::new();
    for base in bases {
        for sub in ["manifests", "manifests-v2"] {
            let mdir = base.join(sub);
            if mdir.is_dir() {
                scan(&mdir, &mut Vec::new(), &mut found);
            }
        }
    }
    found
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
    const REMOVE: &[&str] =
        &["CLAUDECODE", "CLAUDE_CODE_ENTRYPOINT", "CLAUDE_CODE_CHILD_SESSION", "CLAUDE_CODE_SSE_PORT", "ELECTRON_RUN_AS_NODE"];
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
            // Nada do CommandDeck que abriu este app (ex.: o app rodando dentro de uma aba de
            // outro CommandDeck) vaza para os agentes: cargo, pasta de dados, depuração…
            || (k.get(..12).is_some_and(|p| p.eq_ignore_ascii_case("COMMANDDECK_")) && !k.eq_ignore_ascii_case("COMMANDDECK_ASK_WAIT"));
        if !removed {
            env.insert(k, v);
        }
    }
    env.insert("PATH".into(), format!("{}{PATH_SEP}{old_path}", bin_dir.display()));
    env.insert("COMMANDDECK_PORT".into(), port.to_string());
    env.insert("COMMANDDECK_TOKEN".into(), token.to_owned());
    env.insert("COMMANDDECK_AGENT".into(), agent_id.to_owned());
    env.insert("COMMANDDECK_DEPTH".into(), depth.to_string());
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
/// usuário, por isso o `ask-agent --usage` roda a dele e mostra a saída dela.
fn write_claude_hooks(data_dir: &Path) -> std::io::Result<PathBuf> {
    let dir = data_dir.join("hooks");
    std::fs::create_dir_all(&dir)?;
    let path = dir.join("claude-settings.json");
    let hook = |state: &str| serde_json::json!([{ "hooks": [{ "type": "command", "command": format!("ask-agent --signal {state}") }] }]);
    let settings = serde_json::json!({
        "hooks": { "UserPromptSubmit": hook("busy"), "Stop": hook("idle") },
        "statusLine": { "type": "command", "command": "ask-agent --usage", "padding": 0 },
    });
    let text = serde_json::to_string_pretty(&settings).unwrap_or_default();
    if std::fs::read_to_string(&path).ok().as_deref() != Some(text.as_str()) {
        std::fs::write(&path, text)?;
    }
    Ok(path)
}

/// Nome do binário `ollama-agent` na pasta dos executáveis.
pub fn ollama_agent_exe(bin_dir: &Path) -> PathBuf {
    bin_dir.join(if cfg!(windows) { "ollama-agent.exe" } else { "ollama-agent" })
}

/// Monta o comando de uma aba interativa (equivalente a `pty:spawn`).
pub(crate) fn spawn_spec(core: &Core, project: &str, kind: &str) -> Result<SpawnSpec, String> {
    spawn_spec_for(core, project, &Ident::main(kind))
}

/// Como [`spawn_spec`], para uma instância: `COMMANDDECK_AGENT` leva o id dela e o cargo vai
/// em `COMMANDDECK_ROLE` e no prompt de sistema. O terminal abre na pasta do projeto `project`
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
    let sub_model = kind.strip_prefix("ollama:");
    let base = if sub_model.is_some() { "ollama" } else { kind };
    let def = agent_def(base).ok_or_else(|| format!("Agente desconhecido: {kind}"))?;
    let data_dir = core.paths().data_dir.to_string_lossy().into_owned();
    let mut env = agent_env(core, &id, 0, &core.tab_token(project, &id));
    if let Some(role) = &ident.role {
        env.insert("COMMANDDECK_ROLE".into(), role.clone());
    }

    // Pedido > gravado > padrão do cargo; fica gravado para a interface e para reabrir o app.
    let model = core.assign_model(project, ident, None);
    let (program, args, label) = if base == "ollama" {
        let resolved = reg.resolve_ollama(sub_model).ok_or_else(|| def.missing.to_owned())?;
        let model = sub_model.map(str::to_owned).unwrap_or(resolved.model);
        env.insert("COMMANDDECK_OLLAMA_MODEL".into(), model.clone());
        env.insert("COMMANDDECK_OLLAMA_HOST".into(), core.config().ollama_host);
        // O `ask-agent` do modelo roda como ferramenta e precisa da resposta no stdout: espera tudo.
        env.insert("COMMANDDECK_ASK_WAIT".into(), "0".into());
        (ollama_agent_exe(&core.paths().bin_dir), vec![model.clone()], format!("Ollama ({model})"))
    } else {
        let resolved = reg.resolve(base).ok_or_else(|| def.missing.to_owned())?;
        let mut args = Vec::new();
        if base == "claude" {
            args.push("--append-system-prompt".into());
            args.push(reg.bridge_instructions_for(ident));
            // Responder a outro agente é rodar `ask-agent`: libera só esse comando, sem perguntar.
            args.push("--allowedTools".into());
            args.push("Bash(ask-agent:*)".into());
            // Hooks avisam o início e o fim de cada turno (ver `delivery.rs`).
            match write_claude_hooks(&core.paths().data_dir) {
                Ok(path) => {
                    args.push("--settings".into());
                    args.push(path.to_string_lossy().into_owned());
                }
                Err(e) => log::warn!("não consegui gravar os hooks do Claude: {e}"),
            }
        }
        if base == "codex" && resolved.shim.is_none() {
            // Fim de turno do Codex → `ask-agent --signal idle <json>`. Pelo cmd.exe o valor TOML
            // (com aspas) não sobrevive, então só com o executável direto.
            args.push("-c".into());
            args.push(r#"notify=["ask-agent","--signal","idle"]"#.into());
        }
        if base == "claude" || base == "antigravity" {
            args.push("--add-dir".into());
            args.push(data_dir);
        }
        if let Some(m) = &model {
            if base == "opencode" {
                // O TUI não tem --model: servidor próprio + modelo na configuração (ver `models::opencode_tui`).
                let (extra, (key, value)) = models::opencode_tui(m);
                args.extend(extra);
                env.insert(key, value);
            } else {
                args.extend(models::cli_args(base, m));
            }
        }
        if resolved.shim.is_some() {
            args = args.iter().map(|a| cmd_safe_arg(a)).collect();
        }
        let mut full = resolved.pre;
        full.extend(args);
        (resolved.program, full, def.label.to_owned())
    };

    core.ensure_project_context(project, &project_dir);
    Ok(SpawnSpec {
        program,
        args,
        cwd,
        env,
        label,
        color: def.color.to_owned(),
        kind: kind.to_owned(),
        double_enter_on_mention: kind == "claude" || kind == "antigravity",
        model,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn aliases() {
        assert_eq!(resolve_alias("agy"), "antigravity");
        assert_eq!(resolve_alias("amazon-q"), "q");
        assert_eq!(resolve_alias("claude"), "claude");
    }

    #[test]
    fn disk_manifests() {
        let tmp = tempfile::tempdir().expect("tmp");
        let m = tmp.path().join("manifests").join("registry.ollama.ai");
        std::fs::create_dir_all(m.join("library").join("qwen3.5")).expect("mkdir");
        std::fs::create_dir_all(m.join("user").join("custom")).expect("mkdir");
        std::fs::write(m.join("library").join("qwen3.5").join("9b"), "{}").expect("write");
        std::fs::write(m.join("library").join("qwen3.5").join("4b"), "{}").expect("write");
        std::fs::write(m.join("user").join("custom").join("latest"), "{}").expect("write");
        let models = models_from_disk(&[tmp.path().to_path_buf()]);
        assert_eq!(models, ["qwen3.5:4b", "qwen3.5:9b", "user/custom:latest"]);
    }

    #[test]
    fn env_is_sanitized() {
        let base = vec![
            ("CLAUDECODE".to_owned(), "1".to_owned()),
            ("CLAUDE_CODE_ENTRYPOINT".to_owned(), "cli".to_owned()),
            ("HOME".to_owned(), "/h".to_owned()),
            ("PATH".to_owned(), "/usr/bin".to_owned()),
            // Herdados de um CommandDeck que abriu este (app rodando dentro de uma aba).
            ("COMMANDDECK_ROLE".to_owned(), "dev".to_owned()),
            ("COMMANDDECK_HOME".to_owned(), "/outro".to_owned()),
            ("COMMANDDECK_TOKEN".to_owned(), "velho".to_owned()),
            ("COMMANDDECK_DEBUG_UI".to_owned(), "1".to_owned()),
            // Configuração do usuário: continua valendo.
            ("COMMANDDECK_ASK_WAIT".to_owned(), "300".to_owned()),
        ];
        let env = build_agent_env(base, Path::new("/app/bin"), 1234, "tok", "claude", 1);
        for k in ["COMMANDDECK_ROLE", "COMMANDDECK_HOME", "COMMANDDECK_DEBUG_UI"] {
            assert!(!env.contains_key(k), "{k} herdado");
        }
        assert_eq!(env["COMMANDDECK_TOKEN"], "tok");
        assert_eq!(env["COMMANDDECK_ASK_WAIT"], "300");
        assert!(!env.contains_key("CLAUDECODE"));
        assert!(!env.contains_key("CLAUDE_CODE_ENTRYPOINT"));
        assert_eq!(env["HOME"], "/h");
        assert_eq!(env["PATH"], format!("/app/bin{PATH_SEP}/usr/bin"));
        assert_eq!(env["COMMANDDECK_PORT"], "1234");
        assert_eq!(env["COMMANDDECK_TOKEN"], "tok");
        assert_eq!(env["COMMANDDECK_AGENT"], "claude");
        assert_eq!(env["COMMANDDECK_DEPTH"], "1");
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
        let path = write_claude_hooks(tmp.path()).expect("hooks");
        let v: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(&path).expect("ler")).expect("json");
        assert_eq!(v["hooks"]["Stop"][0]["hooks"][0]["command"], "ask-agent --signal idle");
        assert_eq!(v["hooks"]["UserPromptSubmit"][0]["hooks"][0]["command"], "ask-agent --signal busy");
        assert_eq!(v["statusLine"], serde_json::json!({ "type": "command", "command": "ask-agent --usage", "padding": 0 }));
        assert_eq!(write_claude_hooks(tmp.path()).expect("de novo"), path);
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
