use std::path::Path;

use unicode_normalization::UnicodeNormalization;

use super::{Task, TaskError, HISTORY_ITEM_CHARS, HISTORY_SHOWN, MAX_SLUG, MAX_TITLE};
use crate::t;
use crate::util::{self, terminal_text};

/// `New  task!` → `new-task`: lowercase ASCII, no accents, at most `max` characters.
pub fn slugify(title: &str, max: usize) -> String {
    let mut out = String::new();
    for c in title.nfd() {
        if c.is_ascii_alphanumeric() {
            out.push(c.to_ascii_lowercase());
        } else if c.is_ascii() && !out.ends_with('-') {
            out.push('-');
        }
        // Non-ASCII (loose accents left by the decomposition, ideograms…): vanishes without splitting the word.
    }
    let cut: String = out.trim_matches('-').chars().take(max).collect();
    cut.trim_end_matches('-').to_owned()
}

/// `task/<id>-<slug>` (without a slug, `task/<id>`).
pub fn branch_name(id: &str, title: &str) -> String {
    match slugify(title, MAX_SLUG) {
        s if s.is_empty() => format!("task/{id}"),
        s => format!("task/{id}-{s}"),
    }
}

/// `T3`, `t3` and `3` → `t3`. `None` if it is not a task id.
pub fn parse_task_id(text: &str) -> Option<String> {
    let t = text.trim().to_ascii_lowercase();
    let digits = t.strip_prefix('t').unwrap_or(&t);
    if digits.is_empty() || digits.len() > 9 || !digits.chars().all(|c| c.is_ascii_digit()) {
        return None;
    }
    digits.parse::<u64>().ok().map(|n| format!("t{n}"))
}

/// Title on a single line, without extra spaces and with at most [`MAX_TITLE`] characters.
pub fn clean_title(title: &str) -> Result<String, TaskError> {
    let one_line = title.split_whitespace().collect::<Vec<_>>().join(" ");
    let one_line: String = one_line.chars().filter(|c| !c.is_control()).collect();
    if one_line.is_empty() {
        return Err(TaskError::Invalid(t!("the task title cannot be empty").into()));
    }
    Ok(one_line.chars().take(MAX_TITLE).collect())
}

/// Cuts the text at `max` characters with a notice. The notice is plain English on purpose: the brief, the feedback and
/// the report travel to agents (and the report comes from one).
pub(super) fn cap_text(text: &str, max: usize) -> String {
    let (head, cut) = util::truncate_chars(text, max);
    if cut {
        format!("{head}\n\n[text cut at {max} characters]")
    } else {
        head.to_owned()
    }
}

/// The task-start memory block: what the project has already learned (long term) and the current situation
/// (short term). Each part vanishes when empty; empty when both are. Agent-facing text: plain English.
pub fn memory_block(learnings: &str, situation: &str) -> String {
    let mut parts: Vec<String> = Vec::new();
    if !learnings.trim().is_empty() {
        parts.push(format!("What the project has already learned:\n{}", learnings.trim()));
    }
    if !situation.trim().is_empty() {
        parts.push(format!("Current situation:\n{}", situation.trim()));
    }
    parts.join("\n\n")
}

/// The feedbacks the task has already received, to repeat to the agent on a return or rejection (the latest
/// [`HISTORY_SHOWN`], each one cut) followed by a blank line. Empty when there are none.
/// `task.feedbacks` must not already contain the feedback being delivered now.
fn history_section(task: &Task) -> String {
    if task.feedbacks.is_empty() {
        return String::new();
    }
    let skipped = task.feedbacks.len().saturating_sub(HISTORY_SHOWN);
    let mut out = String::from("Earlier feedback on this task (oldest to newest; do not repeat what was already asked):");
    if skipped > 0 {
        out.push_str(&format!("\n({skipped} earlier item(s) omitted)"));
    }
    for (n, f) in task.feedbacks.iter().enumerate().skip(skipped) {
        let (head, cut) = util::truncate_chars(f.text.trim(), HISTORY_ITEM_CHARS);
        out.push_str(&format!("\n{}. [{}] {}{}", n + 1, f.origin, head, if cut { "…" } else { "" }));
    }
    out.push_str("\n\n");
    out
}

/// The learnings of the files the feedback cites (already rendered by [`crate::learnings::render_specific`]) for the
/// return or rejection, followed by a blank line. Empty when there are none.
fn learned_section(learnings: &str) -> String {
    match learnings.trim() {
        "" => String::new(),
        l => format!("What the project has already learned about the cited files:\n\n{l}\n\n"),
    }
}

/// The message that delivers the brief to the agent: the request, the project memory (`memory`, from
/// [`memory_block`]) and the task rules. `boss` is whoever created the task when it is an agent (questions go to
/// it through `ask-agent --boss`). Agent-facing text: plain English.
pub fn start_message(task: &Task, boss: Option<&str>, memory: &str) -> String {
    let brief = if task.brief.trim().is_empty() { task.title.as_str() } else { task.brief.trim() };
    let doubts = match boss {
        Some(b) => format!("Questions: ask whoever created the task (\"{b}\") with ask-agent --boss \"your question\"."),
        None => "Questions: write here in the terminal; the user follows this tab.".to_owned(),
    };
    let memory = match memory.trim() {
        "" => String::new(),
        m => format!("{m}\n\n---\n"),
    };
    terminal_text(&format!(
        "[AgentDeck · Task {id}: {title}]\n\n{brief}\n\n---\n{memory}\
Rules for task {id}:\n\
- Work ONLY in the folder {wt}, on the branch {branch} (created from {base}). Do not touch the main repository or any other folder.\n\
- Make small commits on that branch. Do not merge or push: AgentDeck does the merge, after the review.\n\
- Do not ask anyone (another agent or the user) to merge or to run ask-agent --task merge: that decision belongs to whoever reviews. Your work ends at --task done.\n\
- Before touching a file for the first time, run ask-agent --learn <file> (shows what the project has already learned about it).\n\
- If you find something non-obvious that the next agent would need to know (a trap, a decision, a convention), create a file in memory/learnings/ on this branch (format in memory/learnings/README.md) and cite it in the report: it goes through the same review and merge as the code.\n\
- When you finish, write the report (what was done, files changed, tests run and what is missing) to a file and run: ask-agent --task done {id} --file <file>.\n\
  (Delivering by file preserves accents; piping through standard input (-) makes the Windows shell replace them with \"?\".)\n\
- To learn which agents and models this computer has, and the quota of each, run `ask-agent --models` before choosing `--agent`/`--model`.\n\
- {doubts}

Start now, on your own: this brief is your work order. Do not wait for confirmation or ask whether you can start.",
        id = task.id,
        title = task.title,
        wt = task.worktree.display(),
        branch = task.branch,
        base = task.base,
    ))
}

/// The message that returns the task to the agent with the review feedback. Agent-facing text: plain English.
pub fn return_message(task: &Task, feedback: &str, learnings: &str) -> String {
    terminal_text(&format!(
        "[AgentDeck · Task {id}: {title} — changes requested in the review]\n\n{feedback}\n\n{history}{learned}---\n\
Continue in the same folder ({wt}) and on the branch {branch}, with small commits. When you finish, write the report to a file and run ask-agent --task done {id} --file <file> again \
(delivering by file preserves accents).",
        id = task.id,
        title = task.title,
        wt = task.worktree.display(),
        branch = task.branch,
        feedback = feedback.trim(),
        history = history_section(task),
        learned = learned_section(learnings),
    ))
}

/// The message that returns to the agent a task rejected in validation: the work was already merged into the base,
/// the user tested it and rejected it. `aligned` = the task's branch is already aligned with the base.
/// Agent-facing text: plain English.
pub fn reject_message(task: &Task, feedback: &str, aligned: bool, learnings: &str) -> String {
    let align = if aligned {
        String::new()
    } else {
        format!(
            " Warning: I could not align your branch with {base}; before continuing, run in your folder: git merge {base}.",
            base = task.base
        )
    };
    terminal_text(&format!(
        "[AgentDeck · Task {id}: {title} — rejected in validation]\n\n\
Your previous work was already merged into {base}, but the user tested it and rejected it for the following reason:\n\n{feedback}\n\n{history}{learned}---\n\
Fix it in the same folder ({wt}) and on the same branch {branch}, with small commits.{align} When you finish, write the report to a file and run ask-agent --task done {id} --file <file> again \
(delivering by file preserves accents).",
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

/// The short (ASCII) line that opens a task's agent in place of the full text: quotes, line breaks and accents on
/// the command line get lost in the Windows `.cmd` shims, so the text goes into a file and the line only tells the
/// agent to read it. `intro` says what happened (`the full request is in the file`, `was returned in review`…).
/// `None` if the path is not ASCII (the command line would not carry it whole). Agent-facing text: plain English.
pub fn boot_line(task: &Task, intro: &str, file: &Path) -> Option<String> {
    let line = format!(
        "Task {}: {intro}. Read the file {} and start now, on your own, without waiting for confirmation.",
        task.id,
        file.display()
    );
    (line.is_ascii() && !line.contains(['\n', '\r', '"'])).then_some(line)
}

/// Short list of paths for messages (`a, b, c and 2 more`).
pub(super) fn short_list(paths: &[String]) -> String {
    const SHOWN: usize = 5;
    let list = paths.iter().take(SHOWN).cloned().collect::<Vec<_>>().join(", ");
    match paths.len() > SHOWN {
        true => t!("{list} and {count} more", list, count = paths.len() - SHOWN),
        false => list,
    }
}

#[cfg(test)]
mod tests {
    use super::super::tests::sample;
    use super::*;

    #[test]
    fn slugs_are_ascii_lowercase_and_short() {
        assert_eq!(slugify("Fix the café button, Ünï!", 40), "fix-the-cafe-button-uni");
        assert_eq!(slugify("  --Héllo,   world--  ", 40), "hello-world");
        assert_eq!(slugify("日本語", 40), "");
        assert_eq!(slugify("a/b\\c:d", 40), "a-b-c-d");
        let long = slugify(&"word ".repeat(20), 40);
        assert!(long.len() <= 40 && !long.ends_with('-'), "{long}");
        assert_eq!(branch_name("t3", "Fix the button"), "task/t3-fix-the-button");
        assert_eq!(branch_name("t3", "???"), "task/t3");
        assert!(crate::git::valid_branch_name(&branch_name("t12", "Title with / slash and \"quotes\" ~ ^ :")));
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
        assert_eq!(clean_title("  A \n good\t task ").unwrap(), "A good task");
        assert!(matches!(clean_title(" \n\t "), Err(TaskError::Invalid(_))));
        assert_eq!(clean_title(&"x".repeat(500)).unwrap().chars().count(), MAX_TITLE);
    }

    #[test]
    fn the_start_message_carries_the_brief_and_the_rules() {
        let m = start_message(&sample(), Some("claude"), "");
        assert!(m.starts_with("[AgentDeck · Task t3: Fix the login]\n\nDo X.\n\nThen Y."), "{m}");
        assert!(m.contains("ONLY in the folder") && m.contains("task/t3-fix-the-login") && m.contains("created from main"), "{m}");
        assert!(m.contains("ask-agent --task done t3 --file"), "{m}");
        assert!(m.contains("Do not merge or push"), "{m}");
        assert!(m.contains("Do not ask anyone") && m.contains("ends at --task done"), "{m}");
        assert!(m.contains("ask-agent --boss") && m.contains("\"claude\""), "{m}");
        // The brief ends with an order: the agent does not sit waiting for a "you may start".
        assert!(m.ends_with("Do not wait for confirmation or ask whether you can start."), "{m}");
        // Without an agent boss, the questions stay in the terminal; without a brief, the title is used; no ESC.
        let mut t = sample();
        t.brief = "  ".into();
        t.title = "T\x1b[201~itle".into();
        let m = start_message(&t, None, "");
        assert!(m.contains("write here in the terminal") && !m.contains("--boss"), "{m}");
        assert!(!m.contains('\x1b'), "{m:?}");
    }

    #[test]
    fn the_start_message_carries_the_memory_and_the_memory_rules() {
        // Without memory, no empty section.
        let plain = start_message(&sample(), None, &memory_block("", " "));
        assert!(!plain.contains("What the project has already learned") && !plain.contains("Current situation"), "{plain}");
        // With memory: between the request and the rules, each part with its own title.
        let memory = memory_block("### T (memory/learnings/a.md)\nbody", "Other tasks in progress: none.");
        let m = start_message(&sample(), None, &memory);
        let (brief, learned, situation, rules) = (
            m.find("Then Y."),
            m.find("What the project has already learned:\n### T"),
            m.find("Current situation:\nOther tasks"),
            m.find("Rules for task t3"),
        );
        assert!(brief < learned && learned < situation && situation < rules && rules.is_some(), "{m}");
        // The rules tell the agent to consult and record learnings.
        assert!(m.contains("ask-agent --learn <file>") && m.contains("memory/learnings/"), "{m}");
        assert_eq!(memory_block("only long term", ""), "What the project has already learned:\nonly long term");
    }

    #[test]
    fn feedbacks_come_back_in_the_return_and_reject_messages() {
        let mut t = sample();
        // First return: nothing earlier.
        assert!(!return_message(&t, "The test is missing.", "").contains("Earlier feedback"));
        t.add_feedback("review", "The test is missing.");
        t.add_feedback("validation", "It closes on save.");
        let m = return_message(&t, "Now the log.", "");
        assert!(m.contains("\n\nNow the log.\n\nEarlier feedback on this task"), "{m}");
        assert!(m.contains("1. [review] The test is missing.\n2. [validation] It closes on save.\n\n---"), "{m}");
        let m = reject_message(&t, "It still closes.", true, "");
        assert!(m.contains("It still closes.\n\nEarlier feedback") && m.contains("2. [validation] It closes on save."), "{m}");
        // Many: only the latest, and each one cut.
        for n in 0..10 {
            t.add_feedback("review", &format!("request {n} {}", "x".repeat(2 * HISTORY_ITEM_CHARS)));
        }
        let m = return_message(&t, "one more", "");
        assert!(m.contains("(7 earlier item(s) omitted)") && m.contains("request 9") && !m.contains("request 4"), "{m}");
        assert!(m.contains('…') && m.chars().count() < 6 * (HISTORY_ITEM_CHARS + 60) + 800, "{}", m.chars().count());
    }

    #[test]
    fn the_learnings_of_the_cited_files_come_before_the_closing_rule() {
        let learned = "### T (memory/learnings/a.md)\nbody";
        let mut t = sample();
        t.add_feedback("review", "First.");
        for m in [return_message(&t, "Touch x.rs.", learned), reject_message(&t, "Touch x.rs.", true, learned)] {
            let (feedback, history, section, rule) = (
                m.find("Touch x.rs."),
                m.find("Earlier feedback"),
                m.find("What the project has already learned about the cited files:\n\n### T"),
                m.find("body\n\n---\n"),
            );
            assert!(feedback < history && history < section && section < rule && rule.is_some(), "{m}");
        }
        // Empty or only spaces: the message is identical to the usual one.
        for blank in ["", "  \n "] {
            assert_eq!(return_message(&t, "x", blank), return_message(&t, "x", ""));
            assert!(!reject_message(&t, "x", true, blank).contains("about the cited files"));
        }
    }

    #[test]
    fn the_return_message_has_the_feedback() {
        let m = return_message(&sample(), "  The test is missing.  ", "");
        assert!(m.contains("changes requested") && m.contains("\n\nThe test is missing.\n\n"), "{m}");
        assert!(m.contains("ask-agent --task done t3 --file"), "{m}");
    }

    #[test]
    fn the_reject_message_says_what_was_merged_and_why_it_failed() {
        let m = reject_message(&sample(), "  It closes on save.  ", true, "");
        assert!(m.contains("rejected in validation") && m.contains("already merged into main"), "{m}");
        assert!(
            m.contains(
                "for the following reason:

It closes on save.

"
            ),
            "{m}"
        );
        assert!(m.contains("same branch task/t3-fix-the-login") && m.contains("ask-agent --task done t3 --file"), "{m}");
        assert!(!m.contains("could not align") && !m.contains(''), "{m:?}");
        // Branch that could not be aligned: the agent gets the command to do it.
        assert!(reject_message(&sample(), "x", false, "").contains("run in your folder: git merge main"));
    }

    #[test]
    fn the_boot_line_is_ascii_and_in_english() {
        let line = boot_line(&sample(), "the full request is in the file", Path::new("/d/t3-brief.md")).unwrap();
        assert_eq!(line, "Task t3: the full request is in the file. Read the file /d/t3-brief.md and start now, on your own, without waiting for confirmation.");
        assert_eq!(boot_line(&sample(), "x", Path::new("/d/áá.md")), None);
    }

    #[test]
    fn long_lists_are_shortened_in_the_language_of_the_reader() {
        use crate::i18n::{with_lang, Lang};
        let seven: Vec<String> = (1..=7).map(|n| format!("f{n}")).collect();
        assert_eq!(with_lang(Lang::En, || short_list(&seven)), "f1, f2, f3, f4, f5 and 2 more");
        assert_eq!(with_lang(Lang::PtBr, || short_list(&seven)), "f1, f2, f3, f4, f5 e mais 2");
        assert_eq!(short_list(&seven[..2]), "f1, f2");
    }
}
