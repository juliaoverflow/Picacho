//! Security keys on iPhones, for Proton's second factor (`sync::fido`).
//!
//! A key is held to the top of the iPhone: the Swift side
//! (packaging/ios/SecurityKeys.swift) runs a Core NFC session that looks for
//! an ISO 7816 tag with the FIDO applet and moves APDUs; CTAP2 is the
//! authenticator library's, over the `Token` here, framed as on Android
//! (`sync::ctap_nfc`).
//!
//! Apple's own security key and passkey APIs (AuthenticationServices) only
//! sign for sites that list the app in their associated domains, which
//! proton.me can't for Picacho; Core NFC talks to the key directly.

use crate::sync::ctap_nfc::{self, lost};
use anyhow::{Result, bail};
use async_trait::async_trait;
use std::{
    sync::atomic::{AtomicBool, Ordering},
    time::{Duration, Instant},
};
use webauthn_authenticator_rs::{
    error::WebauthnCError,
    transport::{Token, iso7816::ISO7816ResponseAPDU},
    ui::UiCallback,
};
use webauthn_rs_proto::AuthenticatorTransport;

unsafe extern "C" {
    /// Whether this device can read NFC tags (not iPads, nor the Simulator).
    fn picacho_ios_nfc_available() -> bool;
    /// Starts looking for a key, showing `message` on the system's sheet;
    /// `not_key` and `hold_still` are what it says when another tag is held
    /// to it, or the key moved away too soon. False when this device can't
    /// read NFC tags (iPads, the Simulator).
    fn picacho_ios_nfc_begin(
        message: *const u8,
        len: usize,
        not_key: *const u8,
        not_key_len: usize,
        hold_still: *const u8,
        hold_still_len: usize,
    ) -> bool;
    /// 0 still looking, 1 a key is connected, 2 the sheet was cancelled,
    /// 3 the session ended (it timed out, or failed).
    fn picacho_ios_nfc_poll() -> i32;
    /// Sends an APDU and writes the answer (data, SW1, SW2) to `out`;
    /// returns its length, or -1 if the key was lost.
    fn picacho_ios_nfc_transceive(apdu: *const u8, len: usize, out: *mut u8, cap: usize) -> isize;
    /// Changes the sheet's message.
    fn picacho_ios_nfc_message(message: *const u8, len: usize);
    /// Ends the session: with `error` shown if there is one (`len` > 0).
    fn picacho_ios_nfc_end(error: *const u8, len: usize);
}

/// The largest answer to one APDU: 256 bytes of data, then SW1 and SW2
/// (short APDUs; longer answers come in parts, 61xx).
const MOST: usize = 258;

/// Whether a security key can be held to this device: it reads NFC tags.
/// Asked once: the sign-in sheet asks as it draws.
pub fn available() -> bool {
    static AVAILABLE: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    // Safety: no arguments.
    *AVAILABLE.get_or_init(|| unsafe { picacho_ios_nfc_available() })
}

/// Looking for a security key, until dropped: the system's NFC sheet shows.
pub struct NfcSearch {
    ended: bool,
}

impl NfcSearch {
    pub fn start(message: &str) -> Result<Self> {
        let not_key = t!("ios-keys-that-isnt-a-security-key");
        let hold_still = t!("ios-keys-hold-your-security-key-still");
        // Safety: the strings are valid for the call; Swift copies them.
        let began = unsafe {
            picacho_ios_nfc_begin(
                message.as_ptr(),
                message.len(),
                not_key.as_ptr(),
                not_key.len(),
                hold_still.as_ptr(),
                hold_still.len(),
            )
        };
        if !began {
            bail!(t!("ios-keys-this-device-cant-read-security-keys"));
        }
        Ok(Self { ended: false })
    }

    /// Waits up to `wait` for a key to be held to the phone, or until
    /// `cancel`.
    pub fn find<'c>(&self, cancel: &'c AtomicBool, wait: Duration) -> Result<NfcToken<'c>> {
        let deadline = Instant::now() + wait;
        loop {
            if cancel.load(Ordering::Relaxed) {
                bail!(t!("ios-keys-sign-in-cancelled"));
            }
            if Instant::now() >= deadline {
                bail!(t!("ios-keys-no-security-key-was-found-hold"));
            }
            // Safety: no arguments.
            match unsafe { picacho_ios_nfc_poll() } {
                1 => return Ok(NfcToken { cancel }),
                2 => bail!(t!("ios-keys-using-the-security-key-was-cancelled")),
                3 => bail!(t!("ios-keys-no-security-key-was-found-hold")),
                _ => std::thread::sleep(Duration::from_millis(100)),
            }
        }
    }

    /// Shows `message` on the sheet.
    pub fn say(&self, message: &str) {
        // Safety: `message` is valid for the call.
        unsafe { picacho_ios_nfc_message(message.as_ptr(), message.len()) };
    }

    /// Ends the search, saying `error` on the sheet if there is one.
    pub fn finish(mut self, error: Option<&str>) {
        self.end(error.unwrap_or_default());
    }

    fn end(&mut self, error: &str) {
        if !std::mem::replace(&mut self.ended, true) {
            // Safety: `error` is valid for the call.
            unsafe { picacho_ios_nfc_end(error.as_ptr(), error.len()) };
        }
    }
}

impl Drop for NfcSearch {
    fn drop(&mut self) {
        self.end("");
    }
}

/// The key `NfcSearch` found, for the authenticator library. It stops being
/// waited on once the sign-in is cancelled.
#[derive(Debug)]
pub struct NfcToken<'c> {
    cancel: &'c AtomicBool,
}

impl ctap_nfc::Link for NfcToken<'_> {
    fn exchange(&self, apdu: &[u8]) -> Result<ISO7816ResponseAPDU, WebauthnCError> {
        let mut out = [0u8; MOST];
        // Safety: `apdu` and `out` are valid for the call, `out` for `MOST`
        // bytes; Swift writes at most that.
        let n = unsafe {
            picacho_ios_nfc_transceive(apdu.as_ptr(), apdu.len(), out.as_mut_ptr(), MOST)
        };
        let n = usize::try_from(n).map_err(|_| lost())?;
        if n > MOST {
            return Err(lost());
        }
        ISO7816ResponseAPDU::try_from(&out[..n]).map_err(|_| lost())
    }
    fn cancelled(&self) -> bool {
        self.cancel.load(Ordering::Relaxed)
    }
}

#[async_trait]
impl Token for NfcToken<'_> {
    type Id = ();

    fn has_button(&self) -> bool {
        // Holding the key to the phone is its touch.
        false
    }

    fn get_transport(&self) -> AuthenticatorTransport {
        AuthenticatorTransport::Nfc
    }

    async fn transmit_raw<U>(&mut self, cbor: &[u8], ui: &U) -> Result<Vec<u8>, WebauthnCError>
    where
        U: UiCallback,
    {
        ctap_nfc::transmit(self, cbor, ui)
    }

    async fn cancel(&mut self) -> Result<(), WebauthnCError> {
        Ok(())
    }

    async fn init(&mut self) -> Result<(), WebauthnCError> {
        ctap_nfc::select(self)
    }

    async fn close(&mut self) -> Result<(), WebauthnCError> {
        Ok(())
    }
}
