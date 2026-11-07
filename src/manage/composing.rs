//! Application-wide composing preferences, saved as one Store preference.
use serde::{Deserialize, Serialize};

/// Where a reply to the open message is written.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReplyPlacement {
    /// Below the message in the reader, like a conversation.
    #[default]
    Inline,
    /// In a compose window of its own.
    Window,
}

/// Where new messages open.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ComposeOpen {
    /// A panel floating over the lower right of the main window.
    #[default]
    Floating,
    /// Filling the main window.
    Maximized,
    /// A separate window.
    Window,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct ComposeOptions {
    pub reply_placement: ReplyPlacement,
    pub open_in: ComposeOpen,
    /// New messages start formatted (bold, lists, links…) rather than plain.
    pub formatted: bool,
    /// The reader's Reply button answers everyone.
    pub default_reply_all: bool,
    /// Replies and forwards include the original message.
    pub quote_original: bool,
    pub signature: String,
    /// Add the signature to replies and forwards, not only new messages.
    pub signature_on_replies: bool,
    /// Seconds a sent message waits, with an Undo, before it leaves. 0 sends
    /// at once.
    pub undo_send_seconds: u32,
    /// Ask before sending a message that mentions an attachment without one.
    pub attachment_reminder: bool,
    /// Ask before sending a message without a subject.
    pub warn_empty_subject: bool,
}

impl Default for ComposeOptions {
    fn default() -> Self {
        Self {
            reply_placement: ReplyPlacement::Inline,
            open_in: ComposeOpen::Floating,
            formatted: true,
            default_reply_all: false,
            quote_original: true,
            signature: String::new(),
            signature_on_replies: true,
            undo_send_seconds: 5,
            attachment_reminder: true,
            warn_empty_subject: true,
        }
    }
}

pub const PREFERENCE: &str = "compose_options";

/// Words that suggest the writer meant to attach something.
pub fn mentions_attachment(body: &str) -> bool {
    // Quoted text is someone else's; only what was written now counts.
    let written: String = body
        .lines()
        .take_while(|l| !(l.starts_with("On ") && l.trim_end().ends_with("wrote:")))
        .filter(|l| !l.trim_start().starts_with('>'))
        .collect::<Vec<_>>()
        .join("\n")
        .to_lowercase();
    [
        "attached",
        "attachment",
        "enclosed",
        "i've attached",
        "see attached",
    ]
    .iter()
    .any(|w| written.contains(w))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn older_or_partial_preferences_take_defaults() {
        let o: ComposeOptions = serde_json::from_str(r#"{"signature":"— Sam"}"#).unwrap();
        assert_eq!(o.signature, "— Sam");
        assert_eq!(o.reply_placement, ReplyPlacement::Inline);
        assert_eq!(o.undo_send_seconds, 5);
    }

    #[test]
    fn attachment_reminder_ignores_quoted_text() {
        assert!(mentions_attachment("Hi, the report is attached."));
        assert!(!mentions_attachment(
            "Thanks!\n\nOn Mon, Ana wrote:\n> see the attached file"
        ));
        assert!(!mentions_attachment("Thanks!\n> attachment below"));
    }
}
