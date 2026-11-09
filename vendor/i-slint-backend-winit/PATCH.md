# Local patch: repaint the region invalidated by Windows

This directory contains the published **i-slint-backend-winit 1.18.1** crate
with a single upstream fix backported. The published version, dependency
requirements, public interfaces, source notices, normalized `Cargo.toml`,
`Cargo.toml.orig`, README and all three original license files are preserved.
The package's `.cargo_vcs_info.json` and standalone `Cargo.lock` are omitted;
the provenance below records the former and the workspace lockfile owns
dependency resolution.

## Provenance

- Published package: <https://crates.io/crates/i-slint-backend-winit/1.18.1>
- Original archive: <https://static.crates.io/crates/i-slint-backend-winit/i-slint-backend-winit-1.18.1.crate>
- Archive SHA-256, verified against the original workspace `Cargo.lock`:
  `dae4fcf1b19e9cb109984d2af87d2b9485388c619265bff71dd523133888bee2`
- Source commit recorded by the published package:
  [`372cf0ee5577c3dfec309a45e7b778ba4e81b734`](https://github.com/slint-ui/slint/tree/372cf0ee5577c3dfec309a45e7b778ba4e81b734/internal/backends/winit)
- Backported upstream change:
  [`08a297b7151e52b98b500b4424a8dee770fc7b58`](https://github.com/slint-ui/slint/commit/08a297b7151e52b98b500b4424a8dee770fc7b58),
  [Slint PR #13568](https://github.com/slint-ui/slint/pull/13568), merged on
  2026-09-28.
- Related upstream issue: [Slint #13537](https://github.com/slint-ui/slint/issues/13537).

The crate retains its original
`GPL-3.0-only OR LicenseRef-Slint-Royalty-free-2.0 OR LicenseRef-Slint-Software-3.0`
license choice. Sean Toolbox uses the Royalty-free 2.0 option described in
[`THIRD_PARTY_NOTICES.md`](../../THIRD_PARTY_NOTICES.md#slint); this patch does
not change that choice or the application's existing attribution.

## Problem and change

Windows can invalidate the visible contents after resizing, restoring a
minimized window, or moving it between displays with different scale factors.
Slint 1.18.1 may still regard the software buffer as current and respond with
an empty or partial dirty region. The resulting window can retain only part
of an earlier frame, leaving an unpainted band across the client area.

Only `winitwindowadapter.rs` differs from the published source: the upstream
33-line change is applied without adaptation. Before drawing on Windows,
`mark_windows_update_region_dirty` reads `GetUpdateRect` while winit has not
yet validated the native update region, converts physical coordinates to
logical coordinates using the current scale factor, and marks that region
dirty in Slint's renderer. Internal redraws without a Windows update region
retain partial rendering. No polling, extra repaint timer, background-color
toggle, or Linux/macOS behavior change is introduced.

The backport follows winit's normal `WM_PAINT` path. When winit's event handler
is borrowed, for example while a native modal dialog pumps messages, winit lets
`DefWindowProcW` validate the update region before the buffered
`RedrawRequested`; `GetUpdateRect` is then empty and this change does not mark
that discarded region. That nested-loop path remains outside the backport and
the native probe.

This patches only the winit backend crate. Slint, slint-build and the translation
extractor remain at 1.18.1. At the time of this backport (2026-11-09), Slint's
latest published stable release was still 1.18.1 and 1.19.0 was unreleased.

## Verification and upgrade

To audit the backport, download the original archive, verify its SHA-256 above,
and compare its files with this directory. Except for `PATCH.md` and the two
omitted package metadata files, the only difference must be the exact 33-line
upstream change to `winitwindowadapter.rs`. All original license files must
remain byte-identical.

Windows regression verification must exercise actual native invalidation and
presentation, including a cleared client area followed by a system repaint,
resize, and minimize/restore. Component-only software screenshots do not cover
the winit/Win32 update-region path. Cross-display DPI transitions still require
an environment with differently scaled displays.

### Slint upgrade workflow

Before changing a Slint version, check whether the target stable release contains
#13568 or an equivalent Windows update-region fix.

If the target release contains the fix:

1. Remove the root `[patch.crates-io]` entry and this directory; update
   `Cargo.lock` so the backend resolves from crates.io again.
2. Upgrade every Slint component together: `slint`, `slint-build`, renderer and
   backend selection crates, and the pinned `slint-tr-extractor`.
3. Remove or rewrite the Windows negative-control step and
   `scripts/check_windows_repaint.py`; keep or relocate
   `scripts/smoke_windows_repaint.py` and
   `crates/toolbox-ui/examples/repaint_probe.rs` if the upstream fix still needs
   the native regression.
4. Update `THIRD_PARTY_NOTICES.md`, `docs/DEVELOPMENT.md`,
   `docs/VALIDATION.md`, packaged links, and any license or attribution changes.
5. Run the native regression (if retained) and the full workspace gates.

If the target release does not contain the fix:

1. Download the target `i-slint-backend-winit` crate and verify its SHA-256.
2. Reapply the upstream change to `winitwindowadapter.rs`; adapt only if the
   target source changed. Update the provenance, version, archive hash, and
   upstream commit in this file.
3. Update the root `Cargo.toml` patch entry, `Cargo.lock`, and the exact
   `slint`/`slint-build` pins in `crates/toolbox-ui/Cargo.toml`.
4. Update the CI `slint-tr-extractor` pin and any license or attribution files.
5. Audit the vendored directory again: only this file and the adapter may differ
   from the published crate; all license files stay byte-identical.
6. Run the native reverse/positive check and the full workspace gates.

If the upstream fix exists in a different form, remove this local patch and keep
or adapt the native smoke test as the regression.

Do not upgrade only `slint` or only this backend: the backend version must match
the exact Slint dependency. A Slint upgrade does not authorize a release, tag, or
version bump; follow the project release process separately.
