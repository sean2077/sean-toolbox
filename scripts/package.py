#!/usr/bin/env python3
"""Package native release binaries and documentation. Requires Python 3.11+."""

from __future__ import annotations

import argparse
import hashlib
import os
from pathlib import Path
import platform
import plistlib
import shutil
import sys
import tarfile
import tempfile
import tomllib
import zipfile


REPO = Path(__file__).resolve().parents[1]


def copy_file(source: Path, destination: Path, *, executable: bool = False) -> None:
    destination.parent.mkdir(parents=True, exist_ok=True)
    shutil.copy2(source, destination)
    destination.chmod(0o755 if executable else 0o644)


def package(system: str, architecture: str, binary_dir: Path, output: Path) -> Path:
    with (REPO / "Cargo.toml").open("rb") as manifest:
        version = tomllib.load(manifest)["workspace"]["package"]["version"]
    if os.environ.get("GITHUB_REF_TYPE") == "tag":
        expected = f"v{version}"
        if os.environ.get("GITHUB_REF_NAME") != expected:
            raise ValueError(f"Tag must match Cargo.toml version: expected {expected}")

    suffix = ".exe" if system == "windows" else ""
    binaries = [f"sean-toolbox{suffix}", f"sean-toolbox-ui{suffix}"]
    for binary in binaries:
        if not (binary_dir / binary).is_file():
            raise FileNotFoundError(f"Missing {binary_dir / binary}; build both release binaries first")
    license_inventory = REPO / "target/THIRD_PARTY_LICENSES.txt"
    if "[MISSING LICENSE TEXT:" in license_inventory.read_text(encoding="utf-8"):
        raise ValueError("Dependency license collection is incomplete; resolve the reported packages before packaging")

    package_name = f"sean-toolbox-{version}-{system}-{architecture}"
    output.mkdir(parents=True, exist_ok=True)
    with tempfile.TemporaryDirectory(prefix=".package-", dir=output) as temporary:
        root = Path(temporary) / package_name
        root.mkdir()
        for document in ("README.md", "LICENSE", "THIRD_PARTY_NOTICES.md"):
            copy_file(REPO / document, root / document)
        copy_file(REPO / "crates/toolbox-projects/LICENSE.recentry", root / "LICENSE.recentry")
        for document in ("USAGE.md", "VALIDATION.md"):
            copy_file(REPO / "docs" / document, root / "docs" / document)
        shutil.copytree(REPO / "docs/images", root / "docs/images")
        copy_file(license_inventory, root / "THIRD_PARTY_LICENSES.txt")
        if (REPO / "licenses").is_dir():
            shutil.copytree(REPO / "licenses", root / "licenses")

        if system == "macos":
            app = root / "Sean Toolbox.app"
            binary_target = app / "Contents/MacOS"
            with (REPO / "packaging/macos/Info.plist").open("rb") as source:
                info = plistlib.load(source)
            info["CFBundleShortVersionString"] = version.split("-")[0]
            info["CFBundleVersion"] = version.split("-")[0]
            info["SeanToolboxVersion"] = version
            (app / "Contents").mkdir(parents=True)
            with (app / "Contents/Info.plist").open("wb") as target:
                plistlib.dump(info, target, sort_keys=False)
            copy_file(REPO / "packaging/icons/sean-toolbox.icns", app / "Contents/Resources/sean-toolbox.icns")
            copy_file(REPO / "packaging/macos/README.md", root / "INSTALL.md")
        elif system == "linux":
            binary_target = root / "bin"
            copy_file(REPO / "packaging/linux/sean-toolbox.desktop", root / "share/applications/sean-toolbox.desktop")
            copy_file(REPO / "packaging/sean-toolbox.svg", root / "share/icons/hicolor/scalable/apps/sean-toolbox.svg")
            copy_file(REPO / "packaging/icons/sean-toolbox.png", root / "share/icons/hicolor/256x256/apps/sean-toolbox.png")
            copy_file(REPO / "packaging/linux/README.md", root / "INSTALL.md")
        else:
            binary_target = root
            copy_file(REPO / "packaging/windows/README.md", root / "INSTALL.md")
        for binary in binaries:
            copy_file(binary_dir / binary, binary_target / binary, executable=True)

        if system == "linux":
            archive = output / f"{package_name}.tar.gz"
            with tarfile.open(archive, "w:gz") as target:
                target.add(root, arcname=package_name)
        else:
            archive = output / f"{package_name}.zip"
            with zipfile.ZipFile(archive, "w", compression=zipfile.ZIP_DEFLATED, compresslevel=9) as target:
                for path in sorted(root.rglob("*")):
                    target.write(path, arcname=path.relative_to(root.parent))

    with archive.open("rb") as content:
        digest = hashlib.file_digest(content, "sha256").hexdigest()
    archive.with_name(archive.name + ".sha256").write_text(f"{digest}  {archive.name}\n", encoding="utf-8")
    return archive


def main() -> int:
    system = {"Windows": "windows", "Darwin": "macos", "Linux": "linux"}.get(platform.system())
    architecture = {"AMD64": "x86_64", "x86_64": "x86_64", "arm64": "aarch64", "aarch64": "aarch64"}.get(platform.machine())
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--platform", choices=("windows", "macos", "linux"), default=system)
    parser.add_argument("--arch", choices=("x86_64", "aarch64"), default=architecture)
    parser.add_argument("--binary-dir", type=Path, default=REPO / "target/release")
    parser.add_argument("--out-dir", type=Path, default=REPO / "dist")
    arguments = parser.parse_args()
    if arguments.platform is None or arguments.arch is None:
        parser.error("Specify --platform and --arch for this build")
    try:
        archive = package(arguments.platform, arguments.arch, arguments.binary_dir.resolve(), arguments.out_dir.resolve())
    except (OSError, ValueError, KeyError) as error:
        parser.exit(1, f"Packaging failed: {error}\n")
    print(archive)
    return 0


if __name__ == "__main__":
    sys.exit(main())
