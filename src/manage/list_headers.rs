//! A mailing list's headers: where to get help, subscribe, post, reach
//! its owner and read its archive (RFC 2369), and the list's identity
//! (List-Id, RFC 2919). They are only stored and shown on request; none
//! of these addresses is ever opened or written to by itself.
//! List-Unsubscribe has its own handling, in `subscriptions`.
use serde::{Deserialize, Serialize};

/// What a list header's address is for (RFC 2369 §3).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum ListLinkKind {
    Help,
    Subscribe,
    Unsubscribe,
    Post,
    Owner,
    Archive,
}
impl ListLinkKind {
    /// Each kind with its header's name.
    pub const HEADERS: [(Self, &'static str); 6] = [
        (Self::Help, "List-Help"),
        (Self::Subscribe, "List-Subscribe"),
        (Self::Unsubscribe, "List-Unsubscribe"),
        (Self::Post, "List-Post"),
        (Self::Owner, "List-Owner"),
        (Self::Archive, "List-Archive"),
    ];
}

/// One address a list header gives (a `mailto:`, `https:`… URI).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ListLink {
    pub kind: ListLinkKind,
    pub uri: String,
}

/// The URIs in a list header, in its order (RFC 2369 §2: each in angle
/// brackets, comma-separated, with comments allowed). List-Post's `NO`
/// (§3.4: posting isn't allowed) gives none.
pub fn uris(header: &str) -> Vec<String> {
    crate::manage::subscriptions::targets(header)
}

/// The links in a message's list headers (except List-Unsubscribe, kept
/// on its own), from `header`, which gives a header's value by name.
pub fn links(header: impl Fn(&str) -> String) -> Vec<ListLink> {
    let mut out = Vec::new();
    for (kind, name) in ListLinkKind::HEADERS {
        if kind == ListLinkKind::Unsubscribe {
            continue;
        }
        for uri in uris(&header(name)) {
            // Only addresses a person could use; nothing that would run.
            let lower = uri.to_ascii_lowercase();
            if ["mailto:", "https:", "http:", "ftp:", "news:", "nntp:"]
                .iter()
                .any(|s| lower.starts_with(s))
            {
                out.push(ListLink { kind, uri });
            }
        }
    }
    out
}

/// The list's identity from List-Id (RFC 2919 §2: an optional phrase,
/// then `<list-id>`, a dot-atom such as `users.example.org`), lowercase.
/// Empty where the header has none.
pub fn list_id(header: &str) -> String {
    let Some(start) = header.rfind('<') else {
        return String::new();
    };
    let Some(end) = header[start..].find('>') else {
        return String::new();
    };
    let id = header[start + 1..start + end].trim();
    let dot_atom = !id.is_empty()
        && !id.starts_with('.')
        && !id.ends_with('.')
        && !id.contains("..")
        && id.contains('.')
        && id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"!#$%&'*+-/=?^_`{|}~.".contains(&b));
    if dot_atom { id.to_ascii_lowercase() } else { String::new() }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn list_headers_are_read() {
        let headers = |name: &str| {
            match name {
                "List-Help" => "<mailto:list@host.com?subject=help> (List Instructions)",
                "List-Subscribe" => "<https://example.com/sub>,\r\n <mailto:list-request@host.com?subject=subscribe>",
                "List-Post" => "NO (posting not allowed on this list)",
                "List-Owner" => "<mailto:listmom@host.com> (Contact Person for Help)",
                "List-Archive" => "<javascript:alert(1)>, <https://example.com/archive>",
                "List-Unsubscribe" => "<mailto:leave@host.com>",
                _ => "",
            }
            .to_owned()
        };
        assert_eq!(
            links(headers),
            [
                ListLink { kind: ListLinkKind::Help, uri: "mailto:list@host.com?subject=help".into() },
                ListLink { kind: ListLinkKind::Subscribe, uri: "https://example.com/sub".into() },
                ListLink {
                    kind: ListLinkKind::Subscribe,
                    uri: "mailto:list-request@host.com?subject=subscribe".into()
                },
                ListLink { kind: ListLinkKind::Owner, uri: "mailto:listmom@host.com".into() },
                ListLink { kind: ListLinkKind::Archive, uri: "https://example.com/archive".into() },
            ]
        );
        assert!(uris("NO").is_empty());
        assert_eq!(list_id("List Header Mailing List <List-Header.Example.ORG>"), "list-header.example.org");
        assert_eq!(list_id("<commonspace-users.list-id.within.com>"), "commonspace-users.list-id.within.com");
        assert_eq!(list_id("no brackets.example.org"), "");
        assert_eq!(list_id("<not a dot atom>"), "");
        assert_eq!(list_id("<nodot>"), "");
    }
}
