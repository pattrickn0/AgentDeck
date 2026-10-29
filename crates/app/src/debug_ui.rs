//! Fake interface states for screenshots (`HOLT_DEBUG_UI`) and visual checks. DEBUG ONLY.

use crate::{deck::Deck, theme};

/// DEBUG ONLY: fake projects in the sidebar (`HOLT_DEBUG_UI=projects-demo`): one active with three agents (one busy), another with one, another empty.
fn debug_projects() -> Vec<crate::deck::ProjectView> {
    use crate::deck::{AgentBadge, ProjectView};
    let agent = |id: &str, parent: Option<&str>, kind: &str, label: &str, color, busy| AgentBadge {
        id: id.into(),
        parent: parent.map(Into::into),
        kind: kind.into(),
        label: label.into(),
        model: match id {
            "claude@dev" => Some("sonnet".into()),
            "claude@review" => Some("opus".into()),
            "antigravity@sidebar" => Some("claude-sonnet-5-5-medium".into()),
            _ => None,
        },
        color,
        busy,
        done: false,
        alive: true,
        tab: None,
    };
    let project = |id: &str, name: &str, active, agents| ProjectView {
        id: id.into(),
        name: name.into(),
        path: format!(r"C:\Users\you\{name}").into(),
        active,
        expanded: true,
        agents,
    };
    vec![
        project(
            "p1",
            "holt-dev",
            true,
            vec![
                agent("claude", None, "claude", "Claude", theme::CLAUDE, true),
                agent("claude@dev", Some("claude"), "claude", "Claude · Dev", theme::CLAUDE, false),
                agent("codex", Some("claude@dev"), "codex", "Codex", theme::SHELL, false),
                agent("antigravity@sidebar", Some("claude"), "antigravity", "Antigravity · Sidebar", theme::ANTIGRAVITY, false),
                agent("claude@review", None, "claude", "Claude · Review", theme::CLAUDE, false),
            ],
        ),
        project("p2", "jwic", false, vec![agent("claude", None, "claude", "Claude", theme::CLAUDE, false)]),
        project("p3", "new-site", false, vec![]),
    ]
}

/// DEBUG ONLY: fake usage in the title bar (`HOLT_DEBUG_UI=usage-demo` and `usage-panel`): Claude
/// in the middle of the session, Antigravity with the week in warning (and the three model groups), Codex with the session exhausted.
fn debug_usage() -> std::collections::HashMap<String, holt_core::AgentUsage> {
    use holt_core::{AgentUsage, UsageGroup, UsageWindow};
    let now = holt_core::util::now_ms();
    let w = |used, hours: u64| Some(UsageWindow { used, resets_at_ms: Some(now + hours * 3_600_000) });
    std::collections::HashMap::from([
        ("claude".into(), AgentUsage { session: w(0.34, 2), weekly: w(0.12, 100), updated_ms: now - 60_000, ..Default::default() }),
        (
            "antigravity".into(),
            AgentUsage {
                session: w(0.45, 3),
                weekly: w(0.86, 30),
                updated_ms: now - 120_000,
                local: false,
                groups: vec![
                    UsageGroup { name: "Gemini Models".into(), session: w(0.45, 3), weekly: w(0.2, 60) },
                    UsageGroup { name: "Gemini 4 Argon".into(), session: None, weekly: w(0.86, 30) },
                    UsageGroup { name: "Claude and GPT models".into(), session: w(0.0, 5), weekly: w(0.1, 150) },
                ],
            },
        ),
        (
            "codex".into(),
            AgentUsage {
                session: Some(UsageWindow { used: 1.0, resets_at_ms: Some(now + 40 * 60_000) }),
                weekly: w(0.4, 80),
                updated_ms: now - 5 * 3_600_000,
                ..Default::default()
            },
        ),
    ])
}

/// DEBUG ONLY: `HOLT_DEBUG_UI=notices,notices-open,tasks-demo,tasks-empty,tasks-detail,tasks-messages,tasks-messages-open,tasks-running,tasks-conflict,tasks-stopped,tasks-changes,tasks-request,tasks-draft,tasks-done,tasks-new-more,tasks-failed,tasks-confirm,tasks-discard,tasks-return,tasks-error,tasks-refused-main,tasks-refused-main-files,tasks-refused-commit,tasks-refused-conflict,tasks-stash-kept,tasks-busy,tasks-new,tasks-new-attach,tasks-new-effort,tasks-new-level,tasks-new-opencode,tasks-new-agent,tasks-new-model,tasks-new-provider,tasks-new-provider-models,settings,newtab,newtab-models:<agent>,drop,drop-pane:<n>,drop-task,usage-demo,usage-panel,usage-panel:<agent>,projects-demo,project:<id>,git,git-demo,git-diff,git-hunks,git-branches,git-branches-merge,git-merging,git-rebase,git-conflicts,git-conflict-file,git-history,git-commit,git-stash,git-stash-diff,git-norepo,git-nogit,split-drag,docs-demo,docs-demo-file,docs-demo-md,docs-demo-report,docs-demo-unreadable,docs-demo-many,docs-demo-badge,docs-demo-edit,docs-demo-find,docs-demo-diff-find,docs-demo-external,docs-demo-confirm,docs-demo-large,docs-demo-md-code,download,download-retry,download-error,mic-recording,mic-transcribing,mic-opening,dictation-ask` opens menus/states
/// for screenshots in smoke mode.
pub(crate) fn apply(deck: &mut Deck) {
    let Some(v) = holt_core::env_var("DEBUG_UI") else { return };
    for item in v.split(',').map(str::trim) {
        match item {
            "notices" => deck.debug_notices(),
            "notices-open" => {
                deck.debug_notices();
                deck.notices.toggle();
            }
            "settings" => deck.settings_open = true,
            "dictation-ask" => deck.dictation_prompt = true,
            "shortcuts" => deck.toggle_shortcuts = true,
            "newtab" => deck.new_tab_menu_open = true,
            // Second step of the new-agent menu (choose the model): `newtab-models:claude`, `newtab-models:opencode`…
            // and, on OpenCode, `newtab-models:opencode@opencode` opens the provider's models directly.
            k if k.starts_with("newtab-models:") => {
                let rest = &k["newtab-models:".len()..];
                let (kind, provider) = match rest.split_once('@') {
                    Some((kind, provider)) => (kind, Some(provider.to_owned())),
                    None => (rest, None),
                };
                deck.new_tab_menu_open = true;
                deck.new_tab_models_for = Some(kind.to_owned());
                deck.new_tab_provider = provider;
            }
            "drop" => deck.drop_hover = true,
            k if k.starts_with("drop-pane:") => {
                deck.drop_hover = true;
                deck.debug_drop_slot = k["drop-pane:".len()..].parse().ok();
            }
            "drop-task" => {
                debug_tasks(deck, "tasks-new-attach");
                deck.drop_hover = true;
                deck.debug_drop_task_form = true;
            }
            "git" => deck.set_right_view(crate::deck::RightView::Git),
            "git-demo" | "git-diff" | "git-branches" => {
                deck.set_right_view(crate::deck::RightView::Git);
                deck.git.debug_hold = true;
                deck.git.view = crate::git_ui::GitView::Ready(crate::git_ui::demo_status());
                deck.git.message = "Git tab in the right bar".into();
                if item == "git-diff" {
                    let entry = crate::git_ui::demo_status().entries[2].clone();
                    deck.git.diff = Some(crate::git_ui::OpenDiff { entry, staged: false, diff: Some(Ok(crate::git_ui::demo_diff())) });
                }
                if item == "git-branches" {
                    deck.git.branch_menu = true;
                    deck.git.branches = crate::git_ui::demo_branches();
                }
            }
            "git-merging" | "git-rebase" | "git-conflicts" | "git-conflict-file" | "git-history" | "git-commit" | "git-stash"
            | "git-stash-diff" | "git-branches-merge" | "git-hunks" => {
                use crate::git_ui::{self as g, GitSubView};
                deck.set_right_view(crate::deck::RightView::Git);
                deck.git.debug_hold = true;
                let mut st = g::demo_conflicts_status();
                if item == "git-rebase" {
                    st.state = holt_core::git::RepoState::Rebasing;
                    st.entries.retain(|e| e.kind != holt_core::git::EntryKind::Unmerged);
                }
                if matches!(item, "git-history" | "git-commit" | "git-stash" | "git-stash-diff" | "git-hunks" | "git-branches-merge") {
                    st = g::demo_status();
                }
                deck.git.view = g::GitView::Ready(st);
                deck.git.message.clear();
                match item {
                    "git-merging" => deck.git.subview = GitSubView::Changes,
                    "git-rebase" => deck.git.subview = GitSubView::Changes,
                    "git-conflicts" | "git-conflict-file" => {
                        deck.git.subview = GitSubView::Conflicts;
                        if item == "git-conflict-file" {
                            deck.git.conflict_open =
                                Some(g::OpenConflict { path: "crates/core/src/git.rs".into(), file: Some(Ok(g::demo_conflict_file())) });
                        }
                    }
                    "git-history" | "git-commit" => {
                        deck.git.subview = GitSubView::History;
                        deck.git.log = g::demo_log();
                        deck.git.log_has_more = true;
                        if item == "git-commit" {
                            let c = g::demo_log().remove(0);
                            deck.git.commit_detail = Some(g::OpenCommit {
                                hash: c.hash,
                                short_hash: c.short_hash,
                                subject: c.subject,
                                detail: Some(Ok(g::demo_commit_detail())),
                            });
                        }
                    }
                    "git-stash" | "git-stash-diff" => {
                        deck.git.subview = GitSubView::Stash;
                        deck.git.stashes = g::demo_stashes();
                        if item == "git-stash-diff" {
                            let stash = g::demo_stashes().remove(0);
                            deck.git.stash_diff = Some(g::OpenStashDiff { stash, diff: Some(Ok(g::demo_diff())) });
                        }
                    }
                    "git-hunks" => {
                        let entry = g::demo_status().entries[2].clone();
                        deck.git.diff = Some(g::OpenDiff { entry, staged: false, diff: Some(Ok(g::demo_hunks_diff())) });
                    }
                    "git-branches-merge" => {
                        deck.git.branch_menu = true;
                        deck.git.branch_menu_mode = g::BranchMenuMode::Merge;
                        deck.git.branches = g::demo_branches();
                    }
                    _ => {}
                }
            }
            "git-norepo" | "git-nogit" => {
                deck.set_right_view(crate::deck::RightView::Git);
                deck.git.debug_hold = true;
                deck.git.view = if item == "git-norepo" { crate::git_ui::GitView::NotRepo } else { crate::git_ui::GitView::NoGit };
            }
            "github" => deck.set_right_view(crate::deck::RightView::GitHub),
            "github-demo" | "github-issues" | "github-actions" | "github-menu" => {
                deck.github.debug_hold = true;
                deck.set_right_view(crate::deck::RightView::GitHub);
                let ov = crate::github_ui::demo_overview();
                deck.github.repo = Some(ov.repo.clone());
                deck.github.view = crate::github_ui::GithubView::Ready(ov);
                let detail = crate::github_ui::demo_pr_detail();
                let log = crate::github_ui::demo_job_log();
                deck.github.run_jobs.insert(9001, detail.jobs.clone());
                deck.github.pr_details.insert(42, detail);
                deck.github.job_logs.insert(501, Ok(log));
                match item {
                    "github-issues" => {
                        deck.github.tab = crate::github_ui::GithubTab::Issues;
                        deck.github.selected_issue = Some(38);
                    }
                    "github-actions" => {
                        deck.github.tab = crate::github_ui::GithubTab::Actions;
                        deck.github.selected_run = Some(9001);
                        deck.github.open_log = Some((501, "clippy (windows-latest)".into(), None));
                    }
                    _ => {
                        deck.github.tab = crate::github_ui::GithubTab::Prs;
                        deck.github.selected_pr = Some(42);
                        deck.github.open_log = Some((501, "clippy (windows-latest)".into(), Some(42)));
                        if item == "github-menu" {
                            let anchor = egui::Rect::from_min_size(egui::pos2(900.0, 330.0), egui::vec2(140.0, 22.0));
                            deck.open_github_agent_menu(anchor, "PR #42".into(), "review", "Review PR #42".into());
                        }
                    }
                }
            }
            "github-norepo" => {
                deck.github.debug_hold = true;
                deck.set_right_view(crate::deck::RightView::GitHub);
                deck.github.view = crate::github_ui::GithubView::Error(holt_core::github::GithubError::NoRemote);
            }
            "github-error" => {
                deck.github.debug_hold = true;
                deck.set_right_view(crate::deck::RightView::GitHub);
                deck.github.view = crate::github_ui::GithubView::Error(holt_core::github::GithubError::NotFound {
                    repo: "pattr/holt".into(),
                    authenticated: false,
                });
            }
            "tasks"
            | "tasks-demo"
            | "tasks-empty"
            | "tasks-detail"
            | "tasks-messages"
            | "tasks-messages-open"
            | "tasks-running"
            | "tasks-conflict"
            | "tasks-stopped"
            | "tasks-changes"
            | "tasks-request"
            | "tasks-draft"
            | "tasks-done"
            | "tasks-new-more"
            | "tasks-failed"
            | "tasks-confirm"
            | "tasks-discard"
            | "tasks-validating"
            | "tasks-reject"
            | "tasks-discard-validating"
            | "tasks-return"
            | "tasks-error"
            | "tasks-refused-main"
            | "tasks-refused-main-files"
            | "tasks-refused-commit"
            | "tasks-refused-conflict"
            | "tasks-stash-kept"
            | "tasks-busy"
            | "tasks-new"
            | "tasks-new-attach"
            | "tasks-new-effort"
            | "tasks-new-level"
            | "tasks-new-opencode"
            | "tasks-new-agent"
            | "tasks-new-model"
            | "tasks-new-provider"
            | "tasks-new-provider-models" => debug_tasks(deck, item),
            "docs-demo"
            | "docs-demo-file"
            | "docs-demo-md"
            | "docs-demo-report"
            | "docs-demo-unreadable"
            | "docs-demo-many"
            | "docs-demo-badge"
            | "docs-demo-edit"
            | "docs-demo-find"
            | "docs-demo-diff-find"
            | "docs-demo-external"
            | "docs-demo-confirm"
            | "docs-demo-large"
            | "docs-demo-md-code" => crate::docs_demo::apply(deck, item),
            "usage-demo" => deck.debug_usage = Some(debug_usage()),
            _ if item == "usage-panel" || item.starts_with("usage-panel:") => {
                deck.debug_usage = Some(debug_usage());
                deck.debug_usage_panel = Some(item.strip_prefix("usage-panel:").unwrap_or_default().to_owned());
            }
            "projects-demo" => deck.debug_projects = Some(debug_projects()),
            "split-drag" => {
                deck.pane_drag = Some(0);
                deck.debug_pane_drop = Some(1);
            }
            "sidebar-collapsed" => deck.sidebar_collapsed = true,
            "attach" => debug_tasks(deck, "tasks-new-attach"),
            "empty" => deck.debug_close_all = true,
            "download" | "download-retry" | "download-error" => {
                deck.download = Some((debug_download(item), None));
                deck.debug_hold_download = true;
            }
            "mic-recording" => {
                deck.mic_phase = crate::dictation::MicPhase::Recording(crate::dictation::MicTarget::Tab(1));
            }
            "mic-transcribing" => {
                deck.mic_phase = crate::dictation::MicPhase::Transcribing(crate::dictation::MicTarget::Tab(1));
            }
            "mic-opening" => {
                deck.mic_phase = crate::dictation::MicPhase::Opening(crate::dictation::MicTarget::Tab(1));
            }
            other => {
                if let Some(id) = other.strip_prefix("project:") {
                    deck.debug_switch_project = Some(id.to_owned());
                } else if let Some(dir) = other.strip_prefix("expand:") {
                    deck.tree.expanded.insert(dir.to_owned());
                } else if let Some(f) = other.strip_prefix("select:") {
                    deck.tree.active_file = Some(f.to_owned());
                }
            }
        }
    }
}

/// DEBUG ONLY: fake states of the Tasks tab (`HOLT_DEBUG_UI=tasks-*`), without git or the core.
fn debug_tasks(deck: &mut Deck, item: &str) {
    use crate::tasks_ui::{self as t, Confirm, FormMenu, TasksView};
    deck.set_right_view(crate::deck::RightView::Tasks);
    deck.tasks.debug_hold = true;
    deck.tasks.loaded = true;
    deck.tasks.tasks = if item == "tasks-empty" { Vec::new() } else { t::demo_tasks() };
    let detail = |id: &str| TasksView::Detail(id.to_owned());
    match item {
        "tasks-validating" | "tasks-reject" | "tasks-discard-validating" => {
            deck.tasks.view = detail("t8");
            match item {
                "tasks-reject" => {
                    deck.tasks.return_open = true;
                    deck.tasks.feedback = "When a merged task is opened, the header still shows \"under review\".".into();
                }
                "tasks-discard-validating" => deck.tasks.confirm = Some(Confirm::Discard("t8".into())),
                _ => {}
            }
        }
        "tasks-messages" | "tasks-messages-open" => {
            deck.tasks.view = detail("t4");
            deck.tasks.detail_tab = Some(t::DetailTab::Timeline);
            if item == "tasks-messages-open" {
                if let Some(m) = deck.tasks.task("t4").and_then(|t| t.messages.get(1).map(|m| m.at_ms)) {
                    deck.tasks.open_messages.insert(("t4".into(), m));
                }
            }
        }
        "tasks-detail"
        | "tasks-confirm"
        | "tasks-return"
        | "tasks-error"
        | "tasks-refused-main"
        | "tasks-refused-main-files"
        | "tasks-refused-commit"
        | "tasks-refused-conflict"
        | "tasks-stash-kept"
        | "tasks-busy" => {
            deck.tasks.view = detail("t4");
            match item {
                "tasks-confirm" => deck.tasks.confirm = Some(Confirm::Merge("t4".into())),
                "tasks-return" => {
                    deck.tasks.return_open = true;
                    deck.tasks.feedback = "The case of no task in the project is still missing, plus a test for the diff summary.".into();
                }
                "tasks-error" => {
                    deck.tasks.notice = Some(
                        "the main repository has uncommitted changes (crates/app/src/deck.rs, crates/app/src/app.rs): commit or stash them and try again"
                            .into(),
                    )
                }
                "tasks-refused-main" | "tasks-refused-main-files" => {
                    let files = ["crates/app/src/deck.rs", "crates/app/src/app.rs", "memory/backlog.md"].map(String::from).to_vec();
                    deck.tasks.notice = Some("the main repository has uncommitted changes".into());
                    deck.tasks.notice_block = Some(holt_core::MergeBlock::MainDirty(files));
                    deck.tasks.notice_files = item == "tasks-refused-main-files";
                }
                "tasks-refused-commit" => {
                    let files = ["crates/core/src/tasks.rs", "memory/manual-tests.md"].map(String::from).to_vec();
                    deck.tasks.notice = Some("the task's worktree has uncommitted work".into());
                    deck.tasks.notice_block = Some(holt_core::MergeBlock::TaskDirty(files));
                }
                "tasks-refused-conflict" => {
                    deck.tasks.notice = Some("conflict while merging".into());
                    deck.tasks.notice_block = Some(holt_core::MergeBlock::Conflict(vec!["crates/app/src/app.rs".into()]));
                }
                "tasks-stash-kept" => {
                    deck.tasks.notice_info = true;
                    deck.tasks.notice = Some("Your uncommitted changes are still kept in the git stash (\"holt: saved before merging task t4\"): handing them back would clash with what the task brought in crates/app/src/app.rs. Nothing was lost. To recover them: open a terminal in the project folder, run git stash list to find the entry and git stash pop to bring it back (resolving the clash).".into());
                }
                "tasks-busy" => deck.tasks.busy = Some(t::Busy { id: "t4".into(), label: "Merging…" }),
                _ => {}
            }
        }
        "tasks-discard" => {
            deck.tasks.view = detail("t6");
            deck.tasks.confirm = Some(Confirm::Discard("t6".into()));
        }
        "tasks-running" => deck.tasks.view = detail("t6"),
        "tasks-conflict" => deck.tasks.view = detail("t10"),
        "tasks-stopped" => deck.tasks.view = detail("t5"),
        "tasks-changes" => {
            deck.tasks.view = detail("t4");
            deck.tasks.detail_tab = Some(t::DetailTab::Changes);
        }
        "tasks-request" => {
            deck.tasks.view = detail("t9");
        }
        "tasks-draft" => deck.tasks.view = detail("t7"),
        "tasks-done" => deck.tasks.view = detail("t2"),
        "tasks-failed" => deck.tasks.view = detail("t3"),
        "tasks-new"
        | "tasks-new-attach"
        | "tasks-new-effort"
        | "tasks-new-level"
        | "tasks-new-opencode"
        | "tasks-new-agent"
        | "tasks-new-model"
        | "tasks-new-provider"
        | "tasks-new-provider-models"
        | "tasks-new-more" => {
            deck.tasks_open_new();
            let f = &mut deck.tasks.form;
            f.kind =
                if item.starts_with("tasks-new-provider") || item == "tasks-new-opencode" { "opencode".into() } else { "claude".into() };
            if item == "tasks-new-effort" {
                f.model = Some("sonnet".into());
                f.effort = Some(holt_core::models::Effort::High);
            }
            if item == "tasks-new-level" {
                f.kind_is_auto = false;
                f.model = Some("haiku".into());
                f.level = holt_core::capability::Level::Complex;
            }
            f.base = "dev".into();
            f.title = "Document the bridge /tasks routes".into();
            f.brief = "Explain each route in memory/ARCHITECTURE.md, with the example body and the error codes.\n\nAlso check the ask-agent --task text.".into();
            if item == "tasks-new-attach" {
                f.attachments = vec![
                    r"C:\Users\you\holt-dev\data\attachments\1a2b3c-screenshot.png".into(),
                    r"C:\Users\you\holt-dev\data\attachments\9f8e-report.pdf".into(),
                ];
            }
            f.focus_title = false;
            f.more_open = item == "tasks-new-more";
            f.menu = match item {
                "tasks-new-agent" => Some(FormMenu::Agent),
                "tasks-new-model" | "tasks-new-provider" | "tasks-new-provider-models" => Some(FormMenu::Model),
                _ => None,
            };
            if item == "tasks-new-provider-models" {
                f.provider = Some("opencode".into());
            }
            if item == "tasks-new-level" {
                deck.tasks_form_refresh();
            }
            if item.starts_with("tasks-new-provider") {
                deck.request_models("opencode");
            }
        }
        _ => {}
    }
}

/// Fake progress for screenshots of the download bar (`HOLT_DEBUG_UI`).
fn debug_download(kind: &str) -> holt_core::DownloadProgress {
    use holt_core::{DownloadProgress, DownloadState};
    let mut p = DownloadProgress {
        label: "large-v3-turbo model".into(),
        state: DownloadState::Downloading,
        received: 680_000_000,
        total: Some(1_624_555_275),
        speed: 12_900_000.0,
        attempt: 0,
        max_attempts: 0,
        retry_in_ms: 0,
        message: None,
    };
    match kind {
        "download-retry" => (p.state, p.attempt, p.max_attempts, p.retry_in_ms, p.speed) = (DownloadState::Retrying, 2, 5, 4200, 0.0),
        "download-error" => {
            (p.state, p.message) = (DownloadState::Error, Some("SHA-256 does not match (expected 1fc70f77…, got 9a3b02c1…)".into()))
        }
        _ => {}
    }
    p
}
