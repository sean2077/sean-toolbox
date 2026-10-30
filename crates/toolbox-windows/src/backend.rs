use toolbox_core::{LaunchCommand, QuakeAlignment, QuakeConfig, QuakeEdge, QuakeMonitor};

/// Desktop coordinates, including monitors left of or above the primary screen.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WindowRect {
    pub x: i32,
    pub y: i32,
    pub width: u32,
    pub height: u32,
}

pub fn quake_bounds(area: WindowRect, config: &QuakeConfig) -> Result<WindowRect, String> {
    if area.width == 0
        || area.height == 0
        || area.width > i32::MAX as u32
        || area.height > i32::MAX as u32
        || i64::from(area.x) + i64::from(area.width) > i64::from(i32::MAX)
        || i64::from(area.y) + i64::from(area.height) > i64::from(i32::MAX)
        || !(10..=100).contains(&config.width_percent)
        || !(10..=100).contains(&config.height_percent)
    {
        return Err(
            "Invalid Quake dimensions or display work area / 下拉窗口或显示器尺寸无效".into(),
        );
    }
    let width = (u64::from(area.width) * u64::from(config.width_percent) / 100).max(1) as u32;
    let height = (u64::from(area.height) * u64::from(config.height_percent) / 100).max(1) as u32;
    let align = |space: u32| match config.alignment {
        QuakeAlignment::Start => 0,
        QuakeAlignment::Center => space / 2,
        QuakeAlignment::End => space,
    };
    let (dx, dy) = match config.edge {
        QuakeEdge::Top => (align(area.width - width), 0),
        QuakeEdge::Bottom => (align(area.width - width), area.height - height),
        QuakeEdge::Left => (0, align(area.height - height)),
        QuakeEdge::Right => (area.width - width, align(area.height - height)),
    };
    let x = i32::try_from(i64::from(area.x) + i64::from(dx))
        .map_err(|_| "Display horizontal coordinates exceed the supported range")?;
    let y = i32::try_from(i64::from(area.y) + i64::from(dy))
        .map_err(|_| "Display vertical coordinates exceed the supported range")?;
    Ok(WindowRect {
        x,
        y,
        width,
        height,
    })
}

/// An opaque identifier includes process identity and must be revalidated by
/// the platform backend before changing a window.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Window {
    pub id: String,
    pub title: String,
    /// A retained Quake window can exist without being visible or minimized.
    pub hidden: bool,
    pub minimized: bool,
    pub maximized: bool,
}

/// Kept public to support deterministic engine tests and native acceptance
/// tools. Implementations are intentionally not required to be `Send`.
pub trait Desktop {
    fn check_available(&mut self) -> Result<(), String>;
    fn windows(&mut self, matcher: &str) -> Result<Vec<Window>, String>;
    fn foreground(&mut self) -> Result<Option<String>, String>;
    fn minimize(&mut self, id: &str) -> Result<(), String>;
    fn restore(&mut self, id: &str) -> Result<(), String>;
    fn maximize(&mut self, id: &str) -> Result<(), String>;
    fn activate(&mut self, id: &str) -> Result<bool, String>;
    fn check_quake(&mut self, _config: &QuakeConfig) -> Result<(), String> {
        Err(
            "Quake docking is not supported by this desktop backend / 当前桌面不支持下拉窗口"
                .into(),
        )
    }
    /// Capture before restoring or launching an application can change focus.
    fn quake_work_area(&mut self, _monitor: QuakeMonitor) -> Result<WindowRect, String> {
        Err("Cannot read the display work area / 无法读取显示器工作区".into())
    }
    fn place_quake(
        &mut self,
        _id: &str,
        _bounds: WindowRect,
        _always_on_top: bool,
    ) -> Result<(), String> {
        Err("Quake placement is not supported / 不支持下拉窗口定位".into())
    }
    /// Undo temporary style changes, preserving the window's original topmost state.
    fn release_quake(&mut self, _id: &str) -> Result<(), String> {
        Ok(())
    }
    fn clear_quake(&mut self) -> Result<(), String> {
        Ok(())
    }
    /// Keep the application alive while dismissing its Quake window. Backends
    /// without retained hidden-window support fall back to minimization.
    fn hide_quake(&mut self, id: &str) -> Result<(), String> {
        self.minimize(id)
    }
    fn is_foreground_family(&mut self, id: &str) -> Result<bool, String> {
        Ok(self.foreground()?.as_deref() == Some(id))
    }
    fn hide_quake_if_unfocused(&mut self, id: &str) -> Result<bool, String> {
        if self.foreground()?.is_none() || self.is_foreground_family(id)? {
            return Ok(false);
        }
        self.hide_quake(id)?;
        Ok(true)
    }
    /// An error must mean that no process was started, so a fallback is safe.
    fn launch(&mut self, command: &LaunchCommand) -> Result<(), String> {
        crate::launch::spawn(command)
    }
}

impl<T: Desktop + ?Sized> Desktop for Box<T> {
    fn check_available(&mut self) -> Result<(), String> {
        (**self).check_available()
    }
    fn windows(&mut self, matcher: &str) -> Result<Vec<Window>, String> {
        (**self).windows(matcher)
    }
    fn foreground(&mut self) -> Result<Option<String>, String> {
        (**self).foreground()
    }
    fn minimize(&mut self, id: &str) -> Result<(), String> {
        (**self).minimize(id)
    }
    fn restore(&mut self, id: &str) -> Result<(), String> {
        (**self).restore(id)
    }
    fn maximize(&mut self, id: &str) -> Result<(), String> {
        (**self).maximize(id)
    }
    fn activate(&mut self, id: &str) -> Result<bool, String> {
        (**self).activate(id)
    }
    fn check_quake(&mut self, config: &QuakeConfig) -> Result<(), String> {
        (**self).check_quake(config)
    }
    fn quake_work_area(&mut self, monitor: QuakeMonitor) -> Result<WindowRect, String> {
        (**self).quake_work_area(monitor)
    }
    fn place_quake(
        &mut self,
        id: &str,
        bounds: WindowRect,
        always_on_top: bool,
    ) -> Result<(), String> {
        (**self).place_quake(id, bounds, always_on_top)
    }
    fn release_quake(&mut self, id: &str) -> Result<(), String> {
        (**self).release_quake(id)
    }
    fn clear_quake(&mut self) -> Result<(), String> {
        (**self).clear_quake()
    }
    fn hide_quake(&mut self, id: &str) -> Result<(), String> {
        (**self).hide_quake(id)
    }
    fn is_foreground_family(&mut self, id: &str) -> Result<bool, String> {
        (**self).is_foreground_family(id)
    }
    fn hide_quake_if_unfocused(&mut self, id: &str) -> Result<bool, String> {
        (**self).hide_quake_if_unfocused(id)
    }
    fn launch(&mut self, command: &LaunchCommand) -> Result<(), String> {
        (**self).launch(command)
    }
}

pub(crate) fn name() -> &'static str {
    if cfg!(windows) {
        "Windows / Win32"
    } else if cfg!(target_os = "macos") {
        "macOS / Accessibility"
    } else if cfg!(target_os = "linux") {
        "Linux / X11"
    } else {
        "Unsupported"
    }
}

pub(crate) fn connect() -> Result<Box<dyn Desktop>, String> {
    #[cfg(windows)]
    {
        Ok(Box::new(crate::windows::WindowsDesktop::new()?))
    }
    #[cfg(target_os = "macos")]
    {
        Ok(Box::new(crate::macos::MacDesktop::new()?))
    }
    #[cfg(target_os = "linux")]
    {
        Ok(Box::new(crate::linux::X11Desktop::new()?))
    }
    #[cfg(not(any(windows, target_os = "macos", target_os = "linux")))]
    {
        Err("Native window control is not available on this platform".into())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn quake_geometry_respects_work_area_edges_and_alignment() {
        let area = WindowRect {
            x: -1600,
            y: 32,
            width: 1600,
            height: 960,
        };
        for (edge, alignment, x, y) in [
            (QuakeEdge::Top, QuakeAlignment::Start, -1600, 32),
            (QuakeEdge::Top, QuakeAlignment::Center, -1200, 32),
            (QuakeEdge::Top, QuakeAlignment::End, -800, 32),
            (QuakeEdge::Bottom, QuakeAlignment::Center, -1200, 752),
            (QuakeEdge::Left, QuakeAlignment::Start, -1600, 32),
            (QuakeEdge::Left, QuakeAlignment::Center, -1600, 392),
            (QuakeEdge::Left, QuakeAlignment::End, -1600, 752),
            (QuakeEdge::Right, QuakeAlignment::Center, -800, 392),
        ] {
            let config = QuakeConfig {
                edge,
                alignment,
                width_percent: 50,
                height_percent: 25,
                ..Default::default()
            };
            assert_eq!(
                quake_bounds(area, &config).unwrap(),
                WindowRect {
                    x,
                    y,
                    width: 800,
                    height: 240
                }
            );
        }
    }

    #[test]
    fn quake_geometry_rejects_invalid_percentages_and_overflowing_desktops() {
        let area = WindowRect {
            x: 0,
            y: 0,
            width: 1920,
            height: 1040,
        };
        for percent in [0, 9, 101, 255] {
            assert!(
                quake_bounds(
                    area,
                    &QuakeConfig {
                        width_percent: percent,
                        ..Default::default()
                    }
                )
                .is_err()
            );
            assert!(
                quake_bounds(
                    area,
                    &QuakeConfig {
                        height_percent: percent,
                        ..Default::default()
                    }
                )
                .is_err()
            );
        }
        assert!(quake_bounds(WindowRect { width: 0, ..area }, &QuakeConfig::default()).is_err());
        assert!(
            quake_bounds(
                WindowRect {
                    x: i32::MAX - 10,
                    ..area
                },
                &QuakeConfig::default()
            )
            .is_err()
        );
        assert!(
            quake_bounds(
                WindowRect {
                    height: u32::MAX,
                    ..area
                },
                &QuakeConfig::default()
            )
            .is_err()
        );
    }
}
