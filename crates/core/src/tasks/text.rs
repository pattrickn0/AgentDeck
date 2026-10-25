use std::path::Path;

use unicode_normalization::UnicodeNormalization;

use super::{Task, TaskError, HISTORY_ITEM_CHARS, HISTORY_SHOWN, MAX_SLUG, MAX_TITLE};
use crate::util::{self, terminal_text};

/// `Tarefa  nova!` → `tarefa-nova`: ASCII minúsculo, sem acentos, no máximo `max` caracteres.
pub fn slugify(title: &str, max: usize) -> String {
    let mut out = String::new();
    for c in title.nfd() {
        if c.is_ascii_alphanumeric() {
            out.push(c.to_ascii_lowercase());
        } else if c.is_ascii() && !out.ends_with('-') {
            out.push('-');
        }
        // Não ASCII (acentos soltos pela decomposição, ideogramas…): some sem separar a palavra.
    }
    let cut: String = out.trim_matches('-').chars().take(max).collect();
    cut.trim_end_matches('-').to_owned()
}

/// `tarefa/<id>-<slug>` (sem slug, `tarefa/<id>`).
pub fn branch_name(id: &str, title: &str) -> String {
    match slugify(title, MAX_SLUG) {
        s if s.is_empty() => format!("tarefa/{id}"),
        s => format!("tarefa/{id}-{s}"),
    }
}

/// `T3`, `t3` e `3` → `t3`. `None` se não for um id de tarefa.
pub fn parse_task_id(text: &str) -> Option<String> {
    let t = text.trim().to_ascii_lowercase();
    let digits = t.strip_prefix('t').unwrap_or(&t);
    if digits.is_empty() || digits.len() > 9 || !digits.chars().all(|c| c.is_ascii_digit()) {
        return None;
    }
    digits.parse::<u64>().ok().map(|n| format!("t{n}"))
}

/// Título numa linha só, sem espaços sobrando e com no máximo [`MAX_TITLE`] caracteres.
pub fn clean_title(title: &str) -> Result<String, TaskError> {
    let one_line = title.split_whitespace().collect::<Vec<_>>().join(" ");
    let one_line: String = one_line.chars().filter(|c| !c.is_control()).collect();
    if one_line.is_empty() {
        return Err(TaskError::Invalid("o título da tarefa não pode ficar vazio".into()));
    }
    Ok(one_line.chars().take(MAX_TITLE).collect())
}

pub(super) fn cap_text(text: &str, max: usize) -> String {
    let (head, cut) = util::truncate_chars(text, max);
    if cut {
        format!("{head}\n\n[texto cortado em {max} caracteres]")
    } else {
        head.to_owned()
    }
}

/// O bloco de memória do início da tarefa: o que o projeto já aprendeu (longo prazo) e a situação atual
/// (curto prazo). Cada parte some quando está vazia; vazio quando as duas estão.
pub fn memory_block(learnings: &str, situation: &str) -> String {
    let mut parts: Vec<String> = Vec::new();
    if !learnings.trim().is_empty() {
        parts.push(format!("O que o projeto já aprendeu:\n{}", learnings.trim()));
    }
    if !situation.trim().is_empty() {
        parts.push(format!("Situação atual:\n{}", situation.trim()));
    }
    parts.join("\n\n")
}

/// Os feedbacks que a tarefa já recebeu, para repetir ao agente numa devolução ou reprovação (os últimos
/// [`HISTORY_SHOWN`], cada um cortado) e seguidos de uma linha em branco. Vazio quando não há nenhum.
/// `task.feedbacks` não pode já conter o feedback que está sendo entregue agora.
fn history_section(task: &Task) -> String {
    if task.feedbacks.is_empty() {
        return String::new();
    }
    let skipped = task.feedbacks.len().saturating_sub(HISTORY_SHOWN);
    let mut out = String::from("Feedbacks anteriores desta tarefa (do mais antigo ao mais novo; não repita o que já foi pedido):");
    if skipped > 0 {
        out.push_str(&format!("\n(mais {skipped} anterior(es) omitido(s))"));
    }
    for (n, f) in task.feedbacks.iter().enumerate().skip(skipped) {
        let (head, cut) = util::truncate_chars(f.text.trim(), HISTORY_ITEM_CHARS);
        out.push_str(&format!("\n{}. [{}] {}{}", n + 1, f.origin, head, if cut { "…" } else { "" }));
    }
    out.push_str("\n\n");
    out
}

/// Os aprendizados dos arquivos que o feedback cita (já prontos, de [`crate::learnings::render_specific`]) para a
/// devolução ou reprovação, seguidos de uma linha em branco. Vazio quando não há nenhum.
fn learned_section(learnings: &str) -> String {
    match learnings.trim() {
        "" => String::new(),
        l => format!("O que o projeto já aprendeu sobre os arquivos citados:\n\n{l}\n\n"),
    }
}

/// A mensagem que entrega o brief ao agente: o pedido, a memória do projeto (`memory`, de [`memory_block`]) e
/// as regras da tarefa. `boss` é quem criou a tarefa quando é um agente (as dúvidas vão a ele por
/// `ask-agent --boss`).
pub fn start_message(task: &Task, boss: Option<&str>, memory: &str) -> String {
    let brief = if task.brief.trim().is_empty() { task.title.as_str() } else { task.brief.trim() };
    let doubts = match boss {
        Some(b) => format!("Dúvidas: pergunte a quem criou a tarefa (\"{b}\") com ask-agent --boss \"sua pergunta\"."),
        None => "Dúvidas: escreva aqui no terminal; o usuário acompanha esta aba.".to_owned(),
    };
    let memory = match memory.trim() {
        "" => String::new(),
        m => format!("{m}\n\n---\n"),
    };
    terminal_text(&format!(
        "[AgentDeck · Tarefa {id}: {title}]\n\n{brief}\n\n---\n{memory}\
Regras da tarefa {id}:\n\
- Trabalhe SOMENTE na pasta {wt}, na branch {branch} (criada a partir de {base}). Não mexa no repositório principal nem em outras pastas.\n\
- Faça commits pequenos nessa branch. Não faça merge nem push: quem mescla é o AgentDeck, depois da revisão.\n\
- Não peça a ninguém (outro agente ou o usuário) que mescle ou rode ask-agent --task merge: a decisão é de quem revisa. O seu trabalho termina no --task done.\n\
- Antes de mexer num arquivo pela primeira vez, rode ask-agent --learn <arquivo> (mostra o que o projeto já aprendeu sobre ele).\n\
- Se descobrir algo não óbvio que o próximo agente precisaria saber (armadilha, decisão, convenção), crie um arquivo em memory/aprendizados/ nesta branch (formato em memory/aprendizados/README.md) e cite-o no relatório: ele passa pela mesma revisão e mescla que o código.\n\
- Ao terminar, escreva o relatório (o que foi feito, arquivos alterados, testes rodados e o que falta) em um arquivo e rode: ask-agent --task done {id} --file <arquivo>.\n\
  (Entregar por arquivo preserva os acentos; canalizar pela entrada padrão (-) faz o shell do Windows trocá-los por \"?\".)\n\
- Para saber que agentes e modelos este computador tem, e a cota de cada um, rode `ask-agent --models` antes de escolher `--agent`/`--model`.\n\
- {doubts}

Comece agora, por conta própria: este brief é a sua ordem de trabalho. Não espere confirmação nem pergunte se pode começar.",
        id = task.id,
        title = task.title,
        wt = task.worktree.display(),
        branch = task.branch,
        base = task.base,
    ))
}

/// A mensagem que devolve a tarefa ao agente com o feedback da revisão.
pub fn return_message(task: &Task, feedback: &str, learnings: &str) -> String {
    terminal_text(&format!(
        "[AgentDeck · Tarefa {id}: {title} — ajustes pedidos na revisão]\n\n{feedback}\n\n{history}{learned}---\n\
Continue na mesma pasta ({wt}) e na branch {branch}, com commits pequenos. Ao terminar, escreva o relatório em um arquivo e rode de novo ask-agent --task done {id} --file <arquivo> \
(entregar por arquivo preserva os acentos).",
        id = task.id,
        title = task.title,
        wt = task.worktree.display(),
        branch = task.branch,
        feedback = feedback.trim(),
        history = history_section(task),
        learned = learned_section(learnings),
    ))
}

/// A mensagem que devolve ao agente uma tarefa reprovada na validação: o trabalho já foi mesclado na base,
/// o usuário testou e reprovou. `aligned` = a branch da tarefa já está alinhada com a base.
pub fn reject_message(task: &Task, feedback: &str, aligned: bool, learnings: &str) -> String {
    let align = if aligned {
        String::new()
    } else {
        format!(
            " Atenção: não consegui alinhar a sua branch com {base}; antes de continuar, rode na sua pasta: git merge {base}.",
            base = task.base
        )
    };
    terminal_text(&format!(
        "[AgentDeck · Tarefa {id}: {title} — reprovada na validação]\n\n\
O seu trabalho anterior já foi mesclado em {base}, mas o usuário testou e reprovou pelo seguinte motivo:\n\n{feedback}\n\n{history}{learned}---\n\
Corrija na mesma pasta ({wt}) e na mesma branch {branch}, com commits pequenos.{align} Ao terminar, escreva o relatório em um arquivo e rode de novo ask-agent --task done {id} --file <arquivo> \
(entregar por arquivo preserva os acentos).",
        id = task.id,
        title = task.title,
        base = task.base,
        wt = task.worktree.display(),
        branch = task.branch,
        feedback = feedback.trim(),
        history = history_section(task),
        learned = learned_section(learnings),
    ))
}

/// A linha curta (ASCII) que abre o agente de uma tarefa no lugar do texto completo: aspas, quebras de linha e
/// acentos na linha de comando se perdem nos shims `.cmd` do Windows, então o texto vai num arquivo e a linha só
/// manda lê-lo. `intro` diz o que aconteceu (`o pedido esta no arquivo`, `foi devolvida na revisao`…).
/// `None` se o caminho não é ASCII (a linha de comando não o carregaria inteiro).
pub fn boot_line(task: &Task, intro: &str, file: &Path) -> Option<String> {
    let line = format!(
        "Tarefa {}: {intro}. Leia o arquivo {} e comece agora, por conta propria, sem esperar confirmacao.",
        task.id,
        file.display()
    );
    (line.is_ascii() && !line.contains(['\n', '\r', '"'])).then_some(line)
}

/// Lista curta de caminhos para mensagens (`a, b, c e mais 2`).
pub(super) fn short_list(paths: &[String]) -> String {
    const SHOWN: usize = 5;
    let mut out = paths.iter().take(SHOWN).cloned().collect::<Vec<_>>().join(", ");
    if paths.len() > SHOWN {
        out.push_str(&format!(" e mais {}", paths.len() - SHOWN));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::super::tests::sample;
    use super::*;

    #[test]
    fn slugs_are_ascii_lowercase_and_short() {
        assert_eq!(slugify("Corrigir o botão de Ação!", 40), "corrigir-o-botao-de-acao");
        assert_eq!(slugify("  --Olá,   mundo--  ", 40), "ola-mundo");
        assert_eq!(slugify("日本語", 40), "");
        assert_eq!(slugify("a/b\\c:d", 40), "a-b-c-d");
        let long = slugify(&"palavra ".repeat(20), 40);
        assert!(long.len() <= 40 && !long.ends_with('-'), "{long}");
        assert_eq!(branch_name("t3", "Corrigir o botão"), "tarefa/t3-corrigir-o-botao");
        assert_eq!(branch_name("t3", "???"), "tarefa/t3");
        assert!(crate::git::valid_branch_name(&branch_name("t12", "Título com / barra e \"aspas\" ~ ^ :")));
    }

    #[test]
    fn task_ids_are_normalized() {
        assert_eq!(parse_task_id("t3").as_deref(), Some("t3"));
        assert_eq!(parse_task_id(" T03 ").as_deref(), Some("t3"));
        assert_eq!(parse_task_id("12").as_deref(), Some("t12"));
        for bad in ["", "t", "tx", "t-1", "t3a", "claude@t3", "../t3", "12345678901"] {
            assert_eq!(parse_task_id(bad), None, "{bad}");
        }
    }

    #[test]
    fn titles_are_one_clean_line() {
        assert_eq!(clean_title("  Uma \n tarefa\t boa ").unwrap(), "Uma tarefa boa");
        assert!(matches!(clean_title(" \n\t "), Err(TaskError::Invalid(_))));
        assert_eq!(clean_title(&"x".repeat(500)).unwrap().chars().count(), MAX_TITLE);
    }

    #[test]
    fn the_start_message_carries_the_brief_and_the_rules() {
        let m = start_message(&sample(), Some("claude"), "");
        assert!(m.starts_with("[AgentDeck · Tarefa t3: Corrigir o login]\n\nFaça X.\n\nDepois Y."), "{m}");
        assert!(m.contains("SOMENTE na pasta") && m.contains("tarefa/t3-corrigir-o-login") && m.contains("criada a partir de main"), "{m}");
        assert!(m.contains("ask-agent --task done t3 --file"), "{m}");
        assert!(m.contains("Não faça merge nem push"), "{m}");
        assert!(m.contains("Não peça a ninguém") && m.contains("termina no --task done"), "{m}");
        assert!(m.contains("ask-agent --boss") && m.contains("\"claude\""), "{m}");
        // O brief termina com uma ordem: o agente não fica esperando um "pode começar".
        assert!(m.ends_with("Não espere confirmação nem pergunte se pode começar."), "{m}");
        // Sem chefe agente, as dúvidas ficam no terminal; sem brief, vale o título; sem ESC.
        let mut t = sample();
        t.brief = "  ".into();
        t.title = "T\x1b[201~ítulo".into();
        let m = start_message(&t, None, "");
        assert!(m.contains("escreva aqui no terminal") && !m.contains("--boss"), "{m}");
        assert!(!m.contains('\x1b'), "{m:?}");
    }

    #[test]
    fn the_start_message_carries_the_memory_and_the_memory_rules() {
        // Sem memória, nada de seção vazia.
        let plain = start_message(&sample(), None, &memory_block("", " "));
        assert!(!plain.contains("O que o projeto já aprendeu") && !plain.contains("Situação atual"), "{plain}");
        // Com memória: entre o pedido e as regras, cada parte com o seu título.
        let memory = memory_block("### T (memory/aprendizados/a.md)\ncorpo", "Outras tarefas em andamento: nenhuma.");
        let m = start_message(&sample(), None, &memory);
        let (brief, learned, situation, rules) = (
            m.find("Depois Y."),
            m.find("O que o projeto já aprendeu:\n### T"),
            m.find("Situação atual:\nOutras tarefas"),
            m.find("Regras da tarefa t3"),
        );
        assert!(brief < learned && learned < situation && situation < rules && rules.is_some(), "{m}");
        // As regras mandam consultar e registrar aprendizados.
        assert!(m.contains("ask-agent --learn <arquivo>") && m.contains("memory/aprendizados/"), "{m}");
        assert_eq!(memory_block("só longo prazo", ""), "O que o projeto já aprendeu:\nsó longo prazo");
    }

    #[test]
    fn feedbacks_come_back_in_the_return_and_reject_messages() {
        let mut t = sample();
        // Primeira devolução: nada anterior.
        assert!(!return_message(&t, "Faltou o teste.", "").contains("Feedbacks anteriores"));
        t.add_feedback("revisão", "Faltou o teste.");
        t.add_feedback("validação", "Fecha ao salvar.");
        let m = return_message(&t, "Agora o log.", "");
        assert!(m.contains("\n\nAgora o log.\n\nFeedbacks anteriores desta tarefa"), "{m}");
        assert!(m.contains("1. [revisão] Faltou o teste.\n2. [validação] Fecha ao salvar.\n\n---"), "{m}");
        let m = reject_message(&t, "Ainda fecha.", true, "");
        assert!(m.contains("Ainda fecha.\n\nFeedbacks anteriores") && m.contains("2. [validação] Fecha ao salvar."), "{m}");
        // Muitos: só os últimos, e cada um cortado.
        for n in 0..10 {
            t.add_feedback("revisão", &format!("pedido {n} {}", "x".repeat(2 * HISTORY_ITEM_CHARS)));
        }
        let m = return_message(&t, "mais um", "");
        assert!(m.contains("(mais 7 anterior(es) omitido(s))") && m.contains("pedido 9") && !m.contains("pedido 4"), "{m}");
        assert!(m.contains('…') && m.chars().count() < 6 * (HISTORY_ITEM_CHARS + 60) + 800, "{}", m.chars().count());
    }

    #[test]
    fn the_learnings_of_the_cited_files_come_before_the_closing_rule() {
        let learned = "### T (memory/aprendizados/a.md)\ncorpo";
        let mut t = sample();
        t.add_feedback("revisão", "Primeiro.");
        for m in [return_message(&t, "Mexa em x.rs.", learned), reject_message(&t, "Mexa em x.rs.", true, learned)] {
            let (feedback, history, section, rule) = (
                m.find("Mexa em x.rs."),
                m.find("Feedbacks anteriores"),
                m.find("O que o projeto já aprendeu sobre os arquivos citados:\n\n### T"),
                m.find("corpo\n\n---\n"),
            );
            assert!(feedback < history && history < section && section < rule && rule.is_some(), "{m}");
        }
        // Vazio ou só espaços: a mensagem fica idêntica à de sempre.
        for blank in ["", "  \n "] {
            assert_eq!(return_message(&t, "x", blank), return_message(&t, "x", ""));
            assert!(!reject_message(&t, "x", true, blank).contains("sobre os arquivos citados"));
        }
    }

    #[test]
    fn the_return_message_has_the_feedback() {
        let m = return_message(&sample(), "  Faltou o teste.  ", "");
        assert!(m.contains("ajustes pedidos") && m.contains("\n\nFaltou o teste.\n\n"), "{m}");
        assert!(m.contains("ask-agent --task done t3 --file"), "{m}");
    }

    #[test]
    fn the_reject_message_says_what_was_merged_and_why_it_failed() {
        let m = reject_message(&sample(), "  Fecha ao salvar.  ", true, "");
        assert!(m.contains("reprovada na validação") && m.contains("já foi mesclado em main"), "{m}");
        assert!(
            m.contains(
                "pelo seguinte motivo:

Fecha ao salvar.

"
            ),
            "{m}"
        );
        assert!(m.contains("mesma branch tarefa/t3-corrigir-o-login") && m.contains("ask-agent --task done t3 --file"), "{m}");
        assert!(!m.contains("alinhar") && !m.contains(''), "{m:?}");
        // Branch que não pôde ser alinhada: o agente recebe o comando para fazê-lo.
        assert!(reject_message(&sample(), "x", false, "").contains("rode na sua pasta: git merge main"));
    }
}
