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

Launch commands use an executable and a separate argument array. There is no shell-string evaluation. Executable/working-directory paths support `~`, `%NAME%` and `${NAME}`; argument values remain literal. Batch/PowerShell/VBS/JS executable entries are rejected. On macOS, `.app` launch uses the system `/usr/bin/open` utility. Windows includes common installation-path resolution for Code.exe, chrome.exe and wezterm-gui.exe.

## Acceptance

```sh
cargo test -p toolbox-windows
# Requires xvfb, openbox, xterm and x11-utils; creates an isolated X server.
bash crates/toolbox-windows/tests/run-x11-smoke.sh
```

The X11 smoke test exercises actual process launch, enumeration, focus, maximize, minimize/restore, stable cycling, minimize-all and stale-handle rejection, then closes its own test clients. It must never run against the user's active desktop. Windows and macOS cross-compilation checks validate bindings and types; they do not replace interactive acceptance for foreground policies, TCC, multiple displays, Spaces or application-specific window behavior.
