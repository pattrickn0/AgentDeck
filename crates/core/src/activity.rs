//! Última atividade de cada aba de agente (saída no terminal ou turno em andamento).
//!
//! A interface avisa aqui enquanto uma aba está trabalhando. A ponte usa isso para contar o
//! tempo de espera de uma pergunta a partir do fim do trabalho da aba, e não de quando a
//! pergunta foi feita: um agente ocupado numa tarefa longa não estoura o prazo.

use std::collections::HashMap;
use std::time::Instant;

use parking_lot::Mutex;

#[derive(Default)]
pub struct Activity {
    last: Mutex<HashMap<String, Instant>>,
}

impl Activity {
    /// A aba `id` fez algo agora (produziu saída ou está no meio de um turno).
    pub fn touch(&self, id: &str, at: Instant) {
        let mut last = self.last.lock();
        match last.get_mut(id) {
            Some(prev) => *prev = (*prev).max(at),
            None => {
                last.insert(id.to_owned(), at);
            }
        }
    }

    /// Último sinal de atividade da aba `id`, se houve algum.
    pub fn last(&self, id: &str) -> Option<Instant> {
        self.last.lock().get(id).copied()
    }

    /// A aba foi fechada.
    pub fn forget(&self, id: &str) {
        self.last.lock().remove(id);
    }

    /// Esquece as abas cujas chaves começam com `prefix` (as de um projeto removido).
    pub fn forget_prefix(&self, prefix: &str) {
        self.last.lock().retain(|k, _| !k.starts_with(prefix));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[test]
    fn guarda_o_sinal_mais_recente() {
        let a = Activity::default();
        let t0 = Instant::now();
        assert_eq!(a.last("x"), None);
        a.touch("x", t0 + Duration::from_secs(5));
        a.touch("x", t0);
        assert_eq!(a.last("x"), Some(t0 + Duration::from_secs(5)));
        a.forget("x");
        assert_eq!(a.last("x"), None);
    }

    #[test]
    fn esquece_um_projeto_inteiro() {
        let a = Activity::default();
        let t0 = Instant::now();
        for k in ["p1#claude", "p1#claude@dev", "p2#claude"] {
            a.touch(k, t0);
        }
        a.forget_prefix("p1#");
        assert_eq!((a.last("p1#claude"), a.last("p1#claude@dev"), a.last("p2#claude")), (None, None, Some(t0)));
    }
}
