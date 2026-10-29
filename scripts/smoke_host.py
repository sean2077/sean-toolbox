#!/usr/bin/env python3
"""Linux host/UI acceptance in a private Xvfb desktop and temporary user profile.

Build both adjacent binaries first, then run:
  python3 scripts/smoke_host.py --binary target/debug/sean-toolbox
  python3 scripts/smoke_host.py --binary target/release/sean-toolbox --measure

Requires Python 3.10+, Xvfb, Openbox, xterm, xdotool, xprop and libX11.
No existing desktop, toolbox instance, HOME or configuration is used. The
optional ten-second measurement covers this Linux host PID with no UI process;
it is not a cross-platform benchmark or a resource-use guarantee.
"""

from __future__ import annotations

import argparse
import copy
import ctypes
import ctypes.util
import json
import os
from pathlib import Path
import re
import select
import shutil
import signal
import socket
import struct
import subprocess
import sys
import tempfile
import time
from typing import Any, Callable


MAX_MESSAGE = 1024 * 1024


class SmokeFailure(RuntimeError):
    pass


def require(condition: Any, message: str) -> None:
    if not condition:
        raise SmokeFailure(message)


def wait_for(description: str, predicate: Callable[[], Any], timeout: float = 12.0) -> Any:
    deadline = time.monotonic() + timeout
    last_error: Exception | None = None
    while True:
        try:
            result = predicate()
            if result:
                return result
        except (OSError, subprocess.SubprocessError) as error:
            last_error = error
        if time.monotonic() >= deadline:
            suffix = f": {last_error}" if last_error else ""
            raise SmokeFailure(f"Timed out waiting for {description}{suffix}")
        time.sleep(0.04)


def process_info(pid: int) -> dict[str, int | str] | None:
    try:
        contents = Path(f"/proc/{pid}/stat").read_text()
        fields = contents[contents.rfind(")") + 2 :].split()
        return {
            "state": fields[0],
            "ppid": int(fields[1]),
            "group": int(fields[2]),
            "ticks": int(fields[11]) + int(fields[12]),
            "start": int(fields[19]),
        }
    except (FileNotFoundError, ProcessLookupError):
        return None


def process_exists(pid: int, started: int) -> bool:
    info = process_info(pid)
    return info is not None and info["start"] == started


def children_of(pid: int) -> set[int]:
    # A worker thread spawns the UI; the main thread's children file alone does
    # not list children created by all of the process's threads.
    children: set[int] = set()
    paths = list(Path(f"/proc/{pid}/task").glob("*/children"))
    for path in paths:
        try:
            children.update(int(value) for value in path.read_text().split())
        except FileNotFoundError:
            pass
    if paths:
        return children
    # Some container kernels omit the optional /proc task/children files.
    # PPID in /proc/<pid>/stat provides the same process relationship without
    # reading unrelated command lines or environments.
    for path in Path("/proc").iterdir():
        if path.name.isdigit():
            info = process_info(int(path.name))
            if info is not None and info["ppid"] == pid:
                children.add(int(path.name))
    return children


def become_child_subreaper() -> None:
    # Adopt this test's orphaned descendants if the host exits before its test
    # xterm. This affects only the smoke-test process, not the tested binaries.
    libc = ctypes.CDLL(None, use_errno=True)
    libc.prctl.argtypes = [ctypes.c_int] + [ctypes.c_ulong] * 4
    libc.prctl.restype = ctypes.c_int
    if libc.prctl(36, 1, 0, 0, 0) != 0:  # PR_SET_CHILD_SUBREAPER
        code = ctypes.get_errno()
        raise OSError(code, "Cannot enable isolated descendant reaping")


def receive_exact(connection: socket.socket, length: int) -> bytes:
    chunks = bytearray()
    while len(chunks) < length:
        data = connection.recv(length - len(chunks))
        if not data:
            raise SmokeFailure(
                f"IPC closed before a complete response ({len(chunks)}/{length} bytes)"
            )
        chunks.extend(data)
    return bytes(chunks)


def request(endpoint: Path, message: dict[str, Any]) -> dict[str, Any]:
    payload = json.dumps(message, ensure_ascii=False).encode("utf-8")
    require(len(payload) <= MAX_MESSAGE, "Test request exceeds IPC limit")
    with socket.socket(socket.AF_UNIX, socket.SOCK_STREAM) as connection:
        connection.settimeout(8.0)
        connection.connect(str(endpoint))
        connection.sendall(struct.pack("<I", len(payload)) + payload)
        length = struct.unpack("<I", receive_exact(connection, 4))[0]
        require(0 < length <= MAX_MESSAGE, f"Invalid IPC response length: {length}")
        response = json.loads(receive_exact(connection, length))
    require(isinstance(response, dict), "IPC response is not a JSON object")
    return response


def require_key_released(display: str, key: str) -> None:
    """Prove the old unmodified key can be grabbed by another X11 client.

    Merely observing no toolbox action would miss a leaked passive grab whose
    binding was removed from the host's internal dispatch table.
    """
    library = ctypes.util.find_library("X11")
    require(library, "libX11 is required to check that the old shortcut was unregistered")
    xlib = ctypes.CDLL(library)
    pointer, ulong, integer = ctypes.c_void_p, ctypes.c_ulong, ctypes.c_int

    class XErrorEvent(ctypes.Structure):
        _fields_ = [
            ("type", integer), ("display", pointer), ("resourceid", ulong),
            ("serial", ulong), ("error_code", ctypes.c_ubyte),
            ("request_code", ctypes.c_ubyte), ("minor_code", ctypes.c_ubyte),
        ]

    xlib.XOpenDisplay.argtypes, xlib.XOpenDisplay.restype = [ctypes.c_char_p], pointer
    xlib.XCloseDisplay.argtypes = [pointer]
    xlib.XDefaultRootWindow.argtypes, xlib.XDefaultRootWindow.restype = [pointer], ulong
    xlib.XStringToKeysym.argtypes, xlib.XStringToKeysym.restype = [ctypes.c_char_p], ulong
    xlib.XKeysymToKeycode.argtypes = [pointer, ulong]
    xlib.XKeysymToKeycode.restype = ctypes.c_ubyte
    xlib.XGrabKey.argtypes = [pointer, integer, ctypes.c_uint, ulong, integer, integer, integer]
    xlib.XUngrabKey.argtypes = [pointer, integer, ctypes.c_uint, ulong]
    xlib.XSync.argtypes = [pointer, integer]
    xlib.XSetErrorHandler.argtypes, xlib.XSetErrorHandler.restype = [pointer], pointer
    errors: list[int] = []
    callback_type = ctypes.CFUNCTYPE(integer, pointer, ctypes.POINTER(XErrorEvent))

    @callback_type
    def on_error(_display: int, event: Any) -> int:
        errors.append(event.contents.error_code)
        return 0

    connection = xlib.XOpenDisplay(display.encode())
    require(connection, "Cannot open the isolated X11 display for the shortcut probe")
    previous = xlib.XSetErrorHandler(ctypes.cast(on_error, pointer))
    try:
        root = xlib.XDefaultRootWindow(connection)
        code = xlib.XKeysymToKeycode(connection, xlib.XStringToKeysym(key.encode()))
        require(code != 0, f"No X11 keycode for {key}")
        xlib.XGrabKey(connection, code, 0, root, 0, 1, 1)
        xlib.XSync(connection, 0)
        xlib.XUngrabKey(connection, code, 0, root)
        xlib.XSync(connection, 0)
        require(not errors, f"Old shortcut {key} remains grabbed; X11 errors: {errors}")
    finally:
        xlib.XSetErrorHandler(previous)
        xlib.XCloseDisplay(connection)


class Desktop:
    def __init__(self, binary: Path, root: Path) -> None:
        self.binary = binary
        self.ui_binary = binary.with_name("sean-toolbox-ui")
        self.root = root
        self.environment = os.environ.copy()
        for name in (
            "DISPLAY", "WAYLAND_DISPLAY", "XAUTHORITY", "DBUS_SESSION_BUS_ADDRESS",
            "DBUS_SESSION_BUS_PID", "DBUS_STARTER_ADDRESS", "DBUS_STARTER_BUS_TYPE",
        ):
            self.environment.pop(name, None)
        for variable, directory in (
            ("HOME", "home"), ("XDG_RUNTIME_DIR", "run"),
            ("XDG_CONFIG_HOME", "config"), ("XDG_CACHE_HOME", "cache"),
            ("XDG_DATA_HOME", "data"), ("XDG_STATE_HOME", "state"), ("TMPDIR", "tmp"),
        ):
            path = root / directory
            path.mkdir(mode=0o700)
            self.environment[variable] = str(path)
        self.environment["XDG_SESSION_TYPE"] = "x11"
        self.environment["GDK_BACKEND"] = "x11"
        self.environment["WINIT_UNIX_BACKEND"] = "x11"
        self.config_path = root / "config" / "toolbox.json"
        self.environment["SEAN_TOOLBOX_CONFIG"] = str(self.config_path)
        self.socket_directory = root / "run" / "sean-toolbox"
        self.endpoint = self.socket_directory / "sean-toolbox-host-v1.sock"
        self.processes: list[subprocess.Popen[bytes]] = []
        self.logs: dict[str, Any] = {}
        self.host: subprocess.Popen[bytes] | None = None
        self.host_id = 0
        self.ui_identities: dict[int, int] = {}
        self.failures: list[str] = []
        self.terminal_class = f"SeanToolboxSmoke{os.getpid()}"

    def spawn(self, command: list[str], log_name: str, **kwargs: Any) -> subprocess.Popen[bytes]:
        log = (self.root / f"{log_name}.log").open("wb")
        self.logs[log_name] = log
        process = subprocess.Popen(
            command, env=self.environment, stdin=subprocess.DEVNULL,
            stdout=log, stderr=subprocess.STDOUT, start_new_session=True, **kwargs,
        )
        self.processes.append(process)
        return process

    def command(self, *args: str, check: bool = True) -> subprocess.CompletedProcess[str]:
        result = subprocess.run(
            args, env=self.environment, stdin=subprocess.DEVNULL,
            capture_output=True, text=True, timeout=5,
        )
        if check:
            require(result.returncode == 0, f"{' '.join(args)} failed: {result.stderr.strip()}")
        return result

    def rpc(self, kind: str, data: Any = None) -> dict[str, Any]:
        command = {"type": kind}
        if data is not None:
            command["data"] = data
        return request(self.endpoint, {"version": 1, "command": command})

    def start(self) -> dict[str, Any]:
        read_fd, write_fd = os.pipe()
        try:
            server = self.spawn(
                ["Xvfb", "-displayfd", str(write_fd), "-screen", "0", "1280x800x24",
                 "-nolisten", "tcp", "-ac"], "xvfb", pass_fds=(write_fd,),
            )
        finally:
            os.close(write_fd)
        try:
            require(select.select([read_fd], [], [], 8)[0], "Xvfb did not allocate a display")
            display = os.read(read_fd, 64).decode().strip()
            require(display.isdigit() and server.poll() is None, "Xvfb failed to start")
        finally:
            os.close(read_fd)
        self.environment["DISPLAY"] = f":{display}"
        self.spawn(["openbox", "--sm-disable"], "openbox")
        wait_for("Openbox EWMH readiness", lambda: "window id" in self.command(
            "xprop", "-root", "_NET_SUPPORTING_WM_CHECK", check=False
        ).stdout)
        config = {
            "schema_version": 1, "language": "en", "launcher_hotkey": "Ctrl+Alt+Space",
            "autostart": False, "projects_enabled": False, "windows_enabled": True,
            "ui_idle_seconds": 0, "vscode_path_override": None,
            "profiles": [{
                "id": "smoke-xterm", "name": "Smoke terminal", "platform": "linux",
                "matcher": self.terminal_class, "hotkey": "F8", "enabled": True,
                "maximize": False, "cycle_continuous": True,
                "launch": [{"executable": shutil.which("xterm"), "args": [
                    "-class", self.terminal_class, "-title", "Toolbox smoke terminal",
                    "-e", shutil.which("cat") or "/bin/cat",
                ], "cwd": str(self.root / "home")}],
            }],
        }
        self.config_path.write_text(json.dumps(config, indent=2) + "\n")
        self.config_path.chmod(0o600)
        self.host = self.spawn([str(self.binary), "--background"], "host")
        response = wait_for("host IPC", lambda: self.rpc("get_config"))
        require(response.get("ok"), f"Host startup failed: {response}")
        self.host_id = response["status"]["process_id"]
        require(self.host_id == self.host.pid, "The test did not start its own isolated host")
        status = self.rpc("status")
        require(status.get("ok") and status["status"]["window_control"], f"Window control is unavailable: {status}")
        require(status["status"]["hotkeys"], f"Global shortcuts are unavailable: {status}")
        return response

    def windows(self, *, pid: int | None = None) -> list[int]:
        args = ["xdotool", "search"]
        if pid is None:
            args += ["--class", f"^{re.escape(self.terminal_class)}$"]
        else:
            args += ["--all", "--onlyvisible", "--pid", str(pid), "--name", "^Sean Toolbox$"]
        result = self.command(*args, check=False)
        return [int(line) for line in result.stdout.splitlines() if line.isdigit()]

    def minimized(self, window: int) -> bool:
        properties = self.command("xprop", "-id", str(window), "_NET_WM_STATE", "WM_STATE").stdout
        return "_NET_WM_STATE_HIDDEN" in properties or "window state: Iconic" in properties

    def active(self) -> int:
        result = self.command("xdotool", "getactivewindow", check=False)
        return int(result.stdout.strip()) if result.returncode == 0 else 0

    def key(self, key: str) -> None:
        self.command("xdotool", "key", "--clearmodifiers", key)

    def ui_children(self) -> list[int]:
        result = []
        for pid in children_of(self.host_id):
            try:
                if Path(f"/proc/{pid}/exe").resolve() == self.ui_binary:
                    result.append(pid)
            except (OSError, RuntimeError):
                pass
        return result

    def show_ui(self) -> tuple[int, int, Path]:
        response = self.rpc("show", "diagnostics")
        require(response.get("ok"), f"Show diagnostics failed: {response}")
        try:
            children = wait_for("UI child", self.ui_children)
        except SmokeFailure as error:
            details = {}
            for child in children_of(self.host_id):
                try:
                    details[child] = os.readlink(f"/proc/{child}/exe")
                except OSError as failure:
                    details[child] = str(failure)
            raise SmokeFailure(f"{error}; host children: {details}; expected UI: {self.ui_binary}") from error
        require(len(children) == 1, f"Expected one UI child, got {children}")
        pid = children[0]
        info = process_info(pid)
        require(info, "UI exited before becoming ready")
        self.ui_identities[pid] = int(info["start"])
        arguments = Path(f"/proc/{pid}/cmdline").read_bytes().split(b"\0")
        endpoint_id = arguments[arguments.index(b"--endpoint") + 1].decode()
        endpoint = self.socket_directory / f"{endpoint_id}.sock"
        wait_for("UI activation endpoint", lambda: endpoint.exists())
        acknowledgement = request(endpoint, {"page": "diagnostics"})
        require(acknowledgement.get("ok"), f"UI did not acknowledge diagnostics: {acknowledgement}")
        windows = wait_for("visible diagnostics window", lambda: self.windows(pid=pid))
        require(len(windows) == 1, f"Expected one UI window, got {windows}")
        # The public UI endpoint acknowledges presentation, while configuration
        # loads asynchronously. Allow its first native frames and local IPC load
        # to settle before sending a real user dismiss action.
        time.sleep(1.0)
        return pid, windows[0], endpoint

    def require_ui_reaped(self, pid: int, endpoint: Path) -> None:
        started = self.ui_identities[pid]
        try:
            wait_for("UI exit and child reaping", lambda: not process_exists(pid, started), 8)
        except SmokeFailure as error:
            raise SmokeFailure(f"{error}; remaining process state: {process_info(pid)}") from error
        wait_for("UI socket cleanup", lambda: not endpoint.exists(), 3)
        require(not self.ui_children(), f"Unexpected UI child remains: {self.ui_children()}")
        require(not list(self.socket_directory.glob("sean-toolbox-ui-*.sock")), "Stale UI activation socket remains")

    def measure(self) -> None:
        require(not self.ui_children(), "Resource sample requires no UI process")
        before = process_info(self.host_id)
        require(before and before["state"] != "Z", "Host is not live before resource sample")
        hz = os.sysconf("SC_CLK_TCK")
        started = time.monotonic()
        rss_samples = []
        while True:
            status = Path(f"/proc/{self.host_id}/status").read_text()
            rss = re.search(r"^VmRSS:\s+(\d+)\s+kB$", status, re.MULTILINE)
            require(rss, "Host RSS is unavailable during resource sample")
            rss_samples.append(int(rss.group(1)) / 1024)
            remaining = 10.0 - (time.monotonic() - started)
            if remaining <= 0:
                break
            time.sleep(min(0.2, remaining))
        elapsed = time.monotonic() - started
        after = process_info(self.host_id)
        require(after and after["start"] == before["start"] and after["state"] != "Z", "Host changed during resource sample")
        require(not self.ui_children(), "UI appeared during resource sample")
        cpu = (int(after["ticks"]) - int(before["ticks"])) / hz / elapsed * 100.0
        print(json.dumps({"measurement": {
            "scope": "Linux Xvfb, this host PID only, UI unloaded; not a cross-platform guarantee",
            "binary": str(self.binary), "pid": self.host_id, "seconds": round(elapsed, 3),
            "cpu_percent_of_one_core": round(cpu, 3),
            "rss_mib_min": round(min(rss_samples), 3),
            "rss_mib_mean": round(sum(rss_samples) / len(rss_samples), 3),
            "rss_mib_max": round(max(rss_samples), 3),
            "rss_samples": len(rss_samples),
        }}, ensure_ascii=False), flush=True)

    def run(self, measure: bool) -> None:
        initial = self.start()
        config, revision = initial["config"], initial["revision"]
        self.key("F8")
        windows = wait_for("F8 launching a real xterm", self.windows)
        require(len(windows) == 1, f"F8 created multiple terminals: {windows}")
        terminal = windows[0]
        wait_for("terminal foreground", lambda: self.active() == terminal)
        self.rpc("status")  # Drain the worker result before the next trigger.
        self.key("F8")
        wait_for("second F8 minimizing xterm", lambda: self.minimized(terminal))
        self.rpc("status")
        require(self.windows() == [terminal], "Second F8 created another process/window")
        print("PASS real F8 launch and second-trigger minimize", flush=True)

        original_bytes = self.config_path.read_bytes()
        duplicate = copy.deepcopy(config)
        second = copy.deepcopy(duplicate["profiles"][0])
        second["id"] = "smoke-duplicate"
        duplicate["profiles"].append(second)
        denied = self.rpc("save_config", {"config": duplicate, "expected_revision": revision})
        require(not denied.get("ok"), "Duplicate shortcut configuration was accepted")
        require(self.config_path.read_bytes() == original_bytes, "Rejected duplicate configuration changed the file")
        stale = self.rpc("save_config", {
            "config": config, "expected_revision": revision - 1 if revision else 1,
        })
        require(not stale.get("ok"), "Stale configuration revision was accepted")
        require(self.config_path.read_bytes() == original_bytes, "Rejected stale revision changed the file")
        unchanged = self.rpc("get_config")
        require(unchanged["revision"] == revision and unchanged["config"] == config, "Rejected save changed host state")
        print("PASS duplicate/stale saves rejected without changing file or live configuration", flush=True)

        replacement = copy.deepcopy(config)
        replacement["profiles"][0]["hotkey"] = "F9"
        saved = self.rpc("save_config", {"config": replacement, "expected_revision": revision})
        require(saved.get("ok") and saved["revision"] != revision, f"Valid rebind failed: {saved}")
        require(json.loads(self.config_path.read_text()) == replacement, "Successful save was not persisted")
        try:
            require_key_released(self.environment["DISPLAY"], "F8")
        except SmokeFailure as error:
            # A leaked old grab does not prevent checking the new hotkey and UI
            # lifecycle. Keep testing those, but retain an overall failing exit.
            self.failures.append(str(error))
            print(f"FAIL (continuing independent checks): {error}", file=sys.stderr, flush=True)
        self.key("F8")
        for _ in range(6):
            time.sleep(0.1)
            require(self.minimized(terminal), "Old F8 binding still activates the terminal")
        self.key("F9")
        wait_for("new F9 restoring and activating xterm", lambda: not self.minimized(terminal) and self.active() == terminal)
        self.rpc("status")
        print("PASS F9 activates the terminal and old F8 no longer dispatches an action", flush=True)
        if not self.failures:
            print("PASS old F8 X11 passive grab is released", flush=True)

        second_host = self.command(str(self.binary), "--background")
        require(second_host.returncode == 0, "Second host launch failed")
        require(self.rpc("ping")["status"]["process_id"] == self.host_id, "Second invocation replaced the host PID")
        require(self.host is not None and self.host.poll() is None, "Original host exited during single-instance test")
        print("PASS single-instance invocation preserves host PID", flush=True)

        ui_pid, window, endpoint = self.show_ui()
        self.command("xdotool", "windowactivate", "--sync", str(window))
        self.key("Escape")
        self.require_ui_reaped(ui_pid, endpoint)
        require(self.rpc("ping")["status"]["process_id"] == self.host_id, "Host did not survive UI unload")
        print("PASS diagnostics → Escape → idle=0 exit; no UI zombie/socket; host stays live", flush=True)
        if measure:
            self.measure()

        ui_pid, _, endpoint = self.show_ui()
        stopped = self.rpc("quit")
        require(stopped.get("ok") and stopped.get("message") and stopped.get("status", {}).get("process_id") == self.host_id, f"Quit did not return a complete success response: {stopped}")
        require(self.host is not None and self.host.wait(timeout=8) == 0, "Host failed to exit cleanly")
        self.require_ui_reaped(ui_pid, endpoint)
        wait_for("host socket cleanup", lambda: not self.endpoint.exists(), 3)
        require(not list(self.socket_directory.glob("*.sock")), "IPC sockets remain after Quit")
        print("PASS Quit returns a complete response, exits host and removes its UI/socket", flush=True)

    def dump_logs(self) -> None:
        for name, log in self.logs.items():
            log.flush()
            text = (self.root / f"{name}.log").read_text(errors="replace")[-6000:]
            if text.strip():
                print(f"\n--- {name} log ---\n{text}", file=sys.stderr)

    def close(self) -> None:
        if self.host is not None and self.host.poll() is None:
            try:
                self.rpc("quit")
                self.host.wait(timeout=3)
            except (OSError, SmokeFailure, subprocess.SubprocessError):
                pass
        # Every spawned supervisor has its own process group. Its xterm/UI
        # descendants inherit that group, so only this test's processes are
        # terminated, even after the host itself has exited.
        for process in reversed(self.processes):
            try:
                os.killpg(process.pid, signal.SIGTERM)
            except ProcessLookupError:
                pass
            try:
                process.wait(timeout=2)
            except subprocess.TimeoutExpired:
                try:
                    os.killpg(process.pid, signal.SIGKILL)
                except ProcessLookupError:
                    pass
                process.wait(timeout=2)
        # The subreaper may now own xterm's orphaned descendants. Reap them only
        # during cleanup, so UI lifecycle assertions still require the host to
        # perform its own wait/reaping correctly during the actual test.
        deadline = time.monotonic() + 3.0
        while True:
            try:
                pid, _ = os.waitpid(-1, os.WNOHANG)
            except ChildProcessError:
                break
            if pid:
                continue
            remaining = children_of(os.getpid())
            for child in remaining:
                try:
                    os.kill(child, signal.SIGKILL)
                except ProcessLookupError:
                    pass
            require(time.monotonic() < deadline, f"Test descendants did not exit: {remaining}")
            time.sleep(0.02)
        for log in self.logs.values():
            log.close()


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--binary", type=Path, required=True, help="Built sean-toolbox host, next to sean-toolbox-ui")
    parser.add_argument("--measure", action="store_true", help="Sample idle host RSS and one-core CPU for ten seconds after UI unload")
    options = parser.parse_args()
    require(sys.platform.startswith("linux"), "This isolated acceptance test currently runs on Linux only")
    binary = options.binary.expanduser().resolve(strict=True)
    require(os.access(binary, os.X_OK), f"Host is not executable: {binary}")
    require(os.access(binary.with_name("sean-toolbox-ui"), os.X_OK), "Build sean-toolbox-ui next to the host first")
    for command in ("Xvfb", "openbox", "xterm", "xdotool", "xprop"):
        require(shutil.which(command), f"Missing test dependency: {command}")
    become_child_subreaper()
    def terminate(_signal: int, _frame: Any) -> None:
        raise KeyboardInterrupt
    signal.signal(signal.SIGTERM, terminate)
    with tempfile.TemporaryDirectory(prefix="st-smoke-", dir="/tmp") as directory:
        desktop = Desktop(binary, Path(directory))
        try:
            desktop.run(options.measure)
            require(not desktop.failures, "; ".join(desktop.failures))
        except Exception:
            desktop.dump_logs()
            raise
        finally:
            desktop.close()
    print("PASS host integration smoke; isolated processes and temporary profile cleaned", flush=True)
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
