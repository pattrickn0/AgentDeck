use std::path::Path;

use super::text::cap_text;
use super::{Task, MAX_STAT};
use crate::git::{run, GitError, Repo, COMMIT_TIMEOUT, WRITE_TIMEOUT};
use crate::util;

/// Caminhos sujos de um `git status --porcelain=v1` (o destino, nas renomeações).
pub fn dirty_paths(porcelain: &str) -> Vec<String> {
    porcelain
        .lines()
        .filter(|l| l.len() > 3)
        .map(|l| {
            let path = &l[3..];
            let path = path.rsplit_once(" -> ").map_or(path, |(_, new)| new);
            path.trim_matches('"').to_owned()
        })
        .collect()
}

/// A pasta da worktree está dentro de `<raiz>/.agentdeck/worktrees` (e não é a própria pasta-pai).
pub(super) fn safe_worktree(worktree: &Path, allowed: &Path) -> bool {
    let wt = util::absolutize(worktree);
    util::path_within(&wt, allowed) && !util::same_path(&wt, allowed)
}

/// Branch de tarefa (`tarefa/…`) com nome aceitável.
pub(super) fn valid_task_branch(branch: &str) -> bool {
    branch.starts_with("tarefa/") && crate::git::valid_branch_name(branch)
}

/// Saída padrão de um comando do git no repositório principal (limite das operações locais).
pub(super) fn git_out(repo: &Repo, args: &[&str]) -> Result<String, GitError> {
    run(repo.cmd(args), None, WRITE_TIMEOUT).map(|o| String::from_utf8_lossy(&o.stdout).into_owned())
}

/// Arquivos que a branch trouxe em relação à base (`git diff --name-only base...branch`); vazio se a branch
/// não existe mais ou o git falha.
pub(crate) fn changed_files(repo: &Repo, base: &str, branch: &str) -> Vec<String> {
    if !valid_task_branch(branch) || !crate::git::valid_branch_name(base) {
        return Vec::new();
    }
    let range = format!("{base}...{branch}");
    repo.read(&["diff", "--name-only", "--no-ext-diff", &range, "--"])
        .map(|o| String::from_utf8_lossy(&o.stdout).lines().map(str::trim).filter(|l| !l.is_empty()).map(str::to_owned).collect())
        .unwrap_or_default()
}

/// `git diff --stat base...branch`, cortado; "sem alterações" quando vazio.
pub(super) fn diff_stat(repo: &Repo, base: &str, branch: &str) -> Result<String, String> {
    let out = git_out(repo, &["diff", "--stat", &format!("{base}...{branch}"), "--"]).map_err(|e| e.to_string())?;
    let out = out.trim();
    if out.is_empty() {
        return Ok("(nenhuma alteração em relação à base)".into());
    }
    Ok(cap_text(out, MAX_STAT))
}

/// Caminhos com alterações (inclusive novos, não rastreados) na worktree. `None` se a pasta não existe
/// ou o git não respondeu.
pub(super) fn worktree_dirty(repo: &Repo, worktree: &Path) -> Option<Vec<String>> {
    if !worktree.is_dir() {
        return None;
    }
    let cmd = crate::git::base_command(&repo.git, worktree, &["status", "--porcelain=v1"]);
    let out = run(cmd, None, WRITE_TIMEOUT).ok()?;
    Some(dirty_paths(&String::from_utf8_lossy(&out.stdout)))
}

/// Como [`worktree_dirty`], mas pedindo ao git que não tome o `index.lock` da worktree (`--no-optional-locks`):
/// serve para olhar a pasta de outro agente sem atrapalhar o `git add`/`commit` dele.
pub(crate) fn worktree_dirty_unlocked(repo: &Repo, worktree: &Path) -> Option<Vec<String>> {
    if !worktree.is_dir() {
        return None;
    }
    let cmd = crate::git::base_command(&repo.git, worktree, &["--no-optional-locks", "status", "--porcelain=v1"]);
    let out = run(cmd, None, WRITE_TIMEOUT).ok()?;
    Some(dirty_paths(&String::from_utf8_lossy(&out.stdout)))
}

/// Une os arquivos da branch com os sem commit: normalizados (`/`), sem repetição, em ordem alfabética e sem o
/// `AGENTS.md`, que o app reescreve e não é trabalho do agente.
pub(crate) fn union_files(committed: Vec<String>, dirty: Option<Vec<String>>) -> Vec<String> {
    let mut files: Vec<String> = committed
        .into_iter()
        .chain(dirty.unwrap_or_default())
        .map(|f| crate::learnings::normalize_path(&f))
        .filter(|f| !f.is_empty() && f != "AGENTS.md")
        .collect();
    files.sort();
    files.dedup();
    files
}

/// O hash curto (7 caracteres) de um hash completo, para mensagens e eventos.
pub(super) fn short_hash(hash: &str) -> &str {
    hash.get(..7).unwrap_or(hash)
}

/// A branch existe neste repositório.
pub(super) fn branch_exists(repo: &Repo, branch: &str) -> bool {
    repo.read(&["rev-parse", "--verify", "-q", &format!("refs/heads/{branch}")]).is_ok()
}

/// Põe a branch da tarefa no mesmo commit da base já mesclada, dentro da worktree (`git merge --ff-only`:
/// a branch é ancestral do commit de mesclagem, então avança sem criar commit). Erro = a mensagem do git.
pub(super) fn align_with_base(repo: &Repo, task: &Task) -> Result<(), String> {
    if !task.worktree.is_dir() {
        return Err(format!("a pasta {} não existe", task.worktree.display()));
    }
    let cmd = crate::git::base_command(&repo.git, &task.worktree, &["merge", "--ff-only", &task.base]);
    run(cmd, None, COMMIT_TIMEOUT).map(|_| ()).map_err(|e| e.to_string())
}

/// Apaga a branch (se existir). `force` = `-D`.
pub(super) fn delete_branch(repo: &Repo, branch: &str, force: bool) -> Result<(), String> {
    if repo.read(&["rev-parse", "--verify", "-q", &format!("refs/heads/{branch}")]).is_err() {
        return Ok(());
    }
    repo.write(&["branch", if force { "-D" } else { "-d" }, branch], None).map(|_| ()).map_err(|e| e.to_string())
}

/// Arquivos novos do usuário (não rastreados, no repositório principal) que a branch da tarefa também cria.
pub(super) fn untracked_clash(repo: &Repo, base: &str, branch: &str) -> Vec<String> {
    let range = format!("{base}...{branch}");
    let added = git_out(repo, &["diff", "--name-only", "-z", "--no-renames", "--diff-filter=A", &range, "--"]).unwrap_or_default();
    let untracked = git_out(repo, &["ls-files", "--others", "--exclude-standard", "-z"]).unwrap_or_default();
    let mine: std::collections::HashSet<&str> = untracked.split('\0').filter(|p| !p.is_empty()).collect();
    added.split('\0').filter(|p| mine.contains(p)).map(str::to_owned).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn porcelain_paths() {
        let text = " M src/a.rs\n?? novo arquivo.txt\nR  velho.rs -> novo.rs\nA  \"com aspas.txt\"\n";
        assert_eq!(dirty_paths(text), ["src/a.rs", "novo arquivo.txt", "novo.rs", "com aspas.txt"]);
        assert!(dirty_paths("").is_empty());
    }

    #[test]
    fn the_situation_files_join_the_branch_and_the_uncommitted_work() {
        let got = union_files(
            vec!["b.rs".into(), "a.rs".into(), "AGENTS.md".into()],
            Some(vec!["c\\d.rs".into(), "a.rs".into(), "AGENTS.md".into()]),
        );
        assert_eq!(got, ["a.rs", "b.rs", "c/d.rs"]);
        // A worktree que o git não respondeu não apaga o que a branch já tem.
        assert_eq!(union_files(vec!["a.rs".into()], None), ["a.rs"]);
        assert!(union_files(Vec::new(), Some(Vec::new())).is_empty());
    }

    #[test]
    fn only_the_expected_folder_is_ever_touched() {
        // Como no código real, a pasta permitida já vem absoluta (no Windows, `/repo` ganha a unidade).
        let allowed = &util::absolutize(Path::new("/repo/.agentdeck/worktrees"));
        assert!(safe_worktree(Path::new("/repo/.agentdeck/worktrees/t1"), allowed));
        assert!(!safe_worktree(Path::new("/repo/.agentdeck/worktrees"), allowed));
        assert!(!safe_worktree(Path::new("/repo/src"), allowed));
        assert!(!safe_worktree(Path::new("/repo/.agentdeck/worktrees/../../src"), allowed));
        assert!(valid_task_branch("tarefa/t1-x") && !valid_task_branch("main") && !valid_task_branch("tarefa/../x"));
    }
}
