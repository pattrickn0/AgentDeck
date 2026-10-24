//! `ask-agent --learn` traz a situação atual do projeto (sem arquivos) e o aviso de choque com outra tarefa
//! (com arquivos), para o agente de qualquer aba, com o git real.

use std::path::Path;
use std::time::Instant;

use serde_json::json;

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

/// O texto de `POST /learn` para o agente do token, com os `paths` dados.
fn learn(env: &Env, token: &str, paths: &[&str]) -> String {
    let (st, b) = http(&env.core, "POST", "/learn", token, &json!({ "paths": paths }).to_string());
    assert_eq!(st, 200, "{b}");
    b["text"].as_str().expect("texto").to_owned()
}

/// Cria e inicia uma tarefa pelo orquestrador; devolve a pasta da worktree.
fn start_task(env: &Env, boss: &str, title: &str) -> String {
    start_task_with(env, boss, title, "Faça.")
}

fn start_task_with(env: &Env, boss: &str, title: &str, brief: &str) -> String {
    let (st, b) = http(&env.core, "POST", "/tasks", boss, &json!({ "title": title, "brief": brief, "start": true }).to_string());
    assert_eq!(st, 200, "{b}");
    b["task"]["worktree"].as_str().expect("worktree").to_owned()
}

const HISTORY: &str = "memory/historico.md";

#[test]
fn learn_brings_the_situation_to_any_tab_and_warns_about_clashes() {
    need_git!();
    let env = start();
    let core = &env.core;
    core.register_instance(&env.project, "claude");
    let boss = tab_token(&env, "claude");
    let (w1, w2) = (start_task(&env, &boss, "Primeira"), start_task(&env, &boss, "Segunda"));
    // t1 tem `app.rs` e o histórico na branch; t2 só tem `wip.rs`, sem commit.
    commit_file(Path::new(&w1), "app.rs", "um\n", "feat: app");
    std::fs::create_dir_all(Path::new(&w1).join("memory")).expect("pasta");
    commit_file(Path::new(&w1), HISTORY, "- t1\n", "docs: historico");
    std::fs::write(Path::new(&w2).join("wip.rs"), "rascunho\n").expect("wip");
    let (a1, a2) = (tab_token(&env, "claude@t1"), tab_token(&env, "claude@t2"));

    // Aba normal, sem arquivos: o que já trazia e a situação, com as duas tarefas e os commits da base.
    let started = Instant::now();
    let text = learn(&env, &boss, &[]);
    eprintln!("learn sem arquivos com 2 tarefas abertas: {:?}", started.elapsed());
    assert!(text.starts_with("O projeto ainda não tem aprendizados"), "{text}");
    let (_, situation) = text.split_once("Situação atual:\n").unwrap_or_else(|| panic!("sem situação: {text}"));
    assert!(situation.contains("- t1 \"Primeira\" (claude@t1, em andamento): mexe em app.rs, memory/historico.md"), "{situation}");
    assert!(situation.contains("- t2 \"Segunda\" (claude@t2, em andamento): mexe em wip.rs"), "{situation}");
    assert!(situation.contains("Últimos commits de main:"), "{situation}");

    // O agente de uma tarefa não se vê na própria situação.
    let own = learn(&env, &a1, &[]);
    assert!(own.contains("Situação atual:") && own.contains("- t2 \"Segunda\"") && !own.contains("- t1 "), "{own}");
    assert!(own.contains("Últimos commits de main:"), "{own}");

    // Com arquivos: só o aviso de choque, no topo; o próprio arquivo e o histórico não contam.
    assert!(!learn(&env, &a1, &["app.rs"]).contains("ATENÇÃO"), "o próprio arquivo");
    assert!(!learn(&env, &a2, &[HISTORY]).contains("ATENÇÃO"), "o histórico mescla por união");
    assert!(!learn(&env, &a2, &["outro.rs"]).contains("ATENÇÃO"), "sem choque, nada a mais");
    assert!(!learn(&env, &a2, &["app.rs"]).contains("Situação atual:"), "com arquivos a situação não vem inteira");
    // Arquivo só na branch de t1.
    let on_branch = learn(&env, &a2, &["app.rs"]);
    assert!(
        on_branch.starts_with(
            "ATENÇÃO:\napp.rs: a t1 (claude@t1, em andamento; já mexeu) também mexe nele; mudanças nas mesmas linhas vão conflitar na mesclagem\n\n"
        ),
        "{on_branch}"
    );
    // Arquivo só sem commit na worktree de t2.
    let dirty = learn(&env, &a1, &["wip.rs"]);
    assert!(dirty.contains("wip.rs: a t2 (claude@t2, em andamento; já mexeu) também mexe nele"), "{dirty}");
    // O caminho absoluto dentro da própria worktree vale como o relativo (e continua sem avisar do próprio arquivo).
    let own_abs = Path::new(&w1).join("app.rs").to_string_lossy().into_owned();
    assert!(!learn(&env, &a1, &[own_abs.as_str()]).contains("ATENÇÃO"));
    // A aba normal também é avisada.
    assert!(learn(&env, &boss, &["app.rs"]).contains("app.rs: a t1 (claude@t1, em andamento; já mexeu) também mexe nele"));
    core.shutdown();
}

#[test]
fn learn_without_other_tasks_says_so_and_sends_nothing_extra_for_files() {
    need_git!();
    let env = start();
    env.core.register_instance(&env.project, "claude");
    let boss = tab_token(&env, "claude");
    let w1 = start_task(&env, &boss, "Sozinha");
    commit_file(Path::new(&w1), "app.rs", "um\n", "feat: app");
    let agent = tab_token(&env, "claude@t1");
    let alone = learn(&env, &agent, &[]);
    assert!(alone.contains("Situação atual:\nOutras tarefas em andamento: nenhuma."), "{alone}");
    assert!(!learn(&env, &agent, &["app.rs"]).contains("ATENÇÃO"));
    // O orquestrador (aba normal) é avisado da tarefa que já mexeu.
    assert!(learn(&env, &boss, &["app.rs"]).contains("a t1 (claude@t1, em andamento; já mexeu)"));
    env.core.shutdown();
}

#[test]
fn learn_stays_quick_with_several_tasks_open() {
    need_git!();
    let env = start();
    env.core.register_instance(&env.project, "claude");
    let boss = tab_token(&env, "claude");
    for n in 0..3 {
        let wt = start_task(&env, &boss, &format!("Tarefa {n}"));
        commit_file(Path::new(&wt), &format!("f{n}.rs"), "x\n", "feat: f");
        std::fs::write(Path::new(&wt).join(format!("sujo{n}.rs")), "y\n").expect("sujo");
    }
    let started = Instant::now();
    let text = learn(&env, &boss, &[]);
    let elapsed = started.elapsed();
    eprintln!("learn sem arquivos com 3 tarefas abertas: {elapsed:?}");
    assert!(text.contains("- t3 \"Tarefa 2\" (claude@t3, em andamento): mexe em f2.rs, sujo2.rs"), "{text}");
    assert!(elapsed.as_secs() < 5, "lento: {elapsed:?}");
    env.core.shutdown();
}

#[test]
fn a_fresh_task_clashes_through_its_request_but_a_merged_one_does_not() {
    need_git!();
    let env = start();
    env.core.register_instance(&env.project, "claude");
    let boss = tab_token(&env, "claude");
    commit_file(
        &env.ws,
        "alvo.rs",
        "base
",
        "feat: alvo",
    );
    commit_file(
        &env.ws,
        "pronto.rs",
        "base
",
        "feat: pronto",
    );
    // t1 acabou de nascer: worktree limpa, nenhum commit; só o pedido cita `alvo.rs`.
    start_task_with(&env, &boss, "Recém-criada", "Mude o alvo.rs e mais nada.");
    // t2 também mexe em `pronto.rs`, entrega e é mesclada: fica aguardando validação, já na base.
    let w2 = start_task_with(&env, &boss, "Já mesclada", "Mude o pronto.rs.");
    commit_file(
        Path::new(&w2),
        "pronto.rs",
        "novo
",
        "feat: pronto",
    );
    let (st, b) = http(&env.core, "POST", "/tasks/t2/done", &tab_token(&env, "claude@t2"), &json!({ "report": "Pronto." }).to_string());
    assert_eq!(st, 200, "{b}");
    let (st, b) = http(&env.core, "POST", "/tasks/t2/merge", &boss, "{}");
    assert_eq!((st, b["task"]["status"].as_str()), (200, Some("validating")), "{b}");
    let w3 = start_task(&env, &boss, "Terceira");
    assert!(Path::new(&w3).is_dir());
    let agent = tab_token(&env, "claude@t3");

    let cited = learn(&env, &agent, &["alvo.rs"]);
    assert!(
        cited.starts_with(
            "ATENÇÃO:
alvo.rs: a t1 (claude@t1, em andamento; o pedido cita) também mexe nele;"
        ),
        "{cited}"
    );
    // Aguardando validação já está na base: sem aviso, mas segue na situação.
    assert!(!learn(&env, &agent, &["pronto.rs"]).contains("ATENÇÃO"));
    let situation = learn(&env, &agent, &[]);
    assert!(
        situation.contains("- t1 \"Recém-criada\" (claude@t1, em andamento): ainda sem arquivos na branch; o pedido cita alvo.rs"),
        "{situation}"
    );
    assert!(situation.contains("- t2 \"Já mesclada\" (claude@t2, aguardando validação)"), "{situation}");
    env.core.shutdown();
}
