//! Proton push notifications (`sync::proton_push`) for the Proton Direct
//! accounts that turn them on (`SyncSettings::proton_push`).
//!
//! One push client serves all of them. Each account that wants pushes has
//! its session opened (as any account operation would) and registered with
//! that client; whenever a session opens, ends or is replaced the list is
//! brought up to date. What arrives is shown through `on_proton_push`, if
//! the account announces new mail at all (`SyncSettings::notify`).
//!
//! Registrations Proton refuses (or push not starting) are kept by account
//! (`proton_push_failures`) until the account registers, stops wanting
//! pushes or is removed; `on_proton_push_change` hears when they change.
use super::*;
use crate::sync::proton_push;
use crate::sync::proton_push::Presentation;

/// What a Proton push tells of.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ProtonPushed<'a> {
    /// A new message: its row's id, its sender and subject.
    NewMail {
        message: &'a str,
        sender: &'a str,
        subject: &'a str,
    },
    /// A notice from Proton: who it's from, and what it says.
    Notice { from: &'a str, body: &'a str },
}

/// Who hears Proton's pushes; see `on_proton_push`.
pub type ProtonPushHear = Arc<dyn Fn(&Account, ProtonPushed<'_>) + Send + Sync>;
/// Who hears that push registration failures changed; see
/// `on_proton_push_change`.
pub type ProtonPushChange = Arc<dyn Fn() + Send + Sync>;

/// Held while the push client is brought in line with the settings, so two
/// changes in a row don't start it twice or stop it under the other.
static RECONCILING: Mutex<()> = Mutex::new(());

impl Backend {
    /// Who hears Proton's pushes (new mail and Proton's notices).
    pub fn on_proton_push(
        &self,
        hear: impl Fn(&Account, ProtonPushed<'_>) + Send + Sync + 'static,
    ) {
        *self.proton_push.lock().unwrap() = Some(Arc::new(hear));
    }
    /// Who hears that `proton_push_failures` changed (one listener: a newer
    /// one replaces it).
    pub fn on_proton_push_change(&self, hear: impl Fn() + Send + Sync + 'static) {
        *self.proton_push_change.lock().unwrap() = Some(Arc::new(hear));
    }
    /// Why each account's push notifications aren't registered, by account
    /// id: Proton refused the registration, or push couldn't start.
    pub fn proton_push_failures(&self) -> HashMap<String, String> {
        self.proton_push_failures.lock().unwrap().clone()
    }
    /// Records an account's registration result, telling the listener if
    /// that changes what's failing.
    fn set_proton_push_failure(&self, account: &str, failure: Option<String>) {
        let mut failures = self.proton_push_failures.lock().unwrap();
        let changed = match failure {
            Some(why) => failures.insert(account.to_owned(), why.clone()) != Some(why),
            None => failures.remove(account).is_some(),
        };
        drop(failures);
        if changed {
            self.proton_push_changed();
        }
    }
    /// Forgets the failures of accounts that no longer want pushes.
    fn keep_proton_push_failures(&self, wanted: &[Account]) {
        let mut failures = self.proton_push_failures.lock().unwrap();
        let before = failures.len();
        failures.retain(|id, _| wanted.iter().any(|a| &a.id == id));
        let changed = failures.len() != before;
        drop(failures);
        if changed {
            self.proton_push_changed();
        }
    }
    fn proton_push_changed(&self) {
        let hear = self.proton_push_change.lock().unwrap().clone();
        if let Some(hear) = hear {
            hear();
        }
    }
    /// Tries the account's push registration again (Try Again): starts
    /// push if it couldn't start, and registers the sessions Proton
    /// refused. Returns at once.
    pub fn retry_proton_push(self: &Arc<Self>) {
        if !proton_push::available() {
            return;
        }
        let backend = Arc::downgrade(self);
        let _ = std::thread::Builder::new()
            .name("proton-push".into())
            .spawn(move || {
                if let Some(backend) = backend.upgrade() {
                    backend.reconcile_proton_push_now();
                    proton_push::retry();
                }
            });
    }
    /// The Proton Direct accounts that want pushes.
    fn wants_proton_push(&self) -> Vec<Account> {
        self.store
            .accounts()
            .unwrap_or_default()
            .into_iter()
            .filter(|a| a.provider == Provider::Proton && a.sync.proton_push)
            .collect()
    }
    /// Brings Proton push in line with the accounts' settings: starts the
    /// push client (with its saved token) and registers every account that
    /// wants pushes, or stops it when none does. Opening sessions may take
    /// the network, so this runs on a thread of its own and returns at once.
    pub fn reconcile_proton_push(self: &Arc<Self>) {
        if !proton_push::available() {
            return;
        }
        let backend = Arc::downgrade(self);
        let _ = std::thread::Builder::new()
            .name("proton-push".into())
            .spawn(move || {
                if let Some(backend) = backend.upgrade() {
                    backend.reconcile_proton_push_now();
                }
            });
    }
    fn reconcile_proton_push_now(self: &Arc<Self>) {
        let _reconciling = RECONCILING.lock().unwrap_or_else(|e| e.into_inner());
        let wanted = self.wants_proton_push();
        self.keep_proton_push_failures(&wanted);
        if wanted.is_empty() {
            if proton_push::running() {
                proton_push::stop();
            }
            return;
        }
        let (pushed, registered) = (Arc::downgrade(self), Arc::downgrade(self));
        let started = proton_push::start(
            self.store.clone(),
            Arc::new(move |account, presentation| {
                if let Some(backend) = pushed.upgrade() {
                    backend.proton_pushed(account, presentation);
                }
            }),
            Arc::new(move |account, result| {
                if let Some(backend) = registered.upgrade() {
                    backend.set_proton_push_failure(account, result.err());
                }
            }),
        );
        if let Err(e) = started {
            proton_push::note(&format!("{e:#}"));
            // No account that wants pushes gets them.
            for a in &wanted {
                self.set_proton_push_failure(&a.id, Some(format!("{e:#}")));
            }
            return;
        }
        // Each account's session, so it can be registered. One that can't
        // open (signed out, offline) is registered once something opens it.
        for a in &wanted {
            let lock = self.account_lock(&a.id);
            let _operation = lock.lock().unwrap();
            let _ = self.with_proton(a, |_| Ok(()));
        }
        self.update_proton_push();
        proton_push::set_ready();
    }
    /// Registers the open sessions of the accounts that want pushes, and
    /// no others. Opens nothing; called whenever a session opens or ends.
    pub(super) fn update_proton_push(&self) {
        if !proton_push::running() {
            return;
        }
        let wanted = self.wants_proton_push();
        self.keep_proton_push_failures(&wanted);
        let sessions = self.proton.lock().unwrap();
        let list = wanted
            .iter()
            .filter_map(|a| sessions.get(&a.id).map(|s| s.push_registration(&a.id)))
            .collect();
        drop(sessions);
        proton_push::set_sessions(list);
    }
    /// Shows a push for `account` (Picacho's id), if it still wants pushes
    /// and announces new mail.
    fn proton_pushed(&self, account: &str, presentation: Presentation) {
        let Some(a) = self
            .store
            .accounts()
            .unwrap_or_default()
            .into_iter()
            .find(|a| a.id == account)
        else {
            return;
        };
        if !a.sync.notify || !a.sync.proton_push {
            return;
        }
        let Some(hear) = self.proton_push.lock().unwrap().clone() else {
            return;
        };
        let named = |s: &protected_email_bridge::push::Sender| {
            if s.name.trim().is_empty() {
                s.address.clone()
            } else {
                s.name.clone()
            }
        };
        match presentation {
            Presentation::ShowEmail {
                subject,
                sender,
                message_id,
            } => hear(
                &a,
                ProtonPushed::NewMail {
                    message: &proton::row_id(&a, &message_id),
                    sender: &named(&sender),
                    subject: &subject,
                },
            ),
            Presentation::ShowNotice {
                content, sender, ..
            } => {
                let from = named(&sender);
                hear(
                    &a,
                    ProtonPushed::Notice {
                        from: if from.is_empty() { "Proton" } else { &from },
                        body: &content,
                    },
                );
            }
            // The message was read or moved elsewhere. Notifications here
            // can't be taken back, so the next one replaces it.
            Presentation::Dismiss { .. } | Presentation::Ignore => {}
        }
    }
}
