//! pt-BR translations for the area: Tasks in the core: crates/core/src/tasks/** (merge refusals, notices and errors shown in the UI).
//!
//! Entries are `(English, Portuguese)`; the English text is the key used in `t!("...")`. See the `i18n` module docs for
//! the conventions and the coverage test.

pub const ENTRIES: &[(&str, &str)] = &[
    // Status labels.
    ("draft", "rascunho"),
    ("queued", "na fila"),
    ("in progress", "em andamento"),
    ("in review", "em revisão"),
    ("completed", "concluída"),
    ("awaiting validation", "aguardando validação"),
    ("discarded", "descartada"),
    ("errored", "com erro"),
    // Merge block explanations (the notice on the screen).
    (
        "You have changes of your own, not yet committed, in the project's main folder ({files}). Git does not merge over them. You can set them aside for a moment: AgentDeck merges the task and puts each change back in place.",
        "Há alterações suas, ainda sem commit, na pasta principal do projeto ({files}). O git não mescla por cima delas. Você pode guardá-las por um instante: o AgentDeck mescla a tarefa e devolve cada alteração ao lugar.",
    ),
    (
        "The agent left uncommitted work in the task's folder ({files}). Only what was committed enters the merge. Ask it to commit and deliver again.",
        "O agente deixou trabalho sem commit na pasta da tarefa ({files}). Só o que foi commitado entra na mesclagem. Peça a ele para fazer o commit e entregar de novo.",
    ),
    (
        "The task and the branch \"{base}\" changed the same lines ({files}). Nothing was merged and the task is still in review. Ask the agent to combine the two versions and deliver again.",
        "A tarefa e a branch \"{base}\" mudaram as mesmas linhas ({files}). Nada foi mesclado e a tarefa continua em revisão. Peça ao agente para juntar as duas versões e entregar de novo.",
    ),
    ("unidentified files", "arquivos não identificados"),
    ("not identified", "não identificados"),
    // Text helpers.
    ("the task title cannot be empty", "o título da tarefa não pode ficar vazio"),
    ("[text cut at {max} characters]", "[texto cortado em {max} caracteres]"),
    ("{list} and {count} more", "{list} e mais {count}"),
    // Creating a task.
    ("invalid agent \"{agent}\": give only the model (claude, antigravity…)", "agente inválido \"{agent}\": informe só o modelo (claude, antigravity…)"),
    (
        "unknown or not installed agent \"{agent}\". Available: {available}",
        "agente desconhecido ou não instalado \"{agent}\". Disponíveis: {available}",
    ),
    ("invalid model: \"{model}\"", "modelo inválido: \"{model}\""),
    ("the agent {kind} does not accept choosing a model", "o agente {kind} não aceita escolher modelo"),
    ("effort not applicable to {kind}: {why}", "esforço não aplicável a {kind}: {why}"),
    (
        "the repository has no commits yet; make the first commit before creating tasks",
        "o repositório ainda não tem nenhum commit; faça o primeiro commit antes de criar tarefas",
    ),
    (
        "the repository has no branch (detached HEAD); give the task's base",
        "o repositório está sem branch (HEAD solto); informe a base da tarefa",
    ),
    ("the base \"{base}\" is not a local branch of this repository", "a base \"{base}\" não é uma branch local deste repositório"),
    ("agent \"{kind}\" cannot have a task instance", "agente \"{kind}\" não pode ter instância de tarefa"),
    ("created by {creator}", "criada por {creator}"),
    ("created", "criada"),
    (
        "agent chosen automatically by capacity and quota: {reason}",
        "agente escolhido automaticamente pela capacidade e pela cota: {reason}",
    ),
    ("capacity warning: {warning}", "aviso de capacidade: {warning}"),
    ("agent chosen automatically: {kind} (no quota reading)", "agente escolhido automaticamente: {kind} (sem leitura de cota)"),
    ("quota warning: {note}", "aviso de cota: {note}"),
    // Starting a task and the queue.
    ("queue: could not start: {error}", "fila: não consegui começar: {error}"),
    (
        "task {id} is {status}: only a draft (or a task that failed to start) can be started",
        "a tarefa {id} está {status}: só se inicia um rascunho (ou uma tarefa que falhou ao iniciar)",
    ),
    (
        "tasks run in the agents' tabs: turn on \"Open agent tabs\" in Settings (in headless mode there is no tab or worktree per agent)",
        "as tarefas rodam nas abas dos agentes: ligue \"Abrir abas dos agentes\" nas Configurações (no modo headless não há aba nem worktree por agente)",
    ),
    (
        "the scope of task {id} overlaps that of {holder}, which is in progress: try again when it finishes",
        "o escopo da tarefa {id} se sobrepõe ao da {holder}, que está em andamento: tente de novo quando ela terminar",
    ),
    (
        "queued: the scope overlaps that of {holder}; it starts when that one finishes",
        "na fila: o escopo se sobrepõe ao da {holder}; começa quando ela terminar",
    ),
    ("task {id} vanished", "tarefa {id} sumiu"),
    ("started: worktree {worktree} on branch {branch}", "iniciada: worktree {worktree} na branch {branch}"),
    ("brief delivered to the agent", "brief entregue ao agente"),
    ("Task {id} started: {title} ({instance})", "Tarefa {id} iniciada: {title} ({instance})"),
    (
        "data of task {id} outside what was expected (folder {folder} / branch {branch}); I touched nothing",
        "dados da tarefa {id} fora do esperado (pasta {folder} / branch {branch}); não mexi em nada",
    ),
    ("the base \"{base}\" no longer exists in this repository", "a base \"{base}\" não existe mais neste repositório"),
    (
        "the branch {branch} already exists in this repository; remove it or discard the task",
        "a branch {branch} já existe neste repositório; remova-a ou descarte a tarefa",
    ),
    ("the folder {folder} already exists; remove it or discard the task", "a pasta {folder} já existe; remova-a ou descarte a tarefa"),
    ("could not create the worktree: {error}", "não consegui criar a worktree: {error}"),
    ("could not write {file}: {error}", "não consegui escrever {file}: {error}"),
    // The report.
    (
        "task {id} is {status}: only a task in progress receives the report",
        "a tarefa {id} está {status}: só uma tarefa em andamento recebe o relatório",
    ),
    (
        "task {id} was already merged and awaits the user's validation; there are no new commits to deliver. If something is still missing, the user decides (Reject)",
        "a tarefa {id} já foi mesclada e espera a validação do usuário; não há commits novos para entregar. Se algo ainda falta, quem decide é o usuário (Reprovar)",
    ),
    (
        "the report is empty: say what was done, the files, the tests and what is missing",
        "o relatório está vazio: diga o que foi feito, os arquivos, os testes e o que falta",
    ),
    (
        "warning: there are uncommitted changes in the worktree ({files}); the agent must commit before the merge",
        "atenção: há alterações sem commit na worktree ({files}); o agente precisa fazer o commit antes da mesclagem",
    ),
    ("could not get the diff: {error}", "não consegui o diff: {error}"),
    ("touched files outside the scope: {files}", "mexeu fora do escopo: {files}"),
    (
        "the agent delivered more work after the merge; in review",
        "o agente entregou mais trabalho depois da mesclagem; em revisão",
    ),
    ("report received; in review", "relatório recebido; em revisão"),
    ("Task {id} ready for review: {title}", "Tarefa {id} pronta para revisão: {title}"),
    // The diff.
    (
        "data of task {id} outside what was expected; I did not read the diff",
        "dados da tarefa {id} fora do esperado; não li o diff",
    ),
    (
        "the merge commit ({hash}) no longer exists in the repository (was the history rewritten?): there is no diff to show",
        "o commit da mesclagem ({hash}) não existe mais no repositório (o histórico foi reescrito?): não há diff para mostrar",
    ),
    ("the branch {branch} no longer exists: there is no diff to show", "a branch {branch} não existe mais: não há diff para mostrar"),
    // Returning a task.
    (
        "task {id} is {status}: only a task in review can be returned",
        "a tarefa {id} está {status}: só se devolve uma tarefa em revisão",
    ),
    ("the feedback is empty: say what needs to change", "o feedback está vazio: diga o que precisa mudar"),
    (
        "tasks run in the agents' tabs: turn on \"Open agent tabs\" in Settings",
        "as tarefas rodam nas abas dos agentes: ligue \"Abrir abas dos agentes\" nas Configurações",
    ),
    ("returned to the agent with feedback", "devolvida ao agente com feedback"),
    ("feedback delivered to the agent", "feedback entregue ao agente"),
    // Merging.
    (
        "task {id} is {status}: only a task in review can be merged",
        "a tarefa {id} está {status}: só se mescla uma tarefa em revisão",
    ),
    ("merge refused: {reason}", "mesclagem recusada: {reason}"),
    ("data of task {id} outside what was expected; I merged nothing", "dados da tarefa {id} fora do esperado; não mesclei nada"),
    (
        "the main repository is on the branch \"{now}\", not \"{base}\": switch to \"{base}\" and try again",
        "o repositório principal está na branch \"{now}\", não em \"{base}\": troque para \"{base}\" e tente de novo",
    ),
    (
        "the main repository has a detached HEAD: switch to \"{base}\" and try again",
        "o repositório principal está com o HEAD solto: troque para \"{base}\" e tente de novo",
    ),
    (
        "the main repository is in the middle of another operation (merge, rebase…): finish or abort it first",
        "o repositório principal está no meio de outra operação (mesclagem, rebase…): termine ou aborte antes",
    ),
    (
        "the main repository has uncommitted changes ({files}): commit or stash them and try again",
        "o repositório principal tem alterações sem commit ({files}): faça o commit ou guarde-as e tente de novo",
    ),
    (
        "the task's worktree has uncommitted work ({files}): return the task asking for the commit, or discard it",
        "a worktree da tarefa tem trabalho sem commit ({files}): devolva a tarefa pedindo o commit, ou descarte-a",
    ),
    ("the branch {branch} no longer exists: discard the task", "a branch {branch} não existe mais: descarte a tarefa"),
    (
        "no new commits since the last merge ({hash}); awaiting validation again",
        "sem commits novos desde a última mesclagem ({hash}); aguardando validação de novo",
    ),
    (
        "Task {id} has no new commits, awaiting validation again: {title}",
        "Tarefa {id} sem commits novos, aguardando validação de novo: {title}",
    ),
    (
        "the branch {branch} has no new commits relative to \"{base}\": there is nothing to merge (discard the task)",
        "a branch {branch} não tem commits novos em relação a \"{base}\": não há o que mesclar (descarte a tarefa)",
    ),
    (
        "the main repository has new files of yours, uncommitted, that the task also creates ({files}): move them to another folder (or delete them) and try again",
        "o repositório principal tem arquivos novos seus, sem commit, que a tarefa também cria ({files}): mova-os para outra pasta (ou apague-os) e tente de novo",
    ),
    (
        "could not stash the main repository's changes ({error}); nothing was changed",
        "não consegui guardar as alterações do repositório principal ({error}); nada foi mudado",
    ),
    (
        "stashed {count} uncommitted file(s) of the main repository",
        "guardei {count} arquivo(s) sem commit do repositório principal no stash",
    ),
    ("stashed changes: still in the stash", "alterações guardadas: continuam no stash"),
    ("stashed changes: given back", "alterações guardadas: devolvidas"),
    (
        "the main repository's AGENTS.md has uncommitted changes of yours, and the branch {branch} also touches it: commit or stash them and try again",
        "o AGENTS.md do repositório principal tem alterações suas sem commit, e a branch {branch} também mexe nele: faça o commit ou guarde-as e tente de novo",
    ),
    (
        "could not prepare the main repository's AGENTS.md for the merge: {error}",
        "não consegui preparar o AGENTS.md do repositório principal para a mesclagem: {error}",
    ),
    (
        "warning: could not rewrite the app's block in AGENTS.md: {error}",
        "aviso: não consegui regravar o bloco do app no AGENTS.md: {error}",
    ),
    (
        "conflict while merging {branch} into \"{base}\" (files: {files}); the merge was undone and the task is still in review. Return it to the agent asking it to update the branch with the base, or resolve it by hand",
        "conflito ao mesclar {branch} em \"{base}\" (arquivos: {files}); a mesclagem foi desfeita e a tarefa continua em revisão. Devolva-a ao agente pedindo para atualizar a branch com a base, ou resolva à mão",
    ),
    ("conflict in the merge ({files}); undone", "conflito na mesclagem ({files}); desfeita"),
    (
        "conflict while merging and could not undo it ({abort}): run git merge --abort in {folder}; your uncommitted changes are still stashed (git stash list)",
        "conflito ao mesclar e não consegui desfazer ({abort}): rode git merge --abort em {folder}; as suas alterações sem commit continuam guardadas (git stash list)",
    ),
    (
        "conflict while merging and could not undo it ({abort}): run git merge --abort in {folder}",
        "conflito ao mesclar e não consegui desfazer ({abort}): rode git merge --abort em {folder}",
    ),
    ("the merge failed: {error}", "a mesclagem falhou: {error}"),
    ("merged into {base} ({hash}); awaiting validation", "mesclada em {base} ({hash}); aguardando validação"),
    (
        "warning: could not align the branch {branch} with {base}: {error}",
        "aviso: não consegui alinhar a branch {branch} com {base}: {error}",
    ),
    (
        "Task {id} merged into {base}, awaiting validation: {title}",
        "Tarefa {id} mesclada em {base}, aguardando validação: {title}",
    ),
    // Validating, rejecting, discarding.
    (
        "task {id} is {status}: only a task awaiting validation can be validated",
        "a tarefa {id} está {status}: só se valida uma tarefa aguardando validação",
    ),
    ("data of task {id} outside what was expected; I removed nothing", "dados da tarefa {id} fora do esperado; não removi nada"),
    ("validation refused: {reason}", "validação recusada: {reason}"),
    (
        "the task's worktree has uncommitted work ({files}): reject the task asking for the commit, or discard it",
        "a worktree da tarefa tem trabalho sem commit ({files}): reprove a tarefa pedindo o commit, ou descarte-a",
    ),
    (
        "the branch {branch} has commits that were not merged into \"{base}\" yet: reject the task so the agent delivers them, or discard it",
        "a branch {branch} tem commits que ainda não foram mesclados em \"{base}\": reprove a tarefa para o agente entregá-los, ou descarte-a",
    ),
    (
        "the base {base} no longer exists; I did not check whether the branch had commits outside it",
        "a base {base} não existe mais; não conferi se a branch tinha commits fora dela",
    ),
    ("validation incomplete: {why}", "validação incompleta: {why}"),
    (
        "could not delete the worktree folder ({why}): close whatever is using it and validate again",
        "não consegui apagar a pasta da worktree ({why}): feche o que a usa e valide de novo",
    ),
    ("the branch {branch} remained: {error}", "a branch {branch} ficou: {error}"),
    ("validated; completed; worktree and branch removed", "validada; concluída; worktree e branch removidas"),
    ("warning: {warning}", "aviso: {warning}"),
    ("Task {id} completed: {title}", "Tarefa {id} concluída: {title}"),
    (
        "task {id} is {status}: only a task awaiting validation can be rejected",
        "a tarefa {id} está {status}: só se reprova uma tarefa aguardando validação",
    ),
    (
        "the reason is empty: say what did not work in the test",
        "o motivo está vazio: diga o que não funcionou no teste",
    ),
    ("rejected in validation; returned to the agent with the reason", "reprovada na validação; devolvida ao agente com o motivo"),
    (
        "warning: the scope overlaps that of {other}, which is in progress; the next merge may conflict",
        "aviso: o escopo se sobrepõe ao da {other}, que está em andamento; a próxima mesclagem pode dar conflito",
    ),
    ("rejection reason delivered to the agent", "motivo da reprovação entregue ao agente"),
    ("data of task {id} outside what was expected; I touched nothing", "dados da tarefa {id} fora do esperado; não mexi em nada"),
    (
        "the worktree folder vanished and I could not recreate it: {error}",
        "a pasta da worktree sumiu e não consegui recriá-la: {error}",
    ),
    ("task {id} is already {status}", "a tarefa {id} já está {status}"),
    (
        "repository unavailable ({error}): only the task record changed",
        "repositório indisponível ({error}): só o registro da tarefa mudou",
    ),
    (
        "discarded; worktree and branch removed (what was already merged stays in {base})",
        "descartada; worktree e branch removidas (o que já foi mesclado continua em {base})",
    ),
    ("discarded; worktree and branch removed", "descartada; worktree e branch removidas"),
    ("Task {id} discarded: {title}", "Tarefa {id} descartada: {title}"),
    ("the folder {folder} remained ({git}{error}); {left}", "a pasta {folder} ficou ({git}{error}); {left}"),
    (
        "the folder {folder} is not in .agentdeck/worktrees: I did not touch it",
        "a pasta {folder} não está em .agentdeck/worktrees: não mexi nela",
    ),
    ("git worktree prune failed: {error}", "git worktree prune falhou: {error}"),
    // Git operations and the stash notices.
    ("(no changes relative to the base)", "(nenhuma alteração em relação à base)"),
    ("the folder {folder} does not exist", "a pasta {folder} não existe"),
    (
        "Your uncommitted changes are still kept in git's stash (\"{message}\"): {why} Nothing was lost. To recover them: open a terminal in the project folder, run git stash list to find the entry and git stash pop to give it back (resolving whatever clashes).",
        "Suas alterações sem commit continuam guardadas no stash do git (\"{message}\"): {why} Nada se perdeu. Para recuperá-las: abra um terminal na pasta do projeto, rode git stash list para achar a entrada e git stash pop para devolvê-la (resolvendo o que chocar).",
    ),
    (
        "The stash with your changes is no longer in git's list (did someone use or delete it?); check with git stash list.",
        "O stash com as suas alterações não está mais na lista do git (alguém o usou ou apagou?); confira com git stash list.",
    ),
    (
        "giving them back, they would clash with what the task brought in {files}.",
        "ao devolvê-las elas se chocariam com o que a tarefa trouxe em {files}.",
    ),
    (
        "I could not check whether they clash with what the task brought.",
        "não consegui conferir se elas se chocam com o que a tarefa trouxe.",
    ),
    (
        "Your uncommitted changes were stashed during the merge and are already back in the folder.",
        "Suas alterações sem commit foram guardadas durante a mesclagem e já voltaram para a pasta.",
    ),
    (
        " The folder was left with conflicting files: check with git status.",
        " A pasta ficou com arquivos em conflito: confira com git status.",
    ),
    ("git could not give them back ({error}).{mess}", "o git não conseguiu devolvê-las ({error}).{mess}"),
    // Store.
    ("error: {message}", "erro: {message}"),
    ("task \"{id}\" not found in this project", "tarefa \"{id}\" não encontrada neste projeto"),
    ("unknown project: {project}", "projeto desconhecido: {project}"),
    (
        "the project is not a git repository; tasks need one (run git init)",
        "o projeto não é um repositório git; as tarefas precisam de um (rode git init)",
    ),
];
