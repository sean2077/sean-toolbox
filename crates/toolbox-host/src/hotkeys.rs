use global_hotkey::{
    GlobalHotKeyManager,
    hotkey::{Code, HotKey, Modifiers},
};
use std::{collections::HashMap, path::Path, str::FromStr};
use toolbox_core::{Config, HotkeySpec, Platform};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Binding {
    Launcher,
    Profile(String),
}

trait Registrar {
    fn register(&self, key: HotKey) -> Result<(), String>;
    fn unregister(&self, key: HotKey) -> Result<(), String>;
}
impl Registrar for GlobalHotKeyManager {
    fn register(&self, key: HotKey) -> Result<(), String> {
        self.register(key).map_err(|e| e.to_string())
    }
    fn unregister(&self, key: HotKey) -> Result<(), String> {
        self.unregister(key).map_err(|e| e.to_string())
    }
}
type Bindings = HashMap<u32, (HotKey, Binding)>;
pub struct Hotkeys {
    manager: Option<Box<dyn Registrar>>,
    #[cfg(target_os = "linux")]
    wayland: Option<crate::wayland::KWinHotkeys>,
    active: Bindings,
}
pub struct Prepared {
    desired: Bindings,
    added: Vec<HotKey>,
    #[cfg(target_os = "linux")]
    wayland: Option<crate::wayland::Update>,
}

impl Hotkeys {
    pub fn new(config: &Config, _executable: &Path) -> (Self, Vec<String>) {
        let mut warnings = Vec::new();
        #[cfg(target_os = "linux")]
        let wayland_session = crate::wayland::is_wayland();
        #[cfg(not(target_os = "linux"))]
        let wayland_session = false;
        #[cfg(target_os = "linux")]
        let wayland = if wayland_session {
            match crate::wayland::KWinHotkeys::detect(_executable) {
                Ok(mut backend) => {
                    warnings.extend(backend.sync(config));
                    Some(backend)
                }
                Err(error) => {
                    warnings.push(format!(
                        "Hotkeys: {error} / Wayland 全局快捷键不可用，可使用托盘或命令行"
                    ));
                    None
                }
            }
        } else {
            None
        };
        let manager = if wayland_session {
            // The compositor owns global shortcuts on Wayland.
            None
        } else {
            match GlobalHotKeyManager::new() {
                Ok(manager) => Some(Box::new(manager) as Box<dyn Registrar>),
                Err(error) => {
                    warnings.push(format!("Hotkeys: {error}"));
                    None
                }
            }
        };
        let mut this = Self {
            manager,
            #[cfg(target_os = "linux")]
            wayland,
            active: HashMap::new(),
        };
        if let Some(manager) = &this.manager {
            match desired(config) {
                Ok(keys) => {
                    for (id, (key, binding)) in keys {
                        match manager.register(key) {
                            Ok(()) => {
                                this.active.insert(id, (key, binding));
                            }
                            Err(error) => {
                                warnings.push(format!("Hotkeys: {key} is unavailable: {error}"))
                            }
                        }
                    }
                }
                Err(error) => warnings.push(format!("Hotkeys: {error}")),
            }
        }
        (this, warnings)
    }

    pub fn available(&self) -> bool {
        #[cfg(target_os = "linux")]
        if let Some(wayland) = &self.wayland {
            return wayland.available();
        }
        self.manager.is_some() && !self.active.is_empty()
    }

    pub fn is_wayland(&self) -> bool {
        #[cfg(target_os = "linux")]
        {
            self.wayland.is_some()
        }
        #[cfg(not(target_os = "linux"))]
        {
            false
        }
    }

    /// Ask the compositor script to run the profile's toggle. Used by the CLI
    /// and tray because native Wayland clients cannot enumerate windows.
    #[cfg(target_os = "linux")]
    pub fn invoke_profile(&self, id: &str) -> Result<(), String> {
        if let Some(wayland) = &self.wayland {
            return wayland.invoke(id);
        }
        Err("Wayland shortcuts are not active".into())
    }

    pub fn shutdown(&mut self) {
        #[cfg(target_os = "linux")]
        if let Some(wayland) = &mut self.wayland {
            wayland.shutdown();
        }
    }

    pub fn binding(&self, id: u32) -> Option<Binding> {
        self.active.get(&id).map(|(_, b)| b.clone())
    }

    /// Acquire every new shortcut before touching the current registrations.
    pub fn prepare(&self, config: &Config) -> Result<Prepared, String> {
        let desired = desired(config)?;
        let mut added = Vec::new();
        if let Some(manager) = &self.manager {
            for (id, (key, _)) in &desired {
                if !self.active.contains_key(id) {
                    if let Err(error) = manager.register(*key) {
                        for key in added {
                            let _ = manager.unregister(key);
                        }
                        return Err(format!(
                            "快捷键 {key} 注册失败 / Shortcut unavailable: {error}"
                        ));
                    }
                    added.push(*key);
                }
            }
        }
        Ok(Prepared {
            desired,
            added,
            #[cfg(target_os = "linux")]
            wayland: self.wayland.as_ref().map(|_| crate::wayland::Update {
                script: crate::wayland::render(config),
                bindings: crate::wayland::desired_bindings(config),
            }),
        })
    }

    pub fn rollback(&self, prepared: Prepared) {
        if let Some(manager) = &self.manager {
            for key in prepared.added {
                let _ = manager.unregister(key);
            }
        }
    }

    pub fn commit(&mut self, prepared: Prepared) -> Vec<String> {
        let mut warnings = Vec::new();
        if let Some(manager) = &self.manager {
            for (id, (key, _)) in &self.active {
                if !prepared.desired.contains_key(id)
                    && let Err(e) = manager.unregister(*key)
                {
                    warnings.push(format!("Hotkeys: failed to release {key}: {e}"));
                }
            }
            self.active = prepared.desired;
        }
        #[cfg(target_os = "linux")]
        if let (Some(backend), Some(update)) = (self.wayland.as_mut(), prepared.wayland) {
            warnings.extend(backend.sync_script(&update.script, &update.bindings));
        }
        warnings
    }
}

fn desired(config: &Config) -> Result<Bindings, String> {
    config.validate()?;
    let mut keys = Bindings::new();
    let launcher = native_key(&config.projects.hotkey)?;
    keys.insert(launcher.id(), (launcher, Binding::Launcher));
    if config.windows.enabled {
        for profile in &config.windows.profiles {
            if profile.enabled
                && profile.platform == Platform::current()
                && !profile.hotkey.is_empty()
            {
                let key = native_key(&profile.hotkey)?;
                keys.insert(key.id(), (key, Binding::Profile(profile.id.clone())));
            }
        }
    }
    Ok(keys)
}

fn native_key(text: &str) -> Result<HotKey, String> {
    let key = HotkeySpec::parse(text)?;
    let mut mods = Modifiers::empty();
    if key.ctrl {
        mods |= Modifiers::CONTROL;
    }
    if key.alt {
        mods |= Modifiers::ALT;
    }
    if key.shift {
        mods |= Modifiers::SHIFT;
    }
    if key.super_key {
        mods |= Modifiers::META;
    }
    let name = match key.key.as_str() {
        "SPACE" => "Space".into(),
        "TAB" => "Tab".into(),
        "ENTER" => "Enter".into(),
        "ESCAPE" => "Escape".into(),
        s if s.len() == 1 && s.as_bytes()[0].is_ascii_digit() => format!("Digit{s}"),
        s if s.len() == 1 => format!("Key{s}"),
        s => s.into(),
    };
    let code = Code::from_str(&name).map_err(|_| format!("Unsupported key: {name}"))?;
    Ok(HotKey::new(Some(mods), code))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{cell::RefCell, collections::HashSet, rc::Rc};
    struct Fake {
        held: Rc<RefCell<HashSet<u32>>>,
        denied: u32,
    }
    impl Registrar for Fake {
        fn register(&self, key: HotKey) -> Result<(), String> {
            if key.id() == self.denied {
                return Err("occupied".into());
            }
            self.held.borrow_mut().insert(key.id());
            Ok(())
        }
        fn unregister(&self, key: HotKey) -> Result<(), String> {
            self.held.borrow_mut().remove(&key.id());
            Ok(())
        }
    }
    #[test]
    fn conflict_and_storage_rollback_keep_old_shortcuts() {
        let held = Rc::new(RefCell::new(HashSet::new()));
        let mut keys = Hotkeys {
            manager: Some(Box::new(Fake {
                held: held.clone(),
                denied: native_key("F12").unwrap().id(),
            })),
            #[cfg(target_os = "linux")]
            wayland: None,
            active: HashMap::new(),
        };
        let initial = Config::default();
        keys.commit(keys.prepare(&initial).unwrap());
        let old = held.borrow().clone();
        let mut changed = initial.clone();
        changed.projects.hotkey = "F12".into();
        assert!(keys.prepare(&changed).is_err());
        assert_eq!(*held.borrow(), old);
        changed.projects.hotkey = "Ctrl+Alt+P".into();
        let pending = keys.prepare(&changed).unwrap();
        keys.rollback(pending);
        assert_eq!(*held.borrow(), old);
        keys.commit(keys.prepare(&changed).unwrap());
        assert_eq!(
            *held.borrow(),
            desired(&changed).unwrap().keys().copied().collect()
        );
    }
}
