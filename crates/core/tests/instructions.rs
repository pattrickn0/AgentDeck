//! Como as instruções do AgentDeck chegam a cada CLI, num projeto do usuário que ainda não tem `AGENTS.md`:
//! cada aba recebe pelo canal do próprio CLI, e só o que não tem outro jeito ganha um `AGENTS.md` novo.

use std::path::{Path, PathBuf};

use agentdeck_core::paths::AppPaths;
use agentdeck_core::{Config, Core, CoreEvent, FeedKind, Ident, StartOptions};
use crossbeam_channel::Receiver;

struct Env {
    _tmp: tempfile::TempDir,
    /// Onde ficam os projetos do usuário: fora da pasta do app (`root`), que o app trata como dele.
    users: tempfile::TempDir,
    core: Core,
    rx: Receiver<CoreEvent>,
    root: PathBuf,
}

/// `Core` em pastas temporárias com CLIs falsos (só precisam existir: a resolução não os executa).
fn start(agents: &[&str]) -> Env {
    let tmp = tempfile::tempdir().expect("tmp");
    let root = tmp.path().to_path_buf();
    let paths = AppPaths {
        app_dir: root.clone(),
        bin_dir: root.join("bin"),
        data_dir: root.join("dados"),
        default_workspace: root.join("workspace"),
        whisper_dir: root.join("whisper"),
        config_file: root.join("cfg").join("config.json"),
    };
    let fake_bin = root.join("fake-bin");
    std::fs::create_dir_all(&fake_bin).expect("mkdir");
    for name in agents {
        write_fake(&fake_bin, name);
    }
    let opts = StartOptions {
        search_path: Some(fake_bin.into_os_string()),
        home_dir: Some(root.join("home")),
        watch_workspace: false,
        config: Some(Config::with_project(paths.default_workspace.clone())),
        system_dirs: false,
    };
    let (core, rx) = Core::start_with(paths, opts).expect("start");
    Env { _tmp: tmp, users: tempfile::tempdir().expect("tmp"), core, rx, root }
}

#[cfg(unix)]
fn write_fake(dir: &Path, name: &str) {
    use std::os::unix::fs::PermissionsExt;
    let path = dir.join(name);
    std::fs::write(&path, "#!/bin/sh\n").expect("write");
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).expect("chmod");
}

#[cfg(not(unix))]
fn write_fake(dir: &Path, name: &str) {
    // Antigravity é `agy.exe`; os outros, `<nome>.exe`.
    let exe = if name == "antigravity" { "agy" } else { name };
    std::fs::write(dir.join(format!("{exe}.exe")), "").expect("write");
}

impl Env {
    /// Pasta nova do usuário (fora da pasta do app), sem `AGENTS.md`, como projeto aberto agora.
    fn user_project(&self, name: &str) -> (String, PathBuf) {
        let dir = self.users.path().join(name);
        std::fs::create_dir_all(&dir).expect("mkdir");
        (self.core.create_project(&dir, Some(name)).id, dir)
    }

    fn spec(&self, project: &str, ident: &str) -> agentdeck_core::SpawnSpec {
        self.core.spawn_spec_for(project, &Ident::parse(ident).expect("id")).expect("spec")
    }

    fn feed_texts(&self) -> Vec<String> {
        self.rx
            .try_iter()
            .filter_map(|ev| match ev {
                CoreEvent::Feed(f) if f.kind == FeedKind::System => Some(f.text),
                _ => None,
            })
            .collect()
    }
}

fn arg_after(args: &[String], flag: &str) -> String {
    let i = args.iter().position(|a| a == flag).unwrap_or_else(|| panic!("sem {flag} em {args:?}"));
    args[i + 1].clone()
}

#[test]
fn every_cli_gets_the_instructions_without_touching_the_users_repository() {
    let env = start(&["claude", "codex", "opencode", "aider", "interpreter"]);
    let (project, dir) = env.user_project("novo");

    // Claude: prompt de sistema, com a regra dos subagentes e a memória geral.
    let claude = env.spec(&project, "claude");
    let text = arg_after(&claude.args, "--append-system-prompt");
    assert!(text.contains("NÃO use subagentes internos do seu CLI"), "{text}");
    assert!(text.contains("memory/aprendizados/") && text.contains("ask-agent --learn"), "{text}");
    assert!(text.contains("ask-agent --task new"), "{text}");

    // Codex: `-c developer_instructions=<texto TOML>`.
    let codex = env.spec(&project, "codex");
    let value = codex.args.iter().find(|a| a.starts_with("developer_instructions=")).expect("-c developer_instructions");
    let toml = value.strip_prefix("developer_instructions=").expect("chave");
    let decoded: String = serde_json::from_str(toml).expect("string TOML/JSON");
    assert!(decoded.contains("NÃO use subagentes internos"), "{decoded}");
    assert!(decoded.contains("Seu nome aqui é \"codex\""), "{decoded}");

    // OpenCode: arquivo de instruções na configuração (e servidor próprio, que a lê).
    let opencode = env.spec(&project, "opencode");
    assert!(opencode.args.contains(&"--standalone".to_owned()), "{:?}", opencode.args);
    let config: serde_json::Value = serde_json::from_str(&opencode.env["OPENCODE_CONFIG_CONTENT"]).expect("json");
    let file = PathBuf::from(config["instructions"][0].as_str().expect("arquivo"));
    assert!(file.starts_with(env.root.join("dados")), "{}", file.display());
    let from_file = std::fs::read_to_string(&file).expect("ler");
    assert!(from_file.contains("NÃO use subagentes internos") && from_file.contains("AGENTDECK_AGENT"), "{from_file}");

    // Aider: o mesmo arquivo, como contexto só de leitura.
    let aider = env.spec(&project, "aider");
    assert_eq!(PathBuf::from(arg_after(&aider.args, "--read")), file);

    // Open Interpreter: instruções personalizadas.
    let interpreter = env.spec(&project, "interpreter");
    assert!(arg_after(&interpreter.args, "--custom_instructions").contains("NÃO use subagentes internos"));

    // Nada disso suja o repositório do usuário.
    assert!(!dir.join("AGENTS.md").exists(), "o app criou AGENTS.md para um CLI que tem canal próprio");
    assert!(env.feed_texts().is_empty());
}

#[test]
fn a_cli_that_only_reads_agents_md_gets_one_with_just_the_app_block() {
    let env = start(&["antigravity"]);
    let (project, dir) = env.user_project("novo");
    assert!(!dir.join("AGENTS.md").exists());

    env.spec(&project, "antigravity");
    let text = std::fs::read_to_string(dir.join("AGENTS.md")).expect("AGENTS.md criado");
    assert!(text.starts_with("<!-- agentdeck:start -->") && text.trim_end().ends_with("<!-- agentdeck:end -->"), "{text}");
    assert!(text.contains("NÃO use subagentes internos") && text.contains("memory/aprendizados/"), "{text}");
    // O usuário é avisado na Conversa, uma vez.
    let notices = env.feed_texts();
    assert_eq!(notices.len(), 1, "{notices:?}");
    assert!(notices[0].contains("AGENTS.md"), "{notices:?}");

    // Abrir de novo não muda nada nem avisa de novo (nada novo no `git status`).
    env.spec(&project, "antigravity");
    assert_eq!(std::fs::read_to_string(dir.join("AGENTS.md")).expect("ler"), text);
    assert!(env.feed_texts().is_empty());
}

#[test]
fn the_users_own_agents_md_is_kept_and_not_announced_as_created() {
    let env = start(&["antigravity"]);
    let (project, dir) = env.user_project("com-regras");
    std::fs::write(dir.join("AGENTS.md"), "# Regras do meu projeto\n\nUse tabs.\n").expect("write");

    env.spec(&project, "antigravity");
    let text = std::fs::read_to_string(dir.join("AGENTS.md")).expect("ler");
    assert!(text.starts_with("# Regras do meu projeto\n\nUse tabs.\n") && text.contains("NÃO use subagentes internos"), "{text}");
    assert!(env.feed_texts().is_empty());
}
