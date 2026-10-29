use toolbox_core::LaunchCommand;

/// An opaque identifier includes process identity and must be revalidated by
/// the platform backend before changing a window.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Window {
    pub id: String,
    pub title: String,
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
