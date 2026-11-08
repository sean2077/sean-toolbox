# toolbox-ui

`sean-toolbox-ui` is the Slint frontend. Launch it through `sean-toolbox`, which
manages the UI process, its local endpoint and the saved configuration.

## Windows and interaction

The UI has two windows:

- `LauncherWindow`: a frameless, always-on-top recent-project search panel, with
  a default size of 760×432 logical pixels, 36-pixel result rows, path tooltips,
  remote markers and a selection counter. Alt+Enter expands a selectable full
  path; Escape closes those details before dismissing the panel.
  The global shortcut, tray and `sean-toolbox show projects` open this panel.
- `AppWindow`: the management window, with projects, application profiles,
  skills, settings and diagnostics pages. The preferred size is 1120×780, with
  an 800×580 minimum and compact icon navigation below 980 pixels wide.

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
  concurrent updates leave the draft available for correction. Saving an app
  retains its editor. Testing uses its saved configuration while preserving
  drafts. Filtered profile rows retain their source indices, so selection and
  actions do not depend on the currently visible row position.
- Program selection suggests the name and matcher for a new profile. Basic
  fields stay visible; IDs, matching rules and launch alternatives are advanced
  options. Field-specific validation opens the relevant section when needed.
  Launcher shortcut and cache time errors also update on every settings edit.
- Shortcut recording accepts normal UI key events and the host's registered
  shortcuts. A token-scoped three-second host lease intercepts the latter while
  the recorder polls. Native registrations stay intact; closing, focus loss or
  cancellation ends capture, and a missing UI expires without a resident timer.
- Settings previews apply language and theme immediately. Discarding settings
  restores their saved values and preserves any application draft.
- Static text uses Slint's bundled gettext translations. Rust-generated status
  and confirmation messages retain their identity and arguments, so language
  previews also refresh existing text, project summaries, imports and Skills
  details. Local worker errors and configuration checks run by the UI are
  translated using the current locale; backend diagnostics and user-provided
  metadata remain verbatim.
- Diagnostics refresh on request, showing capabilities, activity and errors.
  A failed refresh clears the previous capability snapshot.
- Skills exposes installed skills, repository discovery, local imports,
  repository settings and current operation status. Commands run in the host; the UI worker
  only performs IPC and folder selection. Full-width rows open details on
  demand. Bulk actions show hidden selections and use the same app icons as
  rows: checked means every selected skill uses the app (click disables all),
  a partial badge or none means a click enables all; tooltips give counts. Both folder and scan imports review the source, copy/link
  mode and deployment apps before submission. A picked folder whose name
  matches a managed skill offers that installed item instead of an import.
  The source filter follows its repository by ID and falls back to All once
  that repository is removed. A separate global deploy-mode
  selector chooses symlinks (default) or copies for new managed-skill app
  deployments; existing deployments keep their mode. Daily and manual updates
  share the host's serialized worker and persisted status. Operation status
  shows current state and errors, rather than a historical activity log.
- While the Skills page is visible, a single outstanding request polls the
  snapshot every 15 seconds, or every second while the host is busy. Navigation
  and closing stop polling. The UI waits for a command acknowledgement before
  releasing its memory; an accepted host job continues without the UI.
- Imports accept INI or JSON up to 1 MiB and show a preview. Merge replaces
  profiles with matching case-insensitive IDs while preserving settings. JSON
  replacement restores settings as well. Export reads the host's saved
  configuration and writes it atomically.

Native file dialogs use the active window as their parent. Linux file selection
requires an XDG Desktop Portal file-chooser backend or `zenity`; dialog errors
appear in the UI. Modal dialogs retain keyboard focus, including shortcuts.

## Appearance

System, Light and Dark themes share neutral surfaces and a restrained blue
accent across both windows, with Slint's Fluent native controls and matching
window decorations. System mode follows platform theme
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

`ui/main.slint` generates the Rust bindings and exports `ui/launcher.slint`
and `ui/skills.slint`.
Shared rows are defined in `ui/project-types.slint`, components in
`ui/components.slint`, shared messages in `ui/common-messages.slint`, and
embedded icons in `ui/icons/`. The frontend uses
Slint's Winit backend and software renderer.

Every page stays mounted, and layout constraints propagate from each page up
to the window, which Winit applies as the native minimum and maximum size. A
`max-width`, an explicit `width` on a page, or a fixed-width direct child of a
vertical layout caps the whole content column, so the window can no longer fill
the screen when maximized. Let text elide inside stretching layouts, and centre
fixed-size items inside a nested layout instead.

The UI binary accepts `--page projects|windows|settings|diagnostics|skills` and
`--endpoint <identifier>`. The endpoint identifies a current-user IPC channel.
Ctrl/Cmd+5 opens Skills; Ctrl/Cmd+F searches the current Skills view. Existing
page shortcuts retain their assignments.

Unit tests cover drafts, themes, imports, launch arguments, request ordering and
process lifetime. Linux integration checks are available in
[`scripts/smoke_host.py`](../../scripts/smoke_host.py) and
[`scripts/smoke_appearance.py`](../../scripts/smoke_appearance.py).
See the [development guide](../../docs/DEVELOPMENT.md) for their prerequisites and
commands, and the [validation record](../../docs/VALIDATION.md) for dated results.
The development guide also owns the translation resource layout, update/check
commands and the boundary between localized UI messages and raw diagnostics.

`scripts/render_ui.py` renders the actual Slint components with clearly labeled
example data, without a desktop session or host process. It is useful for
reviewing language, theme, geometry and UI states; it does not replace native
focus, global-hotkey or lifecycle tests.
