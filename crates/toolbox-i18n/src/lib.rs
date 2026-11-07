//! Application messages compiled from the shared gettext catalogs.
//!
//! Keep a [`Message`] and its arguments until presentation time, then render it
//! with an explicit [`Locale`]. The host and UI can share messages without a
//! process-global language, runtime catalog parser, or Slint dependency.

/// Languages supported by the application and its configuration file.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Locale {
    En,
    ZhCn,
}

impl Locale {
    /// Resolve a supported configuration tag, falling back to English.
    pub fn from_tag(tag: &str) -> Self {
        match tag {
            "zh-CN" => Self::ZhCn,
            _ => Self::En,
        }
    }

    /// Return the stable tag used by configuration and Slint's bundled catalog.
    pub const fn tag(self) -> &'static str {
        match self {
            Self::En => "en",
            Self::ZhCn => "zh-CN",
        }
    }

    /// Return the index used by the UI language selector.
    ///
    /// Keep this order in sync with the selector model in `ui/main.slint`.
    pub const fn index(self) -> i32 {
        match self {
            Self::ZhCn => 0,
            Self::En => 1,
        }
    }

    /// Resolve the UI language selector index, falling back to Chinese.
    pub const fn from_index(index: i32) -> Self {
        match index {
            1 => Self::En,
            _ => Self::ZhCn,
        }
    }
}

include!(concat!(env!("OUT_DIR"), "/messages.rs"));

#[cfg(test)]
mod tests {
    use super::Locale;

    #[test]
    fn configuration_tags_round_trip_and_unknown_tags_fall_back() {
        for locale in [Locale::En, Locale::ZhCn] {
            assert_eq!(Locale::from_tag(locale.tag()), locale);
        }
        for unknown in ["", "fr", "zh", "en-US", "../../zh-CN"] {
            assert_eq!(Locale::from_tag(unknown), Locale::En);
        }
    }

    #[test]
    fn selector_indices_round_trip_and_unknown_indices_fall_back() {
        for locale in [Locale::En, Locale::ZhCn] {
            assert_eq!(Locale::from_index(locale.index()), locale);
        }
        assert_eq!(Locale::ZhCn.index(), 0);
        assert_eq!(Locale::En.index(), 1);
        for unknown in [-1, 2, i32::MAX] {
            assert_eq!(Locale::from_index(unknown), Locale::ZhCn);
        }
    }
}
