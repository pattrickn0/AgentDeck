//! Eventos do backend para a UI (equivalente aos canais `feed` e `workspace:changed`).

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum FeedKind {
    /// Pergunta de um agente para outro via ask-agent.
    Ask,
    /// Resposta do agente consultado.
    Answer,
    /// Mensagem do usuário para a aba ativa.
    User,
    /// Indicador transitório ("⚙️ [read_file] x"). É substituído
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
    /// Id da instância que fala, sem o projeto (`claude@dev`).
    pub from: Option<String>,
    /// Destinatário (id sem o projeto).
    pub to: Option<String>,
    pub text: String,
    /// Projeto da conversa (`None` = global, como os avisos do Whisper).
    pub project: Option<String>,
}

impl FeedEntry {
    pub fn new(kind: FeedKind, from: Option<&str>, to: Option<&str>, text: impl Into<String>) -> Self {
        Self { ts: crate::util::now_ms(), kind, from: from.map(str::to_owned), to: to.map(str::to_owned), text: text.into(), project: None }
    }

    /// A mesma entrada, no projeto `project`.
    pub fn in_project(mut self, project: &str) -> Self {
        self.project = Some(project.to_owned());
        self
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
    /// O repositório git do workspace mudou (`.git/HEAD`, índice, refs…: commit, checkout, fetch
    /// feitos pelo terminal ou por um agente) — reler o status.
    GitChanged,
    /// A lista de agentes/modelos Ollama foi atualizada em background.
    AgentsChanged,
    /// Progresso de um download em segundo plano.
    Download(DownloadProgress),
    /// O ditado por voz foi baixado, removido ou o download terminou (com ou sem erro): reler
    /// `Core::dictation_installed`.
    DictationChanged,
    /// Uma pergunta do `ask-agent` para uma instância (aba): a UI abre a aba se `create` e digita
    /// o `prompt` no terminal dela quando estiver pronta (ver [`Delivery`]).
    Deliver(Delivery),
    /// O agente da instância `id` do projeto `project` avisou (por hook) que começou (`busy`) ou
    /// terminou um turno. Mais confiável que adivinhar pela saída do terminal; ver `delivery::gate`.
    AgentSignal {
        project: String,
        id: String,
        busy: bool,
    },
    /// O uso de algum agente mudou (ver `Core::usage_snapshot`).
    UsageChanged,
    /// Resultado de uma operação assíncrona do GitHub (PRs, issues, Actions, logs).
    Github(crate::github::GithubEvent),
    /// As tarefas do projeto mudaram (criação, início, relatório, mesclagem…): reler com
    /// `Core::tasks`. Emitido a cada gravação, também quando a mudança veio da ponte (`ask-agent --task`).
    TasksChanged {
        project: String,
    },
    /// O core encerrou a instância `id` do projeto (tarefa mesclada ou descartada): a interface fecha
    /// a aba dela, se estiver aberta.
    CloseInstance {
        project: String,
        id: String,
    },
}

/// Mensagem a digitar no terminal de uma instância de agente.
#[derive(Debug, Clone)]
pub struct Delivery {
    /// Número da pergunta (para avisar o core se a entrega falhar).
    pub request: u64,
    /// Projeto da instância de destino (e de quem perguntou: a ponte não cruza projetos).
    pub project: String,
    /// Instância de destino (já registrada no core).
    pub ident: crate::instances::Ident,
    /// A aba ainda não existe: a UI precisa abri-la.
    pub create: bool,
    /// Quem perguntou (id sem o projeto).
    pub from: String,
    /// Texto completo a digitar (mensagem + como responder).
    pub prompt: String,
    /// É a resposta (atrasada) a uma pergunta que a instância fez, não uma pergunta nova:
    /// se a aba dela já fechou, não é reaberta.
    pub answer: bool,
    /// Prompt inicial curto (ASCII) que substitui `prompt` quando a aba precisa ser aberta (ou reaberta):
    /// vai na linha de comando do CLI (`Core::set_boot_prompt`) em vez de ser digitado num terminal que
    /// ainda está subindo. Só as tarefas usam; a interface digita `prompt` se o CLI não aceita ou a aba já está viva.
    pub boot: Option<String>,
}
