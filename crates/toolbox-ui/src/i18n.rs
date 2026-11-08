//! Preserve application message identity across language previews and worker replies.
//! External diagnostics and user-provided data are intentionally displayed verbatim.
use toolbox_core::ConfigError;
use toolbox_i18n::{Locale, Message, messages};

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum UiText {
    Localized(Box<Message>),
    Raw(String),
}

impl UiText {
    pub fn render(&self, locale: Locale) -> String {
        match self {
            Self::Localized(message) => message.render(locale),
            Self::Raw(text) => text.clone(),
        }
    }
}

impl Default for UiText {
    fn default() -> Self {
        Self::Raw(String::new())
    }
}

impl From<Message> for UiText {
    fn from(message: Message) -> Self {
        Self::Localized(Box::new(message))
    }
}

/// Local configuration checks know their reason, so present it in the UI
/// language. The host keeps the English `Display` text for IPC and logs.
impl From<ConfigError> for UiText {
    fn from(error: ConfigError) -> Self {
        match error {
            ConfigError::UnsupportedVersion => messages::config_error_unsupported_version(),
            ConfigError::InvalidLanguage => messages::config_error_language(),
            ConfigError::TooManyProfiles => messages::config_error_too_many_profiles(),
            ConfigError::IdleTimeout => messages::invalid_idle_timeout(),
            ConfigError::LauncherShortcut(_) => messages::shortcut_supported_keys(),
            ConfigError::ProfileShortcut(id, _) => messages::config_error_profile_shortcut(id),
            ConfigError::QuakeDimensions(id) => messages::config_error_quake_dimensions(id),
            ConfigError::InvalidProfileId(id) => messages::config_error_profile_id(id),
            ConfigError::InvalidNameOrMatcher(id) => messages::config_error_name_or_matcher(id),
            ConfigError::DuplicateShortcut(shortcut) => {
                messages::config_error_duplicate_shortcut(shortcut)
            }
            ConfigError::LaunchCandidates(id) => messages::config_error_launch_candidates(id),
            ConfigError::InvalidLaunchCommand(id) => messages::config_error_launch_command(id),
            ConfigError::InvalidVsCodePath => messages::config_error_vscode_path(),
        }
        .into()
    }
}

impl From<String> for UiText {
    fn from(text: String) -> Self {
        Self::Raw(text)
    }
}

impl From<&str> for UiText {
    fn from(text: &str) -> Self {
        Self::Raw(text.to_owned())
    }
}
