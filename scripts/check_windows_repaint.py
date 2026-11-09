#!/usr/bin/env python3
"""Check native Windows repaint sensitivity, then run the complete fixed matrix.

Run from a clean CI checkout with `python scripts/check_windows_repaint.py`.
The negative control builds an unpatched copy of the vendored backend in a
temporary overlay, so the tracked source is never modified. It requires the
initial bottom-invalidation check to fail, then rebuilds the repository source
and runs all six patched cases.
"""

from __future__ import annotations

import json
from pathlib import Path
import re
import shutil
import subprocess
import sys
import tempfile


ROOT = Path(__file__).resolve().parents[1]
VENDOR = Path("vendor/i-slint-backend-winit")
ADAPTER = VENDOR / "winitwindowadapter.rs"
CLEAN_PATHS = (VENDOR.as_posix(), "Cargo.toml", "Cargo.lock")
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


def verify_clean() -> None:
    subprocess.run(
        ["git", "diff", "--exit-code", "HEAD", "--", *CLEAN_PATHS],
        cwd=ROOT, check=True,
    )


def build_probe(
    label: str, *, config: str | None = None, overlay: Path | None = None
) -> Path:
    print(f"=== BUILD {label} ===", flush=True)
    command = ["cargo", "build", "--locked"]
    if config is not None:
        command += ["--config", config]
    command += ["--message-format=json", "-p", "toolbox-ui", "--example", "repaint_probe"]
    result = subprocess.run(
        command,
        cwd=ROOT, check=True, stdout=subprocess.PIPE,
        text=True, encoding="utf-8", errors="replace",
    )
    executables = []
    backend_artifacts = []
    overlay_root = overlay.resolve() if overlay is not None else None
    for line in result.stdout.splitlines():
        try:
            message = json.loads(line)
        except json.JSONDecodeError:
            continue
        if message.get("reason") != "compiler-artifact":
            continue
        target = message.get("target")
        manifest = message.get("manifest_path")
        if manifest and "i-slint-backend-winit" in message.get("package_id", ""):
            backend_artifacts.append(Path(manifest))
        if (
            isinstance(target, dict)
            and target.get("name") == "repaint_probe"
            and message.get("executable")
        ):
            executables.append(Path(message["executable"]))
    require(executables, "cargo did not report the repaint_probe executable")
    if overlay_root is not None:
        require(
            any(path.resolve().is_relative_to(overlay_root) for path in backend_artifacts),
            f"{label} build did not use the temporary backend overlay {overlay_root}",
        )
    executable = executables[-1]
    print(f"=== PROBE {label}: {executable} ===", flush=True)
    return executable


def run_probe(label: str, arguments: list[str], binary: Path) -> subprocess.CompletedProcess[str]:
    print(f"=== RUN {label} ===", flush=True)
    result = subprocess.run(
        [sys.executable, "scripts/smoke_windows_repaint.py", "--binary", str(binary), *arguments],
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
    verify_clean()
    adapter = ROOT / ADAPTER
    source = adapter.read_bytes()
    call = REPAINT_CALL.replace(b"\n", b"\r\n") if b"\r\n" in source else REPAINT_CALL
    require(source.count(call) == 1, "expected exactly one unchanged upstream repaint call")
    errors = []
    try:
        with tempfile.TemporaryDirectory(prefix="sean-toolbox-repaint-") as temporary:
            overlay = Path(temporary) / VENDOR.name
            shutil.copytree(ROOT / VENDOR, overlay)
            (overlay / ADAPTER.name).write_bytes(source.replace(call, b"", 1))
            config = f'patch.crates-io.i-slint-backend-winit.path="{overlay.as_posix()}"'
            binary = build_probe("unpatched", config=config, overlay=overlay)
            check_negative(run_probe("unpatched light/1.0", ["--themes", "light", "--scales", "1.0"], binary))
            print("PASS: unpatched probe failed at initial bottom invalidation", flush=True)
    except (OSError, RuntimeError, subprocess.SubprocessError) as error:
        errors.append(f"negative control: {error}")
        print(f"FAIL: {errors[-1]}", file=sys.stderr, flush=True)

    # Run the full positive matrix even if the negative control failed unexpectedly.
    try:
        binary = build_probe("patched")
        check_positive(run_probe("patched complete matrix", [], binary))
    except (OSError, RuntimeError, ValueError, subprocess.SubprocessError) as error:
        errors.append(f"positive matrix: {error}")
        print(f"FAIL: {errors[-1]}", file=sys.stderr, flush=True)

    verify_clean()
    require(not errors, "; ".join(errors))
    print("PASS: native regression sensitivity and six patched cases / eighteen phases", flush=True)


if __name__ == "__main__":
    try:
        main()
    except (OSError, RuntimeError, subprocess.SubprocessError) as error:
        print(f"FAIL: {error}", file=sys.stderr)
        sys.exit(1)
