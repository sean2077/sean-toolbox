//! Current-user local transport for Recentry host/UI messages.
//!
//! Windows uses named pipes. Unix platforms use owner-only Unix-domain
//! sockets and verify the peer user where the operating system supports it.
//!
//! Each connection has one request/response owner. The synchronous handle must
//! not be cloned for concurrent reads and writes; doing so can deadlock.

use serde::{Serialize, de::DeserializeOwned};

pub const MAX_MESSAGE_BYTES: usize = 1024 * 1024;

#[derive(Debug, thiserror::Error)]
pub enum IpcError {
    #[error("IPC is unsupported on this platform")]
    Unsupported,
    #[error("invalid current-user endpoint: {0}")]
    InvalidEndpoint(String),
    #[error("message exceeds the IPC limit")]
    MessageTooLarge,
    #[error("{operation} failed with Windows error {code}")]
    Windows { operation: &'static str, code: u32 },
    #[error("{operation} failed: {source}")]
    Io {
        operation: &'static str,
        #[source]
        source: std::io::Error,
    },
    #[error("protocol encoding failed: {0}")]
    Protocol(#[from] serde_json::Error),
}

pub fn current_user_endpoint(id: &str) -> Result<String, IpcError> {
    if id.is_empty()
        || id.len() > 64
        || !id
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
    {
        return Err(IpcError::InvalidEndpoint(
            "the id must contain 1-64 ASCII letters, digits, hyphens, or underscores".to_owned(),
        ));
    }

    #[cfg(windows)]
    {
        return Ok(format!(r"\\.\pipe\{id}-{}", platform::current_user_sid()?));
    }

    #[cfg(unix)]
    {
        use std::{env, os::unix::ffi::OsStrExt, path::PathBuf};

        let uid = unsafe { libc::geteuid() };
        let runtime_root = env::var_os("XDG_RUNTIME_DIR")
            .map(PathBuf::from)
            .filter(|path| path.is_absolute());
        let directory = if let Some(root) = runtime_root {
            root.join("sean-toolbox")
        } else {
            PathBuf::from("/tmp").join(format!("sean-toolbox-{uid}"))
        };
        ensure_private_directory(&directory, uid)?;
        let endpoint = directory.join(format!("{id}.sock"));
        if endpoint.as_os_str().as_bytes().len() > 100 {
            return Err(IpcError::InvalidEndpoint(
                "the Unix socket path exceeds 100 bytes".to_owned(),
            ));
        }
        return Ok(endpoint.to_string_lossy().into_owned());
    }

    #[allow(unreachable_code)]
    Err(IpcError::Unsupported)
}

#[cfg(unix)]
fn ensure_private_directory(path: &std::path::Path, uid: u32) -> Result<(), IpcError> {
    use std::{
        fs,
        os::unix::fs::{DirBuilderExt, MetadataExt, PermissionsExt},
    };

    match fs::symlink_metadata(path) {
        Ok(metadata) => {
            if metadata.file_type().is_symlink() || !metadata.is_dir() || metadata.uid() != uid {
                return Err(IpcError::InvalidEndpoint(
                    "the runtime directory is not an owner-controlled directory".to_owned(),
                ));
            }
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            match fs::DirBuilder::new().mode(0o700).create(path) {
                Ok(()) => {}
                Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {}
                Err(source) => {
                    return Err(IpcError::Io {
                        operation: "create runtime directory",
                        source,
                    });
                }
            }
            // A competing first-start thread may have created the path. Check
            // its type and owner before chmod, which would otherwise follow a symlink.
            let metadata = fs::symlink_metadata(path).map_err(|source| IpcError::Io {
                operation: "inspect created runtime directory",
                source,
            })?;
            if metadata.file_type().is_symlink() || !metadata.is_dir() || metadata.uid() != uid {
                return Err(IpcError::InvalidEndpoint(
                    "the runtime directory is not owner-controlled".into(),
                ));
            }
        }
        Err(source) => {
            return Err(IpcError::Io {
                operation: "inspect runtime directory",
                source,
            });
        }
    }
    fs::set_permissions(path, fs::Permissions::from_mode(0o700)).map_err(|source| {
        IpcError::Io {
            operation: "secure runtime directory",
            source,
        }
    })?;
    let metadata = fs::symlink_metadata(path).map_err(|source| IpcError::Io {
        operation: "verify runtime directory",
        source,
    })?;
    if metadata.file_type().is_symlink()
        || !metadata.is_dir()
        || metadata.uid() != uid
        || metadata.mode() & 0o077 != 0
    {
        return Err(IpcError::InvalidEndpoint(
            "the runtime directory permissions are not owner-only".to_owned(),
        ));
    }
    Ok(())
}

#[cfg(windows)]
mod platform {
    use std::{
        cell::Cell,
        ffi::c_void,
        marker::PhantomData,
        mem::size_of,
        ptr::{null, null_mut},
        thread,
        time::{Duration, Instant},
    };

    use serde::{Serialize, de::DeserializeOwned};
    use windows_sys::Win32::{
        Foundation::{
            CloseHandle, ERROR_FILE_NOT_FOUND, ERROR_IO_INCOMPLETE, ERROR_IO_PENDING,
            ERROR_PIPE_BUSY, ERROR_PIPE_CONNECTED, GetLastError, HANDLE, INVALID_HANDLE_VALUE,
            LocalFree, WAIT_OBJECT_0, WAIT_TIMEOUT,
        },
        Security::{
            Authorization::{
                ConvertSidToStringSidW, ConvertStringSecurityDescriptorToSecurityDescriptorW,
                SDDL_REVISION_1,
            },
            GetTokenInformation, PSECURITY_DESCRIPTOR, SECURITY_ATTRIBUTES, TOKEN_QUERY,
            TOKEN_USER, TokenUser,
        },
        Storage::FileSystem::{
            CreateFileW, FILE_FLAG_OVERLAPPED, OPEN_EXISTING, PIPE_ACCESS_DUPLEX, ReadFile,
            SECURITY_IDENTIFICATION, SECURITY_SQOS_PRESENT, WriteFile,
        },
        System::{
            IO::{CancelIoEx, GetOverlappedResult, OVERLAPPED},
            Pipes::{
                ConnectNamedPipe, CreateNamedPipeW, PIPE_REJECT_REMOTE_CLIENTS,
                PIPE_UNLIMITED_INSTANCES, PIPE_WAIT, WaitNamedPipeW,
            },
            Threading::{
                CreateEventW, GetCurrentProcess, INFINITE, OpenProcessToken, WaitForSingleObject,
            },
        },
    };

    use super::{IpcError, MAX_MESSAGE_BYTES};

    struct Handle(HANDLE);
    impl Drop for Handle {
        fn drop(&mut self) {
            unsafe {
                CloseHandle(self.0);
            }
        }
    }

    /// Query the process token, never USERNAME or another user-controlled
    /// environment variable. Different Windows users get different pipe names.
    pub(super) fn current_user_sid() -> Result<String, IpcError> {
        let mut token = null_mut();
        if unsafe { OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token) } == 0 {
            return Err(last_error("OpenProcessToken"));
        }
        let token = Handle(token);
        let mut bytes = 0;
        unsafe {
            GetTokenInformation(token.0, TokenUser, null_mut(), 0, &mut bytes);
        }
        if bytes < size_of::<TOKEN_USER>() as u32 {
            return Err(last_error("GetTokenInformation size"));
        }
        // A usize allocation is sufficiently aligned for TOKEN_USER and SID.
        let mut storage = vec![0usize; (bytes as usize).div_ceil(size_of::<usize>())];
        if unsafe {
            GetTokenInformation(
                token.0,
                TokenUser,
                storage.as_mut_ptr().cast(),
                bytes,
                &mut bytes,
            )
        } == 0
        {
            return Err(last_error("GetTokenInformation"));
        }
        let user = unsafe { &*storage.as_ptr().cast::<TOKEN_USER>() };
        let mut sid = null_mut();
        if unsafe { ConvertSidToStringSidW(user.User.Sid, &mut sid) } == 0 {
            return Err(last_error("ConvertSidToStringSidW"));
        }
        // The conversion API guarantees a terminated, LocalAlloc-owned string.
        let mut length = 0;
        unsafe {
            while *sid.add(length) != 0 {
                length += 1;
            }
        }
        let value = String::from_utf16_lossy(unsafe { std::slice::from_raw_parts(sid, length) });
        unsafe {
            LocalFree(sid.cast());
        }
        Ok(value)
    }

    pub struct PipeServer {
        name: Vec<u16>,
    }

    pub struct PipeConnection {
        handle: HANDLE,
        timeout: Duration,
        poisoned: Cell<bool>,
        _not_sync: PhantomData<Cell<()>>,
    }
    // One connection has one owner; concurrent reads/writes remain prohibited
    // by !Sync. Every overlapped operation is completed/cancelled before return.
    unsafe impl Send for PipeConnection {}
    impl Drop for PipeConnection {
        fn drop(&mut self) {
            unsafe {
                CloseHandle(self.handle);
            }
        }
    }

    struct SecurityDescriptor(PSECURITY_DESCRIPTOR);
    impl SecurityDescriptor {
        fn current_owner_only() -> Result<Self, IpcError> {
            let sid = current_user_sid()?;
            // An explicit token SID avoids OW resolving to the Administrators
            // group when an elevated process creates the pipe.
            let sddl = wide(&format!("O:{sid}D:P(A;;GA;;;{sid})"));
            let mut descriptor = null_mut();
            if unsafe {
                ConvertStringSecurityDescriptorToSecurityDescriptorW(
                    sddl.as_ptr(),
                    SDDL_REVISION_1,
                    &mut descriptor,
                    null_mut(),
                )
            } == 0
            {
                Err(last_error("security descriptor"))
            } else {
                Ok(Self(descriptor))
            }
        }
        fn attributes(&mut self) -> SECURITY_ATTRIBUTES {
            SECURITY_ATTRIBUTES {
                nLength: size_of::<SECURITY_ATTRIBUTES>() as u32,
                lpSecurityDescriptor: self.0.cast::<c_void>(),
                bInheritHandle: 0,
            }
        }
    }
    impl Drop for SecurityDescriptor {
        fn drop(&mut self) {
            unsafe {
                LocalFree(self.0.cast());
            }
        }
    }

    /// A pending operation must retain its OVERLAPPED and transfer buffer until
    /// the kernel acknowledges completion. On timeout, cancellation is followed
    /// by GetOverlappedResult(wait=true) before either allocation is released.
    struct Operation {
        handle: HANDLE,
        event: Handle,
        state: Box<OVERLAPPED>,
        pending: bool,
    }
    impl Operation {
        fn new(handle: HANDLE) -> Result<Self, IpcError> {
            let event = unsafe { CreateEventW(null(), 1, 0, null()) };
            if event.is_null() {
                return Err(last_error("CreateEventW"));
            }
            let mut state: Box<OVERLAPPED> = Box::new(unsafe { std::mem::zeroed() });
            state.hEvent = event;
            Ok(Self {
                handle,
                event: Handle(event),
                state,
                pending: false,
            })
        }
        fn complete(
            &mut self,
            immediate: bool,
            timeout: Option<Duration>,
            operation: &'static str,
        ) -> Result<u32, IpcError> {
            if !immediate {
                let code = unsafe { GetLastError() };
                if code != ERROR_IO_PENDING {
                    return Err(IpcError::Windows { operation, code });
                }
                self.pending = true;
                let milliseconds = timeout
                    .map(|t| t.as_millis().min(u128::from(INFINITE - 1)) as u32)
                    .unwrap_or(INFINITE);
                let waited = unsafe { WaitForSingleObject(self.event.0, milliseconds) };
                if waited == WAIT_TIMEOUT {
                    return Err(IpcError::Io {
                        operation,
                        source: std::io::Error::new(
                            std::io::ErrorKind::TimedOut,
                            "IPC operation timed out",
                        ),
                    });
                }
                if waited != WAIT_OBJECT_0 {
                    return Err(last_error("WaitForSingleObject"));
                }
            }
            let mut transferred = 0;
            if unsafe { GetOverlappedResult(self.handle, &*self.state, &mut transferred, 0) } == 0 {
                let code = unsafe { GetLastError() };
                if code != ERROR_IO_INCOMPLETE {
                    self.pending = false;
                }
                return Err(IpcError::Windows { operation, code });
            }
            self.pending = false;
            Ok(transferred)
        }
    }
    impl Drop for Operation {
        fn drop(&mut self) {
            if self.pending {
                unsafe {
                    CancelIoEx(self.handle, &*self.state);
                    let mut transferred = 0;
                    // Cancellation can race with success. Both results mean the
                    // kernel has finished using the OVERLAPPED and its buffer.
                    GetOverlappedResult(self.handle, &*self.state, &mut transferred, 1);
                }
            }
        }
    }

    fn validate_name(name: &str) -> Result<(), IpcError> {
        if !name.starts_with(r"\\.\pipe\") || name.len() > 240 || name.contains('\0') {
            Err(IpcError::InvalidEndpoint(
                "expected a local Windows named pipe of at most 240 bytes".into(),
            ))
        } else {
            Ok(())
        }
    }
    impl PipeServer {
        /// Prepares the endpoint. Named-pipe instances are created by accept;
        /// process-level single-instance ownership is enforced by the host lock.
        pub fn bind(name: &str) -> Result<Self, IpcError> {
            validate_name(name)?;
            Ok(Self { name: wide(name) })
        }
        pub fn accept(&self) -> Result<PipeConnection, IpcError> {
            let mut descriptor = SecurityDescriptor::current_owner_only()?;
            let attributes = descriptor.attributes();
            let handle = unsafe {
                CreateNamedPipeW(
                    self.name.as_ptr(),
                    PIPE_ACCESS_DUPLEX | FILE_FLAG_OVERLAPPED,
                    PIPE_WAIT | PIPE_REJECT_REMOTE_CLIENTS,
                    PIPE_UNLIMITED_INSTANCES,
                    64 * 1024,
                    64 * 1024,
                    5_000,
                    &attributes,
                )
            };
            if handle == INVALID_HANDLE_VALUE {
                return Err(last_error("CreateNamedPipeW"));
            }
            let connection = PipeConnection {
                handle,
                timeout: Duration::from_secs(5),
                poisoned: Cell::new(false),
                _not_sync: PhantomData,
            };
            let mut operation = Operation::new(handle)?;
            let connected = unsafe { ConnectNamedPipe(handle, &mut *operation.state) };
            if connected == 0 && unsafe { GetLastError() } == ERROR_PIPE_CONNECTED {
                return Ok(connection);
            }
            // An accept may wait indefinitely; once connected, every read and
            // write uses the bounded connection timeout.
            operation.complete(connected != 0, None, "ConnectNamedPipe")?;
            Ok(connection)
        }
    }

    impl PipeConnection {
        pub fn send<T: Serialize>(&self, message: &T) -> Result<(), IpcError> {
            let payload = serde_json::to_vec(message)?;
            if payload.len() > MAX_MESSAGE_BYTES {
                return Err(IpcError::MessageTooLarge);
            }
            let deadline = Instant::now() + self.timeout;
            self.write_all(&(payload.len() as u32).to_le_bytes(), deadline)?;
            self.write_all(&payload, deadline)
        }
        pub fn receive<T: DeserializeOwned>(&self) -> Result<T, IpcError> {
            let deadline = Instant::now() + self.timeout;
            let mut length = [0u8; 4];
            self.read_exact(&mut length, deadline)?;
            let length = u32::from_le_bytes(length) as usize;
            if length > MAX_MESSAGE_BYTES {
                self.poisoned.set(true);
                return Err(IpcError::MessageTooLarge);
            }
            let mut payload = vec![0u8; length];
            self.read_exact(&mut payload, deadline)?;
            Ok(serde_json::from_slice(&payload)?)
        }
        fn check_connection(&self) -> Result<(), IpcError> {
            if self.poisoned.get() {
                Err(IpcError::Io {
                    operation: "IPC connection",
                    source: std::io::Error::new(
                        std::io::ErrorKind::BrokenPipe,
                        "connection is unusable after an incomplete transfer",
                    ),
                })
            } else {
                Ok(())
            }
        }
        fn read_exact(&self, mut buffer: &mut [u8], deadline: Instant) -> Result<(), IpcError> {
            self.check_connection()?;
            while !buffer.is_empty() {
                let mut operation =
                    Operation::new(self.handle).inspect_err(|_| self.poisoned.set(true))?;
                let ok = unsafe {
                    ReadFile(
                        self.handle,
                        buffer.as_mut_ptr(),
                        buffer.len().min(u32::MAX as usize) as u32,
                        null_mut(),
                        &mut *operation.state,
                    )
                };
                let result = operation.complete(
                    ok != 0,
                    Some(deadline.saturating_duration_since(Instant::now())),
                    "ReadFile",
                );
                // Finish cancellation while the borrowed buffer is still alive.
                drop(operation);
                let read = result.inspect_err(|_| self.poisoned.set(true))?;
                if read == 0 {
                    self.poisoned.set(true);
                    return Err(IpcError::Io {
                        operation: "ReadFile",
                        source: std::io::Error::from(std::io::ErrorKind::UnexpectedEof),
                    });
                }
                buffer = &mut buffer[read as usize..];
            }
            Ok(())
        }
        fn write_all(&self, mut buffer: &[u8], deadline: Instant) -> Result<(), IpcError> {
            self.check_connection()?;
            while !buffer.is_empty() {
                let mut operation =
                    Operation::new(self.handle).inspect_err(|_| self.poisoned.set(true))?;
                let ok = unsafe {
                    WriteFile(
                        self.handle,
                        buffer.as_ptr(),
                        buffer.len().min(u32::MAX as usize) as u32,
                        null_mut(),
                        &mut *operation.state,
                    )
                };
                let result = operation.complete(
                    ok != 0,
                    Some(deadline.saturating_duration_since(Instant::now())),
                    "WriteFile",
                );
                drop(operation);
                let written = result.inspect_err(|_| self.poisoned.set(true))?;
                if written == 0 {
                    self.poisoned.set(true);
                    return Err(IpcError::Io {
                        operation: "WriteFile",
                        source: std::io::Error::from(std::io::ErrorKind::WriteZero),
                    });
                }
                buffer = &buffer[written as usize..];
            }
            Ok(())
        }
    }

    pub fn connect(name: &str, timeout_ms: u32) -> Result<PipeConnection, IpcError> {
        validate_name(name)?;
        let name = wide(name);
        let started = Instant::now();
        let timeout = Duration::from_millis(u64::from(timeout_ms));
        loop {
            let handle = unsafe {
                CreateFileW(
                    name.as_ptr(),
                    windows_sys::Win32::Foundation::GENERIC_READ
                        | windows_sys::Win32::Foundation::GENERIC_WRITE,
                    0,
                    null(),
                    OPEN_EXISTING,
                    FILE_FLAG_OVERLAPPED | SECURITY_SQOS_PRESENT | SECURITY_IDENTIFICATION,
                    null_mut(),
                )
            };
            if handle != INVALID_HANDLE_VALUE {
                return Ok(PipeConnection {
                    handle,
                    timeout,
                    poisoned: Cell::new(false),
                    _not_sync: PhantomData,
                });
            }
            let code = unsafe { GetLastError() };
            if code != ERROR_PIPE_BUSY && code != ERROR_FILE_NOT_FOUND {
                return Err(IpcError::Windows {
                    operation: "CreateFileW",
                    code,
                });
            }
            let remaining = timeout.saturating_sub(started.elapsed());
            if remaining.is_zero() {
                return Err(IpcError::Io {
                    operation: "connect",
                    source: std::io::Error::new(
                        std::io::ErrorKind::TimedOut,
                        "IPC connection timed out",
                    ),
                });
            }
            let wait_ms = remaining.as_millis().min(100) as u32;
            unsafe {
                WaitNamedPipeW(name.as_ptr(), wait_ms);
            }
            thread::sleep(Duration::from_millis(1));
        }
    }
    fn wide(value: &str) -> Vec<u16> {
        value.encode_utf16().chain(Some(0)).collect()
    }
    fn last_error(operation: &'static str) -> IpcError {
        IpcError::Windows {
            operation,
            code: unsafe { GetLastError() },
        }
    }
}

#[cfg(unix)]
mod platform {
    use std::{
        cell::Cell,
        fs,
        io::{Read, Write},
        marker::PhantomData,
        os::unix::{
            ffi::OsStrExt,
            fs::{FileTypeExt, MetadataExt, PermissionsExt},
            io::AsRawFd,
            net::{UnixListener, UnixStream},
        },
        path::{Path, PathBuf},
        thread,
        time::{Duration, Instant},
    };

    use serde::{Serialize, de::DeserializeOwned};

    use super::{IpcError, MAX_MESSAGE_BYTES, ensure_private_directory};

    pub struct PipeServer {
        listener: UnixListener,
        path: PathBuf,
        device: u64,
        inode: u64,
    }

    pub struct PipeConnection {
        stream: UnixStream,
        _not_sync: PhantomData<Cell<()>>,
    }

    impl Drop for PipeServer {
        fn drop(&mut self) {
            if let Ok(metadata) = fs::symlink_metadata(&self.path)
                && metadata.file_type().is_socket()
                && metadata.dev() == self.device
                && metadata.ino() == self.inode
            {
                let _ = fs::remove_file(&self.path);
            }
        }
    }

    impl PipeServer {
        pub fn bind(name: &str) -> Result<Self, IpcError> {
            let path = PathBuf::from(name);
            if !path.is_absolute() || path.as_os_str().as_bytes().len() > 100 {
                return Err(IpcError::InvalidEndpoint(
                    "a Unix endpoint must be an absolute path of at most 100 bytes".to_owned(),
                ));
            }
            let parent = path.parent().ok_or_else(|| {
                IpcError::InvalidEndpoint("the Unix endpoint has no parent directory".to_owned())
            })?;
            ensure_private_directory(parent, unsafe { libc::geteuid() })?;
            let listener = bind_listener(&path)?;
            fs::set_permissions(&path, fs::Permissions::from_mode(0o600)).map_err(|source| {
                IpcError::Io {
                    operation: "secure Unix socket",
                    source,
                }
            })?;
            let metadata = fs::symlink_metadata(&path).map_err(|source| IpcError::Io {
                operation: "inspect Unix socket",
                source,
            })?;
            if !metadata.file_type().is_socket()
                || metadata.uid() != unsafe { libc::geteuid() }
                || metadata.mode() & 0o077 != 0
            {
                let _ = fs::remove_file(&path);
                return Err(IpcError::InvalidEndpoint(
                    "the Unix socket is not owner-only".to_owned(),
                ));
            }
            Ok(Self {
                listener,
                path,
                device: metadata.dev(),
                inode: metadata.ino(),
            })
        }

        pub fn accept(&self) -> Result<PipeConnection, IpcError> {
            let (stream, _) = self.listener.accept().map_err(|source| IpcError::Io {
                operation: "accept Unix socket",
                source,
            })?;
            verify_peer(&stream)?;
            let deadline = Some(Duration::from_secs(5));
            stream
                .set_read_timeout(deadline)
                .and_then(|()| stream.set_write_timeout(deadline))
                .map_err(|source| IpcError::Io {
                    operation: "set server socket timeout",
                    source,
                })?;
            Ok(PipeConnection {
                stream,
                _not_sync: PhantomData,
            })
        }
    }

    impl PipeConnection {
        pub fn send<T: Serialize>(&self, message: &T) -> Result<(), IpcError> {
            let payload = serde_json::to_vec(message)?;
            if payload.len() > MAX_MESSAGE_BYTES {
                return Err(IpcError::MessageTooLarge);
            }
            let mut stream = &self.stream;
            stream
                .write_all(&(payload.len() as u32).to_le_bytes())
                .and_then(|()| stream.write_all(&payload))
                .map_err(|source| IpcError::Io {
                    operation: "write Unix socket",
                    source,
                })
        }

        pub fn receive<T: DeserializeOwned>(&self) -> Result<T, IpcError> {
            let mut stream = &self.stream;
            let mut length = [0u8; 4];
            stream
                .read_exact(&mut length)
                .map_err(|source| IpcError::Io {
                    operation: "read Unix socket frame",
                    source,
                })?;
            let length = u32::from_le_bytes(length) as usize;
            if length > MAX_MESSAGE_BYTES {
                return Err(IpcError::MessageTooLarge);
            }
            let mut payload = vec![0u8; length];
            stream
                .read_exact(&mut payload)
                .map_err(|source| IpcError::Io {
                    operation: "read Unix socket payload",
                    source,
                })?;
            Ok(serde_json::from_slice(&payload)?)
        }
    }

    pub fn connect(name: &str, timeout_ms: u32) -> Result<PipeConnection, IpcError> {
        let path = Path::new(name);
        if !path.is_absolute() || path.as_os_str().as_bytes().len() > 100 {
            return Err(IpcError::InvalidEndpoint(
                "a Unix endpoint must be an absolute path of at most 100 bytes".to_owned(),
            ));
        }
        let started = Instant::now();
        let timeout = Duration::from_millis(u64::from(timeout_ms));
        loop {
            match verify_socket_path(path) {
                Ok(()) => {}
                Err(IpcError::Io { source, .. })
                    if source.kind() == std::io::ErrorKind::NotFound =>
                {
                    if started.elapsed() >= timeout {
                        return Err(IpcError::Io {
                            operation: "connect Unix socket",
                            source: std::io::Error::new(
                                std::io::ErrorKind::TimedOut,
                                "connection timed out",
                            ),
                        });
                    }
                    thread::sleep(Duration::from_millis(2));
                    continue;
                }
                Err(error) => return Err(error),
            }
            match UnixStream::connect(path) {
                Ok(stream) => {
                    verify_peer(&stream)?;
                    let io_timeout = timeout.max(Duration::from_millis(1));
                    stream
                        .set_read_timeout(Some(io_timeout))
                        .map_err(|source| IpcError::Io {
                            operation: "set Unix socket read timeout",
                            source,
                        })?;
                    stream
                        .set_write_timeout(Some(io_timeout))
                        .map_err(|source| IpcError::Io {
                            operation: "set Unix socket write timeout",
                            source,
                        })?;
                    return Ok(PipeConnection {
                        stream,
                        _not_sync: PhantomData,
                    });
                }
                Err(error)
                    if matches!(
                        error.kind(),
                        std::io::ErrorKind::NotFound
                            | std::io::ErrorKind::ConnectionRefused
                            | std::io::ErrorKind::WouldBlock
                    ) =>
                {
                    if started.elapsed() >= timeout {
                        return Err(IpcError::Io {
                            operation: "connect Unix socket",
                            source: std::io::Error::new(
                                std::io::ErrorKind::TimedOut,
                                "connection timed out",
                            ),
                        });
                    }
                    thread::sleep(Duration::from_millis(2));
                }
                Err(source) => {
                    return Err(IpcError::Io {
                        operation: "connect Unix socket",
                        source,
                    });
                }
            }
        }
    }

    fn bind_listener(path: &Path) -> Result<UnixListener, IpcError> {
        match UnixListener::bind(path) {
            Ok(listener) => Ok(listener),
            Err(error) if error.kind() == std::io::ErrorKind::AddrInUse => {
                recover_stale_endpoint(path)?;
                UnixListener::bind(path).map_err(|source| IpcError::Io {
                    operation: "bind Unix socket",
                    source,
                })
            }
            Err(source) => Err(IpcError::Io {
                operation: "bind Unix socket",
                source,
            }),
        }
    }

    fn recover_stale_endpoint(path: &Path) -> Result<(), IpcError> {
        let metadata = fs::symlink_metadata(path).map_err(|source| IpcError::Io {
            operation: "inspect occupied Unix endpoint",
            source,
        })?;
        if metadata.file_type().is_symlink()
            || !metadata.file_type().is_socket()
            || metadata.uid() != unsafe { libc::geteuid() }
        {
            return Err(IpcError::InvalidEndpoint(
                "the occupied Unix endpoint is not an owner-controlled socket".to_owned(),
            ));
        }
        match UnixStream::connect(path) {
            Ok(_) => Err(IpcError::Io {
                operation: "bind Unix socket",
                source: std::io::Error::new(
                    std::io::ErrorKind::AddrInUse,
                    "another Recentry process owns the endpoint",
                ),
            }),
            Err(error)
                if matches!(
                    error.kind(),
                    std::io::ErrorKind::ConnectionRefused | std::io::ErrorKind::NotFound
                ) =>
            {
                fs::remove_file(path).map_err(|source| IpcError::Io {
                    operation: "remove stale Unix socket",
                    source,
                })
            }
            Err(source) => Err(IpcError::Io {
                operation: "probe occupied Unix endpoint",
                source,
            }),
        }
    }

    fn verify_socket_path(path: &Path) -> Result<(), IpcError> {
        let metadata = fs::symlink_metadata(path).map_err(|source| IpcError::Io {
            operation: "inspect Unix socket peer",
            source,
        })?;
        if metadata.file_type().is_symlink()
            || !metadata.file_type().is_socket()
            || metadata.uid() != unsafe { libc::geteuid() }
            || metadata.mode() & 0o077 != 0
        {
            return Err(IpcError::InvalidEndpoint(
                "the Unix socket peer is not owned exclusively by the current user".to_owned(),
            ));
        }
        Ok(())
    }

    #[cfg(target_os = "linux")]
    fn verify_peer(stream: &UnixStream) -> Result<(), IpcError> {
        let mut credentials = libc::ucred {
            pid: 0,
            uid: 0,
            gid: 0,
        };
        let mut length = std::mem::size_of::<libc::ucred>() as libc::socklen_t;
        let result = unsafe {
            libc::getsockopt(
                stream.as_raw_fd(),
                libc::SOL_SOCKET,
                libc::SO_PEERCRED,
                (&mut credentials as *mut libc::ucred).cast(),
                &mut length,
            )
        };
        if result != 0 {
            return Err(IpcError::Io {
                operation: "read Unix peer credentials",
                source: std::io::Error::last_os_error(),
            });
        }
        if credentials.uid != unsafe { libc::geteuid() } {
            return Err(IpcError::InvalidEndpoint(
                "the Unix socket peer belongs to another user".to_owned(),
            ));
        }
        Ok(())
    }

    #[cfg(target_os = "macos")]
    fn verify_peer(stream: &UnixStream) -> Result<(), IpcError> {
        let mut uid = 0;
        let mut gid = 0;
        let result = unsafe { libc::getpeereid(stream.as_raw_fd(), &mut uid, &mut gid) };
        if result != 0 {
            return Err(IpcError::Io {
                operation: "read Unix peer credentials",
                source: std::io::Error::last_os_error(),
            });
        }
        if uid != unsafe { libc::geteuid() } {
            return Err(IpcError::InvalidEndpoint(
                "the Unix socket peer belongs to another user".to_owned(),
            ));
        }
        Ok(())
    }

    #[cfg(not(any(target_os = "linux", target_os = "macos")))]
    fn verify_peer(_stream: &UnixStream) -> Result<(), IpcError> {
        Ok(())
    }
}

#[cfg(not(any(windows, unix)))]
mod platform {
    use serde::{Serialize, de::DeserializeOwned};

    use super::IpcError;

    pub struct PipeServer;
    pub struct PipeConnection;

    impl PipeServer {
        pub fn bind(_name: &str) -> Result<Self, IpcError> {
            Err(IpcError::Unsupported)
        }

        pub fn accept(&self) -> Result<PipeConnection, IpcError> {
            Err(IpcError::Unsupported)
        }
    }

    impl PipeConnection {
        pub fn send<T: Serialize>(&self, _message: &T) -> Result<(), IpcError> {
            Err(IpcError::Unsupported)
        }

        pub fn receive<T: DeserializeOwned>(&self) -> Result<T, IpcError> {
            Err(IpcError::Unsupported)
        }
    }

    pub fn connect(_name: &str, _timeout_ms: u32) -> Result<PipeConnection, IpcError> {
        Err(IpcError::Unsupported)
    }
}

pub use platform::{PipeConnection, PipeServer, connect};

pub type LocalConnection = PipeConnection;
pub type LocalServer = PipeServer;

pub fn connect_local(endpoint: &str, timeout_ms: u32) -> Result<LocalConnection, IpcError> {
    connect(endpoint, timeout_ms)
}

pub fn request<Req: Serialize, Resp: DeserializeOwned>(
    name: &str,
    request: &Req,
    timeout_ms: u32,
) -> Result<Resp, IpcError> {
    let connection = connect(name, timeout_ms)?;
    connection.send(request)?;
    connection.receive()
}

#[cfg(all(test, windows))]
mod tests {
    use std::{process, thread};

    use crate::test_protocol::{HostCommand, HostResponse};

    use super::*;

    #[test]
    fn current_user_endpoint_is_validated_and_platform_scoped() {
        let sid = platform::current_user_sid().unwrap();
        assert!(sid.starts_with("S-1-"));
        assert_eq!(
            current_user_endpoint("sean-toolbox-host-v1").unwrap(),
            format!(r"\\.\pipe\sean-toolbox-host-v1-{sid}")
        );
        assert!(current_user_endpoint("../other-user").is_err());
        assert!(current_user_endpoint("").is_err());
    }

    #[test]
    fn local_transport_names_expose_the_platform_adapter() {
        let name = test_pipe("local-alias");
        let server = LocalServer::bind(&name).unwrap();
        let worker = thread::spawn(move || {
            let connection: LocalConnection = server.accept().unwrap();
            assert_eq!(
                connection.receive::<HostCommand>().unwrap(),
                HostCommand::Ping
            );
            connection.send(&HostResponse::Pong).unwrap();
        });
        let response = request::<_, HostResponse>(&name, &HostCommand::Ping, 5_000).unwrap();
        assert_eq!(response, HostResponse::Pong);
        worker.join().unwrap();
    }

    fn test_pipe(label: &str) -> String {
        current_user_endpoint(&format!("sean-toolbox-ipc-test-{}-{label}", process::id())).unwrap()
    }

    #[test]
    fn current_user_pipe_round_trips_a_protocol_message() {
        let name = test_pipe("roundtrip");
        let server = PipeServer::bind(&name).unwrap();
        let worker = thread::spawn(move || {
            let connection = server.accept().unwrap();
            assert_eq!(
                connection.receive::<HostCommand>().unwrap(),
                HostCommand::Ping
            );
            connection.send(&HostResponse::Pong).unwrap();
        });
        let response = request::<_, HostResponse>(&name, &HostCommand::Ping, 5_000).unwrap();
        assert_eq!(response, HostResponse::Pong);
        worker.join().unwrap();
    }

    #[test]
    fn transport_rejects_oversized_messages_before_writing() {
        let name = test_pipe("oversized");
        let server = PipeServer::bind(&name).unwrap();
        let worker = thread::spawn(move || {
            let connection = server.accept().unwrap();
            assert!(matches!(
                connection.send(&"x".repeat(MAX_MESSAGE_BYTES + 1)),
                Err(IpcError::MessageTooLarge)
            ));
        });
        let _client = connect(&name, 5_000).unwrap();
        worker.join().unwrap();
    }

    #[test]
    fn client_read_timeout_cancels_pending_io_and_poisoned_connection_is_rejected() {
        use std::{
            sync::mpsc,
            time::{Duration, Instant},
        };
        let name = test_pipe("read-timeout");
        let server = PipeServer::bind(&name).unwrap();
        let (release, wait) = mpsc::channel();
        let worker = thread::spawn(move || {
            let _connection = server.accept().unwrap();
            wait.recv_timeout(Duration::from_secs(10)).unwrap();
        });
        let client = connect(&name, 150).unwrap();
        let started = Instant::now();
        let error = client.receive::<HostResponse>().unwrap_err();
        assert!(
            matches!(error, IpcError::Io { source, .. } if source.kind() == std::io::ErrorKind::TimedOut)
        );
        assert!(
            started.elapsed() < Duration::from_secs(3),
            "read must be bounded"
        );
        assert!(
            matches!(client.receive::<HostResponse>(), Err(IpcError::Io { source, .. }) if source.kind() == std::io::ErrorKind::BrokenPipe)
        );
        release.send(()).unwrap();
        worker.join().unwrap();
    }
}

#[cfg(all(test, unix))]
mod unix_tests {
    use std::{
        fs,
        os::unix::{fs::PermissionsExt, net::UnixListener},
        path::Path,
        process, thread,
    };

    use crate::test_protocol::{HostCommand, HostResponse};

    use super::*;

    #[test]
    fn concurrent_runtime_creation_stays_owner_only_and_rejects_symlinks() {
        use std::{
            sync::{Arc, Barrier},
            time::{SystemTime, UNIX_EPOCH},
        };
        let path = std::env::temp_dir().join(format!(
            "sean-toolbox-dir-test-{}-{}",
            process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let barrier = Arc::new(Barrier::new(8));
        let workers: Vec<_> = (0..8)
            .map(|_| {
                let path = path.clone();
                let barrier = barrier.clone();
                thread::spawn(move || {
                    barrier.wait();
                    ensure_private_directory(&path, unsafe { libc::geteuid() }).unwrap();
                })
            })
            .collect();
        for worker in workers {
            worker.join().unwrap();
        }
        assert_eq!(
            fs::metadata(&path).unwrap().permissions().mode() & 0o777,
            0o700
        );
        let link = path.with_extension("link");
        std::os::unix::fs::symlink(&path, &link).unwrap();
        assert!(ensure_private_directory(&link, unsafe { libc::geteuid() }).is_err());
        fs::remove_file(link).unwrap();
        fs::remove_dir(path).unwrap();
    }

    fn test_endpoint(label: &str) -> String {
        current_user_endpoint(&format!("sean-toolbox-ipc-{}-{label}", process::id())).unwrap()
    }

    #[test]
    fn owner_only_socket_round_trips_and_is_removed_on_drop() {
        let endpoint = test_endpoint("roundtrip");
        let server = LocalServer::bind(&endpoint).unwrap();
        assert_eq!(
            fs::metadata(Path::new(&endpoint))
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o600
        );
        let worker = thread::spawn(move || {
            let connection = server.accept().unwrap();
            assert_eq!(
                connection.receive::<HostCommand>().unwrap(),
                HostCommand::Ping
            );
            connection.send(&HostResponse::Pong).unwrap();
        });
        let response = request::<_, HostResponse>(&endpoint, &HostCommand::Ping, 5_000).unwrap();
        assert_eq!(response, HostResponse::Pong);
        worker.join().unwrap();
        assert!(!Path::new(&endpoint).exists());
    }

    #[test]
    fn stale_owner_socket_is_recovered_but_active_socket_is_rejected() {
        let endpoint = test_endpoint("stale");
        let stale = UnixListener::bind(&endpoint).unwrap();
        drop(stale);
        assert!(Path::new(&endpoint).exists());

        let server = LocalServer::bind(&endpoint).unwrap();
        assert!(LocalServer::bind(&endpoint).is_err());
        drop(server);
        assert!(!Path::new(&endpoint).exists());
    }

    #[test]
    fn unix_transport_rejects_relative_and_oversized_messages() {
        assert!(LocalServer::bind("relative.sock").is_err());
        let endpoint = test_endpoint("oversized");
        let server = LocalServer::bind(&endpoint).unwrap();
        let worker = thread::spawn(move || {
            let connection = server.accept().unwrap();
            assert!(matches!(
                connection.send(&"x".repeat(MAX_MESSAGE_BYTES + 1)),
                Err(IpcError::MessageTooLarge)
            ));
        });
        let _connection = connect_local(&endpoint, 5_000).unwrap();
        worker.join().unwrap();
    }
}

/// Make a bounded local request to the resident host.
pub fn call(command: toolbox_core::HostCommand) -> Result<toolbox_core::Response, String> {
    let endpoint = current_user_endpoint(toolbox_core::HOST_ENDPOINT).map_err(|e| e.to_string())?;
    request(&endpoint, &toolbox_core::Request::new(command), 5_000).map_err(|e| e.to_string())
}

#[cfg(test)]
mod test_protocol {
    use serde::{Deserialize, Serialize};
    #[derive(Debug, PartialEq, Serialize, Deserialize)]
    pub enum HostCommand {
        Ping,
    }
    #[derive(Debug, PartialEq, Serialize, Deserialize)]
    pub enum HostResponse {
        Pong,
    }
}
