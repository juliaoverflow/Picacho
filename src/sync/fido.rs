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
//! USB keys and phones sign exactly the client data Proton's library built
//! (its hash goes to the authenticator); Windows Hello builds its own, which
//! is sent instead. While one waits, `Prompt` says what for (touch the key,
//! scan the QR code), and the window shows it.
use anyhow::{Result, anyhow};
use serde_json::Value;
#[cfg(not(target_os = "ios"))]
use std::sync::atomic::Ordering;
use std::sync::{Arc, Mutex, atomic::AtomicBool};
#[cfg(not(target_os = "ios"))]
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
    /// The ways this platform offers, in order.
    pub fn offered() -> &'static [Self] {
        #[cfg(windows)]
        return &[Self::Windows];
        #[cfg(not(windows))]
        &[Self::SecurityKey, Self::Phone]
    }
    pub fn label(self) -> &'static str {
        match self {
            Self::SecurityKey => "Use a security key",
            Self::Phone if cfg!(target_os = "android") => "Use a passkey",
            Self::Phone => "Use a passkey on your phone",
            #[cfg(windows)]
            Self::Windows => "Use a passkey or security key",
        }
    }
}

/// What an authenticator is waiting for, for the window to show.
#[cfg_attr(target_os = "ios", allow(dead_code))]
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
}

/// The authenticator library's questions, answered from the sign-in.
#[cfg(not(target_os = "ios"))]
#[derive(Debug)]
struct Answers {
    pin: Option<String>,
    pin_asked: AtomicBool,
    prompt: Arc<Mutex<Prompt>>,
}
#[cfg(not(target_os = "ios"))]
impl Answers {
    fn show(&self, p: Prompt) {
        *self.prompt.lock().unwrap() = p;
    }
}
#[cfg(not(target_os = "ios"))]
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
        self.show(Prompt::Phone("Connecting to your phone…"));
    }
    fn cable_status_update(&self, state: CableState) {
        self.show(Prompt::Phone(match state {
            CableState::ConnectingToTunnelServer => "Connecting to your phone…",
            CableState::WaitingForAuthenticatorResponse => "Confirm the sign-in on your phone.",
            _ => "Talking to your phone…",
        }));
    }
}

fn bytes(v: Option<&Value>) -> Option<Vec<u8>> {
    v?.as_array()?
        .iter()
        .map(|x| x.as_u64().and_then(|n| u8::try_from(n).ok()))
        .collect()
}

/// Proton's `AuthenticationOptions` (bytes as arrays of numbers) as a
/// WebAuthn request.
pub fn request_options(options: &Value) -> Result<PublicKeyCredentialRequestOptions> {
    let pk = options
        .get("publicKey")
        .ok_or_else(|| anyhow!("Proton didn't say which security keys to use."))?;
    let challenge =
        bytes(pk.get("challenge")).ok_or_else(|| anyhow!("Proton sent no challenge."))?;
    let rp_id = pk
        .get("rpId")
        .and_then(Value::as_str)
        .ok_or_else(|| anyhow!("Proton didn't name itself for the key."))?
        .to_owned();
    let allow_credentials = pk
        .get("allowCredentials")
        .and_then(Value::as_array)
        .map(|list| {
            list.iter()
                .filter_map(|c| {
                    Some(AllowCredentials {
                        type_: "public-key".into(),
                        id: bytes(c.get("id"))?,
                        transports: None,
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
        extensions: None,
    })
}

/// The WebAuthn error, said for a person; a PIN problem asks for the PIN.
#[cfg(not(target_os = "ios"))]
fn explain(e: WebauthnCError, answers: &Answers) -> anyhow::Error {
    use webauthn_authenticator_rs::error::CtapError::*;
    match e {
        WebauthnCError::Ctap(Ctap2PinInvalid | Ctap2PinAuthInvalid) => anyhow::Error::new(
            NeedsPin("That PIN isn't right. Enter your security key's PIN."),
        ),
        WebauthnCError::Ctap(Ctap2PinBlocked | Ctap2PinAuthBlocked) => anyhow!(
            "This security key is locked after too many wrong PINs. Unplug it and plug it in again, or reset it."
        ),
        _ if answers.pin_asked.load(Ordering::Relaxed) && answers.pin.is_none() => {
            anyhow::Error::new(NeedsPin(
                "This security key has a PIN. Enter it to use the key.",
            ))
        }
        WebauthnCError::PinTooShort | WebauthnCError::PinTooLong => anyhow::Error::new(NeedsPin(
            "That PIN isn't the right length. Enter your security key's PIN.",
        )),
        WebauthnCError::Ctap(Ctap1Timeout | Ctap2UserActionTimeout | Ctap2ActionTimeout) => {
            anyhow!("The security key wasn't touched in time. Try again.")
        }
        WebauthnCError::Cancelled | WebauthnCError::Ctap(Ctap2KeepAliveCancel) => {
            anyhow!("Using the security key was cancelled.")
        }
        WebauthnCError::PermissionDenied => anyhow!(
            "Picacho isn't allowed to use Bluetooth or USB security keys here. Allow it in the system settings and try again."
        ),
        WebauthnCError::NoBluetoothAdapter => anyhow!(
            "A passkey on a phone needs Bluetooth on this computer, to know the phone is nearby. Turn Bluetooth on, or use a security key."
        ),
        WebauthnCError::Ctap(Ctap2NoCredentials) => anyhow!(
            "That key or passkey isn't registered with this Proton account. Use another, or the code from your authenticator app."
        ),
        WebauthnCError::ApduTransmission => anyhow!(
            "The connection to the security key was lost. Keep it in place until it's done, and try again."
        ),
        other => anyhow!("The security key or passkey didn't work ({other:?})."),
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
        #[cfg(windows)]
        Method::Windows => windows(options),
    };
    answers.show(Prompt::Idle);
    let (credential, own_client_data) = result.map_err(|e| match e {
        Failure::Webauthn(e) => explain(e, &answers),
        Failure::Other(e) => e,
    })?;
    Ok(Assertion {
        client_data_json: own_client_data.unwrap_or_else(|| client_data_json.to_vec()),
        authenticator_data: credential.response.authenticator_data,
        signature: credential.response.signature,
        credential_id: credential.raw_id,
    })
}

#[cfg(not(target_os = "ios"))]
enum Failure {
    Webauthn(WebauthnCError),
    Other(anyhow::Error),
}
#[cfg(not(target_os = "ios"))]
impl From<WebauthnCError> for Failure {
    fn from(e: WebauthnCError) -> Self {
        Self::Webauthn(e)
    }
}
#[cfg(not(target_os = "ios"))]
impl From<anyhow::Error> for Failure {
    fn from(e: anyhow::Error) -> Self {
        Self::Other(e)
    }
}
/// The signed credential, and the client data when the authenticator made
/// its own (Windows Hello).
#[cfg(not(any(target_os = "android", target_os = "ios")))]
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
            "Plug in your security key, or turn on NFC to hold it against your phone…",
        ),
        Nfc::Missing => Prompt::Phone("Plug in your security key…"),
    });
    let token = search.find(cancel, WAIT)?;
    let button = token.has_button();
    answers.show(Prompt::Working);
    let mut auth = CtapAuthenticator::new(token, answers).await.ok_or_else(|| {
        anyhow!("That security key couldn't be used. Keep it in place until it's done, or try another.")
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
#[cfg(not(any(target_os = "android", target_os = "ios")))]
async fn cancelled(cancel: &AtomicBool) {
    while !cancel.load(Ordering::Relaxed) {
        tokio::time::sleep(std::time::Duration::from_millis(150)).await;
    }
}

#[cfg(not(target_os = "ios"))]
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
            Ok(None) | Err(_) => return Err(anyhow!("No security key was found. Plug one in and try again.").into()),
        },
        () = cancelled(cancel) => return Err(anyhow!("Sign-in cancelled.").into()),
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

#[cfg(not(any(target_os = "android", target_os = "ios")))]
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
            Err(_) => return Err(anyhow!("Your phone didn't connect in time. Try again, with Bluetooth on on both devices.").into()),
        },
        () = cancelled(cancel) => return Err(anyhow!("Sign-in cancelled.").into()),
    };
    let signed = AuthenticatorBackendHashedClientData::perform_auth(
        &mut auth,
        hash.to_vec(),
        options,
        WAIT.as_millis() as u32,
    )?;
    Ok((signed, None))
}

#[cfg(windows)]
fn windows(options: PublicKeyCredentialRequestOptions) -> Signed {
    use webauthn_authenticator_rs::{AuthenticatorBackend, win10::Win10};
    let origin = url::Url::parse(&format!("https://{}", options.rp_id))
        .map_err(|_| anyhow!("Proton named itself oddly for the key."))?;
    let signed = Win10::default().perform_auth(origin, options, WAIT.as_millis() as u32)?;
    let client_data = signed.response.client_data_json.clone();
    Ok((signed, Some(client_data)))
}

/// iPhones have no security key or passkey support here yet.
#[cfg(target_os = "ios")]
pub async fn assert(
    _method: Method,
    _options: PublicKeyCredentialRequestOptions,
    _client_data_json: &[u8],
    _client_data_hash: &[u8],
    _pin: Option<String>,
    _prompt: Arc<Mutex<Prompt>>,
    _cancel: &AtomicBool,
) -> Result<Assertion> {
    Err(anyhow!("Security keys and passkeys aren't available on iPhones yet."))
}

/// Whether Proton's options name any key at all, and this system can use one.
pub fn usable(options: Option<&Value>) -> bool {
    !cfg!(target_os = "ios")
        && options.and_then(|o| request_options(o).ok()).is_some()
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
        assert!(!usable(Some(&serde_json::json!({}))));
        assert!(!usable(None));
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
