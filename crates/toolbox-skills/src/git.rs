use crate::files;
use std::{
    ffi::{OsStr, OsString},
    fs,
    io::Read,
    path::{Path, PathBuf},
    process::{Child, Command, Stdio},
    sync::atomic::{AtomicBool, Ordering},
    thread,
    time::{Duration, Instant},
};
use toolbox_core::skills::SkillRepository;

const MAX_ARCHIVE: u64 = 64 * 1024 * 1024;
const MAX_ENTRIES: usize = 20_000;
const MAX_CHECKOUT_BYTES: u64 = 256 * 1024 * 1024;
const MAX_GIT_STDOUT: u64 = 4 * 1024 * 1024;

pub(crate) struct Checkout {
    pub _temporary: tempfile::TempDir,
    pub tree: PathBuf,
    pub revision: String,
    pub blocked: Vec<String>,
}

pub(crate) trait Fetcher: Send {
    fn fetch(
        &self,
        repository: &SkillRepository,
        root: &Path,
        cancel: &AtomicBool,
    ) -> Result<Checkout, String>;
}

pub(crate) struct GitFetcher;

impl Fetcher for GitFetcher {
    fn fetch(
        &self,
        repository: &SkillRepository,
        root: &Path,
        cancel: &AtomicBool,
    ) -> Result<Checkout, String> {
        let temporary = tempfile::Builder::new()
            .prefix("checkout-")
            .tempdir_in(root)
            .map_err(|e| e.to_string())?;
        let bare = temporary.path().join("repository.git");
        let mut args: Vec<OsString> = [
            "clone",
            "--bare",
            "--depth=1",
            "--single-branch",
            "--no-tags",
        ]
        .into_iter()
        .map(OsString::from)
        .collect();
        if let Some(branch) = &repository.branch {
            args.extend([OsString::from("--branch"), OsString::from(branch)]);
        }
        args.extend([
            OsString::from("--"),
            OsString::from(&repository.url),
            bare.clone().into_os_string(),
        ]);
        run(temporary.path(), args, cancel, None)?;
        let revision = run(
            temporary.path(),
            [
                OsString::from("--git-dir"),
                bare.clone().into_os_string(),
                OsString::from("rev-parse"),
                OsString::from("HEAD"),
            ],
            cancel,
            None,
        )?;
        let revision = revision.trim().to_owned();
        if !(revision.len() == 40 || revision.len() == 64)
            || !revision.bytes().all(|c| c.is_ascii_hexdigit())
        {
            return Err("Git returned an invalid commit identifier".into());
        }
        archive_repository(temporary, &bare, revision, cancel)
    }
}

pub(crate) fn archive_repository(
    temporary: tempfile::TempDir,
    bare: &Path,
    revision: String,
    cancel: &AtomicBool,
) -> Result<Checkout, String> {
    let modes = run(
        temporary.path(),
        [
            OsString::from("--git-dir"),
            bare.as_os_str().to_owned(),
            OsString::from("ls-tree"),
            OsString::from("-r"),
            OsString::from("-z"),
            OsString::from(&revision),
        ],
        cancel,
        None,
    )?;
    let submodules: Vec<_> = modes
        .split('\0')
        .filter_map(|line| line.split_once('\t'))
        .filter(|(mode, _)| mode.starts_with("160000 "))
        .map(|(_, path)| path.to_owned())
        .collect();
    let archive = temporary.path().join("source.tar");
    run(
        temporary.path(),
        [
            OsString::from("--git-dir"),
            bare.as_os_str().to_owned(),
            OsString::from("archive"),
            OsString::from("--format=tar"),
            OsString::from(format!("--output={}", archive.display())),
            OsString::from(&revision),
        ],
        cancel,
        Some(&archive),
    )?;
    let mut checkout = read_archive(temporary, archive, revision, cancel)?;
    checkout.blocked.extend(submodules);
    Ok(checkout)
}

/// `git archive` does not invoke checkout filters, hooks, or submodule commands.
/// Extract each member ourselves; tar's path/link restoration APIs are not used.
pub(crate) fn read_archive(
    temporary: tempfile::TempDir,
    archive_path: PathBuf,
    revision: String,
    cancel: &AtomicBool,
) -> Result<Checkout, String> {
    if fs::metadata(&archive_path)
        .map_err(|e| e.to_string())?
        .len()
        > MAX_ARCHIVE
    {
        return Err("Repository snapshot exceeds 64 MiB".into());
    }
    let tree = temporary.path().join("tree");
    fs::create_dir(&tree).map_err(|e| e.to_string())?;
    let file = fs::File::open(&archive_path).map_err(|e| e.to_string())?;
    let mut archive = tar::Archive::new(file);
    let mut size = 0u64;
    let mut count = 0;
    let mut blocked = Vec::new();
    for entry in archive.entries().map_err(|e| e.to_string())? {
        if cancel.load(Ordering::Acquire) {
            return Err("Skills operation was cancelled".into());
        }
        let mut entry = entry.map_err(|e| e.to_string())?;
        count += 1;
        if count > MAX_ENTRIES {
            return Err("Repository contains more than 20000 archive entries".into());
        }
        let path = entry.path().map_err(|e| e.to_string())?;
        let text = path
            .to_str()
            .ok_or("Repository contains a non-Unicode path")?
            .trim_end_matches('/')
            .to_owned();
        if text.is_empty() {
            continue;
        }
        let relative = match files::relative(&text) {
            Ok(path) => path,
            Err(_) => {
                blocked.push(text);
                continue;
            }
        };
        let kind = entry.header().entry_type();
        if !kind.is_dir() && !kind.is_file() {
            blocked.push(text);
            continue;
        }
        let destination = tree.join(relative);
        let parent = destination
            .parent()
            .ok_or("Missing archive member parent")?;
        files::checked_path(&tree, parent, true)?;
        if kind.is_dir() {
            if !destination.exists() {
                fs::create_dir(&destination).map_err(|e| e.to_string())?;
            }
            files::metadata(&destination)?;
            continue;
        }
        let entry_size = entry.size();
        size = size.saturating_add(entry_size);
        if size > MAX_ARCHIVE {
            return Err("Repository files exceed 64 MiB".into());
        }
        // Large non-skill assets may exist in a catalog; don't materialize them.
        if entry_size > 8 * 1024 * 1024 {
            blocked.push(text);
            continue;
        }
        let mut output = fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&destination)
            .map_err(|e| e.to_string())?;
        let copied = std::io::copy(&mut (&mut entry).take(entry_size + 1), &mut output)
            .map_err(|e| e.to_string())?;
        if copied != entry_size {
            return Err("Truncated repository archive".into());
        }
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let executable = entry.header().mode().map_err(|e| e.to_string())? & 0o111 != 0;
            fs::set_permissions(
                &destination,
                fs::Permissions::from_mode(if executable { 0o755 } else { 0o644 }),
            )
            .map_err(|e| e.to_string())?;
        }
    }
    Ok(Checkout {
        _temporary: temporary,
        tree,
        revision,
        blocked,
    })
}

pub(crate) fn run<I, S>(
    directory: &Path,
    args: I,
    cancel: &AtomicBool,
    archive: Option<&Path>,
) -> Result<String, String>
where
    I: IntoIterator<Item = S>,
    S: AsRef<OsStr>,
{
    if cancel.load(Ordering::Acquire) {
        return Err("Skills operation was cancelled".into());
    }
    let log = tempfile::NamedTempFile::new_in(directory).map_err(|e| e.to_string())?;
    let errors = tempfile::NamedTempFile::new_in(directory).map_err(|e| e.to_string())?;
    let hooks = directory.join("empty-hooks");
    fs::create_dir_all(&hooks).map_err(|e| e.to_string())?;
    let mut command = Command::new("git");
    for (key, _) in std::env::vars_os() {
        if key.to_string_lossy().starts_with("GIT_") {
            command.env_remove(key);
        }
    }
    command
        .current_dir(directory)
        .env("GIT_TERMINAL_PROMPT", "0")
        .env("GCM_INTERACTIVE", "never")
        .env("GIT_ASKPASS", "")
        .env("SSH_ASKPASS", "")
        .arg("-c")
        .arg(format!("core.hooksPath={}", hooks.display()))
        .args([
            "-c",
            "core.fsmonitor=false",
            "-c",
            "protocol.allow=never",
            "-c",
            "protocol.https.allow=always",
            "-c",
            "submodule.recurse=false",
            "-c",
            "credential.interactive=false",
            "-c",
            "http.lowSpeedLimit=1",
            "-c",
            "http.lowSpeedTime=30",
        ])
        .args(args)
        .stdin(Stdio::null())
        .stdout(log.reopen().map_err(|e| e.to_string())?)
        .stderr(errors.reopen().map_err(|e| e.to_string())?);
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        command.process_group(0);
    }
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        // Suspend before attaching the job: even a short-lived Git command or
        // an immediately spawned credential/HTTPS helper stays in the job.
        command.creation_flags(0x0800_0000 | 0x0000_0004);
    }
    let mut child = command
        .spawn()
        .map_err(|e| format!("Cannot start Git; install Git and ensure it is on PATH: {e}"))?;
    #[cfg(windows)]
    let _job = match Job::attach(&child) {
        Ok(job) => {
            if let Err(error) = resume(&child) {
                let _ = child.kill();
                let _ = child.wait();
                return Err(error);
            }
            job
        }
        Err(error) => {
            let _ = child.kill();
            let _ = child.wait();
            return Err(error);
        }
    };
    let start = Instant::now();
    let mut last_size_check = Instant::now();
    let status = loop {
        let check_size = last_size_check.elapsed() >= Duration::from_millis(400);
        let checkout_exceeded = check_size && checkout_too_large(directory);
        if check_size {
            last_size_check = Instant::now();
        }
        let exceeded = checkout_exceeded
            || log
                .as_file()
                .metadata()
                .is_ok_and(|m| m.len() > MAX_GIT_STDOUT)
            || errors
                .as_file()
                .metadata()
                .is_ok_and(|m| m.len() > 64 * 1024)
            || archive.is_some_and(|p| fs::metadata(p).is_ok_and(|m| m.len() > MAX_ARCHIVE));
        if cancel.load(Ordering::Acquire) || start.elapsed() > Duration::from_secs(90) || exceeded {
            terminate(&mut child);
            return Err(if exceeded {
                "Git output exceeded the repository size limit"
            } else if cancel.load(Ordering::Acquire) {
                "Skills operation was cancelled"
            } else {
                "Git operation timed out after 90 seconds"
            }
            .into());
        }
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) => thread::sleep(Duration::from_millis(40)),
            Err(error) => {
                terminate(&mut child);
                return Err(format!("Cannot wait for Git: {error}"));
            }
        }
    };
    if checkout_too_large(directory) {
        return Err("Git working files exceed the 256 MiB checkout budget".into());
    }
    let output = files::read_limited(log.path(), MAX_GIT_STDOUT)?;
    if !status.success() {
        let stderr = files::read_limited(errors.path(), 64 * 1024)?;
        return Err(format!(
            "Git failed: {}",
            files::limited(&String::from_utf8_lossy(&stderr), 500)
        ));
    }
    String::from_utf8(output).map_err(|_| "Git returned invalid UTF-8".into())
}

fn checkout_too_large(root: &Path) -> bool {
    let mut pending = vec![root.to_owned()];
    let mut total = 0u64;
    let mut count = 0;
    while let Some(path) = pending.pop() {
        let Ok(entries) = fs::read_dir(path) else {
            continue;
        };
        for entry in entries.flatten() {
            count += 1;
            if count > MAX_ENTRIES {
                return true;
            }
            let Ok(metadata) = fs::symlink_metadata(entry.path()) else {
                continue;
            };
            #[cfg(windows)]
            {
                use std::os::windows::fs::MetadataExt;
                if metadata.file_attributes() & 0x400 != 0 {
                    return true;
                }
            }
            if metadata.is_file() {
                total = total.saturating_add(metadata.len());
                if total > MAX_CHECKOUT_BYTES {
                    return true;
                }
            } else if metadata.is_dir() && !metadata.file_type().is_symlink() {
                pending.push(entry.path());
            }
        }
    }
    false
}

fn terminate(child: &mut Child) {
    #[cfg(unix)]
    unsafe {
        // Every Git command owns a separate process group, including HTTPS helpers.
        libc::kill(-(child.id() as i32), libc::SIGKILL);
    }
    let _ = child.kill();
    let _ = child.wait();
}

#[cfg(windows)]
struct Job(windows_sys::Win32::Foundation::HANDLE);

#[cfg(windows)]
impl Job {
    fn attach(child: &Child) -> Result<Self, String> {
        use std::os::windows::io::AsRawHandle;
        use windows_sys::Win32::{
            Foundation::CloseHandle,
            System::JobObjects::{
                AssignProcessToJobObject, CreateJobObjectW, JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
                JOBOBJECT_EXTENDED_LIMIT_INFORMATION, JobObjectExtendedLimitInformation,
                SetInformationJobObject,
            },
        };
        unsafe {
            let handle = CreateJobObjectW(std::ptr::null(), std::ptr::null());
            if handle.is_null() {
                return Err(format!(
                    "Cannot create Git process job: {}",
                    std::io::Error::last_os_error()
                ));
            }
            let mut info: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = std::mem::zeroed();
            info.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
            if SetInformationJobObject(
                handle,
                JobObjectExtendedLimitInformation,
                &info as *const _ as *const _,
                std::mem::size_of_val(&info) as u32,
            ) == 0
                || AssignProcessToJobObject(handle, child.as_raw_handle() as _) == 0
            {
                let error = std::io::Error::last_os_error();
                CloseHandle(handle);
                return Err(format!("Cannot contain Git process: {error}"));
            }
            Ok(Self(handle))
        }
    }
}

#[cfg(windows)]
impl Drop for Job {
    fn drop(&mut self) {
        unsafe {
            windows_sys::Win32::Foundation::CloseHandle(self.0);
        }
    }
}

#[cfg(windows)]
fn resume(child: &Child) -> Result<(), String> {
    use windows_sys::Win32::{
        Foundation::{CloseHandle, INVALID_HANDLE_VALUE},
        System::{
            Diagnostics::ToolHelp::{
                CreateToolhelp32Snapshot, TH32CS_SNAPTHREAD, THREADENTRY32, Thread32First,
                Thread32Next,
            },
            Threading::{OpenThread, ResumeThread, THREAD_SUSPEND_RESUME},
        },
    };
    unsafe {
        let snapshot = CreateToolhelp32Snapshot(TH32CS_SNAPTHREAD, 0);
        if snapshot == INVALID_HANDLE_VALUE {
            return Err(format!(
                "Cannot inspect suspended Git thread: {}",
                std::io::Error::last_os_error()
            ));
        }
        let mut entry = THREADENTRY32 {
            dwSize: std::mem::size_of::<THREADENTRY32>() as u32,
            ..Default::default()
        };
        let mut found = Thread32First(snapshot, &mut entry) != 0;
        let mut result = Err("Cannot locate suspended Git thread".to_owned());
        while found {
            if entry.th32OwnerProcessID == child.id() {
                let thread = OpenThread(THREAD_SUSPEND_RESUME, 0, entry.th32ThreadID);
                if thread.is_null() {
                    result = Err(format!(
                        "Cannot open suspended Git thread: {}",
                        std::io::Error::last_os_error()
                    ));
                } else {
                    result = match ResumeThread(thread) {
                        1 => Ok(()),
                        u32::MAX => Err(format!(
                            "Cannot resume Git: {}",
                            std::io::Error::last_os_error()
                        )),
                        count => Err(format!(
                            "Git startup thread had an unexpected suspend count: {count}"
                        )),
                    };
                    CloseHandle(thread);
                }
                break;
            }
            found = Thread32Next(snapshot, &mut entry) != 0;
        }
        CloseHandle(snapshot);
        result
    }
}

pub fn repository_url(input: &str) -> Result<String, String> {
    let input = input.trim();
    let short = input
        .strip_prefix("https://github.com/")
        .unwrap_or(input)
        .trim_end_matches('/')
        .trim_end_matches(".git");
    let parts: Vec<_> = short.split('/').collect();
    if parts.len() != 2
        || parts.iter().any(|part| {
            part.is_empty()
                || part.len() > 100
                || part.starts_with(['.', '-'])
                || !part
                    .bytes()
                    .all(|c| c.is_ascii_alphanumeric() || b"._-".contains(&c))
        })
    {
        return Err("Use owner/repo or https://github.com/owner/repo (without credentials, query, or subpath)".into());
    }
    Ok(format!("https://github.com/{}/{}.git", parts[0], parts[1]))
}

pub fn branch(input: Option<String>) -> Result<Option<String>, String> {
    let Some(input) = input
        .map(|value| value.trim().to_owned())
        .filter(|value| !value.is_empty())
    else {
        return Ok(None);
    };
    if input.len() > 200
        || input.starts_with(['-', '/', '.'])
        || input.ends_with(['/', '.'])
        || input.ends_with(".lock")
        || input.contains("..")
        || input.contains("@{")
        || input.contains("//")
        || input
            .chars()
            .any(|c| c.is_control() || c.is_whitespace() || "~^:?*[\\".contains(c))
        || input
            .split('/')
            .any(|part| part.starts_with('.') || part.ends_with(".lock"))
    {
        return Err("Invalid Git branch name".into());
    }
    Ok(Some(input))
}
