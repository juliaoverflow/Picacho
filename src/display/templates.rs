//! Settings → Composing: a signature for each mailbox (or everyone's), and
//! templates, reusable messages inserted from the composer.
use super::*;
use crate::manage::composing::Template;

#[derive(Clone, Debug, PartialEq)]
pub(super) enum TemplateAction {
    /// Edits this mailbox's signature (its id), or everyone's (None).
    SignatureFor(Option<String>),
    /// Adds the template in the form.
    Add,
    Remove(usize),
}

pub(super) struct TemplatesUi {
    /// The mailbox whose signature the field shows; everyone's when None.
    pub(super) signature_for: Option<String>,
    pub(super) name: Entity<InputState>,
    pub(super) subject: Entity<InputState>,
    pub(super) body: Entity<TextareaState>,
}
impl TemplatesUi {
    pub(super) fn new(window: &mut Window, cx: &mut App) -> Self {
        Self {
            signature_for: None,
            name: input("Name, as Thanks for applying", false, window, cx),
            subject: input("Subject (optional)", false, window, cx),
            body: textarea("What the template writes, as Hi {first_name},", window, cx),
        }
    }
}

impl MailApp {
    /// Saves the signature field's text as the chosen mailbox's signature,
    /// or everyone's.
    pub(super) fn save_signature(&mut self, signature: String, cx: &mut Context<Self>) {
        let mut options = self.compose_options.clone();
        match &self.templates.signature_for {
            Some(id) if signature.trim().is_empty() => {
                options.account_signatures.remove(id);
            }
            Some(id) => {
                options.account_signatures.insert(id.clone(), signature);
            }
            None => options.signature = signature,
        }
        self.save_compose_options(options, cx);
    }

    pub(super) fn save_template(&mut self, t: Template, cx: &mut Context<Self>) {
        let mut options = self.compose_options.clone();
        let name = t.name.clone();
        match options.templates.iter_mut().find(|x| x.name == t.name) {
            Some(x) => *x = t,
            None => options.templates.push(t),
        }
        self.save_compose_options(options, cx);
        self.notice(format!("Saved the template “{name}”"), false, cx);
    }

    pub(super) fn template_action(
        &mut self,
        action: TemplateAction,
        w: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match action {
            TemplateAction::SignatureFor(id) => {
                let text = match &id {
                    Some(id) => self
                        .compose_options
                        .account_signatures
                        .get(id)
                        .cloned()
                        .unwrap_or_default(),
                    None => self.compose_options.signature.clone(),
                };
                // Chosen first, so the field's change saves where it should.
                self.templates.signature_for = id;
                self.signature_input
                    .update(cx, |s, cx| s.set_value(text, w, cx));
            }
            TemplateAction::Add => {
                let name = Self::value(&self.templates.name, cx).trim().to_owned();
                let subject = Self::value(&self.templates.subject, cx).trim().to_owned();
                let body = self.templates.body.read(cx).value().to_string();
                if name.is_empty() || body.trim().is_empty() {
                    self.notice("A template needs a name and some text", true, cx);
                    return;
                }
                self.save_template(Template { name, subject, body }, cx);
                Self::set(&self.templates.name, "", w, cx);
                Self::set(&self.templates.subject, "", w, cx);
                self.templates.body.update(cx, |s, cx| s.set_value("", w, cx));
            }
            TemplateAction::Remove(i) => {
                let mut options = self.compose_options.clone();
                if i < options.templates.len() {
                    options.templates.remove(i);
                    self.save_compose_options(options, cx);
                }
            }
        }
        cx.notify();
    }

    /// Which mailbox's signature the field edits.
    pub(super) fn signature_chooser(&self, cx: &Context<Self>) -> Option<Div> {
        if self.accounts.is_empty() {
            return None;
        }
        let act = |id: Option<String>| Action::Templates(TemplateAction::SignatureFor(id));
        let mut options = vec![(
            "Every mailbox".to_owned(),
            act(None),
            self.templates.signature_for.is_none(),
        )];
        for a in &self.accounts {
            let has_own = self.compose_options.account_signatures.contains_key(&a.id);
            options.push((
                format!("{}{}", a.label(), if has_own { " ✓" } else { "" }),
                act(Some(a.id.clone())),
                self.templates.signature_for.as_deref() == Some(a.id.as_str()),
            ));
        }
        Some(self.segmented("signature-for", options, cx))
    }

    /// The templates section of Settings → Composing.
    pub(super) fn templates_settings(&self, cx: &Context<Self>) -> Div {
        let c = self.colors();
        let mut section = div().flex().flex_col().gap_3().child(self.section("Templates")).child(
            div()
                .text_size(px(12.))
                .text_color(rgb(c.muted))
                .child("Messages you send often. Insert one from the composer's Templates button, which also saves the message you're writing as a template."),
        ).child(
            div()
                .text_size(px(12.))
                .text_color(rgb(c.muted))
                .child(format!(
                    "Placeholders fill in as a template is inserted: {}.",
                    crate::manage::composing::PLACEHOLDERS
                        .iter()
                        .map(|(p, what)| format!("{p} {what}"))
                        .collect::<Vec<_>>()
                        .join(", ")
                )),
        );
        for (i, t) in self.compose_options.templates.iter().enumerate() {
            section = section.child(
                div()
                    .id(SharedString::from(format!("template-row-{i}")))
                    .flex()
                    .items_center()
                    .gap_3()
                    .py(px(6.))
                    .border_b_1()
                    .border_color(rgb(c.border))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .flex()
                            .flex_col()
                            .child(div().text_size(px(13.)).truncate().child(t.name.clone()))
                            .child(
                                div()
                                    .text_size(px(11.))
                                    .text_color(rgb(c.muted))
                                    .truncate()
                                    .child(t.body.lines().next().unwrap_or("").to_owned()),
                            ),
                    )
                    .child(self.button(
                        format!("remove-template-{i}"),
                        "@Remove template",
                        "trash",
                        Action::Templates(TemplateAction::Remove(i)),
                        false,
                        cx,
                    )),
            );
        }
        section
            .child(field(Input::new(&self.templates.name), c, false))
            .child(field(Input::new(&self.templates.subject), c, false))
            .child(
                div()
                    .h(px(110.))
                    .child(textarea_field(Textarea::new(&self.templates.body), c, false).h_full()),
            )
            .child(div().flex().justify_end().child(self.button(
                "add-template",
                "Add Template",
                "plus",
                Action::Templates(TemplateAction::Add),
                true,
                cx,
            )))
    }
}
