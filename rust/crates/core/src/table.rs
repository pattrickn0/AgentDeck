//! Mesa de orquestração: vários agentes em rodadas (equivalente a `table:run`/`table:stop`).

use std::sync::atomic::Ordering;

use crate::events::{FeedEntry, FeedKind};
use crate::headless::HeadlessOptions;
use crate::{Core, CoreEvent, TableRequest};

/// Instrução de sistema extra enviada aos participantes.
pub const TABLE_SYSTEM: &str = "Você está numa mesa de colaboração entre agentes de IA no CommandDeck.";

struct Turn {
    agent: String,
    round: u32,
    text: String,
}

/// Monta o prompt de um turno (mesmo texto do JS).
pub(crate) fn turn_prompt(agents: &[String], agent: &str, task: &str, refs: &str, history: &str, round: u32, rounds: u32) -> String {
    let others: Vec<&str> = agents.iter().filter(|a| a.as_str() != agent).map(String::as_str).collect();
    let files = if refs.is_empty() { String::new() } else { format!("\nArquivos: {refs}") };
    let is_last = round == rounds && agents.last().is_some_and(|a| a == agent);
    [
        format!("Você participa de uma mesa de colaboração com: {}. O usuário acompanha a conversa.", others.join(", ")),
        format!("Tarefa do usuário:\n{task}{files}"),
        if history.is_empty() { "Você é o primeiro a falar.".to_owned() } else { format!("Conversa até agora:\n\n{history}") },
        format!(
            "Agora é sua vez (rodada {round} de {rounds}). Contribua, critique e construa sobre o que os outros disseram. Seja objetivo.{}",
            if is_last { " Como último a falar, feche com uma síntese final da mesa." } else { "" }
        ),
    ]
    .join("\n\n")
}

pub(crate) fn run_table(core: &Core, req: TableRequest) {
    let abort = &core.inner.state.table_abort;
    abort.store(false, Ordering::SeqCst);
    let mut transcript: Vec<Turn> = Vec::new();
    let refs: String = req.attachments.iter().map(|a| format!("@{a}")).collect::<Vec<_>>().join(" ");
    let user_text = if refs.is_empty() { req.task.clone() } else { format!("{}\n{refs}", req.task) };
    core.feed(FeedEntry::new(FeedKind::User, Some("você"), Some(&req.agents.join(" + ")), user_text));

    let mut r = 1;
    while r <= req.rounds && !abort.load(Ordering::SeqCst) {
        for agent in &req.agents {
            if abort.load(Ordering::SeqCst) {
                break;
            }
            let history: Vec<String> = transcript.iter().map(|t| format!("### {} (rodada {})\n{}", t.agent, t.round, t.text)).collect();
            let prompt = turn_prompt(&req.agents, agent, &req.task, &refs, &history.join("\n\n"), r, req.rounds);
            core.feed(FeedEntry::new(FeedKind::Thinking, Some(agent), None, format!("rodada {r}/{}…", req.rounds)));
            let opts =
                HeadlessOptions { extra_system: Some(TABLE_SYSTEM.into()), attachments: req.attachments.clone(), ..Default::default() };
            match core.run_headless_with(agent, &prompt, opts, abort) {
                Ok(text) => {
                    transcript.push(Turn { agent: agent.clone(), round: r, text: text.clone() });
                    core.feed(FeedEntry::new(FeedKind::Table, Some(agent), None, text));
                }
                Err(e) => core.feed(FeedEntry::new(FeedKind::Error, Some(agent), None, e.to_string())),
            }
        }
        r += 1;
    }
    let end = if abort.load(Ordering::SeqCst) { "Mesa interrompida." } else { "Mesa encerrada." };
    core.feed(FeedEntry::system(end));
    core.emit(CoreEvent::TableFinished);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn prompt_structure() {
        let agents: Vec<String> = vec!["claude".into(), "ollama:qwen3.5:9b".into()];
        let p = turn_prompt(&agents, "claude", "Faça X", "", "", 1, 2);
        assert_eq!(
            p,
            "Você participa de uma mesa de colaboração com: ollama:qwen3.5:9b. O usuário acompanha a conversa.\n\nTarefa do usuário:\nFaça X\n\nVocê é o primeiro a falar.\n\nAgora é sua vez (rodada 1 de 2). Contribua, critique e construa sobre o que os outros disseram. Seja objetivo."
        );
        let p = turn_prompt(&agents, "ollama:qwen3.5:9b", "Faça X", "@a.png @b.txt", "### claude (rodada 1)\nok", 2, 2);
        assert!(p.contains("Tarefa do usuário:\nFaça X\nArquivos: @a.png @b.txt"));
        assert!(p.contains("Conversa até agora:\n\n### claude (rodada 1)\nok"));
        assert!(p.ends_with("Seja objetivo. Como último a falar, feche com uma síntese final da mesa."));
    }
}
