//! Backend events for the UI (equivalent to the `feed` and `workspace:changed` channels).

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum FeedKind {
    /// A question from one agent to another through ask-agent.
    Ask,
    /// The answer of the agent that was asked.
    Answer,
    /// A message from the user to the active tab.
    User,
    /// Transient indicator ("⚙️ [read_file] x"). It is replaced
    /// by the next message from the same `from`.
    Thinking,
    /// System message (grey, without a frame).
    System,
    /// Error (red border).
    Error,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FeedEntry {
    /// Epoch in milliseconds.
    pub ts: u64,
    pub kind: FeedKind,
    /// Id of the instance that speaks, without the project (`claude@dev`).
    pub from: Option<String>,
    /// Recipient (id without the project).
    pub to: Option<String>,
    pub text: String,
    /// Project of the conversation (`None` = global, like the Whisper notices).
    pub project: Option<String>,
}

impl FeedEntry {
    pub fn new(kind: FeedKind, from: Option<&str>, to: Option<&str>, text: impl Into<String>) -> Self {
        Self { ts: crate::util::now_ms(), kind, from: from.map(str::to_owned), to: to.map(str::to_owned), text: text.into(), project: None }
    }

    /// The same entry, in the project `project`.
    pub fn in_project(mut self, project: &str) -> Self {
        self.project = Some(project.to_owned());
        self
    }

    pub fn system(text: impl Into<String>) -> Self {
        Self::new(FeedKind::System, None, None, text)
    }
}

/// Phase of a download in progress (the UI's progress bar).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum DownloadState {
    Downloading,
    /// Checking the SHA-256 of the downloaded file.
    Verifying,
    /// The connection dropped: waiting to try again.
    Retrying,
    Extracting,
    /// Download finished, only the post-processing is left.
    Finalizing,
    Done,
    Error,
}

/// Progress of a download (Whisper model or binaries).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DownloadProgress {
    pub label: String,
    pub state: DownloadState,
    pub received: u64,
    pub total: Option<u64>,
    /// Bytes per second (smoothed average).
    pub speed: f64,
    /// Retry in progress (`Retrying`): number and maximum.
    pub attempt: u32,
    pub max_attempts: u32,
    pub retry_in_ms: u64,
    /// Error message (`Error`) or of the last transient error (`Retrying`).
    pub message: Option<String>,
}

#[derive(Debug, Clone)]
pub enum CoreEvent {
    Feed(FeedEntry),
    /// Something changed in the working folder (300 ms debounce) — reload the tree.
    WorkspaceChanged,
    /// The workspace's git repository changed (`.git/HEAD`, index, refs…: a commit, checkout or fetch
    /// made by the terminal or by an agent) — read the status again.
    GitChanged,
    /// Progress of a download in the background.
    Download(DownloadProgress),
    /// The voice dictation was downloaded or removed, or the download finished (with or without an
    /// error): read `Core::dictation_installed` again.
    DictationChanged,
    /// A question from `ask-agent` to an instance (tab): the UI opens the tab if `create` and types
    /// the `prompt` in its terminal when it is ready (see [`Delivery`]).
    Deliver(Delivery),
    /// The agent of instance `id` of project `project` reported (through a hook) that it started (`busy`)
    /// or finished a turn. More reliable than guessing from the terminal output; see `delivery::gate`.
    AgentSignal {
        project: String,
        id: String,
        busy: bool,
    },
    /// The usage of some agent changed (see `Core::usage_snapshot`).
    UsageChanged,
    /// Result of an asynchronous GitHub operation (PRs, issues, Actions, logs).
    Github(crate::github::GithubEvent),
    /// The project's tasks changed (creation, start, report, merge…): read them again with
    /// `Core::tasks`. Emitted on every save, also when the change came from the bridge (`ask-agent --task`).
    TasksChanged {
        project: String,
    },
    /// The core shut down instance `id` of the project (task merged or discarded): the UI closes
    /// its tab, if it is open.
    CloseInstance {
        project: String,
        id: String,
    },
    /// A second opening of the app asked (`POST /activate`) this instance to show its window.
    Activate,
}

/// Message to type in an agent instance's terminal.
#[derive(Debug, Clone)]
pub struct Delivery {
    /// Question number (to tell the core if the delivery fails).
    pub request: u64,
    /// Project of the destination instance (and of who asked: the bridge does not cross projects).
    pub project: String,
    /// Destination instance (already registered in the core).
    pub ident: crate::instances::Ident,
    /// The tab does not exist yet: the UI has to open it.
    pub create: bool,
    /// Who asked (id without the project).
    pub from: String,
    /// Full text to type (message + how to answer).
    pub prompt: String,
    /// It is the (late) answer to a question the instance asked, not a new question:
    /// if its tab is already closed, it is not reopened.
    pub answer: bool,
    /// Short initial prompt (ASCII) that replaces `prompt` when the tab has to be opened (or reopened):
    /// it goes on the CLI's command line (`Core::set_boot_prompt`) instead of being typed into a terminal that
    /// is still starting. Only tasks use it; the UI types `prompt` if the CLI does not accept it or the tab is
    /// already alive.
    pub boot: Option<String>,
}
