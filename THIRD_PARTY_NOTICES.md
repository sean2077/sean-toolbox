# Third-party notices

Sean Toolbox's original source is MIT-licensed. The components below retain
their own licenses and attribution notices.

## Recentry

Source: <https://github.com/sean2077/recentry>, commit
`7eb5b8ad2575fa58b3cdffc34a40381dd3311201`.

The project provider, identity/ranking/opening policy, contract tests and local IPC
transport are adapted from Recentry. Its full original MIT notice is retained in
`crates/toolbox-projects/LICENSE.recentry` and `crates/toolbox-ipc/LICENSE.recentry`.
Sean Toolbox changes include its UI wrapper, configuration protocol, bounded IPC
operations and additional validation. The original tests are retained alongside
new application-specific tests.

## WinGuake

Source: <https://github.com/sean2077/winguake> (MIT, Copyright 2025 sean2077).
The application-window behavior and old INI configuration format informed the
Rust implementation. The INI importer was adapted from commit
`eaffe0312b1f266a27f6878545389eac4737e4b2`; the original notice is retained in
`licenses/WinGuake-MIT.txt`.

## Windows Terminal Quake

Source: <https://github.com/flyingpie/windows-terminal-quake>, commit
`5a30af25040de90dd833388036a15e93e3ead527` (MIT).

The per-application Quake mode, attached-window interaction, relative geometry,
and monitor choices were informed by this project. Sean Toolbox implements them
independently in its Rust desktop backends, using display work areas and native
Windows foreground notifications.

## Slint

Source: <https://github.com/slint-ui/slint/tree/v1.18.1>.

Slint 1.18.1 is used under the **Slint Royalty-free Desktop, Mobile, and Web
Applications License, version 2.0** for this desktop application. The license is
retained in `licenses/Slint-Royalty-free-2.0.md`. The application provides the
`AboutSlint` widget on its top-level Diagnostics & About page, and the public
README displays the official Slint attribution badge.

## Other Rust dependencies

`vendor/global-hotkey` preserves global-hotkey 0.9.0 and its original MIT / Apache
notices. A small X11 unregister fix flushes the connection so a released shortcut
is immediately available to other applications. See its `PATCH.md` for the source
revision and the native integration regression test. Windows and macOS code is
unchanged by that patch.

Exact dependency versions are recorded in `Cargo.lock`. `scripts/collect_licenses.py`
collects the resolved Cargo packages' license notices, package versions and source
links into `target/THIRD_PARTY_LICENSES.txt`; release packaging includes that file.
The inventory includes resolved build-time and other-target dependencies and
retains their upstream notices. Major components include winit, global-hotkey, tray-icon, x11rb, windows-sys,
rusqlite/SQLite, serde and rfd.
