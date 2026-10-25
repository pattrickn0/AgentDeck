use super::git_ops::{
    align_with_base, changed_files, git_out, safe_worktree, short_hash, untracked_clash, valid_task_branch, worktree_dirty,
};
use super::stash::{stash_pop, stash_push, Stash};
use super::text::short_list;
use super::{dirty_paths, git_error, MergeBlock, MergeOutcome, MergeRefusal, Task, TaskError, TaskStatus, AGENTDECK_DIR};
use crate::events::{CoreEvent, FeedEntry};
use crate::git::{run, RepoState, COMMIT_TIMEOUT};
use crate::util::{self, now_ms};
use crate::Core;

impl Core {
    // ---------- Mesclar ----------

    /// Mescla a branch da tarefa na base (`git merge --no-ff`) no repositório principal e põe a tarefa em
    /// `Validating`: a aba, a worktree e a branch ficam (o usuário ainda vai testar) e a branch da tarefa é
    /// alinhada com a base já mesclada (`git merge --ff-only`). Exige `Review` (ou `Failed` com relatório,
    /// para tentar de novo). Recusa, sem mudar nada, se o repositório principal não está na base ou tem alterações
    /// sem commit em arquivos rastreados (o `AGENTS.md`, que o app reescreve, não conta), ou se a worktree
    /// da tarefa tem trabalho sem commit. Em conflito, desfaz a mesclagem (`merge --abort`) e a tarefa continua em
    /// `Review`, com os arquivos em conflito na mensagem. Se o git falha por outro motivo, a tarefa também fica em
    /// `Review`, com o erro no histórico e devolvido a quem chamou. Se o `AGENTS.md` da pasta principal só difere
    /// pelo bloco do app e a branch o altera, ele volta ao do HEAD durante a mesclagem e o bloco é regravado depois.
    pub fn task_merge(&self, project: &str, id: &str) -> Result<Task, TaskError> {
        self.merge_task(project, id, false).map(|o| o.task).map_err(|r| r.error)
    }

    /// Como [`Core::task_merge`], para a interface: a recusa que tem saída vem com o motivo estruturado
    /// ([`MergeRefusal::block`], para a tela oferecer o botão), e `stash_main` liga o "guardar minhas alterações,
    /// mesclar e devolvê-las": as alterações sem commit do repositório principal vão para um `git stash`
    /// (identificado pela tarefa) e voltam (`stash pop`) depois da mesclagem, qualquer que seja o desfecho. Se
    /// não voltam limpas (conflito com o que a tarefa trouxe), ficam no stash, intactas, e o aviso diz como
    /// recuperá-las. Só o clique do usuário liga isso: a linha de comando (`task_merge`) nunca guarda nada.
    pub fn task_merge_assisted(&self, project: &str, id: &str, stash_main: bool) -> Result<MergeOutcome, MergeRefusal> {
        self.merge_task(project, id, stash_main)
    }

    fn merge_task(&self, project: &str, id: &str, stash_main: bool) -> Result<MergeOutcome, MergeRefusal> {
        let _op = self.inner.tasks.op.lock();
        let task = self.task_or_404(project, id)?;
        let retry = task.status == TaskStatus::Failed && task.report.is_some();
        if task.status != TaskStatus::Review && !retry {
            return Err(TaskError::Conflict(format!(
                "a tarefa {} está {}: só se mescla uma tarefa em revisão",
                task.id,
                task.status.label()
            ))
            .into());
        }
        let repo = self.task_repo(project)?;
        let refuse_with = |t: &Task, msg: String, block: Option<MergeBlock>| -> MergeRefusal {
            self.update_task(project, &t.id, |t| t.log(format!("mesclagem recusada: {msg}")));
            MergeRefusal { error: TaskError::Conflict(msg), block, stash_note: None }
        };
        let refuse = |t: &Task, msg: String| -> MergeRefusal { refuse_with(t, msg, None) };
        let expected = util::absolutize(&repo.root.join(AGENTDECK_DIR).join("worktrees"));
        if !valid_task_branch(&task.branch) || !crate::git::valid_branch_name(&task.base) || !safe_worktree(&task.worktree, &expected) {
            return Err(TaskError::Failed(format!("dados da tarefa {} fora do esperado; não mesclei nada", task.id)).into());
        }
        let _git = self.inner.git.op.lock();

        // O repositório principal precisa estar pronto para receber a mesclagem.
        match repo.read(&["symbolic-ref", "--short", "-q", "HEAD"]) {
            Ok(out) if String::from_utf8_lossy(&out.stdout).trim() == task.base => {}
            Ok(out) => {
                let now = String::from_utf8_lossy(&out.stdout).trim().to_owned();
                return Err(refuse(
                    &task,
                    format!(
                        "o repositório principal está na branch \"{now}\", não em \"{base}\": troque para \"{base}\" e tente de novo",
                        base = task.base
                    ),
                ));
            }
            Err(_) => {
                return Err(refuse(
                    &task,
                    format!("o repositório principal está com o HEAD solto: troque para \"{}\" e tente de novo", task.base),
                ))
            }
        }
        if repo.state() != RepoState::Normal {
            return Err(refuse(
                &task,
                "o repositório principal está no meio de outra operação (mesclagem, rebase…): termine ou aborte antes".into(),
            ));
        }
        let status = git_out(&repo, &["status", "--porcelain=v1", "--untracked-files=no"]).map_err(git_error)?;
        // O app reescreve o bloco do AGENTS.md; o git protege o arquivo se a mesclagem o tocar.
        let dirty: Vec<String> = dirty_paths(&status).into_iter().filter(|p| p != "AGENTS.md").collect();
        if !dirty.is_empty() && !stash_main {
            return Err(refuse_with(
                &task,
                format!(
                    "o repositório principal tem alterações sem commit ({}): faça o commit ou guarde-as e tente de novo",
                    short_list(&dirty)
                ),
                Some(MergeBlock::MainDirty(dirty)),
            ));
        }
        if let Some(d) = worktree_dirty(&repo, &task.worktree).filter(|d| !d.is_empty()) {
            return Err(refuse_with(
                &task,
                format!(
                    "a worktree da tarefa tem trabalho sem commit ({}): devolva a tarefa pedindo o commit, ou descarte-a",
                    short_list(&d)
                ),
                Some(MergeBlock::TaskDirty(d)),
            ));
        }
        if repo.read(&["rev-parse", "--verify", "-q", &format!("refs/heads/{}", task.branch)]).is_err() {
            return Err(refuse(&task, format!("a branch {} não existe mais: descarte a tarefa", task.branch)));
        }
        let ahead = git_out(&repo, &["rev-list", "--count", &format!("{}..{}", task.base, task.branch)]).map_err(git_error)?;
        if ahead.trim() == "0" {
            // Reprovada e entregue sem commit novo: o agente concluiu que não havia o que mudar. O que a
            // última mesclagem trouxe já está na base; volta a esperar a validação, sem novo commit de mesclagem.
            if let Some(last) = task.merge_commit.as_deref() {
                let short = short_hash(last).to_owned();
                let updated = self.update_task(project, &task.id, |t| {
                    t.status = TaskStatus::Validating;
                    t.finished_ms = Some(now_ms());
                    t.error = None;
                    t.log(format!("sem commits novos desde a última mesclagem ({short}); aguardando validação de novo"));
                    t.clone()
                });
                let task = updated.ok_or_else(|| TaskError::NotFound(format!("tarefa {id} sumiu")))?;
                self.feed(
                    FeedEntry::system(format!("Tarefa {} sem commits novos, aguardando validação de novo: {}", task.id, task.title))
                        .in_project(project),
                );
                return Ok(MergeOutcome { task, note: None });
            }
            return Err(refuse(
                &task,
                format!(
                    "a branch {} não tem commits novos em relação a \"{}\": não há o que mesclar (descarte a tarefa)",
                    task.branch, task.base
                ),
            ));
        }
        if let Err(e) = repo.check_gpg_ready() {
            return Err(refuse(&task, e.to_string()));
        }
        // Arquivo novo (não rastreado) do usuário que a branch também cria: o git recusaria no meio da mesclagem. Vale
        // com ou sem "guardar": o stash não leva arquivos não rastreados, e aqui ainda não se guardou nada.
        let clash = untracked_clash(&repo, &task.base, &task.branch);
        if !clash.is_empty() {
            return Err(refuse(
                &task,
                format!(
                    "o repositório principal tem arquivos novos seus, sem commit, que a tarefa também cria ({}): mova-os para outra pasta (ou apague-os) e tente de novo",
                    short_list(&clash)
                ),
            ));
        }

        // "Guardar minhas alterações": o que está sem commit vai para o stash (menos o AGENTS.md, que o app
        // trata à parte, abaixo). Daqui em diante todo desfecho passa por `give_back`, que devolve o stash.
        let mut stashed: Option<Stash> = None;
        if stash_main && !dirty.is_empty() {
            match stash_push(&repo, &task) {
                Ok(s) => stashed = s,
                Err(e) => {
                    return Err(refuse(
                        &task,
                        format!("não consegui guardar as alterações do repositório principal ({e}); nada foi mudado"),
                    ))
                }
            }
            if stashed.is_some() {
                self.update_task(project, &task.id, |t| {
                    t.log(format!("guardei {} arquivo(s) sem commit do repositório principal no stash", dirty.len()))
                });
            }
        }
        let give_back = || -> Option<String> {
            let stash = stashed.as_ref()?;
            let (note, kept) = stash_pop(&repo, stash);
            self.update_task(project, &task.id, |t| {
                t.log(format!("alterações guardadas: {}", if kept { "continuam no stash" } else { "devolvidas" }))
            });
            Some(note)
        };

        // O app reescreve o bloco do AGENTS.md da pasta principal, e o git recusa mesclar sobre um arquivo
        // alterado que a branch também mexe. Se a alteração é só o bloco do app, o arquivo volta ao do HEAD
        // para a mesclagem e o bloco é regravado depois (com sucesso, conflito desfeito ou falha). Se o usuário
        // editou o texto fora do bloco, nada é descartado: recusa.
        let mut app_block: Option<String> = None;
        let agents_dirty = dirty_paths(&status).iter().any(|p| p == "AGENTS.md");
        if agents_dirty && changed_files(&repo, &task.base, &task.branch).iter().any(|f| f == "AGENTS.md") {
            let file = repo.root.join("AGENTS.md");
            let current = std::fs::read_to_string(&file).unwrap_or_default();
            let committed = git_out(&repo, &["show", "HEAD:AGENTS.md"]).unwrap_or_default();
            if !crate::workspace::only_marker_blocks_differ(&committed, &current) {
                let note = give_back();
                return Err(refuse(
                    &task,
                    with_note(
                        format!(
                            "o AGENTS.md do repositório principal tem alterações suas sem commit, e a branch {} também mexe nele: faça o commit ou guarde-as e tente de novo",
                            task.branch
                        ),
                        note,
                    ),
                ));
            }
            if let Err(e) = repo.write(&["checkout", "--", "AGENTS.md"], None) {
                let note = give_back();
                return Err(refuse(
                    &task,
                    with_note(format!("não consegui preparar o AGENTS.md do repositório principal para a mesclagem: {e}"), note),
                ));
            }
            app_block = Some(current);
        }
        let restore_block = || {
            let Some(saved) = &app_block else { return };
            if let Err(e) = crate::workspace::restore_marker_block(&repo.root.join("AGENTS.md"), saved) {
                log::warn!("falha ao regravar o bloco do AGENTS.md depois da mesclagem: {e}");
                self.update_task(project, &task.id, |t| t.log(format!("aviso: não consegui regravar o bloco do app no AGENTS.md: {e}")));
            }
        };

        // A mesclagem.
        let message = format!("Merge tarefa {}: {}", task.id, task.title);
        if let Err(e) = run(repo.cmd(&["merge", "--no-ff", "-m", &message, &task.branch]), None, COMMIT_TIMEOUT) {
            if repo.state() == RepoState::Merging {
                let files = git_out(&repo, &["diff", "--name-only", "--diff-filter=U"]).unwrap_or_default();
                let files: Vec<String> = files.lines().map(str::trim).filter(|l| !l.is_empty()).map(str::to_owned).collect();
                return match repo.write(&["merge", "--abort"], None) {
                    Ok(_) => {
                        restore_block();
                        let note = give_back();
                        let msg = format!(
                            "conflito ao mesclar {} em \"{}\" (arquivos: {}); a mesclagem foi desfeita e a tarefa continua em revisão. Devolva-a ao agente pedindo para atualizar a branch com a base, ou resolva à mão",
                            task.branch,
                            task.base,
                            if files.is_empty() { "não identificados".to_owned() } else { short_list(&files) }
                        );
                        self.update_task(project, &task.id, |t| t.log(format!("conflito na mesclagem ({}); desfeita", short_list(&files))));
                        Err(MergeRefusal {
                            error: TaskError::Conflict(with_note(msg, note.clone())),
                            block: Some(MergeBlock::Conflict(files)),
                            stash_note: note,
                        })
                    }
                    // Sem conseguir desfazer, a pasta está no meio da mesclagem: o stash não é devolvido por cima.
                    Err(abort) => Err(self
                        .fail_merge(
                            project,
                            &task.id,
                            format!(
                                "conflito ao mesclar e não consegui desfazer ({abort}): rode git merge --abort em {}{}",
                                repo.root.display(),
                                if stashed.is_some() {
                                    "; as suas alterações sem commit continuam guardadas (git stash list)"
                                } else {
                                    ""
                                }
                            ),
                        )
                        .into()),
                };
            }
            restore_block();
            let note = give_back();
            return Err(self.fail_merge(project, &task.id, with_note(format!("a mesclagem falhou: {e}"), note)).into());
        }
        restore_block();
        let note = give_back();
        let full_head = git_out(&repo, &["rev-parse", "HEAD"]).map(|s| s.trim().to_owned()).unwrap_or_default();
        let head = short_hash(&full_head).to_owned();

        // A branch da tarefa passa a ser a própria base: se for reprovada, o agente continua dali.
        let aligned = align_with_base(&repo, &task);
        let merged = self.update_task(project, &task.id, |t| {
            t.status = TaskStatus::Validating;
            t.finished_ms = Some(now_ms());
            t.error = None;
            t.merge_commit = Some(full_head.clone()).filter(|h| !h.is_empty());
            t.log(format!("mesclada em {} ({head}); aguardando validação", t.base));
            if let Err(e) = &aligned {
                t.log(format!("aviso: não consegui alinhar a branch {} com {}: {e}", t.branch, t.base));
            }
            t.clone()
        });
        let task = merged.ok_or_else(|| TaskError::NotFound(format!("tarefa {id} sumiu")))?;
        self.emit(CoreEvent::GitChanged);
        self.feed(
            FeedEntry::system(format!("Tarefa {} mesclada em {}, aguardando validação: {}", task.id, task.base, task.title))
                .in_project(project),
        );
        // A tarefa deixou de segurar o escopo: quem esperava por ela parte da base já com esta mesclagem.
        self.advance_queue_locked(project);
        Ok(MergeOutcome { task, note })
    }
}

/// A mensagem seguida do aviso do stash, se houve.
fn with_note(msg: String, note: Option<String>) -> String {
    match note {
        Some(n) => format!("{msg} {n}"),
        None => msg,
    }
}
