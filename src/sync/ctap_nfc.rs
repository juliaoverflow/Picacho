//! CTAP2 over NFC (CTAP 2.1 §11.3): ISO 7816 APDUs to a security key held
//! to a phone, for Proton's second factor. The platform moves the bytes
//! (Android's `SecurityKeys`, iOS's Core NFC session); this frames them, for
//! each platform's `Token` (`android::keys`, `ios::keys`).
use std::time::{Duration, Instant};
use webauthn_authenticator_rs::{
    error::{CtapError, WebauthnCError},
    transport::iso7816::ISO7816ResponseAPDU,
    ui::UiCallback,
};

/// A key held to the phone, as the platform reaches it.
pub trait Link {
    /// Sends one APDU and reads the key's answer.
    fn exchange(&self, apdu: &[u8]) -> Result<ISO7816ResponseAPDU, WebauthnCError>;
    /// Whether the sign-in was cancelled: the key stops being waited on.
    fn cancelled(&self) -> bool;
}

/// How long a key may take to answer.
pub const ANSWER: Duration = Duration::from_secs(60);
/// The longest answer read: a key's is a few kilobytes at most, and one that
/// keeps saying "more" doesn't fill memory until the time is up.
const MOST: usize = 64 * 1024;

/// The FIDO applet (CTAP 2.1 §11.3.3), and what selecting it answers.
pub const APPLET: [u8; 8] = [0xa0, 0x00, 0x00, 0x06, 0x47, 0x2f, 0x00, 0x01];
const FIDO_2_0: &[u8] = b"FIDO_2_0";
const U2F_V2: &[u8] = b"U2F_V2";

/// The connection to the key was lost (or it answered nonsense).
pub fn lost() -> WebauthnCError {
    WebauthnCError::ApduTransmission
}

/// Selects the FIDO applet; a key without one isn't a security key.
pub fn select(link: &impl Link) -> Result<(), WebauthnCError> {
    let mut select = vec![0x00, 0xa4, 0x04, 0x00, APPLET.len() as u8];
    select.extend_from_slice(&APPLET);
    select.push(0x00);
    let answer = link.exchange(&select)?;
    if !answer.is_ok() || (answer.data != FIDO_2_0 && answer.data != U2F_V2) {
        return Err(WebauthnCError::NotSupported);
    }
    Ok(())
}

/// Sends a CTAP2 command (NFCCTAP_MSG) and reads the whole answer.
pub fn transmit<U: UiCallback>(
    link: &impl Link,
    cbor: &[u8],
    ui: &U,
) -> Result<Vec<u8>, WebauthnCError> {
    transmit_with(link, cbor, ui, Duration::from_millis(100))
}

fn transmit_with<U: UiCallback>(
    link: &impl Link,
    cbor: &[u8],
    ui: &U,
    pause: Duration,
) -> Result<Vec<u8>, WebauthnCError> {
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
        let a = link.exchange(&apdu)?;
        if !last && !a.is_ok() {
            return Err(lost());
        }
        answer = Some(a);
    }
    let mut answer = answer.ok_or(WebauthnCError::MessageTooShort)?;
    let mut data = Vec::new();
    let deadline = Instant::now() + ANSWER;
    loop {
        if link.cancelled() {
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
                if data.len() > MOST {
                    return Err(WebauthnCError::Ctap(CtapError::Ctap1InvalidLength));
                }
                answer = link.exchange(&[0x00, 0xc0, 0x00, 0x00, more])?;
            }
            // Still working: ask again (NFCCTAP_GETRESPONSE).
            (0x91, 0x00) => {
                ui.processing();
                std::thread::sleep(pause);
                answer = link.exchange(&[0x80, 0x11, 0x00, 0x00, 0x00])?;
            }
            _ => return Err(lost()),
        }
    }
}

/// A CTAP2 answer: its status byte, then the CBOR.
pub fn ctap_answer(mut data: Vec<u8>) -> Result<Vec<u8>, WebauthnCError> {
    if data.is_empty() {
        return Err(WebauthnCError::MessageTooShort);
    }
    let status = CtapError::from(data.remove(0));
    if !status.is_ok() {
        return Err(WebauthnCError::Ctap(status));
    }
    Ok(data)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{cell::RefCell, collections::VecDeque};
    use webauthn_authenticator_rs::types::{CableRequestType, CableState, EnrollSampleStatus};

    /// A key that answers from a script, noting what it was sent.
    struct Scripted {
        answers: RefCell<VecDeque<Vec<u8>>>,
        sent: RefCell<Vec<Vec<u8>>>,
    }
    impl Scripted {
        fn new(answers: &[&[u8]]) -> Self {
            Self {
                answers: RefCell::new(answers.iter().map(|a| a.to_vec()).collect()),
                sent: RefCell::default(),
            }
        }
    }
    impl Link for Scripted {
        fn exchange(&self, apdu: &[u8]) -> Result<ISO7816ResponseAPDU, WebauthnCError> {
            self.sent.borrow_mut().push(apdu.to_vec());
            let answer = self.answers.borrow_mut().pop_front().ok_or_else(lost)?;
            ISO7816ResponseAPDU::try_from(answer.as_slice()).map_err(|_| lost())
        }
        fn cancelled(&self) -> bool {
            false
        }
    }
    #[derive(Debug)]
    struct Quiet;
    impl UiCallback for Quiet {
        fn request_pin(&self) -> Option<String> {
            None
        }
        fn request_touch(&self) {}
        fn processing(&self) {}
        fn fingerprint_enrollment_feedback(&self, _: u32, _: Option<EnrollSampleStatus>) {}
        fn cable_qr_code(&self, _: CableRequestType, _: String) {}
        fn dismiss_qr_code(&self) {}
        fn cable_status_update(&self, _: CableState) {}
    }

    #[test]
    fn selects_the_fido_applet() {
        let key = Scripted::new(&[b"FIDO_2_0\x90\x00"]);
        assert!(select(&key).is_ok());
        assert_eq!(key.sent.borrow()[0][..5], [0x00, 0xa4, 0x04, 0x00, 8]);
        // A card without it (a payment card, a transit pass) isn't a key.
        let card = Scripted::new(&[b"\x6a\x82"]);
        assert!(matches!(select(&card), Err(WebauthnCError::NotSupported)));
    }

    #[test]
    fn long_commands_are_chained_and_answers_gathered() {
        // A 300-byte command goes in two parts; the answer comes in two
        // (61xx, GET RESPONSE) after the key asked for patience (9100).
        let key = Scripted::new(&[
            b"\x90\x00",
            b"\x91\x00",
            b"\x00\x01\x61\x02",
            b"\x02\x03\x90\x00",
        ]);
        let answer = transmit_with(&key, &[7; 300], &Quiet, Duration::ZERO).unwrap();
        assert_eq!(answer, [1, 2, 3]);
        let sent = key.sent.borrow();
        assert_eq!(sent[0][..5], [0x90, 0x10, 0x00, 0x00, 255]);
        assert_eq!(sent[1][..5], [0x80, 0x10, 0x00, 0x00, 45]);
        assert_eq!(sent[1].last(), Some(&0x00));
        assert_eq!(sent[2], [0x80, 0x11, 0x00, 0x00, 0x00]);
        assert_eq!(sent[3], [0x00, 0xc0, 0x00, 0x00, 0x02]);
    }

    #[test]
    fn a_failing_status_is_the_keys_error() {
        // CTAP2_ERR_NO_CREDENTIALS.
        let key = Scripted::new(&[b"\x2e\x90\x00"]);
        assert!(matches!(
            transmit(&key, &[1], &Quiet),
            Err(WebauthnCError::Ctap(CtapError::Ctap2NoCredentials))
        ));
        assert!(matches!(
            ctap_answer(Vec::new()),
            Err(WebauthnCError::MessageTooShort)
        ));
    }

    #[test]
    fn an_endless_answer_is_cut_off() {
        let chunk = [[0u8; 255].as_slice(), b"\x61\xff"].concat();
        let answers: Vec<&[u8]> = std::iter::repeat_n(chunk.as_slice(), 300).collect();
        let key = Scripted::new(&answers);
        assert!(matches!(
            transmit_with(&key, &[1], &Quiet, Duration::ZERO),
            Err(WebauthnCError::Ctap(CtapError::Ctap1InvalidLength))
        ));
    }
}
