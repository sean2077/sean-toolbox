#!/usr/bin/env python3
"""Export the SVG app icon with Inkscape and Pillow; not needed for ordinary builds."""

from __future__ import annotations

import argparse
import io
import os
from pathlib import Path
import shutil
import struct
import subprocess
import sys
import tempfile
import xml.etree.ElementTree as ET
import zlib

from PIL import Image


REPO = Path(__file__).resolve().parents[1]
SOURCE = REPO / "packaging/sean-toolbox.svg"
OUTPUT = REPO / "packaging/icons"


def png(size: int, rgba: bytes) -> bytes:
    """Write timestamp-free RGBA PNGs with only the three required chunks."""
    def chunk(kind: bytes, data: bytes) -> bytes:
        return struct.pack(">I", len(data)) + kind + data + struct.pack(">I", zlib.crc32(kind + data))

    stride = size * 4
    scanlines = b"".join(b"\0" + rgba[y * stride:(y + 1) * stride] for y in range(size))
    return (
        b"\x89PNG\r\n\x1a\n"
        + chunk(b"IHDR", struct.pack(">IIBBBBB", size, size, 8, 6, 0, 0, 0))
        + chunk(b"IDAT", zlib.compress(scanlines, 9))
        + chunk(b"IEND", b"")
    )


def render(converter: str, svg: bytes, size: int, environment: dict[str, str]) -> bytes:
    # Render at four times the output resolution, then downsample. Re-encoding
    # the raw pixels avoids renderer metadata and keeps all formats alike.
    result = subprocess.run(
        [converter, "--pipe", "--export-type=png", "--export-filename=-",
         f"--export-width={size * 4}", f"--export-height={size * 4}"],
        input=svg, stdout=subprocess.PIPE, stderr=subprocess.PIPE, check=True, env=environment,
    )
    with Image.open(io.BytesIO(result.stdout)) as rendered:
        return rendered.convert("RGBA").resize((size, size), Image.Resampling.LANCZOS).tobytes()


def template_source(svg: bytes) -> bytes:
    # The menu-bar template is the same toolbox mark with no tile or colors.
    root = ET.fromstring(svg)
    for child in list(root):
        if child.get("id") != "mark":
            root.remove(child)
    for element in root.iter():
        if "stroke" in element.attrib:
            element.set("stroke", "#000000")
    return ET.tostring(root)


def ico(images: dict[int, bytes]) -> bytes:
    sizes = (16, 24, 32, 48, 64, 128, 256)
    offset = 6 + len(sizes) * 16
    directory = bytearray(struct.pack("<HHH", 0, 1, len(sizes)))
    payload = bytearray()
    for size in sizes:
        data = images[size]
        directory += struct.pack("<BBBBHHII", size % 256, size % 256, 0, 0, 1, 32, len(data), offset)
        payload += data
        offset += len(data)
    return bytes(directory + payload)


def icns(images: dict[int, bytes]) -> bytes:
    # PNG payload types understood by the supported macOS 11+ baseline.
    entries = ((b"icp4", 16), (b"icp5", 32), (b"icp6", 64),
               (b"ic07", 128), (b"ic08", 256), (b"ic09", 512), (b"ic10", 1024))
    body = b"".join(kind + struct.pack(">I", len(images[size]) + 8) + images[size]
                    for kind, size in entries)
    return b"icns" + struct.pack(">I", len(body) + 8) + body


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--check", action="store_true", help="regenerate in memory and check committed exports")
    args = parser.parse_args()
    converter = shutil.which("inkscape")
    if converter is None:
        parser.exit(1, "Install Inkscape to regenerate icons; ordinary app builds use the committed exports.\n")
    source = SOURCE.read_bytes()
    with tempfile.TemporaryDirectory(prefix="sean-toolbox-icons-") as temporary:
        environment = dict(os.environ)
        for variable in ("INKSCAPE_PROFILE_DIR", "XDG_CONFIG_HOME", "XDG_DATA_HOME", "XDG_CACHE_HOME"):
            environment[variable] = str(Path(temporary) / variable.lower())
        raw = {size: render(converter, source, size, environment)
               for size in (16, 24, 32, 48, 64, 128, 256, 512, 1024)}
        template = render(converter, template_source(source), 32, environment)
    images = {size: png(size, rgba) for size, rgba in raw.items()}
    exports = {
        "sean-toolbox.png": images[256],
        "sean-toolbox.ico": ico(images),
        "sean-toolbox.icns": icns(images),
        "tray-32.rgba": raw[32],
        "tray-template-32.rgba": template,
    }
    if args.check:
        stale = [name for name, data in exports.items()
                 if not (OUTPUT / name).is_file() or (OUTPUT / name).read_bytes() != data]
        if stale:
            parser.exit(1, "Stale icon exports: " + ", ".join(stale) + "\n")
        print("All icon exports match the SVG source.")
    else:
        OUTPUT.mkdir(parents=True, exist_ok=True)
        for name, data in exports.items():
            (OUTPUT / name).write_bytes(data)
        print(f"Exported {len(exports)} icon assets to {OUTPUT.relative_to(REPO)}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
