//! Caixa de entrada de respostas (`ask-agent --inbox`).
//!
//! Toda pergunta de um agente a uma aba que termina (com resposta, no prazo ou depois, ou com falha)
//! deixa uma entrada aqui, na caixa de quem perguntou. A entrada nasce "não entregue" e passa a
//! entregue quando a resposta chega de fato a ele: o 200 da ponte foi escrito sem erro, ou a interface
//! digitou a resposta atrasada no terminal dele. Se nada disso aconteceu (a aba fechou, a conexão já
//! tinha sido abandonada), a resposta continua aqui, e `ask-agent --inbox` a mostra. A caixa vive na
//! memória do app: guarda as últimas [`INBOX_LIMIT`] respostas de cada instância.

use std::collections::{HashMap, VecDeque};

use parking_lot::Mutex;
use serde::Serialize;

/// Respostas guardadas por instância que perguntou.
pub const INBOX_LIMIT: usize = 20;

/// Uma resposta recebida por quem perguntou.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct InboxEntry {
    /// Número da pergunta.
    pub request: u64,
    /// Quem respondeu (id da instância, sem o projeto).
    pub from: String,
    /// O texto da resposta, ou o motivo da falha.
    pub text: String,
    /// A pergunta terminou sem resposta (`text` é o motivo).
    pub failed: bool,
    /// Epoch em milissegundos.
    pub ts: u64,
    /// A resposta já chegou a quem perguntou.
    pub delivered: bool,
}

/// Caixas por instância que perguntou (chave com o escopo do projeto: `instances::scoped`).
#[derive(Default)]
pub struct Inbox {
    boxes: Mutex<HashMap<String, VecDeque<InboxEntry>>>,
}

impl Inbox {
    /// Guarda a resposta na caixa de `asker`. Uma pergunta que já tem entrada ali é substituída; passando
    /// de [`INBOX_LIMIT`], sai a mais antiga.
    pub fn push(&self, asker: &str, entry: InboxEntry) {
        let mut boxes = self.boxes.lock();
        let list = boxes.entry(asker.to_owned()).or_default();
        list.retain(|e| e.request != entry.request);
        list.push_back(entry);
        while list.len() > INBOX_LIMIT {
            list.pop_front();
        }
    }

    /// Marca a resposta da pergunta `request` como entregue. `false` = não está em nenhuma caixa.
    pub fn mark_delivered(&self, request: u64) -> bool {
        let mut boxes = self.boxes.lock();
        let entry = boxes.values_mut().flat_map(|l| l.iter_mut()).find(|e| e.request == request);
        entry.map(|e| e.delivered = true).is_some()
    }

    /// A resposta da pergunta `request` e a quem ela pertence (chave de `asker`).
    pub fn find(&self, request: u64) -> Option<(String, InboxEntry)> {
        let boxes = self.boxes.lock();
        boxes.iter().find_map(|(asker, l)| l.iter().find(|e| e.request == request).map(|e| (asker.clone(), e.clone())))
    }

    /// As respostas de `asker`, da mais antiga para a mais nova: só as não entregues, ou as
    /// [`INBOX_LIMIT`] com `all`. Quem pede as não entregues as recebe aqui, então elas passam a entregues.
    pub fn read(&self, asker: &str, all: bool) -> Vec<InboxEntry> {
        let mut boxes = self.boxes.lock();
        let Some(list) = boxes.get_mut(asker) else { return Vec::new() };
        let shown: Vec<InboxEntry> = list.iter().filter(|e| all || !e.delivered).cloned().collect();
        for e in list.iter_mut() {
            e.delivered = true;
        }
        shown
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(request: u64) -> InboxEntry {
        InboxEntry { request, from: "antigravity".into(), text: format!("resposta {request}"), failed: false, ts: 1, delivered: false }
    }

    #[test]
    fn it_keeps_the_last_twenty_answers_of_each_asker() {
        let inbox = Inbox::default();
        for n in 0..25 {
            inbox.push("p::claude", entry(n));
        }
        inbox.push("p::opencode", entry(100));
        let all = inbox.read("p::claude", true);
        assert_eq!(all.len(), INBOX_LIMIT);
        assert_eq!(all.first().map(|e| e.request), Some(5));
        assert_eq!(all.last().map(|e| e.request), Some(24));
        assert_eq!(inbox.read("p::opencode", true).len(), 1);
        assert!(inbox.read("p::ninguem", true).is_empty());
    }

    #[test]
    fn delivered_answers_leave_the_default_listing_but_stay_in_all() {
        let inbox = Inbox::default();
        inbox.push("p::claude", entry(1));
        inbox.push("p::claude", entry(2));
        assert!(inbox.mark_delivered(1));
        assert!(!inbox.mark_delivered(9));
        let pending: Vec<u64> = inbox.read("p::claude", false).iter().map(|e| e.request).collect();
        assert_eq!(pending, [2]);
        // Ler as pendentes as entrega: a próxima leitura não as repete.
        assert!(inbox.read("p::claude", false).is_empty());
        let all: Vec<u64> = inbox.read("p::claude", true).iter().map(|e| e.request).collect();
        assert_eq!(all, [1, 2]);
    }

    #[test]
    fn a_repeated_question_replaces_its_entry_and_find_names_the_asker() {
        let inbox = Inbox::default();
        inbox.push("p::claude", entry(3));
        assert!(inbox.mark_delivered(3));
        inbox.push("p::claude", InboxEntry { text: "nova".into(), ..entry(3) });
        let (asker, found) = inbox.find(3).unwrap();
        assert_eq!((asker.as_str(), found.text.as_str(), found.delivered), ("p::claude", "nova", false));
        assert_eq!(inbox.read("p::claude", true).len(), 1);
        assert!(inbox.find(4).is_none());
    }
}
