//! `ask-agent --task ...` de ponta a ponta: o binário real contra a ponte real, com o git real
//! (pula com aviso se o `git` não estiver instalado).

use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};
use std::sync::OnceLock;

use agentdeck_core::paths::AppPaths;
use agentdeck_core::{Config, Core, Ident, StartOptions};

fn git_exe() -> Option<PathBuf> {
    which::which("git").ok()
}

/// Configuração global do git isolada (identidade para os commits, branch `main`).
fn isolated_git_config() {
    static DIR: OnceLock<PathBuf> = OnceLock::new();
    DIR.get_or_init(|| {
        let dir = tempfile::tempdir().expect("tmp").keep();
        std::fs::write(
            dir.join("gitconfig"),
            "[user]\n\tname = Teste\n\temail = teste@example.com\n[init]\n\tdefaultBranch = main\n[commit]\n\tgpgsign = false\n",
        )
        .expect("gitconfig");
        std::env::set_var("GIT_CONFIG_GLOBAL", dir.join("gitconfig"));
        std::env::set_var("GIT_CONFIG_NOSYSTEM", "1");
        std::env::set_var("GIT_CEILING_DIRECTORIES", std::env::temp_dir());
        dir
    });
}

fn git(dir: &Path, args: &[&str]) {
    let out = Command::new(git_exe().expect("git")).current_dir(dir).args(args).output().expect("git");
    assert!(out.status.success(), "git {args:?}: {}", String::from_utf8_lossy(&out.stderr));
}

fn start_core(root: &Path) -> Core {
    let paths = AppPaths {
        app_dir: root.to_path_buf(),
        bin_dir: root.join("bin"),
        data_dir: root.join("dados"),
        default_workspace: root.join("workspace"),
        whisper_dir: root.join("whisper"),
        config_file: root.join("cfg").join("config.json"),
    };
    // Um "claude" instalado: script no Unix; no Windows, o .exe vazio onde o app procura.
    let fake_bin = root.join("fake-bin");
    std::fs::create_dir_all(&fake_bin).expect("mkdir");
    if cfg!(windows) {
        let bin = root.join("home").join(".local").join("bin");
        std::fs::create_dir_all(&bin).expect("mkdir");
        std::fs::write(bin.join("claude.exe"), b"").expect("write");
    } else {
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let script = fake_bin.join("claude");
            std::fs::write(&script, "#!/bin/sh\ncat >/dev/null\n").expect("write");
            std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).expect("chmod");
        }
    }
    let git_dir = git_exe().and_then(|g| g.parent().map(Path::to_path_buf)).unwrap_or_default();
    let opts = StartOptions {
        search_path: Some(std::env::join_paths([fake_bin, git_dir]).expect("PATH")),
        home_dir: Some(root.join("home")),
        watch_workspace: false,
        config: Some(Config::with_project(paths.default_workspace.clone())),
        system_dirs: false,
    };
    let core = Core::start_with(paths, opts).expect("start").0;
    core.enable_tab_delivery();
    core
}

/// `ask-agent` com o token mestre (usuário) ou o de uma aba (`agent = Some((id, token))`).
fn run(core: &Core, agent: Option<(&str, &str)>, args: &[&str], stdin: &str) -> Output {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_ask-agent"));
    for name in ["PORT", "TOKEN", "AGENT", "DEPTH", "ASK_WAIT"] {
        cmd.env_remove(format!("COMMANDDECK_{name}"));
    }
    cmd.args(args).env_remove("AGENTDECK_DEPTH").env("AGENTDECK_PORT", core.bridge_port().to_string());
    match agent {
        Some((id, token)) => cmd.env("AGENTDECK_TOKEN", token).env("AGENTDECK_AGENT", id),
        None => cmd.env("AGENTDECK_TOKEN", core.bridge_token()).env("AGENTDECK_AGENT", "shell"),
    };
    cmd.stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::piped());
    let mut child = cmd.spawn().expect("spawn ask-agent");
    if let Some(mut pipe) = child.stdin.take() {
        let _ = pipe.write_all(stdin.as_bytes());
    }
    child.wait_with_output().expect("output")
}

fn text(b: &[u8]) -> String {
    String::from_utf8_lossy(b).into_owned()
}

#[test]
fn the_task_commands_drive_the_whole_lifecycle() {
    if git_exe().is_none() {
        eprintln!("git não instalado: teste pulado");
        return;
    }
    isolated_git_config();
    let tmp = tempfile::tempdir().expect("tmp");
    let core = start_core(tmp.path());
    let ws = tmp.path().join("workspace");
    git(&ws, &["init", "-q"]);
    std::fs::write(ws.join("a.txt"), "um\n").expect("w");
    git(&ws, &["add", "-A"]);
    git(&ws, &["commit", "-q", "-m", "inicial"]);
    let project = core.active_project_id();

    let out = run(&core, None, &["--task", "list"], "");
    assert_eq!((out.status.code(), text(&out.stdout).trim()), (Some(0), "Nenhuma tarefa neste projeto."), "{}", text(&out.stderr));

    // Criar e iniciar, com o pedido pela entrada padrão e as opções.
    let out =
        run(&core, None, &["--task", "new", "Corrigir o login", "--model", "sonnet", "--start", "-"], "Conserte o login.\nCom testes.");
    assert_eq!(out.status.code(), Some(0), "{}", text(&out.stderr));
    let shown = text(&out.stdout);
    assert!(shown.contains("Tarefa t1 criada: Corrigir o login") && shown.contains("claude@t1 (sonnet)"), "{shown}");
    assert!(shown.contains("iniciada na worktree"), "{shown}");
    let t1 = core.task(&project, "t1").expect("t1");
    assert_eq!((t1.brief.as_str(), t1.model.as_deref()), ("Conserte o login.\nCom testes.", Some("sonnet")));
    assert!(t1.worktree.is_dir());

    // Só criar (pedido num arquivo com acentos, via `--file`), listar, mostrar.
    let brief = tmp.path().join("pedido.md");
    std::fs::write(&brief, "Faça outra coisa: ação e revisão.\n").expect("w");
    let out = run(&core, None, &["--task", "new", "Segunda tarefa", "--file", brief.to_str().expect("utf8")], "");
    assert_eq!(out.status.code(), Some(0), "{}", text(&out.stderr));
    assert!(text(&out.stdout).contains("inicie com: ask-agent --task start t2"), "{}", text(&out.stdout));
    assert_eq!(core.task(&project, "t2").expect("t2").brief, "Faça outra coisa: ação e revisão.");
    let out = run(&core, None, &["--task", "list"], "");
    let list = text(&out.stdout);
    let lines: Vec<&str> = list.lines().collect();
    assert_eq!(lines.len(), 2, "{list}");
    assert!(
        lines[0].starts_with("t1")
            && lines[0].contains("andamento")
            && lines[0].contains("claude@t1 (sonnet)")
            && lines[0].ends_with("Corrigir o login"),
        "{list}"
    );
    assert!(lines[1].starts_with("t2") && lines[1].contains("rascunho"), "{list}");
    let out = run(&core, None, &["--task", "show", "1"], "");
    assert!(text(&out.stdout).contains("Worktree:") && text(&out.stdout).contains("Base: main"), "{}", text(&out.stdout));

    // O agente da tarefa entrega o relatório (token da aba dele); não pode mexer no resto.
    let token = core.spawn_spec_for(&project, &Ident::parse("claude@t1").expect("id")).expect("spec").env["AGENTDECK_TOKEN"].clone();
    let agent = Some(("claude@t1", token.as_str()));
    std::fs::write(t1.worktree.join("login.txt"), "ok\n").expect("w");
    git(&t1.worktree, &["add", "-A"]);
    git(&t1.worktree, &["commit", "-q", "-m", "fix: login"]);
    let out = run(&core, agent, &["--task", "merge", "t1"], "");
    assert_eq!(out.status.code(), Some(1));
    assert!(text(&out.stderr).contains("só consulta tarefas"), "{}", text(&out.stderr));
    let out = run(&core, agent, &["--task", "done", "t1", "-"], "  \n");
    assert_eq!(out.status.code(), Some(2), "relatório vazio é erro de uso");
    let out = run(&core, agent, &["--task", "done", "t1", "-"], "Arrumei o login.\nTestes: ok.");
    assert_eq!(out.status.code(), Some(0), "{}", text(&out.stderr));
    assert!(text(&out.stdout).contains("Relatório da tarefa t1 entregue"), "{}", text(&out.stdout));
    let out = run(&core, None, &["--task", "show", "t1"], "");
    let detail = text(&out.stdout);
    assert!(
        detail.contains("Estado: revisão") && detail.contains("Relatório:\nArrumei o login.") && detail.contains("login.txt"),
        "{detail}"
    );

    // Devolver, entregar de novo e mesclar.
    let out = run(&core, None, &["--task", "return", "t1", "Falta", "o", "teste"], "");
    assert_eq!(out.status.code(), Some(0), "{}", text(&out.stderr));
    assert!(text(&out.stdout).contains("devolvida a claude@t1"), "{}", text(&out.stdout));
    // O relatório também vem de um arquivo (`--file`): é o caminho que preserva os acentos, que o
    // shell do agente (PowerShell 5.1, `$OutputEncoding` em US-ASCII) trocaria por "?" no stdin.
    let report = tmp.path().join("relatorio.md");
    std::fs::write(&report, "Agora com teste: ação e coração ✓.\n").expect("w");
    let out = run(&core, agent, &["--task", "done", "t1", "--file", report.to_str().expect("utf8")], "");
    assert_eq!(out.status.code(), Some(0), "{}", text(&out.stderr));
    let t1 = core.task(&project, "t1").expect("t1");
    assert_eq!(t1.report.as_deref(), Some("Agora com teste: ação e coração ✓."), "acentos preservados pelo --file");
    let out = run(&core, None, &["--task", "merge", "t1"], "");
    assert_eq!(out.status.code(), Some(0), "{}", text(&out.stderr));
    assert!(text(&out.stdout).contains("Tarefa t1 mesclada em main; aguardando validação"), "{}", text(&out.stdout));
    assert!(ws.join("login.txt").is_file() && t1.worktree.is_dir(), "mesclar mantém a worktree até validar");
    let out = run(&core, None, &["--task", "show", "t1"], "");
    assert!(text(&out.stdout).contains("Estado: validação"), "{}", text(&out.stdout));

    // Reprovar (o motivo vem de um arquivo) e entregar de novo; depois validar.
    let out = run(&core, None, &["--task", "reject", "t1"], "");
    assert_eq!(out.status.code(), Some(2), "reprovar sem motivo é erro de uso");
    let why = tmp.path().join("motivo.md");
    std::fs::write(&why, "Não salva a sessão: ação ✗.\n").expect("w");
    let out = run(&core, None, &["--task", "reject", "t1", "--file", why.to_str().expect("utf8")], "");
    assert_eq!(out.status.code(), Some(0), "{}", text(&out.stderr));
    assert!(text(&out.stdout).contains("Tarefa t1 reprovada; claude@t1 recebeu o motivo"), "{}", text(&out.stdout));
    let out = run(&core, None, &["--task", "validate", "t1"], "");
    assert_eq!(out.status.code(), Some(1), "em andamento não se valida");
    std::fs::write(t1.worktree.join("login2.txt"), "ok\n").expect("w");
    git(&t1.worktree, &["add", "-A"]);
    git(&t1.worktree, &["commit", "-q", "-m", "fix: sessão"]);
    assert_eq!(run(&core, agent, &["--task", "done", "t1", "-"], "Sessão corrigida.").status.code(), Some(0));
    assert_eq!(run(&core, None, &["--task", "merge", "t1"], "").status.code(), Some(0));
    let out = run(&core, None, &["--task", "validate", "t1"], "");
    assert_eq!(out.status.code(), Some(0), "{}", text(&out.stderr));
    assert!(text(&out.stdout).contains("Tarefa t1 validada e concluída"), "{}", text(&out.stdout));
    assert!(ws.join("login2.txt").is_file() && !t1.worktree.exists());

    // Erros: estado errado (1), uso incorreto (2), agente inválido (1), descartar.
    let out = run(&core, None, &["--task", "merge", "t1"], "");
    assert_eq!(out.status.code(), Some(1));
    assert!(text(&out.stderr).contains("concluída"), "{}", text(&out.stderr));
    assert_eq!(run(&core, None, &["--task", "bogus"], "").status.code(), Some(2));
    assert_eq!(run(&core, None, &["--task"], "").status.code(), Some(2));
    assert_eq!(run(&core, None, &["--task", "show", "xyz"], "").status.code(), Some(2));
    assert_eq!(run(&core, None, &["--task", "show", "t9"], "").status.code(), Some(1));
    let out = run(&core, None, &["--task", "new", "x", "--agent", "shell", "-"], "pedido");
    assert_eq!(out.status.code(), Some(1));
    assert!(text(&out.stderr).contains("agente desconhecido ou não instalado"), "{}", text(&out.stderr));
    let out = run(&core, None, &["--task", "discard", "t2"], "");
    assert_eq!(out.status.code(), Some(0), "{}", text(&out.stderr));
    assert!(text(&out.stdout).contains("Tarefa t2 descartada"), "{}", text(&out.stdout));
    let out = run(&core, None, &["--task", "list"], "");
    assert!(text(&out.stdout).contains("concluída") && text(&out.stdout).contains("descartada"), "{}", text(&out.stdout));
    // A ajuda cita as tarefas.
    assert!(text(&run(&core, None, &["--help"], "").stdout).contains("--task"));
    core.shutdown();
}
