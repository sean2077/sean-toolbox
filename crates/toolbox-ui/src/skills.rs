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
    let directory = directory.to_lowercase();
    snapshot
        .installed
        .iter()
        .any(|skill| skill.directory.to_lowercase() == directory)
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
        messages::skills_source_all().render(locale),
        messages::skills_source_local().render(locale),
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
                        .unwrap_or_else(|| messages::skills_source_local().render(locale))
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
                        messages::skills_enabled()
                    } else {
                        messages::skills_disabled()
                    }
                    .render(locale)
                    .into(),
                    enabled,
                    app0: skill.apps.contains(&SkillApp::Claude),
                    app1: skill.apps.contains(&SkillApp::Codex),
                    app2: skill.apps.contains(&SkillApp::Gemini),
                    app3: skill.apps.contains(&SkillApp::OpenCode),
                    app4: skill.apps.contains(&SkillApp::Grok),
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
        }
        let ui = self.ui();
        let skills = ui.global::<SkillsState>();
        skills.set_submitting(true);
        skills.set_error_text("".into());
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
            }
            Err(error) => {
                self.state.borrow_mut().skills.error = Some(error.clone());
                self.render_skills();
                self.error(error);
            }
        }
        self.schedule_skills_poll();
        // Accepted operations live in the host. They must not retain the UI.
        self.schedule_unload();
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
        skills.set_error_text(error.into());
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
                let detail = messages::skills_repository_installed(count, branch).render(locale);
                SkillRepositoryRow {
                    id: repository.id.clone().into(),
                    url: repository.url.clone().into(),
                    detail: detail.into(),
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
        let source_count = options.len().max(1).min(i32::MAX as usize) as i32;
        let status_filter = skills.get_status_filter().clamp(0, 2);
        let source_filter = skills.get_source_filter().clamp(0, source_count - 1);
        if skills.get_status_filter() != status_filter {
            skills.set_status_filter(status_filter);
        }
        if skills.get_source_filter() != source_filter {
            skills.set_source_filter(source_filter);
        }
        if !skills.get_source_options().iter().eq(options.iter()) {
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
        let all_selected = !rows.is_empty() && rows.iter().all(|row| row.selected);
        skills.set_selected_count(state.skills.selected_ids.len().min(i32::MAX as usize) as i32);
        skills.set_all_selected(all_selected);
        let selected_skills = snapshot
            .installed
            .iter()
            .filter(|skill| state.skills.selected_ids.contains(&skill.id))
            .collect::<Vec<_>>();
        let has_selection = !selected_skills.is_empty();
        let batch_app = |app: SkillApp| {
            has_selection
                && selected_skills
                    .iter()
                    .all(|skill| skill.apps.contains(&app))
        };
        skills.set_batch_app0(batch_app(SkillApp::Claude));
        skills.set_batch_app1(batch_app(SkillApp::Codex));
        skills.set_batch_app2(batch_app(SkillApp::Gemini));
        skills.set_batch_app3(batch_app(SkillApp::OpenCode));
        skills.set_batch_app4(batch_app(SkillApp::Grok));
        let selected = skills.get_selected_id();
        let selected = rows
            .iter()
            .find(|row| row.id == selected)
            .or_else(|| rows.first());
        skills.set_selected_id(selected.map(|row| row.id.clone()).unwrap_or_default());
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
        skills.set_selected_updatable(false);
        let installed = (view == 0)
            .then(|| {
                snapshot
                    .installed
                    .iter()
                    .find(|skill| skill.id == skills.get_selected_id().as_str())
            })
            .flatten();
        if let Some(installed) = installed {
            let revision = installed
                .revision
                .as_deref()
                .map(|revision| revision.chars().take(12).collect::<String>())
                .unwrap_or_else(|| "—".into());
            let updated = relative_time(installed.updated_at, now_seconds(), locale);
            let detail = match installed.target.as_deref().filter(|_| installed.linked) {
                Some(target) => messages::skills_linked_detail(
                    &installed.directory,
                    &revision,
                    target,
                    &updated,
                ),
                None => messages::skills_detail(&installed.directory, &revision, &updated),
            };
            skills.set_selected_detail(detail.render(locale).into());
            skills.set_selected_summary(
                messages::skills_selected_summary(&installed.directory, revision, updated)
                    .render(locale)
                    .into(),
            );
            skills.set_selected_updatable(installed.repository_id.is_some());
            skills.set_selected_error(installed.last_error.clone().unwrap_or_default().into());
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
                path: snapshot
                    .app_paths
                    .iter()
                    .find(|item| item.app == *app)
                    .map(|item| item.path.clone())
                    .unwrap_or_default()
                    .into(),
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
        self.render_skills();
    }

    pub(super) fn select_skill(&self, id: String) {
        self.ui().global::<SkillsState>().set_selected_id(id.into());
        self.render_skills();
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
        self.submit_skills(SkillsCommand::AddRepository {
            url,
            branch: (!branch.is_empty()).then_some(branch),
        });
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
        if selected {
            state.skills.selected_ids.extend(ids);
        } else {
            state.skills.selected_ids.clear();
        }
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
        if id.is_empty() || skills.get_selected_installed() {
            return;
        }
        let apps = self.state.borrow().skills.install_apps.clone();
        let command = match skills.get_view() {
            1 => SkillsCommand::Install {
                candidate_id: id,
                apps,
            },
            2 => SkillsCommand::ImportLocal {
                path: id,
                apps,
                link: skills.get_link_import(),
            },
            _ => return,
        };
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
            messages::skills_uninstall_title(),
            messages::skills_uninstall_body(),
        );
    }

    pub(super) fn import_skill_folder(&self, path: Result<Option<String>, UiText>) {
        self.state.borrow_mut().dialog_open = false;
        self.ui().set_dialog_busy(false);
        match path {
            Ok(Some(path)) => {
                self.skills_view(0);
                let link = self.ui().global::<SkillsState>().get_link_import();
                self.submit_skills(SkillsCommand::ImportLocal {
                    path,
                    apps: Vec::new(),
                    link,
                });
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
        with_controller(|controller| {
            controller.submit_skills(SkillsCommand::SetDeployMode {
                mode: if mode == 0 {
                    SkillDeployMode::Symlink
                } else {
                    SkillDeployMode::Copy
                },
            })
        });
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
        with_controller(|controller| controller.navigate(crate::page_index(UiPage::Diagnostics)))
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
}
