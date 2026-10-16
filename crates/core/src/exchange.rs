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
    /// Quantas perguntas pendentes há antes desta na cadeia (0 = quem perguntou não estava
    /// respondendo a ninguém).
    depth: i64,
    /// Quando a pergunta foi feita (epoch ms).
    since_ms: u64,
    /// O `ask-agent` de quem perguntou já voltou (HTTP 202): a resposta será digitada depois no
    /// terminal dele. Quem perguntou não está bloqueado, então a pergunta não entra na detecção de
    /// ciclos (mas continua contando na profundidade).
    detached: bool,
    tx: Sender<Reply>,
}

/// Retrato de uma pergunta pendente (para o mapa de conversas da interface).
///
/// As chaves internas têm o escopo do projeto (`<projeto>#<id>`, ver [`crate::instances::scoped`]);
/// aqui vêm separadas: `project` e os ids sem o projeto. Uma chave sem escopo dá `project` vazio.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PendingInfo {
    pub id: u64,
    pub project: String,
    pub asker: String,
    pub target: String,
    pub depth: i64,
    pub since_ms: u64,
    pub detached: bool,
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
        for p in list.iter().filter(|p| p.asker == node && !p.detached) {
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
        self.register_at(asker, target, 0)
    }

    /// Como [`Exchange::register`], guardando a profundidade da pergunta na cadeia (ver
    /// [`Exchange::chain_depth`]).
    pub fn register_at(&self, asker: &str, target: &str, depth: i64) -> Result<(u64, Receiver<Reply>), String> {
        let mut st = self.state.lock();
        if asker == target || reaches(&st.list, target, asker) {
            return Err(format!("\"{target}\" já está esperando uma resposta de \"{asker}\" (ciclo de espera)"));
        }
        let id = st.next;
        st.next += 1;
        let (tx, rx) = bounded(1);
        let since_ms = crate::util::now_ms();
        st.list.push(Pending { id, asker: asker.to_owned(), target: target.to_owned(), depth, since_ms, detached: false, tx });
        Ok((id, rx))
    }

    /// `asker` esperar `target` fecharia um ciclo de espera?
    pub fn would_cycle(&self, asker: &str, target: &str) -> bool {
        asker == target || reaches(&self.state.lock().list, target, asker)
    }

    /// Renomeia uma instância nas perguntas pendentes.
    pub fn rename(&self, old_id: &str, new_id: &str) {
        let mut st = self.state.lock();
        for p in &mut st.list {
            if p.asker == old_id {
                p.asker = new_id.to_owned();
            }
            if p.target == old_id {
                p.target = new_id.to_owned();
            }
        }
    }

    /// Profundidade de uma pergunta nova feita por `asker`: 0 se ninguém espera resposta dele;
    /// senão, uma a mais que a pergunta mais profunda que ele está respondendo. Com abas, a
    /// profundidade não viaja pelo ambiente (a pergunta é digitada), então é medida aqui.
    pub fn chain_depth(&self, asker: &str) -> i64 {
        self.state.lock().list.iter().filter(|p| p.target == asker).map(|p| p.depth + 1).max().unwrap_or(0)
    }

    /// `from` respondeu a `to`: entrega ao pedido pendente mais antigo de `to` para `from`.
    ///
    /// Uma pergunta nova de `from` para `to` enquanto `to` espera `from` fecharia um ciclo de
    /// espera (recusado em [`Exchange::register`]); por isso essa mensagem só pode ser resposta.
    pub fn reply(&self, from: &str, to: &str, text: &str) -> bool {
        let mut st = self.state.lock();
        let Some(pos) = st.list.iter().position(|p| p.asker == to && p.target == from) else { return false };
        let p = st.list.remove(pos);
        let _ = p.tx.send(Reply::Answer(text.to_owned()));
        true
    }

    /// `from` respondeu à pergunta número `id`. Só vale se a pergunta foi feita a `from`.
    /// Devolve quem perguntou.
    pub fn reply_id(&self, id: u64, from: &str, text: &str) -> Option<String> {
        let mut st = self.state.lock();
        let pos = st.list.iter().position(|p| p.id == id && p.target == from)?;
        let p = st.list.remove(pos);
        let _ = p.tx.send(Reply::Answer(text.to_owned()));
        Some(p.asker)
    }

    /// Há alguém esperando resposta de `from` para `to`? (`ask-agent` de `from` para `to` seria resposta.)
    pub fn is_waiting(&self, asker: &str, target: &str) -> bool {
        self.state.lock().list.iter().any(|p| p.asker == asker && p.target == target)
    }

    /// O pedido `id` ainda espera resposta (não foi respondido, cancelado nem expirou).
    pub fn is_pending(&self, id: u64) -> bool {
        self.state.lock().list.iter().any(|p| p.id == id)
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

    /// Falha todos os pedidos feitos por `asker` (a aba dele foi fechada: ninguém lê a resposta).
    pub fn fail_asker(&self, asker: &str, reason: &str) -> usize {
        let mut st = self.state.lock();
        let (gone, kept): (Vec<Pending>, Vec<Pending>) = std::mem::take(&mut st.list).into_iter().partition(|p| p.asker == asker);
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

    /// O `ask-agent` de quem fez a pergunta `id` voltou sem a resposta (202). Idempotente.
    pub fn detach(&self, id: u64) {
        if let Some(p) = self.state.lock().list.iter_mut().find(|p| p.id == id) {
            p.detached = true;
        }
    }

    /// Perguntas pendentes, da mais antiga para a mais nova.
    pub fn snapshot(&self) -> Vec<PendingInfo> {
        self.state
            .lock()
            .list
            .iter()
            .map(|p| {
                let (project, asker) = crate::instances::unscoped(&p.asker);
                PendingInfo {
                    id: p.id,
                    project: project.to_owned(),
                    asker: asker.to_owned(),
                    target: crate::instances::unscoped(&p.target).1.to_owned(),
                    depth: p.depth,
                    since_ms: p.since_ms,
                    detached: p.detached,
                }
            })
            .collect()
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
    fn replies_by_id_only_from_the_target() {
        let ex = Exchange::default();
        let (first, r1) = ex.register("a", "b").unwrap();
        let (second, r2) = ex.register("a", "b").unwrap();
        assert_eq!(ex.reply_id(second, "c", "intruso"), None, "só quem recebeu a pergunta responde");
        assert_eq!(ex.reply_id(second, "b", "dois").as_deref(), Some("a"));
        assert_eq!(r2.recv_timeout(T), Ok(Reply::Answer("dois".into())));
        assert!(r1.try_recv().is_err(), "a outra pergunta segue pendente");
        assert_eq!(ex.reply_id(second, "b", "de novo"), None);
        assert_eq!(ex.reply_id(first, "b", "um").as_deref(), Some("a"));
    }

    #[test]
    fn chain_depth_follows_the_pending_questions() {
        let ex = Exchange::default();
        assert_eq!(ex.chain_depth("a"), 0);
        ex.register_at("a", "b", 0).unwrap();
        assert_eq!(ex.chain_depth("b"), 1, "b responde a uma pergunta de profundidade 0");
        ex.register_at("b", "c", 1).unwrap();
        assert_eq!(ex.chain_depth("c"), 2);
        assert_eq!(ex.chain_depth("a"), 0);
    }

    #[test]
    fn failing_an_asker_drops_its_questions() {
        let ex = Exchange::default();
        let (_, r1) = ex.register("a", "b").unwrap();
        let (_, r2) = ex.register("c", "b").unwrap();
        assert_eq!(ex.fail_asker("a", "fechada"), 1);
        assert_eq!(r1.recv_timeout(T), Ok(Reply::Failed("fechada".into())));
        assert!(r2.try_recv().is_err());
        assert!(!ex.is_waiting("a", "b"));
        // Sem a pergunta antiga, b pode perguntar a a sem "ciclo".
        assert!(ex.register("b", "a").is_ok());
    }

    #[test]
    fn the_snapshot_lists_pending_questions_in_order() {
        let ex = Exchange::default();
        let (a, _ra) = ex.register_at("a", "b", 0).unwrap();
        let (b, _rb) = ex.register_at("b", "c", 1).unwrap();
        let snap = ex.snapshot();
        assert_eq!(
            snap.iter().map(|p| (p.id, p.asker.as_str(), p.target.as_str(), p.depth)).collect::<Vec<_>>(),
            [(a, "a", "b", 0), (b, "b", "c", 1)]
        );
        assert!(snap.iter().all(|p| p.since_ms > 0 && !p.detached));
        ex.detach(a);
        ex.detach(a);
        assert!(ex.snapshot()[0].detached);
        ex.reply("b", "a", "ok");
        assert_eq!(ex.snapshot().len(), 1);
        assert_eq!(ex.snapshot()[0].project, "", "chave sem escopo");
        ex.detach(a); // já removida: nada acontece
        assert_eq!(ex.snapshot().len(), 1);
    }

    #[test]
    fn a_detached_question_does_not_block_and_makes_no_cycle() {
        // claude espera claude@dev, que espera antigravity: antigravity perguntar a claude fecharia o ciclo…
        let ex = Exchange::default();
        let (first, _r1) = ex.register("claude", "claude@dev").unwrap();
        ex.register("claude@dev", "antigravity").unwrap();
        assert!(ex.would_cycle("antigravity", "claude"));
        // …mas o ask-agent de claude já voltou (202): claude está livre, então não há ciclo.
        ex.detach(first);
        assert!(!ex.would_cycle("antigravity", "claude"));
        assert!(ex.register("antigravity", "claude").is_ok());
        // A profundidade continua contando a pergunta destacada.
        assert_eq!(ex.chain_depth("claude@dev"), 1);
    }

    #[test]
    fn scoped_keys_keep_projects_apart_and_the_snapshot_splits_them() {
        use crate::instances::scoped;
        let ex = Exchange::default();
        let (a, b) = ("p00000001", "p00000002");
        ex.register_at(&scoped(a, "claude"), &scoped(a, "claude@dev"), 0).unwrap();
        // O mesmo par em outro projeto não é ciclo nem conta na profundidade de lá.
        assert!(!ex.would_cycle(&scoped(b, "claude@dev"), &scoped(b, "claude")));
        assert!(ex.would_cycle(&scoped(a, "claude@dev"), &scoped(a, "claude")));
        assert_eq!(ex.chain_depth(&scoped(b, "claude@dev")), 0);
        assert_eq!(ex.chain_depth(&scoped(a, "claude@dev")), 1);
        // Responder no projeto errado não entrega.
        assert!(!ex.reply(&scoped(b, "claude@dev"), &scoped(b, "claude"), "x"));
        let snap = ex.snapshot();
        assert_eq!((snap[0].project.as_str(), snap[0].asker.as_str(), snap[0].target.as_str()), (a, "claude", "claude@dev"));
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
