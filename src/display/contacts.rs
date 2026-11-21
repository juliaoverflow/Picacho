//! Settings → Contacts: the address book the composer suggests from. A
//! CardDAV server (Fastmail, iCloud, Nextcloud, Radicale…) is read from, and
//! vCard files can be imported and the whole book exported.
use super::*;

impl MailApp {
    pub(super) fn contacts_page(&self, cx: &Context<Self>) -> AnyElement {
        let c = self.colors();
        let cards = self.backend.contacts();
        let server = self.backend.contacts_server();
        let busy = self.contacts_busy;
        let label = |text: &'static str| {
            div()
                .text_size(px(12.))
                .font_weight(FontWeight::MEDIUM)
                .text_color(rgb(c.muted))
                .child(text)
        };
        let count = crate::manage::backend::contacts::people(&cards).len();
        let mut page = div()
            .flex()
            .flex_col()
            .gap_5()
            .child(self.section("Contacts"))
            .child(
                div()
                    .id("contacts-summary")
                    .text_size(px(12.))
                    .text_color(rgb(c.muted))
                    .child(format!(
                        "{} in your address book. The composer suggests them first, then people from your mail.",
                        match count {
                            0 => "No addresses".to_owned(),
                            1 => "1 address".to_owned(),
                            n => format!("{n} addresses"),
                        }
                    )),
            )
            .child(self.section("Contacts server (CardDAV)"));
        page = if let Some(server) = server {
            let synced = self
                .backend
                .contacts_synced()
                .and_then(|t| chrono::DateTime::from_timestamp(t, 0))
                .map(|t| {
                    t.with_timezone(&chrono::Local)
                        .format("%b %-d, %H:%M")
                        .to_string()
                })
                .unwrap_or_else(|| "never".into());
            page.child(
                self.setting(
                    &server.url,
                    &format!(
                        "Signed in as {}. Read {synced}; read again each time Picacho starts.",
                        server.user
                    ),
                    div()
                        .flex()
                        .gap_2()
                        .child(
                            self.button(
                                "contacts-sync",
                                if busy { "Reading…" } else { "Read Now" },
                                "refresh",
                                Action::SyncContacts,
                                false,
                                cx,
                            )
                            .when(busy, |d| d.opacity(0.6)),
                        )
                        .child(self.button(
                            "contacts-disconnect",
                            "Disconnect",
                            "close",
                            Action::DisconnectContacts,
                            false,
                            cx,
                        )),
                ),
            )
        } else {
            page.child(
                div()
                    .flex()
                    .flex_col()
                    .gap_3()
                    .max_w(px(420.))
                    .child(
                        div()
                            .text_size(px(12.))
                            .text_color(rgb(c.muted))
                            .child("Read your address books from Fastmail, iCloud, Nextcloud, Radicale or another CardDAV server. They are only read; edit contacts where they're kept. iCloud and Fastmail need an app password."),
                    )
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .gap_1()
                            .child(label("Server"))
                            .child(
                                div()
                                    .id("contacts-url")
                                    .child(field(Input::new(&self.contacts_url), c, busy)),
                            ),
                    )
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .gap_1()
                            .child(label("User name"))
                            .child(
                                div()
                                    .id("contacts-user")
                                    .child(field(Input::new(&self.contacts_user), c, busy)),
                            ),
                    )
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .gap_1()
                            .child(label("Password"))
                            .child(
                                div()
                                    .id("contacts-password")
                                    .child(field(Input::new(&self.contacts_password), c, busy)),
                            )
                            .child(
                                div()
                                    .text_size(px(11.))
                                    .text_color(rgb(c.muted))
                                    .child("Kept in the system keychain."),
                            ),
                    )
                    .child(
                        div().flex().child(
                            self.button(
                                "contacts-connect",
                                if busy { "Connecting…" } else { "Connect" },
                                "plus",
                                Action::ConnectContacts,
                                true,
                                cx,
                            )
                            .when(busy, |d| d.opacity(0.6)),
                        ),
                    ),
            )
        };
        page = page.when_some(self.contacts_error.clone(), |d, e| {
            d.child(
                div()
                    .id("contacts-error")
                    .text_size(px(12.))
                    .text_color(rgb(c.danger))
                    .child(e),
            )
        });
        page.child(self.people_section(&cards, cx))
            .child(self.section("Files"))
            .child(self.setting(
                "vCard files",
                "Import contacts exported from another address book (.vcf), or save yours as one.",
                div()
                    .flex()
                    .flex_wrap()
                    .gap_2()
                    .child(self.button(
                        "contacts-import",
                        "Import…",
                        "upload",
                        Action::ImportContacts,
                        false,
                        cx,
                    ))
                    .child(self.button(
                        "contacts-export",
                        "Export…",
                        "download",
                        Action::ExportContacts,
                        false,
                        cx,
                    ))
                    .when(cards.iter().any(|c| !crate::manage::backend::contacts::from_server(c)), |d| {
                        d.child(self.button(
                            "contacts-clear",
                            "Remove Those on This Computer",
                            "trash",
                            Action::ClearImportedContacts,
                            false,
                            cx,
                        ))
                    }),
            ))
            .into_any_element()
    }

    /// Handles Contacts settings' actions.
    pub(super) fn contacts_action(
        &mut self,
        action: Action,
        w: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match action {
            Action::ConnectContacts => {
                let server = crate::sync::carddav::Server {
                    url: self.contacts_url.read(cx).value().trim().to_owned(),
                    user: self.contacts_user.read(cx).value().trim().to_owned(),
                    book: String::new(),
                };
                let password = self.contacts_password.read(cx).value().to_string();
                // Not kept on screen; the keychain has it once it works.
                self.contacts_password
                    .update(cx, |input, cx| input.set_text("", w, cx));
                self.contacts_run(cx, move |b| {
                    b.connect_contacts(server, &password)
                        .map(|n| format!("Read {n} contacts"))
                });
            }
            Action::SyncContacts => {
                self.contacts_run(cx, |b| {
                    b.sync_contacts().map(|n| format!("Read {n} contacts"))
                });
            }
            Action::DisconnectContacts => {
                self.contacts_run(cx, |b| {
                    b.disconnect_contacts()
                        .map(|()| "Contacts server disconnected".into())
                });
            }
            Action::ClearImportedContacts => {
                self.contacts_run(cx, |b| {
                    b.clear_imported_contacts()
                        .map(|()| "Imported contacts removed".into())
                });
            }
            Action::ImportContacts => {
                let picker = cx.prompt_for_paths(PathPromptOptions {
                    files: true,
                    directories: false,
                    multiple: true,
                    prompt: Some("Import".into()),
                });
                cx.spawn(async move |this, cx| {
                    if let Ok(Ok(Some(paths))) = picker.await {
                        let _ = this.update(cx, |this, cx| {
                            this.contacts_run(cx, move |b| {
                                b.import_contacts(&paths)
                                    .map(|n| format!("Imported {n} contacts"))
                            });
                        });
                    }
                })
                .detach();
            }
            Action::ExportContacts => {
                let start = self.backend.store.path.clone();
                let picker = cx.prompt_for_new_path(&start, Some("Contacts.vcf"));
                cx.spawn(async move |this, cx| {
                    if let Ok(Ok(Some(path))) = picker.await {
                        let _ = this.update(cx, |this, cx| {
                            this.contacts_run(cx, move |b| {
                                b.export_contacts(&path)
                                    .map(|n| format!("Exported {n} contacts"))
                            });
                        });
                    }
                })
                .detach();
            }
            Action::NewContact => self.edit_contact(None, w, cx),
            Action::EditContact(uid) => {
                let card = self.backend.contacts().into_iter().find(|c| c.uid == uid);
                self.edit_contact(card, w, cx);
            }
            Action::CancelContactEdit => self.contact_editing = None,
            Action::SaveContact => {
                let Some(uid) = self.contact_editing.clone() else {
                    return;
                };
                let value =
                    |i: &Entity<InputState>, cx: &Context<Self>| i.read(cx).value().to_string();
                let list = |text: String| -> Vec<String> {
                    text.split([',', ';', '\n'])
                        .map(|p| p.trim().to_owned())
                        .filter(|p| !p.is_empty())
                        .collect()
                };
                let mut card = self
                    .backend
                    .contacts()
                    .into_iter()
                    .find(|c| !uid.is_empty() && c.uid == uid)
                    .unwrap_or_default();
                card.name = value(&self.contact_name, cx);
                card.emails = list(value(&self.contact_emails, cx));
                card.phones = list(value(&self.contact_phones, cx));
                card.org = value(&self.contact_org, cx).trim().to_owned();
                card.note = value(&self.contact_note, cx).trim().to_owned();
                self.contact_editing = None;
                self.contacts_run(cx, move |b| {
                    b.save_contact(card).map(|c| format!("Saved {}", c.name))
                });
            }
            Action::RemoveContact(uid) => {
                self.contact_editing = None;
                self.contacts_run(cx, move |b| {
                    b.delete_contact(&uid).map(|()| "Contact deleted".into())
                });
            }
            _ => {}
        }
    }

    fn contacts_run(
        &mut self,
        cx: &mut Context<Self>,
        job: impl FnOnce(Arc<Backend>) -> anyhow::Result<String> + Send + 'static,
    ) {
        if self.contacts_busy {
            return;
        }
        self.contacts_busy = true;
        self.contacts_error = None;
        self.task(cx, job, |this, result, cx| {
            this.contacts_busy = false;
            // Suggestions are gathered again with the new address book.
            this.contacts_cache.borrow_mut().take();
            match result {
                Ok(done) => this.notice(done, false, cx),
                Err(e) => this.contacts_error = Some(format!("{e:#}")),
            }
        });
    }

    /// Reads the address book from its server once at startup, quietly.
    pub(super) fn sync_contacts_at_start(&mut self, cx: &mut Context<Self>) {
        if self.backend.contacts_server().is_none() {
            return;
        }
        self.task(
            cx,
            |b| b.sync_contacts(),
            |this, result, _| {
                if result.is_ok() {
                    this.contacts_cache.borrow_mut().take();
                }
            },
        );
    }

    fn edit_contact(
        &mut self,
        card: Option<crate::sync::carddav::Card>,
        w: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let card = card.unwrap_or_default();
        for (input, text) in [
            (&self.contact_name, card.name.clone()),
            (&self.contact_emails, card.emails.join(", ")),
            (&self.contact_phones, card.phones.join(", ")),
            (&self.contact_org, card.org.clone()),
            (&self.contact_note, card.note.clone()),
        ] {
            input.update(cx, |i, cx| i.set_text(text, w, cx));
        }
        self.contacts_error = None;
        self.contact_editing = Some(card.uid);
    }

    /// The address book: a filter, the people in it (each opens for
    /// editing), and the form for the one being added or edited.
    fn people_section(
        &self,
        cards: &[crate::sync::carddav::Card],
        cx: &Context<Self>,
    ) -> AnyElement {
        let c = self.colors();
        let busy = self.contacts_busy;
        let filter = self.contact_filter.read(cx).value().trim().to_lowercase();
        let mut shown: Vec<&crate::sync::carddav::Card> = cards
            .iter()
            .filter(|card| {
                filter.is_empty()
                    || card.name.to_lowercase().contains(&filter)
                    || card
                        .emails
                        .iter()
                        .any(|e| e.to_lowercase().contains(&filter))
                    || card.org.to_lowercase().contains(&filter)
            })
            .collect();
        shown.sort_by_key(|card| card.name.to_lowercase());
        let total = shown.len();
        shown.truncate(200);
        let labelled = |text: &'static str, id: &'static str, input: &Entity<InputState>| {
            div()
                .flex()
                .flex_col()
                .gap_1()
                .child(
                    div()
                        .text_size(px(12.))
                        .font_weight(FontWeight::MEDIUM)
                        .text_color(rgb(c.muted))
                        .child(text),
                )
                .child(div().id(id).child(field(Input::new(input), c, busy)))
        };
        let editor = self.contact_editing.as_ref().map(|uid| {
            div()
                .id("contact-editor")
                .role(Role::Form)
                .aria_label(if uid.is_empty() {
                    "New contact"
                } else {
                    "Edit contact"
                })
                .p_4()
                .rounded(px(12.))
                .border_1()
                .border_color(rgb(c.border))
                .bg(rgb(c.raised))
                .flex()
                .flex_col()
                .gap_3()
                .max_w(px(480.))
                .child(
                    div()
                        .text_size(px(14.))
                        .font_weight(FontWeight::SEMIBOLD)
                        .child(if uid.is_empty() {
                            "New Contact"
                        } else {
                            "Edit Contact"
                        }),
                )
                .child(labelled("Name", "contact-name", &self.contact_name))
                .child(labelled(
                    "Email addresses (separated by commas)",
                    "contact-emails",
                    &self.contact_emails,
                ))
                .child(labelled(
                    "Phone numbers",
                    "contact-phones",
                    &self.contact_phones,
                ))
                .child(labelled("Organization", "contact-org", &self.contact_org))
                .child(labelled("Note", "contact-note", &self.contact_note))
                .child(div().text_size(px(11.)).text_color(rgb(c.muted)).child(
                    if self.backend.contacts_server().is_some() {
                        "Saved to your contacts server."
                    } else {
                        "Saved on this computer."
                    },
                ))
                .child(
                    div()
                        .flex()
                        .flex_wrap()
                        .gap_2()
                        .child(self.button(
                            "contact-save",
                            "Save",
                            "check",
                            Action::SaveContact,
                            true,
                            cx,
                        ))
                        .child(self.button(
                            "contact-cancel",
                            "Cancel",
                            "close",
                            Action::CancelContactEdit,
                            false,
                            cx,
                        ))
                        .when(!uid.is_empty(), |d| {
                            d.child(self.button(
                                "contact-delete",
                                "Delete",
                                "trash",
                                Action::RemoveContact(uid.clone()),
                                false,
                                cx,
                            ))
                        }),
                )
        });
        let list = div()
            .id("contacts-list")
            .role(Role::List)
            .aria_label("Contacts")
            .rounded(px(12.))
            .border_1()
            .border_color(rgb(c.border))
            .bg(rgb(c.raised))
            .overflow_hidden()
            .flex()
            .flex_col();
        let list = if shown.is_empty() {
            list.child(
                div()
                    .px_4()
                    .py_3()
                    .text_size(px(12.))
                    .text_color(rgb(c.muted))
                    .child(if cards.is_empty() {
                        "No contacts yet."
                    } else {
                        "No one matches."
                    }),
            )
        } else {
            list.children(shown.into_iter().enumerate().map(|(i, card)| {
                let detail = [card.emails.join(", "), card.org.clone()]
                    .into_iter()
                    .filter(|s| !s.is_empty())
                    .collect::<Vec<_>>()
                    .join(" · ");
                let name = if card.name.is_empty() {
                    card.emails.first().cloned().unwrap_or_default()
                } else {
                    card.name.clone()
                };
                let uid = card.uid.clone();
                div()
                    .id(SharedString::from(format!("contact-{i}")))
                    .role(Role::ListItem)
                    .aria_label(format!("{name}, {detail}"))
                    .aria_position_in_set(i + 1)
                    .aria_size_of_set(total)
                    .focusable()
                    .tab_stop(true)
                    .focus_visible(move |s| s.border_2().border_color(rgb(c.accent)))
                    .px_4()
                    .py(px(10.))
                    .border_b_1()
                    .border_color(rgb(c.border))
                    .cursor_pointer()
                    .hover(move |s| s.bg(rgb(c.surface)))
                    .flex()
                    .items_center()
                    .gap_3()
                    .child(icon_el("contact", 16., c.muted))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .flex()
                            .flex_col()
                            .child(
                                div()
                                    .text_size(px(13.))
                                    .font_weight(FontWeight::MEDIUM)
                                    .truncate()
                                    .child(name),
                            )
                            .child(
                                div()
                                    .text_size(px(11.5))
                                    .text_color(rgb(c.muted))
                                    .truncate()
                                    .child(detail),
                            ),
                    )
                    .on_click(cx.listener(move |this, _, w, cx| {
                        this.handle(Action::EditContact(uid.clone()), w, cx)
                    }))
            }))
        };
        div()
            .flex()
            .flex_col()
            .gap_3()
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_2()
                    .child(div().flex_1().child(self.section("People")))
                    .when(self.contact_editing.is_none(), |d| {
                        d.child(self.button(
                            "contact-new",
                            "New Contact",
                            "plus",
                            Action::NewContact,
                            false,
                            cx,
                        ))
                    }),
            )
            .children(editor)
            .when(cards.len() > 8, |d| {
                d.child(div().id("contact-filter").max_w(px(320.)).child(field(
                    Input::new(&self.contact_filter),
                    c,
                    false,
                )))
            })
            .child(list)
            .when(total > 200, |d| {
                d.child(
                    div()
                        .text_size(px(11.5))
                        .text_color(rgb(c.muted))
                        .child(format!("Showing 200 of {total}. Type to narrow them down.")),
                )
            })
            .into_any_element()
    }
}
