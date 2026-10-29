use crate::Config;
use std::{
    env, fs,
    io::{Read, Write},
    path::{Path, PathBuf},
};

#[derive(Debug, Clone)]
pub struct ConfigStore {
    path: PathBuf,
}

impl ConfigStore {
    pub fn new(path: PathBuf) -> Self {
        Self { path }
    }
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Missing files use defaults. Invalid files are preserved and reported.
    pub fn load(&self) -> Result<Config, String> {
        let file = match fs::File::open(&self.path) {
            Ok(file) => file,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                return Ok(Config::default());
            }
            Err(error) => return Err(format!("Cannot read {}: {error}", self.path.display())),
        };
        let mut bytes = Vec::new();
        file.take(1024 * 1024 + 1)
            .read_to_end(&mut bytes)
            .map_err(|e| e.to_string())?;
        if bytes.len() > 1024 * 1024 {
            return Err("Configuration exceeds 1 MiB".into());
        }
        let config: Config = serde_json::from_slice(&bytes)
            .map_err(|e| format!("Invalid configuration {}: {e}", self.path.display()))?;
        config.validate()?;
        Ok(config)
    }

    /// Replace the destination atomically; a failed write keeps the previous file intact.
    pub fn save(&self, config: &Config) -> Result<(), String> {
        config.validate()?;
        let mut bytes = serde_json::to_vec_pretty(config).map_err(|e| e.to_string())?;
        bytes.push(b'\n');
        if bytes.len() > 1024 * 1024 {
            return Err("Configuration exceeds 1 MiB when formatted".into());
        }
        let parent = self
            .path
            .parent()
            .filter(|p| !p.as_os_str().is_empty())
            .ok_or("Configuration needs a parent directory")?;
        fs::create_dir_all(parent)
            .map_err(|e| format!("Cannot create configuration directory: {e}"))?;
        let mut file = tempfile::NamedTempFile::new_in(parent).map_err(|e| e.to_string())?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            file.as_file()
                .set_permissions(fs::Permissions::from_mode(0o600))
                .map_err(|e| e.to_string())?;
        }
        file.write_all(&bytes)
            .and_then(|()| file.as_file().sync_all())
            .map_err(|e| e.to_string())?;
        file.persist(&self.path)
            .map_err(|e| format!("Cannot replace configuration: {}", e.error))?;
        #[cfg(unix)]
        {
            if let Ok(directory) = fs::File::open(parent) {
                let _ = directory.sync_all();
            }
        }
        Ok(())
    }
}

pub fn default_config_path() -> Result<PathBuf, String> {
    if let Some(path) = env::var_os("SEAN_TOOLBOX_CONFIG") {
        let path = PathBuf::from(path);
        return if path.is_absolute() {
            Ok(path)
        } else {
            Err("SEAN_TOOLBOX_CONFIG must be an absolute path".into())
        };
    }
    #[cfg(windows)]
    {
        env::var_os("LOCALAPPDATA")
            .map(|p| PathBuf::from(p).join("SeanToolbox/config.json"))
            .ok_or("LOCALAPPDATA is unavailable".into())
    }
    #[cfg(target_os = "macos")]
    {
        env::var_os("HOME")
            .map(|p| PathBuf::from(p).join("Library/Application Support/SeanToolbox/config.json"))
            .ok_or("HOME is unavailable".into())
    }
    #[cfg(not(any(windows, target_os = "macos")))]
    {
        let root = env::var_os("XDG_CONFIG_HOME")
            .map(PathBuf::from)
            .filter(|p| p.is_absolute())
            .or_else(|| env::var_os("HOME").map(|p| PathBuf::from(p).join(".config")))
            .ok_or("HOME is unavailable")?;
        Ok(root.join("sean-toolbox/config.json"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn missing_config_does_not_create_file_and_save_replaces_atomically() {
        let dir = tempfile::tempdir().unwrap();
        let store = ConfigStore::new(dir.path().join("config.json"));
        assert_eq!(store.load().unwrap(), Config::default());
        assert!(!store.path().exists());
        let mut config = Config::default();
        store.save(&config).unwrap();
        config.language = "en".into();
        store.save(&config).unwrap();
        assert_eq!(store.load().unwrap(), config);
    }
    #[test]
    fn corrupt_file_and_invalid_save_preserve_previous_bytes() {
        let dir = tempfile::tempdir().unwrap();
        let store = ConfigStore::new(dir.path().join("config.json"));
        fs::write(store.path(), b"not json").unwrap();
        assert!(store.load().is_err());
        let config = Config {
            schema_version: 9,
            ..Config::default()
        };
        assert!(store.save(&config).is_err());
        assert_eq!(fs::read(store.path()).unwrap(), b"not json");
    }
    #[test]
    fn oversized_formatted_config_never_replaces_a_loadable_file() {
        let dir = tempfile::tempdir().unwrap();
        let store = ConfigStore::new(dir.path().join("config.json"));
        let mut config = Config::default();
        store.save(&config).unwrap();
        let original = fs::read(store.path()).unwrap();
        let command = crate::LaunchCommand {
            executable: "program".into(),
            args: vec!["x".repeat(32768); 4],
            cwd: None,
        };
        config.profiles[0].launch = vec![command; 8];
        assert!(config.validate().is_ok());
        assert!(store.save(&config).is_err());
        assert_eq!(fs::read(store.path()).unwrap(), original);
        assert!(store.load().is_ok());
    }
}
