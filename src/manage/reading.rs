//! Application-wide reading defaults. Message overrides use the same value type.
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct ReadingOptions {
    pub html: bool,
    pub block_remote: bool,
}

impl Default for ReadingOptions {
    fn default() -> Self {
        Self {
            html: true,
            block_remote: true,
        }
    }
}

pub fn message_key(account: &str, message: &str) -> String {
    format!(
        "reading:{}",
        serde_json::to_string(&(account, message)).unwrap()
    )
}
