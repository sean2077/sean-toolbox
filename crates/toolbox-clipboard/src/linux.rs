//! Linux Wayland clipboard backend using `ext_data_control_manager_v1`.
//!
//! KWin exposes the data-control protocol to regular clients, so the backend can
//! watch and replace the selection without stealing focus. The protocol runs on
//! its own thread; the service only sees a cached snapshot and a write queue.
//! A wake pipe interrupts the thread's `poll` when a write is queued.
//!
//! Text, HTML and images received from the peer are served from memory. Files
//! announced by the peer become a source whose file list is fetched from the
//! service when an application reads it (see [`FileSourceData`]).

use crate::backend::{Backend, BackendEvent, Fetcher, ORIGIN_MIME, PASTE_WAIT, ReadContent};
use crate::model::{ClipboardContent, blob_meta};
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
const FILE_MIMES: [&str; 2] = ["text/uri-list", "x-special/gnome-copied-files"];

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
    pending_files: Mutex<Option<(String, Fetcher)>>,
    events: Mutex<Option<Sender<BackendEvent>>>,
    stop: AtomicBool,
    wake: [OwnedFd; 2],
}

impl Inner {
    fn wake(&self) {
        let byte = 1u8;
        unsafe {
            libc::write(self.wake[1].as_raw_fd(), (&raw const byte).cast(), 1);
        }
    }
}

impl LinuxBackend {
    pub fn new() -> Self {
        let mut fds = [0i32; 2];
        let created =
            unsafe { libc::pipe2(fds.as_mut_ptr(), libc::O_CLOEXEC | libc::O_NONBLOCK) } == 0;
        assert!(created, "pipe2: {}", std::io::Error::last_os_error());
        let wake = unsafe { [OwnedFd::from_raw_fd(fds[0]), OwnedFd::from_raw_fd(fds[1])] };
        Self {
            inner: Arc::new(Inner {
                latest: Mutex::new(None),
                pending_write: Mutex::new(None),
                pending_files: Mutex::new(None),
                events: Mutex::new(None),
                stop: AtomicBool::new(false),
                wake,
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
        self.inner.wake();
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
        self.inner.wake();
        Ok(())
    }

    fn offer_files(&mut self, id: &str, fetcher: Fetcher) -> Result<(), String> {
        *self.inner.pending_files.lock().unwrap() = Some((id.to_owned(), fetcher));
        self.inner.wake();
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

    fn set_file_selection(
        &mut self,
        qh: &QueueHandle<Self>,
        id: String,
        fetcher: Fetcher,
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
        let source = manager.create_data_source(qh, FileSourceData { id, fetcher });
        for mime in FILE_MIMES
            .into_iter()
            .chain([ORIGIN_MIME, PASSWORD_HINT_MIME])
        {
            source.offer(mime.to_owned());
        }
        device.set_selection(Some(&source));
        self.source = Some(source);
        *inner.latest.lock().unwrap() = Some(ReadContent {
            content: ClipboardContent::default(),
            remote_origin: true,
            sensitive: false,
        });
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
        mimes.insert(
            ORIGIN_MIME.into(),
            blob_meta(content.fingerprint().as_bytes())
                .hash
                .into_bytes(),
        );
        Self { mimes }
    }
}

/// A source for files announced by the peer. Reading `text/uri-list` (what
/// Dolphin does on paste) asks the service for the staged files. The service
/// answers within [`PASTE_WAIT`] — before Qt's one-second pipe timeout — or
/// the read gets no data while the transfer continues.
///
/// The entry carries `x-kde-passwordManagerHint=secret` so Klipper, which reads
/// every new selection, does not request the file list and start the transfer
/// right after the copy.
#[derive(Clone)]
struct FileSourceData {
    id: String,
    fetcher: Fetcher,
}

impl FileSourceData {
    fn bytes_for(&self, mime: &str) -> Option<Vec<u8>> {
        match mime {
            ORIGIN_MIME => Some(blob_meta(self.id.as_bytes()).hash.into_bytes()),
            PASSWORD_HINT_MIME => Some(b"secret".to_vec()),
            "text/uri-list" | "x-special/gnome-copied-files" => {
                let paths = self.fetcher.fetch(&self.id, PASTE_WAIT).ok()?;
                let uris = uri_lines(&paths);
                Some(if mime == "text/uri-list" {
                    format!("{uris}\r\n").into_bytes()
                } else {
                    format!("copy\n{uris}").into_bytes()
                })
            }
            _ => None,
        }
    }
}

fn uri_lines(paths: &[std::path::PathBuf]) -> String {
    paths
        .iter()
        .filter_map(|path| url::Url::from_file_path(path).ok())
        .map(|url| url.to_string())
        .collect::<Vec<_>>()
        .join("\r\n")
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
        let files = inner.pending_files.lock().unwrap().take();
        if let Some((id, fetcher)) = files
            && let Err(error) = state.set_file_selection(&qh, id, fetcher, &inner)
        {
            eprintln!("Sean Toolbox clipboard (Wayland): file selection: {error}");
            if let Some(events) = inner.events.lock().unwrap().as_ref() {
                let _ = events.send(BackendEvent::WriteFailed);
            }
        }
        conn.flush().map_err(|error| error.to_string())?;
        if let Some(guard) = conn.prepare_read() {
            // Sleep until Wayland events, a queued write (wake pipe) or the
            // pending selection read is due.
            let timeout = state.read_at.map_or(-1, |at| {
                at.saturating_duration_since(Instant::now()).as_millis() as i32
            });
            let mut fds = [
                libc::pollfd {
                    fd: guard.connection_fd().as_raw_fd(),
                    events: libc::POLLIN,
                    revents: 0,
                },
                libc::pollfd {
                    fd: inner.wake[0].as_raw_fd(),
                    events: libc::POLLIN,
                    revents: 0,
                },
            ];
            let ready = unsafe { libc::poll(fds.as_mut_ptr(), 2, timeout) };
            if ready > 0 && fds[0].revents & libc::POLLIN != 0 {
                let _ = guard.read();
            }
            if fds[1].revents & libc::POLLIN != 0 {
                let mut drain = [0u8; 64];
                while unsafe {
                    libc::read(
                        inner.wake[0].as_raw_fd(),
                        drain.as_mut_ptr().cast(),
                        drain.len(),
                    )
                } > 0
                {}
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

impl Dispatch<ExtDataControlSourceV1, FileSourceData> for State {
    fn event(
        state: &mut Self,
        source: &ExtDataControlSourceV1,
        event: ext_data_control_source_v1::Event,
        data: &FileSourceData,
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        match event {
            ext_data_control_source_v1::Event::Send { mime_type, fd } => {
                // The file list may wait for the transfer; never block the
                // Wayland thread.
                let data = data.clone();
                thread::spawn(move || {
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
