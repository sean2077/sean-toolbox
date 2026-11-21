//! Linux Wayland clipboard backend using `ext_data_control_manager_v1`.
//!
//! KWin exposes the data-control protocol to regular clients, so the backend can
//! watch and replace the selection without stealing focus. The protocol runs on
//! its own thread; the service only sees a cached snapshot and a write queue.

use crate::backend::{
    Backend, BackendEvent, DEFERRED_FILE_TIMEOUT, DEFERRED_TIMEOUT, Fetcher, ORIGIN_MIME,
    ReadContent,
};
use crate::model::{ClipboardContent, DeferredOffer, blob_meta};
use std::{
    collections::HashMap,
    os::fd::{AsFd, AsRawFd, FromRawFd, OwnedFd, RawFd},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
        mpsc::Sender,
    },
    thread,
    time::{Duration, Instant},
};
use wayland_client::{
    Connection, Dispatch, Proxy, QueueHandle, event_created_child,
    protocol::{
        wl_registry::{self, WlRegistry},
        wl_seat::{self, WlSeat},
    },
};
use wayland_protocols::ext::data_control::v1::client::{
    ext_data_control_device_v1::{self, ExtDataControlDeviceV1},
    ext_data_control_manager_v1::ExtDataControlManagerV1,
    ext_data_control_offer_v1::{self, ExtDataControlOfferV1},
    ext_data_control_source_v1::{self, ExtDataControlSourceV1},
};

const READ_DEADLINE: Duration = Duration::from_secs(2);
const MAX_READ_BYTES: usize = 64 * 1024 * 1024;
const PASSWORD_HINT_MIME: &str = "x-kde-passwordManagerHint";
/// Clipboard monitors that read every new selection in the background and do
/// not honor the password-manager hint. fcitx5's clipboard addon would pull a
/// deferred item right after the copy. Klipper skips hinted entries itself.
const BACKGROUND_READERS: &[&str] = &["fcitx5"];

pub fn backend() -> Result<Box<dyn Backend>, String> {
    Ok(Box::new(LinuxBackend::new()))
}

pub struct LinuxBackend {
    inner: Arc<Inner>,
    started: bool,
}

struct Inner {
    latest: Mutex<Option<ReadContent>>,
    pending_write: Mutex<Option<ClipboardContent>>,
    pending_defer: Mutex<Option<DeferredOffer>>,
    fetcher: Mutex<Option<Fetcher>>,
    events: Mutex<Option<Sender<BackendEvent>>>,
    stop: AtomicBool,
}

impl LinuxBackend {
    pub fn new() -> Self {
        Self {
            inner: Arc::new(Inner {
                latest: Mutex::new(None),
                pending_write: Mutex::new(None),
                pending_defer: Mutex::new(None),
                fetcher: Mutex::new(None),
                events: Mutex::new(None),
                stop: AtomicBool::new(false),
            }),
            started: false,
        }
    }
}

impl Default for LinuxBackend {
    fn default() -> Self {
        Self::new()
    }
}

impl Drop for LinuxBackend {
    fn drop(&mut self) {
        self.inner.stop.store(true, Ordering::Release);
    }
}

impl Backend for LinuxBackend {
    fn watch(&mut self, events: Sender<BackendEvent>) -> Result<(), String> {
        if self.started {
            return Ok(());
        }
        self.started = true;
        *self.inner.events.lock().unwrap() = Some(events);
        let inner = self.inner.clone();
        thread::Builder::new()
            .name("toolbox-clipboard-wayland".into())
            .spawn(move || {
                if let Err(error) = run(inner.clone()) {
                    eprintln!("Sean Toolbox clipboard (Wayland): {error}");
                    if let Some(events) = inner.events.lock().unwrap().as_ref() {
                        let _ = events.send(BackendEvent::Unavailable);
                    }
                }
            })
            .map(|_| ())
            .map_err(|error| error.to_string())
    }

    fn read(&mut self) -> Result<ReadContent, String> {
        Ok(self
            .inner
            .latest
            .lock()
            .unwrap()
            .clone()
            .unwrap_or_default())
    }

    fn write(&mut self, content: &ClipboardContent) -> Result<(), String> {
        *self.inner.pending_write.lock().unwrap() = Some(content.clone());
        Ok(())
    }

    fn set_fetcher(&mut self, fetcher: Fetcher) {
        *self.inner.fetcher.lock().unwrap() = Some(fetcher);
    }

    fn defer(&mut self, offer: &DeferredOffer) -> Result<(), String> {
        *self.inner.pending_defer.lock().unwrap() = Some(offer.clone());
        Ok(())
    }
}

#[derive(Default)]
struct State {
    manager: Option<ExtDataControlManagerV1>,
    seat: Option<WlSeat>,
    device: Option<ExtDataControlDeviceV1>,
    source: Option<ExtDataControlSourceV1>,
    current_offer: Option<ExtDataControlOfferV1>,
    pending_offers: HashMap<u32, ExtDataControlOfferV1>,
    offers: HashMap<u32, Vec<String>>,
    read_at: Option<Instant>,
    finished: bool,
}

impl State {
    fn set_selection(
        &mut self,
        qh: &QueueHandle<Self>,
        content: &ClipboardContent,
        inner: &Arc<Inner>,
    ) -> Result<(), String> {
        let manager = self
            .manager
            .as_ref()
            .ok_or("data-control manager missing")?;
        let device = self.device.as_ref().ok_or("data-control device missing")?;
        if let Some(old) = self.source.take() {
            old.destroy();
        }
        let data = SourceData::new(content);
        let mimes = data.mimes.keys().cloned().collect::<Vec<_>>();
        let source = manager.create_data_source(qh, data);
        for mime in mimes {
            source.offer(mime);
        }
        device.set_selection(Some(&source));
        self.source = Some(source);
        *inner.latest.lock().unwrap() = Some(ReadContent {
            content: content.clone(),
            remote_origin: true,
            sensitive: false,
        });
        Ok(())
    }

    fn set_deferred_selection(
        &mut self,
        qh: &QueueHandle<Self>,
        offer: &DeferredOffer,
        inner: &Arc<Inner>,
    ) -> Result<(), String> {
        let manager = self
            .manager
            .as_ref()
            .ok_or("data-control manager missing")?;
        let device = self.device.as_ref().ok_or("data-control device missing")?;
        if let Some(old) = self.source.take() {
            old.destroy();
        }
        let fetcher = inner
            .fetcher
            .lock()
            .unwrap()
            .clone()
            .ok_or("clipboard fetcher is not installed")?;
        let data = LazySourceData::new(offer.clone(), fetcher);
        let mimes = data.mimes.clone();
        let source = manager.create_data_source(qh, data);
        for mime in mimes {
            source.offer(mime);
        }
        device.set_selection(Some(&source));
        self.source = Some(source);
        *inner.latest.lock().unwrap() = Some(ReadContent {
            content: ClipboardContent::default(),
            remote_origin: true,
            sensitive: false,
        });
        notify(inner);
        Ok(())
    }

    fn read_selection(&mut self, conn: &Connection, inner: &Arc<Inner>, deadline: Instant) {
        // One deadline covers text, HTML, image and files; no MIME gets a
        // fresh 2 s budget, so the event thread is bounded by READ_DEADLINE.
        self.read_at = None;
        let Some(offer) = self.current_offer.clone() else {
            *inner.latest.lock().unwrap() = Some(ReadContent::default());
            notify(inner);
            return;
        };
        let offer_id = offer.id().protocol_id();
        let mimes = self.offers.get(&offer_id).cloned().unwrap_or_default();
        let has = |mime: &str| mimes.iter().any(|value| value == mime);
        if has(ORIGIN_MIME) {
            *inner.latest.lock().unwrap() = Some(ReadContent {
                content: ClipboardContent::default(),
                remote_origin: true,
                sensitive: false,
            });
            notify(inner);
            return;
        }
        if has("x-kde-passwordManagerHint")
            && let Ok(hint) = receive(conn, &offer, "x-kde-passwordManagerHint", deadline)
            && hint == b"secret"
        {
            *inner.latest.lock().unwrap() = Some(ReadContent {
                content: ClipboardContent::default(),
                remote_origin: false,
                sensitive: true,
            });
            notify(inner);
            return;
        }
        let mut content = ClipboardContent::default();
        let text_mime = [
            "text/plain;charset=utf-8",
            "text/plain",
            "UTF8_STRING",
            "STRING",
            "TEXT",
        ]
        .into_iter()
        .find(|mime| has(mime));
        if let Some(mime) = text_mime
            && Instant::now() < deadline
            && let Ok(bytes) = receive(conn, &offer, mime, deadline)
        {
            content.text = Some(String::from_utf8_lossy(&bytes).into_owned());
        }
        if has("text/html")
            && Instant::now() < deadline
            && let Ok(bytes) = receive(conn, &offer, "text/html", deadline)
        {
            content.html = Some(String::from_utf8_lossy(&bytes).into_owned());
        }
        if has("image/png")
            && Instant::now() < deadline
            && let Ok(bytes) = receive(conn, &offer, "image/png", deadline)
        {
            content.image = Some(bytes);
        }
        if Instant::now() < deadline {
            if has("text/uri-list")
                && let Ok(bytes) = receive(conn, &offer, "text/uri-list", deadline)
            {
                content.files = parse_uri_list(&bytes);
            } else if has("x-special/gnome-copied-files")
                && let Ok(bytes) = receive(conn, &offer, "x-special/gnome-copied-files", deadline)
            {
                content.files = parse_gnome_files(&bytes);
            }
        }
        *inner.latest.lock().unwrap() = Some(ReadContent {
            content,
            remote_origin: false,
            sensitive: false,
        });
        notify(inner);
    }

    fn clear_offers(&mut self) {
        if let Some(offer) = self.current_offer.take() {
            offer.destroy();
        }
        let pending = std::mem::take(&mut self.pending_offers);
        for (_, offer) in pending {
            offer.destroy();
        }
        self.offers.clear();
    }
}

fn notify(inner: &Arc<Inner>) {
    if let Some(events) = inner.events.lock().unwrap().as_ref() {
        let _ = events.send(BackendEvent::Changed);
    }
}

fn receive(
    conn: &Connection,
    offer: &ExtDataControlOfferV1,
    mime: &str,
    deadline: Instant,
) -> Result<Vec<u8>, String> {
    if Instant::now() >= deadline {
        return Err("clipboard read deadline reached".into());
    }
    let mut fds = [0i32; 2];
    if unsafe { libc::pipe2(fds.as_mut_ptr(), libc::O_CLOEXEC) } != 0 {
        return Err(format!("pipe: {}", std::io::Error::last_os_error()));
    }
    let read_fd = unsafe { OwnedFd::from_raw_fd(fds[0]) };
    let write_fd = unsafe { OwnedFd::from_raw_fd(fds[1]) };
    offer.receive(mime.to_owned(), write_fd.as_fd());
    conn.flush().map_err(|error| error.to_string())?;
    // Closing the write end lets the source finish and close its end, which is
    // what produces EOF for our read loop.
    drop(write_fd);
    let raw = read_fd.as_raw_fd();
    let flags = unsafe { libc::fcntl(raw, libc::F_GETFL) };
    if flags >= 0 {
        unsafe {
            libc::fcntl(raw, libc::F_SETFL, flags | libc::O_NONBLOCK);
        }
    }
    let mut bytes = Vec::new();
    let mut buffer = [0u8; 8192];
    loop {
        let now = Instant::now();
        if now >= deadline {
            return Err("clipboard read timed out".into());
        }
        let remaining = deadline.saturating_duration_since(now);
        let timeout = remaining.as_millis().clamp(1, 100) as i32;
        let mut pollfd = libc::pollfd {
            fd: raw,
            events: libc::POLLIN,
            revents: 0,
        };
        let ready = unsafe { libc::poll(&mut pollfd, 1, timeout) };
        if ready < 0 {
            let error = std::io::Error::last_os_error();
            if error.kind() == std::io::ErrorKind::Interrupted {
                continue;
            }
            return Err(format!("poll: {error}"));
        }
        if ready == 0 {
            continue;
        }
        let count = unsafe { libc::read(raw, buffer.as_mut_ptr().cast(), buffer.len()) };
        match count {
            0 => break,
            value if value > 0 => {
                bytes.extend_from_slice(&buffer[..value as usize]);
                if bytes.len() > MAX_READ_BYTES {
                    return Err("clipboard payload exceeds 64 MiB".into());
                }
            }
            _ => {
                let error = std::io::Error::last_os_error();
                if matches!(
                    error.kind(),
                    std::io::ErrorKind::Interrupted | std::io::ErrorKind::WouldBlock
                ) {
                    continue;
                }
                return Err(format!("read: {error}"));
            }
        }
    }
    Ok(bytes)
}

fn write_all_deadline(fd: RawFd, bytes: &[u8], deadline: Instant) -> Result<(), String> {
    let flags = unsafe { libc::fcntl(fd, libc::F_GETFL) };
    if flags >= 0 {
        unsafe {
            libc::fcntl(fd, libc::F_SETFL, flags | libc::O_NONBLOCK);
        }
    }
    let mut offset = 0;
    while offset < bytes.len() {
        let now = Instant::now();
        if now >= deadline {
            return Err("clipboard write timed out".into());
        }
        let remaining = deadline.saturating_duration_since(now);
        let timeout = remaining.as_millis().clamp(1, 100) as i32;
        let mut pollfd = libc::pollfd {
            fd,
            events: libc::POLLOUT,
            revents: 0,
        };
        let ready = unsafe { libc::poll(&mut pollfd, 1, timeout) };
        if ready < 0 {
            let error = std::io::Error::last_os_error();
            if error.kind() == std::io::ErrorKind::Interrupted {
                continue;
            }
            return Err(format!("poll: {error}"));
        }
        if ready == 0 {
            continue;
        }
        if pollfd.revents & (libc::POLLERR | libc::POLLHUP | libc::POLLNVAL) != 0 {
            return Err("clipboard destination closed".into());
        }
        let written =
            unsafe { libc::write(fd, bytes[offset..].as_ptr().cast(), bytes.len() - offset) };
        if written > 0 {
            offset += written as usize;
        } else {
            let error = std::io::Error::last_os_error();
            if matches!(
                error.kind(),
                std::io::ErrorKind::Interrupted | std::io::ErrorKind::WouldBlock
            ) {
                continue;
            }
            return Err(format!("write: {error}"));
        }
    }
    Ok(())
}

/// Name the process holding the read end of a clipboard transfer pipe, i.e.
/// the application that asked for the data. Only same-user processes are
/// visible; this is a diagnostic and never blocks the transfer.
fn pipe_reader(fd: RawFd) -> (String, String) {
    let unknown = || ("?".to_owned(), "unknown".to_owned());
    let Ok(target) = std::fs::read_link(format!("/proc/self/fd/{fd}")) else {
        return unknown();
    };
    let own = std::process::id().to_string();
    let Ok(processes) = std::fs::read_dir("/proc") else {
        return unknown();
    };
    for process in processes.flatten() {
        let pid = process.file_name().to_string_lossy().into_owned();
        if pid == own || !pid.bytes().all(|byte| byte.is_ascii_digit()) {
            continue;
        }
        let Ok(fds) = std::fs::read_dir(process.path().join("fd")) else {
            continue;
        };
        if fds
            .flatten()
            .any(|entry| std::fs::read_link(entry.path()).is_ok_and(|link| link == target))
        {
            let name = std::fs::read_to_string(process.path().join("comm")).unwrap_or_default();
            return (pid, name.trim().to_owned());
        }
    }
    unknown()
}

fn parse_uri_list(bytes: &[u8]) -> Vec<std::path::PathBuf> {
    String::from_utf8_lossy(bytes)
        .lines()
        .filter(|line| !line.trim().is_empty() && !line.starts_with('#'))
        .filter_map(|line| url::Url::parse(line.trim()).ok())
        .filter_map(|url| url.to_file_path().ok())
        .collect()
}

fn parse_gnome_files(bytes: &[u8]) -> Vec<std::path::PathBuf> {
    String::from_utf8_lossy(bytes)
        .lines()
        .filter(|line| !line.trim().is_empty() && !line.eq_ignore_ascii_case("copy"))
        .filter_map(|line| url::Url::parse(line.trim()).ok())
        .filter_map(|url| url.to_file_path().ok())
        .collect()
}

struct SourceData {
    mimes: HashMap<String, Vec<u8>>,
}

impl SourceData {
    fn new(content: &ClipboardContent) -> Self {
        let mut mimes = HashMap::new();
        if let Some(text) = &content.text {
            let bytes = text.as_bytes().to_vec();
            for mime in [
                "text/plain;charset=utf-8",
                "text/plain",
                "UTF8_STRING",
                "STRING",
                "TEXT",
            ] {
                mimes.insert(mime.to_owned(), bytes.clone());
            }
        }
        if let Some(html) = &content.html {
            mimes.insert("text/html".into(), html.as_bytes().to_vec());
        }
        if let Some(image) = &content.image {
            mimes.insert("image/png".into(), image.clone());
        }
        if !content.files.is_empty() {
            let uris = content
                .files
                .iter()
                .filter_map(|path| url::Url::from_file_path(path).ok())
                .map(|url| url.to_string())
                .collect::<Vec<_>>()
                .join("\r\n");
            let uri_list = format!("{uris}\r\n");
            mimes.insert("text/uri-list".into(), uri_list.clone().into_bytes());
            let gnome = format!("copy\n{uris}");
            mimes.insert("x-special/gnome-copied-files".into(), gnome.into_bytes());
        }
        mimes.insert(
            ORIGIN_MIME.into(),
            blob_meta(content.fingerprint().as_bytes())
                .hash
                .into_bytes(),
        );
        Self { mimes }
    }
}

/// A Wayland data source whose payload is fetched from the peer when an
/// application actually reads the selection. `SourceData` is built once after
/// the fetch and reused for every requested MIME type.
#[derive(Clone)]
struct LazySourceData {
    offer: DeferredOffer,
    fetcher: Fetcher,
    payload: Arc<Mutex<Option<Arc<SourceData>>>>,
    mimes: Vec<String>,
}

impl LazySourceData {
    fn new(offer: DeferredOffer, fetcher: Fetcher) -> Self {
        let mut mimes = Vec::new();
        if offer.text.is_some() {
            for mime in [
                "text/plain;charset=utf-8",
                "text/plain",
                "UTF8_STRING",
                "STRING",
                "TEXT",
            ] {
                mimes.push(mime.to_owned());
            }
        }
        if offer.html.is_some() {
            mimes.push("text/html".into());
        }
        if offer.image.is_some() {
            mimes.push("image/png".into());
        }
        if offer.files.is_some() {
            mimes.push("text/uri-list".into());
            mimes.push("x-special/gnome-copied-files".into());
        }
        mimes.push(ORIGIN_MIME.into());
        // Clipboard managers (Klipper) read every new selection, which would
        // pull the payload right after the copy. They skip secret entries.
        mimes.push(PASSWORD_HINT_MIME.into());
        Self {
            offer,
            fetcher,
            payload: Arc::new(Mutex::new(None)),
            mimes,
        }
    }

    fn bytes_for(&self, mime: &str) -> Option<Vec<u8>> {
        if mime == ORIGIN_MIME {
            return Some(blob_meta(self.offer.id.as_bytes()).hash.into_bytes());
        }
        if mime == PASSWORD_HINT_MIME {
            return Some(b"secret".to_vec());
        }
        self.payload()?.mimes.get(mime).cloned()
    }

    fn payload(&self) -> Option<Arc<SourceData>> {
        // Hold the lock across the fetch: an application usually asks for
        // several MIME types at once, and they must share one transfer.
        let mut slot = self.payload.lock().unwrap();
        if let Some(payload) = slot.clone() {
            return Some(payload);
        }
        let timeout = if self.offer.has_files() {
            DEFERRED_FILE_TIMEOUT
        } else {
            DEFERRED_TIMEOUT
        };
        let content = match self.fetcher.fetch(&self.offer.id, timeout) {
            Ok(content) => content,
            Err(error) => {
                eprintln!("Sean Toolbox clipboard (Wayland): fetch failed: {error}");
                return None;
            }
        };
        let payload = Arc::new(SourceData::new(&content));
        *slot = Some(payload.clone());
        Some(payload)
    }
}

fn run(inner: Arc<Inner>) -> Result<(), String> {
    let conn = Connection::connect_to_env().map_err(|error| error.to_string())?;
    let mut queue = conn.new_event_queue();
    let qh = queue.handle();
    conn.display().get_registry(&qh, ());
    let mut state = State::default();
    queue
        .roundtrip(&mut state)
        .map_err(|error| error.to_string())?;
    if state.manager.is_none() || state.seat.is_none() {
        return Err("compositor does not expose ext_data_control_manager_v1".into());
    }
    let manager = state.manager.clone().expect("checked above");
    let seat = state.seat.clone().expect("checked above");
    state.device = Some(manager.get_data_device(&seat, &qh, ()));
    queue
        .roundtrip(&mut state)
        .map_err(|error| error.to_string())?;
    loop {
        if inner.stop.load(Ordering::Acquire) {
            state.clear_offers();
            return Ok(());
        }
        if state.finished {
            state.clear_offers();
            return Err("data-control device finished".into());
        }
        let pending = inner.pending_write.lock().unwrap().take();
        if let Some(content) = pending {
            state.set_selection(&qh, &content, &inner)?;
        }
        let deferred = inner.pending_defer.lock().unwrap().take();
        if let Some(offer) = deferred
            && let Err(error) = state.set_deferred_selection(&qh, &offer, &inner)
        {
            eprintln!("Sean Toolbox clipboard (Wayland): deferred selection: {error}");
        }
        conn.flush().map_err(|error| error.to_string())?;
        if let Some(guard) = conn.prepare_read() {
            let mut pollfd = libc::pollfd {
                fd: guard.connection_fd().as_raw_fd(),
                events: libc::POLLIN,
                revents: 0,
            };
            let ready = unsafe { libc::poll(&mut pollfd, 1, 20) };
            if ready > 0 && pollfd.revents & libc::POLLIN != 0 {
                let _ = guard.read();
            }
        }
        queue
            .dispatch_pending(&mut state)
            .map_err(|error| error.to_string())?;
        if let Some(at) = state.read_at
            && Instant::now() >= at
        {
            state.read_selection(&conn, &inner, Instant::now() + READ_DEADLINE);
        }
    }
}

impl Dispatch<WlRegistry, ()> for State {
    fn event(
        state: &mut Self,
        registry: &WlRegistry,
        event: wl_registry::Event,
        _: &(),
        _: &Connection,
        qh: &QueueHandle<Self>,
    ) {
        if let wl_registry::Event::Global {
            name,
            interface,
            version,
        } = event
        {
            match interface.as_str() {
                "ext_data_control_manager_v1" => {
                    state.manager = Some(registry.bind::<ExtDataControlManagerV1, _, _>(
                        name,
                        version.min(1),
                        qh,
                        (),
                    ));
                }
                "wl_seat" => {
                    state.seat = Some(registry.bind::<WlSeat, _, _>(name, version.min(9), qh, ()));
                }
                _ => {}
            }
        }
    }
}

impl Dispatch<WlSeat, ()> for State {
    fn event(
        _: &mut Self,
        _: &WlSeat,
        _: wl_seat::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
    }
}

impl Dispatch<ExtDataControlManagerV1, ()> for State {
    fn event(
        _: &mut Self,
        _: &ExtDataControlManagerV1,
        _: <ExtDataControlManagerV1 as Proxy>::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
    }
}

impl Dispatch<ExtDataControlDeviceV1, ()> for State {
    fn event(
        state: &mut Self,
        _: &ExtDataControlDeviceV1,
        event: ext_data_control_device_v1::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        match event {
            ext_data_control_device_v1::Event::DataOffer { id } => {
                let id_value = id.id().protocol_id();
                state.offers.entry(id_value).or_default();
                state.pending_offers.insert(id_value, id);
            }
            ext_data_control_device_v1::Event::Selection { id } => {
                let new_id = id.as_ref().map(|offer| offer.id().protocol_id());
                let old_pending = std::mem::take(&mut state.pending_offers);
                for (offer_id, offer) in old_pending {
                    if Some(offer_id) != new_id {
                        offer.destroy();
                        state.offers.remove(&offer_id);
                    }
                }
                if let Some(offer) = id {
                    let offer_id = offer.id().protocol_id();
                    if let Some(old) = state.current_offer.take()
                        && old.id().protocol_id() != offer_id
                    {
                        state.offers.remove(&old.id().protocol_id());
                        old.destroy();
                    }
                    state.current_offer = Some(offer);
                } else {
                    if let Some(old) = state.current_offer.take() {
                        state.offers.remove(&old.id().protocol_id());
                        old.destroy();
                    }
                    state.offers.clear();
                }
                state.read_at = Some(Instant::now() + Duration::from_millis(50));
            }
            ext_data_control_device_v1::Event::PrimarySelection { id: Some(offer) } => {
                state.offers.remove(&offer.id().protocol_id());
                offer.destroy();
            }
            ext_data_control_device_v1::Event::Finished => {
                state.finished = true;
            }
            _ => {}
        }
    }

    event_created_child!(State, ExtDataControlDeviceV1, [
        ext_data_control_device_v1::EVT_DATA_OFFER_OPCODE => (ExtDataControlOfferV1, ()),
    ]);
}

impl Dispatch<ExtDataControlOfferV1, ()> for State {
    fn event(
        state: &mut Self,
        offer: &ExtDataControlOfferV1,
        event: ext_data_control_offer_v1::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        if let ext_data_control_offer_v1::Event::Offer { mime_type } = event {
            state
                .offers
                .entry(offer.id().protocol_id())
                .or_default()
                .push(mime_type);
        }
    }
}

impl Dispatch<ExtDataControlSourceV1, SourceData> for State {
    fn event(
        state: &mut Self,
        source: &ExtDataControlSourceV1,
        event: ext_data_control_source_v1::Event,
        data: &SourceData,
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        match event {
            ext_data_control_source_v1::Event::Send { mime_type, fd } => {
                if let Some(bytes) = data.mimes.get(&mime_type).cloned() {
                    // Never write a large payload on the Wayland event thread.
                    // The helper is non-blocking and gives up after 5 s so a
                    // reader that never drains the pipe cannot leak a thread.
                    thread::spawn(move || {
                        let _ = write_all_deadline(
                            fd.as_raw_fd(),
                            &bytes,
                            Instant::now() + Duration::from_secs(5),
                        );
                    });
                }
            }
            ext_data_control_source_v1::Event::Cancelled => {
                let id = source.id().protocol_id();
                if state
                    .source
                    .as_ref()
                    .is_some_and(|current| current.id().protocol_id() == id)
                {
                    state.source = None;
                }
                source.destroy();
            }
            _ => {}
        }
    }
}

impl Dispatch<ExtDataControlSourceV1, LazySourceData> for State {
    fn event(
        state: &mut Self,
        source: &ExtDataControlSourceV1,
        event: ext_data_control_source_v1::Event,
        data: &LazySourceData,
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        match event {
            ext_data_control_source_v1::Event::Send { mime_type, fd } => {
                // The first paste fetches the payload from the peer, which can
                // take seconds for files; never block the Wayland thread.
                let data = data.clone();
                thread::spawn(move || {
                    if mime_type != ORIGIN_MIME && mime_type != PASSWORD_HINT_MIME {
                        let (pid, name) = pipe_reader(fd.as_raw_fd());
                        if BACKGROUND_READERS.contains(&name.as_str()) {
                            // Closing the pipe hands the monitor an empty
                            // read; the entry stays deferred for the paste.
                            eprintln!(
                                "Sean Toolbox clipboard (Wayland): ignored background request for {mime_type} from pid {pid} {name}"
                            );
                            return;
                        }
                        eprintln!(
                            "Sean Toolbox clipboard (Wayland): {mime_type} for pid {pid} {name}"
                        );
                    }
                    if let Some(bytes) = data.bytes_for(&mime_type) {
                        let _ = write_all_deadline(
                            fd.as_raw_fd(),
                            &bytes,
                            Instant::now() + Duration::from_secs(5),
                        );
                    }
                });
            }
            ext_data_control_source_v1::Event::Cancelled => {
                let id = source.id().protocol_id();
                if state
                    .source
                    .as_ref()
                    .is_some_and(|current| current.id().protocol_id() == id)
                {
                    state.source = None;
                }
                source.destroy();
            }
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    #[ignore = "reads the real Wayland clipboard; run manually in a KDE session"]
    fn reads_the_current_selection_without_changing_it() {
        let mut backend = LinuxBackend::new();
        let (sender, receiver) = std::sync::mpsc::channel();
        backend.watch(sender).unwrap();
        let deadline = Instant::now() + Duration::from_secs(3);
        while Instant::now() < deadline {
            if receiver.recv_timeout(Duration::from_millis(200)).is_ok() {
                break;
            }
        }
        let read = backend.read().unwrap();
        eprintln!(
            "wayland clipboard: kind={} remote={} sensitive={}",
            read.content.kind(),
            read.remote_origin,
            read.sensitive
        );
    }

    #[test]
    fn uri_lists_decode_spaces_and_chinese() {
        let files =
            parse_uri_list(b"# comment\r\nfile:///tmp/a%20b/\xe4\xb8\xad\xe6\x96\x87.txt\r\n");
        assert_eq!(files.len(), 1);
        assert!(files[0].ends_with("a b/中文.txt"));
    }

    #[test]
    fn gnome_file_lists_skip_the_copy_header() {
        let files = parse_gnome_files(b"copy\nfile:///tmp/x%20y.txt");
        assert_eq!(files, vec![std::path::PathBuf::from("/tmp/x y.txt")]);
    }

    #[test]
    fn source_offers_all_rich_text_mimes_and_origin() {
        let content = ClipboardContent {
            text: Some("hello".into()),
            html: Some("<b>hello</b>".into()),
            image: Some(vec![1, 2, 3]),
            files: vec![PathBuf::from("/tmp/中文.txt")],
        };
        let data = SourceData::new(&content);
        for mime in [
            "text/plain;charset=utf-8",
            "text/plain",
            "UTF8_STRING",
            "STRING",
            "TEXT",
            "text/html",
            "image/png",
            "text/uri-list",
            "x-special/gnome-copied-files",
            ORIGIN_MIME,
        ] {
            assert!(data.mimes.contains_key(mime), "{mime}");
        }
    }

    #[test]
    fn nonblocking_write_gives_up_without_a_reader() {
        let mut fds = [0i32; 2];
        assert_eq!(unsafe { libc::pipe2(fds.as_mut_ptr(), libc::O_CLOEXEC) }, 0);
        let read_fd = unsafe { OwnedFd::from_raw_fd(fds[0]) };
        let write_fd = unsafe { OwnedFd::from_raw_fd(fds[1]) };
        let data = vec![0u8; 1024 * 1024];
        let result = write_all_deadline(
            write_fd.as_raw_fd(),
            &data,
            Instant::now() + Duration::from_millis(100),
        );
        assert!(result.is_err());
        drop(write_fd);
        drop(read_fd);
    }
}
