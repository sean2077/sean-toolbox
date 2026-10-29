#!/usr/bin/env python3
"""Collect local license texts for Cargo's locked dependencies; never downloads files."""

from __future__ import annotations

import argparse
import hashlib
from pathlib import Path
import json
import re
import subprocess
import sys


REPO = Path(__file__).resolve().parents[1]
OVERRIDES = REPO / "licenses/dependencies/manifest.json"


def license_files(package: dict, workspace: Path, workspace_members: set[str]) -> list[Path]:
    manifest_dir = Path(package["manifest_path"]).resolve().parent
    allowed = workspace if package["id"] in workspace_members else manifest_dir
    candidates: list[Path] = []
    if package.get("license_file"):
        candidates.append(manifest_dir / package["license_file"])
    for path in sorted(manifest_dir.iterdir()):
        if path.name.upper().startswith(("LICENSE", "LICENCE", "COPYING", "UNLICENSE", "UNLICENCE", "NOTICE", "COPYRIGHT")):
            if path.is_dir():
                candidates.extend(sorted(path.rglob("*")))
            else:
                candidates.append(path)
    if package["id"] in workspace_members:
        candidates.extend(workspace.glob("LICENSE*"))
    found = []
    seen = set()
    for path in candidates:
        resolved = path.resolve()
        # A dependency's manifest must not make the collector copy unrelated host files.
        if not resolved.is_relative_to(allowed) or not resolved.is_file() or resolved in seen:
            continue
        seen.add(resolved)
        found.append(resolved)
    return found


def collect(metadata: dict, output: Path, *, target: str = "", overrides_path: Path = OVERRIDES) -> list[str]:
    workspace = Path(metadata["workspace_root"]).resolve()
    workspace_members = set(metadata["workspace_members"])
    overrides = {}
    if overrides_path.is_file():
        document = json.loads(overrides_path.read_text(encoding="utf-8"))
        if document.get("schema_version") != 1 or not isinstance(document.get("packages"), dict):
            raise ValueError("Invalid license override manifest")
        overrides = document["packages"]
    resolved = {node["id"] for node in metadata["resolve"]["nodes"]}
    packages = sorted(
        (package for package in metadata["packages"] if package["id"] in resolved),
        key=lambda package: (package["name"].lower(), package["version"], package["id"]),
    )
    sections = [
        "Sean Toolbox — dependency license texts\n"
        "Collected from the local Cargo cache with --locked --offline.\n"
        f"Target platform: {target or 'unspecified'}\n"
        "The original declared license expressions are retained below.\n"
        "See THIRD_PARTY_NOTICES.md for Sean Toolbox's selected Slint license.\n"
        "This inventory includes resolved build and test dependencies as well.\n"
    ]
    missing = []
    for package in packages:
        name = f"{package['name']} {package['version']}"
        source = package.get("repository") or package.get("homepage") or ""
        if not source and (package.get("source") or "").startswith("registry+"):
            source = f"https://crates.io/crates/{package['name']}/{package['version']}"
        if not source:
            source = package.get("source") or "Workspace package"
        authors = "; ".join(package.get("authors") or []) or "Not specified in the published manifest"
        sections.append(f"\n{'=' * 78}\n{name}\nDeclared license: {package.get('license') or 'not declared'}\nSource: {source}\nUpstream authors: {authors}\n")
        manifest_dir = Path(package["manifest_path"]).resolve().parent
        texts = 0
        for path in license_files(package, workspace, workspace_members):
            try:
                # License files are plain text. Binary/unreadable candidates are reported.
                content = path.read_text(encoding="utf-8-sig")
            except (UnicodeError, OSError) as error:
                sections.append(f"[Could not read license file {path.name}: {error}]\n")
                continue
            if not content.strip():
                continue
            label = path.relative_to(manifest_dir) if path.is_relative_to(manifest_dir) else path.relative_to(workspace)
            sections.append(f"\n--- {label.as_posix()} ---\n{content.rstrip()}\n")
            if not path.name.upper().startswith(("NOTICE", "COPYRIGHT")):
                texts += 1
        for entry in overrides.get(f"{package['name']}@{package['version']}", []):
            if entry["license"] not in re.findall(r"[A-Za-z0-9.+-]+", package.get("license") or ""):
                raise ValueError(f"Override selects an undeclared license for {name}")
            path = (overrides_path.parent / entry["file"]).resolve()
            if not path.is_relative_to(overrides_path.parent.resolve()):
                raise ValueError(f"License override escapes its directory: {entry['file']}")
            data = path.read_bytes()
            if hashlib.sha256(data).hexdigest() != entry["sha256"]:
                raise ValueError(f"License override checksum mismatch: {entry['file']}")
            content = data.decode("utf-8-sig")
            if not content.strip():
                raise ValueError(f"License override is empty: {entry['file']}")
            sections.append(f"\n--- Upstream license supplement: {entry['file']} ---\n"
                            f"Retrieved from: {entry['source']}\n"
                            f"Selected license: {entry['license']}\n"
                            f"Supplement type: {entry.get('kind', 'upstream_license_text')}\n"
                            f"{entry.get('note', '')}\n\n{content.rstrip()}\n")
            if entry.get("kind") != "upstream_license_declaration":
                texts += 1
        if not texts:
            missing.append(f"{name}: {package.get('license') or 'no declared license'}")
            sections.append("[MISSING LICENSE TEXT: no readable LICENSE/COPYING/license_file was packaged upstream.]\n")

    output.parent.mkdir(parents=True, exist_ok=True)
    output.write_text("\n".join(sections), encoding="utf-8")
    print(f"Collected {len(packages)} packages into {output}")
    return missing


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--output", type=Path, default=REPO / "target/THIRD_PARTY_LICENSES.txt")
    parser.add_argument("--target", help="Cargo target triple; defaults to the native rustc host")
    arguments = parser.parse_args()
    try:
        target = arguments.target
        if target is None:
            version = subprocess.run(["rustc", "-vV"], cwd=REPO, check=True, capture_output=True, text=True)
            target = next(line.removeprefix("host: ") for line in version.stdout.splitlines() if line.startswith("host: "))
        result = subprocess.run(
            ["cargo", "metadata", "--locked", "--offline", "--format-version", "1", "--filter-platform", target],
            cwd=REPO, check=True, capture_output=True, text=True,
        )
        missing = collect(json.loads(result.stdout), arguments.output.resolve(), target=target)
    except (OSError, ValueError, KeyError, StopIteration, subprocess.CalledProcessError) as error:
        if isinstance(error, subprocess.CalledProcessError):
            print(error.stderr.strip(), file=sys.stderr)
        parser.exit(1, f"License collection failed: {error}\n")
    if missing:
        print("Missing local license texts (release packaging must resolve these):", file=sys.stderr)
        for package in missing:
            print(f"  - {package}", file=sys.stderr)
        return 1
    return 0


if __name__ == "__main__":
    sys.exit(main())
