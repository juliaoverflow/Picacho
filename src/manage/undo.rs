//! Undo for moves: messages a move (archive, trash, spam, a folder) took
//! out of their mailbox go back where each one was.
//!
//! Proton and JMAP keep a message's id wherever it is, so the move is made
//! again the other way. IMAP gives a moved message a new UID (and so a new
//! id) in its new mailbox, so it is found there by its Message-ID, as
//! snoozing does, and moved back. A message without a Message-ID, or no
//! longer where the move put it, stays where it is.
//!
//! Gmail's All Mail holds every message under all its labels: archived
//! mail is copied back to where it was (giving back that label) rather than
//! moved out of All Mail, which at Gmail would put it in the trash.
use super::*;

/// One message as it was before a move, and where the move put it.
#[derive(Clone, Debug)]
pub struct Moved {
    pub before: Mail,
    pub change: Change,
}

impl Backend {
    /// Puts back messages a move took out of their mailboxes. Returns how
    /// many went back.
    pub fn undo_move(&self, a: &Account, moved: &[Moved]) -> Result<usize> {
        ensure!(
            moved.iter().all(|m| m.before.account == a.id && m.change.moves()),
            "Only moves of this account's mail can be undone."
        );
        if moved.is_empty() {
            return Ok(0);
        }
        if a.provider == Provider::Pop3 {
            // Moves are only on this computer: put the rows back as they were.
            let before: Vec<Mail> = moved.iter().map(|m| m.before.clone()).collect();
            self.store.commit_changes(&before, false)?;
            return Ok(before.len());
        }
        if matches!(a.provider, Provider::Proton | Provider::Jmap) {
            return self.undo_by_id(a, moved);
        }
        let lock = self.account_lock(&a.id);
        let _operation = lock.lock().unwrap();
        self.with_imap(a, |c| {
            self.replay_offline(c, a)?;
            let names = c.names(false)?.to_vec();
            let gmail = c.extensions()?.gmail;
            let all_mail = gmail
                .then(|| resolve_in(&names, a.provider, Folder::Archive).ok())
                .flatten();
            let mut back = 0;
            for m in moved {
                let id = m.before.message_id.trim();
                if id.is_empty() {
                    continue;
                }
                let target = match &m.change {
                    Change::Move(folder) => resolve_in(&names, a.provider, *folder)?,
                    Change::MoveTo(name) => name.clone(),
                    _ => continue,
                };
                let home = m.before.remote_folder.clone();
                if home == target {
                    continue;
                }
                let selected = c.select(&target)?;
                let uids: Vec<u32> = c
                    .uid_search(&format!("HEADER Message-ID {}", quote(id)?))?
                    .into_iter()
                    .collect();
                if uids.is_empty() {
                    continue;
                }
                let set = uid_ranges(&uids);
                let wire = c.mailbox(&home)?;
                if all_mail.as_ref() == Some(&target) {
                    // Gmail: the label back; All Mail keeps it as it always does.
                    c.session
                        .run_command_and_check_ok(format!("UID COPY {set} {wire}"))?;
                    back += 1;
                    continue;
                }
                if c.extensions()?.moves {
                    c.session
                        .run_command_and_check_ok(format!("UID MOVE {set} {wire}"))?;
                } else {
                    c.can_erase(&uids)?;
                    c.session
                        .run_command_and_check_ok(format!("UID COPY {set} {wire}"))?;
                    c.erase(&uids)?;
                }
                // The copies in the mailbox it went to are gone from there.
                let prefix = format!("{}:{target}:{}:", a.id, selected.validity);
                let gone: Vec<String> = uids.iter().map(|u| format!("{prefix}{u}")).collect();
                self.store.delete_ids(&a.id, &gone)?;
                let to = match &m.change {
                    Change::Move(folder) => Mailbox::Standard(*folder),
                    _ => Mailbox::Remote(target.clone()),
                };
                self.extend_layout(a, &to, selected.validity, std::iter::empty(), &uids);
                if let Some(mut state) = self.store.mailbox_state(&a.id, &target)?
                    && state.validity == selected.validity
                {
                    state.exists = state.exists.saturating_sub(uids.len() as u32);
                    self.store.save_mailbox_state(&a.id, &target, Some(&state))?;
                }
                back += 1;
            }
            Ok(back)
        })
    }

    /// Proton and JMAP: each message keeps its id, so it moves back by it,
    /// to the folder it was in.
    fn undo_by_id(&self, a: &Account, moved: &[Moved]) -> Result<usize> {
        let mut back = 0;
        for m in moved {
            let home = if m.before.folder == Folder::Other {
                Change::MoveTo(m.before.remote_folder.clone())
            } else {
                Change::Move(m.before.folder)
            };
            let mut now = self
                .store
                .message(&m.before.id)?
                .unwrap_or_else(|| m.before.clone());
            if home.moved_already(&now) {
                continue;
            }
            // Where the move put it, if the cache hasn't caught up.
            m.change.apply(&mut now);
            self.change_many(a, &[now], home)?;
            back += 1;
        }
        Ok(back)
    }
}
