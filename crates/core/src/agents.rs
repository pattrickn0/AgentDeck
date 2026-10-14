//! Registro de agentes: resolução dos executáveis oficiais, montagem do comando
//! interativo (PTY) e instruções da ponte (equivalente a `AGENTS`, `resolve*`, `peers`,
//! `bridgeInstructions` e `pty:spawn` do main.js).

use std::collections::BTreeMap;
use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use parking_lot::Mutex;

use crate::Core;

/// Item de menu de agente (equivalente ao retorno de `agents:list`).
#[derive(Debug, Clone, PartialEq)]
pub struct AgentInfo {
    /// `claude`, `antigravity`, `ollama`, `ollama:<modelo>`, `gemini`, ..., `shell`.
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

/// Todos os agentes, na ordem dos menus.
pub const AGENTS: &[AgentDef] = &[
    AgentDef {
        id: "claude",
        label: "Claude",
        color: COLOR_CLAUDE,
        peer: true,
        missing: "Claude Code CLI (claude) não encontrado. Instale com:\r\n  npm install -g @anthropic-ai/claude-code",
    },
    AgentDef {
        id: "antigravity",
        label: "Antigravity",
        color: COLOR_ANTIGRAVITY,
        peer: true,
        missing: "Antigravity CLI (agy) não encontrado. Instale no PowerShell com:\r\n  irm https://antigravity.google/cli/install.ps1 | iex",
    },
    AgentDef {
        id: "ollama",
        label: "Ollama",
        color: COLOR_OLLAMA,
        peer: true,
        missing: "Ollama CLI não encontrado. Instale em: https://ollama.com/download",
    },
    AgentDef {
        id: "gemini",
        label: "Gemini",
        color: "#1a73e8",
        peer: true,
        missing: "Gemini CLI não encontrado. Instale com:\r\n  npm install -g @google/gemini-cli",
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
    ("geminicli", "gemini"),
    ("gh-copilot", "copilot"),
    ("amazon-q", "q"),
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
}

impl Resolved {
    fn direct(p: PathBuf) -> Self {
        Self { program: p, pre: Vec::new() }
    }
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

    fn which(&self, name: &str) -> Option<PathBuf> {
        let found = match &self.disc.search_path {
            Some(p) => which::which_in(name, Some(p), std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."))),
            None => which::which(name),
        };
        found.ok()
    }

    fn home(&self) -> &Path {
        &self.disc.home
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
            let env = |k: &str| std::env::var_os(k).map(PathBuf::from).unwrap_or_default();
            let home = self.home().to_path_buf();
            let exe = format!("{name}.exe");
            let which_exe = || self.which(&exe);
            let local_bin = Some(home.join(".local").join("bin").join(&exe));
            let candidates = match id {
                "claude" => vec![
                    Some(env("APPDATA").join("npm").join("node_modules").join("@anthropic-ai").join("claude-code").join("bin").join("claude.exe")),
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
                "gemini" => vec![which_exe()],
                _ => vec![which_exe(), local_bin],
            };
            if let Some(p) = first_existing(candidates) {
                return Some(Resolved::direct(p));
            }
            // Shims do npm/pip (`.cmd`) precisam passar pelo cmd.exe.
            let shim = if id == "antigravity" {
                self.which("agy.cmd")
            } else {
                self.which(&format!("{name}.cmd")).or_else(|| self.which(name))
            };
            shim.map(|p| {
                if p.extension().is_some_and(|e| e.eq_ignore_ascii_case("exe")) {
                    Resolved::direct(p)
                } else {
                    Resolved { program: "cmd.exe".into(), pre: vec!["/d".into(), "/c".into(), name.into()] }
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
        let (program, mut pre): (PathBuf, Vec<String>) =
            if is_cmd { ("cmd.exe".into(), vec!["/d".into(), "/c".into(), "gh".into()]) } else { (gh, Vec::new()) };
        let mut cmd = std::process::Command::new(&program);
        cmd.args(&pre).args(["extension", "list"]);
        let out = crate::process::run_captured(cmd, None, Duration::from_secs(3), None, None).ok()?;
        if out.stdout.contains("gh-copilot") || out.stdout.contains("copilot") {
            pre.push("copilot".into());
            return Some(Resolved { program, pre });
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
            let env = |k: &str, d: &str| std::env::var_os(k).map(PathBuf::from).unwrap_or_else(|| PathBuf::from(d));
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
        let model = specific
            .map(str::to_owned)
            .or_else(|| models.first().cloned())
            .unwrap_or_else(|| DEFAULT_OLLAMA_MODEL.to_owned());
        (exe.is_some() || !models.is_empty()).then_some(OllamaResolved { model, models })
    }

    // ---------- Pares e instruções ----------

    /// Só os agentes instalados e habilitados como peer entram na ponte (+ `ollama:<modelo>`).
    pub fn peers(&self) -> Vec<String> {
        let mut out: Vec<String> =
            AGENTS.iter().filter(|a| a.peer && self.resolve(a.id).is_some()).map(|a| a.id.to_owned()).collect();
        if let Some(o) = self.resolve_ollama(None) {
            out.extend(o.models.iter().map(|m| format!("ollama:{m}")));
        }
        out
    }

    /// Texto injetado no prompt de sistema / AGENTS.md (equivalente a `bridgeInstructions`).
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
                "Agentes disponíveis: {}. A resposta é impressa no stdout e pode levar alguns minutos (use timeout de até 600000 ms).",
                others.join(",")
            ),
            "O histórico de cada par de agentes é mantido entre chamadas. O usuário tem acesso à \"Conversa integrada\" em tempo real no CommandDeck, portanto você NÃO precisa relatar nem resumir a conversa dos agentes para ele.".to_owned(),
            "Ao colaborar com outro agente (ex: Antigravity), mantenha o diálogo diretamente com ele (iterando, tirando dúvidas e alinhando) até concluir a tarefa, em vez de parar no meio para fazer perguntas ao usuário.".to_owned(),
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
        Resolved { program: "powershell.exe".into(), pre: vec!["-NoLogo".into()] }
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
pub(crate) fn agent_env(core: &Core, agent_id: &str, depth: i64) -> BTreeMap<String, String> {
    let base: Vec<(String, String)> =
        std::env::vars_os().map(|(k, v)| (k.to_string_lossy().into_owned(), v.to_string_lossy().into_owned())).collect();
    build_agent_env(base, &core.paths().bin_dir, core.bridge_port(), core.bridge_token(), agent_id, depth)
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
        let removed = if cfg!(windows) { REMOVE.iter().any(|r| r.eq_ignore_ascii_case(&k)) } else { REMOVE.contains(&k.as_str()) };
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
    env
}

/// Nome do binário `ollama-agent` na pasta dos executáveis.
pub fn ollama_agent_exe(bin_dir: &Path) -> PathBuf {
    bin_dir.join(if cfg!(windows) { "ollama-agent.exe" } else { "ollama-agent" })
}

/// Máximo de caracteres do primeiro prompt de uma aba (vai em argv ou variável de ambiente).
pub const MAX_INITIAL_PROMPT: usize = 4000;

/// O que acrescentar ao abrir uma aba a partir de uma conversa da ponte (takeover do visualizador).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SpawnExtras {
    /// Id da conversa a retomar (o mesmo que a ponte usa em modo headless).
    pub resume: Option<String>,
    /// Primeira mensagem, já enviada ao agente ao subir.
    pub prompt: Option<String>,
}

/// Agentes cujo CLI sabe retomar a conversa da ponte e receber um prompt inicial: só eles abrem em
/// modo visualizador. Os demais ganham uma aba interativa comum.
pub fn viewer_supported(kind: &str) -> bool {
    kind == "claude" || kind == "antigravity" || kind.starts_with("ollama:")
}

/// Um id de conversa só pode ter letras, números, `-`, `_` e `.` (vai em argv).
fn safe_conversation_id(id: &str) -> bool {
    !id.is_empty() && id.len() <= 128 && id.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'))
}

/// `.cmd`/`.bat` passam pelo `cmd.exe`, que interpreta `& | ^`: nesses casos o texto do usuário
/// nunca vai em argv.
fn is_shell_shim(program: &Path) -> bool {
    program
        .extension()
        .and_then(|e| e.to_str())
        .is_some_and(|e| e.eq_ignore_ascii_case("cmd") || e.eq_ignore_ascii_case("bat"))
}

/// Argumentos e variáveis de ambiente extras de [`SpawnExtras`] para o agente `base`.
///
/// - `claude`: `--resume <id>` e o prompt como argumento posicional, depois de `--` (o
///   `--add-dir` aceita vários valores e engoliria o prompt).
/// - `antigravity`: `--conversation <id>` e `-i <prompt>` (continua interativo depois do prompt).
/// - `ollama`: o prompt vai em `COMMANDDECK_INITIAL_PROMPT` (o `ollama-agent` o lê ao subir).
pub(crate) fn extras_for(base: &str, program: &Path, extras: &SpawnExtras) -> (Vec<String>, Vec<(String, String)>) {
    let resume = extras.resume.as_deref().filter(|id| safe_conversation_id(id));
    let prompt: Option<String> = extras
        .prompt
        .as_deref()
        .map(|p| p.chars().filter(|c| *c != '\0').take(MAX_INITIAL_PROMPT).collect::<String>())
        .filter(|p| !p.trim().is_empty());
    let (mut args, mut env) = (Vec::new(), Vec::new());
    match base {
        "claude" => {
            if let Some(id) = resume {
                args.extend(["--resume".to_owned(), id.to_owned()]);
            }
            if let (Some(p), false) = (&prompt, is_shell_shim(program)) {
                args.extend(["--".to_owned(), p.clone()]);
            }
        }
        "antigravity" => {
            if let Some(id) = resume {
                args.extend(["--conversation".to_owned(), id.to_owned()]);
            }
            if let (Some(p), false) = (&prompt, is_shell_shim(program)) {
                args.extend(["-i".to_owned(), p.clone()]);
            }
        }
        "ollama" => {
            if let Some(p) = prompt {
                env.push(("COMMANDDECK_INITIAL_PROMPT".to_owned(), p));
            }
        }
        _ => {}
    }
    (args, env)
}

/// Monta o comando de uma aba interativa (equivalente a `pty:spawn`).
pub(crate) fn spawn_spec(core: &Core, kind: &str) -> Result<SpawnSpec, String> {
    spawn_spec_with(core, kind, &SpawnExtras::default())
}

/// Como [`spawn_spec`], retomando uma conversa e/ou enviando o primeiro prompt.
pub(crate) fn spawn_spec_with(core: &Core, kind: &str, extras: &SpawnExtras) -> Result<SpawnSpec, String> {
    let reg = &core.inner.agents;
    let sub_model = kind.strip_prefix("ollama:");
    let base = if sub_model.is_some() { "ollama" } else { kind };
    let def = agent_def(base).ok_or_else(|| format!("Agente desconhecido: {kind}"))?;
    let data_dir = core.paths().data_dir.to_string_lossy().into_owned();
    let mut env = agent_env(core, kind, 0);

    let (program, args, label) = if base == "ollama" {
        let resolved = reg.resolve_ollama(sub_model).ok_or_else(|| def.missing.to_owned())?;
        let model = sub_model.map(str::to_owned).unwrap_or(resolved.model);
        env.insert("COMMANDDECK_OLLAMA_MODEL".into(), model.clone());
        env.insert("COMMANDDECK_OLLAMA_HOST".into(), core.config().ollama_host);
        let program = ollama_agent_exe(&core.paths().bin_dir);
        env.extend(extras_for("ollama", &program, extras).1);
        (program, vec![model.clone()], format!("Ollama ({model})"))
    } else {
        let resolved = reg.resolve(base).ok_or_else(|| def.missing.to_owned())?;
        let mut args = resolved.pre;
        if base == "claude" {
            args.push("--append-system-prompt".into());
            args.push(reg.bridge_instructions(Some("claude")));
        }
        if base == "claude" || base == "antigravity" {
            args.push("--add-dir".into());
            args.push(data_dir);
        }
        // Por último: o prompt posicional do Claude precisa vir depois de todas as opções.
        args.extend(extras_for(base, &resolved.program, extras).0);
        (resolved.program, args, def.label.to_owned())
    };

    Ok(SpawnSpec {
        program,
        args,
        cwd: core.workspace(),
        env,
        label,
        color: def.color.to_owned(),
        kind: kind.to_owned(),
        double_enter_on_mention: kind == "claude" || kind == "antigravity",
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extras_per_agent() {
        let exe = Path::new("C:/bin/agent.exe");
        let both = SpawnExtras { resume: Some("abc-123".into()), prompt: Some("oi & tudo bem?".into()) };

        let (args, env) = extras_for("claude", exe, &both);
        assert_eq!(args, ["--resume", "abc-123", "--", "oi & tudo bem?"]);
        assert!(env.is_empty());

        let (args, _) = extras_for("antigravity", exe, &both);
        assert_eq!(args, ["--conversation", "abc-123", "-i", "oi & tudo bem?"]);

        let (args, env) = extras_for("ollama", exe, &both);
        assert!(args.is_empty());
        assert_eq!(env, [("COMMANDDECK_INITIAL_PROMPT".to_owned(), "oi & tudo bem?".to_owned())]);

        // Agentes sem retomada verificada não recebem nada.
        assert_eq!(extras_for("gemini", exe, &both), (Vec::new(), Vec::new()));
        assert_eq!(extras_for("claude", exe, &SpawnExtras::default()), (Vec::new(), Vec::new()));
    }

    #[test]
    fn extras_are_sanitized() {
        let exe = Path::new("agent.exe");
        // Id suspeito (espaço, aspas, ponto e vírgula) é descartado; o prompt segue.
        let bad = SpawnExtras { resume: Some("x; rm -rf /".into()), prompt: Some("ola".into()) };
        assert_eq!(extras_for("claude", exe, &bad).0, ["--", "ola"]);
        // Prompt vazio/só espaços não vira argumento.
        let blank = SpawnExtras { resume: Some("id1".into()), prompt: Some("   ".into()) };
        assert_eq!(extras_for("antigravity", exe, &blank).0, ["--conversation", "id1"]);
        // Limite de tamanho e NUL removido.
        let long = SpawnExtras { resume: None, prompt: Some(format!("a\0{}", "b".repeat(MAX_INITIAL_PROMPT * 2))) };
        let args = extras_for("antigravity", exe, &long).0;
        assert_eq!(args[1].chars().count(), MAX_INITIAL_PROMPT);
        assert!(!args[1].contains('\0'));
    }

    #[test]
    fn shell_shims_never_get_the_prompt_in_argv() {
        let extras = SpawnExtras { resume: Some("id1".into()), prompt: Some("a & calc".into()) };
        for shim in ["C:/npm/claude.cmd", "C:/npm/claude.BAT"] {
            assert_eq!(extras_for("claude", Path::new(shim), &extras).0, ["--resume", "id1"]);
        }
        // O ollama-agent usa variável de ambiente, então segue com o prompt.
        assert_eq!(extras_for("ollama", Path::new("x.cmd"), &extras).1.len(), 1);
    }

    #[test]
    fn viewer_support() {
        assert!(viewer_supported("claude"));
        assert!(viewer_supported("antigravity"));
        assert!(viewer_supported("ollama:qwen3.5:9b"));
        assert!(!viewer_supported("gemini"));
        assert!(!viewer_supported("ollama")); // sem modelo definido
    }

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
        ];
        let env = build_agent_env(base, Path::new("/app/bin"), 1234, "tok", "claude", 1);
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

    #[cfg(windows)]
    #[test]
    fn env_path_casing_windows() {
        let base = vec![("Path".to_owned(), "C:\\Windows".to_owned())];
        let env = build_agent_env(base, Path::new("C:\\app"), 1, "t", "shell", 0);
        assert!(!env.contains_key("Path"));
        assert_eq!(env["PATH"], "C:\\app;C:\\Windows");
    }
}
