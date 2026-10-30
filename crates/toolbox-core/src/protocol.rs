use crate::Config;
use serde::{Deserialize, Serialize};
pub const PROTOCOL_VERSION: u32 = 1;
pub const HOST_ENDPOINT: &str = "sean-toolbox-host-v1";
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum UiPage {
    #[default]
    Projects,
    Windows,
    Settings,
    Diagnostics,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", content = "data", rename_all = "snake_case")]
pub enum HostCommand {
    Ping,
    Show(UiPage),
    GetConfig,
    SaveConfig {
        config: Config,
        expected_revision: u64,
    },
    Toggle {
        id: String,
    },
    Status,
    Quit,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Request {
    pub version: u32,
    pub command: HostCommand,
}
impl Request {
    pub fn new(command: HostCommand) -> Self {
        Self {
            version: PROTOCOL_VERSION,
            command,
        }
    }
}
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct HostStatus {
    pub process_id: u32,
    pub backend: String,
    pub window_control: bool,
    pub hotkeys: bool,
    pub tray: bool,
    pub warnings: Vec<String>,
    pub last_action: String,
}
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Response {
    pub ok: bool,
    pub message: String,
    pub config: Option<Config>,
    pub status: Option<HostStatus>,
    pub revision: u64,
}
impl Response {
    pub fn success(message: impl Into<String>) -> Self {
        Self {
            ok: true,
            message: message.into(),
            ..Default::default()
        }
    }
    pub fn error(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
            ..Default::default()
        }
    }
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UiRequest {
    pub page: UiPage,
    /// A startup probe acknowledges the UI event loop without activating it again.
    #[serde(default)]
    pub readiness_only: bool,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn existing_ui_requests_remain_normal_activations() {
        let request: UiRequest = serde_json::from_str(r#"{"page":"projects"}"#).unwrap();
        assert_eq!(request.page, UiPage::Projects);
        assert!(!request.readiness_only);
    }

    #[test]
    fn readiness_probe_is_distinct_from_an_activation() {
        let request: UiRequest =
            serde_json::from_str(r#"{"page":"settings","readiness_only":true}"#).unwrap();
        assert_eq!(request.page, UiPage::Settings);
        assert!(request.readiness_only);
        assert_eq!(
            serde_json::to_value(&request).unwrap(),
            serde_json::json!({"page": "settings", "readiness_only": true})
        );
    }
}
