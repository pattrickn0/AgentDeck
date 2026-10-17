//! Mesa de tarefas com o git real (pula com aviso se o `git` não estiver instalado): criar, iniciar,
//! relatório, devolver, mesclar, descartar e persistência. A ponte fica em `tasks_bridge.rs`.
use std::process::Command;

use commanddeck_core::util::same_path;
use commanddeck_core::{CoreEvent, Ident, TaskError, TaskSpec, TaskStatus};

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
    assert_eq!(spec_for.env["COMMANDDECK_AGENT"], "claude@t1");
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
    assert!(prompt.contains("ask-agent --task done t1 -") && prompt.contains("ask-agent --boss"), "{prompt}");
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
    assert!(sent[0].1.contains("Faltou o teste do caso vazio.") && sent[0].1.contains("ask-agent --task done t1 -"), "{}", sent[0].1);
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
fn merge_makes_a_no_ff_commit_and_cleans_up() {
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
    assert_eq!(merged.status, TaskStatus::Merged);
    assert!(merged.finished_ms.is_some() && merged.error.is_none());
    // Commit de mesclagem (dois pais) com a mensagem da tarefa, na base.
    assert_eq!(git(&env.ws, &["log", "-1", "--format=%s"]).trim(), "Merge tarefa t1: Mesclar isto");
    assert_eq!(git(&env.ws, &["log", "-1", "--format=%P"]).split_whitespace().count(), 2, "--no-ff");
    assert_eq!(git(&env.ws, &["symbolic-ref", "--short", "HEAD"]).trim(), "main");
    assert!(env.ws.join("feature.txt").is_file());
    assert_ne!(git(&env.ws, &["rev-parse", "HEAD"]), head_before);
    // Limpeza: worktree, pasta e branch sumiram; a aba foi fechada e a instância esquecida.
    assert!(!t.worktree.exists());
    assert_eq!(worktree_count(&env.ws), 1);
    assert_eq!(branches(&env.ws), ["main"]);
    assert!(events(&env).iter().any(|e| matches!(e, CoreEvent::CloseInstance { id, .. } if id == "claude@t1")));
    assert!(!core.project_instance_ids(&env.project).contains(&"claude@t1".to_owned()));
    assert!(merged.events.iter().any(|e| e.text.contains("mesclada em main")), "{:?}", merged.events);
    // A alteração do AGENTS.md seguiu intacta no diretório de trabalho.
    assert!(std::fs::read_to_string(&agents_md).expect("AGENTS.md").contains("reescrito pelo app"));
    // Depois de mesclada, nada mais vale; o terminal de uma nova aba com esse id volta à pasta do projeto.
    assert!(matches!(core.task_merge(&env.project, &t.id), Err(TaskError::Conflict(_))));
    assert!(matches!(core.task_discard(&env.project, &t.id), Err(TaskError::Conflict(_))));
    let ident = Ident::parse("claude@t1").expect("id");
    assert!(same_path(&core.spawn_spec_for(&env.project, &ident).expect("spec").cwd, &env.ws));
    core.shutdown();
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
    assert_eq!(core.task_merge(&env.project, &t.id).expect("merge").status, TaskStatus::Merged);
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
    assert_eq!(core.task_merge(&env.project, &t.id).expect("merge").status, TaskStatus::Merged);
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
    assert_eq!(std::fs::read_dir(file.parent().expect("pasta")).expect("dir").count(), 1, "sem arquivos temporários sobrando");

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
