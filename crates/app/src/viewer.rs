//! Aba **visualizadora**: quando outro agente chama (`ask-agent`) um agente sem aba aberta, o app
//! abre a aba dele em segundo plano e mostra ali a conversa da ponte. Nenhum processo roda na aba
//! (o transporte continua sendo o modo headless); ela é um terminal só de leitura com uma linha de
//! prompt. Quando o usuário digita uma mensagem e dá Enter, ele "assume" a conversa: a aba vira
//! o CLI interativo do agente, retomando a mesma conversa e já enviando essa mensagem.
//!
//! Este módulo é só lógica de texto (sem janela nem PTY), para ser testável: ele produz os bytes
//! ANSI que o `Deck` escreve no emulador da aba.

use commanddeck_core::agents::MAX_INITIAL_PROMPT;
use commanddeck_core::{FeedEntry, FeedKind};
use unicode_width::UnicodeWidthChar;

/// Quantos blocos de conversa o visualizador guarda (para redesenhar ao redimensionar).
const MAX_BLOCKS: usize = 200;

/// Resultado de uma tecla/colagem no visualizador.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ViewerEvent {
    /// Nada mudou.
    None,
    /// O prompt mudou: bytes ANSI para escrever no emulador.
    Redraw(String),
    /// Enter com texto: assumir a conversa, com esta primeira mensagem.
    Takeover(String),
}

#[derive(Debug, Clone)]
pub struct Viewer {
    /// `kind` da aba (agente que ela representa).
    pub agent: String,
    /// Par `de->para` cuja conversa a ponte usa (é a que o takeover retoma).
    pub pair: String,
    /// Nome para exibição.
    name: String,
    buffer: String,
    /// Blocos já formatados (ANSI + CRLF), do mais antigo ao mais novo.
    blocks: Vec<String>,
}

impl Viewer {
    pub fn new(agent: &str, pair: &str, name: &str) -> Self {
        Self { agent: agent.to_owned(), pair: pair.to_owned(), name: name.to_owned(), buffer: String::new(), blocks: Vec::new() }
    }

    fn is_ollama(&self) -> bool {
        self.agent.starts_with("ollama")
    }

    /// Vale a pena mostrar esta entrada do feed aqui?
    pub fn wants(&self, e: &FeedEntry) -> bool {
        match e.kind {
            FeedKind::Ask => e.to.as_deref() == Some(self.agent.as_str()),
            FeedKind::Answer | FeedKind::Error => e.from.as_deref() == Some(self.agent.as_str()),
            // Chamadas de ferramenta só existem para o Ollama (os CLIs devolvem só o texto final).
            FeedKind::Thinking => self.is_ollama() && e.from.as_deref() == Some("ollama"),
            _ => false,
        }
    }

    fn hint(&self) -> &'static str {
        if self.is_ollama() {
            "[Visualizador — digite uma mensagem e dê Enter para abrir uma conversa nova com o modelo]"
        } else {
            "[Visualizador — digite uma mensagem e dê Enter para assumir esta conversa]"
        }
    }

    /// Cabeçalho fixo no topo da aba.
    pub fn header(&self) -> String {
        format!(
            "\x1b[1m{}\x1b[0m \x1b[90m— visualizador da ponte\x1b[0m\r\n\x1b[90mAs perguntas que outros agentes fizerem a este agente aparecem aqui.\x1b[0m\r\n\r\n",
            sanitize_inline(&self.name)
        )
    }

    /// Rodapé (dica) e linha de prompt. O cursor termina no fim do prompt, sem quebra de linha.
    fn tail(&self, cols: usize) -> String {
        format!("\x1b[90m{}\x1b[0m\r\n{}", fit(self.hint(), cols), self.prompt_line(cols))
    }

    /// `> ` + o fim do que foi digitado, numa única linha (com `…` à esquerda se não couber).
    fn prompt_line(&self, cols: usize) -> String {
        let budget = cols.saturating_sub(3).max(1);
        let mut shown = Vec::new();
        let mut used = 0;
        for c in self.buffer.chars().rev() {
            let w = c.width().unwrap_or(0);
            if used + w > budget {
                break;
            }
            used += w;
            shown.push(c);
        }
        let truncated = shown.len() < self.buffer.chars().count();
        if truncated {
            // Abre espaço para a reticência (largura 1).
            while used + 1 > budget {
                match shown.pop() {
                    Some(c) => used -= c.width().unwrap_or(0),
                    None => break,
                }
            }
        }
        let text: String = shown.into_iter().rev().collect();
        format!("\x1b[1;36m>\x1b[0m {}{text}", if truncated { "…" } else { "" })
    }

    /// Tela inteira: cabeçalho, blocos e rodapé (primeira vez e a cada redimensionamento).
    pub fn full_render(&self, cols: usize) -> String {
        format!("{}{}{}", self.header(), self.blocks.concat(), self.tail(cols))
    }

    /// Guarda a entrada e devolve os bytes que a anexam acima do rodapé, sem bagunçar o prompt.
    pub fn push(&mut self, e: &FeedEntry, who: &dyn Fn(&str) -> String, cols: usize) -> String {
        let block = format_entry(e, who);
        self.append_block(block, cols)
    }

    /// Aviso amarelo acima do prompt (ex.: "espere o agente terminar"). `typed` volta ao prompt
    /// para o usuário não perder o que escreveu.
    pub fn notice(&mut self, text: &str, typed: &str, cols: usize) -> String {
        self.buffer = typed.to_owned();
        let block = format!("\x1b[33m{}\x1b[0m\r\n\r\n", sanitize_inline(text));
        self.append_block(block, cols)
    }

    fn append_block(&mut self, block: String, cols: usize) -> String {
        self.blocks.push(block.clone());
        if self.blocks.len() > MAX_BLOCKS {
            self.blocks.remove(0);
        }
        // Apaga o prompt e o rodapé (cursor no fim do prompt), imprime o bloco e redesenha o rodapé.
        format!("\r\x1b[2K\x1b[1A\r\x1b[2K{block}{}", self.tail(cols))
    }

    fn redraw_prompt(&self, cols: usize) -> String {
        format!("\r\x1b[2K{}", self.prompt_line(cols))
    }

    /// Tecla (bytes como o terminal os enviaria) digitada na aba.
    pub fn input(&mut self, bytes: &[u8], cols: usize) -> ViewerEvent {
        let text = String::from_utf8_lossy(bytes);
        if text == "\x1b" {
            return self.clear(cols);
        }
        // Setas, Home/End, relatórios de mouse etc.: não editam o prompt.
        if text.starts_with('\x1b') {
            return ViewerEvent::None;
        }
        let mut changed = false;
        for ch in text.chars() {
            match ch {
                '\r' | '\n' => {
                    if self.buffer.trim().is_empty() {
                        continue; // Enter vazio não assume nada
                    }
                    return ViewerEvent::Takeover(std::mem::take(&mut self.buffer).trim().to_owned());
                }
                '\x7f' | '\x08' => changed |= self.buffer.pop().is_some(),
                '\x15' | '\x03' => {
                    changed |= !self.buffer.is_empty();
                    self.buffer.clear();
                }
                c if c.is_control() => {}
                c => {
                    if self.buffer.chars().count() < MAX_INITIAL_PROMPT {
                        self.buffer.push(c);
                        changed = true;
                    }
                }
            }
        }
        if changed {
            ViewerEvent::Redraw(self.redraw_prompt(cols))
        } else {
            ViewerEvent::None
        }
    }

    /// Colagem: quebras de linha viram espaço (o prompt é de uma linha só).
    pub fn paste(&mut self, text: &str, cols: usize) -> ViewerEvent {
        let flat: String = sanitize_inline(text);
        let room = MAX_INITIAL_PROMPT.saturating_sub(self.buffer.chars().count());
        if flat.is_empty() || room == 0 {
            return ViewerEvent::None;
        }
        self.buffer.extend(flat.chars().take(room));
        ViewerEvent::Redraw(self.redraw_prompt(cols))
    }

    fn clear(&mut self, cols: usize) -> ViewerEvent {
        if self.buffer.is_empty() {
            return ViewerEvent::None;
        }
        self.buffer.clear();
        ViewerEvent::Redraw(self.redraw_prompt(cols))
    }

    #[cfg(test)]
    fn buffer(&self) -> &str {
        &self.buffer
    }
}

/// Bloco colorido de uma entrada do feed. Todo texto de agente é saneado: nenhuma sequência de
/// escape vinda de uma resposta chega ao emulador.
pub fn format_entry(e: &FeedEntry, who: &dyn Fn(&str) -> String) -> String {
    let from = e.from.as_deref().map(who).unwrap_or_default();
    let to = e.to.as_deref().map(who).unwrap_or_default();
    let body = sanitize(&e.text);
    match e.kind {
        FeedKind::Ask => format!("\x1b[1;36m{from} → {to}\x1b[0m\r\n{body}\r\n\r\n"),
        FeedKind::Answer => format!("\x1b[1;32m{from} → {to}\x1b[0m\r\n{body}\r\n\r\n"),
        FeedKind::Error => format!("\x1b[1;31m✖ {from} → {to}\x1b[0m\r\n\x1b[31m{body}\x1b[0m\r\n\r\n"),
        _ => format!("\x1b[90m{}\x1b[0m\r\n", sanitize_inline(&e.text)),
    }
}

/// Texto de agente seguro para o emulador: sem sequências de escape/controle e com `\r\n`.
pub fn sanitize(text: &str) -> String {
    let mut out = String::with_capacity(text.len() + text.len() / 8);
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '\x1b' => match chars.peek() {
                // CSI: ESC [ ... byte final (0x40..=0x7E)
                Some('[') => {
                    chars.next();
                    for n in chars.by_ref() {
                        if ('\u{40}'..='\u{7e}').contains(&n) {
                            break;
                        }
                    }
                }
                // OSC: ESC ] ... BEL ou ESC \
                Some(']') => {
                    chars.next();
                    while let Some(n) = chars.next() {
                        if n == '\x07' {
                            break;
                        }
                        if n == '\x1b' {
                            chars.next();
                            break;
                        }
                    }
                }
                // Qualquer outra sequência de dois caracteres.
                Some(_) => {
                    chars.next();
                }
                None => {}
            },
            '\r' => {
                if chars.peek() == Some(&'\n') {
                    chars.next();
                }
                out.push_str("\r\n");
            }
            '\n' => out.push_str("\r\n"),
            '\t' => out.push_str("    "),
            c if c.is_control() => {}
            c => out.push(c),
        }
    }
    out
}

/// Como [`sanitize`], mas numa linha só (quebras viram espaço).
pub fn sanitize_inline(text: &str) -> String {
    sanitize(text).replace("\r\n", " ").trim().to_owned()
}

/// Corta `text` para caber em `cols` colunas, com `…` se precisar.
fn fit(text: &str, cols: usize) -> String {
    let budget = cols.saturating_sub(1).max(1);
    let mut out = String::new();
    let mut used = 0;
    for c in text.chars() {
        let w = c.width().unwrap_or(0);
        if used + w > budget {
            out.pop();
            out.push('…');
            return out;
        }
        used += w;
        out.push(c);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn who(id: &str) -> String {
        id.to_owned()
    }

    fn entry(kind: FeedKind, from: &str, to: &str, text: &str) -> FeedEntry {
        FeedEntry::new(kind, Some(from), Some(to), text)
    }

    fn viewer() -> Viewer {
        Viewer::new("antigravity", "claude->antigravity", "Antigravity")
    }

    #[test]
    fn sanitize_strips_escapes_and_normalizes_newlines() {
        assert_eq!(sanitize("a\nb\r\nc\rd"), "a\r\nb\r\nc\r\nd");
        assert_eq!(sanitize("\x1b[31mvermelho\x1b[0m"), "vermelho");
        assert_eq!(sanitize("x\x1b]0;título\x07y"), "xy");
        assert_eq!(sanitize("x\x1b]52;c;ZGF0YQ==\x1b\\y"), "xy");
        assert_eq!(sanitize("a\x1bcb\x07c\x7fd"), "abcd");
        assert_eq!(sanitize("tab\taqui"), "tab    aqui");
        assert_eq!(sanitize("acentuação ✓ 日本"), "acentuação ✓ 日本");
        assert_eq!(sanitize_inline("um\ndois\r\ntrês  "), "um dois três");
        // Escape pendente no fim não derruba nada.
        assert_eq!(sanitize("fim\x1b"), "fim");
    }

    #[test]
    fn only_relevant_entries_are_shown() {
        let v = viewer();
        assert!(v.wants(&entry(FeedKind::Ask, "claude", "antigravity", "oi")));
        assert!(!v.wants(&entry(FeedKind::Ask, "antigravity", "claude", "oi")));
        assert!(v.wants(&entry(FeedKind::Answer, "antigravity", "claude", "ok")));
        assert!(!v.wants(&entry(FeedKind::Answer, "claude", "antigravity", "ok")));
        assert!(v.wants(&entry(FeedKind::Error, "antigravity", "claude", "falhou")));
        assert!(!v.wants(&entry(FeedKind::Thinking, "ollama", "", "⚙️")));
        assert!(!v.wants(&FeedEntry::system("x")));

        // Thinking só interessa ao Ollama.
        let o = Viewer::new("ollama:qwen3.5:9b", "claude->ollama:qwen3.5:9b", "Qwen");
        assert!(o.wants(&FeedEntry::new(FeedKind::Thinking, Some("ollama"), None, "⚙️ [read_file] a")));
        assert!(!o.wants(&FeedEntry::new(FeedKind::Thinking, Some("claude"), None, "x")));
    }

    #[test]
    fn blocks_never_carry_agent_escape_sequences() {
        let e = entry(FeedKind::Answer, "antigravity", "claude", "ok\x1b[2J\x1b]0;pwn\x07 fim");
        let block = format_entry(&e, &who);
        assert!(block.contains("antigravity → claude"));
        assert!(block.contains("ok fim"));
        // Só as sequências do próprio visualizador (cores `ESC[..m`).
        let escapes: Vec<&str> = block.split('\x1b').skip(1).collect();
        assert!(escapes.iter().all(|s| s.starts_with('[') && s.split('m').next().is_some_and(|p| p[1..].chars().all(|c| c.is_ascii_digit() || c == ';'))));
    }

    #[test]
    fn pushing_a_block_redraws_the_tail_without_losing_what_was_typed() {
        let mut v = viewer();
        v.input("oi".as_bytes(), 80);
        let out = v.push(&entry(FeedKind::Ask, "claude", "antigravity", "pergunta"), &who, 80);
        // Apaga prompt e rodapé antes de imprimir o bloco…
        assert!(out.starts_with("\r\x1b[2K\x1b[1A\r\x1b[2K"));
        assert!(out.contains("claude → antigravity\x1b[0m\r\npergunta\r\n\r\n"));
        // …e redesenha rodapé + prompt, com o texto que já estava digitado.
        assert!(out.ends_with("oi"));
        assert!(out.contains("Visualizador"));
    }

    #[test]
    fn full_render_has_header_blocks_and_tail() {
        let mut v = viewer();
        v.push(&entry(FeedKind::Answer, "antigravity", "claude", "resposta"), &who, 100);
        let full = v.full_render(100);
        let (h, b, t) = (full.find("visualizador da ponte"), full.find("resposta"), full.find("assumir esta conversa"));
        assert!(h.is_some() && b.is_some() && t.is_some());
        assert!(h < b && b < t);
    }

    #[test]
    fn typing_editing_and_takeover() {
        let mut v = viewer();
        // Enter vazio não assume.
        assert_eq!(v.input(b"\r", 80), ViewerEvent::None);
        assert_eq!(v.input(b"   \r", 80), ViewerEvent::Redraw("\r\x1b[2K\x1b[1;36m>\x1b[0m    ".to_owned()));
        assert_eq!(v.input(b"\r", 80), ViewerEvent::None, "só espaços também não assume");

        v.input("olá, çã".as_bytes(), 80);
        assert_eq!(v.buffer(), "   olá, çã");
        // Backspace remove um caractere Unicode, não um byte.
        v.input(b"\x7f", 80);
        assert_eq!(v.buffer(), "   olá, ç");
        // Ctrl+U e Ctrl+C limpam; Esc também.
        v.input(b"\x15", 80);
        assert_eq!(v.buffer(), "");
        v.input(b"abc", 80);
        v.input(b"\x03", 80);
        assert_eq!(v.buffer(), "");
        v.input(b"abc", 80);
        v.input(b"\x1b", 80);
        assert_eq!(v.buffer(), "");

        // Setas, Home e relatórios de mouse são ignorados.
        v.input(b"ok", 80);
        for seq in [&b"\x1b[A"[..], b"\x1b[1;5D", b"\x1bOH", b"\x1b[<0;5;3M"] {
            assert_eq!(v.input(seq, 80), ViewerEvent::None);
        }
        assert_eq!(v.buffer(), "ok");

        // Enter com texto assume, entregando a mensagem sem espaços nas pontas, e esvazia o buffer.
        v.input(b"  mais um", 80);
        assert_eq!(v.input(b"\r", 80), ViewerEvent::Takeover("ok  mais um".to_owned()));
        assert_eq!(v.buffer(), "");
    }

    #[test]
    fn paste_flattens_lines_and_respects_the_limit() {
        let mut v = viewer();
        assert!(matches!(v.paste("linha um\nlinha dois\r\n\x1b[31mtres", 80), ViewerEvent::Redraw(_)));
        assert_eq!(v.buffer(), "linha um linha dois tres");
        assert_eq!(v.paste("", 80), ViewerEvent::None);

        let mut v = viewer();
        v.paste(&"x".repeat(MAX_INITIAL_PROMPT + 500), 80);
        assert_eq!(v.buffer().chars().count(), MAX_INITIAL_PROMPT);
        // Cheio: digitar mais não muda nada.
        assert_eq!(v.input(b"y", 80), ViewerEvent::None);
        assert_eq!(v.paste("y", 80), ViewerEvent::None);
    }

    #[test]
    fn the_prompt_is_always_one_line_that_fits() {
        let mut v = viewer();
        let width = |s: &str| {
            // Largura visível: tira as sequências de cor.
            let plain = sanitize(s);
            plain.chars().map(|c| c.width().unwrap_or(0)).sum::<usize>()
        };
        for cols in [10usize, 24, 80] {
            v.buffer = "início ".to_owned() + &"palavra ".repeat(40) + "FIM";
            let line = v.prompt_line(cols);
            assert!(width(&line) < cols, "{cols}: {line:?}");
            assert!(line.contains("…"), "trunca à esquerda");
            assert!(line.ends_with("FIM"), "mostra o fim do que foi digitado");
        }
        // Caracteres largos (2 colunas) também respeitam o limite.
        v.buffer = "日本語のテキストをたくさん入力します".to_owned();
        let line = v.prompt_line(20);
        assert!(width(&line) < 20, "{line:?}");
        // Cabe inteiro: sem reticência.
        v.buffer = "curto".to_owned();
        assert!(!v.prompt_line(80).contains('…'));
        // Largura mínima não entra em pânico.
        let _ = v.prompt_line(0);
        let _ = v.prompt_line(1);
        let _ = v.tail(1);
    }

    #[test]
    fn old_blocks_are_dropped_past_the_limit() {
        let mut v = viewer();
        for i in 0..(MAX_BLOCKS + 25) {
            v.push(&entry(FeedKind::Ask, "claude", "antigravity", &format!("msg {i}")), &who, 80);
        }
        let full = v.full_render(80);
        assert!(!full.contains("msg 0\r\n"));
        assert!(full.contains(&format!("msg {}\r\n", MAX_BLOCKS + 24)));
    }

    #[test]
    fn ollama_viewer_warns_that_takeover_starts_fresh() {
        let o = Viewer::new("ollama:qwen3.5:9b", "claude->ollama:qwen3.5:9b", "Qwen");
        assert!(o.hint().contains("conversa nova"));
        assert!(viewer().hint().contains("assumir"));
    }
}
