# Native window module

`WindowService` is created and used on the host's window worker thread. It has no idle polling loop and does not require `Send`. `capabilities()` reports an unavailable desktop or permission error without making the other toolbox modules unusable.

| Backend | Matching | Native operations | Limits |
| --- | --- | --- | --- |
| Windows | Executable filename, case insensitive; a full path is reduced to its filename | Win32 enumeration, PID/process-start validation, ordinary foreground requests, asynchronous show/minimize/restore | An interactive shell is required. Protected/elevated targets may be unreadable or reject activation. |
| macOS | Bundle ID, application display name, executable filename or path, case insensitive | Accessibility window objects, AppKit application activation, visible display work area | The **host executable** requires Accessibility permission. Application-specific AX behavior needs native acceptance. |
| Linux X11 | WM_CLASS instance/class or process executable filename/path, case insensitive | EWMH/ICCCM requests through x11rb, client/process identity validation | Requires an EWMH window manager. Native Wayland sessions are explicitly unsupported. |

Matchers are literal application identifiers. AHK selectors, title expressions and regular expressions are not evaluated.

## Toggle behavior

- No matching window: try launch candidates until a process starts, then wait up to 3 seconds for a window. A successful spawn never falls through to another executable. Matching application identifiers share a 20-second cooldown while startup is unresolved, including after a configuration reload.
- One matching window: minimize it when foreground; otherwise restore and activate it. Restoring preserves its maximization state.
- Multiple matching windows: retain a stable order even when the operating system changes Z-order. A continuous cycle wraps; a finite cycle minimizes all at the end and starts at the first window on the next trigger. Closing the selected window retains its surviving successor.
- Maximize only a newly launched window. Recheck foreground before launch-related mutations and after native waits; if the user changed applications, preserve their focus.

## Quake docking

Each profile can opt into `quake.enabled`; omitted settings keep the original toggle/cycle behavior. Quake selects the initially focused matching window, otherwise the first matching window, and then keeps that same window until it closes. Pressing its shortcut while the selected window is focused minimizes it; another press places and activates it. Other windows belonging to the application are left alone. `maximize` and `cycle_continuous` remain stored but apply only to normal mode.

The configuration uses `edge` (`top`, `bottom`, `left`, `right`), `alignment` (`start`, `center`, `end` along the docked edge), `width_percent` and `height_percent` (10–100), and `monitor` (`cursor`, `foreground`, `primary`). Defaults are top, center, 100% width, 50% height, and cursor monitor. The target work area is captured before launch, restore or activation can change foreground. Dimensions include the native window frame; application minimum sizes and terminal cell increments can constrain the result. Rejected placement is reported as an error.

`always_on_top` defaults to false. Windows and X11 can temporarily raise a shown Quake window above other applications. Its original stacking state is recorded and restored on hide, configuration reset, failure or orderly host exit. An application that was already topmost stays topmost. Quake uses normal minimization to hide, rather than moving windows offscreen or terminating their processes. Docking does not restore the previous window size when it is disabled; the last visible geometry remains usable.

`hide_on_focus_loss` defaults to false and is supported on Windows only. The host receives `EVENT_SYSTEM_FOREGROUND` notifications through `SetWinEventHook`; it coalesces them onto the window worker, with no idle timer or desktop enumeration loop. Only a successfully shown, opted-in Quake session is armed. The worker re-reads actual foreground and keeps owned dialogs open. A final foreground check precedes minimization; refocusing cancels auto-hide without changing the visible window’s stacking. Only confirmed minimization disarms the session before style cleanup. A window has one Quake owner even when several profiles match it. Hook installation failure rejects the opted-in operation instead of pretending auto-hide is active. Escape remains available to the target application.

| Backend | Quake placement | Optional behavior |
| --- | --- | --- |
| Windows | Monitor work area; cursor, foreground or primary display; move before final resize for monitor DPI changes | Temporary topmost and native foreground-change auto-hide. Elevated targets and application constraints can reject operations. |
| Linux X11 | XRandR 1.5 active monitors intersected with the current EWMH work area; cursor, foreground or primary display | Temporary `_NET_WM_STATE_ABOVE`; requires `_NET_MOVERESIZE_WINDOW`. Auto-hide is explicitly unsupported. |
| macOS | Accessibility position/size in `NSScreen.visibleFrame`; cursor, foreground or primary display | External-app topmost and auto-hide are explicitly unsupported. Exit native fullscreen before docking; Accessibility permission and writable bounds are required. |

Launch commands use an executable and a separate argument array. There is no shell-string evaluation. Executable/working-directory paths support `~`, `%NAME%` and `${NAME}`; argument values remain literal. Batch/PowerShell/VBS/JS executable entries are rejected. On macOS, `.app` launch uses the system `/usr/bin/open` utility. Windows includes common installation-path resolution for Code.exe, chrome.exe and wezterm-gui.exe.

## Acceptance

```sh
cargo test -p toolbox-windows
# Requires xvfb, openbox, xterm and x11-utils; creates an isolated X server.
bash crates/toolbox-windows/tests/run-x11-smoke.sh
```

The X11 smoke test exercises actual process launch, enumeration, focus, maximize, minimize/restore, stable cycling, minimize-all, Quake work-area placement, fixed-window toggling, temporary topmost cleanup, preservation of an existing topmost state, and stale-handle rejection, then closes its own test clients. It must never run against the user's active desktop. Windows and macOS cross-compilation checks validate bindings and types; they do not replace interactive acceptance for foreground policies, TCC, multiple displays, Spaces or application-specific window behavior.
