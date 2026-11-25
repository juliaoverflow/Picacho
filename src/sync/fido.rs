//! Signing in with a security key or a passkey (WebAuthn/FIDO2), for
//! Proton's second factor.
//!
//! Three ways to reach an authenticator:
//! - a security key over USB, on every platform (CTAP2; passkeys stored
//!   on the key work the same way);
//! - a passkey on a phone: Picacho shows a QR code, the phone's camera reads
//!   it and the phone answers over the internet once Bluetooth shows it is
//!   nearby (caBLE, "hybrid");
//! - on Windows, Windows Hello, which covers the computer's own passkeys,
//!   security keys and phones itself.
//!
//! On Android a security key is held to the phone (NFC) or plugged in
//! (USB), and the phone's own passkeys come from Android's Credential
//! Manager (`android::keys`).
//!
//! On iPhones a security key is held to the phone (Core NFC, `ios::keys`),
//! and a passkey comes from another device by QR code, as on a computer.
//! Apple's passkey APIs only sign for sites that name the app in their
//! associated domains, which proton.me doesn't for Picacho.
//!
//! USB keys and phones sign exactly the client data Proton's library built
//! (its hash goes to the authenticator); Windows Hello builds its own, which
//! is sent instead. While one waits, `Prompt` says what for (touch the key,
//! scan the QR code), and the window shows it.
use anyhow::{Result, anyhow};
use serde_json::Value;
use std::sync::atomic::Ordering;
use std::sync::{Arc, Mutex, atomic::AtomicBool};
use webauthn_authenticator_rs::{
    AuthenticatorBackendHashedClientData,
    error::WebauthnCError,
    types::{CableRequestType, CableState, EnrollSampleStatus},
    ui::UiCallback,
};
use webauthn_rs_proto::{
    AllowCredentials, PublicKeyCredentialRequestOptions, UserVerificationPolicy,
};

/// How to reach the authenticator.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Method {
    /// A security key plugged into this computer (or, on Android, held to
    /// the phone).
    SecurityKey,
    /// A passkey on a phone, by QR code; on Android, this phone's own
    /// passkeys (its password manager).
    Phone,
    /// Windows Hello: the system's own passkeys, keys and phones.
    #[cfg(windows)]
    Windows,
}
impl Method {
    /// The ways this platform offers, in order. On Windows, Windows Hello
    /// handles keys and phones itself (and asks Windows 10 1903 and later
    /// to let it); without it (webauthn.dll), Picacho talks to them as on
    /// other systems.
    pub fn offered() -> &'static [Self] {
        #[cfg(windows)]
        if crate::display::windows::has_windows_hello() {
            return &[Self::Windows];
        }
        // iPads have no NFC to hold a key to.
        #[cfg(target_os = "ios")]
        if !crate::ios::keys::available() {
            return &[Self::Phone];
        }
        &[Self::SecurityKey, Self::Phone]
    }
    pub fn label(self) -> &'static str {
        match self {
            Self::SecurityKey => t!("sync-fido-use-a-security-key"),
            Self::Phone if cfg!(target_os = "android") => t!("sync-fido-use-a-passkey"),
            Self::Phone if cfg!(target_os = "ios") => t!("sync-fido-use-a-passkey-on-another-device"),
            Self::Phone => t!("sync-fido-use-a-passkey-on-your-phone"),
            #[cfg(windows)]
            Self::Windows => t!("sync-fido-use-a-passkey-or-security-key"),
        }
    }
}

/// What an authenticator is waiting for, for the window to show.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub enum Prompt {
    #[default]
    Idle,
    /// Looking for a security key: plug one in.
    Insert,
    /// The key is waiting to be touched.
    Touch,
    /// The key or phone is working.
    Working,
    /// Scan this QR code (a `FIDO:/…` address) with the phone.
    Qr(String),
    /// The phone is connecting; what it is doing.
    Phone(&'static str),
}

/// The key asked for its PIN and none (or a wrong one) was given: the
/// window asks for it, and the key is tried again with it.
#[derive(Debug)]
pub struct NeedsPin(pub &'static str);
impl std::fmt::Display for NeedsPin {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.0)
    }
}
impl std::error::Error for NeedsPin {}

/// What the authenticator signed.
pub struct Assertion {
    pub client_data_json: Vec<u8>,
    pub authenticator_data: Vec<u8>,
    pub signature: Vec<u8>,
    pub credential_id: Vec<u8>,
    /// The account the passkey is for (WebAuthn Level 3 §5.2.2
    /// `userHandle`), where the authenticator says: a discoverable
    /// credential (a passkey) does, a security key asked for a listed
    /// credential need not. Proton identifies the key by its id.
    #[allow(dead_code)]
    pub user_handle: Option<Vec<u8>>,
}
/// The user handle of a WebAuthn response, as WebAuthn §5.2.2 allows: none
/// where it is missing or empty (an authenticator that stored none).
fn user_handle(handle: Option<&[u8]>) -> Option<Vec<u8>> {
    // §5.4.3: at most 64 bytes; a longer one isn't a user handle.
    handle.filter(|h| !h.is_empty() && h.len() <= 64).map(<[u8]>::to_vec)
}

/// The authenticator library's questions, answered from the sign-in.
#[derive(Debug)]
struct Answers {
    pin: Option<String>,
    pin_asked: AtomicBool,
    prompt: Arc<Mutex<Prompt>>,
}
impl Answers {
    fn show(&self, p: Prompt) {
        *self.prompt.lock().unwrap() = p;
    }
}
impl UiCallback for Answers {
    fn request_pin(&self) -> Option<String> {
        self.pin_asked.store(true, Ordering::Relaxed);
        self.pin.clone()
    }
    fn request_touch(&self) {
        self.show(Prompt::Touch);
    }
    fn processing(&self) {
        self.show(Prompt::Working);
    }
    fn fingerprint_enrollment_feedback(&self, _: u32, _: Option<EnrollSampleStatus>) {}
    fn cable_qr_code(&self, _: CableRequestType, url: String) {
        self.show(Prompt::Qr(url));
    }
    fn dismiss_qr_code(&self) {
        self.show(Prompt::Phone(t!("sync-fido-connecting-to-your-phone")));
    }
    fn cable_status_update(&self, state: CableState) {
        self.show(Prompt::Phone(match state {
            CableState::ConnectingToTunnelServer => t!("sync-fido-connecting-to-your-phone"),
            CableState::WaitingForAuthenticatorResponse => t!("sync-fido-confirm-the-sign-in-on-your"),
            _ => t!("sync-fido-talking-to-your-phone"),
        }));
    }
}

fn bytes(v: Option<&Value>) -> Option<Vec<u8>> {
    v?.as_array()?
        .iter()
        .map(|x| x.as_u64().and_then(|n| u8::try_from(n).ok()))
        .collect()
}

/// Whether `rp_id` is one of Proton's sites (or a site under one).
fn proton_rp(rp_id: &str) -> bool {
    // A host name, nothing else: "evil.example/.proton.me" ends the same,
    // but as an origin it is evil.example's.
    crate::sync::mta_sts::is_domain(rp_id)
        && ["proton.me", "protonmail.com", "protonmail.ch"]
        .iter()
        .any(|d| rp_id == *d || rp_id.ends_with(&format!(".{d}")))
}

/// Proton's `AuthenticationOptions` (bytes as arrays of numbers) as a
/// WebAuthn request.
pub fn request_options(options: &Value) -> Result<PublicKeyCredentialRequestOptions> {
    let pk = options
        .get("publicKey")
        .ok_or_else(|| anyhow!(t!("sync-fido-proton-didnt-say-which-security-keys")))?;
    let challenge =
        bytes(pk.get("challenge")).ok_or_else(|| anyhow!(t!("sync-fido-proton-sent-no-challenge")))?;
    let rp_id = pk
        .get("rpId")
        .and_then(Value::as_str)
        .ok_or_else(|| anyhow!(t!("sync-fido-proton-didnt-name-itself-for-the")))?
        .to_ascii_lowercase();
    // The relying party becomes the origin a passkey is asked for (Android's
    // Credential Manager, Windows Hello): only Proton's own sites, so an
    // answer naming another site can't get its passkeys signed.
    anyhow::ensure!(
        proton_rp(&rp_id),
        t!("sync-fido-proton-asked-for-a-security-key", rp_id = rp_id)
    );
    let allow_credentials = pk
        .get("allowCredentials")
        .and_then(Value::as_array)
        .map(|list| {
            list.iter()
                .filter_map(|c| {
                    // The ways each key is reached, as the server knows
                    // them, so the platform asks the right one first.
                    let transports = c.get("transports").and_then(Value::as_array).map(|t| {
                        t.iter()
                            .filter_map(Value::as_str)
                            .filter_map(|t| serde_json::from_value(Value::String(t.into())).ok())
                            .collect()
                    });
                    Some(AllowCredentials {
                        type_: "public-key".into(),
                        id: bytes(c.get("id"))?,
                        transports,
                    })
                })
                .collect()
        })
        .unwrap_or_default();
    let user_verification = match pk.get("userVerification").and_then(Value::as_str) {
        Some("required") => UserVerificationPolicy::Required,
        Some("discouraged") => UserVerificationPolicy::Discouraged_DO_NOT_USE,
        _ => UserVerificationPolicy::Preferred,
    };
    Ok(PublicKeyCredentialRequestOptions {
        challenge,
        timeout: pk
            .get("timeout")
            .and_then(Value::as_u64)
            .and_then(|t| u32::try_from(t).ok()),
        rp_id,
        allow_credentials,
        user_verification,
        hints: None,
        // The `appid` extension (FIDO U2F keys registered under the old
        // AppID), where Proton asks for it: such keys sign only for it.
        extensions: pk
            .get("extensions")
            .and_then(|e| e.get("appid"))
            .and_then(Value::as_str)
            .filter(|a| a.starts_with("https://") && url::Url::parse(a).ok().and_then(|u| u.host_str().map(proton_rp)).unwrap_or(false))
            .map(|appid| webauthn_rs_proto::extensions::RequestAuthenticationExtensions {
                appid: Some(appid.to_owned()),
                uvm: None,
                hmac_get_secret: None,
            }),
    })
}

/// The WebAuthn error, said for a person; a PIN problem asks for the PIN.
fn explain(e: WebauthnCError, answers: &Answers) -> anyhow::Error {
    use webauthn_authenticator_rs::error::CtapError::*;
    match e {
        WebauthnCError::Ctap(Ctap2PinInvalid | Ctap2PinAuthInvalid) => anyhow::Error::new(
            NeedsPin(t!("sync-fido-that-pin-isnt-right-enter-your")),
        ),
        WebauthnCError::Ctap(Ctap2PinBlocked | Ctap2PinAuthBlocked) => anyhow!(
            t!("sync-fido-this-security-key-is-locked-after")
        ),
        _ if answers.pin_asked.load(Ordering::Relaxed) && answers.pin.is_none() => {
            anyhow::Error::new(NeedsPin(
                t!("sync-fido-this-security-key-has-a-pin"),
            ))
        }
        WebauthnCError::PinTooShort | WebauthnCError::PinTooLong => anyhow::Error::new(NeedsPin(
            t!("sync-fido-that-pin-isnt-the-right-length"),
        )),
        WebauthnCError::Ctap(Ctap1Timeout | Ctap2UserActionTimeout | Ctap2ActionTimeout) => {
            anyhow!(t!("sync-fido-the-security-key-wasnt-touched-in"))
        }
        WebauthnCError::Cancelled | WebauthnCError::Ctap(Ctap2KeepAliveCancel) => {
            anyhow!(t!("sync-fido-using-the-security-key-was-cancelled"))
        }
        WebauthnCError::PermissionDenied => anyhow!(
            t!("sync-fido-picacho-isnt-allowed-to-use-bluetooth")
        ),
        WebauthnCError::NoBluetoothAdapter if cfg!(target_os = "ios") => anyhow!(
            t!("sync-fido-a-passkey-on-another-device-needs")
        ),
        WebauthnCError::NoBluetoothAdapter => anyhow!(
            t!("sync-fido-a-passkey-on-a-phone-needs")
        ),
        WebauthnCError::Ctap(Ctap2NoCredentials) => anyhow!(
            t!("sync-fido-that-key-or-passkey-isnt-registered")
        ),
        WebauthnCError::ApduTransmission => anyhow!(
            t!("sync-fido-the-connection-to-the-security-key")
        ),
        other => anyhow!(t!("sync-fido-the-security-key-or-passkey-didnt", error = format!("{:?}", other))),
    }
}

/// Asks an authenticator to sign Proton's challenge. `client_data_json` is
/// the client data Proton's library made (its SHA-256, `client_data_hash`,
/// is what keys sign); `prompt` follows what it waits for. Waits up to a
/// minute for a key or phone, or until `cancel`.
#[cfg(not(any(target_os = "android", target_os = "ios")))]
pub async fn assert(
    method: Method,
    options: PublicKeyCredentialRequestOptions,
    client_data_json: &[u8],
    client_data_hash: &[u8],
    pin: Option<String>,
    prompt: Arc<Mutex<Prompt>>,
    cancel: &AtomicBool,
) -> Result<Assertion> {
    let answers = Answers {
        pin: pin.filter(|p| !p.is_empty()),
        pin_asked: AtomicBool::new(false),
        prompt,
    };
    let result = match method {
        Method::SecurityKey => {
            answers.show(Prompt::Insert);
            security_key(&answers, options, client_data_hash, cancel).await
        }
        Method::Phone => phone(&answers, options, client_data_hash, cancel).await,
        // Offered only with webauthn.dll, which is delay-loaded: checked
        // again, as calling it without one would end the process.
        #[cfg(windows)]
        Method::Windows if !crate::display::windows::has_windows_hello() => Err(anyhow!(
            t!("sync-fido-windows-hello-isnt-available-on-this")
        )
        .into()),
        #[cfg(windows)]
        Method::Windows => windows(options, cancel).await,
    };
    answers.show(Prompt::Idle);
    let (credential, own_client_data) = result.map_err(|e| match e {
        Failure::Webauthn(e) => explain(e, &answers),
        Failure::Other(e) => e,
    })?;
    Ok(Assertion {
        client_data_json: own_client_data.unwrap_or_else(|| client_data_json.to_vec()),
        user_handle: user_handle(credential.response.user_handle.as_deref()),
        authenticator_data: credential.response.authenticator_data,
        signature: credential.response.signature,
        credential_id: credential.raw_id,
    })
}

enum Failure {
    Webauthn(WebauthnCError),
    Other(anyhow::Error),
}
impl From<WebauthnCError> for Failure {
    fn from(e: WebauthnCError) -> Self {
        Self::Webauthn(e)
    }
}
impl From<anyhow::Error> for Failure {
    fn from(e: anyhow::Error) -> Self {
        Self::Other(e)
    }
}
/// The signed credential, and the client data when the authenticator made
/// its own (Windows Hello).
#[cfg(not(target_os = "android"))]
type Signed =
    std::result::Result<(webauthn_rs_proto::PublicKeyCredential, Option<Vec<u8>>), Failure>;

/// On Android: a security key held to the phone or plugged in, or a
/// passkey from Android's Credential Manager.
#[cfg(target_os = "android")]
pub async fn assert(
    method: Method,
    options: PublicKeyCredentialRequestOptions,
    client_data_json: &[u8],
    client_data_hash: &[u8],
    pin: Option<String>,
    prompt: Arc<Mutex<Prompt>>,
    cancel: &AtomicBool,
) -> Result<Assertion> {
    let answers = Answers {
        pin: pin.filter(|p| !p.is_empty()),
        pin_asked: AtomicBool::new(false),
        prompt,
    };
    if method == Method::Phone {
        answers.show(Prompt::Working);
        let signed = crate::android::passkey(&options, client_data_json, client_data_hash, cancel);
        answers.show(Prompt::Idle);
        return signed;
    }
    let result = android_key(&answers, options, client_data_hash, cancel).await;
    answers.show(Prompt::Idle);
    let credential = result.map_err(|e| match e {
        Failure::Webauthn(e) => explain(e, &answers),
        Failure::Other(e) => e,
    })?;
    Ok(Assertion {
        client_data_json: client_data_json.to_vec(),
        user_handle: user_handle(credential.response.user_handle.as_deref()),
        authenticator_data: credential.response.authenticator_data.into(),
        signature: credential.response.signature.into(),
        credential_id: credential.raw_id.into(),
    })
}

/// A security key held to the phone (NFC) or plugged in (USB). Each step
/// blocks this thread; the key stops being waited on once `cancel` is set.
#[cfg(target_os = "android")]
async fn android_key(
    answers: &Answers,
    options: PublicKeyCredentialRequestOptions,
    hash: &[u8],
    cancel: &AtomicBool,
) -> std::result::Result<webauthn_rs_proto::PublicKeyCredential, Failure> {
    use crate::android::{KeySearch, Nfc};
    use webauthn_authenticator_rs::{ctap2::CtapAuthenticator, transport::Token};
    let search = KeySearch::start()?;
    answers.show(match search.nfc {
        Nfc::On => Prompt::Insert,
        Nfc::Off => Prompt::Phone(
            t!("sync-fido-plug-in-your-security-key-or"),
        ),
        Nfc::Missing => Prompt::Phone(t!("sync-fido-plug-in-your-security-key")),
    });
    let token = search.find(cancel, WAIT)?;
    let button = token.has_button();
    answers.show(Prompt::Working);
    let mut auth = CtapAuthenticator::new(token, answers).await.ok_or_else(|| {
        anyhow!(t!("sync-fido-that-security-key-couldnt-be-used"))
    })?;
    answers.show(if button { Prompt::Touch } else { Prompt::Working });
    let signed = AuthenticatorBackendHashedClientData::perform_auth(
        &mut auth,
        hash.to_vec(),
        options,
        WAIT.as_millis() as u32,
    )?;
    drop(auth);
    drop(search);
    Ok(signed)
}

/// Resolves when the sign-in is cancelled.
#[cfg(not(target_os = "android"))]
async fn cancelled(cancel: &AtomicBool) {
    while !cancel.load(Ordering::Relaxed) {
        tokio::time::sleep(std::time::Duration::from_millis(150)).await;
    }
}

const WAIT: std::time::Duration = std::time::Duration::from_secs(60);

#[cfg(not(any(target_os = "android", target_os = "ios")))]
async fn security_key(
    answers: &Answers,
    options: PublicKeyCredentialRequestOptions,
    hash: &[u8],
    cancel: &AtomicBool,
) -> Signed {
    use futures::StreamExt;
    use webauthn_authenticator_rs::{
        ctap2::CtapAuthenticator,
        transport::{AnyTransport, TokenEvent, Transport},
    };
    let transport = AnyTransport::new().await?;
    let mut tokens = transport.watch().await?;
    let find = async {
        while let Some(event) = tokens.next().await {
            if let TokenEvent::Added(token) = event
                && let Some(auth) = CtapAuthenticator::new(token, answers).await
            {
                return Some(auth);
            }
        }
        None
    };
    let mut auth = tokio::select! {
        found = tokio::time::timeout(WAIT, find) => match found {
            Ok(Some(auth)) => auth,
            Ok(None) | Err(_) => return Err(anyhow!(t!("sync-fido-no-security-key-was-found-plug")).into()),
        },
        () = cancelled(cancel) => return Err(anyhow!(t!("sync-fido-sign-in-cancelled")).into()),
    };
    answers.show(Prompt::Touch);
    let signed = AuthenticatorBackendHashedClientData::perform_auth(
        &mut auth,
        hash.to_vec(),
        options,
        WAIT.as_millis() as u32,
    )?;
    Ok((signed, None))
}

#[cfg(not(target_os = "android"))]
async fn phone(
    answers: &Answers,
    options: PublicKeyCredentialRequestOptions,
    hash: &[u8],
    cancel: &AtomicBool,
) -> Signed {
    use webauthn_authenticator_rs::cable::connect_cable_authenticator;
    let connect = connect_cable_authenticator(CableRequestType::GetAssertion, answers);
    let mut auth = tokio::select! {
        connected = tokio::time::timeout(WAIT * 2, connect) => match connected {
            Ok(auth) => auth?,
            Err(_) => return Err(anyhow!(t!("sync-fido-your-phone-didnt-connect-in-time")).into()),
        },
        () = cancelled(cancel) => return Err(anyhow!(t!("sync-fido-sign-in-cancelled")).into()),
    };
    let signed = AuthenticatorBackendHashedClientData::perform_auth(
        &mut auth,
        hash.to_vec(),
        options,
        WAIT.as_millis() as u32,
    )?;
    Ok((signed, None))
}

/// Windows Hello, on a thread of its own (its call blocks until the dialog
/// closes), so the sign-in can be cancelled. webauthn.dll can cancel its
/// dialog (WebAuthNCancelCurrentOperation, with the id passed in the
/// options' pCancellationId), but webauthn-authenticator-rs 0.6.2 passes
/// no id, so a cancelled sign-in stops waiting at once and leaves the
/// dialog to its own Cancel button or its timeout (`WAIT`); its answer is
/// then dropped unread. The thread owns everything it uses.
#[cfg(windows)]
async fn windows(options: PublicKeyCredentialRequestOptions, cancel: &AtomicBool) -> Signed {
    use webauthn_authenticator_rs::{AuthenticatorBackend, win10::Win10};
    let origin = url::Url::parse(&format!("https://{}", options.rp_id))
        .map_err(|_| anyhow!(t!("sync-fido-proton-named-itself-oddly-for-the")))?;
    let hello = tokio::task::spawn_blocking(move || {
        Win10::default().perform_auth(origin, options, WAIT.as_millis() as u32)
    });
    let signed = tokio::select! {
        done = hello => done.map_err(|e| anyhow!(t!("sync-fido-windows-hello-stopped-unexpectedly", error = e)))??,
        () = cancelled(cancel) => return Err(WebauthnCError::Cancelled.into()),
    };
    let client_data = signed.response.client_data_json.clone();
    Ok((signed, Some(client_data)))
}

/// On iPhones: a security key held to the phone (NFC), or a passkey on
/// another device by QR code (caBLE), as on a computer.
#[cfg(target_os = "ios")]
pub async fn assert(
    method: Method,
    options: PublicKeyCredentialRequestOptions,
    client_data_json: &[u8],
    client_data_hash: &[u8],
    pin: Option<String>,
    prompt: Arc<Mutex<Prompt>>,
    cancel: &AtomicBool,
) -> Result<Assertion> {
    let answers = Answers {
        pin: pin.filter(|p| !p.is_empty()),
        pin_asked: AtomicBool::new(false),
        prompt,
    };
    let result = match method {
        Method::SecurityKey => ios_key(&answers, options, client_data_hash, cancel).await,
        Method::Phone => phone(&answers, options, client_data_hash, cancel).await,
    };
    answers.show(Prompt::Idle);
    let (credential, _) = result.map_err(|e| match e {
        Failure::Webauthn(e) => explain(e, &answers),
        Failure::Other(e) => e,
    })?;
    Ok(Assertion {
        client_data_json: client_data_json.to_vec(),
        user_handle: user_handle(credential.response.user_handle.as_deref()),
        authenticator_data: credential.response.authenticator_data,
        signature: credential.response.signature,
        credential_id: credential.raw_id,
    })
}

/// A security key held to the top of the iPhone, through the system's NFC
/// sheet. Each step blocks this thread; the key stops being waited on once
/// `cancel` is set.
#[cfg(target_os = "ios")]
async fn ios_key(
    answers: &Answers,
    options: PublicKeyCredentialRequestOptions,
    hash: &[u8],
    cancel: &AtomicBool,
) -> Signed {
    use crate::ios::keys::NfcSearch;
    use webauthn_authenticator_rs::ctap2::CtapAuthenticator;
    const HOLD: &str = "Hold your security key near the top of your iPhone.";
    answers.show(Prompt::Insert);
    let search = NfcSearch::start(HOLD)?;
    let token = search.find(cancel, WAIT)?;
    search.say(t!("sync-fido-keep-holding-your-security-key"));
    answers.show(Prompt::Working);
    let Some(mut auth) = CtapAuthenticator::new(token, answers).await else {
        search.finish(Some(t!("sync-fido-that-security-key-couldnt-be-used-3")));
        return Err(anyhow!(
            t!("sync-fido-that-security-key-couldnt-be-used-2")
        )
        .into());
    };
    let signed = AuthenticatorBackendHashedClientData::perform_auth(
        &mut auth,
        hash.to_vec(),
        options,
        WAIT.as_millis() as u32,
    );
    drop(auth);
    search.finish(signed.is_err().then_some(t!("sync-fido-the-security-key-didnt-sign-in")));
    Ok((signed?, None))
}

/// Whether Proton's options name any key at all, and this system can use one.
pub fn usable(options: Option<&Value>) -> bool {
    options.and_then(|o| request_options(o).ok()).is_some()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_protons_options() {
        let options = serde_json::json!({"publicKey": {
            "challenge": [1, 2, 3],
            "timeout": 60000,
            "rpId": "proton.me",
            "allowCredentials": [{"id": [9, 8], "type": "public-key"}],
            "userVerification": "discouraged"
        }});
        let r = request_options(&options).unwrap();
        assert_eq!(r.challenge, [1, 2, 3]);
        assert_eq!(r.rp_id, "proton.me");
        assert_eq!(r.allow_credentials[0].id, [9, 8]);
        assert_eq!(r.timeout, Some(60000));
        assert_eq!(
            r.user_verification,
            UserVerificationPolicy::Discouraged_DO_NOT_USE
        );
        assert!(usable(Some(&options)));
        // Another site's passkeys aren't asked for.
        let elsewhere = serde_json::json!({"publicKey": {
            "challenge": [1], "rpId": "google.com"
        }});
        assert!(request_options(&elsewhere).is_err());
        assert!(proton_rp("account.proton.me"));
        assert!(!proton_rp("evilproton.me"));
        assert!(!proton_rp("proton.me.evil.com"));
        assert!(!proton_rp("victim.example/.proton.me"));
        assert!(!proton_rp("victim.example:443/x.proton.me"));
        assert!(!usable(Some(&serde_json::json!({}))));
        assert!(!usable(None));
    }

    /// WebAuthn §5.2.2 and §5.4.3: a user handle is kept where given, at
    /// most 64 bytes, and none is none.
    #[test]
    fn user_handles_are_kept_where_given() {
        assert_eq!(user_handle(Some(b"user-1")), Some(b"user-1".to_vec()));
        assert_eq!(user_handle(Some(b"")), None);
        assert_eq!(user_handle(None), None);
        assert_eq!(user_handle(Some(&[7; 65])), None);
        // A cancelled sign-in says so, rather than that the key failed.
        let answers = Answers { pin: None, pin_asked: AtomicBool::new(false), prompt: Default::default() };
        let e = explain(WebauthnCError::Cancelled, &answers);
        assert_eq!(e.to_string(), "Using the security key was cancelled.");
    }

    #[test]
    fn a_pin_the_key_asked_for_is_asked_for() {
        let answers = Answers {
            pin: None,
            pin_asked: AtomicBool::new(true),
            prompt: Default::default(),
        };
        let e = explain(WebauthnCError::Cancelled, &answers);
        assert!(e.downcast_ref::<NeedsPin>().is_some());
        let e = explain(
            WebauthnCError::Ctap(webauthn_authenticator_rs::error::CtapError::Ctap2PinInvalid),
            &answers,
        );
        assert!(e.downcast_ref::<NeedsPin>().is_some());
    }
}
