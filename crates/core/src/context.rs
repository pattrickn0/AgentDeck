//! Extension point for extra prompt context (a future local/remote RAG).
//!
//! Today no provider is registered: prompt injection and RAG are left for a later
//! step. The headless executor calls [`ContextProvider::augment`]
//! before sending the message, so plugging in a RAG does not require touching them.

/// A context request for a message about to be sent to an agent.
#[derive(Debug, Clone)]
pub struct ContextRequest<'a> {
    pub agent: &'a str,
    pub prompt: &'a str,
    pub workspace: &'a std::path::Path,
}

/// A retrieved piece of context (e.g. a document chunk with its source).
#[derive(Debug, Clone)]
pub struct ContextChunk {
    pub source: String,
    pub text: String,
}

pub trait ContextProvider: Send + Sync {
    fn name(&self) -> &str;
    /// Returns the relevant pieces. It must be fast or respect a time budget.
    fn augment(&self, req: &ContextRequest<'_>) -> Vec<ContextChunk>;
}

/// Shared list of registered providers (empty by default).
pub type Providers = std::sync::Arc<parking_lot::RwLock<Vec<std::sync::Arc<dyn ContextProvider>>>>;

/// Asks every registered provider, in registration order.
pub fn gather(providers: &Providers, req: &ContextRequest<'_>) -> Vec<ContextChunk> {
    let list: Vec<_> = providers.read().iter().cloned().collect();
    list.iter().flat_map(|p| p.augment(req)).collect()
}

/// Formats the pieces to append to a prompt (empty when there are no pieces).
pub fn render(chunks: &[ContextChunk]) -> String {
    if chunks.is_empty() {
        return String::new();
    }
    let body: Vec<String> = chunks.iter().map(|c| format!("[{}]\n{}", c.source, c.text.trim())).collect();
    format!("=== ADDITIONAL CONTEXT ===\n{}", body.join("\n\n"))
}
