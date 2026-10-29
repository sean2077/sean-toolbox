use std::{
    path::PathBuf,
    process::{Child, Command, Stdio},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
        mpsc,
    },
    thread,
    time::{Duration, Instant},
};
use toolbox_core::{Response, UiPage, UiRequest};

pub struct UiProcess {
    tx: mpsc::Sender<Option<UiPage>>,
    stopping: Arc<AtomicBool>,
    worker: Option<thread::JoinHandle<()>>,
}

impl UiProcess {
    pub fn new(executable: PathBuf, notify: impl Fn(Result<(), String>) + Send + 'static) -> Self {
        let (tx, rx) = mpsc::channel();
        let stopping = Arc::new(AtomicBool::new(false));
        let stopping_worker = stopping.clone();
        let worker = thread::spawn(move || {
            let mut child: Option<Child> = None;
            let mut endpoint = String::new();
            let mut generation = 0;
            loop {
                let message = if child.is_some() {
                    rx.recv_timeout(Duration::from_secs(1))
                } else {
                    rx.recv().map_err(|_| mpsc::RecvTimeoutError::Disconnected)
                };
                let page = match message {
                    Ok(Some(page)) => page,
                    Ok(None) | Err(mpsc::RecvTimeoutError::Disconnected) => break,
                    Err(mpsc::RecvTimeoutError::Timeout) => {
                        if child
                            .as_mut()
                            .is_some_and(|c| matches!(c.try_wait(), Ok(Some(_))))
                        {
                            child = None;
                            cleanup_endpoint(&endpoint);
                        }
                        continue;
                    }
                };
                if stopping_worker.load(Ordering::Relaxed) {
                    break;
                }
                let alive = match child.as_mut().map(Child::try_wait) {
                    Some(Ok(None)) => true,
                    Some(Err(error)) => {
                        notify(Err(format!("Cannot inspect the UI process: {error}")));
                        continue;
                    }
                    Some(Ok(Some(_))) => {
                        child = None;
                        cleanup_endpoint(&endpoint);
                        false
                    }
                    None => false,
                };
                if alive {
                    #[cfg(windows)]
                    if let Some(child) = &child {
                        unsafe {
                            windows_sys::Win32::UI::WindowsAndMessaging::AllowSetForegroundWindow(
                                child.id(),
                            );
                        }
                    }
                    let response: Result<Response, _> =
                        toolbox_ipc::request(&endpoint, &UiRequest { page }, 2_000);
                    match response {
                        Ok(response) if response.ok => {
                            notify(Ok(()));
                            continue;
                        }
                        _ => {
                            // An unresponsive existing UI is reported, never duplicated or killed here.
                            notify(Err(
                                "界面未响应，请关闭界面后重试 / The existing UI is not responding"
                                    .into(),
                            ));
                            continue;
                        }
                    }
                }
                generation += 1;
                let id = format!("sean-toolbox-ui-{}-{generation}", std::process::id());
                endpoint = match toolbox_ipc::current_user_endpoint(&id) {
                    Ok(x) => x,
                    Err(e) => {
                        notify(Err(e.to_string()));
                        continue;
                    }
                };
                let page_name = match page {
                    UiPage::Projects => "projects",
                    UiPage::Windows => "windows",
                    UiPage::Settings => "settings",
                    UiPage::Diagnostics => "diagnostics",
                };
                let mut command = Command::new(&executable);
                command
                    .args(["--page", page_name, "--endpoint", &id])
                    .stdin(Stdio::null());
                #[cfg(windows)]
                {
                    use std::os::windows::process::CommandExt;
                    command.creation_flags(0x08000000);
                }
                match command.spawn() {
                    Ok(process) => {
                        #[cfg(windows)]
                        unsafe {
                            windows_sys::Win32::UI::WindowsAndMessaging::AllowSetForegroundWindow(
                                process.id(),
                            );
                        }
                        child = Some(process);
                    }
                    Err(e) => {
                        notify(Err(format!("Cannot start {}: {e}", executable.display())));
                        continue;
                    }
                }
                let started = Instant::now();
                loop {
                    if stopping_worker.load(Ordering::Relaxed) {
                        break;
                    }
                    if child
                        .as_mut()
                        .is_some_and(|c| matches!(c.try_wait(), Ok(Some(_))))
                    {
                        notify(Err(
                            "界面进程提前退出 / UI process exited during startup".into()
                        ));
                        break;
                    }
                    if toolbox_ipc::request::<_, Response>(&endpoint, &UiRequest { page }, 150)
                        .is_ok_and(|r| r.ok)
                    {
                        notify(Ok(()));
                        break;
                    }
                    if started.elapsed() > Duration::from_secs(8) {
                        notify(Err("界面启动超时 / UI startup timed out".into()));
                        break;
                    }
                    thread::sleep(Duration::from_millis(50));
                }
            }
            // Only our own child is stopped when the user explicitly exits the host.
            if let Some(mut child) = child {
                let _ = child.kill();
                let _ = child.wait();
                cleanup_endpoint(&endpoint);
            }
        });
        Self {
            tx,
            stopping,
            worker: Some(worker),
        }
    }
    pub fn show(&self, page: UiPage) -> Result<(), String> {
        self.tx
            .send(Some(page))
            .map_err(|_| "UI supervisor stopped".into())
    }
}

fn cleanup_endpoint(endpoint: &str) {
    #[cfg(unix)]
    {
        use std::os::unix::fs::{FileTypeExt, MetadataExt};
        if std::fs::symlink_metadata(endpoint)
            .is_ok_and(|m| m.file_type().is_socket() && m.uid() == unsafe { libc::geteuid() })
        {
            let _ = std::fs::remove_file(endpoint);
        }
    }
    #[cfg(not(unix))]
    let _ = endpoint;
}

impl Drop for UiProcess {
    fn drop(&mut self) {
        self.stopping.store(true, Ordering::Relaxed);
        let _ = self.tx.send(None);
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}
