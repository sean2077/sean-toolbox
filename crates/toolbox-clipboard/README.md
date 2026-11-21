# toolbox-clipboard

Cross-machine clipboard bridge for the two fixed Sean Toolbox machines.

The crate is UI-independent. `service` runs the Noise NNpsk0 TCP connection and
a platform backend; `protocol` defines the messages; `frame` carries them with
32 KiB binary chunks; `sync` supplies the 200 ms debounce and echo guard;
`transfer` expands file selections, checks paths, stages received files and
cleans the receive cache.

## Behavior

- **Text, HTML and images are pushed** right after a local copy (`Offer`
  followed by `Chunk`/`End`). The peer writes them to its clipboard at once, so
  a paste is instant and works offline. Clipboard history tools may read them
  like any local copy.
- **Files are announced** (`FileOffer` with the manifest). Nothing is
  transferred until an application on the peer pastes: the backend's clipboard
  entry asks the service for the files, the service sends `Fetch`, and the
  sender streams the files. A paste waits at most `PASTE_WAIT` (800 ms) because
  Qt abandons a clipboard pipe after one second without data. When the files
  arrive in time the paste succeeds directly; otherwise it gets no data, a
  notification reports the transfer, a second notification says the files are
  ready, and the next paste is served from the cache.
- A new copy on either side replaces announced files and cancels their
  transfer. A file whose size changed after the copy fails the fetch.

## Threads

One worker thread owns all state and blocks on a single event channel (local
clipboard changes, peer frames, paste requests, a one-second timer). Each
connection has a reader thread doing blocking reads and a writer thread
draining a bounded frame queue; file streaming runs on its own thread. The read
timeout (45 s) is the dead-peer detector; both sides ping every 15 s. The
listening side accepts a new authenticated connection at any time and drops the
old one, so a peer waking from sleep is never locked out. `TCP_NODELAY` is set
and each frame is one write.

## Content

| Kind | Wire payload | Linux Wayland | Windows |
| --- | --- | --- | --- |
| Text | UTF-8 | `text/plain;charset=utf-8` plus legacy text MIME types | `CF_UNICODETEXT` |
| HTML | UTF-8 fragment | `text/html` | `HTML Format` (CF_HTML) |
| Image | PNG | `image/png` | `PNG` plus `CF_DIB` |
| Files | manifest, then chunks on `Fetch` | `text/uri-list`, `x-special/gnome-copied-files` | delayed-rendered `CF_HDROP` + `Preferred DropEffect=COPY` |

Limits: 16 MiB per text/HTML item and 64 MiB per image (an oversized part is
skipped with a notification); files up to `max_transfer_bytes`, at most 1 GiB.

Announced-file entries carry `x-kde-passwordManagerHint=secret` (Wayland) and
`CanIncludeInClipboardHistory=0`/`CanUploadToCloudClipboard=0` (Windows):
Klipper and Win+V history read every new entry, and reading the file list
would start the transfer at copy time.

## Receive cache

Files are staged in `$XDG_CACHE_HOME/sean-toolbox/clipboard` (Linux) or
`%LOCALAPPDATA%\sean-toolbox\clipboard` (Windows) and renamed into place when
complete. `transfer::cleanup_cache` runs at start-up, after each received item
and hourly: it removes interrupted `*.tmp` directories, items older than 24
hours and the oldest items while the total exceeds 2 GiB. The item on the
clipboard is kept unless it alone exceeds the cap; if it is removed, the next
paste fetches it again. `sean-toolbox clipboard clear-cache` (or the UI button)
removes everything except a running transfer. The sender keeps no copy.

## Echo prevention

The writer marks its clipboard content with `application/x-sean-toolbox-origin`
(Wayland) or `SeanToolbox.ClipOrigin` (Windows); a selection carrying the marker
is ignored. Content is also debounced for 200 ms and compared by fingerprint
against the last sent/received item. There is no periodic send: an idle bridge
only exchanges pings.

## Configuration and key

`toolbox-core` stores the `clipboard` section in the normal `config.json`. A
`mode` key written by earlier builds is accepted and dropped on save. The
32-byte pre-shared key is a separate file:

- Linux: `~/.config/sean-toolbox/clipboard.key` (0600)
- Windows: `%LOCALAPPDATA%\SeanToolbox\clipboard.key`

`sean-toolbox clipboard keygen` creates the file and never prints the key.
Never put the key in `config.json` or on a command line.

## Known limitations

- A large file paste needs two pastes (the first starts the transfer). Explorer
  and Dolphin get no progress bar; the notification and the UI status show it.
  Streaming files into the paste (OLE `FileContents` on Windows, FUSE on Linux,
  as RDP clients do) is not implemented.
- Notifications use `org.freedesktop.Notifications` through zbus on Linux and
  `notify-rust` toasts on Windows.

## Testing

Unit tests cover framing, Noise handshakes, CF_HTML offsets, PNG/DIB
conversion, URI-list parsing, debounce/echo suppression, path safety, file
staging, cache cleanup and a loopback service with two `FakeBackend` peers
(push, echo, announce-then-paste, slow paste, changed source, replacement,
silent peer takeover, wrong key, limits).

```sh
cargo test --locked -p toolbox-clipboard
```

The real Wayland read probe is ignored and must only be run with a strict
external timeout. Do not use `wl-paste` to judge bridge health on
`wl-clipboard` 2.2.x: it lacks data-control support and times out when it cannot
gain focus.
