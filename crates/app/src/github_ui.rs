//! Estado da aba "GitHub" na barra direita e ponte entre [`Deck`] e [`agentdeck_core::github`].
//!
//! Toda chamada ao GitHub (`git remote`, `git credential fill`, `gh auth token` e API REST) roda
//! fora da thread da interface e volta por [`CoreEvent::Github`]; aqui só ficam o estado da tela e
//! o tratamento desses eventos. A pintura está em `ui/github.rs`.

use std::collections::HashMap;
use std::time::Instant;

use agentdeck_core::github::{
    CheckStatus, GithubError, GithubEvent, GithubOverview, GithubPayload, Issue, JobLog, PrChecksDetail, PullRequest, RateLimitInfo,
    RepoSlug, WorkflowJob, WorkflowRun,
};
use agentdeck_core::Ident;
use egui::Rect;

use crate::deck::{Deck, RightView};

/// Sub-aba ativa dentro da aba GitHub.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum GithubTab {
    #[default]
    Prs,
    Issues,
    Actions,
}

/// O que a aba mostra para o projeto ativo.
#[derive(Debug, Clone, Default)]
pub enum GithubView {
    #[default]
    Loading,
    /// Nenhum projeto aberto no AgentDeck.
    NoProject,
    /// Falha sem dados anteriores na tela (sem remoto do GitHub, rede, login recusado, limite...).
    Error(GithubError),
    /// Dados carregados do repositório.
    Ready(GithubOverview),
}

/// Menu "Enviar para agente": o que será enviado e onde o menu ancora.
#[derive(Debug, Clone)]
pub struct AgentMenuTarget {
    /// Retângulo do botão que abriu o menu.
    pub anchor: Rect,
    /// Rótulo curto do que será enviado (ex.: `PR #42`).
    pub title: String,
    /// Mensagem pronta.
    pub prompt: String,
}

/// Estado da aba GitHub no [`Deck`].
#[derive(Default)]
pub struct GithubUi {
    pub tab: GithubTab,
    pub view: GithubView,
    /// Último repositório conhecido (para o título quando a leitura seguinte falha).
    pub repo: Option<RepoSlug>,
    /// Há uma leitura geral em andamento.
    pub refreshing: bool,
    /// A leitura em andamento é forçada (botão Atualizar): refaz também o detalhe aberto.
    pub forced: bool,
    /// Aviso ou erro recente no topo da aba `(texto, é_erro)`.
    pub notice: Option<(String, bool)>,
    /// Quando a última leitura terminou.
    pub last_refresh: Option<Instant>,

    pub selected_pr: Option<u64>,
    /// Checks e jobs por número de PR.
    pub pr_details: HashMap<u64, PrChecksDetail>,
    pub pr_loading: Option<u64>,

    pub selected_issue: Option<u64>,

    pub selected_run: Option<u64>,
    /// Jobs por execução.
    pub run_jobs: HashMap<u64, Vec<WorkflowJob>>,
    pub run_loading: Option<u64>,

    /// Fim do log por job (ou o motivo de não ter vindo).
    pub job_logs: HashMap<u64, Result<JobLog, GithubError>>,
    pub log_loading: Option<u64>,
    /// Job cujo log está aberto `(job_id, nome, PR de origem)`.
    pub open_log: Option<(u64, String, Option<u64>)>,

    pub agent_menu: Option<AgentMenuTarget>,
    /// Modelo escolhido para abrir/usar uma instância com cargo.
    pub new_role_kind: String,
    /// Cargo digitado (ex.: `review`).
    pub new_role_name: String,

    /// SOMENTE DEPURAÇÃO (`AGENTDECK_DEBUG_UI=github*`): ignora eventos reais e leituras.
    pub debug_hold: bool,
}

impl GithubUi {
    /// Dados carregados, se houver.
    pub fn overview(&self) -> Option<&GithubOverview> {
        match &self.view {
            GithubView::Ready(ov) => Some(ov),
            _ => None,
        }
    }

    /// PRs abertos (selo da aba na barra direita).
    pub fn open_prs_count(&self) -> u32 {
        self.overview().map(|o| o.prs.len() as u32).unwrap_or(0)
    }

    /// Dica do ícone da aba.
    pub fn tab_tooltip(&self) -> String {
        match &self.view {
            GithubView::Ready(ov) => {
                let (prs, issues) = (ov.prs.len(), ov.issues.len());
                format!(
                    "GitHub · {} ({} PR{}, {} issue{})",
                    ov.repo,
                    prs,
                    if prs == 1 { "" } else { "s" },
                    issues,
                    if issues == 1 { "" } else { "s" }
                )
            }
            _ => match &self.repo {
                Some(r) => format!("GitHub · {r}"),
                None => "GitHub (PRs, issues e Actions)".to_string(),
            },
        }
    }
}

/// Cargo digitado pelo usuário vira palavra válida de instância (minúsculas, números, `-`, `_`).
pub fn normalize_role(text: &str) -> String {
    text.trim()
        .to_lowercase()
        .chars()
        .map(|c| if c.is_whitespace() { '-' } else { c })
        .filter(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || matches!(c, '-' | '_'))
        .take(32)
        .collect()
}

/// `modelo` ou `modelo@cargo` a partir do que o menu tem.
pub fn instance_spec(kind: &str, role: &str) -> String {
    let role = normalize_role(role);
    if role.is_empty() {
        kind.to_owned()
    } else {
        format!("{kind}@{role}")
    }
}

impl Deck {
    /// Limpa o estado ao trocar de projeto; com a aba aberta, já lê o novo.
    pub fn github_reset_for_project(&mut self) {
        if self.github.debug_hold {
            return;
        }
        let tab = self.github.tab;
        self.github = GithubUi {
            tab,
            view: if self.active_project.is_empty() { GithubView::NoProject } else { GithubView::Loading },
            ..Default::default()
        };
        if self.right_view == RightView::GitHub {
            self.github_refresh();
        }
    }

    /// Lê os dados do projeto ativo em segundo plano (usa o cache de 60 s do core).
    pub fn github_refresh(&mut self) {
        self.github_load(false);
    }

    /// Botão Atualizar: ignora os caches e refaz também o detalhe aberto.
    pub fn github_refresh_now(&mut self) {
        self.github_load(true);
    }

    fn github_load(&mut self, force: bool) {
        if self.github.debug_hold {
            return;
        }
        if self.active_project.is_empty() || self.workspace.as_os_str().is_empty() {
            self.github.view = GithubView::NoProject;
            self.github.refreshing = false;
            return;
        }
        if self.github.refreshing {
            self.github.forced |= force;
            return;
        }
        if force {
            self.github.pr_details.clear();
            self.github.run_jobs.clear();
            self.github.job_logs.clear();
        }
        self.github.refreshing = true;
        self.github.forced = force;
        self.github.notice = None;
        self.core.github_refresh(force);
    }

    /// Seleciona (ou desseleciona) um PR e carrega os checks dele se ainda não os tem.
    pub fn github_select_pr(&mut self, number: u64) {
        if self.github.selected_pr == Some(number) {
            self.github.selected_pr = None;
            return;
        }
        self.github.selected_pr = Some(number);
        self.github.open_log = None;
        if self.github.debug_hold {
            return;
        }
        self.github_load_pr_detail(number, false);
    }

    fn github_load_pr_detail(&mut self, number: u64, force: bool) {
        if self.github.pr_loading == Some(number) {
            return;
        }
        if let Some(detail) = self.github.pr_details.get(&number) {
            let jobs = detail.jobs.clone();
            self.auto_open_failed_job_log(Some(number), &jobs);
            return;
        }
        let Some(sha) = self.github.overview().and_then(|ov| ov.prs.iter().find(|p| p.number == number)).map(|p| p.head_sha.clone()) else {
            return;
        };
        self.github.pr_loading = Some(number);
        self.core.github_load_pr(number, sha, force);
    }

    /// Seleciona (ou desseleciona) uma execução do Actions e carrega os jobs dela.
    pub fn github_select_run(&mut self, run_id: u64) {
        if self.github.selected_run == Some(run_id) {
            self.github.selected_run = None;
            return;
        }
        self.github.selected_run = Some(run_id);
        self.github.open_log = None;
        if self.github.debug_hold {
            return;
        }
        self.github_load_run_detail(run_id, false);
    }

    fn github_load_run_detail(&mut self, run_id: u64, force: bool) {
        if self.github.run_loading == Some(run_id) {
            return;
        }
        if let Some(jobs) = self.github.run_jobs.get(&run_id) {
            let jobs = jobs.clone();
            self.auto_open_failed_job_log(None, &jobs);
            return;
        }
        self.github.run_loading = Some(run_id);
        self.core.github_load_run_jobs(run_id, force);
    }

    /// Abre o painel com o fim do log de um job e o baixa se ainda não o tem.
    pub fn github_request_job_log(&mut self, job_id: u64, job_name: String, pr_number: Option<u64>) {
        self.github.open_log = Some((job_id, job_name, pr_number));
        if self.github.debug_hold || self.github.job_logs.contains_key(&job_id) {
            return;
        }
        self.github.log_loading = Some(job_id);
        self.core.github_load_job_log(job_id, false);
    }

    /// Com jobs vermelhos na lista, abre o log do primeiro (se nenhum deles já está aberto).
    fn auto_open_failed_job_log(&mut self, pr_number: Option<u64>, jobs: &[WorkflowJob]) {
        let Some(failed) = jobs.iter().find(|j| j.status == CheckStatus::Failure) else { return };
        let keep_current =
            self.github.open_log.as_ref().is_some_and(|(id, _, _)| jobs.iter().any(|j| j.id == *id && j.status == CheckStatus::Failure));
        if !keep_current {
            self.github_request_job_log(failed.id, failed.name.clone(), pr_number);
        }
    }

    /// Trata um evento do core. Eventos de outro projeto ou de uma leitura antiga são ignorados.
    pub fn on_github_event(&mut self, ev: GithubEvent) {
        if self.github.debug_hold || ev.project != self.active_project {
            return;
        }
        match ev.payload {
            GithubPayload::Overview(Ok(overview)) => {
                self.github.refreshing = false;
                self.github.last_refresh = Some(Instant::now());
                self.github.notice = None;
                self.github.repo = Some(overview.repo.clone());
                let forced = std::mem::take(&mut self.github.forced);
                self.github.view = GithubView::Ready(overview);
                // Forçado: recarrega o que estava aberto (os caches da tela foram limpos).
                if let Some(num) = self.github.selected_pr {
                    self.github_load_pr_detail(num, forced);
                }
                if let Some(id) = self.github.selected_run {
                    self.github_load_run_detail(id, forced);
                }
            }
            GithubPayload::Overview(Err(error)) => {
                self.github.refreshing = false;
                self.github.forced = false;
                if matches!(self.github.view, GithubView::Ready(_)) {
                    // Mantém o que já estava na tela e avisa na faixa do topo.
                    self.github.notice = Some((error.to_string(), true));
                } else {
                    self.github.view = GithubView::Error(error);
                }
            }
            GithubPayload::PrDetail { pr_number, result } => {
                if self.github.pr_loading == Some(pr_number) {
                    self.github.pr_loading = None;
                }
                match result {
                    Ok(mut detail) => {
                        let status = detail.status();
                        if let GithubView::Ready(ov) = &mut self.github.view {
                            if let Some(pr) = ov.prs.iter_mut().find(|p| p.number == pr_number) {
                                if !detail.runs.is_empty() {
                                    pr.checks = status;
                                }
                            }
                        }
                        let failed_log = detail.failed_log.take();
                        let jobs = detail.jobs.clone();
                        self.github.pr_details.insert(pr_number, detail);
                        self.store_preloaded_log(failed_log, &jobs);
                        if self.github.selected_pr == Some(pr_number) {
                            self.auto_open_failed_job_log(Some(pr_number), &jobs);
                        }
                    }
                    Err(e) => self.github.notice = Some((format!("PR #{pr_number}: {e}"), true)),
                }
            }
            GithubPayload::RunJobs { run_id, result, failed_log } => {
                if self.github.run_loading == Some(run_id) {
                    self.github.run_loading = None;
                }
                match result {
                    Ok(jobs) => {
                        self.store_preloaded_log(failed_log, &jobs);
                        self.github.run_jobs.insert(run_id, jobs.clone());
                        if self.github.selected_run == Some(run_id) {
                            self.auto_open_failed_job_log(None, &jobs);
                        }
                    }
                    Err(e) => self.github.notice = Some((format!("Execução #{run_id}: {e}"), true)),
                }
            }
            GithubPayload::JobLog { job_id, result } => {
                if self.github.log_loading == Some(job_id) {
                    self.github.log_loading = None;
                }
                self.github.job_logs.insert(job_id, result);
            }
        }
    }

    /// Guarda o log que o core já baixou junto com os jobs.
    fn store_preloaded_log(&mut self, log: Option<(u64, Result<JobLog, GithubError>)>, jobs: &[WorkflowJob]) {
        if let Some((job_id, result)) = log {
            if jobs.iter().any(|j| j.id == job_id) {
                self.github.job_logs.insert(job_id, result);
            }
        }
    }

    /// Abre uma URL do GitHub no navegador padrão.
    pub fn open_github_browser(&mut self, url: &str) {
        if let Err(e) = self.core.open_github_url(url) {
            self.github.notice = Some((format!("Não foi possível abrir o navegador: {e}"), true));
        }
    }

    /// Abre o menu "Enviar para agente" com a mensagem pronta.
    pub fn open_github_agent_menu(&mut self, anchor: Rect, title: String, suggested_role: &str, prompt: String) {
        if self.github.new_role_kind.is_empty() {
            self.github.new_role_kind =
                self.active_tab().map(|t| t.kind.clone()).filter(|k| k != "shell").unwrap_or_else(|| self.agents.fallback_kind());
        }
        self.github.new_role_name = suggested_role.to_owned();
        self.github.agent_menu = Some(AgentMenuTarget { anchor, title, prompt });
    }

    /// Envia a mensagem pronta para a instância `spec` (`claude`, `claude@review`...), abrindo a aba
    /// se ela não existir. Fecha o menu e avisa o resultado na faixa do topo.
    pub fn send_github_prompt_to_agent(&mut self, spec: &str, prompt: String) {
        self.github.agent_menu = None;
        let Some(want) = Ident::parse(spec) else {
            self.github.notice = Some((format!("Nome de agente inválido: \"{spec}\"."), true));
            return;
        };
        self.github.notice = Some(match self.send_to_instance(&want, false, prompt) {
            Ok(label) => (format!("Enviado para {label}."), false),
            Err(msg) => (msg, true),
        });
    }
}

// ---------------------------------------------------------------------------
// Dados fictícios para capturas de tela (`AGENTDECK_DEBUG_UI=github-demo`)
// ---------------------------------------------------------------------------

const DEMO_SHA_42: &str = "61621aa9c4f01b2e3d4a5f60718293a4b5c6d7e8";

pub fn demo_overview() -> GithubOverview {
    let pr = |number, title: &str, author: &str, head: &str, sha: &str, draft, checks| PullRequest {
        number,
        title: title.into(),
        author: author.into(),
        head_branch: head.into(),
        base_branch: "dev".into(),
        head_sha: sha.into(),
        draft,
        html_url: format!("https://github.com/pattr/agentdeck/pull/{number}"),
        updated_at: "2026-10-16T03:30:00Z".into(),
        checks,
    };
    let issue = |number, title: &str, author: &str, labels: &[&str], comments, body: &str| Issue {
        number,
        title: title.into(),
        author: author.into(),
        labels: labels.iter().map(|l| (*l).to_owned()).collect(),
        comments,
        html_url: format!("https://github.com/pattr/agentdeck/issues/{number}"),
        updated_at: "2026-10-15T21:10:00Z".into(),
        body_preview: body.into(),
    };
    let run = |id, title: &str, branch: &str, sha: &str, status, conclusion: Option<&str>| WorkflowRun {
        id,
        name: "CI".into(),
        display_title: title.into(),
        head_branch: branch.into(),
        head_sha: sha.into(),
        event: "pull_request".into(),
        status,
        raw_status: if conclusion.is_some() { "completed".into() } else { "in_progress".into() },
        conclusion: conclusion.map(str::to_owned),
        html_url: format!("https://github.com/pattr/agentdeck/actions/runs/{id}"),
        created_at: "2026-10-16T03:28:00Z".into(),
    };
    GithubOverview {
        repo: RepoSlug::new("pattr", "agentdeck"),
        authenticated: true,
        prs: vec![
            pr(42, "feat(github): integração com PRs, issues e Actions", "pattr", "feat/github", DEMO_SHA_42, false, CheckStatus::Failure),
            pr(
                39,
                "feat(git): histórico de commits e seletor de branch",
                "ana-dev",
                "feat/git-panel",
                "89ab12cd34ef56ab78cd90ef12ab34cd56ef7890",
                false,
                CheckStatus::Success,
            ),
            pr(
                44,
                "wip(terminal): busca incremental no scrollback",
                "carlos",
                "feat/term-search",
                "11223344556677889900aabbccddeeff00112233",
                true,
                CheckStatus::Pending,
            ),
            pr(45, "docs: guia de contribuição", "bia", "docs/contrib", "", false, CheckStatus::Neutral),
        ],
        issues: vec![
            issue(
                38,
                "Atalho para alternar entre as abas Git e GitHub",
                "pattr",
                &["enhancement", "ui"],
                3,
                "Seria útil ter atalhos de teclado dedicados para abrir as abas da barra direita rapidamente, sem usar o mouse.",
            ),
            issue(
                31,
                "ConPTY no Windows 11 24H2: cursor pisca ao redimensionar",
                "marcos",
                &["bug", "windows"],
                5,
                "Ao arrastar o divisor da barra direita rapidamente, o ConPTY emite sequências DECSCUSR extras e o cursor pisca.",
            ),
        ],
        runs: vec![
            run(
                9001,
                "feat(github): integração com PRs, issues e Actions",
                "feat/github",
                DEMO_SHA_42,
                CheckStatus::Failure,
                Some("failure"),
            ),
            run(
                9000,
                "feat(git): histórico de commits e seletor de branch",
                "feat/git-panel",
                "89ab12cd34ef56ab78cd90ef12ab34cd56ef7890",
                CheckStatus::Success,
                Some("success"),
            ),
            run(
                8999,
                "wip(terminal): busca incremental no scrollback",
                "feat/term-search",
                "11223344556677889900aabbccddeeff00112233",
                CheckStatus::Pending,
                None,
            ),
        ],
        rate_limit: Some(RateLimitInfo { remaining: Some(4892), limit: Some(5000), reset_at_epoch_s: None }),
    }
}

pub fn demo_pr_detail() -> PrChecksDetail {
    let job = |id, name: &str, status, conclusion: &str, steps: &[&str]| WorkflowJob {
        id,
        run_id: 9001,
        name: name.into(),
        status,
        conclusion: Some(conclusion.into()),
        html_url: format!("https://github.com/pattr/agentdeck/actions/runs/9001/job/{id}"),
        failed_steps: steps.iter().map(|s| (*s).to_owned()).collect(),
    };
    PrChecksDetail {
        pr_number: 42,
        head_sha: DEMO_SHA_42.into(),
        runs: vec![demo_overview().runs[0].clone()],
        jobs: vec![
            job(501, "clippy (windows-latest)", CheckStatus::Failure, "failure", &["cargo clippy --workspace -- -D warnings"]),
            job(502, "test (windows-latest)", CheckStatus::Success, "success", &[]),
            job(503, "fmt", CheckStatus::Success, "success", &[]),
        ],
        failed_log: None,
    }
}

pub fn demo_job_log() -> JobLog {
    JobLog {
        lines: [
            "Checking agentdeck-core v0.3.0 (D:\\a\\agentdeck\\crates\\core)",
            "Checking agentdeck v0.3.0 (D:\\a\\agentdeck\\crates\\app)",
            "error: redundant closure",
            "  --> crates/core/src/github.rs:312:44",
            "   |",
            "312|     let slug = url.and_then(|u| parse_github_remote(&u));",
            "   |                             ^^^^^^^^^^^^^^^^^^^^^^^^^^^ help: replace the closure with the function itself",
            "   = note: `-D clippy::redundant-closure` implied by `-D warnings`",
            "error: could not compile `agentdeck-core` (lib) due to 1 previous error",
            "##[error]Process completed with exit code 101.",
        ]
        .iter()
        .map(|l| (*l).to_owned())
        .collect(),
        truncated: true,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roles_become_valid_words() {
        assert_eq!(normalize_role("  Review "), "review");
        assert_eq!(normalize_role("code review!"), "code-review");
        assert_eq!(normalize_role("@ci"), "ci");
        assert_eq!(normalize_role("???"), "");
        assert_eq!(normalize_role(&"a".repeat(50)).len(), 32);
    }

    #[test]
    fn instance_specs_parse_as_idents() {
        assert_eq!(instance_spec("claude", ""), "claude");
        assert_eq!(instance_spec("claude", "Review"), "claude@review");
        let id = Ident::parse(&instance_spec("ollama:qwen3.5:4b", "ci")).expect("id válido");
        assert_eq!(id.kind, "ollama:qwen3.5:4b");
        assert_eq!(id.role.as_deref(), Some("ci"));
    }

    #[test]
    fn demo_data_is_consistent() {
        let ov = demo_overview();
        let detail = demo_pr_detail();
        assert!(ov.prs.iter().any(|p| p.number == detail.pr_number && p.head_sha == detail.head_sha));
        assert_eq!(detail.status(), CheckStatus::Failure);
        assert!(detail.jobs.iter().any(|j| j.status == CheckStatus::Failure));
        let ui = GithubUi { view: GithubView::Ready(ov), ..Default::default() };
        assert_eq!(ui.open_prs_count(), 4);
        assert!(ui.tab_tooltip().contains("pattr/agentdeck"));
    }
}
