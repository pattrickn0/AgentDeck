//! Quando digitar uma mensagem de outro agente no terminal de uma aba.
//!
//! A pergunta do `ask-agent` é digitada no terminal da instância (ver `bridge.rs`). Digitar na hora
//! errada mistura o texto com o que o usuário está digitando ou com a resposta que o agente ainda
//! está escrevendo. Esta é a regra, sem interface, para a UI consultar a cada quadro.
//!
//! Sinais usados, do mais confiável ao menos:
//! 1. **Teclado do usuário**: nada é digitado logo depois de uma tecla, e espera-se bem mais
//!    quando há texto começado e ainda não enviado.
//! 2. **Hooks do agente** (`ask-agent --signal busy|idle`, ligados no Claude e no Codex): com
//!    eles, a aba só está livre depois do aviso de fim de turno.
//! 3. **Heurística da saída**: sem hooks, a aba está livre quando o terminal fica parado por
//!    [`READY_IDLE`]. Se nunca sossegar, a mensagem vai depois de [`READY_TIMEOUT`] na fila
//!    (contado de quando *entrou na fila*, não de quando a aba abriu).

use std::collections::VecDeque;
use std::time::{Duration, Instant};

/// Sem hooks, o terminal precisa ficar parado por este tempo.
pub const READY_IDLE: Duration = Duration::from_millis(1500);
/// Com hooks, basta a saída assentar depois do aviso de fim de turno.
pub const HOOK_SETTLE: Duration = Duration::from_millis(300);
/// Sem hooks, se o terminal nunca sossegar, a mensagem é digitada depois deste tempo na fila.
pub const READY_TIMEOUT: Duration = Duration::from_secs(30);
/// Depois de uma tecla do usuário (sem texto pendente: Enter, Ctrl+C, setas…).
pub const USER_QUIET: Duration = Duration::from_secs(2);
/// Depois de uma tecla do usuário que deixou texto começado na linha (sem Enter).
pub const TYPING_QUIET: Duration = Duration::from_secs(20);
/// Intervalo para conferir de novo enquanto a aba está ocupada.
pub const POLL: Duration = Duration::from_millis(300);

/// O que a UI sabe da aba agora.
#[derive(Debug, Clone, Copy)]
pub struct TabState {
    /// O processo está rodando.
    pub alive: bool,
    /// O agente está no meio de um turno (Enter do usuário, mensagem enviada ou hook `busy`).
    pub busy: bool,
    /// A aba já recebeu algum aviso por hook: `busy` vem do agente, não da heurística.
    pub hooked: bool,
    /// Última saída do processo.
    pub last_output: Option<Instant>,
    /// Última tecla do usuário nesta aba.
    pub last_key: Option<Instant>,
    /// O usuário começou a digitar algo e ainda não enviou (nem apagou com Ctrl+C/Ctrl+U/Esc).
    pub typing: bool,
}

/// Decisão para a primeira mensagem da fila.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Gate {
    /// Pode digitar agora.
    Ready,
    /// Conferir de novo neste instante.
    WaitUntil(Instant),
    /// Não há como entregar (a aba não está rodando).
    Fail,
}

/// Pode digitar no terminal da aba uma mensagem que está na fila desde `queued_at`?
pub fn gate(tab: &TabState, queued_at: Instant, now: Instant) -> Gate {
    if !tab.alive {
        return Gate::Fail;
    }
    if let Some(key) = tab.last_key {
        let quiet = if tab.typing { TYPING_QUIET } else { USER_QUIET };
        if now < key + quiet {
            return Gate::WaitUntil(key + quiet);
        }
    }
    if tab.busy {
        return Gate::WaitUntil(now + POLL);
    }
    let settle = if tab.hooked { HOOK_SETTLE } else { READY_IDLE };
    match tab.last_output {
        Some(out) if now >= out + settle => Gate::Ready,
        _ if !tab.hooked && now >= queued_at + READY_TIMEOUT => Gate::Ready,
        Some(out) if tab.hooked => Gate::WaitUntil(out + settle),
        Some(out) => Gate::WaitUntil((out + settle).min(queued_at + READY_TIMEOUT)),
        // Com hooks e sem nenhuma saída ainda: a TUI não desenhou nada, espere.
        None if tab.hooked => Gate::WaitUntil(now + POLL),
        None => Gate::WaitUntil(queued_at + READY_TIMEOUT),
    }
}

/// Fila de mensagens de uma aba, com o instante em que cada uma entrou.
#[derive(Debug)]
pub struct Mailbox<T> {
    items: VecDeque<(Instant, T)>,
}

impl<T> Default for Mailbox<T> {
    fn default() -> Self {
        Self { items: VecDeque::new() }
    }
}

impl<T> Mailbox<T> {
    pub fn push(&mut self, now: Instant, item: T) {
        self.items.push_back((now, item));
    }

    /// Quando a primeira mensagem entrou na fila.
    pub fn front_since(&self) -> Option<Instant> {
        self.items.front().map(|(t, _)| *t)
    }

    pub fn pop(&mut self) -> Option<T> {
        self.items.pop_front().map(|(_, item)| item)
    }

    pub fn drain(&mut self) -> impl Iterator<Item = T> + '_ {
        self.items.drain(..).map(|(_, item)| item)
    }

    pub fn is_empty(&self) -> bool {
        self.items.is_empty()
    }

    /// As mensagens na ordem da fila.
    pub fn iter(&self) -> impl Iterator<Item = &T> {
        self.items.iter().map(|(_, item)| item)
    }

    pub fn len(&self) -> usize {
        self.items.len()
    }
}

/// Efeito de uma tecla do usuário sobre "há texto começado na linha?".
/// `bytes` é o que a tecla manda ao processo.
pub fn typing_after(bytes: &[u8], was_typing: bool) -> bool {
    match bytes {
        // Enter envia; Ctrl+C, Ctrl+U e Esc (sozinho) costumam limpar a linha.
        b"\r" | b"\n" | [0x03] | [0x15] | [0x1b] => false,
        // Teclas de controle e sequências (setas, Backspace…) não mudam o estado.
        [b] if *b < 0x20 || *b == 0x7f => was_typing,
        [0x1b, ..] => was_typing,
        _ => true,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ms(n: u64) -> Duration {
        Duration::from_millis(n)
    }

    fn idle_tab(t0: Instant) -> TabState {
        TabState { alive: true, busy: false, hooked: false, last_output: Some(t0), last_key: None, typing: false }
    }

    #[test]
    fn a_dead_tab_fails() {
        let t0 = Instant::now();
        let tab = TabState { alive: false, ..idle_tab(t0) };
        assert_eq!(gate(&tab, t0, t0 + ms(5000)), Gate::Fail);
    }

    #[test]
    fn without_hooks_the_terminal_must_be_quiet() {
        let t0 = Instant::now();
        let tab = idle_tab(t0);
        assert_eq!(gate(&tab, t0, t0 + ms(1000)), Gate::WaitUntil(t0 + READY_IDLE));
        assert_eq!(gate(&tab, t0, t0 + READY_IDLE), Gate::Ready);
    }

    #[test]
    fn the_timeout_counts_from_the_queue_not_from_the_spawn() {
        // A aba existe há horas e ainda está escrevendo: antes a mensagem saía na hora (o prazo
        // contava de quando a aba abriu). Agora espera sossegar ou READY_TIMEOUT na fila.
        let t0 = Instant::now() + Duration::from_secs(3600);
        let queued = t0;
        let writing = |now: Instant| TabState { last_output: Some(now), ..idle_tab(t0) };
        let now = queued + ms(100);
        assert_eq!(gate(&writing(now), queued, now), Gate::WaitUntil(now + READY_IDLE));
        let now = queued + READY_TIMEOUT;
        assert_eq!(gate(&writing(now), queued, now), Gate::Ready, "nunca sossegou: vai mesmo assim");
    }

    #[test]
    fn a_busy_tab_waits_even_after_the_timeout() {
        let t0 = Instant::now();
        let tab = TabState { busy: true, ..idle_tab(t0) };
        let now = t0 + READY_TIMEOUT * 4;
        assert_eq!(gate(&tab, t0, now), Gate::WaitUntil(now + POLL));
    }

    #[test]
    fn user_keys_hold_the_delivery() {
        let t0 = Instant::now();
        let now = t0 + ms(10_000);
        // Tecla há 1 s, sem texto começado: espera completar USER_QUIET.
        let tab = TabState { last_key: Some(now - ms(1000)), ..idle_tab(t0) };
        assert_eq!(gate(&tab, t0, now), Gate::WaitUntil(now - ms(1000) + USER_QUIET));
        // Texto começado e não enviado: espera bem mais, e o timeout da fila não fura isso.
        let tab = TabState { last_key: Some(now - ms(1000)), typing: true, ..idle_tab(t0) };
        let late = t0 + READY_TIMEOUT * 2;
        let tab_late = TabState { last_key: Some(late - ms(1000)), ..tab };
        assert_eq!(gate(&tab_late, t0, late), Gate::WaitUntil(late - ms(1000) + TYPING_QUIET));
        assert_eq!(gate(&tab, t0, now - ms(1000) + TYPING_QUIET), Gate::Ready);
    }

    #[test]
    fn with_hooks_only_the_end_of_turn_frees_the_tab() {
        let t0 = Instant::now();
        let hooked = TabState { hooked: true, ..idle_tab(t0) };
        // Pausa longa sem saída no meio do turno (hook disse busy): continua esperando.
        let busy = TabState { busy: true, ..hooked };
        assert!(matches!(gate(&busy, t0, t0 + ms(60_000)), Gate::WaitUntil(_)));
        // Fim de turno: basta a saída assentar por HOOK_SETTLE.
        assert_eq!(gate(&hooked, t0, t0 + ms(100)), Gate::WaitUntil(t0 + HOOK_SETTLE));
        assert_eq!(gate(&hooked, t0, t0 + HOOK_SETTLE), Gate::Ready);
        // Com hooks não há "vai mesmo assim": sem saída nenhuma, espera.
        let blank = TabState { last_output: None, ..hooked };
        assert!(matches!(gate(&blank, t0, t0 + READY_TIMEOUT * 2), Gate::WaitUntil(_)));
    }

    #[test]
    fn a_tab_that_never_printed_waits_for_the_timeout() {
        let t0 = Instant::now();
        let tab = TabState { last_output: None, ..idle_tab(t0) };
        assert_eq!(gate(&tab, t0, t0 + ms(10)), Gate::WaitUntil(t0 + READY_TIMEOUT));
        assert_eq!(gate(&tab, t0, t0 + READY_TIMEOUT), Gate::Ready);
    }

    #[test]
    fn typing_state_follows_the_keys() {
        assert!(typing_after(b"a", false));
        assert!(typing_after("ç".as_bytes(), false));
        assert!(!typing_after(b"\r", true));
        assert!(!typing_after(&[0x03], true), "Ctrl+C");
        assert!(!typing_after(&[0x15], true), "Ctrl+U");
        assert!(!typing_after(&[0x1b], true), "Esc");
        assert!(typing_after(&[0x7f], true), "Backspace não sabe se apagou tudo");
        assert!(typing_after(b"\x1b[D", true), "seta");
        assert!(!typing_after(b"\x1b[D", false));
    }

    #[test]
    fn the_mailbox_keeps_order_and_arrival_time() {
        let t0 = Instant::now();
        let mut mb = Mailbox::default();
        assert!(mb.front_since().is_none());
        mb.push(t0, "a");
        mb.push(t0 + ms(5), "b");
        assert_eq!((mb.front_since(), mb.len()), (Some(t0), 2));
        assert_eq!(mb.pop(), Some("a"));
        assert_eq!(mb.front_since(), Some(t0 + ms(5)));
        assert_eq!(mb.drain().collect::<Vec<_>>(), ["b"]);
        assert!(mb.is_empty());
    }
}
