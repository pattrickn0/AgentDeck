//! O `Deck` da mesa de tarefas de ponta a ponta, com o core e o git de verdade (pula com aviso sem o
//! `git`): criar e iniciar pelo formulário, o relatório chegando, mesclar com confirmação, devolver,
//! validar, reprovar e descartar. Sem janela: o quadro é só `before_frame` (eventos do core e respostas das threads).

use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, Instant};

use agentdeck_core::paths::AppPaths;
use agentdeck_core::{Config, Core, MergeBlock, StartOptions, Task, TaskStatus};

use crate::deck::Deck;
use crate::docs::{CenterView, DiffDoc, DocState};
use crate::tasks_ui::{actions_for, can_show_diff, diff_button_label, parse_diff_stat, Confirm, TaskAct, TasksView};

fn git_available() -> bool {
    Command::new("git").arg("--version").output().is_ok_and(|o| o.status.success())
}

fn git(dir: &Path, args: &[&str]) {
    let out = Command::new("git").current_dir(dir).args(args).output().expect("git");
    assert!(out.status.success(), "git {args:?}: {}", String::from_utf8_lossy(&out.stderr));
}

/// Pasta do executável do git (para o PATH de busca do core).
fn git_dir() -> PathBuf {
    // `which` não vem no contêiner do Fedora da CI; `command -v` é do próprio shell.
    let out =
        if cfg!(windows) { Command::new("where").arg("git").output() } else { Command::new("sh").args(["-c", "command -v git"]).output() };
    let out = out.expect("which git");
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

    /// O estado do documento `uid` (que precisa existir).
    fn doc_state(&self, uid: u64) -> &DocState {
        &self.deck.docs.get(uid).unwrap_or_else(|| panic!("documento {uid} não está aberto")).state
    }

    /// Abre o diff da tarefa no centro e espera ele chegar (a leitura roda numa thread). Devolve o uid do documento
    /// e o diff.
    fn open_task_diff(&mut self, id: &str) -> (u64, DiffDoc) {
        self.deck.docs_open_task_diff(id);
        let uid = self.deck.docs.current_uid(&self.project()).expect("documento em foco");
        assert!(matches!(self.doc_state(uid), DocState::Loading), "carregando");
        assert_eq!(self.deck.docs.view, CenterView::Docs, "abrir um documento leva o centro para Documentos");
        self.pump("o diff", |d| !matches!(d.docs.get(uid).map(|t| &t.state), Some(DocState::Loading)));
        match self.doc_state(uid) {
            DocState::Diff(doc) => (uid, doc.clone()),
            other => panic!("esperava o diff, veio {other:?}"),
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

    // O diff completo vem do core numa thread e abre no centro (a barra direita continua no detalhe).
    let (diff_uid, doc) = env.open_task_diff("t1");
    assert!(doc.diff.lines.iter().any(|l| l.text == "+dois"), "{:?}", doc.diff.lines);
    assert_eq!(doc.files.iter().map(|f| (f.path.as_str(), f.status, f.added)).collect::<Vec<_>>(), [("novo.txt", 'A', 1)]);
    assert_eq!(env.deck.tasks.view, TasksView::Detail("t1".into()));
    // Abrir de novo só foca o mesmo documento.
    env.deck.docs_open_task_diff("t1");
    assert_eq!(env.deck.docs.count(&env.project()), 1);
    assert_eq!(env.deck.docs.current_uid(&env.project()), Some(diff_uid));

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

    // Aguardando validação o botão do diff mostra o que a mesclagem trouxe (a branch já é a própria base).
    let validating = env.task("t1");
    assert!(can_show_diff(&validating) && validating.merge_commit.is_some());
    assert_eq!(diff_button_label(&validating), "Ver o que foi mesclado");
    let (merged_uid, doc) = env.open_task_diff("t1");
    assert_ne!(merged_uid, diff_uid, "o diff da mesclagem é outro documento, não o da branch de antes");
    assert!(doc.diff.lines.iter().any(|l| l.text == "+dois"), "{:?}", doc.diff.lines);
    assert_eq!(env.deck.docs.count(&env.project()), 2);
    // Fechar o último documento volta o centro para os agentes.
    env.deck.docs_close(diff_uid);
    assert_eq!(env.deck.docs.view, CenterView::Docs);
    env.deck.docs_close(merged_uid);
    env.deck.docs.sync(&env.project());
    assert_eq!(env.deck.docs.view, CenterView::Agents);

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
fn a_task_whose_scope_overlaps_waits_in_the_queue_and_starts_by_itself_when_the_other_is_merged() {
    if !git_available() {
        eprintln!("git não instalado: teste pulado");
        return;
    }
    let mut env = Env::start();
    env.pump("os agentes e o status do git", |d| d.initialized && d.git.status().is_some());
    crate::theme::install_fonts(&env.ctx);
    crate::theme::install_style(&env.ctx);

    // O escopo do formulário (um caminho por linha ou separado por vírgula) vai para o core.
    env.deck.tasks_open_new();
    env.deck.tasks.form.scope = " crates/core \n\nmemory/GERAL.md, crates\\cli ".into();
    env.deck.tasks.form.title = "Mexe no core".into();
    env.deck.tasks_submit_form();
    env.pump("a criação", |d| !d.tasks.form.busy);
    assert_eq!(env.task("t1").scope, ["crates/core", "memory/GERAL.md", "crates/cli"]);
    assert_eq!(env.task("t1").status, TaskStatus::Running);
    assert!(env.deck.tasks.form.scope.is_empty(), "o formulário volta vazio");
    env.pump("a aba do agente", |d| d.tasks.task("t1").is_some_and(|t| d.tasks_tab_open(t)));

    // Um escopo inválido aparece no formulário, sem criar nada.
    env.deck.tasks_open_new();
    env.deck.tasks.form.title = "Curinga".into();
    env.deck.tasks.form.scope = "crates/*.rs".into();
    env.deck.tasks_submit_form();
    env.pump("o erro do escopo", |d| !d.tasks.form.busy);
    assert!(env.deck.tasks.form.error.as_deref().is_some_and(|e| e.contains("não de padrões")), "{:?}", env.deck.tasks.form.error);
    assert_eq!(env.core.tasks(&env.project()).len(), 1);

    // A segunda, com escopo dentro do da primeira, entra na fila: sem aba, só o botão de descartar.
    env.deck.tasks.form.title = "Mexe no git.rs".into();
    env.deck.tasks.form.scope = "crates/core/src/git.rs".into();
    env.deck.tasks_submit_form();
    env.pump("a criação da segunda", |d| !d.tasks.form.busy && d.tasks.task("t2").is_some());
    let queued = env.task("t2");
    assert_eq!((queued.status, queued.waiting_for.as_deref()), (TaskStatus::Queued, Some("t1")));
    assert_eq!(env.deck.tasks.view, TasksView::Detail("t2".into()));
    assert!(!env.tab_open("t2") && !queued.worktree.exists());
    assert_eq!(actions_for(&queued, false), [TaskAct::Discard]);
    assert_eq!(crate::tasks_ui::Group::of(queued.status), crate::tasks_ui::Group::Queued);
    // A lista e o detalhe da tarefa na fila desenham sem problema.
    draw_frame(&mut env, vec![]);
    env.deck.tasks_back();
    draw_frame(&mut env, vec![]);

    // A primeira entrega e é mesclada pelo botão: a segunda começa sozinha, com a mesclagem na base.
    env.agent_commits("t1", "core.txt", "dois\n");
    env.core.task_done(&env.project(), "t1", "Feito.").expect("done");
    env.pump("a revisão", |d| d.tasks.task("t1").is_some_and(|t| t.status == TaskStatus::Review));
    // O arquivo está fora do escopo declarado: a revisão mostra o aviso (e a mesclagem segue livre).
    assert_eq!(env.task("t1").outside_scope, ["core.txt"]);
    env.deck.tasks_open_detail("t1");
    draw_frame(&mut env, vec![]);
    env.deck.tasks_act("t1", TaskAct::Merge);
    env.deck.tasks_confirm_accepted();
    env.pump("a mesclagem", |d| d.tasks.busy.is_none());
    env.pump("a tarefa da fila começar", |d| d.tasks.task("t2").is_some_and(|t| t.status == TaskStatus::Running));
    let started = env.task("t2");
    assert!(started.waiting_for.is_none() && started.worktree.join("core.txt").is_file(), "parte da base já com a mesclagem da t1");
    env.pump("a aba da tarefa que saiu da fila", |d| d.tasks.task("t2").is_some_and(|t| d.tasks_tab_open(t)));
    assert_eq!(actions_for(&started, true), [TaskAct::GoToTab, TaskAct::Discard]);
    draw_frame(&mut env, vec![]);
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

    // O repositório principal sujo recusa a mesclagem: a mensagem do core vira o aviso vermelho. Pelo botão
    // do cartão (na lista), a recusa abre o detalhe, que é onde o aviso aparece.
    std::fs::write(env.ws.join("a.txt"), "mudou\n").expect("w");
    env.deck.tasks.view = TasksView::List;
    env.deck.tasks_act(&id, TaskAct::Merge);
    env.deck.tasks_confirm_accepted();
    env.pump("a recusa", |d| d.tasks.busy.is_none());
    let notice = env.deck.tasks.notice.clone().expect("aviso");
    assert!(notice.contains("alterações sem commit") && notice.contains("a.txt"), "{notice}");
    assert_eq!(env.deck.tasks.view, TasksView::Detail(id.clone()), "a recusa vinda da lista abre o detalhe");
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
fn a_refused_merge_offers_the_way_out_in_the_notice() {
    if !git_available() {
        eprintln!("git não instalado: teste pulado");
        return;
    }
    let mut env = Env::start();
    env.pump("os agentes", |d| d.initialized);
    let id = env.create_from_form("Saídas da recusa", true);
    env.pump("a aba", |d| d.tasks.task("t1").is_some_and(|t| d.tasks_tab_open(t)));
    env.agent_commits(&id, "x.txt", "x\n");
    env.core.task_done(&env.project(), &id, "Feito.").expect("done");
    env.pump("revisão", |d| d.tasks.task("t1").is_some_and(|t| t.status == TaskStatus::Review));

    // Alterações do usuário na pasta principal: o aviso traz o motivo e "Guardar minhas alterações e mesclar".
    std::fs::write(env.ws.join("a.txt"), "mudou\n").expect("w");
    env.deck.tasks_act(&id, TaskAct::Merge);
    env.deck.tasks_confirm_accepted();
    env.pump("a recusa", |d| d.tasks.busy.is_none());
    assert_eq!(env.deck.tasks.notice_block, Some(MergeBlock::MainDirty(vec!["a.txt".into()])));
    assert!(env.deck.tasks.notice.is_some() && !env.deck.tasks.notice_info);
    // "Pedir ao agente" não vale para o que é do usuário.
    env.deck.tasks_ask_agent(&id);
    assert!(env.deck.tasks.busy.is_none() && env.task(&id).status == TaskStatus::Review);
    // O clique guarda, mescla e devolve: o aviso passa a ser a nota (informação, sem motivo), a tarefa vai a validação.
    env.deck.tasks_merge_stash(&id);
    assert!(env.deck.tasks.busy.is_some() && env.deck.tasks.notice.is_none() && env.deck.tasks.notice_block.is_none());
    env.pump("guardar e mesclar", |d| d.tasks.busy.is_none());
    assert_eq!(env.task(&id).status, TaskStatus::Validating);
    assert!(env.ws.join("x.txt").exists(), "a mesclagem trouxe o arquivo");
    // (o git do usuário pode converter as quebras de linha ao devolver)
    let back = std::fs::read_to_string(env.ws.join("a.txt")).expect("a.txt");
    assert_eq!(back.replace("\r\n", "\n"), "mudou\n", "a alteração do usuário voltou");
    let note = env.deck.tasks.notice.clone().expect("nota");
    assert!(env.deck.tasks.notice_info && env.deck.tasks.notice_block.is_none() && note.contains("voltaram"), "{note}");

    // Reprovada e entregue de novo, com trabalho esquecido na pasta da tarefa: "Pedir ao agente para fazer o commit".
    git(&env.ws, &["checkout", "-q", "--", "a.txt"]);
    env.deck.tasks_act(&id, TaskAct::Reject);
    env.deck.tasks.feedback = "Ajuste.".into();
    env.deck.tasks_send_return(&id);
    env.pump("a reprovação", |d| d.tasks.busy.is_none());
    env.agent_commits(&id, "y.txt", "y\n");
    std::fs::write(env.task(&id).worktree.join("esqueci.txt"), "z\n").expect("w");
    env.core.task_done(&env.project(), &id, "De novo.").expect("done");
    env.pump("revisão 2", |d| d.tasks.task("t1").is_some_and(|t| t.status == TaskStatus::Review));
    env.deck.tasks_act(&id, TaskAct::Merge);
    env.deck.tasks_confirm_accepted();
    env.pump("a recusa 2", |d| d.tasks.busy.is_none());
    assert_eq!(env.deck.tasks.notice_block, Some(MergeBlock::TaskDirty(vec!["esqueci.txt".into()])));
    assert!(!env.deck.tasks.notice_info);
    env.deck.tasks_ask_agent(&id);
    assert!(env.deck.tasks.busy.is_some() && env.deck.tasks.notice_block.is_none(), "limpa o aviso e devolve");
    env.pump("devolver", |d| d.tasks.busy.is_none());
    let back = env.task(&id);
    assert_eq!(back.status, TaskStatus::Running);
    assert!(back.events.iter().any(|e| e.text.contains("devolvida ao agente")), "{:?}", back.events);
    assert!(env.deck.tasks.notice.is_none());
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
    let ident = agentdeck_core::Ident::parse(&instance).expect("id");
    let spec = env.core.spawn_spec_for(&env.project(), &ident).expect("spec");
    assert!(agentdeck_core::util::same_path(&spec.cwd, &env.task(&id).worktree), "{}", spec.cwd.display());
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
            agentdeck_core::TaskSpec { title: title.into(), brief: brief.into(), creator: Some("usuário".into()), ..Default::default() };
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
            agentdeck_core::TaskSpec { title: title.into(), brief: brief.into(), creator: Some("usuário".into()), ..Default::default() };
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

#[test]
fn files_markdown_and_commits_open_in_the_center_and_the_files_reload_when_they_change() {
    if !git_available() {
        eprintln!("git não instalado: teste pulado");
        return;
    }
    let mut env = Env::start();
    env.pump("os agentes e o status do git", |d| d.initialized && d.git.status().is_some());
    let project = env.project();
    std::fs::write(env.ws.join("a.rs"), "fn a() {}\n\tfn b() {}\n").expect("w");
    std::fs::write(env.ws.join("nota.md"), "# Título\n\n- [ ] um\n").expect("w");
    std::fs::write(env.ws.join("bin.dat"), b"MZ\0\0").expect("w");
    let wait = |env: &mut Env, uid: u64| {
        env.pump("o documento", |d| !matches!(d.docs.get(uid).map(|t| &t.state), Some(DocState::Loading)));
    };

    // Duplo clique no explorador: texto, markdown e binário.
    env.deck.tree_file_double_click("a.rs");
    let rs = env.deck.docs.current_uid(&project).expect("a.rs");
    wait(&mut env, rs);
    // Um arquivo pequeno abre como código (lê, busca e edita), sem edição ligada.
    assert!(matches!(env.doc_state(rs), DocState::Source(s) if s.text == "fn a() {}\n\tfn b() {}\n" && !s.editing && !s.dirty()));
    env.deck.tree_file_double_click("nota.md");
    let md = env.deck.docs.current_uid(&project).expect("nota.md");
    wait(&mut env, md);
    assert!(matches!(env.doc_state(md), DocState::Source(s) if s.text.starts_with("# Título") && !s.raw));
    env.deck.tree_file_double_click("bin.dat");
    let bin = env.deck.docs.current_uid(&project).expect("bin.dat");
    wait(&mut env, bin);
    assert!(matches!(env.doc_state(bin), DocState::Unreadable(m) if m.contains("binary")));
    env.deck.tree_file_double_click("sumiu.txt");
    let gone = env.deck.docs.current_uid(&project).expect("sumiu.txt");
    wait(&mut env, gone);
    assert!(matches!(env.doc_state(gone), DocState::Failed(m) if m.contains("sumiu.txt")));
    assert_eq!(env.deck.docs.count(&project), 4);
    // Abrir o mesmo arquivo só o foca.
    env.deck.tree_file_double_click("a.rs");
    assert_eq!((env.deck.docs.count(&project), env.deck.docs.current_uid(&project)), (4, Some(rs)));

    // O arquivo muda no disco (o watcher avisa): o documento é relido sem passar por "Carregando…".
    std::fs::write(env.ws.join("a.rs"), "fn a() {}\nfn c() {}\n").expect("w");
    env.deck.docs_reload_files();
    assert!(matches!(env.doc_state(rs), DocState::Source(_)), "a leitura antiga continua na tela até a nova chegar");
    env.pump(
        "o arquivo relido",
        |d| matches!(d.docs.get(rs).map(|t| &t.state), Some(DocState::Source(s)) if s.text == "fn a() {}\nfn c() {}\n"),
    );
    // O arquivo que sumiu aparece como erro; o que voltou, lido de novo.
    std::fs::write(env.ws.join("sumiu.txt"), "voltei\n").expect("w");
    env.deck.docs_reload_files();
    env.pump("o arquivo que voltou", |d| matches!(d.docs.get(gone).map(|t| &t.state), Some(DocState::Source(_))));

    // O diff de um commit (Histórico) e o de um arquivo alterado (aba Git).
    git(&env.ws, &["add", "-A"]);
    git(&env.ws, &["commit", "-q", "-m", "feat: arquivos novos"]);
    let out = Command::new("git").current_dir(&env.ws).args(["rev-parse", "HEAD"]).output().expect("rev-parse");
    let hash = String::from_utf8_lossy(&out.stdout).trim().to_owned();
    env.deck.docs_open_commit(&hash, &hash[..7], "feat: arquivos novos");
    let commit = env.deck.docs.current_uid(&project).expect("commit");
    wait(&mut env, commit);
    let DocState::Diff(doc) = env.doc_state(commit) else { panic!("esperava o diff do commit") };
    let paths: Vec<&str> = doc.files.iter().map(|f| f.path.as_str()).collect();
    for expected in ["a.rs", "bin.dat", "nota.md", "sumiu.txt"] {
        assert!(paths.contains(&expected), "{expected} não está em {paths:?}");
    }

    std::fs::write(env.ws.join("a.rs"), "fn a() {}\nfn d() {}\n").expect("w");
    let entry = agentdeck_core::git::GitEntry {
        path: "a.rs".into(),
        orig_path: None,
        index: '.',
        worktree: 'M',
        kind: agentdeck_core::git::EntryKind::Ordinary,
    };
    env.deck.docs_open_file_diff(&entry, false);
    let file_diff = env.deck.docs.current_uid(&project).expect("diff do arquivo");
    wait(&mut env, file_diff);
    let DocState::Diff(doc) = env.doc_state(file_diff) else { panic!("esperava o diff do arquivo") };
    assert!(doc.diff.lines.iter().any(|l| l.text == "+fn d() {}"), "{:?}", doc.diff.lines);
    // O lado preparado do mesmo arquivo é outro documento.
    env.deck.docs_open_file_diff(&entry, true);
    assert_ne!(env.deck.docs.current_uid(&project), Some(file_diff));

    // O documento é do projeto: um documento fechado durante a leitura não quebra nada, e "Recarregar" relê.
    env.deck.docs_reload(rs);
    assert!(matches!(env.doc_state(rs), DocState::Source(_)), "um arquivo aberto como código é relido sem passar por Carregando");
    env.deck.docs_close(rs);
    env.pump("a resposta de um documento fechado", |d| d.docs.get(rs).is_none());
    env.deck.before_frame(&env.ctx);
}

#[test]
fn the_task_request_and_report_open_as_markdown_from_the_loaded_task() {
    if !git_available() {
        eprintln!("git não instalado: teste pulado");
        return;
    }
    let mut env = Env::start();
    env.pump("os agentes e o status do git", |d| d.initialized && d.git.status().is_some());
    let id = env.create_from_form("Pedido e relatório", true);
    env.agent_commits(&id, "novo.txt", "dois\n");
    env.core.task_done(&env.project(), &id, "## Feito\n\n- [x] criei o novo.txt").expect("done");
    env.pump("a revisão", |d| d.tasks.task("t1").is_some_and(|t| t.status == TaskStatus::Review));
    let project = env.project();

    env.deck.docs_open_task_text(&id, false);
    let brief = env.deck.docs.current_uid(&project).expect("pedido");
    assert!(matches!(env.doc_state(brief), DocState::Markdown(t) if t == "Faça o pedido."), "o pedido já vem da tarefa carregada");
    env.deck.docs_open_task_text(&id, true);
    let report = env.deck.docs.current_uid(&project).expect("relatório");
    assert!(matches!(env.doc_state(report), DocState::Markdown(t) if t.starts_with("## Feito")));
    assert_ne!(brief, report);
    // Não são relidos do disco: o botão de recarregar não existe para eles.
    env.deck.docs_reload(report);
    assert!(matches!(env.doc_state(report), DocState::Markdown(_)));
    assert_eq!(env.deck.docs.view, CenterView::Docs);
}

#[test]
fn reading_documents_keeps_the_terminals_running_unresized_and_marks_finished_turns_as_unseen() {
    if !git_available() {
        eprintln!("git não instalado: teste pulado");
        return;
    }
    let mut env = Env::start();
    env.pump("os agentes e o status do git", |d| d.initialized && d.git.status().is_some());
    // A pintura usa as fontes do app (a família `ui-semibold`…).
    crate::theme::install_fonts(&env.ctx);
    crate::theme::install_style(&env.ctx);
    let mut out = env.ctx.run_ui(egui::RawInput::default(), |_| {});
    out.textures_delta.clear();
    let id = env.create_from_form("Agente que trabalha", true);
    env.pump("a aba do agente", |d| d.tasks.task("t1").is_some_and(|t| d.tasks_tab_open(t)));
    let project = env.project();
    let instance = env.task(&id).instance;
    let frame = |env: &mut Env, w: f32, h: f32| {
        let input =
            egui::RawInput { screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(w, h))), ..Default::default() };
        let Env { deck, ctx, .. } = env;
        let mut out = ctx.run_ui(input, |ui| crate::ui::draw(deck, ui));
        out.textures_delta.clear();
    };
    let tab = |env: &Env| env.deck.tabs.iter().position(|t| t.project == project && t.ident.id() == instance).expect("aba");

    // Com os agentes à vista, o terminal se ajusta ao centro e o PTY muda depois do tamanho parado.
    frame(&mut env, 1280.0, 860.0);
    let i = tab(&env);
    assert!(env.deck.tabs[i].resize_pending(), "o terminal pede o tamanho do centro");
    env.pump("o PTY mudar de tamanho", |d| !d.tabs.iter().any(|t| t.resize_pending()));
    let size = env.deck.tabs[i].session.size();

    // Lendo documentos: a janela muda de tamanho e o PTY não é tocado (nem com o centro escondido, nem a zero).
    env.deck.docs_open_file("a.txt");
    env.deck.before_frame(&env.ctx);
    assert_eq!(env.deck.docs.view, CenterView::Docs);
    assert!(!env.deck.term_focus, "o terminal não recebe o teclado enquanto está escondido");
    frame(&mut env, 900.0, 600.0);
    frame(&mut env, 1100.0, 700.0);
    let i = tab(&env);
    assert!(!env.deck.tabs[i].resize_pending(), "nenhum redimensionamento pedido com o terminal escondido");
    assert_eq!(env.deck.tabs[i].session.size(), size);

    // O agente termina um turno enquanto se lê: conta como não visto, mesmo sendo a aba ativa.
    assert_eq!(env.deck.agents_badge(), crate::docs::Badge::default());
    env.deck.on_agent_signal(&project, &instance, true);
    assert_eq!(env.deck.agents_badge(), crate::docs::Badge { done: 0, busy: 1 });
    env.deck.on_agent_signal(&project, &instance, false);
    assert!(env.deck.tabs[i].done);
    assert_eq!(env.deck.agents_badge(), crate::docs::Badge { done: 1, busy: 0 });
    // Ativar a aba sem sair dos documentos (ex.: um anexo) não a dá como vista.
    env.deck.activate(i);
    assert!(env.deck.tabs[i].done && !env.deck.term_focus);

    // Voltar aos agentes (a chave, o atalho ou clicar num agente): a aba ativa é vista e o terminal recebe o teclado.
    env.deck.show_agents();
    env.deck.before_frame(&env.ctx);
    assert!(!env.deck.tabs[i].done && env.deck.term_focus);
    frame(&mut env, 1100.0, 700.0);
    assert!(env.deck.tabs[tab(&env)].resize_pending(), "ao voltar, o terminal se ajusta ao tamanho novo");

    // O atalho alterna (só com documentos abertos) e Ctrl+Tab passa pelos documentos no modo Documentos.
    env.deck.push_action(crate::deck::Action::ToggleCenter);
    env.deck.before_frame(&env.ctx);
    assert_eq!(env.deck.docs.view, CenterView::Docs);
    env.deck.docs_open_file("b.txt");
    let second = env.deck.docs.current_uid(&project);
    env.deck.push_action(crate::deck::Action::CycleTab { back: false });
    env.deck.before_frame(&env.ctx);
    assert_ne!(env.deck.docs.current_uid(&project), second);
    env.deck.push_action(crate::deck::Action::ToggleCenter);
    env.deck.before_frame(&env.ctx);
    assert_eq!(env.deck.docs.view, CenterView::Agents);
}

/// "Encerrar" da bandeja: com um agente no meio de um turno o app pergunta antes (sem a pergunta o turno se perderia), e o
/// pedido da segunda abertura chega como `CoreEvent::Activate`.
#[test]
fn quitting_asks_while_an_agent_is_in_the_middle_of_a_turn_and_the_second_launch_asks_for_the_window() {
    if !git_available() {
        eprintln!("git não instalado: teste pulado");
        return;
    }
    let mut env = Env::start();
    env.pump("os agentes e o status do git", |d| d.initialized && d.git.status().is_some());
    let id = env.create_from_form("Agente que trabalha", true);
    env.pump("a aba do agente", |d| d.tasks.task("t1").is_some_and(|t| d.tasks_tab_open(t)));
    let (project, instance) = (env.project(), env.task(&id).instance);

    env.deck.on_agent_signal(&project, &instance, true);
    assert_eq!(env.deck.working_tabs().len(), 1);
    env.deck.request_quit();
    assert!(!env.deck.quit_requested, "no meio de um turno o app pergunta antes de sair");
    assert_eq!(env.deck.docs.confirm, Some(crate::docs::Unsaved::Busy));
    env.deck.docs_answer(crate::docs::Answer::Cancel);
    assert!(!env.deck.quit_requested && env.deck.docs.confirm.is_none());
    env.deck.request_quit();
    env.deck.docs_answer(crate::docs::Answer::Discard);
    assert!(env.deck.quit_requested, "\"Encerrar mesmo assim\"");

    env.deck.quit_requested = false;
    env.deck.on_agent_signal(&project, &instance, false);
    env.deck.request_quit();
    assert!(env.deck.quit_requested, "sem turno em andamento não há o que perguntar");

    assert!(!env.deck.activate_requested);
    env.deck.on_core_event(agentdeck_core::CoreEvent::Activate);
    assert!(env.deck.activate_requested);
}

#[test]
fn the_highlight_follows_the_active_tab_and_the_pty_resizes_once_per_switch() {
    if !git_available() {
        eprintln!("git não instalado: teste pulado");
        return;
    }
    use agentdeck_core::Organization;
    let mut env = Env::start();
    env.pump("os agentes e o status do git", |d| d.initialized && d.git.status().is_some());
    crate::theme::install_fonts(&env.ctx);
    crate::theme::install_style(&env.ctx);
    let mut out = env.ctx.run_ui(egui::RawInput::default(), |_| {});
    out.textures_delta.clear();
    let frame = |env: &mut Env| {
        let input = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1400.0, 860.0))),
            ..Default::default()
        };
        let Env { deck, ctx, .. } = env;
        let mut out = ctx.run_ui(input, |ui| crate::ui::draw(deck, ui));
        out.textures_delta.clear();
    };
    let tab_of = |env: &Env, kind: &str| env.deck.tabs.iter().position(|t| t.kind == kind).expect("aba");
    // Dois CLIs (dois grupos), o último aberto é o selecionado.
    env.deck.open_tab("claude", false);
    env.deck.open_tab("shell", false);
    env.deck.set_organization(Organization::Highlight);
    frame(&mut env);
    let big_group = |env: &Env| env.deck.panes.first().and_then(|p| p.groups.first()).cloned();
    assert_eq!(env.deck.panes.len(), 2);
    assert_eq!(big_group(&env).as_deref(), Some("shell"));
    let (big, small) = (env.deck.panes[0].rect, env.deck.panes[1].rect);
    assert!(big.width() > small.width() * 1.8, "a janela do selecionado é a grande: {big:?} × {small:?}");
    env.pump("o PTY de cada janela ter o tamanho", |d| !d.tabs.iter().any(|t| t.resize_pending()) && d.metrics.is_some());
    frame(&mut env);
    env.pump("o PTY se ajustar", |d| !d.tabs.iter().any(|t| t.resize_pending()));

    // Quadros parados não pedem redimensionamento.
    for _ in 0..5 {
        frame(&mut env);
    }
    assert!(!env.deck.tabs.iter().any(|t| t.resize_pending()), "um quadro sem mudança não mexe no PTY");

    // Selecionar o outro CLI troca as janelas: a dele vira a grande e o PTY muda uma vez, com o tamanho parado.
    let (claude, shell) = (tab_of(&env, "claude"), tab_of(&env, "shell"));
    let (size_claude, size_shell) = (env.deck.tabs[claude].session.size(), env.deck.tabs[shell].session.size());
    env.deck.activate(claude);
    frame(&mut env);
    assert_eq!(big_group(&env).as_deref(), Some("claude"));
    assert_eq!(env.deck.panes[0].rect, big, "o destaque troca de CLI, a janela grande continua no mesmo lugar");
    assert!(env.deck.tabs[claude].resize_pending() && env.deck.tabs[shell].resize_pending(), "as duas janelas mudaram de tamanho");
    // Nada é redimensionado enquanto o tamanho está mudando (ainda dentro dos 150 ms): o tamanho do PTY é o de antes.
    assert_eq!((env.deck.tabs[claude].session.size(), env.deck.tabs[shell].session.size()), (size_claude, size_shell));
    env.pump("o PTY mudar de tamanho", |d| !d.tabs.iter().any(|t| t.resize_pending()));
    assert_ne!(env.deck.tabs[claude].session.size(), size_claude, "o Claude agora está na janela grande");
    for _ in 0..5 {
        frame(&mut env);
    }
    assert!(!env.deck.tabs.iter().any(|t| t.resize_pending()), "depois da troca, nenhum redimensionamento por quadro");

    // Trocar a organização é gravado e o segmento certo volta ao reabrir (a escolha vai para a Config).
    env.deck.persist_layout_now();
    assert_eq!(env.core.config().organization(), Organization::Highlight);
    env.deck.set_organization(Organization::Tabs);
    frame(&mut env);
    assert_eq!(env.deck.panes.len(), 1, "em Abas há uma janela só");
}

#[test]
fn switching_there_and_back_inside_the_delay_resizes_nothing() {
    if !git_available() {
        eprintln!("git não instalado: teste pulado");
        return;
    }
    use agentdeck_core::Organization;
    let mut env = Env::start();
    env.pump("os agentes e o status do git", |d| d.initialized && d.git.status().is_some());
    crate::theme::install_fonts(&env.ctx);
    crate::theme::install_style(&env.ctx);
    let mut out = env.ctx.run_ui(egui::RawInput::default(), |_| {});
    out.textures_delta.clear();
    let frame = |env: &mut Env| {
        let input = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1400.0, 860.0))),
            ..Default::default()
        };
        let Env { deck, ctx, .. } = env;
        let mut out = ctx.run_ui(input, |ui| crate::ui::draw(deck, ui));
        out.textures_delta.clear();
    };
    let tab_of = |env: &Env, kind: &str| env.deck.tabs.iter().position(|t| t.kind == kind).expect("aba");
    env.deck.open_tab("claude", false);
    env.deck.open_tab("shell", false);
    env.deck.set_organization(Organization::Highlight);
    frame(&mut env);
    env.pump("o PTY de cada janela ter o tamanho", |d| !d.tabs.iter().any(|t| t.resize_pending()));
    let (claude, shell) = (tab_of(&env, "claude"), tab_of(&env, "shell"));
    let before = (env.deck.tabs[claude].session.size(), env.deck.tabs[shell].session.size());

    // Aba trocada e trocada de volta: as duas janelas mudaram e voltaram dentro do prazo, e o PTY
    // continua no tamanho que já tinha (o pendente do meio do caminho é cancelado).
    env.deck.activate(claude);
    frame(&mut env);
    assert!(env.deck.tabs[claude].resize_pending() && env.deck.tabs[shell].resize_pending(), "as janelas trocaram de tamanho");
    env.deck.activate(shell);
    frame(&mut env);
    assert!(!env.deck.tabs.iter().any(|t| t.resize_pending()), "voltar ao tamanho atual cancela o redimensionamento");
    std::thread::sleep(Duration::from_millis(800));
    frame(&mut env);
    frame(&mut env);
    assert_eq!((env.deck.tabs[claude].session.size(), env.deck.tabs[shell].session.size()), before, "ir e voltar não reimprime nada");
}

#[test]
fn five_quick_switches_resize_each_tab_at_most_once_to_the_final_size() {
    if !git_available() {
        eprintln!("git não instalado: teste pulado");
        return;
    }
    use agentdeck_core::Organization;
    let mut env = Env::start();
    env.pump("os agentes e o status do git", |d| d.initialized && d.git.status().is_some());
    crate::theme::install_fonts(&env.ctx);
    crate::theme::install_style(&env.ctx);
    let mut out = env.ctx.run_ui(egui::RawInput::default(), |_| {});
    out.textures_delta.clear();
    let frame = |env: &mut Env| {
        let input = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1400.0, 860.0))),
            ..Default::default()
        };
        let Env { deck, ctx, .. } = env;
        let mut out = ctx.run_ui(input, |ui| crate::ui::draw(deck, ui));
        out.textures_delta.clear();
    };
    let tab_of = |env: &Env, kind: &str| env.deck.tabs.iter().position(|t| t.kind == kind).expect("aba");
    env.deck.open_tab("claude", false);
    env.deck.open_tab("shell", false);
    env.deck.set_organization(Organization::Highlight);
    frame(&mut env);
    env.pump("o PTY de cada janela ter o tamanho", |d| !d.tabs.iter().any(|t| t.resize_pending()));
    let (claude, shell) = (tab_of(&env, "claude"), tab_of(&env, "shell"));

    // Cinco trocas seguidas (cada uma dentro do prazo do redimensionamento): o pendente é sempre
    // substituído pelo tamanho novo, então cada aba muda de tamanho uma vez só, para o tamanho final.
    let started = Instant::now();
    for _ in 0..5 {
        let next = if env.deck.active == Some(claude) { shell } else { claude };
        env.deck.activate(next);
        frame(&mut env);
    }
    assert!(started.elapsed() < Duration::from_millis(600), "as cinco trocas precisam caber no prazo de uma");
    let mut changes = [0u32; 2];
    let mut last: Vec<_> = [claude, shell].iter().map(|&i| env.deck.tabs[i].session.size()).collect();
    let limit = Instant::now() + Duration::from_secs(30);
    while env.deck.tabs.iter().any(|t| t.resize_pending()) {
        assert!(Instant::now() < limit, "tempo esgotado esperando o PTY se ajustar");
        env.deck.before_frame(&env.ctx);
        for (n, &i) in [claude, shell].iter().enumerate() {
            let size = env.deck.tabs[i].session.size();
            if size != last[n] {
                changes[n] += 1;
                last[n] = size;
            }
        }
        std::thread::sleep(Duration::from_millis(15));
    }
    assert!(changes.iter().all(|&n| n <= 1), "cada aba muda de tamanho no máximo uma vez, não {changes:?}");
    assert!(changes.contains(&1), "as cinco trocas terminam num tamanho diferente do inicial: {changes:?}");
    let metrics = env.deck.metrics.clone().expect("as fontes instaladas");
    for pane in &env.deck.panes {
        let idx = env.deck.tabs.iter().position(|t| t.uid == pane.shown_uid).expect("aba da janela");
        assert_eq!(env.deck.tabs[idx].session.size(), metrics.grid_size_for(pane.body), "o PTY fica no tamanho final da janela");
    }
}

#[test]
fn a_tab_that_never_stops_printing_gets_resized_at_the_three_second_ceiling() {
    if !git_available() {
        eprintln!("git não instalado: teste pulado");
        return;
    }
    let mut env = Env::start();
    env.pump("os agentes e o status do git", |d| d.initialized && d.git.status().is_some());
    crate::theme::install_fonts(&env.ctx);
    crate::theme::install_style(&env.ctx);
    let mut out = env.ctx.run_ui(egui::RawInput::default(), |_| {});
    out.textures_delta.clear();
    let frame = |env: &mut Env, w: f32, h: f32| {
        let input =
            egui::RawInput { screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(w, h))), ..Default::default() };
        let Env { deck, ctx, .. } = env;
        let mut out = ctx.run_ui(input, |ui| crate::ui::draw(deck, ui));
        out.textures_delta.clear();
    };
    env.deck.open_tab("shell", false);
    frame(&mut env, 1400.0, 860.0);
    env.pump("o PTY ter o tamanho", |d| !d.tabs.iter().any(|t| t.resize_pending()));
    let tab = env.deck.tabs.len() - 1;

    // A janela muda de tamanho e a aba não para de escrever: o próximo redimensionamento espera a
    // saída parar por 300 ms, mas um agente que imprime sem parar não pode segurá-lo para sempre: sai no teto de 3 s.
    frame(&mut env, 1000.0, 700.0);
    assert!(env.deck.tabs[tab].resize_pending(), "a janela mudou de tamanho e o PTY ainda espera");
    let started = Instant::now();
    while env.deck.tabs[tab].resize_pending() {
        assert!(started.elapsed() < Duration::from_secs(10), "o redimensionamento não saiu nem no teto de 3 s");
        *env.deck.tabs[tab].session.shared.last_output.lock() = Some(Instant::now());
        env.deck.before_frame(&env.ctx);
        std::thread::sleep(Duration::from_millis(10));
    }
    let elapsed = started.elapsed();
    assert!(
        elapsed >= Duration::from_millis(2500) && elapsed <= Duration::from_secs(4),
        "com saída sem parar o redimensionamento sai no teto de 3 s, não em {elapsed:?}"
    );
}

#[test]
fn editing_a_file_saves_it_as_it_was_and_asks_before_losing_what_was_typed() {
    if !git_available() {
        eprintln!("git não instalado: teste pulado");
        return;
    }
    let mut env = Env::start();
    env.pump("os agentes e o status do git", |d| d.initialized && d.git.status().is_some());
    let project = env.project();
    let path = env.ws.join("edit.rs");
    std::fs::write(&path, "fn a() {}\r\nfn b() {}\r\n").expect("w");
    env.deck.docs_open_file("edit.rs");
    let uid = env.deck.docs.current_uid(&project).expect("edit.rs");
    env.pump("o arquivo", |d| matches!(d.docs.get(uid).map(|t| &t.state), Some(DocState::Source(_))));
    let src = |env: &mut Env| match &mut env.deck.docs.get_mut(uid).expect("aberto").state {
        DocState::Source(s) => s.clone(),
        other => panic!("esperava código, veio {other:?}"),
    };
    let edit = |env: &mut Env, f: &dyn Fn(&mut crate::docs::Source)| {
        if let DocState::Source(s) = &mut env.deck.docs.get_mut(uid).expect("aberto").state {
            f(s);
        }
    };
    assert_eq!(src(&mut env).text, "fn a() {}\nfn b() {}\n", "a tela trabalha com \\n");

    // Editar e gravar: o disco ganha o texto novo no mesmo fim de linha (CRLF), sem BOM, e a edição deixa de estar suja.
    edit(&mut env, &|s| {
        s.set_editing(true);
        s.text = "fn a() {}\nfn c() {}\n".into();
        s.touched();
    });
    assert!(env.deck.docs.get(uid).expect("aberto").dirty());
    env.deck.push_action(crate::deck::Action::SaveDoc);
    env.deck.before_frame(&env.ctx);
    assert!(src(&mut env).saving, "grava numa thread");
    env.pump("a gravação", |d| !d.docs.get(uid).is_some_and(|t| t.dirty() || matches!(&t.state, DocState::Source(s) if s.saving)));
    assert_eq!(std::fs::read(&path).expect("r"), b"fn a() {}\r\nfn c() {}\r\n");
    // A releitura do watcher sobre o que acabou de ser gravado não avisa de nada.
    env.deck.docs_reload_files();
    env.pump("a releitura", |d| d.docs.get(uid).is_some_and(|t| matches!(&t.state, DocState::Source(s) if s.external.is_none())));
    assert!(!env.deck.docs.get(uid).expect("aberto").dirty());

    // Um agente muda o arquivo com edição não salva: pergunta (manter a minha / recarregar) e nada se perde.
    edit(&mut env, &|s| {
        s.text = "fn a() {}\nfn minha() {}\n".into();
        s.touched();
    });
    std::fs::write(&path, "fn a() {}\r\nfn do_agente() {}\r\n").expect("w");
    env.deck.docs_reload_files();
    env.pump("o aviso", |d| d.docs.get(uid).is_some_and(|t| matches!(&t.state, DocState::Source(s) if s.external.is_some())));
    assert_eq!(src(&mut env).text, "fn a() {}\nfn minha() {}\n", "o que o usuário digitou continua na tela");
    // Sem escolher, o Ctrl+S não grava por cima do que o agente fez.
    env.deck.docs_save(uid, crate::docs::After::Nothing);
    assert!(!src(&mut env).saving);
    assert_eq!(std::fs::read(&path).expect("r"), b"fn a() {}\r\nfn do_agente() {}\r\n");
    edit(&mut env, &|s| s.keep_mine());
    env.deck.docs_save(uid, crate::docs::After::Nothing);
    env.pump("a gravação por cima", |d| {
        !d.docs.get(uid).is_some_and(|t| t.dirty() || matches!(&t.state, DocState::Source(s) if s.saving))
    });
    assert_eq!(std::fs::read(&path).expect("r"), b"fn a() {}\r\nfn minha() {}\r\n");

    // Fechar a aba com edição pergunta; cancelar mantém tudo; "salvar" grava e fecha; "não salvar" fecha sem gravar.
    edit(&mut env, &|s| {
        s.text.push_str("// mais\n");
        s.touched();
    });
    env.deck.docs_request_close(uid);
    assert_eq!(env.deck.docs.confirm, Some(crate::docs::Unsaved::Tab(uid)));
    assert!(env.deck.docs.get(uid).is_some(), "a aba continua aberta enquanto pergunta");
    env.deck.docs_answer(crate::docs::Answer::Cancel);
    assert!(env.deck.docs.confirm.is_none() && env.deck.docs.get(uid).is_some() && env.deck.docs.get(uid).expect("aberto").dirty());
    env.deck.docs_request_close(uid);
    env.deck.docs_answer(crate::docs::Answer::Save);
    env.pump("gravar e fechar", |d| d.docs.get(uid).is_none());
    assert_eq!(std::fs::read(&path).expect("r"), b"fn a() {}\r\nfn minha() {}\r\n// mais\r\n");
    env.deck.docs_open_file("edit.rs");
    let again = env.deck.docs.current_uid(&project).expect("edit.rs");
    env.pump("o arquivo de novo", |d| matches!(d.docs.get(again).map(|t| &t.state), Some(DocState::Source(_))));
    if let Some(DocState::Source(s)) = env.deck.docs.get_mut(again).map(|t| &mut t.state) {
        s.text.push_str("// descartado\n");
    }
    env.deck.docs_request_close(again);
    env.deck.docs_answer(crate::docs::Answer::Discard);
    assert!(env.deck.docs.get(again).is_none());
    assert_eq!(std::fs::read(&path).expect("r"), b"fn a() {}\r\nfn minha() {}\r\n// mais\r\n", "descartar não grava");

    // Fechar o app: sem edição, sai direto; com edição, pergunta (e "sair sem salvar" sai).
    env.deck.docs_open_file("edit.rs");
    let third = env.deck.docs.current_uid(&project).expect("edit.rs");
    env.pump("o arquivo outra vez", |d| matches!(d.docs.get(third).map(|t| &t.state), Some(DocState::Source(_))));
    if let Some(DocState::Source(s)) = env.deck.docs.get_mut(third).map(|t| &mut t.state) {
        s.text.push_str("// sem gravar\n");
    }
    env.deck.request_quit();
    assert!(!env.deck.quit_requested, "com edição não salva o app pergunta antes de sair");
    assert_eq!(env.deck.docs.confirm, Some(crate::docs::Unsaved::Quit));
    env.deck.docs_answer(crate::docs::Answer::Cancel);
    assert!(!env.deck.quit_requested);
    env.deck.request_quit();
    env.deck.docs_answer(crate::docs::Answer::Save);
    env.pump("gravar tudo e sair", |d| d.quit_requested);
    assert!(std::fs::read(&path).expect("r").ends_with(b"// sem gravar\r\n"));
    env.deck.quit_requested = false;
    env.deck.request_quit();
    assert!(env.deck.quit_requested, "sem edição não há o que perguntar");
}

/// Um quadro da janela inteira (1280 × 860) com os eventos de `events`; devolve o que foi copiado para a área de transferência.
fn draw_frame(env: &mut Env, events: Vec<egui::Event>) -> Vec<String> {
    let input = egui::RawInput {
        screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1280.0, 860.0))),
        events,
        ..Default::default()
    };
    let Env { deck, ctx, .. } = env;
    let mut out = ctx.run_ui(input, |ui| crate::ui::draw(deck, ui));
    out.textures_delta.clear();
    out.platform_output
        .commands
        .iter()
        .filter_map(|c| match c {
            egui::OutputCommand::CopyText(t) => Some(t.clone()),
            _ => None,
        })
        .collect()
}

#[test]
fn the_editor_takes_typing_the_search_and_the_diff_selects_and_copies() {
    if !git_available() {
        eprintln!("git não instalado: teste pulado");
        return;
    }
    let mut env = Env::start();
    env.pump("os agentes e o status do git", |d| d.initialized && d.git.status().is_some());
    crate::theme::install_fonts(&env.ctx);
    crate::theme::install_style(&env.ctx);
    let mut out = env.ctx.run_ui(egui::RawInput::default(), |_| {});
    out.textures_delta.clear();
    let project = env.project();
    std::fs::write(env.ws.join("e.rs"), "fn a() {}\nfn b() {}\n").expect("w");
    env.deck.docs_open_file("e.rs");
    let uid = env.deck.docs.current_uid(&project).expect("e.rs");
    env.pump("o arquivo", |d| matches!(d.docs.get(uid).map(|t| &t.state), Some(DocState::Source(_))));
    let text = |env: &Env| match &env.deck.docs.get(uid).expect("aberto").state {
        DocState::Source(s) => (s.text.clone(), s.dirty(), s.version),
        other => panic!("esperava código, veio {other:?}"),
    };
    draw_frame(&mut env, vec![]);
    // Em leitura o teclado não muda nada.
    let editor = egui::Id::new(("doc-src", uid));
    env.ctx.memory_mut(|m| m.request_focus(editor));
    draw_frame(&mut env, vec![]);
    draw_frame(&mut env, vec![egui::Event::Text("Z".into())]);
    assert_eq!(text(&env).0, "fn a() {}\nfn b() {}\n", "só leitura não aceita digitação");

    // Em edição, o que se digita entra no texto, marca como não salvo e conta uma versão nova.
    if let Some(DocState::Source(s)) = env.deck.docs.get_mut(uid).map(|t| &mut t.state) {
        s.set_editing(true);
    }
    draw_frame(&mut env, vec![]);
    env.ctx.memory_mut(|m| m.request_focus(editor));
    draw_frame(&mut env, vec![]);
    let before = text(&env).2;
    draw_frame(&mut env, vec![egui::Event::Text("Z".into())]);
    let (typed, dirty, version) = text(&env);
    assert!(typed.contains('Z') && dirty && version > before, "{typed:?}");
    draw_frame(&mut env, vec![]);

    // Ctrl+F abre a busca e o campo recebe o texto; o galley marca as ocorrências e a contagem aparece.
    env.deck.push_action(crate::deck::Action::FindInDoc);
    env.deck.before_frame(&env.ctx);
    draw_frame(&mut env, vec![]);
    let find_field = egui::Id::new(("doc-find", uid));
    env.ctx.memory_mut(|m| m.request_focus(find_field));
    draw_frame(&mut env, vec![egui::Event::Text("fn".into())]);
    draw_frame(&mut env, vec![]);
    let tab = env.deck.docs.get(uid).expect("aberto");
    assert!(tab.find.open && tab.find.query == "fn" && tab.find.count() == 2, "{:?}", tab.find);

    // Diff: seleciona arrastando e copia com Ctrl+C só o código das linhas (sem número, sem + nem −).
    let id = env.create_from_form("Para o diff", true);
    env.agent_commits(&id, "novo.rs", "// um\nfn dois() {}\n");
    let (diff_uid, _) = env.open_task_diff(&id);
    draw_frame(&mut env, vec![]);
    draw_frame(&mut env, vec![]);
    let at = |x: f32, y: f32| egui::Pos2::new(x, y);
    let click = |pos| vec![egui::Event::PointerMoved(pos)];
    let button =
        |pos, pressed| egui::Event::PointerButton { pos, button: egui::PointerButton::Primary, pressed, modifiers: egui::Modifiers::NONE };
    let (start, end) = (at(520.0, 125.0), at(820.0, 240.0));
    draw_frame(&mut env, click(start));
    draw_frame(&mut env, vec![button(start, true)]);
    draw_frame(&mut env, click(at(500.0, 160.0)));
    draw_frame(&mut env, click(end));
    draw_frame(&mut env, vec![button(end, false)]);
    let copied = draw_frame(&mut env, vec![egui::Event::Copy]);
    assert_eq!(copied.len(), 1, "um Ctrl+C copia uma vez: {copied:?}");
    assert!(!copied[0].is_empty() && !copied[0].contains("+//") && !copied[0].starts_with('+'), "{:?}", copied[0]);
    assert!(copied[0].contains("fn dois() {}"), "{:?}", copied[0]);
    let _ = diff_uid;
}

#[test]
fn every_document_state_draws_without_trouble() {
    if !git_available() {
        eprintln!("git não instalado: teste pulado");
        return;
    }
    let mut env = Env::start();
    env.pump("os agentes e o status do git", |d| d.initialized && d.git.status().is_some());
    crate::theme::install_fonts(&env.ctx);
    crate::theme::install_style(&env.ctx);
    let mut out = env.ctx.run_ui(egui::RawInput::default(), |_| {});
    out.textures_delta.clear();
    for item in [
        "docs-demo",
        "docs-demo-file",
        "docs-demo-md",
        "docs-demo-report",
        "docs-demo-unreadable",
        "docs-demo-many",
        "docs-demo-edit",
        "docs-demo-find",
        "docs-demo-diff-find",
        "docs-demo-external",
        "docs-demo-confirm",
        "docs-demo-large",
        "docs-demo-md-code",
    ] {
        env.deck.docs = crate::docs::Docs::default();
        crate::docs_demo::apply(&mut env.deck, item);
        draw_frame(&mut env, vec![]);
        draw_frame(&mut env, vec![]);
    }
}

/// Roda só o trabalho sem quadro (`Deck::background`, o que o laço faz com a janela escondida ou minimizada) até `done`.
/// Devolve o último prazo pedido.
fn background_until(env: &mut Env, what: &str, done: impl Fn(&Deck) -> bool) -> Option<Instant> {
    let limit = Instant::now() + Duration::from_secs(30);
    loop {
        let next = env.deck.background();
        if done(&env.deck) {
            return next;
        }
        assert!(Instant::now() < limit, "tempo esgotado esperando: {what}");
        std::thread::sleep(Duration::from_millis(15));
    }
}

fn screen_has(deck: &Deck, tab: usize, needle: &str) -> bool {
    crate::terminal::record::grid_text(&deck.tabs[tab].session.shared.emu.lock()).contains(needle)
}

#[test]
fn a_hidden_window_still_types_the_queued_messages_and_the_late_answers_and_never_resizes_the_terminals() {
    if !git_available() {
        eprintln!("git não instalado: teste pulado");
        return;
    }
    use agentdeck_core::Delivery;
    let mut env = Env::start();
    env.pump("os agentes e o status do git", |d| d.initialized && d.git.status().is_some());
    crate::theme::install_fonts(&env.ctx);
    crate::theme::install_style(&env.ctx);
    let mut out = env.ctx.run_ui(egui::RawInput::default(), |_| {});
    out.textures_delta.clear();
    // Um terminal de verdade (o PTY, a saída e o eco são os do sistema) com a janela ainda à vista: um quadro o ajusta.
    env.deck.open_tab("shell", false);
    let input =
        egui::RawInput { screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1280.0, 860.0))), ..Default::default() };
    let Env { deck, ctx, .. } = &mut env;
    let mut out = ctx.run_ui(input, |ui| crate::ui::draw(deck, ui));
    out.textures_delta.clear();
    env.pump("o PTY ter o tamanho", |d| !d.tabs.iter().any(|t| t.resize_pending()));
    let tab = env.deck.tabs.len() - 1;
    let (project, ident) = (env.project(), env.deck.tabs[tab].ident.clone());
    let size = env.deck.tabs[tab].session.size();
    // O sinal do hook (como o Claude e o Codex fazem) tira a espera do "terminal parado" do caminho.
    env.deck.on_agent_signal(&project, &ident.id(), true);
    env.deck.on_agent_signal(&project, &ident.id(), false);
    let delivery = |prompt: &str, answer: bool| Delivery {
        request: 0,
        project: project.clone(),
        ident: ident.clone(),
        create: false,
        from: "outro".into(),
        prompt: prompt.into(),
        answer,
        boot: None,
    };

    // Janela minimizada: nenhum quadro, só `background`. A mensagem de outro agente é digitada quando a aba fica livre.
    background_until(&mut env, "o terminal falar", |d| {
        d.tabs[tab].session.shared.output_seq.load(std::sync::atomic::Ordering::Relaxed) > 0
    });
    env.deck.on_deliver(delivery("echo mensagem-minimizada-1", false));
    background_until(&mut env, "a mensagem ser digitada", |d| screen_has(d, tab, "mensagem-minimizada-1"));

    // A resposta "digitada depois" sai do mesmo jeito. (A aba é de um shell, que a regra recusa por segurança: faz-se
    // passar por um CLI só para a regra; o PTY continua o de um shell.)
    env.deck.tabs[tab].kind = "claude".into();
    background_until(&mut env, "a aba ficar livre", |d| !d.tabs[tab].busy);
    env.deck.on_deliver(delivery("echo resposta-tardia-2", true));
    background_until(&mut env, "a resposta ser digitada", |d| screen_has(d, tab, "resposta-tardia-2"));

    // Sem nenhum quadro, o terminal não muda de tamanho nem pede para mudar.
    assert!(!env.deck.tabs[tab].resize_pending(), "background não ajusta terminais ao tamanho da janela");
    assert_eq!(env.deck.tabs[tab].session.size(), size);

    // Sem atividade o trabalho acaba: nenhum prazo, e o laço dorme até um evento do core (nada de CPU à toa).
    let limit = Instant::now() + Duration::from_secs(30);
    while env.deck.background().is_some() {
        assert!(Instant::now() < limit, "ainda há prazos agendados com a aba parada");
        std::thread::sleep(Duration::from_millis(50));
    }
}
