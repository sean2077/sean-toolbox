# toolbox-clipboard

Cross-machine clipboard bridge for the two fixed Sean Toolbox machines.

The crate is UI-independent. `service` owns a Noise NNpsk0 TCP session and a
platform backend; `protocol` defines length-prefixed control messages and
32 KiB blob chunks; `sync` supplies the 200 ms debounce and echo guards;
`transfer` expands file selections, validates paths and stages received files.

## Transfer timing

`ClipboardSyncConfig.mode` selects how a local copy reaches the peer:

- `on_paste` (default): the source sends `Defer` with only `BlobMeta`/`Manifest` metadata
  and keeps the snapshot in a bounded store (eight items). The peer installs a
  deferred clipboard entry and sends `Fetch` when an application pastes; the
  source answers with the same `Chunk`/`End` frames under the announced id.
- `eager`: `Offer` followed by chunks, as soon as the 200 ms debounce elapses.

Protocol version 2 adds `Defer`, `Fetch` and `FetchFailed`. A `Defer` whose id
is unknown or whose files changed after the copy is answered with
`FetchFailed`, and the paste shows no data.

On Windows the destination registers the offered formats with
`SetClipboardData(format, NULL)` and answers `WM_RENDERFORMAT` by fetching the
payload. Files are staged in the cache before `CF_HDROP` is returned. The
message-only window stays the clipboard owner, so the origin marker remains
present and the render does not echo back. `WM_RENDERALLFORMATS` is ignored:
an unrendered entry is dropped when the host exits instead of stalling exit.

On Wayland the destination installs an `ext_data_control_source_v1` whose
`send` requests fetch the payload on a worker thread; concurrent MIME requests
share one fetch. Deferred entries carry `x-kde-passwordManagerHint=secret`
(Wayland) and `CanIncludeInClipboardHistory=0`/`CanUploadToCloudClipboard=0`
(Windows) so Klipper and Win+V history do not pull them right after the copy.

## Content

| Kind | Wire payload | Linux Wayland | Windows |
| --- | --- | --- | --- |
| Text | UTF-8 | `text/plain;charset=utf-8` plus legacy text MIME types | `CF_UNICODETEXT` |
| HTML | UTF-8 fragment | `text/html` | `HTML Format` (CF_HTML) |
| Image | PNG | `image/png` | `PNG` plus `CF_DIB` |
| Files | manifest + chunks | `text/uri-list`, `x-special/gnome-copied-files` | `CF_HDROP` + `Preferred DropEffect=COPY` |

Limits: 16 MiB per text/HTML item, 64 MiB per image, and a configurable total
file budget capped at 1 GiB. The sender streams files; the receiver writes into
a temporary directory and promotes it only after the transfer completes.

## Echo prevention

The writer marks its clipboard content with `application/x-sean-toolbox-origin`
(Wayland) or `SeanToolbox.ClipOrigin` (Windows). A selection carrying the marker
is ignored. Content is also debounced for 200 ms and compared by a canonical
fingerprint against the last sent/received snapshot. There is no periodic
content send: an idle bridge sends nothing.

## Configuration and key

`toolbox-core` stores the `clipboard` section in the normal `config.json`.
The 32-byte pre-shared key is a separate file:

- Linux: `~/.config/sean-toolbox/clipboard.key` (0600)
- Windows: `%LOCALAPPDATA%\SeanToolbox\clipboard.key`

`sean-toolbox clipboard keygen` creates the file and never prints the key.
Never put the key or the Noise secret in `config.json` or a command line.

## Known limitations

- Sending is synchronous. During a 1 GiB transfer the sender does not process
  inbound frames; two simultaneous large sends can reach the 10 s write timeout.
  A new local copy does not cancel an in-flight send yet.
- A deferred paste blocks
  while the source reads the payload; a large file selection can exceed the
  target application's clipboard timeout. The source must still hold the
  snapshot (last eight items) and be online when the peer pastes. On Windows
  `WM_RENDERFORMAT` is only served when the process holding the clipboard owns
  the foreground window (or the foreground window is `ApplicationFrameHost`);
  background monitors such as NetEase UU Remote (`GameViewer.exe`) get no data
  and the format stays delayed. Wayland does not expose the active client, so
  Linux identifies the reader through the transfer pipe (`/proc/*/fd`) and
  refuses the processes in `BACKGROUND_READERS` (fcitx5's clipboard addon reads
  every selection even with `IgnorePasswordFromPasswordManager`). Klipper skips
  entries marked `x-kde-passwordManagerHint=secret`. Other Linux monitors can
  still pull a deferred item early; every request is logged with its reader.
- The disconnect detector uses a 45 s application silence timeout (three 15 s
  heartbeats); TCP keepalive is not set separately.
- Notifications use `org.freedesktop.Notifications` through zbus on Linux and
  `notify-rust` toasts on Windows. They cover limits and file transfer
  start/finish only, not every clipboard item.
- Schema 1 configs are accepted and migrated to schema 2; the UI always writes
  schema 2. A schema 1 file may now also carry a `clipboard` section, but schema 2
  is the canonical format.

## Testing

Unit tests cover framing, Noise handshakes, CF_HTML offsets, PNG/DIB conversion,
URI-list parsing, debounce/echo suppression, path safety, file staging and a
loopback service with two `FakeBackend` peers. The 60-second idle test is
ignored by default:

```sh
cargo test --locked -p toolbox-clipboard
cargo test --locked -p toolbox-clipboard idle_one_minute_has_zero_sends -- --ignored
```

The real Wayland read probe is also ignored and must only be run with a strict
external timeout. Do not use `wl-paste` to judge bridge health on
`wl-clipboard` 2.2.x: it lacks data-control support and times out when it cannot
gain focus.
