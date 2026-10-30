# toolbox-ui

`sean-toolbox-ui` is the Slint frontend. Launch it through `sean-toolbox`, which
manages the UI process, its local endpoint and the saved configuration.

## Windows and interaction

The UI has two windows:

- `LauncherWindow`: a frameless, always-on-top recent-project search panel, with
  a default size of 760×408 logical pixels, path tooltips and a selection counter.
  The global shortcut, tray and `sean-toolbox show projects` open this panel.
- `AppWindow`: the management window, with projects, application profiles,
  settings and diagnostics pages.

Each launcher activation refreshes history, clears the query and focuses search.
Repeated activations reuse the panel. Up/Down selects a result; Enter or
double-click opens it. Escape, focus loss or a successful open dismisses the
panel. An open failure keeps the panel visible with the error. Focus-loss handling
starts after the panel gains focus, and open results are tied to the launcher
invocation that requested them.

Launcher use preserves the management window's current page and drafts.
Keyboard shortcuts and editing instructions are in the
[usage guide](../../docs/USAGE.md).

## State and background work

The Slint event loop owns widgets and editable drafts. A blocking worker handles
project discovery, process launching, file access, native dialogs and host
requests. Results return through `slint::invoke_from_event_loop`.

- Search requests carry increasing generation numbers. The worker and UI apply
  results for the current generation; opening a row also checks its generation.
- Application drafts and settings drafts are separate. Saves carry the host's
  expected configuration revision. Failed validation, shortcut conflicts or
  concurrent updates leave the draft available for correction.
- Settings previews apply language and theme immediately. Discarding settings
  restores their saved values and preserves any application draft.
- Diagnostics refresh on request, showing capabilities, activity and errors.
  A failed refresh clears the previous capability snapshot.
- Imports accept INI or JSON up to 1 MiB and show a preview. Merge replaces
  profiles with matching case-insensitive IDs while preserving settings. JSON
  replacement restores settings as well. Export reads the host's saved
  configuration and writes it atomically.

Native file dialogs use the active window as their parent. Linux file selection
requires an XDG Desktop Portal file-chooser backend or `zenity`; dialog errors
appear in the UI. Modal dialogs retain keyboard focus, including shortcuts.

## Appearance

System, Light and Dark themes share Slint's Fluent palette across both windows,
native controls and window decorations. System mode follows platform theme
notifications. Each application profile has a Quake editor with a layout preview,
presets and controls for the supported platform options. Native docking behavior
is documented in the [window module](../toolbox-windows/README.md).

The window icon is embedded from
[`packaging/sean-toolbox.svg`](../../packaging/sean-toolbox.svg). Platform exports
are described in the [packaging guide](../../packaging/README.md). The
Diagnostics/About page includes Slint's `AboutSlint` attribution widget.

## Process lifetime

The UI can exit once both windows are hidden and there are no drafts, dialogs or
operations in progress. New configurations use a zero-second idle timeout.
A longer timeout caches the UI after automatic dismissal, such as losing focus
or opening a project. Explicit Close or Escape releases the last window
immediately once eligible.

Reopening a window stops the idle timer. The timer checks eligibility again on
the event loop before exiting. Shutdown wakes the activation listener to remove
its Unix socket. The host remains available for shortcuts and starts the UI again
when requested. Project discovery, focus handling and diagnostics are event-driven.

## Development

Run from the repository root:

```sh
cargo build --locked -p toolbox-host -p toolbox-ui
cargo test --locked -p toolbox-ui
cargo clippy --locked -p toolbox-ui --all-targets -- -D warnings
```

`ui/main.slint` generates the Rust bindings and exports `ui/launcher.slint`.
Shared rows are defined in `ui/project-types.slint`, components in
`ui/components.slint`, and embedded icons in `ui/icons/`. The frontend uses
Slint's Winit backend and software renderer.

The UI binary accepts `--page projects|windows|settings|diagnostics` and
`--endpoint <identifier>`. The endpoint identifies a current-user IPC channel.

Unit tests cover drafts, themes, imports, launch arguments, request ordering and
process lifetime. Linux integration checks are available in
[`scripts/smoke_host.py`](../../scripts/smoke_host.py) and
[`scripts/smoke_appearance.py`](../../scripts/smoke_appearance.py).
See the [development guide](../../docs/DEVELOPMENT.md) for their prerequisites and
commands, and the [validation record](../../docs/VALIDATION.md) for dated results.
