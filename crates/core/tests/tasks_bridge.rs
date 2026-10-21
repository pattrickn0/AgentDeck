//! A ponte HTTP das tarefas (`/tasks`) com o git real: o ciclo de vida inteiro pelo token mestre e as
//! permissões do agente de uma tarefa (consulta e entrega o próprio relatório; não mescla, não descarta, não cria).

use std::path::Path;

use commanddeck_core::TaskStatus;
use serde_json::{json, Value};

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
fn the_bridge_serves_the_whole_lifecycle_with_the_master_token() {
    need_git!();
    let env = start();
    let master = env.core.bridge_token().to_owned();
    let call = |method: &str, path: &str, body: Value| http(&env.core, method, path, &master, &body.to_string());

    let (st, b) = call("POST", "/tasks", json!({ "title": "Pela ponte", "brief": "Faça pela ponte.", "model": "sonnet" }));
    assert_eq!(st, 200, "{b}");
    assert_eq!((b["task"]["id"].as_str(), b["task"]["status"].as_str()), (Some("t1"), Some("draft")));
    assert_eq!(
        (b["task"]["kind"].as_str(), b["task"]["model"].as_str(), b["task"]["creator"].as_str()),
        (Some("claude"), Some("sonnet"), Some("usuário"))
    );

    // Erros de validação e rotas.
    assert_eq!(call("POST", "/tasks", json!({ "title": " " })).0, 400);
    assert_eq!(call("POST", "/tasks", json!({ "title": "x", "kind": "nao-existe" })).0, 400);
    assert_eq!(call("POST", "/tasks", json!({ "title": 5 })).0, 400);
    assert_eq!(call("GET", "/tasks/t9", json!(null)).0, 404);
    assert_eq!(call("GET", "/tasks/xyz", json!(null)).0, 400);
    assert_eq!(call("POST", "/tasks/t1/explode", json!({})).0, 404);
    assert_eq!(call("POST", "/tasks/t1/merge", json!({})).0, 409, "rascunho não se mescla");
    assert_eq!(call("POST", "/tasks/t9/start", json!({})).0, 404);

    // Criar já iniciando.
    let (st, b) = call("POST", "/tasks", json!({ "title": "Já começa", "brief": "Vai.", "start": true }));
    assert_eq!(st, 200, "{b}");
    assert_eq!(b["task"]["status"], "running");
    assert!(Path::new(b["task"]["worktree"].as_str().expect("worktree")).is_dir());
    // Iniciar o rascunho t1 pelo caminho próprio.
    let (st, b) = call("POST", "/tasks/t1/start", json!({}));
    assert_eq!((st, b["task"]["status"].as_str()), (200, Some("running")));

    // Listas.
    let (st, b) = call("GET", "/tasks", json!(null));
    assert_eq!(st, 200);
    let list = b["tasks"].as_array().expect("lista");
    assert_eq!(list.iter().map(|t| t["id"].as_str().unwrap_or("")).collect::<Vec<_>>(), ["t1", "t2"]);
    assert!(list[0].get("brief").is_none() && list[0].get("report").is_none(), "a lista é enxuta");
    let (st, b) = call("GET", "/tasks/t1", json!(null));
    assert_eq!(st, 200);
    assert_eq!(b["task"]["brief"], "Faça pela ponte.");
    assert!(b["task"]["events"].as_array().is_some_and(|e| e.len() >= 2));
    let (st, b) = http(&env.core, "GET", &format!("/tasks?project={}", env.project), &master, "");
    assert_eq!((st, b["tasks"].as_array().map(Vec::len)), (200, Some(2)));
    assert_eq!(http(&env.core, "GET", "/tasks?project=pnenhum", &master, "").0, 400);

    // Relatório, devolução, mesclagem e descarte.
    let t1 = env.core.task(&env.project, "t1").expect("t1");
    commit_file(&t1.worktree, "ponte.txt", "ok\n", "feat: ponte");
    let (st, b) = call("POST", "/tasks/t1/done", json!({ "report": "Pela ponte, pronto." }));
    assert_eq!((st, b["task"]["status"].as_str(), b["task"]["report"].as_str()), (200, Some("review"), Some("Pela ponte, pronto.")));
    assert!(b["task"]["diff_stat"].as_str().is_some_and(|s| s.contains("ponte.txt")));
    assert_eq!(call("POST", "/tasks/t1/done", json!({ "report": "de novo" })).0, 409);
    let (st, b) = call("POST", "/tasks/t1/return", json!({ "feedback": "Falta um teste." }));
    assert_eq!((st, b["task"]["status"].as_str()), (200, Some("running")));
    assert_eq!(call("POST", "/tasks/t1/done", json!({ "report": "Com teste." })).0, 200);
    let (st, b) = call("POST", "/tasks/t1/merge", json!({}));
    assert_eq!((st, b["task"]["status"].as_str()), (200, Some("validating")), "{b}");
    assert!(env.ws.join("ponte.txt").is_file() && t1.worktree.is_dir(), "mesclar não limpa");
    // Reprovar: sem motivo é erro de uso; com motivo volta a andamento. Depois, entrega, mescla e valida.
    assert_eq!(call("POST", "/tasks/t1/reject", json!({})).0, 400);
    let (st, b) = call("POST", "/tasks/t1/reject", json!({ "feedback": "Não abre no app." }));
    assert_eq!((st, b["task"]["status"].as_str()), (200, Some("running")), "{b}");
    assert_eq!(call("POST", "/tasks/t1/validate", json!({})).0, 409, "validar só a partir de aguardando validação");
    assert_eq!(call("POST", "/tasks/t1/reject", json!({ "feedback": "x" })).0, 409);
    commit_file(&t1.worktree, "ponte2.txt", "ok\n", "fix: abre no app");
    assert_eq!(call("POST", "/tasks/t1/done", json!({ "report": "Corrigido." })).0, 200);
    assert_eq!(call("POST", "/tasks/t1/merge", json!({})).0, 200);
    let (st, b) = call("POST", "/tasks/t1/validate", json!({}));
    assert_eq!((st, b["task"]["status"].as_str()), (200, Some("merged")), "{b}");
    assert!(!t1.worktree.exists());
    let (st, b) = call("POST", "/tasks/t2/discard", json!({}));
    assert_eq!((st, b["task"]["status"].as_str()), (200, Some("discarded")));
    assert_eq!(call("POST", "/tasks/t2/discard", json!({})).0, 409);
    env.core.shutdown();
}

#[test]
fn a_task_agent_cannot_merge_discard_or_create() {
    need_git!();
    let env = start();
    let core = &env.core;
    // O orquestrador (claude) cria duas tarefas pela ponte, com o token da aba dele.
    core.register_instance(&env.project, "claude");
    let boss = tab_token(&env, "claude");
    let created = |title: &str| {
        let (st, b) = http(core, "POST", "/tasks", &boss, &json!({ "title": title, "brief": "Faça.", "start": true }).to_string());
        assert_eq!(st, 200, "{b}");
        b["task"].clone()
    };
    let (t1, t2) = (created("Primeira"), created("Segunda"));
    assert_eq!((t1["creator"].as_str(), t1["instance"].as_str()), (Some("claude"), Some("claude@t1")));
    assert_eq!(core.team(&env.project, "claude@t1").boss.as_deref(), Some("claude"), "quem criou é o chefe");

    // O agente da tarefa t1 consulta e entrega o relatório da própria tarefa, só.
    let agent = tab_token(&env, "claude@t1");
    let call = |token: &str, method: &str, path: &str, body: Value| http(core, method, path, token, &body.to_string());
    assert_eq!(call(&agent, "GET", "/tasks", json!(null)).0, 200);
    assert_eq!(call(&agent, "GET", "/tasks/t2", json!(null)).0, 200);
    for (path, body) in [
        ("/tasks", json!({ "title": "Subtarefa", "brief": "x" })),
        ("/tasks/t1/start", json!({})),
        ("/tasks/t1/return", json!({ "feedback": "x" })),
        ("/tasks/t1/merge", json!({})),
        ("/tasks/t1/validate", json!({})),
        ("/tasks/t1/reject", json!({ "feedback": "x" })),
        ("/tasks/t1/discard", json!({})),
        ("/tasks/t2/discard", json!({})),
        ("/tasks/t2/merge", json!({})),
    ] {
        let (st, b) = call(&agent, "POST", path, body);
        assert_eq!(st, 403, "{path}: {b}");
        assert!(b["error"].as_str().is_some_and(|e| e.contains("só consulta tarefas")), "{b}");
    }
    // O relatório de outra tarefa também não.
    let (st, b) = call(&agent, "POST", "/tasks/t2/done", json!({ "report": "me passando por t2" }));
    assert_eq!(st, 403, "{b}");
    assert_eq!(core.task(&env.project, "t2").map(|t| t.status), Some(TaskStatus::Running));
    // E um agente que não é o da tarefa não entrega relatório por ela.
    let (st, _) = call(&boss, "POST", "/tasks/t1/done", json!({ "report": "eu entrego por ele" }));
    assert_eq!(st, 403);
    // Nada mudou: t1 e t2 seguem em andamento, o nome do criador intacto, nenhuma t3.
    assert_eq!(
        core.tasks(&env.project).iter().map(|t| (t.id.as_str(), t.status)).collect::<Vec<_>>(),
        [("t1", TaskStatus::Running), ("t2", TaskStatus::Running)]
    );

    // O relatório da própria tarefa vale.
    commit_file(Path::new(t1["worktree"].as_str().expect("worktree")), "feito.txt", "ok\n", "feat: feito");
    let (st, b) = call(&agent, "POST", "/tasks/t1/done", json!({ "report": "Feito por mim." }));
    assert_eq!((st, b["task"]["status"].as_str()), (200, Some("review")), "{b}");
    // Quem criou (o orquestrador, não o agente da tarefa) mescla, valida e descarta.
    let (st, b) = call(&boss, "POST", "/tasks/t1/merge", json!({}));
    assert_eq!((st, b["task"]["status"].as_str()), (200, Some("validating")), "{b}");
    // Aguardando validação o agente segue vivo (o token vale), mas continua sem poder validar nem reprovar.
    assert_eq!(call(&agent, "GET", "/tasks", json!(null)).0, 200);
    assert_eq!(call(&agent, "POST", "/tasks/t1/validate", json!({})).0, 403);
    assert_eq!(call(&agent, "POST", "/tasks/t1/reject", json!({ "feedback": "x" })).0, 403);
    let (st, b) = call(&boss, "POST", "/tasks/t1/validate", json!({}));
    assert_eq!((st, b["task"]["status"].as_str()), (200, Some("merged")), "{b}");
    let (st, _) = call(&boss, "POST", "/tasks/t2/discard", json!({}));
    assert_eq!(st, 200);
    // O token do agente da t1 deixou de existir com a instância; da t2 também.
    assert_eq!(call(&agent, "GET", "/tasks", json!(null)).0, 403);
    assert_eq!(t2["id"], "t2");
    core.shutdown();
}

#[test]
fn learn_answers_from_the_worktree_of_the_asking_agent() {
    need_git!();
    let env = start();
    let core = &env.core;
    // Um aprendizado versionado na base, antes de a tarefa existir (a worktree parte dela).
    let text = "---\ntitulo: Cuidado com src\nresumo: Regra de src.\ntouches: [src/**]\n---\nNão mexa em src sem ler isto.\n";
    std::fs::create_dir_all(env.ws.join("memory").join("aprendizados")).expect("pasta");
    commit_file(&env.ws, "memory/aprendizados/src.md", text, "docs: aprendizado");
    core.register_instance(&env.project, "claude");
    let boss = tab_token(&env, "claude");
    let (st, b) = http(core, "POST", "/tasks", &boss, &json!({ "title": "Com memória", "brief": "Faça.", "start": true }).to_string());
    assert_eq!(st, 200, "{b}");
    let worktree = b["task"]["worktree"].as_str().expect("worktree").to_owned();
    let agent = tab_token(&env, "claude@t1");
    let ask = |token: &str, paths: Value| http(core, "POST", "/learn", token, &json!({ "paths": paths }).to_string());

    // Caminho relativo e caminho absoluto dentro da worktree dão a mesma resposta.
    let (st, rel) = ask(&agent, json!(["src/lib.rs"]));
    assert_eq!(st, 200, "{rel}");
    let rel = rel["text"].as_str().expect("texto").to_owned();
    assert!(rel.contains("Não mexa em src sem ler isto.") && rel.contains("### Cuidado com src"), "{rel}");
    let absolute = Path::new(&worktree).join("src").join("lib.rs").to_string_lossy().into_owned();
    let (st, abs) = ask(&agent, json!([absolute]));
    assert_eq!((st, abs["text"].as_str()), (200, Some(rel.as_str())));
    // Arquivo sem aprendizado: diz que nada casa, e mostra o índice.
    let (st, none) = ask(&agent, json!(["docs/x.md"]));
    assert_eq!(st, 200);
    assert!(
        none["text"]
            .as_str()
            .is_some_and(|t| t.contains("Nenhum aprendizado específico") && t.contains("memory/aprendizados/src.md — Regra de src.")),
        "{none}"
    );
    // Sem caminhos: os globais e o índice dos específicos. O orquestrador (sem tarefa) lê do repositório do projeto.
    let (st, b) = ask(&agent, json!([]));
    assert!(
        st == 200 && b["text"].as_str().is_some_and(|t| t.starts_with("Aprendizados do projeto:") && t.contains("memory/aprendizados/src.md — Regra de src.")),
        "{st} {b}"
    );
    let (st, b) = ask(&boss, json!(["src/lib.rs"]));
    assert!(st == 200 && b["text"].as_str().is_some_and(|t| t.contains("Não mexa em src")), "{st} {b}");
    // Só POST.
    assert_eq!(http(core, "GET", "/learn", &boss, "").0, 404);
}
