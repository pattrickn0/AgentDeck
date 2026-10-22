//! Mesa de tarefas com o git real (pula com aviso se o `git` não estiver instalado): criar, iniciar,
//! relatório, devolver, mesclar, validar, reprovar, descartar e persistência. A ponte fica em `tasks_bridge.rs`.
use std::process::Command;

use agentdeck_core::util::same_path;
use agentdeck_core::{CoreEvent, Ident, TaskError, TaskSpec, TaskStatus};

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

#[test]
fn create_validates_and_records_the_task() {
    need_git!();
    let env = start();
    let core = &env.core;
    let t = core
        .task_create(
            &env.project,
            TaskSpec { model: Some("opus".into()), creator: Some("usuário".into()), ..spec("  Corrigir o botão\n de Ação! ") },
        )
        .expect("create");
    assert_eq!((t.id.as_str(), t.status), ("t1", TaskStatus::Draft));
    assert_eq!(t.title, "Corrigir o botão de Ação!");
    assert_eq!((t.base.as_str(), t.kind.as_str(), t.model.as_deref()), ("main", "claude", Some("opus")));
    assert_eq!(t.branch, "tarefa/t1-corrigir-o-botao-de-acao");
    assert_eq!(t.instance, "claude@t1");
    assert!(same_path(&t.worktree, &env.ws.join(".agentdeck").join("worktrees").join("t1")), "{}", t.worktree.display());
    assert!(t.created_ms > 0 && t.started_ms.is_none() && t.events.len() == 1);
    // Rascunho: nada foi criado no git.
    assert!(!t.worktree.exists());
    assert_eq!(branches(&env.ws), ["main"]);
    assert!(evs_tasks_changed(&events(&env)), "TasksChanged a cada gravação");
    // O número seguinte; a leitura devolve o mesmo.
    let t2 = core.task_create(&env.project, spec("Outra")).expect("create 2");
    assert_eq!(t2.id, "t2");
    assert_eq!(core.tasks(&env.project).iter().map(|t| t.id.as_str()).collect::<Vec<_>>(), ["t1", "t2"]);
    assert_eq!(core.task(&env.project, "T1").map(|t| t.title), Some("Corrigir o botão de Ação!".to_owned()));
    assert!(core.task(&env.project, "t9").is_none() && core.task(&env.project, "claude").is_none());
    // A base pedida precisa ser uma branch local.
    git(&env.ws, &["branch", "estavel"]);
    let on_base = core.task_create(&env.project, TaskSpec { base: Some("estavel".into()), ..spec("Na estável") }).expect("base pedida");
    assert_eq!(on_base.base, "estavel");

    let invalid = |s: TaskSpec| match core.task_create(&env.project, s) {
        Err(TaskError::Invalid(m)) => m,
        other => panic!("esperava Invalid, veio {other:?}"),
    };
    assert!(invalid(spec("  \n ")).contains("título"));
    assert!(invalid(TaskSpec { kind: "shell".into(), ..spec("x") }).contains("agente desconhecido ou não instalado"));
    assert!(invalid(TaskSpec { kind: "opencode".into(), ..spec("x") }).contains("não instalado"));
    assert!(invalid(TaskSpec { kind: "claude@dev".into(), ..spec("x") }).contains("informe só o modelo"));
    assert!(invalid(TaskSpec { model: Some("a b;c".into()), ..spec("x") }).contains("modelo inválido"));
    assert!(invalid(TaskSpec { kind: "aider".into(), model: Some("gpt".into()), ..spec("x") })
        .contains("o agente aider não aceita escolher modelo"));
    assert!(invalid(TaskSpec { base: Some("nao-existe".into()), ..spec("x") }).contains("não é uma branch local"));
    assert!(invalid(TaskSpec { base: Some("--exec".into()), ..spec("x") }).contains("não é uma branch local"));
    // Nada disso gastou número: o próximo é t4.
    assert_eq!(core.task_create(&env.project, spec("Quarta")).expect("create").id, "t4");
    core.shutdown();
}

fn evs_tasks_changed(evs: &[CoreEvent]) -> bool {
    evs.iter().any(|e| matches!(e, CoreEvent::TasksChanged { .. }))
}

#[test]
fn create_needs_a_repository_with_a_commit_and_a_branch() {
    need_git!();
    // Projeto que não é repositório.
    let env = start_with(true, false);
    match env.core.task_create(&env.project, spec("x")) {
        Err(TaskError::Invalid(m)) => assert!(m.contains("não é um repositório git"), "{m}"),
        other => panic!("{other:?}"),
    }
    // Repositório sem nenhum commit.
    git(&env.ws, &["init", "-q"]);
    match env.core.task_create(&env.project, spec("x")) {
        Err(TaskError::Invalid(m)) => assert!(m.contains("nenhum commit"), "{m}"),
        other => panic!("{other:?}"),
    }
    // HEAD solto, sem base informada.
    std::fs::write(env.ws.join("a.txt"), "x").expect("w");
    git(&env.ws, &["add", "-A"]);
    git(&env.ws, &["commit", "-q", "-m", "inicial"]);
    git(&env.ws, &["checkout", "-q", "--detach"]);
    match env.core.task_create(&env.project, spec("x")) {
        Err(TaskError::Invalid(m)) => assert!(m.contains("HEAD solto"), "{m}"),
        other => panic!("{other:?}"),
    }
    let ok = env.core.task_create(&env.project, TaskSpec { base: Some("main".into()), ..spec("x") });
    assert!(ok.is_ok(), "{ok:?}");
    // Projeto desconhecido.
    assert!(matches!(env.core.task_create("pxxxxxxxx", spec("x")), Err(TaskError::NotFound(_))));
    env.core.shutdown();
}

#[test]
fn start_makes_the_worktree_the_instance_and_delivers_the_brief() {
    need_git!();
    let env = start();
    let core = &env.core;
    // Quem pede é o agente `claude`, aberto: vira o chefe da instância da tarefa.
    let boss = core.register_instance(&env.project, "claude");
    let t = core
        .task_create(&env.project, TaskSpec { model: Some("opus".into()), creator: Some(boss.id()), ..spec("Corrigir o login") })
        .expect("create");
    events(&env);
    let t = core.task_start(&env.project, &t.id).expect("start");
    assert_eq!(t.status, TaskStatus::Running);
    assert!(t.started_ms.is_some() && t.error.is_none());

    // Worktree e branch existem; o repositório principal continua limpo (a pasta é excluída pelo git).
    assert!(t.worktree.join(".git").exists() && t.worktree.join("a.txt").is_file());
    assert!(branches(&env.ws).contains(&t.branch), "{:?}", branches(&env.ws));
    assert_eq!(git(&t.worktree, &["symbolic-ref", "--short", "HEAD"]).trim(), t.branch);
    assert_eq!(worktree_count(&env.ws), 2);
    assert_eq!(git(&env.ws, &["status", "--porcelain"]).trim(), "", "o repositório principal fica limpo");
    let exclude = std::fs::read_to_string(env.ws.join(".git").join("info").join("exclude")).expect("exclude");
    assert_eq!(exclude.lines().filter(|l| l.trim() == "/.agentdeck/").count(), 1, "{exclude}");
    assert!(!env.ws.join(".gitignore").exists(), "o .gitignore do usuário não é tocado");

    // A instância: id, modelo gravado, chefe e o terminal na worktree.
    assert!(core.project_instance_ids(&env.project).contains(&"claude@t1".to_owned()));
    let ident = Ident::parse("claude@t1").expect("id");
    assert_eq!(core.instance_model(&env.project, &ident).as_deref(), Some("opus"));
    assert_eq!(core.team(&env.project, "claude@t1").boss.as_deref(), Some("claude"));
    let spec_for = core.spawn_spec_for(&env.project, &ident).expect("spec");
    assert!(same_path(&spec_for.cwd, &t.worktree), "{}", spec_for.cwd.display());
    assert!(spec_for.args.windows(2).any(|w| w == ["--model", "opus"]), "{:?}", spec_for.args);
    assert_eq!(spec_for.env["AGENTDECK_AGENT"], "claude@t1");
    // Outras instâncias do projeto continuam na pasta do projeto.
    assert!(same_path(&core.spawn_spec_for(&env.project, &boss).expect("spec").cwd, &env.ws));

    // O brief foi para a aba pelo caminho das mensagens (request 0), com as regras da tarefa.
    let evs = events(&env);
    let sent = deliveries(&evs);
    assert_eq!(sent.len(), 1, "{sent:?}");
    let (to, prompt) = &sent[0];
    assert_eq!(to, "claude@t1");
    assert!(prompt.contains("Faça: Corrigir o login\n\nCom cuidado."), "{prompt}");
    assert!(prompt.contains(&t.worktree.display().to_string()) && prompt.contains(&t.branch), "{prompt}");
    assert!(prompt.contains("ask-agent --task done t1 --file") && prompt.contains("ask-agent --boss"), "{prompt}");
    assert!(!prompt.contains('\x1b'));
    assert!(evs.iter().any(|e| matches!(e, CoreEvent::Deliver(d) if d.create && d.from == "claude" && d.project == env.project)));

    // Iniciar de novo não vale; o segundo início não repete a exclusão.
    assert!(matches!(core.task_start(&env.project, "t1"), Err(TaskError::Conflict(_))));
    let t2 = core.task_create(&env.project, spec("Segunda")).expect("create");
    core.task_start(&env.project, &t2.id).expect("start 2");
    let exclude = std::fs::read_to_string(env.ws.join(".git").join("info").join("exclude")).expect("exclude");
    assert_eq!(exclude.lines().filter(|l| l.trim() == "/.agentdeck/").count(), 1, "{exclude}");
    assert_eq!(worktree_count(&env.ws), 3);
    core.shutdown();
}

#[test]
fn start_refuses_without_tabs_and_recovers_from_a_failure() {
    need_git!();
    // Sem as abas dos agentes (headless): recusa com a explicação e deixa o rascunho como está.
    let env = start_with(false, true);
    let t = env.core.task_create(&env.project, spec("Sem abas")).expect("create");
    match env.core.task_start(&env.project, &t.id) {
        Err(TaskError::Conflict(m)) => assert!(m.contains("abas dos agentes"), "{m}"),
        other => panic!("{other:?}"),
    }
    assert_eq!(env.core.task(&env.project, &t.id).map(|t| t.status), Some(TaskStatus::Draft));
    env.core.shutdown();

    // Falha ao iniciar: a base sumiu. A tarefa fica Failed com a mensagem; ao voltar a base, iniciar de novo funciona.
    let env = start();
    git(&env.ws, &["branch", "temporaria"]);
    let t = env.core.task_create(&env.project, TaskSpec { base: Some("temporaria".into()), ..spec("Vai falhar") }).expect("create");
    git(&env.ws, &["branch", "-q", "-D", "temporaria"]);
    match env.core.task_start(&env.project, &t.id) {
        Err(TaskError::Failed(m)) => assert!(m.contains("não existe mais"), "{m}"),
        other => panic!("{other:?}"),
    }
    let failed = env.core.task(&env.project, &t.id).expect("task");
    assert_eq!(failed.status, TaskStatus::Failed);
    assert!(failed.error.as_deref().is_some_and(|e| e.contains("temporaria")), "{failed:?}");
    assert!(failed.events.iter().any(|e| e.text.starts_with("erro:")));
    assert!(deliveries(&events(&env)).is_empty(), "nada foi entregue");
    git(&env.ws, &["branch", "temporaria", "main"]);
    let again = env.core.task_start(&env.project, &t.id).expect("nova tentativa");
    assert_eq!((again.status, again.error), (TaskStatus::Running, None));
    assert!(again.worktree.is_dir());
    env.core.shutdown();
}

#[test]
fn done_stores_the_report_and_the_diff_stat() {
    need_git!();
    let env = start();
    let core = &env.core;
    let t = core.task_create(&env.project, spec("Relatório")).expect("create");
    // Só uma tarefa em andamento recebe relatório.
    assert!(matches!(core.task_done(&env.project, &t.id, "x"), Err(TaskError::Conflict(_))));
    let t = core.task_start(&env.project, &t.id).expect("start");
    assert!(matches!(core.task_done(&env.project, &t.id, " \n "), Err(TaskError::Invalid(_))));
    assert!(matches!(core.task_done(&env.project, "t9", "x"), Err(TaskError::NotFound(_))));
    commit_file(&t.worktree, "novo.rs", "fn main() {}\n", "feat: novo");
    std::fs::write(t.worktree.join("sujo.txt"), "esqueci o commit").expect("w");
    let done = core.task_done(&env.project, &t.id, "Fiz o novo.rs.\nTestes: ok.").expect("done");
    assert_eq!(done.status, TaskStatus::Review);
    assert_eq!(done.report.as_deref(), Some("Fiz o novo.rs.\nTestes: ok."));
    let stat = done.diff_stat.expect("diff stat");
    assert!(stat.contains("novo.rs") && stat.contains("1 file changed"), "{stat}");
    assert!(done.finished_ms.is_some());
    assert!(done.events.iter().any(|e| e.text.contains("sem commit") && e.text.contains("sujo.txt")), "{:?}", done.events);
    assert!(done.events.iter().any(|e| e.text.contains("relatório recebido")));
    // Em revisão, entregar de novo não vale.
    assert!(matches!(core.task_done(&env.project, &t.id, "outra vez"), Err(TaskError::Conflict(_))));
    core.shutdown();
}

#[test]
fn task_diff_shows_only_what_the_branch_brought() {
    use agentdeck_core::git::DiffKind;
    need_git!();
    let env = start();
    let core = &env.core;
    let t = core.task_create(&env.project, spec("Diff")).expect("create");
    // Rascunho: a branch ainda não existe.
    assert!(matches!(core.task_diff(&env.project, &t.id), Err(TaskError::Conflict(_))));
    assert!(matches!(core.task_diff(&env.project, "t9"), Err(TaskError::NotFound(_))));
    let t = core.task_start(&env.project, &t.id).expect("start");
    commit_file(&t.worktree, "novo.rs", "fn main() {}\n", "feat: novo");
    // A base anda depois que a tarefa saiu: o diff `base...branch` não mostra o que a base ganhou.
    commit_file(&env.ws, "da_base.txt", "só na base\n", "chore: base");
    let diff = core.task_diff(&env.project, &t.id).expect("diff");
    assert!(!diff.truncated && !diff.binary);
    let added: Vec<&str> = diff.lines.iter().filter(|l| l.kind == DiffKind::Add).map(|l| l.text.as_str()).collect();
    assert_eq!(added, ["+fn main() {}"], "{:?}", diff.lines);
    let meta = diff.lines.iter().filter(|l| l.kind == DiffKind::Meta).map(|l| l.text.as_str()).collect::<Vec<_>>().join("\n");
    assert!(meta.contains("novo.rs") && !meta.contains("da_base.txt"), "{meta}");
    // Descartada: a branch some, o diff também.
    core.task_discard(&env.project, &t.id).expect("discard");
    assert!(matches!(core.task_diff(&env.project, &t.id), Err(TaskError::Conflict(_))));
    core.shutdown();
}

#[test]
fn return_sends_the_feedback_and_goes_back_to_running() {
    need_git!();
    let env = start();
    let core = &env.core;
    let t = core.task_create(&env.project, spec("Devolver")).expect("create");
    assert!(matches!(core.task_return(&env.project, &t.id, "x"), Err(TaskError::Conflict(_))));
    let t = core.task_start(&env.project, &t.id).expect("start");
    assert!(matches!(core.task_return(&env.project, &t.id, "x"), Err(TaskError::Conflict(_))), "em andamento não se devolve");
    commit_file(&t.worktree, "a.rs", "1\n", "feat: a");
    core.task_done(&env.project, &t.id, "Pronto.").expect("done");
    events(&env);

    assert!(matches!(core.task_return(&env.project, &t.id, "  "), Err(TaskError::Invalid(_))));
    let back = core.task_return(&env.project, &t.id, "Faltou o teste do caso vazio.").expect("return");
    assert_eq!((back.status, back.finished_ms), (TaskStatus::Running, None));
    assert_eq!(back.report.as_deref(), Some("Pronto."), "o relatório anterior fica até o próximo");
    let sent = deliveries(&events(&env));
    assert_eq!(sent.len(), 1, "{sent:?}");
    assert_eq!(sent[0].0, "claude@t1");
    assert!(sent[0].1.contains("Faltou o teste do caso vazio.") && sent[0].1.contains("ask-agent --task done t1 --file"), "{}", sent[0].1);
    // O ciclo continua: novo commit, novo relatório, e a aba (fechada à mão no meio) é registrada de novo.
    core.close_instance(&env.project, "claude@t1");
    commit_file(&t.worktree, "b.rs", "2\n", "fix: b");
    let again = core.task_done(&env.project, &t.id, "Agora sim.").expect("done 2");
    assert_eq!((again.status, again.report.as_deref()), (TaskStatus::Review, Some("Agora sim.")));
    assert!(again.diff_stat.as_deref().is_some_and(|s| s.contains("a.rs") && s.contains("b.rs")));
    core.task_return(&env.project, &t.id, "Mais um ajuste.").expect("return 2");
    assert!(core.project_instance_ids(&env.project).contains(&"claude@t1".to_owned()), "a instância volta ao core");
    core.shutdown();
}

#[test]
fn merge_makes_a_no_ff_commit_and_waits_for_validation() {
    need_git!();
    let env = start();
    let core = &env.core;
    let t = task_in_review(&env, "Mesclar isto", "feature.txt");
    events(&env);
    // O app reescreve o bloco do AGENTS.md no projeto: isso não impede a mesclagem.
    let agents_md = env.ws.join("AGENTS.md");
    let text = std::fs::read_to_string(&agents_md).expect("AGENTS.md");
    std::fs::write(&agents_md, format!("{text}\n<!-- reescrito pelo app -->\n")).expect("w");
    let head_before = git(&env.ws, &["rev-parse", "HEAD"]);

    let merged = core.task_merge(&env.project, &t.id).expect("merge");
    assert_eq!(merged.status, TaskStatus::Validating);
    assert!(merged.finished_ms.is_some() && merged.error.is_none());
    // Commit de mesclagem (dois pais) com a mensagem da tarefa, na base.
    assert_eq!(git(&env.ws, &["log", "-1", "--format=%s"]).trim(), "Merge tarefa t1: Mesclar isto");
    assert_eq!(git(&env.ws, &["log", "-1", "--format=%P"]).split_whitespace().count(), 2, "--no-ff");
    assert_eq!(git(&env.ws, &["symbolic-ref", "--short", "HEAD"]).trim(), "main");
    assert!(env.ws.join("feature.txt").is_file());
    assert_ne!(git(&env.ws, &["rev-parse", "HEAD"]), head_before);
    // Nada foi limpo: worktree, pasta, branch e instância seguem; a aba não foi fechada.
    assert!(t.worktree.is_dir());
    assert_eq!(worktree_count(&env.ws), 2);
    assert_eq!(branches(&env.ws), ["main", t.branch.as_str()]);
    assert!(!events(&env).iter().any(|e| matches!(e, CoreEvent::CloseInstance { .. })));
    assert!(core.project_instance_ids(&env.project).contains(&"claude@t1".to_owned()));
    assert_eq!(core.task_of_instance(&env.project, "claude@t1").map(|t| t.status), Some(TaskStatus::Validating));
    assert!(
        merged.events.iter().any(|e| e.text.contains("mesclada em main") && e.text.contains("aguardando validação")),
        "{:?}",
        merged.events
    );
    // A branch da tarefa foi alinhada com a base (fast-forward): o mesmo commit, e a worktree já tem o merge.
    assert_eq!(git(&t.worktree, &["rev-parse", "HEAD"]), git(&env.ws, &["rev-parse", "main"]));
    assert_eq!(git(&env.ws, &["rev-parse", &t.branch]), git(&env.ws, &["rev-parse", "main"]));
    // O terminal de uma aba reaberta continua na worktree.
    let ident = Ident::parse("claude@t1").expect("id");
    assert!(same_path(&core.spawn_spec_for(&env.project, &ident).expect("spec").cwd, &t.worktree));
    // A alteração do AGENTS.md seguiu intacta no diretório de trabalho.
    assert!(std::fs::read_to_string(&agents_md).expect("AGENTS.md").contains("reescrito pelo app"));
    // Mesclar de novo não vale; descartar vale (e deixa o merge na base).
    assert!(matches!(core.task_merge(&env.project, &t.id), Err(TaskError::Conflict(_))));
    core.shutdown();
}

#[test]
fn validate_cleans_up_and_completes() {
    need_git!();
    let env = start();
    let core = &env.core;
    let t = task_in_review(&env, "Validar isto", "feature.txt");
    // Antes de mesclar não se valida.
    match core.task_validate(&env.project, &t.id) {
        Err(TaskError::Conflict(m)) => assert!(m.contains("só se valida uma tarefa aguardando validação"), "{m}"),
        other => panic!("{other:?}"),
    }
    core.task_merge(&env.project, &t.id).expect("merge");
    events(&env);

    let done = core.task_validate(&env.project, &t.id).expect("validate");
    assert_eq!(done.status, TaskStatus::Merged);
    assert!(done.finished_ms.is_some() && done.error.is_none());
    assert!(done.events.iter().any(|e| e.text.contains("validada")), "{:?}", done.events);
    // Limpeza: worktree, pasta e branch sumiram; a aba foi fechada e a instância esquecida.
    assert!(!t.worktree.exists());
    assert_eq!(worktree_count(&env.ws), 1);
    assert_eq!(branches(&env.ws), ["main"]);
    assert!(events(&env).iter().any(|e| matches!(e, CoreEvent::CloseInstance { id, .. } if id == "claude@t1")));
    assert!(!core.project_instance_ids(&env.project).contains(&"claude@t1".to_owned()));
    assert!(env.ws.join("feature.txt").is_file(), "o merge continua na base");
    // Concluída é final: nada mais vale, e o terminal de uma nova aba com esse id volta à pasta do projeto.
    for r in [core.task_validate(&env.project, &t.id), core.task_merge(&env.project, &t.id), core.task_discard(&env.project, &t.id)] {
        assert!(matches!(r, Err(TaskError::Conflict(_))), "{r:?}");
    }
    assert!(matches!(core.task_reject(&env.project, &t.id, "x"), Err(TaskError::Conflict(_))));
    assert!(core.task_of_instance(&env.project, "claude@t1").is_none());
    let ident = Ident::parse("claude@t1").expect("id");
    assert!(same_path(&core.spawn_spec_for(&env.project, &ident).expect("spec").cwd, &env.ws));
    core.shutdown();
}

#[test]
fn validate_is_refused_while_there_is_unmerged_work() {
    need_git!();
    let env = start();
    let core = &env.core;
    let t = task_in_review(&env, "Trabalho solto", "feature.txt");
    core.task_merge(&env.project, &t.id).expect("merge");
    let refused = |expect: &str| match core.task_validate(&env.project, &t.id) {
        Err(TaskError::Conflict(m)) => assert!(m.contains(expect), "{m}"),
        other => panic!("esperava recusa com \"{expect}\", veio {other:?}"),
    };
    // O agente seguiu mexendo: arquivo sem commit...
    std::fs::write(t.worktree.join("solto.txt"), "x").expect("w");
    refused("sem commit (solto.txt)");
    // ...e commit que a base não tem.
    git(&t.worktree, &["add", "-A"]);
    git(&t.worktree, &["commit", "-q", "-m", "feat: a mais"]);
    refused("commits que ainda não foram mesclados");
    let now = core.task(&env.project, &t.id).expect("task");
    assert_eq!(now.status, TaskStatus::Validating, "{now:?}");
    assert!(now.worktree.is_dir() && branches(&env.ws).contains(&now.branch), "nada foi removido");
    assert!(now.events.iter().any(|e| e.text.contains("validação recusada")), "{:?}", now.events);
    // Reprovar entrega esse trabalho de volta ao ciclo normal.
    core.task_reject(&env.project, &t.id, "Entregue o que falta.").expect("reject");
    core.task_done(&env.project, &t.id, "Agora sim.").expect("done");
    core.task_merge(&env.project, &t.id).expect("merge 2");
    assert_eq!(core.task_validate(&env.project, &t.id).expect("validate").status, TaskStatus::Merged);
    assert!(env.ws.join("solto.txt").is_file());
    core.shutdown();
}

#[test]
fn reject_returns_the_task_to_the_same_agent_in_the_same_worktree() {
    need_git!();
    let env = start();
    let core = &env.core;
    let t = task_in_review(&env, "Reprovar isto", "feature.txt");
    // Fora de aguardando validação não se reprova.
    assert!(matches!(core.task_reject(&env.project, &t.id, "x"), Err(TaskError::Conflict(_))), "em revisão");
    core.task_merge(&env.project, &t.id).expect("merge");
    let base_head = git(&env.ws, &["rev-parse", "main"]);
    assert!(matches!(core.task_reject(&env.project, &t.id, " \n "), Err(TaskError::Invalid(_))), "sem motivo");
    assert_eq!(core.task(&env.project, &t.id).map(|t| t.status), Some(TaskStatus::Validating), "a recusa não muda nada");
    events(&env);

    let back = core.task_reject(&env.project, &t.id, "  Ao salvar, o app fecha.  ").expect("reject");
    assert_eq!((back.status, back.finished_ms), (TaskStatus::Running, None));
    assert_eq!(back.worktree, t.worktree);
    assert!(back.events.iter().any(|e| e.text.contains("reprovada")), "{:?}", back.events);
    // O motivo chega ao mesmo agente, dizendo que o trabalho já foi mesclado.
    let evs = events(&env);
    let sent = deliveries(&evs);
    assert_eq!(sent.len(), 1, "{sent:?}");
    let (to, prompt) = &sent[0];
    assert_eq!(to, "claude@t1");
    for needle in [
        "reprovada na validação",
        "já foi mesclado em main",
        "reprovou pelo seguinte motivo",
        "Ao salvar, o app fecha.",
        "mesma branch tarefa/t1-reprovar-isto",
        "ask-agent --task done t1 --file",
    ] {
        assert!(prompt.contains(needle), "{needle}: {prompt}");
    }
    assert!(!prompt.contains("alinhar"), "a branch estava alinhada: {prompt}");
    // A worktree segue a mesma, com o código da base (o merge incluso), sem tocar na base.
    assert!(t.worktree.join("feature.txt").is_file());
    assert_eq!(git(&env.ws, &["rev-parse", "main"]), base_head);
    // O ciclo segue: corrige, entrega, revisão, mesclar, aguardando validação, validar.
    commit_file(&t.worktree, "correcao.txt", "ok\n", "fix: o app não fecha");
    let again = core.task_done(&env.project, &t.id, "Corrigido.").expect("done");
    assert_eq!(again.status, TaskStatus::Review);
    assert!(again.diff_stat.as_deref().is_some_and(|s| s.contains("correcao.txt") && !s.contains("feature.txt")), "{:?}", again.diff_stat);
    assert_eq!(core.task_merge(&env.project, &t.id).expect("merge 2").status, TaskStatus::Validating);
    assert!(env.ws.join("correcao.txt").is_file());
    assert_eq!(core.task_validate(&env.project, &t.id).expect("validate").status, TaskStatus::Merged);
    assert!(!t.worktree.exists());
    core.shutdown();
}

#[test]
fn reject_reopens_the_tab_and_recreates_a_missing_worktree() {
    need_git!();
    let env = start();
    let core = &env.core;
    let t = task_in_review(&env, "Aba fechada", "feature.txt");
    core.task_merge(&env.project, &t.id).expect("merge");
    // O usuário fechou a aba (o core esqueceu a instância) e a pasta foi apagada à mão.
    core.close_instance(&env.project, &t.instance);
    std::fs::remove_dir_all(&t.worktree).expect("rm");
    events(&env);

    let back = core.task_reject(&env.project, &t.id, "Não funciona.").expect("reject");
    assert_eq!(back.status, TaskStatus::Running);
    // A pasta voltou, na branch da tarefa e com o código da base; a entrega pede a abertura da aba (`create`).
    assert!(t.worktree.is_dir() && t.worktree.join("feature.txt").is_file());
    assert_eq!(git(&t.worktree, &["symbolic-ref", "--short", "HEAD"]).trim(), t.branch);
    assert!(core.project_instance_ids(&env.project).contains(&"claude@t1".to_owned()), "a instância voltou ao core");
    let evs = events(&env);
    assert!(evs.iter().any(|e| matches!(e, CoreEvent::Deliver(d) if d.request == 0 && d.create && d.ident.id() == "claude@t1")), "{evs:?}");
    assert!(same_path(&core.spawn_spec_for(&env.project, &Ident::parse("claude@t1").expect("id")).expect("spec").cwd, &t.worktree));
    // Sem a pasta E sem a branch (apagadas por fora): recria a partir da base.
    commit_file(&t.worktree, "x.txt", "x\n", "feat: x");
    core.task_done(&env.project, &t.id, "De novo.").expect("done");
    core.task_merge(&env.project, &t.id).expect("merge 2");
    git(&env.ws, &["worktree", "remove", "--force", &t.worktree.to_string_lossy()]);
    git(&env.ws, &["branch", "-D", &t.branch]);
    core.task_reject(&env.project, &t.id, "Ainda não.").expect("reject 2");
    assert!(t.worktree.join("x.txt").is_file());
    assert_eq!(git(&env.ws, &["rev-parse", &t.branch]), git(&env.ws, &["rev-parse", "main"]));
    core.shutdown();
}

#[test]
fn reject_needs_the_agent_tabs() {
    need_git!();
    let env = start();
    let core = &env.core;
    let t = task_in_review(&env, "Sem abas", "feature.txt");
    core.task_merge(&env.project, &t.id).expect("merge");
    core.update_config(|c| c.auto_open_agent_tabs = false);
    match core.task_reject(&env.project, &t.id, "x") {
        Err(TaskError::Conflict(m)) => assert!(m.contains("Abrir abas dos agentes"), "{m}"),
        other => panic!("{other:?}"),
    }
    assert_eq!(core.task(&env.project, &t.id).map(|t| t.status), Some(TaskStatus::Validating));
    core.shutdown();
}

#[test]
fn a_task_awaiting_validation_can_be_discarded_and_the_merge_stays() {
    need_git!();
    let env = start();
    let core = &env.core;
    let t = task_in_review(&env, "Descartar depois", "feature.txt");
    core.task_merge(&env.project, &t.id).expect("merge");
    events(&env);
    let gone = core.task_discard(&env.project, &t.id).expect("discard");
    assert_eq!((gone.status, gone.worktree.exists()), (TaskStatus::Discarded, false));
    assert_eq!(branches(&env.ws), ["main"]);
    assert_eq!(worktree_count(&env.ws), 1);
    assert!(gone.events.iter().any(|e| e.text.contains("o que já foi mesclado continua em main")), "{:?}", gone.events);
    assert!(events(&env).iter().any(|e| matches!(e, CoreEvent::CloseInstance { id, .. } if id == "claude@t1")));
    assert!(env.ws.join("feature.txt").is_file(), "descartar não desfaz o merge");
    core.shutdown();
}

#[test]
fn a_task_awaiting_validation_survives_a_restart() {
    need_git!();
    let env = start();
    let t = task_in_review(&env, "Reinício", "feature.txt");
    env.core.task_merge(&env.project, &t.id).expect("merge");
    let text = std::fs::read_to_string(env.root.join("dados").join("tarefas").join(format!("{}.json", env.project))).expect("json");
    assert!(text.contains("\"status\": \"validating\""), "{text}");
    let (root, project, config) = (env.root.clone(), env.project.clone(), env.config.clone());
    let Env { _tmp, core: first, rx, .. } = env;
    first.shutdown();
    drop((first, rx));
    let (core, _rx) = start_core(&root, config, true);
    let (env_project, back) = (project.as_str(), core.task(&project, "t1").expect("t1"));
    assert_eq!(back.status, TaskStatus::Validating);
    // O agente continua sendo o da tarefa e o terminal abre na worktree.
    assert_eq!(core.task_of_instance(env_project, "claude@t1").map(|t| t.id), Some("t1".to_owned()));
    assert!(same_path(&core.spawn_spec_for(env_project, &Ident::parse("claude@t1").expect("id")).expect("spec").cwd, &t.worktree));
    assert_eq!(core.task_validate(env_project, "t1").expect("validate").status, TaskStatus::Merged);
    core.shutdown();
    drop(_tmp);
}

#[test]
fn a_failed_merge_keeps_the_agent_terminal_in_the_worktree_after_a_restart() {
    need_git!();
    let env = start();
    let t = task_in_review(&env, "Mesclagem falhou", "feature.txt");
    // Simula o erro de infraestrutura na mesclagem: o arquivo grava `Failed` com o relatório já entregue.
    let file = env.root.join("dados").join("tarefas").join(format!("{}.json", env.project));
    let (root, project, config) = (env.root.clone(), env.project.clone(), env.config.clone());
    let Env { _tmp, core: first, rx, .. } = env;
    first.shutdown();
    drop((first, rx));
    let text = std::fs::read_to_string(&file).expect("json");
    assert!(text.contains("\"status\": \"review\""), "{text}");
    std::fs::write(&file, text.replace("\"status\": \"review\"", "\"status\": \"failed\"")).expect("w");
    let (core, _rx) = start_core(&root, config, true);
    let failed = core.task(&project, "t1").expect("t1");
    assert_eq!((failed.status, failed.report.is_some()), (TaskStatus::Failed, true));
    let ident = Ident::parse("claude@t1").expect("id");
    assert!(same_path(&core.spawn_spec_for(&project, &ident).expect("spec").cwd, &t.worktree), "na worktree, não no repositório principal");
    core.shutdown();
    drop(_tmp);
}

#[test]
fn merge_is_refused_until_the_main_worktree_is_ready() {
    need_git!();
    let env = start();
    let core = &env.core;
    let t = task_in_review(&env, "Recusas", "feature.txt");
    let still_in_review = || {
        let now = core.task(&env.project, &t.id).expect("task");
        assert_eq!(now.status, TaskStatus::Review, "{now:?}");
        assert!(now.worktree.is_dir(), "nada foi removido");
    };
    let refused = |expect: &str| match core.task_merge(&env.project, &t.id) {
        Err(TaskError::Conflict(m)) => assert!(m.contains(expect), "{m}"),
        other => panic!("esperava recusa com \"{expect}\", veio {other:?}"),
    };

    // O repositório principal em outra branch.
    git(&env.ws, &["checkout", "-q", "-b", "outra"]);
    refused("está na branch \"outra\", não em \"main\"");
    still_in_review();
    git(&env.ws, &["checkout", "-q", "main"]);

    // Alteração sem commit em arquivo rastreado.
    std::fs::write(env.ws.join("a.txt"), "mudei\n").expect("w");
    refused("alterações sem commit (a.txt)");
    still_in_review();
    git(&env.ws, &["checkout", "-q", "--", "a.txt"]);

    // Trabalho sem commit na worktree da tarefa.
    std::fs::write(t.worktree.join("esqueci.txt"), "x").expect("w");
    refused("sem commit (esqueci.txt)");
    still_in_review();
    std::fs::remove_file(t.worktree.join("esqueci.txt")).expect("rm");

    // Tudo pronto: agora vai (a recusa não deixou rastro).
    assert!(t.events.len() < core.task(&env.project, &t.id).expect("task").events.len(), "as recusas ficam no histórico");
    assert_eq!(core.task_merge(&env.project, &t.id).expect("merge").status, TaskStatus::Validating);
    core.shutdown();
}

#[test]
fn merge_needs_new_commits() {
    need_git!();
    let env = start();
    let core = &env.core;
    let t = core.task_create(&env.project, spec("Sem nada")).expect("create");
    let t = core.task_start(&env.project, &t.id).expect("start");
    let done = core.task_done(&env.project, &t.id, "Não precisei mudar nada.").expect("done");
    assert!(done.diff_stat.as_deref().is_some_and(|s| s.contains("nenhuma alteração")), "{:?}", done.diff_stat);
    match core.task_merge(&env.project, &t.id) {
        Err(TaskError::Conflict(m)) => assert!(m.contains("não tem commits novos"), "{m}"),
        other => panic!("{other:?}"),
    }
    assert!(core.task_discard(&env.project, &t.id).is_ok());
    core.shutdown();
}

#[test]
fn a_conflicting_merge_is_aborted_and_the_task_stays_in_review() {
    need_git!();
    let env = start();
    let core = &env.core;
    let t = core.task_create(&env.project, spec("Conflito")).expect("create");
    let t = core.task_start(&env.project, &t.id).expect("start");
    commit_file(&t.worktree, "a.txt", "versão da tarefa\ndois\n", "feat: muda a.txt na tarefa");
    core.task_done(&env.project, &t.id, "Mudei a.txt.").expect("done");
    // Enquanto isso a base mudou a mesma linha.
    commit_file(&env.ws, "a.txt", "versão da base\ndois\n", "fix: muda a.txt na base");
    let head = git(&env.ws, &["rev-parse", "HEAD"]);

    match core.task_merge(&env.project, &t.id) {
        Err(TaskError::Conflict(m)) => assert!(m.contains("conflito") && m.contains("a.txt") && m.contains("desfeita"), "{m}"),
        other => panic!("{other:?}"),
    }
    // A mesclagem foi desfeita: sem MERGE_HEAD, sem alterações, mesmo HEAD, arquivo da base.
    assert!(!env.ws.join(".git").join("MERGE_HEAD").exists());
    assert_eq!(git(&env.ws, &["status", "--porcelain", "--untracked-files=no"]).trim(), "");
    assert_eq!(git(&env.ws, &["rev-parse", "HEAD"]), head);
    assert_eq!(std::fs::read_to_string(env.ws.join("a.txt")).expect("a.txt"), "versão da base\ndois\n");
    let now = core.task(&env.project, &t.id).expect("task");
    assert_eq!(now.status, TaskStatus::Review);
    assert!(now.worktree.is_dir() && branches(&env.ws).contains(&now.branch));
    assert!(now.events.iter().any(|e| e.text.contains("conflito na mesclagem")), "{:?}", now.events);

    // Devolvida, a tarefa traz a base para dentro da branch (resolve à mão aqui) e agora mescla.
    core.task_return(&env.project, &t.id, "Atualize com a main e resolva o conflito.").expect("return");
    let out = Command::new(git_exe().expect("git")).current_dir(&t.worktree).args(["merge", "main"]).output().expect("git");
    assert!(!out.status.success(), "o conflito acontece na branch da tarefa");
    std::fs::write(t.worktree.join("a.txt"), "resolvido\ndois\n").expect("w");
    git(&t.worktree, &["add", "-A"]);
    git(&t.worktree, &["commit", "-q", "-m", "merge: resolve"]);
    core.task_done(&env.project, &t.id, "Resolvido.").expect("done 2");
    assert_eq!(core.task_merge(&env.project, &t.id).expect("merge").status, TaskStatus::Validating);
    assert_eq!(std::fs::read_to_string(env.ws.join("a.txt")).expect("a.txt"), "resolvido\ndois\n");
    core.shutdown();
}

#[test]
fn discard_removes_everything_even_without_the_folder() {
    need_git!();
    let env = start();
    let core = &env.core;

    // Rascunho: só o registro.
    let draft = core.task_create(&env.project, spec("Rascunho")).expect("create");
    events(&env);
    assert_eq!(core.task_discard(&env.project, &draft.id).expect("discard").status, TaskStatus::Discarded);
    assert!(!events(&env).iter().any(|e| matches!(e, CoreEvent::CloseInstance { .. })), "rascunho não tem aba");

    // Em andamento, com trabalho sem commit: vai tudo embora.
    let t = core.task_create(&env.project, spec("Em andamento")).expect("create");
    let t = core.task_start(&env.project, &t.id).expect("start");
    std::fs::write(t.worktree.join("lixo.txt"), "x").expect("w");
    events(&env);
    let gone = core.task_discard(&env.project, &t.id).expect("discard");
    assert_eq!((gone.status, gone.worktree.exists()), (TaskStatus::Discarded, false));
    assert_eq!(branches(&env.ws), ["main"]);
    assert_eq!(worktree_count(&env.ws), 1);
    assert!(events(&env).iter().any(|e| matches!(e, CoreEvent::CloseInstance { id, .. } if id == "claude@t2")));
    assert!(!core.project_instance_ids(&env.project).contains(&"claude@t2".to_owned()));
    assert!(matches!(core.task_discard(&env.project, &t.id), Err(TaskError::Conflict(_))), "já descartada");

    // Em revisão, com a pasta apagada à mão: descarta do mesmo jeito e o git não fica com registro velho.
    let review = task_in_review(&env, "Pasta apagada", "x.txt");
    std::fs::remove_dir_all(&review.worktree).expect("rm");
    assert_eq!(worktree_count(&env.ws), 2, "o git ainda lembra da worktree");
    let gone = core.task_discard(&env.project, &review.id).expect("discard sem pasta");
    assert_eq!(gone.status, TaskStatus::Discarded);
    assert_eq!(branches(&env.ws), ["main"]);
    assert_eq!(worktree_count(&env.ws), 1);

    // Com a pasta E a branch já apagadas por fora.
    let t = core.task_create(&env.project, spec("Tudo por fora")).expect("create");
    let t = core.task_start(&env.project, &t.id).expect("start");
    git(&env.ws, &["worktree", "remove", "--force", &t.worktree.to_string_lossy()]);
    git(&env.ws, &["branch", "-D", &t.branch]);
    assert_eq!(core.task_discard(&env.project, &t.id).expect("discard").status, TaskStatus::Discarded);
    core.shutdown();
}

#[test]
fn the_terminal_of_a_missing_worktree_says_so() {
    need_git!();
    let env = start();
    let core = &env.core;
    let t = core.task_create(&env.project, spec("Pasta some")).expect("create");
    let t = core.task_start(&env.project, &t.id).expect("start");
    std::fs::remove_dir_all(&t.worktree).expect("rm");
    let ident = Ident::parse("claude@t1").expect("id");
    match core.spawn_spec_for(&env.project, &ident) {
        Err(m) => assert!(m.contains("A pasta da tarefa não existe mais"), "{m}"),
        Ok(s) => panic!("abriu em {}", s.cwd.display()),
    }
    core.shutdown();
}

#[test]
fn tasks_survive_a_restart_and_numbers_are_never_reused() {
    need_git!();
    let env = start();
    let core = env.core.clone();
    let t1 = core.task_create(&env.project, spec("Primeira")).expect("create");
    let t2 = core.task_create(&env.project, spec("Segunda")).expect("create");
    core.task_start(&env.project, &t1.id).expect("start");
    core.task_discard(&env.project, &t2.id).expect("discard");
    let before = core.tasks(&env.project);
    let (root, project, config, ws) = (env.root.clone(), env.project.clone(), env.config.clone(), env.ws.clone());

    // O arquivo fica na pasta de dados, não no repositório do usuário.
    let file = root.join("dados").join("tarefas").join(format!("{project}.json"));
    let text = std::fs::read_to_string(&file).expect("arquivo das tarefas");
    assert!(text.contains("\"id\": \"t1\"") && text.contains("\"status\": \"discarded\"") && text.contains("\"next\": 3"), "{text}");
    assert!(!ws.join("tarefas").exists());
    let names: Vec<String> = std::fs::read_dir(file.parent().expect("pasta"))
        .expect("dir")
        .flatten()
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .collect();
    assert!(
        names.iter().all(|n| !n.ends_with(".tmp")) && names.iter().any(|n| n == &format!("{project}.json")),
        "sem arquivos temporários sobrando: {names:?}"
    );

    // Reinicia o app (mesma pasta de dados e mesma configuração).
    let Env { _tmp, core: first, rx, .. } = env;
    first.shutdown();
    drop((first, core, rx));
    let (core2, _rx2) = start_core(&root, config, true);
    assert_eq!(core2.tasks(&project), before, "as tarefas voltam como estavam");
    let t3 = core2.task_create(&project, spec("Terceira")).expect("create");
    assert_eq!(t3.id, "t3", "o número da descartada não é reaproveitado");
    // E a tarefa em andamento continua com o terminal na worktree dela.
    let ident = Ident::parse("claude@t1").expect("id");
    assert!(same_path(&core2.spawn_spec_for(&project, &ident).expect("spec").cwd, &before[0].worktree));
    core2.shutdown();
    drop(_tmp);
}

#[test]
fn removing_a_project_keeps_its_tasks_file() {
    need_git!();
    let env = start();
    let t = env.core.task_create(&env.project, spec("Fica")).expect("create");
    let file = env.root.join("dados").join("tarefas").join(format!("{}.json", env.project));
    assert!(file.is_file());
    assert!(env.core.remove_project(&env.project));
    assert!(file.is_file(), "o arquivo das tarefas não some com o projeto");
    assert!(std::fs::read_to_string(&file).expect("ler").contains(&t.id));
    env.core.shutdown();
}

/// O `Deliver` de tarefa (request 0) mais recente: `(instância, prompt, boot)`.
fn last_task_delivery(evs: &[CoreEvent]) -> (String, String, Option<String>) {
    evs.iter()
        .rev()
        .find_map(|e| match e {
            CoreEvent::Deliver(d) if d.request == 0 => Some((d.ident.id(), d.prompt.clone(), d.boot.clone())),
            _ => None,
        })
        .expect("nenhuma entrega de tarefa")
}

#[test]
fn the_brief_goes_to_a_file_and_the_boot_line_is_short_ascii() {
    need_git!();
    let env = start();
    let core = &env.core;
    let t = core.task_create(&env.project, spec("Corrigir o botão de Ação")).expect("create");
    events(&env);
    let t = core.task_start(&env.project, &t.id).expect("start");
    let (to, prompt, boot) = last_task_delivery(&events(&env));
    assert_eq!(to, "claude@t1");
    // O texto completo (com acentos e quebras de linha) fica num arquivo da pasta de dados...
    let file = env.root.join("dados").join("tarefas").join(format!("{}-t1-brief.md", env.project));
    assert_eq!(std::fs::read_to_string(&file).expect("brief gravado"), prompt);
    assert!(prompt.contains("Corrigir o botão de Ação") && prompt.contains('\n'));
    // ...e a linha de comando leva só uma frase curta, em ASCII, de uma linha, que aponta para ele.
    let line = boot.expect("claude aceita prompt inicial");
    assert!(line.is_ascii() && !line.contains(['\n', '"']) && line.len() < 400, "{line}");
    assert!(line.starts_with("Tarefa t1: ") && line.contains("comece agora"), "{line}");
    assert!(line.contains(&file.to_string_lossy().into_owned()), "{line} / {}", file.display());
    // Devolver e reprovar também: o arquivo é o do feedback (sobrescrito a cada vez).
    core.task_done(&env.project, &t.id, "Feito.").expect("done");
    core.task_return(&env.project, &t.id, "Falta o teste de açaí.").expect("return");
    let (_, prompt, boot) = last_task_delivery(&events(&env));
    let feedback = env.root.join("dados").join("tarefas").join(format!("{}-t1-feedback.md", env.project));
    assert_eq!(std::fs::read_to_string(&feedback).expect("feedback gravado"), prompt);
    assert!(prompt.contains("Falta o teste de açaí."));
    let line = boot.expect("boot da devolução");
    assert!(line.is_ascii() && line.contains("devolvida") && line.contains("-t1-feedback.md"), "{line}");
    commit_file(&t.worktree, "x.txt", "x\n", "feat: x");
    core.task_done(&env.project, &t.id, "De novo.").expect("done 2");
    core.task_merge(&env.project, &t.id).expect("merge");
    core.task_reject(&env.project, &t.id, "Quebrou ao salvar.").expect("reject");
    let (_, prompt, boot) = last_task_delivery(&events(&env));
    assert_eq!(std::fs::read_to_string(&feedback).expect("feedback regravado"), prompt);
    assert!(prompt.contains("reprovada na validação") && prompt.contains("Quebrou ao salvar."));
    assert!(boot.is_some_and(|l| l.is_ascii() && l.contains("reprovada")));
    core.shutdown();
}

#[test]
fn a_cli_without_an_initial_prompt_keeps_the_typed_delivery() {
    need_git!();
    let env = start();
    let t = env.core.task_create(&env.project, TaskSpec { kind: "aider".into(), ..spec("Com aider") }).expect("create");
    events(&env);
    env.core.task_start(&env.project, &t.id).expect("start");
    let (to, prompt, boot) = last_task_delivery(&events(&env));
    assert_eq!(to, "aider@t1");
    assert_eq!(boot, None, "aider não tem prompt inicial: o brief é digitado");
    assert!(prompt.contains("Faça: Com aider"));
    assert!(!env.root.join("dados").join("tarefas").join(format!("{}-t1-brief.md", env.project)).exists());
    env.core.shutdown();
}

#[test]
fn the_spawn_spec_carries_the_boot_prompt_once_per_cli() {
    need_git!();
    let env = start();
    let core = &env.core;
    install_fake_agent(&env.root, "agy");
    install_fake_agent(&env.root, "opencode");
    install_fake_agent(&env.root, "codex");
    let line = "Tarefa t7: o pedido completo esta no arquivo C:/Users/Ana Bia/dados/tarefas/p1-t7-brief.md. Leia o arquivo e comece agora.";
    let spec_of = |id: &str, boot: bool| {
        let ident = Ident::parse(id).expect("id");
        core.register_instance(&env.project, id);
        if boot {
            core.set_boot_prompt(&env.project, id, line.to_owned());
        }
        core.spawn_spec_for(&env.project, &ident).expect("spec")
    };

    // Claude: posicional, ANTES das opções (`--add-dir` e `--allowedTools` aceitam vários valores e o engoliriam).
    let claude = spec_of("claude@t7", true);
    assert_eq!(claude.args[0], line, "{:?}", claude.args);
    assert_eq!(claude.args[1], "--append-system-prompt");
    assert!(claude.args.iter().filter(|a| a.as_str() == line).count() == 1);
    // Antigravity: `-i "<prompt>"` (--prompt-interactive), no fim.
    let agy = spec_of("antigravity@t7", true);
    assert_eq!(agy.args[agy.args.len() - 2..], ["-i".to_owned(), line.to_owned()], "{:?}", agy.args);
    // OpenCode (TUI): `--prompt "<prompt>"` no fim (com modelo, o `--standalone` vem antes).
    let opencode = spec_of("opencode@t7", true);
    assert_eq!(opencode.args[opencode.args.len() - 2..], ["--prompt".to_owned(), line.to_owned()], "{:?}", opencode.args);
    // Os demais ignoram: mesmo com um prompt guardado, o spec não muda e o prompt não vaza para outra abertura.
    let codex = spec_of("codex@t7", true);
    assert!(!codex.args.iter().any(|a| a.contains("Tarefa t7")), "{:?}", codex.args);
    let aider = spec_of("aider@t7", true);
    assert!(!aider.args.iter().any(|a| a.contains("Tarefa t7")), "{:?}", aider.args);

    // Uma vez só: a abertura seguinte (reiniciar a aba, reabrir o app) sobe sem o prompt.
    for id in ["claude@t7", "antigravity@t7", "opencode@t7"] {
        let again = spec_of(id, false);
        assert!(!again.args.iter().any(|a| a == line || a == "-i" || a == "--prompt"), "{id}: {:?}", again.args);
    }
    // Sem prompt guardado, o spec do Claude começa como sempre.
    assert_eq!(spec_of("claude@t8", false).args[0], "--append-system-prompt");
    assert_eq!(core.take_boot_prompt(&env.project, "claude@t7"), None);
    core.shutdown();
}

#[test]
fn a_boot_line_is_refused_when_the_path_is_not_plain_ascii() {
    use agentdeck_core::tasks::boot_line;
    let t = agentdeck_core::Task { id: "t3".into(), ..agentdeck_core::Task::default() };
    let ok = boot_line(&t, "o pedido esta no arquivo", std::path::Path::new("C:/dados/tarefas/p1-t3-brief.md")).expect("ascii");
    assert!(
        ok.starts_with("Tarefa t3: o pedido esta no arquivo. Leia o arquivo C:/dados") && ok.ends_with("sem esperar confirmacao."),
        "{ok}"
    );
    // Caminho com acento (ou aspas): a linha de comando de um shim `.cmd` não o carregaria inteiro; cai na entrega digitada.
    assert_eq!(boot_line(&t, "x", std::path::Path::new("C:/Usuários/Ana/brief.md")), None);
    assert_eq!(boot_line(&t, "x", std::path::Path::new("C:/a\"b/brief.md")), None);
}

// ---------- Depois da mesclagem: merge_commit, diff, done e validar sem becos sem saída ----------

fn diff_adds(diff: &agentdeck_core::git::GitDiff) -> Vec<String> {
    use agentdeck_core::git::DiffKind;
    diff.lines.iter().filter(|l| l.kind == DiffKind::Add).map(|l| l.text.clone()).collect()
}

#[test]
fn merge_stores_the_full_merge_commit_and_a_new_merge_overwrites_it() {
    need_git!();
    let env = start();
    let core = &env.core;
    let t = task_in_review(&env, "Guardar o commit", "feature.txt");
    assert_eq!(core.task(&env.project, &t.id).and_then(|t| t.merge_commit), None, "antes de mesclar não há commit");
    let merged = core.task_merge(&env.project, &t.id).expect("merge");
    let head = git(&env.ws, &["rev-parse", "HEAD"]).trim().to_owned();
    assert_eq!(head.len(), 40);
    assert_eq!(merged.merge_commit.as_deref(), Some(head.as_str()), "hash completo do commit de mesclagem");
    assert!(merged.events.iter().any(|e| e.text.contains(&format!("({})", &head[..7]))), "{:?}", merged.events);
    // Reprovada e entregue com um commit novo: a nova mesclagem sobrescreve.
    core.task_reject(&env.project, &t.id, "Falta um ajuste.").expect("reject");
    commit_file(&t.worktree, "ajuste.txt", "ok\n", "fix: ajuste");
    core.task_done(&env.project, &t.id, "Ajustado.").expect("done");
    let again = core.task_merge(&env.project, &t.id).expect("merge 2");
    let head2 = git(&env.ws, &["rev-parse", "HEAD"]).trim().to_owned();
    assert_ne!(head2, head);
    assert_eq!(again.merge_commit.as_deref(), Some(head2.as_str()));
    // Persiste: o que se lê de novo é o mesmo.
    assert_eq!(core.task(&env.project, &t.id).and_then(|t| t.merge_commit).as_deref(), Some(head2.as_str()));
    core.shutdown();
}

#[test]
fn a_task_saved_before_merge_commit_existed_still_loads() {
    let old = r#"{"id":"t4","title":"Antiga","status":"validating","base":"main","branch":"tarefa/t4-antiga"}"#;
    let t: agentdeck_core::Task = serde_json::from_str(old).expect("tarefa antiga");
    assert_eq!((t.id.as_str(), t.status, t.merge_commit.as_deref()), ("t4", TaskStatus::Validating, None));
    // E o campo vai e volta pelo JSON.
    let t = agentdeck_core::Task { merge_commit: Some("abc".into()), ..t };
    let back: agentdeck_core::Task = serde_json::from_str(&serde_json::to_string(&t).expect("json")).expect("volta");
    assert_eq!(back.merge_commit.as_deref(), Some("abc"));
}

#[test]
fn the_diff_of_a_task_awaiting_validation_is_what_the_merge_brought() {
    need_git!();
    let env = start();
    let core = &env.core;
    let t = task_in_review(&env, "Ver o mesclado", "feature.txt");
    let before = core.task_diff(&env.project, &t.id).expect("diff em revisão");
    assert_eq!(diff_adds(&before), ["+conteúdo"]);
    // A base anda antes da mesclagem: o commit de mesclagem não traz isso, então o diff também não.
    commit_file(&env.ws, "da_base.txt", "só na base\n", "chore: base");
    core.task_merge(&env.project, &t.id).expect("merge");
    // Depois do alinhamento, `base...branch` é vazio; o diff vem do commit de mesclagem.
    let diff = core.task_diff(&env.project, &t.id).expect("diff em validação");
    assert_eq!(diff_adds(&diff), ["+conteúdo"], "{:?}", diff.lines);
    assert!(!diff.truncated && !diff.binary);
    let text: String = diff.lines.iter().map(|l| l.text.as_str()).collect::<Vec<_>>().join("\n");
    assert!(text.contains("feature.txt") && !text.contains("da_base.txt"), "{text}");
    // Concluída: branch e worktree somem, o diff do que foi mesclado continua.
    core.task_validate(&env.project, &t.id).expect("validate");
    let done = core.task_diff(&env.project, &t.id).expect("diff concluída");
    assert_eq!(diff_adds(&done), ["+conteúdo"]);
    // Descartada continua sem diff (a branch some e não há commit de mesclagem).
    let other = core.task_create(&env.project, spec("Outra")).expect("create");
    core.task_discard(&env.project, &other.id).expect("discard");
    assert!(matches!(core.task_diff(&env.project, &other.id), Err(TaskError::Conflict(_))));
    core.shutdown();
}

#[test]
fn reject_then_done_without_commits_goes_back_to_validating_without_a_new_merge() {
    need_git!();
    let env = start();
    let core = &env.core;
    let t = task_in_review(&env, "Nada a mudar", "feature.txt");
    let merged = core.task_merge(&env.project, &t.id).expect("merge");
    let commit = merged.merge_commit.clone().expect("commit");
    core.task_reject(&env.project, &t.id, "Achei que quebrava, mas talvez não.").expect("reject");
    let done = core.task_done(&env.project, &t.id, "Conferi: não há o que mudar.").expect("done");
    assert_eq!(done.status, TaskStatus::Review);
    let head = git(&env.ws, &["rev-parse", "HEAD"]);
    // Sem commits novos, mas já mesclada antes: não recusa, não mescla de novo.
    let again = core.task_merge(&env.project, &t.id).expect("merge sem commits novos");
    assert_eq!(again.status, TaskStatus::Validating);
    assert_eq!(again.merge_commit.as_deref(), Some(commit.as_str()), "o commit da mesclagem não muda");
    assert_eq!(git(&env.ws, &["rev-parse", "HEAD"]), head, "nenhum commit de mesclagem novo");
    assert!(
        again.events.iter().any(|e| e.text.contains("sem commits novos desde a última mesclagem")
            && e.text.contains(&commit[..7])
            && e.text.contains("aguardando validação de novo")),
        "{:?}",
        again.events
    );
    assert!(again.finished_ms.is_some() && again.error.is_none());
    // A worktree segue; o diff mostra o que a mesclagem trouxe; validar conclui.
    assert!(t.worktree.is_dir());
    assert_eq!(diff_adds(&core.task_diff(&env.project, &t.id).expect("diff")), ["+conteúdo"]);
    assert_eq!(core.task_validate(&env.project, &t.id).expect("validate").status, TaskStatus::Merged);
    core.shutdown();
}

#[test]
fn done_while_awaiting_validation_needs_new_commits() {
    need_git!();
    let env = start();
    let core = &env.core;
    let t = task_in_review(&env, "Entregar depois", "feature.txt");
    core.task_merge(&env.project, &t.id).expect("merge");
    // Sem commits novos: 409 com a explicação, e nada muda.
    match core.task_done(&env.project, &t.id, "Mais uma coisa.") {
        Err(TaskError::Conflict(m)) => assert_eq!(
            m,
            format!(
                "a tarefa {} já foi mesclada e espera a validação do usuário; não há commits novos para entregar. Se algo ainda falta, quem decide é o usuário (Reprovar)",
                t.id
            )
        ),
        other => panic!("{other:?}"),
    }
    let still = core.task(&env.project, &t.id).expect("task");
    assert_eq!(still.status, TaskStatus::Validating);
    assert!(still.report.as_deref().is_some_and(|r| r.starts_with("Feito.")), "o relatório antigo ficou");
    // O usuário corrigiu algo na aba da tarefa: com commit novo o agente consegue entregar.
    commit_file(&t.worktree, "correcao.txt", "ok\n", "fix: corrigido na aba");
    let done = core.task_done(&env.project, &t.id, "Entreguei a correção feita na aba.").expect("done");
    assert_eq!(done.status, TaskStatus::Review);
    assert_eq!(done.report.as_deref(), Some("Entreguei a correção feita na aba."));
    assert!(done.diff_stat.as_deref().is_some_and(|s| s.contains("correcao.txt") && !s.contains("feature.txt")), "{:?}", done.diff_stat);
    assert!(
        done.events.iter().any(|e| e.text.contains("o agente entregou mais trabalho depois da mesclagem; em revisão")),
        "{:?}",
        done.events
    );
    // Daí o ciclo normal: mesclar → aguardando validação → validar.
    assert_eq!(core.task_merge(&env.project, &t.id).expect("merge 2").status, TaskStatus::Validating);
    assert_eq!(core.task_validate(&env.project, &t.id).expect("validate").status, TaskStatus::Merged);
    assert!(env.ws.join("correcao.txt").is_file());
    core.shutdown();
}

#[test]
fn validate_completes_with_a_warning_when_the_base_branch_is_gone() {
    need_git!();
    let env = start();
    let core = &env.core;
    git(&env.ws, &["branch", "estavel"]);
    git(&env.ws, &["checkout", "-q", "estavel"]);
    let t = core.task_create(&env.project, TaskSpec { base: Some("estavel".into()), ..spec("Base some") }).expect("create");
    let t = core.task_start(&env.project, &t.id).expect("start");
    commit_file(&t.worktree, "feature.txt", "conteúdo\n", "feat: base some");
    core.task_done(&env.project, &t.id, "Feito.").expect("done");
    core.task_merge(&env.project, &t.id).expect("merge");
    // A base é apagada à mão (o repositório volta para a main) enquanto a tarefa espera validação.
    git(&env.ws, &["checkout", "-q", "main"]);
    git(&env.ws, &["branch", "-D", "estavel"]);
    let done = core.task_validate(&env.project, &t.id).expect("validate sem a base");
    assert_eq!(done.status, TaskStatus::Merged);
    assert!(
        done.events
            .iter()
            .any(|e| e.text.contains("aviso: a base estavel não existe mais; não conferi se a branch tinha commits fora dela")),
        "{:?}",
        done.events
    );
    assert!(!t.worktree.exists());
    assert!(!branches(&env.ws).contains(&t.branch), "a branch da tarefa foi removida");
    core.shutdown();
}
