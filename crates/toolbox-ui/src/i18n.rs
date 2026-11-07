//! Preserve application message identity across language previews and worker replies.
//! External diagnostics and user-provided data are intentionally displayed verbatim.
use toolbox_i18n::{Locale, Message};

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
