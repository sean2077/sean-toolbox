#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]

mod autostart;
mod hotkeys;
mod runtime;
mod ui_process;

use std::{
    fs::{self, OpenOptions},
    process::{Command, Stdio},
    thread,
    time::{Duration, Instant},
};
use toolbox_core::{
    HostCommand, Response, UiPage,
    storage::{ConfigStore, default_config_path},
};

fn main() {
    if let Err(error) = start() {
        eprintln!("Sean Toolbox: {error}");
        #[cfg(all(windows, not(debug_assertions)))]
        {
            use windows_sys::Win32::UI::WindowsAndMessaging::{MB_ICONERROR, MessageBoxW};
            let text: Vec<u16> = error.encode_utf16().chain(Some(0)).collect();
            let title: Vec<u16> = "Sean Toolbox".encode_utf16().chain(Some(0)).collect();
            if !std::env::args().any(|a| a == "--background") {
                unsafe {
                    MessageBoxW(
                        std::ptr::null_mut(),
                        text.as_ptr(),
                        title.as_ptr(),
                        MB_ICONERROR,
                    );
                }
            }
        }
        std::process::exit(1);
    }
}

fn start() -> Result<(), String> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    #[cfg(windows)]
    if !args.is_empty() && args[0] != "--background" {
        use windows_sys::Win32::System::Console::{ATTACH_PARENT_PROCESS, AttachConsole};
        unsafe {
            AttachConsole(ATTACH_PARENT_PROCESS);
        }
    }
    let command = match args.first().map(String::as_str) {
        Some("--help" | "-h" | "help") => {
            println!(
                "Sean Toolbox — 个人工具箱\n\nsean-toolbox                              打开最近项目\nsean-toolbox show [projects|windows|settings|diagnostics]\nsean-toolbox --background                 常驻托盘，不打开界面\nsean-toolbox toggle <profile-id>           切换应用窗口\nsean-toolbox status                       输出 JSON 诊断\nsean-toolbox config                       输出当前配置与版本\nsean-toolbox config-path                  输出配置路径\nsean-toolbox quit                         退出宿主与界面\nsean-toolbox --version\n\n首次默认热键：Ctrl+Alt+Space；WezTerm F3、VS Code F4、Chrome F5。\n配置可在界面修改。两份可执行文件必须保留在同一目录。"
            );
            return Ok(());
        }
        Some("--version" | "-V") => {
            println!("Sean Toolbox {}", env!("CARGO_PKG_VERSION"));
            return Ok(());
        }
        Some("config-path") => {
            println!("{}", default_config_path()?.display());
            return Ok(());
        }
        Some("--background") if args.len() == 1 => return run_host(),
        None => HostCommand::Show(UiPage::Projects),
        Some("show") if args.len() <= 2 => HostCommand::Show(parse_page(
            args.get(1).map(String::as_str).unwrap_or("projects"),
        )?),
        Some("toggle") if args.len() == 2 => HostCommand::Toggle {
            id: args[1].clone(),
        },
        Some("status") if args.len() == 1 => HostCommand::Status,
        Some("config") if args.len() == 1 => HostCommand::GetConfig,
        Some("quit") if args.len() == 1 => HostCommand::Quit,
        _ => return Err("Unknown arguments. Run sean-toolbox --help".into()),
    };
    let response = if quick_ping() {
        toolbox_ipc::call(command.clone())
    } else {
        Err("Host is not running".into())
    };
    match response {
        Ok(response) => print_response(response),
        Err(_) if matches!(command, HostCommand::Show(_)) => {
            launch_host()?;
            let started = Instant::now();
            loop {
                if quick_ping() {
                    return print_response(toolbox_ipc::call(command.clone())?);
                }
                if started.elapsed() >= Duration::from_secs(10) {
                    return Err(format!(
                        "宿主启动失败 / Host failed to start; inspect {}",
                        default_config_path()?.with_file_name("host.log").display()
                    ));
                }
                thread::sleep(Duration::from_millis(80));
            }
        }
        Err(error) => Err(format!(
            "宿主未运行或无法连接，请先启动 Sean Toolbox / Host unavailable: {error}"
        )),
    }
}

fn parse_page(page: &str) -> Result<UiPage, String> {
    match page {
        "projects" => Ok(UiPage::Projects),
        "windows" => Ok(UiPage::Windows),
        "settings" => Ok(UiPage::Settings),
        "diagnostics" => Ok(UiPage::Diagnostics),
        _ => Err(format!("Unknown page: {page}")),
    }
}

fn print_response(response: Response) -> Result<(), String> {
    println!(
        "{}",
        serde_json::to_string_pretty(&response).map_err(|e| e.to_string())?
    );
    if response.ok {
        Ok(())
    } else {
        Err(response.message)
    }
}

fn launch_host() -> Result<(), String> {
    let config_path = default_config_path()?;
    fs::create_dir_all(
        config_path
            .parent()
            .ok_or("Missing configuration directory")?,
    )
    .map_err(|e| e.to_string())?;
    let log_path = config_path.with_file_name("host.log");
    if fs::metadata(&log_path).is_ok_and(|m| m.len() > 1024 * 1024) {
        let _ = fs::rename(&log_path, log_path.with_extension("log.1"));
    }
    let log = OpenOptions::new()
        .create(true)
        .append(true)
        .open(&log_path)
        .map_err(|e| e.to_string())?;
    let mut command = Command::new(std::env::current_exe().map_err(|e| e.to_string())?);
    command
        .arg("--background")
        .stdin(Stdio::null())
        .stdout(log.try_clone().map_err(|e| e.to_string())?)
        .stderr(log);
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        command.creation_flags(0x08000000 | 0x00000200);
    }
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        unsafe {
            command.pre_exec(|| {
                if libc::setsid() == -1 {
                    Err(std::io::Error::last_os_error())
                } else {
                    Ok(())
                }
            });
        }
    }
    let _child = command.spawn().map_err(|e| e.to_string())?;
    Ok(())
}

fn run_host() -> Result<(), String> {
    if quick_ping() {
        return Ok(());
    }
    let endpoint = toolbox_ipc::current_user_endpoint(toolbox_core::HOST_ENDPOINT)
        .map_err(|e| e.to_string())?;
    let path = default_config_path()?;
    let parent = path.parent().ok_or("Missing configuration directory")?;
    fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    #[cfg(unix)]
    let lock_path = std::path::Path::new(&endpoint).with_extension("lock");
    #[cfg(windows)]
    let lock_path = {
        let directory = std::path::PathBuf::from(
            std::env::var_os("LOCALAPPDATA").ok_or("LOCALAPPDATA is unavailable")?,
        )
        .join("SeanToolbox");
        fs::create_dir_all(&directory).map_err(|e| e.to_string())?;
        directory.join("host.lock")
    };
    let lock = OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(lock_path)
        .map_err(|e| e.to_string())?;
    lock.try_lock()
        .map_err(|_| "Sean Toolbox is already starting or running")?;
    let store = ConfigStore::new(path);
    let config = store.load()?;
    config.validate()?;
    if !store.path().exists() {
        store.save(&config)?;
    }
    let server = toolbox_ipc::PipeServer::bind(&endpoint).map_err(|e| e.to_string())?;
    let result = runtime::run(config, store, server, None);
    drop(lock);
    result
}

fn quick_ping() -> bool {
    toolbox_ipc::current_user_endpoint(toolbox_core::HOST_ENDPOINT)
        .ok()
        .is_some_and(|endpoint| {
            toolbox_ipc::request::<_, Response>(
                &endpoint,
                &toolbox_core::Request::new(HostCommand::Ping),
                120,
            )
            .is_ok_and(|response| {
                #[cfg(windows)]
                if response.ok
                    && let Some(status) = &response.status
                {
                    unsafe {
                        windows_sys::Win32::UI::WindowsAndMessaging::AllowSetForegroundWindow(
                            status.process_id,
                        );
                    }
                }
                response.ok
            })
        })
}
