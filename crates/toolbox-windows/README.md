# toolbox-windows

Controls application windows through `WindowService`, which lives on the host's
window worker thread. Window operations run when requested. `capabilities()`
reports backend availability and any required desktop permissions.

## Backends and matching

Matchers are literal application identifiers, compared case-insensitively.

| Backend | Matching | Native operations | Requirements |
| --- | --- | --- | --- |
| Windows | Executable filename; a full path is reduced to its filename | Win32 enumeration, process identity checks, foreground requests, asynchronous show/minimize/restore | An interactive shell; elevated or protected targets can reject access |
| macOS | Bundle ID, application display name, executable filename or path | Accessibility window objects and AppKit application activation | Accessibility permission for the host executable |
| Linux X11 | WM_CLASS instance/class or process executable filename/path | EWMH/ICCCM requests through x11rb and client/process identity checks | X11 with an EWMH window manager |

Native Wayland window control is unavailable.

## Toggle behavior

- **No matching window:** try launch candidates in order until a process starts,
  then wait up to 3 seconds for a window. A pending launch has a 20-second
  cooldown shared by matching application identifiers, including across
  configuration reloads.
- **One window:** minimize it when foreground; otherwise restore and activate it,
  preserving its maximization state.
- **Multiple windows:** keep a stable cycle order across Z-order changes. With
  `cycle_continuous` enabled, the last window is followed by the first. Otherwise
  the final step minimizes all windows, and the next trigger starts at the first.
  A closed window's surviving successor keeps its place in the cycle.
- **After launch:** apply `maximize` to the newly opened window. Foreground checks
  preserve the user's focus if they switch applications while waiting.

## Quake docking

`quake.enabled` selects a fixed window for repeated show/minimize operations. It
chooses the focused matching window, or the first match, and retains it until it
closes. Showing the window applies the configured position and size. Ordinary
`maximize` and `cycle_continuous` settings apply when Quake is disabled.

Placement uses an edge (`top`, `bottom`, `left`, `right`), alignment (`start`,
`center`, `end`), width and height percentages (10–100), and display selection
(`cursor`, `foreground`, `primary`). Defaults are top, center, 100% width,
50% height and the cursor's display.

The backend captures the work area before launch or activation changes foreground.
Dimensions include the window frame and follow the application's minimum size
and terminal cell increments. Disabling Quake leaves the last window geometry in
place.

| Backend | Placement | Additional options |
| --- | --- | --- |
| Windows | Monitor work area; move before final resize for monitor DPI changes | Temporary topmost and foreground-change auto-hide |
| Linux X11 | XRandR 1.5 active monitors intersected with the EWMH work area; requires `_NET_MOVERESIZE_WINDOW` | Temporary `_NET_WM_STATE_ABOVE` |
| macOS | Accessibility position/size within `NSScreen.visibleFrame`; requires writable bounds and a window outside native fullscreen | Placement only |

`always_on_top` defaults to false. When enabled, the original stacking state is
recorded and restored on hide, configuration reset, failure or orderly host exit.
Hiding uses normal minimization.

`hide_on_focus_loss` defaults to false and is available on Windows. The host
receives `EVENT_SYSTEM_FOREGROUND` events through `SetWinEventHook` and forwards
them to the window worker. A shown Quake window is eligible for auto-hide; its
owned dialogs count as part of that window. The worker rechecks foreground before
minimizing, so refocusing cancels dismissal. Confirmed minimization ends the
visible session before stacking cleanup. Each window has one Quake owner, and
hook installation errors are returned to the caller. Escape remains available
to the target application.

## Launch commands

A launch candidate supplies an executable, an argument array and an optional
working directory. Paths expand `~`, `%NAME%` and `${NAME}`; arguments stay literal.
Entries ending in `.cmd`, `.bat`, `.ps1`, `.vbs` or `.js` are rejected.

On macOS, `.app` entries launch through `/usr/bin/open`. Windows resolves common
installation paths for `Code.exe`, `chrome.exe` and `wezterm-gui.exe`.

## Tests

Run from the repository root:

```sh
cargo test --locked -p toolbox-windows
bash crates/toolbox-windows/tests/run-x11-smoke.sh
```

The X11 script requires Xvfb, Openbox, xterm and x11-utils. It creates a private
display and checks launch, matching, focus, maximize/minimize/restore, cycling,
Quake placement, topmost cleanup and stale-handle rejection. It closes its test
clients when finished.

On Windows and macOS, check foreground policies, Accessibility permissions,
multiple displays, Spaces and application-specific window behavior on the target
desktop. Dated results are recorded in [VALIDATION.md](../../docs/VALIDATION.md).
