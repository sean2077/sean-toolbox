# toolbox-clipboard

Cross-machine clipboard bridge for the two fixed Sean Toolbox machines.

The crate is UI-independent. `service` owns a Noise NNpsk0 TCP session and a
platform backend; `protocol` defines length-prefixed control messages and
32 KiB blob chunks; `sync` supplies the 200 ms debounce and echo guards;
`transfer` expands file selections, validates paths and stages received files.

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
