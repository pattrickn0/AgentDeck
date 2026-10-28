//! pt-BR translations for the area: Subscription usage: crates/core/src/usage/** and the title bar usage widget (crates/app/src/ui/usage.rs).
//!
//! Entries are `(English, Portuguese)`; the English text is the key used in `t!("...")`. See the `i18n` module docs for
//! the conventions and the coverage test.

pub const ENTRIES: &[(&str, &str)] = &[
    ("5 h", "5 h"),
    ("7 days", "7 dias"),
    ("Fri", "sex"),
    ("Mon", "seg"),
    ("Sat", "sáb"),
    ("SUBSCRIPTION USAGE", "USO DA ASSINATURA"),
    ("Session", "Sessão"),
    ("Show this group in the title bar", "Mostrar este grupo na barra de título"),
    ("Sun", "dom"),
    ("Thu", "qui"),
    (
        "The title bar shows this group. Click to return to the summary.",
        "A barra de título mostra este grupo. Clique para voltar ao resumo.",
    ),
    ("Tue", "ter"),
    ("Wed", "qua"),
    ("Week", "Semana"),
    ("blocked", "bloqueado"),
    ("blocked until {clock}", "bloqueado até {clock}"),
    ("not reported by source", "a fonte não informa"),
    ("on bar", "na barra"),
    ("pin to bar", "fixar na barra"),
    ("read just now", "lido agora"),
    ("read {days} days ago", "lido há {days} dias"),
    ("read {hours} h ago", "lido há {hours} h"),
    ("read {mins} min ago", "lido há {mins} min"),
    ("renewed · awaiting new reading", "renovou · aguardando leitura nova"),
    ("renews in {duration}", "renova em {duration}"),
    ("renews in {duration} · at {clock}", "renova em {duration} · às {clock}"),
    ("renews {weekday_time}", "renova {weekday_time}"),
    ("unpin from bar", "soltar da barra"),
    (
        "{kind} is at {current_pct}% quota; {alt_kind} is at {alt_pct}%",
        "{kind} está com {current_pct}% da cota usada; {alt_kind} está com {alt_pct}%",
    ),
];
