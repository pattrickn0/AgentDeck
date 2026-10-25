use std::path::PathBuf;

use super::git_ops::{
    branch_exists, delete_branch, diff_stat, git_out, safe_worktree, short_hash, union_files, valid_task_branch, worktree_dirty,
};
use super::text::{cap_text, short_list};
use super::{
    boot_line, branch_name, changed_files, clean_title, git_error, memory_block, return_message, start_message, Task, TaskError, TaskSpec,
    TaskStatus, AGENTDECK_DIR, MAX_OUTSIDE, MAX_TEXT,
};
use crate::capability;
use crate::events::{CoreEvent, Delivery, FeedEntry};
use crate::git::{run, Repo, COMMIT_TIMEOUT, WRITE_TIMEOUT};
use crate::instances::Ident;
use crate::util::{self, now_ms};
use crate::{models, Core};

/// O novo conteúdo de `info/exclude` com `/.agentdeck/`, ou `None` se já está lá.
pub fn exclude_with_agentdeck(existing: &str) -> Option<String> {
    if existing.lines().map(str::trim).any(|l| matches!(l, "/.agentdeck/" | "/.agentdeck" | ".agentdeck/" | ".agentdeck")) {
        return None;
    }
    let mut text = existing.to_owned();
    if !text.is_empty() && !text.ends_with('\n') {
        text.push('\n');
    }
    text.push_str("# AgentDeck: worktrees das tarefas\n/.agentdeck/\n");
    Some(text)
}

impl Core {
    // ---------- Criar ----------

    /// Cria uma tarefa em rascunho. Confere o agente, o modelo e o repositório (a branch atual é a
    /// base, se nenhuma for pedida). Roda o git: chame fora da thread da interface.
    pub fn task_create(&self, project: &str, spec: TaskSpec) -> Result<Task, TaskError> {
        let _op = self.inner.tasks.op.lock();
        let title = clean_title(&spec.title)?;
        let brief = cap_text(spec.brief.trim(), MAX_TEXT);
        let kinds = self.inner.agents.peers();

        // Agente: instalado, com aba (nada de `shell`) e sem `@` (o cargo é o id da tarefa).
        let automatic_choice = spec.kind.trim().is_empty();
        let selected = automatic_choice.then(|| capability::select(&self.capability_candidates(&kinds, spec.effort), spec.level)).flatten();
        let asked = match spec.kind.trim().to_lowercase() {
            k if k.is_empty() => selected
                .as_ref()
                .map(|choice| choice.kind.clone())
                .or_else(|| kinds.iter().find(|kind| kind.as_str() == "claude").cloned())
                .or_else(|| kinds.first().cloned())
                .unwrap_or_else(|| "claude".to_owned()),
            k => k,
        };
        if asked.contains('@') {
            return Err(TaskError::Invalid(format!(
                "agente inválido \"{}\": informe só o modelo (claude, antigravity…)",
                asked.escape_debug()
            )));
        }
        let kind = crate::agents::resolve_alias(&asked).to_owned();
        if !kinds.contains(&kind) || Ident::parse(&kind).is_none() {
            return Err(TaskError::Invalid(format!(
                "agente desconhecido ou não instalado \"{}\". Disponíveis: {}",
                asked.escape_debug(),
                kinds.join(", ")
            )));
        }
        // Modelo: o pedido (validado) ou o padrão do cargo; só quem aceita escolher.
        let requested = spec.model.as_deref().map(str::trim).filter(|m| !m.is_empty());
        if let Some(m) = requested {
            if !models::valid_model(m) {
                return Err(TaskError::Invalid(format!("modelo inválido: \"{}\"", m.chars().take(40).collect::<String>().escape_debug())));
            }
            if !models::accepts_model(&kind) {
                return Err(TaskError::Invalid(format!("o agente {kind} não aceita escolher modelo")));
            }
        }
        let model = models::resolve_model(&Ident::main(&kind), requested, None, &self.config().role_models);
        // Esforço: só onde o CLI o aceita. No Antigravity ele troca o sufixo do modelo, então o modelo gravado
        // já é o que vai abrir.
        let (model, effort) = match spec.effort {
            Some(e) => match models::effort_unavailable(&kind, model.as_deref()) {
                Some(why) => return Err(TaskError::Invalid(format!("esforço não aplicável a {kind}: {why}"))),
                None => (model.map(|m| models::effort_model(&kind, &m, e)), Some(e)),
            },
            None => (model, None),
        };

        // Repositório e base.
        let repo = self.task_repo(project)?;
        if repo.read(&["rev-parse", "--verify", "-q", "HEAD"]).is_err() {
            return Err(TaskError::Invalid(
                "o repositório ainda não tem nenhum commit; faça o primeiro commit antes de criar tarefas".into(),
            ));
        }
        let base = match spec.base.as_deref().map(str::trim).filter(|b| !b.is_empty()) {
            Some(b) => b.to_owned(),
            None => match repo.read(&["symbolic-ref", "--short", "-q", "HEAD"]) {
                Ok(out) => String::from_utf8_lossy(&out.stdout).trim().to_owned(),
                Err(_) => return Err(TaskError::Invalid("o repositório está sem branch (HEAD solto); informe a base da tarefa".into())),
            },
        };
        if !crate::git::valid_branch_name(&base) || repo.read(&["rev-parse", "--verify", "-q", &format!("refs/heads/{base}")]).is_err() {
            return Err(TaskError::Invalid(format!("a base \"{}\" não é uma branch local deste repositório", base.escape_debug())));
        }

        let scope = crate::scope::normalize_all(&spec.scope).map_err(TaskError::Invalid)?;

        let number = self.with_tasks(project, false, |p| p.next.max(1));
        let id = format!("t{number}");
        let instance = Ident::parse(&format!("{kind}@{id}"))
            .map(|i| i.id())
            .ok_or_else(|| TaskError::Invalid(format!("agente \"{kind}\" não pode ter instância de tarefa")))?;
        let mut task = Task {
            branch: branch_name(&id, &title),
            worktree: util::absolutize(&repo.root.join(AGENTDECK_DIR).join("worktrees").join(&id)),
            id: id.clone(),
            title,
            brief,
            status: TaskStatus::Draft,
            scope,
            base,
            level: spec.level,
            kind,
            model,
            effort,
            instance,
            creator: spec.creator.map(|c| c.trim().to_owned()).filter(|c| !c.is_empty()),
            created_ms: now_ms(),
            ..Task::default()
        };
        task.log(match &task.creator {
            Some(c) => format!("criada por {c}"),
            None => "criada".to_owned(),
        });
        if automatic_choice {
            if let Some(choice) = selected {
                task.log(format!("agente escolhido automaticamente pela capacidade e pela cota: {}", choice.reason));
                if let Some(warning) = choice.warning {
                    task.log(format!("aviso de capacidade: {warning}"));
                }
            } else {
                task.log(format!("agente escolhido automaticamente: {} (sem leitura de cota)", task.kind));
            }
        } else {
            if let Some(note) = self.quota_advisory(&task.kind, &kinds) {
                task.log(format!("aviso de cota: {note}"));
            }
            if let Some(note) = self.capability_advisory(&task.kind, task.model.as_deref(), task.level, task.effort) {
                task.log(format!("aviso de capacidade: {note}"));
            }
        }
        self.with_tasks(project, true, |p| {
            p.next = number + 1;
            p.tasks.push(task.clone());
        });
        Ok(task)
    }

    // ---------- Iniciar ----------

    /// Inicia a tarefa: worktree + branch, instância do agente com o terminal na worktree e o brief
    /// entregue à aba. Vale para `Draft` e para `Failed` sem relatório (nova tentativa). Só com as abas dos
    /// agentes ligadas (tarefas não rodam em headless). Se a tarefa tem escopo e ele se sobrepõe ao de uma tarefa em
    /// andamento ou em revisão (ou de outra que já espera), ela vai para `Queued` em vez de começar (o `Ok` traz a
    /// tarefa já na fila; uma que falhou ao iniciar não entra na fila: recusa com `Conflict`). Aceita também uma
    /// tarefa `Queued`, para conferir de novo. Roda o git: fora da thread da interface.
    pub fn task_start(&self, project: &str, id: &str) -> Result<Task, TaskError> {
        let _op = self.inner.tasks.op.lock();
        let result = self.start_locked(project, id);
        if result.is_err() {
            // Uma tarefa que falhou ao iniciar não segura mais o escopo dela.
            self.advance_queue_locked(project);
        }
        result
    }

    /// Começa a fila que já pode começar, no projeto (ver [`Core::advance_queue_locked`]). A interface chama ao
    /// abrir o projeto: cobre o que ficou pelo caminho se o app fechou entre a tarefa sair e a seguinte começar.
    /// Roda o git: fora da thread da interface.
    pub fn tasks_advance_queue(&self, project: &str) {
        let _op = self.inner.tasks.op.lock();
        self.advance_queue_locked(project);
    }

    /// Passa pela fila do projeto na ordem de chegada e inicia cada tarefa cujo escopo já não se sobrepõe ao de
    /// ninguém que ande à frente; as que continuam bloqueadas só atualizam o "espera por". A base é a de agora:
    /// a worktree sai do tip da base neste momento, já com o que a tarefa anterior mesclou. Quem chama já tem a
    /// trava das operações. Uma falha (aba desligada, git) fica no histórico da tarefa e não para as seguintes.
    pub(super) fn advance_queue_locked(&self, project: &str) {
        let mut queue: Vec<Task> = self.tasks(project).into_iter().filter(|t| t.status == TaskStatus::Queued).collect();
        queue.sort_by_key(queue_key);
        for task in queue {
            if let Err(e) = self.start_locked(project, &task.id) {
                log::warn!("fila: a tarefa {} não começou: {e}", task.id);
                self.update_task(project, &task.id, |t| {
                    if t.status == TaskStatus::Queued {
                        t.log(format!("fila: não consegui começar: {e}"));
                    }
                });
            }
        }
    }

    fn start_locked(&self, project: &str, id: &str) -> Result<Task, TaskError> {
        let task = self.task_or_404(project, id)?;
        let retry = task.status == TaskStatus::Failed && task.report.is_none();
        if !matches!(task.status, TaskStatus::Draft | TaskStatus::Queued) && !retry {
            return Err(TaskError::Conflict(format!(
                "a tarefa {} está {}: só se inicia um rascunho (ou uma tarefa que falhou ao iniciar)",
                task.id,
                task.status.label()
            )));
        }
        if !self.tab_delivery() {
            return Err(TaskError::Conflict(
                "as tarefas rodam nas abas dos agentes: ligue \"Abrir abas dos agentes\" nas Configurações (no modo headless não há aba nem worktree por agente)".into(),
            ));
        }
        if let Some(holder) = queue_holder(&self.tasks(project), &task) {
            if retry {
                return Err(TaskError::Conflict(format!(
                    "o escopo da tarefa {} se sobrepõe ao da {holder}, que está em andamento: tente de novo quando ela terminar",
                    task.id
                )));
            }
            let now = now_ms();
            return self
                .update_task(project, &task.id, |t| {
                    if t.waiting_for.as_deref() != Some(holder.as_str()) {
                        t.log(format!("na fila: o escopo se sobrepõe ao da {holder}; começa quando ela terminar"));
                    }
                    t.status = TaskStatus::Queued;
                    t.queued_ms.get_or_insert(now);
                    t.waiting_for = Some(holder);
                    t.clone()
                })
                .ok_or_else(|| TaskError::NotFound(format!("tarefa {id} sumiu")));
        }
        if let Err(e) = self.prepare_worktree(project, &task, retry) {
            return Err(self.fail_task(project, &task.id, e.message().to_owned()));
        }
        let now = now_ms();
        let started = self.update_task(project, &task.id, |t| {
            t.status = TaskStatus::Running;
            t.started_ms = Some(now);
            t.finished_ms = None;
            t.error = None;
            t.queued_ms = None;
            t.waiting_for = None;
            t.log(format!("iniciada: worktree {} na branch {}", t.worktree.display(), t.branch));
            t.clone()
        });
        let Some(task) = started else { return Err(TaskError::NotFound(format!("tarefa {id} sumiu"))) };
        let ident = self.open_task_instance(project, &task);
        let boss = self.task_boss(project, &task);
        let prompt = start_message(&task, boss.as_deref(), &self.task_memory(project, &task));
        let boot = self.task_boot_prompt(project, &task, "brief", "o pedido completo esta no arquivo", &prompt);
        self.deliver_to_task(project, &task, ident, prompt, boot, "brief entregue ao agente");
        self.feed(FeedEntry::system(format!("Tarefa {} iniciada: {} ({})", task.id, task.title, task.instance)).in_project(project));
        Ok(task)
    }

    /// O bloco de memória do início da tarefa ([`memory_block`]): os aprendizados que valem para o pedido (lidos
    /// da worktree, que já tem a base) e a situação atual. Qualquer falha só deixa a parte de fora: a tarefa
    /// nunca deixa de começar por causa da memória.
    fn task_memory(&self, project: &str, task: &Task) -> String {
        let brief = if task.brief.trim().is_empty() { task.title.as_str() } else { task.brief.as_str() };
        let cited = crate::learnings::paths_in_text(brief, &task.worktree);
        let learnings = crate::learnings::render(&crate::learnings::load(&task.worktree), &cited, crate::learnings::START_BUDGET);
        memory_block(&learnings, &self.task_situation(project, task, &cited))
    }

    /// Os aprendizados específicos dos arquivos que o `feedback` novo cita e que o agente ainda não viu (nem no
    /// pedido, nem nos feedbacks anteriores de `before`), lidos da worktree. O índice e os globais ele já recebeu
    /// no início da tarefa. Vazio se não há arquivo novo ou aprendizado que case.
    pub(super) fn feedback_learnings(&self, before: &Task, feedback: &str) -> String {
        let brief = if before.brief.trim().is_empty() { before.title.as_str() } else { before.brief.as_str() };
        let seen: Vec<String> = std::iter::once(brief)
            .chain(before.feedbacks.iter().map(|f| f.text.as_str()))
            .flat_map(|text| crate::learnings::paths_in_text(text, &before.worktree))
            .collect();
        let fresh: Vec<String> =
            crate::learnings::paths_in_text(feedback, &before.worktree).into_iter().filter(|p| !seen.contains(p)).collect();
        if fresh.is_empty() {
            return String::new();
        }
        crate::learnings::render_specific(&crate::learnings::load(&before.worktree), &fresh, crate::learnings::FEEDBACK_BUDGET)
    }

    /// A "situação atual" ([`crate::situation::render`]): as outras tarefas em andamento do projeto, com os
    /// arquivos que a branch delas já mexeu, e os últimos commits da base. Vazia se não há repositório.
    fn task_situation(&self, project: &str, task: &Task, cited: &[String]) -> String {
        let Ok(repo) = self.task_repo(project) else { return String::new() };
        let others = self.busy_tasks(project, &repo, &task.instance);
        let commits = crate::situation::base_commits(&repo, &task.base);
        crate::situation::render(&others, &commits, &task.base, cited, crate::situation::BUDGET)
    }

    /// Worktree + branch da tarefa: exclui `/.agentdeck/` do git, limpa o que sobrou de uma tentativa
    /// anterior e cria a worktree a partir da base.
    fn prepare_worktree(&self, project: &str, task: &Task, retry: bool) -> Result<(), TaskError> {
        let repo = self.task_repo(project)?;
        self.ensure_exclude(&repo)?;
        let expected = util::absolutize(&repo.root.join(AGENTDECK_DIR).join("worktrees"));
        if !safe_worktree(&task.worktree, &expected) || !valid_task_branch(&task.branch) {
            return Err(TaskError::Failed(format!(
                "dados da tarefa {} fora do esperado (pasta {} / branch {}); não mexi em nada",
                task.id,
                task.worktree.display(),
                task.branch
            )));
        }
        if retry {
            let _ = self.remove_task_worktree(&repo, task);
            let _ = delete_branch(&repo, &task.branch, true);
        }
        if repo.read(&["rev-parse", "--verify", "-q", &format!("refs/heads/{}", task.base)]).is_err() {
            return Err(TaskError::Failed(format!("a base \"{}\" não existe mais neste repositório", task.base)));
        }
        if repo.read(&["rev-parse", "--verify", "-q", &format!("refs/heads/{}", task.branch)]).is_ok() {
            return Err(TaskError::Failed(format!("a branch {} já existe neste repositório; remova-a ou descarte a tarefa", task.branch)));
        }
        if task.worktree.exists() {
            return Err(TaskError::Failed(format!("a pasta {} já existe; remova-a ou descarte a tarefa", task.worktree.display())));
        }
        let path = task.worktree.to_string_lossy().into_owned();
        run(repo.cmd(&["worktree", "add", "-b", &task.branch, &path, &task.base]), None, COMMIT_TIMEOUT)
            .map_err(|e| TaskError::Failed(format!("não consegui criar a worktree: {e}")))?;
        Ok(())
    }

    /// Escreve `/.agentdeck/` em `<git comum>/info/exclude`, uma vez (nunca no `.gitignore`).
    fn ensure_exclude(&self, repo: &Repo) -> Result<(), TaskError> {
        let common = repo
            .read(&["rev-parse", "--path-format=absolute", "--git-common-dir"])
            .ok()
            .map(|o| PathBuf::from(String::from_utf8_lossy(&o.stdout).trim()))
            .filter(|p| !p.as_os_str().is_empty())
            .unwrap_or_else(|| repo.git_dir.clone());
        let file = common.join("info").join("exclude");
        let existing = std::fs::read_to_string(&file).unwrap_or_default();
        if let Some(text) = exclude_with_agentdeck(&existing) {
            std::fs::create_dir_all(common.join("info"))
                .and_then(|_| std::fs::write(&file, text))
                .map_err(|e| TaskError::Failed(format!("não consegui escrever {}: {e}", file.display())))?;
        }
        Ok(())
    }

    /// Registra a instância da tarefa (modelo, chefe) e a devolve; idempotente.
    pub(super) fn open_task_instance(&self, project: &str, task: &Task) -> Ident {
        let ident = Ident::parse(&task.instance).unwrap_or_else(|| Ident::main(&task.kind));
        self.with_instances(project, |list| list.register(ident.clone()));
        self.assign_model(project, &ident, task.model.as_deref());
        if let Some(boss) = self.task_boss(project, task) {
            if let Err(e) = self.set_boss(project, &task.instance, Some(&boss)) {
                log::warn!("chefe da tarefa {}: {e}", task.id);
            }
        }
        ident
    }

    /// Quem criou a tarefa, quando é um agente com aba aberta (o usuário não é chefe de ninguém).
    fn task_boss(&self, project: &str, task: &Task) -> Option<String> {
        let creator = task.creator.as_deref()?;
        (creator != task.instance && self.project_instance_ids(project).iter().any(|o| o == creator)).then(|| creator.to_owned())
    }

    /// Prompt inicial para a abertura do processo da tarefa: grava `prompt` em `dados/tarefas/<projeto>-<id>-<stem>.md`
    /// e devolve a linha curta que manda o agente lê-lo. `None` se o CLI não aceita prompt inicial, se o arquivo
    /// não pôde ser gravado ou se a linha não cabe em ASCII: a entrega fica digitada, como sempre.
    pub(super) fn task_boot_prompt(&self, project: &str, task: &Task, stem: &str, intro: &str, prompt: &str) -> Option<String> {
        models::boot_prompt_flag(Ident::parse(&task.instance)?.kind.as_str())?;
        let dir = self.paths().data_dir.join("tarefas");
        let file = util::absolutize(&dir.join(format!("{project}-{}-{stem}.md", task.id)));
        let file = util::strip_verbatim(&file);
        match std::fs::create_dir_all(&dir).and_then(|_| std::fs::write(&file, prompt)) {
            Ok(()) => boot_line(task, intro, &file),
            Err(e) => {
                log::warn!("não consegui gravar {}: {e}", file.display());
                None
            }
        }
    }

    /// Guarda o prompt inicial da próxima abertura do processo de `instance`: `agents::spawn_spec_for` o leva
    /// na linha de comando (uma vez). A interface chama antes de abrir ou reabrir a aba.
    pub fn set_boot_prompt(&self, project: &str, instance: &str, line: String) {
        self.inner.tasks.boot.lock().insert(format!("{project}/{instance}"), line);
    }

    /// Pega (e esquece) o prompt inicial guardado para `instance`.
    pub fn take_boot_prompt(&self, project: &str, instance: &str) -> Option<String> {
        self.inner.tasks.boot.lock().remove(&format!("{project}/{instance}"))
    }

    /// Manda `prompt` à aba da tarefa (abre a aba se preciso) pelo caminho das mensagens entre agentes. Se a
    /// aba precisar ser aberta e o CLI aceitar um prompt inicial, a interface usa `boot` na linha de comando em
    /// vez de digitar `prompt` (ver [`Delivery::boot`]).
    pub(super) fn deliver_to_task(&self, project: &str, task: &Task, ident: Ident, prompt: String, boot: Option<String>, log: &str) {
        let from = task.creator.clone().unwrap_or_else(|| "usuário".to_owned());
        self.emit(CoreEvent::Deliver(Delivery {
            request: 0,
            project: project.to_owned(),
            ident,
            create: true,
            from,
            prompt,
            answer: false,
            boot,
        }));
        self.update_task(project, &task.id, |t| t.log(log));
    }

    // ---------- Relatório ----------

    /// O agente terminou: guarda o relatório e o `git diff --stat base...branch` e põe a tarefa em
    /// revisão. Exige `Running`, ou `Validating` com commits novos na branch (o usuário corrigiu algo na aba
    /// enquanto esperava a validação e o agente entrega o que falta; sem commits novos, recusa). A ponte só
    /// deixa o próprio agente da tarefa chamar.
    pub fn task_done(&self, project: &str, id: &str, report: &str) -> Result<Task, TaskError> {
        let _op = self.inner.tasks.op.lock();
        let task = self.task_or_404(project, id)?;
        let after_merge = task.status == TaskStatus::Validating;
        if task.status != TaskStatus::Running && !after_merge {
            return Err(TaskError::Conflict(format!(
                "a tarefa {} está {}: só uma tarefa em andamento recebe o relatório",
                task.id,
                task.status.label()
            )));
        }
        if after_merge {
            let repo = self.task_repo(project)?;
            let has_new = valid_task_branch(&task.branch)
                && crate::git::valid_branch_name(&task.base)
                && branch_exists(&repo, &task.branch)
                && git_out(&repo, &["rev-list", "--count", &format!("{}..{}", task.base, task.branch)])
                    .map(|n| n.trim() != "0")
                    .unwrap_or(false);
            if !has_new {
                return Err(TaskError::Conflict(format!(
                    "a tarefa {} já foi mesclada e espera a validação do usuário; não há commits novos para entregar. Se algo ainda falta, quem decide é o usuário (Reprovar)",
                    task.id
                )));
            }
        }
        let report = report.trim();
        if report.is_empty() {
            return Err(TaskError::Invalid("o relatório está vazio: diga o que foi feito, os arquivos, os testes e o que falta".into()));
        }
        let report = cap_text(report, MAX_TEXT);
        let (stat, warning, outside) = match self.task_repo(project) {
            Ok(repo) => {
                let dirty = worktree_dirty(&repo, &task.worktree).filter(|d| !d.is_empty());
                let outside = match task.scope.is_empty() {
                    true => Vec::new(),
                    false => {
                        crate::scope::outside(&task.scope, &union_files(changed_files(&repo, &task.base, &task.branch), dirty.clone()))
                    }
                };
                let warning = dirty.map(|d| {
                    format!(
                        "atenção: há alterações sem commit na worktree ({}); o agente precisa fazer o commit antes da mesclagem",
                        short_list(&d)
                    )
                });
                (diff_stat(&repo, &task.base, &task.branch), warning, outside)
            }
            Err(e) => (Err(e.message().to_owned()), None, Vec::new()),
        };
        let updated = self.update_task(project, &task.id, |t| {
            t.report = Some(report);
            match &stat {
                Ok(s) => t.diff_stat = Some(s.clone()),
                Err(e) => t.log(format!("não consegui o diff: {e}")),
            }
            if let Some(w) = &warning {
                t.log(w.clone());
            }
            if !outside.is_empty() {
                t.log(format!("mexeu fora do escopo: {}", short_list(&outside)));
            }
            t.outside_scope = outside.into_iter().take(MAX_OUTSIDE).collect();
            t.status = TaskStatus::Review;
            t.finished_ms = Some(now_ms());
            t.log(if after_merge {
                "o agente entregou mais trabalho depois da mesclagem; em revisão"
            } else {
                "relatório recebido; em revisão"
            });
            t.clone()
        });
        let task = updated.ok_or_else(|| TaskError::NotFound(format!("tarefa {id} sumiu")))?;
        self.feed(FeedEntry::system(format!("Tarefa {} pronta para revisão: {}", task.id, task.title)).in_project(project));
        Ok(task)
    }

    // ---------- Diff ----------

    /// O diff completo da tarefa (`git diff base...branch`, só o que a branch trouxe), no mesmo formato
    /// do diff da aba Git, para a interface mostrar. Em `Validating` e `Merged` com `merge_commit`, é o que a
    /// mesclagem trouxe (`git diff <hash>^1 <hash>`): depois do alinhamento a branch é a própria base e o
    /// diff por intervalo ficaria vazio. Só leitura: não toma a trava das operações. Falha com `Conflict` se a
    /// branch já não existe (tarefa descartada) ou se o commit da mesclagem sumiu (histórico reescrito).
    pub fn task_diff(&self, project: &str, id: &str) -> Result<crate::git::GitDiff, TaskError> {
        let task = self.task_or_404(project, id)?;
        let repo = self.task_repo(project)?;
        if !valid_task_branch(&task.branch) || !crate::git::valid_branch_name(&task.base) {
            return Err(TaskError::Failed(format!("dados da tarefa {} fora do esperado; não li o diff", task.id)));
        }
        let merged = matches!(task.status, TaskStatus::Validating | TaskStatus::Merged);
        let args: Vec<String> = if let Some(hash) = task.merge_commit.as_deref().filter(|_| merged) {
            let well_formed = !hash.is_empty() && hash.len() <= 64 && hash.bytes().all(|b| b.is_ascii_hexdigit());
            if !well_formed || repo.read(&["rev-parse", "--verify", "-q", &format!("{hash}^{{commit}}")]).is_err() {
                return Err(TaskError::Conflict(format!(
                    "o commit da mesclagem ({}) não existe mais no repositório (o histórico foi reescrito?): não há diff para mostrar",
                    short_hash(hash)
                )));
            }
            let first_parent = format!("{hash}^1");
            ["diff", "--no-color", "--no-ext-diff", "-M", first_parent.as_str(), hash, "--"].map(str::to_owned).to_vec()
        } else {
            if !branch_exists(&repo, &task.branch) {
                return Err(TaskError::Conflict(format!("a branch {} não existe mais: não há diff para mostrar", task.branch)));
            }
            let range = format!("{}...{}", task.base, task.branch);
            ["diff", "--no-color", "--no-ext-diff", "-M", range.as_str(), "--"].map(str::to_owned).to_vec()
        };
        let args: Vec<&str> = args.iter().map(String::as_str).collect();
        let out = run(repo.cmd(&args), None, WRITE_TIMEOUT).map_err(git_error)?;
        let cut = out.stdout.len() > crate::git::DIFF_MAX_BYTES;
        let bytes = &out.stdout[..out.stdout.len().min(crate::git::DIFF_MAX_BYTES)];
        let mut diff = crate::git::parse_diff(&String::from_utf8_lossy(bytes), crate::git::DIFF_MAX_LINES);
        diff.truncated |= cut;
        Ok(diff)
    }

    // ---------- Devolver ----------

    /// Revisão pediu ajustes: `Review` (ou `Failed` com relatório, de uma mesclagem que falhou antes) volta a
    /// `Running` e o feedback é entregue ao agente (a aba é reaberta se foi fechada).
    pub fn task_return(&self, project: &str, id: &str, feedback: &str) -> Result<Task, TaskError> {
        let _op = self.inner.tasks.op.lock();
        let task = self.task_or_404(project, id)?;
        // `Failed` com relatório é uma mesclagem que falhou numa versão anterior (hoje fica em `Review`).
        if task.status != TaskStatus::Review && !(task.status == TaskStatus::Failed && task.report.is_some()) {
            return Err(TaskError::Conflict(format!(
                "a tarefa {} está {}: só se devolve uma tarefa em revisão",
                task.id,
                task.status.label()
            )));
        }
        let feedback = feedback.trim();
        if feedback.is_empty() {
            return Err(TaskError::Invalid("o feedback está vazio: diga o que precisa mudar".into()));
        }
        if !self.tab_delivery() {
            return Err(TaskError::Conflict(
                "as tarefas rodam nas abas dos agentes: ligue \"Abrir abas dos agentes\" nas Configurações".into(),
            ));
        }
        let feedback = cap_text(feedback, MAX_TEXT);
        let updated = self.update_task(project, &task.id, |t| {
            t.status = TaskStatus::Running;
            t.finished_ms = None;
            t.error = None;
            t.log("devolvida ao agente com feedback");
            // `before` não tem o feedback novo: a mensagem mostra só os anteriores.
            let before = t.clone();
            t.add_feedback("revisão", &feedback);
            (before, t.clone())
        });
        let (before, task) = updated.ok_or_else(|| TaskError::NotFound(format!("tarefa {id} sumiu")))?;
        let ident = self.open_task_instance(project, &task);
        let prompt = return_message(&before, &feedback, &self.feedback_learnings(&before, &feedback));
        let boot = self.task_boot_prompt(project, &task, "feedback", "foi devolvida na revisao com ajustes, descritos no arquivo", &prompt);
        self.deliver_to_task(project, &task, ident, prompt, boot, "feedback entregue ao agente");
        Ok(task)
    }
}

/// O número do id (`t12` → 12); 0 se o id não tem esse formato.
fn task_number(id: &str) -> u64 {
    id.strip_prefix('t').and_then(|n| n.parse().ok()).unwrap_or(0)
}

/// A ordem na fila: quem entrou antes, e pelo número em caso de empate. Quem não está na fila vai para o fim.
fn queue_key(task: &Task) -> (u64, u64) {
    (task.queued_ms.unwrap_or(u64::MAX), task_number(&task.id))
}

/// Por qual tarefa `task` espera, se espera: uma em andamento ou em revisão cujo escopo se sobrepõe ao dela (as
/// primeiras a ser citadas) ou, não havendo, uma que já esperava na fila antes dela e tem escopo sobreposto.
/// `Validating` não segura: o código já está na base. Sem escopo reservado (ver [`crate::scope`]) ninguém espera.
pub(super) fn queue_holder(tasks: &[Task], task: &Task) -> Option<String> {
    let clash = |o: &&Task| o.id != task.id && crate::scope::overlaps(&o.scope, &task.scope);
    let active = tasks.iter().filter(clash).find(|o| matches!(o.status, TaskStatus::Running | TaskStatus::Review));
    let ahead = || {
        tasks.iter().filter(clash).filter(|o| o.status == TaskStatus::Queued && queue_key(o) < queue_key(task)).min_by_key(|o| queue_key(o))
    };
    active.or_else(ahead).map(|o| o.id.clone())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exclude_is_written_once() {
        let first = exclude_with_agentdeck("# regras\n*.log").unwrap();
        assert_eq!(first, "# regras\n*.log\n# AgentDeck: worktrees das tarefas\n/.agentdeck/\n");
        assert_eq!(exclude_with_agentdeck(&first), None);
        assert!(exclude_with_agentdeck("").unwrap().ends_with("/.agentdeck/\n"));
        assert_eq!(exclude_with_agentdeck("a\n/.agentdeck/\n"), None);
    }
}
