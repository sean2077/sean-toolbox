#!/usr/bin/env python3
"""Exercise saved appearance and native icons on an isolated Linux X11 desktop.

Build the adjacent host/UI binaries first, then run:
  python3 scripts/smoke_appearance.py --binary target/release/sean-toolbox

Uses the same temporary profile and private Xvfb desktop as smoke_host.py.
Optional --screenshots uses ImageMagick's import command; it is not needed for
the assertions. Native pixels and _NET_WM_ICON are read through libX11.
"""

from __future__ import annotations

import argparse
import ctypes
import ctypes.util
import json
import os
from pathlib import Path
import shutil
import signal
import sqlite3
import subprocess
import sys
import tempfile
import time
from typing import Any

from smoke_host import Desktop, SmokeFailure, become_child_subreaper, process_info, request, require, wait_for


class X11Probe:
    def __init__(self, display: str) -> None:
        library = ctypes.util.find_library("X11")
        require(library, "libX11 is required for native appearance checks")
        self.x = ctypes.CDLL(library)
        ptr, integer, ulong = ctypes.c_void_p, ctypes.c_int, ctypes.c_ulong
        self.x.XOpenDisplay.argtypes, self.x.XOpenDisplay.restype = [ctypes.c_char_p], ptr
        self.x.XCloseDisplay.argtypes = [ptr]
        self.x.XGetImage.argtypes = [ptr, ulong, integer, integer, ctypes.c_uint, ctypes.c_uint, ulong, integer]
        self.x.XGetImage.restype = ptr
        self.x.XGetPixel.argtypes, self.x.XGetPixel.restype = [ptr, integer, integer], ulong
        self.x.XDestroyImage.argtypes = [ptr]
        self.x.XInternAtom.argtypes, self.x.XInternAtom.restype = [ptr, ctypes.c_char_p, integer], ulong
        self.x.XGetWindowProperty.argtypes = [
            ptr, ulong, ulong, ctypes.c_long, ctypes.c_long, integer, ulong,
            ctypes.POINTER(ulong), ctypes.POINTER(integer), ctypes.POINTER(ulong),
            ctypes.POINTER(ulong), ctypes.POINTER(ptr),
        ]
        self.x.XFree.argtypes = [ptr]
        self.display = self.x.XOpenDisplay(display.encode())
        require(self.display, "Cannot connect to the private X11 display")

    def pixel(self, window: int, x: int, y: int) -> tuple[int, int, int]:
        image = self.x.XGetImage(self.display, window, x, y, 1, 1, ctypes.c_ulong(-1).value, 2)
        require(image, "Cannot read a pixel from the native window")
        try:
            value = self.x.XGetPixel(image, 0, 0)
            return (value >> 16 & 255, value >> 8 & 255, value & 255)
        finally:
            self.x.XDestroyImage(image)

    def icon(self, window: int) -> list[int]:
        atom = self.x.XInternAtom(self.display, b"_NET_WM_ICON", 0)
        actual_type, count, remaining = ctypes.c_ulong(), ctypes.c_ulong(), ctypes.c_ulong()
        actual_format, data = ctypes.c_int(), ctypes.c_void_p()
        result = self.x.XGetWindowProperty(
            self.display, window, atom, 0, 1024 * 1024, 0, 0,
            ctypes.byref(actual_type), ctypes.byref(actual_format), ctypes.byref(count),
            ctypes.byref(remaining), ctypes.byref(data),
        )
        require(result == 0 and actual_format.value == 32 and data.value, "Native window has no _NET_WM_ICON")
        try:
            require(remaining.value == 0, "Native icon property unexpectedly exceeds the probe limit")
            values = ctypes.cast(data, ctypes.POINTER(ctypes.c_ulong))
            return [values[index] & 0xFFFFFFFF for index in range(count.value)]
        finally:
            self.x.XFree(data)

    def close(self) -> None:
        self.x.XCloseDisplay(self.display)


class AppearanceDesktop(Desktop):
    def __init__(self, binary: Path, root: Path, screenshots: Path | None, scale: float) -> None:
        super().__init__(binary, root)
        self.screenshots = screenshots
        self.scale = scale
        self.environment["SLINT_SCALE_FACTOR"] = str(scale)
        self.probe: X11Probe | None = None
        self.manager = 0
        self.ui_pid = 0

    def spawn(self, command: list[str], log_name: str, **kwargs: Any) -> subprocess.Popen[bytes]:
        if command[0] == "Xvfb":
            screen = "2560x1800x24" if self.scale > 1 else "1600x1100x24"
            command = [screen if value == "1280x800x24" else value for value in command]
        return super().spawn(command, log_name, **kwargs)

    def point(self, value: float) -> int:
        return round(value * self.scale)

    def visible(self, title: str) -> list[int]:
        result = self.command("xdotool", "search", "--onlyvisible", "--all", "--pid", str(self.ui_pid), "--name", title, check=False)
        return [int(line) for line in result.stdout.splitlines() if line.isdigit()]

    def activate(self, window: int) -> None:
        self.command("xdotool", "windowactivate", "--sync", str(window))
        wait_for("native focus", lambda: self.active() == window)

    def click(self, x: float, y: float, window: int | None = None) -> None:
        self.command("xdotool", "mousemove", "--window", str(window or self.manager), str(self.point(x)), str(self.point(y)), "click", "1")

    def scroll_editor(self, count: int, button: int = 5) -> None:
        self.command("xdotool", "mousemove", "--window", str(self.manager),
                     str(self.point(1068)), str(self.point(400)), "click", "--repeat", str(count), "--delay", "40", str(button))

    def edit_first_profile(self) -> None:
        self.click(285, 274)
        # Mounted pages keep scroll positions. Reset through the real scrollbar
        # before interacting with fields; scrolling over a SpinBox edits it.
        self.scroll_editor(20, 4)

    def snapshot(self, label: str, window: int | str | None = None) -> None:
        if self.screenshots is None:
            return
        if window != "root":
            # Keep hover-expanded scrollbars and tooltips off the captured
            # controls; this is a native window capture without image edits.
            self.command("xdotool", "mousemove", "--window", str(window or self.manager),
                         str(self.point(16)), str(self.point(16)))
        # This wait is only for screenshots of the next compositor frame; all
        # pass/fail checks use observable native or persisted state below.
        time.sleep(0.15)
        suffix = "-150" if self.scale > 1 else ""
        path = self.screenshots / f"appearance-{label}{suffix}.png"
        self.command("import", "-window", str(window or self.manager), str(path))
        require(path.read_bytes().endswith(b"\x00\x00\x00\x00IEND\xaeB`\x82"),
                f"Native screenshot was not completely written: {path}")
        print(path, flush=True)

    def theme_color(self, window: int | None = None) -> tuple[int, int, int]:
        require(self.probe is not None, "Native probe is not initialized")
        x, y = (210, 20) if window is None else (8, 8)
        return self.probe.pixel(window or self.manager, self.point(x), self.point(y))

    def wait_theme(self, dark: bool, window: int | None = None) -> None:
        wait_for(f"{'dark' if dark else 'light'} native surface", lambda: (
            max(self.theme_color(window)) < 90 if dark else min(self.theme_color(window)) > 220
        ))

    def choose_theme(self, scheme: str) -> None:
        self.activate(self.manager)
        self.key("ctrl+3")
        self.click({"system": 380, "light": 654, "dark": 930}[scheme], 275)
        if scheme != "system":
            self.wait_theme(scheme == "dark")

    def saved(self) -> dict[str, Any]:
        response = self.rpc("get_config")
        require(response.get("ok"), f"Cannot read saved configuration: {response}")
        return response["config"]

    def save_theme(self, scheme: str) -> None:
        self.choose_theme(scheme)
        self.key("ctrl+s")
        wait_for("theme persisted by the real settings UI", lambda: self.saved()["theme"] == scheme)
        require(json.loads(self.config_path.read_text())["theme"] == scheme, "Saved theme did not reach disk")

    def show_manager(self) -> tuple[int, Path]:
        self.ui_pid, self.manager, endpoint = self.show_ui()
        self.activate(self.manager)
        self.key("ctrl+3")
        return self.ui_pid, endpoint

    def wait_popup_geometry(self, window: int) -> None:
        expected = (self.point(760), self.point(408))
        screen = tuple(map(int, self.command("xdotool", "getdisplaygeometry").stdout.split()))
        require(all(actual >= wanted for actual, wanted in zip(screen, expected)),
                f"Private screen {screen} is too small for the expected panel {expected}")
        centered = tuple((available - wanted) // 2 for available, wanted in zip(screen, expected))
        observed = (0, 0)
        position = (0, 0)
        correct_since: float | None = None

        def stable_size() -> bool:
            nonlocal observed, position, correct_since
            geometry = dict(line.split("=", 1) for line in self.command(
                "xdotool", "getwindowgeometry", "--shell", str(window)
            ).stdout.splitlines())
            observed = (int(geometry["WIDTH"]), int(geometry["HEIGHT"]))
            position = (int(geometry["X"]), int(geometry["Y"]))
            if observed != expected or any(abs(actual - wanted) > 1 for actual, wanted in zip(position, centered)):
                correct_since = None
                return False
            now = time.monotonic()
            if correct_since is None:
                correct_since = now
            # X11 resize is asynchronous. Observe the settled native size so
            # a later fixed-size hint cannot silently shrink a focused panel.
            return now - correct_since >= 0.16

        try:
            wait_for(f"stable project panel dimensions {expected}", stable_size, timeout=4)
        except SmokeFailure as error:
            hints = self.command("xprop", "-id", str(window), "WM_NORMAL_HINTS", check=False).stdout.strip()
            raise SmokeFailure(f"Project panel dimensions {observed} at {position}, expected {expected} at {centered}; {hints}") from error

    def popup(self, dark: bool) -> int:
        self.key("ctrl+alt+space")
        # xdotool search reads legacy WM_NAME, whose UTF-8 middle dot can be
        # exposed as Latin-1. Match ASCII here and verify the UTF-8 title below.
        window = wait_for("recent-project panel", lambda: self.visible("^Sean Toolbox.*Recent Projects$"))[0]
        require(self.command("xdotool", "getwindowname", str(window)).stdout.strip() == "Sean Toolbox · Recent Projects", "Global shortcut opened the wrong native surface")
        wait_for("project panel focus", lambda: self.active() == window)
        self.wait_theme(dark, window)
        self.wait_popup_geometry(window)
        return window

    def dismiss_popup(self, window: int) -> None:
        self.key("Escape")
        wait_for("panel dismissed", lambda: window not in self.visible("^Sean Toolbox.*Recent Projects$"))
        require(self.ui_children() == [self.ui_pid], "Dismissing a panel exited the visible manager")
        require(self.manager in self.visible("^Sean Toolbox$"), "Dismissing a panel hid the manager")
        self.activate(self.manager)

    def restart_ui(self, previous: int, endpoint: Path) -> tuple[int, Path]:
        self.activate(self.manager)
        self.key("Escape")
        self.require_ui_reaped(previous, endpoint)
        current, endpoint = self.show_manager()
        require(current != previous, "Reopening reused a UI process that should have exited")
        return current, endpoint

    def track_ui(self) -> tuple[int, Path]:
        children = wait_for("isolated UI child", self.ui_children)
        require(len(children) == 1, f"Expected one UI child, got {children}")
        self.ui_pid = children[0]
        info = process_info(self.ui_pid)
        require(info, "UI exited before it could be tracked")
        self.ui_identities[self.ui_pid] = int(info["start"])
        arguments = Path(f"/proc/{self.ui_pid}/cmdline").read_bytes().split(b"\0")
        endpoint = self.socket_directory / (arguments[arguments.index(b"--endpoint") + 1].decode() + ".sock")
        wait_for("UI activation socket", endpoint.exists)
        return self.ui_pid, endpoint

    def check_close_lifecycle(self, current: int, endpoint: Path) -> None:
        self.activate(self.manager)
        self.key("Escape")
        self.require_ui_reaped(current, endpoint)
        state = self.rpc("get_config")
        config = state["config"]
        config["ui_idle_seconds"] = 60
        response = self.rpc("save_config", {"config": config, "expected_revision": state["revision"]})
        require(response.get("ok"), "Warm-cache lifecycle fixture was rejected")
        dark = config["theme"] == "dark"

        self.key("ctrl+alt+space")
        current, endpoint = self.track_ui()
        popup = wait_for("cold project panel", lambda: self.visible("^Sean Toolbox.*Recent Projects$"))[0]
        wait_for("cold project panel acquires focus", lambda: self.active() == popup)
        self.wait_theme(dark, popup)
        self.wait_popup_geometry(popup)
        self.spawn(["xterm", "-title", "Appearance external target", "-e", "cat"], "appearance-external")
        external = wait_for("external focus target", lambda: self.command(
            "xdotool", "search", "--onlyvisible", "--name", "^Appearance external target$", check=False
        ).stdout.strip())
        self.activate(int(external.splitlines()[0]))
        wait_for("automatic blur dismissal", lambda: popup not in self.visible("^Sean Toolbox.*Recent Projects$"))
        require(self.ui_children() == [current], "Automatic blur ignored the explicit warm-cache timeout")
        self.key("ctrl+alt+space")
        wait_for("same panel reused from warm cache", lambda: popup in self.visible("^Sean Toolbox.*Recent Projects$") and self.active() == popup)
        self.wait_popup_geometry(popup)
        require(self.ui_children() == [current], "Warm activation replaced the retained UI process")
        print("PASS automatic focus-loss dismissal preserves the configured warm cache and reuses the UI", flush=True)

        self.key("Escape")
        self.require_ui_reaped(current, endpoint)
        require(self.rpc("ping")["status"]["process_id"] == self.host_id, "Manual panel close exited the host")
        self.key("ctrl+alt+space")
        reopened, endpoint = self.track_ui()
        require(reopened != current, "Explicit panel dismissal kept the old UI PID")
        popup = wait_for("reopened project panel", lambda: self.visible("^Sean Toolbox.*Recent Projects$"))[0]
        self.activate(popup)
        self.wait_popup_geometry(popup)
        self.key("alt+F4")
        self.require_ui_reaped(reopened, endpoint)
        print("PASS Escape and native close release the last panel process despite a 60-second cache", flush=True)

        reopened, endpoint = self.show_manager()
        self.key("alt+F4")
        self.require_ui_reaped(reopened, endpoint)
        require(self.rpc("ping")["status"]["process_id"] == self.host_id, "Closing the manager exited the host")
        print("PASS native manager close releases its UI process/socket while the host stays live", flush=True)

        self.key("ctrl+alt+space")
        self.track_ui()
        wait_for("panel for rapid reopen checks", self.ready_popup)
        for index in range(8):
            self.key("Escape")
            self.command(str(self.binary), "show", "projects")
            wait_for(f"rapid reopen {index + 1}", self.ready_popup)
            self.wait_popup_geometry(self.visible("^Sean Toolbox.*Recent Projects$")[0])
            warnings = self.rpc("status")["status"]["warnings"]
            require(not any(warning.startswith("UI:") for warning in warnings), f"Rapid reopen reported a UI failure: {warnings}")
        print("PASS eight immediate Escape → Show cycles keep one responsive UI process and panel", flush=True)

        current, endpoint = self.track_ui()
        os.kill(current, signal.SIGSTOP)
        try:
            self.command(str(self.binary), "show", "projects")
            warnings = wait_for("unresponsive UI is reported", lambda: [
                warning for warning in self.rpc("status")["status"]["warnings"]
                if warning.startswith("UI:")
            ])
            require(any("not responding" in warning for warning in warnings), f"Wrong unresponsive-child warning: {warnings}")
            require(self.ui_children() == [current], "An unresponsive live UI was replaced or duplicated")
        finally:
            os.kill(current, signal.SIGCONT)
        self.command(str(self.binary), "show", "projects")
        wait_for("UI resumes after SIGCONT", self.ready_popup)
        wait_for("successful show clears the UI warning", lambda: not any(
            warning.startswith("UI:") for warning in self.rpc("status")["status"]["warnings"]
        ))
        require(self.ui_children() == [current], "Recovering an unresponsive UI replaced its PID")
        self.key("Escape")
        self.require_ui_reaped(current, endpoint)
        print("PASS a stopped live UI is reported without duplication and recovers with the same PID", flush=True)

    def ready_popup(self) -> bool:
        children = self.ui_children()
        require(len(children) <= 1, f"Multiple live UI children: {children}")
        if not children:
            return False
        self.ui_pid = children[0]
        try:
            arguments = Path(f"/proc/{self.ui_pid}/cmdline").read_bytes().split(b"\0")
            endpoint = self.socket_directory / (arguments[arguments.index(b"--endpoint") + 1].decode() + ".sock")
            reply = request(endpoint, {"page": "projects", "readiness_only": True})
        except (OSError, ValueError, SmokeFailure):
            return False
        windows = self.visible("^Sean Toolbox.*Recent Projects$")
        require(len(windows) <= 1, f"Multiple visible project panels: {windows}")
        return bool(reply.get("ok") and windows and self.active() == windows[0])

    def add_projects(self) -> Path:
        install = self.root / "home/.local/share/code"
        (install / "resources/app").mkdir(parents=True)
        (install / "resources/app/product.json").write_text(json.dumps({
            "applicationName": "code", "nameShort": "Code", "version": "1.0.0",
            "sharedDataFolderName": ".vscode",
        }))
        executable = install / "code"
        executable.write_text("#!/bin/sh\nexit 0\n")
        executable.chmod(0o755)
        database = self.root / "home/.vscode/sharedStorage/state.vscdb"
        database.parent.mkdir(parents=True)
        entries = []
        for name in (
            "sean-toolbox", "recentry", "terminal-workspace", "design-system",
            "api-playground", "docs-site", "system-tools", "workbench",
            "design-tokens", "workspace-notes", "mobile-client", "中文项目",
            "example-project", "automation-scripts",
        ):
            directory = self.root / "home/Projects" / name
            directory.mkdir(parents=True)
            entries.append({"folderUri": directory.as_uri(), "label": name})
        with sqlite3.connect(database) as connection:
            connection.execute("CREATE TABLE ItemTable (key TEXT PRIMARY KEY, value TEXT)")
            connection.execute("INSERT INTO ItemTable VALUES (?, ?)", (
                "history.recentlyOpenedPathsList", json.dumps({"entries": entries}),
            ))
        return executable

    def check_icon(self, window: int) -> list[int]:
        require(self.probe is not None, "Native probe is not initialized")
        values = self.probe.icon(window)
        require(len(values) > 2, "Native icon is empty")
        width, height = values[:2]
        require(width >= 16 and height >= 16 and len(values) >= 2 + width * height, "Native icon dimensions are invalid")
        pixels = values[2:2 + width * height]
        require(any(value >> 24 == 0 for value in pixels), "Native icon lost its transparent background")
        require(any(value >> 24 > 200 and (value >> 8 & 255) > 180 and (value >> 16 & 255) < 160 and (value & 255) > 130 for value in pixels), "Native icon is missing the new mint-colored prompt")
        return values

    def check_quake_editor(self) -> None:
        self.key("ctrl+2")
        self.edit_first_profile()
        self.scroll_editor(5)
        self.click(485, 236)
        require(self.probe is not None, "Native probe is not initialized")
        wait_for("enabled Quake preview is drawn", lambda: (
            (color := self.probe.pixel(self.manager, self.point(770), self.point(250)))[2] - color[0] > 10
        ))
        self.click(760, 351)
        self.key("ctrl+s")
        expected = {"enabled": True, "edge": "right", "alignment": "center", "width_percent": 40, "height_percent": 100}
        wait_for("Quake controls persist the right-side preset", lambda: expected.items() <= self.saved()["profiles"][0]["quake"].items())
        self.key("F8")
        windows = wait_for("Quake profile launches a real terminal", self.windows)
        require(len(windows) == 1, "Quake profile created multiple application windows")
        terminal = windows[0]
        wait_for("Quake terminal foreground", lambda: self.active() == terminal)
        self.rpc("status")
        self.key("F8")
        wait_for("second Quake hotkey minimizes the same terminal", lambda: self.minimized(terminal))
        require(self.windows() == [terminal], "Quake toggle launched another terminal")
        self.activate(self.manager)
        print("PASS Quake UI preset → saved profile → real F8 launch and second-trigger minimize", flush=True)

        self.edit_first_profile()
        self.scroll_editor(3)
        self.snapshot("en-light-quake-editor")
        self.click(90, 620)
        self.click(1000, 428)
        self.key("Up")
        self.key("Return")
        self.key("ctrl+s")
        wait_for("Chinese language selected and saved through the UI", lambda: self.saved()["language"] == "zh-CN")
        self.save_theme("dark")
        self.save_theme("light")
        self.snapshot("cn-light-settings")
        popup = self.popup(False)
        self.snapshot("cn-light-launcher", popup)
        self.dismiss_popup(popup)
        self.save_theme("dark")
        self.snapshot("cn-dark-settings")
        popup = self.popup(True)
        self.snapshot("cn-dark-launcher", popup)
        self.dismiss_popup(popup)
        self.click(90, 210)
        self.edit_first_profile()
        self.scroll_editor(3)
        self.snapshot("cn-dark-quake-editor")
        self.command("xdotool", "windowsize", "--sync", str(self.manager), str(self.point(900)), str(self.point(650)))
        geometry = dict(line.split("=", 1) for line in self.command("xdotool", "getwindowgeometry", "--shell", str(self.manager)).stdout.splitlines())
        require(int(geometry["WIDTH"]) == self.point(900) and int(geometry["HEIGHT"]) == self.point(650), "Manager cannot reach the supported minimum size")
        self.snapshot("cn-dark-quake-minimum")
        self.command("xdotool", "windowsize", "--sync", str(self.manager), str(self.point(1120)), str(self.point(780)))
        print("PASS Chinese light/dark controls and native default/minimum window sizes", flush=True)

    def run_appearance(self) -> None:
        initial = self.start()
        config = initial["config"]
        config.update(language="en", theme="system", projects_enabled=True, ui_idle_seconds=0)
        config["vscode_path_override"] = str(self.add_projects())
        response = self.rpc("save_config", {"config": config, "expected_revision": initial["revision"]})
        require(response.get("ok"), f"Appearance fixture was rejected: {response}")
        self.probe = X11Probe(self.environment["DISPLAY"])
        try:
            previous, endpoint = self.show_manager()
            system_dark = max(self.theme_color()) < 90
            manager_icon = self.check_icon(self.manager)
            for scheme in ("dark", "light", "system"):
                self.save_theme(scheme)
                dark = system_dark if scheme == "system" else scheme == "dark"
                self.wait_theme(dark)
                self.snapshot(f"en-{scheme}-settings")
                popup = self.popup(dark)
                require(self.check_icon(popup) == manager_icon, "Manager and launcher native icons differ")
                self.snapshot(f"en-{scheme}-launcher", popup)
                self.dismiss_popup(popup)
                previous, endpoint = self.restart_ui(previous, endpoint)
                self.wait_theme(dark)
                require(self.saved()["theme"] == scheme, "New UI process changed the saved theme")
                print(f"PASS real {scheme} choice → save → native manager/panel → fresh UI PID", flush=True)
            print("PASS both native windows expose the new matching application icon", flush=True)

            self.save_theme("light")
            self.key("ctrl+2")
            self.click(285, 274)
            self.key("ctrl+a")
            self.command("xdotool", "type", "--clearmodifiers", "--delay", "2", "Terminal")
            self.key("ctrl+3")
            self.choose_theme("dark")
            require(self.saved()["theme"] == "light", "Preview persisted without Save")
            popup = self.popup(True)
            self.dismiss_popup(popup)
            self.click(945, 700)
            wait_for("discard confirmation", lambda: max(self.theme_color()) < 30)
            self.key("Tab")
            self.key("Return")
            self.wait_theme(False)
            require(self.saved()["theme"] == "light", "Discard changed the saved theme")
            self.key("ctrl+2")
            self.key("ctrl+s")
            wait_for("application draft survived theme discard", lambda: self.saved()["profiles"][0]["name"] == "Terminal")
            self.snapshot("en-light-draft-retained")
            print("PASS theme preview is unsaved; Discard restores the saved theme and retains the application draft", flush=True)
            self.check_quake_editor()
            self.check_close_lifecycle(previous, endpoint)
        finally:
            self.probe.close()
            self.probe = None


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--binary", type=Path, required=True)
    parser.add_argument("--screenshots", type=Path, help="Optional directory for real native window screenshots")
    parser.add_argument("--scale", type=float, default=1.0, choices=(1.0, 1.5))
    args = parser.parse_args()
    require(sys.platform.startswith("linux"), "Native appearance smoke currently runs on Linux X11 only")
    binary = args.binary.expanduser().resolve(strict=True)
    require(os.access(binary, os.X_OK) and os.access(binary.with_name("sean-toolbox-ui"), os.X_OK), "Build both adjacent executables first")
    for command in ("Xvfb", "openbox", "xterm", "xdotool", "xprop"):
        require(shutil.which(command), f"Missing test dependency: {command}")
    if args.screenshots:
        require(shutil.which("import"), "Optional screenshots require ImageMagick import")
        args.screenshots.mkdir(parents=True, exist_ok=True)
    become_child_subreaper()
    def terminate(_signal: int, _frame: Any) -> None:
        raise KeyboardInterrupt
    signal.signal(signal.SIGTERM, terminate)
    with tempfile.TemporaryDirectory(prefix="st-appearance-", dir="/tmp") as directory:
        desktop = AppearanceDesktop(binary, Path(directory), args.screenshots, args.scale)
        try:
            desktop.run_appearance()
        except Exception:
            try:
                desktop.snapshot("failure", "root")
            except (SmokeFailure, OSError, subprocess.SubprocessError):
                pass
            desktop.dump_logs()
            raise
        finally:
            desktop.close()
    print("PASS appearance acceptance; isolated desktop and processes cleaned", flush=True)
    return 0


if __name__ == "__main__":
    try:
        raise SystemExit(main())
    except (SmokeFailure, OSError, subprocess.SubprocessError) as error:
        print(f"FAIL: {error}", file=sys.stderr)
        raise SystemExit(1)
    except KeyboardInterrupt:
        print("Interrupted; isolated test processes cleaned", file=sys.stderr)
        raise SystemExit(130)
