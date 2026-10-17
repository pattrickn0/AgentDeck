//! Estado da aba "GitHub" na barra direita e ponte entre [`Deck`] e [`commanddeck_core::github`].
//!
//! Toda chamada ao GitHub (`git remote`, `git credential fill`, `gh auth token` e API REST) roda
//! fora da thread da interface e entrega [`GithubEvent`] via [`CoreEvent::Github`].

use std::collections::HashMap;
use std::time::Instant;

use commanddeck_core::github::{
    CheckStatus, GithubError, GithubEvent, GithubOverview, GithubPayload, Issue, JobLog, PrChecksDetail, PullRequest, RateLimitInfo,
    RepoSlug, WorkflowJob, WorkflowRun,
};
use commanddeck_core::{FeedEntry, FeedKind, Ident};
use egui::Rect;

use crate::deck::{Deck, Queued, RightView};

/// Sub-aba ativa dentro da aba GitHub.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum GithubTab {
    #[default]
    Prs,
    Issues,
    Actions,
}

/// Estado principal da visão GitHub para o projeto ativo.
#[derive(Debug, Clone, Default)]
pub enum GithubView {
    #[default]
    Loading,
    /// Nenhum projeto aberto no CommandDeck.
    NoProject,
    /// A pasta não tem repositório git ou o remoto `origin` não é do GitHub.
    NotGithub { reason: String },
    /// Falha ao carregar os dados iniciais (sem cache anterior).
    Error { repo: Option<RepoSlug>, error: GithubError },
    /// Dados carregados do repositório.
    Ready(GithubOverview),
}

/// Alvo do menu "Enviar para agente ▾".
#[derive(Debug, Clone)]
pub struct AgentMenuTarget {
    /// Retângulo do botão que abriu o menu (âncora do popup).
    pub anchor: Rect,
    /// Rótulo curto do que será enviado (ex.: `PR #42` ou `Job "cargo clippy"`).
    pub title: String,
    /// Cargo sugerido ao abrir uma nova instância (ex.: `review`, `ci`, `fix`).
    pub suggested_role: &'static str,
    /// Mensagem pronta a ser enviada para o agente escolhido.
    pub prompt: String,
}

/// Estado da aba GitHub na [`Deck`].
#[derive(Default)]
pub struct GithubUi {
    pub tab: GithubTab,
    pub view: GithubView,
    /// Operação de atualização geral em andamento.
    pub refreshing: bool,
    /// Aviso ou erro recente exibido no topo da aba `(texto, é_erro)`.
    pub notice: Option<(String, bool)>,
    /// Quando o último refresh completou para o projeto ativo.
    pub last_refresh: Option<Instant>,

    /// PR selecionado na sub-aba PRs (`number`).
    pub selected_pr: Option<u64>,
    /// Detalhes de checks/jobs por número de PR.
    pub pr_details: HashMap<u64, PrChecksDetail>,
    /// PR cujo detalhe está carregando no momento.
    pub pr_loading: Option<u64>,

    /// Issue selecionada na sub-aba Issues (`number`).
    pub selected_issue: Option<u64>,

    /// Execução (workflow run) selecionada na sub-aba Actions (`run_id`).
    pub selected_run: Option<u64>,
    /// Jobs carregados por `run_id`.
    pub run_jobs: HashMap<u64, Vec<WorkflowJob>>,
    /// `run_id` cujos jobs estão carregando no momento.
    pub run_loading: Option<u64>,

    /// Logs carregados por `job_id`.
    pub job_logs: HashMap<u64, JobLog>,
    /// `job_id` cujo log está sendo baixado no momento.
    pub log_loading: Option<u64>,
    /// Job cujo log está aberto no painel inferior `(job_id, nome_do_job, pr_number)`.
    pub open_log: Option<(u64, String, Option<u64>)>,

    /// Menu "Enviar para agente ▾" aberto.
    pub agent_menu: Option<AgentMenuTarget>,
    /// Modelo escolhido para abrir uma nova instância com cargo no menu "Enviar para agente".
    pub new_role_kind: String,
    /// Nome do cargo digitado no menu "Enviar para agente" (ex.: `review`, `ci`).
    pub new_role_name: String,

    /// SOMENTE DEPURAÇÃO (`COMMANDDECK_DEBUG_UI=github*`): ignora eventos reais para manter o estado de demonstração.
    pub debug_hold: bool,
}

impl GithubUi {
    /// Dados carregados do GitHub, se disponíveis.
    pub fn overview(&self) -> Option<&GithubOverview> {
        match &self.view {
            GithubView::Ready(ov) => Some(ov),
            _ => None,
        }
    }

    /// Número de PRs abertos (para o badge na aba da barra direita).
    pub fn open_prs_count(&self) -> u32 {
        self.overview().map(|o| o.pulls.len() as u32).unwrap_or(0)
    }

    /// Texto da dica (tooltip) do ícone do GitHub no cabeçalho da barra direita.
    pub fn tab_tooltip(&self) -> String {
        match &self.view {
            GithubView::Ready(ov) => {
                let prs = ov.pulls.len();
                let issues = ov.issues.len();
                format!("GitHub · {} ({} PR{}, {} issue{})", ov.repo, prs, if prs == 1 { "" } else { "s" }, issues, if issues == 1 { "" } else { "s" })
            }
            GithubView::Error { repo: Some(r), .. } => format!("GitHub · {r}"),
            _ => "GitHub (PRs, issues e Actions)".to_string(),
        }
    }
}

impl Deck {
    /// Limpa o estado da aba GitHub ao trocar de projeto. Se a aba estiver visível, dispara a leitura.
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

    /// Atualiza os dados do GitHub do projeto ativo (em segundo plano).
    pub fn github_refresh(&mut self) {
        if self.github.debug_hold {
            return;
        }
        if self.active_project.is_empty() || self.workspace.as_os_str().is_empty() {
            self.github.view = GithubView::NoProject;
            self.github.refreshing = false;
            return;
        }
        self.github.refreshing = true;
        self.github.notice = None;
        self.core.github_refresh(&self.active_project);
    }

    /// Seleciona um PR na lista e carrega os checks/jobs dele se ainda não estiverem em cache.
    pub fn github_select_pr(&mut self, number: u64) {
        if self.github.selected_pr == Some(number) {
            self.github.selected_pr = None;
            return;
        }
        self.github.selected_pr = Some(number);
        if self.github.debug_hold {
            return;
        }
        if let Some(detail) = self.github.pr_details.get(&number).cloned() {
            self.auto_open_failed_job_log(Some(number), &detail.jobs);
            return;
        }
        if let Some(ov) = self.github.overview() {
            if let Some(pr) = ov.pulls.iter().find(|p| p.number == number) {
                let repo = ov.repo.clone();
                let sha = pr.head_sha.clone();
                let branch = pr.head_ref.clone();
                self.github.pr_loading = Some(number);
                self.core.github_load_pr(&self.active_project, repo, number, sha, branch);
            }
        }
    }

    /// Seleciona uma execução (run) na sub-aba Actions e busca os jobs dela.
    pub fn github_select_run(&mut self, run_id: u64) {
        if self.github.selected_run == Some(run_id) {
            self.github.selected_run = None;
            return;
        }
        self.github.selected_run = Some(run_id);
        if self.github.debug_hold {
            return;
        }
        if let Some(jobs) = self.github.run_jobs.get(&run_id).cloned() {
            self.auto_open_failed_job_log(None, &jobs);
            return;
        }
        if let Some(ov) = self.github.overview() {
            let repo = ov.repo.clone();
            self.github.run_loading = Some(run_id);
            self.core.github_load_run_jobs(&self.active_project, repo, run_id);
        }
    }

    /// Baixa (ou mostra do cache) o fim do log de um job que falhou.
    pub fn github_request_job_log(&mut self, job_id: u64, job_name: String, pr_number: Option<u64>) {
        self.github.open_log = Some((job_id, job_name.clone(), pr_number));
        if self.github.debug_hold || self.github.job_logs.contains_key(&job_id) {
            return;
        }
        if let Some(ov) = self.github.overview() {
            let repo = ov.repo.clone();
            self.github.log_loading = Some(job_id);
            self.core.github_load_job_log(&self.active_project, repo, job_id, job_name);
        }
    }

    /// Se houver algum job que falhou na lista, seleciona automaticamente o primeiro e busca o log.
    fn auto_open_failed_job_log(&mut self, pr_number: Option<u64>, jobs: &[WorkflowJob]) {
        if let Some(failed) = jobs.iter().find(|j| j.status == CheckStatus::Failure) {
            let keep_current = self
                .github
                .open_log
                .as_ref()
                .is_some_and(|(id, _, _)| jobs.iter().any(|j| j.id == *id && j.status == CheckStatus::Failure));
            if !keep_current {
                self.github_request_job_log(failed.id, failed.name.clone(), pr_number);
            }
        }
    }

    /// Trata um [`GithubEvent`] emitido pelo core.
    pub fn on_github_event(&mut self, ev: GithubEvent) {
        if self.github.debug_hold || ev.project != self.active_project {
            return;
        }
        match ev.payload {
            GithubPayload::NotGithubRepo { reason } => {
                self.github.refreshing = false;
                self.github.view = GithubView::NotGithub { reason };
            }
            GithubPayload::Overview(Ok(overview)) => {
                self.github.refreshing = false;
                self.github.last_refresh = Some(Instant::now());
                self.github.notice = None;
                // Se o PR selecionado já estava aberto e seus detalhes ainda não foram carregados, dispara.
                let selected_pr = self.github.selected_pr;
                self.github.view = GithubView::Ready(overview);
                if let Some(num) = selected_pr {
                    if !self.github.pr_details.contains_key(&num) {
                        self.github.selected_pr = None;
                        self.github_select_pr(num);
                    }
                }
            }
            GithubPayload::Overview(Err(error)) => {
                self.github.refreshing = false;
                let msg = error.message_pt();
                if matches!(self.github.view, GithubView::Ready(_)) {
                    // Mantém o cache anterior na tela e mostra o erro como faixa de aviso.
                    self.github.notice = Some((msg, true));
                } else {
                    let repo = self.core.github_repo_at(&self.workspace).ok();
                    self.github.view = GithubView::Error { repo, error };
                }
            }
            GithubPayload::PrDetail(Ok(detail)) => {
                let pr_num = detail.pr_number;
                if self.github.pr_loading == Some(pr_num) {
                    self.github.pr_loading = None;
                }
                if let GithubView::Ready(ov) = &mut self.github.view {
                    if let Some(pr) = ov.pulls.iter_mut().find(|p| p.number == pr_num) {
                        pr.checks = detail.status;
                    }
                }
                let jobs = detail.jobs.clone();
                self.github.pr_details.insert(pr_num, detail);
                if self.github.selected_pr == Some(pr_num) {
                    self.auto_open_failed_job_log(Some(pr_num), &jobs);
                }
            }
            GithubPayload::PrDetail(Err { pr_number, error }) => {
                if self.github.pr_loading == Some(pr_number) {
                    self.github.pr_loading = None;
                }
                self.github.notice = Some((format!("PR #{pr_number}: {}", error.message_pt()), true));
            }
            GithubPayload::RunJobs(Ok { run_id, jobs }) => {
                if self.github.run_loading == Some(run_id) {
                    self.github.run_loading = None;
                }
                if self.github.selected_run == Some(run_id) {
                    self.auto_open_failed_job_log(None, &jobs);
                }
                self.github.run_jobs.insert(run_id, jobs);
            }
            GithubPayload::RunJobs(Err { run_id, error }) => {
                if self.github.run_loading == Some(run_id) {
                    self.github.run_loading = None;
                }
                self.github.notice = Some((format!("Execução #{run_id}: {}", error.message_pt()), true));
            }
            GithubPayload::JobLog(Ok(log)) => {
                if self.github.log_loading == Some(log.job_id) {
                    self.github.log_loading = None;
                }
                self.github.job_logs.insert(log.job_id, log);
            }
            GithubPayload::JobLog(Err { job_id, error }) => {
                if self.github.log_loading == Some(job_id) {
                    self.github.log_loading = None;
                }
                self.github.notice = Some((format!("Log do job: {}", error.message_pt()), true));
            }
        }
    }

    /// Abre uma URL do GitHub no navegador padrão do sistema.
    pub fn open_github_browser(&mut self, url: &str) {
        if let Err(e) = self.core.open_github_url(url) {
            self.github.notice = Some((format!("Não foi possível abrir o navegador: {e}"), true));
        }
    }

    /// Abre o menu "Enviar para agente ▾" para um PR, issue ou job falho.
    pub fn open_github_agent_menu(&mut self, anchor: Rect, title: String, suggested_role: &'static str, prompt: String) {
        if self.github.new_role_kind.is_empty() {
            self.github.new_role_kind = self
                .active_tab()
                .map(|t| t.kind.clone())
                .filter(|k| k != "shell")
                .unwrap_or_else(|| self.agents.fallback_kind());
        }
        self.github.new_role_name = suggested_role.to_owned();
        self.github.agent_menu = Some(AgentMenuTarget { anchor, title, suggested_role, prompt });
    }

    /// Envia uma mensagem pronta (de PR, issue ou log de job) para uma instância de agente do
    /// projeto ativo, abrindo a aba se ela ainda não existir (mesmo fluxo de entrega do `ask-agent`).
    pub fn send_github_prompt_to_agent(&mut self, target_spec: &str, force_new: bool, prompt: String) {
        let project = self.active_project.clone();
        if project.is_empty() {
            self.system("Abra uma pasta como projeto antes de enviar para um agente.");
            return;
        }
        let Some(want) = Ident::parse(target_spec.trim()) else {
            self.github.notice = Some((format!("Identificador de agente inválido: \"{target_spec}\"."), true));
            return;
        };
        if !self.agents.knows(&want.kind) {
            self.github.notice = Some((format!("O agente \"{}\" não está disponível.", want.kind), true));
            return;
        }
        self.load_project(&project);
        let ident = match self.core.resolve_instance(&project, &want, force_new) {
            Ok((ident, _)) => ident,
            Err(candidates) => {
                // Mais de uma instância do mesmo cargo aberta: usa a primeira encontrada.
                if let Some(first) = candidates.first().and_then(|id| Ident::parse(id)) {
                    first
                } else {
                    let (ident, _) = self.core.resolve_instance(&project, &want, true).expect("force_new não falha");
                    ident
                }
            }
        };
        let id = ident.id();
        let found = self.tabs.iter().position(|t| t.project == project && t.ident.id() == id);
        let (idx, newly_opened) = match found {
            Some(i) => (i, false),
            None => {
                self.add_tab(project.clone(), ident.clone(), false, false);
                (self.tabs.len() - 1, true)
            }
        };
        self.activate(idx);
        if !self.tabs[idx].alive {
            self.start(idx);
        }
        // Se a aba acabou de ser aberta, reiniciada ou está ocupada, entra na fila da aba (`delivery::gate`
        // espera o processo iniciar / terminar o turno antes de colar e apertar Enter).
        let should_queue = newly_opened
            || self.tabs[idx].busy
            || !self.tabs[idx].pending_enter.is_empty()
            || !self.tabs[idx].queue.is_empty()
            || self.tabs[idx].seen_output == 0;
        if should_queue {
            self.tabs[idx].queue.push(
                Instant::now(),
                Queued { request: 0, prompt: prompt.clone(), from: "você".to_owned(), answer: false },
            );
        } else {
            self.submit(idx, &prompt);
        }
        let label = self.tabs[idx].label.clone();
        self.add_feed(FeedEntry::new(FeedKind::User, Some("você"), Some(&id), prompt));
        self.github.notice = Some((format!("Enviado para {label}."), false));
        self.github.agent_menu = None;
    }
}

// ---------------------------------------------------------------------------
// Dados fictícios para capturas de tela (`COMMANDDECK_DEBUG_UI=github-demo`)
// ---------------------------------------------------------------------------

pub fn demo_overview() -> GithubOverview {
    let repo = RepoSlug { owner: "pattr".into(), repo: "commanddeck".into() };
    GithubOverview {
        repo,
        authenticated: true,
        rate_limit: Some(RateLimitInfo { limit: 5000, remaining: 4892, reset_epoch_s: 0 }),
        pulls: vec![
            PullRequest {
                number: 42,
                title: "feat(github): integração com PRs, issues e Actions".into(),
                author: "pattr".into(),
                head_ref: "feat/github".into(),
                base_ref: "dev".into(),
                head_sha: "61621aa9c4f01b2e3d4a5f60718293a4b5c6d7e8".into(),
                draft: false,
                html_url: "https://github.com/pattr/commanddeck/pull/42".into(),
                updated_at: "2026-10-16T03:30:00Z".into(),
                body: "Adiciona a aba GitHub na barra direita com PRs, issues, Actions e envio direto para agentes.".into(),
                checks: CheckStatus::Failure,
            },
            PullRequest {
                number: 39,
                title: "feat(git): histórico de commits, stash e seletor de branch".into(),
                author: "ana-dev".into(),
                head_ref: "feat/git-panel".into(),
                base_ref: "dev".into(),
                head_sha: "89ab12cd34ef56ab78cd90ef12ab34cd56ef7890".into(),
                draft: false,
                html_url: "https://github.com/pattr/commanddeck/pull/39".into(),
                updated_at: "2026-10-16T02:15:00Z".into(),
                body: "Expande a aba Git com histórico recente e troca rápida de branch.".into(),
                checks: CheckStatus::Success,
            },
            PullRequest {
                number: 44,
                title: "wip(terminal): busca incremental no scrollback (Ctrl+F)".into(),
                author: "carlos".into(),
                head_ref: "feat/term-search".into(),
                base_ref: "dev".into(),
                head_sha: "11223344556677889900aabbccddeeff00112233".into(),
                draft: true,
                html_url: "https://github.com/pattr/commanddeck/pull/44".into(),
                updated_at: "2026-10-16T01:40:00Z".into(),
                body: "Rascunho inicial da barra de busca no terminal.".into(),
                checks: CheckStatus::Pending,
            },
        ],
        issues: vec![
            Issue {
                number: 38,
                title: "Atalho Ctrl+Shift+G para alternar entre Git e GitHub".into(),
                author: "pattr".into(),
                labels: vec!["enhancement".into(), "ui".into()],
                comments: 3,
                html_url: "https://github.com/pattr/commanddeck/issues/38".into(),
                updated_at: "2026-10-15T21:10:00Z".into(),
                body: "Seria útil ter atalhos de teclado dedicados para abrir as abas da barra direita rapidamente.".into(),
            },
            Issue {
                number: 31,
                title: "ConPTY no Windows 11 24H2: cursor pisca ao redimensionar rápido".into(),
                author: "marcos".into(),
                labels: vec!["bug".into(), "windows".into()],
                comments: 5,
                html_url: "https://github.com/pattr/commanddeck/issues/31".into(),
                updated_at: "2026-10-14T18:05:00Z".into(),
                body: "Ao arrastar o divisor da barra direita rapidamente, o ConPTY emite sequências DECSCUSR extras.".into(),
            },
        ],
        runs: vec![
            WorkflowRun {
                id: 9001,
                name: "CI".into(),
                display_title: "feat(github): integração com PRs, issues e Actions".into(),
                head_branch: "feat/github".into(),
                head_sha: "61621aa9c4f01b2e3d4a5f60718293a4b5c6d7e8".into(),
                event: "pull_request".into(),
                status: CheckStatus::Failure,
                html_url: "https://github.com/pattr/commanddeck/actions/runs/9001".into(),
                created_at: "2026-10-16T03:28:00Z".into(),
                run_number: 184,
            },
            WorkflowRun {
                id: 9000,
                name: "CI".into(),
                display_title: "feat(git): histórico de commits, stash e seletor de branch".into(),
                head_branch: "feat/git-panel".into(),
                head_sha: "89ab12cd34ef56ab78cd90ef12ab34cd56ef7890".into(),
                event: "pull_request".into(),
                status: CheckStatus::Success,
                html_url: "https://github.com/pattr/commanddeck/actions/runs/9000".into(),
                created_at: "2026-10-16T02:12:00Z".into(),
                run_number: 183,
            },
            WorkflowRun {
                id: 8999,
                name: "CI".into(),
                display_title: "wip(terminal): busca incremental no scrollback".into(),
                head_branch: "feat/term-search".into(),
                head_sha: "11223344556677889900aabbccddeeff00112233".into(),
                event: "pull_request".into(),
                status: CheckStatus::Pending,
                html_url: "https://github.com/pattr/commanddeck/actions/runs/8999".into(),
                created_at: "2026-10-16T01:39:00Z".into(),
                run_number: 182,
            },
        ],
    }
}

pub fn demo_pr_detail() -> PrChecksDetail {
    PrChecksDetail {
        pr_number: 42,
        head_sha: "61621aa9c4f01b2e3d4a5f60718293a4b5c6d7e8".into(),
        status: CheckStatus::Failure,
        runs: vec![demo_overview().runs[0].clone()],
        jobs: vec![
            WorkflowJob {
                id: 501,
                run_id: 9001,
                name: "cargo clippy (windows-latest)".into(),
                status: CheckStatus::Failure,
                html_url: "https://github.com/pattr/commanddeck/actions/runs/9001/job/501".into(),
                failed_steps: vec!["Run cargo clippy --workspace -- -D warnings".into()],
            },
            WorkflowJob {
                id: 502,
                run_id: 9001,
                name: "cargo test (windows-latest)".into(),
                status: CheckStatus::Success,
                html_url: "https://github.com/pattr/commanddeck/actions/runs/9001/job/502".into(),
                failed_steps: vec![],
            },
            WorkflowJob {
                id: 503,
                run_id: 9001,
                name: "cargo fmt --check".into(),
                status: CheckStatus::Success,
                html_url: "https://github.com/pattr/commanddeck/actions/runs/9001/job/503".into(),
                failed_steps: vec![],
            },
        ],
    }
}

pub fn demo_job_log() -> JobLog {
    JobLog {
        job_id: 501,
        job_name: "cargo clippy (windows-latest)".into(),
        truncated: true,
        tail: concat!(
            "Checking commanddeck-core v0.3.0 (C:\\work\\commanddeck\\crates\\core)\n",
            "Checking commanddeck v0.3.0 (C:\\work\\commanddeck\\crates\\app)\n",
            "error: redundant closure\n",
            "  --> crates/core/src/github.rs:312:44\n",
            "   |\n",
            "312|     let slug = url.and_then(|u| parse_github_remote(&u));\n",
            "   |                             ^^^^^^^^^^^^^^^^^^^^^^^^^^^ help: replace the closure with the function itself: `parse_github_remote`\n",
            "   |\n",
            "   = note: `-D clippy::redundant-closure` implied by `-D warnings`\n",
            "error: could not compile `commanddeck-core` (lib) due to 1 previous error\n",
            "##[error]Process completed with exit code 101."
        )
        .into(),
    }
}
