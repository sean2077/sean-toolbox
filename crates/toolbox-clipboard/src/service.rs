//! Clipboard bridge service.
//!
//! One worker thread owns all state and blocks on a single event channel:
//! local clipboard changes, frames from the peer, paste requests from the
//! backend and a one-second timer. Each connection has a reader thread
//! (blocking reads) and a writer thread (draining a bounded queue), so nothing
//! polls the network.
//!
//! Text, HTML and images are pushed right after a local copy. Files are only
//! announced (`FileOffer`); the receiver asks for them (`Fetch`) when an
//! application pastes. A paste waits up to [`PASTE_WAIT`]; a longer transfer
//! keeps running, a notification says when it is ready, and the next paste is
//! served from the receive cache.

use crate::backend::{
    Backend, BackendEvent, FetchError, FetchRequest, FetchResult, Fetcher, PASTE_WAIT,
};
use crate::crypto::{self, Session};
use crate::frame::{CHUNK_BYTES, Frame};
use crate::model::{
    BlobMeta, ClipboardContent, DEFAULT_MAX_TRANSFER_BYTES, MAX_HTML_BYTES, MAX_IMAGE_BYTES,
    MAX_TEXT_BYTES, Manifest, blob_meta, bytes_hash,
};
use crate::notify::{Notifier, desktop_notifier};
use crate::protocol::{BlobKind, Message, PROTO_VERSION};
use crate::sync::LoopGuard;
use crate::transfer::{
    self, CACHE_MAX_AGE, CACHE_MAX_BYTES, FileSink, cache_root, cleanup_cache, clear_cache,
};
use serde::{Deserialize, Serialize};
use std::{
    collections::HashMap,
    fs::File,
    io::{ErrorKind, Read, Write},
    net::{Shutdown, TcpListener, TcpStream, ToSocketAddrs},
    path::{Path, PathBuf},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
        mpsc::{self, Receiver, RecvTimeoutError, Sender, SyncSender},
    },
    thread,
    time::{Duration, Instant, SystemTime},
};

pub const HEARTBEAT: Duration = Duration::from_secs(15);
pub const CONNECT_TIMEOUT: Duration = Duration::from_secs(4);
const HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(10);
const WRITE_TIMEOUT: Duration = Duration::from_secs(30);
const MAX_BACKOFF: Duration = Duration::from_secs(30);
const TICK: Duration = Duration::from_secs(1);
const IDLE_POLL: Duration = Duration::from_millis(200);
const CLEANUP_INTERVAL: Duration = Duration::from_secs(60 * 60);
const OUTBOUND_FRAMES: usize = 64;

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
    /// Close the connection when nothing arrives for this long. Both sides
    /// send a ping every [`HEARTBEAT`], so a live peer is never silent.
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

/// A running file transfer.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Transfer {
    pub receiving: bool,
    pub done: u64,
    pub total: u64,
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
    pub transfer: Option<Transfer>,
    pub cache_bytes: u64,
    pub last_error: Option<String>,
}

pub struct Handle {
    status: Arc<Mutex<Status>>,
    stop: Arc<AtomicBool>,
    events: Sender<Event>,
    thread: Option<thread::JoinHandle<()>>,
}

impl Handle {
    pub fn status(&self) -> Status {
        self.status.lock().unwrap().clone()
    }

    /// Remove received files, except a transfer that is still running.
    pub fn clear_cache(&self) {
        let _ = self.events.send(Event::ClearCache);
    }

    pub fn stop(&mut self) {
        self.stop.store(true, Ordering::Release);
        let _ = self.events.send(Event::Stop);
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
    mut backend: Box<dyn Backend>,
    notifier: Arc<dyn Notifier>,
) -> Result<Handle, String> {
    let key =
        crypto::read_key(&settings.key_path).map_err(|error| format!("clipboard key: {error}"))?;
    let status = Arc::new(Mutex::new(Status {
        enabled: true,
        role: settings.role.as_str().into(),
        peer: match settings.role {
            Role::Connect => settings.peer.clone(),
            Role::Listen => settings.listen.clone(),
        },
        ..Default::default()
    }));
    let stop = Arc::new(AtomicBool::new(false));
    let (events, inbox) = mpsc::channel();

    let (backend_tx, backend_rx) = mpsc::channel();
    backend.watch(backend_tx)?;
    let forward = events.clone();
    thread::spawn(move || {
        while let Ok(event) = backend_rx.recv() {
            if forward.send(Event::Backend(event)).is_err() {
                break;
            }
        }
    });

    let need_link = Arc::new(AtomicBool::new(true));
    match settings.role {
        Role::Listen => {
            let listener = TcpListener::bind(&settings.listen)
                .map_err(|error| format!("cannot listen on {}: {error}", settings.listen))?;
            let (events, stop, status) = (events.clone(), stop.clone(), status.clone());
            thread::spawn(move || accept_loop(listener, key, events, stop, status));
        }
        Role::Connect => {
            let (events, stop, status) = (events.clone(), stop.clone(), status.clone());
            let (peer, need_link) = (settings.peer.clone(), need_link.clone());
            thread::spawn(move || connect_loop(peer, key, events, stop, status, need_link));
        }
    }

    let fetch_events = events.clone();
    let fetcher = Fetcher::new(move |request| fetch_events.send(Event::Fetch(request)).is_ok());
    let worker = Worker {
        settings,
        backend,
        fetcher,
        notifier,
        status: status.clone(),
        events: events.clone(),
        need_link,
        guard: LoopGuard::new(),
        connection: None,
        next_connection: 0,
        incoming: None,
        remote_files: None,
        local_files: None,
        send_job: None,
        last_cleanup: Instant::now(),
    };
    let thread = thread::spawn(move || worker.run(inbox));
    Ok(Handle {
        status,
        stop,
        events,
        thread: Some(thread),
    })
}

pub fn default_cache_dir() -> PathBuf {
    cache_root()
}

enum Event {
    Backend(BackendEvent),
    Fetch(FetchRequest),
    Connected(TcpStream, Session),
    Frame(u64, Frame),
    Closed(u64, String),
    SendDone(String, Result<(), String>),
    ClearCache,
    Stop,
}

// ---------------------------------------------------------------------------
// Connections

fn handshake(
    mut stream: TcpStream,
    initiator: bool,
    key: &[u8; crypto::KEY_BYTES],
) -> Result<(TcpStream, Session), String> {
    // Frames are small and a paste waits for them: disable Nagle.
    stream
        .set_nodelay(true)
        .map_err(|error| error.to_string())?;
    stream
        .set_read_timeout(Some(HANDSHAKE_TIMEOUT))
        .map_err(|error| error.to_string())?;
    stream
        .set_write_timeout(Some(HANDSHAKE_TIMEOUT))
        .map_err(|error| error.to_string())?;
    let session = if initiator {
        Session::initiator(&mut stream, key)
    } else {
        Session::responder(&mut stream, key)
    }
    .map_err(|error| format!("Noise handshake: {error}"))?;
    Ok((stream, session))
}

fn accept_loop(
    listener: TcpListener,
    key: [u8; crypto::KEY_BYTES],
    events: Sender<Event>,
    stop: Arc<AtomicBool>,
    status: Arc<Mutex<Status>>,
) {
    if listener.set_nonblocking(true).is_err() {
        return;
    }
    while !stop.load(Ordering::Acquire) {
        match listener.accept() {
            Ok((stream, _)) => {
                let _ = stream.set_nonblocking(false);
                // A new authenticated connection replaces the current one, so
                // a peer that reconnects after sleep is never locked out.
                match handshake(stream, false, &key) {
                    Ok((stream, session)) => {
                        if events.send(Event::Connected(stream, session)).is_err() {
                            return;
                        }
                    }
                    Err(error) => set_status(&status, |status| status.last_error = Some(error)),
                }
            }
            Err(error) if error.kind() == ErrorKind::WouldBlock => thread::sleep(IDLE_POLL),
            Err(error) => {
                set_status(&status, |status| {
                    status.last_error = Some(format!("accept: {error}"));
                });
                thread::sleep(Duration::from_secs(1));
            }
        }
    }
}

fn connect_loop(
    peer: String,
    key: [u8; crypto::KEY_BYTES],
    events: Sender<Event>,
    stop: Arc<AtomicBool>,
    status: Arc<Mutex<Status>>,
    need_link: Arc<AtomicBool>,
) {
    let mut backoff = Duration::from_secs(1);
    while !stop.load(Ordering::Acquire) {
        if !need_link.load(Ordering::Acquire) {
            thread::sleep(IDLE_POLL);
            continue;
        }
        match dial(&peer, &key) {
            Ok((stream, session)) => {
                backoff = Duration::from_secs(1);
                need_link.store(false, Ordering::Release);
                if events.send(Event::Connected(stream, session)).is_err() {
                    return;
                }
            }
            Err(error) => {
                set_status(&status, |status| {
                    status.connected = false;
                    status.last_error = Some(error);
                });
                // Sleep in short steps so stopping stays responsive.
                let until = Instant::now() + backoff;
                while Instant::now() < until && !stop.load(Ordering::Acquire) {
                    thread::sleep(IDLE_POLL);
                }
                backoff = (backoff * 2).min(MAX_BACKOFF);
            }
        }
    }
}

fn dial(peer: &str, key: &[u8; crypto::KEY_BYTES]) -> Result<(TcpStream, Session), String> {
    // Resolve on every attempt so a host name follows address changes.
    let addresses = peer
        .to_socket_addrs()
        .map_err(|error| format!("invalid peer {peer}: {error}"))?;
    let mut last = format!("no address for {peer}");
    for address in addresses {
        match TcpStream::connect_timeout(&address, CONNECT_TIMEOUT) {
            Ok(stream) => return handshake(stream, true, key),
            Err(error) => last = format!("connect {address}: {error}"),
        }
    }
    Err(last)
}

struct Connection {
    id: u64,
    stream: TcpStream,
    outbound: SyncSender<Frame>,
    hello: bool,
    last_ping: Instant,
}

impl Connection {
    fn open(
        id: u64,
        stream: TcpStream,
        session: Session,
        silence: Duration,
        events: Sender<Event>,
    ) -> Result<Self, String> {
        // The read timeout doubles as the dead-peer detector.
        stream
            .set_read_timeout(Some(silence))
            .map_err(|error| error.to_string())?;
        stream
            .set_write_timeout(Some(WRITE_TIMEOUT))
            .map_err(|error| error.to_string())?;
        let session = Arc::new(Mutex::new(session));
        let (outbound, queue) = mpsc::sync_channel::<Frame>(OUTBOUND_FRAMES);

        let mut writer = stream.try_clone().map_err(|error| error.to_string())?;
        let (write_session, write_events) = (session.clone(), events.clone());
        thread::spawn(move || {
            while let Ok(frame) = queue.recv() {
                if let Err(error) = write_frame(&mut writer, &write_session, &frame) {
                    let _ = writer.shutdown(Shutdown::Both);
                    let _ = write_events.send(Event::Closed(id, error));
                    return;
                }
            }
        });

        let mut reader = stream.try_clone().map_err(|error| error.to_string())?;
        thread::spawn(move || {
            loop {
                match read_frame(&mut reader, &session) {
                    Ok(frame) => {
                        if events.send(Event::Frame(id, frame)).is_err() {
                            return;
                        }
                    }
                    Err(error) => {
                        let _ = events.send(Event::Closed(id, error));
                        return;
                    }
                }
            }
        });

        Ok(Self {
            id,
            stream,
            outbound,
            hello: false,
            last_ping: Instant::now(),
        })
    }

    fn send(&self, message: &Message) -> Result<(), String> {
        let frame = message.frame().map_err(|error| error.to_string())?;
        self.send_frame(frame)
    }

    fn send_frame(&self, frame: Frame) -> Result<(), String> {
        self.outbound
            .send(frame)
            .map_err(|_| "connection closed".to_string())
    }

    fn close(&self) {
        let _ = self.stream.shutdown(Shutdown::Both);
    }
}

fn write_frame(
    stream: &mut TcpStream,
    session: &Mutex<Session>,
    frame: &Frame,
) -> Result<(), String> {
    let plain = frame.encode().map_err(|error| error.to_string())?;
    let cipher = session
        .lock()
        .unwrap()
        .seal(&plain)
        .map_err(|error| error.to_string())?;
    let len = u16::try_from(cipher.len()).map_err(|_| "ciphertext too large")?;
    // One write per frame: a separate length write would be its own segment.
    let mut packet = Vec::with_capacity(2 + cipher.len());
    packet.extend_from_slice(&len.to_be_bytes());
    packet.extend_from_slice(&cipher);
    stream
        .write_all(&packet)
        .map_err(|error| format!("write: {error}"))
}

fn read_frame(stream: &mut TcpStream, session: &Mutex<Session>) -> Result<Frame, String> {
    fn read(stream: &mut TcpStream, buffer: &mut [u8]) -> Result<(), String> {
        stream
            .read_exact(buffer)
            .map_err(|error| match error.kind() {
                ErrorKind::WouldBlock | ErrorKind::TimedOut => "peer silent".to_string(),
                ErrorKind::UnexpectedEof => "peer closed the connection".to_string(),
                _ => format!("read: {error}"),
            })
    }
    let mut length = [0u8; 2];
    read(stream, &mut length)?;
    let length = u16::from_be_bytes(length) as usize;
    if length < 16 {
        return Err("invalid transport frame length".into());
    }
    let mut cipher = vec![0u8; length];
    read(stream, &mut cipher)?;
    let plain = session
        .lock()
        .unwrap()
        .open(&cipher)
        .map_err(|error| error.to_string())?;
    Frame::decode(&plain).map_err(|error| error.to_string())
}

// ---------------------------------------------------------------------------
// Worker state

/// Text/HTML/image payloads being received for one `Offer`.
struct Incoming {
    id: String,
    parts: HashMap<BlobKind, (BlobMeta, Vec<u8>, bool)>,
}

enum FileState {
    Idle,
    Receiving {
        sink: FileSink,
        done: u64,
        started: Instant,
        announced: bool,
    },
    Ready {
        dir: PathBuf,
        paths: Vec<PathBuf>,
    },
}

/// Files the peer announced and that are on the local clipboard.
struct RemoteFiles {
    id: String,
    manifest: Manifest,
    state: FileState,
    waiters: Vec<Sender<FetchResult>>,
}

/// Files copied locally and announced to the peer.
struct LocalFiles {
    id: String,
    manifest: Manifest,
    roots: HashMap<String, PathBuf>,
}

struct SendJob {
    id: String,
    cancel: Arc<AtomicBool>,
}

struct Worker {
    settings: Settings,
    backend: Box<dyn Backend>,
    fetcher: Fetcher,
    notifier: Arc<dyn Notifier>,
    status: Arc<Mutex<Status>>,
    events: Sender<Event>,
    need_link: Arc<AtomicBool>,
    guard: LoopGuard,
    connection: Option<Connection>,
    next_connection: u64,
    incoming: Option<Incoming>,
    remote_files: Option<RemoteFiles>,
    local_files: Option<LocalFiles>,
    send_job: Option<SendJob>,
    last_cleanup: Instant,
}

impl Worker {
    fn run(mut self, inbox: Receiver<Event>) {
        self.cleanup();
        loop {
            let wait = self.guard.next_deadline().map_or(TICK, |deadline| {
                deadline.saturating_duration_since(Instant::now()).min(TICK)
            });
            match inbox.recv_timeout(wait) {
                Ok(Event::Stop) | Err(RecvTimeoutError::Disconnected) => break,
                Ok(event) => self.handle(event),
                Err(RecvTimeoutError::Timeout) => {}
            }
            self.timers();
        }
        if let Some(connection) = self.connection.take() {
            connection.close();
        }
        self.cancel_send_job();
        self.drop_remote_files("clipboard service stopped");
    }

    fn handle(&mut self, event: Event) {
        match event {
            Event::Backend(BackendEvent::Changed) => self.local_change(),
            Event::Backend(BackendEvent::WriteFailed) => {
                self.fail("写入剪贴板失败：剪贴板被其他程序占用".into());
            }
            Event::Backend(BackendEvent::Unavailable) => {
                self.set_error("the clipboard backend stopped".into());
            }
            Event::Fetch(request) => self.paste_request(request),
            Event::Connected(stream, session) => self.connected(stream, session),
            Event::Frame(id, frame) => {
                if self.is_current(id)
                    && let Err(error) = self.frame(frame)
                {
                    self.disconnect(error);
                }
            }
            Event::Closed(id, error) => {
                if self.is_current(id) {
                    self.disconnect(error);
                }
            }
            Event::SendDone(id, result) => self.send_done(id, result),
            Event::ClearCache => {
                let active = self.active_temp();
                let left = clear_cache(&self.settings.cache_dir, active.as_deref());
                if let Some(files) = &mut self.remote_files
                    && matches!(files.state, FileState::Ready { .. })
                {
                    files.state = FileState::Idle;
                }
                set_status(&self.status, |status| status.cache_bytes = left);
            }
            Event::Stop => {}
        }
    }

    fn is_current(&self, id: u64) -> bool {
        self.connection
            .as_ref()
            .is_some_and(|connection| connection.id == id)
    }

    fn connected_peer(&self) -> bool {
        self.connection
            .as_ref()
            .is_some_and(|connection| connection.hello)
    }

    fn timers(&mut self) {
        let now = Instant::now();
        if let Some(connection) = &mut self.connection
            && connection.hello
            && now.duration_since(connection.last_ping) >= HEARTBEAT
        {
            connection.last_ping = now;
            let _ = connection.send(&Message::Ping);
        }
        if let Some((content, _)) = self.guard.ready(now) {
            self.publish(content);
        }
        if let Some(files) = &mut self.remote_files
            && let FileState::Receiving {
                started, announced, ..
            } = &mut files.state
            && !*announced
            && now.duration_since(*started) >= PASTE_WAIT
        {
            *announced = true;
            self.notifier.notify(
                "Sean Toolbox",
                &format!(
                    "正在接收 {} 个文件（{}），完成后再粘贴一次",
                    files.manifest.entries.len(),
                    human_size(files.manifest.total_size)
                ),
            );
        }
        if now.duration_since(self.last_cleanup) >= CLEANUP_INTERVAL {
            self.cleanup();
        }
    }

    fn cleanup(&mut self) {
        self.last_cleanup = Instant::now();
        let keep = match self.remote_files.as_ref().map(|files| &files.state) {
            Some(FileState::Ready { dir, .. }) => Some(dir.clone()),
            _ => None,
        };
        let active = self.active_temp();
        let left = cleanup_cache(
            &self.settings.cache_dir,
            CACHE_MAX_BYTES,
            CACHE_MAX_AGE,
            keep.as_deref(),
            active.as_deref(),
        );
        // An expired or evicted current item is fetched again on paste.
        if let Some(files) = &mut self.remote_files
            && let FileState::Ready { paths, .. } = &files.state
            && !paths.iter().all(|path| path.exists())
        {
            files.state = FileState::Idle;
        }
        set_status(&self.status, |status| status.cache_bytes = left);
    }

    fn active_temp(&self) -> Option<PathBuf> {
        match self.remote_files.as_ref().map(|files| &files.state) {
            Some(FileState::Receiving { sink, .. }) => Some(sink.temp_dir().to_owned()),
            _ => None,
        }
    }

    fn set_error(&self, error: String) {
        set_status(&self.status, |status| status.last_error = Some(error));
    }

    fn fail(&self, error: String) {
        self.notifier
            .notify("Sean Toolbox", &format!("剪贴板同步失败：{error}"));
        self.set_error(error);
    }

    // -- connection lifecycle --------------------------------------------------

    fn connected(&mut self, stream: TcpStream, session: Session) {
        if let Some(old) = self.connection.take() {
            old.close();
            self.connection_lost("replaced by a new connection");
        }
        self.next_connection += 1;
        let id = self.next_connection;
        let peer = stream.peer_addr().map(|address| address.to_string()).ok();
        let opened = Connection::open(
            id,
            stream,
            session,
            self.settings.silence_timeout,
            self.events.clone(),
        );
        let hello = Message::Hello {
            proto_version: PROTO_VERSION,
            host_name: self.settings.host_name.clone(),
        };
        match opened.and_then(|connection| connection.send(&hello).map(|()| connection)) {
            Ok(connection) => {
                self.connection = Some(connection);
                if let Some(peer) = peer {
                    set_status(&self.status, |status| status.peer = peer);
                }
            }
            Err(error) => {
                self.set_error(error);
                self.need_link.store(true, Ordering::Release);
            }
        }
    }

    fn disconnect(&mut self, error: String) {
        if let Some(connection) = self.connection.take() {
            connection.close();
        }
        self.need_link.store(true, Ordering::Release);
        set_status(&self.status, |status| {
            status.connected = false;
            status.last_error = Some(error);
        });
        self.connection_lost("connection lost");
    }

    /// Abort transfers that belonged to the closed connection.
    fn connection_lost(&mut self, reason: &str) {
        self.incoming = None;
        self.cancel_send_job();
        if let Some(files) = &mut self.remote_files
            && matches!(files.state, FileState::Receiving { .. })
        {
            if let FileState::Receiving { sink, .. } =
                std::mem::replace(&mut files.state, FileState::Idle)
            {
                sink.cancel();
            }
            for waiter in files.waiters.drain(..) {
                let _ = waiter.send(Err(FetchError::Failed(reason.to_owned())));
            }
            set_status(&self.status, |status| status.transfer = None);
        }
    }

    fn send(&self, message: &Message) -> Result<(), String> {
        match &self.connection {
            Some(connection) if connection.hello => connection.send(message),
            _ => Err("the other PC is not connected".into()),
        }
    }

    // -- local clipboard -------------------------------------------------------

    fn local_change(&mut self) {
        let Ok(read) = self.backend.read() else {
            return;
        };
        if read.remote_origin {
            return;
        }
        // The user replaced the clipboard: announced files are no longer on it.
        self.drop_remote_files("the clipboard changed");
        if !read.sensitive {
            self.guard.observe(read.content, Instant::now());
        }
    }

    /// Send a debounced local copy to the peer.
    fn publish(&mut self, content: ClipboardContent) {
        if !self.connected_peer() {
            return;
        }
        let result = if content.files.is_empty() {
            self.push_content(&content)
        } else {
            self.offer_files(&content)
        };
        match result {
            Ok(true) => {
                self.guard.note_sent(&content);
                set_status(&self.status, |status| {
                    status.last_sent = Some(format!("{} {}", content.kind(), timestamp()));
                    status.sent_count += 1;
                });
            }
            Ok(false) => {}
            Err(error) => self.disconnect(error),
        }
    }

    /// Push text, HTML and image. Oversized parts are skipped with a
    /// notification. Returns whether anything was sent.
    fn push_content(&mut self, content: &ClipboardContent) -> Result<bool, String> {
        let candidates: [(BlobKind, Option<&[u8]>, u64, &str); 3] = [
            (
                BlobKind::Text,
                content.text.as_deref().map(str::as_bytes),
                MAX_TEXT_BYTES,
                "文字",
            ),
            (
                BlobKind::Html,
                content.html.as_deref().map(str::as_bytes),
                MAX_HTML_BYTES,
                "HTML",
            ),
            (
                BlobKind::Image,
                content.image.as_deref(),
                MAX_IMAGE_BYTES,
                "图片",
            ),
        ];
        let mut parts = Vec::new();
        let mut skipped = Vec::new();
        for (kind, bytes, limit, name) in candidates {
            match bytes {
                Some(bytes) if bytes.len() as u64 > limit => {
                    skipped.push(format!("{name}超过 {}", human_size(limit)));
                }
                Some(bytes) => parts.push((kind, bytes)),
                None => {}
            }
        }
        if !skipped.is_empty() {
            self.notifier
                .notify("Sean Toolbox", &format!("未同步：{}", skipped.join("，")));
        }
        if parts.is_empty() {
            return Ok(false);
        }
        let meta = |kind| {
            parts
                .iter()
                .find(|(part, _)| *part == kind)
                .map(|(_, bytes)| blob_meta(bytes))
        };
        let id = new_id();
        let offer = Message::Offer {
            id: id.clone(),
            text: meta(BlobKind::Text),
            html: meta(BlobKind::Html),
            image: meta(BlobKind::Image),
        };
        let connection = self.connection.as_ref().ok_or("not connected")?;
        connection.send(&offer)?;
        for (kind, bytes) in &parts {
            for (index, chunk) in bytes.chunks(CHUNK_BYTES).enumerate() {
                let frame = Message::Chunk {
                    id: id.clone(),
                    kind: *kind,
                    offset: (index * CHUNK_BYTES) as u64,
                }
                .chunk_frame(chunk.to_vec())
                .map_err(|error| error.to_string())?;
                connection.send_frame(frame)?;
            }
            connection.send(&Message::End {
                id: id.clone(),
                kind: *kind,
            })?;
        }
        Ok(true)
    }

    /// Announce local files. Returns whether the offer was sent.
    fn offer_files(&mut self, content: &ClipboardContent) -> Result<bool, String> {
        let manifest =
            match transfer::collect_manifest(&content.files, self.settings.max_transfer_bytes) {
                Ok(manifest) => manifest,
                Err(transfer::TransferError::TooLarge(_)) => {
                    self.notifier.notify(
                        "Sean Toolbox",
                        &format!(
                            "文件总大小超过 {}，不会同步",
                            human_size(self.settings.max_transfer_bytes)
                        ),
                    );
                    return Ok(false);
                }
                Err(error) => {
                    self.fail(error.to_string());
                    return Ok(false);
                }
            };
        self.cancel_send_job();
        let roots = content
            .files
            .iter()
            .filter_map(|path| {
                let name = path.file_name()?.to_str()?.to_owned();
                Some((name, path.clone()))
            })
            .collect();
        let id = new_id();
        self.send(&Message::FileOffer {
            id: id.clone(),
            files: manifest.clone(),
        })?;
        self.local_files = Some(LocalFiles {
            id,
            manifest,
            roots,
        });
        Ok(true)
    }

    // -- peer messages ---------------------------------------------------------

    fn frame(&mut self, frame: Frame) -> Result<(), String> {
        let message = Message::from_frame(&frame).map_err(|error| error.to_string())?;
        match message {
            Message::Hello { proto_version, .. } => {
                if proto_version != PROTO_VERSION {
                    return Err(format!(
                        "the other PC runs protocol {proto_version}, this one {PROTO_VERSION}; update both"
                    ));
                }
                if let Some(connection) = &mut self.connection {
                    connection.hello = true;
                }
                set_status(&self.status, |status| {
                    status.connected = true;
                    status.last_error = None;
                });
            }
            Message::Offer {
                id,
                text,
                html,
                image,
            } => {
                self.drop_remote_files("the other PC copied something new");
                let mut parts = HashMap::new();
                for (kind, meta, limit) in [
                    (BlobKind::Text, text, MAX_TEXT_BYTES),
                    (BlobKind::Html, html, MAX_HTML_BYTES),
                    (BlobKind::Image, image, MAX_IMAGE_BYTES),
                ] {
                    if let Some(meta) = meta {
                        if meta.size > limit {
                            return Err("the other PC announced an oversized item".into());
                        }
                        let capacity = meta.size as usize;
                        parts.insert(kind, (meta, Vec::with_capacity(capacity), false));
                    }
                }
                self.incoming = Some(Incoming { id, parts });
            }
            Message::FileOffer { id, files } => self.file_offer(id, files)?,
            Message::Fetch { id } => self.serve_fetch(id)?,
            Message::FetchFailed { id, message } => self.file_failed(&id, message),
            Message::Chunk { id, kind, offset } => self.chunk(&id, kind, offset, &frame.data)?,
            Message::End { id, kind } => self.end(&id, kind)?,
            Message::Cancel { id } => {
                if self.send_job.as_ref().is_some_and(|job| job.id == id) {
                    self.cancel_send_job();
                }
                self.file_failed(&id, "the other PC cancelled the transfer".into());
            }
            Message::Ping => {}
        }
        Ok(())
    }

    fn chunk(&mut self, id: &str, kind: BlobKind, offset: u64, data: &[u8]) -> Result<(), String> {
        if kind == BlobKind::File {
            let Some(files) = &mut self.remote_files else {
                return Ok(());
            };
            if files.id != id {
                return Ok(());
            }
            let FileState::Receiving { sink, done, .. } = &mut files.state else {
                return Ok(());
            };
            if let Err(error) = sink.write_chunk(&files.manifest, offset, data) {
                let _ = self.send(&Message::Cancel { id: id.to_owned() });
                self.file_failed(id, error.to_string());
                return Ok(());
            }
            *done += data.len() as u64;
            let (done, total) = (*done, files.manifest.total_size);
            set_status(&self.status, |status| {
                status.transfer = Some(Transfer {
                    receiving: true,
                    done,
                    total,
                });
            });
            return Ok(());
        }
        let Some(incoming) = &mut self.incoming else {
            return Ok(());
        };
        if incoming.id != id {
            return Ok(());
        }
        let Some((meta, buffer, _)) = incoming.parts.get_mut(&kind) else {
            return Err("chunk for a part that was not announced".into());
        };
        if offset != buffer.len() as u64 || buffer.len() as u64 + data.len() as u64 > meta.size {
            return Err("out-of-order or oversized chunk".into());
        }
        buffer.extend_from_slice(data);
        Ok(())
    }

    fn end(&mut self, id: &str, kind: BlobKind) -> Result<(), String> {
        if kind == BlobKind::File {
            self.files_complete(id);
            return Ok(());
        }
        let Some(incoming) = &mut self.incoming else {
            return Ok(());
        };
        if incoming.id != id {
            return Ok(());
        }
        let Some((meta, buffer, done)) = incoming.parts.get_mut(&kind) else {
            return Err("end for a part that was not announced".into());
        };
        if buffer.len() as u64 != meta.size || bytes_hash(buffer) != meta.hash {
            return Err("clipboard payload failed verification".into());
        }
        *done = true;
        if !incoming.parts.values().all(|(_, _, done)| *done) {
            return Ok(());
        }
        let mut parts = self.incoming.take().expect("checked above").parts;
        let mut take = |kind| parts.remove(&kind).map(|(_, bytes, _)| bytes);
        let content = ClipboardContent {
            text: take(BlobKind::Text).map(|bytes| String::from_utf8_lossy(&bytes).into_owned()),
            html: take(BlobKind::Html).map(|bytes| String::from_utf8_lossy(&bytes).into_owned()),
            image: take(BlobKind::Image),
            files: Vec::new(),
        };
        match self.backend.write(&content) {
            Ok(()) => {
                self.guard.note_received(&content);
                set_status(&self.status, |status| {
                    status.last_received = Some(format!("{} {}", content.kind(), timestamp()));
                    status.received_count += 1;
                });
            }
            Err(error) => self.fail(format!("写入剪贴板失败：{error}")),
        }
        Ok(())
    }

    // -- files: receiver -------------------------------------------------------

    fn file_offer(&mut self, id: String, manifest: Manifest) -> Result<(), String> {
        manifest.validate()?;
        self.incoming = None;
        self.drop_remote_files("the other PC copied something new");
        if manifest.total_size > self.settings.max_transfer_bytes {
            self.notifier.notify(
                "Sean Toolbox",
                &format!(
                    "对方复制的文件超过 {}，不会同步",
                    human_size(self.settings.max_transfer_bytes)
                ),
            );
            return Ok(());
        }
        if let Err(error) = self.backend.offer_files(&id, self.fetcher.clone()) {
            self.fail(format!("写入剪贴板失败：{error}"));
            return Ok(());
        }
        self.remote_files = Some(RemoteFiles {
            id,
            manifest,
            state: FileState::Idle,
            waiters: Vec::new(),
        });
        Ok(())
    }

    /// An application pasted announced files.
    fn paste_request(&mut self, request: FetchRequest) {
        let connected = self.connected_peer();
        let Some(files) = self
            .remote_files
            .as_mut()
            .filter(|files| files.id == request.id)
        else {
            let _ = request.reply.send(Err(FetchError::Failed(
                "these files are no longer offered".into(),
            )));
            return;
        };
        if let FileState::Ready { paths, .. } = &files.state {
            if paths.iter().all(|path| path.exists()) {
                let _ = request.reply.send(Ok(paths.clone()));
                return;
            }
            files.state = FileState::Idle;
        }
        if matches!(files.state, FileState::Receiving { .. }) {
            files.waiters.push(request.reply);
            return;
        }
        if !connected {
            let _ = request.reply.send(Err(FetchError::Failed(
                "the other PC is not connected".into(),
            )));
            self.notifier
                .notify("Sean Toolbox", "对方电脑未连接，无法获取文件");
            return;
        }
        let sink = match FileSink::create(&self.settings.cache_dir, &files.id, &files.manifest) {
            Ok(sink) => sink,
            Err(error) => {
                let _ = request
                    .reply
                    .send(Err(FetchError::Failed(error.to_string())));
                self.fail(format!("无法创建接收目录：{error}"));
                return;
            }
        };
        let id = files.id.clone();
        let total = files.manifest.total_size;
        files.state = FileState::Receiving {
            sink,
            done: 0,
            started: Instant::now(),
            announced: false,
        };
        files.waiters.push(request.reply);
        set_status(&self.status, |status| {
            status.transfer = Some(Transfer {
                receiving: true,
                done: 0,
                total,
            });
        });
        if let Err(error) = self.send(&Message::Fetch { id: id.clone() }) {
            self.file_failed(&id, error);
        }
    }

    fn files_complete(&mut self, id: &str) {
        let Some(files) = self
            .remote_files
            .as_mut()
            .filter(|files| files.id == id && matches!(files.state, FileState::Receiving { .. }))
        else {
            return;
        };
        let FileState::Receiving {
            sink, announced, ..
        } = std::mem::replace(&mut files.state, FileState::Idle)
        else {
            return;
        };
        set_status(&self.status, |status| status.transfer = None);
        match sink.finish(&files.manifest) {
            Ok(paths) => {
                let dir = paths
                    .first()
                    .and_then(|path| path.parent())
                    .map(Path::to_path_buf)
                    .unwrap_or_else(|| self.settings.cache_dir.clone());
                for waiter in files.waiters.drain(..) {
                    let _ = waiter.send(Ok(paths.clone()));
                }
                files.state = FileState::Ready { dir, paths };
                if announced {
                    self.notifier
                        .notify("Sean Toolbox", "文件已就绪，可以粘贴了");
                }
                set_status(&self.status, |status| {
                    status.last_received = Some(format!("Files {}", timestamp()));
                    status.received_count += 1;
                });
                self.cleanup();
            }
            Err(error) => {
                for waiter in files.waiters.drain(..) {
                    let _ = waiter.send(Err(FetchError::Failed(error.to_string())));
                }
                self.fail(format!("接收文件失败：{error}"));
            }
        }
    }

    fn file_failed(&mut self, id: &str, message: String) {
        let Some(files) = self.remote_files.as_mut().filter(|files| files.id == id) else {
            return;
        };
        let state = std::mem::replace(&mut files.state, FileState::Idle);
        let was_receiving = matches!(state, FileState::Receiving { .. });
        if let FileState::Receiving { sink, .. } = state {
            sink.cancel();
        }
        for waiter in files.waiters.drain(..) {
            let _ = waiter.send(Err(FetchError::Failed(message.clone())));
        }
        if was_receiving {
            set_status(&self.status, |status| status.transfer = None);
            self.fail(format!("获取文件失败：{message}"));
        }
    }

    /// Forget announced files, cancelling a running transfer.
    fn drop_remote_files(&mut self, reason: &str) {
        let Some(mut files) = self.remote_files.take() else {
            return;
        };
        if let FileState::Receiving { sink, .. } =
            std::mem::replace(&mut files.state, FileState::Idle)
        {
            sink.cancel();
            let _ = self.send(&Message::Cancel {
                id: files.id.clone(),
            });
            set_status(&self.status, |status| status.transfer = None);
        }
        for waiter in files.waiters.drain(..) {
            let _ = waiter.send(Err(FetchError::Failed(reason.to_owned())));
        }
    }

    // -- files: sender ---------------------------------------------------------

    fn serve_fetch(&mut self, id: String) -> Result<(), String> {
        let Some(local) = self.local_files.as_ref().filter(|local| local.id == id) else {
            return self.send(&Message::FetchFailed {
                id,
                message: "a newer copy replaced these files".into(),
            });
        };
        if self.send_job.as_ref().is_some_and(|job| job.id == id) {
            return Ok(());
        }
        let connection = self.connection.as_ref().ok_or("not connected")?;
        let cancel = Arc::new(AtomicBool::new(false));
        let sender = FileSender {
            id: id.clone(),
            manifest: local.manifest.clone(),
            roots: local.roots.clone(),
            outbound: connection.outbound.clone(),
            cancel: cancel.clone(),
            status: self.status.clone(),
        };
        self.cancel_send_job();
        let events = self.events.clone();
        thread::spawn(move || {
            let id = sender.id.clone();
            let result = sender.run();
            let _ = events.send(Event::SendDone(id, result));
        });
        self.send_job = Some(SendJob { id, cancel });
        Ok(())
    }

    fn send_done(&mut self, id: String, result: Result<(), String>) {
        if self.send_job.as_ref().is_some_and(|job| job.id == id) {
            self.send_job = None;
        }
        set_status(&self.status, |status| status.transfer = None);
        match result {
            Ok(()) => set_status(&self.status, |status| {
                status.last_sent = Some(format!("Files {}", timestamp()));
            }),
            Err(error) => {
                let _ = self.send(&Message::FetchFailed {
                    id,
                    message: error.clone(),
                });
                self.set_error(error);
            }
        }
    }

    fn cancel_send_job(&mut self) {
        if let Some(job) = self.send_job.take() {
            job.cancel.store(true, Ordering::Release);
        }
    }
}

/// Streams announced files to the peer on its own thread.
struct FileSender {
    id: String,
    manifest: Manifest,
    roots: HashMap<String, PathBuf>,
    outbound: SyncSender<Frame>,
    cancel: Arc<AtomicBool>,
    status: Arc<Mutex<Status>>,
}

impl FileSender {
    fn run(self) -> Result<(), String> {
        let total = self.manifest.total_size;
        let mut offset = 0u64;
        let mut buffer = vec![0u8; CHUNK_BYTES];
        for entry in &self.manifest.entries {
            let path = self.local_path(&entry.path)?;
            let mut file =
                File::open(&path).map_err(|error| format!("open {}: {error}", path.display()))?;
            let size = file.metadata().map_err(|error| error.to_string())?.len();
            if size != entry.size {
                return Err(format!("{} changed after it was copied", path.display()));
            }
            let mut sent = 0u64;
            while sent < entry.size {
                if self.cancel.load(Ordering::Acquire) {
                    return Ok(());
                }
                let want = (entry.size - sent).min(CHUNK_BYTES as u64) as usize;
                let count = file
                    .read(&mut buffer[..want])
                    .map_err(|error| format!("read {}: {error}", path.display()))?;
                if count == 0 {
                    return Err(format!("{} changed after it was copied", path.display()));
                }
                let frame = Message::Chunk {
                    id: self.id.clone(),
                    kind: BlobKind::File,
                    offset: offset + sent,
                }
                .chunk_frame(buffer[..count].to_vec())
                .map_err(|error| error.to_string())?;
                if self.outbound.send(frame).is_err() {
                    return Ok(());
                }
                sent += count as u64;
                let done = offset + sent;
                set_status(&self.status, |status| {
                    status.transfer = Some(Transfer {
                        receiving: false,
                        done,
                        total,
                    });
                });
            }
            offset += entry.size;
        }
        let end = Message::End {
            id: self.id.clone(),
            kind: BlobKind::File,
        }
        .frame()
        .map_err(|error| error.to_string())?;
        let _ = self.outbound.send(end);
        Ok(())
    }

    fn local_path(&self, wire: &str) -> Result<PathBuf, String> {
        let mut parts = wire.split('/');
        let root = parts.next().unwrap_or_default();
        let base = self
            .roots
            .get(root)
            .ok_or_else(|| format!("{root} is no longer available"))?;
        Ok(parts.fold(base.clone(), |path, part| path.join(part)))
    }
}

fn set_status(status: &Arc<Mutex<Status>>, update: impl FnOnce(&mut Status)) {
    if let Ok(mut status) = status.lock() {
        update(&mut status);
    }
}

fn new_id() -> String {
    let mut bytes = [0u8; 8];
    rand::fill(&mut bytes);
    crate::model::hex(&bytes)
}

fn timestamp() -> String {
    SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .map(|duration| duration.as_secs().to_string())
        .unwrap_or_default()
}

fn human_size(bytes: u64) -> String {
    const MIB: u64 = 1024 * 1024;
    if bytes >= 1024 * MIB {
        format!("{:.1} GB", bytes as f64 / (1024 * MIB) as f64)
    } else if bytes >= MIB {
        format!("{:.0} MB", bytes as f64 / MIB as f64)
    } else {
        format!("{:.0} KB", (bytes as f64 / 1024.0).ceil())
    }
}

#[cfg(test)]
mod tests;
