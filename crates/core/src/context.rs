//! Ponto de extensão para contexto adicional de prompts (futuro RAG local/remoto).
//!
//! Hoje nenhum provedor é registrado: injeção de prompt e RAG ficam para uma etapa
//! futura. O executor headless chama [`ContextProvider::augment`]
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

/// Lista compartilhada de provedores registrados (vazia por padrão).
pub type Providers = std::sync::Arc<parking_lot::RwLock<Vec<std::sync::Arc<dyn ContextProvider>>>>;

/// Consulta todos os provedores registrados, na ordem de registro.
pub fn gather(providers: &Providers, req: &ContextRequest<'_>) -> Vec<ContextChunk> {
    let list: Vec<_> = providers.read().iter().cloned().collect();
    list.iter().flat_map(|p| p.augment(req)).collect()
}

/// Formata os trechos para anexar a um prompt (vazio quando não há trechos).
pub fn render(chunks: &[ContextChunk]) -> String {
    if chunks.is_empty() {
        return String::new();
    }
    let body: Vec<String> = chunks.iter().map(|c| format!("[{}]\n{}", c.source, c.text.trim())).collect();
    format!("=== CONTEXTO ADICIONAL ===\n{}", body.join("\n\n"))
}
