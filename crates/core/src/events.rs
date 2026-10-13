//! Eventos do backend para a UI (equivalente aos canais `feed` e `workspace:changed`).

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum FeedKind {
    /// Pergunta de um agente para outro via ask-agent.
    Ask,
    /// Resposta do agente consultado.
    Answer,
    /// Mensagem do usuário (envio para "todos" ou mesa).
    User,
    /// Fala de um agente numa mesa.
    Table,
    /// Indicador transitório ("rodada 1/2…", "⚙️ [read_file] x"). É substituído
    /// pela próxima mensagem do mesmo `from`.
    Thinking,
    /// Mensagem do sistema (cinza, sem moldura).
    System,
    /// Erro (borda vermelha).
    Error,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FeedEntry {
    /// Epoch em milissegundos.
    pub ts: u64,
    pub kind: FeedKind,
    pub from: Option<String>,
    /// Destinatário(s); vários separados por " + ".
    pub to: Option<String>,
    pub text: String,
}

impl FeedEntry {
    pub fn new(kind: FeedKind, from: Option<&str>, to: Option<&str>, text: impl Into<String>) -> Self {
        Self {
            ts: crate::util::now_ms(),
            kind,
            from: from.map(str::to_owned),
            to: to.map(str::to_owned),
            text: text.into(),
        }
    }

    pub fn system(text: impl Into<String>) -> Self {
        Self::new(FeedKind::System, None, None, text)
    }
}

#[derive(Debug, Clone)]
pub enum CoreEvent {
    Feed(FeedEntry),
    /// Algo mudou na pasta de trabalho (debounce de 300 ms) — recarregar a árvore.
    WorkspaceChanged,
    /// A mesa terminou (normalmente ou interrompida).
    TableFinished,
    /// A lista de agentes/modelos Ollama foi atualizada em background.
    AgentsChanged,
}
