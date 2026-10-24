//! Memória de curto prazo: a "situação atual" do projeto.
//!
//! Diz ao agente o que está acontecendo agora no projeto, para ele não pisar em ninguém: as outras tarefas em
//! andamento (com os arquivos que a branch delas já mexeu) e os últimos commits da base. Sai por dois caminhos:
//! empurrada no início de uma tarefa (`task_situation`) e por demanda no `ask-agent --learn` de qualquer aba
//! ([`crate::Core::learn`]), que também avisa quando um arquivo que o agente vai mexer já é mexido por outra
//! tarefa ([`clash_warnings`]). Quem monta a lista de tarefas com os arquivos é [`Core::busy_tasks`]; o resto
//! são funções puras.

use crate::git::Repo;
use crate::tasks::{changed_files, union_files, worktree_dirty_unlocked};
use crate::{Core, TaskStatus};

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
    /// Arquivos que o pedido da tarefa cita e que ainda não estão em `files`: é onde o choque começa, antes do
    /// primeiro commit. Vazio em `Validating` (já está na base).
    pub cited: Vec<String>,
}

fn clip(text: &str, max: usize) -> String {
    let (head, cut) = crate::util::truncate_chars(text, max);
    if cut {
        format!("{}…", head.trim_end())
    } else {
        head.to_owned()
    }
}

/// Arquivos que não contam como choque: o histórico mescla por união (`.gitattributes`), então duas tarefas
/// acrescentando ali não conflitam.
const MERGE_SAFE: &[&str] = &["memory/historico.md"];

/// Os últimos commits de `base` (`git log --oneline`); vazio se o git falha.
pub(crate) fn base_commits(repo: &Repo, base: &str) -> Vec<String> {
    repo.read(&["log", "--oneline", "-8", base, "--"])
        .map(|o| String::from_utf8_lossy(&o.stdout).lines().map(str::to_owned).collect())
        .unwrap_or_default()
}

impl Core {
    /// As tarefas do projeto que estão mexendo no código agora (em andamento, em revisão, aguardando validação),
    /// menos a da instância `except_instance`, cada uma com os arquivos que mexe e os que o pedido dela cita. Cada
    /// tarefa pede ao git dois comandos (a branch e a worktree), então rodam em paralelo, uma thread por tarefa.
    pub(crate) fn busy_tasks(&self, project: &str, repo: &Repo, except_instance: &str) -> Vec<OtherTask> {
        let busy: Vec<_> = self
            .tasks(project)
            .into_iter()
            .filter(|t| {
                t.instance != except_instance && matches!(t.status, TaskStatus::Running | TaskStatus::Review | TaskStatus::Validating)
            })
            .collect();
        std::thread::scope(|scope| {
            let jobs: Vec<_> = busy
                .into_iter()
                .map(|t| {
                    scope.spawn(move || {
                        // `Validating` já está na base (a branch foi alinhada): não há diff a mostrar.
                        // Em andamento, junta o que já está na branch com o que ainda não foi commitado.
                        let (files, cited) = match t.status {
                            TaskStatus::Validating => (Vec::new(), Vec::new()),
                            _ => {
                                let files =
                                    union_files(changed_files(repo, &t.base, &t.branch), worktree_dirty_unlocked(repo, &t.worktree));
                                let brief = if t.brief.trim().is_empty() { t.title.as_str() } else { t.brief.as_str() };
                                let mut cited = crate::learnings::paths_in_text(brief, &t.worktree);
                                cited.retain(|c| !files.contains(c));
                                (files, cited)
                            }
                        };
                        OtherTask { id: t.id, title: t.title, instance: t.instance, status: t.status.label().to_owned(), files, cited }
                    })
                })
                .collect();
            jobs.into_iter().filter_map(|j| j.join().ok()).collect()
        })
    }
}

/// Um aviso por arquivo de `paths` que outra tarefa de `others` também mexe, dizendo de onde veio (`já mexeu`:
/// está na branch ou na worktree dela; `o pedido cita`: ainda não mexeu, mas o pedido dela cita o arquivo).
/// `memory/historico.md` não conta. Um por linha; vazio se não há choque.
pub fn clash_warnings(others: &[OtherTask], paths: &[String]) -> String {
    let mut lines: Vec<String> = Vec::new();
    for path in paths {
        if MERGE_SAFE.iter().any(|m| m.eq_ignore_ascii_case(path)) || lines.iter().any(|l| l.starts_with(&format!("{path}: "))) {
            continue;
        }
        let who: Vec<String> = others
            .iter()
            .filter_map(|t| {
                let origin = if t.files.iter().any(|f| f.eq_ignore_ascii_case(path)) {
                    "já mexeu"
                } else if t.cited.iter().any(|f| f.eq_ignore_ascii_case(path)) {
                    "o pedido cita"
                } else {
                    return None;
                };
                Some(format!("a {} ({}, {}; {origin})", t.id, t.instance, t.status))
            })
            .collect();
        if who.is_empty() {
            continue;
        }
        let also = if who.len() == 1 { "também mexe" } else { "também mexem" };
        lines.push(format!("{path}: {} {also} nele; mudanças nas mesmas linhas vão conflitar na mesclagem", who.join(" e ")));
    }
    lines.join("\n")
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
                if !t.cited.is_empty() {
                    let shown: Vec<&str> = t.cited.iter().take(MAX_FILES).map(String::as_str).collect();
                    line.push_str(&format!("; o pedido cita {}", shown.join(", ")));
                }
            } else {
                let shown: Vec<&str> = t.files.iter().take(MAX_FILES).map(String::as_str).collect();
                line.push_str(&format!(": mexe em {}", shown.join(", ")));
                if t.files.len() > MAX_FILES {
                    line.push_str(&format!(" e mais {}", t.files.len() - MAX_FILES));
                }
                let clash: Vec<&str> = t.files.iter().chain(&t.cited).filter(|f| own_paths.contains(f)).map(String::as_str).collect();
                if !clash.is_empty() {
                    line.push_str(&format!(". ATENÇÃO: o seu pedido cita {}; combine com essa tarefa antes de mexer", clash.join(", ")));
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
            cited: Vec::new(),
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
        assert!(
            out.contains("- t12 \"Título t12\" (claude@t12, em andamento): mexe em crates/app/src/ui/tabs.rs, crates/core/src/tasks.rs"),
            "{out}"
        );
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
    fn clashes_name_the_other_tasks_and_skip_the_history() {
        let a = other("t37", &["crates/app/src/app.rs", "memory/historico.md"]);
        let b = OtherTask { cited: vec!["crates/app/src/app.rs".into(), "memory/historico.md".into()], ..other("t40", &["x.rs"]) };
        let paths = ["crates/app/src/app.rs", "memory/historico.md", "y.rs"].map(String::from);
        let out = clash_warnings(&[a.clone(), b], &paths);
        assert_eq!(
            out,
            "crates/app/src/app.rs: a t37 (claude@t37, em andamento; já mexeu) e a t40 (claude@t40, em andamento; o pedido cita) também mexem nele; mudanças nas mesmas linhas vão conflitar na mesclagem"
        );
        let one = clash_warnings(&[a], &paths[..1]);
        assert!(one.contains("a t37 (claude@t37, em andamento; já mexeu) também mexe nele;"), "{one}");
        assert_eq!(clash_warnings(&[], &paths), "");
    }

    #[test]
    fn a_task_with_no_files_yet_shows_what_its_request_cites() {
        let t = OtherTask { cited: vec!["a.rs".into(), "b.rs".into()], ..other("t12", &[]) };
        let out = render(&[t], &[], "dev", &[], BUDGET);
        assert!(out.contains("ainda sem arquivos na branch; o pedido cita a.rs, b.rs"), "{out}");
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
