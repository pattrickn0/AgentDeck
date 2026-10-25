use super::git_ops::{align_with_base, branch_exists, delete_branch, git_out, safe_worktree, valid_task_branch, worktree_dirty};
use super::lifecycle::queue_holder;
use super::text::{cap_text, short_list};
use super::{git_error, reject_message, Task, TaskError, TaskStatus, AGENTDECK_DIR, MAX_TEXT, REMOVE_TRIES, REMOVE_WAIT};
use crate::events::{CoreEvent, FeedEntry};
use crate::git::{run, Repo, COMMIT_TIMEOUT};
use crate::util::{self, now_ms};
use crate::Core;

impl Core {
    // ---------- Validar ----------

    /// O usuário testou e aprovou: fecha a aba, remove a worktree e a branch e marca a tarefa como
    /// concluída (`Merged`). Exige `Validating`. Recusa, sem mudar nada, se a worktree tem trabalho sem
    /// commit ou se a branch tem commits que a base não tem (o agente seguiu trabalhando): reprove a
    /// tarefa ou descarte-a.
    pub fn task_validate(&self, project: &str, id: &str) -> Result<Task, TaskError> {
        let _op = self.inner.tasks.op.lock();
        let task = self.task_or_404(project, id)?;
        if task.status != TaskStatus::Validating {
            return Err(TaskError::Conflict(format!(
                "a tarefa {} está {}: só se valida uma tarefa aguardando validação",
                task.id,
                task.status.label()
            )));
        }
        let repo = self.task_repo(project)?;
        let expected = util::absolutize(&repo.root.join(AGENTDECK_DIR).join("worktrees"));
        if !valid_task_branch(&task.branch) || !crate::git::valid_branch_name(&task.base) || !safe_worktree(&task.worktree, &expected) {
            return Err(TaskError::Failed(format!("dados da tarefa {} fora do esperado; não removi nada", task.id)));
        }
        let _git = self.inner.git.op.lock();
        let refuse = |msg: String| -> TaskError {
            self.update_task(project, &task.id, |t| t.log(format!("validação recusada: {msg}")));
            TaskError::Conflict(msg)
        };
        if let Some(d) = worktree_dirty(&repo, &task.worktree).filter(|d| !d.is_empty()) {
            return Err(refuse(format!(
                "a worktree da tarefa tem trabalho sem commit ({}): reprove a tarefa pedindo o commit, ou descarte-a",
                short_list(&d)
            )));
        }
        let mut warnings = Vec::new();
        if branch_exists(&repo, &task.branch) {
            if branch_exists(&repo, &task.base) {
                let ahead = git_out(&repo, &["rev-list", "--count", &format!("{}..{}", task.base, task.branch)]).map_err(git_error)?;
                if ahead.trim() != "0" {
                    return Err(refuse(format!(
                        "a branch {} tem commits que ainda não foram mesclados em \"{}\": reprove a tarefa para o agente entregá-los, ou descarte-a",
                        task.branch, task.base
                    )));
                }
            } else {
                warnings.push(format!("a base {} não existe mais; não conferi se a branch tinha commits fora dela", task.base));
            }
        }

        // Limpeza: aba, worktree, branch (o que ela trouxe já está na base).
        self.close_task_instance(project, &task);
        warnings.extend(self.remove_task_worktree(&repo, &task));
        if task.worktree.exists() {
            // Algo ainda segura a pasta: concluir agora deixaria pasta e branch órfãs, sem como limpar depois.
            let why = warnings.join("; ");
            self.update_task(project, &task.id, |t| t.log(format!("validação incompleta: {why}")));
            return Err(TaskError::Conflict(format!(
                "não consegui apagar a pasta da worktree ({why}): feche o que a usa e valide de novo"
            )));
        }
        if let Err(e) = delete_branch(&repo, &task.branch, true) {
            warnings.push(format!("a branch {} ficou: {e}", task.branch));
        }
        let done = self.update_task(project, &task.id, |t| {
            t.status = TaskStatus::Merged;
            t.finished_ms = Some(now_ms());
            t.error = None;
            t.log("validada; concluída; worktree e branch removidas");
            for w in &warnings {
                t.log(format!("aviso: {w}"));
            }
            t.clone()
        });
        let task = done.ok_or_else(|| TaskError::NotFound(format!("tarefa {id} sumiu")))?;
        self.emit(CoreEvent::GitChanged);
        self.feed(FeedEntry::system(format!("Tarefa {} concluída: {}", task.id, task.title)).in_project(project));
        Ok(task)
    }

    // ---------- Reprovar ----------

    /// O usuário testou e reprovou: `Validating` volta a `Running` na mesma worktree (que já tem o código da
    /// base) e o motivo é entregue ao mesmo agente (a aba é reaberta se foi fechada). Se a pasta da worktree
    /// sumiu nesse meio-tempo, é recriada a partir da branch (ou da base). O ciclo segue: done → revisão →
    /// mesclar → aguardando validação.
    pub fn task_reject(&self, project: &str, id: &str, feedback: &str) -> Result<Task, TaskError> {
        let _op = self.inner.tasks.op.lock();
        let task = self.task_or_404(project, id)?;
        if task.status != TaskStatus::Validating {
            return Err(TaskError::Conflict(format!(
                "a tarefa {} está {}: só se reprova uma tarefa aguardando validação",
                task.id,
                task.status.label()
            )));
        }
        let feedback = feedback.trim();
        if feedback.is_empty() {
            return Err(TaskError::Invalid("o motivo está vazio: diga o que não funcionou no teste".into()));
        }
        if !self.tab_delivery() {
            return Err(TaskError::Conflict(
                "as tarefas rodam nas abas dos agentes: ligue \"Abrir abas dos agentes\" nas Configurações".into(),
            ));
        }
        let feedback = cap_text(feedback, MAX_TEXT);
        let aligned = {
            let repo = self.task_repo(project)?;
            let _git = self.inner.git.op.lock();
            self.restore_worktree(&repo, &task)?;
            align_with_base(&repo, &task)
        };
        let updated = self.update_task(project, &task.id, |t| {
            t.status = TaskStatus::Running;
            t.finished_ms = None;
            t.log("reprovada na validação; devolvida ao agente com o motivo");
            if let Err(e) = &aligned {
                t.log(format!("aviso: não consegui alinhar a branch {} com {}: {e}", t.branch, t.base));
            }
            // `before` não tem o feedback novo: a mensagem mostra só os anteriores.
            let before = t.clone();
            t.add_feedback("validação", &feedback);
            (before, t.clone())
        });
        let (before, task) = updated.ok_or_else(|| TaskError::NotFound(format!("tarefa {id} sumiu")))?;
        // `Validating` não segurava o escopo: se outra tarefa ocupou esse espaço nesse meio-tempo, só avisa.
        if let Some(other) = queue_holder(&self.tasks(project), &task) {
            self.update_task(project, &task.id, |t| {
                t.log(format!("aviso: o escopo se sobrepõe ao da {other}, que está em andamento; a próxima mesclagem pode dar conflito"))
            });
        }
        let ident = self.open_task_instance(project, &task);
        let prompt = reject_message(&before, &feedback, aligned.is_ok(), &self.feedback_learnings(&before, &feedback));
        let boot = self.task_boot_prompt(project, &task, "feedback", "foi reprovada na validacao, com o motivo no arquivo", &prompt);
        self.deliver_to_task(project, &task, ident, prompt, boot, "motivo da reprovação entregue ao agente");
        Ok(task)
    }

    /// Garante a pasta da worktree: se foi apagada à mão, recria a partir da branch da tarefa (ou da base,
    /// se a branch também sumiu). Só mexe em `<raiz>/.agentdeck/worktrees`.
    fn restore_worktree(&self, repo: &Repo, task: &Task) -> Result<(), TaskError> {
        let expected = util::absolutize(&repo.root.join(AGENTDECK_DIR).join("worktrees"));
        if !valid_task_branch(&task.branch) || !crate::git::valid_branch_name(&task.base) || !safe_worktree(&task.worktree, &expected) {
            return Err(TaskError::Failed(format!("dados da tarefa {} fora do esperado; não mexi em nada", task.id)));
        }
        if task.worktree.is_dir() {
            return Ok(());
        }
        let _ = repo.write(&["worktree", "prune"], None);
        let path = task.worktree.to_string_lossy().into_owned();
        let args: Vec<&str> = if branch_exists(repo, &task.branch) {
            vec!["worktree", "add", &path, &task.branch]
        } else {
            vec!["worktree", "add", "-b", &task.branch, &path, &task.base]
        };
        run(repo.cmd(&args), None, COMMIT_TIMEOUT)
            .map(|_| ())
            .map_err(|e| TaskError::Failed(format!("a pasta da worktree sumiu e não consegui recriá-la: {e}")))
    }

    // ---------- Descartar ----------

    /// Descarta a tarefa (qualquer estado que não seja final): fecha a aba, remove a worktree (à força,
    /// mesmo se a pasta já sumiu) e a branch. Funciona sem o repositório: aí só o registro muda. Em
    /// `Validating` o que já foi mesclado continua na base: descartar não desfaz o merge.
    pub fn task_discard(&self, project: &str, id: &str) -> Result<Task, TaskError> {
        let _op = self.inner.tasks.op.lock();
        let task = self.task_or_404(project, id)?;
        if task.status.is_terminal() {
            return Err(TaskError::Conflict(format!("a tarefa {} já está {}", task.id, task.status.label())));
        }
        let was_validating = task.status == TaskStatus::Validating;
        let mut warnings = Vec::new();
        if task.started_ms.is_some() {
            self.close_task_instance(project, &task);
        }
        match self.task_repo(project) {
            Ok(repo) => {
                let _git = self.inner.git.op.lock();
                warnings.extend(self.remove_task_worktree(&repo, &task));
                if valid_task_branch(&task.branch) {
                    if let Err(e) = delete_branch(&repo, &task.branch, true) {
                        warnings.push(format!("a branch {} ficou: {e}", task.branch));
                    }
                }
            }
            Err(e) => warnings.push(format!("repositório indisponível ({e}): só o registro da tarefa mudou")),
        }
        let updated = self.update_task(project, &task.id, |t| {
            t.status = TaskStatus::Discarded;
            t.finished_ms = Some(now_ms());
            if was_validating {
                t.log(format!("descartada; worktree e branch removidas (o que já foi mesclado continua em {})", t.base));
            } else {
                t.log("descartada; worktree e branch removidas");
            }
            for w in &warnings {
                t.log(format!("aviso: {w}"));
            }
            t.clone()
        });
        let task = updated.ok_or_else(|| TaskError::NotFound(format!("tarefa {id} sumiu")))?;
        self.emit(CoreEvent::GitChanged);
        self.feed(FeedEntry::system(format!("Tarefa {} descartada: {}", task.id, task.title)).in_project(project));
        self.advance_queue_locked(project);
        Ok(task)
    }

    /// Esquece a instância da tarefa no core e manda a interface fechar a aba dela.
    fn close_task_instance(&self, project: &str, task: &Task) {
        self.close_instance(project, &task.instance);
        self.emit(CoreEvent::CloseInstance { project: project.to_owned(), id: task.instance.clone() });
    }

    /// Remove a worktree da tarefa e as marcas dela no git. Só mexe na pasta se ela estiver mesmo em
    /// `<raiz>/.agentdeck/worktrees` (o JSON pode ter sido editado). Tenta por alguns segundos: no
    /// Windows o terminal do agente ainda segura a pasta logo depois de a aba fechar. Devolve os avisos.
    pub(super) fn remove_task_worktree(&self, repo: &Repo, task: &Task) -> Vec<String> {
        let mut warnings = Vec::new();
        let allowed = util::absolutize(&repo.root.join(AGENTDECK_DIR).join("worktrees"));
        if safe_worktree(&task.worktree, &allowed) {
            let dir = util::absolutize(&task.worktree);
            let path = dir.to_string_lossy().into_owned();
            let mut last = String::new();
            for attempt in 0..REMOVE_TRIES {
                if !dir.exists() {
                    break;
                }
                match repo.write(&["worktree", "remove", "--force", &path], None) {
                    Ok(_) => {}
                    Err(e) => last = e.to_string(),
                }
                if !dir.exists() {
                    break;
                }
                if attempt + 1 < REMOVE_TRIES {
                    std::thread::sleep(REMOVE_WAIT);
                }
            }
            if dir.exists() {
                // O git desistiu (ou removeu só o registro): apaga a pasta, que é nossa.
                if let Err(e) = std::fs::remove_dir_all(&dir) {
                    // O que ficou e quem segura: sem isso não se descobre a causa (visto na `t14`).
                    let left = crate::file_lock::leftovers(&dir).describe(&dir);
                    let git = if last.is_empty() { String::new() } else { format!("{last}; ") };
                    log::warn!("tarefa {}: a worktree {} não saiu ({git}{e}); {left}", task.id, dir.display());
                    warnings.push(format!("a pasta {} ficou ({git}{e}); {left}", dir.display()));
                }
            }
            if !dir.exists() {
                crate::trust::forget(&dir, &self.inner.home);
            }
        } else {
            warnings.push(format!("a pasta {} não está em .agentdeck/worktrees: não mexi nela", task.worktree.display()));
        }
        // Tira do git as worktrees cuja pasta não existe mais (inclusive a que acabou de sumir).
        if let Err(e) = repo.write(&["worktree", "prune"], None) {
            warnings.push(format!("git worktree prune falhou: {e}"));
        }
        warnings
    }
}
