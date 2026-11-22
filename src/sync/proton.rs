//! Proton Mail without Bridge: Proton's own API, through the
//! `protected_email_bridge` library, with no IMAP or SMTP in between.
//!
//! This is the protocol side only: SRP sign-in (with two-factor codes and
//! two-password mode), resuming a saved session, listing and decrypting
//! messages, label changes and sending. Manage's `proton.rs` decides what is
//! cached and when. Nothing here touches the database or the UI.
//!
//! The library is asynchronous; Picacho's account work runs on its own
//! background threads, so every call here blocks on one shared runtime.
use anyhow::{Context, Result, anyhow, bail, ensure};
use base64::Engine;
use protected_email_bridge::{
    Error as ProtonError,
    account::{PasswordArchive, PersistedAccount, StateStore},
    api::{
        ApiClient, HumanVerification, SessionTokens,
        mail::MessageFilter,
        types::{Label, MessageEvent, MessageMetadata, Notification, label_type},
    },
    auth,
    config::{DeploymentConfig, GatewaySettings, LocalArtifacts},
    gateway::AccountContext,
    literal,
    smtp::pipeline::{self, Submission},
};
use sha2::Digest;
use std::sync::{
    Arc, Mutex, OnceLock,
    atomic::{AtomicBool, Ordering},
};

/// `API_BASE_URL`: Proton's mail API.
const API_BASE_URL: &str = "https://mail-api.proton.me/";
/// `SRP_SALT_SUFFIX`: appended to the salt of password versions 3 and 4.
const SRP_SALT_SUFFIX: [u8; 6] = *b"proton";
/// `REFRESH_REDIRECT_URI`: sent with every token refresh.
const REFRESH_REDIRECT_URI: &str = "https://protonmail.ch/";
/// `MODULUS_SIGNING_KEY`: the key that signs the SRP moduli Proton hands out.
/// A modulus without its signature is refused, so a server in the middle
/// can't pick one that weakens the password proof.
const MODULUS_SIGNING_KEY: &str = "-----BEGIN PGP PUBLIC KEY BLOCK-----

xjMEXAHLgxYJKwYBBAHaRw8BAQdAFurWXXwjTemqjD7CXjXVyKf0of7n9Ctm
L8v9enkzggHNEnByb3RvbkBzcnAubW9kdWx1c8J3BBAWCgApBQJcAcuDBgsJ
BwgDAgkQNQWFxOlRjyYEFQgKAgMWAgECGQECGwMCHgEAAPGRAP9sauJsW12U
MnTQUZpsbJb53d0Wv55mZIIiJL2XulpWPQD/V6NglBd96lZKBmInSXX/kXat
Sv+y0io+LR8i2+jV+AbOOARcAcuDEgorBgEEAZdVAQUBAQdAeJHUz1c9+KfE
kSIgcBRE3WuXC4oj5a2/U3oASExGDW4DAQgHwmEEGBYIABMFAlwBy4MJEDUF
hcTpUY8mAhsMAAD/XQD8DxNI6E78meodQI+wLsrKLeHn32iLvUqJbVDhfWSU
WO4BAMcm1u02t4VKw++ttECPt+HUgPUq5pqQWe5Q2cW4TMsE
=Y4Mw
-----END PGP PUBLIC KEY BLOCK-----
";

/// What builds set for Proton's `x-pm-appversion`
/// (`<platform>-<token>@<version>`), as with the other sign-in
/// registrations: `PICACHO_PROTON_APP_TOKEN` and `PICACHO_PROTON_APP_VERSION`.
const APP_TOKEN: Option<&str> = option_env!("PICACHO_PROTON_APP_TOKEN");
const APP_VERSION: Option<&str> = option_env!("PICACHO_PROTON_APP_VERSION");
/// The `<platform>` Proton sees in place of the one this build runs on
/// (`linux`, `macos` or `windows`): `PICACHO_PROTON_APP_PLATFORM_OVERRIDE`.
/// It doesn't make Direct available by itself.
const APP_PLATFORM: Option<&str> = option_env!("PICACHO_PROTON_APP_PLATFORM_OVERRIDE");
/// The same three for registering this device for push notifications
/// alone (`POST /core/v4/devices`): `PICACHO_PROTON_PUSH_APP_TOKEN`,
/// `PICACHO_PROTON_PUSH_APP_VERSION` and `PICACHO_PROTON_PUSH_APP_PLATFORM`.
/// Each one unset is the one above.
const PUSH_APP_TOKEN: Option<&str> = option_env!("PICACHO_PROTON_PUSH_APP_TOKEN");
const PUSH_APP_VERSION: Option<&str> = option_env!("PICACHO_PROTON_PUSH_APP_VERSION");
const PUSH_APP_PLATFORM: Option<&str> = option_env!("PICACHO_PROTON_PUSH_APP_PLATFORM");

fn set(value: Option<&'static str>) -> Option<&'static str> {
    value.filter(|v| !v.trim().is_empty())
}
/// Whether a build with these values may sign in to Proton directly: it
/// needs at least one of them. The one left out keeps its default.
fn registered(token: Option<&str>, version: Option<&str>) -> bool {
    [token, version]
        .into_iter()
        .any(|v| v.is_some_and(|v| !v.trim().is_empty()))
}
/// Why Direct isn't offered, when it isn't.
pub const UNAVAILABLE: &str = "This copy of Picacho was built without Proton's app registration, so Proton Mail Bridge is the only way to connect Proton Mail.";

/// Whether this build offers Proton without Bridge (Direct). Without its
/// app token and version, only Proton Mail Bridge is offered. Tests always
/// have it, to exercise Direct; the window's flag can turn it off.
pub fn available() -> bool {
    cfg!(test) || registered(APP_TOKEN, APP_VERSION)
}
/// The app name Proton sees in `x-pm-appversion`.
fn app_token() -> &'static str {
    set(APP_TOKEN).unwrap_or("picacho")
}
/// The app version Proton sees in `x-pm-appversion`.
fn app_version() -> &'static str {
    set(APP_VERSION).unwrap_or(env!("CARGO_PKG_VERSION"))
}

fn config() -> Arc<DeploymentConfig> {
    static CONFIG: OnceLock<Arc<DeploymentConfig>> = OnceLock::new();
    CONFIG
        .get_or_init(|| {
            Arc::new(DeploymentConfig {
                api_base_url: API_BASE_URL.into(),
                modulus_signing_key: MODULUS_SIGNING_KEY.into(),
                srp_salt_suffix: SRP_SALT_SUFFIX,
                refresh_redirect_uri: REFRESH_REDIRECT_URI.into(),
                app_product_token: app_token().into(),
                app_version: app_version().into(),
                app_platform: set(APP_PLATFORM).map(|p| p.trim().to_owned()),
                push_app_product_token: set(PUSH_APP_TOKEN).map(|t| t.trim().to_owned()),
                push_app_version: set(PUSH_APP_VERSION).map(|v| v.trim().to_owned()),
                push_app_platform: set(PUSH_APP_PLATFORM).map(|p| p.trim().to_owned()),
                product_name: "Picacho".into(),
                api_tls_pins: Vec::new(),
            })
        })
        .clone()
}

/// The deployment every Proton client here uses (push's too).
pub(crate) fn deployment() -> Arc<DeploymentConfig> {
    config()
}

/// One runtime for every Proton account, started on first use.
pub(crate) fn runtime() -> &'static tokio::runtime::Runtime {
    static RUNTIME: OnceLock<tokio::runtime::Runtime> = OnceLock::new();
    RUNTIME.get_or_init(|| {
        tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .thread_name("proton")
            .enable_all()
            .build()
            .expect("start the Proton runtime")
    })
}

fn block_on<T>(future: impl std::future::Future<Output = T>) -> T {
    runtime().block_on(future)
}

/// Proton's error, said the way the rest of Picacho expects: a session
/// Proton ended needs the person to sign in again (`AccessFailure`), and a
/// network failure can be tried again later (`Unreachable`).
fn explain(e: ProtonError) -> anyhow::Error {
    use crate::manage::status::AccessFailure;
    let auth = matches!(e, ProtonError::Deauthenticated)
        || e.status() == Some(401)
        || e.api().is_some_and(|a| a.code == 10013);
    if auth {
        return anyhow!("Proton ended this session. Sign in again to keep using this account.")
            .context(AccessFailure::Authentication);
    }
    // Human verification outside a sign-in: it is passed by signing in
    // again, which shows Proton's check.
    if matches!(e, ProtonError::HumanVerificationRequired { .. })
        || e.api().is_some_and(|a| a.code == 9001)
    {
        return anyhow!(
            "Proton wants to check that it's you. Sign in again to this account to pass its check."
        )
        .context(AccessFailure::Authentication);
    }
    if let ProtonError::Network(why) = &e {
        return anyhow::Error::new(crate::sync::retry::Unreachable(format!(
            "Couldn't reach Proton: {why}"
        )));
    }
    if let Some(api) = e.api()
        && matches!(api.code, 5003 | 5005 | 2064)
    {
        return anyhow!(
            "Proton didn't accept this app ({}). Proton Mail Bridge still works with this account.",
            api.message
        );
    }
    anyhow::Error::new(e)
}

/// A failure during a sign-in. There is no saved session yet for Proton
/// to have ended, so a refusal that would mean that elsewhere (401, 10013)
/// is reported with Proton's own words and which step it came at.
fn explain_sign_in(e: ProtonError, step: &str) -> anyhow::Error {
    let refused = matches!(e, ProtonError::Deauthenticated)
        || e.status() == Some(401)
        || e.api().is_some_and(|a| a.code == 10013);
    if !refused {
        return explain(e);
    }
    let said = match e.api() {
        Some(a) if !a.message.trim().is_empty() => format!(
            " Proton said: “{}” (HTTP {}, code {}).",
            a.message.trim(),
            a.status,
            a.code
        ),
        Some(a) => format!(" (HTTP {}, code {}).", a.status, a.code),
        None => format!(" ({e})."),
    };
    anyhow!("Proton refused this sign-in {step}.{said} Start again with your password.")
}

trait Explained<T> {
    fn explained(self) -> Result<T>;
}
impl<T> Explained<T> for std::result::Result<T, ProtonError> {
    fn explained(self) -> Result<T> {
        self.map_err(explain)
    }
}

/// What a Proton account keeps in the keychain: the session, and the
/// salted key passphrase that unlocks its keys. The password itself isn't
/// kept, as with Proton Mail Bridge.
#[derive(Clone, Default, PartialEq, Eq)]
pub struct Saved {
    pub uid: String,
    pub access_token: String,
    pub refresh_token: String,
    /// The salted key passphrase, base64.
    pub key: String,
}
/// Only the session's id: its tokens and key passphrase stay out of logs.
impl std::fmt::Debug for Saved {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Saved")
            .field("uid", &self.uid)
            .finish_non_exhaustive()
    }
}
impl Saved {
    fn tokens(&self) -> SessionTokens {
        SessionTokens {
            uid: self.uid.clone(),
            access_token: self.access_token.clone(),
            refresh_token: self.refresh_token.clone(),
        }
    }
    fn with_tokens(&self, t: &SessionTokens) -> Self {
        Self {
            uid: t.uid.clone(),
            access_token: t.access_token.clone(),
            refresh_token: t.refresh_token.clone(),
            key: self.key.clone(),
        }
    }
}

/// A finished sign-in.
pub struct SignedIn {
    pub saved: Saved,
    pub name: String,
    /// Every address of the account that is turned on, primary first.
    pub addresses: Vec<String>,
}

/// The check's result from one of Proton's verification page's messages
/// (`display::verify` on desktops, `android::verify`), if it is the one
/// saying it passed: (token, type). `Err` when the page asks to close.
#[cfg_attr(target_os = "ios", allow(dead_code))]
pub(crate) fn verification_result(message: &str) -> std::result::Result<Option<(String, String)>, ()> {
    let Ok(value) = serde_json::from_str::<serde_json::Value>(message) else {
        return Ok(None);
    };
    let text = |v: &serde_json::Value| v.as_str().map(str::trim).unwrap_or_default().to_owned();
    let (token, kind) = match value["type"].as_str().unwrap_or_default() {
        // verify.proton.me
        "HUMAN_VERIFICATION_SUCCESS" => (
            text(&value["payload"]["token"]),
            text(&value["payload"]["type"]),
        ),
        // Proton's older CAPTCHA page (/core/v4/captcha)
        "pm_captcha" => (text(&value["token"]), "captcha".to_owned()),
        "CLOSE" | "HUMAN_VERIFICATION_CLOSE" => return Err(()),
        _ => return Ok(None),
    };
    let fits = |s: &str, max: usize| {
        !s.is_empty() && s.len() <= max && s.bytes().all(|b| b.is_ascii_graphic())
    };
    if !fits(&token, 4096) {
        return Ok(None);
    }
    let kind = if fits(&kind, 32) {
        kind
    } else {
        "captcha".to_owned()
    };
    Ok(Some((token, kind)))
}

/// The argument that starts Picacho as Proton's verification window
/// (`display::verify`) rather than as the app.
#[cfg(not(any(target_os = "android", target_os = "ios")))]
pub const VERIFY_FLAG: &str = "--proton-verify";

/// Proton's verification page for a sign-in it wants checked.
#[cfg(not(target_os = "ios"))]
fn verification_url(methods: &[String], token: &str) -> String {
    let methods: Vec<&str> = methods
        .iter()
        .map(|m| m.trim())
        .filter(|m| !m.is_empty() && m.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_'))
        .collect();
    let methods = if methods.is_empty() {
        "captcha".to_owned()
    } else {
        methods.join(",")
    };
    let mut url = url::Url::parse("https://verify.proton.me/").expect("a valid URL");
    url.query_pairs_mut()
        .append_pair("methods", &methods)
        .append_pair("token", token)
        .append_pair("embed", "true");
    url.into()
}

/// What the verification window printed: the solved token and its type.
#[cfg(not(any(target_os = "android", target_os = "ios")))]
fn verified(output: &[u8]) -> Option<HumanVerification> {
    let line = String::from_utf8_lossy(output);
    let value: serde_json::Value = serde_json::from_str(line.lines().last()?.trim()).ok()?;
    let token = value["token"].as_str()?.trim();
    let kind = value["type"].as_str().unwrap_or("captcha").trim();
    let fits = |s: &str| !s.is_empty() && s.bytes().all(|b| b.is_ascii_graphic());
    (fits(token) && fits(kind)).then(|| HumanVerification {
        token: token.to_owned(),
        methods: vec![kind.to_owned()],
    })
}

/// Shows Proton's human verification (its CAPTCHA, or a code by email or
/// text where it offers those) in the system webview, in a window of its
/// own, and waits until it passes, the window is closed, or the sign-in is
/// cancelled.
#[cfg(target_os = "ios")]
fn verify_human(_: &[String], _: &str, _: &AtomicBool) -> Result<HumanVerification> {
    bail!(
        "Proton wants to check that it's you, but its check can't be shown on iPhones yet. Sign in once at account.proton.me and try again."
    )
}
/// On Android the page opens in an activity of its own, with a WebView
/// (`VerifyActivity`).
#[cfg(target_os = "android")]
fn verify_human(methods: &[String], token: &str, cancel: &AtomicBool) -> Result<HumanVerification> {
    match crate::android::verify(&verification_url(methods, token), cancel)? {
        Some((token, kind)) => Ok(HumanVerification {
            token,
            methods: vec![kind],
        }),
        None => bail!("Proton's check was closed before it finished. Sign in again to try it again."),
    }
}
#[cfg(not(any(target_os = "android", target_os = "ios")))]
fn verify_human(methods: &[String], token: &str, cancel: &AtomicBool) -> Result<HumanVerification> {
    use std::io::Read;
    // The window is WebView2's, whose loader is delay-loaded and whose
    // runtime Windows 10 may lack: asked here, as the check's process would
    // otherwise end without a word.
    #[cfg(windows)]
    if !crate::display::windows::has_webview2() {
        bail!(
            "Proton wants to check that it's you, which needs Microsoft Edge WebView2. Install it from go.microsoft.com/fwlink/p/?LinkId=2124703 and sign in again."
        );
    }
    let exe = std::env::current_exe().context("Couldn't find Picacho to show Proton's check")?;
    let mut child = std::process::Command::new(exe)
        .arg(VERIFY_FLAG)
        .arg(verification_url(methods, token))
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .spawn()
        .context("Couldn't open a window for Proton's check")?;
    // Read as it is written: a webview that logs to standard output could
    // otherwise fill the pipe and stall the window until it was cancelled.
    let reading = child.stdout.take().map(|mut out| {
        std::thread::spawn(move || {
            let mut output = Vec::new();
            let _ = out.by_ref().take(1024 * 1024).read_to_end(&mut output);
            // Past the limit, keep draining so the window never blocks.
            let _ = std::io::copy(&mut out, &mut std::io::sink());
            output
        })
    });
    let status = loop {
        if cancel.load(Ordering::Relaxed) {
            let _ = child.kill();
            let _ = child.wait();
            bail!("Sign-in cancelled.");
        }
        if let Some(status) = child.try_wait()? {
            break status;
        }
        std::thread::sleep(std::time::Duration::from_millis(100));
    };
    let output = reading.and_then(|r| r.join().ok()).unwrap_or_default();
    match (status.code(), verified(&output)) {
        (Some(0), Some(hv)) => Ok(hv),
        (Some(64 | 70), _) => bail!(
            "Proton wants to check that it's you, but its check couldn't be shown here. Use Proton Mail Bridge, or sign in once at account.proton.me and try again."
        ),
        _ => bail!("Proton's check was closed before it finished. Sign in again to try it again."),
    }
}

/// What a Proton sign-in needs next.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Next {
    /// A second factor: a code from an authenticator app (`code`), and/or a
    /// security key or passkey (`key`), whichever the account has.
    SecondFactor { code: bool, key: bool },
    /// The mailbox password (two-password mode).
    MailboxPassword,
    /// Nothing more: the keys can be unlocked.
    Ready,
}

/// Why a sign-in step can't be taken: the sign-in was ended.
const ENDED: &str = "This sign-in has ended. Start again with your password.";

/// A Proton sign-in in progress, one step at a time: the password was
/// accepted (`begin`), and Proton may still want a second factor and the
/// mailbox password. Dropped or `abort`ed, its session is signed out.
pub struct Login {
    pending: Option<auth::PendingLogin>,
}

impl Login {
    /// Sends the password (SRP). When Proton asks for human verification,
    /// its page is shown in the system webview (`verifying` is called
    /// first) and the password is sent again with the result.
    pub fn begin(
        username: &str,
        password: &str,
        cancel: &AtomicBool,
        verifying: &dyn Fn(),
    ) -> Result<Self> {
        block_on(async {
            let client = ApiClient::new(config()).explained()?;
            let mut hv = None;
            let mut checks = 0;
            loop {
                match auth::login(&client, username, password.as_bytes(), hv.take(), |_| false)
                    .await
                {
                    Ok(p) => return Ok(Self { pending: Some(p) }),
                    Err(ProtonError::HumanVerificationRequired { methods, token }) => {
                        if checks >= 3 {
                            bail!(
                                "Proton still asks to check that it's you after {checks} tries. Wait a while, then sign in again."
                            );
                        }
                        checks += 1;
                        verifying();
                        hv = Some(verify_human(&methods, &token, cancel)?);
                        if cancel.load(Ordering::Relaxed) {
                            bail!("Sign-in cancelled.");
                        }
                    }
                    Err(e) if e.api().is_some_and(|a| a.code == 8002) => {
                        bail!("Proton didn't accept this email address and password.")
                    }
                    Err(ProtonError::Srp(why)) => bail!(
                        "Proton's sign-in couldn't be verified ({why}). Nothing was sent; try again."
                    ),
                    Err(e) => {
                        return Err(explain_sign_in(e, "at the password"))
                            .context("Couldn't sign in to Proton");
                    }
                }
            }
        })
    }
    fn pending(&self) -> Result<&auth::PendingLogin> {
        self.pending.as_ref().context(ENDED)
    }
    fn pending_mut(&mut self) -> Result<&mut auth::PendingLogin> {
        self.pending.as_mut().context(ENDED)
    }
    /// Whether the sign-in can't go on (Proton or the library ended it);
    /// it has to start again with the password.
    pub fn ended(&self) -> bool {
        self.pending.is_none()
    }
    /// What Proton needs next.
    pub fn next(&self) -> Result<Next> {
        let p = self.pending()?;
        Ok(if p.needs_second_factor() {
            let tf = &p.auth.two_factor;
            Next::SecondFactor {
                code: tf.totp(),
                key: tf.fido2()
                    && crate::sync::fido::usable(tf.fido2.authentication_options.as_ref()),
            }
        } else if p.needs_mailbox_password() {
            Next::MailboxPassword
        } else {
            Next::Ready
        })
    }
    /// The code from an authenticator app.
    pub fn submit_code(&mut self, code: &str) -> Result<()> {
        let code: String = code.chars().filter(|c| !c.is_whitespace()).collect();
        ensure!(
            !code.is_empty(),
            "Enter the code from your authenticator app."
        );
        let pending = self.pending_mut()?;
        block_on(pending.submit_totp(&code)).map_err(|e| {
            if e.api().is_some_and(|a| a.status == 422 || a.code == 8002) {
                anyhow!("Proton didn't accept that code. Enter the current one from your authenticator app.")
            } else {
                explain_sign_in(e, "at the authenticator code")
            }
        })
    }
    /// A security key or passkey: asks it to sign Proton's challenge, then
    /// hands Proton what it signed. `prompt` follows what the key waits for.
    pub fn submit_key(
        &mut self,
        method: crate::sync::fido::Method,
        pin: Option<String>,
        prompt: Arc<Mutex<crate::sync::fido::Prompt>>,
        cancel: &AtomicBool,
    ) -> Result<()> {
        let pending = self.pending_mut()?;
        let request = pending
            .fido2_request()
            .map_err(|e| explain_sign_in(e, "at the security key"))?;
        let options = crate::sync::fido::request_options(&request.options)?;
        block_on(async {
            let signed = crate::sync::fido::assert(
                method,
                options,
                &request.client_data_json,
                &request.client_data_hash,
                pin,
                prompt,
                cancel,
            )
            .await?;
            // What Proton checks the signature against: the client data
            // the authenticator signed.
            let request = auth::Fido2Request {
                client_data_hash: if signed.client_data_json == request.client_data_json {
                    request.client_data_hash
                } else {
                    sha2::Sha256::digest(&signed.client_data_json).into()
                },
                client_data_json: signed.client_data_json.clone(),
                ..request
            };
            pending
                .submit_fido2(
                    &request,
                    &signed.authenticator_data,
                    &signed.signature,
                    &signed.credential_id,
                )
                .await
                .map_err(|e| {
                    if e.api().is_some_and(|a| a.status == 422 || a.code == 8002) {
                        anyhow!("Proton didn't accept that security key or passkey. Use one registered with this account.")
                    } else {
                        explain_sign_in(e, "at the security key")
                    }
                })
        })
    }
    /// Unlocks the account's keys, with the mailbox password in
    /// two-password mode. A wrong mailbox password keeps the sign-in, to try
    /// another; any other failure ends it.
    pub fn finish(&mut self, mailbox_password: Option<&str>) -> Result<SignedIn> {
        let pending = self.pending()?;
        let mailbox = mailbox_password
            .map(str::as_bytes)
            .filter(|m| !m.is_empty());
        if pending.needs_mailbox_password() && mailbox.is_none() {
            bail!("Enter the mailbox password.");
        }
        let done = match block_on(pending.finish(mailbox)) {
            Ok(done) => done,
            Err(ProtonError::KeyUnlock) => {
                return Err(anyhow!(if pending.needs_mailbox_password() {
                    "That mailbox password didn't unlock this account's keys."
                } else {
                    "The password signed in, but didn't unlock this account's keys."
                }));
            }
            // The library signed the session out.
            Err(e) => {
                self.pending = None;
                return Err(explain_sign_in(e, "while unlocking the account"));
            }
        };
        // Signed in: the session is the account's now, not to be signed out.
        self.pending = None;
        let mut addresses = done.addresses.clone();
        protected_email_bridge::api::types::sort_addresses(&mut addresses);
        let name = [done.user.display_name.trim(), done.user.name.trim()]
            .into_iter()
            .find(|n| !n.is_empty())
            .unwrap_or_default()
            .to_owned();
        Ok(SignedIn {
            saved: Saved {
                uid: done.tokens.uid.clone(),
                access_token: done.tokens.access_token.clone(),
                refresh_token: done.tokens.refresh_token.clone(),
                key: base64::engine::general_purpose::STANDARD.encode(&*done.salted_key_passphrase),
            },
            name,
            addresses: addresses
                .iter()
                .filter(|a| a.is_enabled())
                .map(|a| a.email.clone())
                .collect(),
        })
    }
    /// Gives up, signing the session out.
    pub fn abort(mut self) {
        if let Some(p) = self.pending.take() {
            block_on(async {
                let _ = p.abort().await;
            });
        }
    }
}
impl Drop for Login {
    fn drop(&mut self) {
        if let Some(p) = self.pending.take() {
            // A sign-in left half done: sign it out in the background.
            runtime().spawn(async move {
                let _ = p.abort().await;
            });
        }
    }
}

/// What Proton's event stream said since an earlier event.
pub struct Events {
    /// The newest event id, to ask from next time.
    pub latest: String,
    /// Message changes.
    pub messages: Vec<MessageEvent>,
    /// Proton says to refresh everything.
    pub refresh: bool,
    /// Account notices Proton shows its users (`Notifications`).
    pub notices: Vec<Notification>,
}

/// Ends a saved session at Proton (removing the account here).
pub fn sign_out(saved: &Saved) {
    block_on(async {
        if let Ok(client) = ApiClient::new(config()) {
            client.set_tokens(saved.tokens()).await;
            let _ = client.logout().await;
        }
    });
}

/// The library keeps gateway state through this; Picacho keeps its own
/// (the keychain and its cache), so nothing is written.
struct NoState;
impl StateStore for NoState {
    fn load_all(&self) -> protected_email_bridge::Result<Vec<PersistedAccount>> {
        Ok(Vec::new())
    }
    fn save(&self, _: &PersistedAccount) -> protected_email_bridge::Result<()> {
        Ok(())
    }
    fn delete(&self, _: &str) -> protected_email_bridge::Result<()> {
        Ok(())
    }
    fn load_archive(&self) -> protected_email_bridge::Result<PasswordArchive> {
        Ok(PasswordArchive::default())
    }
    fn save_archive(&self, _: &PasswordArchive) -> protected_email_bridge::Result<()> {
        Ok(())
    }
}

/// Proton lists at most this many messages per request.
const PAGE: usize = 150;

/// An attachment of a message too large to download whole: its id, name,
/// size and type.
pub type AttachmentInfo = (String, String, usize, String);

/// A signed-in Proton account with its keys unlocked, held in memory.
pub struct Session {
    ctx: Arc<AccountContext>,
    saved: Mutex<Saved>,
    /// Set when Proton issued new tokens (each refresh replaces the
    /// refresh token), until `renewed` hands them to the keychain.
    renewed: Arc<Mutex<Option<SessionTokens>>>,
}

/// A folder or label of the account.
#[derive(Clone, Debug)]
pub struct Folder {
    pub id: String,
    /// Its full name, with `/` between levels.
    pub path: String,
    pub depth: usize,
    /// A folder (where a message is) rather than a label (tags on it).
    pub exclusive: bool,
}

impl Session {
    /// Resumes a saved session and unlocks its keys.
    pub fn open(saved: &Saved) -> Result<Self> {
        use crate::manage::status::AccessFailure;
        if saved.uid.is_empty() || saved.refresh_token.is_empty() {
            return Err(anyhow!("Sign in to Proton again.").context(AccessFailure::Authentication));
        }
        let salted = base64::engine::general_purpose::STANDARD
            .decode(&saved.key)
            .map_err(|_| {
                anyhow!("Sign in to Proton again.").context(AccessFailure::Authentication)
            })?;
        let renewed = Arc::new(Mutex::new(None));
        let ctx = block_on(async {
            let client = ApiClient::new(config()).explained()?;
            client.set_tokens(saved.tokens()).await;
            let slot = renewed.clone();
            client
                .set_token_listener(Arc::new(move |t: &SessionTokens| {
                    *slot.lock().unwrap() = Some(t.clone());
                }))
                .await;
            let (user, ring) = match auth::unlock_from_passphrase(&client, &salted).await {
                Err(ProtonError::KeyUnlock) => {
                    return Err(anyhow!(
                        "This account's keys changed at Proton (a new password?). Sign in again."
                    )
                    .context(AccessFailure::Authentication));
                }
                other => other.explained()?,
            };
            let addresses = client.get_addresses().await.explained()?;
            let mut persisted = PersistedAccount {
                user_id: user.id.clone(),
                primary_email: user.email.clone(),
                ..Default::default()
            };
            persisted.set_salted_key_passphrase(&salted);
            let ctx = AccountContext::new(
                client,
                Arc::new(LocalArtifacts::default()),
                Arc::new(GatewaySettings::default()),
                Arc::new(NoState),
                persisted,
                user,
                addresses,
                ring,
            );
            ctx.reload_labels().await.explained()?;
            Ok::<_, anyhow::Error>(ctx)
        })?;
        Ok(Self {
            ctx,
            saved: Mutex::new(saved.clone()),
            renewed,
        })
    }
    /// New tokens to save, if Proton issued any since the last call.
    pub fn renewed(&self) -> Option<Saved> {
        let tokens = self.renewed.lock().unwrap().take()?;
        let mut saved = self.saved.lock().unwrap();
        *saved = saved.with_tokens(&tokens);
        Some(saved.clone())
    }
    /// Tokens `renewed` handed out that couldn't be saved: handed out
    /// again next time, unless newer ones came meanwhile.
    pub fn unsaved(&self, saved: Saved) {
        let mut renewed = self.renewed.lock().unwrap();
        if renewed.is_none() {
            *renewed = Some(saved.tokens());
        }
    }
    /// What registering this session for push notifications needs
    /// (`proton_push`); `account` is Picacho's id for the account.
    pub fn push_registration(&self, account: &str) -> crate::sync::proton_push::Registration {
        crate::sync::proton_push::Registration {
            account: account.to_owned(),
            uid: self.saved.lock().unwrap().uid.clone(),
            user_id: self.ctx.user_id(),
            client: self.ctx.client.clone(),
        }
    }
    /// The account's folders and labels (not Proton's own mailboxes).
    pub fn folders(&self) -> Vec<Folder> {
        let mut out: Vec<Folder> = self
            .ctx
            .labels()
            .into_values()
            .filter(|l| matches!(l.kind, label_type::FOLDER | label_type::LABEL))
            .map(|l| Folder {
                depth: l.path.matches('/').count(),
                path: if l.path.is_empty() {
                    l.name.clone()
                } else {
                    l.path.clone()
                },
                exclusive: l.kind == label_type::FOLDER,
                id: l.id,
            })
            .collect();
        out.sort_by_key(|f| f.path.to_lowercase());
        out
    }
    /// Reads the folders and labels again.
    pub fn reload_folders(&self) -> Result<()> {
        block_on(self.ctx.reload_labels()).explained()
    }
    pub fn label(&self, id: &str) -> Option<Label> {
        self.ctx.label(id)
    }
    /// One page of a mailbox's messages, newest first.
    pub fn list(&self, label_id: &str, page: usize, size: usize) -> Result<Vec<MessageMetadata>> {
        let filter = MessageFilter {
            label_id: label_id.into(),
            desc: true,
            ..Default::default()
        };
        block_on(self.ctx.client.list_messages(&filter, page, size)).explained()
    }
    /// How many messages a mailbox has.
    pub fn count(&self, label_id: &str) -> Result<usize> {
        let filter = MessageFilter {
            label_id: label_id.into(),
            ..Default::default()
        };
        let n = block_on(self.ctx.client.count_messages(&filter)).explained()?;
        Ok(usize::try_from(n).unwrap_or(0))
    }
    /// The current state of these messages; ones that are gone are left out.
    pub fn metadata(&self, ids: &[String]) -> Result<Vec<MessageMetadata>> {
        block_on(self.ctx.client.list_messages_by_id(ids, None)).explained()
    }
    /// A message as standard MIME (RFC 5322), decrypted with its address's
    /// keys, attachments included. A part that won't decrypt is replaced by
    /// a note saying so, as Bridge does.
    pub fn literal(&self, id: &str) -> Result<Vec<u8>> {
        block_on(self.literal_async(id))
    }
    /// Several messages as `literal` gives each, asked for at once rather
    /// than one after another: each its own result, in order.
    pub fn literals(&self, ids: &[String]) -> Vec<Result<Vec<u8>>> {
        block_on(futures::future::join_all(ids.iter().map(|id| self.literal_async(id))))
    }
    async fn literal_async(&self, id: &str) -> Result<Vec<u8>> {
        {
            let client = &self.ctx.client;
            let msg = client.get_message(id).await.map_err(|e| {
                if protected_email_bridge::api::mail::is_unprocessable(&e) {
                    anyhow!("This message is no longer at Proton.")
                } else {
                    explain(e)
                }
            })?;
            let ring = self.ctx.keyring(&msg.metadata.address_id).map_err(|_| {
                anyhow!("The keys of the address this message was sent to are locked or gone.")
            })?;
            let mut packets = Vec::with_capacity(msg.attachments.len());
            for a in &msg.attachments {
                packets.push(client.get_attachment(&a.id).await.explained()?);
            }
            Ok(literal::decrypt_and_build(&msg, &ring, packets, &self.ctx.art).0)
        }
    }
    /// A message too large to download whole: its text alone, as MIME, and
    /// its attachments (id, name, size, type), each downloaded by itself
    /// when saved (`attachment`).
    pub fn text_only(&self, id: &str) -> Result<(Vec<u8>, Vec<AttachmentInfo>)> {
        let msg = block_on(self.ctx.client.get_message(id)).explained()?;
        let ring = self.ctx.keyring(&msg.metadata.address_id).map_err(|_| {
            anyhow!("The keys of the address this message was sent to are locked or gone.")
        })?;
        let attachments = msg
            .attachments
            .iter()
            .map(|a| {
                (
                    a.id.clone(),
                    a.name.clone(),
                    usize::try_from(a.size).unwrap_or(0),
                    a.mime_type.clone(),
                )
            })
            .collect();
        let mut text = msg;
        text.attachments.clear();
        let body = ring
            .decrypt(text.body.as_bytes())
            .map_err(|e| e.to_string());
        let literal = literal::build_or_placeholder(&text, body, Vec::new(), &self.ctx.art).0;
        Ok((literal, attachments))
    }
    /// One attachment of a message, decrypted.
    pub fn attachment(&self, message_id: &str, attachment_id: &str) -> Result<Vec<u8>> {
        block_on(async {
            let msg = self.ctx.client.get_message(message_id).await.explained()?;
            let attachment = msg
                .attachments
                .iter()
                .find(|a| a.id == attachment_id)
                .context("That attachment is no longer at Proton.")?;
            let ring = self.ctx.keyring(&msg.metadata.address_id).explained()?;
            let data = self
                .ctx
                .client
                .get_attachment(attachment_id)
                .await
                .explained()?;
            let mut packets = base64::engine::general_purpose::STANDARD
                .decode(&attachment.key_packets)
                .context("Proton sent an attachment key that couldn't be read.")?;
            packets.extend_from_slice(&data);
            Ok(ring.decrypt(&packets).explained()?.data)
        })
    }
    pub fn mark_read(&self, ids: &[String], read: bool) -> Result<()> {
        let client = &self.ctx.client;
        block_on(async {
            if read {
                client.mark_read(ids).await
            } else {
                client.mark_unread(ids).await
            }
        })
        .explained()
    }
    /// Adds a label, or moves the messages when it is a folder or one of
    /// Proton's mailboxes (Inbox, Archive, Spam, Trash).
    pub fn label_messages(&self, label_id: &str, ids: &[String], add: bool) -> Result<()> {
        let client = &self.ctx.client;
        block_on(async {
            if add {
                client.label_messages(label_id, ids).await
            } else {
                client.unlabel_messages(label_id, ids).await
            }
        })
        .explained()
    }
    pub fn mark_forwarded(&self, ids: &[String]) -> Result<()> {
        block_on(self.ctx.client.mark_forwarded(ids)).explained()
    }
    /// Deletes messages for good.
    pub fn delete(&self, ids: &[String]) -> Result<()> {
        block_on(self.ctx.client.delete_messages(ids)).explained()
    }
    /// Makes a label (tags) or folder; returns its id.
    pub fn create_label(&self, name: &str, folder: bool, parent: Option<&str>) -> Result<String> {
        let kind = if folder {
            label_type::FOLDER
        } else {
            label_type::LABEL
        };
        let label = block_on(self.ctx.client.create_label(
            name,
            protected_email_bridge::config::limits::NEW_LABEL_COLOR,
            kind,
            parent,
        ))
        .explained()?;
        let id = label.id.clone();
        self.ctx.put_label(label);
        Ok(id)
    }
    pub fn rename_label(&self, id: &str, name: &str) -> Result<()> {
        let old = self
            .ctx
            .label(id)
            .context("That folder no longer exists at Proton.")?;
        let parent = Some(old.parent_id.as_str()).filter(|p| !p.is_empty());
        let label =
            block_on(self.ctx.client.update_label(id, name, &old.color, parent)).explained()?;
        self.ctx.put_label(label);
        Ok(())
    }
    pub fn delete_label(&self, id: &str) -> Result<()> {
        block_on(self.ctx.client.delete_label(id)).explained()?;
        self.ctx.remove_label(id);
        Ok(())
    }
    /// Storage used and allowed, in bytes.
    pub fn quota(&self) -> (u64, u64) {
        let user = self.ctx.user();
        (
            u64::try_from(user.used_space).unwrap_or(0),
            u64::try_from(user.max_space).unwrap_or(0),
        )
    }
    /// Where Proton's event stream is now.
    pub fn latest_event(&self) -> Result<String> {
        block_on(self.ctx.client.get_latest_event_id()).explained()
    }
    /// What changed since event `since`; see `Events`.
    pub fn events_since(&self, since: &str) -> Result<Events> {
        let events = block_on(self.ctx.client.get_events_chained(since)).explained()?;
        let latest = events
            .last()
            .map(|e| e.event_id.clone())
            .unwrap_or_else(|| since.to_owned());
        let refresh = events.iter().any(|e| e.refresh != 0);
        if events.iter().any(|e| !e.labels.is_empty()) {
            let _ = self.reload_folders();
        }
        let mut messages = Vec::new();
        let mut notices = Vec::new();
        for e in events {
            messages.extend(e.messages);
            notices.extend(e.notifications);
        }
        Ok(Events {
            latest,
            messages,
            refresh,
            notices,
        })
    }
    /// Every message of a mailbox, newest first, up to `most`.
    /// The ids of a label's unread messages, a page at a time: only the
    /// ids are kept, not every message's listing, which for a large folder
    /// was tens of megabytes at once.
    pub fn unread_ids(&self, label_id: &str) -> Result<Vec<String>> {
        let mut out = Vec::new();
        for page in 0.. {
            let batch = self.list(label_id, page, PAGE)?;
            let last = batch.len() < PAGE;
            out.extend(batch.into_iter().filter(|m| m.unread).map(|m| m.id));
            if last {
                break;
            }
        }
        Ok(out)
    }
    pub fn list_all(&self, label_id: &str, most: usize) -> Result<Vec<MessageMetadata>> {
        let mut out = Vec::new();
        for page in 0.. {
            let batch = self.list(label_id, page, PAGE)?;
            let last = batch.len() < PAGE;
            out.extend(batch);
            if last || out.len() >= most {
                break;
            }
        }
        out.truncate(most);
        Ok(out)
    }
    /// The address a message written here is from: the one its From names,
    /// if the account has it and may send from it; otherwise the primary.
    fn address_for(
        &self,
        from: Option<&str>,
    ) -> Result<protected_email_bridge::api::types::Address> {
        from.and_then(|f| self.ctx.owned_address(f))
            .filter(|a| a.is_enabled() && a.send)
            .or_else(|| self.ctx.primary_address())
            .context("This Proton account has no address to write from.")
    }
    /// Saves a draft written here (a whole MIME message) as a new Proton
    /// draft, encrypted with its address's key, attachments included, as
    /// Proton Mail Bridge does when a mail app saves to Drafts. Returns its id.
    pub fn save_draft(&self, raw: &[u8]) -> Result<String> {
        use protected_email_bridge::api::mail::{DraftTemplate, draft_action};
        let m = protected_email_bridge::smtp::parse::parse_message(raw, true).explained()?;
        let address = self.address_for(m.from.as_ref().map(|f| f.address.as_str()))?;
        let ring = self.ctx.keyring(&address.id).explained()?;
        let body = if m.rich_body.is_empty() {
            &m.plain_body
        } else {
            &m.rich_body
        };
        let template = DraftTemplate {
            subject: m.subject.clone(),
            sender: protected_email_bridge::api::types::Addr::new(
                address.display_name.clone(),
                address.email.clone(),
            ),
            to_list: m.to.clone(),
            cc_list: m.cc.clone(),
            bcc_list: m.bcc.clone(),
            body: ring.encrypt_armored(body.as_bytes(), false).explained()?,
            mime_type: if m.rich_body.is_empty() {
                "text/plain".into()
            } else {
                "text/html".into()
            },
            unread: false,
            external_id: m.external_id.clone(),
        };
        block_on(async {
            let draft = self
                .ctx
                .client
                .create_draft(&template, None, draft_action::REPLY)
                .await
                .explained()?;
            for a in &m.attachments {
                let mut a = a.clone();
                a.disposition = if a.disposition == "inline" && a.content_id.is_some() {
                    "inline".into()
                } else {
                    "attachment".into()
                };
                pipeline::upload_attachment(&self.ctx, &draft.metadata.id, &a, &ring)
                    .await
                    .explained()?;
            }
            Ok(draft.metadata.id)
        })
    }
    /// Adds a finished message to a mailbox (Sent, say) without sending it,
    /// encrypted for the account first, as Bridge does for a mail app's
    /// APPEND. Returns its id.
    pub fn import(&self, raw: &[u8], label_id: &str, sent: bool) -> Result<String> {
        use protected_email_bridge::api::mail::{ImportMetadata, ImportRequest};
        use protected_email_bridge::api::types::message_flag;
        let mut root = protected_email_bridge::mime::Part::parse(raw)
            .context("The message couldn't be read to save it at Proton.")?;
        let from = root.header.get("From").and_then(|f| {
            protected_email_bridge::mime::parse_address_list(&f)?
                .into_iter()
                .next()
                .map(|a| a.address)
        });
        let address = self.address_for(from.as_deref())?;
        let ring = self.ctx.keyring(&address.id).explained()?;
        protected_email_bridge::smtp::parse::ensure_text_part(&mut root);
        let literal = protected_email_bridge::import::encrypt_for_import(&root.to_bytes(), &ring)
            .explained()?;
        let request = ImportRequest {
            literal,
            metadata: ImportMetadata {
                address_id: address.id.clone(),
                label_ids: vec![label_id.to_owned()],
                unread: false,
                flags: if sent {
                    message_flag::SENT
                } else {
                    message_flag::RECEIVED
                },
            },
        };
        let mut results = block_on(
            self.ctx
                .client
                .import_messages(std::slice::from_ref(&request)),
        )
        .explained()?;
        results
            .pop()
            .context("Proton didn't say whether it kept the message.")?
            .explained()
    }
    /// Sends a finished MIME message from one of the account's addresses.
    /// Proton encrypts it for each recipient it can (other Proton users, and
    /// anyone with a trusted public key), signs it, and files it in Sent.
    pub fn send(&self, from: &str, recipients: &[String], raw: &[u8]) -> Result<()> {
        let sender = self
            .ctx
            .owned_address(from)
            .with_context(|| format!("{from} isn't one of this Proton account's addresses."))?;
        let submission = Submission {
            reverse_path: sender.email.clone(),
            recipients: recipients.to_vec(),
            data: raw.to_vec(),
        };
        block_on(pipeline::submit(Some(&self.ctx), &sender.id, submission))
            .map_err(|e| match e {
                pipeline::SubmitError::AuthFailed => explain(ProtonError::Deauthenticated),
                e => anyhow!("{e}"),
            })
            .map(|_| ())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A refusal during a sign-in names the step and keeps Proton's words,
    /// not "Proton ended this session" (there is none yet); the same
    /// refusal of a saved session still asks to sign in again.
    #[test]
    fn sign_in_refusals_keep_protons_words() {
        let refused = || {
            ProtonError::Api(protected_email_bridge::error::ApiError {
                status: 401,
                code: 10013,
                message: "Invalid refresh token".into(),
                details: None,
            })
        };
        let text = format!("{:#}", explain_sign_in(refused(), "at the password"));
        assert!(!text.contains("ended this session"), "{text}");
        assert!(text.contains("at the password"), "{text}");
        assert!(
            text.contains("Invalid refresh token") && text.contains("10013"),
            "{text}"
        );
        let text = format!(
            "{:#}",
            explain_sign_in(ProtonError::Deauthenticated, "while unlocking the account")
        );
        assert!(text.contains("while unlocking"), "{text}");
        let saved = explain(refused());
        assert!(format!("{saved:#}").contains("ended this session"));
        assert!(
            saved
                .downcast_ref::<crate::manage::status::AccessFailure>()
                .is_some()
        );
        // Other failures read as they always did.
        let wrong = explain_sign_in(ProtonError::KeyUnlock, "while unlocking the account");
        assert!(!format!("{wrong:#}").contains("refused this sign-in"));
    }

    /// A sign-in Proton ended (a failed last step signs it out) says so
    /// at each later step rather than panicking.
    #[test]
    fn an_ended_sign_in_refuses_further_steps() {
        let mut login = Login { pending: None };
        assert!(login.ended());
        assert!(login.next().is_err());
        assert!(login.submit_code("123456").is_err());
        let Err(error) = login.finish(Some("mailbox")) else {
            panic!("an ended sign-in finished");
        };
        assert_eq!(error.to_string(), ENDED);
        login.abort();
    }

    #[test]
    fn saved_sessions_keep_their_secrets_out_of_debug() {
        let saved = Saved {
            uid: "uid-1".into(),
            access_token: "access-secret".into(),
            refresh_token: "refresh-secret".into(),
            key: "key-secret".into(),
        };
        let shown = format!("{saved:?}");
        assert!(shown.contains("uid-1") && !shown.contains("secret"), "{shown}");
    }

    #[test]
    fn modulus_signing_key_parses() {
        let key = protected_email_bridge::crypto::parse_public_key(MODULUS_SIGNING_KEY.as_bytes())
            .expect("the modulus signing key is valid armor");
        assert_eq!(
            protected_email_bridge::crypto::first_identity_name(&key),
            "proton@srp.modulus"
        );
    }

    #[test]
    fn deployment_config() {
        let c = config();
        assert_eq!(c.srp_salt_suffix, *b"proton");
        assert!(c.api_base_url.starts_with("https://"));
        assert!(
            c.app_version_header()
                .contains(&format!("-{}@", app_token()))
        );
        // The platform named at build time, else the one built for.
        let platform = set(APP_PLATFORM)
            .map(str::trim)
            .unwrap_or(protected_email_bridge::config::platform_token());
        assert!(
            c.app_version_header().starts_with(&format!("{platform}-")),
            "{}",
            c.app_version_header()
        );
    }

    #[test]
    fn verification_page_and_result() {
        let url = verification_url(
            &["captcha".into(), "email".into(), "bad method!".into()],
            "t/k+n",
        );
        assert!(url.starts_with("https://verify.proton.me/?"));
        assert!(url.contains("methods=captcha%2Cemail&"));
        assert!(url.contains("token=t%2Fk%2Bn"));
        assert!(!url.contains("bad"));
        let hv = verified(b"noise\n{\"token\":\"solved\",\"type\":\"captcha\"}\n").unwrap();
        assert_eq!(hv.token, "solved");
        assert_eq!(hv.methods, ["captcha"]);
        assert!(verified(b"").is_none());
        assert!(verified(b"{\"token\":\"has space\"}").is_none());
    }

    #[test]
    fn direct_needs_a_token_or_a_version() {
        assert!(!registered(None, None));
        assert!(!registered(Some(""), Some("  ")));
        assert!(registered(Some("token"), None));
        assert!(registered(None, Some("1.2.3")));
        assert!(registered(Some("token"), Some("1.2.3")));
    }

    #[test]
    fn saved_tokens_replace_only_the_session() {
        let saved = Saved {
            uid: "u".into(),
            access_token: "a".into(),
            refresh_token: "r".into(),
            key: "k".into(),
        };
        let next = saved.with_tokens(&SessionTokens {
            uid: "u".into(),
            access_token: "a2".into(),
            refresh_token: "r2".into(),
        });
        assert_eq!(next.key, "k");
        assert_eq!(next.refresh_token, "r2");
    }
}
