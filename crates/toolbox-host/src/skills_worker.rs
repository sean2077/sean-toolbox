//! One owner serializes skills mutations and runs the persisted daily schedule.
//! Desktop/IPC threads only read the cached snapshot or submit bounded work.
use std::{
    path::{Path, PathBuf},
    sync::{
        Arc, Condvar, Mutex,
        atomic::{AtomicBool, Ordering},
        mpsc,
    },
    thread,
    time::Duration,
};
use toolbox_core::skills::{SkillsCommand, SkillsSnapshot};
use toolbox_skills::{SkillsService, unix_now};

const RETRY_SECONDS: u64 = 3_600;
const CLOCK_CHECK_SECONDS: u64 = 60;
type Shared = Arc<(Mutex<SkillsSnapshot>, Condvar)>;

pub struct SkillsWorker {
    tx: mpsc::SyncSender<Option<SkillsCommand>>,
    snapshot: Shared,
    stopping: Arc<AtomicBool>,
    worker: Option<thread::JoinHandle<()>>,
}

impl SkillsWorker {
    pub fn new(config_path: &Path) -> Self {
        Self::with_paths(skills_paths(config_path))
    }

    fn with_paths(paths: Result<(PathBuf, PathBuf), String>) -> Self {
        let (tx, rx) = mpsc::sync_channel(1);
        let snapshot = Arc::new((
            Mutex::new(SkillsSnapshot {
                busy: true,
                message: "正在读取 Skills / Loading skills".into(),
                ..SkillsSnapshot::default()
            }),
            Condvar::new(),
        ));
        let stopping = Arc::new(AtomicBool::new(false));
        let shared = snapshot.clone();
        let cancelled = stopping.clone();
        let worker = thread::spawn(move || {
            let open = || {
                let (root, home) = paths.clone()?;
                SkillsService::open(root, home, cancelled.clone())
            };
            let mut service = match open() {
                Ok(service) => {
                    publish(&shared, service.snapshot());
                    Some(service)
                }
                Err(error) => {
                    publish(&shared, failed_snapshot(error));
                    None
                }
            };
            let mut retry_not_before = 0;
            loop {
                if cancelled.load(Ordering::Acquire) {
                    break;
                }
                let due = service.as_ref().and_then(SkillsService::next_due);
                let received = match due {
                    Some(_) => rx.recv_timeout(wait_duration(due, unix_now(), retry_not_before)),
                    None => rx.recv().map_err(|_| mpsc::RecvTimeoutError::Disconnected),
                };
                let command = match received {
                    Ok(Some(command)) => Some(command),
                    Ok(None) | Err(mpsc::RecvTimeoutError::Disconnected) => break,
                    Err(mpsc::RecvTimeoutError::Timeout) => {
                        // Recheck wall time after sleep/resume or clock changes.
                        if due.is_none_or(|due| unix_now() < due.max(retry_not_before)) {
                            continue;
                        }
                        let mut cached = shared.0.lock().unwrap_or_else(|e| e.into_inner());
                        if cached.busy {
                            // A manual command reserved the worker at the deadline.
                            continue;
                        }
                        cached.busy = true;
                        cached.message = "正在每日更新 Skills / Updating skills daily".into();
                        cached.last_error = None;
                        drop(cached);
                        shared.1.notify_all();
                        None
                    }
                };
                if cancelled.load(Ordering::Acquire) {
                    break;
                }
                // A corrupt/unavailable store does not disable the rest of the
                // toolbox. An explicit retry can recover after the user fixes it.
                if service.is_none() {
                    match open() {
                        Ok(opened) => service = Some(opened),
                        Err(error) => {
                            publish(&shared, failed_snapshot(error));
                            continue;
                        }
                    }
                }
                let service = service.as_mut().expect("service opened above");
                let now = unix_now();
                let all_update = command.as_ref().is_none_or(|command| {
                    matches!(command, SkillsCommand::Update { skill_id: None })
                });
                let result = match command {
                    Some(command) => service.execute(command, now),
                    None => service.run_daily(now),
                };
                // Even a failed state-file write cannot cause an automatic
                // zero-delay retry loop. The service persists ordinary retries.
                if all_update {
                    retry_not_before = unix_now().saturating_add(if result.is_err() {
                        RETRY_SECONDS
                    } else {
                        CLOCK_CHECK_SECONDS
                    });
                }
                let mut next = service.snapshot();
                match result {
                    Ok(message) => next.message = message,
                    Err(error) => {
                        next.message = "Skills 操作未完成 / Skills operation incomplete".into();
                        next.last_error = Some(error);
                    }
                }
                if let Some(due) = next.next_update_at.as_mut() {
                    *due = (*due).max(retry_not_before);
                }
                publish(&shared, next);
            }
        });
        Self {
            tx,
            snapshot,
            stopping,
            worker: Some(worker),
        }
    }

    pub fn snapshot(&self) -> SkillsSnapshot {
        self.snapshot
            .0
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
    }

    /// Reserve before enqueueing, so a poll immediately sees busy and a second
    /// command cannot race the first command or the scheduled update.
    pub fn submit(&self, command: SkillsCommand) -> Result<SkillsSnapshot, String> {
        let mut snapshot = self.snapshot.0.lock().unwrap_or_else(|e| e.into_inner());
        if snapshot.busy || self.stopping.load(Ordering::Acquire) {
            return Err(
                "Skills 正在处理上一操作，请稍候 / Skills operation already in progress".into(),
            );
        }
        let previous = snapshot.clone();
        snapshot.busy = true;
        snapshot.message = "正在处理 Skills / Processing skills".into();
        snapshot.last_error = None;
        if self.tx.try_send(Some(command)).is_err() {
            *snapshot = previous;
            return Err("Skills 后台不可用 / Skills worker unavailable".into());
        }
        Ok(snapshot.clone())
    }

    pub fn stop(&mut self) {
        self.stopping.store(true, Ordering::Release);
        let _ = self.tx.try_send(None);
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}

impl Drop for SkillsWorker {
    fn drop(&mut self) {
        self.stop();
    }
}

fn publish(shared: &Shared, mut snapshot: SkillsSnapshot) {
    snapshot.busy = false;
    bound_status(&mut snapshot);
    *shared.0.lock().unwrap_or_else(|e| e.into_inner()) = snapshot;
    shared.1.notify_all();
}

/// The service reserves space for status, but errors returned from an operation
/// can aggregate many source failures. Bound the final host overlay as well.
fn bound_status(snapshot: &mut SkillsSnapshot) {
    fn text(value: &str) -> String {
        let mut characters = value.chars();
        let mut bounded: String = characters.by_ref().take(4_000).collect();
        if characters.next().is_some() {
            bounded.push_str("… [请查看各项结果 / See individual results]");
        }
        bounded
    }
    snapshot.message = text(&snapshot.message);
    snapshot.last_error = snapshot.last_error.as_deref().map(text);
}

fn failed_snapshot(error: String) -> SkillsSnapshot {
    SkillsSnapshot {
        message: "无法读取 Skills / Cannot load skills".into(),
        last_error: Some(error),
        ..SkillsSnapshot::default()
    }
}

fn skills_paths(config_path: &Path) -> Result<(PathBuf, PathBuf), String> {
    let root = config_path
        .parent()
        .ok_or("Missing configuration directory")?
        .join("skills");
    #[cfg(windows)]
    let home = std::env::var_os("USERPROFILE");
    #[cfg(not(windows))]
    let home = std::env::var_os("HOME");
    let home = home
        .map(PathBuf::from)
        .filter(|path| path.is_absolute())
        .ok_or("无法确定用户目录 / An absolute user home directory is required for skills")?;
    Ok((root, home))
}

fn wait_duration(due: Option<u64>, now: u64, retry_not_before: u64) -> Duration {
    Duration::from_secs(
        due.map(|due| due.max(retry_not_before).saturating_sub(now))
            .unwrap_or(CLOCK_CHECK_SECONDS)
            .min(CLOCK_CHECK_SECONDS),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn wait_idle(worker: &SkillsWorker) -> SkillsSnapshot {
        let guard = worker.snapshot.0.lock().unwrap();
        let (guard, timeout) = worker
            .snapshot
            .1
            .wait_timeout_while(guard, Duration::from_secs(5), |snapshot| snapshot.busy)
            .unwrap();
        assert!(!timeout.timed_out(), "worker did not finish: {guard:?}");
        guard.clone()
    }

    #[test]
    fn overdue_schedule_catches_up_but_failed_writes_cannot_spin() {
        assert_eq!(wait_duration(Some(100), 200, 0), Duration::ZERO);
        assert_eq!(
            wait_duration(Some(100), 200, 3_800),
            Duration::from_secs(60)
        );
        assert_eq!(wait_duration(Some(250), 200, 0), Duration::from_secs(50));
        assert_eq!(wait_duration(Some(300), 500, 510), Duration::from_secs(10));
    }

    #[test]
    fn commands_are_acknowledged_busy_and_persist_without_the_ui() {
        let dir = tempfile::tempdir().unwrap();
        let home = dir.path().join("home");
        std::fs::create_dir(&home).unwrap();
        let root = dir.path().join("skills");
        let mut worker = SkillsWorker::with_paths(Ok((root.clone(), home.clone())));
        assert!(wait_idle(&worker).last_error.is_none());
        let accepted = worker
            .submit(SkillsCommand::SetDailyUpdate { enabled: false })
            .unwrap();
        assert!(accepted.busy);
        let done = wait_idle(&worker);
        assert!(!done.daily_update);
        assert!(done.last_error.is_none());
        worker.stop();
        let reloaded = SkillsService::open(root, home, Arc::new(AtomicBool::new(false))).unwrap();
        assert!(!reloaded.snapshot().daily_update);
    }

    #[test]
    fn initialization_failure_is_reported_and_retry_does_not_hang() {
        let worker = SkillsWorker::with_paths(Err("test store unavailable".into()));
        assert_eq!(
            wait_idle(&worker).last_error.as_deref(),
            Some("test store unavailable")
        );
        assert!(worker.submit(SkillsCommand::ScanLocal).unwrap().busy);
        assert_eq!(
            wait_idle(&worker).last_error.as_deref(),
            Some("test store unavailable")
        );
    }

    #[test]
    fn oversized_operation_errors_keep_the_status_response_readable() {
        let shared = Arc::new((Mutex::new(SkillsSnapshot::default()), Condvar::new()));
        publish(&shared, failed_snapshot("仓库错误".repeat(300_000)));
        let snapshot = shared.0.lock().unwrap().clone();
        assert!(
            snapshot
                .last_error
                .as_deref()
                .unwrap()
                .starts_with("仓库错误")
        );
        assert!(
            snapshot
                .last_error
                .as_deref()
                .unwrap()
                .contains("See individual results")
        );
        let response = toolbox_core::Response {
            skills: Some(snapshot),
            ..toolbox_core::Response::success("Skills status")
        };
        let payload = serde_json::to_vec(&response).unwrap();
        assert!(payload.len() < 32 * 1024);
        assert!(payload.len() < toolbox_ipc::MAX_MESSAGE_BYTES);
    }
}
