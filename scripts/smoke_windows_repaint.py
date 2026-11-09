#!/usr/bin/env python3
"""Check Windows invalidation against the real winit/software rendering surface.

Build with `cargo build --locked -p toolbox-ui --example repaint_probe`, then:
  python scripts/smoke_windows_repaint.py --binary target/debug/examples/repaint_probe.exe

Only the uniquely titled, PID-matched fixture window is touched. No host, user
configuration, external application, or screen capture is used. Each case has a
separate native fixture and a timeout-bounded worker; the parent always reaps it.
The scale matrix uses SLINT_SCALE_FACTOR, not physical multi-monitor DPI changes.
"""

from __future__ import annotations

import argparse
import ctypes
from ctypes import wintypes as wt
import json
import os
from pathlib import Path
import subprocess
import sys
import tempfile
import time
import uuid


SENTINEL = (237, 0, 237)
COLORS = {
    "dark": {"canvas": (23, 25, 31), "sidebar": (18, 20, 25), "footer": (40, 53, 79)},
    "light": {"canvas": (246, 247, 249), "sidebar": (238, 240, 244), "footer": (220, 231, 255)},
}


def require(condition: object, message: str) -> None:
    if not condition:
        raise RuntimeError(message)


def wait_for(predicate, message: str, timeout: float = 3.0):
    deadline = time.monotonic() + timeout
    while time.monotonic() < deadline:
        value = predicate()
        if value:
            return value
        time.sleep(0.02)
    raise RuntimeError(message)


class NativeWindow:
    def __init__(self, pid: int, title: str):
        self.user = ctypes.WinDLL("user32", use_last_error=True)
        self.gdi = ctypes.WinDLL("gdi32", use_last_error=True)
        self.enum_callback = ctypes.WINFUNCTYPE(wt.BOOL, wt.HWND, wt.LPARAM)
        pointer = ctypes.POINTER
        signatures = {
            "SetThreadDpiAwarenessContext": ([ctypes.c_void_p], ctypes.c_void_p),
            "EnumWindows": ([self.enum_callback, wt.LPARAM], wt.BOOL),
            "GetWindowThreadProcessId": ([wt.HWND, pointer(wt.DWORD)], wt.DWORD),
            "GetWindowTextW": ([wt.HWND, wt.LPWSTR, ctypes.c_int], ctypes.c_int),
            "IsWindowVisible": ([wt.HWND], wt.BOOL),
            "IsIconic": ([wt.HWND], wt.BOOL),
            "GetClientRect": ([wt.HWND, pointer(wt.RECT)], wt.BOOL),
            "GetWindowRect": ([wt.HWND, pointer(wt.RECT)], wt.BOOL),
            "GetDC": ([wt.HWND], wt.HDC),
            "ReleaseDC": ([wt.HWND, wt.HDC], ctypes.c_int),
            "FillRect": ([wt.HDC, pointer(wt.RECT), wt.HBRUSH], ctypes.c_int),
            "RedrawWindow": ([wt.HWND, pointer(wt.RECT), wt.HRGN, wt.UINT], wt.BOOL),
            "SetWindowPos": ([wt.HWND, wt.HWND, ctypes.c_int, ctypes.c_int,
                              ctypes.c_int, ctypes.c_int, wt.UINT], wt.BOOL),
            "ShowWindow": ([wt.HWND, ctypes.c_int], wt.BOOL),
        }
        for name, (arguments, result) in signatures.items():
            function = getattr(self.user, name)
            function.argtypes, function.restype = arguments, result
        self.gdi.GetPixel.argtypes, self.gdi.GetPixel.restype = [wt.HDC, ctypes.c_int, ctypes.c_int], wt.DWORD
        self.gdi.CreateSolidBrush.argtypes, self.gdi.CreateSolidBrush.restype = [wt.DWORD], wt.HBRUSH
        self.gdi.DeleteObject.argtypes, self.gdi.DeleteObject.restype = [wt.HGDIOBJ], wt.BOOL
        self.gdi.GdiFlush.argtypes, self.gdi.GdiFlush.restype = [], wt.BOOL
        # All rectangles and pixels below are physical client coordinates.
        require(self.user.SetThreadDpiAwarenessContext(ctypes.c_void_p(-4)),
                "cannot establish per-monitor-aware coordinates")

        def find():
            found = []

            @self.enum_callback
            def visit(hwnd, _):
                owner = wt.DWORD()
                self.user.GetWindowThreadProcessId(hwnd, ctypes.byref(owner))
                if owner.value == pid and self.user.IsWindowVisible(hwnd):
                    caption = ctypes.create_unicode_buffer(256)
                    self.user.GetWindowTextW(hwnd, caption, len(caption))
                    if caption.value == title:
                        found.append(hwnd)
                return True

            require(self.user.EnumWindows(visit, 0), "cannot enumerate fixture windows")
            require(len(found) <= 1, "fixture has multiple matching windows")
            return found[0] if found else None

        self.hwnd = wait_for(find, f"fixture window did not appear for PID {pid}", 8.0)

    def size(self) -> tuple[int, int]:
        rect = wt.RECT()
        require(self.user.GetClientRect(self.hwnd, ctypes.byref(rect)), "GetClientRect failed")
        return rect.right - rect.left, rect.bottom - rect.top

    def pixels(self, points: list[tuple[int, int]]) -> list[tuple[int, int, int]]:
        dc = self.user.GetDC(self.hwnd)
        require(dc, "cannot read the fixture client surface")
        try:
            result = []
            for x, y in points:
                value = self.gdi.GetPixel(dc, x, y)
                require(value != 0xFFFFFFFF, f"GetPixel failed at {(x, y)}")
                result.append((value & 255, value >> 8 & 255, value >> 16 & 255))
            return result
        finally:
            self.user.ReleaseDC(self.hwnd, dc)

    def damage(self, rects: list[wt.RECT]) -> None:
        dc = self.user.GetDC(self.hwnd)
        require(dc, "cannot write the fixture client surface")
        brush = self.gdi.CreateSolidBrush(SENTINEL[0] | SENTINEL[1] << 8 | SENTINEL[2] << 16)
        try:
            require(brush, "CreateSolidBrush failed")
            for rect in rects:
                require(self.user.FillRect(dc, ctypes.byref(rect), brush), "FillRect failed")
            require(self.gdi.GdiFlush(), "GdiFlush failed")
        finally:
            if brush:
                self.gdi.DeleteObject(brush)
            self.user.ReleaseDC(self.hwnd, dc)

    def repaint(self, rect: wt.RECT | None = None) -> None:
        # RDW_INTERNALPAINT has no update rectangle. RDW_INVALIDATE simulates
        # pixels Windows discarded while Slint's own buffer remains unchanged.
        flags = 0x0100 | (0x0001 if rect is not None else 0x0002)  # RDW_UPDATENOW
        require(self.user.RedrawWindow(self.hwnd, ctypes.byref(rect) if rect else None, None, flags),
                "RedrawWindow failed")

    def grow(self, width: int, height: int) -> None:
        client_width, client_height = self.size()
        outer = wt.RECT()
        require(self.user.GetWindowRect(self.hwnd, ctypes.byref(outer)), "GetWindowRect failed")
        width += outer.right - outer.left - client_width
        height += outer.bottom - outer.top - client_height
        require(self.user.SetWindowPos(self.hwnd, None, 0, 0, width, height, 0x0002 | 0x0004 | 0x0010),
                "SetWindowPos failed")  # SWP_NOMOVE | SWP_NOZORDER | SWP_NOACTIVATE


def sample_points(rect: wt.RECT) -> list[tuple[int, int]]:
    # Include the exact physical edges: fractional scaling must round outwards.
    xs = (rect.left, (rect.left + rect.right) // 2, rect.right - 1)
    ys = (rect.top, (rect.top + rect.bottom) // 2, rect.bottom - 1)
    return [(x, y) for x in xs for y in ys]


def expect_pixels(window: NativeWindow, points, expected, message: str) -> None:
    actual = window.pixels(points)
    require(actual == expected, f"{message}; points={points}, expected={expected}, actual={actual}")


def verify_case(window: NativeWindow, scale: float, theme: str, phase: str) -> dict:
    width, height = window.size()
    colors = COLORS[theme]
    points = [(int(24 * scale), height - int(12 * scale)),
              (width // 2, height // 2), (width // 2, height - int(12 * scale)),
              (width - 6, height // 2), (width - 6, height - 6)]
    expected = [colors["sidebar"], colors["canvas"], colors["footer"],
                colors["canvas"], colors["footer"]]
    try:
        wait_for(lambda: window.pixels(points) == expected, f"{phase}: incomplete initial frame")
    except RuntimeError:
        expect_pixels(window, points, expected, f"{phase}: incomplete initial frame; size={(width, height)}")
        raise
    # Drain a normal redraw without changing a Slint property or window size.
    window.repaint()
    bottom = wt.RECT(4, height - 23, width - 4, height - 4)
    right = wt.RECT(width - 23, 5, width - 4, height - 29)
    bottom_points, right_points = sample_points(bottom), sample_points(right)
    original_bottom, original_right = window.pixels(bottom_points), window.pixels(right_points)
    window.damage([bottom, right])
    sentinel = [SENTINEL] * len(bottom_points)
    expect_pixels(window, bottom_points + right_points, sentinel * 2,
                  f"{phase}: native sentinel write did not reach the client surface")
    window.repaint()
    expect_pixels(window, bottom_points + right_points, sentinel * 2,
                  f"{phase}: internal redraw repainted unchanged pixels; fixture was not idle")
    window.repaint(bottom)
    actual = window.pixels(bottom_points)
    require(actual == original_bottom,
            f"{phase}: bottom invalidation was not repainted; size={(width, height)}, "
            f"points={bottom_points}, expected={original_bottom}, actual={actual}")
    expect_pixels(window, right_points, sentinel,
                  f"{phase}: partial invalidation unexpectedly repainted the unrelated right region")
    window.repaint(right)
    actual = window.pixels(right_points)
    require(actual == original_right,
            f"{phase}: right invalidation was not repainted; size={(width, height)}, "
            f"points={right_points}, expected={original_right}, actual={actual}")
    return {"phase": phase, "client_size": [width, height],
            "bottom_points": bottom_points, "right_points": right_points}


def worker(args) -> None:
    window = NativeWindow(args.pid, args.title)
    initial = (round(320 * args.scale), round(220 * args.scale))
    wait_for(lambda: window.size() == initial,
             f"initial client size did not become {initial}; scale={args.scale}")
    checks = [verify_case(window, args.scale, args.theme, "initial")]
    grown = (round(380 * args.scale), round(260 * args.scale))
    window.grow(*grown)
    wait_for(lambda: window.size() == grown, f"grown client size did not become {grown}")
    checks.append(verify_case(window, args.scale, args.theme, "grown"))
    window.user.ShowWindow(window.hwnd, 6)  # SW_MINIMIZE
    wait_for(lambda: window.user.IsIconic(window.hwnd), "fixture did not minimize")
    window.user.ShowWindow(window.hwnd, 9)  # SW_RESTORE
    wait_for(lambda: not window.user.IsIconic(window.hwnd), "fixture did not restore")
    checks.append(verify_case(window, args.scale, args.theme, "restored"))
    print(json.dumps({"theme": args.theme, "scale": args.scale, "checks": checks}))


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--binary", type=Path, default=Path("target/debug/examples/repaint_probe.exe"))
    parser.add_argument("--scales", nargs="+", type=float, choices=(1.0, 1.5, 2.0), default=[1.0, 1.5, 2.0])
    parser.add_argument("--themes", nargs="+", choices=("light", "dark"), default=["light", "dark"])
    parser.add_argument("--worker", action="store_true", help=argparse.SUPPRESS)
    parser.add_argument("--pid", type=int, help=argparse.SUPPRESS)
    parser.add_argument("--title", help=argparse.SUPPRESS)
    parser.add_argument("--scale", type=float, help=argparse.SUPPRESS)
    parser.add_argument("--theme", help=argparse.SUPPRESS)
    args = parser.parse_args()
    require(sys.platform == "win32", "unsupported platform: this test requires a Windows desktop")
    if args.worker:
        worker(args)
        return
    binary = args.binary.resolve()
    require(binary.is_file(), f"missing fixture: {binary}; build the repaint_probe example first")
    results = []
    for theme in args.themes:
        for scale in args.scales:
            title = f"Sean Toolbox repaint probe {uuid.uuid4().hex}"
            environment = {**os.environ, "SLINT_SCALE_FACTOR": str(scale)}
            with tempfile.TemporaryFile() as log:
                process = subprocess.Popen([str(binary), theme, title], env=environment,
                                           stdin=subprocess.DEVNULL, stdout=log, stderr=log)
                try:
                    completed = subprocess.run(
                        [sys.executable, str(Path(__file__).resolve()), "--worker",
                         "--pid", str(process.pid), "--title", title,
                         "--scale", str(scale), "--theme", theme],
                        capture_output=True, text=True, timeout=25, check=False,
                    )
                    log.seek(0)
                    native_log = log.read().decode("utf-8", errors="replace")
                    require(completed.returncode == 0,
                            f"Windows repaint failed ({theme}, scale={scale}):\n"
                            f"{completed.stdout}{completed.stderr}{native_log}")
                    require(process.poll() is None, f"fixture exited early ({theme}, scale={scale}): {native_log}")
                    results.append(json.loads(completed.stdout))
                except subprocess.TimeoutExpired as error:
                    raise RuntimeError(f"Windows repaint timed out ({theme}, scale={scale})") from error
                finally:
                    if process.poll() is None:
                        process.terminate()
                    try:
                        process.wait(timeout=5)
                    except subprocess.TimeoutExpired:
                        process.kill()
                        process.wait(timeout=5)
    print(json.dumps({"renderer": "native winit/software", "cases": results}, indent=2))
    print(f"PASS: {len(results)} native repaint cases; initial, grown and restored windows")


if __name__ == "__main__":
    try:
        main()
    except (OSError, RuntimeError, subprocess.SubprocessError) as error:
        print(f"FAIL: {error}", file=sys.stderr)
        sys.exit(1)
