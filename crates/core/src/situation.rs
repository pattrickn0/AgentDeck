//! Memória de curto prazo: a "situação atual" entregue no início de uma tarefa.
//!
//! Diz ao agente o que está acontecendo agora no projeto, para ele não pisar em ninguém: as outras tarefas em
//! andamento (com os arquivos que a branch delas já mexeu) e os últimos commits da base. Funções puras; quem
//! consulta o git e a lista de tarefas é `tasks.rs`.

/// Orçamento (em caracteres) da seção inteira.
pub const BUDGET: usize = 1_500;
/// Outras tarefas citadas, e arquivos mostrados por tarefa.
const MAX_TASKS: usize = 6;
const MAX_FILES: usize = 6;

/// Uma tarefa em andamento no projeto (que não é a que está começando).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OtherTask {
    pub id: String,
    pub title: String,
    /// A instância do agente (`claude@t12`).
    pub instance: String,
    /// O estado, para pessoas (`em andamento`).
    pub status: String,
    /// Arquivos que a branch já mexeu (`git diff --name-only base...branch`); vazio = nenhum até agora.
    pub files: Vec<String>,
}

fn clip(text: &str, max: usize) -> String {
    let (head, cut) = crate::util::truncate_chars(text, max);
    if cut {
        format!("{}…", head.trim_end())
    } else {
        head.to_owned()
    }
}

/// A seção "Situação atual". `own_paths` são os arquivos que o pedido da tarefa cita: se outra tarefa mexe em
/// um deles, a linha avisa. `commits` são linhas de `git log --oneline` da base. Cabe em `budget` caracteres:
/// o que não couber é cortado do fim (primeiro os commits).
pub fn render(others: &[OtherTask], commits: &[String], base: &str, own_paths: &[String], budget: usize) -> String {
    let mut lines: Vec<String> = Vec::new();
    if others.is_empty() {
        lines.push("Outras tarefas em andamento: nenhuma.".to_owned());
    } else {
        lines.push("Outras tarefas em andamento (evite os mesmos arquivos):".to_owned());
        for t in others.iter().take(MAX_TASKS) {
            let mut line = format!("- {} \"{}\" ({}, {})", t.id, clip(&t.title, 60), t.instance, t.status);
            if t.files.is_empty() {
                line.push_str(": ainda sem arquivos na branch");
            } else {
                let shown: Vec<&str> = t.files.iter().take(MAX_FILES).map(String::as_str).collect();
                line.push_str(&format!(": mexe em {}", shown.join(", ")));
                if t.files.len() > MAX_FILES {
                    line.push_str(&format!(" e mais {}", t.files.len() - MAX_FILES));
                }
                let clash: Vec<&str> = t.files.iter().filter(|f| own_paths.contains(f)).map(String::as_str).collect();
                if !clash.is_empty() {
                    line.push_str(&format!(". ATENÇÃO: o seu pedido cita {}; combine com ela antes de mexer", clash.join(", ")));
                }
            }
            lines.push(line);
        }
        if others.len() > MAX_TASKS {
            lines.push(format!("- … e mais {} tarefa(s)", others.len() - MAX_TASKS));
        }
    }
    if !commits.is_empty() {
        lines.push(format!("Últimos commits de {base}:"));
        lines.extend(commits.iter().map(|c| format!("- {}", clip(c.trim(), 90))));
    }
    let mut out = String::new();
    for line in lines {
        let line = clip(&line, 400);
        if out.chars().count() + line.chars().count() + 2 > budget {
            out.push('…');
            break;
        }
        if !out.is_empty() {
            out.push('\n');
        }
        out.push_str(&line);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn other(id: &str, files: &[&str]) -> OtherTask {
        OtherTask {
            id: id.into(),
            title: format!("Título {id}"),
            instance: format!("claude@{id}"),
            status: "em andamento".into(),
            files: files.iter().map(|f| (*f).to_owned()).collect(),
        }
    }

    fn commits() -> Vec<String> {
        (0..8).map(|n| format!("abc{n:04} commit número {n} com uma mensagem bem comprida para ocupar espaço")).collect()
    }

    #[test]
    fn an_empty_project_still_lists_the_base_commits() {
        let out = render(&[], &["1fa59a7 Merge tarefa t11".into()], "dev", &[], BUDGET);
        assert_eq!(out, "Outras tarefas em andamento: nenhuma.\nÚltimos commits de dev:\n- 1fa59a7 Merge tarefa t11");
    }

    #[test]
    fn other_tasks_show_their_files_and_flag_a_clash() {
        let out = render(
            &[other("t12", &["crates/app/src/ui/tabs.rs", "crates/core/src/tasks.rs"]), other("t14", &[])],
            &[],
            "dev",
            &["crates/core/src/tasks.rs".to_owned()],
            BUDGET,
        );
        assert!(out.contains("- t12 \"Título t12\" (claude@t12, em andamento): mexe em crates/app/src/ui/tabs.rs, crates/core/src/tasks.rs"), "{out}");
        assert!(out.contains("ATENÇÃO: o seu pedido cita crates/core/src/tasks.rs"), "{out}");
        assert!(out.contains("- t14 \"Título t14\" (claude@t14, em andamento): ainda sem arquivos na branch"), "{out}");
        assert!(!out.contains("Últimos commits"), "{out}");
    }

    #[test]
    fn long_lists_are_summarised() {
        let files: Vec<String> = (0..10).map(|n| format!("f{n}.rs")).collect();
        let many: Vec<OtherTask> = (1..=9).map(|n| OtherTask { files: files.clone(), ..other(&format!("t{n}"), &[]) }).collect();
        let out = render(&many, &[], "dev", &[], 100_000);
        assert!(out.contains("f5.rs e mais 4"), "{out}");
        assert!(out.contains("- … e mais 3 tarefa(s)") && !out.contains("- t7 "), "{out}");
    }

    #[test]
    fn the_budget_cuts_the_commits_first() {
        let tasks = [other("t12", &["a.rs"])];
        let out = render(&tasks, &commits(), "dev", &[], BUDGET);
        assert!(out.chars().count() <= BUDGET, "{}", out.chars().count());
        assert!(out.contains("- t12 ") && out.contains("Últimos commits de dev:"), "{out}");
        // Com pouco espaço, os commits somem do fim, nunca as tarefas.
        let tight = render(&tasks, &commits(), "dev", &[], 200);
        assert!(tight.chars().count() <= 200 && tight.contains("- t12 ") && tight.ends_with('…'), "{tight}");
        assert!(!tight.contains("abc0007"), "{tight}");
    }
}
