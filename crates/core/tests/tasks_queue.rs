//! Escopo de escrita e fila da mesa de tarefas, com o git real (pula com aviso se o `git` não estiver instalado):
//! a tarefa cujo escopo se sobrepõe ao de outra em andamento espera na fila e começa sozinha, partindo da base
//! já com a mesclagem da outra.
use agentdeck_core::{Task, TaskError, TaskSpec, TaskStatus};

#[path = "support/tasks_env.rs"]
mod env;
use env::*;

macro_rules! need_git {
    () => {
        if git_exe().is_none() {
            eprintln!("git não instalado: teste pulado");
            return;
        }
    };
}

fn scoped(title: &str, scope: &[&str]) -> TaskSpec {
    TaskSpec { scope: scope.iter().map(|s| (*s).to_owned()).collect(), ..spec(title) }
}

/// Cria e inicia uma tarefa com escopo; devolve como ficou depois do `start`.
fn start_scoped(env: &Env, title: &str, scope: &[&str]) -> Task {
    let t = env.core.task_create(&env.project, scoped(title, scope)).expect("create");
    env.core.task_start(&env.project, &t.id).expect("start")
}

/// Leva a tarefa em andamento até a revisão, com um commit em `file`.
fn deliver(env: &Env, t: &Task, file: &str) {
    if let Some(dir) = std::path::Path::new(file).parent().filter(|d| !d.as_os_str().is_empty()) {
        std::fs::create_dir_all(t.worktree.join(dir)).expect("mkdir");
    }
    commit_file(&t.worktree, file, "conteúdo\n", &format!("feat: {}", t.title));
    env.core.task_done(&env.project, &t.id, "Feito.").expect("done");
}

fn status(env: &Env, id: &str) -> TaskStatus {
    env.core.task(&env.project, id).expect("tarefa").status
}

#[test]
fn an_overlapping_scope_waits_in_the_queue_and_starts_when_the_other_is_merged() {
    need_git!();
    let env = start();
    let core = &env.core;
    let a = start_scoped(&env, "Mexe no core", &["crates/core"]);
    assert_eq!(a.status, TaskStatus::Running);
    assert_eq!(a.scope, ["crates/core"]);
    events(&env);

    let b = start_scoped(&env, "Mexe no git.rs", &["crates/core/src/git.rs"]);
    assert_eq!((b.status, b.waiting_for.as_deref()), (TaskStatus::Queued, Some("t1")));
    assert!(b.queued_ms.is_some() && b.started_ms.is_none());
    assert!(b.events.iter().any(|e| e.text.contains("na fila") && e.text.contains("t1")), "{:?}", b.events);
    // Na fila não há worktree, branch, instância nem entrega ao agente.
    assert!(!b.worktree.exists());
    assert_eq!(branches(&env.ws), ["main", a.branch.as_str()]);
    assert!(deliveries(&events(&env)).iter().all(|(to, _)| to != "claude@t2"));
    assert!(!core.project_instance_ids(&env.project).contains(&"claude@t2".to_owned()));
    // Iniciar de novo enquanto o escopo está ocupado só confere: continua na fila.
    assert_eq!(core.task_start(&env.project, "t2").expect("start de novo").status, TaskStatus::Queued);
    // A entrega da outra para revisão não libera: em revisão ela ainda segura.
    deliver(&env, &a, "crates/core/novo.rs");
    assert_eq!(status(&env, "t2"), TaskStatus::Queued);

    // Mescla a primeira: a segunda começa sozinha, já com a mesclagem.
    core.task_merge(&env.project, "t1").expect("merge");
    let b = core.task(&env.project, "t2").expect("t2");
    assert_eq!(b.status, TaskStatus::Running, "{:?}", b.events);
    assert!(b.waiting_for.is_none() && b.queued_ms.is_none() && b.started_ms.is_some());
    assert!(b.worktree.join("crates/core/novo.rs").is_file(), "parte da base com a mesclagem da t1");
    assert_eq!(git(&b.worktree, &["rev-parse", "main"]), git(&env.ws, &["rev-parse", "main"]));
    assert!(deliveries(&events(&env)).iter().any(|(to, prompt)| to == "claude@t2" && prompt.contains("Mexe no git.rs")));
    assert!(core.project_instance_ids(&env.project).contains(&"claude@t2".to_owned()));
    core.shutdown();
}

#[test]
fn discarding_the_task_in_front_also_starts_the_queued_one() {
    need_git!();
    let env = start();
    let core = &env.core;
    start_scoped(&env, "Primeira", &["crates/core"]);
    let b = start_scoped(&env, "Segunda", &["crates/core/src"]);
    assert_eq!(b.status, TaskStatus::Queued);
    core.task_discard(&env.project, "t1").expect("discard");
    assert_eq!(status(&env, "t2"), TaskStatus::Running);
    core.shutdown();
}

#[test]
fn a_task_that_fails_to_start_frees_the_scope_for_the_queue() {
    need_git!();
    let env = start();
    let core = &env.core;
    let a = core.task_create(&env.project, scoped("Vai falhar", &["crates/core"])).expect("create");
    // A pasta da worktree já existe: o início falha e a tarefa vira `Failed`.
    std::fs::create_dir_all(&a.worktree).expect("mkdir");
    let b = core.task_create(&env.project, scoped("Espera", &["crates/core"])).expect("create");
    // Sozinha, a t2 não espera ninguém (a t1 ainda é rascunho).
    let a_started = core.task_start(&env.project, &a.id);
    assert!(matches!(a_started, Err(TaskError::Failed(_))), "{a_started:?}");
    assert_eq!(status(&env, "t1"), TaskStatus::Failed);
    assert_eq!(core.task_start(&env.project, &b.id).expect("start").status, TaskStatus::Running);
    // Nova tentativa da que falhou, com o escopo ocupado: recusa e continua falha, sem entrar na fila.
    std::fs::remove_dir_all(&a.worktree).expect("rm");
    match core.task_start(&env.project, &a.id) {
        Err(TaskError::Conflict(m)) => assert!(m.contains("t2"), "{m}"),
        other => panic!("{other:?}"),
    }
    assert_eq!(status(&env, "t1"), TaskStatus::Failed);
    core.shutdown();
}

#[test]
fn disjoint_scopes_start_together() {
    need_git!();
    let env = start();
    let a = start_scoped(&env, "App", &["crates/app"]);
    let b = start_scoped(&env, "Cli", &["crates/cli"]);
    let c = start_scoped(&env, "Prefixo parecido", &["crates/ap"]);
    for t in [a, b, c] {
        assert_eq!(t.status, TaskStatus::Running, "{}", t.title);
        assert!(t.waiting_for.is_none());
    }
    env.core.shutdown();
}

#[test]
fn memory_and_cargo_lock_do_not_reserve_anything() {
    need_git!();
    let env = start();
    let a = start_scoped(&env, "A", &["crates/app", "memory/pendencias.md", "Cargo.lock"]);
    let b = start_scoped(&env, "B", &["crates/cli", "memory/pendencias.md", "Cargo.lock"]);
    let c = start_scoped(&env, "C", &["memory/pendencias.md"]);
    assert_eq!((a.status, b.status, c.status), (TaskStatus::Running, TaskStatus::Running, TaskStatus::Running));
    env.core.shutdown();
}

#[test]
fn a_task_without_scope_neither_queues_nor_holds_anyone() {
    need_git!();
    let env = start();
    let a = start_scoped(&env, "Com escopo", &["crates/core"]);
    let none = env.core.task_create(&env.project, spec("Sem escopo")).expect("create");
    assert!(none.scope.is_empty());
    assert_eq!(env.core.task_start(&env.project, &none.id).expect("start").status, TaskStatus::Running);
    let b = start_scoped(&env, "Outra sem escopo e uma que espera", &["crates/core"]);
    assert_eq!((a.status, b.status, b.waiting_for.as_deref()), (TaskStatus::Running, TaskStatus::Queued, Some("t1")));
    env.core.shutdown();
}

#[test]
fn awaiting_validation_does_not_hold_the_queue() {
    need_git!();
    let env = start();
    let core = &env.core;
    let a = start_scoped(&env, "Primeira", &["crates/core"]);
    deliver(&env, &a, "crates/core/a.rs");
    core.task_merge(&env.project, "t1").expect("merge");
    assert_eq!(status(&env, "t1"), TaskStatus::Validating);
    // O código já está na base: a segunda começa na hora, com ele.
    let b = start_scoped(&env, "Segunda", &["crates/core"]);
    assert_eq!(b.status, TaskStatus::Running);
    assert!(b.worktree.join("crates/core/a.rs").is_file());
    // Reprovar a primeira a põe de volta em andamento, sobreposta à segunda: só um aviso.
    core.task_reject(&env.project, "t1", "não funcionou").expect("reject");
    let a = core.task(&env.project, "t1").expect("t1");
    assert_eq!(a.status, TaskStatus::Running);
    assert!(a.events.iter().any(|e| e.text.contains("aviso") && e.text.contains("t2")), "{:?}", a.events);
    core.shutdown();
}

#[test]
fn the_queue_keeps_its_order_and_waits_for_whoever_is_ahead() {
    need_git!();
    let env = start();
    let core = &env.core;
    let a = start_scoped(&env, "A", &["crates/core"]);
    let b = start_scoped(&env, "B", &["crates/core/src"]);
    // A t3 não colide com a t1 (em andamento) além de pelo escopo da t2: entra atrás dela.
    let c = start_scoped(&env, "C", &["crates/core/src/git.rs", "crates/app"]);
    let d = start_scoped(&env, "D", &["crates/app"]);
    assert_eq!((a.status, b.status, c.status, d.status), (TaskStatus::Running, TaskStatus::Queued, TaskStatus::Queued, TaskStatus::Queued));
    assert_eq!(c.waiting_for.as_deref(), Some("t1"));
    assert_eq!(d.waiting_for.as_deref(), Some("t3"));
    deliver(&env, &a, "crates/core/x.rs");
    core.task_merge(&env.project, "t1").expect("merge");
    // A t1 saiu: a t2 (primeira da fila) começa; a t3 passa a esperar a t2; a t4 segue atrás da t3.
    assert_eq!(status(&env, "t2"), TaskStatus::Running);
    let c = core.task(&env.project, "t3").expect("t3");
    assert_eq!((c.status, c.waiting_for.as_deref()), (TaskStatus::Queued, Some("t2")));
    assert_eq!(core.task(&env.project, "t4").expect("t4").waiting_for.as_deref(), Some("t3"));
    core.task_discard(&env.project, "t2").expect("discard");
    assert_eq!(status(&env, "t3"), TaskStatus::Running);
    let d = core.task(&env.project, "t4").expect("t4");
    assert_eq!((d.status, d.waiting_for.as_deref()), (TaskStatus::Queued, Some("t3")));
    core.shutdown();
}

#[test]
fn the_queue_survives_a_restart_and_starts_when_the_other_ends() {
    need_git!();
    let env = start();
    start_scoped(&env, "Primeira", &["crates/core"]);
    let b = start_scoped(&env, "Segunda", &["crates/core/src"]);
    assert_eq!(b.status, TaskStatus::Queued);
    let (root, project, config) = (env.root.clone(), env.project.clone(), env.config.clone());
    let Env { _tmp, core: first, rx, .. } = env;
    first.shutdown();
    drop((first, rx));

    let (core, _rx) = start_core(&root, config, true);
    let b = core.task(&project, "t2").expect("t2");
    assert_eq!((b.status, b.waiting_for.as_deref()), (TaskStatus::Queued, Some("t1")));
    assert_eq!(b.scope, ["crates/core/src"]);
    // Nada começa enquanto a primeira segue em andamento.
    core.tasks_advance_queue(&project);
    assert_eq!(core.task(&project, "t2").expect("t2").status, TaskStatus::Queued);
    core.task_discard(&project, "t1").expect("discard");
    assert_eq!(core.task(&project, "t2").expect("t2").status, TaskStatus::Running);
    core.shutdown();
    drop(_tmp);
}

#[test]
fn advancing_the_queue_catches_up_after_a_restart() {
    need_git!();
    let env = start();
    let a = start_scoped(&env, "Primeira", &["crates/core"]);
    start_scoped(&env, "Segunda", &["crates/core"]);
    let (root, project, config, ws) = (env.root.clone(), env.project.clone(), env.config.clone(), env.ws.clone());
    let Env { _tmp, core: first, rx, .. } = env;
    first.shutdown();
    drop((first, rx));
    // Com o app fechado a primeira some do caminho (aqui, o registro dela passa a descartada à mão).
    let file = root.join("dados").join("tarefas").join(format!("{project}.json"));
    let text = std::fs::read_to_string(&file).expect("json");
    let mut json: serde_json::Value = serde_json::from_str(&text).expect("parse");
    json["tasks"][0]["status"] = serde_json::json!("discarded");
    std::fs::write(&file, serde_json::to_string(&json).expect("ser")).expect("write");
    git(&ws, &["worktree", "remove", "--force", &a.worktree.to_string_lossy()]);

    let (core, _rx) = start_core(&root, config, true);
    assert_eq!(core.task(&project, "t2").expect("t2").status, TaskStatus::Queued);
    core.tasks_advance_queue(&project);
    assert_eq!(core.task(&project, "t2").expect("t2").status, TaskStatus::Running);
    core.shutdown();
    drop(_tmp);
}

#[test]
fn done_warns_about_files_outside_the_scope_without_blocking() {
    need_git!();
    let env = start();
    let core = &env.core;
    let t = start_scoped(&env, "Sai do escopo", &["crates/cli", "memory/pendencias.md"]);
    std::fs::create_dir_all(t.worktree.join("crates/cli")).expect("mkdir");
    std::fs::create_dir_all(t.worktree.join("crates/core")).expect("mkdir");
    std::fs::create_dir_all(t.worktree.join("memory")).expect("mkdir");
    std::fs::write(t.worktree.join("crates/cli/dentro.rs"), "x").expect("w");
    std::fs::write(t.worktree.join("crates/core/fora.rs"), "x").expect("w");
    std::fs::write(t.worktree.join("memory/GERAL.md"), "x").expect("w");
    std::fs::write(t.worktree.join("Cargo.lock"), "x").expect("w");
    git(&t.worktree, &["add", "-A"]);
    git(&t.worktree, &["commit", "-q", "-m", "feat: tudo"]);
    let done = core.task_done(&env.project, &t.id, "Feito.").expect("done");
    assert_eq!(done.status, TaskStatus::Review, "o aviso não bloqueia");
    assert_eq!(done.outside_scope, ["crates/core/fora.rs"]);
    assert!(
        done.events.iter().any(|e| e.text.contains("mexeu fora do escopo") && e.text.contains("crates/core/fora.rs")),
        "{:?}",
        done.events
    );
    // E a mesclagem segue normal.
    core.task_merge(&env.project, &t.id).expect("merge");

    // Sem escopo declarado não há de que se desviar.
    let free = core.task_create(&env.project, spec("Livre")).expect("create");
    let free = core.task_start(&env.project, &free.id).expect("start");
    commit_file(&free.worktree, "qualquer.txt", "x", "feat: qualquer");
    assert!(core.task_done(&env.project, &free.id, "Feito.").expect("done").outside_scope.is_empty());
    core.shutdown();
}

#[test]
fn the_scope_is_normalized_and_validated_on_create() {
    need_git!();
    let env = start();
    let core = &env.core;
    let t = core
        .task_create(&env.project, scoped("Normaliza", &[" .\\crates\\core\\ ", "crates/core", "Crates/Core/", "crates/cli/src/lib.rs"]))
        .expect("create");
    assert_eq!(t.scope, ["crates/core", "crates/cli/src/lib.rs"]);
    for bad in ["/abs", "C:\\x", "../fora", "crates/*.rs", "crates/?", " "] {
        match core.task_create(&env.project, scoped("Ruim", &[bad])) {
            Err(TaskError::Invalid(_)) => {}
            other => panic!("{bad:?}: {other:?}"),
        }
    }
    // Nada disso gastou número.
    assert_eq!(core.task_create(&env.project, spec("Seguinte")).expect("create").id, "t2");
    core.shutdown();
}
