use super::git_ops::git_out;
use super::text::short_list;
use super::Task;
use crate::git::{run, Repo, COMMIT_TIMEOUT};

/// O stash que o app criou para guardar as alterações do repositório principal (o hash identifica a entrada:
/// a pilha do stash é compartilhada com as outras worktrees, `stash@{0}` pode não ser a nossa na hora de devolver).
pub(super) struct Stash {
    sha: String,
    message: String,
}

/// O topo do stash (`refs/stash`), se há.
fn stash_top(repo: &Repo) -> Option<String> {
    repo.read(&["rev-parse", "-q", "--verify", "refs/stash"]).ok().map(|o| String::from_utf8_lossy(&o.stdout).trim().to_owned())
}

/// Guarda no stash as alterações sem commit dos arquivos rastreados do repositório principal, menos o `AGENTS.md`
/// (o app trata dele à parte). Não leva arquivos não rastreados. `Ok(None)` = não havia o que guardar.
pub(super) fn stash_push(repo: &Repo, task: &Task) -> Result<Option<Stash>, String> {
    let listed = git_out(repo, &["diff", "--name-only", "-z", "--no-renames", "HEAD", "--"]).map_err(|e| e.to_string())?;
    let mut stdin = Vec::new();
    for path in listed.split('\0').filter(|p| !p.is_empty() && *p != "AGENTS.md") {
        stdin.extend_from_slice(path.as_bytes());
        stdin.push(0);
    }
    if stdin.is_empty() {
        return Ok(None);
    }
    let before = stash_top(repo);
    let message = format!("agentdeck: guardado antes de mesclar a tarefa {}", task.id);
    let args = ["stash", "push", "-m", message.as_str(), "--pathspec-from-file=-", "--pathspec-file-nul"];
    run(repo.cmd(&args), Some(stdin), COMMIT_TIMEOUT).map_err(|e| e.to_string())?;
    let after = stash_top(repo);
    Ok(after.filter(|a| Some(a) != before.as_ref()).map(|sha| Stash { sha, message }))
}

/// A posição atual (`stash@{n}`) da entrada do stash, se ela ainda existe.
fn stash_ref(repo: &Repo, sha: &str) -> Option<String> {
    let list = git_out(repo, &["stash", "list", "--format=%H %gd"]).ok()?;
    list.lines().find_map(|l| l.split_once(' ').filter(|(h, _)| *h == sha).map(|(_, r)| r.trim().to_owned()))
}

/// Os arquivos em que devolver o stash por cima de `HEAD` daria conflito, sem tocar na pasta
/// (`git merge-tree --write-tree`, git 2.38+). `None` = não deu para conferir (git antigo, erro).
fn stash_pop_conflicts(repo: &Repo, sha: &str) -> Option<Vec<String>> {
    let base = format!("--merge-base={sha}^1");
    let out = repo.cmd(&["merge-tree", "--write-tree", "--name-only", "--no-messages", &base, "HEAD", sha]).output().ok()?;
    match out.status.code() {
        Some(0) => Some(Vec::new()),
        Some(1) => {
            let text = String::from_utf8_lossy(&out.stdout);
            // A primeira linha é o tree; os arquivos em conflito vêm depois, até uma linha vazia.
            Some(text.lines().skip(1).take_while(|l| !l.is_empty()).map(str::to_owned).collect())
        }
        _ => None,
    }
}

/// Devolve o stash à pasta principal depois da mesclagem. Só faz o `stash pop` quando a conferência diz que não há
/// conflito; na dúvida, ou em conflito, deixa no stash (intacto) e o aviso explica como recuperar. Devolve o aviso
/// e se o stash continua guardado.
pub(super) fn stash_pop(repo: &Repo, stash: &Stash) -> (String, bool) {
    let recover = "Para recuperá-las: abra um terminal na pasta do projeto, rode git stash list para achar a entrada e git stash pop para devolvê-la (resolvendo o que chocar).";
    let kept = |why: String| {
        (
            format!(
                "Suas alterações sem commit continuam guardadas no stash do git (\"{}\"): {why} Nada se perdeu. {recover}",
                stash.message
            ),
            true,
        )
    };
    if stash_ref(repo, &stash.sha).is_none() {
        return (
            "O stash com as suas alterações não está mais na lista do git (alguém o usou ou apagou?); confira com git stash list.".into(),
            true,
        );
    }
    match stash_pop_conflicts(repo, &stash.sha) {
        Some(files) if files.is_empty() => {}
        Some(files) => return kept(format!("ao devolvê-las elas se chocariam com o que a tarefa trouxe em {}.", short_list(&files))),
        None => return kept("não consegui conferir se elas se chocam com o que a tarefa trouxe.".into()),
    }
    // Aplica pelo sha, não por `stash@{n}`: a pilha é de todas as worktrees, e um `stash push` de outro entre a conferência
    // e a devolução mudaria a posição (o `pop` devolveria e apagaria a entrada de outra pessoa). A entrada só sai depois,
    // procurada de novo pelo sha.
    match repo.write(&["stash", "apply", &stash.sha], None) {
        Ok(_) => {
            if let Some(reference) = stash_ref(repo, &stash.sha) {
                if let Err(e) = repo.write(&["stash", "drop", &reference], None) {
                    log::warn!("as alterações voltaram, mas a entrada do stash ficou ({e})");
                }
            }
            ("Suas alterações sem commit foram guardadas durante a mesclagem e já voltaram para a pasta.".to_owned(), false)
        }
        Err(e) => {
            let unmerged = git_out(repo, &["diff", "--name-only", "--diff-filter=U"]).unwrap_or_default();
            let mess = if unmerged.trim().is_empty() { "" } else { " A pasta ficou com arquivos em conflito: confira com git status." };
            kept(format!("o git não conseguiu devolvê-las ({e}).{mess}"))
        }
    }
}
