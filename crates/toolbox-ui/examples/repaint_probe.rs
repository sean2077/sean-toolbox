//! Static native window for `scripts/smoke_windows_repaint.py`.
//!
//! This deliberately has no animation, hover state, host, IPC, or configuration.
//! Once shown, only Windows invalidation can repaint the damaged test regions.

use slint::ComponentHandle;
use slint::winit_030::{EventResult, WinitWindowAccessor, winit::event::WindowEvent};

slint::slint! {
    export component RepaintProbe inherits Window {
        in property <bool> dark;
        in property <string> caption;
        title: root.caption;
        preferred-width: 320px;
        preferred-height: 220px;
        min-width: 240px;
        min-height: 160px;
        background: root.dark ? #17191f : #f6f7f9;
        callback geometry-changed(length, length);
        changed width => { root.geometry-changed(root.width, root.height); }
        changed height => { root.geometry-changed(root.width, root.height); }

        Rectangle {
            x: 0px;
            y: 0px;
            width: 72px;
            height: parent.height;
            background: root.dark ? #121419 : #eef0f4;
        }
        Rectangle {
            x: 72px;
            y: parent.height - self.height;
            width: parent.width - self.x;
            height: 36px;
            background: root.dark ? #28354f : #dce7ff;
        }
    }
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<String> = std::env::args().collect();
    if args.len() != 3 || !matches!(args[1].as_str(), "light" | "dark") {
        return Err("use scripts/smoke_windows_repaint.py to run this fixture".into());
    }
    let variant =
        std::env::var("SEAN_TOOLBOX_REPAINT_VARIANT").unwrap_or_else(|_| "baseline".into());
    if !matches!(variant.as_str(), "baseline" | "unbatched" | "opaque") {
        return Err("unknown repaint diagnostic variant".into());
    }
    eprintln!("Repaint variant: {variant}");
    #[cfg(windows)]
    if variant == "unbatched" {
        // SAFETY: This changes only the current UI thread's GDI batch limit.
        let previous_limit = unsafe { windows_sys::Win32::Graphics::Gdi::GdiSetBatchLimit(1) };
        if previous_limit == 0 {
            return Err("GdiSetBatchLimit(1) failed".into());
        }
    }
    let backend = slint::BackendSelector::new()
        .backend_name("winit".into())
        .renderer_name("software".into());
    let backend = if variant == "opaque" {
        backend.with_winit_window_attributes_hook(|attributes| attributes.with_transparent(false))
    } else {
        backend
    };
    backend.select()?;
    let probe = RepaintProbe::new()?;
    probe.on_geometry_changed(|width, height| {
        eprintln!("Slint logical size: {width}x{height}");
    });
    probe.window().on_winit_window_event(|window, event| {
        if matches!(
            event,
            WindowEvent::Resized(_) | WindowEvent::RedrawRequested
        ) {
            let size = window.size();
            eprintln!(
                "Winit event before Slint dispatch: {event:?}; cached physical size: {}x{}",
                size.width, size.height
            );
        }
        EventResult::Propagate
    });
    probe.set_dark(args[1] == "dark");
    probe.set_caption(args[2].as_str().into());
    probe.run()?;
    Ok(())
}
