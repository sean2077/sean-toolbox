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

This patches only the winit backend crate. Slint, slint-build and the translation
extractor remain at 1.18.1. At the time of this backport (2026-11-09), Slint's
latest published stable release was still 1.18.1 and 1.19.0 was unreleased.

## Verification and removal

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

Remove the root Cargo patch and this directory when upgrading to a stable
Slint release that contains #13568 (or an equivalent fix). Update the Slint
compiler/runtime, translation extractor and attribution together, then rerun
the native repaint regression and normal workspace checks.
