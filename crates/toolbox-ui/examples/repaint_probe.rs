//! Static native window for `scripts/smoke_windows_repaint.py`.
//!
//! This deliberately has no animation, hover state, host, IPC, or configuration.
//! Once shown, only Windows invalidation can repaint the damaged test regions.

use slint::ComponentHandle;

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
    slint::BackendSelector::new()
        .backend_name("winit".into())
        .renderer_name("software".into())
        .select()?;
    let probe = RepaintProbe::new()?;
    probe.set_dark(args[1] == "dark");
    probe.set_caption(args[2].as_str().into());
    probe.run()?;
    Ok(())
}
