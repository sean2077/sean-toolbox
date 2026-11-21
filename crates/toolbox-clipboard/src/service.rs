//! Clipboard bridge service: Noise transport, receive staging and echo guards.

use crate::backend::{Backend, BackendEvent};
use crate::crypto::{self, Session};
use crate::frame::{CHUNK_BYTES, Frame};
use crate::model::{
    self, BlobMeta, ClipboardContent, DEFAULT_MAX_TRANSFER_BYTES, MAX_HTML_BYTES, MAX_IMAGE_BYTES,
    MAX_TEXT_BYTES, Manifest, blob_meta,
};
use crate::notify::{Notifier, desktop_notifier};
use crate::protocol::{BlobKind, Message, PROTO_VERSION};
use crate::sync::LoopGuard;
use crate::transfer::{FileSink, cache_root, cleanup_cache};
use serde::{Deserialize, Serialize};
use std::{
    collections::HashSet,
    io::{ErrorKind, Read, Write},
    net::{TcpListener, TcpStream, ToSocketAddrs},
    path::{Path, PathBuf},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
        mpsc::{self, Receiver},
    },
    thread,
    time::{Duration, Instant, SystemTime},
};

pub const HEARTBEAT: Duration = Duration::from_secs(15);
pub const CONNECT_TIMEOUT: Duration = Duration::from_secs(4);
const LOOP_DELAY: Duration = Duration::from_millis(20);
const READ_TIMEOUT: Duration = Duration::from_millis(20);
const MAX_BACKOFF: Duration = Duration::from_secs(30);
const CACHE_KEEP: usize = 5;
const CACHE_AGE: Duration = Duration::from_secs(7 * 24 * 60 * 60);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Role {
    Listen,
    Connect,
}

impl Role {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Listen => "listen",
            Self::Connect => "connect",
        }
    }
}

#[derive(Debug, Clone)]
pub struct Settings {
    pub role: Role,
    pub listen: String,
    pub peer: String,
    pub max_transfer_bytes: u64,
    pub key_path: PathBuf,
    pub cache_dir: PathBuf,
    pub host_name: String,
    /// Disconnect when no inbound frame arrives for this long (3 heartbeats).
    pub silence_timeout: Duration,
}

impl Settings {
    pub fn with_defaults(
        role: Role,
        listen: impl Into<String>,
        peer: impl Into<String>,
        key_path: PathBuf,
        cache_dir: PathBuf,
        host_name: impl Into<String>,
    ) -> Self {
        Self {
            role,
            listen: listen.into(),
            peer: peer.into(),
            max_transfer_bytes: DEFAULT_MAX_TRANSFER_BYTES,
            key_path,
            cache_dir,
            host_name: host_name.into(),
            silence_timeout: Duration::from_secs(45),
        }
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Status {
    pub enabled: bool,
    pub role: String,
    pub connected: bool,
    pub peer: String,
    pub last_sent: Option<String>,
    pub last_received: Option<String>,
    pub sent_count: u64,
    pub received_count: u64,
    pub last_error: Option<String>,
}

pub struct Handle {
    status: Arc<Mutex<Status>>,
    stop: Arc<AtomicBool>,
    thread: Option<thread::JoinHandle<()>>,
}

impl Handle {
    pub fn status(&self) -> Status {
        self.status.lock().unwrap().clone()
    }

    pub fn stop(&mut self) {
        self.stop.store(true, Ordering::Release);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

impl Drop for Handle {
    fn drop(&mut self) {
        self.stop();
    }
}

/// Start the platform backend. Returns an error when the key or backend is
/// unavailable; the host keeps running and reports the warning.
pub fn start(settings: Settings) -> Result<Handle, String> {
    let backend = crate::platform::backend()?;
    start_with_notifier(settings, backend, desktop_notifier())
}

pub fn start_with_backend(settings: Settings, backend: Box<dyn Backend>) -> Result<Handle, String> {
    start_with_notifier(settings, backend, desktop_notifier())
}

pub fn start_with_notifier(
    settings: Settings,
    backend: Box<dyn Backend>,
    notifier: Arc<dyn Notifier>,
) -> Result<Handle, String> {
    let key =
        crypto::read_key(&settings.key_path).map_err(|error| format!("clipboard key: {error}"))?;
    let status = Arc::new(Mutex::new(Status {
        enabled: true,
        role: settings.role.as_str().into(),
        peer: if settings.role == Role::Connect {
            settings.peer.clone()
        } else {
            settings.listen.clone()
        },
        ..Default::default()
    }));
    let stop = Arc::new(AtomicBool::new(false));
    let thread_status = status.clone();
    let thread_stop = stop.clone();
    let thread = thread::spawn(move || {
        if let Err(error) = run_service(
            settings,
            key,
            backend,
            thread_status.clone(),
            thread_stop,
            notifier,
        ) {
            if let Ok(mut status) = thread_status.lock() {
                status.connected = false;
                status.last_error = Some(error.clone());
            }
            eprintln!("Sean Toolbox clipboard: {error}");
        }
    });
    Ok(Handle {
        status,
        stop,
        thread: Some(thread),
    })
}

fn run_service(
    settings: Settings,
    key: [u8; crypto::KEY_BYTES],
    mut backend: Box<dyn Backend>,
    status: Arc<Mutex<Status>>,
    stop: Arc<AtomicBool>,
    notifier: Arc<dyn Notifier>,
) -> Result<(), String> {
    let _ = cleanup_cache(&settings.cache_dir, CACHE_KEEP, CACHE_AGE);
    let (events_tx, events_rx) = mpsc::channel();
    backend.watch(events_tx)?;
    let mut guard = LoopGuard::new();
    match settings.role {
        Role::Listen => listen_loop(
            &settings,
            &key,
            &mut backend,
            &events_rx,
            &status,
            &stop,
            &mut guard,
            &notifier,
        ),
        Role::Connect => connect_loop(
            &settings,
            &key,
            &mut backend,
            &events_rx,
            &status,
            &stop,
            &mut guard,
            &notifier,
        ),
    }
}

enum SessionEnd {
    Closed,
    Replace(NetStream),
}

fn handshake(
    mut stream: TcpStream,
    initiator: bool,
    key: &[u8; crypto::KEY_BYTES],
) -> Result<NetStream, String> {
    // The Noise handshake can take longer than a clipboard poll interval on a
    // LAN or after a Windows sleep; give it its own generous read timeout.
    stream
        .set_write_timeout(Some(Duration::from_secs(10)))
        .map_err(|error| error.to_string())?;
    stream
        .set_read_timeout(Some(Duration::from_secs(10)))
        .map_err(|error| error.to_string())?;
    let session = if initiator {
        Session::initiator(&mut stream, key)
    } else {
        Session::responder(&mut stream, key)
    }
    .map_err(|error| format!("Noise handshake: {error}"))?;
    stream
        .set_read_timeout(Some(READ_TIMEOUT))
        .map_err(|error| error.to_string())?;
    Ok(NetStream::new(stream, session))
}

#[allow(clippy::too_many_arguments)]
fn listen_loop(
    settings: &Settings,
    key: &[u8; crypto::KEY_BYTES],
    backend: &mut Box<dyn Backend>,
    events: &Receiver<BackendEvent>,
    status: &Arc<Mutex<Status>>,
    stop: &Arc<AtomicBool>,
    guard: &mut LoopGuard,
    notifier: &Arc<dyn Notifier>,
) -> Result<(), String> {
    let listener = TcpListener::bind(&settings.listen)
        .map_err(|error| format!("cannot listen on {}: {error}", settings.listen))?;
    listener
        .set_nonblocking(true)
        .map_err(|error| error.to_string())?;
    while !stop.load(Ordering::Acquire) {
        match listener.accept() {
            Ok((stream, peer)) => {
                set_status(status, |status| {
                    status.peer = peer.to_string();
                    status.last_error = None;
                });
                let mut net = match handshake(stream, false, key) {
                    Ok(net) => net,
                    Err(error) => {
                        set_status(status, |status| {
                            status.connected = false;
                            status.last_error = Some(error);
                        });
                        continue;
                    }
                };
                loop {
                    match session(
                        net,
                        key,
                        settings,
                        backend,
                        events,
                        status,
                        stop,
                        guard,
                        notifier,
                        Some(&listener),
                    ) {
                        Ok(SessionEnd::Replace(next)) => net = next,
                        Ok(SessionEnd::Closed) => break,
                        Err(error) => {
                            set_status(status, |status| {
                                status.connected = false;
                                status.last_error = Some(error);
                            });
                            break;
                        }
                    }
                }
            }
            Err(error) if error.kind() == ErrorKind::WouldBlock => {
                drain_events(backend, events, guard);
                thread::sleep(LOOP_DELAY);
            }
            Err(error) => return Err(format!("accept failed: {error}")),
        }
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn connect_loop(
    settings: &Settings,
    key: &[u8; crypto::KEY_BYTES],
    backend: &mut Box<dyn Backend>,
    events: &Receiver<BackendEvent>,
    status: &Arc<Mutex<Status>>,
    stop: &Arc<AtomicBool>,
    guard: &mut LoopGuard,
    notifier: &Arc<dyn Notifier>,
) -> Result<(), String> {
    let mut backoff = Duration::from_secs(1);
    loop {
        if stop.load(Ordering::Acquire) {
            return Ok(());
        }
        let addresses = settings
            .peer
            .to_socket_addrs()
            .map_err(|error| format!("invalid peer {}: {error}", settings.peer))?;
        let mut connected = None;
        for address in addresses {
            match TcpStream::connect_timeout(&address, CONNECT_TIMEOUT) {
                Ok(stream) => {
                    connected = Some(stream);
                    break;
                }
                Err(error) => {
                    set_status(status, |status| {
                        status.connected = false;
                        status.last_error = Some(format!("connect {address}: {error}"));
                    });
                }
            }
        }
        let Some(stream) = connected else {
            drain_events(backend, events, guard);
            thread::sleep(backoff);
            backoff = (backoff * 2).min(MAX_BACKOFF);
            continue;
        };
        backoff = Duration::from_secs(1);
        let mut net = match handshake(stream, true, key) {
            Ok(net) => net,
            Err(error) => {
                set_status(status, |status| {
                    status.connected = false;
                    status.last_error = Some(error);
                });
                thread::sleep(Duration::from_secs(1));
                continue;
            }
        };
        loop {
            match session(
                net, key, settings, backend, events, status, stop, guard, notifier, None,
            ) {
                Ok(SessionEnd::Replace(next)) => net = next,
                Ok(SessionEnd::Closed) => break,
                Err(error) => {
                    set_status(status, |status| {
                        status.connected = false;
                        status.last_error = Some(error);
                    });
                    break;
                }
            }
        }
        thread::sleep(Duration::from_secs(1));
    }
}

fn drain_events(
    backend: &mut Box<dyn Backend>,
    events: &Receiver<BackendEvent>,
    guard: &mut LoopGuard,
) {
    while let Ok(event) = events.try_recv() {
        if event == BackendEvent::Changed
            && let Ok(read) = backend.read()
            && !read.remote_origin
            && !read.sensitive
        {
            guard.observe(read.content, Instant::now());
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn session(
    mut net: NetStream,
    key: &[u8; crypto::KEY_BYTES],
    settings: &Settings,
    backend: &mut Box<dyn Backend>,
    events: &Receiver<BackendEvent>,
    status: &Arc<Mutex<Status>>,
    stop: &Arc<AtomicBool>,
    guard: &mut LoopGuard,
    notifier: &Arc<dyn Notifier>,
    listener: Option<&TcpListener>,
) -> Result<SessionEnd, String> {
    net.send(&Message::Hello {
        proto_version: PROTO_VERSION,
        host_name: settings.host_name.clone(),
    })?;

    let mut hello = false;
    let mut receiving: Option<ReceiveState> = None;
    let mut last_ping = Instant::now();
    let mut last_inbound = Instant::now();
    while !stop.load(Ordering::Acquire) {
        let now = Instant::now();
        match net.try_frame() {
            Ok(Some(frame)) => {
                last_inbound = now;
                let message = Message::from_frame(&frame).map_err(|error| error.to_string())?;
                match message {
                    Message::Hello { proto_version, .. } => {
                        if proto_version != PROTO_VERSION {
                            return Err(format!("protocol mismatch: {proto_version}"));
                        }
                        hello = true;
                        set_status(status, |status| {
                            status.connected = true;
                            status.last_error = None;
                        });
                    }
                    Message::Offer {
                        id,
                        text,
                        html,
                        image,
                        files,
                    } => {
                        if let Some(old) = receiving.take() {
                            old.cancel();
                        }
                        let has_files = files.is_some();
                        if has_files {
                            notifier.notify("Sean Toolbox", "正在接收文件…");
                        }
                        let cancel_id = id.clone();
                        match ReceiveState::create(
                            &settings.cache_dir,
                            id,
                            text,
                            html,
                            image,
                            files,
                            settings.max_transfer_bytes,
                        ) {
                            Ok(next) => receiving = Some(next),
                            Err(error) => {
                                notifier
                                    .notify("Sean Toolbox", &format!("接收剪贴板失败：{error}"));
                                set_status(status, |status| {
                                    status.last_error = Some(error);
                                });
                                let _ = net.send(&Message::Cancel { id: cancel_id });
                            }
                        }
                    }
                    Message::Chunk { id, kind, offset } => {
                        let mut failure = None;
                        if let Some(state) = receiving.as_mut()
                            && state.id == id
                            && !frame.data.is_empty()
                            && let Err(error) = state.chunk(kind, offset, &frame.data)
                        {
                            failure = Some(error);
                        }
                        if let Some(error) = failure {
                            notifier.notify("Sean Toolbox", &format!("接收剪贴板失败：{error}"));
                            set_status(status, |status| {
                                status.last_error = Some(error);
                            });
                            if let Some(state) = receiving.take() {
                                state.cancel();
                            }
                            let _ = net.send(&Message::Cancel { id });
                        }
                    }
                    Message::End { id, kind } => {
                        let mut failure = None;
                        let mut complete = false;
                        if let Some(state) = receiving.as_mut()
                            && state.id == id
                        {
                            match state.end(kind) {
                                Ok(()) => complete = state.complete(),
                                Err(error) => failure = Some(error),
                            }
                        }
                        if let Some(error) = failure {
                            notifier.notify("Sean Toolbox", &format!("接收剪贴板失败：{error}"));
                            set_status(status, |status| {
                                status.last_error = Some(error);
                            });
                            if let Some(state) = receiving.take() {
                                state.cancel();
                            }
                            let _ = net.send(&Message::Cancel { id });
                        } else if complete {
                            let state = receiving.take().expect("state still present");
                            match state.finish() {
                                Ok(content) => {
                                    if let Err(error) = backend.write(&content) {
                                        notifier.notify(
                                            "Sean Toolbox",
                                            &format!("写入剪贴板失败：{error}"),
                                        );
                                        set_status(status, |status| {
                                            status.last_error = Some(error);
                                        });
                                        let _ = net.send(&Message::Cancel { id });
                                    } else {
                                        if !content.files.is_empty() {
                                            notifier.notify("Sean Toolbox", "文件已接收");
                                        }
                                        guard.note_received(&content);
                                        set_status(status, |status| {
                                            status.last_received =
                                                Some(format!("{} {}", content.kind(), timestamp()));
                                            status.received_count += 1;
                                        });
                                        let _ = cleanup_cache(
                                            &settings.cache_dir,
                                            CACHE_KEEP,
                                            CACHE_AGE,
                                        );
                                    }
                                }
                                Err(error) => {
                                    notifier
                                        .notify("Sean Toolbox", &format!("接收文件失败：{error}"));
                                    set_status(status, |status| {
                                        status.last_error = Some(error);
                                    });
                                    let _ = net.send(&Message::Cancel { id });
                                }
                            }
                        }
                    }
                    Message::Cancel { id } => {
                        if let Some(state) = receiving.take()
                            && state.id == id
                        {
                            state.cancel();
                        }
                    }
                    Message::Ping => {
                        net.send(&Message::Pong)?;
                    }
                    Message::Pong => {}
                }
            }
            Ok(None) => {}
            Err(error) => return Err(error),
        }

        if now.duration_since(last_inbound) >= settings.silence_timeout {
            return Err("peer silent".into());
        }
        drain_events(backend, events, guard);
        if hello && let Some((content, _)) = guard.ready(now) {
            let has_files = !content.files.is_empty();
            if has_files {
                notifier.notify("Sean Toolbox", "正在发送文件…");
            }
            match send_content(&mut net, &content, settings.max_transfer_bytes) {
                Ok(()) => {
                    if has_files {
                        notifier.notify("Sean Toolbox", "文件已发送");
                    }
                    guard.note_sent(&content);
                    set_status(status, |status| {
                        status.last_sent = Some(format!("{} {}", content.kind(), timestamp()));
                        status.sent_count += 1;
                    });
                }
                Err(SendContentError::Content(error)) => {
                    notifier.notify("Sean Toolbox", &format!("剪贴板同步失败：{error}"));
                    set_status(status, |status| {
                        status.last_error = Some(error);
                    });
                }
                Err(SendContentError::Network(error)) => return Err(error),
            }
        }
        if hello && now.duration_since(last_ping) >= HEARTBEAT {
            net.send(&Message::Ping)?;
            last_ping = now;
        }
        if let Some(listener) = listener {
            match listener.accept() {
                Ok((stream, peer)) => {
                    set_status(status, |status| {
                        status.peer = peer.to_string();
                    });
                    match handshake(stream, false, key) {
                        Ok(next) => {
                            if let Some(state) = receiving.take() {
                                state.cancel();
                            }
                            return Ok(SessionEnd::Replace(next));
                        }
                        Err(error) => {
                            eprintln!("Sean Toolbox clipboard: replacement handshake: {error}");
                        }
                    }
                }
                Err(error) if error.kind() == ErrorKind::WouldBlock => {}
                Err(error) => return Err(format!("accept: {error}")),
            }
        }
        thread::sleep(LOOP_DELAY);
    }
    if let Some(state) = receiving {
        state.cancel();
    }
    Ok(SessionEnd::Closed)
}

enum SendContentError {
    Content(String),
    Network(String),
}

impl From<String> for SendContentError {
    fn from(error: String) -> Self {
        Self::Network(error)
    }
}

fn send_content(
    net: &mut NetStream,
    content: &ClipboardContent,
    max_transfer_bytes: u64,
) -> Result<(), SendContentError> {
    let text = content
        .text
        .as_ref()
        .map(|value| blob_meta(value.as_bytes()));
    let html = content
        .html
        .as_ref()
        .map(|value| blob_meta(value.as_bytes()));
    let image = content.image.as_ref().map(|value| blob_meta(value));
    if text.as_ref().is_some_and(|meta| meta.size > MAX_TEXT_BYTES) {
        return Err(SendContentError::Content(
            "text exceeds the 16 MiB limit".into(),
        ));
    }
    if html.as_ref().is_some_and(|meta| meta.size > MAX_HTML_BYTES) {
        return Err(SendContentError::Content(
            "HTML exceeds the 16 MiB limit".into(),
        ));
    }
    if image
        .as_ref()
        .is_some_and(|meta| meta.size > MAX_IMAGE_BYTES)
    {
        return Err(SendContentError::Content(
            "image exceeds the 64 MiB limit".into(),
        ));
    }
    let files = if content.files.is_empty() {
        None
    } else {
        Some(
            crate::transfer::collect_manifest(&content.files, max_transfer_bytes)
                .map_err(|error| SendContentError::Content(error.to_string()))?,
        )
    };
    if let Some(manifest) = &files
        && manifest.total_size > max_transfer_bytes
    {
        return Err(SendContentError::Content(
            "files exceed the transfer limit".into(),
        ));
    }
    let id = new_id();
    net.send(&Message::Offer {
        id: id.clone(),
        text,
        html,
        image,
        files: files.clone(),
    })?;
    let result = (|| -> Result<(), SendContentError> {
        if let Some(text) = &content.text {
            send_bytes(net, &id, BlobKind::Text, text.as_bytes())?;
        }
        if let Some(html) = &content.html {
            send_bytes(net, &id, BlobKind::Html, html.as_bytes())?;
        }
        if let Some(image) = &content.image {
            send_bytes(net, &id, BlobKind::Image, image)?;
        }
        if let Some(manifest) = &files {
            let mut roots = std::collections::HashMap::new();
            for path in &content.files {
                if let Some(name) = path.file_name().and_then(|name| name.to_str()) {
                    roots.insert(name.to_owned(), path.clone());
                }
            }
            let mut offset = 0u64;
            for entry in &manifest.entries {
                let mut parts = entry.path.splitn(2, '/');
                let root_name = parts.next().unwrap_or_default();
                let rest = parts.next();
                let Some(root) = roots.get(root_name) else {
                    return Err(SendContentError::Content(format!(
                        "missing file root: {root_name}"
                    )));
                };
                let actual = match rest {
                    Some(rest) => rest
                        .split('/')
                        .fold(root.clone(), |path, part| path.join(part)),
                    None => root.clone(),
                };
                let mut file = std::fs::File::open(&actual).map_err(|error| {
                    SendContentError::Content(format!("open {}: {error}", actual.display()))
                })?;
                let mut buffer = [0u8; CHUNK_BYTES];
                let mut written = 0u64;
                loop {
                    use std::io::Read;
                    let count = file.read(&mut buffer).map_err(|error| {
                        SendContentError::Content(format!("read {}: {error}", actual.display()))
                    })?;
                    if count == 0 {
                        break;
                    }
                    net.send_chunk(
                        &id,
                        BlobKind::File,
                        offset + written,
                        buffer[..count].to_vec(),
                    )?;
                    written += count as u64;
                }
                if written != entry.size {
                    return Err(SendContentError::Content(format!(
                        "file changed while reading: {}",
                        entry.path
                    )));
                }
                offset += written;
            }
            net.send(&Message::End {
                id: id.clone(),
                kind: BlobKind::File,
            })?;
        }
        Ok(())
    })();
    match result {
        Err(SendContentError::Content(error)) => {
            let _ = net.send(&Message::Cancel { id });
            Err(SendContentError::Content(error))
        }
        other => other,
    }
}

fn send_bytes(net: &mut NetStream, id: &str, kind: BlobKind, bytes: &[u8]) -> Result<(), String> {
    for (index, chunk) in bytes.chunks(CHUNK_BYTES).enumerate() {
        net.send_chunk(id, kind, (index * CHUNK_BYTES) as u64, chunk.to_vec())?;
    }
    net.send(&Message::End {
        id: id.to_owned(),
        kind,
    })
}

struct NetStream {
    stream: TcpStream,
    session: Session,
    buffer: Vec<u8>,
}

impl NetStream {
    fn new(stream: TcpStream, session: Session) -> Self {
        Self {
            stream,
            session,
            buffer: Vec::new(),
        }
    }

    fn send(&mut self, message: &Message) -> Result<(), String> {
        let frame = message.frame().map_err(|error| error.to_string())?;
        self.send_frame(&frame)
    }

    fn send_chunk(
        &mut self,
        id: &str,
        kind: BlobKind,
        offset: u64,
        data: Vec<u8>,
    ) -> Result<(), String> {
        let frame = Message::Chunk {
            id: id.to_owned(),
            kind,
            offset,
        }
        .chunk_frame(data)
        .map_err(|error| error.to_string())?;
        self.send_frame(&frame)
    }

    fn send_frame(&mut self, frame: &Frame) -> Result<(), String> {
        let bytes = frame.encode().map_err(|error| error.to_string())?;
        let cipher = self
            .session
            .seal(&bytes)
            .map_err(|error| error.to_string())?;
        let len = u16::try_from(cipher.len()).map_err(|_| "ciphertext too large")?;
        self.stream
            .write_all(&len.to_be_bytes())
            .and_then(|()| self.stream.write_all(&cipher))
            .and_then(|()| self.stream.flush())
            .map_err(|error| error.to_string())
    }

    fn try_frame(&mut self) -> Result<Option<Frame>, String> {
        loop {
            if self.buffer.len() >= 2 {
                let len = u16::from_be_bytes([self.buffer[0], self.buffer[1]]) as usize;
                if !(16..=65535).contains(&len) {
                    return Err("invalid transport frame length".into());
                }
                if self.buffer.len() >= 2 + len {
                    let cipher = self.buffer[2..2 + len].to_vec();
                    self.buffer.drain(..2 + len);
                    let plain = self
                        .session
                        .open(&cipher)
                        .map_err(|error| error.to_string())?;
                    return Frame::decode(&plain)
                        .map(Some)
                        .map_err(|error| error.to_string());
                }
            }
            let mut chunk = [0u8; 8192];
            match self.stream.read(&mut chunk) {
                Ok(0) => return Err("peer closed the connection".into()),
                Ok(count) => self.buffer.extend_from_slice(&chunk[..count]),
                Err(error)
                    if matches!(
                        error.kind(),
                        ErrorKind::WouldBlock | ErrorKind::TimedOut | ErrorKind::Interrupted
                    ) =>
                {
                    return Ok(None);
                }
                Err(error) => return Err(format!("read: {error}")),
            }
        }
    }
}

struct ReceiveState {
    id: String,
    text: Option<BlobMeta>,
    html: Option<BlobMeta>,
    image: Option<BlobMeta>,
    files: Option<Manifest>,
    text_buf: Vec<u8>,
    html_buf: Vec<u8>,
    image_buf: Vec<u8>,
    sink: Option<FileSink>,
    ended: HashSet<BlobKind>,
}

impl ReceiveState {
    #[allow(clippy::too_many_arguments)]
    fn create(
        cache_dir: &Path,
        id: String,
        text: Option<BlobMeta>,
        html: Option<BlobMeta>,
        image: Option<BlobMeta>,
        files: Option<Manifest>,
        max_transfer_bytes: u64,
    ) -> Result<Self, String> {
        if text.as_ref().is_some_and(|meta| meta.size > MAX_TEXT_BYTES) {
            return Err("peer text exceeds the 16 MiB limit".into());
        }
        if html.as_ref().is_some_and(|meta| meta.size > MAX_HTML_BYTES) {
            return Err("peer HTML exceeds the 16 MiB limit".into());
        }
        if image
            .as_ref()
            .is_some_and(|meta| meta.size > MAX_IMAGE_BYTES)
        {
            return Err("peer image exceeds the 64 MiB limit".into());
        }
        if files
            .as_ref()
            .is_some_and(|manifest| manifest.total_size > max_transfer_bytes)
        {
            return Err("peer files exceed the transfer limit".into());
        }
        let sink = match &files {
            Some(manifest) => {
                Some(FileSink::create(cache_dir, &id, manifest).map_err(|e| e.to_string())?)
            }
            None => None,
        };
        Ok(Self {
            id,
            text,
            html,
            image,
            files,
            text_buf: Vec::new(),
            html_buf: Vec::new(),
            image_buf: Vec::new(),
            sink,
            ended: HashSet::new(),
        })
    }

    fn chunk(&mut self, kind: BlobKind, offset: u64, data: &[u8]) -> Result<(), String> {
        match kind {
            BlobKind::Text => write_buffer(&mut self.text_buf, self.text.as_ref(), offset, data),
            BlobKind::Html => write_buffer(&mut self.html_buf, self.html.as_ref(), offset, data),
            BlobKind::Image => write_buffer(&mut self.image_buf, self.image.as_ref(), offset, data),
            BlobKind::File => {
                let manifest = self.files.as_ref().ok_or("unexpected file chunk")?;
                let sink = self.sink.as_mut().ok_or("missing file sink")?;
                sink.write_chunk(manifest, offset, data)
                    .map_err(|error| error.to_string())
            }
        }
    }

    fn end(&mut self, kind: BlobKind) -> Result<(), String> {
        match kind {
            BlobKind::Text => verify_len(&self.text_buf, self.text.as_ref(), "text")?,
            BlobKind::Html => verify_len(&self.html_buf, self.html.as_ref(), "HTML")?,
            BlobKind::Image => verify_len(&self.image_buf, self.image.as_ref(), "image")?,
            BlobKind::File => {}
        }
        self.ended.insert(kind);
        Ok(())
    }

    fn complete(&self) -> bool {
        self.text
            .as_ref()
            .is_none_or(|_| self.ended.contains(&BlobKind::Text))
            && self
                .html
                .as_ref()
                .is_none_or(|_| self.ended.contains(&BlobKind::Html))
            && self
                .image
                .as_ref()
                .is_none_or(|_| self.ended.contains(&BlobKind::Image))
            && self
                .files
                .as_ref()
                .is_none_or(|_| self.ended.contains(&BlobKind::File))
    }

    fn finish(self) -> Result<ClipboardContent, String> {
        let text = if let Some(meta) = &self.text {
            if model::bytes_hash(&self.text_buf) != meta.hash {
                return Err("text hash mismatch".into());
            }
            Some(String::from_utf8(self.text_buf).map_err(|_| "text is not UTF-8")?)
        } else {
            None
        };
        let html = if let Some(meta) = &self.html {
            if model::bytes_hash(&self.html_buf) != meta.hash {
                return Err("HTML hash mismatch".into());
            }
            Some(String::from_utf8(self.html_buf).map_err(|_| "HTML is not UTF-8")?)
        } else {
            None
        };
        let image = if let Some(meta) = &self.image {
            if model::bytes_hash(&self.image_buf) != meta.hash {
                return Err("image hash mismatch".into());
            }
            Some(self.image_buf)
        } else {
            None
        };
        let files = if let Some(manifest) = &self.files {
            let sink = self.sink.ok_or("missing file sink")?;
            let directory = sink.finish().map_err(|error| error.to_string())?;
            let mut roots = std::collections::BTreeSet::new();
            for entry in &manifest.entries {
                if let Some(root) = entry.path.split('/').next() {
                    roots.insert(root.to_owned());
                }
            }
            roots.into_iter().map(|root| directory.join(root)).collect()
        } else {
            Vec::new()
        };
        Ok(ClipboardContent {
            text,
            html,
            image,
            files,
        })
    }

    fn cancel(self) {
        if let Some(sink) = self.sink {
            sink.cancel();
        }
    }
}

fn write_buffer(
    buffer: &mut Vec<u8>,
    meta: Option<&BlobMeta>,
    offset: u64,
    data: &[u8],
) -> Result<(), String> {
    let meta = meta.ok_or("chunk for a missing payload")?;
    let end = offset
        .checked_add(data.len() as u64)
        .ok_or("chunk offset overflow")?;
    if end > meta.size {
        return Err("chunk exceeds payload size".into());
    }
    let offset = offset as usize;
    if buffer.len() < offset + data.len() {
        buffer.resize(offset + data.len(), 0);
    }
    buffer[offset..offset + data.len()].copy_from_slice(data);
    Ok(())
}

fn verify_len(buffer: &[u8], meta: Option<&BlobMeta>, name: &str) -> Result<(), String> {
    if let Some(meta) = meta
        && buffer.len() as u64 != meta.size
    {
        return Err(format!("{name} length mismatch"));
    }
    Ok(())
}

fn set_status(status: &Arc<Mutex<Status>>, update: impl FnOnce(&mut Status)) {
    if let Ok(mut status) = status.lock() {
        update(&mut status);
    }
}

fn new_id() -> String {
    use rand::RngCore;
    let mut bytes = [0u8; 16];
    rand::rng().fill_bytes(&mut bytes);
    model::hex(&bytes)
}

fn timestamp() -> String {
    SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .map(|duration| duration.as_secs().to_string())
        .unwrap_or_else(|_| "?".into())
}

// Re-export the cache helper used by the host.
pub fn default_cache_dir() -> PathBuf {
    cache_root()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::backend::FakeBackend;
    use crate::notify::NoopNotifier;
    use std::io::{Read, Write};
    use std::net::TcpListener;

    fn free_port() -> u16 {
        TcpListener::bind("127.0.0.1:0")
            .unwrap()
            .local_addr()
            .unwrap()
            .port()
    }

    fn key_file(dir: &std::path::Path) -> PathBuf {
        let path = dir.join("clipboard.key");
        crypto::write_key(&path, &crypto::generate_key()).unwrap();
        path
    }

    fn wait_until(timeout: Duration, mut check: impl FnMut() -> bool) -> bool {
        let deadline = Instant::now() + timeout;
        while Instant::now() < deadline {
            if check() {
                return true;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        false
    }

    #[test]
    fn loopback_all_content_types_and_reverse_direction() {
        let dir = tempfile::tempdir().unwrap();
        let key = key_file(dir.path());
        let port = free_port();
        let listener_fake = FakeBackend::new();
        let connector_fake = FakeBackend::new();
        let mut listener = start_with_notifier(
            Settings::with_defaults(
                Role::Listen,
                format!("127.0.0.1:{port}"),
                "",
                key.clone(),
                dir.path().join("cache-listen"),
                "listener",
            ),
            Box::new(listener_fake.clone()),
            Arc::new(NoopNotifier),
        )
        .unwrap();
        let mut connector = start_with_notifier(
            Settings::with_defaults(
                Role::Connect,
                "",
                format!("127.0.0.1:{port}"),
                key,
                dir.path().join("cache-connect"),
                "connector",
            ),
            Box::new(connector_fake.clone()),
            Arc::new(NoopNotifier),
        )
        .unwrap();
        assert!(wait_until(Duration::from_secs(5), || {
            listener.status().connected && connector.status().connected
        }));

        let source = dir.path().join("source");
        std::fs::create_dir_all(&source).unwrap();
        std::fs::write(source.join("中文.txt"), b"file-body").unwrap();
        let png = b"\x89PNG\r\n\x1a\nfake".to_vec();
        let content = ClipboardContent {
            text: Some("text-body".into()),
            html: Some("<p>html-中文</p>".into()),
            image: Some(png.clone()),
            files: vec![source.clone()],
        };
        listener_fake.set_local(content.clone());
        assert!(wait_until(Duration::from_secs(8), || {
            !connector_fake.writes().is_empty()
        }));
        let received = connector_fake.writes().pop().unwrap();
        connector_fake.clear_writes();
        assert_eq!(received.text.as_deref(), Some("text-body"));
        assert_eq!(received.html.as_deref(), Some("<p>html-中文</p>"));
        assert_eq!(received.image.as_deref(), Some(png.as_slice()));
        assert_eq!(received.files.len(), 1);
        assert_eq!(
            std::fs::read(received.files[0].join("中文.txt")).unwrap(),
            b"file-body"
        );

        // Reverse direction must work through the same persistent connection.
        let reverse = ClipboardContent {
            text: Some("reverse".into()),
            ..Default::default()
        };
        connector_fake.set_local(reverse.clone());
        assert!(wait_until(Duration::from_secs(5), || {
            !listener_fake.writes().is_empty()
        }));
        assert_eq!(
            listener_fake.writes().pop().unwrap().text.as_deref(),
            Some("reverse")
        );
        listener_fake.clear_writes();

        // No echo loops after both directions settle.
        std::thread::sleep(Duration::from_millis(700));
        assert_eq!(connector_fake.writes().len(), 0);
        assert_eq!(listener_fake.writes().len(), 0);
        connector.stop();
        listener.stop();
    }

    #[test]
    #[ignore = "takes 60 seconds; run manually for the idle acceptance gate"]
    fn idle_one_minute_has_zero_sends() {
        let dir = tempfile::tempdir().unwrap();
        let key = key_file(dir.path());
        let port = free_port();
        let listener_fake = FakeBackend::new();
        let connector_fake = FakeBackend::new();
        let mut listener = start_with_notifier(
            Settings::with_defaults(
                Role::Listen,
                format!("127.0.0.1:{port}"),
                "",
                key.clone(),
                dir.path().join("cache-listen"),
                "listener",
            ),
            Box::new(listener_fake.clone()),
            Arc::new(NoopNotifier),
        )
        .unwrap();
        let mut connector = start_with_notifier(
            Settings::with_defaults(
                Role::Connect,
                "",
                format!("127.0.0.1:{port}"),
                key,
                dir.path().join("cache-connect"),
                "connector",
            ),
            Box::new(connector_fake),
            Arc::new(NoopNotifier),
        )
        .unwrap();
        assert!(wait_until(Duration::from_secs(5), || {
            listener.status().connected && connector.status().connected
        }));
        std::thread::sleep(Duration::from_secs(60));
        assert_eq!(listener.status().sent_count, 0);
        assert_eq!(connector.status().sent_count, 0);
        connector.stop();
        listener.stop();
    }

    #[test]
    fn loopback_text_is_sent_once_and_not_echoed() {
        let dir = tempfile::tempdir().unwrap();
        let key = key_file(dir.path());
        let port = free_port();
        let listener_fake = FakeBackend::new();
        let connector_fake = FakeBackend::new();
        let mut listener = start_with_notifier(
            Settings::with_defaults(
                Role::Listen,
                format!("127.0.0.1:{port}"),
                "",
                key.clone(),
                dir.path().join("cache"),
                "listener",
            ),
            Box::new(listener_fake.clone()),
            Arc::new(NoopNotifier),
        )
        .unwrap();
        let mut connector = start_with_notifier(
            Settings::with_defaults(
                Role::Connect,
                "",
                format!("127.0.0.1:{port}"),
                key,
                dir.path().join("cache2"),
                "connector",
            ),
            Box::new(connector_fake.clone()),
            Arc::new(NoopNotifier),
        )
        .unwrap();
        assert!(wait_until(Duration::from_secs(5), || {
            listener.status().connected && connector.status().connected
        }));
        listener_fake.set_local(ClipboardContent {
            text: Some("hello-中文".into()),
            ..Default::default()
        });
        assert!(wait_until(Duration::from_secs(5), || {
            !connector_fake.writes().is_empty()
        }));
        assert_eq!(
            connector_fake.writes()[0].text.as_deref(),
            Some("hello-中文")
        );
        // The echo marker prevents the receiver from sending it back.
        std::thread::sleep(Duration::from_millis(600));
        assert!(listener_fake.writes().is_empty());
        assert_eq!(listener.status().sent_count, 1);
        assert_eq!(connector.status().received_count, 1);
        connector.stop();
        listener.stop();
    }

    fn raw_client(port: u16, key: &[u8; crypto::KEY_BYTES]) -> (TcpStream, crypto::Session) {
        let deadline = Instant::now() + Duration::from_secs(3);
        let mut stream = loop {
            match TcpStream::connect(("127.0.0.1", port)) {
                Ok(stream) => break stream,
                Err(error) if Instant::now() < deadline => {
                    let _ = error;
                    std::thread::sleep(Duration::from_millis(20));
                }
                Err(error) => panic!("raw client connect: {error}"),
            }
        };
        let session = crypto::Session::initiator(&mut stream, key).unwrap();
        (stream, session)
    }

    fn send_hello(stream: &mut TcpStream, session: &mut crypto::Session) {
        let frame = Message::Hello {
            proto_version: PROTO_VERSION,
            host_name: "raw".into(),
        }
        .frame()
        .unwrap();
        let bytes = frame.encode().unwrap();
        let cipher = session.seal(&bytes).unwrap();
        let len = u16::try_from(cipher.len()).unwrap();
        stream.write_all(&len.to_be_bytes()).unwrap();
        stream.write_all(&cipher).unwrap();
        stream.flush().unwrap();
    }

    fn eventually_closed(stream: &mut TcpStream) -> bool {
        stream
            .set_read_timeout(Some(Duration::from_secs(1)))
            .unwrap();
        let mut buffer = [0u8; 256];
        for _ in 0..10 {
            match stream.read(&mut buffer) {
                Ok(0) | Err(_) => return true,
                Ok(_) => {}
            }
        }
        false
    }

    fn listen_settings(
        port: u16,
        key: PathBuf,
        cache: PathBuf,
        silence_timeout: Duration,
    ) -> Settings {
        let mut settings = Settings::with_defaults(
            Role::Listen,
            format!("127.0.0.1:{port}"),
            "",
            key,
            cache,
            "listener",
        );
        settings.silence_timeout = silence_timeout;
        settings
    }

    #[test]
    fn silent_peer_times_out_and_a_new_connection_is_accepted() {
        let dir = tempfile::tempdir().unwrap();
        let key = key_file(dir.path());
        let key_bytes = crypto::read_key(&key).unwrap();
        let port = free_port();
        let fake = FakeBackend::new();
        let mut listener = start_with_notifier(
            listen_settings(
                port,
                key.clone(),
                dir.path().join("cache"),
                Duration::from_millis(300),
            ),
            Box::new(fake),
            Arc::new(NoopNotifier),
        )
        .unwrap();
        let (mut first, mut first_session) = raw_client(port, &key_bytes);
        send_hello(&mut first, &mut first_session);
        assert!(wait_until(Duration::from_secs(5), || {
            listener.status().connected
        }));
        assert!(wait_until(Duration::from_secs(3), || {
            listener.status().last_error.as_deref() == Some("peer silent")
        }));
        assert!(!listener.status().connected);
        assert!(eventually_closed(&mut first));

        let (mut second, mut second_session) = raw_client(port, &key_bytes);
        send_hello(&mut second, &mut second_session);
        assert!(wait_until(Duration::from_secs(5), || {
            listener.status().connected
        }));
        assert_eq!(
            listener.status().peer,
            second.local_addr().unwrap().to_string()
        );
        listener.stop();
    }

    #[test]
    fn a_new_connection_replaces_a_still_live_peer() {
        let dir = tempfile::tempdir().unwrap();
        let key = key_file(dir.path());
        let key_bytes = crypto::read_key(&key).unwrap();
        let port = free_port();
        let fake = FakeBackend::new();
        let mut listener = start_with_notifier(
            listen_settings(
                port,
                key.clone(),
                dir.path().join("cache"),
                Duration::from_secs(5),
            ),
            Box::new(fake),
            Arc::new(NoopNotifier),
        )
        .unwrap();
        let (mut first, mut first_session) = raw_client(port, &key_bytes);
        send_hello(&mut first, &mut first_session);
        assert!(wait_until(Duration::from_secs(5), || {
            listener.status().connected
        }));
        let (mut second, mut second_session) = raw_client(port, &key_bytes);
        send_hello(&mut second, &mut second_session);
        assert!(wait_until(Duration::from_secs(5), || {
            listener.status().peer == second.local_addr().unwrap().to_string()
                && listener.status().connected
        }));
        assert!(eventually_closed(&mut first));
        listener.stop();
    }
}
