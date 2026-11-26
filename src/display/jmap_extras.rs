//! What JMAP servers add, in Settings: the sending identities the server
//! keeps (renamed, signed or removed there, so every app sees the change),
//! and what became of messages sent over JMAP (each recipient's delivery,
//! as the server knows it).
use super::*;

/// What a check of a JMAP account found: conversations changed, and sent
/// messages whose delivery changed (by Message-ID) with the new state.
type Refreshed = (
    Option<crate::sync::jmap::ThreadChanges>,
    Option<(Vec<String>, String)>,
);

/// A JMAP account's server identities, as last loaded.
#[derive(Clone, Debug, Default)]
pub(super) struct ServerIdentities {
    pub account: String,
    pub list: Vec<crate::sync::jmap::Identity>,
    pub error: Option<String>,
}

impl MailApp {
    /// Settings → account → Sending: the server's own identities, for JMAP
    /// accounts.
    pub(super) fn server_identities_setting(&self, a: &Account, cx: &Context<Self>) -> Option<AnyElement> {
        if a.provider != crate::manage::model::Provider::Jmap {
            return None;
        }
        let c = self.colors();
        let loaded = self.server_identities.as_ref().filter(|s| s.account == a.id);
        let mut list = div().flex().flex_col().gap_2();
        if let Some(s) = loaded {
            for (i, identity) in s.list.iter().enumerate() {
                list = list.child(
                    div()
                        .id(SharedString::from(format!("server-identity-{}-{i}", a.id)))
                        .flex()
                        .items_center()
                        .gap_3()
                        .px_3()
                        .py_2()
                        .rounded(px(9.))
                        .border_1()
                        .border_color(rgb(c.border))
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .flex()
                                .flex_col()
                                .child(div().text_size(px(13.)).truncate().child(identity.email.clone()))
                                .child(
                                    div()
                                        .text_size(px(11.5))
                                        .text_color(rgb(c.muted))
                                        .truncate()
                                        .child(if identity.name.is_empty() {
                                            t!("jmap-extras-no-name").to_owned()
                                        } else {
                                            identity.name.clone()
                                        }),
                                ),
                        )
                        .child(self.button(
                            SharedString::from(format!("server-identity-name-{}-{i}", a.id)),
                            t!("jmap-extras-use-name-below"),
                            "check",
                            Action::RenameServerIdentity(a.id.clone(), identity.id.clone()),
                            false,
                            cx,
                        ))
                        .when(identity.may_delete, |d| {
                            d.child(self.button(
                                SharedString::from(format!("server-identity-remove-{}-{i}", a.id)),
                                t!("jmap-extras-remove"),
                                "trash",
                                Action::RemoveServerIdentity(a.id.clone(), identity.id.clone()),
                                false,
                                cx,
                            ))
                        }),
                );
            }
        }
        Some(
            div()
                .id(SharedString::from(format!("server-identities-{}", a.id)))
                .flex()
                .flex_col()
                .gap_2()
                .child(self.setting(
                    t!("jmap-extras-addresses-on-the-server"),
                    t!("jmap-extras-the-addresses-your-mail-service-lets"),
                    div()
                        .flex()
                        .gap_2()
                        .child(self.button(
                            SharedString::from(format!("server-identities-load-{}", a.id)),
                            if loaded.is_some() { t!("jmap-extras-read-again") } else { t!("jmap-extras-show") },
                            "refresh",
                            Action::LoadServerIdentities(a.id.clone()),
                            false,
                            cx,
                        )),
                ))
                .child(list)
                .when(loaded.is_some(), |d| {
                    d.child(
                        div()
                            .flex()
                            .flex_wrap()
                            .items_center()
                            .gap_2()
                            .child(
                                div()
                                    .id("server-identity-field")
                                    .w(px(240.))
                                    .child(field(Input::new(&self.collection_name), c, false)),
                            )
                            .child(self.button(
                                SharedString::from(format!("server-identity-add-{}", a.id)),
                                t!("jmap-extras-add-address"),
                                "plus",
                                Action::AddServerIdentity(a.id.clone()),
                                false,
                                cx,
                            )),
                    )
                })
                .children(loaded.and_then(|s| s.error.clone()).map(|e| {
                    div().text_size(px(12.)).text_color(rgb(c.danger)).child(e)
                }))
                .into_any_element(),
        )
    }

    /// Handles the server identity actions.
    pub(super) fn server_identity_action(&mut self, action: Action, w: &mut Window, cx: &mut Context<Self>) {
        let account_of = |this: &Self, id: &str| this.accounts.iter().find(|a| a.id == id).cloned();
        let typed = self.collection_name.read(cx).value().trim().to_owned();
        match action {
            Action::LoadServerIdentities(id) => {
                let Some(a) = account_of(self, &id) else { return };
                self.load_server_identities(a, cx);
            }
            Action::RenameServerIdentity(id, identity) => {
                let Some(a) = account_of(self, &id) else { return };
                let change = crate::sync::jmap::IdentityChange {
                    name: Some(typed),
                    ..Default::default()
                };
                self.collection_name.update(cx, |i, cx| i.set_text("", w, cx));
                self.server_identity_job(a, cx, move |b, a| b.update_jmap_identity(a, &identity, &change));
            }
            Action::RemoveServerIdentity(id, identity) => {
                let Some(a) = account_of(self, &id) else { return };
                self.server_identity_job(a, cx, move |b, a| b.destroy_jmap_identity(a, &identity));
            }
            Action::AddServerIdentity(id) => {
                let Some(a) = account_of(self, &id) else { return };
                let name = a.name.clone();
                self.collection_name.update(cx, |i, cx| i.set_text("", w, cx));
                self.server_identity_job(a, cx, move |b, a| b.create_jmap_identity(a, &typed, &name).map(drop));
            }
            _ => {}
        }
        cx.notify();
    }

    /// Brings in the rest of an open message's conversation from a JMAP
    /// server (Thread/get): messages in other mailboxes, or not listed
    /// yet, so the conversation shows whole. Once per conversation.
    pub(super) fn complete_jmap_thread(&mut self, m: &Mail, cx: &mut Context<Self>) {
        let thread = m.thread_id().to_owned();
        if !thread.starts_with("jmap:") || !self.threads_completed.insert(thread.clone()) {
            return;
        }
        self.watch_jmap_threads(&m.account, cx);
        let Some(a) = self.accounts.iter().find(|a| a.id == m.account).cloned() else {
            return;
        };
        if a.provider != crate::manage::model::Provider::Jmap {
            return;
        }
        let m = m.clone();
        self.task(
            cx,
            move |b| {
                let found = b.jmap_conversation(&a, &m)?;
                Ok(b.with_readable_previews(found))
            },
            |this, result: anyhow::Result<Vec<Mail>>, cx| {
                let Ok(found) = result else { return };
                let mut added = false;
                for m in found {
                    if !this.messages.iter().any(|x| x.id == m.id) {
                        this.thread_extras.insert(m.id.clone());
                        this.messages.push(m);
                        added = true;
                    }
                }
                if added {
                    this.list_revision += 1;
                    cx.notify();
                }
            },
        );
    }

    /// The reader's delivery line for a message sent over JMAP from here.
    pub(super) fn delivery_line(&self, m: &Mail, cx: &Context<Self>) -> Option<AnyElement> {
        if m.folder != Folder::Sent {
            return None;
        }
        let a = self.accounts.iter().find(|a| a.id == m.account)?;
        if a.provider != crate::manage::model::Provider::Jmap {
            return None;
        }
        let c = self.colors();
        Some(match self.deliveries.get(&m.id) {
            Some(text) => div()
                .id("delivery-status")
                .mt_3()
                .text_size(px(12.))
                .text_color(rgb(c.muted))
                .child(text.clone())
                .into_any_element(),
            None => div()
                .mt_3()
                .flex()
                .child(self.button("delivery-check", t!("jmap-extras-delivery"), "send", Action::CheckDelivery, false, cx))
                .into_any_element(),
        })
    }

    pub(super) fn check_delivery(&mut self, cx: &mut Context<Self>) {
        let Some(m) = self.selected_mail() else { return };
        let Some(a) = self.account_of(&m) else { return };
        let id = m.id.clone();
        if !self.delivery_states.contains_key(&a.id) {
            // Changes are asked from now on, with the mail checks.
            self.delivery_states.insert(a.id.clone(), String::new());
            let account = a.clone();
            let key = a.id.clone();
            self.task(
                cx,
                move |b| b.jmap_delivery_changes(&account, ""),
                move |this, result, _| {
                    if let Ok(Some((_, state))) = result {
                        this.delivery_states.insert(key.clone(), state);
                    }
                },
            );
        }
        self.task(
            cx,
            move |b| b.delivery_of(&a, &m),
            move |this, result, cx| {
                let text = match result {
                    Ok(None) => t!("jmap-extras-the-server-no-longer-says-what").to_owned(),
                    Ok(Some(status)) => {
                        let mut parts: Vec<String> = status
                            .delivery
                            .iter()
                            .map(|(to, delivered, _)| {
                                let what = match delivered.as_str() {
                                    "yes" => "delivered",
                                    "no" => t!("jmap-extras-not-delivered"),
                                    "queued" => t!("jmap-extras-on-its-way"),
                                    _ => t!("jmap-extras-sent-no-report"),
                                };
                                format!("{to}: {what}")
                            })
                            .collect();
                        if status.undo == "pending" {
                            parts.insert(0, t!("jmap-extras-waiting-to-be-sent").into());
                        }
                        if parts.is_empty() {
                            t!("jmap-extras-sent").into()
                        } else {
                            parts.join(" · ")
                        }
                    }
                    Err(e) => format!("{e:#}"),
                };
                this.deliveries.insert(id.clone(), text);
                cx.notify();
            },
        );
    }

    pub(super) fn preview_attachment(&mut self, index: usize, cx: &mut Context<Self>) {
        let Some(m) = self.selected_mail() else { return };
        let Some(a) = self.account_of(&m) else { return };
        let Some(part) = m.attachments().get(index).map(|x| x.part) else { return };
        let key = (m.id.clone(), index);
        self.task(
            cx,
            move |b| b.preview_attached(&a, &m, part),
            move |this, result, cx| {
                let text = result.unwrap_or_else(|e| format!("{e:#}"));
                let previews = std::rc::Rc::make_mut(&mut this.attached_previews);
                if previews.len() >= super::READER_CACHE {
                    previews.clear();
                }
                previews.insert(key.clone(), text);
                cx.notify();
            },
        );
    }

    /// Other JMAP accounts sharing the open message's account's sign-in:
    /// (id, label).
    pub(super) fn jmap_copy_targets(&self) -> Vec<(String, String)> {
        let Some(m) = self.selected_mail() else { return Vec::new() };
        let Some(from) = self.accounts.iter().find(|a| a.id == m.account) else {
            return Vec::new();
        };
        if from.provider != crate::manage::model::Provider::Jmap {
            return Vec::new();
        }
        let owner = |a: &Account| {
            if a.credentials_of.is_empty() { a.id.clone() } else { a.credentials_of.clone() }
        };
        let mine = owner(from);
        self.accounts
            .iter()
            .filter(|a| a.id != from.id && a.provider == crate::manage::model::Provider::Jmap && owner(a) == mine)
            .map(|a| (a.id.clone(), if a.name.is_empty() { a.email.clone() } else { a.name.clone() }))
            .collect()
    }

    pub(super) fn copy_to_jmap_account(&mut self, to: String, cx: &mut Context<Self>) {
        let Some(m) = self.selected_mail() else { return };
        let Some(from) = self.account_of(&m) else { return };
        let Some(target) = self.accounts.iter().find(|a| a.id == to).cloned() else { return };
        self.task(
            cx,
            move |b| b.copy_between_jmap(&from, &[m], &target, &Mailbox::Standard(Folder::Inbox)),
            |this, result, cx| match result {
                Ok(n) => this.notice(t!("jmap-extras-copied-to-the-other-accounts-inbox", n = n), false, cx),
                Err(e) => this.notice(format!("{e:#}"), true, cx),
            },
        );
    }

    /// Once per account and session: notes the server's thread state, and
    /// then with each mail check (`refresh_jmap_threads`) asks which
    /// conversations changed (Thread/changes), so an open conversation
    /// that gained messages elsewhere is completed again.
    fn watch_jmap_threads(&mut self, account: &str, cx: &mut Context<Self>) {
        if self.thread_states.contains_key(account) {
            return;
        }
        let Some(a) = self.accounts.iter().find(|a| a.id == account).cloned() else { return };
        self.thread_states.insert(account.to_owned(), String::new());
        let id = account.to_owned();
        self.task(
            cx,
            move |b| b.jmap_thread_changes(&a, ""),
            move |this, result, _| {
                if let Ok(Some((_, _, state))) = result {
                    this.thread_states.insert(id.clone(), state);
                }
            },
        );
    }
    /// With each mail check: conversations that changed on JMAP servers are
    /// completed again when next opened, and sent messages whose delivery
    /// changed show it afresh.
    pub(super) fn refresh_jmap_threads(&mut self, cx: &mut Context<Self>) {
        for (account, since) in self.thread_states.clone() {
            if since.is_empty() {
                continue;
            }
            let Some(a) = self.accounts.iter().find(|a| a.id == account).cloned() else { continue };
            let deliveries = self.delivery_states.get(&account).cloned();
            self.task(
                cx,
                move |b| {
                    let threads = b.jmap_thread_changes(&a, &since)?;
                    let delivered = match deliveries.filter(|d| !d.is_empty()) {
                        Some(d) => b.jmap_delivery_changes(&a, &d).ok().flatten(),
                        None => None,
                    };
                    Ok((threads, delivered))
                },
                move |this, result: anyhow::Result<Refreshed>, cx| {
                    let Ok((threads, delivered)) = result else { return };
                    match threads {
                        Some((changed, gone, state)) => {
                            for t in changed.iter().chain(&gone) {
                                this.threads_completed.remove(&format!("jmap:{t}"));
                            }
                            this.thread_states.insert(account.clone(), state);
                        }
                        // Too much changed to say: everything again.
                        None => {
                            this.threads_completed.clear();
                            this.thread_states.remove(&account);
                        }
                    }
                    if let Some((message_ids, state)) = delivered {
                        let bare = |id: &str| id.trim().trim_start_matches('<').trim_end_matches('>').to_ascii_lowercase();
                        let wanted: Vec<String> = message_ids.iter().map(|i| bare(i)).collect();
                        let stale: Vec<String> = this
                            .messages
                            .iter()
                            .filter(|m| wanted.contains(&bare(&m.message_id)))
                            .map(|m| m.id.clone())
                            .collect();
                        for id in stale {
                            this.deliveries.remove(&id);
                        }
                        this.delivery_states.insert(account.clone(), state);
                    }
                    cx.notify();
                },
            );
        }
    }

    fn load_server_identities(&mut self, a: Account, cx: &mut Context<Self>) {
        let id = a.id.clone();
        self.task(
            cx,
            move |b| b.jmap_identities(&a),
            move |this, result, cx| {
                this.server_identities = Some(match result {
                    Ok(list) => ServerIdentities { account: id.clone(), list, error: None },
                    Err(e) => ServerIdentities { account: id.clone(), list: Vec::new(), error: Some(format!("{e:#}")) },
                });
                cx.notify();
            },
        );
    }

    fn server_identity_job(
        &mut self,
        a: Account,
        cx: &mut Context<Self>,
        job: impl FnOnce(&Backend, &Account) -> anyhow::Result<()> + Send + 'static,
    ) {
        let again = a.clone();
        self.task(
            cx,
            move |b| job(&b, &a),
            move |this, result, cx| {
                match result {
                    Ok(()) => this.load_server_identities(again, cx),
                    Err(e) => {
                        if let Some(s) = this.server_identities.as_mut() {
                            s.error = Some(format!("{e:#}"));
                        }
                    }
                }
                cx.notify();
            },
        );
    }
}
