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
        Self {
            mail,
            app,
            main,
            html: cx.new(HtmlView::new),
            colors,
            options,
            app_dark,
            own,
            loading,
            quotes_open: false,
            scroll: ScrollHandle::new(),
            focus: cx.focus_handle(),
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
        let m = &self.mail;
        window.set_window_title(&m.subject);
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
        let to = super::thread::short_names(&m.to, &self.own);
        let cc = super::thread::short_names(&m.cc, &self.own);
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
            let hidden = super::thread::hide_html_quotes(html);
            has_quotes = hidden.is_some();
            let shown = match &hidden {
                Some(h) if !self.quotes_open => h.as_str(),
                _ => html.as_str(),
            };
            let masked = mask.as_ref().map(|k| k.apply(shown));
            let shown = masked.as_deref().unwrap_or(shown);
            let blocked = self.options.block_remote;
            let clean = self.clean_links(cx);
            self.html.update(cx, |view, cx| {
                view.set_content(shown, &m.inline, blocked, dark, clean, cx)
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
            body = body.child(
                gpui_component::text::TextView::markdown(
                    SharedString::from(format!("window-body-{}-{}", m.id, self.quotes_open)),
                    if self.clean_links(cx) {
                        plain_markdown(&crate::manage::tracking::clean_text(shown))
                    } else {
                        plain_markdown(shown)
                    },
                    window,
                    cx,
                )
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
                            "Hide quoted text"
                        } else {
                            "Show quoted text"
                        })
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.quotes_open = !this.quotes_open;
                            this.html.update(cx, |v, cx| v.clear(cx));
                            cx.notify();
                        })),
                ),
            );
        }
        let attachments = m.attachments.iter().map(|a| {
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
                .child(format!("{}  ·  {} KB", a.name, a.size / 1024))
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
                    .child(m.subject.clone()),
            )
            .child(self.button("w-reply", "Reply", "reply", cx, |t, _, cx| t.reply(1, cx)))
            .child(
                self.button("w-reply-all", "Reply all", "reply-all", cx, |t, _, cx| {
                    t.reply(2, cx)
                }),
            )
            .child(
                self.button("w-forward", "Forward", "forward", cx, |t, _, cx| {
                    t.reply(3, cx)
                }),
            )
            .when(m.html.is_some(), |d| {
                let blocked = self.options.block_remote;
                d.child(self.button(
                    "w-remote",
                    if blocked {
                        "Load images"
                    } else {
                        "Block images"
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
                            .child(m.subject.clone()),
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
                                            format!(
                                                "to {}{}",
                                                to.join(", "),
                                                if cc.is_empty() {
                                                    String::new()
                                                } else {
                                                    format!(" · cc {}", cc.join(", "))
                                                }
                                            ),
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
            ))
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
            window::detached_options(SharedString::from(mail.subject.clone()), bounds),
            {
                let mail = mail.clone();
                move |window, cx| {
                    let view = cx.new(|cx| {
                        MessageWindow::new(mail, app, main, colors, options, dark, own, cx)
                    });
                    view.read(cx).focus.focus(window);
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
