# Local patch: acknowledge X11 shortcut unregistration

This directory contains the published **global-hotkey 0.9.0** source, with its
original version and **Apache-2.0 OR MIT** license choice preserved. The original
`LICENSE-APACHE`, `LICENSE-MIT`, `LICENSE.spdx`, README, normalized Cargo manifest
and original manifest are included. Registry cache markers and the dependency's
lockfile are intentionally omitted.

## Provenance

- Published package: <https://crates.io/crates/global-hotkey/0.9.0>
- Upstream: <https://github.com/tauri-apps/global-hotkey>
- Source commit recorded by the published crate's `.cargo_vcs_info.json`:
  [`b4185ab2f37b62e4d1bd2b8410fcf93e338f55fc`](https://github.com/tauri-apps/global-hotkey/tree/b4185ab2f37b62e4d1bd2b8410fcf93e338f55fc)

## Reproduced problem

In an isolated Xvfb/Openbox session, Sean Toolbox successfully changed an
application shortcut from F8 to F9. F9 activated the application, and F8 stopped
dispatching toolbox actions. However, an independent X11 client still received
`BadAccess` when trying to grab unmodified F8: the old passive grab remained
owned by the host and prevented other applications from using that key.

The published X11 `unregister_hotkey` implementation queued `UngrabKey` requests
and discarded their errors. It returned success without a flush or an
acknowledging round trip. The event loop's `poll_for_event()` does not flush
x11rb's outgoing buffer, so the obsolete grabs could persist until an unrelated
subsequent request flushed that connection.

## Change

Only `src/platform_impl/x11/mod.rs::unregister_hotkey` is changed:

1. Check each `UngrabKey` request and propagate submission/server errors as
   `FailedToUnRegister`.
2. Explicitly flush the connection and propagate a flush failure before changing
   the in-memory hotkey table or returning success.

Windows/macOS implementations, public APIs, dependency versions and the event
loop's existing polling behavior are unchanged. This is a narrow source patch,
not a replacement hotkey implementation.

## Regression check and removal

Run `python3 scripts/smoke_host.py --binary target/release/sean-toolbox` after
building both toolbox binaries. The script uses a second libX11 connection to
grab and release F8 after the F8→F9 save, as well as testing real F9 activation.
Checking only the lack of an F8 action would miss the leaked server-side grab.

Keep this patch until an upstream release acknowledges unregistration and
propagates its errors. Then update the dependency, remove the root Cargo patch
and this directory, and rerun the same native regression test.
