//! Native application-window control. A service lives on one worker thread; it
//! does not run a background polling loop when idle.
mod backend;
mod engine;
mod launch;
#[cfg(target_os = "linux")]
mod linux;
#[cfg(target_os = "macos")]
mod macos;
#[cfg(windows)]
mod windows;

pub use backend::{Desktop, Window, WindowRect, quake_bounds};
pub use engine::{Engine, EngineOptions};
use toolbox_core::{AppProfile, Platform};

#[derive(Debug, Clone)]
pub struct WindowCapabilities {
    pub backend: String,
    pub available: bool,
    pub reason: Option<String>,
}

pub struct WindowService {
    engine: Option<Engine<Box<dyn Desktop>>>,
}

impl Default for WindowService {
    fn default() -> Self {
        Self::new()
    }
}

impl WindowService {
    pub fn new() -> Self {
        Self { engine: None }
    }

    fn connect(&mut self) -> Result<&mut Engine<Box<dyn Desktop>>, String> {
        if self.engine.is_none() {
            let backend = backend::connect()?;
            self.engine = Some(Engine::new(backend));
        }
        let engine = self.engine.as_mut().expect("engine initialized above");
        engine.backend_mut().check_available()?;
        Ok(engine)
    }

    pub fn capabilities(&mut self) -> WindowCapabilities {
        let reason = self.connect().err();
        WindowCapabilities {
            backend: backend::name().into(),
            available: reason.is_none(),
            reason,
        }
    }

    pub fn toggle(&mut self, profile: &AppProfile) -> Result<String, String> {
        if profile.platform != Platform::current() {
            return Err(
                "This application profile belongs to another platform / 该应用配置属于其他系统"
                    .into(),
            );
        }
        self.connect()?.toggle(profile)
    }

    pub fn reset(&mut self) -> Result<(), String> {
        // Keep launch cooldowns across configuration reloads to prevent a
        // duplicate launch while an already-started application is opening.
        if let Some(engine) = &mut self.engine {
            engine.reset()?;
        }
        Ok(())
    }

    pub fn foreground_changed(&mut self) -> Result<(), String> {
        if let Some(engine) = &mut self.engine {
            engine.foreground_changed()?;
        }
        Ok(())
    }
}
