//! Descoberta do repositório GitHub pelo remoto `origin` com o git real (pula com aviso se o `git`
//! não estiver instalado). Sem rede: só `git remote get-url`.

use std::path::{Path, PathBuf};
use std::process::Command;

use commanddeck_core::github::{GithubError, RepoSlug};
use commanddeck_core::paths::AppPaths;
use commanddeck_core::{Config, Core, StartOptions};

fn git_exe() -> Option<PathBuf> {
    which::which("git").ok()
}

fn start(root: &Path) -> Core {
    let paths = AppPaths {
        app_dir: root.to_path_buf(),
        bin_dir: root.join("bin"),
        data_dir: root.join("dados"),
        default_workspace: root.join("ws"),
        whisper_dir: root.join("whisper"),
        config_file: root.join("cfg").join("config.json"),
    };
    let search = git_exe().and_then(|g| g.parent().map(Path::to_path_buf)).unwrap_or_default();
    let opts = StartOptions {
        search_path: Some(search.into_os_string()),
        home_dir: Some(root.join("home")),
        ollama_discovery: false,
        watch_workspace: false,
        config: Some(Config::with_project(paths.default_workspace.clone())),
        system_dirs: false,
    };
    Core::start_with(paths, opts).expect("start").0
}

fn git(dir: &Path, args: &[&str]) {
    let out = Command::new("git")
        .current_dir(dir)
        .args(args)
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_CEILING_DIRECTORIES", std::env::temp_dir())
        .output()
        .expect("git");
    assert!(out.status.success(), "git {args:?}: {}", String::from_utf8_lossy(&out.stderr));
}

#[test]
fn finds_the_repo_from_the_origin_remote() {
    if git_exe().is_none() {
        eprintln!("git não instalado: teste pulado");
        return;
    }
    let tmp = tempfile::tempdir().expect("tmp");
    let core = start(tmp.path());
    let repo = tmp.path().join("repo");
    std::fs::create_dir_all(&repo).unwrap();

    // Fora de um repositório.
    assert_eq!(core.github_repo_at(&repo), Err(GithubError::NotARepo));

    git(&repo, &["init", "-q"]);
    // Sem remoto.
    assert_eq!(core.github_repo_at(&repo), Err(GithubError::NoRemote));

    git(&repo, &["remote", "add", "origin", "git@github.com:dono/repo.git"]);
    assert_eq!(core.github_repo_at(&repo), Ok(RepoSlug::new("dono", "repo")));

    git(&repo, &["remote", "set-url", "origin", "https://github.com/outro/projeto"]);
    assert_eq!(core.github_repo_at(&repo), Ok(RepoSlug::new("outro", "projeto")));

    git(&repo, &["remote", "set-url", "origin", "https://gitlab.com/dono/repo.git"]);
    assert!(matches!(core.github_repo_at(&repo), Err(GithubError::NotGithubRemote { .. })));

    // Pasta vazia / inexistente: sem projeto.
    assert_eq!(core.github_repo_at(Path::new("")), Err(GithubError::NoProject));
}

#[test]
fn opening_urls_is_limited_to_github() {
    if git_exe().is_none() {
        return;
    }
    let tmp = tempfile::tempdir().expect("tmp");
    let core = start(tmp.path());
    assert!(core.open_github_url("file:///C:/Windows/System32/calc.exe").is_err());
    assert!(core.open_github_url("https://evil.example/https://github.com/").is_err());
    assert!(core.open_github_url("https://github.com.evil.example/x").is_err());
}

/// Rede e credencial reais: `cargo test -p commanddeck-core --test github -- --ignored --nocapture`.
/// Lê PRs, issues e execuções do repositório do `origin` desta cópia do projeto e imprime um resumo
/// (nunca o token).
#[test]
#[ignore = "usa a rede e a credencial do usuário"]
fn live_overview_of_this_repository() {
    use commanddeck_core::github::GithubPayload;
    use commanddeck_core::CoreEvent;

    let tmp = tempfile::tempdir().expect("tmp");
    let root = tmp.path();
    // Pasta temporária com o mesmo `origin` desta cópia (o core reescreve o AGENTS.md da pasta de trabalho).
    let here = Path::new(env!("CARGO_MANIFEST_DIR"));
    let url = Command::new("git").current_dir(here).args(["remote", "get-url", "origin"]).output().expect("git");
    let url = String::from_utf8_lossy(&url.stdout).trim().to_owned();
    let repo_dir = root.join("repo");
    std::fs::create_dir_all(&repo_dir).unwrap();
    git(&repo_dir, &["init", "-q"]);
    git(&repo_dir, &["remote", "add", "origin", &url]);
    let paths = AppPaths {
        app_dir: root.to_path_buf(),
        bin_dir: root.join("bin"),
        data_dir: root.join("dados"),
        default_workspace: repo_dir.clone(),
        whisper_dir: root.join("whisper"),
        config_file: root.join("cfg").join("config.json"),
    };
    let search = git_exe().and_then(|g| g.parent().map(Path::to_path_buf)).unwrap_or_default();
    let opts = StartOptions {
        search_path: Some(search.into_os_string()),
        home_dir: Some(root.join("home")),
        ollama_discovery: false,
        watch_workspace: false,
        config: Some(Config::with_project(repo_dir)),
        system_dirs: false,
    };
    let (core, rx) = Core::start_with(paths, opts).expect("start");
    core.github_refresh(true);
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(60);
    loop {
        let left = deadline.saturating_duration_since(std::time::Instant::now());
        match rx.recv_timeout(left) {
            Ok(CoreEvent::Github(ev)) => match ev.payload {
                GithubPayload::Overview(Ok(ov)) => {
                    println!(
                        "{}: autenticado={} PRs={} issues={} execuções={} limite={:?}",
                        ov.repo,
                        ov.authenticated,
                        ov.prs.len(),
                        ov.issues.len(),
                        ov.runs.len(),
                        ov.rate_limit
                    );
                    for pr in ov.prs.iter().take(3) {
                        println!("  PR #{} {} [{}]", pr.number, pr.title, pr.checks.label());
                    }
                    if let Some(run) = ov.runs.first() {
                        println!("  última execução: {} ({}) {:?}", run.display_title, run.raw_status, run.conclusion);
                        core.github_load_run_jobs(run.id, true);
                        // Espera também os jobs (e o fim do log, se algum job falhou).
                        while let Ok(CoreEvent::Github(ev2)) = rx.recv_timeout(std::time::Duration::from_secs(60)) {
                            if let GithubPayload::RunJobs { result, failed_log, .. } = ev2.payload {
                                let jobs = result.expect("jobs");
                                println!(
                                    "  jobs: {}",
                                    jobs.iter().map(|j| format!("{}={}", j.name, j.status.label())).collect::<Vec<_>>().join(", ")
                                );
                                match failed_log {
                                    Some((id, Ok(log))) => {
                                        println!("  log do job {id}: {} linhas (cortado: {})", log.lines.len(), log.truncated)
                                    }
                                    Some((id, Err(e))) => println!("  log do job {id}: {e}"),
                                    None => println!("  nenhum job vermelho"),
                                }
                                // O download do log (com o redirecionamento para o armazenamento) também vale para job verde.
                                if let Some(job) = jobs.first() {
                                    core.github_load_job_log(job.id, true);
                                    while let Ok(CoreEvent::Github(ev3)) = rx.recv_timeout(std::time::Duration::from_secs(60)) {
                                        if let GithubPayload::JobLog { result, .. } = ev3.payload {
                                            match result {
                                                Ok(log) => println!(
                                                    "  log de {}: {} linhas (cortado: {}); última: {:?}",
                                                    job.name,
                                                    log.lines.len(),
                                                    log.truncated,
                                                    log.lines.last()
                                                ),
                                                Err(e) => println!("  log de {}: {e}", job.name),
                                            }
                                            break;
                                        }
                                    }
                                }
                                break;
                            }
                        }
                    }
                    return;
                }
                GithubPayload::Overview(Err(e)) => panic!("falha ao ler o GitHub: {e}"),
                _ => {}
            },
            Ok(_) => {}
            Err(_) => panic!("sem resposta do GitHub em 60 s"),
        }
    }
}
