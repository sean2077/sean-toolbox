//! A blocking worker keeps SQLite, filesystem access, dialogs and IPC off the UI thread.
use crate::model::{ImportedConfig, MAX_IMPORT_BYTES};
use std::{
    fs::File,
    io::{Read, Write},
    path::PathBuf,
    process::{Command, Stdio},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicU64, Ordering},
        mpsc::Receiver,
    },
};
use toolbox_core::{Config, HostCommand, Response, UiPage, UiRequest};
use toolbox_projects::ProjectItem;

// rfd exposes Option for both cancel and backend errors. Preserve its actual
// error report so a missing portal/zenity does not look like a successful action.
static DIALOG_ERROR: Mutex<Option<String>> = Mutex::new(None);
struct DialogLogger;
static DIALOG_LOGGER: DialogLogger = DialogLogger;
impl log::Log for DialogLogger {
    fn enabled(&self, metadata: &log::Metadata<'_>) -> bool {
        metadata.level() == log::Level::Error && metadata.target().starts_with("rfd")
    }
    fn log(&self, record: &log::Record<'_>) {
        if self.enabled(record.metadata())
            && let Ok(mut error) = DIALOG_ERROR.lock()
        {
            *error = Some(record.args().to_string());
        }
    }
    fn flush(&self) {}
}
pub fn init_dialog_logging() {
    if log::set_logger(&DIALOG_LOGGER).is_ok() {
        log::set_max_level(log::LevelFilter::Error);
    }
}
fn native_dialog<T>(show: impl FnOnce() -> Option<T>) -> Result<Option<T>, String> {
    if let Ok(mut error) = DIALOG_ERROR.lock() {
        *error = None;
    }
    let result = show();
    if result.is_none()
        && let Some(error) = DIALOG_ERROR.lock().ok().and_then(|mut error| error.take())
    {
        return Err(format!("文件选择器失败 / File dialog failed: {error}"));
    }
    Ok(result)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SaveKind {
    Profile,
    Settings,
    Delete,
    Import,
}

pub enum DialogKind {
    Browse(i32),
    Import,
    Export,
}

pub enum Work {
    Load {
        force: bool,
    },
    Save {
        config: Config,
        revision: u64,
        kind: SaveKind,
    },
    Discover {
        generation: u64,
        query: String,
        code_path: Option<String>,
    },
    Search {
        generation: u64,
        query: String,
    },
    Open {
        id: String,
        code_path: Option<String>,
    },
    Toggle(String),
    Status,
    Dialog(DialogKind, rfd::FileDialog),
    Link(i32),
}

pub enum Event {
    Loaded {
        response: Result<Response, String>,
        force: bool,
    },
    Saved {
        response: Result<Response, String>,
        config: Config,
        kind: SaveKind,
    },
    Projects {
        generation: u64,
        result: Result<Vec<ProjectItem>, String>,
        total: usize,
    },
    Opened(Result<(), String>),
    Toggled(Result<Response, String>),
    Status(Result<Response, String>),
    PathPicked {
        kind: i32,
        path: Result<Option<String>, String>,
    },
    Imported(Result<Option<ImportedConfig>, String>),
    Exported(Result<Option<PathBuf>, String>),
    LinkOpened(Result<(), String>),
    ServerError(String),
    Ready {
        reply: std::sync::mpsc::Sender<Response>,
    },
    Show {
        page: UiPage,
        reply: std::sync::mpsc::Sender<Response>,
    },
}

fn successful(response: Response) -> Result<Response, String> {
    if response.ok {
        Ok(response)
    } else {
        Err(response.message)
    }
}

pub fn run(receiver: Receiver<Work>, latest_query: Arc<AtomicU64>) {
    let mut projects = Vec::<ProjectItem>::new();
    let mut discovery_error = None::<String>;
    while let Ok(work) = receiver.recv() {
        let event = match work {
            Work::Load { force } => Event::Loaded {
                response: toolbox_ipc::call(HostCommand::GetConfig).and_then(successful),
                force,
            },
            Work::Save {
                config,
                revision,
                kind,
            } => Event::Saved {
                response: toolbox_ipc::call(HostCommand::SaveConfig {
                    config: config.clone(),
                    expected_revision: revision,
                })
                .and_then(successful),
                config,
                kind,
            },
            Work::Discover {
                generation,
                query,
                code_path,
            } => {
                let result = toolbox_projects::discover(code_path.as_deref());
                match result {
                    Ok(items) => {
                        discovery_error = None;
                        projects = items;
                        if latest_query.load(Ordering::Relaxed) != generation {
                            continue;
                        }
                        let result = toolbox_projects::search(&projects, &query)
                            .into_iter()
                            .take(500)
                            .collect();
                        Event::Projects {
                            generation,
                            result: Ok(result),
                            total: projects.len(),
                        }
                    }
                    Err(error) => {
                        discovery_error = Some(error.clone());
                        projects.clear();
                        Event::Projects {
                            generation,
                            result: Err(error),
                            total: 0,
                        }
                    }
                }
            }
            Work::Search { generation, query } => {
                if latest_query.load(Ordering::Relaxed) != generation {
                    continue;
                }
                let result = discovery_error.clone().map_or_else(
                    || {
                        Ok(toolbox_projects::search(&projects, &query)
                            .into_iter()
                            .take(500)
                            .collect())
                    },
                    Err,
                );
                Event::Projects {
                    generation,
                    result,
                    total: projects.len(),
                }
            }
            Work::Open { id, code_path } => {
                let result = projects
                    .iter()
                    .find(|item| item.id == id)
                    .ok_or_else(|| {
                        "项目列表已变化，请刷新 / Project list changed; refresh and retry"
                            .to_owned()
                    })
                    .and_then(|item| toolbox_projects::open(item, code_path.as_deref()));
                Event::Opened(result)
            }
            Work::Toggle(id) => {
                Event::Toggled(toolbox_ipc::call(HostCommand::Toggle { id }).and_then(successful))
            }
            Work::Status => {
                Event::Status(toolbox_ipc::call(HostCommand::Status).and_then(successful))
            }
            Work::Dialog(kind, dialog) => match kind {
                DialogKind::Browse(kind) => {
                    let selected = native_dialog(|| {
                        if kind == 1 {
                            dialog.pick_folder()
                        } else {
                            dialog.pick_file()
                        }
                    });
                    let path = selected.and_then(|selected| {
                        selected
                            .map(|p| {
                                p.into_os_string().into_string().map_err(|_| {
                                    "路径不是有效的 Unicode / Path is not valid Unicode".to_owned()
                                })
                            })
                            .transpose()
                    });
                    Event::PathPicked { kind, path }
                }
                DialogKind::Import => Event::Imported(import_file(dialog)),
                DialogKind::Export => Event::Exported(export_file(dialog)),
            },
            Work::Link(index) => Event::LinkOpened(open_link(index)),
        };
        crate::post(event);
    }
}

fn import_file(dialog: rfd::FileDialog) -> Result<Option<ImportedConfig>, String> {
    let Some(path) = native_dialog(|| {
        dialog
            .add_filter("Toolbox / WinGuake", &["json", "ini"])
            .pick_file()
    })?
    else {
        return Ok(None);
    };
    let mut bytes = Vec::new();
    File::open(&path)
        .map_err(|e| e.to_string())?
        .take((MAX_IMPORT_BYTES + 1) as u64)
        .read_to_end(&mut bytes)
        .map_err(|e| e.to_string())?;
    if bytes.len() > MAX_IMPORT_BYTES {
        return Err("配置文件超过 1 MiB / Configuration exceeds 1 MiB".into());
    }
    let text = String::from_utf8(bytes)
        .map_err(|_| "请将配置文件保存为 UTF-8 / Save the configuration as UTF-8".to_owned())?;
    ImportedConfig::parse(
        &text,
        path.file_name()
            .unwrap_or_default()
            .to_string_lossy()
            .into_owned(),
    )
    .map(Some)
}

fn export_file(dialog: rfd::FileDialog) -> Result<Option<PathBuf>, String> {
    // Ask the host again so an export never silently contains a stale UI draft.
    let response = toolbox_ipc::call(HostCommand::GetConfig).and_then(successful)?;
    let config = response
        .config
        .ok_or_else(|| "Host returned no configuration".to_owned())?;
    let Some(path) = native_dialog(|| {
        dialog
            .add_filter("JSON", &["json"])
            .set_file_name("sean-toolbox.json")
            .save_file()
    })?
    else {
        return Ok(None);
    };
    let parent = path
        .parent()
        .ok_or_else(|| "Invalid export directory".to_owned())?;
    let mut file = tempfile::NamedTempFile::new_in(parent).map_err(|e| e.to_string())?;
    serde_json::to_writer_pretty(&mut file, &config).map_err(|e| e.to_string())?;
    file.write_all(b"\n").map_err(|e| e.to_string())?;
    file.as_file().sync_all().map_err(|e| e.to_string())?;
    file.persist(&path).map_err(|e| e.to_string())?;
    Ok(Some(path))
}

fn open_link(index: i32) -> Result<(), String> {
    let url = if index == 1 {
        "https://slint.dev"
    } else {
        "https://github.com/sean2077/sean-toolbox"
    };
    #[cfg(windows)]
    let mut command = {
        use std::os::windows::process::CommandExt;
        let mut command = Command::new("rundll32.exe");
        command
            .arg("url.dll,FileProtocolHandler")
            .creation_flags(0x0800_0000);
        command
    };
    #[cfg(target_os = "macos")]
    let mut command = Command::new("open");
    #[cfg(all(unix, not(target_os = "macos")))]
    let mut command = Command::new("xdg-open");
    let status = command
        .arg(url)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map_err(|e| e.to_string())?;
    if status.success() {
        Ok(())
    } else {
        Err(format!("浏览器打开失败 / Browser open failed: {status}"))
    }
}

pub fn serve(endpoint_id: String, stopping: Arc<AtomicBool>) {
    let result = (|| -> Result<(), String> {
        let endpoint =
            toolbox_ipc::current_user_endpoint(&endpoint_id).map_err(|e| e.to_string())?;
        let server = toolbox_ipc::PipeServer::bind(&endpoint).map_err(|e| e.to_string())?;
        while !stopping.load(Ordering::Relaxed) {
            let connection = server.accept().map_err(|e| e.to_string())?;
            if stopping.load(Ordering::Relaxed) {
                break;
            }
            let request = match connection.receive::<UiRequest>() {
                Ok(request) => request,
                Err(error) => {
                    let _ = connection.send(&Response::error(error.to_string()));
                    continue;
                }
            };
            let (reply, receive) = std::sync::mpsc::channel();
            if request.readiness_only {
                crate::post(Event::Ready { reply });
            } else {
                crate::post(Event::Show {
                    page: request.page,
                    reply,
                });
            }
            let response = receive
                .recv_timeout(std::time::Duration::from_secs(3))
                .unwrap_or_else(|_| Response::error("UI did not acknowledge activation"));
            let _ = connection.send(&response);
        }
        Ok(())
    })();
    if let Err(error) = result {
        crate::post(Event::ServerError(error));
    }
}

pub fn stop_server(endpoint_id: &str) {
    // Wake accept after the UI loop stops, allowing PipeServer::drop to remove
    // its own socket rather than leaving a new stale path for every UI launch.
    if let Ok(endpoint) = toolbox_ipc::current_user_endpoint(endpoint_id) {
        let _ = toolbox_ipc::connect_local(&endpoint, 200);
    }
}
