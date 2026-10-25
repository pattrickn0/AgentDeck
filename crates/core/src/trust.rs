//! Confiança da pasta. Claude, Codex e Antigravity perguntam "Do you trust this folder?" na primeira vez que abrem
//! numa pasta, e uma aba aberta em segundo plano (por `ask-agent` ou por uma tarefa) fica parada na pergunta, sem
//! ninguém ver. Abrir a pasta como projeto no AgentDeck já é a decisão do usuário, então o app registra a confiança
//! antes de abrir o CLI (conferido em 2026-10-25 com Claude Code 2.1.310, Codex 0.164.0 e `agy` 1.4.7):
//!
//! - **Claude:** `projects["C:/pasta"].hasTrustDialogAccepted` em `~/.claude.json`; vale para as subpastas (a worktree de
//!   uma tarefa herda a do projeto), então só grava quando nem a pasta nem uma pasta-mãe foi aceita;
//! - **Codex:** `-c projects={'<pasta>'={trust_level='trusted'}}` na linha de comando, sem gravar o `config.toml`;
//! - **Antigravity:** `trustedWorkspaces` em `~/.gemini/antigravity-cli/settings.json`, por **caminho exato** (nem a
//!   pasta-mãe nem outra grafia valem), por isso cada worktree pedia de novo; a entrada sai quando a worktree sai.
//!
//! A pasta pessoal e a raiz de um disco nunca são registradas: confiar nelas valeria para tudo lá dentro.

use std::path::{Path, PathBuf};
use std::sync::Mutex;

use serde_json::{Map, Value};

/// As gravações nos arquivos dos CLIs são ler, mudar e trocar: duas abas abrindo juntas não podem se atropelar.
static WRITES: Mutex<()> = Mutex::new(());

/// Prepara a confiança de `cwd` (e de `project_dir`, se for outra pasta) para o CLI `kind` e devolve os argumentos que
/// vão na linha de comando (só o Codex usa). Falha ao gravar só fica no log: no pior caso, o CLI pergunta como antes.
pub(crate) fn prepare(kind: &str, cwd: &Path, project_dir: &Path, home: &Path) -> Vec<String> {
    let mut dirs: Vec<&Path> = vec![cwd];
    if !same_path(cwd, project_dir) {
        dirs.push(project_dir);
    }
    dirs.retain(|d| may_trust(d, home));
    let Some(&first) = dirs.first() else { return Vec::new() };
    match kind {
        "claude" => {
            if let Err(e) = trust_claude(&claude_file(home), first) {
                log::warn!("confiança da pasta para o Claude ({}): {e}", first.display());
            }
            Vec::new()
        }
        "codex" => codex_args(&dirs),
        "antigravity" if first == cwd => {
            if let Err(e) = trust_agy(&agy_file(home), cwd) {
                log::warn!("confiança da pasta para o Antigravity ({}): {e}", cwd.display());
            }
            Vec::new()
        }
        _ => Vec::new(),
    }
}

/// Tira `dir` (a worktree de uma tarefa que acabou de sair) da lista do Antigravity, para ela não crescer a cada tarefa.
pub(crate) fn forget(dir: &Path, home: &Path) {
    if let Err(e) = forget_agy(&agy_file(home), dir) {
        log::warn!("confiança da pasta do Antigravity ({}): {e}", dir.display());
    }
}

/// A pasta pessoal e a raiz de um disco ficam de fora.
fn may_trust(dir: &Path, home: &Path) -> bool {
    dir.parent().is_some() && !same_path(dir, home)
}

fn same_path(a: &Path, b: &Path) -> bool {
    let norm = |p: &Path| {
        let s = p.to_string_lossy().replace('\\', "/");
        let s = s.trim_end_matches('/').to_owned();
        if cfg!(windows) {
            s.to_lowercase()
        } else {
            s
        }
    };
    norm(a) == norm(b)
}

/// `~/.claude.json` (ou `$CLAUDE_CONFIG_DIR/.claude.json`, onde o Claude Code o procura quando a variável existe).
fn claude_file(home: &Path) -> PathBuf {
    match std::env::var_os("CLAUDE_CONFIG_DIR").filter(|v| !v.is_empty()) {
        Some(dir) => PathBuf::from(dir).join(".claude.json"),
        None => home.join(".claude.json"),
    }
}

fn agy_file(home: &Path) -> PathBuf {
    home.join(".gemini").join("antigravity-cli").join("settings.json")
}

/// A chave do Claude: barras normais, sem a barra do fim (`C:/Users/voce/projeto`).
fn claude_key(dir: &Path) -> String {
    dir.to_string_lossy().replace('\\', "/").trim_end_matches('/').to_owned()
}

/// `dir` ou uma pasta-mãe dele já tem a confiança aceita no Claude.
fn claude_trusted(projects: &Map<String, Value>, dir: &Path) -> bool {
    let mut cur = Some(dir);
    while let Some(d) = cur {
        let accepted = projects
            .iter()
            .any(|(k, v)| same_path(Path::new(k), d) && v.get("hasTrustDialogAccepted").and_then(Value::as_bool) == Some(true));
        if accepted {
            return true;
        }
        cur = d.parent();
    }
    false
}

/// Marca `dir` como aceita no `~/.claude.json`. Sem o arquivo (Claude Code nunca aberto), não cria: o primeiro uso tem
/// outras perguntas (login, tema) que o usuário precisa ver de qualquer jeito.
fn trust_claude(file: &Path, dir: &Path) -> std::io::Result<()> {
    let _guard = WRITES.lock().unwrap_or_else(|e| e.into_inner());
    let Some(mut root) = read_json(file)? else { return Ok(()) };
    let Some(obj) = root.as_object_mut() else { return Ok(()) };
    let projects = obj.entry("projects").or_insert_with(|| Value::Object(Map::new()));
    let Some(projects) = projects.as_object_mut() else { return Ok(()) };
    if claude_trusted(projects, dir) {
        return Ok(());
    }
    let entry = projects.entry(claude_key(dir)).or_insert_with(|| Value::Object(Map::new()));
    if let Some(entry) = entry.as_object_mut() {
        entry.insert("hasTrustDialogAccepted".into(), Value::Bool(true));
    }
    write_json(file, &root)?;
    log::info!("confiança da pasta registrada para o Claude: {}", dir.display());
    Ok(())
}

/// O caminho como o `agy` o vê na pasta atual: no Windows, com `\` (o processo recebe a pasta normalizada).
fn agy_key(dir: &Path) -> String {
    let s = dir.to_string_lossy();
    let s = if cfg!(windows) { s.replace('/', "\\") } else { s.into_owned() };
    let trimmed = s.trim_end_matches(['\\', '/']);
    if trimmed.is_empty() {
        s
    } else {
        trimmed.to_owned()
    }
}

/// Acrescenta `dir` aos `trustedWorkspaces` do Antigravity. Sem a pasta `antigravity-cli` (o `agy` nunca abriu), não cria
/// nada; sem o arquivo, cria só com a lista.
fn trust_agy(file: &Path, dir: &Path) -> std::io::Result<()> {
    if !file.parent().is_some_and(Path::is_dir) {
        return Ok(());
    }
    let _guard = WRITES.lock().unwrap_or_else(|e| e.into_inner());
    let mut root = read_json(file)?.unwrap_or_else(|| Value::Object(Map::new()));
    let Some(obj) = root.as_object_mut() else { return Ok(()) };
    let list = obj.entry("trustedWorkspaces").or_insert_with(|| Value::Array(Vec::new()));
    let Some(list) = list.as_array_mut() else { return Ok(()) };
    let key = agy_key(dir);
    if list.iter().any(|v| v.as_str() == Some(key.as_str())) {
        return Ok(());
    }
    list.push(Value::String(key));
    write_json(file, &root)?;
    log::info!("confiança da pasta registrada para o Antigravity: {}", dir.display());
    Ok(())
}

fn forget_agy(file: &Path, dir: &Path) -> std::io::Result<()> {
    let _guard = WRITES.lock().unwrap_or_else(|e| e.into_inner());
    let Some(mut root) = read_json(file)? else { return Ok(()) };
    let Some(list) = root.get_mut("trustedWorkspaces").and_then(Value::as_array_mut) else { return Ok(()) };
    let key = agy_key(dir);
    let before = list.len();
    list.retain(|v| v.as_str() != Some(key.as_str()));
    if list.len() != before {
        write_json(file, &root)?;
    }
    Ok(())
}

/// `-c projects={'<pasta>'={trust_level='trusted'}}`. O valor é TOML com aspas simples, que sobrevivem ao shim `.cmd`
/// (as duplas o `cmd_safe_arg` tira). Uma pasta com `'` não cabe numa string literal do TOML e fica de fora. Para o
/// Codex a confiança vale pela raiz do repositório; a worktree e o projeto vão juntos e qualquer um basta.
fn codex_args(dirs: &[&Path]) -> Vec<String> {
    let entries: Vec<String> = dirs
        .iter()
        .map(|d| d.to_string_lossy().into_owned())
        .filter(|s| !s.contains(['\'', '\n', '\r']))
        .map(|s| format!("'{s}'={{trust_level='trusted'}}"))
        .collect();
    if entries.is_empty() {
        return Vec::new();
    }
    vec!["-c".into(), format!("projects={{{}}}", entries.join(","))]
}

/// `None` quando o arquivo não existe. Um arquivo que não é JSON é erro: não se regrava o que não se entendeu.
fn read_json(file: &Path) -> std::io::Result<Option<Value>> {
    let bytes = match std::fs::read(file) {
        Ok(b) => b,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(e),
    };
    let text = String::from_utf8_lossy(bytes.strip_prefix(b"\xEF\xBB\xBF").unwrap_or(&bytes)).into_owned();
    serde_json::from_str(&text).map(Some).map_err(std::io::Error::other)
}

/// Grava ao lado e troca: o CLI nunca lê o arquivo pela metade.
fn write_json(file: &Path, value: &Value) -> std::io::Result<()> {
    let text = serde_json::to_string_pretty(value).map_err(std::io::Error::other)?;
    let tmp = file.with_extension(format!("agentdeck-{}.tmp", std::process::id()));
    std::fs::write(&tmp, text)?;
    std::fs::rename(&tmp, file).inspect_err(|_| {
        let _ = std::fs::remove_file(&tmp);
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn json(file: &Path) -> Value {
        serde_json::from_str(&std::fs::read_to_string(file).unwrap()).unwrap()
    }

    #[test]
    fn the_home_folder_and_a_drive_root_are_never_trusted() {
        let home = Path::new("/home/voce");
        assert!(!may_trust(home, home));
        assert!(!may_trust(Path::new("/home/voce/"), home));
        assert!(!may_trust(Path::new("/"), home));
        assert!(may_trust(Path::new("/home/voce/projeto"), home));
        assert!(prepare("codex", home, home, home).is_empty());
    }

    #[test]
    fn codex_gets_the_trust_on_the_command_line() {
        let args = codex_args(&[Path::new("/p/projeto/.agentdeck/worktrees/t1"), Path::new("/p/projeto")]);
        assert_eq!(
            args,
            ["-c", "projects={'/p/projeto/.agentdeck/worktrees/t1'={trust_level='trusted'},'/p/projeto'={trust_level='trusted'}}"]
        );
        assert!(codex_args(&[Path::new("/p/o'brien")]).is_empty(), "aspas simples não cabem na string literal");
    }

    /// A aba do projeto (pasta = projeto) leva uma chave só: chave repetida não é TOML e o Codex nem abre.
    #[test]
    fn codex_never_repeats_a_folder() {
        let home = Path::new("/home/voce");
        let p = Path::new("/p/projeto");
        assert_eq!(prepare("codex", p, Path::new("/p/projeto/"), home), ["-c", "projects={'/p/projeto'={trust_level='trusted'}}"]);
    }

    #[test]
    fn claude_marks_a_new_folder_and_respects_a_trusted_parent() {
        let tmp = tempfile::tempdir().unwrap();
        let file = tmp.path().join(".claude.json");
        // Sem o arquivo, nada é criado.
        trust_claude(&file, Path::new("/p/novo")).unwrap();
        assert!(!file.exists());

        std::fs::write(&file, r#"{"numStartups": 3, "projects": {"/p/projeto": {"hasTrustDialogAccepted": true, "x": 1}}}"#).unwrap();
        // A worktree dentro do projeto herda: o arquivo não muda.
        let before = std::fs::read_to_string(&file).unwrap();
        trust_claude(&file, Path::new("/p/projeto/.agentdeck/worktrees/t1")).unwrap();
        assert_eq!(std::fs::read_to_string(&file).unwrap(), before);

        trust_claude(&file, Path::new("/p/novo")).unwrap();
        let v = json(&file);
        assert_eq!(v["projects"]["/p/novo"]["hasTrustDialogAccepted"], true);
        assert_eq!(v["projects"]["/p/projeto"]["x"], 1, "o resto fica como estava");
        assert_eq!(v["numStartups"], 3);
    }

    #[test]
    fn claude_turns_an_existing_refusal_into_trust_and_keeps_the_entry() {
        let tmp = tempfile::tempdir().unwrap();
        let file = tmp.path().join(".claude.json");
        std::fs::write(&file, r#"{"projects": {"C:/p/novo": {"hasTrustDialogAccepted": false, "allowedTools": ["a"]}}}"#).unwrap();
        trust_claude(&file, Path::new(r"C:\p\novo")).unwrap();
        let v = json(&file);
        assert_eq!(v["projects"]["C:/p/novo"]["hasTrustDialogAccepted"], true);
        assert_eq!(v["projects"]["C:/p/novo"]["allowedTools"][0], "a");
    }

    #[test]
    fn antigravity_lists_the_exact_folder_and_forgets_it() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("antigravity-cli");
        let file = dir.join("settings.json");
        let wt = tmp.path().join("projeto").join("wt");
        // Sem a pasta do agy, nada é criado.
        trust_agy(&file, &wt).unwrap();
        assert!(!file.exists());

        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(&file, "\u{feff}{\"model\": \"Flash\", \"trustedWorkspaces\": [\"/outra\"]}").unwrap();
        trust_agy(&file, &wt).unwrap();
        trust_agy(&file, &wt).unwrap();
        let v = json(&file);
        let key = agy_key(&wt);
        assert_eq!(v["trustedWorkspaces"], serde_json::json!(["/outra", key]), "uma vez só");
        assert_eq!(v["model"], "Flash");

        forget_agy(&file, &wt).unwrap();
        assert_eq!(json(&file)["trustedWorkspaces"], serde_json::json!(["/outra"]));
    }

    #[test]
    fn a_file_that_is_not_json_is_left_alone() {
        let tmp = tempfile::tempdir().unwrap();
        let file = tmp.path().join("settings.json");
        std::fs::write(&file, "{ isto não é json").unwrap();
        assert!(trust_agy(&file, Path::new("/p/x")).is_err());
        assert_eq!(std::fs::read_to_string(&file).unwrap(), "{ isto não é json");
    }

    #[cfg(windows)]
    #[test]
    fn on_windows_the_agy_key_uses_backslashes() {
        assert_eq!(agy_key(Path::new("C:/Users/voce/projeto/")), r"C:\Users\voce\projeto");
        assert!(claude_trusted(
            serde_json::json!({"c:/users/voce/projeto": {"hasTrustDialogAccepted": true}}).as_object().unwrap(),
            Path::new(r"C:\Users\voce\projeto\.agentdeck\worktrees\t3")
        ));
    }
}
