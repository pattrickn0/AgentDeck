//! Mapa de conversas entre agentes em texto (estilo terminal), sem egui.
//!
//! A interface monta um [`Snapshot`] (o que o core sabe das perguntas pendentes + o que só a
//! interface sabe: filas das abas, quem está ocupado, conversas recentes) e [`render_map`]
//! devolve as linhas já cortadas na largura, cada pedaço com um [`Style`] para colorir. A pintura
//! fica em `ui/map.rs`.
//!
//! Só caracteres que existem nas fontes monoespaçadas comuns do Windows (WGL4/Latin-1): símbolos
//! como ▶ ✖ ✓ caem para outra fonte, proporcional, e desalinham a grade.

use std::collections::HashSet;

/// Por quanto tempo uma conversa concluída aparece (esmaecida) em "Recentes".
pub const RECENT_MS: u64 = 60_000;

/// Estado de um agente (aba).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NodeState {
    /// No meio de um turno.
    Busy,
    Idle,
    /// O processo da aba encerrou.
    Ended,
    /// Não há aba (ex.: pergunta headless).
    Unknown,
}

/// Em que pé está uma pergunta entre dois agentes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LinkState {
    /// Esperando a aba de destino ficar livre para ser digitada (posição na fila, 1 = a próxima).
    Queued(usize),
    /// Já digitada; quem perguntou espera a resposta.
    Typed,
    /// O `ask-agent` de quem perguntou voltou (202): a resposta será digitada depois.
    Detached,
    /// Respondida em modo headless (sem aba).
    Headless,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Link {
    /// Número da pergunta (`None` no modo headless).
    pub id: Option<u64>,
    pub from: String,
    pub to: String,
    pub since_ms: u64,
    pub state: LinkState,
}

/// Resposta atrasada esperando para ser digitada na aba de quem perguntou.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AnswerInFlight {
    pub request: u64,
    /// Quem respondeu.
    pub from: String,
    /// Quem perguntou (vai receber).
    pub to: String,
}

/// Conversa concluída há pouco.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Recent {
    /// Quem respondeu (ou falhou).
    pub from: String,
    /// Quem tinha perguntado.
    pub to: String,
    pub ok: bool,
    pub at_ms: u64,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Snapshot {
    /// Agentes com aba aberta, na ordem das abas.
    pub nodes: Vec<(String, NodeState)>,
    /// Da pergunta mais antiga para a mais nova.
    pub links: Vec<Link>,
    pub answers: Vec<AnswerInFlight>,
    pub recent: Vec<Recent>,
}

impl Snapshot {
    /// Há algo acontecendo que muda com o tempo (o mapa precisa ser redesenhado a cada segundo)?
    pub fn is_live(&self, now_ms: u64) -> bool {
        !self.links.is_empty() || !self.answers.is_empty() || self.recent.iter().any(|r| now_ms.saturating_sub(r.at_ms) < RECENT_MS)
    }

    fn node_state(&self, id: &str) -> NodeState {
        self.nodes.iter().find(|(n, _)| n == id).map_or(NodeState::Unknown, |(_, s)| *s)
    }
}

/// Como pintar um pedaço da linha.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Style {
    /// Título de seção.
    Heading,
    /// Nome de agente.
    Name,
    /// Traços da árvore e detalhes.
    Muted,
    Busy,
    Idle,
    Ended,
    /// Número da pergunta.
    Id,
    Error,
    /// Conversa recente (esmaecida).
    Faded,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Line {
    pub spans: Vec<(String, Style)>,
    /// Agente da linha: clicar nela ativa a aba dele.
    pub node: Option<String>,
}

impl Line {
    fn push(&mut self, text: impl Into<String>, style: Style) -> &mut Self {
        self.spans.push((text.into(), style));
        self
    }

    /// Largura em colunas (um caractere = uma coluna).
    pub fn width(&self) -> usize {
        self.spans.iter().map(|(t, _)| t.chars().count()).sum()
    }

    #[cfg(test)]
    pub fn text(&self) -> String {
        self.spans.iter().map(|(t, _)| t.as_str()).collect()
    }

    /// Corta a linha em `cols` colunas (o último caractere vira "…").
    fn clip(mut self, cols: usize) -> Self {
        if self.width() <= cols {
            return self;
        }
        let mut left = cols.saturating_sub(1);
        let mut out = Vec::new();
        for (text, style) in self.spans.drain(..) {
            if left == 0 {
                break;
            }
            let n = text.chars().count();
            if n <= left {
                left -= n;
                out.push((text, style));
            } else {
                out.push((text.chars().take(left).collect(), style));
                left = 0;
            }
        }
        out.push(("…".into(), Style::Muted));
        self.spans = out;
        self
    }
}

/// Corta um nome no meio para caber em `max` colunas: `ollama:qwen3.5:9b` → `ollama:q…5:9b`.
pub fn fit_name(name: &str, max: usize) -> String {
    let n = name.chars().count();
    if n <= max {
        return name.to_owned();
    }
    if max <= 1 {
        return "…".chars().take(max).collect();
    }
    let keep = max - 1;
    let head = keep.div_ceil(2);
    let tail = keep - head;
    let chars: Vec<char> = name.chars().collect();
    let mut out: String = chars[..head].iter().collect();
    out.push('…');
    out.extend(&chars[n - tail..]);
    out
}

/// `m:ss` (ou `h:mm:ss`).
pub fn elapsed(ms: u64) -> String {
    let s = ms / 1000;
    if s >= 3600 {
        format!("{}:{:02}:{:02}", s / 3600, (s / 60) % 60, s % 60)
    } else {
        format!("{}:{:02}", s / 60, s % 60)
    }
}

fn marker(state: NodeState) -> (&'static str, Style) {
    match state {
        NodeState::Busy => ("●", Style::Busy),
        NodeState::Idle => ("○", Style::Idle),
        NodeState::Ended => ("×", Style::Ended),
        NodeState::Unknown => ("○", Style::Muted),
    }
}

fn link_detail(link: &Link, now_ms: u64) -> String {
    let t = elapsed(now_ms.saturating_sub(link.since_ms));
    match link.state {
        LinkState::Queued(n) => format!("na fila ({n}) · {t}"),
        LinkState::Typed => format!("digitada · espera {t}"),
        LinkState::Detached => format!("202 · responde depois · {t}"),
        LinkState::Headless => format!("headless · {t}"),
    }
}

fn heading(text: &str) -> Line {
    let mut l = Line::default();
    l.push(text, Style::Heading);
    l
}

/// Linha de um agente na árvore: `prefixo` + `seta` + marcador + nome [+ `#id` alinhado à direita].
fn node_line(snap: &Snapshot, prefix: &str, arrow: &str, name: &str, id: Option<u64>, repeated: bool, cols: usize) -> Line {
    let mut l = Line { node: Some(name.to_owned()), ..Default::default() };
    let (mark, style) = marker(snap.node_state(name));
    let lead = prefix.chars().count() + arrow.chars().count() + 2;
    let id_text = id.map(|n| format!("#{n}")).unwrap_or_default();
    let suffix = if repeated { " ↑" } else { "" };
    let room_with_id = cols.saturating_sub(lead + suffix.len() + id_text.chars().count() + 1);
    let show_id = !id_text.is_empty() && room_with_id >= 6;
    let room = if show_id { room_with_id } else { cols.saturating_sub(lead + suffix.chars().count()) };
    let shown = fit_name(name, room.max(1));
    l.push(prefix, Style::Muted).push(arrow, Style::Muted).push(mark, style).push(" ", Style::Muted).push(shown, Style::Name);
    if repeated {
        l.push(suffix, Style::Muted);
    }
    if show_id {
        let pad = cols.saturating_sub(l.width() + id_text.chars().count());
        l.push(" ".repeat(pad.max(1)), Style::Muted).push(id_text, Style::Id);
    }
    l
}

/// Desenha a árvore de quem pergunta a quem a partir de `root`.
fn tree(snap: &Snapshot, root: &str, cols: usize, now_ms: u64, seen: &mut HashSet<String>, out: &mut Vec<Line>) {
    seen.insert(root.to_owned());
    out.push(node_line(snap, "", "", root, None, false, cols));
    children(snap, root, "", cols, now_ms, seen, out);
}

fn children(snap: &Snapshot, parent: &str, prefix: &str, cols: usize, now_ms: u64, seen: &mut HashSet<String>, out: &mut Vec<Line>) {
    let kids: Vec<&Link> = snap.links.iter().filter(|l| l.from == parent).collect();
    for (i, link) in kids.iter().enumerate() {
        let last = i + 1 == kids.len();
        let repeated = seen.contains(&link.to);
        out.push(node_line(snap, prefix, if last { "└─► " } else { "├─► " }, &link.to, link.id, repeated, cols));
        let inner = format!("{prefix}{}", if last { "    " } else { "│   " });
        let mut detail = Line::default();
        detail.push(format!("{inner}  "), Style::Muted).push(link_detail(link, now_ms), Style::Muted);
        out.push(detail);
        if !repeated {
            seen.insert(link.to.clone());
            children(snap, &link.to, &inner, cols, now_ms, seen, out);
        }
    }
}

/// O mapa inteiro, em linhas de no máximo `cols` colunas.
pub fn render_map(snap: &Snapshot, cols: usize, now_ms: u64) -> Vec<Line> {
    let cols = cols.max(20);
    let mut out = Vec::new();
    let recent: Vec<&Recent> = {
        let mut r: Vec<&Recent> = snap.recent.iter().filter(|r| now_ms.saturating_sub(r.at_ms) < RECENT_MS).collect();
        r.sort_by_key(|r| std::cmp::Reverse(r.at_ms));
        r
    };

    if !snap.links.is_empty() {
        let n = snap.links.len();
        out.push(heading(&format!("AO VIVO · {n} pergunta{}", if n == 1 { "" } else { "s" })));
        // Raízes: quem pergunta sem estar respondendo a ninguém (na ordem da pergunta mais antiga).
        let mut seen = HashSet::new();
        let mut roots: Vec<&str> = Vec::new();
        for l in &snap.links {
            if !snap.links.iter().any(|o| o.to == l.from) && !roots.contains(&l.from.as_str()) {
                roots.push(&l.from);
            }
        }
        for root in roots {
            tree(snap, root, cols, now_ms, &mut seen, &mut out);
        }
        // Por segurança (o core recusa ciclos): perguntas que sobraram fora de qualquer árvore.
        let left: Vec<String> = snap.links.iter().filter(|l| !seen.contains(&l.from)).map(|l| l.from.clone()).collect();
        for from in left {
            if !seen.contains(&from) {
                tree(snap, &from, cols, now_ms, &mut seen, &mut out);
            }
        }
    }

    if !snap.answers.is_empty() {
        out.push(heading("RESPOSTAS A CAMINHO"));
        for a in &snap.answers {
            let mut l = Line { node: Some(a.to.clone()), ..Default::default() };
            let half = cols.saturating_sub(16) / 2;
            l.push("  ", Style::Muted)
                .push(fit_name(&a.from, half.max(4)), Style::Name)
                .push(" ─► ", Style::Muted)
                .push(fit_name(&a.to, half.max(4)), Style::Name)
                .push(format!("  #{}", a.request), Style::Id);
            out.push(l);
        }
    }

    if !recent.is_empty() {
        out.push(heading("RECENTES"));
        for r in &recent {
            let mut l = Line { node: Some(r.from.clone()), ..Default::default() };
            let age = elapsed(now_ms.saturating_sub(r.at_ms));
            let (status, style) = if r.ok { ("ok", Style::Faded) } else { ("×", Style::Error) };
            let half = cols.saturating_sub(18) / 2;
            l.push("  ", Style::Faded)
                .push(fit_name(&r.from, half.max(4)), Style::Faded)
                .push(" ─► ", Style::Faded)
                .push(fit_name(&r.to, half.max(4)), Style::Faded)
                .push("  ", Style::Faded)
                .push(status, style)
                .push(format!("  {age}"), Style::Faded);
            out.push(l);
        }
    }

    if out.is_empty() {
        out.push(heading("Nenhuma conversa entre agentes agora."));
    }

    // Agentes abertos que não aparecem em nada acima.
    let busy_ids: HashSet<&str> = snap
        .links
        .iter()
        .flat_map(|l| [l.from.as_str(), l.to.as_str()])
        .chain(snap.answers.iter().flat_map(|a| [a.from.as_str(), a.to.as_str()]))
        .collect();
    let idle: Vec<&(String, NodeState)> = snap.nodes.iter().filter(|(n, _)| !busy_ids.contains(n.as_str())).collect();
    if !idle.is_empty() {
        out.push(Line::default());
        out.push(heading("SEM CONVERSA"));
        let mut line = Line::default();
        for (name, state) in idle {
            let (mark, style) = marker(*state);
            let name = fit_name(name, cols.saturating_sub(4));
            let w = 2 + 2 + name.chars().count();
            if line.width() + w > cols && line.width() > 0 {
                out.push(std::mem::take(&mut line));
            }
            line.push("  ", Style::Muted).push(mark, style).push(" ", Style::Muted).push(name, Style::Name);
        }
        out.push(line);
    }

    out.into_iter().map(|l| l.clip(cols)).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    const NOW: u64 = 10_000_000;

    fn link(id: u64, from: &str, to: &str, ago: u64, state: LinkState) -> Link {
        Link { id: Some(id), from: from.into(), to: to.into(), since_ms: NOW - ago, state }
    }

    fn texts(lines: &[Line]) -> Vec<String> {
        lines.iter().map(Line::text).collect()
    }

    fn sample() -> Snapshot {
        Snapshot {
            nodes: vec![
                ("claude".into(), NodeState::Busy),
                ("claude@dev".into(), NodeState::Busy),
                ("antigravity".into(), NodeState::Idle),
                ("codex".into(), NodeState::Idle),
                ("opencode".into(), NodeState::Idle),
                ("shell".into(), NodeState::Ended),
            ],
            links: vec![
                link(12, "claude", "claude@dev", 42_000, LinkState::Typed),
                link(13, "claude@dev", "antigravity", 5_000, LinkState::Queued(1)),
                link(14, "claude", "codex", 70_000, LinkState::Detached),
            ],
            answers: vec![AnswerInFlight { request: 9, from: "opencode".into(), to: "claude".into() }],
            recent: vec![
                Recent { from: "claude@dev".into(), to: "claude".into(), ok: true, at_ms: NOW - 20_000 },
                Recent { from: "codex".into(), to: "claude".into(), ok: false, at_ms: NOW - 90_000 },
            ],
        }
    }

    #[test]
    fn an_empty_map_says_so() {
        let lines = render_map(&Snapshot::default(), 40, NOW);
        assert_eq!(texts(&lines), ["Nenhuma conversa entre agentes agora."]);
        let snap = Snapshot { nodes: vec![("claude".into(), NodeState::Idle)], ..Default::default() };
        let t = texts(&render_map(&snap, 40, NOW));
        assert_eq!(t, ["Nenhuma conversa entre agentes agora.", "", "SEM CONVERSA", "  ○ claude"]);
    }

    #[test]
    fn the_call_tree_shows_who_asks_whom() {
        let t = texts(&render_map(&sample(), 44, NOW));
        let expected = [
            "AO VIVO · 3 perguntas",
            "● claude",
            "├─► ● claude@dev                         #12",
            "│     digitada · espera 0:42",
            "│   └─► ○ antigravity                    #13",
            "│         na fila (1) · 0:05",
            "└─► ○ codex                              #14",
            "      202 · responde depois · 1:10",
            "RESPOSTAS A CAMINHO",
            "  opencode ─► claude  #9",
            "RECENTES",
            "  claude@dev ─► claude  ok  0:20",
            "",
            "SEM CONVERSA",
            "  × shell",
        ];
        assert_eq!(t, expected, "\n{}", t.join("\n"));
    }

    #[test]
    fn two_askers_to_the_same_agent_mark_the_repeat() {
        let snap = Snapshot {
            links: vec![link(1, "a", "c", 1000, LinkState::Typed), link(2, "b", "c", 1000, LinkState::Typed)],
            ..Default::default()
        };
        let t = texts(&render_map(&snap, 40, NOW));
        assert!(t.iter().any(|l| l.contains("○ c ↑")), "{t:?}");
        assert_eq!(t.iter().filter(|l| l.contains("○ c")).count(), 2);
    }

    #[test]
    fn a_cycle_cannot_loop_forever() {
        // O core recusa ciclos; mesmo assim o desenho termina.
        let snap =
            Snapshot { links: vec![link(1, "a", "b", 0, LinkState::Typed), link(2, "b", "a", 0, LinkState::Typed)], ..Default::default() };
        let t = texts(&render_map(&snap, 40, NOW));
        assert!(t.len() < 10, "{t:?}");
    }

    #[test]
    fn no_line_is_wider_than_the_panel_and_only_safe_symbols_are_used() {
        let mut snap = sample();
        snap.links.push(link(99, "ollama:user/um-modelo-com-nome-enorme:latest", "claude@revisao@ana", 3_700_000, LinkState::Headless));
        for cols in 20..=60 {
            for line in render_map(&snap, cols, NOW) {
                assert!(line.width() <= cols, "{cols} colunas: {:?} ({})", line.text(), line.width());
                for c in line.text().chars() {
                    let ok = c.is_ascii() || ('\u{a0}'..='\u{ff}').contains(&c) || "─│├└═╦╠╚►◄●○×…↑".contains(c);
                    assert!(ok, "caractere fora da lista: {c:?} em {:?}", line.text());
                }
            }
        }
    }

    #[test]
    fn recents_fade_out_after_a_minute() {
        let mut snap = Snapshot::default();
        snap.recent.push(Recent { from: "a".into(), to: "b".into(), ok: true, at_ms: NOW - RECENT_MS + 1 });
        assert!(texts(&render_map(&snap, 40, NOW)).contains(&"RECENTES".to_owned()));
        assert!(snap.is_live(NOW));
        assert!(!texts(&render_map(&snap, 40, NOW + 2)).contains(&"RECENTES".to_owned()));
        assert!(!snap.is_live(NOW + 2));
    }

    #[test]
    fn lines_know_their_agent_for_clicks() {
        let lines = render_map(&sample(), 44, NOW);
        let line = lines.iter().find(|l| l.text().contains("antigravity")).expect("linha");
        assert_eq!(line.node.as_deref(), Some("antigravity"));
    }

    #[test]
    fn names_are_cut_in_the_middle() {
        assert_eq!(fit_name("ollama:qwen3.5:9b", 9), "olla…5:9b");
        assert_eq!(fit_name("claude", 9), "claude");
        assert_eq!(elapsed(3_725_000), "1:02:05");
        assert_eq!(elapsed(65_000), "1:05");
    }
}
