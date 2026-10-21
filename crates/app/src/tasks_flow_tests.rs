//! O `Deck` da mesa de tarefas de ponta a ponta, com o core e o git de verdade (pula com aviso sem o
//! `git`): criar e iniciar pelo formulário, o relatório chegando, mesclar com confirmação, devolver,
//! validar, reprovar e descartar. Sem janela: o quadro é só `before_frame` (eventos do core e respostas das threads).

use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, Instant};

use commanddeck_core::paths::AppPaths;
use commanddeck_core::{Config, Core, StartOptions, Task, TaskStatus};

use crate::deck::Deck;
use crate::tasks_ui::{actions_for, parse_diff_stat, Confirm, TaskAct, TasksView};

fn git_available() -> bool {
    Command::new("git").arg("--version").output().is_ok_and(|o| o.status.success())
}

fn git(dir: &Path, args: &[&str]) {
    let out = Command::new("git").current_dir(dir).args(args).output().expect("git");
    assert!(out.status.success(), "git {args:?}: {}", String::from_utf8_lossy(&out.stderr));
}

/// Pasta do executável do git (para o PATH de busca do core).
fn git_dir() -> PathBuf {
    let probe = if cfg!(windows) { "where" } else { "which" };
    let out = Command::new(probe).arg("git").output().expect("which git");
    let first = String::from_utf8_lossy(&out.stdout).lines().next().unwrap_or("").trim().to_owned();
    Path::new(&first).parent().map(Path::to_path_buf).unwrap_or_default()
}

/// Um agente `claude` falso instalado (o core só confere que o executável existe).
fn install_fake_claude(root: &Path) {
    let bin = if cfg!(windows) { root.join("home").join(".local").join("bin") } else { root.join("fake-bin") };
    std::fs::create_dir_all(&bin).expect("mkdir");
    let exe = bin.join(if cfg!(windows) { "claude.exe" } else { "claude" });
    std::fs::write(&exe, b"").expect("write");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::write(&exe, "#!/bin/sh\ncat >/dev/null\n").expect("write");
        std::fs::set_permissions(&exe, std::fs::Permissions::from_mode(0o755)).expect("chmod");
    }
}

struct Env {
    _tmp: tempfile::TempDir,
    ws: PathBuf,
    core: Core,
    deck: Deck,
    ctx: egui::Context,
}

impl Env {
    fn start() -> Env {
        let tmp = tempfile::tempdir().expect("tmp");
        let root = tmp.path().to_path_buf();
        install_fake_claude(&root);
        let ws = root.join("ws");
        std::fs::create_dir_all(&ws).expect("ws");
        git(&ws, &["init", "-q", "-b", "main"]);
        for (k, v) in [("user.name", "Teste"), ("user.email", "teste@example.com"), ("commit.gpgsign", "false")] {
            git(&ws, &["config", k, v]);
        }
        std::fs::write(ws.join("a.txt"), "um\n").expect("w");
        git(&ws, &["add", "-A"]);
        git(&ws, &["commit", "-q", "-m", "inicial"]);
        let paths = AppPaths {
            app_dir: root.clone(),
            bin_dir: root.join("bin"),
            data_dir: root.join("dados"),
            default_workspace: root.join("ws"),
            whisper_dir: root.join("whisper"),
            config_file: root.join("cfg").join("config.json"),
        };
        let opts = StartOptions {
            search_path: Some(std::env::join_paths([root.join("fake-bin"), git_dir()]).expect("PATH")),
            home_dir: Some(root.join("home")),
            ollama_discovery: false,
            watch_workspace: false,
            config: Some(Config::with_project(ws.clone())),
            system_dirs: false,
        };
        let (core, rx) = Core::start_with(paths, opts).expect("start");
        let ctx = egui::Context::default();
        // Um quadro vazio carrega as fontes (o `Deck` mede o terminal no primeiro `before_frame`).
        let mut out = ctx.run_ui(egui::RawInput::default(), |_| {});
        out.textures_delta.clear();
        let deck = Deck::new(core.clone(), rx, std::sync::Arc::new(|| {}));
        Env { _tmp: tmp, ws, core, deck, ctx }
    }

    /// Roda quadros até `done` ficar verdadeiro (ou 30 s).
    fn pump(&mut self, what: &str, done: impl Fn(&Deck) -> bool) {
        let limit = Instant::now() + Duration::from_secs(30);
        while !done(&self.deck) {
            assert!(Instant::now() < limit, "tempo esgotado esperando: {what}");
            self.deck.before_frame(&self.ctx);
            std::thread::sleep(Duration::from_millis(15));
        }
    }

    fn task(&self, id: &str) -> Task {
        self.deck.tasks.task(id).cloned().unwrap_or_else(|| panic!("tarefa {id} não está na lista"))
    }

    fn project(&self) -> String {
        self.deck.active_project.clone()
    }

    fn tab_open(&self, id: &str) -> bool {
        self.deck.tasks.task(id).is_some_and(|t| self.deck.tasks_tab_open(t))
    }

    /// Preenche o formulário, envia e espera a resposta; devolve o id da tarefa aberta no detalhe.
    fn create_from_form(&mut self, title: &str, start_now: bool) -> String {
        self.deck.tasks_open_new();
        let f = &mut self.deck.tasks.form;
        f.title = title.to_owned();
        f.brief = "Faça o pedido.".into();
        f.start_now = start_now;
        self.deck.tasks_submit_form();
        assert!(self.deck.tasks.form.busy, "criando em segundo plano");
        self.pump("a criação", |d| !d.tasks.form.busy);
        match &self.deck.tasks.view {
            TasksView::Detail(id) => id.clone(),
            other => panic!("esperava o detalhe da tarefa nova, veio {other:?}"),
        }
    }

    /// O agente faz um commit na worktree da tarefa.
    fn agent_commits(&self, id: &str, file: &str, content: &str) {
        let t = self.task(id);
        std::fs::write(t.worktree.join(file), content).expect("w");
        git(&t.worktree, &["add", "-A"]);
        git(&t.worktree, &["commit", "-q", "-m", &format!("feat: {file}")]);
    }
}

#[test]
fn the_form_creates_and_starts_a_task_then_review_and_merge_run_off_the_ui_thread() {
    if !git_available() {
        eprintln!("git não instalado: teste pulado");
        return;
    }
    let mut env = Env::start();
    env.pump("os agentes e o status do git", |d| d.initialized && d.git.status().is_some());
    // O formulário abre com o agente padrão e a branch atual como base.
    env.deck.tasks_open_new();
    assert_eq!((env.deck.tasks.form.kind.as_str(), env.deck.tasks.form.base.as_str()), ("claude", "main"));
    assert!(env.deck.tasks.form.start_now && env.deck.tasks.form.focus_title);
    // Título vazio: nada acontece. Erro do core aparece no formulário.
    env.deck.tasks_submit_form();
    assert!(!env.deck.tasks.form.busy);
    env.deck.tasks.form.title = "Tarefa com base ruim".into();
    env.deck.tasks.form.base = "nao-existe".into();
    env.deck.tasks_submit_form();
    env.pump("o erro de validação", |d| !d.tasks.form.busy);
    let err = env.deck.tasks.form.error.clone().expect("erro inline");
    assert!(err.contains("não é uma branch local"), "{err}");
    assert_eq!(env.deck.tasks.view, TasksView::New, "o formulário continua aberto");
    assert!(env.core.tasks(&env.project()).is_empty());

    // Criar e iniciar: o detalhe abre com a tarefa em andamento, a aba do agente aparece.
    env.deck.tasks.form.base = "main".into();
    env.deck.tasks.form.error = None;
    env.deck.tasks_submit_form();
    env.pump("a criação", |d| !d.tasks.form.busy);
    assert_eq!(env.deck.tasks.view, TasksView::Detail("t1".into()));
    assert!(env.deck.tasks.form.title.is_empty(), "o formulário volta vazio");
    let t = env.task("t1");
    assert_eq!((t.status, t.base.as_str(), t.kind.as_str()), (TaskStatus::Running, "main", "claude"));
    env.pump("a aba do agente", |d| d.tasks.task("t1").is_some_and(|t| d.tasks_tab_open(t)));
    assert_eq!(actions_for(&env.task("t1"), true), [TaskAct::GoToTab, TaskAct::Discard]);

    // O agente trabalha e entrega o relatório (vem do core, como pela ponte): a lista relê sozinha.
    env.agent_commits("t1", "novo.txt", "dois\n");
    env.core.task_done(&env.project(), "t1", "Criei o novo.txt.").expect("done");
    env.pump("a tarefa chegar em revisão", |d| d.tasks.task("t1").is_some_and(|t| t.status == TaskStatus::Review));
    assert_eq!(env.deck.tasks.review_count(), 1);
    assert!(parse_diff_stat(env.task("t1").diff_stat.as_deref().unwrap_or("")).is_some_and(|s| s.files == 1));

    // O diff completo vem do core numa thread.
    env.deck.tasks_open_diff("t1");
    assert!(env.deck.tasks.diff.as_ref().is_some_and(|d| d.diff.is_none()), "carregando");
    env.pump("o diff", |d| d.tasks.diff.as_ref().is_some_and(|d| d.diff.is_some()));
    let diff = env.deck.tasks.diff.as_ref().and_then(|d| d.diff.clone()).expect("diff").expect("sem erro");
    assert!(diff.lines.iter().any(|l| l.text == "+dois"), "{:?}", diff.lines);
    env.deck.tasks_back();
    assert!(env.deck.tasks.diff.is_none() && env.deck.tasks.view == TasksView::Detail("t1".into()));

    // Devolver: pede o feedback, vai para a thread, volta a andamento.
    env.deck.tasks_act("t1", TaskAct::Return);
    assert!(env.deck.tasks.return_open);
    env.deck.tasks.feedback = "  Falta um teste.  ".into();
    env.deck.tasks_send_return("t1");
    assert!(env.deck.tasks.busy.is_some(), "uma operação por vez");
    env.pump("devolver", |d| d.tasks.busy.is_none());
    assert_eq!(env.task("t1").status, TaskStatus::Running);
    assert!(!env.deck.tasks.return_open && env.deck.tasks.feedback.is_empty());
    env.core.task_done(&env.project(), "t1", "Agora tem teste.").expect("done 2");
    env.pump("a segunda revisão", |d| d.tasks.task("t1").is_some_and(|t| t.status == TaskStatus::Review));

    // Mesclar pede confirmação; o "sim" mescla e a tarefa fica aguardando validação: a aba segue aberta.
    env.deck.tasks_act("t1", TaskAct::Merge);
    assert_eq!(env.deck.tasks.confirm, Some(Confirm::Merge("t1".into())));
    assert_eq!(env.task("t1").status, TaskStatus::Review, "nada mudou antes do sim");
    env.deck.tasks_confirm_accepted();
    assert!(env.deck.tasks.confirm.is_none() && env.deck.tasks.busy.is_some());
    env.pump("a mesclagem", |d| d.tasks.busy.is_none());
    assert_eq!(env.task("t1").status, TaskStatus::Validating);
    assert!(env.deck.tasks.notice.is_none());
    assert!(env.ws.join("novo.txt").exists(), "a mesclagem trouxe o arquivo para a base");
    assert!(env.tab_open("t1") && env.task("t1").worktree.is_dir(), "o agente continua vivo até validar");
    assert_eq!((env.deck.tasks.review_count(), env.deck.tasks.validating_count(), env.deck.tasks.attention_count()), (0, 1, 1));
    assert_eq!(actions_for(&env.task("t1"), true), [TaskAct::Validate, TaskAct::Reject, TaskAct::GoToTab, TaskAct::Discard]);

    // Reprovar: abre o mesmo formulário de feedback; enviar volta a tarefa ao agente, na mesma worktree.
    env.deck.tasks_act("t1", TaskAct::Reject);
    assert!(env.deck.tasks.return_open);
    env.deck.tasks_send_return("t1");
    assert!(env.deck.tasks.busy.is_none(), "sem motivo não envia");
    env.deck.tasks.feedback = "  O arquivo novo não aparece no app.  ".into();
    env.deck.tasks_send_return("t1");
    assert!(env.deck.tasks.busy.is_some());
    env.pump("a reprovação", |d| d.tasks.busy.is_none());
    let back = env.task("t1");
    assert_eq!(back.status, TaskStatus::Running);
    assert!(back.worktree.is_dir() && env.tab_open("t1"));
    assert!(!env.deck.tasks.return_open && env.deck.tasks.feedback.is_empty());
    assert_eq!(env.deck.tasks.attention_count(), 0);
    // O agente corrige e o ciclo segue: revisão, mesclar, aguardando validação.
    env.agent_commits("t1", "corrigido.txt", "ok\n");
    env.core.task_done(&env.project(), "t1", "Corrigi.").expect("done 3");
    env.pump("a terceira revisão", |d| d.tasks.task("t1").is_some_and(|t| t.status == TaskStatus::Review));
    env.deck.tasks_act("t1", TaskAct::Merge);
    env.deck.tasks_confirm_accepted();
    env.pump("a segunda mesclagem", |d| d.tasks.busy.is_none());
    assert_eq!(env.task("t1").status, TaskStatus::Validating);

    // Validar não pergunta: conclui, fecha a aba e apaga a worktree.
    env.deck.tasks_act("t1", TaskAct::Validate);
    assert!(env.deck.tasks.confirm.is_none() && env.deck.tasks.busy.is_some());
    env.pump("a validação", |d| d.tasks.busy.is_none());
    assert_eq!(env.task("t1").status, TaskStatus::Merged);
    assert!(env.deck.tasks.notice.is_none() && !env.task("t1").worktree.exists());
    env.pump("a aba fechar", |d| d.tasks.task("t1").is_some_and(|t| !d.tasks_tab_open(t)));
    assert!(actions_for(&env.task("t1"), false).is_empty());
    assert_eq!(env.deck.tasks.attention_count(), 0);
}

#[test]
fn a_refused_merge_shows_the_core_message_and_discard_asks_first() {
    if !git_available() {
        eprintln!("git não instalado: teste pulado");
        return;
    }
    let mut env = Env::start();
    env.pump("os agentes", |d| d.initialized);
    // Rascunho (sem "Iniciar agora"): só aparece na lista; iniciar roda numa thread.
    let id = env.create_from_form("Rascunho", false);
    assert_eq!(env.task(&id).status, TaskStatus::Draft);
    assert_eq!(actions_for(&env.task(&id), false), [TaskAct::Start, TaskAct::Discard]);
    env.deck.tasks_act(&id, TaskAct::Start);
    env.pump("iniciar", |d| d.tasks.busy.is_none());
    assert_eq!(env.task(&id).status, TaskStatus::Running);
    env.agent_commits(&id, "x.txt", "x\n");
    env.core.task_done(&env.project(), &id, "Feito.").expect("done");
    env.pump("revisão", |d| d.tasks.task("t1").is_some_and(|t| t.status == TaskStatus::Review));

    // O repositório principal sujo recusa a mesclagem: a mensagem do core vira o aviso vermelho.
    std::fs::write(env.ws.join("a.txt"), "mudou\n").expect("w");
    env.deck.tasks_act(&id, TaskAct::Merge);
    env.deck.tasks_confirm_accepted();
    env.pump("a recusa", |d| d.tasks.busy.is_none());
    let notice = env.deck.tasks.notice.clone().expect("aviso");
    assert!(notice.contains("alterações sem commit") && notice.contains("a.txt"), "{notice}");
    assert_eq!(env.task(&id).status, TaskStatus::Review, "continua em revisão");

    // Descartar: confirmação primeiro (cancelar não faz nada), depois roda e fecha tudo.
    env.deck.tasks_act(&id, TaskAct::Discard);
    assert_eq!(env.deck.tasks.confirm, Some(Confirm::Discard(id.clone())));
    env.deck.tasks.confirm = None;
    assert_eq!(env.task(&id).status, TaskStatus::Review);
    env.deck.tasks_act(&id, TaskAct::Discard);
    env.deck.tasks_confirm_accepted();
    env.pump("o descarte", |d| d.tasks.busy.is_none());
    let t = env.task(&id);
    assert_eq!(t.status, TaskStatus::Discarded);
    assert!(!t.worktree.exists());
    assert!(env.deck.tasks.notice.is_none(), "o aviso antigo sumiu com a nova operação");
}

#[test]
fn reopening_the_tab_of_a_running_task_registers_the_instance_again() {
    if !git_available() {
        eprintln!("git não instalado: teste pulado");
        return;
    }
    let mut env = Env::start();
    env.pump("os agentes", |d| d.initialized);
    let id = env.create_from_form("Com aba", true);
    env.pump("a aba", |d| d.tasks.task("t1").is_some_and(|t| d.tasks_tab_open(t)));
    // O usuário fecha a aba: a tarefa continua em andamento, e o botão passa a ser "Reabrir aba".
    let instance = env.task(&id).instance;
    let idx = env.deck.tabs.iter().position(|t| t.ident.id() == instance).expect("aba");
    env.deck.close_tab(idx);
    assert!(!env.tab_open(&id));
    assert_eq!(actions_for(&env.task(&id), false), [TaskAct::ReopenTab, TaskAct::Discard]);
    env.deck.tasks_act(&id, TaskAct::ReopenTab);
    assert!(env.tab_open(&id), "a aba voltou");
    assert!(env.deck.tasks.notice.is_none());
    // O terminal abre na worktree da tarefa (o core resolve pela instância).
    let ident = commanddeck_core::Ident::parse(&instance).expect("id");
    let spec = env.core.spawn_spec_for(&env.project(), &ident).expect("spec");
    assert!(commanddeck_core::util::same_path(&spec.cwd, &env.task(&id).worktree), "{}", spec.cwd.display());
    // Ir para a aba só a ativa; uma tarefa já resolvida não reabre a aba fechada.
    env.deck.tasks_act(&id, TaskAct::GoToTab);
    assert_eq!(env.deck.active.map(|i| env.deck.tabs[i].ident.id()), Some(instance.clone()));
    let merged = Task { status: TaskStatus::Merged, ..env.task(&id) };
    assert!(env.deck.open_task_tab(&merged).is_ok(), "a aba aberta só é ativada");
    env.deck.close_tab(env.deck.active.expect("ativa"));
    assert!(env.deck.open_task_tab(&merged).is_err());
    // Aguardando validação o agente continua vivo: a aba fechada pode ser reaberta.
    let validating = Task { status: TaskStatus::Validating, ..env.task(&id) };
    assert!(env.deck.open_task_tab(&validating).is_ok() && env.tab_open(&id));
}

#[test]
fn a_new_task_gets_the_learnings_and_the_current_situation_and_feedbacks_come_back() {
    if !git_available() {
        eprintln!("git não instalado: teste pulado");
        return;
    }
    let mut env = Env::start();
    env.pump("os agentes", |d| d.initialized);
    // O projeto já tem aprendizados na base: um global, um de a.txt e um de outra área.
    let dir = env.ws.join("memory").join("aprendizados");
    std::fs::create_dir_all(&dir).expect("dir");
    let write = |name: &str, touches: &str, body: &str| {
        let text = format!("---\ntitulo: Aprendizado {name}\nresumo: Resumo {name}.\ntouches: {touches}\n---\n{body}\n");
        std::fs::write(dir.join(format!("{name}.md")), text).expect("w");
    };
    write("global", "[]", "Vale para todo mundo.");
    write("do-arquivo", "[a.txt]", "Cuidado ao mexer no a.txt.");
    write("de-outra-area", "[crates/**]", "Só para o código.");
    git(&env.ws, &["add", "-A"]);
    git(&env.ws, &["commit", "-q", "-m", "docs: aprendizados"]);

    let project = env.project();
    let tasks_dir = env._tmp.path().join("dados").join("tarefas");
    fn create(env: &Env, project: &str, title: &str, brief: &str) -> String {
        let spec =
            commanddeck_core::TaskSpec { title: title.into(), brief: brief.into(), creator: Some("usuário".into()), ..Default::default() };
        let t = env.core.task_create(project, spec).expect("criar");
        env.core.task_start(project, &t.id).expect("iniciar");
        t.id
    }

    // A primeira tarefa mexe no a.txt: ainda sem outras tarefas, com os commits da base.
    let t1 = create(&env, &project, "Primeira", "Ajuste o a.txt e nada mais.");
    let first = std::fs::read_to_string(tasks_dir.join(format!("{project}-{t1}-brief.md"))).expect("brief da t1");
    assert!(first.contains("O que o projeto já aprendeu:") && first.contains("### Aprendizado do-arquivo"), "{first}");
    assert!(first.contains("Cuidado ao mexer no a.txt.") && first.contains("Vale para todo mundo."), "{first}");
    assert!(first.contains("Outras tarefas em andamento: nenhuma.") && first.contains("docs: aprendizados"), "{first}");
    // O que não casa vira só a linha do índice.
    assert!(
        first.contains("- memory/aprendizados/de-outra-area.md — Resumo de-outra-area.") && !first.contains("Só para o código."),
        "{first}"
    );

    // A t1 já mexeu no a.txt; a t2 também o cita, e ainda vê o aviso.
    env.pump("a t1 na lista", |d| d.tasks.task("t1").is_some());
    env.agent_commits(&t1, "a.txt", "dois\n");
    let t2 = create(&env, &project, "Segunda", "Também mexa no `a.txt`.");
    let second = std::fs::read_to_string(tasks_dir.join(format!("{project}-{t2}-brief.md"))).expect("brief da t2");
    assert!(second.contains(&format!("- {t1} \"Primeira\" (claude@{t1}, em andamento): mexe em a.txt")), "{second}");
    assert!(second.contains("ATENÇÃO: o seu pedido cita a.txt"), "{second}");

    // O feedback anterior volta na devolução seguinte.
    env.pump("a t2 na lista", |d| d.tasks.task("t2").is_some());
    env.agent_commits(&t2, "b.txt", "b\n");
    env.core.task_done(&project, &t2, "Feito.").expect("done");
    env.core.task_return(&project, &t2, "Primeiro ajuste: renomeie.").expect("return 1");
    let after_first = std::fs::read_to_string(tasks_dir.join(format!("{project}-{t2}-feedback.md"))).expect("feedback 1");
    assert!(!after_first.contains("Feedbacks anteriores"), "{after_first}");
    env.core.task_done(&project, &t2, "Feito de novo.").expect("done 2");
    let back = env.core.task_return(&project, &t2, "Segundo ajuste: teste.").expect("return 2");
    let after_second = std::fs::read_to_string(tasks_dir.join(format!("{project}-{t2}-feedback.md"))).expect("feedback 2");
    assert!(
        after_second.contains("1. [revisão] Primeiro ajuste: renomeie.") && after_second.contains("Segundo ajuste: teste."),
        "{after_second}"
    );
    assert_eq!(back.feedbacks.len(), 2, "a tarefa devolvida guarda os dois");
}

#[test]
fn feedback_brings_the_learnings_of_new_files_and_the_situation_lists_uncommitted_work() {
    if !git_available() {
        eprintln!("git não instalado: teste pulado");
        return;
    }
    let mut env = Env::start();
    env.pump("os agentes", |d| d.initialized);
    let dir = env.ws.join("memory").join("aprendizados");
    std::fs::create_dir_all(&dir).expect("dir");
    for (name, touches, body) in
        [("global", "[]", "Vale para todo mundo."), ("do-a", "[a.txt]", "Cuidado com o a."), ("do-b", "[b.txt]", "Cuidado com o b.")]
    {
        let text = format!("---\ntitulo: Aprendizado {name}\nresumo: Resumo {name}.\ntouches: {touches}\n---\n{body}\n");
        std::fs::write(dir.join(format!("{name}.md")), text).expect("w");
    }
    git(&env.ws, &["add", "-A"]);
    git(&env.ws, &["commit", "-q", "-m", "docs: aprendizados"]);

    let project = env.project();
    let tasks_dir = env._tmp.path().join("dados").join("tarefas");
    fn create(env: &Env, project: &str, title: &str, brief: &str) -> String {
        let spec =
            commanddeck_core::TaskSpec { title: title.into(), brief: brief.into(), creator: Some("usuário".into()), ..Default::default() };
        let t = env.core.task_create(project, spec).expect("criar");
        env.core.task_start(project, &t.id).expect("iniciar");
        t.id
    }
    let feedback_file = |id: &str| std::fs::read_to_string(tasks_dir.join(format!("{project}-{id}-feedback.md"))).expect("feedback");

    // A t1 cita só o a.txt e entrega o b.txt.
    let t1 = create(&env, &project, "Primeira", "Ajuste o a.txt.");
    env.pump("a t1 na lista", |d| d.tasks.task("t1").is_some());
    env.agent_commits(&t1, "b.txt", "b\n");
    env.core.task_done(&project, &t1, "Feito.").expect("done");

    // Devolução citando o a.txt (já no pedido) e o b.txt (novo): só o aprendizado do b.txt vai, sem global nem índice.
    env.core.task_return(&project, &t1, "Revise a.txt e também b.txt.").expect("return");
    let first = feedback_file(&t1);
    assert!(first.contains("O que o projeto já aprendeu sobre os arquivos citados:"), "{first}");
    assert!(first.contains("Cuidado com o b.") && !first.contains("Cuidado com o a."), "{first}");
    assert!(!first.contains("Vale para todo mundo.") && !first.contains("Outros aprendizados"), "{first}");

    // O b.txt agora já foi citado num feedback anterior: citar de novo não repete o aprendizado.
    env.core.task_done(&project, &t1, "Feito de novo.").expect("done 2");
    env.core.task_return(&project, &t1, "Ainda falta no b.txt.").expect("return 2");
    let second = feedback_file(&t1);
    assert!(second.contains("Feedbacks anteriores") && !second.contains("sobre os arquivos citados"), "{second}");
    assert!(!second.contains("Cuidado com o b."), "{second}");

    // A reprovação segue a mesma regra: depois de mesclar, o a.txt (pedido) e o b.txt (feedback) já foram vistos.
    env.core.task_done(&project, &t1, "Pronto.").expect("done 3");
    env.core.task_merge(&project, &t1).expect("merge");
    env.core.task_reject(&project, &t1, "Não funciona: veja a.txt e b.txt.").expect("reject");
    let rejected = feedback_file(&t1);
    assert!(rejected.contains("reprovada na validação") && !rejected.contains("sobre os arquivos citados"), "{rejected}");

    // Uma tarefa nova vê o trabalho ainda sem commit da t2 na situação atual.
    let t2 = create(&env, &project, "Segunda", "Faça outra coisa.");
    env.pump("a t2 na lista", |d| d.tasks.task("t2").is_some());
    let work = env.task(&t2).worktree;
    std::fs::write(work.join("novo.txt"), "x\n").expect("w");
    std::fs::write(work.join("a.txt"), "mudou\n").expect("w");
    let t3 = create(&env, &project, "Terceira", "Nada.");
    let brief = std::fs::read_to_string(tasks_dir.join(format!("{project}-{t3}-brief.md"))).expect("brief da t3");
    assert!(brief.contains(&format!("- {t2} \"Segunda\" (claude@{t2}, em andamento): mexe em a.txt, novo.txt")), "{brief}");
}
