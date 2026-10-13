//! Ponto de extensão para contexto adicional de prompts (futuro RAG local/remoto).
//!
//! Hoje nenhum provedor é registrado: injeção de prompt e RAG ficam para uma etapa
//! futura. O motor Ollama e o executor headless chamam [`ContextProvider::augment`]
//! antes de enviar a mensagem, então plugar um RAG não exige mexer neles.

/// Pedido de contexto para uma mensagem prestes a ser enviada a um agente.
#[derive(Debug, Clone)]
pub struct ContextRequest<'a> {
    pub agent: &'a str,
    pub prompt: &'a str,
    pub workspace: &'a std::path::Path,
}

/// Trecho de contexto recuperado (ex.: chunk de documento com origem).
#[derive(Debug, Clone)]
pub struct ContextChunk {
    pub source: String,
    pub text: String,
}

pub trait ContextProvider: Send + Sync {
    fn name(&self) -> &str;
    /// Retorna trechos relevantes. Deve ser rápido ou respeitar um orçamento de tempo.
    fn augment(&self, req: &ContextRequest<'_>) -> Vec<ContextChunk>;
}
