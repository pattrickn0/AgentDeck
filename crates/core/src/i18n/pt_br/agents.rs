//! pt-BR translations for the area: Agent registry, capability, models and the Holt instructions (crates/core/src/agents.rs, capability.rs, models.rs).
//!
//! Entries are `(English, Portuguese)`; the English text is the key used in `t!("...")`. See the `i18n` module docs for
//! the conventions and the coverage test.

pub const ENTRIES: &[(&str, &str)] = &[
    ("{kind} has no chat view.", "{kind} não tem visão de chat."),
    // Effort (models.rs)
    ("low", "baixo"),
    ("medium", "médio"),
    ("high", "alto"),
    ("Pick a model: in Antigravity the effort is part of its name", "Escolha um modelo: no Antigravity o esforço é parte do nome dele"),
    ("This Antigravity model has no effort variants", "Este modelo do Antigravity não tem versões por esforço"),
    (
        "OpenCode only accepts effort in headless mode; the tab does not get it",
        "O OpenCode só aceita esforço no modo headless; a aba não recebe",
    ),
    ("This agent has no effort", "Este agente não tem esforço"),
    ("invalid model: \"{model}\"", "modelo inválido: \"{model}\""),
    ("the {kind} agent does not accept choosing a model", "o agente {kind} não aceita escolher modelo"),
    // Capability (capability.rs)
    ("Simple", "Simples"),
    ("Medium", "Média"),
    ("Complex", "Complexa"),
    ("simple", "simples"),
    ("complex", "complexa"),
    ("low effort: down to {level}", "esforço baixo: desce para {level}"),
    (
        "measured: {count} {level} tasks in a row on the first try, up to {next}",
        "medida: {count} tarefas {level} seguidas de primeira, sobe para {next}",
    ),
    ("measured: failed at {level}, down to {next}", "medida: falhou em {level}, desce para {next}"),
    (
        "no model capable of a {level} task has quota; I used {agent}, the least spent",
        "nenhum modelo capaz de tarefa {level} tem cota; usei {agent}, o menos gasto",
    ),
    (
        "no available model can handle a {level} task; {agent} only reaches {capacity}",
        "nenhum modelo disponível dá conta de tarefa {level}; {agent} vai até {capacity}",
    ),
    ("{agent}, the one that reaches furthest (up to {capacity})", "{agent}, o que mais alcança (até {capacity})"),
    ("{agent}, capable and with {free}% free", "{agent}, capaz e com {free}% livre"),
    ("{agent}, capable and without quota", "{agent}, capaz e sem cota"),
    ("{agent}, capable (no quota reading)", "{agent}, capaz (sem leitura de cota)"),
    ("{agent} only reaches {capacity}", "{agent} só vai até {capacity}"),
    ("{agent} {used}% used", "{agent} {used}% usada"),
    ("{agent} no quota reading", "{agent} sem leitura de cota"),
    ("{level}: {parts}", "{level}: {parts}"),
    ("the task is {level} and {agent} only reaches {capacity}{why}", "a tarefa é {level} e {agent} só vai até {capacity}{why}"),
    // Install hints (agents.rs)
    (
        "Claude Code CLI (claude) not found. Install with:\r\n  npm install -g @anthropic-ai/claude-code",
        "Claude Code CLI (claude) não encontrado. Instale com:\r\n  npm install -g @anthropic-ai/claude-code",
    ),
    (
        "Antigravity CLI (agy) not found. Install from PowerShell with:\r\n  irm https://antigravity.google/cli/install.ps1 | iex",
        "Antigravity CLI (agy) não encontrado. Instale no PowerShell com:\r\n  irm https://antigravity.google/cli/install.ps1 | iex",
    ),
    (
        "Antigravity CLI (agy) not found. See the installer for your system at:\r\n  https://antigravity.google",
        "Antigravity CLI (agy) não encontrado. Veja o instalador para o seu sistema em:\r\n  https://antigravity.google",
    ),
    (
        "Codex CLI not found. Install with:\r\n  npm install -g @openai/codex",
        "Codex CLI não encontrado. Instale com:\r\n  npm install -g @openai/codex",
    ),
    (
        "OpenCode not found. Install with:\r\n  npm install -g opencode-ai",
        "OpenCode não encontrado. Instale com:\r\n  npm install -g opencode-ai",
    ),
    (
        "Aider CLI not found. Install with:\r\n  pip install aider-chat",
        "Aider CLI não encontrado. Instale com:\r\n  pip install aider-chat",
    ),
    (
        "Goose CLI not found. Install from: https://github.com/block/goose",
        "Goose CLI não encontrado. Instale a partir de: https://github.com/block/goose",
    ),
    (
        "GitHub Copilot CLI not found. Install with:\r\n  gh extension install github/gh-copilot",
        "GitHub Copilot CLI não encontrado. Instale com:\r\n  gh extension install github/gh-copilot",
    ),
    ("Amazon Q Developer CLI not found.", "Amazon Q Developer CLI não encontrado."),
    (
        "Open Interpreter not found. Install with:\r\n  pip install open-interpreter",
        "Open Interpreter não encontrado. Instale com:\r\n  pip install open-interpreter",
    ),
    ("Shell not found.", "Shell não encontrado."),
    // Errors (agents.rs)
    ("Unknown project: {project}", "Projeto desconhecido: {project}"),
    (
        "The task folder no longer exists ({dir}). Discard the task on the tasks desk.",
        "A pasta da tarefa não existe mais ({dir}). Descarte a tarefa na mesa de tarefas.",
    ),
    ("Unknown agent: {kind}", "Agente desconhecido: {kind}"),
];
