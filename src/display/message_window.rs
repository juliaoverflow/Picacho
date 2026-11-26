//! An email in a window of its own, opened from the list or the reader.
//! It reads like the reader; replies open in compose windows.
use super::*;

/// The root view of a message window.
pub(super) struct MessageWindow {
    mail: Mail,
    app: WeakEntity<MailApp>,
    /// The main window, where the app's composers are created.
    main: AnyWindowHandle,
    html: Entity<HtmlView>,
    colors: Colors,
    options: ReadingOptions,
    app_dark: bool,
    own: Vec<String>,
    loading: bool,
    quotes_open: bool,
    scroll: ScrollHandle,
    focus: FocusHandle,
    /// The link under the pointer, shown at the bottom left.
    hovered: Option<String>,
    /// The HTML as shown (quotes hidden, alias and codes masked), with what
    /// it was made from: made again on every frame, it was the whole body
    /// read and copied several times over for each move of the pointer.
    shown_html: Option<(u64, Rc<String>, bool)>,
    _links: Subscription,
    _reveals: Subscription,
}

impl MessageWindow {
    #[allow(clippy::too_many_arguments)]
    fn new(
        mail: Mail,
        app: WeakEntity<MailApp>,
        main: AnyWindowHandle,
        colors: Colors,
        options: ReadingOptions,
        app_dark: bool,
        own: Vec<String>,
        cx: &mut Context<Self>,
    ) -> Self {
        let loading = mail.body.is_none() && mail.html.is_none();
        let html = cx.new(HtmlView::new);
        // A desktop window: links open at once, as the reader's do there.
        let links = cx.subscribe(&html, |this, _, link: &HtmlLink, cx| match link {
            HtmlLink::Hovered(url) => {
                this.hovered = url.clone();
                cx.notify();
            }
            HtmlLink::Opened(url) => {
                let url = if this.clean_links(cx) {
                    crate::manage::tracking::clean_url(url).unwrap_or_else(|| url.clone())
                } else {
                    url.clone()
                };
                if let Some(parsed) = crate::display::html::openable_url(&url) {
                    // A link with a known trick (a lookalike name, plain
                    // http…) gets the main window's warning, as the
                    // reader's does, unless its site is trusted there.
                    if !super::links::link_dangers(&parsed).is_empty()
                        && let Some(app) = this.app.upgrade()
                    {
                        app.update(cx, |app, cx| app.open_email_link(url, cx));
                        let _ = this.main.update(cx, |_, w, _| w.activate_window());
                    } else {
                        cx.open_url(parsed.as_str());
                    }
                }
            }
        });
        // A `#` link's target, or what Tab focused.
        let reveals = cx.subscribe(&html, |this, _, reveal: &HtmlReveal, cx| {
            crate::display::html::scroll_to_reveal(&this.scroll, reveal);
            cx.notify();
        });
        Self {
            mail,
            app,
            main,
            html,
            colors,
            options,
            app_dark,
            own,
            loading,
            quotes_open: false,
            scroll: ScrollHandle::new(),
            focus: cx.focus_handle(),
            hovered: None,
            shown_html: None,
            _links: links,
            _reveals: reveals,
        }
    }

    /// Whether links lose their tracking, as set in the app's Settings.
    fn clean_links(&self, cx: &App) -> bool {
        self.app.upgrade().is_none_or(|app| app.read(cx).clean_links)
    }

    fn reply(&self, mode: u8, cx: &mut Context<Self>) {
        let (app, mail) = (self.app.clone(), self.mail.clone());
        let _ = self.main.update(cx, move |_, w, cx| {
            let _ = app.update(cx, |app, cx| {
                app.compose_reply(mode, Some(mail), Some(Placement::Window), w, cx)
            });
        });
    }

    fn button(
        &self,
        id: &'static str,
        label: &'static str,
        icon: &'static str,
        cx: &Context<Self>,
        click: impl Fn(&mut Self, &mut Window, &mut Context<Self>) + 'static,
    ) -> Stateful<Div> {
        let c = self.colors;
        div()
            .id(id)
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .h(px(28.))
            .px_2()
            .flex()
            .items_center()
            .gap_2()
            .rounded(px(7.))
            .cursor_pointer()
            .text_size(px(12.))
            .font_weight(FontWeight::MEDIUM)
            .text_color(rgb(c.muted))
            .hover(move |s| s.bg(rgb(c.raised)).text_color(rgb(c.text)))
            .child(icon_el(icon, 15., c.muted))
            .when(!label.is_empty(), |d| d.child(label))
            .on_click(cx.listener(move |this, _, w, cx| click(this, w, cx)))
    }
}

impl Render for MessageWindow {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let c = self.colors;
        if super::applock::covered(cx) {
            return super::applock::cover(c, window);
        }
        let m = &self.mail;
        // Sign-in codes found by Local AI are hidden here too.
        let codes = self
            .app
            .upgrade()
            .map(|app| app.read(cx).hidden_codes(m))
            .unwrap_or_default();
        let subject = crate::manage::local_ai::otp::mask(&m.subject, &codes).into_owned();
        super::window::set_title(window, &subject);
        // A mailbox kept to its aliases shows the alias used in its place.
        let mask = self.app.upgrade().and_then(|app| app.read(cx).mask_for(m));
        let (sender, from) = match &mask {
            Some(k) => (
                if k.sent { k.sender() } else { m.sender.clone() },
                k.apply(&m.from).into_owned(),
            ),
            None => (m.sender.clone(), m.from.clone()),
        };
        let dark = self.options.theme.is_dark(self.app_dark);
        // Reverse aliases a mailbox kept to its aliases wrote to show as
        // the contacts they forward to.
        let unreversed = |list: &str| match &mask {
            Some(k) => crate::manage::forwarding::unreverse(list, &k.contacts).into_owned(),
            None => list.to_owned(),
        };
        let to = super::thread::short_names(&unreversed(&m.to), &self.own);
        let cc = super::thread::short_names(&unreversed(m.cc()), &self.own);
        let mut body = div().flex().flex_col();
        let mut has_quotes = false;
        if self.loading {
            body = body.child(
                div()
                    .h(px(160.))
                    .flex()
                    .items_center()
                    .justify_center()
                    .child(spinner(22., c.muted)),
            );
        } else if self.options.html
            && let Some(html) = &m.html
        {
            let key = {
                use std::hash::{Hash, Hasher};
                let mut h = std::collections::hash_map::DefaultHasher::new();
                // The window's message is fixed (its HTML with it): what it
                // is shown with is what can change. The length too, should it
                // be filled in (downloaded) later.
                (html.len(), &m.id, self.quotes_open, codes.as_slice(), mask.is_some()).hash(&mut h);
                h.finish()
            };
            if self.shown_html.as_ref().is_none_or(|(k, ..)| *k != key) {
                let hidden = super::thread::hide_html_quotes(html);
                let quotes = hidden.is_some();
                let shown = match &hidden {
                    Some(h) if !self.quotes_open => h.as_str(),
                    _ => html.as_str(),
                };
                let masked = mask.as_ref().map(|k| k.apply(shown));
                let shown = masked.as_deref().unwrap_or(shown);
                let shown = Rc::new(crate::manage::local_ai::otp::mask_html(shown, &codes).into_owned());
                self.shown_html = Some((key, shown, quotes));
            }
            let (_, shown, quotes) = self.shown_html.as_ref().expect("made above");
            has_quotes = *quotes;
            let shown = shown.clone();
            let blocked = self.options.block_remote;
            let clean = self.clean_links(cx);
            self.html.update(cx, |view, cx| {
                view.set_content(&shown, &m.inline, blocked, dark, clean, cx)
            });
            body = body.child(self.html.clone());
        } else {
            let text = m.body.clone().unwrap_or_default();
            let (written, quoted) = super::thread::split_quoted(&text);
            has_quotes = quoted.is_some();
            let shown = if self.quotes_open {
                text.as_str()
            } else {
                written
            };
            let masked = mask.as_ref().map(|k| k.apply(shown));
            let shown = masked.as_deref().unwrap_or(shown);
            let shown = crate::manage::local_ai::otp::mask(shown, &codes);
            let shown = shown.as_ref();
            body = body.child(
                gpui_component::text::TextView::markdown(
                    SharedString::from(format!("window-body-{}-{}", m.id, self.quotes_open)),
                    if self.clean_links(cx) {
                        plain_markdown(&crate::manage::tracking::clean_text(shown))
                    } else {
                        plain_markdown(shown)
                    })
                .selectable(true)
                .h_auto()
                .text_size(px(14.))
                .line_height(relative(1.8)),
            );
        }
        if has_quotes {
            let open = self.quotes_open;
            body = body.child(
                div().flex().mt_3().child(
                    div()
                        .id("window-quotes")
                        .h(px(24.))
                        .px_2()
                        .flex()
                        .items_center()
                        .gap_2()
                        .rounded(px(7.))
                        .bg(rgb(c.raised))
                        .border_1()
                        .border_color(rgb(c.border))
                        .text_size(px(11.))
                        .text_color(rgb(c.muted))
                        .cursor_pointer()
                        .child("•••")
                        .child(if open {
                            t!("message-window-hide-quoted-text")
                        } else {
                            t!("message-window-show-quoted-text")
                        })
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.quotes_open = !this.quotes_open;
                            this.html.update(cx, |v, cx| v.clear(cx));
                            cx.notify();
                        })),
                ),
            );
        }
        let attachments = m.attachments().iter().map(|a| {
            div()
                .mt_2()
                .px_3()
                .py_2()
                .flex()
                .items_center()
                .gap_2()
                .rounded_lg()
                .border_1()
                .border_color(rgb(c.border))
                .text_size(px(12.))
                .child(icon_el("paperclip", 14., c.muted))
                .child(format!("{}  ·  {}", a.name, human_size(a.size as u64)))
        });
        let toolbar = super::window::detached_bar("message-titlebar", c)
            .child(icon_el("mail", 15., c.muted))
            .child(
                div()
                    .flex_1()
                    .min_w(px(60.))
                    .truncate()
                    .text_size(px(13.))
                    .font_weight(FontWeight::SEMIBOLD)
                    .child(subject.clone()),
            )
            .child(self.button("w-reply", t!("message-window-reply"), "reply", cx, |t, _, cx| t.reply(1, cx)))
            .child(
                self.button("w-reply-all", t!("message-window-reply-all"), "reply-all", cx, |t, _, cx| {
                    t.reply(2, cx)
                }),
            )
            .child(
                self.button("w-forward", t!("message-window-forward"), "forward", cx, |t, _, cx| {
                    t.reply(3, cx)
                }),
            )
            .when(m.html.is_some(), |d| {
                let blocked = self.options.block_remote;
                d.child(self.button(
                    "w-remote",
                    if blocked {
                        t!("message-window-load-images")
                    } else {
                        t!("message-window-block-images")
                    },
                    if blocked { "image-off" } else { "image" },
                    cx,
                    |t, _, cx| {
                        t.options.block_remote = !t.options.block_remote;
                        t.html.update(cx, |v, cx| v.clear(cx));
                        cx.notify();
                    },
                ))
            });
        let toolbar =
            super::window::detached_titlebar(toolbar, c, |_, w, _| w.remove_window(), window, cx);
        div()
            .id("message-window")
            .key_context("Detached")
            .track_focus(&self.focus)
            .map(|d| super::window::window_frame(d, c, window))
            .size_full()
            .flex()
            .flex_col()
            .bg(rgb(c.surface))
            .text_color(rgb(c.text))
            .font_family(".SystemUIFont")
            .text_size(px(14.))
            .on_action(cx.listener(|_, _: &CloseWindow, w, _| w.remove_window()))
            .on_action(cx.listener(|_, _: &Close, w, _| w.remove_window()))
            .child(toolbar)
            .child(panes::scroll_frame(
                div()
                    .id("message-window-content")
                    .track_scroll(&self.scroll)
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .px_8()
                    .pt_6()
                    .pb_10()
                    .child(
                        div()
                            .mb_5()
                            .text_size(px(22.))
                            .font_weight(FontWeight::SEMIBOLD)
                            .child(subject.clone()),
                    )
                    .child(
                        div()
                            .mb_6()
                            .flex()
                            .items_center()
                            .gap_3()
                            .child(avatar(&m.initials(), 40., 0, c))
                            .child(
                                div()
                                    .flex_1()
                                    .min_w_0()
                                    .flex()
                                    .flex_col()
                                    .gap_1()
                                    .child(
                                        div()
                                            .flex()
                                            .gap_2()
                                            .items_center()
                                            .child(
                                                div()
                                                    .font_weight(FontWeight::SEMIBOLD)
                                                    .text_size(px(13.))
                                                    .child(sender),
                                            )
                                            .child(
                                                div()
                                                    .text_size(px(11.))
                                                    .text_color(rgb(c.muted))
                                                    .child(format!("<{from}>")),
                                            ),
                                    )
                                    .child(
                                        div().text_size(px(11.)).text_color(rgb(c.muted)).child(
                                            if cc.is_empty() {
                                                t!("message-window-to", to = to.join(", "))
                                            } else {
                                                t!("message-window-to-cc", to = to.join(", "), cc = cc.join(", "))
                                            },
                                        ),
                                    ),
                            )
                            .child(
                                div()
                                    .text_size(px(11.))
                                    .text_color(rgb(c.muted))
                                    .child(date_full(m.date)),
                            ),
                    )
                    .child(body)
                    .children(attachments),
                &self.scroll,
            )
            .children(
                self.hovered
                    .as_deref()
                    .map(|url| super::links::link_status_pill(url, c)),
            ))
            .into_any_element()
    }
}

impl MailApp {
    /// Opens a message in a window of its own, reading its body first if
    /// the list has only its headers.
    pub(super) fn open_message_window(
        &mut self,
        id: String,
        w: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(mail) = self
            .messages
            .iter()
            .find(|m| m.id == id && !m.pending)
            .cloned()
        else {
            return;
        };
        self.learn_contacts(&mail, cx);
        let mail = if mail.body.is_none() {
            self.backend
                .store
                .message(&id)
                .ok()
                .flatten()
                .filter(|m| m.body.is_some())
                .map(|full| {
                    self.backend
                        .with_readable(vec![Mail {
                            unread: mail.unread,
                            starred: mail.starred,
                            folder: mail.folder,
                            ..full
                        }])
                        .remove(0)
                })
                .unwrap_or(mail)
        } else {
            mail
        };
        self.show_mail_window(mail, w, cx);
    }

    /// Opens an email file (.eml or Outlook .msg) in a message window of its
    /// own, as when one is opened from the system's file manager.
    pub(super) fn open_eml_file(&mut self, path: &std::path::Path, w: &mut Window, cx: &mut Context<Self>) {
        // Read and parsed off the UI thread: an email file can be large, and
        // an Outlook .msg is converted first.
        let path = path.to_path_buf();
        cx.spawn_in(w, async move |this, cx| {
            let file = path.clone();
            let parsed = cx
                .background_executor()
                .spawn(async move {
                    std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| read_email_file(&file)))
                        .unwrap_or_else(|_| Err(anyhow::anyhow!(t!("ui-something-went-wrong-while-doing-this"))))
                })
                .await;
            let _ = this.update_in(cx, |this, w, cx| match parsed {
                Ok(mut mail) => {
                    mail.unread = false;
                    this.show_mail_window(mail, w, cx);
                }
                Err(e) => this.notice(
                    t!("message-window-couldnt-open", path = path.display(), e = format!("{:#}", e)),
                    true,
                    cx,
                ),
            });
        })
        .detach();
    }

    fn show_mail_window(&mut self, mail: Mail, w: &mut Window, cx: &mut Context<Self>) {
        let id = mail.id.clone();
        let options = self.reading_options(&mail);
        let (app, main, colors, dark) = (
            cx.weak_entity(),
            w.window_handle(),
            self.colors(),
            self.dark,
        );
        let own: Vec<String> = self.accounts.iter().map(|a| a.email.clone()).collect();
        // Read also for the images the HTML shows from the message itself.
        let needs_body = mail.body.is_none() && mail.html.is_none() || mail.lacks_inline_parts();
        let account = self.account_of(&mail);
        let unread = mail.unread;
        let made: Rc<std::cell::RefCell<Option<Entity<MessageWindow>>>> = Rc::default();
        let out = made.clone();
        let bounds = Bounds::centered(None, size(px(880.), px(780.)), cx);
        let opened = cx.open_window(
            window::detached_options(
                SharedString::from(self.hide_codes(&mail, &mail.subject).into_owned()),
                bounds,
            ),
            {
                let mail = mail.clone();
                move |window, cx| {
                    let view = cx.new(|cx| {
                        MessageWindow::new(mail, app, main, colors, options, dark, own, cx)
                    });
                    { let focus = view.read(cx).focus.clone(); focus.focus(window, cx); }
                    *out.borrow_mut() = Some(view.clone());
                    cx.new(|cx| Root::new(view, window, cx))
                }
            },
        );
        if let Err(e) = opened {
            self.notice(format!("{e:#}"), true, cx);
            return;
        }
        if unread {
            self.apply(vec![id], Change::Read(true), cx);
        }
        let (Some(view), Some(a)) = (made.borrow_mut().take(), account) else {
            return;
        };
        if needs_body {
            let view = view.downgrade();
            self.task(
                cx,
                move |b| b.read(&a, &mail),
                move |_, result, cx| {
                    let _ = view.update(cx, |v, cx| {
                        if let Ok(full) = result {
                            v.mail = full;
                        }
                        v.loading = false;
                        cx.notify();
                    });
                },
            );
        }
    }
}

/// The largest email file opened, and the largest a converted Outlook
/// .msg may become: the largest message the cache reads back.
const MAX_EMAIL_FILE: u64 = crate::manage::store::MAX_SOURCE;
/// The largest Outlook .msg file opened: converting one reads its streams,
/// perhaps several times over, before the message is written.
const MAX_MSG_FILE: u64 = 64 * 1024 * 1024;

/// An email file (.eml, or Outlook's .msg converted) read as a message.
/// Its size is checked before it is read, on the file opened (not its
/// name, which could be another file by then), and it is read no further
/// than the limit, however it grows meanwhile.
fn read_email_file(path: &std::path::Path) -> anyhow::Result<Mail> {
    use std::io::Read as _;
    let named_msg = path.extension().is_some_and(|e| e.eq_ignore_ascii_case("msg"));
    let file = std::fs::File::open(path)?;
    let limit = if named_msg { MAX_MSG_FILE } else { MAX_EMAIL_FILE };
    let size = file.metadata()?.len();
    anyhow::ensure!(size <= limit, t!("message-window-the-file-is-too-large"));
    let mut raw = Vec::with_capacity(size as usize);
    file.take(limit + 1).read_to_end(&mut raw)?;
    anyhow::ensure!(raw.len() as u64 <= limit, t!("message-window-the-file-is-too-large"));
    let raw = if crate::sync::msg::is_msg(&raw) || named_msg {
        anyhow::ensure!(raw.len() as u64 <= MAX_MSG_FILE, t!("message-window-the-file-is-too-large"));
        crate::sync::msg::to_eml_within(&raw, MAX_EMAIL_FILE as usize)?
    } else {
        raw
    };
    let mut account = Account::new(Provider::Imap);
    account.id = String::new();
    let id = format!("file:{}", path.display());
    crate::sync::mime::parse_message(&raw, &account, &id, Folder::Inbox, "", 0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::prelude::v1::test;

    /// An email file larger than may be opened is refused by its size,
    /// before it is read (sparse files here, so nothing is written); one
    /// within it opens.
    #[test]
    fn email_files_are_refused_by_size_before_being_read() -> anyhow::Result<()> {
        let dir = std::env::temp_dir().join(format!("picacho-eml-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir)?;
        let huge = dir.join("huge.eml");
        std::fs::File::create(&huge)?.set_len(MAX_EMAIL_FILE + 1)?;
        let err = read_email_file(&huge).unwrap_err();
        assert_eq!(err.to_string(), t!("message-window-the-file-is-too-large"));
        let msg = dir.join("large.msg");
        std::fs::File::create(&msg)?.set_len(MAX_MSG_FILE + 1)?;
        assert_eq!(read_email_file(&msg).unwrap_err().to_string(), t!("message-window-the-file-is-too-large"));
        let small = dir.join("small.eml");
        std::fs::write(&small, b"From: ana@example.com\r\nSubject: Hello\r\n\r\nHi\r\n")?;
        assert_eq!(read_email_file(&small)?.subject, "Hello");
        std::fs::remove_dir_all(dir)?;
        Ok(())
    }
}
