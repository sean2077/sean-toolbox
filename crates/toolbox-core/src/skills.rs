//! Wire models for the host-owned skills manager.
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SkillApp {
    Claude,
    Codex,
    Gemini,
    OpenCode,
    Grok,
}

impl SkillApp {
    pub const ALL: [Self; 5] = [
        Self::Claude,
        Self::Codex,
        Self::Gemini,
        Self::OpenCode,
        Self::Grok,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Self::Claude => "Claude Code",
            Self::Codex => "Codex",
            Self::Gemini => "Gemini CLI",
            Self::OpenCode => "OpenCode",
            Self::Grok => "Grok Build",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SkillAppPath {
    pub app: SkillApp,
    pub path: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SkillRepository {
    pub id: String,
    /// Canonical HTTPS GitHub repository URL, without credentials.
    pub url: String,
    /// None follows the remote's default branch.
    pub branch: Option<String>,
    pub revision: Option<String>,
    pub last_error: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SkillCandidate {
    pub id: String,
    pub repository_id: String,
    pub path: String,
    pub directory: String,
    pub name: String,
    pub description: String,
    pub installed: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct InstalledSkill {
    pub id: String,
    pub directory: String,
    pub name: String,
    pub description: String,
    pub repository_id: Option<String>,
    pub source_path: Option<String>,
    pub revision: Option<String>,
    pub apps: Vec<SkillApp>,
    pub updated_at: u64,
    pub last_error: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LocalSkill {
    pub path: String,
    pub directory: String,
    pub name: String,
    pub description: String,
    pub app: Option<SkillApp>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct SkillsSnapshot {
    pub storage_path: String,
    pub app_paths: Vec<SkillAppPath>,
    pub repositories: Vec<SkillRepository>,
    pub installed: Vec<InstalledSkill>,
    pub available: Vec<SkillCandidate>,
    pub local: Vec<LocalSkill>,
    pub daily_update: bool,
    pub last_attempt_at: Option<u64>,
    pub last_success_at: Option<u64>,
    pub next_update_at: Option<u64>,
    /// The host overlays this while a command or daily update is running.
    pub busy: bool,
    pub message: String,
    pub last_error: Option<String>,
    pub warnings: Vec<String>,
    /// Some display rows were omitted to fit the IPC response budget.
    pub truncated: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", content = "data", rename_all = "snake_case")]
pub enum SkillsCommand {
    AddRepository {
        url: String,
        branch: Option<String>,
    },
    RemoveRepository {
        id: String,
    },
    Discover,
    Install {
        candidate_id: String,
        apps: Vec<SkillApp>,
    },
    ImportLocal {
        path: String,
        apps: Vec<SkillApp>,
    },
    ScanLocal,
    SetEnabled {
        skill_id: String,
        app: SkillApp,
        enabled: bool,
    },
    Uninstall {
        skill_id: String,
    },
    /// None updates all repository-backed installed skills.
    Update {
        skill_id: Option<String>,
    },
    SetDailyUpdate {
        enabled: bool,
    },
}
