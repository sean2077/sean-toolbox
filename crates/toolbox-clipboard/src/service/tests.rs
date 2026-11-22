use super::*;
use crate::backend::FakeBackend;
use crate::notify::NoopNotifier;
use std::fs;

fn free_port() -> u16 {
    TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port()
}

fn wait_until(timeout: Duration, mut check: impl FnMut() -> bool) -> bool {
    let deadline = Instant::now() + timeout;
    while Instant::now() < deadline {
        if check() {
            return true;
        }
        thread::sleep(Duration::from_millis(20));
    }
    false
}

fn text(value: &str) -> ClipboardContent {
    ClipboardContent {
        text: Some(value.into()),
        ..Default::default()
    }
}

/// Two services joined over loopback, each with its own fake clipboard.
struct Pair {
    _dir: tempfile::TempDir,
    listen: Handle,
    connect: Handle,
    listen_clip: FakeBackend,
    connect_clip: FakeBackend,
    listen_cache: PathBuf,
    connect_cache: PathBuf,
}

impl Pair {
    fn start() -> Self {
        Self::start_with(|_| {})
    }

    fn start_with(adjust: impl Fn(&mut Settings)) -> Self {
        let dir = tempfile::tempdir().unwrap();
        let key = dir.path().join("clipboard.key");
        crypto::write_key(&key, &crypto::generate_key()).unwrap();
        let address = format!("127.0.0.1:{}", free_port());
        let listen_cache = dir.path().join("listen-cache");
        let connect_cache = dir.path().join("connect-cache");
        let mut listen_settings = Settings::with_defaults(
            Role::Listen,
            address.clone(),
            "",
            key.clone(),
            listen_cache.clone(),
            "listen",
        );
        let mut connect_settings = Settings::with_defaults(
            Role::Connect,
            "",
            address,
            key,
            connect_cache.clone(),
            "connect",
        );
        adjust(&mut listen_settings);
        adjust(&mut connect_settings);
        let listen_clip = FakeBackend::new();
        let connect_clip = FakeBackend::new();
        let listen = start_with_notifier(
            listen_settings,
            Box::new(listen_clip.clone()),
            Arc::new(NoopNotifier),
        )
        .unwrap();
        let connect = start_with_notifier(
            connect_settings,
            Box::new(connect_clip.clone()),
            Arc::new(NoopNotifier),
        )
        .unwrap();
        let pair = Self {
            _dir: dir,
            listen,
            connect,
            listen_clip,
            connect_clip,
            listen_cache,
            connect_cache,
        };
        assert!(
            wait_until(Duration::from_secs(10), || pair.listen.status().connected
                && pair.connect.status().connected),
            "peers did not connect: {:?} / {:?}",
            pair.listen.status(),
            pair.connect.status()
        );
        pair
    }
}

#[test]
fn text_html_and_image_are_pushed_in_both_directions() {
    let pair = Pair::start();
    let rich = ClipboardContent {
        text: Some("飞书 消息".into()),
        html: Some("<b>飞书</b> 消息".into()),
        image: Some(vec![7u8; 200 * 1024]),
        files: Vec::new(),
    };
    pair.listen_clip.set_local(rich.clone());
    assert!(wait_until(Duration::from_secs(5), || pair
        .connect_clip
        .writes()
        .contains(&rich)));

    pair.connect_clip.set_local(text("reply"));
    assert!(wait_until(Duration::from_secs(5), || pair
        .listen_clip
        .writes()
        .contains(&text("reply"))));
}

#[test]
fn a_received_item_is_not_echoed_and_idle_stays_silent() {
    let pair = Pair::start();
    pair.listen_clip.set_local(text("once"));
    assert!(wait_until(Duration::from_secs(5), || pair
        .connect
        .status()
        .received_count
        == 1));
    thread::sleep(Duration::from_secs(2));
    assert_eq!(pair.listen.status().sent_count, 1);
    assert_eq!(
        pair.connect.status().sent_count,
        0,
        "the write must not echo back"
    );
    assert_eq!(pair.listen.status().received_count, 0);
}

#[test]
fn the_same_text_is_sent_again_after_a_remote_copy() {
    let pair = Pair::start();
    pair.listen_clip.set_local(text("A"));
    assert!(wait_until(Duration::from_secs(5), || pair
        .connect
        .status()
        .received_count
        == 1));
    pair.connect_clip.set_local(text("B"));
    assert!(wait_until(Duration::from_secs(5), || pair
        .listen
        .status()
        .received_count
        == 1));
    pair.listen_clip.set_local(text("A"));
    assert!(wait_until(Duration::from_secs(5), || pair
        .connect
        .status()
        .received_count
        == 2));
}

#[test]
fn oversized_text_is_skipped_and_the_session_stays_up() {
    let pair = Pair::start();
    pair.listen_clip
        .set_local(text(&"x".repeat(MAX_TEXT_BYTES as usize + 1)));
    thread::sleep(Duration::from_millis(600));
    assert_eq!(pair.connect.status().received_count, 0);
    pair.listen_clip.set_local(text("small"));
    assert!(wait_until(Duration::from_secs(5), || pair
        .connect_clip
        .writes()
        .contains(&text("small"))));
    assert!(pair.listen.status().connected);
}

fn sample_files(root: &Path) -> Vec<PathBuf> {
    let folder = root.join("资料");
    fs::create_dir_all(folder.join("sub")).unwrap();
    fs::write(folder.join("sub/a.txt"), b"alpha").unwrap();
    fs::write(folder.join("empty"), b"").unwrap();
    let single = root.join("b.bin");
    fs::write(&single, vec![3u8; 100 * 1024]).unwrap();
    vec![folder, single]
}

fn copy_files(pair: &Pair, files: Vec<PathBuf>) {
    pair.listen_clip.set_local(ClipboardContent {
        files,
        ..Default::default()
    });
    assert!(
        wait_until(Duration::from_secs(5), || pair
            .connect_clip
            .offered_id()
            .is_some()),
        "files were not announced"
    );
}

#[test]
fn files_are_only_announced_until_the_peer_pastes() {
    let pair = Pair::start();
    let source = tempfile::tempdir().unwrap();
    copy_files(&pair, sample_files(source.path()));
    thread::sleep(Duration::from_millis(500));
    assert!(
        !pair.connect_cache.exists() || fs::read_dir(&pair.connect_cache).unwrap().count() == 0,
        "nothing may be transferred before a paste"
    );
    assert_eq!(pair.connect.status().received_count, 0);

    let paths = pair
        .connect_clip
        .paste_files(Duration::from_secs(5))
        .unwrap();
    assert_eq!(paths.len(), 2);
    let named = |name: &str| {
        paths
            .iter()
            .find(|path| path.ends_with(name))
            .unwrap()
            .clone()
    };
    assert_eq!(fs::read(named("资料").join("sub/a.txt")).unwrap(), b"alpha");
    assert_eq!(fs::read(named("资料").join("empty")).unwrap(), b"");
    assert_eq!(fs::read(named("b.bin")).unwrap().len(), 100 * 1024);

    // A second paste is served from the cache.
    let again = pair
        .connect_clip
        .paste_files(Duration::from_millis(50))
        .unwrap();
    assert_eq!(again, paths);
    assert_eq!(pair.connect.status().received_count, 1);
}

#[test]
fn a_slow_paste_reports_progress_and_the_next_paste_succeeds() {
    let pair = Pair::start();
    let source = tempfile::tempdir().unwrap();
    copy_files(&pair, sample_files(source.path()));
    assert_eq!(
        pair.connect_clip.paste_files(Duration::ZERO),
        Err(FetchError::InProgress)
    );
    assert!(wait_until(Duration::from_secs(5), || pair
        .connect_clip
        .paste_files(Duration::from_millis(50))
        .is_ok()));
}

#[test]
fn a_file_changed_after_the_copy_fails_the_paste() {
    let pair = Pair::start();
    let source = tempfile::tempdir().unwrap();
    let files = sample_files(source.path());
    copy_files(&pair, files.clone());
    fs::write(&files[1], b"shorter").unwrap();
    let result = pair.connect_clip.paste_files(Duration::from_secs(5));
    assert!(matches!(result, Err(FetchError::Failed(_))), "{result:?}");
    assert!(pair.connect.status().connected);
}

#[test]
fn a_new_copy_replaces_announced_files() {
    let pair = Pair::start();
    let source = tempfile::tempdir().unwrap();
    copy_files(&pair, sample_files(source.path()));
    pair.listen_clip.set_local(text("newer"));
    assert!(wait_until(Duration::from_secs(5), || pair
        .connect_clip
        .writes()
        .contains(&text("newer"))));
    assert!(pair.connect_clip.offered_id().is_none());
}

#[test]
fn clearing_the_cache_refetches_on_the_next_paste() {
    let pair = Pair::start();
    let source = tempfile::tempdir().unwrap();
    copy_files(&pair, sample_files(source.path()));
    let paths = pair
        .connect_clip
        .paste_files(Duration::from_secs(5))
        .unwrap();
    assert!(wait_until(Duration::from_secs(2), || pair
        .connect
        .status()
        .cache_bytes
        > 0));
    pair.connect.clear_cache();
    assert!(wait_until(Duration::from_secs(2), || paths
        .iter()
        .all(|path| !path.exists())));
    assert_eq!(pair.connect.status().cache_bytes, 0);
    let again = pair
        .connect_clip
        .paste_files(Duration::from_secs(5))
        .unwrap();
    assert!(again.iter().all(|path| path.exists()));
    let _ = &pair.listen_cache;
}

#[test]
fn a_silent_peer_is_dropped_and_a_new_connection_takes_over() {
    let dir = tempfile::tempdir().unwrap();
    let key_path = dir.path().join("clipboard.key");
    let key = crypto::generate_key();
    crypto::write_key(&key_path, &key).unwrap();
    let address = format!("127.0.0.1:{}", free_port());
    let mut settings = Settings::with_defaults(
        Role::Listen,
        address.clone(),
        "",
        key_path.clone(),
        dir.path().join("cache"),
        "listen",
    );
    settings.silence_timeout = Duration::from_secs(1);
    let clip = FakeBackend::new();
    let listen =
        start_with_notifier(settings, Box::new(clip.clone()), Arc::new(NoopNotifier)).unwrap();

    // An authenticated client that never says Hello occupies the slot.
    let stream = TcpStream::connect(&address).unwrap();
    let (_silent, _session) = handshake(stream, true, &key).unwrap();
    thread::sleep(Duration::from_millis(300));
    assert!(!listen.status().connected);

    let peer_clip = FakeBackend::new();
    let connect = start_with_notifier(
        Settings::with_defaults(
            Role::Connect,
            "",
            address,
            key_path,
            dir.path().join("peer-cache"),
            "connect",
        ),
        Box::new(peer_clip.clone()),
        Arc::new(NoopNotifier),
    )
    .unwrap();
    assert!(wait_until(Duration::from_secs(10), || listen
        .status()
        .connected
        && connect.status().connected));
    peer_clip.set_local(text("after takeover"));
    assert!(wait_until(Duration::from_secs(5), || clip
        .writes()
        .contains(&text("after takeover"))));
}

#[test]
fn a_wrong_key_never_connects() {
    let dir = tempfile::tempdir().unwrap();
    let good = dir.path().join("good.key");
    let bad = dir.path().join("bad.key");
    crypto::write_key(&good, &crypto::generate_key()).unwrap();
    crypto::write_key(&bad, &crypto::generate_key()).unwrap();
    let address = format!("127.0.0.1:{}", free_port());
    let listen = start_with_notifier(
        Settings::with_defaults(
            Role::Listen,
            address.clone(),
            "",
            good,
            dir.path().join("a"),
            "listen",
        ),
        Box::new(FakeBackend::new()),
        Arc::new(NoopNotifier),
    )
    .unwrap();
    let connect = start_with_notifier(
        Settings::with_defaults(Role::Connect, "", address, bad, dir.path().join("b"), "c"),
        Box::new(FakeBackend::new()),
        Arc::new(NoopNotifier),
    )
    .unwrap();
    thread::sleep(Duration::from_secs(2));
    assert!(!listen.status().connected && !connect.status().connected);
    assert!(connect.status().last_error.is_some());
}

#[test]
fn manual_suspension_discards_both_directions_and_resume_does_not_replay() {
    let pair = Pair::start();
    pair.listen.set_manual_suspend(true);
    assert!(wait_until(Duration::from_secs(2), || {
        let status = pair.listen.status();
        status.suspended && status.suspended_by.as_deref() == Some("manual")
    }));

    pair.listen_clip.set_local(text("paused-local"));
    pair.connect_clip.set_local(text("paused-peer"));
    thread::sleep(Duration::from_millis(1500));
    assert!(
        !pair.connect_clip.writes().contains(&text("paused-local")),
        "a local copy must not be sent while paused"
    );
    assert!(
        !pair.listen_clip.writes().contains(&text("paused-peer")),
        "peer content must not be written while paused"
    );
    assert_eq!(pair.listen.status().sent_count, 0);
    assert_eq!(pair.listen.status().received_count, 0);

    pair.listen.set_manual_suspend(false);
    assert!(wait_until(Duration::from_secs(2), || !pair
        .listen
        .status()
        .suspended));
    thread::sleep(Duration::from_millis(800));
    assert!(
        !pair.connect_clip.writes().contains(&text("paused-local")),
        "resume must not replay a pre-pause local copy"
    );
    assert!(
        !pair.listen_clip.writes().contains(&text("paused-peer")),
        "resume must not replay pre-pause peer content"
    );

    pair.listen_clip.set_local(text("after-resume"));
    assert!(wait_until(Duration::from_secs(5), || pair
        .connect_clip
        .writes()
        .contains(&text("after-resume"))));
}

#[test]
fn a_detected_remote_session_suspends_until_the_recovery_delay_ends() {
    let pair = Pair::start_with(|settings| {
        settings.suspend.recover_delay = Duration::from_millis(150);
    });
    let _ = pair
        .listen
        .events
        .send(Event::RemoteSession(Some("uuremote-server".into())));
    assert!(wait_until(Duration::from_secs(2), || {
        let status = pair.listen.status();
        status.suspended && status.suspended_by.as_deref() == Some("uuremote-server")
    }));

    let _ = pair.listen.events.send(Event::RemoteSession(None));
    thread::sleep(Duration::from_millis(20));
    assert!(
        pair.listen.status().suspended,
        "the recovery delay must keep the bridge paused"
    );
    assert!(wait_until(Duration::from_secs(2), || !pair
        .listen
        .status()
        .suspended));
}

#[test]
fn manual_resume_does_not_override_an_active_remote_session() {
    let pair = Pair::start_with(|settings| {
        settings.suspend.recover_delay = Duration::from_millis(50);
    });
    let _ = pair
        .listen
        .events
        .send(Event::RemoteSession(Some("uuremote-server".into())));
    assert!(wait_until(Duration::from_secs(2), || pair
        .listen
        .status()
        .suspended));
    pair.listen.set_manual_suspend(true);
    assert!(wait_until(Duration::from_secs(2), || {
        pair.listen.status().suspended_by.as_deref() == Some("manual")
    }));
    pair.listen.set_manual_suspend(false);
    thread::sleep(Duration::from_millis(50));
    let status = pair.listen.status();
    assert!(status.suspended);
    assert_eq!(status.suspended_by.as_deref(), Some("uuremote-server"));
}

#[test]
fn manual_resume_after_the_session_ended_clears_the_pause() {
    let pair = Pair::start_with(|settings| {
        settings.suspend.recover_delay = Duration::from_millis(50);
    });
    let _ = pair
        .listen
        .events
        .send(Event::RemoteSession(Some("uuremote-server".into())));
    assert!(wait_until(Duration::from_secs(2), || pair
        .listen
        .status()
        .suspended));
    pair.listen.set_manual_suspend(true);
    let _ = pair.listen.events.send(Event::RemoteSession(None));
    thread::sleep(Duration::from_millis(100));
    assert!(pair.listen.status().suspended);
    assert_eq!(pair.listen.status().suspended_by.as_deref(), Some("manual"));

    pair.listen.set_manual_suspend(false);
    assert!(wait_until(Duration::from_secs(2), || !pair
        .listen
        .status()
        .suspended));
}

#[test]
fn files_over_the_limit_are_not_announced() {
    let pair = Pair::start_with(|settings| settings.max_transfer_bytes = 1024);
    let source = tempfile::tempdir().unwrap();
    pair.listen_clip.set_local(ClipboardContent {
        files: sample_files(source.path()),
        ..Default::default()
    });
    thread::sleep(Duration::from_millis(800));
    assert!(pair.connect_clip.offered_id().is_none());
    assert_eq!(pair.listen.status().sent_count, 0);
}
