//! Perguntas entre abas que aguardam resposta.
//!
//! Com abas interativas, `ask-agent A "msg"` digita a mensagem no terminal da instância A e o
//! `ask-agent` de quem perguntou fica **esperando**. A resposta chega quando A roda
//! `ask-agent <quem-perguntou> "resposta"`: essa chamada é reconhecida como resposta (há uma
//! pergunta pendente no sentido contrário) e entregue a quem espera. Nada de raspar a tela.

use crossbeam_channel::{bounded, Receiver, Sender};
use parking_lot::Mutex;

/// O que quem espera recebe.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Reply {
    Answer(String),
    Failed(String),
}

struct Pending {
    id: u64,
    asker: String,
    target: String,
    tx: Sender<Reply>,
}

#[derive(Default)]
struct State {
    next: u64,
    list: Vec<Pending>,
}

#[derive(Default)]
pub struct Exchange {
    state: Mutex<State>,
}

/// Há um caminho `from → … → to` seguindo as perguntas pendentes (quem espera → quem responde)?
fn reaches(list: &[Pending], from: &str, to: &str) -> bool {
    let mut seen = vec![from.to_owned()];
    let mut stack = vec![from.to_owned()];
    while let Some(node) = stack.pop() {
        for p in list.iter().filter(|p| p.asker == node) {
            if p.target == to {
                return true;
            }
            if !seen.contains(&p.target) {
                seen.push(p.target.clone());
                stack.push(p.target.clone());
            }
        }
    }
    false
}

impl Exchange {
    /// Registra que `asker` espera resposta de `target`. Recusa se isso fecharia um ciclo de
    /// espera (A espera B, B espera C, C espera A): ninguém responderia nunca.
    pub fn register(&self, asker: &str, target: &str) -> Result<(u64, Receiver<Reply>), String> {
        let mut st = self.state.lock();
        if asker == target || reaches(&st.list, target, asker) {
            return Err(format!("\"{target}\" já está esperando uma resposta de \"{asker}\" (ciclo de espera)"));
        }
        let id = st.next;
        st.next += 1;
        let (tx, rx) = bounded(1);
        st.list.push(Pending { id, asker: asker.to_owned(), target: target.to_owned(), tx });
        Ok((id, rx))
    }

    /// `from` respondeu a `to`: entrega ao pedido pendente mais antigo de `to` para `from`.
    pub fn reply(&self, from: &str, to: &str, text: &str) -> bool {
        let mut st = self.state.lock();
        let Some(pos) = st.list.iter().position(|p| p.asker == to && p.target == from) else { return false };
        let p = st.list.remove(pos);
        let _ = p.tx.send(Reply::Answer(text.to_owned()));
        true
    }

    /// Há alguém esperando resposta de `from` para `to`? (`ask-agent` de `from` para `to` seria resposta.)
    pub fn is_waiting(&self, asker: &str, target: &str) -> bool {
        self.state.lock().list.iter().any(|p| p.asker == asker && p.target == target)
    }

    /// Encerra um pedido sem resposta (timeout ou falha). Idempotente.
    pub fn fail(&self, id: u64, reason: &str) {
        let mut st = self.state.lock();
        if let Some(pos) = st.list.iter().position(|p| p.id == id) {
            let p = st.list.remove(pos);
            let _ = p.tx.send(Reply::Failed(reason.to_owned()));
        }
    }

    /// Falha todos os pedidos que aguardam `target` (a aba dele foi fechada ou morreu).
    pub fn fail_target(&self, target: &str, reason: &str) -> usize {
        let mut st = self.state.lock();
        let (gone, kept): (Vec<Pending>, Vec<Pending>) = std::mem::take(&mut st.list).into_iter().partition(|p| p.target == target);
        st.list = kept;
        for p in &gone {
            let _ = p.tx.send(Reply::Failed(reason.to_owned()));
        }
        gone.len()
    }

    /// Remove o pedido (quem esperava desistiu ou já recebeu).
    pub fn remove(&self, id: u64) {
        self.state.lock().list.retain(|p| p.id != id);
    }

    pub fn len(&self) -> usize {
        self.state.lock().list.len()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    const T: Duration = Duration::from_millis(500);

    #[test]
    fn a_reply_reaches_the_one_waiting() {
        let ex = Exchange::default();
        let (_, rx) = ex.register("claude", "claude@dev").unwrap();
        assert!(ex.is_waiting("claude", "claude@dev"));
        assert!(ex.reply("claude@dev", "claude", "pronto"));
        assert_eq!(rx.recv_timeout(T), Ok(Reply::Answer("pronto".into())));
        assert!(ex.is_empty());
        // Sem ninguém esperando, uma "resposta" não é resposta.
        assert!(!ex.reply("claude@dev", "claude", "de novo"));
    }

    #[test]
    fn replies_go_to_the_oldest_request_first() {
        let ex = Exchange::default();
        let (_, first) = ex.register("a", "b").unwrap();
        let (_, second) = ex.register("a", "b").unwrap();
        ex.reply("b", "a", "um");
        ex.reply("b", "a", "dois");
        assert_eq!(first.recv_timeout(T), Ok(Reply::Answer("um".into())));
        assert_eq!(second.recv_timeout(T), Ok(Reply::Answer("dois".into())));
    }

    #[test]
    fn the_direction_matters() {
        let ex = Exchange::default();
        let (_, rx) = ex.register("a", "b").unwrap();
        // c respondendo a a, ou b respondendo a c, não é a resposta esperada.
        assert!(!ex.reply("c", "a", "x"));
        assert!(!ex.reply("b", "c", "x"));
        assert!(rx.try_recv().is_err());
    }

    #[test]
    fn wait_cycles_are_refused() {
        let ex = Exchange::default();
        ex.register("a", "b").unwrap();
        ex.register("b", "c").unwrap();
        // c esperar a fecharia a → b → c → a.
        let err = ex.register("c", "a").unwrap_err();
        assert!(err.contains("ciclo"), "{err}");
        // Sem ciclo: d espera a, e a espera d só se d não depender de a… (a → b → c; d → a)
        ex.register("d", "a").unwrap();
        assert!(ex.register("a", "d").is_err(), "a → d → a");
        // Perguntar a si mesmo também.
        assert!(ex.register("x", "x").is_err());
    }

    #[test]
    fn failing_a_request_or_a_target_unblocks_the_waiters() {
        let ex = Exchange::default();
        let (id, rx) = ex.register("a", "b").unwrap();
        ex.fail(id, "tempo esgotado");
        assert_eq!(rx.recv_timeout(T), Ok(Reply::Failed("tempo esgotado".into())));
        ex.fail(id, "de novo"); // idempotente

        let (_, r1) = ex.register("a", "b").unwrap();
        let (_, r2) = ex.register("c", "b").unwrap();
        let (_, r3) = ex.register("a", "z").unwrap();
        assert_eq!(ex.fail_target("b", "a aba foi fechada"), 2);
        assert_eq!(r1.recv_timeout(T), Ok(Reply::Failed("a aba foi fechada".into())));
        assert_eq!(r2.recv_timeout(T), Ok(Reply::Failed("a aba foi fechada".into())));
        assert!(r3.try_recv().is_err(), "quem espera outro alvo segue esperando");
        assert_eq!(ex.len(), 1);
    }

    #[test]
    fn removing_a_request_forgets_it() {
        let ex = Exchange::default();
        let (id, _rx) = ex.register("a", "b").unwrap();
        ex.remove(id);
        assert!(!ex.is_waiting("a", "b"));
        // Depois de removido, o ciclo some.
        assert!(ex.register("b", "a").is_ok());
    }
}
