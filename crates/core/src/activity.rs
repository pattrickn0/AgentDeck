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
}
