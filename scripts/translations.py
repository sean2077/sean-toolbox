#!/usr/bin/env python3
"""Update/check the shared gettext catalogs using the pinned Slint extractor.

Run from any directory. `update` writes locales/ui.pot and merges the Chinese
PO without changing existing translations. `check` leaves sources untouched.
Neither command installs tools; normal application builds only need Cargo.
"""

from __future__ import annotations

import argparse
from pathlib import Path
import subprocess
import tempfile
import tomllib


ROOT = Path(__file__).resolve().parents[1]
LOCALES = ROOT / "locales"
CHINESE = LOCALES / "zh-CN" / "LC_MESSAGES" / "toolbox-ui.po"


def run(arguments: list[str]) -> None:
    subprocess.run(arguments, cwd=ROOT, check=True)


def extract(destination: Path) -> None:
    with (ROOT / "crates/toolbox-ui/Cargo.toml").open("rb") as source:
        dependency = tomllib.load(source)["dependencies"]["slint"]["version"]
    if not dependency.startswith("="):
        raise ValueError("Pin Slint to an exact version before extracting translations.")
    expected = dependency.removeprefix("=")
    try:
        installed = subprocess.run(
            ["slint-tr-extractor", "--version"],
            cwd=ROOT,
            capture_output=True,
            text=True,
            check=True,
        ).stdout.strip()
    except FileNotFoundError as error:
        raise ValueError(
            f"Install the matching extractor: cargo install slint-tr-extractor "
            f"--version {expected} --locked"
        ) from error
    if installed != f"slint-tr-extractor {expected}":
        raise ValueError(
            f"Expected slint-tr-extractor {expected}, got {installed!r}. "
            "The extractor must match the application's Slint version."
        )
    files = sorted(
        # Keep candidate paths in the same forward-slash form on every platform:
        # the extractor records them verbatim in the PO "#:" references, so a
        # Windows run must not rewrite the catalogs with backslashes.
        path.relative_to(ROOT).as_posix()
        for path in (ROOT / "crates/toolbox-ui/ui").rglob("*.slint")
    )
    if not files:
        raise ValueError("No Slint sources found.")
    run(
        [
            "slint-tr-extractor",
            "--package-name",
            "toolbox-ui",
            "-o",
            str(destination),
            *files,
        ]
    )


def check(extracted: Path, temporary: Path) -> None:
    # The build script checks IDs, completeness, fuzzy flags, plural forms, and
    # placeholders. The example compares semantic entries, ignoring timestamps
    # and source line numbers emitted by the official extractor.
    run(
        [
            "cargo",
            "run",
            "--locked",
            "--quiet",
            "-p",
            "toolbox-i18n",
            "--example",
            "catalogs",
            "--",
            "compare-ui",
            str(extracted),
        ]
    )
    # Write inside the staging directory: the MSYS2 msgfmt treats a bare
    # "nul" output path as a regular file on Windows.
    run(
        [
            "msgfmt",
            "--check",
            "--check-format",
            "-o",
            str(temporary / "check.mo"),
            str(CHINESE),
        ]
    )
    print("Translation catalogs are complete and match the Slint sources.")


def update(extracted: Path, temporary: Path) -> None:
    combined = temporary / "messages.pot"
    translated = temporary / "toolbox-ui.po"
    run(
        [
            "msgcat",
            "--use-first",
            "--no-wrap",
            "-o",
            str(combined),
            str(extracted),
            str(LOCALES / "rust.pot"),
        ]
    )
    run(
        [
            "msgmerge",
            "--no-fuzzy-matching",
            "--no-wrap",
            "-o",
            str(translated),
            str(CHINESE),
            str(combined),
        ]
    )
    # Stage both outputs before replacing either source. New entries may still
    # be untranslated: fill them in, then use `check` or the normal Cargo gates.
    run(
        [
            "msgfmt",
            "--check",
            "--check-format",
            "-o",
            str(temporary / "update.mo"),
            str(translated),
        ]
    )
    extracted.replace(LOCALES / "ui.pot")
    translated.replace(CHINESE)
    print("Updated ui.pot and merged the Chinese PO. Translate new entries, then run check.")


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("command", choices=("check", "update"))
    args = parser.parse_args()
    try:
        # Keep staged files on the same filesystem for atomic individual writes.
        with tempfile.TemporaryDirectory(prefix=".translations-", dir=LOCALES) as directory:
            temporary = Path(directory)
            extracted = temporary / "ui.pot"
            extract(extracted)
            if args.command == "check":
                check(extracted, temporary)
            else:
                update(extracted, temporary)
    except (OSError, ValueError, subprocess.CalledProcessError) as error:
        parser.exit(1, f"Translation command failed: {error}\n")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
