# Third-party notices

Sean Toolbox's original source is MIT-licensed. Dependency licenses apply to
their respective components; this file does not relicense them.

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
Rust implementation. The INI importer was adapted from the prior uTools importer
at commit `eaffe0312b1f266a27f6878545389eac4737e4b2`; the original notice is retained
in `licenses/WinGuake-MIT.txt`. No AutoHotkey or Koffi runtime is included.

## Windows Terminal Quake

Source: <https://github.com/flyingpie/windows-terminal-quake>, commit
`5a30af25040de90dd833388036a15e93e3ead527` (MIT).

The per-application Quake mode, attached-window interaction, relative geometry,
and monitor choices were informed by this project. Sean Toolbox implements them
independently in its Rust desktop backends, using display work areas and native
Windows foreground notifications. No WTQ source, assets, .NET runtime, polling
focus tracker, or window animation implementation is included.

## Slint

Source: <https://github.com/slint-ui/slint/tree/v1.18.1>.

Slint 1.18.1 is used under the **Slint Royalty-free Desktop, Mobile, and Web
Applications License, version 2.0** for this desktop application. The license is
retained in `licenses/Slint-Royalty-free-2.0.md`. The application provides the
`AboutSlint` widget on its top-level Diagnostics & About page, and the public
README displays the official Slint attribution badge.

The application's MIT license does not override Slint's license conditions.
Changes that expose Slint's API as a third-party SDK or target embedded systems
need a separate review against Slint's available licenses.

## Other Rust dependencies

`vendor/global-hotkey` preserves global-hotkey 0.9.0 and its original MIT / Apache
notices. A small X11 unregister fix flushes the connection so a released shortcut
is immediately available to other applications. See its `PATCH.md` for the source
revision and the native integration regression test. Windows and macOS code is
unchanged by that patch.

Exact dependency versions are recorded in `Cargo.lock`. `scripts/collect_licenses.py`
collects the resolved Cargo packages' license notices, package versions and source
links into `target/THIRD_PARTY_LICENSES.txt`; release packaging includes that file.
It may include build-only and other-target dependencies as a conservative superset.
The generated text retains upstream notices rather than replacing them with this
summary. Major components include winit, global-hotkey, tray-icon, x11rb, windows-sys,
rusqlite/SQLite, serde and rfd.
