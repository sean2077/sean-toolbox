#!/usr/bin/env python3
"""Check native Windows repaint sensitivity, then run the complete fixed matrix.

Run from a clean CI checkout with `python scripts/check_windows_repaint.py`.
This temporarily removes the upstream repaint call, builds the static probe,
and requires the initial bottom-invalidation check to fail. Original source
bytes are restored before rebuilding and running all six patched cases.
"""

from __future__ import annotations

import hashlib
import json
from pathlib import Path
import re
import subprocess
import sys


ROOT = Path(__file__).resolve().parents[1]
ADAPTER = Path("vendor/i-slint-backend-winit/winitwindowadapter.rs")
REPAINT_CALL = (
    b'        #[cfg(target_os = "windows")]\n'
    b"        self.mark_windows_update_region_dirty();\n\n"
)
EXPECTED_PAIRS = {(theme, scale) for theme in ("light", "dark") for scale in (1.0, 1.5, 2.0)}
EXPECTED_PHASES = ["initial", "grown", "restored"]
EXPECTED_FAILURE = re.compile(
    r"FAIL: initial: bottom invalidation was not repainted; "
    r"size=\(\d+, \d+\), points=\[.+\], expected=\[.+\], actual=\[.+\]"
)


def require(condition: object, message: str) -> None:
    if not condition:
        raise RuntimeError(message)


def build_probe(label: str) -> None:
    print(f"=== BUILD {label} ===", flush=True)
    subprocess.run(
        ["cargo", "build", "--locked", "-p", "toolbox-ui", "--example", "repaint_probe"],
        cwd=ROOT, check=True,
    )


def run_probe(label: str, arguments: list[str]) -> subprocess.CompletedProcess[str]:
    print(f"=== RUN {label} ===", flush=True)
    result = subprocess.run(
        [sys.executable, "scripts/smoke_windows_repaint.py", *arguments],
        cwd=ROOT, capture_output=True, text=True, encoding="utf-8", errors="replace", check=False,
    )
    for stream, output in (("stdout", result.stdout), ("stderr", result.stderr)):
        print(f"=== BEGIN {label} {stream} ===", flush=True)
        print(output, end="" if output.endswith("\n") else "\n", flush=True)
        print(f"=== END {label} {stream} ===", flush=True)
    print(f"=== EXIT {label}: {result.returncode} ===", flush=True)
    return result


def check_negative(result: subprocess.CompletedProcess[str]) -> None:
    require(result.returncode == 1,
            f"unpatched probe must exit 1, received {result.returncode}")
    failures = [line for line in result.stderr.splitlines() if line.startswith("FAIL: ")]
    require(
        len(failures) == 2
        and failures[0] == "FAIL: Windows repaint failed (light, scale=1.0):"
        and EXPECTED_FAILURE.fullmatch(failures[1]) is not None,
        f"unpatched probe did not fail at initial bottom invalidation: {failures!r}",
    )


def check_positive(result: subprocess.CompletedProcess[str]) -> None:
    require(result.returncode == 0,
            f"patched matrix must exit 0, received {result.returncode}")
    report, _ = json.JSONDecoder().raw_decode(result.stdout.lstrip())
    require(isinstance(report, dict) and report.get("renderer") == "native winit/software",
            "patched matrix did not report the native winit/software renderer")
    cases = report.get("cases")
    require(isinstance(cases, list) and len(cases) == len(EXPECTED_PAIRS),
            "patched matrix must report exactly six cases")
    pairs = []
    for case in cases:
        require(isinstance(case, dict), "patched matrix contains an invalid case")
        theme, scale = case.get("theme"), case.get("scale")
        require(theme in ("light", "dark") and type(scale) in (int, float)
                and scale in (1.0, 1.5, 2.0),
                f"patched matrix contains an unexpected case: {theme!r}, {scale!r}")
        pairs.append((theme, scale))
        require(case.get("resize_dispatch") == "async",
                f"patched case {theme}/{scale} did not use owner-thread resize dispatch")
        checks = case.get("checks")
        require(isinstance(checks, list), f"patched case {theme}/{scale} has no phase results")
        phases = [check.get("phase") if isinstance(check, dict) else None for check in checks]
        require(phases == EXPECTED_PHASES,
                f"patched case {theme}/{scale} has unexpected phases: {phases!r}")
    require(set(pairs) == EXPECTED_PAIRS, f"patched matrix has missing or duplicate cases: {pairs!r}")


def main() -> None:
    require(sys.platform == "win32", "this check requires a Windows desktop")
    subprocess.run(
        ["git", "diff", "--exit-code", "HEAD", "--", ADAPTER.as_posix(), "Cargo.lock"],
        cwd=ROOT, check=True,
    )
    adapter = ROOT / ADAPTER
    original = adapter.read_bytes()
    checksum = hashlib.sha256(original).hexdigest()
    call = REPAINT_CALL.replace(b"\n", b"\r\n") if b"\r\n" in original else REPAINT_CALL
    require(original.count(call) == 1, "expected exactly one unchanged upstream repaint call")
    errors = []
    try:
        adapter.write_bytes(original.replace(call, b"", 1))
        build_probe("unpatched")
        check_negative(run_probe("unpatched light/1.0", ["--themes", "light", "--scales", "1.0"]))
        print("PASS: unpatched probe failed at initial bottom invalidation", flush=True)
    except (OSError, RuntimeError, subprocess.SubprocessError) as error:
        errors.append(f"negative control: {error}")
        print(f"FAIL: {errors[-1]}", file=sys.stderr, flush=True)
    finally:
        # Restoration must complete before any patched build or later packaging.
        adapter.write_bytes(original)
        require(adapter.read_bytes() == original, "could not restore original adapter bytes")
        print(f"RESTORED: {ADAPTER.as_posix()} SHA-256 {checksum}", flush=True)

    # Run the full positive matrix even if the negative control failed unexpectedly.
    try:
        build_probe("patched")
        check_positive(run_probe("patched complete matrix", []))
    except (OSError, RuntimeError, ValueError, subprocess.SubprocessError) as error:
        errors.append(f"positive matrix: {error}")
        print(f"FAIL: {errors[-1]}", file=sys.stderr, flush=True)

    require(adapter.read_bytes() == original, "adapter bytes changed after restoration")
    subprocess.run(
        ["git", "diff", "--exit-code", "--", ADAPTER.as_posix(), "Cargo.lock"],
        cwd=ROOT, check=True,
    )
    require(not errors, "; ".join(errors))
    print("PASS: native regression sensitivity and six patched cases / eighteen phases", flush=True)


if __name__ == "__main__":
    try:
        main()
    except (OSError, RuntimeError, subprocess.SubprocessError) as error:
        print(f"FAIL: {error}", file=sys.stderr)
        sys.exit(1)
