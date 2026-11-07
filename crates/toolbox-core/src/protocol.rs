use crate::Config;
use crate::skills::{SkillsCommand, SkillsSnapshot};
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
    Skills,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", content = "data", rename_all = "snake_case")]
pub enum HostCommand {
    Ping,
    Show(UiPage),
    GetConfig,
    GetSkills,
    Skills(SkillsCommand),
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
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub skills: Option<SkillsSnapshot>,
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

    #[test]
    fn responses_without_skills_remain_compatible() {
        let response: Response = serde_json::from_str(
            r#"{"ok":true,"message":"pong","config":null,"status":null,"revision":42}"#,
        )
        .unwrap();
        assert_eq!(response.revision, 42);
        assert!(response.skills.is_none());
        assert!(
            serde_json::to_value(response)
                .unwrap()
                .get("skills")
                .is_none()
        );
    }

    #[test]
    fn skills_commands_have_an_explicit_wire_contract() {
        let request = Request::new(HostCommand::Skills(SkillsCommand::SetDailyUpdate {
            enabled: true,
        }));
        assert_eq!(
            serde_json::to_value(request).unwrap(),
            serde_json::json!({
                "version": 1,
                "command": {"type": "skills", "data": {
                    "type": "set_daily_update", "data": {"enabled": true}
                }}
            })
        );
        let request = Request::new(HostCommand::Skills(SkillsCommand::SetDeployMode {
            mode: crate::skills::SkillDeployMode::Copy,
        }));
        assert_eq!(
            serde_json::to_value(request).unwrap(),
            serde_json::json!({
                "version": 1,
                "command": {"type": "skills", "data": {
                    "type": "set_deploy_mode", "data": {"mode": "copy"}
                }}
            })
        );
        let page: UiRequest = serde_json::from_str(r#"{"page":"skills"}"#).unwrap();
        assert_eq!(page.page, UiPage::Skills);
    }
}
