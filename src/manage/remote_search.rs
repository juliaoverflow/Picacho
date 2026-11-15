//! Searching the whole mailbox on the server, not just the mail listed
//! here. Gmail is asked in its own search syntax (X-GM-RAW), so anything
//! Gmail's search box understands works (`older_than:1y`, `larger:5M`…);
//! other servers get the search's terms as IMAP SEARCH criteria, with
//! words searching the whole message (TEXT). Results come newest first,
//! ordered by the server (SORT) where it can, and those not listed yet are
//! fetched (headers only) and kept like any other listed mail.
use super::*;

/// What a server-side search found.
#[derive(Clone, Debug, Default)]
pub struct ServerResults {
    /// The newest matches, newest first.
    pub mails: Vec<Mail>,
    /// How many messages matched in all.
    pub total: usize,
}

/// The most server folders an All Mail search goes through.
const MOST_FOLDERS: usize = 25;

impl Backend {
    /// Searches the mailbox (or, for All Mail and Starred, the account's
    /// mailboxes) on the server for `query`, returning up to `limit` of
    /// the newest matches.
    pub fn search_server(
        &self,
        a: &Account,
        mailbox: &Mailbox,
        query: &str,
        limit: usize,
    ) -> Result<ServerResults> {
        ensure!(
            a.provider != Provider::Pop3,
            "POP3 servers can't search; only mail on this computer is searched."
        );
        ensure!(
            mailbox.folder() != Folder::Drafts,
            "Drafts are searched on this computer."
        );
        let query = query.trim();
        ensure!(!query.is_empty(), "Type something to search for.");
        let lock = self.account_lock(&a.id);
        let _operation = lock.lock().unwrap();
        self.with_imap(a, |c| {
            let names = c.names(false)?.to_vec();
            let starred = mailbox.folder() == Folder::Starred;
            let mut remotes: Vec<(String, Folder)> = Vec::new();
            if mailbox.folder().is_virtual() {
                let standard = if a.provider == Provider::Gmail {
                    vec![Folder::Archive]
                } else {
                    vec![
                        Folder::Inbox,
                        Folder::Sent,
                        Folder::Archive,
                        Folder::Junk,
                        Folder::Trash,
                    ]
                };
                for folder in standard {
                    if let Ok(remote) = resolve_in(&names, a.provider, folder)
                        && !remotes.iter().any(|(r, _)| *r == remote)
                    {
                        remotes.push((remote, folder));
                    }
                }
                if a.provider != Provider::Gmail {
                    for f in custom_folders(&names).into_iter().take(MOST_FOLDERS) {
                        if !remotes.iter().any(|(r, _)| *r == f.name) {
                            remotes.push((f.name, Folder::Other));
                        }
                    }
                }
            } else {
                remotes.push((remote_name(&names, a, mailbox)?, mailbox.folder()));
            }
            let ext = c.extensions()?;
            let utf8 = c.enabled.utf8;
            let string = |text: &str| search_string(text, &ext, utf8);
            let mut criteria = if ext.gmail {
                format!("X-GM-RAW {}", string(query)?)
            } else {
                crate::manage::search::Search::parse(query).imap_criteria(&string)?
            };
            if starred {
                criteria.push_str(" FLAGGED");
            }
            let mut out = ServerResults::default();
            for (remote, folder) in remotes {
                let selected = match c.select(&remote) {
                    Ok(selected) => selected,
                    // One folder that can't be opened doesn't stop the rest.
                    Err(e) if !crate::sync::retry::later(&e) && mailbox.folder().is_virtual() => {
                        continue;
                    }
                    Err(e) => return Err(e),
                };
                let uids = sorted_search(&mut c.session, &ext, &selected, &criteria, utf8)?;
                out.total += uids.len();
                let newest = &uids[..uids.len().min(limit)];
                let prefix = format!("{}:{remote}:{}:", a.id, selected.validity);
                let mut missing = Vec::new();
                for uid in newest {
                    match self.store.message(&format!("{prefix}{uid}"))? {
                        Some(m) => out.mails.push(m),
                        None => missing.push(*uid),
                    }
                }
                let listed = self.list_uids(c, a, folder, &remote, selected.validity, &missing)?;
                self.store.save_messages(&listed)?;
                out.mails.extend(listed);
            }
            out.mails.sort_by(Mail::newest_first);
            // The same Gmail message under several labels shows once.
            let mut seen = std::collections::HashSet::new();
            out.mails.retain(|m| {
                seen.insert(if m.message_id.is_empty() {
                    m.id.clone()
                } else {
                    m.message_id.clone()
                })
            });
            out.mails.truncate(limit);
            Ok(out)
        })
    }
}
