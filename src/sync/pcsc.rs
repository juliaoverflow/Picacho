//! PC/SC, the system's smart card service (pcsc-lite on Linux and the
//! BSDs, the PCSC framework on macOS, WinSCard on Windows), for OpenPGP
//! cards: YubiKeys, Nitrokeys, Gnuk tokens and smart cards in a reader.
//!
//! The library is loaded when a card is first asked for, as Kerberos's is
//! (`sync/gss.rs`): builds need no PC/SC headers, and computers without it
//! run as before, saying so when a security key is used. In a Flatpak the
//! sandbox must reach pcscd (`--socket=pcsc`).
//!
//! Each reader with a card is offered to openpgp-card as a `CardBackend`;
//! APDUs go through `SCardTransmit` inside a PC/SC transaction, so other
//! programs (GnuPG's scdaemon) can share the card between operations.
// Phones have no PC/SC: only `cards`, saying so, is used there. The
// platforms' DWORD is u32 on some, so casts to it are needed only on others.
#![cfg_attr(any(target_os = "android", target_os = "ios"), allow(dead_code))]
#![allow(clippy::unnecessary_cast)]
use card_backend::{CardBackend, SmartcardError};
#[cfg(not(any(target_os = "android", target_os = "ios")))]
use card_backend::{CardCaps, CardTransaction, PinType};

#[cfg(not(any(target_os = "android", target_os = "ios")))]
mod platform {
    use std::ffi::{c_char, c_void};

    #[cfg(target_os = "macos")]
    pub type Dword = u32;
    #[cfg(target_os = "macos")]
    pub type Long = i32;
    #[cfg(target_os = "macos")]
    pub type Context = i32;
    #[cfg(target_os = "macos")]
    pub type Handle = i32;

    #[cfg(windows)]
    pub type Dword = u32;
    #[cfg(windows)]
    pub type Long = i32;
    #[cfg(windows)]
    pub type Context = usize;
    #[cfg(windows)]
    pub type Handle = usize;

    // pcsc-lite: DWORD is `unsigned long`, LONG and the handles `long`.
    #[cfg(all(unix, not(target_os = "macos")))]
    pub type Dword = std::ffi::c_ulong;
    #[cfg(all(unix, not(target_os = "macos")))]
    pub type Long = std::ffi::c_long;
    #[cfg(all(unix, not(target_os = "macos")))]
    pub type Context = std::ffi::c_long;
    #[cfg(all(unix, not(target_os = "macos")))]
    pub type Handle = std::ffi::c_long;

    #[repr(C)]
    pub struct IoRequest {
        pub protocol: Dword,
        pub length: Dword,
    }

    pub type Establish = unsafe extern "system" fn(Dword, *const c_void, *const c_void, *mut Context) -> Long;
    pub type Release = unsafe extern "system" fn(Context) -> Long;
    pub type ListReaders = unsafe extern "system" fn(Context, *const c_char, *mut c_char, *mut Dword) -> Long;
    pub type Connect = unsafe extern "system" fn(Context, *const c_char, Dword, Dword, *mut Handle, *mut Dword) -> Long;
    pub type Disconnect = unsafe extern "system" fn(Handle, Dword) -> Long;
    pub type Begin = unsafe extern "system" fn(Handle) -> Long;
    pub type End = unsafe extern "system" fn(Handle, Dword) -> Long;
    pub type Transmit = unsafe extern "system" fn(
        Handle,
        *const IoRequest,
        *const u8,
        Dword,
        *mut IoRequest,
        *mut u8,
        *mut Dword,
    ) -> Long;
    pub type Reconnect = unsafe extern "system" fn(Handle, Dword, Dword, Dword, *mut Dword) -> Long;

    #[cfg(target_os = "macos")]
    pub const LIBRARIES: &[&str] = &["/System/Library/Frameworks/PCSC.framework/PCSC"];
    #[cfg(windows)]
    pub const LIBRARIES: &[&str] = &["winscard.dll"];
    #[cfg(all(unix, not(target_os = "macos")))]
    pub const LIBRARIES: &[&str] = &["libpcsclite.so.1", "libpcsclite.so"];

    /// Windows names the text functions' ANSI forms with an `A`.
    #[cfg(windows)]
    pub const LIST_READERS: &[u8] = b"SCardListReadersA\0";
    #[cfg(not(windows))]
    pub const LIST_READERS: &[u8] = b"SCardListReaders\0";
    #[cfg(windows)]
    pub const CONNECT: &[u8] = b"SCardConnectA\0";
    #[cfg(not(windows))]
    pub const CONNECT: &[u8] = b"SCardConnect\0";
}

#[cfg(not(any(target_os = "android", target_os = "ios")))]
use platform::*;

/// PC/SC's answers that matter here (`SCARD_*`, as unsigned 32-bit codes).
const SUCCESS: u32 = 0;
const NO_READERS: u32 = 0x8010_002E;
const NO_SERVICE: u32 = 0x8010_001D;
const NO_SMARTCARD: u32 = 0x8010_000C;
const RESET: u32 = 0x8010_0068;
const INSUFFICIENT_BUFFER: u32 = 0x8010_0008;

#[cfg(not(any(target_os = "android", target_os = "ios")))]
struct Library {
    _lib: libloading::Library,
    establish: Establish,
    release: Release,
    list_readers: ListReaders,
    connect: Connect,
    disconnect: Disconnect,
    begin: Begin,
    end: End,
    transmit: Transmit,
    reconnect: Reconnect,
}

/// The system's PC/SC library, loaded once; `Err` says why it can't be.
#[cfg(not(any(target_os = "android", target_os = "ios")))]
fn library() -> Result<&'static Library, String> {
    static LIB: std::sync::OnceLock<Result<Library, String>> = std::sync::OnceLock::new();
    LIB.get_or_init(|| {
        let mut last = String::new();
        for name in LIBRARIES {
            // SAFETY: loading the system's PC/SC library runs its
            // initializers, which have no requirements of the caller.
            match unsafe { libloading::Library::new(name) } {
                Ok(lib) => {
                    // SAFETY: the symbols are PC/SC's, declared with the
                    // platform's types above; the library is kept with them.
                    return unsafe {
                        Ok(Library {
                            establish: *lib.get::<Establish>(b"SCardEstablishContext\0").map_err(|e| e.to_string())?,
                            release: *lib.get::<Release>(b"SCardReleaseContext\0").map_err(|e| e.to_string())?,
                            list_readers: *lib.get::<ListReaders>(LIST_READERS).map_err(|e| e.to_string())?,
                            connect: *lib.get::<Connect>(CONNECT).map_err(|e| e.to_string())?,
                            disconnect: *lib.get::<Disconnect>(b"SCardDisconnect\0").map_err(|e| e.to_string())?,
                            begin: *lib.get::<Begin>(b"SCardBeginTransaction\0").map_err(|e| e.to_string())?,
                            end: *lib.get::<End>(b"SCardEndTransaction\0").map_err(|e| e.to_string())?,
                            transmit: *lib.get::<Transmit>(b"SCardTransmit\0").map_err(|e| e.to_string())?,
                            reconnect: *lib.get::<Reconnect>(b"SCardReconnect\0").map_err(|e| e.to_string())?,
                            _lib: lib,
                        })
                    };
                }
                Err(e) => last = e.to_string(),
            }
        }
        Err(last)
    })
    .as_ref()
    .map_err(Clone::clone)
}

/// A PC/SC answer as an unsigned code, whatever the platform's LONG.
fn code(rv: impl TryInto<i64>) -> u32 {
    rv.try_into().map(|v: i64| v as u32).unwrap_or(u32::MAX)
}

fn describe(rv: u32) -> String {
    match rv {
        NO_READERS => t!("security-keys-no-reader").into(),
        NO_SERVICE => t!("security-keys-no-pcsc-service").into(),
        NO_SMARTCARD => t!("security-keys-no-card-in-reader").into(),
        _ => t!("security-keys-pcsc-error", code = format!("{rv:#010x}")),
    }
}

/// A PC/SC context: the connection to the smart card service.
#[cfg(not(any(target_os = "android", target_os = "ios")))]
struct Session {
    lib: &'static Library,
    context: Context,
}
#[cfg(not(any(target_os = "android", target_os = "ios")))]
impl Drop for Session {
    fn drop(&mut self) {
        // SAFETY: the context was established and is released once.
        unsafe { (self.lib.release)(self.context) };
    }
}

/// The readers with a card in them, each as a backend openpgp-card can
/// open; readers that can't be connected to (empty ones) are left out.
#[cfg(not(any(target_os = "android", target_os = "ios")))]
pub fn cards() -> Result<Vec<Box<dyn CardBackend + Send + Sync>>, SmartcardError> {
    let lib = library().map_err(|e| SmartcardError::ContextError(t!("security-keys-no-pcsc-library", error = e)))?;
    let mut context: Context = 0;
    // SAFETY: an out-pointer to a context; scope 0 is SCARD_SCOPE_USER.
    let rv = code(unsafe { (lib.establish)(0, std::ptr::null(), std::ptr::null(), &mut context) });
    if rv != SUCCESS {
        return Err(SmartcardError::ContextError(describe(rv)));
    }
    let session = std::sync::Arc::new(Session { lib, context });
    let readers = list_readers(&session)?;
    let mut out: Vec<Box<dyn CardBackend + Send + Sync>> = Vec::new();
    for reader in readers {
        if let Ok(card) = PcscCard::connect(session.clone(), &reader) {
            out.push(Box::new(card));
        }
    }
    Ok(out)
}

#[cfg(any(target_os = "android", target_os = "ios"))]
pub fn cards() -> Result<Vec<Box<dyn CardBackend + Send + Sync>>, SmartcardError> {
    Err(SmartcardError::ContextError(t!("security-keys-not-on-this-device").into()))
}

#[cfg(not(any(target_os = "android", target_os = "ios")))]
fn list_readers(session: &Session) -> Result<Vec<std::ffi::CString>, SmartcardError> {
    let mut len: Dword = 0;
    // SAFETY: asks for the size the multi-string needs.
    let rv = code(unsafe { (session.lib.list_readers)(session.context, std::ptr::null(), std::ptr::null_mut(), &mut len) });
    if rv == NO_READERS {
        return Ok(Vec::new());
    }
    if rv != SUCCESS {
        return Err(SmartcardError::ReaderError(describe(rv)));
    }
    let mut buffer = vec![0u8; len as usize];
    // SAFETY: the buffer holds `len` bytes, as the call above said.
    let rv = code(unsafe {
        (session.lib.list_readers)(session.context, std::ptr::null(), buffer.as_mut_ptr().cast(), &mut len)
    });
    if rv == NO_READERS {
        return Ok(Vec::new());
    }
    if rv != SUCCESS {
        return Err(SmartcardError::ReaderError(describe(rv)));
    }
    buffer.truncate(len as usize);
    Ok(split_readers(&buffer))
}

/// A PC/SC multi-string (names each ending in NUL, then another NUL) as
/// names.
fn split_readers(buffer: &[u8]) -> Vec<std::ffi::CString> {
    buffer
        .split(|b| *b == 0)
        .filter(|name| !name.is_empty())
        .filter_map(|name| std::ffi::CString::new(name).ok())
        .collect()
}

/// One card, connected (shared with other programs).
#[cfg(not(any(target_os = "android", target_os = "ios")))]
pub struct PcscCard {
    session: std::sync::Arc<Session>,
    handle: Handle,
    protocol: Dword,
    reset: bool,
}

// SAFETY: the handle is an integer PC/SC lets any thread use; calls on
// one card are serialized by `&mut self`.
#[cfg(not(any(target_os = "android", target_os = "ios")))]
unsafe impl Send for PcscCard {}
#[cfg(not(any(target_os = "android", target_os = "ios")))]
unsafe impl Sync for PcscCard {}
#[cfg(not(any(target_os = "android", target_os = "ios")))]
unsafe impl Send for Session {}
#[cfg(not(any(target_os = "android", target_os = "ios")))]
unsafe impl Sync for Session {}

/// SCARD_SHARE_SHARED, SCARD_PROTOCOL_T0 | T1, SCARD_LEAVE_CARD.
const SHARE_SHARED: u32 = 2;
const PROTOCOLS: u32 = 1 | 2;
const LEAVE_CARD: u32 = 0;

#[cfg(not(any(target_os = "android", target_os = "ios")))]
impl PcscCard {
    fn connect(session: std::sync::Arc<Session>, reader: &std::ffi::CStr) -> Result<Self, SmartcardError> {
        let mut handle: Handle = 0;
        let mut protocol: Dword = 0;
        // SAFETY: a NUL-terminated reader name and out-pointers.
        let rv = code(unsafe {
            (session.lib.connect)(
                session.context,
                reader.as_ptr(),
                SHARE_SHARED as Dword,
                PROTOCOLS as Dword,
                &mut handle,
                &mut protocol,
            )
        });
        if rv != SUCCESS {
            return Err(SmartcardError::SmartCardConnectionError(describe(rv)));
        }
        Ok(Self { session, handle, protocol, reset: false })
    }
}

#[cfg(not(any(target_os = "android", target_os = "ios")))]
impl Drop for PcscCard {
    fn drop(&mut self) {
        // SAFETY: the handle was connected and is let go once.
        unsafe { (self.session.lib.disconnect)(self.handle, LEAVE_CARD as Dword) };
    }
}

#[cfg(not(any(target_os = "android", target_os = "ios")))]
impl CardBackend for PcscCard {
    fn limit_card_caps(&self, card_caps: CardCaps) -> CardCaps {
        card_caps
    }
    fn transaction(
        &mut self,
        reselect_application: Option<&[u8]>,
    ) -> Result<Box<dyn CardTransaction + Send + Sync + '_>, SmartcardError> {
        let lib = self.session.lib;
        // SAFETY: a connected handle.
        let mut rv = code(unsafe { (lib.begin)(self.handle) });
        if rv == RESET {
            // Another program reset the card: connect again, and select
            // the application again below.
            let mut protocol: Dword = 0;
            // SAFETY: as SCardConnect, on the same handle.
            rv = code(unsafe {
                (lib.reconnect)(self.handle, SHARE_SHARED as Dword, PROTOCOLS as Dword, LEAVE_CARD as Dword, &mut protocol)
            });
            if rv == SUCCESS {
                self.protocol = protocol;
                self.reset = true;
                // SAFETY: as above.
                rv = code(unsafe { (lib.begin)(self.handle) });
            }
        }
        if rv != SUCCESS {
            return Err(SmartcardError::Error(describe(rv)));
        }
        let mut tx = Box::new(PcscTransaction { card: self });
        if tx.card.reset
            && let Some(app) = reselect_application
        {
            tx.select(app)?;
            tx.card.reset = false;
        }
        Ok(tx)
    }
}

#[cfg(not(any(target_os = "android", target_os = "ios")))]
struct PcscTransaction<'c> {
    card: &'c mut PcscCard,
}

#[cfg(not(any(target_os = "android", target_os = "ios")))]
impl Drop for PcscTransaction<'_> {
    fn drop(&mut self) {
        // SAFETY: the transaction begun in `transaction` ends once.
        unsafe { (self.card.session.lib.end)(self.card.handle, LEAVE_CARD as Dword) };
    }
}

#[cfg(not(any(target_os = "android", target_os = "ios")))]
impl CardTransaction for PcscTransaction<'_> {
    fn transmit(&mut self, cmd: &[u8], buf_size: usize) -> Result<Vec<u8>, SmartcardError> {
        let lib = self.card.session.lib;
        let send = IoRequest { protocol: self.card.protocol, length: std::mem::size_of::<IoRequest>() as Dword };
        // Room for the answer and its status word; grown once if PC/SC
        // says it needs more.
        let mut size = buf_size.max(2) + 2;
        for _ in 0..2 {
            let mut out = vec![0u8; size];
            let mut len = size as Dword;
            // SAFETY: `cmd` and `out` are valid for their lengths; the
            // receive protocol structure is optional.
            let rv = code(unsafe {
                (lib.transmit)(
                    self.card.handle,
                    &send,
                    cmd.as_ptr(),
                    cmd.len() as Dword,
                    std::ptr::null_mut(),
                    out.as_mut_ptr(),
                    &mut len,
                )
            });
            if rv == INSUFFICIENT_BUFFER {
                size = 65_538;
                continue;
            }
            if rv != SUCCESS {
                return Err(SmartcardError::Error(describe(rv)));
            }
            out.truncate(len as usize);
            return Ok(out);
        }
        Err(SmartcardError::Error(describe(INSUFFICIENT_BUFFER)))
    }
    fn feature_pinpad_verify(&self) -> bool {
        false
    }
    fn feature_pinpad_modify(&self) -> bool {
        false
    }
    fn pinpad_verify(&mut self, _pin: PinType, _card_caps: &Option<CardCaps>) -> Result<Vec<u8>, SmartcardError> {
        Err(SmartcardError::Error(t!("security-keys-no-pinpad").into()))
    }
    fn pinpad_modify(&mut self, _pin: PinType, _card_caps: &Option<CardCaps>) -> Result<Vec<u8>, SmartcardError> {
        Err(SmartcardError::Error(t!("security-keys-no-pinpad").into()))
    }
    fn was_reset(&self) -> bool {
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reader_names_are_split_from_the_multi_string() {
        let names = split_readers(b"Yubico YubiKey OTP+FIDO+CCID 00 00\0Nitrokey 3 [CCID] 01 00\0\0");
        let names: Vec<&str> = names.iter().map(|n| n.to_str().unwrap()).collect();
        assert_eq!(names, ["Yubico YubiKey OTP+FIDO+CCID 00 00", "Nitrokey 3 [CCID] 01 00"]);
        assert!(split_readers(b"\0").is_empty());
    }

    #[test]
    fn answers_read_the_same_on_every_platform() {
        // pcsc-lite's 64-bit LONG and Windows' and macOS's 32-bit one.
        assert_eq!(code(0x8010_002E_i64), NO_READERS);
        assert_eq!(code(0x8010_002E_u32 as i32), NO_READERS);
        assert_eq!(code(0_i32), SUCCESS);
    }
}
