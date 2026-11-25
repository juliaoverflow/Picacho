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
        let count = crate::manage::backend::contacts::people(&cards).len();
        let server_rows = if let Some(server) = server {
            let synced = self
                .backend
                .contacts_synced()
                .and_then(|t| chrono::DateTime::from_timestamp(t, 0))
                .map(|t| {
                    t.with_timezone(&chrono::Local)
                        .local(t!("dates-month-day-time"))
                })
                .unwrap_or_else(|| t!("contacts-never").to_owned());
            let account = self
                .backend
                .contacts_mail_account()
                .filter(|_| server.jmap_account.is_empty())
                .and_then(|id| self.accounts.iter().find(|a| a.id == id));
            Rows::new()
                .row(self.setting_row(
                    &server.url,
                    &t!(
                        "contacts-signed-in-as-read-read-again",
                        how = if account.is_some() { "account" } else { "password" },
                        user = server.user,
                        email = account.map(|a| a.email.as_str()).unwrap_or_default(),
                        synced = synced
                    ),
                    div()
                        .flex()
                        .flex_wrap()
                        .gap_2()
                        .child(
                            self.button(
                                "contacts-sync",
                                if busy { t!("contacts-reading") } else { t!("contacts-read-now") },
                                "refresh",
                                Action::SyncContacts,
                                false,
                                cx,
                            )
                            .when(busy, |d| d.opacity(0.6)),
                        )
                        .child(self.button(
                            "contacts-disconnect",
                            t!("contacts-disconnect"),
                            "close",
                            Action::DisconnectContacts,
                            false,
                            cx,
                        )),
                ))
                .row(self.setting_row(
                    t!("contacts-new-address-book"),
                    "",
                    div()
                        .flex()
                        .flex_wrap()
                        .items_center()
                        .gap_2()
                        .child(
                            div()
                                .id("address-book-name")
                                .w(px(200.))
                                .max_w_full()
                                .child(field(Input::new(&self.collection_name), c, busy)),
                        )
                        .child(self.button(
                            "address-book-create",
                            t!("contacts-make-address-book"),
                            "plus",
                            Action::CreateAddressBook,
                            false,
                            cx,
                        )),
                ))
                .row(self.address_books_list(&server, cx))
        } else {
            Rows::new().row(
                div()
                    .flex()
                    .flex_col()
                    .gap_3()
                    .max_w(px(420.))
                    .child(
                        div()
                            .text_size(px(12.))
                            .text_color(rgb(c.muted))
                            .child(t!("contacts-read-and-edit-your-address-books")),
                    )
                    .children(self.jmap_sources("contacts-jmap", Action::ConnectJmapContacts, cx))
                    .child(labelled(t!("contacts-server"), "contacts-url", &self.contacts_url))
                    .child(labelled(t!("contacts-user-name"), "contacts-user", &self.contacts_user))
                    .child(labelled(t!("contacts-password"), "contacts-password", &self.contacts_password))
                    .child(
                        div().flex().child(
                            self.button(
                                "contacts-connect",
                                if busy { t!("contacts-connecting") } else { t!("contacts-connect") },
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
        let server_rows = server_rows.when_some(self.contacts_error.clone(), |rows, e| {
            rows.row(
                div()
                    .id("contacts-error")
                    .text_size(px(12.))
                    .text_color(rgb(c.danger))
                    .child(e),
            )
        });
        let files = Rows::new().row(self.setting_row(
            t!("contacts-vcard-files"),
            "",
            div()
                .flex()
                .flex_wrap()
                .gap_2()
                .child(self.button(
                    "contacts-import",
                    t!("contacts-import"),
                    "upload",
                    Action::ImportContacts,
                    false,
                    cx,
                ))
                .child(self.button(
                    "contacts-export",
                    t!("contacts-export"),
                    "download",
                    Action::ExportContacts,
                    false,
                    cx,
                ))
                .when(cards.iter().any(|c| !crate::manage::backend::contacts::from_server(c)), |d| {
                    d.child(self.button(
                        "contacts-clear",
                        t!("contacts-remove-those-on-this-computer"),
                        "trash",
                        Action::ClearImportedContacts,
                        false,
                        cx,
                    ))
                }),
        ));
        self.settings_column()
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap_2()
                    .child(self.people_section(&cards, cx))
                    .child(
                        div()
                            .id("contacts-summary")
                            .px_1()
                            .text_size(px(12.))
                            .text_color(rgb(c.muted))
                            .child(t!("contacts-in-your-address-book-the-composer", count = count)),
                    ),
            )
            .child(self.settings_group(t!("contacts-contacts-server-carddav"), server_rows))
            .child(self.settings_group(t!("contacts-files"), files))
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
                    ..Default::default()
                };
                let password = self.contacts_password.read(cx).value().to_string();
                // Not kept on screen; the keychain has it once it works.
                self.contacts_password
                    .update(cx, |input, cx| input.set_text("", w, cx));
                self.contacts_run(cx, move |b| {
                    b.connect_contacts(server, &password)
                        .map(|n| t!("contacts-read-contacts", n = n))
                });
            }
            Action::SyncContacts => {
                self.contacts_run(cx, |b| {
                    b.sync_contacts().map(|n| t!("contacts-read-contacts", n = n))
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
                    prompt: Some(t!("contacts-import-2").into()),
                });
                cx.spawn(async move |this, cx| {
                    if let Ok(Ok(Some(paths))) = picker.await {
                        let _ = this.update(cx, |this, cx| {
                            this.contacts_run(cx, move |b| {
                                b.import_contacts(&paths)
                                    .map(|n| t!("contacts-imported-contacts", n = n))
                            });
                        });
                    }
                })
                .detach();
            }
            Action::ExportContacts => {
                let start = self.save_start();
                let picker = cx.prompt_for_new_path(&start, Some(t!("contacts-contacts-vcf")));
                cx.spawn(async move |this, cx| {
                    if let Ok(Ok(Some(path))) = picker.await {
                        let _ = this.update(cx, |this, cx| {
                            this.contacts_run(cx, move |b| {
                                b.export_contacts(&path)
                                    .map(|n| t!("contacts-exported-contacts", n = n))
                            });
                        });
                    }
                })
                .detach();
            }
            Action::ConnectJmapContacts(id) => {
                let Some(a) = self.accounts.iter().find(|a| a.id == id).cloned() else {
                    return;
                };
                self.contacts_run(cx, move |b| {
                    let cancel = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
                    let n = b.use_mail_account_for_contacts(&a, cancel)?;
                    Ok(t!("contacts-read-contacts-from", n = n, email = a.email))
                });
            }
            Action::SearchServerContacts => {
                let text = self.contact_filter.read(cx).value().trim().to_owned();
                self.contacts_run(cx, move |b| {
                    let n = b.search_server_contacts(&text)?;
                    Ok(match n {
                        0 => t!("contacts-the-server-has-no-one-else").to_owned(),
                        1 => t!("contacts-found-1-more-on-the-server").to_owned(),
                        n => t!("contacts-found-more-on-the-server", n = n),
                    })
                });
            }
            Action::ListAddressBooks => {
                if self.contacts_busy {
                    return;
                }
                self.contacts_busy = true;
                // The storage with them; not worth failing the list for.
                self.task(cx, |b| Ok((b.address_books()?, b.contacts_quota().ok().flatten())), |this, result, cx| {
                    this.contacts_busy = false;
                    match result {
                        Ok((books, quota)) => {
                            this.address_books = Some(books);
                            this.contacts_quota = quota;
                        }
                        Err(e) => this.contacts_error = Some(format!("{e:#}")),
                    }
                    cx.notify();
                });
            }
            Action::RenameAddressBook(href) => {
                let name = self.collection_name.read(cx).value().trim().to_owned();
                if name.is_empty() {
                    self.contacts_error = Some(t!("contacts-write-the-new-name-in-the").into());
                    cx.notify();
                    return;
                }
                self.collection_name.update(cx, |i, cx| i.set_text("", w, cx));
                if let Some(books) = &mut self.address_books {
                    for b in books.iter_mut().filter(|b| b.0 == href) {
                        b.1 = name.clone();
                    }
                }
                self.contacts_run(cx, move |b| {
                    b.rename_address_book(&href, &name)?;
                    Ok(t!("contacts-renamed-the-address-book", name = name))
                });
            }
            Action::DeleteAddressBook(href) => {
                if let Some(books) = &mut self.address_books {
                    books.retain(|b| b.0 != href);
                }
                self.contacts_run(cx, move |b| {
                    b.delete_address_book(&href)?;
                    Ok(t!("contacts-deleted-the-address-book").into())
                });
            }
            Action::CreateAddressBook => {
                let name = self.collection_name.read(cx).value().trim().to_owned();
                self.collection_name.update(cx, |i, cx| i.set_text("", w, cx));
                self.contacts_run(cx, move |b| {
                    b.create_address_book(&name)?;
                    Ok(t!("contacts-made-the-address-book", name = name))
                });
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
                card.addresses = value(&self.contact_addresses, cx)
                    .split(['\n', ';'])
                    .map(|a| a.trim().to_owned())
                    .filter(|a| !a.is_empty())
                    .collect();
                card.birthday = value(&self.contact_birthday, cx).trim().to_owned();
                card.title = value(&self.contact_title, cx).trim().to_owned();
                card.nicknames = list(value(&self.contact_nicknames, cx));
                card.anniversary = value(&self.contact_anniversary, cx).trim().to_owned();
                // Web pages (http/https) and chat addresses (any other URI).
                // Split at commas and spaces only: a URL may hold a `;`.
                let (urls, impps): (Vec<String>, Vec<String>) = value(&self.contact_links, cx)
                    .split(|c: char| c == ',' || c.is_whitespace())
                    .filter(|l| !l.is_empty())
                    .map(str::to_owned)
                    .partition(|l| l.starts_with("http://") || l.starts_with("https://"));
                (card.urls, card.impps) = (urls, impps);
                // A group's members, by address: the UIDs of those people.
                let members = list(value(&self.contact_members, cx));
                if !members.is_empty() || card.group {
                    let all = self.backend.contacts();
                    card.group = !members.is_empty();
                    card.members = members
                        .iter()
                        .filter_map(|m| {
                            all.iter()
                                .find(|c| c.emails.iter().any(|e| e.eq_ignore_ascii_case(m)) || c.name.eq_ignore_ascii_case(m))
                                .map(|c| c.uid.clone())
                        })
                        .collect();
                }
                self.contact_editing = None;
                self.contacts_run(cx, move |b| {
                    b.save_contact(card).map(|c| t!("contacts-saved", name = c.name))
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

    /// The server's address books, with Rename (to the name in the field
    /// above) and Delete; listed when asked.
    fn address_books_list(&self, server: &crate::sync::carddav::Server, cx: &Context<Self>) -> AnyElement {
        let c = self.colors();
        let Some(books) = &self.address_books else {
            return self
                .setting(
                    t!("contacts-address-books"),
                    t!("contacts-rename-or-delete-the-servers-address"),
                    self.button("address-books-list", t!("contacts-show"), "list", Action::ListAddressBooks, false, cx),
                )
                .into_any_element();
        };
        let mut list = div().id("address-books").flex().flex_col().gap_2();
        for (href, name, writable) in books {
            let default = server.book == *href;
            list = list.child(
                div()
                    .flex()
                    .items_center()
                    .gap_2()
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .truncate()
                            .text_size(px(13.))
                            .child(if default {
                                t!("contacts-new-contacts-go-here", name = name)
                            } else {
                                name.clone()
                            }),
                    )
                    .when(*writable, |d| {
                        d.child(self.button(
                            format!("address-book-rename-{href}"),
                            t!("contacts-rename"),
                            "",
                            Action::RenameAddressBook(href.clone()),
                            false,
                            cx,
                        ))
                        .child(
                            self.button(
                                format!("address-book-delete-{href}"),
                                t!("contacts-delete"),
                                "",
                                Action::DeleteAddressBook(href.clone()),
                                false,
                                cx,
                            )
                            .text_color(rgb(c.danger)),
                        )
                    }),
            );
        }
        let storage = self.contacts_quota.map(|(used, left)| {
            let mb = |b: u64| b as f64 / 1_048_576.0;
            t!("contacts-the-server-keeps-mb-of-yours", used = format!("{:.0}", mb(used)), left = format!("{:.0}", mb(left)))
        });
        self.setting(
            t!("contacts-address-books"),
            &{
                    let rename = t!("contacts-rename-uses-the-name-written-in");
                    match storage {
                        Some(storage) => crate::i18n::sentences(rename, &storage),
                        None => rename.to_owned(),
                    }
                },
            list,
        )
        .into_any_element()
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
            (&self.contact_addresses, card.addresses.join("\n")),
            (&self.contact_birthday, card.birthday.clone()),
            (&self.contact_title, card.title.clone()),
            (&self.contact_nicknames, card.nicknames.join(", ")),
            (&self.contact_links, card.urls.iter().chain(&card.impps).cloned().collect::<Vec<_>>().join(", ")),
            (&self.contact_anniversary, card.anniversary.clone()),
            (&self.contact_members, {
                let all = self.backend.contacts();
                card.members
                    .iter()
                    .filter_map(|uid| {
                        let c = all.iter().find(|c| &c.uid == uid)?;
                        c.emails.first().cloned().or_else(|| Some(c.name.clone()))
                    })
                    .collect::<Vec<_>>()
                    .join(", ")
            }),
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
                    t!("contacts-new-contact")
                } else {
                    t!("contacts-edit-contact")
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
                            t!("contacts-new-contact-2")
                        } else {
                            t!("contacts-edit-contact-2")
                        }),
                )
                .child(labelled(t!("contacts-name"), "contact-name", &self.contact_name))
                .child(labelled(
                    t!("contacts-email-addresses-separated-by-commas"),
                    "contact-emails",
                    &self.contact_emails,
                ))
                .child(labelled(
                    t!("contacts-phone-numbers"),
                    "contact-phones",
                    &self.contact_phones,
                ))
                .child(labelled(t!("contacts-organization"), "contact-org", &self.contact_org))
                .child(labelled(t!("contacts-note"), "contact-note", &self.contact_note))
                .child(labelled(t!("contacts-postal-addresses"), "contact-addresses", &self.contact_addresses))
                .child(labelled(t!("contacts-birthday"), "contact-birthday", &self.contact_birthday))
                .child(labelled(t!("contacts-job-title"), "contact-title", &self.contact_title))
                .child(labelled(t!("contacts-nicknames"), "contact-nicknames", &self.contact_nicknames))
                .child(labelled(t!("contacts-web-and-chat"), "contact-links", &self.contact_links))
                .child(labelled(t!("contacts-anniversary"), "contact-anniversary", &self.contact_anniversary))
                .child(labelled(
                    t!("contacts-members-makes-it-a-group"),
                    "contact-members",
                    &self.contact_members,
                ))
                .child(div().text_size(px(11.)).text_color(rgb(c.muted)).child(
                    if self.backend.contacts_server().is_some() {
                        t!("contacts-saved-to-your-contacts-server")
                    } else {
                        t!("contacts-saved-on-this-computer")
                    },
                ))
                .child(
                    div()
                        .flex()
                        .flex_wrap()
                        .gap_2()
                        .child(self.button(
                            "contact-save",
                            t!("contacts-save"),
                            "check",
                            Action::SaveContact,
                            true,
                            cx,
                        ))
                        .child(self.button(
                            "contact-cancel",
                            t!("contacts-cancel"),
                            "close",
                            Action::CancelContactEdit,
                            false,
                            cx,
                        ))
                        .when(!uid.is_empty(), |d| {
                            d.child(self.button(
                                "contact-delete",
                                t!("contacts-delete"),
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
            .aria_label(t!("contacts-contacts"))
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
                        t!("contacts-no-contacts-yet")
                    } else {
                        t!("contacts-no-one-matches")
                    }),
            )
            // A CardDAV server can be asked for more (RFC 6352 §8.6).
            .when(
                !filter.is_empty()
                    && self.backend.contacts_server().is_some_and(|s| s.jmap_account.is_empty()),
                |d| {
                    d.child(div().px_4().pb_3().child(self.button(
                        "contacts-search-server",
                        t!("contacts-search-the-server"),
                        "search",
                        Action::SearchServerContacts,
                        busy,
                        cx,
                    )))
                },
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
                    .child(match card.photo_bytes().and_then(|b| photo_image(&b)) {
                        Some(image) => img(image)
                            .size(px(24.))
                            .rounded_full()
                            .into_any_element(),
                        None => icon_el(if card.group { "user" } else { "contact" }, 16., c.muted)
                            .into_any_element(),
                    })
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
                    .child(div().flex_1().child(self.section(t!("contacts-people"))))
                    .when(self.contact_editing.is_none(), |d| {
                        d.child(self.button(
                            "contact-new",
                            t!("contacts-new-contact-2"),
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
                        .child(t!("contacts-showing-200-of-type-to-narrow", total = total)),
                )
            })
            .into_any_element()
    }
}

/// A contact's embedded photo as an image to draw, where it decodes.
fn photo_image(bytes: &[u8]) -> Option<std::sync::Arc<gpui::Image>> {
    let format = match bytes {
        [0x89, b'P', b'N', b'G', ..] => gpui::ImageFormat::Png,
        [0xFF, 0xD8, ..] => gpui::ImageFormat::Jpeg,
        [b'G', b'I', b'F', ..] => gpui::ImageFormat::Gif,
        _ => return None,
    };
    Some(std::sync::Arc::new(gpui::Image::from_bytes(format, bytes.to_vec())))
}
