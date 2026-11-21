//! Linux Wayland clipboard backend using `ext_data_control_manager_v1`.
//!
//! KWin exposes the data-control protocol to regular clients, so the backend can
//! watch and replace the selection without stealing focus. The protocol runs on
//! its own thread; the service only sees a cached snapshot and a write queue.

use crate::backend::{Backend, BackendEvent, ORIGIN_MIME, ReadContent};
use crate::model::{ClipboardContent, blob_meta};
use std::{
    collections::HashMap,
    fs::File,
    io::Write,
    os::fd::{AsFd, AsRawFd, FromRawFd, OwnedFd},
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
    events: Mutex<Option<Sender<BackendEvent>>>,
    stop: AtomicBool,
}

impl LinuxBackend {
    pub fn new() -> Self {
        Self {
            inner: Arc::new(Inner {
                latest: Mutex::new(None),
                pending_write: Mutex::new(None),
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

    fn read_selection(&mut self, conn: &Connection, inner: &Arc<Inner>, deadline: Instant) {
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
                    // Never write a large payload on the Wayland event thread:
                    // a slow reader would stop selection changes and Cancelled.
                    thread::spawn(move || {
                        let mut file = File::from(fd);
                        let _ = file.write_all(&bytes);
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

#[cfg(test)]
mod tests {
    use super::*;

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
}
