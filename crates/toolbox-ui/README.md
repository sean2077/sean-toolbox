# Toolbox UI

`sean-toolbox-ui` is the on-demand native Slint frontend. Start it through
`sean-toolbox` so the resident host owns the single UI instance, its endpoint,
and the persisted configuration. The host does not depend on this crate.

## Surfaces

External `projects` activation (global shortcut, tray, default launch, or
`sean-toolbox show projects`) opens a separate `LauncherWindow`. It is a compact,
frameless, always-on-top search panel inspired by Recentry, with a default logical
size of 760×408, one-line results, full-path hover hints, and a selected/matched
counter. The larger `AppWindow` retains the four management pages.

Every summon refreshes history, clears the filter, and focuses search. Repeated
summons reset the same panel rather than toggling it. Up/Down clamp at the ends;
Enter or double-click opens, while single-click selects. Escape and focus loss
hide only the panel. Focus-loss dismissal is armed after the panel owns focus.
An open failure remains visible. An old open result cannot hide a new invocation.

The launcher supports Ctrl/Cmd+K or F to focus/select search, Ctrl/Cmd+R to refresh,
and Ctrl/Cmd+, or the settings button to open the management window. Showing or
using the launcher preserves an existing management page and its drafts. Saved
configuration controls project availability and execution.

## Appearance and application editing

Settings offer System, Light and Dark choices with immediate previews on both
surfaces. The shared semantic palette follows Slint's resolved Fluent palette;
native controls and application colors change together. System mode reacts to
the platform's theme notifications. Native window decorations receive the same
preference. Saving persists the choice; discarding settings restores the saved
palette without discarding an application draft.

The app and both native windows share `packaging/sean-toolbox.svg`. Platform
icon exports and executable/bundle integration are documented in the packaging
README. The UI embeds its vector icon at compile time.

Each application editor has an optional Quake section with a live layout
preview, top-half/right-side/work-area presets, edge, alignment, width/height
percentages, display choice and supported focus/topmost options. Unavailable
platform options are disabled and explained. Ordinary cycle/maximize options
are retained and are shown when Quake is off. See the window module README for
the native behavior contract.

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
- Idle release starts only after both surfaces are hidden and no draft, dialog or
  operation needs the process; reopening either surface stops the timer. New
  configurations default to zero, so the UI process exits immediately when
  eligible. An optional nonzero cache applies to automatic focus-loss/open-result
  dismissal. Explicit Close or Escape bypasses that cache for the last surface.
  Eligibility is rechecked on the event loop before exit, so a queued activation
  cannot be closed by an obsolete timer. Normal exit wakes the
  activation listener so it can remove its own Unix socket. The UI performs no
  periodic project, focus, or status polling. The small host remains available
  for shortcuts, and starts a fresh UI process on the next summon.
- Native file dialogs use the active window as their parent. On Linux, install
  a working XDG desktop portal file-chooser backend or `zenity`. Actual backend
  errors reported by `rfd` are displayed in the UI.

## Development

```sh
cargo build -p toolbox-ui
cargo test -p toolbox-ui
cargo clippy -p toolbox-ui --all-targets -- -D warnings
```

The generated bindings come from `ui/main.slint`, which also exports
`ui/launcher.slint`. `ui/project-types.slint` owns the shared row schema,
`ui/components.slint` owns shared visual primitives, and the small `ui/icons/`
SVGs are embedded at build time. The frontend accepts
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
