//! Application-wide reading defaults. Message overrides use the same value type.
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct ReadingOptions {
    pub html: bool,
    pub block_remote: bool,
    /// Light or dark email bodies. Older saved choices have none: light.
    pub theme: EmailTheme,
}

impl Default for ReadingOptions {
    fn default() -> Self {
        Self {
            html: true,
            block_remote: true,
            theme: EmailTheme::Light,
        }
    }
}

/// How an email body is colored, independent of the app's own appearance.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum EmailTheme {
    #[default]
    Light,
    Dark,
    /// Follows the app's light or dark appearance.
    App,
}
impl EmailTheme {
    pub fn is_dark(self, app_dark: bool) -> bool {
        match self {
            EmailTheme::Light => false,
            EmailTheme::Dark => true,
            EmailTheme::App => app_dark,
        }
    }
}

pub fn message_key(account: &str, message: &str) -> String {
    format!(
        "reading:{}",
        serde_json::to_string(&(account, message)).unwrap()
    )
}
