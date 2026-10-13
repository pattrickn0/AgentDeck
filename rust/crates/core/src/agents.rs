//! Registro de agentes: resolução dos executáveis oficiais, montagem do comando
//! interativo (PTY) e instruções da ponte (equivalente a `AGENTS`, `resolve*`, `peers`,
//! `bridgeInstructions` e `pty:spawn` do main.js).

use std::collections::BTreeMap;
use std::path::PathBuf;

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
    /// Ambiente COMPLETO a usar (já derivado do ambiente do processo, com PATH/JWIC_* injetados
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
