# Toolbox UI

`sean-toolbox-ui` is the on-demand native Slint frontend. Start it through
`sean-toolbox` so the resident host owns the single UI instance, its endpoint,
and the persisted configuration. The host does not depend on this crate.

## Responsibilities

- The Slint event loop owns widgets and editable drafts. Filesystem operations,
  recent-project discovery, process launching, native file dialogs, and host
  requests run on a blocking worker. Results return through
  `slint::invoke_from_event_loop`.
- Search requests have monotonic generations. Both the worker and UI reject
  obsolete results; stale displayed rows cannot launch a project while a newer
  query is pending. Entering the project page discovers history even when the
  interface was first opened on a different page. Single-click selects a row;
  double-click or Enter opens it. Up/Down/Enter act on the search field and list,
  preserving ordinary keyboard behavior for buttons and dialogs.
- Application and general-setting drafts are separate. Saves include the host's
  expected revision. Validation, shortcut conflicts, concurrent updates, and
  native failures leave the draft available for correction. Resetting settings
  preserves the application draft; in-flight reload/open results cannot discard
  newly entered drafts. Live language previews do not change saved module state.
- Ctrl/Cmd+K or F focuses project search, Ctrl/Cmd+S saves the current editor,
  and Ctrl/Cmd+1 through 4 changes pages. Modal dialogs keep keyboard focus and
  block these shortcuts. Icon actions retain accessible names and focus rings.
- Diagnostics use capability cards and separate activity, warnings and errors.
  A failed status refresh invalidates the previous capability snapshot. Raw
  technical details remain available on demand without periodic status polling.
- INI/JSON imports are bounded to 1 MiB and previewed before application. Merge
  replaces profiles with the same case-insensitive ID and preserves settings.
  JSON replacement also restores settings. Import never launches an application.
  Export asks the host for the current saved configuration and writes atomically.
- Hiding starts one timer; reopening stops it. A timeout of zero exits
  immediately. Normal exit wakes the activation listener so it can remove its
  own Unix socket. The UI performs no periodic project or status polling.
- Native file dialogs use the active window as their parent. On Linux, install
  a working XDG desktop portal file-chooser backend or `zenity`. Actual backend
  errors reported by `rfd` are displayed in the UI.

## Development

```sh
cargo build -p toolbox-ui
cargo test -p toolbox-ui
cargo clippy -p toolbox-ui --all-targets -- -D warnings
```

The generated bindings come from `ui/main.slint`; `ui/components.slint` owns
shared visual primitives and the small `ui/icons/` SVGs are embedded at build
time. The frontend accepts
`--page projects|windows|settings|diagnostics` and `--endpoint <identifier>`.
The endpoint is a current-user identifier, never an arbitrary socket path.

Unit tests cover literal argv boundaries, import limits, unknown JSON fields,
case-insensitive merge identity, hotkey conflicts, and the difference between
JSON merge and replacement. Linux/X11 UI checks used Xvfb, the real host,
temporary configuration/SQLite data, and a VS Code CLI fixture that records argv.
They exercised keyboard search/open requests, settings and application saves,
native failure recovery, native INI selection/preview/merge, JSON export,
Chinese/English layouts, warm reuse, zero/timed unloading, and socket cleanup.
These checks do not replace Windows/macOS desktop acceptance testing.

Slint is pinned to 1.18.1 with the Winit backend and software renderer. The
Diagnostics/About page contains the official `AboutSlint` attribution widget.
See the repository's license notices for dependency terms.
