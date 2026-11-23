//! Security keys and passkeys on Android, for Proton's second factor
//! (`sync::fido`).
//!
//! Security keys: the Java side (`SecurityKeys`) watches for one held to
//! the phone (NFC reader mode) or plugged in (USB host), and moves bytes;
//! CTAP2 is the authenticator library's, over the two `Token`s here: ISO
//! 7816 APDUs over NFC (CTAP 2.1 §11.3), CTAPHID reports over USB (§11.2).
//!
//! Passkeys: Android's Credential Manager (Android 14 and later), which
//! asks the person's password manager. Picacho calls it as a browser does,
//! for `https://proton.me` with Proton's client data; a password manager
//! may decline apps it doesn't know as browsers.

use super::{
    call_java,
    requests::{Request, Waited},
    with_java,
};
use crate::sync::fido::Assertion;
use anyhow::{Result, anyhow, bail};
use async_trait::async_trait;
use base64::{
    Engine,
    engine::general_purpose::{URL_SAFE, URL_SAFE_NO_PAD},
};
use jni::{JValue, jni_sig, jni_str, objects::JByteArray};
use serde_json::{Value, json};
use std::{
    sync::atomic::{AtomicBool, Ordering},
    time::{Duration, Instant},
};
use webauthn_authenticator_rs::{
    error::{CtapError, WebauthnCError},
    transport::{Token, iso7816::ISO7816ResponseAPDU},
    ui::UiCallback,
};
use webauthn_rs_proto::{
    AuthenticatorTransport, PublicKeyCredentialRequestOptions, UserVerificationPolicy,
};

/// Whether the phone can reach a key by NFC.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Nfc {
    /// The phone has no NFC.
    Missing,
    /// NFC is turned off.
    Off,
    On,
}

/// Looking for a security key, until dropped: NFC reader mode is on and
/// plugged-in keys are looked for (asking permission for one).
pub struct KeySearch {
    pub nfc: Nfc,
}

impl KeySearch {
    pub fn start() -> Result<Self> {
        let state = call_java("SecurityKeys.start", |env, java| {
            env.call_static_method(
                &java.keys,
                jni_str!("start"),
                jni_sig!("(Landroid/content/Context;)I"),
                &[JValue::Object(java.context.as_obj())],
            )?
            .i()
        })
        .ok_or_else(|| anyhow!("Security keys can't be used on this phone."))?;
        Ok(Self {
            nfc: match state {
                2 => Nfc::On,
                1 => Nfc::Off,
                _ => Nfc::Missing,
            },
        })
    }

    /// Waits up to `wait` for a key to be held to the phone or plugged in
    /// (and allowed), or until `cancel`.
    pub fn find<'c>(&self, cancel: &'c AtomicBool, wait: Duration) -> Result<KeyToken<'c>> {
        let deadline = Instant::now() + wait;
        loop {
            if cancel.load(Ordering::Relaxed) {
                bail!("Sign-in cancelled.");
            }
            if Instant::now() >= deadline {
                bail!(if self.nfc == Nfc::On {
                    "No security key was found. Hold it flat against the back of your phone, or plug it in, and try again."
                } else {
                    "No security key was found. Plug it in and try again."
                });
            }
            let found = call_java("SecurityKeys.poll", |env, java| {
                env.call_static_method(&java.keys, jni_str!("poll"), jni_sig!("()I"), &[])?
                    .i()
            });
            match found {
                Some(1) => return Ok(KeyToken::new(Link::Usb, cancel)),
                Some(2) => return Ok(KeyToken::new(Link::Nfc, cancel)),
                Some(3) => bail!(
                    "Picacho wasn't allowed to use the security key. Plug it in again, and allow Picacho to use it."
                ),
                _ => std::thread::sleep(Duration::from_millis(150)),
            }
        }
    }
}

impl Drop for KeySearch {
    fn drop(&mut self) {
        with_java("SecurityKeys.stop", |env, java| {
            env.call_static_method(&java.keys, jni_str!("stop"), jni_sig!("()V"), &[])
                .map(drop)
        });
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Link {
    Usb,
    Nfc,
}

/// The key `KeySearch` found, for the authenticator library. It stops
/// waiting on the key once the sign-in is cancelled.
#[derive(Debug)]
pub struct KeyToken<'c> {
    link: Link,
    cancel: &'c AtomicBool,
    /// The CTAPHID channel the key gave us (USB).
    channel: [u8; 4],
}

/// The CTAPHID broadcast channel, for asking for one of our own.
const BROADCAST: [u8; 4] = [0xff; 4];
const HID_INIT: u8 = 0x86;
const HID_CBOR: u8 = 0x90;
const HID_CANCEL: u8 = 0x91;
const HID_KEEPALIVE: u8 = 0xbb;
const HID_ERROR: u8 = 0xbf;
/// A FIDO HID report's size; the payload of its first packet and the rest.
const REPORT: usize = 64;
const FIRST: usize = REPORT - 7;
const NEXT: usize = REPORT - 5;
/// How long a key may take to answer, touch included.
const ANSWER: Duration = Duration::from_secs(60);

/// The FIDO applet (CTAP 2.1 §11.3.3), and what selecting it answers.
const APPLET: [u8; 8] = [0xa0, 0x00, 0x00, 0x06, 0x47, 0x2f, 0x00, 0x01];
const FIDO_2_0: &[u8] = b"FIDO_2_0";
const U2F_V2: &[u8] = b"U2F_V2";

fn lost() -> WebauthnCError {
    WebauthnCError::ApduTransmission
}

impl<'c> KeyToken<'c> {
    fn new(link: Link, cancel: &'c AtomicBool) -> Self {
        Self {
            link,
            cancel,
            channel: BROADCAST,
        }
    }

    fn cancelled(&self) -> bool {
        self.cancel.load(Ordering::Relaxed)
    }

    // USB.

    fn usb_write(report: &[u8; REPORT]) -> Result<(), WebauthnCError> {
        let written = call_java("SecurityKeys.usbWrite", |env, java| {
            let bytes = env.byte_array_from_slice(report)?;
            env.call_static_method(
                &java.keys,
                jni_str!("usbWrite"),
                jni_sig!("([B)Z"),
                &[JValue::Object(&bytes)],
            )?
            .z()
        });
        if written == Some(true) {
            Ok(())
        } else {
            Err(lost())
        }
    }

    /// A report, or `None` if none came within `timeout_ms`.
    fn usb_read(timeout_ms: i32) -> Result<Option<[u8; REPORT]>, WebauthnCError> {
        // Java answers null when nothing came, an empty array when the key
        // is gone.
        let read = call_java("SecurityKeys.usbRead", |env, java| {
            let read = env
                .call_static_method(
                    &java.keys,
                    jni_str!("usbRead"),
                    jni_sig!("(I)[B"),
                    &[JValue::Int(timeout_ms)],
                )?
                .l()?;
            if read.is_null() {
                return Ok(None);
            }
            // Safety: `usbRead` returns a byte[].
            let read = unsafe { JByteArray::from_raw(env, read.into_raw()) };
            env.convert_byte_array(&read).map(Some)
        })
        .ok_or_else(lost)?;
        match read {
            None => Ok(None),
            Some(bytes) if bytes.len() >= REPORT => {
                let mut report = [0; REPORT];
                report.copy_from_slice(&bytes[..REPORT]);
                Ok(Some(report))
            }
            Some(_) => Err(lost()),
        }
    }

    /// Sends a CTAPHID message, in as many reports as it takes.
    fn usb_send(&self, channel: [u8; 4], command: u8, data: &[u8]) -> Result<(), WebauthnCError> {
        if data.len() > FIRST + 128 * NEXT {
            return Err(WebauthnCError::MessageTooLarge);
        }
        let mut report = [0; REPORT];
        report[..4].copy_from_slice(&channel);
        report[4] = command;
        report[5..7].copy_from_slice(&(data.len() as u16).to_be_bytes());
        let first = data.len().min(FIRST);
        report[7..7 + first].copy_from_slice(&data[..first]);
        Self::usb_write(&report)?;
        for (sequence, chunk) in data[first..].chunks(NEXT).enumerate() {
            let mut report = [0; REPORT];
            report[..4].copy_from_slice(&channel);
            report[4] = sequence as u8;
            report[5..5 + chunk.len()].copy_from_slice(chunk);
            Self::usb_write(&report)?;
        }
        Ok(())
    }

    /// The next report on `channel`, waiting until `deadline`.
    fn usb_report(&self, channel: [u8; 4], deadline: Instant) -> Result<[u8; REPORT], WebauthnCError> {
        loop {
            if self.cancelled() {
                // The key stops waiting for its touch.
                let _ = self.usb_send(channel, HID_CANCEL, &[]);
                return Err(WebauthnCError::Cancelled);
            }
            if Instant::now() >= deadline {
                return Err(WebauthnCError::Ctap(CtapError::Ctap2UserActionTimeout));
            }
            if let Some(report) = Self::usb_read(200)?
                && report[..4] == channel
            {
                return Ok(report);
            }
        }
    }

    /// The next whole CTAPHID message on `channel`: its command and data.
    fn usb_receive(&self, channel: [u8; 4], deadline: Instant) -> Result<(u8, Vec<u8>), WebauthnCError> {
        let first = loop {
            let report = self.usb_report(channel, deadline)?;
            // A continuation without its start is left over from before.
            if report[4] & 0x80 != 0 {
                break report;
            }
        };
        let command = first[4];
        let length = usize::from(u16::from_be_bytes([first[5], first[6]]));
        // CTAPHID carries at most one first packet and 128 continuations
        // (sequence 0 to 127): a longer length is a broken (or hostile)
        // device's, whose sequence count would otherwise overflow.
        if length > FIRST + 128 * NEXT {
            return Err(lost());
        }
        let mut data = first[7..7 + length.min(FIRST)].to_vec();
        let mut sequence = 0u8;
        while data.len() < length {
            let report = self.usb_report(channel, deadline)?;
            if report[4] != sequence {
                return Err(lost());
            }
            let take = (length - data.len()).min(NEXT);
            data.extend_from_slice(&report[5..5 + take]);
            sequence = sequence.checked_add(1).ok_or_else(lost)?;
        }
        Ok((command, data))
    }

    fn usb_init(&mut self) -> Result<(), WebauthnCError> {
        let nonce: [u8; 8] = rand::random();
        self.usb_send(BROADCAST, HID_INIT, &nonce)?;
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            let (command, data) = self.usb_receive(BROADCAST, deadline)?;
            // The answer: the nonce, then the channel the key gave us.
            if command == HID_INIT && data.len() >= 17 && data[..8] == nonce {
                self.channel.copy_from_slice(&data[8..12]);
                return Ok(());
            }
        }
    }

    fn usb_transmit<U: UiCallback>(&self, cbor: &[u8], ui: &U) -> Result<Vec<u8>, WebauthnCError> {
        self.usb_send(self.channel, HID_CBOR, cbor)?;
        let deadline = Instant::now() + ANSWER;
        loop {
            let (command, data) = self.usb_receive(self.channel, deadline)?;
            match command {
                HID_CBOR => return ctap_answer(data),
                // Every 100 ms while the key works; 2 is waiting for a touch.
                HID_KEEPALIVE if data.first() == Some(&2) => ui.request_touch(),
                HID_KEEPALIVE => ui.processing(),
                HID_ERROR => return Err(lost()),
                _ => {}
            }
        }
    }

    // NFC.

    /// Sends an APDU to the key held to the phone, and reads its answer.
    fn nfc_exchange(&self, apdu: &[u8]) -> Result<ISO7816ResponseAPDU, WebauthnCError> {
        let answer = call_java("SecurityKeys.nfcTransceive", |env, java| {
            let bytes = env.byte_array_from_slice(apdu)?;
            let answer = env
                .call_static_method(
                    &java.keys,
                    jni_str!("nfcTransceive"),
                    jni_sig!("([B)[B"),
                    &[JValue::Object(&bytes)],
                )?
                .l()?;
            if answer.is_null() {
                return Ok(None);
            }
            // Safety: `nfcTransceive` returns a byte[].
            let answer = unsafe { JByteArray::from_raw(env, answer.into_raw()) };
            env.convert_byte_array(&answer).map(Some)
        })
        .flatten()
        .ok_or_else(lost)?;
        ISO7816ResponseAPDU::try_from(answer.as_slice()).map_err(|_| lost())
    }

    fn nfc_init(&self) -> Result<(), WebauthnCError> {
        let mut select = vec![0x00, 0xa4, 0x04, 0x00, APPLET.len() as u8];
        select.extend_from_slice(&APPLET);
        select.push(0x00);
        let answer = self.nfc_exchange(&select)?;
        if !answer.is_ok() || (answer.data != FIDO_2_0 && answer.data != U2F_V2) {
            return Err(WebauthnCError::NotSupported);
        }
        Ok(())
    }

    fn nfc_transmit<U: UiCallback>(&self, cbor: &[u8], ui: &U) -> Result<Vec<u8>, WebauthnCError> {
        // Chained short APDUs: every part but the last says more follows.
        let parts: Vec<&[u8]> = cbor.chunks(255).collect();
        let mut answer = None;
        for (i, part) in parts.iter().enumerate() {
            let last = i + 1 == parts.len();
            let mut apdu = vec![if last { 0x80 } else { 0x90 }, 0x10, 0x00, 0x00];
            apdu.push(part.len() as u8);
            apdu.extend_from_slice(part);
            if last {
                apdu.push(0x00);
            }
            let a = self.nfc_exchange(&apdu)?;
            if !last && !a.is_ok() {
                return Err(lost());
            }
            answer = Some(a);
        }
        let mut answer = answer.ok_or(WebauthnCError::MessageTooShort)?;
        let mut data = Vec::new();
        let deadline = Instant::now() + ANSWER;
        loop {
            if self.cancelled() {
                return Err(WebauthnCError::Cancelled);
            }
            if Instant::now() >= deadline {
                return Err(WebauthnCError::Ctap(CtapError::Ctap2UserActionTimeout));
            }
            match (answer.sw1, answer.sw2) {
                (0x90, 0x00) => {
                    data.extend_from_slice(&answer.data);
                    return ctap_answer(data);
                }
                // More of the answer waits: GET RESPONSE.
                (0x61, more) => {
                    data.extend_from_slice(&answer.data);
                    // A key's answer is a few kilobytes at most: one that
                    // keeps saying "more" doesn't fill memory until the time
                    // is up.
                    if data.len() > 64 * 1024 {
                        return Err(WebauthnCError::Ctap(CtapError::Ctap1InvalidLength));
                    }
                    answer = self.nfc_exchange(&[0x00, 0xc0, 0x00, 0x00, more])?;
                }
                // Still working: ask again (NFCCTAP_GETRESPONSE).
                (0x91, 0x00) => {
                    ui.processing();
                    std::thread::sleep(Duration::from_millis(100));
                    answer = self.nfc_exchange(&[0x80, 0x11, 0x00, 0x00, 0x00])?;
                }
                _ => return Err(lost()),
            }
        }
    }
}

/// A CTAP2 answer: its status byte, then the CBOR.
fn ctap_answer(mut data: Vec<u8>) -> Result<Vec<u8>, WebauthnCError> {
    if data.is_empty() {
        return Err(WebauthnCError::MessageTooShort);
    }
    let status = CtapError::from(data.remove(0));
    if !status.is_ok() {
        return Err(WebauthnCError::Ctap(status));
    }
    Ok(data)
}

#[async_trait]
impl Token for KeyToken<'_> {
    type Id = ();

    fn has_button(&self) -> bool {
        // Holding a key to the phone is its touch.
        self.link == Link::Usb
    }

    fn get_transport(&self) -> AuthenticatorTransport {
        match self.link {
            Link::Usb => AuthenticatorTransport::Usb,
            Link::Nfc => AuthenticatorTransport::Nfc,
        }
    }

    async fn transmit_raw<U>(&mut self, cbor: &[u8], ui: &U) -> Result<Vec<u8>, WebauthnCError>
    where
        U: UiCallback,
    {
        match self.link {
            Link::Usb => self.usb_transmit(cbor, ui),
            Link::Nfc => self.nfc_transmit(cbor, ui),
        }
    }

    async fn cancel(&mut self) -> Result<(), WebauthnCError> {
        if self.link == Link::Usb {
            self.usb_send(self.channel, HID_CANCEL, &[])?;
        }
        Ok(())
    }

    async fn init(&mut self) -> Result<(), WebauthnCError> {
        match self.link {
            Link::Usb => self.usb_init(),
            Link::Nfc => self.nfc_init(),
        }
    }

    async fn close(&mut self) -> Result<(), WebauthnCError> {
        Ok(())
    }
}

/// Signs Proton's challenge with a passkey, through Android's Credential
/// Manager: the person picks one from their password manager (or another
/// device, or a security key, where it offers those). `client_data_json` is
/// what Proton's library made; its hash is what gets signed.
pub fn passkey(
    options: &PublicKeyCredentialRequestOptions,
    client_data_json: &[u8],
    client_data_hash: &[u8],
    cancel: &AtomicBool,
) -> Result<Assertion> {
    let b64 = |bytes: &[u8]| URL_SAFE_NO_PAD.encode(bytes);
    let challenge: &[u8] = options.challenge.as_ref();
    let request = json!({
        "challenge": b64(challenge),
        "rpId": options.rp_id,
        "timeout": options.timeout.unwrap_or(60_000),
        "userVerification": match options.user_verification {
            UserVerificationPolicy::Required => "required",
            UserVerificationPolicy::Discouraged_DO_NOT_USE => "discouraged",
            _ => "preferred",
        },
        "allowCredentials": options.allow_credentials.iter().map(|c| json!({
            "type": "public-key",
            "id": b64(c.id.as_ref()),
        })).collect::<Vec<_>>(),
    })
    .to_string();
    // Proton's library names the relying party's own site as the origin.
    let origin = format!("https://{}", options.rp_id);

    let pending = Request::new();
    let id = pending.id();
    let started = call_java("Passkeys.get", |env, java| {
        let request = env.new_string(&request)?;
        let origin = env.new_string(&origin)?;
        let hash = env.byte_array_from_slice(client_data_hash)?;
        env.call_static_method(
            &java.passkeys,
            jni_str!("get"),
            jni_sig!("(ILjava/lang/String;Ljava/lang/String;[B)V"),
            &[
                JValue::Int(id),
                JValue::Object(&request),
                JValue::Object(&origin),
                JValue::Object(&hash),
            ],
        )
        .map(drop)
    });
    if started.is_none() {
        bail!("Passkeys can't be used on this phone.");
    }
    let answer = match pending.next(cancel, Instant::now() + Duration::from_secs(5 * 60)) {
        Waited::Answer(answer) => answer.unwrap_or_default(),
        Waited::Cancelled | Waited::TimedOut => {
            with_java("Passkeys.cancel", |env, java| {
                env.call_static_method(
                    &java.passkeys,
                    jni_str!("cancel"),
                    jni_sig!("(I)V"),
                    &[JValue::Int(id)],
                )
                .map(drop)
            });
            bail!("Sign-in cancelled.");
        }
    };
    let (kind, detail) = answer.split_once(':').unwrap_or((answer.as_str(), ""));
    match kind {
        "ok" => assertion(detail, client_data_json, challenge),
        "cancelled" => bail!("Using a passkey was cancelled."),
        "none" => bail!(
            "No passkey for this Proton account was found. Use another way, or the code from your authenticator app."
        ),
        "unsupported" => bail!(
            "Passkeys need Android 14 or later here. Use a security key, or the code from your authenticator app."
        ),
        "refused" => bail!(
            "Your password manager didn't let Picacho use your Proton passkey: it only lets browsers it knows ask for another site's passkeys. Use a security key, or the code from your authenticator app."
        ),
        _ if detail.is_empty() => bail!("The passkey didn't work."),
        _ => bail!("The passkey didn't work ({detail})."),
    }
}

/// What the passkey signed, from Credential Manager's WebAuthn JSON.
fn assertion(response: &str, client_data_json: &[u8], challenge: &[u8]) -> Result<Assertion> {
    let response: Value =
        serde_json::from_str(response).map_err(|_| anyhow!("The passkey's answer couldn't be read."))?;
    let bytes = |v: &Value| {
        let text = v.as_str()?;
        URL_SAFE_NO_PAD
            .decode(text.trim_end_matches('='))
            .or_else(|_| URL_SAFE.decode(text))
            .ok()
    };
    let signed = &response["response"];
    let (Some(credential_id), Some(authenticator_data), Some(signature)) = (
        bytes(&response["rawId"]).or_else(|| bytes(&response["id"])),
        bytes(&signed["authenticatorData"]),
        bytes(&signed["signature"]),
    ) else {
        bail!("The passkey's answer was incomplete.");
    };
    // Given Proton's hash, the password manager signs it and may answer
    // with placeholder client data; one that made its own (for this
    // challenge) signed that instead.
    let own = bytes(&signed["clientDataJSON"]).filter(|own| {
        serde_json::from_slice::<Value>(own).is_ok_and(|v| {
            v["type"] == "webauthn.get"
                && v["challenge"].as_str() == Some(URL_SAFE_NO_PAD.encode(challenge).as_str())
                && v["origin"].as_str().is_some_and(|o| !o.is_empty())
        })
    });
    Ok(Assertion {
        client_data_json: own.unwrap_or_else(|| client_data_json.to_vec()),
        authenticator_data,
        signature,
        credential_id,
        user_handle: bytes(&signed["userHandle"]).filter(|h| !h.is_empty() && h.len() <= 64),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_a_passkey_answer() {
        let challenge = [1u8, 2, 3];
        let b64 = |b: &[u8]| URL_SAFE_NO_PAD.encode(b);
        let proton = br#"{"challenge":"AQID","origin":"https://proton.me","type":"webauthn.get"}"#;
        let answer = json!({
            "id": b64(&[9, 9]),
            "rawId": b64(&[9, 8]),
            "type": "public-key",
            "response": {
                "clientDataJSON": b64(b"{}"),
                "authenticatorData": b64(&[5]),
                "signature": b64(&[6]),
            }
        });
        let a = assertion(&answer.to_string(), proton, &challenge).unwrap();
        assert_eq!(a.credential_id, [9, 8]);
        assert_eq!(a.authenticator_data, [5]);
        assert_eq!(a.signature, [6]);
        // A placeholder: Proton's client data is what was signed.
        assert_eq!(a.client_data_json, proton);
    }
}
