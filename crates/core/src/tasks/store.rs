use std::collections::HashMap;
use std::path::{Path, PathBuf};

use parking_lot::Mutex;
use serde::{Deserialize, Serialize};

use super::{git_error, parse_task_id, Task, TaskError, TaskStatus};
use crate::events::{CoreEvent, FeedEntry, FeedKind};
use crate::git::{GitError, Repo};
use crate::util::now_ms;
use crate::Core;

// ---------- Armazenamento ----------

/// Tarefas de um projeto como ficam em disco.
#[derive(Debug, Default, Serialize, Deserialize)]
#[serde(default)]
pub(crate) struct ProjectTasks {
    pub(super) version: u32,
    /// Próximo número a usar (nunca diminui).
    pub(super) next: u64,
    pub(super) tasks: Vec<Task>,
}

impl ProjectTasks {
    pub(super) fn task_mut(&mut self, id: &str) -> Option<&mut Task> {
        self.tasks.iter_mut().find(|t| t.id == id)
    }

    /// O contador nunca fica abaixo do maior número já usado (arquivo editado à mão ou antigo).
    fn fix_counter(&mut self) {
        let max = self.tasks.iter().filter_map(|t| parse_task_id(&t.id)).filter_map(|id| id[1..].parse::<u64>().ok()).max().unwrap_or(0);
        self.next = self.next.max(max + 1).max(1);
        self.version = 1;
    }
}

/// Estado das tarefas no core: os projetos já lidos e a trava das operações do ciclo de vida.
#[derive(Default)]
pub(crate) struct TaskStore {
    pub(super) projects: Mutex<HashMap<String, ProjectTasks>>,
    /// Uma operação do ciclo de vida por vez (criar, iniciar, mesclar…): cada uma pode rodar o git.
    pub(super) op: Mutex<()>,
    /// Prompt inicial à espera da abertura do processo (`projeto/instância` → linha), ver `Core::set_boot_prompt`.
    pub(super) boot: Mutex<HashMap<String, String>>,
}

fn load_file(path: &Path) -> ProjectTasks {
    let mut tasks = match std::fs::read_to_string(path) {
        Ok(text) => serde_json::from_str::<ProjectTasks>(&text).unwrap_or_else(|e| {
            // Ilegível: guarda uma cópia antes que a próxima gravação ponha uma lista vazia por cima.
            let backup = path.with_extension(format!("json.ilegivel-{}", now_ms()));
            match std::fs::copy(path, &backup) {
                Ok(_) => log::error!("tarefas ilegíveis em {} ({e}); cópia guardada em {}", path.display(), backup.display()),
                Err(e2) => log::error!("tarefas ilegíveis em {} ({e}; e a cópia falhou: {e2})", path.display()),
            }
            ProjectTasks::default()
        }),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => ProjectTasks::default(),
        Err(e) => {
            log::error!("não consegui ler as tarefas em {}: {e}", path.display());
            ProjectTasks::default()
        }
    };
    tasks.fix_counter();
    tasks
}

/// Grava ao lado e troca: uma queda no meio não deixa o arquivo pela metade.
fn save_file(path: &Path, tasks: &ProjectTasks) -> std::io::Result<()> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let text = serde_json::to_string_pretty(tasks).map_err(std::io::Error::other)?;
    let tmp = path.with_extension("json.tmp");
    std::fs::write(&tmp, text)?;
    std::fs::rename(&tmp, path)
}

/// Id de projeto que serve de nome de arquivo (`p` + hexadecimal, na prática).
fn safe_project_id(id: &str) -> bool {
    !id.is_empty() && id.len() <= 64 && id.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
}

impl Core {
    fn tasks_path(&self, project: &str) -> Option<PathBuf> {
        safe_project_id(project).then(|| self.paths().data_dir.join("tarefas").join(format!("{project}.json")))
    }

    /// Aplica `f` às tarefas do projeto (lidas do disco na primeira vez). Com `save`, grava e avisa a
    /// interface (`TasksChanged`) no fim.
    pub(super) fn with_tasks<R>(&self, project: &str, save: bool, f: impl FnOnce(&mut ProjectTasks) -> R) -> R {
        let path = self.tasks_path(project);
        let out = {
            let mut all = self.inner.tasks.projects.lock();
            let Some(path) = &path else {
                // Id estranho (não é nome de arquivo): nada é lido nem gravado.
                return f(&mut ProjectTasks::default());
            };
            let entry = all.entry(project.to_owned()).or_insert_with(|| load_file(path));
            let out = f(entry);
            if save {
                if let Err(e) = save_file(path, entry) {
                    log::warn!("não consegui gravar as tarefas em {}: {e}", path.display());
                }
            }
            out
        };
        if save {
            self.emit(CoreEvent::TasksChanged { project: project.to_owned() });
        }
        out
    }

    /// As tarefas do projeto, da mais antiga para a mais nova.
    pub fn tasks(&self, project: &str) -> Vec<Task> {
        self.with_tasks(project, false, |p| p.tasks.clone())
    }

    /// Uma tarefa do projeto (`t3`, `T3` ou `3`).
    pub fn task(&self, project: &str, id: &str) -> Option<Task> {
        let id = parse_task_id(id)?;
        self.with_tasks(project, false, |p| p.tasks.iter().find(|t| t.id == id).cloned())
    }

    /// Guarda a entrada do feed na linha do tempo de cada tarefa do projeto a que ela pertence (ver
    /// [`Task::owns_message`]): uma pergunta entre duas tarefas aparece nas duas. Só grava o arquivo se houver dona.
    pub(crate) fn record_task_message(&self, entry: &FeedEntry) {
        let Some(project) = entry.project.as_deref() else { return };
        if !matches!(entry.kind, FeedKind::Ask | FeedKind::Answer | FeedKind::Error) {
            return;
        }
        let owners: Vec<String> =
            self.with_tasks(project, false, |p| p.tasks.iter().filter(|t| t.owns_message(entry)).map(|t| t.id.clone()).collect());
        if owners.is_empty() {
            return;
        }
        self.with_tasks(project, true, |p| {
            for id in &owners {
                if let Some(t) = p.task_mut(id) {
                    t.record_message(entry);
                }
            }
        });
    }

    /// A tarefa de que a instância `instance` é o agente (e que ainda não terminou), se houver.
    pub fn task_of_instance(&self, project: &str, instance: &str) -> Option<Task> {
        self.with_tasks(project, false, |p| {
            p.tasks.iter().find(|t| t.instance == instance && !t.status.is_terminal() && t.status != TaskStatus::Draft).cloned()
        })
    }

    /// A pasta em que o terminal da instância deve abrir: a worktree da tarefa em andamento, em revisão ou
    /// aguardando validação. Uma tarefa `Failed` que já entregou o relatório (a mesclagem falhou) também
    /// segue na worktree: reabrir o app não pode soltar o agente no repositório principal.
    pub(crate) fn task_cwd(&self, project: &str, instance: &str) -> Option<PathBuf> {
        self.with_tasks(project, false, |p| {
            p.tasks
                .iter()
                .find(|t| {
                    t.instance == instance
                        && match t.status {
                            TaskStatus::Running | TaskStatus::Review | TaskStatus::Validating => true,
                            TaskStatus::Failed => t.report.is_some(),
                            _ => false,
                        }
                })
                .map(|t| t.worktree.clone())
        })
    }

    pub(super) fn task_or_404(&self, project: &str, id: &str) -> Result<Task, TaskError> {
        self.task(project, id).ok_or_else(|| TaskError::NotFound(format!("tarefa \"{}\" não encontrada neste projeto", id.escape_debug())))
    }

    /// Altera uma tarefa existente e grava. `None` se ela sumiu.
    pub(super) fn update_task<R>(&self, project: &str, id: &str, f: impl FnOnce(&mut Task) -> R) -> Option<R> {
        self.with_tasks(project, true, |p| p.task_mut(id).map(f))
    }

    /// Marca a tarefa como `Failed` com a mensagem e devolve o erro para quem chamou.
    pub(super) fn fail_task(&self, project: &str, id: &str, message: String) -> TaskError {
        self.update_task(project, id, |t| {
            t.status = TaskStatus::Failed;
            t.error = Some(message.clone());
            t.log(format!("erro: {message}"));
        });
        TaskError::Failed(message)
    }

    /// A mesclagem falhou por motivo externo (não conflito): o erro vai para o histórico e para quem chamou, e a
    /// tarefa fica (ou volta) em `Review`, onde ainda se pode mesclar de novo, devolver ou descartar. `Failed` aqui
    /// seria um beco: a mesa recusava devolver e concluir.
    pub(super) fn fail_merge(&self, project: &str, id: &str, message: String) -> TaskError {
        self.update_task(project, id, |t| {
            t.status = TaskStatus::Review;
            t.error = None;
            t.log(format!("erro: {message}"));
        });
        TaskError::Failed(message)
    }

    // ---------- Git ----------

    /// O repositório do projeto (a raiz dele, mesmo que o projeto seja uma subpasta).
    pub(super) fn task_repo(&self, project: &str) -> Result<Repo, TaskError> {
        let path = self.project_path(project).ok_or_else(|| TaskError::NotFound(format!("projeto desconhecido: {project}")))?;
        self.git_repo_at(&path).map_err(|e| match e {
            GitError::NotARepo => {
                TaskError::Invalid("o projeto não é um repositório git; as tarefas precisam de um (rode git init)".into())
            }
            other => git_error(other),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::super::tests::{entry, sample};
    use super::*;

    #[test]
    fn the_timeline_round_trips_through_the_file_and_old_files_still_load() {
        let tmp = tempfile::tempdir().expect("tmp");
        let file = tmp.path().join("p1.json");
        let mut t = Task { instance: "claude@t3".into(), ..sample() };
        t.record_message(&entry(FeedKind::Ask, Some("claude@t3"), Some("antigravity"), "pergunta"));
        save_file(&file, &ProjectTasks { next: 4, tasks: vec![t.clone()], ..ProjectTasks::default() }).expect("save");
        assert_eq!(load_file(&file).tasks[0].messages, t.messages);
        // Um arquivo de antes da linha do tempo (sem `messages`) abre com ela vazia.
        std::fs::write(&file, r#"{"version":1,"next":4,"tasks":[{"id":"t3","instance":"claude@t3"}]}"#).expect("write");
        assert!(load_file(&file).tasks[0].messages.is_empty());
    }

    #[test]
    fn the_counter_never_goes_back() {
        let mut p = ProjectTasks { next: 2, tasks: vec![Task { id: "t7".into(), ..Task::default() }], ..ProjectTasks::default() };
        p.fix_counter();
        assert_eq!(p.next, 8);
        let mut p = ProjectTasks { next: 40, ..ProjectTasks::default() };
        p.fix_counter();
        assert_eq!(p.next, 40);
        let mut p = ProjectTasks::default();
        p.fix_counter();
        assert_eq!(p.next, 1);
    }

    #[test]
    fn the_file_round_trips_and_an_unreadable_one_is_kept() {
        let tmp = tempfile::tempdir().expect("tmp");
        let file = tmp.path().join("tarefas").join("p1.json");
        assert!(load_file(&file).tasks.is_empty());
        let mut p = ProjectTasks { next: 5, tasks: vec![sample()], ..ProjectTasks::default() };
        p.fix_counter();
        save_file(&file, &p).expect("save");
        save_file(&file, &p).expect("save de novo por cima");
        let back = load_file(&file);
        assert_eq!((back.next, back.tasks.len()), (5, 1));
        assert_eq!(back.tasks[0], sample());
        let text = std::fs::read_to_string(&file).expect("ler");
        assert!(text.contains("\"status\": \"draft\"") && text.contains("\"created_ms\""), "{text}");
        assert!(!file.with_extension("json.tmp").exists());
        // Ilegível: vem vazio, mas o original fica guardado ao lado.
        std::fs::write(&file, "{ isto não é json").expect("write");
        assert!(load_file(&file).tasks.is_empty());
        let kept = std::fs::read_dir(file.parent().expect("pasta"))
            .expect("dir")
            .flatten()
            .any(|e| e.file_name().to_string_lossy().contains("ilegivel"));
        assert!(kept, "a cópia do arquivo ilegível deveria ficar");
    }

    #[test]
    fn project_ids_are_file_names() {
        assert!(safe_project_id("p1a2b3c4d"));
        for bad in ["", "../x", "a/b", "a\\b", "a b", "x.json"] {
            assert!(!safe_project_id(bad), "{bad}");
        }
    }
}
