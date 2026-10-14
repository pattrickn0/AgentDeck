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
        Self { ts: crate::util::now_ms(), kind, from: from.map(str::to_owned), to: to.map(str::to_owned), text: text.into() }
    }

    pub fn system(text: impl Into<String>) -> Self {
        Self::new(FeedKind::System, None, None, text)
    }
}

/// Fase de um download em andamento (barra de progresso da interface).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum DownloadState {
    Downloading,
    /// Conferindo o SHA-256 do arquivo baixado.
    Verifying,
    /// Conexão caiu: aguardando para tentar de novo.
    Retrying,
    Extracting,
    /// Download concluído, falta só o pós-processamento.
    Finalizing,
    Done,
    Error,
}

/// Progresso de um download (modelo ou binários do Whisper).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DownloadProgress {
    pub label: String,
    pub state: DownloadState,
    pub received: u64,
    pub total: Option<u64>,
    /// Bytes por segundo (média suavizada).
    pub speed: f64,
    /// Nova tentativa em andamento (`Retrying`): número e máximo.
    pub attempt: u32,
    pub max_attempts: u32,
    pub retry_in_ms: u64,
    /// Mensagem de erro (`Error`) ou do último erro transitório (`Retrying`).
    pub message: Option<String>,
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
    /// Progresso de um download em segundo plano.
    Download(DownloadProgress),
    /// Uma pergunta do `ask-agent` para uma instância (aba): a UI abre a aba se `create` e digita
    /// o `prompt` no terminal dela quando estiver pronta (ver [`Delivery`]).
    Deliver(Delivery),
    /// O agente da instância `id` avisou (por hook) que começou (`busy`) ou terminou um turno.
    /// Mais confiável que adivinhar pela saída do terminal; ver `delivery::gate`.
    AgentSignal {
        id: String,
        busy: bool,
    },
}

/// Mensagem a digitar no terminal de uma instância de agente.
#[derive(Debug, Clone)]
pub struct Delivery {
    /// Número da pergunta (para avisar o core se a entrega falhar).
    pub request: u64,
    /// Instância de destino (já registrada no core).
    pub ident: crate::instances::Ident,
    /// A aba ainda não existe: a UI precisa abri-la.
    pub create: bool,
    /// Quem perguntou.
    pub from: String,
    /// Texto completo a digitar (mensagem + como responder).
    pub prompt: String,
    /// É a resposta (atrasada) a uma pergunta que a instância fez, não uma pergunta nova:
    /// se a aba dela já fechou, não é reaberta.
    pub answer: bool,
}
