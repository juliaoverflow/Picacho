//! Settings → Composing → Large attachments: the cloud storage (Nextcloud,
//! or any WebDAV folder) that attachments too large to send by mail are
//! uploaded to and shared from as links (`manage::outgoing`).
use super::*;
use crate::manage::composing::{CloudKind, CloudStorage};
use crate::manage::outgoing::CLOUD_SECRET;

#[derive(Clone, Debug, PartialEq)]
pub(super) enum CloudAction {
    Kind(CloudKind),
    /// The mail account whose Drive or OneDrive it is.
    Account(String),
    Expire(u32),
    Save,
    Check,
    Remove,
}

pub(super) struct CloudUi {
    url: Entity<InputState>,
    user: Entity<InputState>,
    password: Entity<InputState>,
    folder: Entity<InputState>,
    public_url: Entity<InputState>,
    /// The kind chosen, before it is saved.
    kind: CloudKind,
    /// Google Drive and OneDrive: the mail account chosen.
    account: String,
    /// Waiting for the browser's sign-in.
    connecting: bool,
    expire_days: u32,
    /// What the last check found: whether it worked, and what it said.
    status: Option<(bool, String)>,
    checking: bool,
}

impl CloudUi {
    pub(super) fn new(saved: &CloudStorage, window: &mut Window, cx: &mut App) -> Self {
        let field = |placeholder: &str, value: &str, masked: bool, window: &mut Window, cx: &mut App| {
            let state = input(placeholder, masked, window, cx);
            let value = value.to_owned();
            state.update(cx, |s, cx| s.set_text(value, window, cx));
            state
        };
        Self {
            url: field(t!("cloud-settings-server-address"), &saved.url, false, window, cx),
            user: field(t!("cloud-settings-user-name"), &saved.user, false, window, cx),
            password: field(t!("cloud-settings-app-password"), "", true, window, cx),
            folder: field(t!("cloud-settings-folder"), &saved.folder, false, window, cx),
            public_url: field(t!("cloud-settings-public-address"), &saved.public_url, false, window, cx),
            kind: if saved.kind == CloudKind::None { CloudKind::Nextcloud } else { saved.kind },
            expire_days: saved.expire_days,
            account: saved.account.clone(),
            connecting: false,
            status: None,
            checking: false,
        }
    }
}

impl MailApp {
    /// The storage as the fields say.
    fn typed_cloud(&self, cx: &App) -> CloudStorage {
        let ui = &self.cloud;
        CloudStorage {
            kind: ui.kind,
            url: Self::value(&ui.url, cx).trim().to_owned(),
            user: Self::value(&ui.user, cx).trim().to_owned(),
            folder: Self::value(&ui.folder, cx).trim().trim_matches('/').to_owned(),
            public_url: if ui.kind == CloudKind::WebDav { Self::value(&ui.public_url, cx).trim().to_owned() } else { String::new() },
            expire_days: ui.expire_days,
            account: if ui.kind.uses_account() { ui.account.clone() } else { String::new() },
        }
    }

    /// The accounts whose own storage `kind` can be: Gmail accounts for
    /// Google Drive, Outlook accounts using Microsoft Graph for OneDrive.
    fn cloud_accounts(&self, kind: CloudKind) -> Vec<&Account> {
        self.accounts
            .iter()
            .filter(|a| match kind {
                CloudKind::GoogleDrive => a.provider == Provider::Gmail && a.oauth && a.sso.is_none(),
                CloudKind::OneDrive => a.provider == Provider::Outlook && a.graph() && a.oauth,
                _ => false,
            })
            .collect()
    }

    pub(super) fn cloud_action(&mut self, action: CloudAction, w: &mut Window, cx: &mut Context<Self>) {
        match action {
            CloudAction::Kind(kind) => {
                self.cloud.kind = kind;
                self.cloud.status = None;
                // The only account that can be, chosen already.
                if kind.uses_account() && !self.cloud_accounts(kind).iter().any(|a| a.id == self.cloud.account) {
                    self.cloud.account = self.cloud_accounts(kind).first().map(|a| a.id.clone()).unwrap_or_default();
                }
            }
            CloudAction::Account(id) => self.cloud.account = id,
            CloudAction::Expire(days) => self.cloud.expire_days = days,
            CloudAction::Save if self.cloud.kind.uses_account() => {
                let storage = self.typed_cloud(cx);
                let Some(a) = self.accounts.iter().find(|a| a.id == storage.account).cloned() else {
                    self.notice(t!("cloud-settings-choose-an-account"), true, cx);
                    return;
                };
                // Access to the storage, asked for in the browser where not
                // given yet; saved once given.
                self.cloud.connecting = true;
                self.cloud.status = None;
                let kind = storage.kind;
                self.task(
                    cx,
                    move |b| b.connect_cloud(&a, kind, Arc::new(std::sync::atomic::AtomicBool::new(false))),
                    move |this, result, cx| {
                        this.cloud.connecting = false;
                        match result {
                            Ok(()) => {
                                let mut options = this.compose_options.clone();
                                options.cloud = storage;
                                this.save_compose_options(options, cx);
                                this.notice(t!("cloud-settings-saved"), false, cx);
                            }
                            Err(e) => this.cloud.status = Some((false, format!("{e:#}"))),
                        }
                    },
                );
            }
            CloudAction::Save => {
                let storage = self.typed_cloud(cx);
                if storage.url.is_empty() || storage.user.is_empty() {
                    self.notice(t!("cloud-settings-needs-address-and-user"), true, cx);
                    return;
                }
                let password = zeroize::Zeroizing::new(Self::value(&self.cloud.password, cx));
                Self::set(&self.cloud.password, "", w, cx);
                let mut options = self.compose_options.clone();
                options.cloud = storage;
                self.save_compose_options(options, cx);
                if !password.is_empty() {
                    let password = password.clone();
                    self.task(
                        cx,
                        move |_| crate::manage::store::write_secret_text(CLOUD_SECRET, &password),
                        |this, result, cx| match result {
                            Ok(()) => this.notice(t!("cloud-settings-saved"), false, cx),
                            Err(e) => this.notice(format!("{e:#}"), true, cx),
                        },
                    );
                } else {
                    self.notice(t!("cloud-settings-saved"), false, cx);
                }
                self.cloud.status = None;
            }
            CloudAction::Check => {
                let storage = self.typed_cloud(cx);
                let typed = zeroize::Zeroizing::new(Self::value(&self.cloud.password, cx));
                self.cloud.checking = true;
                self.cloud.status = None;
                self.task(
                    cx,
                    move |b| {
                        let password = (!typed.is_empty()).then(|| typed.to_string());
                        b.cloud_sharer(&storage, password.as_deref())?.check()
                    },
                    |this, result, _| {
                        this.cloud.checking = false;
                        this.cloud.status = Some(match result {
                            Ok(()) => (true, t!("cloud-settings-connected").to_owned()),
                            Err(e) => (false, format!("{e:#}")),
                        });
                    },
                );
            }
            CloudAction::Remove => {
                let mut options = self.compose_options.clone();
                options.cloud = CloudStorage::default();
                self.save_compose_options(options, cx);
                for field in [&self.cloud.url, &self.cloud.user, &self.cloud.password, &self.cloud.folder, &self.cloud.public_url] {
                    Self::set(field, "", w, cx);
                }
                self.cloud.status = None;
                self.task(cx, |_| crate::manage::store::delete_secret(CLOUD_SECRET), |_, _, _| {});
            }
        }
        cx.notify();
    }

    /// The Large attachments group of Settings → Composing.
    pub(super) fn cloud_settings(&self, cx: &Context<Self>) -> Rows {
        let c = self.colors();
        let ui = &self.cloud;
        let saved = &self.compose_options.cloud;
        let act = |a: CloudAction| Action::Cloud(a);
        let kinds = self
            .segmented(
                "cloud-kind",
                [
                    ("Nextcloud", CloudKind::Nextcloud),
                    ("WebDAV", CloudKind::WebDav),
                    ("Google Drive", CloudKind::GoogleDrive),
                    ("OneDrive", CloudKind::OneDrive),
                ]
                .into_iter()
                .map(|(label, kind)| (label.to_owned(), act(CloudAction::Kind(kind)), ui.kind == kind))
                .collect(),
                cx,
            )
            .max_w(px(460.));
        let text_field = |id: &'static str, state: &Entity<InputState>| {
            div().id(id).debug_selector(move || id.into()).w(px(300.)).max_w_full().child(field(Input::new(state), c, false))
        };
        let state = if saved.configured() {
            let host = match saved.kind {
                CloudKind::GoogleDrive | CloudKind::OneDrive => {
                    let email = self.accounts.iter().find(|a| a.id == saved.account).map(|a| a.email.clone()).unwrap_or_default();
                    format!("{} ({email})", if saved.kind == CloudKind::GoogleDrive { "Google Drive" } else { "OneDrive" })
                }
                _ => saved.url.clone(),
            };
            t!("cloud-settings-in-use", host = host)
        } else {
            t!("cloud-settings-not-set-up").to_owned()
        };
        let mut rows = Rows::new()
            .row(div().text_size(px(12.)).text_color(rgb(c.muted)).child(t!("cloud-settings-explained")))
            .row(self.setting_row(t!("cloud-settings-kind"), "", kinds));
        if ui.kind.uses_account() {
            let accounts = self.cloud_accounts(ui.kind);
            rows = if accounts.is_empty() {
                rows.row(div().text_size(px(12.)).text_color(rgb(c.muted)).child(if ui.kind == CloudKind::GoogleDrive {
                    t!("cloud-settings-no-google-account")
                } else {
                    t!("cloud-settings-no-graph-account")
                }))
            } else {
                let choices = self
                    .segmented(
                        "cloud-account",
                        accounts.iter().map(|a| (a.email.clone(), act(CloudAction::Account(a.id.clone())), a.id == ui.account)).collect(),
                        cx,
                    )
                    .max_w(px(460.));
                rows.row(self.setting_row(t!("cloud-settings-account"), t!("cloud-settings-account-caption"), choices))
            };
            rows = rows.row(self.setting_row(t!("cloud-settings-folder"), "", text_field("cloud-folder", &ui.folder)));
        } else {
            rows = rows
            .row(self.setting_row(
                if ui.kind == CloudKind::Nextcloud { t!("cloud-settings-server-address") } else { t!("cloud-settings-folder-address") },
                "",
                text_field("cloud-url", &ui.url),
            ))
            .row(self.setting_row(t!("cloud-settings-user-name"), "", text_field("cloud-user", &ui.user)))
            .row(self.setting_row(
                t!("cloud-settings-app-password"),
                if saved.configured() { t!("cloud-settings-password-kept") } else { "" },
                text_field("cloud-password", &ui.password),
            ))
            .row(self.setting_row(t!("cloud-settings-folder"), t!("cloud-settings-folder-caption"), text_field("cloud-folder", &ui.folder)));
        }
        if ui.kind == CloudKind::WebDav {
            rows = rows.row(self.setting_row(
                t!("cloud-settings-public-address"),
                t!("cloud-settings-public-address-caption"),
                text_field("cloud-public", &ui.public_url),
            ));
        } else if matches!(ui.kind, CloudKind::Nextcloud | CloudKind::OneDrive) {
            let expire = self
                .segmented(
                    "cloud-expire",
                    [0u32, 7, 30, 90]
                        .into_iter()
                        .map(|days| {
                            (
                                if days == 0 { t!("cloud-settings-never").to_owned() } else { t!("cloud-settings-days", days = days) },
                                act(CloudAction::Expire(days)),
                                ui.expire_days == days,
                            )
                        })
                        .collect(),
                    cx,
                )
                .max_w(px(360.));
            rows = rows.row(self.setting_row(t!("cloud-settings-links-expire"), "", expire));
        }
        let status = match (&ui.status, ui.checking || ui.connecting) {
            (_, true) if ui.connecting => Some((c.muted, t!("cloud-settings-connecting").to_owned())),
            (_, true) => Some((c.muted, t!("cloud-settings-checking").to_owned())),
            (Some((true, s)), _) => Some((c.accent, s.clone())),
            (Some((false, s)), _) => Some((c.danger, s.clone())),
            (None, false) => None,
        };
        rows.row(
            div()
                .flex()
                .flex_wrap()
                .items_center()
                .gap_2()
                .child(div().flex_1().min_w(px(160.)).text_size(px(12.)).text_color(rgb(c.muted)).child(state))
                .children(status.map(|(color, s)| div().text_size(px(12.)).text_color(rgb(color)).child(s)))
                .child(self.button("cloud-check", t!("cloud-settings-check"), "refresh", act(CloudAction::Check), false, cx))
                .when(saved.configured(), |d| d.child(self.button("cloud-remove", t!("cloud-settings-remove"), "trash", act(CloudAction::Remove), false, cx)))
                .child(self.button(
                    "cloud-save",
                    if ui.kind.uses_account() { t!("cloud-settings-connect") } else { t!("cloud-settings-save") },
                    "save",
                    act(CloudAction::Save),
                    true,
                    cx,
                )),
        )
    }
}
