//! Settings → Encryption: your OpenPGP keys and the keys of people you
//! write to. Keys are imported from armored files (`.asc`) exported by
//! GnuPG, Thunderbird, Proton or another app, and kept in the keychain.
use super::*;
use crate::manage::openpgp::{KeyVersion, Keyring};
use anyhow::Context as _;

/// How long a new or renewed OpenPGP key lasts (RFC 9580 §5.2.3.13).
/// Keys don't expire unless you choose so.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(super) enum KeyExpiry {
    #[default]
    Never,
    Years(u32),
}

impl KeyExpiry {
    const CHOICES: [Self; 4] = [Self::Never, Self::Years(1), Self::Years(2), Self::Years(5)];
    fn label(self) -> String {
        match self {
            Self::Never => "Never".into(),
            Self::Years(1) => "1 year".into(),
            Self::Years(n) => format!("{n} years"),
        }
    }
    /// How long from now: `None` for never.
    pub(super) fn lasts(self) -> Option<std::time::Duration> {
        match self {
            Self::Never => None,
            // A year of 365 days and a quarter, as GnuPG counts "1y".
            Self::Years(n) => Some(std::time::Duration::from_secs(u64::from(n) * 31_557_600)),
        }
    }
}

/// "expires Mar 3, 2027" or "expired Mar 3, 2025", for a key that does.
fn expiry_text(expires: Option<i64>) -> Option<String> {
    let at = expires?;
    let date = chrono::DateTime::from_timestamp(at, 0)?.format("%b %-d, %Y").to_string();
    Some(if at <= chrono::Utc::now().timestamp() {
        format!("expired {date}")
    } else {
        format!("expires {date}")
    })
}

impl MailApp {
    pub(super) fn encryption_page(&self, cx: &Context<Self>) -> AnyElement {
        let c = self.colors();
        let ring = self.backend.keyring();
        let keys = ring.list();
        let row = |key: &crate::manage::openpgp::KeyInfo| {
            let fp = key.fingerprint.clone();
            let short: String = fp
                .chars()
                .collect::<Vec<_>>()
                .chunks(4)
                .map(|c| c.iter().collect::<String>())
                .collect::<Vec<_>>()
                .join(" ");
            div()
                .id(SharedString::from(format!("openpgp-key-{fp}")))
                .role(Role::ListItem)
                .aria_label(format!(
                    "{}, {}",
                    key.users.join(", "),
                    if key.secret { "your key" } else { "public key" }
                ))
                .flex()
                .items_center()
                .gap_3()
                .px_4()
                .py_3()
                .border_b_1()
                .border_color(rgb(c.border))
                .child(icon_el(
                    if key.secret { "lock" } else { "contact" },
                    16.,
                    c.muted,
                ))
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .flex()
                        .flex_col()
                        .gap(px(2.))
                        .child(
                            div()
                                .text_size(px(13.))
                                .font_weight(FontWeight::MEDIUM)
                                .truncate()
                                .child(if key.users.is_empty() {
                                    "(no name)".to_owned()
                                } else {
                                    key.users.join(", ")
                                }),
                        )
                        .child(
                            div()
                                .text_size(px(11.))
                                .text_color(rgb(c.muted))
                                .truncate()
                                .child(format!(
                                    "{}{} · {short}",
                                    expiry_text(key.expires).map(|e| format!("{e} · ")).unwrap_or_default(),
                                    if key.secret {
                                        "Your key"
                                    } else if key.found && key.verified {
                                        "Found automatically, checked by you"
                                    } else if key.found {
                                        "Found automatically, not checked: compare the fingerprint with its owner"
                                    } else {
                                        "Public key"
                                    }
                                )),
                        ),
                )
                .when(key.secret, |row| {
                    // Renewing for "Never" takes the end date away.
                    let label = if self.key_expiry == KeyExpiry::Never {
                        "Remove Expiry"
                    } else {
                        "Renew…"
                    };
                    row.when(key.expires.is_some() || self.key_expiry != KeyExpiry::Never, |row| {
                        row.child(self.button(
                            format!("openpgp-renew-{fp}"),
                            label,
                            "",
                            Action::RenewKey(fp.clone()),
                            false,
                            cx,
                        ))
                    })
                })
                .child(self.button(
                    format!("openpgp-check-{fp}"),
                    if key.secret {
                        "Show"
                    } else if key.verified {
                        "Fingerprint"
                    } else {
                        "Check…"
                    },
                    "",
                    Action::CheckKey(fp.clone()),
                    false,
                    cx,
                ))
                .child(self.button(
                    format!("openpgp-remove-{fp}"),
                    "Remove",
                    "trash",
                    Action::RemoveKey(fp),
                    false,
                    cx,
                ))
        };
        let checking = self
            .key_checking
            .as_ref()
            .and_then(|fp| keys.iter().find(|k| &k.fingerprint == fp));
        let list = if keys.is_empty() {
            div()
                .px_4()
                .py_3()
                .text_size(px(12.))
                .text_color(rgb(c.muted))
                .child("No keys yet.")
                .into_any_element()
        } else {
            div()
                .id("openpgp-keys")
                .flex()
                .flex_col()
                .role(Role::List)
                .aria_label("OpenPGP keys")
                .children(keys.iter().map(row))
                .into_any_element()
        };
        div()
            .flex()
            .flex_col()
            .gap_5()
            .child(self.section("OpenPGP"))
            .child(
                div()
                    .text_size(px(12.))
                    .text_color(rgb(c.muted))
                    .child("With your key, Picacho opens mail encrypted to you and signs what you send; with others' public keys, it checks their signatures and encrypts to them. Choose Sign or Encrypt in the composer's menu. Pinal Mail's own encryption needs nothing here."),
            )
            .child(
                div()
                    .rounded(px(12.))
                    .border_1()
                    .border_color(rgb(c.border))
                    .bg(rgb(c.raised))
                    .overflow_hidden()
                    .child(list),
            )
            .child(self.setting(
                "Import keys",
                "Choose armored key files (.asc). A secret key with a passphrase needs it once: it's then kept, without the passphrase, in the system keychain.",
                div()
                    .flex()
                    .flex_col()
                    .gap_2()
                    .max_w(px(420.))
                    .child(
                        div()
                            .id("openpgp-passphrase")
                            .child(field(Input::new(&self.key_passphrase), c, false)),
                    )
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap_2()
                            .child(
                                div()
                                    .text_size(px(12.))
                                    .text_color(rgb(c.muted))
                                    .flex_shrink_0()
                                    .child("New and renewed keys expire:"),
                            )
                            .child(self.segmented(
                                "openpgp-expiry",
                                KeyExpiry::CHOICES
                                    .into_iter()
                                    .map(|e| (e.label(), Action::SetKeyExpiry(e), self.key_expiry == e))
                                    .collect(),
                                cx,
                            )),
                    )
                    .child(div().flex().gap_2().child(self.button(
                        "openpgp-import",
                        "Import Keys…",
                        "upload",
                        Action::ImportKeys,
                        true,
                        cx,
                    )).child(self.button(
                        "openpgp-create",
                        "Create a Key",
                        "lock",
                        Action::CreateKey,
                        false,
                        cx,
                    ))),
            ))
            .when(ring.secret.iter().len() > 0, |page| {
                page.child(self.setting(
                    "Share your public key",
                    "Save it to send to people who'll write to you encrypted, or check your signatures.",
                    div().flex().child(self.button(
                        "openpgp-export",
                        "Save Public Keys…",
                        "download",
                        Action::ExportPublicKeys,
                        false,
                        cx,
                    )),
                ))
            })
            // Autocrypt 1.1 §2.2: prefer-encrypt per account, off by default.
            .children(self.accounts.iter().filter(|a| ring.own_key(&a.email).is_some()).map(|a| {
                self.switch_setting(
                    &format!("Ask for encrypted replies ({})", a.email),
                    "Tells people's mail apps (in the Autocrypt header of mail you send) that you'd like them to encrypt to you by default. Their app decides; apps that don't use Autocrypt ignore it.",
                    format!("autocrypt-mutual-{}", a.id),
                    ring.prefers_mutual(&a.email),
                    Action::ToggleAutocryptMutual(a.email.clone()),
                    cx,
                )
            }))
            .children(checking.map(|k| self.key_check_panel(k, cx)))
            .child(self.setting(
                "Find keys",
                "Look up the keys of people you've written to or heard from, on their domain (WKD) and keys.openpgp.org. Picacho also looks when you send encrypted mail to someone without a key, and keeps the keys people's mail apps send along (Autocrypt). Keys you imported yourself are never replaced.",
                div().flex().gap_2().child(
                    div().w(px(260.)).child(field(Input::new(&self.key_lookup), c, self.key_busy)),
                ).child(
                    self.button(
                        "openpgp-find",
                        if self.key_busy { "Looking…" } else { "Find Key" },
                        "search",
                        Action::FindKey,
                        false,
                        cx,
                    )
                    .when(self.key_busy, |d| d.opacity(0.6)),
                ),
            ))
            .child(self.section("S/MIME"))
            .child(
                div()
                    .text_size(px(12.))
                    .text_color(rgb(c.muted))
                    .child("Certificates many companies and authorities issue. Import yours as a .p12 or .pfx file (with its password); others' certificates are kept from the signed mail they send you, or imported as .pem, .crt or .cer. In the composer, choose S/MIME in place of OpenPGP."),
            )
            .child(self.certificate_list(cx))
            .child(self.setting(
                "Import certificates",
                "Your own (.p12, .pfx) with the password above, or someone else's (.pem, .crt, .cer).",
                div().flex().child(self.button(
                    "smime-import",
                    "Import Certificates…",
                    "upload",
                    Action::ImportCertificates,
                    false,
                    cx,
                )),
            ))
            .when_some(self.key_error.clone(), |page, e| {
                page.child(
                    div()
                        .id("openpgp-error")
                        .role(Role::Alert)
                        .text_size(px(12.))
                        .text_color(rgb(c.danger))
                        .child(e),
                )
            })
            .into_any_element()
    }

    /// Above an Autocrypt Setup Message (your key, sent by another of your
    /// mail apps): the setup code to type, and a button to take the key in.
    pub(super) fn setup_message_card(&self, m: &Mail, cx: &Context<Self>) -> Option<AnyElement> {
        if !m.subject.trim().eq_ignore_ascii_case("Autocrypt Setup Message") || !m.has_attachments {
            return None;
        }
        let c = self.colors();
        Some(
            div()
                .id("autocrypt-setup")
                .role(Role::Region)
                .aria_label("Autocrypt setup message")
                .m_3()
                .p_3()
                .flex()
                .flex_col()
                .gap_2()
                .rounded(px(10.))
                .border_1()
                .border_color(rgb(c.border))
                .bg(rgb(c.raised))
                .child(div().font_weight(FontWeight::MEDIUM).child("Your encryption key, from another mail app"))
                .child(
                    div()
                        .text_size(px(12.))
                        .text_color(rgb(c.muted))
                        .child("Type the setup code that app showed you (36 digits) to use the same key here."),
                )
                .child(
                    div()
                        .flex()
                        .gap_2()
                        .child(div().w(px(320.)).child(field(Input::new(&self.key_passphrase), c, false)))
                        .child(self.button(
                            "autocrypt-setup-import",
                            "Import Key",
                            "lock",
                            Action::ImportSetupMessage(m.id.clone()),
                            true,
                            cx,
                        )),
                )
                .into_any_element(),
        )
    }

    pub(super) fn encryption_action(
        &mut self,
        action: Action,
        w: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match action {
            Action::ImportKeys => {
                let passphrase = self.key_passphrase.read(cx).value().to_string();
                self.key_passphrase
                    .update(cx, |input, cx| input.set_text("", w, cx));
                let picker = cx.prompt_for_paths(PathPromptOptions {
                    files: true,
                    directories: false,
                    multiple: true,
                    prompt: Some("Import".into()),
                });
                cx.spawn(async move |this, cx| {
                    let Ok(Ok(Some(paths))) = picker.await else {
                        return;
                    };
                    let _ = this.update(cx, |this, cx| {
                        this.key_error = None;
                        this.task(
                            cx,
                            move |b| {
                                let mut ring = Keyring::clone(&b.keyring());
                                let mut added = 0;
                                for path in &paths {
                                    let text =
                                        std::fs::read_to_string(path).with_context(|| {
                                            format!("{} isn't an armored key file.", path.display())
                                        })?;
                                    added += ring.import(&text, &passphrase)?;
                                }
                                b.set_keyring(ring)?;
                                Ok(added)
                            },
                            |this, result, cx| match result {
                                Ok(n) => this.notice(
                                    format!("Imported {n} {}", if n == 1 { "key" } else { "keys" }),
                                    false,
                                    cx,
                                ),
                                Err(e) => this.key_error = Some(format!("{e:#}")),
                            },
                        );
                    });
                })
                .detach();
            }
            Action::ImportSetupMessage(id) => {
                let code = self.key_passphrase.read(cx).value().to_string();
                self.key_passphrase.update(cx, |i, cx| i.set_text("", w, cx));
                let Some(m) = self.messages.iter().find(|m| m.id == id).cloned() else {
                    return;
                };
                let Some(a) = self.account_of(&m) else { return };
                self.key_error = None;
                self.task(
                    cx,
                    move |b| {
                        let raw = b.original(&a, &m)?;
                        let mut ring = Keyring::clone(&b.keyring());
                        let n = ring.import_setup_message(&raw, &code)?;
                        b.set_keyring(ring)?;
                        Ok(n)
                    },
                    |this, result, cx| match result {
                        Ok(_) => this.notice("Your key from the other app is ready to use", false, cx),
                        Err(e) => this.notice(format!("{e:#}"), true, cx),
                    },
                );
            }
            Action::CreateKey => {
                let Some(a) = self.current_account() else {
                    self.key_error = Some("Add an account first: the key is for its address.".into());
                    return;
                };
                self.key_error = None;
                let (name, email) = (a.name.clone(), a.email.clone());
                let lasts = self.key_expiry.lasts();
                self.task(
                    cx,
                    move |b| {
                        let mut ring = Keyring::clone(&b.keyring());
                        anyhow::ensure!(
                            ring.own_key(&email).is_none(),
                            "You already have a key for {email}."
                        );
                        let fp = ring.create(&name, &email, lasts, KeyVersion::default())?;
                        b.set_keyring(ring)?;
                        Ok(fp)
                    },
                    |this, result, cx| match result {
                        Ok(_) => this.notice("Your new key is ready. Save your public key to share it.", false, cx),
                        Err(e) => this.key_error = Some(format!("{e:#}")),
                    },
                );
            }
            Action::SetKeyExpiry(choice) => self.key_expiry = choice,
            Action::RenewKey(fp) => {
                self.key_error = None;
                let mut ring = Keyring::clone(&self.backend.keyring());
                match ring
                    .renew(&fp, self.key_expiry.lasts())
                    .and_then(|()| self.backend.set_keyring(ring))
                {
                    // Others have the key as it was until they get it again.
                    Ok(()) => self.notice(
                        "Renewed. Share your public key again so others see the new date.",
                        false,
                        cx,
                    ),
                    Err(e) => self.key_error = Some(format!("{e:#}")),
                }
            }
            Action::CheckKey(fp) => {
                self.key_checking = (self.key_checking.as_ref() != Some(&fp)).then_some(fp);
                self.key_check_input
                    .update(cx, |i, cx| i.set_text("", w, cx));
                self.key_error = None;
            }
            Action::MatchFingerprint(fp) => {
                let given = self.key_check_input.read(cx).value().to_string();
                let mut ring = Keyring::clone(&self.backend.keyring());
                match ring
                    .check(&fp, &given)
                    .and_then(|()| self.backend.set_keyring(ring))
                {
                    Ok(()) => {
                        self.key_checking = None;
                        self.notice("The fingerprints match: the key is checked", false, cx);
                    }
                    Err(e) => self.key_error = Some(format!("{e:#}")),
                }
            }
            Action::VerifyKey(fp) => {
                self.key_checking = None;
                let mut ring = Keyring::clone(&self.backend.keyring());
                ring.verify(&fp);
                match self.backend.set_keyring(ring) {
                    Ok(()) => self.notice("Key marked as checked", false, cx),
                    Err(e) => self.key_error = Some(format!("{e:#}")),
                }
            }
            Action::ToggleAutocryptMutual(email) => {
                let mut ring = Keyring::clone(&self.backend.keyring());
                let on = !ring.prefers_mutual(&email);
                ring.set_prefers_mutual(&email, on);
                if let Err(e) = self.backend.set_keyring(ring) {
                    self.key_error = Some(format!("{e:#}"));
                }
            }
            Action::RemoveKey(fp) => {
                let mut ring = Keyring::clone(&self.backend.keyring());
                ring.remove(&fp);
                match self.backend.set_keyring(ring) {
                    Ok(()) => self.notice("Key removed", false, cx),
                    Err(e) => self.key_error = Some(format!("{e:#}")),
                }
            }
            Action::ExportPublicKeys => {
                let ring = self.backend.keyring();
                let picker =
                    cx.prompt_for_new_path(&self.save_start(), Some("public-key.asc"));
                cx.spawn(async move |this, cx| {
                    let Ok(Ok(Some(path))) = picker.await else {
                        return;
                    };
                    let mut text = String::new();
                    for key in ring.secret_keys() {
                        let public = pgp::composed::SignedPublicKey::from(key);
                        if let Ok(armored) = public.to_armored_string(Default::default()) {
                            text.push_str(&armored);
                            text.push('\n');
                        }
                    }
                    let result = std::fs::write(&path, text);
                    let _ = this.update(cx, |this, cx| match result {
                        Ok(()) => this.notice("Public key saved", false, cx),
                        Err(e) => this.notice(format!("{e:#}"), true, cx),
                    });
                })
                .detach();
            }
            Action::FindKey => {
                let email = self.key_lookup.read(cx).value().trim().to_owned();
                // A fingerprint (40 or 64 hex digits, spaces allowed): by
                // HKP, as the key is imported by hand.
                let hex: String = email.chars().filter(|c| !c.is_whitespace()).collect();
                let hex = hex.trim_start_matches("0x").trim_start_matches("openpgp4fpr:").to_owned();
                if (hex.len() == 40 || hex.len() == 64) && hex.chars().all(|c| c.is_ascii_hexdigit()) {
                    self.key_busy = true;
                    self.key_error = None;
                    self.task(
                        cx,
                        move |b| {
                            let tls = b.tls_connector_pub()?;
                            let data = crate::sync::keys::lookup_fingerprint(&hex, tls)?
                                .context("No key with that fingerprint was found.")?;
                            let mut ring = Keyring::clone(&b.keyring());
                            let text = String::from_utf8(data).context("The key found couldn't be read.")?;
                            ring.import(&text, "")?;
                            b.set_keyring(ring)?;
                            Ok("Found the key on keys.openpgp.org".to_owned())
                        },
                        |this, result, cx| {
                            this.key_busy = false;
                            match result {
                                Ok(done) => this.notice(done, false, cx),
                                Err(e) => this.key_error = Some(format!("{e:#}")),
                            }
                        },
                    );
                    return;
                }
                if !email.contains('@') {
                    self.key_error = Some("Enter an email address or a key's fingerprint to look it up.".into());
                    cx.notify();
                    return;
                }
                self.key_busy = true;
                self.key_error = None;
                self.task(
                    cx,
                    move |b| {
                        let tls = b.tls_connector_pub()?;
                        let found = crate::sync::keys::lookup(&email, tls)?
                            .with_context(|| format!("No key was found for {email}."))?;
                        let mut ring = Keyring::clone(&b.keyring());
                        anyhow::ensure!(
                            ring.add_found_from(&email, &found.0, found.1)?,
                            "You already have a key for {email} that you imported yourself."
                        );
                        b.set_keyring(ring)?;
                        Ok(format!("Found the key for {email} on {}", found.1.label()))
                    },
                    |this, result, cx| {
                        this.key_busy = false;
                        match result {
                            Ok(done) => this.notice(done, false, cx),
                            Err(e) => this.key_error = Some(format!("{e:#}")),
                        }
                    },
                );
            }
            Action::ImportCertificates => {
                let password = self.key_passphrase.read(cx).value().to_string();
                self.key_passphrase
                    .update(cx, |input, cx| input.set_text("", w, cx));
                let picker = cx.prompt_for_paths(PathPromptOptions {
                    files: true,
                    directories: false,
                    multiple: true,
                    prompt: Some("Import".into()),
                });
                cx.spawn(async move |this, cx| {
                    let Ok(Ok(Some(paths))) = picker.await else {
                        return;
                    };
                    let _ = this.update(cx, |this, cx| {
                        this.key_error = None;
                        this.task(
                            cx,
                            move |b| {
                                let mut certs =
                                    crate::manage::smime::Certificates::clone(&b.certificates());
                                let mut added = 0;
                                for path in &paths {
                                    let data = std::fs::read(path)?;
                                    let name = path.to_string_lossy().to_ascii_lowercase();
                                    if name.ends_with(".p12") || name.ends_with(".pfx") {
                                        certs.import_identity(&data, &password)?;
                                    } else {
                                        certs.import_certificate(&data)?;
                                    }
                                    added += 1;
                                }
                                b.set_certificates(certs)?;
                                Ok(added)
                            },
                            |this, result, cx| match result {
                                Ok(n) => this.notice(
                                    format!(
                                        "Imported {n} {}",
                                        if n == 1 {
                                            "certificate"
                                        } else {
                                            "certificates"
                                        }
                                    ),
                                    false,
                                    cx,
                                ),
                                Err(e) => this.key_error = Some(format!("{e:#}")),
                            },
                        );
                    });
                })
                .detach();
            }
            Action::RemoveCertificate(fp) => {
                let mut certs =
                    crate::manage::smime::Certificates::clone(&self.backend.certificates());
                certs.remove(&fp);
                match self.backend.set_certificates(certs) {
                    Ok(()) => self.notice("Certificate removed", false, cx),
                    Err(e) => self.key_error = Some(format!("{e:#}")),
                }
            }
            _ => {}
        }
        cx.notify();
    }

    /// A key's fingerprint, to compare with its owner: in groups to read
    /// out, as a QR code their phone can scan (and yours can show them for
    /// your own key), and a field for the fingerprint they gave you.
    fn key_check_panel(
        &self,
        key: &crate::manage::openpgp::KeyInfo,
        cx: &Context<Self>,
    ) -> AnyElement {
        let c = self.colors();
        let fp = key.fingerprint.clone();
        let groups: Vec<String> = fp
            .chars()
            .collect::<Vec<_>>()
            .chunks(4)
            .map(|g| g.iter().collect())
            .collect();
        let who = key.users.first().cloned().unwrap_or_default();
        let spoken = groups
            .iter()
            .map(|g| g.chars().map(String::from).collect::<Vec<_>>().join(" "))
            .collect::<Vec<_>>()
            .join(", ");
        div()
            .id("openpgp-check-panel")
            .debug_selector(|| "openpgp-check-panel".into())
            .role(Role::Region)
            .aria_label(format!("Fingerprint of {who}"))
            .p_4()
            .rounded(px(12.))
            .border_1()
            .border_color(rgb(c.border))
            .bg(rgb(c.raised))
            .flex()
            .flex_col()
            .gap_3()
            .child(div().text_size(px(14.)).font_weight(FontWeight::SEMIBOLD).child(if key.secret {
                "Your fingerprint".to_owned()
            } else {
                format!("Check {who}")
            }))
            .child(
                div()
                    .text_size(px(12.))
                    .text_color(rgb(c.muted))
                    .child(if key.secret {
                        "Read it out to people who write to you encrypted, or let them scan the code, so they know the key they have is yours."
                    } else {
                        "Ask them for their fingerprint (in person, on a call, or anywhere other than this mail) and compare. If they paste it to you, paste it below."
                    }),
            )
            .child(
                div()
                    .flex()
                    .flex_wrap()
                    .gap_4()
                    .items_center()
                    .child(
                        div()
                            .id("openpgp-fingerprint")
                            .role(Role::Label)
                            .aria_label(spoken)
                            .grid()
                            .grid_cols(5)
                            .gap_x_3()
                            .gap_y_1()
                            .font_family("monospace")
                            .text_size(px(15.))
                            .id("openpgp-fingerprint")
                            .aria_label(format!("Fingerprint {}", groups.join(" ")))
                            .children(groups.iter().map(|g| div().child(g.clone()))),
                    )
                    .child(
                        div()
                            .p_2()
                            .rounded(px(8.))
                            .bg(rgb(0xffffff))
                            .size(px(140.))
                            .id("openpgp-fingerprint-qr")
                            .role(Role::Image)
                            .aria_label("QR code of this key's fingerprint")
                            .child(super::proton_login::qr_code(&format!("OPENPGP4FPR:{fp}"), 124.)),
                    ),
            )
            .when(!key.secret && !key.verified, |d| {
                d.child(
                    div()
                        .flex()
                        .flex_wrap()
                        .gap_2()
                        .items_center()
                        .child(div().id("openpgp-check-input").w(px(360.)).child(field(Input::new(&self.key_check_input), c, false)))
                        .child(self.button("openpgp-match", "Compare", "check", Action::MatchFingerprint(fp.clone()), true, cx))
                        .child(self.button(
                            format!("openpgp-verify-{fp}"),
                            "They Match (compared in person)",
                            "",
                            Action::VerifyKey(fp.clone()),
                            false,
                            cx,
                        )),
                )
            })
            .into_any_element()
    }

    fn certificate_list(&self, cx: &Context<Self>) -> AnyElement {
        let c = self.colors();
        let certs = self.backend.certificates().list();
        let body = if certs.is_empty() {
            div()
                .px_4()
                .py_3()
                .text_size(px(12.))
                .text_color(rgb(c.muted))
                .child("No certificates yet.")
                .into_any_element()
        } else {
            div()
                .flex()
                .flex_col()
                .children(certs.into_iter().map(|cert| {
                    let fp = cert.fingerprint.clone();
                    let expires = chrono::DateTime::from_timestamp(cert.expires, 0)
                        .map(|t| t.format("%b %-d, %Y").to_string())
                        .unwrap_or_default();
                    let who = cert.emails.join(", ");
                    div()
                        .id(SharedString::from(format!("smime-cert-{fp}")))
                        .role(Role::ListItem)
                        .aria_label(format!(
                            "{who}, {}, expires {expires}",
                            if cert.own {
                                "your certificate"
                            } else {
                                "certificate"
                            }
                        ))
                        .flex()
                        .items_center()
                        .gap_3()
                        .px_4()
                        .py_3()
                        .border_b_1()
                        .border_color(rgb(c.border))
                        .child(icon_el(
                            if cert.own { "lock" } else { "contact" },
                            16.,
                            c.muted,
                        ))
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .flex()
                                .flex_col()
                                .gap(px(2.))
                                .child(
                                    div()
                                        .text_size(px(13.))
                                        .font_weight(FontWeight::MEDIUM)
                                        .truncate()
                                        .child(who),
                                )
                                .child(
                                    div()
                                        .text_size(px(11.))
                                        .text_color(rgb(c.muted))
                                        .truncate()
                                        .child(format!(
                                            "{} · issued by {} · expires {expires}",
                                            if cert.own {
                                                "Your certificate"
                                            } else {
                                                "Certificate"
                                            },
                                            if cert.issuer.is_empty() {
                                                "an unnamed issuer".to_owned()
                                            } else {
                                                cert.issuer.clone()
                                            },
                                        )),
                                ),
                        )
                        .child(self.button(
                            format!("smime-remove-{fp}"),
                            "Remove",
                            "trash",
                            Action::RemoveCertificate(fp),
                            false,
                            cx,
                        ))
                }))
                .into_any_element()
        };
        div()
            .id("smime-certificates")
            .role(Role::List)
            .aria_label("S/MIME certificates")
            .rounded(px(12.))
            .border_1()
            .border_color(rgb(c.border))
            .bg(rgb(c.raised))
            .overflow_hidden()
            .child(body)
            .into_any_element()
    }
}

#[cfg(test)]
mod tests {
    use super::super::tests::{act, frame, two_accounts};
    use super::*;
    use std::prelude::v1::test;

    /// Settings → Encryption: the expiry choice (Never by default) makes a
    /// key that expires, shown in its row; Renew… extends it for the
    /// chosen time, and with Never chosen Remove Expiry takes the date away.
    #[gpui::test]
    fn keys_are_made_and_renewed_with_the_chosen_expiry(cx: &mut gpui::TestAppContext) {
        let (view, cx, _) = two_accounts(cx);
        act(&view, Action::Settings, cx);
        act(&view, Action::SettingsPage(Page::Encryption), cx);
        frame(cx);
        assert!(view.read_with(cx, |this, _| this.key_expiry == KeyExpiry::Never));
        assert!(cx.debug_bounds("openpgp-expiry-0").is_some(), "the expiry choice is shown");
        assert!(cx.debug_bounds("openpgp-expiry-3").is_some());

        act(&view, Action::SetKeyExpiry(KeyExpiry::Years(2)), cx);
        act(&view, Action::CreateKey, cx);
        cx.run_until_parked();
        let key = view.read_with(cx, |this, _| this.backend.keyring().list());
        let [key] = key.as_slice() else { panic!("one key: {key:?}") };
        assert!(key.secret);
        let two_years = KeyExpiry::Years(2).lasts().unwrap().as_secs() as i64;
        let made = key.expires.unwrap();
        assert!((made - chrono::Utc::now().timestamp() - two_years).abs() <= 5);
        assert!(expiry_text(key.expires).unwrap().starts_with("expires "));
        let fp = key.fingerprint.clone();
        // Debug selectors are looked up by a 'static name.
        let renew: &'static str = format!("openpgp-renew-{fp}").leak();
        frame(cx);
        assert!(cx.debug_bounds(renew).is_some(), "Renew… on your key");

        // Renewed for five years from now.
        act(&view, Action::SetKeyExpiry(KeyExpiry::Years(5)), cx);
        act(&view, Action::RenewKey(fp.clone()), cx);
        let (renewed, status) = view.read_with(cx, |this, _| (this.backend.keyring().list()[0].expires, this.status.clone()));
        let five_years = KeyExpiry::Years(5).lasts().unwrap().as_secs() as i64;
        assert!((renewed.unwrap() - chrono::Utc::now().timestamp() - five_years).abs() <= 5);
        assert!(status.contains("Share your public key again"), "{status}");

        // Never chosen: Remove Expiry clears it.
        act(&view, Action::SetKeyExpiry(KeyExpiry::Never), cx);
        frame(cx);
        assert!(cx.debug_bounds(renew).is_some(), "Remove Expiry on a key that expires");
        act(&view, Action::RenewKey(fp.clone()), cx);
        assert_eq!(view.read_with(cx, |this, _| this.backend.keyring().list()[0].expires), None);
        frame(cx);
        assert!(
            cx.debug_bounds(renew).is_none(),
            "nothing to remove on a key that never expires"
        );
    }

    #[test]
    fn expiry_dates_say_whether_they_are_past() {
        assert_eq!(expiry_text(None), None);
        assert_eq!(expiry_text(Some(0)).unwrap(), "expired Jan 1, 1970");
        let later = chrono::Utc::now().timestamp() + 86_400 * 400;
        assert!(expiry_text(Some(later)).unwrap().starts_with("expires "));
        assert_eq!(KeyExpiry::Never.lasts(), None);
        assert_eq!(KeyExpiry::Years(1).label(), "1 year");
        assert_eq!(KeyExpiry::Years(5).label(), "5 years");
    }
}
