#!/usr/bin/env python3
"""Render production Slint components with example data, without a native desktop.

Requires the normal Rust/Slint build environment. Uses only Python's standard
library. These layout previews do not exercise the host, IPC, native window
focus, file dialogs, or the user's installed applications and Skills.
"""

from __future__ import annotations

import argparse
import itertools
import json
from pathlib import Path
import struct
import subprocess
import sys
import tempfile
import zlib


ROOT = Path(__file__).resolve().parents[1]
SCENES = (
    "projects", "apps", "apps-advanced", "quake", "settings", "status", "skills",
    "skills-discover", "skills-local", "skills-repositories", "launcher", "launcher-details",
    "skills-details", "skills-problem", "skills-linked", "skills-import", "skills-selected", "skills-activity",
    "import", "confirm",
)


def size(value: str) -> tuple[int, int]:
    try:
        width, height = map(int, value.lower().split("x"))
        if not (240 <= width <= 4096 and 200 <= height <= 4096):
            raise ValueError
        return width, height
    except ValueError as error:
        raise argparse.ArgumentTypeError("use WIDTHxHEIGHT, from 240x200 to 4096x4096") from error


def png_from_ppm(source: Path, destination: Path) -> tuple[int, int]:
    with source.open("rb") as stream:
        if stream.readline() != b"P6\n":
            raise ValueError("capture is not a binary RGB PPM")
        width, height = map(int, stream.readline().split())
        if stream.readline() != b"255\n":
            raise ValueError("capture is not 8-bit RGB")
        pixels = stream.read()
    row_bytes = width * 3
    if len(pixels) != row_bytes * height:
        raise ValueError("capture pixel count does not match its dimensions")

    def chunk(kind: bytes, data: bytes) -> bytes:
        return struct.pack(">I", len(data)) + kind + data + struct.pack(">I", zlib.crc32(kind + data))

    rows = b"".join(b"\0" + pixels[start:start + row_bytes]
                    for start in range(0, len(pixels), row_bytes))
    destination.write_bytes(
        b"\x89PNG\r\n\x1a\n"
        + chunk(b"IHDR", struct.pack(">IIBBBBB", width, height, 8, 2, 0, 0, 0))
        + chunk(b"IDAT", zlib.compress(rows))
        + chunk(b"IEND", b"")
    )
    return width, height


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--output", type=Path, default=ROOT / "target" / "ui-previews")
    parser.add_argument("--scenes", nargs="+", choices=SCENES, default=list(SCENES))
    parser.add_argument("--languages", nargs="+", choices=("en", "zh-CN"), default=["en", "zh-CN"])
    parser.add_argument("--themes", nargs="+", choices=("light", "dark"), default=["light", "dark"])
    parser.add_argument("--sizes", nargs="+", type=size, default=[(1120, 780), (800, 580)])
    parser.add_argument("--scales", nargs="+", type=float, choices=(1.0, 1.5, 2.0), default=[1.0])
    parser.add_argument("--font-family", default="", help="installed or registered font family")
    parser.add_argument("--font-file", type=Path, help="register this local font for the capture only")
    parser.add_argument("--offline", action="store_true", help="use Cargo's existing dependency cache")
    parser.add_argument("--allow-unsupported-size", action="store_true",
                        help="explicitly render outside Slint's declared size limits for comparison")
    parser.add_argument("--verify-appearance-controls", action="store_true",
                        help="verify native-smoke control points using real Slint pointer events")
    args = parser.parse_args()
    points = {}
    if args.verify_appearance_controls:
        from smoke_appearance import APPEARANCE_POINTS, MANAGER_SIZE
        if not {"apps", "settings"}.issubset(args.scenes) or "en" not in args.languages or MANAGER_SIZE not in args.sizes:
            parser.error("control verification needs --scenes apps settings --languages en --sizes 1120x780")
        points = APPEARANCE_POINTS
    output = args.output.resolve()
    output.mkdir(parents=True, exist_ok=True)
    if args.font_file and not args.font_file.is_file():
        parser.error("--font-file must point to an existing font")
    cargo_flags = ["--locked"] + (["--offline"] if args.offline else [])
    subprocess.run(["cargo", "build", *cargo_flags, "-p", "toolbox-ui", "--example", "render_ui"],
                   cwd=ROOT, check=True)
    metadata = json.loads(subprocess.check_output(
        ["cargo", "metadata", *cargo_flags, "--no-deps", "--format-version", "1"], cwd=ROOT))
    executable = Path(metadata["target_directory"]) / "debug" / "examples" / (
        "render_ui.exe" if sys.platform == "win32" else "render_ui")
    captures = []
    seen = set()
    with tempfile.TemporaryDirectory(prefix="sean-toolbox-layout-") as temporary:
        ppm = Path(temporary) / "frame.ppm"
        combinations = itertools.product(args.scenes, args.languages, args.themes, args.sizes, args.scales)
        for scene, language, theme, (width, height), scale in combinations:
            if scene.startswith("launcher"):
                width, height = min(width, 760), min(height, 432)
            key = (scene, language, theme, width, height, scale)
            if key in seen:
                continue
            seen.add(key)
            filename = f"{scene}-{language}-{theme}-{width}x{height}-s{scale:g}.png"
            geometry = json.loads(subprocess.check_output([
                str(executable), str(ppm), scene, language, theme, str(width), str(height), str(scale),
                args.font_family, str(args.font_file.resolve()) if args.font_file else "",
                str(args.allow_unsupported_size).lower(),
                json.dumps(points) if points and scene in ("apps", "settings") and language == "en" and (width, height) == (1120, 780) else "",
            ], cwd=ROOT, text=True))
            physical = png_from_ppm(ppm, output / filename)
            if list(physical) != geometry["window_physical_size"]:
                raise ValueError("capture dimensions do not match the software window")
            captures.append({
                "file": filename, "scene": scene, "language": language, "theme": theme,
                "logical_size": [width, height], "scale": scale, "physical_size": physical,
                "geometry": geometry,
            })
    revision = subprocess.check_output(["git", "rev-parse", "HEAD"], cwd=ROOT, text=True).strip()
    modified = bool(subprocess.check_output(["git", "status", "--porcelain"], cwd=ROOT, text=True))
    (output / "manifest.json").write_text(json.dumps({
        "revision": revision, "modified_worktree": modified, "example_data": True,
        "renderer": "Slint software renderer; no native window or host",
        "font_family": args.font_family or "platform default",
        "font_file": str(args.font_file.resolve()) if args.font_file else None,
        "captures": captures,
    }, ensure_ascii=False, indent=2) + "\n", encoding="utf-8")
    print(f"Rendered {len(captures)} example-data previews to {output}")


if __name__ == "__main__":
    main()
