//! Presentation and request ordering for the host-owned skills manager.
use crate::{
    ConfirmAction, Controller, SkillAppRow, SkillListRow, SkillRepositoryRow, SkillsState, UiPage,
    i18n::UiText,
    with_controller,
    worker::{DialogKind, Work},
};
use slint::{ComponentHandle, Model, ModelRc, SharedString, TimerMode, VecModel};
use std::collections::BTreeSet;
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use toolbox_core::{
    Response,
    skills::{InstalledSkill, SkillApp, SkillDeployMode, SkillsCommand, SkillsSnapshot},
};
use toolbox_i18n::{Locale, messages};

#[derive(Default)]
pub(super) struct SkillsUiState {
    pub snapshot: Option<SkillsSnapshot>,
    pub loading: bool,
    pub submitting: bool,
    pub selected_ids: BTreeSet<String>,
    error: Option<UiText>,
    install_apps: Vec<SkillApp>,
    pending_import: Option<String>,
    discover_after_add: Option<(String, Option<String>)>,
    reveal_after_install: Option<BTreeSet<String>>,
    operation: Option<SkillsCommand>,
    operation_pending: bool,
    operation_error: Option<UiText>,
}

/// A batch always targets the explicit selection, including filtered-out rows.
/// Keep its visible scope separate from the per-application deployment state.
#[derive(Debug, PartialEq, Eq)]
struct SelectionSummary {
    total: usize,
    visible: usize,
    deployed: [usize; 5],
}

fn selection_summary(
    snapshot: &SkillsSnapshot,
    selected: &BTreeSet<String>,
    rows: &[SkillListRow],
) -> SelectionSummary {
    let selected_skills = snapshot
        .installed
        .iter()
        .filter(|skill| selected.contains(&skill.id))
        .collect::<Vec<_>>();
    SelectionSummary {
        total: selected_skills.len(),
        visible: rows
            .iter()
            .filter(|row| selected.contains(row.id.as_str()))
            .count(),
        deployed: std::array::from_fn(|index| {
            selected_skills
                .iter()
                .filter(|skill| skill.apps.contains(&SkillApp::ALL[index]))
                .count()
        }),
    }
}

fn select_visible(selected: &mut BTreeSet<String>, visible: &[String], checked: bool) {
    if checked {
        selected.extend(visible.iter().cloned());
    } else {
        for id in visible {
            selected.remove(id);
        }
    }
}

fn repository_identity(url: &str) -> String {
    url.trim()
        .trim_start_matches("https://github.com/")
        .trim_end_matches('/')
        .trim_end_matches(".git")
        .to_lowercase()
}

fn managed_path(snapshot: &SkillsSnapshot, skill: &InstalledSkill) -> String {
    if let Some(target) = skill.target.as_ref().filter(|_| skill.linked) {
        target.clone()
    } else {
        std::path::Path::new(&snapshot.storage_path)
            .join(&skill.directory)
            .display()
            .to_string()
    }
}

fn deployment_path(snapshot: &SkillsSnapshot, app: SkillApp, directory: &str) -> String {
    snapshot
        .app_paths
        .iter()
        .find(|item| item.app == app)
        .map(|item| {
            if directory.is_empty() {
                item.path.clone()
            } else {
                std::path::Path::new(&item.path)
                    .join(directory)
                    .display()
                    .to_string()
            }
        })
        .unwrap_or_default()
}

fn now_seconds() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

fn relative_time(timestamp: u64, now: u64, locale: Locale) -> String {
    let seconds = timestamp.abs_diff(now);
    match (seconds, timestamp > now) {
        (0..=59, _) => messages::skills_time_now(),
        (60..=3599, true) => messages::skills_time_minutes_future(seconds / 60),
        (60..=3599, false) => messages::skills_time_minutes_past(seconds / 60),
        (3600..=86399, true) => messages::skills_time_hours_future(seconds / 3600),
        (3600..=86399, false) => messages::skills_time_hours_past(seconds / 3600),
        (_, true) => messages::skills_time_days_future(seconds / 86400),
        (_, false) => messages::skills_time_days_past(seconds / 86400),
    }
    .render(locale)
}

fn schedule_text(snapshot: &SkillsSnapshot, now: u64, locale: Locale) -> String {
    let attempt = snapshot
        .last_attempt_at
        .map(|time| relative_time(time, now, locale))
        .unwrap_or_else(|| messages::skills_schedule_never().render(locale));
    let success = snapshot
        .last_success_at
        .map(|time| relative_time(time, now, locale))
        .unwrap_or_else(|| messages::skills_schedule_never().render(locale));
    let next = if !snapshot.daily_update {
        messages::skills_schedule_off().render(locale)
    } else {
        snapshot
            .next_update_at
            .map(|time| {
                if time <= now {
                    messages::skills_schedule_due().render(locale)
                } else {
                    relative_time(time, now, locale)
                }
            })
            .unwrap_or_else(|| messages::skills_schedule_waiting().render(locale))
    };
    messages::skills_schedule(attempt, next, success).render(locale)
}

fn repository_label(snapshot: &SkillsSnapshot, id: &str) -> String {
    snapshot
        .repositories
        .iter()
        .find(|repository| repository.id == id)
        .map(|repository| {
            repository
                .url
                .strip_prefix("https://github.com/")
                .unwrap_or(&repository.url)
                .trim_end_matches(".git")
                .to_owned()
        })
        .unwrap_or_else(|| id.into())
}

fn short_revision(revision: &str) -> &str {
    revision.get(..7).unwrap_or(revision)
}

/// Where a source chip leads: the repository page for the skill's folder,
/// the live source of a link, or the managed copy of a local import.
fn source_target(snapshot: &SkillsSnapshot, skill: &InstalledSkill) -> String {
    if let Some(id) = skill.repository_id.as_deref() {
        let Some(repository) = snapshot.repositories.iter().find(|repo| repo.id == id) else {
            return String::new();
        };
        if !repository.url.starts_with("https://") {
            return String::new();
        }
        let base = repository
            .url
            .trim_end_matches('/')
            .trim_end_matches(".git");
        let path = skill.source_path.as_deref().unwrap_or("").trim_matches('/');
        if path.is_empty() || !base.starts_with("https://github.com/") {
            return base.into();
        }
        // Open the installed commit so the page matches the "@sha" label.
        let reference = skill
            .revision
            .as_deref()
            .filter(|revision| !revision.is_empty())
            .or(repository.branch.as_deref())
            .unwrap_or("HEAD");
        return format!("{base}/tree/{reference}/{path}");
    }
    if skill.linked
        && let Some(target) = skill.target.as_deref()
    {
        return target.into();
    }
    if snapshot.storage_path.is_empty() {
        return String::new();
    }
    std::path::Path::new(&snapshot.storage_path)
        .join(&skill.directory)
        .display()
        .to_string()
}

fn directory_is_managed(snapshot: &SkillsSnapshot, directory: &str) -> bool {
    managed_skill_for_directory(snapshot, directory).is_some()
}

fn managed_skill_for_directory<'a>(
    snapshot: &'a SkillsSnapshot,
    directory: &str,
) -> Option<&'a InstalledSkill> {
    let directory = directory.to_lowercase();
    snapshot
        .installed
        .iter()
        .find(|skill| skill.directory.to_lowercase() == directory)
}

fn operation_targets_selection(operation: &SkillsCommand, view: i32, id: &str) -> bool {
    match (view, operation) {
        (
            0,
            SkillsCommand::SetEnabled { skill_id, .. }
            | SkillsCommand::Uninstall { skill_id }
            | SkillsCommand::Update {
                skill_id: Some(skill_id),
            },
        ) => skill_id == id,
        (1, SkillsCommand::Install { candidate_id, .. }) => candidate_id == id,
        (2, SkillsCommand::ImportLocal { path, .. }) => path == id,
        _ => false,
    }
}

fn installed_matches(
    skill: &InstalledSkill,
    snapshot: &SkillsSnapshot,
    status_filter: i32,
    source_filter: i32,
) -> bool {
    let enabled = !skill.apps.is_empty();
    match status_filter {
        1 if !enabled => return false,
        2 if enabled => return false,
        3 if skill.last_error.is_none() => return false,
        _ => {}
    }
    match source_filter {
        0 => true,
        1 => skill.repository_id.is_none() && !skill.linked,
        2 => skill.linked,
        _ => {
            let repository = usize::try_from(source_filter)
                .ok()
                .and_then(|index| index.checked_sub(3))
                .and_then(|index| snapshot.repositories.get(index));
            repository.is_some_and(|repository| {
                skill.repository_id.as_deref() == Some(repository.id.as_str())
            })
        }
    }
}

fn source_options(snapshot: &SkillsSnapshot, locale: Locale) -> Vec<String> {
    let mut options = vec![
        messages::common_all().render(locale),
        messages::common_local().render(locale),
        messages::skills_source_linked().render(locale),
    ];
    options.extend(
        snapshot
            .repositories
            .iter()
            .map(|repository| repository_label(snapshot, &repository.id)),
    );
    options
}

fn app_counts_text(snapshot: &SkillsSnapshot, locale: Locale) -> String {
    let counts = SkillApp::ALL
        .iter()
        .filter_map(|app| {
            let count = snapshot
                .installed
                .iter()
                .filter(|skill| skill.apps.contains(app))
                .count();
            (count > 0).then(|| messages::skills_app_count(app.label(), count).render(locale))
        })
        .collect::<Vec<_>>();
    if counts.is_empty() {
        messages::skills_no_deployments().render(locale)
    } else {
        counts.join(" · ")
    }
}

fn filtered_rows(
    snapshot: &SkillsSnapshot,
    view: i32,
    query: &str,
    locale: Locale,
    status_filter: i32,
    source_filter: i32,
) -> Vec<SkillListRow> {
    let mut rows = match view {
        0 => snapshot
            .installed
            .iter()
            .filter(|skill| installed_matches(skill, snapshot, status_filter, source_filter))
            .map(|skill| {
                let source = if skill.linked {
                    messages::skills_source_linked().render(locale)
                } else {
                    skill
                        .repository_id
                        .as_deref()
                        .map(|id| {
                            // The installed commit makes drift from upstream visible.
                            let label = repository_label(snapshot, id);
                            match skill.revision.as_deref() {
                                Some(revision) if !revision.is_empty() => {
                                    messages::skills_repository_revision(
                                        label,
                                        short_revision(revision),
                                    )
                                    .render(locale)
                                }
                                _ => label,
                            }
                        })
                        .unwrap_or_else(|| messages::common_local().render(locale))
                };
                let apps = skill
                    .apps
                    .iter()
                    .map(|app| app.label())
                    .collect::<Vec<_>>()
                    .join(", ");
                let apps = if apps.is_empty() {
                    messages::skills_not_deployed().render(locale)
                } else {
                    apps
                };
                let enabled = !skill.apps.is_empty();
                let app_hint = |app| {
                    let path = deployment_path(snapshot, app, &skill.directory);
                    if skill.apps.contains(&app) {
                        messages::skills_app_deployed(app.label(), path)
                    } else {
                        messages::skills_app_not_deployed(app.label(), path)
                    }
                    .render(locale)
                    .into()
                };
                SkillListRow {
                    id: skill.id.clone().into(),
                    name: skill.name.clone().into(),
                    description: skill.description.clone().into(),
                    detail: messages::skills_source_apps(apps, &source)
                        .render(locale)
                        .into(),
                    installed: true,
                    problem: skill.last_error.is_some(),
                    linked: skill.linked,
                    selected: false,
                    updatable: skill.repository_id.is_some(),
                    source: source.into(),
                    source_target: source_target(snapshot, skill).into(),
                    status: if enabled {
                        messages::common_enabled()
                    } else {
                        messages::common_disabled()
                    }
                    .render(locale)
                    .into(),
                    enabled,
                    app0: skill.apps.contains(&SkillApp::Claude),
                    app1: skill.apps.contains(&SkillApp::Codex),
                    app2: skill.apps.contains(&SkillApp::Gemini),
                    app3: skill.apps.contains(&SkillApp::OpenCode),
                    app4: skill.apps.contains(&SkillApp::Grok),
                    app0_hint: app_hint(SkillApp::Claude),
                    app1_hint: app_hint(SkillApp::Codex),
                    app2_hint: app_hint(SkillApp::Gemini),
                    app3_hint: app_hint(SkillApp::OpenCode),
                    app4_hint: app_hint(SkillApp::Grok),
                    deployment_count: skill.apps.len() as i32,
                }
            })
            .collect::<Vec<_>>(),
        1 => snapshot
            .available
            .iter()
            .map(|skill| SkillListRow {
                id: skill.id.clone().into(),
                name: skill.name.clone().into(),
                description: skill.description.clone().into(),
                detail: messages::skills_repository_path(
                    &skill.path,
                    repository_label(snapshot, &skill.repository_id),
                )
                .render(locale)
                .into(),
                installed: skill.installed || directory_is_managed(snapshot, &skill.directory),
                problem: false,
                linked: false,
                selected: false,
                updatable: false,
                source: "".into(),
                source_target: "".into(),
                status: "".into(),
                enabled: false,
                app0: false,
                app1: false,
                app2: false,
                app3: false,
                app4: false,
                ..SkillListRow::default()
            })
            .collect(),
        2 => snapshot
            .local
            .iter()
            .map(|skill| {
                let app = skill
                    .app
                    .map(|app| app.label().to_owned())
                    .unwrap_or_else(|| messages::skills_shared().render(locale));
                let detail = match (&skill.target, skill.linked) {
                    (Some(target), true) => messages::skills_local_link(app, &skill.path, target),
                    _ => messages::skills_local_path(app, &skill.path),
                }
                .render(locale);
                SkillListRow {
                    id: skill.path.clone().into(),
                    name: skill.name.clone().into(),
                    description: skill.description.clone().into(),
                    detail: detail.into(),
                    installed: directory_is_managed(snapshot, &skill.directory),
                    problem: false,
                    linked: skill.linked,
                    selected: false,
                    updatable: false,
                    source: "".into(),
                    source_target: "".into(),
                    status: "".into(),
                    enabled: false,
                    app0: false,
                    app1: false,
                    app2: false,
                    app3: false,
                    app4: false,
                    ..SkillListRow::default()
                }
            })
            .collect(),
        _ => Vec::new(),
    };
    let query = query.trim().to_lowercase();
    rows.retain(|row| {
        query.is_empty()
            || [&row.name, &row.description, &row.detail, &row.id]
                .iter()
                .any(|value| value.to_lowercase().contains(&query))
    });
    rows.sort_by(|left, right| {
        left.name
            .to_lowercase()
            .cmp(&right.name.to_lowercase())
            .then_with(|| left.id.cmp(&right.id))
    });
    rows
}

impl Controller {
    pub(super) fn skills_editable(&self) -> bool {
        let state = self.state.borrow();
        !state.skills.submitting
            && !state.dialog_open
            && state
                .skills
                .snapshot
                .as_ref()
                .is_some_and(|snapshot| !snapshot.busy)
    }

    pub(super) fn refresh_skills(&self) {
        {
            let mut state = self.state.borrow_mut();
            if state.skills.loading || state.skills.submitting || state.dialog_open {
                return;
            }
            state.skills.loading = true;
        }
        self.ui().global::<SkillsState>().set_loading(true);
        self.send(Work::GetSkills);
    }

    pub(super) fn schedule_skills_poll(&self) {
        self.skills_timer.stop();
        let state = self.state.borrow();
        if !state.visible || self.ui().get_page() != crate::page_index(UiPage::Skills) {
            return;
        }
        let busy = state.skills.submitting
            || state
                .skills
                .snapshot
                .as_ref()
                .is_some_and(|snapshot| snapshot.busy);
        self.skills_timer.start(
            TimerMode::SingleShot,
            Duration::from_secs(if busy { 1 } else { 15 }),
            || {
                with_controller(|controller| {
                    controller.refresh_skills();
                    controller.schedule_skills_poll();
                });
            },
        );
    }

    pub(super) fn submit_skills(&self, command: SkillsCommand) {
        if !self.skills_editable() {
            self.render_skills();
            self.say(messages::skills_wait_for_operation());
            return;
        }
        self.idle_timer.stop();
        {
            let mut state = self.state.borrow_mut();
            state.skills.submitting = true;
            state.skills.error = None;
            state.skills.operation = Some(command.clone());
            state.skills.operation_pending = true;
            state.skills.operation_error = None;
        }
        let ui = self.ui();
        let skills = ui.global::<SkillsState>();
        skills.set_submitting(true);
        skills.set_error_text("".into());
        self.render_skills();
        self.say(messages::skills_submitting());
        self.send(Work::Skills(command));
    }

    pub(super) fn receive_skills(&self, result: Result<Response, String>, submitted: bool) {
        {
            let mut state = self.state.borrow_mut();
            if submitted {
                state.skills.submitting = false;
            } else {
                state.skills.loading = false;
            }
        }
        let ui = self.ui();
        let skills = ui.global::<SkillsState>();
        skills.set_loading(self.state.borrow().skills.loading);
        skills.set_submitting(self.state.borrow().skills.submitting);
        match result.map_err(UiText::from).and_then(|response| {
            response
                .skills
                .ok_or_else(|| messages::skills_missing_state().into())
        }) {
            Ok(snapshot) => {
                let new_error = snapshot.last_error.clone().filter(|error| {
                    self.state
                        .borrow()
                        .skills
                        .snapshot
                        .as_ref()
                        .and_then(|previous| previous.last_error.as_ref())
                        != Some(error)
                });
                {
                    let mut state = self.state.borrow_mut();
                    state.skills.error = None;
                    if state.skills.operation_pending && !state.skills.submitting && !snapshot.busy
                    {
                        // Attribute a completed request's error once. A later
                        // background update must not attach its error to an old
                        // item merely because it was the last UI action.
                        state.skills.operation_error =
                            snapshot.last_error.clone().map(UiText::from);
                        state.skills.operation_pending = false;
                    }
                    state.skills.snapshot = Some(snapshot);
                    let installed = state
                        .skills
                        .snapshot
                        .as_ref()
                        .map(|snapshot| {
                            snapshot
                                .installed
                                .iter()
                                .map(|skill| skill.id.clone())
                                .collect::<BTreeSet<_>>()
                        })
                        .unwrap_or_default();
                    state
                        .skills
                        .selected_ids
                        .retain(|id| installed.contains(id));
                }
                self.render_skills();
                self.sync_diagnostics();
                if let Some(error) = new_error {
                    self.error(error);
                } else if submitted {
                    self.say(messages::skills_operation_accepted());
                }
                self.continue_skills_workflow();
            }
            Err(error) => {
                let mut state = self.state.borrow_mut();
                state.skills.error = Some(error.clone());
                if submitted {
                    state.skills.operation_error = Some(error.clone());
                    state.skills.operation_pending = false;
                }
                state.skills.discover_after_add = None;
                state.skills.reveal_after_install = None;
                drop(state);
                self.render_skills();
                self.error(error);
            }
        }
        self.schedule_skills_poll();
        // Accepted operations live in the host. They must not retain the UI.
        self.schedule_unload();
    }

    /// Follow up only after the host publishes completion, not its accepted/busy
    /// preview. Repository discovery remains an ordinary serialized host request.
    fn continue_skills_workflow(&self) {
        let mut state = self.state.borrow_mut();
        let Some(snapshot) = state.skills.snapshot.clone() else {
            return;
        };
        if snapshot.busy || state.skills.submitting {
            return;
        }
        if snapshot.last_error.is_some() {
            state.skills.discover_after_add = None;
            state.skills.reveal_after_install = None;
            return;
        }
        let added = state
            .skills
            .discover_after_add
            .take()
            .and_then(|(url, branch)| {
                snapshot
                    .repositories
                    .iter()
                    .find(|repo| {
                        repository_identity(&repo.url) == repository_identity(&url)
                            && repo.branch == branch
                    })
                    .map(|repo| repository_label(&snapshot, &repo.id))
            });
        let installed = state
            .skills
            .reveal_after_install
            .take()
            .and_then(|previous| {
                snapshot
                    .installed
                    .iter()
                    .find(|skill| !previous.contains(&skill.id))
                    .map(|skill| skill.id.clone())
            });
        if installed.is_some() {
            state.skills.pending_import = None;
        }
        drop(state);
        let ui = self.ui();
        let skills = ui.global::<SkillsState>();
        if let Some(id) = installed {
            skills.set_view(0);
            skills.set_query("".into());
            skills.set_status_filter(0);
            skills.set_source_filter(0);
            skills.set_selected_id(id.into());
            skills.set_details_visible(true);
            self.render_skills();
        }
        if let Some(query) = added {
            // Do not pull the user away from a different view chosen while the
            // request was running. Discovery itself is still the promised follow-up.
            if skills.get_view() == 3 {
                skills.set_view(1);
                skills.set_query(query.into());
            }
            skills.set_repository_url("".into());
            skills.set_repository_branch("".into());
            skills.set_details_visible(false);
            self.render_skills();
            self.submit_skills(SkillsCommand::Discover);
        }
    }

    pub(super) fn render_skills(&self) {
        let ui = self.ui();
        let skills = ui.global::<SkillsState>();
        let locale = self.locale();
        let state = self.state.borrow();
        let error = if state.skills.submitting {
            String::new()
        } else if let Some(error) = &state.skills.error {
            error.render(locale)
        } else {
            state
                .skills
                .snapshot
                .as_ref()
                .and_then(|snapshot| snapshot.last_error.clone())
                .unwrap_or_default()
        };
        skills.set_error_text(error.clone().into());
        let Some(snapshot) = &state.skills.snapshot else {
            skills.set_empty_text(messages::skills_loading().render(locale).into());
            return;
        };
        skills.set_loaded(true);
        skills.set_busy(snapshot.busy);
        skills.set_daily_update(snapshot.daily_update);
        skills.set_deploy_mode(match snapshot.deploy_mode {
            SkillDeployMode::Symlink => 0,
            SkillDeployMode::Copy => 1,
        });
        skills.set_installed_count(snapshot.installed.len().min(i32::MAX as usize) as i32);
        skills.set_storage_path(snapshot.storage_path.clone().into());
        skills.set_schedule_text(schedule_text(snapshot, now_seconds(), locale).into());
        skills.set_activity_text(snapshot.message.clone().into());
        let mut notices = snapshot.warnings.clone();
        if snapshot.truncated {
            notices.insert(0, messages::skills_catalog_truncated().render(locale));
        }
        skills.set_notice_detail(notices.join("\n").into());
        skills.set_notice_text(
            if notices.is_empty() {
                String::new()
            } else {
                messages::skills_notice_count(notices.len()).render(locale)
            }
            .into(),
        );
        let affected_skill_count = snapshot
            .installed
            .iter()
            .filter(|skill| skill.last_error.is_some())
            .count();
        let problem_count = affected_skill_count
            + snapshot
                .repositories
                .iter()
                .filter(|repo| repo.last_error.is_some())
                .count();
        skills.set_problem_count(problem_count as i32);
        skills.set_affected_skill_count(affected_skill_count as i32);
        skills.set_result_summary(
            messages::skills_overview(snapshot.installed.len(), problem_count)
                .render(locale)
                .into(),
        );
        let result_title = if state.skills.submitting {
            messages::skills_request_sending()
        } else if snapshot.busy {
            match state.skills.operation.as_ref() {
                Some(SkillsCommand::Update { .. }) => messages::skills_updating(),
                Some(SkillsCommand::Discover) => messages::skills_discovering(),
                Some(SkillsCommand::Install { .. } | SkillsCommand::ImportLocal { .. }) => {
                    messages::skills_installing()
                }
                _ => messages::skills_background_work(),
            }
        } else if !error.is_empty() || problem_count > 0 {
            messages::skills_attention()
        } else if state.skills.operation.is_some() {
            messages::skills_operation_finished()
        } else {
            messages::skills_ready()
        };
        skills.set_result_title(result_title.render(locale).into());
        let mut activity_detail = vec![snapshot.message.clone()];
        if !error.is_empty() {
            activity_detail.push(error);
        }
        for skill in &snapshot.installed {
            if let Some(error) = &skill.last_error {
                activity_detail.push(format!(
                    "{}\n{}\n{}",
                    skill.name,
                    managed_path(snapshot, skill),
                    error
                ));
            }
        }
        for repository in &snapshot.repositories {
            if let Some(error) = &repository.last_error {
                activity_detail.push(format!("{}\n{}", repository.url, error));
            }
        }
        activity_detail.extend(notices);
        skills.set_activity_detail(
            activity_detail
                .into_iter()
                .filter(|line| !line.is_empty())
                .collect::<Vec<_>>()
                .join("\n\n")
                .into(),
        );
        let repositories = snapshot
            .repositories
            .iter()
            .map(|repository| {
                let count = snapshot
                    .installed
                    .iter()
                    .filter(|skill| skill.repository_id.as_deref() == Some(&repository.id))
                    .count();
                let branch = repository
                    .branch
                    .clone()
                    .unwrap_or_else(|| messages::skills_default_branch().render(locale));
                SkillRepositoryRow {
                    id: repository.id.clone().into(),
                    url: repository.url.clone().into(),
                    detail: messages::skills_repository_installed(count, branch)
                        .render(locale)
                        .into(),
                    error: repository.last_error.clone().unwrap_or_default().into(),
                    removable: count == 0,
                }
            })
            .collect::<Vec<_>>();
        if !skills
            .get_repositories()
            .iter()
            .eq(repositories.iter().cloned())
        {
            skills.set_repositories(ModelRc::new(VecModel::from(repositories)));
        }
        let view = skills.get_view();
        let options = source_options(snapshot, locale)
            .into_iter()
            .map(SharedString::from)
            .collect::<Vec<_>>();
        let status_filter = skills.get_status_filter().clamp(0, 3);
        let source_filter = skills
            .get_source_filter()
            .clamp(0, options.len().saturating_sub(1) as i32);
        skills.set_status_filter(status_filter);
        skills.set_source_filter(source_filter);
        if !skills
            .get_source_options()
            .iter()
            .eq(options.iter().cloned())
        {
            skills.set_source_options(ModelRc::new(VecModel::from(options)));
        }
        skills.set_app_counts_text(app_counts_text(snapshot, locale).into());
        let mut rows = filtered_rows(
            snapshot,
            view,
            &skills.get_query(),
            locale,
            status_filter,
            source_filter,
        );
        for row in &mut rows {
            row.selected = state.skills.selected_ids.contains(row.id.as_str());
        }
        let selection = selection_summary(snapshot, &state.skills.selected_ids, &rows);
        skills.set_all_selected(!rows.is_empty() && selection.visible == rows.len());
        skills.set_selected_count(selection.total as i32);
        skills.set_visible_selected_count(selection.visible as i32);
        skills.set_hidden_selected_count(selection.total.saturating_sub(selection.visible) as i32);
        skills.set_selection_text(
            messages::skills_selection_scope(
                selection.total.saturating_sub(selection.visible),
                selection.total,
            )
            .render(locale)
            .into(),
        );
        let batch_target = skills.get_batch_target().clamp(0, 4) as usize;
        skills.set_batch_target(batch_target as i32);
        skills.set_batch_deployment_text(
            messages::skills_batch_deployment(
                SkillApp::ALL[batch_target].label(),
                selection.deployed[batch_target],
                selection.total,
            )
            .render(locale)
            .into(),
        );
        // Preserve the boolean model for clients of the original UI API. The UI now
        // shows explicit counts and actions, so mixed is no longer displayed as off.
        let fully_deployed =
            |index| selection.total > 0 && selection.deployed[index] == selection.total;
        skills.set_batch_app0(fully_deployed(0));
        skills.set_batch_app1(fully_deployed(1));
        skills.set_batch_app2(fully_deployed(2));
        skills.set_batch_app3(fully_deployed(3));
        skills.set_batch_app4(fully_deployed(4));
        let previous_id = skills.get_selected_id();
        let selected = rows
            .iter()
            .find(|row| row.id == previous_id)
            .or_else(|| rows.first());
        let pending_import = state.skills.pending_import.as_deref();
        let selected_id = pending_import
            .map(SharedString::from)
            .unwrap_or_else(|| selected.map(|row| row.id.clone()).unwrap_or_default());
        skills.set_selected_index(
            rows.iter()
                .position(|row| row.id == selected_id)
                .map(|index| index as i32)
                .unwrap_or(-1),
        );
        if skills.get_details_visible() && pending_import.is_none() && previous_id != selected_id {
            skills.set_details_visible(false);
        }
        skills.set_selected_id(selected_id.clone());
        skills.set_selected_name(selected.map(|row| row.name.clone()).unwrap_or_default());
        skills.set_selected_description(
            selected
                .map(|row| row.description.clone())
                .unwrap_or_default(),
        );
        skills.set_selected_source(selected.map(|row| row.detail.clone()).unwrap_or_default());
        skills.set_selected_installed(selected.is_some_and(|row| row.installed));
        skills.set_selected_linked(selected.is_some_and(|row| row.linked));
        skills.set_selected_detail("".into());
        skills.set_selected_error("".into());
        skills.set_selected_summary("".into());
        skills.set_selected_master_path("".into());
        skills.set_selected_source_target("".into());
        skills.set_selected_managed_id("".into());
        skills.set_selected_updatable(false);
        skills.set_import_from_folder(pending_import.is_some());
        skills.set_import_path(pending_import.unwrap_or_default().into());
        let installed = (view == 0 && pending_import.is_none())
            .then(|| {
                snapshot
                    .installed
                    .iter()
                    .find(|skill| skill.id == selected_id.as_str())
            })
            .flatten();
        let mut directory = String::new();
        if let Some(installed) = installed {
            directory = installed.directory.clone();
            let revision = installed.revision.as_deref().unwrap_or("—");
            let updated = relative_time(installed.updated_at, now_seconds(), locale);
            let path = managed_path(snapshot, installed);
            skills.set_selected_detail(
                messages::skills_detail(&directory, revision, &updated)
                    .render(locale)
                    .into(),
            );
            skills.set_selected_summary(
                messages::skills_selected_summary(&directory, revision, &updated)
                    .render(locale)
                    .into(),
            );
            skills.set_selected_master_path(path.into());
            skills.set_selected_source_target(source_target(snapshot, installed).into());
            skills.set_selected_updatable(installed.repository_id.is_some());
            skills.set_selected_error(installed.last_error.clone().unwrap_or_default().into());
        } else if let Some(path) = pending_import {
            skills.set_selected_name(messages::skills_review_local_import().render(locale).into());
            skills.set_selected_description("".into());
            skills.set_selected_source(path.into());
            skills.set_selected_installed(false);
            skills.set_selected_linked(false);
            skills.set_selected_master_path(path.into());
            skills.set_selected_source_target(path.into());
        } else if view == 1 {
            if let Some(candidate) = snapshot
                .available
                .iter()
                .find(|skill| skill.id == selected_id.as_str())
            {
                directory = candidate.directory.clone();
                if let Some(managed) = managed_skill_for_directory(snapshot, &directory) {
                    skills.set_selected_managed_id(managed.id.clone().into());
                }
                if let Some(repository) = snapshot
                    .repositories
                    .iter()
                    .find(|repo| repo.id == candidate.repository_id)
                {
                    let reference = repository
                        .revision
                        .as_deref()
                        .or(repository.branch.as_deref())
                        .unwrap_or("HEAD");
                    skills.set_selected_source_target(
                        format!(
                            "{}/tree/{}/{}",
                            repository.url.trim_end_matches(".git"),
                            reference,
                            candidate.path.trim_matches('/')
                        )
                        .into(),
                    );
                }
            }
        } else if view == 2
            && let Some(local) = snapshot
                .local
                .iter()
                .find(|skill| skill.path == selected_id.as_str())
        {
            if let Some(managed) = managed_skill_for_directory(snapshot, &local.directory) {
                skills.set_selected_managed_id(managed.id.clone().into());
            }
            // App paths are base directories until the host resolves the local
            // source, including junction targets, at import time.
            skills.set_selected_master_path(
                local.target.as_ref().unwrap_or(&local.path).clone().into(),
            );
            skills.set_selected_source_target(local.path.clone().into());
        }
        if skills.get_selected_error().is_empty()
            && state
                .skills
                .operation
                .as_ref()
                .is_some_and(|operation| operation_targets_selection(operation, view, &selected_id))
            && let Some(error) = &state.skills.operation_error
        {
            skills.set_selected_error(error.render(locale).into());
        }
        let enabled_apps = installed
            .map(|skill| skill.apps.as_slice())
            .unwrap_or(&state.skills.install_apps);
        let apps = SkillApp::ALL
            .iter()
            .enumerate()
            .map(|(index, app)| SkillAppRow {
                index: index as i32,
                name: app.label().into(),
                path: deployment_path(snapshot, *app, &directory).into(),
                enabled: enabled_apps.contains(app),
            })
            .collect::<Vec<_>>();
        if !skills.get_apps().iter().eq(apps.iter().cloned()) {
            skills.set_apps(ModelRc::new(VecModel::from(apps)));
        }
        if !skills.get_rows().iter().eq(rows.iter().cloned()) {
            skills.set_rows(ModelRc::new(VecModel::from(rows)));
        }
        let filtered = !skills.get_query().trim().is_empty()
            || (view == 0 && (status_filter != 0 || source_filter != 0));
        skills.set_filtered(filtered);
        let empty = match (filtered, view) {
            (true, _) => messages::skills_empty_matches(),
            (false, 0) => messages::skills_empty_managed(),
            (false, 1) if snapshot.repositories.is_empty() => messages::skills_empty_sources(),
            (false, 1) => messages::skills_empty_discovery(),
            (false, _) => messages::skills_empty_local(),
        };
        skills.set_empty_text(empty.render(locale).into());
    }

    pub(super) fn skills_view(&self, view: i32) {
        let ui = self.ui();
        let skills = ui.global::<SkillsState>();
        if !(0..=3).contains(&view) || skills.get_view() == view {
            return;
        }
        skills.set_view(view);
        skills.set_selected_id("".into());
        skills.set_details_visible(false);
        skills.set_activity_visible(false);
        {
            let mut state = self.state.borrow_mut();
            state.skills.pending_import = None;
            state.skills.reveal_after_install = None;
        }
        self.render_skills();
    }

    pub(super) fn set_skill_deploy_mode(&self, index: i32) {
        let mode = match index {
            0 => SkillDeployMode::Symlink,
            1 => SkillDeployMode::Copy,
            _ => {
                self.render_skills();
                return;
            }
        };
        if !self.skills_editable() {
            self.render_skills();
            return;
        }
        if self
            .state
            .borrow()
            .skills
            .snapshot
            .as_ref()
            .is_some_and(|snapshot| snapshot.deploy_mode == mode)
        {
            return;
        }
        // The host changes the default only. Never synthesize disable/enable
        // operations or rewrite the user's local source-import choice here.
        self.submit_skills(SkillsCommand::SetDeployMode { mode });
    }

    pub(super) fn select_skill(&self, id: String) {
        self.ui().global::<SkillsState>().set_selected_id(id.into());
        self.render_skills();
    }

    pub(super) fn open_skill_details(&self, id: String) {
        if !self
            .ui()
            .global::<SkillsState>()
            .get_rows()
            .iter()
            .any(|row| row.id.as_str() == id)
        {
            return;
        }
        {
            let mut state = self.state.borrow_mut();
            state.skills.pending_import = None;
            state.skills.reveal_after_install = None;
        }
        self.select_skill(id);
        let ui = self.ui();
        let skills = ui.global::<SkillsState>();
        skills.set_details_visible(true);
        skills.set_activity_visible(false);
    }

    pub(super) fn show_managed_skill(&self) {
        let ui = self.ui();
        let skills = ui.global::<SkillsState>();
        let id = skills.get_selected_managed_id().to_string();
        if id.is_empty()
            || !self
                .state
                .borrow()
                .skills
                .snapshot
                .as_ref()
                .is_some_and(|snapshot| snapshot.installed.iter().any(|skill| skill.id == id))
        {
            return;
        }
        {
            let mut state = self.state.borrow_mut();
            state.skills.pending_import = None;
            state.skills.reveal_after_install = None;
        }
        skills.set_view(0);
        skills.set_query("".into());
        skills.set_status_filter(0);
        skills.set_source_filter(0);
        skills.set_selected_id(id.into());
        skills.set_details_visible(true);
        skills.set_activity_visible(false);
        self.render_skills();
    }

    pub(super) fn close_skill_details(&self) {
        let ui = self.ui();
        let skills = ui.global::<SkillsState>();
        skills.set_details_visible(false);
        {
            let mut state = self.state.borrow_mut();
            state.skills.pending_import = None;
            state.skills.reveal_after_install = None;
        }
        self.render_skills();
    }

    pub(super) fn move_skill_selection(&self, delta: i32) {
        let ui = self.ui();
        let skills = ui.global::<SkillsState>();
        let rows = skills.get_rows();
        if rows.row_count() == 0 {
            return;
        }
        let current = rows
            .iter()
            .position(|row| row.id == skills.get_selected_id())
            .unwrap_or(0) as i32;
        let index = current
            .saturating_add(delta)
            .clamp(0, rows.row_count() as i32 - 1) as usize;
        if let Some(row) = rows.row_data(index) {
            self.select_skill(row.id.into());
        }
    }

    pub(super) fn clear_skill_selection(&self) {
        self.state.borrow_mut().skills.selected_ids.clear();
        self.render_skills();
    }

    fn clear_skill_filters(&self) {
        let ui = self.ui();
        let skills = ui.global::<SkillsState>();
        skills.set_query("".into());
        skills.set_status_filter(0);
        skills.set_source_filter(0);
        self.render_skills();
    }

    fn show_skill_problems(&self) {
        self.skills_view(0);
        self.clear_skill_filters();
        let ui = self.ui();
        let skills = ui.global::<SkillsState>();
        skills.set_activity_visible(false);
        skills.set_details_visible(false);
        skills.set_status_filter(3);
        self.render_skills();
    }

    fn browse_skill_repository(&self, id: String) {
        let snapshot = self.state.borrow().skills.snapshot.clone();
        let Some(snapshot) = snapshot else {
            return;
        };
        if !snapshot.repositories.iter().any(|repo| repo.id == id) {
            return;
        }
        self.skills_view(1);
        self.ui()
            .global::<SkillsState>()
            .set_query(repository_label(&snapshot, &id).into());
        self.render_skills();
        if !snapshot
            .available
            .iter()
            .any(|skill| skill.repository_id == id)
        {
            self.submit_skills(SkillsCommand::Discover);
        }
    }

    pub(super) fn refresh_skill_view(&self) {
        match self.ui().global::<SkillsState>().get_view() {
            1 | 3 => self.submit_skills(SkillsCommand::Discover),
            2 => self.submit_skills(SkillsCommand::ScanLocal),
            _ => self.refresh_skills(),
        }
    }

    pub(super) fn add_skill_repository(&self) {
        let ui = self.ui();
        let skills = ui.global::<SkillsState>();
        let url = skills.get_repository_url().trim().to_owned();
        if url.is_empty() {
            self.error(messages::skills_enter_repository());
            return;
        }
        let branch = skills.get_repository_branch().trim().to_owned();
        if !self.skills_editable() {
            return;
        }
        let branch = (!branch.is_empty()).then_some(branch);
        self.state.borrow_mut().skills.discover_after_add = Some((url.clone(), branch.clone()));
        self.submit_skills(SkillsCommand::AddRepository { url, branch });
    }

    pub(super) fn toggle_skill_app(&self, index: i32, enabled: bool) {
        let Some(app) = usize::try_from(index)
            .ok()
            .and_then(|index| SkillApp::ALL.get(index))
            .copied()
        else {
            return;
        };
        if !self.skills_editable() {
            self.render_skills();
            return;
        }
        let ui = self.ui();
        let skills = ui.global::<SkillsState>();
        if skills.get_view() == 0 {
            let id = skills.get_selected_id().to_string();
            if !id.is_empty() {
                self.submit_skills(SkillsCommand::SetEnabled {
                    skill_id: id,
                    app,
                    enabled,
                });
            }
        } else {
            let mut state = self.state.borrow_mut();
            state.skills.install_apps.retain(|value| *value != app);
            if enabled {
                state.skills.install_apps.push(app);
            }
            drop(state);
            self.render_skills();
        }
    }

    pub(super) fn toggle_skill_row_app(&self, id: String, index: i32, enabled: bool) {
        let Some(app) = usize::try_from(index)
            .ok()
            .and_then(|index| SkillApp::ALL.get(index))
            .copied()
        else {
            return;
        };
        if !self.skills_editable() {
            self.render_skills();
            return;
        }
        let ui = self.ui();
        let skills = ui.global::<SkillsState>();
        if skills.get_view() != 0
            || !skills
                .get_rows()
                .iter()
                .any(|row| row.id.as_str() == id.as_str())
        {
            return;
        }
        skills.set_selected_id(id.clone().into());
        self.render_skills();
        self.submit_skills(SkillsCommand::SetEnabled {
            skill_id: id,
            app,
            enabled,
        });
    }

    pub(super) fn toggle_skill_selection(&self, id: String, selected: bool) {
        let mut state = self.state.borrow_mut();
        if selected {
            state.skills.selected_ids.insert(id);
        } else {
            state.skills.selected_ids.remove(&id);
        }
        drop(state);
        self.render_skills();
    }

    pub(super) fn toggle_skill_selection_all(&self, selected: bool) {
        let ids = self
            .ui()
            .global::<SkillsState>()
            .get_rows()
            .iter()
            .map(|row| row.id.to_string())
            .collect::<Vec<_>>();
        let mut state = self.state.borrow_mut();
        select_visible(&mut state.skills.selected_ids, &ids, selected);
        drop(state);
        self.render_skills();
    }

    pub(super) fn batch_set_skill_app(&self, index: i32, enabled: bool) {
        let Some(app) = usize::try_from(index)
            .ok()
            .and_then(|index| SkillApp::ALL.get(index))
            .copied()
        else {
            return;
        };
        let ids = self
            .state
            .borrow()
            .skills
            .selected_ids
            .iter()
            .cloned()
            .collect::<Vec<_>>();
        if ids.is_empty() {
            return;
        }
        if !self.skills_editable() {
            self.render_skills();
            return;
        }
        self.submit_skills(SkillsCommand::SetEnabledBatch {
            skill_ids: ids,
            app,
            enabled,
        });
    }

    pub(super) fn install_selected_skill(&self) {
        let ui = self.ui();
        let skills = ui.global::<SkillsState>();
        let id = skills.get_selected_id().to_string();
        if id.is_empty() || skills.get_selected_installed() || !self.skills_editable() {
            return;
        }
        let apps = self.state.borrow().skills.install_apps.clone();
        let pending_import = self.state.borrow().skills.pending_import.clone();
        let command = match (pending_import, skills.get_view()) {
            (Some(path), _) => SkillsCommand::ImportLocal {
                path,
                apps,
                link: skills.get_link_import(),
            },
            (None, 1) => SkillsCommand::Install {
                candidate_id: id,
                apps,
            },
            (None, 2) => SkillsCommand::ImportLocal {
                path: id,
                apps,
                link: skills.get_link_import(),
            },
            _ => return,
        };
        let installed = self
            .state
            .borrow()
            .skills
            .snapshot
            .as_ref()
            .map(|snapshot| {
                snapshot
                    .installed
                    .iter()
                    .map(|skill| skill.id.clone())
                    .collect()
            });
        self.state.borrow_mut().skills.reveal_after_install = installed;
        self.submit_skills(command);
    }

    pub(super) fn update_selected_skill(&self) {
        let ui = self.ui();
        let skills = ui.global::<SkillsState>();
        if skills.get_view() == 0 && skills.get_selected_updatable() {
            self.submit_skills(SkillsCommand::Update {
                skill_id: Some(skills.get_selected_id().to_string()),
            });
        }
    }

    pub(super) fn uninstall_selected_skill(&self) {
        let ui = self.ui();
        let skills = ui.global::<SkillsState>();
        if !self.skills_editable() || skills.get_view() != 0 || skills.get_selected_id().is_empty()
        {
            return;
        }
        self.ask(
            ConfirmAction::UninstallSkill(skills.get_selected_id().to_string()),
            messages::skills_uninstall_title(skills.get_selected_name().to_string()),
            messages::skills_uninstall_body(),
        );
    }

    pub(super) fn import_skill_folder(&self, path: Result<Option<String>, UiText>) {
        self.state.borrow_mut().dialog_open = false;
        self.ui().set_dialog_busy(false);
        match path {
            Ok(Some(path)) => {
                self.state.borrow_mut().skills.pending_import = Some(path);
                let ui = self.ui();
                let skills = ui.global::<SkillsState>();
                skills.set_view(2);
                skills.set_details_visible(true);
                skills.set_activity_visible(false);
                self.render_skills();
                self.ui().invoke_focus_page();
            }
            Ok(None) => {}
            Err(error) => self.error(error),
        }
        self.schedule_skills_poll();
        self.schedule_unload();
    }
}

pub(super) fn connect_callbacks(ui: &crate::AppWindow) {
    let skills = ui.global::<SkillsState>();
    skills.on_change_view(|view| with_controller(|controller| controller.skills_view(view)));
    skills.on_search(|query| {
        with_controller(|controller| {
            controller.ui().global::<SkillsState>().set_query(query);
            controller.render_skills();
        });
    });
    skills.on_select(|id| with_controller(|controller| controller.select_skill(id.into())));
    skills.on_open_details(|id| {
        with_controller(|controller| controller.open_skill_details(id.into()))
    });
    skills.on_close_details(|| with_controller(Controller::close_skill_details));
    skills.on_show_managed(|| with_controller(Controller::show_managed_skill));
    skills.on_move_selection(|delta| {
        with_controller(|controller| controller.move_skill_selection(delta))
    });
    skills.on_clear_selection(|| with_controller(Controller::clear_skill_selection));
    skills.on_clear_filters(|| with_controller(Controller::clear_skill_filters));
    skills.on_show_problems(|| with_controller(Controller::show_skill_problems));
    skills.on_browse_repository(|id| {
        with_controller(|controller| controller.browse_skill_repository(id.into()))
    });
    skills.on_refresh(|| with_controller(Controller::refresh_skill_view));
    skills.on_update_all(|| {
        with_controller(|controller| {
            controller.submit_skills(SkillsCommand::Update { skill_id: None })
        });
    });
    skills.on_set_daily(|enabled| {
        with_controller(|controller| {
            controller.submit_skills(SkillsCommand::SetDailyUpdate { enabled })
        });
    });
    skills.on_set_deploy_mode(|mode| {
        with_controller(|controller| controller.set_skill_deploy_mode(mode));
    });
    skills.on_add_repository(|| with_controller(Controller::add_skill_repository));
    skills.on_remove_repository(|id| {
        with_controller(|controller| {
            controller.submit_skills(SkillsCommand::RemoveRepository { id: id.into() })
        });
    });
    skills.on_import_folder(|| {
        with_controller(|controller| {
            if controller.skills_editable() {
                controller.start_dialog(DialogKind::SkillImport);
            }
        });
    });
    skills.on_toggle_app(|index, enabled| {
        with_controller(|controller| controller.toggle_skill_app(index, enabled));
    });
    skills.on_toggle_row_app(|id, index, enabled| {
        with_controller(|controller| controller.toggle_skill_row_app(id.into(), index, enabled));
    });
    skills.on_filter_changed(|| with_controller(Controller::render_skills));
    skills.on_toggle_select(|id, selected| {
        with_controller(|controller| controller.toggle_skill_selection(id.into(), selected));
    });
    skills.on_toggle_select_all(|selected| {
        with_controller(|controller| controller.toggle_skill_selection_all(selected));
    });
    skills.on_batch_app(|index, enabled| {
        with_controller(|controller| controller.batch_set_skill_app(index, enabled));
    });
    skills.on_install(|| with_controller(Controller::install_selected_skill));
    skills.on_update_selected(|| with_controller(Controller::update_selected_skill));
    skills.on_uninstall(|| with_controller(Controller::uninstall_selected_skill));
    skills.on_open_source(|target| {
        with_controller(|controller| controller.send(Work::OpenTarget(target.to_string())));
    });
    skills.on_show_details(|| {
        with_controller(|controller| {
            controller
                .ui()
                .global::<SkillsState>()
                .set_activity_visible(true);
        });
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn source_chips_lead_to_the_repository_folder_link_or_managed_copy() {
        use toolbox_core::skills::SkillRepository;
        let repository = |id: &str, url: &str, branch: Option<&str>| SkillRepository {
            id: id.into(),
            url: url.into(),
            branch: branch.map(Into::into),
            revision: None,
            last_error: None,
        };
        let snapshot = SkillsSnapshot {
            storage_path: "C:\\Users\\me\\skills\\managed".into(),
            repositories: vec![
                repository("main", "https://github.com/owner/skills.git", None),
                repository("pinned", "https://github.com/owner/other", Some("dev")),
            ],
            ..SkillsSnapshot::default()
        };
        let skill = |repository: Option<&str>, path: Option<&str>, linked: bool| InstalledSkill {
            id: "id".into(),
            directory: "review".into(),
            name: "review".into(),
            description: String::new(),
            repository_id: repository.map(Into::into),
            source_path: path.map(Into::into),
            revision: None,
            apps: vec![],
            updated_at: 0,
            last_error: None,
            linked,
            target: linked.then(|| "D:\\vault\\review".into()),
        };
        assert_eq!(
            source_target(
                &snapshot,
                &skill(Some("main"), Some("skills/review"), false)
            ),
            "https://github.com/owner/skills/tree/HEAD/skills/review"
        );
        assert_eq!(
            source_target(&snapshot, &skill(Some("pinned"), Some("review"), false)),
            "https://github.com/owner/other/tree/dev/review"
        );
        assert_eq!(
            source_target(&snapshot, &skill(Some("main"), Some(""), false)),
            "https://github.com/owner/skills"
        );
        let mut installed = skill(Some("pinned"), Some("review"), false);
        installed.revision = Some("0123456789abcdef".into());
        assert_eq!(
            source_target(&snapshot, &installed),
            "https://github.com/owner/other/tree/0123456789abcdef/review",
            "the page matches the installed commit shown as @sha"
        );
        assert_eq!(
            source_target(&snapshot, &skill(Some("gone"), None, false)),
            ""
        );
        assert_eq!(
            source_target(&snapshot, &skill(None, None, true)),
            "D:\\vault\\review"
        );
        assert!(
            source_target(&snapshot, &skill(None, None, false)).ends_with("review"),
            "a local copy opens its managed folder"
        );
    }

    #[test]
    fn schedule_distinguishes_failure_attempt_success_and_next_due() {
        let snapshot = SkillsSnapshot {
            daily_update: true,
            last_attempt_at: Some(7_200),
            last_success_at: Some(3_600),
            next_update_at: Some(10_800),
            ..SkillsSnapshot::default()
        };
        assert_eq!(
            schedule_text(&snapshot, 7_260, Locale::from_tag("en")),
            "Last attempt: 1 min ago · Last success: 1 h ago · Next: in 59 min"
        );
        assert!(
            schedule_text(&snapshot, 10_800, Locale::from_tag("zh-CN"))
                .ends_with("下次更新：已到期")
        );
        assert_eq!(
            relative_time(u64::MAX, 0, Locale::from_tag("en")),
            "in 213503982334601 d"
        );
    }

    #[test]
    fn relative_times_localize_complete_phrases_in_both_directions() {
        for (timestamp, english, chinese) in [
            (172_800, "now", "现在"),
            (172_859, "now", "现在"),
            (172_740, "1 min ago", "1 分钟前"),
            (172_860, "in 1 min", "1 分钟后"),
            (169_200, "1 h ago", "1 小时前"),
            (176_400, "in 1 h", "1 小时后"),
            (86_400, "1 d ago", "1 天前"),
            (259_200, "in 1 d", "1 天后"),
        ] {
            assert_eq!(
                relative_time(timestamp, 172_800, Locale::from_tag("en")),
                english
            );
            assert_eq!(
                relative_time(timestamp, 172_800, Locale::from_tag("zh-CN")),
                chinese
            );
        }
    }

    #[test]
    fn repository_labels_hide_the_canonical_git_suffix() {
        use toolbox_core::skills::SkillRepository;
        let snapshot = SkillsSnapshot {
            repositories: vec![SkillRepository {
                id: "source".into(),
                url: "https://github.com/sean2077/skills.git".into(),
                branch: None,
                revision: None,
                last_error: None,
            }],
            ..SkillsSnapshot::default()
        };
        assert_eq!(repository_label(&snapshot, "source"), "sean2077/skills");
        assert_eq!(repository_label(&snapshot, "missing"), "missing");
    }

    #[test]
    fn linked_local_folders_show_their_target_in_the_list() {
        use toolbox_core::skills::LocalSkill;
        let snapshot = SkillsSnapshot {
            local: vec![LocalSkill {
                path: "/home/.claude/skills/ob".into(),
                directory: "ob".into(),
                name: "ob".into(),
                description: String::new(),
                app: Some(SkillApp::Claude),
                linked: true,
                target: Some("/vault/ob".into()),
            }],
            ..SkillsSnapshot::default()
        };
        let rows = filtered_rows(&snapshot, 2, "", Locale::from_tag("en"), 0, 0);
        assert!(rows[0].linked);
        assert!(rows[0].detail.contains("→ /vault/ob"));
        assert!(!rows[0].installed);
    }

    #[test]
    fn shadowed_directory_names_are_not_offered_for_import_or_install() {
        use toolbox_core::skills::{InstalledSkill, LocalSkill, SkillCandidate};
        let snapshot = SkillsSnapshot {
            installed: vec![InstalledSkill {
                id: "alpha".into(),
                directory: "alpha".into(),
                name: "Alpha".into(),
                description: String::new(),
                repository_id: Some("source".into()),
                source_path: Some("alpha".into()),
                revision: Some("a".repeat(40)),
                apps: Vec::new(),
                updated_at: 0,
                last_error: None,
                linked: false,
                target: None,
            }],
            available: vec![SkillCandidate {
                id: "other:alpha".into(),
                repository_id: "other".into(),
                path: "alpha".into(),
                directory: "alpha".into(),
                name: "Alpha".into(),
                description: String::new(),
                installed: false,
            }],
            local: vec![LocalSkill {
                path: "/tmp/alpha".into(),
                directory: "ALPHA".into(),
                name: "Alpha".into(),
                description: String::new(),
                app: None,
                linked: false,
                target: None,
            }],
            ..SkillsSnapshot::default()
        };
        assert!(filtered_rows(&snapshot, 1, "", Locale::from_tag("zh-CN"), 0, 0)[0].installed);
        assert!(filtered_rows(&snapshot, 2, "", Locale::from_tag("zh-CN"), 0, 0)[0].installed);
    }

    fn installed_skill(
        id: &str,
        repository_id: Option<&str>,
        apps: &[SkillApp],
        linked: bool,
    ) -> InstalledSkill {
        InstalledSkill {
            id: id.into(),
            directory: id.into(),
            name: id.into(),
            description: String::new(),
            repository_id: repository_id.map(Into::into),
            source_path: None,
            revision: None,
            apps: apps.to_vec(),
            updated_at: 0,
            last_error: None,
            linked,
            target: linked.then(|| format!("/vault/{id}")),
        }
    }

    fn filter_ids(snapshot: &SkillsSnapshot, status: i32, source: i32) -> Vec<String> {
        filtered_rows(snapshot, 0, "", Locale::from_tag("zh-CN"), status, source)
            .into_iter()
            .map(|row| row.id.to_string())
            .collect()
    }

    #[test]
    fn installed_rows_filter_by_status_and_source() {
        use toolbox_core::skills::SkillRepository;
        let snapshot = SkillsSnapshot {
            repositories: vec![SkillRepository {
                id: "repo".into(),
                url: "https://github.com/sean2077/skills.git".into(),
                branch: None,
                revision: None,
                last_error: None,
            }],
            installed: vec![
                installed_skill("alpha", Some("repo"), &[SkillApp::Claude], false),
                installed_skill("beta", Some("repo"), &[], false),
                installed_skill("gamma", None, &[SkillApp::Codex], true),
                installed_skill("delta", None, &[], false),
            ],
            ..SkillsSnapshot::default()
        };
        assert_eq!(
            filter_ids(&snapshot, 0, 0),
            ["alpha", "beta", "delta", "gamma"]
        );
        assert_eq!(filter_ids(&snapshot, 1, 0), ["alpha", "gamma"]);
        assert_eq!(filter_ids(&snapshot, 2, 0), ["beta", "delta"]);
        assert_eq!(filter_ids(&snapshot, 0, 1), ["delta"]);
        assert_eq!(filter_ids(&snapshot, 0, 2), ["gamma"]);
        assert_eq!(filter_ids(&snapshot, 0, 3), ["alpha", "beta"]);
        assert_eq!(filter_ids(&snapshot, 1, 3), ["alpha"]);
        assert_eq!(
            source_options(&snapshot, Locale::from_tag("zh-CN")),
            ["全部", "本地", "链接", "sean2077/skills"]
        );
        assert_eq!(
            app_counts_text(&snapshot, Locale::from_tag("zh-CN")),
            "Claude Code 1 · Codex 1"
        );
    }

    #[test]
    fn installed_rows_expose_source_status_and_app_flags() {
        use toolbox_core::skills::SkillRepository;
        let snapshot = SkillsSnapshot {
            repositories: vec![SkillRepository {
                id: "repo".into(),
                url: "https://github.com/sean2077/skills.git".into(),
                branch: None,
                revision: None,
                last_error: None,
            }],
            installed: vec![installed_skill(
                "alpha",
                Some("repo"),
                &[SkillApp::Claude],
                false,
            )],
            ..SkillsSnapshot::default()
        };
        let rows = filtered_rows(&snapshot, 0, "", Locale::from_tag("en"), 0, 0);
        assert_eq!(rows[0].source, "sean2077/skills");
        assert_eq!(rows[0].status, "Enabled");
        assert!(rows[0].enabled);
        assert!(rows[0].app0);
        assert!(!rows[0].app4);
    }

    #[test]
    fn localization_preserves_external_skill_metadata() {
        let mut skill = installed_skill("review", None, &[], false);
        skill.name = "用户名称 / custom name".into();
        skill.description = "External description / 原文 {path}".into();
        let snapshot = SkillsSnapshot {
            installed: vec![skill],
            ..SkillsSnapshot::default()
        };
        for (language, source, status) in [("en", "Local", "Disabled"), ("zh-CN", "本地", "未启用")]
        {
            let rows = filtered_rows(&snapshot, 0, "", Locale::from_tag(language), 0, 0);
            assert_eq!(rows[0].id, "review");
            assert_eq!(rows[0].name, "用户名称 / custom name");
            assert_eq!(rows[0].description, "External description / 原文 {path}");
            assert_eq!(rows[0].source, source);
            assert_eq!(rows[0].status, status);
        }
    }

    fn ux_snapshot() -> SkillsSnapshot {
        use toolbox_core::skills::{SkillAppPath, SkillRepository};
        SkillsSnapshot {
            storage_path: "/toolbox/managed".into(),
            app_paths: SkillApp::ALL
                .iter()
                .map(|app| SkillAppPath {
                    app: *app,
                    path: format!("/apps/{}", app.label()),
                })
                .collect(),
            repositories: vec![SkillRepository {
                id: "source".into(),
                url: "https://github.com/sean2077/skills.git".into(),
                branch: None,
                revision: Some("0123456789abcdef".into()),
                last_error: None,
            }],
            installed: vec![
                installed_skill("alpha", Some("source"), &[SkillApp::Claude], false),
                installed_skill("beta", Some("source"), &[], false),
            ],
            ..SkillsSnapshot::default()
        }
    }

    fn assert_path_eq(actual: &str, expected: &str) {
        // Compare the complete location using the platform's separator rules.
        assert_eq!(std::path::Path::new(actual), std::path::Path::new(expected));
    }

    fn receive_snapshot(controller: &Controller, snapshot: SkillsSnapshot, submitted: bool) {
        controller.receive_skills(
            Ok(Response {
                ok: true,
                skills: Some(snapshot),
                ..Response::default()
            }),
            submitted,
        );
    }

    fn prepare_keyboard_fixture(ui: &crate::AppWindow, controller: &Controller) {
        receive_snapshot(controller, ux_snapshot(), false);
        ui.set_page(4);
        ui.window().set_size(slint::LogicalSize::new(1120.0, 780.0));
        ui.show().unwrap();
        ui.invoke_focus_page();
    }

    fn press_key(ui: &crate::AppWindow, text: impl Into<SharedString>) {
        use slint::platform::WindowEvent;
        let text = text.into();
        ui.window()
            .dispatch_event(WindowEvent::KeyPressed { text: text.clone() });
        ui.window()
            .dispatch_event(WindowEvent::KeyReleased { text });
    }

    fn open_keyboard_menu(ui: &crate::AppWindow) {
        use slint::platform::{Key, WindowEvent};
        ui.window().dispatch_event(WindowEvent::KeyPressed {
            text: Key::Shift.into(),
        });
        press_key(ui, Key::F10);
        ui.window().dispatch_event(WindowEvent::KeyReleased {
            text: Key::Shift.into(),
        });
    }

    #[test]
    fn escape_closes_skill_details_and_activity_before_dismissing_the_window() {
        use slint::platform::Key;
        use std::{cell::Cell, rc::Rc};
        crate::tests::with_test_ui(|ui, controller, _| {
            prepare_keyboard_fixture(ui, controller);
            let dismissed = Rc::new(Cell::new(0));
            let dismissals = dismissed.clone();
            ui.on_dismiss(move || dismissals.set(dismissals.get() + 1));
            let closed = Rc::new(Cell::new(0));
            let closures = closed.clone();
            let weak = ui.as_weak();
            let skills = ui.global::<SkillsState>();
            skills.on_close_details(move || {
                closures.set(closures.get() + 1);
                weak.unwrap()
                    .global::<SkillsState>()
                    .set_details_visible(false);
            });
            controller.open_skill_details("alpha".into());
            ui.invoke_focus_page();
            press_key(ui, Key::Escape);
            assert_eq!(closed.get(), 1);
            assert!(!skills.get_details_visible());
            assert_eq!(dismissed.get(), 0);

            skills.set_activity_visible(true);
            ui.invoke_focus_page();
            press_key(ui, Key::Escape);
            assert!(!skills.get_activity_visible());
            assert_eq!(dismissed.get(), 0);
            press_key(ui, Key::Escape);
            assert_eq!(dismissed.get(), 1);
        });
    }

    #[test]
    fn keyboard_menu_routes_actions_skips_disabled_items_and_ignores_repeat() {
        use slint::platform::{Key, WindowEvent};
        use std::{cell::Cell, rc::Rc};
        crate::tests::with_test_ui(|ui, controller, _| {
            prepare_keyboard_fixture(ui, controller);
            let skills = ui.global::<SkillsState>();
            let updated = Rc::new(Cell::new(0));
            let updates = updated.clone();
            skills.on_update_selected(move || updates.set(updates.get() + 1));
            let uninstalled = Rc::new(Cell::new(0));
            let uninstalls = uninstalled.clone();
            skills.on_uninstall(move || uninstalls.set(uninstalls.get() + 1));
            let dismissed = Rc::new(Cell::new(0));
            let dismissals = dismissed.clone();
            ui.on_dismiss(move || dismissals.set(dismissals.get() + 1));

            // Real WindowEvent routing moves focus from search to the list and
            // then through the popup. No callback is invoked by the test itself.
            press_key(ui, Key::DownArrow);
            open_keyboard_menu(ui);
            press_key(ui, Key::DownArrow);
            ui.window().dispatch_event(WindowEvent::KeyPressRepeated {
                text: Key::Return.into(),
            });
            assert_eq!(updated.get(), 0);
            press_key(ui, Key::Return);
            assert_eq!(updated.get(), 1);
            assert_eq!(uninstalled.get(), 0);

            skills.set_selected_updatable(false);
            open_keyboard_menu(ui);
            press_key(ui, Key::DownArrow);
            press_key(ui, Key::Return);
            assert_eq!(updated.get(), 1);
            assert_eq!(uninstalled.get(), 1);

            open_keyboard_menu(ui);
            press_key(ui, Key::Escape);
            assert_eq!(dismissed.get(), 0, "Escape dismisses only the popup");
            press_key(ui, Key::Escape);
            assert_eq!(dismissed.get(), 1);
        });
    }

    #[test]
    fn list_keyboard_routes_navigation_and_never_selects_an_empty_result() {
        use slint::platform::{Key, WindowEvent};
        use std::{cell::RefCell, rc::Rc};
        crate::tests::with_test_ui(|ui, controller, _| {
            prepare_keyboard_fixture(ui, controller);
            let skills = ui.global::<SkillsState>();
            let movement = Rc::new(RefCell::new(Vec::new()));
            let moved = movement.clone();
            skills.on_move_selection(move |delta| moved.borrow_mut().push(delta));
            let details = Rc::new(RefCell::new(Vec::new()));
            let opened = details.clone();
            skills.on_open_details(move |id| opened.borrow_mut().push(id.to_string()));
            let selection = Rc::new(RefCell::new(Vec::new()));
            let selected = selection.clone();
            skills.on_toggle_select(move |id, checked| {
                selected.borrow_mut().push((id.to_string(), checked));
            });

            press_key(ui, Key::DownArrow);
            assert_eq!(*movement.borrow(), [1]);
            press_key(ui, Key::Return);
            assert_eq!(*details.borrow(), ["alpha"]);
            ui.window()
                .dispatch_event(WindowEvent::KeyPressed { text: " ".into() });
            ui.window()
                .dispatch_event(WindowEvent::KeyPressRepeated { text: " ".into() });
            ui.window()
                .dispatch_event(WindowEvent::KeyReleased { text: " ".into() });
            assert_eq!(*selection.borrow(), [("alpha".to_owned(), true)]);

            skills.set_query("no matching skill exists".into());
            controller.render_skills();
            assert_eq!(skills.get_rows().row_count(), 0);
            assert_eq!(skills.get_selected_index(), -1);
            press_key(ui, " ");
            assert_eq!(*selection.borrow(), [("alpha".to_owned(), true)]);
        });
    }

    #[test]
    fn batch_summary_keeps_hidden_scope_and_mixed_deployments_distinct() {
        let snapshot = ux_snapshot();
        let rows = filtered_rows(&snapshot, 0, "beta", Locale::En, 0, 0);
        let selected = ["alpha".into(), "beta".into()].into_iter().collect();
        assert_eq!(
            selection_summary(&snapshot, &selected, &rows),
            SelectionSummary {
                total: 2,
                visible: 1,
                deployed: [1, 0, 0, 0, 0],
            }
        );
    }

    #[test]
    fn deselect_visible_preserves_hidden_selection_and_explicit_clear_removes_it() {
        crate::tests::with_test_ui(|ui, controller, receive| {
            receive_snapshot(controller, ux_snapshot(), false);
            let skills = ui.global::<SkillsState>();
            controller.toggle_skill_selection("alpha".into(), true);
            skills.set_query("beta".into());
            controller.render_skills();
            controller.toggle_skill_selection_all(true);
            assert_eq!(skills.get_selected_count(), 2);
            assert_eq!(skills.get_hidden_selected_count(), 1);
            assert_eq!(skills.get_visible_selected_count(), 1);
            assert_eq!(
                skills.get_batch_deployment_text(),
                "Claude Code：所选 2 项中已有 1 项部署"
            );
            controller.toggle_skill_selection_all(false);
            assert_eq!(skills.get_selected_count(), 1);
            assert_eq!(skills.get_hidden_selected_count(), 1);
            assert!(!skills.get_all_selected());
            controller.batch_set_skill_app(1, true);
            assert!(
                matches!(receive.try_recv().unwrap(), Work::Skills(SkillsCommand::SetEnabledBatch { skill_ids, app: SkillApp::Codex, enabled: true }) if skill_ids == ["alpha"])
            );
            controller.clear_skill_selection();
            assert_eq!(skills.get_selected_count(), 0);
        });
    }

    #[test]
    fn installed_detail_exposes_the_specific_error_source_and_real_deployment_paths() {
        crate::tests::with_test_ui(|ui, controller, receive| {
            let mut snapshot = ux_snapshot();
            snapshot.installed[0].last_error =
                Some("Preserve edits in /apps/Claude Code/alpha".into());
            snapshot.installed[0].revision = Some("0123456789abcdef".into());
            snapshot.installed[0].source_path = Some("skills/alpha".into());
            receive_snapshot(controller, snapshot, false);
            controller.open_skill_details("alpha".into());
            let skills = ui.global::<SkillsState>();
            assert!(skills.get_details_visible());
            assert_eq!(
                skills.get_selected_error(),
                "Preserve edits in /apps/Claude Code/alpha"
            );
            assert_path_eq(
                skills.get_selected_master_path().as_str(),
                "/toolbox/managed/alpha",
            );
            assert_eq!(
                skills.get_selected_source_target(),
                "https://github.com/sean2077/skills/tree/0123456789abcdef/skills/alpha"
            );
            assert!(skills.get_selected_summary().contains("0123456789abcdef"));
            assert_path_eq(
                skills.get_apps().row_data(1).unwrap().path.as_str(),
                "/apps/Codex/alpha",
            );
            let hint = skills.get_rows().row_data(0).unwrap().app0_hint;
            let (_, deployment_path) = hint
                .split_once('\n')
                .expect("application tooltip includes its deployment path");
            assert_path_eq(deployment_path, "/apps/Claude Code/alpha");
            assert!(
                receive.try_recv().is_err(),
                "opening details never changes deployments"
            );
            controller.close_skill_details();
            assert_eq!(skills.get_selected_id(), "alpha");
            assert!(!skills.get_details_visible());
        });
    }

    #[test]
    fn request_errors_stay_with_the_item_that_failed_and_background_errors_stay_global() {
        crate::tests::with_test_ui(|ui, controller, receive| {
            receive_snapshot(controller, ux_snapshot(), false);
            controller.open_skill_details("alpha".into());
            controller.toggle_skill_app(1, true);
            assert!(matches!(
                receive.try_recv().unwrap(),
                Work::Skills(SkillsCommand::SetEnabled { .. })
            ));
            let mut failed = ux_snapshot();
            failed.last_error = Some("Codex folder already exists".into());
            receive_snapshot(controller, failed.clone(), true);
            let skills = ui.global::<SkillsState>();
            assert_eq!(skills.get_selected_error(), "Codex folder already exists");

            controller.open_skill_details("beta".into());
            assert_eq!(skills.get_selected_error(), "");
            failed.last_error = Some("A later background update could not fetch GitHub".into());
            receive_snapshot(controller, failed, false);
            assert_eq!(skills.get_selected_error(), "");
            assert!(
                skills
                    .get_activity_detail()
                    .contains("later background update")
            );
            controller.open_skill_details("alpha".into());
            assert_eq!(skills.get_selected_error(), "Codex folder already exists");
            assert!(receive.try_recv().is_err());
        });
    }

    #[test]
    fn already_managed_candidates_open_the_actual_installed_source_and_deployments() {
        use toolbox_core::skills::SkillCandidate;
        crate::tests::with_test_ui(|ui, controller, receive| {
            let mut snapshot = ux_snapshot();
            snapshot.available.push(SkillCandidate {
                id: "another-candidate".into(),
                repository_id: "another-source".into(),
                path: "other/alpha".into(),
                directory: "Alpha".into(),
                name: "Another skill using alpha".into(),
                description: "Different content with the same directory name".into(),
                installed: false,
            });
            receive_snapshot(controller, snapshot, false);
            controller.skills_view(1);
            controller.open_skill_details("another-candidate".into());
            let skills = ui.global::<SkillsState>();
            assert!(skills.get_selected_installed());
            assert_eq!(skills.get_selected_managed_id(), "alpha");
            controller.install_selected_skill();
            assert!(receive.try_recv().is_err());
            controller.show_managed_skill();
            assert_eq!(skills.get_view(), 0);
            assert_eq!(skills.get_selected_id(), "alpha");
            assert!(skills.get_details_visible());
            assert!(skills.get_selected_source().contains("sean2077/skills"));
            assert!(skills.get_apps().row_data(0).unwrap().enabled);
            assert!(!skills.get_apps().row_data(1).unwrap().enabled);
            assert_path_eq(
                skills.get_selected_master_path().as_str(),
                "/toolbox/managed/alpha",
            );
            assert!(receive.try_recv().is_err());
        });
    }

    #[test]
    fn navigating_to_another_detail_during_import_preserves_the_new_selection() {
        use toolbox_core::skills::LocalSkill;
        crate::tests::with_test_ui(|ui, controller, receive| {
            let mut snapshot = ux_snapshot();
            snapshot.local.push(LocalSkill {
                path: "/tmp/other-skill".into(),
                directory: "other-skill".into(),
                name: "Other skill".into(),
                description: String::new(),
                app: None,
                linked: false,
                target: None,
            });
            receive_snapshot(controller, snapshot.clone(), false);
            controller.import_skill_folder(Ok(Some("/tmp/local-skill".into())));
            controller.install_selected_skill();
            assert!(matches!(
                receive.try_recv().unwrap(),
                Work::Skills(SkillsCommand::ImportLocal { .. })
            ));
            let mut running = snapshot.clone();
            running.busy = true;
            receive_snapshot(controller, running, true);
            controller.open_skill_details("/tmp/other-skill".into());
            snapshot
                .installed
                .push(installed_skill("local-skill", None, &[], false));
            receive_snapshot(controller, snapshot, false);
            let skills = ui.global::<SkillsState>();
            assert_eq!(skills.get_view(), 2);
            assert_eq!(skills.get_selected_id(), "/tmp/other-skill");
            assert!(skills.get_details_visible());
            assert!(!skills.get_import_from_folder());
        });
    }

    #[test]
    fn attention_filter_reports_failed_items_without_hiding_their_error_details() {
        let mut snapshot = ux_snapshot();
        snapshot.installed[1].last_error = Some("Local copy modified".into());
        assert_eq!(filter_ids(&snapshot, 3, 0), ["beta"]);
        crate::tests::with_test_ui(move |ui, controller, _| {
            receive_snapshot(controller, snapshot, false);
            let skills = ui.global::<SkillsState>();
            assert_eq!(skills.get_problem_count(), 1);
            assert_eq!(skills.get_affected_skill_count(), 1);
            let activity = skills.get_activity_detail();
            let detail = activity
                .split("\n\n")
                .find(|detail| detail.starts_with("beta\n"))
                .expect("the failed skill remains in the activity details");
            let mut fields = detail.splitn(3, '\n');
            assert_eq!(fields.next(), Some("beta"));
            assert_path_eq(fields.next().unwrap(), "/toolbox/managed/beta");
            assert_eq!(fields.next(), Some("Local copy modified"));
            controller.show_skill_problems();
            assert_eq!(skills.get_rows().row_count(), 1);
            assert_eq!(skills.get_rows().row_data(0).unwrap().id, "beta");
        });
    }

    #[test]
    fn repository_only_errors_do_not_offer_an_empty_skill_attention_filter() {
        crate::tests::with_test_ui(|ui, controller, _| {
            let mut snapshot = ux_snapshot();
            snapshot.repositories[0].last_error = Some("Git executable not found".into());
            receive_snapshot(controller, snapshot, false);
            let skills = ui.global::<SkillsState>();
            assert_eq!(skills.get_problem_count(), 1);
            assert_eq!(skills.get_affected_skill_count(), 0);
            assert!(
                skills
                    .get_activity_detail()
                    .contains("https://github.com/sean2077/skills.git\nGit executable not found")
            );
        });
    }

    #[test]
    fn folder_import_is_reviewed_and_uses_the_same_explicit_app_choices_as_scanned_import() {
        crate::tests::with_test_ui(|ui, controller, receive| {
            receive_snapshot(controller, ux_snapshot(), false);
            controller.import_skill_folder(Ok(Some("/tmp/local skill".into())));
            let skills = ui.global::<SkillsState>();
            assert!(skills.get_details_visible());
            assert!(skills.get_import_from_folder());
            assert_eq!(skills.get_import_path(), "/tmp/local skill");
            assert!(
                receive.try_recv().is_err(),
                "choosing a folder only prepares a review"
            );
            controller.toggle_skill_app(1, true);
            skills.set_link_import(true);
            assert!(
                receive.try_recv().is_err(),
                "target selection is a draft until import"
            );
            controller.install_selected_skill();
            assert!(
                matches!(receive.try_recv().unwrap(), Work::Skills(SkillsCommand::ImportLocal { path, apps, link: true }) if path == "/tmp/local skill" && apps == [SkillApp::Codex])
            );
            let mut running = ux_snapshot();
            running.busy = true;
            receive_snapshot(controller, running, true);
            assert!(
                skills.get_import_from_folder(),
                "accepted does not mean installed"
            );
            assert!(skills.get_busy());
            let mut finished = ux_snapshot();
            finished.installed.push(installed_skill(
                "local skill",
                None,
                &[SkillApp::Codex],
                true,
            ));
            receive_snapshot(controller, finished, false);
            assert_eq!(skills.get_view(), 0);
            assert_eq!(skills.get_selected_id(), "local skill");
            assert!(skills.get_selected_installed());
            assert!(!skills.get_import_from_folder());
            assert!(skills.get_details_visible());
        });
    }

    #[test]
    fn deployment_mode_changes_only_the_default_and_keeps_existing_deployments() {
        crate::tests::with_test_ui(|ui, controller, receive| {
            let original = ux_snapshot();
            receive_snapshot(controller, original.clone(), false);
            let skills = ui.global::<SkillsState>();
            assert_eq!(skills.get_deploy_mode(), 0);
            controller.set_skill_deploy_mode(0);
            controller.set_skill_deploy_mode(9);
            assert!(receive.try_recv().is_err());

            controller.set_skill_deploy_mode(1);
            assert!(matches!(
                receive.try_recv().unwrap(),
                Work::Skills(SkillsCommand::SetDeployMode {
                    mode: SkillDeployMode::Copy
                })
            ));
            controller.set_skill_deploy_mode(1);
            assert!(
                receive.try_recv().is_err(),
                "mode changes remain serialized"
            );
            let mut completed = original.clone();
            completed.deploy_mode = SkillDeployMode::Copy;
            receive_snapshot(controller, completed, true);
            assert_eq!(skills.get_deploy_mode(), 1);
            assert!(skills.get_rows().row_data(0).unwrap().app0);
            assert!(!skills.get_rows().row_data(1).unwrap().app0);
            assert_eq!(
                controller
                    .state
                    .borrow()
                    .skills
                    .snapshot
                    .as_ref()
                    .unwrap()
                    .installed,
                original.installed
            );
            assert!(
                receive.try_recv().is_err(),
                "changing the default never redeploys existing app folders"
            );
        });
    }

    #[test]
    fn global_deployment_mode_preserves_import_review_and_the_linked_source_choice() {
        crate::tests::with_test_ui(|ui, controller, receive| {
            receive_snapshot(controller, ux_snapshot(), false);
            controller.import_skill_folder(Ok(Some("/tmp/live-source".into())));
            controller.toggle_skill_app(1, true);
            let skills = ui.global::<SkillsState>();
            skills.set_link_import(true);
            controller.set_skill_deploy_mode(1);
            assert!(matches!(
                receive.try_recv().unwrap(),
                Work::Skills(SkillsCommand::SetDeployMode {
                    mode: SkillDeployMode::Copy
                })
            ));
            let mut completed = ux_snapshot();
            completed.deploy_mode = SkillDeployMode::Copy;
            receive_snapshot(controller, completed, true);
            assert!(skills.get_details_visible());
            assert!(skills.get_import_from_folder());
            assert_eq!(skills.get_import_path(), "/tmp/live-source");
            assert!(skills.get_link_import());
            assert!(skills.get_apps().row_data(1).unwrap().enabled);
            controller.install_selected_skill();
            assert!(
                matches!(receive.try_recv().unwrap(), Work::Skills(SkillsCommand::ImportLocal { path, apps, link: true }) if path == "/tmp/live-source" && apps == [SkillApp::Codex])
            );
            assert!(receive.try_recv().is_err());
        });
    }

    #[test]
    fn cancelled_import_does_not_send_a_mutation_or_change_other_drafts() {
        crate::tests::with_test_ui(|ui, controller, receive| {
            receive_snapshot(controller, ux_snapshot(), false);
            ui.set_setting_hotkey("Ctrl+Alt+P".into());
            controller.settings_changed();
            controller.import_skill_folder(Ok(Some("/tmp/to review".into())));
            controller.close_skill_details();
            assert!(ui.get_settings_dirty());
            assert!(!ui.global::<SkillsState>().get_import_from_folder());
            assert!(receive.try_recv().is_err());
        });
    }

    #[test]
    fn repository_add_starts_discovery_only_after_successful_host_completion() {
        use toolbox_core::skills::SkillRepository;
        crate::tests::with_test_ui(|ui, controller, receive| {
            receive_snapshot(controller, ux_snapshot(), false);
            let skills = ui.global::<SkillsState>();
            controller.skills_view(3);
            skills.set_repository_url("owner/new-skills".into());
            skills.set_repository_branch("dev".into());
            controller.add_skill_repository();
            assert!(
                matches!(receive.try_recv().unwrap(), Work::Skills(SkillsCommand::AddRepository { url, branch: Some(branch) }) if url == "owner/new-skills" && branch == "dev")
            );
            let mut running = ux_snapshot();
            running.busy = true;
            receive_snapshot(controller, running, true);
            assert_eq!(skills.get_view(), 3);
            assert!(receive.try_recv().is_err());
            let mut finished = ux_snapshot();
            finished.repositories.push(SkillRepository {
                id: "new".into(),
                url: "https://github.com/owner/new-skills.git".into(),
                branch: Some("dev".into()),
                revision: None,
                last_error: None,
            });
            receive_snapshot(controller, finished.clone(), false);
            assert_eq!(skills.get_view(), 1);
            assert_eq!(skills.get_query(), "owner/new-skills");
            assert!(matches!(
                receive.try_recv().unwrap(),
                Work::Skills(SkillsCommand::Discover)
            ));
            finished.busy = true;
            receive_snapshot(controller, finished, true);
            assert!(
                receive.try_recv().is_err(),
                "follow-up discovery is enqueued exactly once"
            );
        });
    }

    #[test]
    fn failed_repository_add_keeps_the_form_and_does_not_start_discovery() {
        crate::tests::with_test_ui(|ui, controller, receive| {
            receive_snapshot(controller, ux_snapshot(), false);
            controller.skills_view(3);
            let skills = ui.global::<SkillsState>();
            skills.set_repository_url("invalid input".into());
            controller.add_skill_repository();
            assert!(matches!(
                receive.try_recv().unwrap(),
                Work::Skills(SkillsCommand::AddRepository { .. })
            ));
            let mut failed = ux_snapshot();
            failed.last_error = Some("Invalid repository".into());
            receive_snapshot(controller, failed, true);
            assert_eq!(skills.get_view(), 3);
            assert_eq!(skills.get_repository_url(), "invalid input");
            assert_eq!(skills.get_error_text(), "Invalid repository");
            assert!(receive.try_recv().is_err());
        });
    }
}
