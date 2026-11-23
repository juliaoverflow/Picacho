//! Kerberos for SASL's GSSAPI mechanism (RFC 4752), through the system's
//! own Kerberos: the GSS-API library on Linux, the BSDs and macOS (MIT,
//! Heimdal or Apple's GSS.framework, loaded when first needed, so builds
//! need no Kerberos headers and computers without Kerberos still run
//! Picacho), and SSPI on Windows. The ticket is the one the system already
//! has (`kinit`, or signing in to a Windows domain); Picacho never sees a
//! password for it.
//!
//! Only what a client's sign-in needs: start a security context with a
//! service (`imap@mail.example.com`), step it with the server's tokens,
//! and wrap/unwrap the one security-layer message RFC 4752 exchanges
//! after it. Mutual authentication is required: the server must prove it
//! is the service named, not just accept the ticket.
use anyhow::Result;

/// A client security context, being established or established.
pub(crate) trait Context: Send {
    /// One step: the server's token in (empty for the first), the token
    /// to send back out, and whether the context is now established.
    fn step(&mut self, input: &[u8]) -> Result<(Vec<u8>, bool)>;
    /// Reads a message the server wrapped (integrity-protected).
    fn unwrap(&mut self, message: &[u8]) -> Result<Vec<u8>>;
    /// Wraps a message for the server, integrity-protected only.
    fn wrap(&mut self, message: &[u8]) -> Result<Vec<u8>>;
}

/// Starts a context with `service@host` (a host-based service name) using
/// the system's Kerberos ticket.
pub(crate) fn start(target: &str) -> Result<Box<dyn Context>> {
    imp::start(target, None)
}
/// As `start`, with channel bindings (RFC 2744 §3.11): their application
/// data only, no addresses, as RFC 5801 §5 has GS2 send its header and the
/// TLS channel's binding data in. The server must have the same to accept.
pub(crate) fn start_bound(target: &str, application_data: Vec<u8>) -> Result<Box<dyn Context>> {
    imp::start(target, Some(application_data))
}

/// Explains a failure to start: usually no ticket, or one for another realm.
#[allow(dead_code)]
fn explain(detail: &str) -> anyhow::Error {
    anyhow::anyhow!(
        "Kerberos sign-in failed: {detail}. Get a ticket for your realm first (kinit, or sign in to your domain), then try again."
    )
}

#[cfg(all(unix, not(any(target_os = "android", target_os = "ios"))))]
pub(crate) mod imp {
    use super::{Context, explain};
    use anyhow::{Context as _, Result, bail, ensure};
    use std::ffi::c_void;
    use std::sync::OnceLock;

    type OmUint32 = u32;
    /// `gss_buffer_desc`: a length and its bytes. Two pointer-sized fields,
    /// laid out the same under Apple's `#pragma pack(2)`.
    #[repr(C)]
    struct Buffer {
        length: usize,
        value: *mut c_void,
    }
    impl Buffer {
        fn empty() -> Self {
            Self {
                length: 0,
                value: std::ptr::null_mut(),
            }
        }
        fn of(bytes: &[u8]) -> Self {
            Self {
                length: bytes.len(),
                value: bytes.as_ptr() as *mut c_void,
            }
        }
    }

    /// `gss_OID_desc` (`OM_uint32 length; void *elements`), laid out as
    /// each system's library has it: C's alignment, and on macOS Apple's
    /// `#pragma pack(push, 2)` in <GSS/gssapi.h>, where the pointer
    /// follows the length directly (12 bytes, not 16).
    #[cfg_attr(not(target_os = "macos"), repr(C))]
    #[cfg_attr(target_os = "macos", repr(C, packed(2)))]
    struct OidDesc {
        length: OmUint32,
        elements: *const c_void,
    }
    // A pointer to static bytes, read only.
    unsafe impl Sync for OidDesc {}
    /// Kerberos 5's mechanism, 1.2.840.113554.1.2.2 (RFC 4752 §3.1 asks for
    /// it by name, not the library's default).
    static KRB5_OID_BYTES: [u8; 9] = [0x2a, 0x86, 0x48, 0x86, 0xf7, 0x12, 0x01, 0x02, 0x02];
    static KRB5_OID: OidDesc = OidDesc {
        length: 9,
        elements: KRB5_OID_BYTES.as_ptr() as *const c_void,
    };
    // The layouts the libraries expect.
    #[cfg(all(target_os = "macos", target_pointer_width = "64"))]
    const _: () = assert!(std::mem::size_of::<OidDesc>() == 12);
    #[cfg(all(not(target_os = "macos"), target_pointer_width = "64"))]
    const _: () = assert!(std::mem::size_of::<OidDesc>() == 16);
    #[cfg(all(target_os = "macos", target_pointer_width = "64"))]
    const _: () = assert!(std::mem::size_of::<ChannelBindings>() == 4 + 16 + 4 + 16 + 16);
    #[cfg(all(not(target_os = "macos"), target_pointer_width = "64"))]
    const _: () = assert!(std::mem::size_of::<ChannelBindings>() == 8 + 16 + 8 + 16 + 16);
    /// The mechanism to ask for: Kerberos 5 by its OID (RFC 4752 §3.1),
    /// not the library's default, which may be SPNEGO or another.
    fn mechanism() -> *mut c_void {
        &KRB5_OID as *const OidDesc as *mut c_void
    }

    const COMPLETE: OmUint32 = 0;
    const CONTINUE_NEEDED: OmUint32 = 1;
    /// GSS_ERROR(): the calling and routine error fields.
    const ERROR_MASK: OmUint32 = 0xffff_0000;
    const MUTUAL: OmUint32 = 2;
    const SEQUENCE: OmUint32 = 8;
    const CONF: OmUint32 = 16;
    const INTEG: OmUint32 = 32;
    const GSS_CODE: i32 = 1;
    const MECH_CODE: i32 = 2;

    type InitSecContext = unsafe extern "C" fn(
        *mut OmUint32,
        *mut c_void,
        *mut *mut c_void,
        *mut c_void,
        *mut c_void,
        OmUint32,
        OmUint32,
        *mut c_void,
        *mut Buffer,
        *mut *mut c_void,
        *mut Buffer,
        *mut OmUint32,
        *mut OmUint32,
    ) -> OmUint32;
    type ImportName =
        unsafe extern "C" fn(*mut OmUint32, *mut Buffer, *mut c_void, *mut *mut c_void) -> OmUint32;
    type ReleaseName = unsafe extern "C" fn(*mut OmUint32, *mut *mut c_void) -> OmUint32;
    type ReleaseBuffer = unsafe extern "C" fn(*mut OmUint32, *mut Buffer) -> OmUint32;
    type DeleteSecContext =
        unsafe extern "C" fn(*mut OmUint32, *mut *mut c_void, *mut Buffer) -> OmUint32;
    type Wrap = unsafe extern "C" fn(
        *mut OmUint32,
        *mut c_void,
        i32,
        OmUint32,
        *mut Buffer,
        *mut i32,
        *mut Buffer,
    ) -> OmUint32;
    type Unwrap = unsafe extern "C" fn(
        *mut OmUint32,
        *mut c_void,
        *mut Buffer,
        *mut Buffer,
        *mut i32,
        *mut OmUint32,
    ) -> OmUint32;
    type DisplayStatus = unsafe extern "C" fn(
        *mut OmUint32,
        OmUint32,
        i32,
        *mut c_void,
        *mut OmUint32,
        *mut Buffer,
    ) -> OmUint32;

    /// The library's functions, and the host-based service name type.
    pub(super) struct Library {
        _library: libloading::Library,
        init_sec_context: InitSecContext,
        import_name: ImportName,
        release_name: ReleaseName,
        release_buffer: ReleaseBuffer,
        delete_sec_context: DeleteSecContext,
        wrap: Wrap,
        unwrap: Unwrap,
        display_status: DisplayStatus,
        hostbased_service: *mut c_void,
    }
    // The functions and the OID are the library's, valid while it's loaded
    // (for the life of the process); GSS-API is thread-safe for distinct
    // contexts.
    unsafe impl Send for Library {}
    unsafe impl Sync for Library {}

    /// Where each system keeps its GSS-API library.
    const CANDIDATES: &[&str] = if cfg!(target_os = "macos") {
        &[
            "/System/Library/Frameworks/GSS.framework/GSS",
            "/System/Library/Frameworks/Kerberos.framework/Kerberos",
        ]
    } else {
        // MIT, then Heimdal (the BSDs' base system, some Linux systems).
        &[
            "libgssapi_krb5.so.2",
            "libgssapi.so.3",
            "libgssapi.so.10",
            "libgssapi_krb5.so",
            "libgssapi.so",
        ]
    };

    pub(super) fn library() -> Result<&'static Library> {
        static LIBRARY: OnceLock<std::result::Result<Library, String>> = OnceLock::new();
        LIBRARY
            .get_or_init(|| load().map_err(|e| format!("{e:#}")))
            .as_ref()
            .map_err(|e| anyhow::anyhow!("{e}"))
    }

    fn load() -> Result<Library> {
        let mut last = None;
        for name in CANDIDATES {
            // SAFETY: loading the system's GSS-API library runs only its
            // own initializers.
            match unsafe { libloading::Library::new(name) } {
                Ok(library) => return bind(library),
                Err(e) => last = Some(e),
            }
        }
        bail!(
            "This computer has no Kerberos (GSS-API) library{}. Install your system's Kerberos package (krb5 or heimdal).",
            last.map(|e| format!(" ({e})")).unwrap_or_default()
        )
    }

    fn bind(library: libloading::Library) -> Result<Library> {
        // SAFETY: each symbol is looked up by its GSS-API name and given
        // its C signature from RFC 2744.
        unsafe {
            macro_rules! function {
                ($name:literal) => {
                    *library.get($name).with_context(|| {
                        format!(
                            "The Kerberos library lacks {}",
                            String::from_utf8_lossy(&$name[..$name.len() - 1])
                        )
                    })?
                };
            }
            // MIT exports the name type as a variable holding the OID's
            // address; Heimdal and Apple export the OID itself.
            let hostbased_service = if let Ok(variable) =
                library.get::<*mut *mut c_void>(b"GSS_C_NT_HOSTBASED_SERVICE\0")
            {
                **variable
            } else {
                *library
                    .get::<*mut c_void>(b"__gss_c_nt_hostbased_service_oid_desc\0")
                    .context("The Kerberos library doesn't name host-based services")?
            };
            ensure!(
                !hostbased_service.is_null(),
                "The Kerberos library has no host-based service name type."
            );
            Ok(Library {
                init_sec_context: function!(b"gss_init_sec_context\0"),
                import_name: function!(b"gss_import_name\0"),
                release_name: function!(b"gss_release_name\0"),
                release_buffer: function!(b"gss_release_buffer\0"),
                delete_sec_context: function!(b"gss_delete_sec_context\0"),
                wrap: function!(b"gss_wrap\0"),
                unwrap: function!(b"gss_unwrap\0"),
                display_status: function!(b"gss_display_status\0"),
                hostbased_service,
                _library: library,
            })
        }
    }

    impl Library {
        /// The library's words for a status: the GSS-API's, then the
        /// mechanism's (Kerberos': "No Kerberos credentials available…").
        pub(super) fn describe(&self, major: OmUint32, minor: OmUint32) -> String {
            let mut parts = Vec::new();
            for (code, kind) in [(major, GSS_CODE), (minor, MECH_CODE)] {
                if code == 0 {
                    continue;
                }
                let mut context = 0;
                for _ in 0..8 {
                    let mut ignored = 0;
                    let mut text = Buffer::empty();
                    // SAFETY: an output buffer the library fills and we release.
                    let status = unsafe {
                        (self.display_status)(
                            &mut ignored,
                            code,
                            kind,
                            std::ptr::null_mut(),
                            &mut context,
                            &mut text,
                        )
                    };
                    if status & ERROR_MASK != 0 {
                        break;
                    }
                    let message = unsafe { take(self, &mut text) };
                    let message = String::from_utf8_lossy(&message).trim().to_owned();
                    if !message.is_empty() && !parts.contains(&message) {
                        parts.push(message);
                    }
                    if context == 0 {
                        break;
                    }
                }
            }
            if parts.is_empty() {
                format!("GSS-API status {major:#x}/{minor}")
            } else {
                parts.join("; ")
            }
        }
    }

    /// A buffer's bytes, copied, and the buffer released.
    ///
    /// SAFETY: `buffer` must have been filled by the library.
    unsafe fn take(library: &Library, buffer: &mut Buffer) -> Vec<u8> {
        let bytes = if buffer.value.is_null() || buffer.length == 0 {
            Vec::new()
        } else {
            unsafe { std::slice::from_raw_parts(buffer.value as *const u8, buffer.length) }.to_vec()
        };
        let mut minor = 0;
        unsafe { (library.release_buffer)(&mut minor, buffer) };
        bytes
    }

    /// `gss_channel_bindings_struct` (RFC 2744 §3.11): addresses of type
    /// GSS_C_AF_NULLADDR (0) and empty, and the application data. Packed
    /// to 2 on macOS, as Apple's header has it (see `OidDesc`).
    #[cfg_attr(not(target_os = "macos"), repr(C))]
    #[cfg_attr(target_os = "macos", repr(C, packed(2)))]
    struct ChannelBindings {
        initiator_addrtype: OmUint32,
        initiator_address: Buffer,
        acceptor_addrtype: OmUint32,
        acceptor_address: Buffer,
        application_data: Buffer,
    }
    struct Gss {
        library: &'static Library,
        context: *mut c_void,
        target: *mut c_void,
        established: bool,
        /// The channel bindings' application data, where there are any.
        bindings: Option<Vec<u8>>,
    }
    // A context is used by one sign-in at a time, which may move between
    // threads; GSS-API contexts aren't tied to the thread that made them.
    unsafe impl Send for Gss {}

    pub(super) fn start(target: &str, bindings: Option<Vec<u8>>) -> Result<Box<dyn Context>> {
        let library = library()?;
        let mut minor = 0;
        let mut name = std::ptr::null_mut();
        let mut text = Buffer::of(target.as_bytes());
        // SAFETY: the input buffer outlives the call; the name is released on drop.
        let major = unsafe {
            (library.import_name)(&mut minor, &mut text, library.hostbased_service, &mut name)
        };
        if major & ERROR_MASK != 0 {
            bail!(explain(&library.describe(major, minor)));
        }
        Ok(Box::new(Gss {
            library,
            context: std::ptr::null_mut(),
            target: name,
            established: false,
            bindings,
        }))
    }

    impl Context for Gss {
        fn step(&mut self, input: &[u8]) -> Result<(Vec<u8>, bool)> {
            ensure!(
                !self.established,
                "The Kerberos sign-in was already complete."
            );
            let mut minor = 0;
            let mut token = Buffer::of(input);
            let mut output = Buffer::empty();
            let mut flags = 0;
            let mut bindings = self.bindings.as_ref().map(|data| ChannelBindings {
                initiator_addrtype: 0,
                initiator_address: Buffer::empty(),
                acceptor_addrtype: 0,
                acceptor_address: Buffer::empty(),
                application_data: Buffer::of(data),
            });
            let bindings_ptr = bindings
                .as_mut()
                .map_or(std::ptr::null_mut(), |b| b as *mut ChannelBindings as *mut c_void);
            // SAFETY: GSS_C_NO_CREDENTIAL (the default ticket) and
            // GSS_C_NO_OID (the default mechanism, Kerberos); buffers and
            // the bindings live through the call, and the output is
            // released by `take`.
            let major = unsafe {
                (self.library.init_sec_context)(
                    &mut minor,
                    std::ptr::null_mut(),
                    &mut self.context,
                    self.target,
                    mechanism(),
                    MUTUAL | SEQUENCE | INTEG | CONF,
                    0,
                    bindings_ptr,
                    if self.context.is_null() {
                        std::ptr::null_mut()
                    } else {
                        &mut token
                    },
                    std::ptr::null_mut(),
                    &mut output,
                    &mut flags,
                    std::ptr::null_mut(),
                )
            };
            let out = unsafe { take(self.library, &mut output) };
            if major & ERROR_MASK != 0 {
                bail!(explain(&self.library.describe(major, minor)));
            }
            // Supplementary bits besides "continue" (a duplicate, old or out
            // of order token: the server's answer replayed) were taken to
            // mean "continue".
            ensure!(
                major & 0xffff & !CONTINUE_NEEDED == 0,
                "The server's Kerberos answer was replayed or out of order; not signed in."
            );
            let done = major & 0xffff == COMPLETE && major & CONTINUE_NEEDED == 0;
            if done {
                ensure!(
                    flags & MUTUAL != 0,
                    "The server didn't prove it is the service it claims to be (no Kerberos mutual authentication); not signed in."
                );
                // The security-layer exchange is wrapped (RFC 4752 §3.1):
                // integrity must be on.
                ensure!(
                    flags & INTEG != 0,
                    "The Kerberos sign-in has no integrity protection; not signed in."
                );
                self.established = true;
            }
            Ok((out, done))
        }
        fn unwrap(&mut self, message: &[u8]) -> Result<Vec<u8>> {
            ensure!(self.established, "The Kerberos sign-in isn't complete.");
            let mut minor = 0;
            let mut input = Buffer::of(message);
            let mut output = Buffer::empty();
            let mut confidential = 0;
            let mut qop = 0;
            // SAFETY: as in `step`.
            let major = unsafe {
                (self.library.unwrap)(
                    &mut minor,
                    self.context,
                    &mut input,
                    &mut output,
                    &mut confidential,
                    &mut qop,
                )
            };
            let out = unsafe { take(self.library, &mut output) };
            if major & ERROR_MASK != 0 {
                bail!(
                    "The server's Kerberos message couldn't be read: {}",
                    self.library.describe(major, minor)
                );
            }
            Ok(out)
        }
        fn wrap(&mut self, message: &[u8]) -> Result<Vec<u8>> {
            ensure!(self.established, "The Kerberos sign-in isn't complete.");
            let mut minor = 0;
            let mut input = Buffer::of(message);
            let mut output = Buffer::empty();
            let mut confidential = 0;
            // SAFETY: as in `step`. Integrity only (conf_req 0): TLS
            // already encrypts the connection.
            let major = unsafe {
                (self.library.wrap)(
                    &mut minor,
                    self.context,
                    0,
                    0,
                    &mut input,
                    &mut confidential,
                    &mut output,
                )
            };
            let out = unsafe { take(self.library, &mut output) };
            if major & ERROR_MASK != 0 {
                bail!(
                    "Picacho couldn't sign its Kerberos answer: {}",
                    self.library.describe(major, minor)
                );
            }
            Ok(out)
        }
    }
    impl Drop for Gss {
        fn drop(&mut self) {
            let mut minor = 0;
            // SAFETY: each handle is the library's, released once.
            unsafe {
                if !self.context.is_null() {
                    (self.library.delete_sec_context)(
                        &mut minor,
                        &mut self.context,
                        std::ptr::null_mut(),
                    );
                }
                if !self.target.is_null() {
                    (self.library.release_name)(&mut minor, &mut self.target);
                }
            }
        }
    }

    /// The server's side, for tests against a real KDC: accepts a client's
    /// context with a keytab's key.
    #[cfg(test)]
    pub(crate) mod acceptor {
        use super::*;
        type AcceptSecContext = unsafe extern "C" fn(
            *mut OmUint32,
            *mut *mut c_void,
            *mut c_void,
            *mut Buffer,
            *mut c_void,
            *mut *mut c_void,
            *mut *mut c_void,
            *mut Buffer,
            *mut OmUint32,
            *mut OmUint32,
            *mut *mut c_void,
        ) -> OmUint32;
        pub(crate) struct Acceptor {
            library: &'static Library,
            accept: AcceptSecContext,
            context: *mut c_void,
            /// The channel bindings' application data the client must
            /// have used, where set.
            pub(crate) bindings: Option<Vec<u8>>,
        }
        unsafe impl Send for Acceptor {}
        impl Acceptor {
            pub(crate) fn new() -> Result<Self> {
                let library = library()?;
                let accept = unsafe {
                    *library
                        ._library
                        .get::<AcceptSecContext>(b"gss_accept_sec_context\0")?
                };
                Ok(Self {
                    library,
                    accept,
                    context: std::ptr::null_mut(),
                    bindings: None,
                })
            }
            /// A client token in; the token to send back, and whether done.
            pub(crate) fn step(&mut self, input: &[u8]) -> Result<(Vec<u8>, bool)> {
                let mut minor = 0;
                let mut token = Buffer::of(input);
                let mut output = Buffer::empty();
                let mut flags = 0;
                let mut bindings = self.bindings.as_ref().map(|data| ChannelBindings {
                    initiator_addrtype: 0,
                    initiator_address: Buffer::empty(),
                    acceptor_addrtype: 0,
                    acceptor_address: Buffer::empty(),
                    application_data: Buffer::of(data),
                });
                let bindings_ptr = bindings
                    .as_mut()
                    .map_or(std::ptr::null_mut(), |b| b as *mut ChannelBindings as *mut c_void);
                let major = unsafe {
                    (self.accept)(
                        &mut minor,
                        &mut self.context,
                        std::ptr::null_mut(),
                        &mut token,
                        bindings_ptr,
                        std::ptr::null_mut(),
                        std::ptr::null_mut(),
                        &mut output,
                        &mut flags,
                        std::ptr::null_mut(),
                        std::ptr::null_mut(),
                    )
                };
                let out = unsafe { take(self.library, &mut output) };
                ensure!(
                    major & ERROR_MASK == 0,
                    "accept: {}",
                    self.library.describe(major, minor)
                );
                Ok((out, major == COMPLETE))
            }
            pub(crate) fn wrap(&mut self, message: &[u8]) -> Result<Vec<u8>> {
                let mut minor = 0;
                let mut input = Buffer::of(message);
                let mut output = Buffer::empty();
                let mut confidential = 0;
                let major = unsafe {
                    (self.library.wrap)(
                        &mut minor,
                        self.context,
                        0,
                        0,
                        &mut input,
                        &mut confidential,
                        &mut output,
                    )
                };
                let out = unsafe { take(self.library, &mut output) };
                ensure!(
                    major & ERROR_MASK == 0,
                    "wrap: {}",
                    self.library.describe(major, minor)
                );
                Ok(out)
            }
            pub(crate) fn unwrap(&mut self, message: &[u8]) -> Result<Vec<u8>> {
                let mut minor = 0;
                let mut input = Buffer::of(message);
                let mut output = Buffer::empty();
                let (mut confidential, mut qop) = (0, 0);
                let major = unsafe {
                    (self.library.unwrap)(
                        &mut minor,
                        self.context,
                        &mut input,
                        &mut output,
                        &mut confidential,
                        &mut qop,
                    )
                };
                let out = unsafe { take(self.library, &mut output) };
                ensure!(
                    major & ERROR_MASK == 0,
                    "unwrap: {}",
                    self.library.describe(major, minor)
                );
                Ok(out)
            }
        }
        impl Drop for Acceptor {
            fn drop(&mut self) {
                let mut minor = 0;
                if !self.context.is_null() {
                    unsafe {
                        (self.library.delete_sec_context)(
                            &mut minor,
                            &mut self.context,
                            std::ptr::null_mut(),
                        )
                    };
                }
            }
        }
    }
}

#[cfg(windows)]
mod imp {
    //! SSPI's Kerberos package, with the signed-in Windows user's ticket.
    use super::{Context, explain};
    use anyhow::{Result, bail, ensure};
    use windows_sys::Win32::Foundation::{SEC_E_OK, SEC_I_CONTINUE_NEEDED};
    use windows_sys::Win32::Security::Authentication::Identity::{
        AcquireCredentialsHandleW, DecryptMessage, DeleteSecurityContext, EncryptMessage,
        FreeContextBuffer, FreeCredentialsHandle, ISC_REQ_ALLOCATE_MEMORY, ISC_REQ_CONFIDENTIALITY,
        ISC_REQ_INTEGRITY, ISC_REQ_MUTUAL_AUTH, ISC_REQ_SEQUENCE_DETECT, ISC_RET_INTEGRITY,
        ISC_RET_MUTUAL_AUTH,
        InitializeSecurityContextW, QueryContextAttributesW, SECBUFFER_DATA, SECBUFFER_PADDING,
        SECBUFFER_STREAM, SECBUFFER_TOKEN, SECBUFFER_VERSION, SECPKG_ATTR_SIZES,
        SECPKG_CRED_OUTBOUND, SECQOP_WRAP_NO_ENCRYPT, SECURITY_NATIVE_DREP, SecBuffer,
        SecBufferDesc, SecPkgContext_Sizes,
    };
    use windows_sys::Win32::Security::Credentials::SecHandle;

    fn wide(text: &str) -> Vec<u16> {
        text.encode_utf16().chain(std::iter::once(0)).collect()
    }

    struct Sspi {
        credentials: SecHandle,
        context: Option<SecHandle>,
        /// The service principal: `imap/mail.example.com`.
        target: Vec<u16>,
        established: bool,
        /// SEC_CHANNEL_BINDINGS followed by its application data, where
        /// there are channel bindings.
        bindings: Option<Vec<u8>>,
    }
    /// SSPI's channel bindings: the header, then the application data
    /// right after it (its offset counts from the header's start).
    fn sspi_bindings(data: &[u8]) -> Vec<u8> {
        use windows_sys::Win32::Security::Authentication::Identity::SEC_CHANNEL_BINDINGS;
        let header = std::mem::size_of::<SEC_CHANNEL_BINDINGS>();
        let fields = SEC_CHANNEL_BINDINGS {
            cbApplicationDataLength: data.len() as u32,
            dwApplicationDataOffset: header as u32,
            ..Default::default()
        };
        let mut out = vec![0u8; header];
        // SAFETY: a plain C struct of u32s, copied as bytes.
        unsafe {
            std::ptr::copy_nonoverlapping(
                &fields as *const SEC_CHANNEL_BINDINGS as *const u8,
                out.as_mut_ptr(),
                header,
            )
        };
        out.extend_from_slice(data);
        out
    }
    // SSPI handles aren't tied to a thread.
    unsafe impl Send for Sspi {}

    pub(super) fn start(target: &str, bindings: Option<Vec<u8>>) -> Result<Box<dyn Context>> {
        // SSPI names services as `service/host`.
        let spn = target.replacen('@', "/", 1);
        let package = wide("Kerberos");
        let mut credentials = SecHandle::default();
        let mut expiry = 0i64;
        // SAFETY: the signed-in user's own credentials, outbound only.
        let status = unsafe {
            AcquireCredentialsHandleW(
                std::ptr::null(),
                package.as_ptr(),
                SECPKG_CRED_OUTBOUND,
                std::ptr::null(),
                std::ptr::null(),
                None,
                std::ptr::null(),
                &mut credentials,
                &mut expiry,
            )
        };
        if status != SEC_E_OK {
            bail!(explain(&format!(
                "Windows has no Kerberos credentials for you (SSPI {status:#x})"
            )));
        }
        Ok(Box::new(Sspi {
            credentials,
            context: None,
            target: wide(&spn),
            established: false,
            bindings: bindings.as_deref().map(sspi_bindings),
        }))
    }

    impl Context for Sspi {
        fn step(&mut self, input: &[u8]) -> Result<(Vec<u8>, bool)> {
            ensure!(
                !self.established,
                "The Kerberos sign-in was already complete."
            );
            let mut bindings = self.bindings.clone().unwrap_or_default();
            let mut in_buffers = [
                SecBuffer {
                    cbBuffer: input.len() as u32,
                    BufferType: SECBUFFER_TOKEN,
                    pvBuffer: input.as_ptr() as *mut _,
                },
                SecBuffer {
                    cbBuffer: bindings.len() as u32,
                    BufferType: windows_sys::Win32::Security::Authentication::Identity::SECBUFFER_CHANNEL_BINDINGS,
                    pvBuffer: bindings.as_mut_ptr() as *mut _,
                },
            ];
            // The first call has no token, only the bindings (where any).
            let first = self.context.is_none();
            let (start, count) = match (first, self.bindings.is_some()) {
                (true, true) => (1, 1),
                (true, false) => (0, 0),
                (false, true) => (0, 2),
                (false, false) => (0, 1),
            };
            let in_desc = SecBufferDesc {
                ulVersion: SECBUFFER_VERSION,
                cBuffers: count,
                pBuffers: in_buffers[start..].as_mut_ptr(),
            };
            let mut out_buffer = SecBuffer {
                cbBuffer: 0,
                BufferType: SECBUFFER_TOKEN,
                pvBuffer: std::ptr::null_mut(),
            };
            let mut out_desc = SecBufferDesc {
                ulVersion: SECBUFFER_VERSION,
                cBuffers: 1,
                pBuffers: &mut out_buffer,
            };
            let mut next = SecHandle::default();
            let mut attributes = 0u32;
            let mut expiry = 0i64;
            let previous = self.context;
            // SAFETY: buffers live through the call; SSPI allocates the
            // output token, freed below.
            let status = unsafe {
                InitializeSecurityContextW(
                    &self.credentials,
                    previous
                        .as_ref()
                        .map_or(std::ptr::null(), |c| c as *const _),
                    self.target.as_ptr(),
                    ISC_REQ_MUTUAL_AUTH
                        | ISC_REQ_INTEGRITY
                        | ISC_REQ_SEQUENCE_DETECT
                        | ISC_REQ_CONFIDENTIALITY
                        | ISC_REQ_ALLOCATE_MEMORY,
                    0,
                    SECURITY_NATIVE_DREP,
                    if count > 0 { &in_desc } else { std::ptr::null() },
                    0,
                    &mut next,
                    &mut out_desc,
                    &mut attributes,
                    &mut expiry,
                )
            };
            let out = if out_buffer.pvBuffer.is_null() {
                Vec::new()
            } else {
                let bytes = unsafe {
                    std::slice::from_raw_parts(
                        out_buffer.pvBuffer as *const u8,
                        out_buffer.cbBuffer as usize,
                    )
                }
                .to_vec();
                unsafe { FreeContextBuffer(out_buffer.pvBuffer) };
                bytes
            };
            if status != SEC_E_OK && status != SEC_I_CONTINUE_NEEDED {
                bail!(explain(&format!("SSPI refused ({status:#x})")));
            }
            self.context = Some(next);
            let done = status == SEC_E_OK;
            if done {
                ensure!(
                    attributes & ISC_RET_MUTUAL_AUTH != 0,
                    "The server didn't prove it is the service it claims to be (no Kerberos mutual authentication); not signed in."
                );
                ensure!(
                    attributes & ISC_RET_INTEGRITY != 0,
                    "The Kerberos sign-in has no integrity protection; not signed in."
                );
                self.established = true;
            }
            Ok((out, done))
        }
        fn unwrap(&mut self, message: &[u8]) -> Result<Vec<u8>> {
            let context = self.context.filter(|_| self.established);
            let Some(context) = context else {
                bail!("The Kerberos sign-in isn't complete.");
            };
            let mut data = message.to_vec();
            let mut buffers = [
                SecBuffer {
                    cbBuffer: data.len() as u32,
                    BufferType: SECBUFFER_STREAM,
                    pvBuffer: data.as_mut_ptr() as *mut _,
                },
                SecBuffer {
                    cbBuffer: 0,
                    BufferType: SECBUFFER_DATA,
                    pvBuffer: std::ptr::null_mut(),
                },
            ];
            let desc = SecBufferDesc {
                ulVersion: SECBUFFER_VERSION,
                cBuffers: 2,
                pBuffers: buffers.as_mut_ptr(),
            };
            let mut qop = 0;
            // SAFETY: decrypted in place, within `data`.
            let status = unsafe { DecryptMessage(&context, &desc, 0, &mut qop) };
            ensure!(
                status == SEC_E_OK,
                "The server's Kerberos message couldn't be read (SSPI {status:#x})."
            );
            let start = (buffers[1].pvBuffer as usize).checked_sub(data.as_ptr() as usize);
            let end = start.and_then(|s| s.checked_add(buffers[1].cbBuffer as usize));
            match (start, end) {
                (Some(s), Some(e)) if e <= data.len() => Ok(data[s..e].to_vec()),
                _ => bail!("The server's Kerberos message couldn't be read."),
            }
        }
        fn wrap(&mut self, message: &[u8]) -> Result<Vec<u8>> {
            let context = self.context.filter(|_| self.established);
            let Some(context) = context else {
                bail!("The Kerberos sign-in isn't complete.");
            };
            let mut sizes = SecPkgContext_Sizes::default();
            // SAFETY: SECPKG_ATTR_SIZES fills a SecPkgContext_Sizes.
            let status = unsafe {
                QueryContextAttributesW(&context, SECPKG_ATTR_SIZES, &mut sizes as *mut _ as *mut _)
            };
            ensure!(
                status == SEC_E_OK,
                "Picacho couldn't sign its Kerberos answer (SSPI {status:#x})."
            );
            let mut trailer = vec![0u8; sizes.cbSecurityTrailer as usize];
            let mut data = message.to_vec();
            let mut padding = vec![0u8; sizes.cbBlockSize as usize];
            let mut buffers = [
                SecBuffer {
                    cbBuffer: trailer.len() as u32,
                    BufferType: SECBUFFER_TOKEN,
                    pvBuffer: trailer.as_mut_ptr() as *mut _,
                },
                SecBuffer {
                    cbBuffer: data.len() as u32,
                    BufferType: SECBUFFER_DATA,
                    pvBuffer: data.as_mut_ptr() as *mut _,
                },
                SecBuffer {
                    cbBuffer: padding.len() as u32,
                    BufferType: SECBUFFER_PADDING,
                    pvBuffer: padding.as_mut_ptr() as *mut _,
                },
            ];
            let desc = SecBufferDesc {
                ulVersion: SECBUFFER_VERSION,
                cBuffers: 3,
                pBuffers: buffers.as_mut_ptr(),
            };
            // SAFETY: signed, not encrypted (TLS already encrypts).
            let status = unsafe { EncryptMessage(&context, SECQOP_WRAP_NO_ENCRYPT, &desc, 0) };
            ensure!(
                status == SEC_E_OK,
                "Picacho couldn't sign its Kerberos answer (SSPI {status:#x})."
            );
            let mut out = trailer[..buffers[0].cbBuffer as usize].to_vec();
            out.extend_from_slice(&data[..buffers[1].cbBuffer as usize]);
            out.extend_from_slice(&padding[..buffers[2].cbBuffer as usize]);
            Ok(out)
        }
    }
    impl Drop for Sspi {
        fn drop(&mut self) {
            // SAFETY: each handle is SSPI's, released once.
            unsafe {
                if let Some(context) = self.context.take() {
                    DeleteSecurityContext(&context);
                }
                FreeCredentialsHandle(&self.credentials);
            }
        }
    }
}

#[cfg(any(target_os = "android", target_os = "ios"))]
mod imp {
    use super::Context;
    use anyhow::{Result, bail};
    pub(super) fn start(_target: &str, _bindings: Option<Vec<u8>>) -> Result<Box<dyn Context>> {
        bail!("Kerberos sign-in isn't available on phones and tablets.")
    }
}

#[cfg(all(test, all(unix, not(any(target_os = "android", target_os = "ios")))))]
pub(crate) mod tests {
    //! Against a real Kerberos realm: a KDC started for the test in a
    //! temporary directory (MIT's krb5kdc, kdb5_util, kadmin.local, kinit).
    //! Opt-in: `cargo test -- --ignored kerberos`.
    use super::*;
    use std::process::Command;

    /// A throwaway realm with a user and an `imap/localhost` service, its
    /// environment set for this process. The KDC runs until dropped.
    pub(crate) struct Realm {
        kdc: std::process::Child,
        pub(crate) dir: std::path::PathBuf,
        // Kerberos reads its configuration from the process's environment:
        // one realm at a time.
        _alone: std::sync::MutexGuard<'static, ()>,
    }
    impl Drop for Realm {
        fn drop(&mut self) {
            let _ = self.kdc.kill();
            let _ = self.kdc.wait();
            let _ = std::fs::remove_dir_all(&self.dir);
        }
    }
    pub(crate) fn realm() -> Result<Realm> {
        use anyhow::ensure;
        static ONE: std::sync::Mutex<()> = std::sync::Mutex::new(());
        let alone = ONE.lock().unwrap_or_else(|e| e.into_inner());
        let dir = std::env::temp_dir().join(format!("picacho-krb-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir)?;
        let port = std::net::TcpListener::bind("127.0.0.1:0")?
            .local_addr()?
            .port();
        let d = dir.display();
        std::fs::write(
            dir.join("krb5.conf"),
            format!(
                "[libdefaults]\n default_realm = PICACHO.TEST\n dns_lookup_kdc = false\n dns_lookup_realm = false\n rdns = false\n dns_canonicalize_hostname = false\n qualify_shortname = \"\"\n default_ccache_name = FILE:{d}/ccache\n default_keytab_name = FILE:{d}/keytab\n[realms]\n PICACHO.TEST = {{\n  kdc = 127.0.0.1:{port}\n }}\n[domain_realm]\n localhost = PICACHO.TEST\n .localdomain = PICACHO.TEST\n localdomain = PICACHO.TEST\n"
            ),
        )?;
        std::fs::write(
            dir.join("kdc.conf"),
            format!(
                "[kdcdefaults]\n kdc_ports = {port}\n kdc_tcp_ports = {port}\n[realms]\n PICACHO.TEST = {{\n  database_name = {d}/principal\n  key_stash_file = {d}/stash\n  acl_file = {d}/kadm5.acl\n  kdc_ports = {port}\n  kdc_tcp_ports = {port}\n }}\n[logging]\n kdc = FILE:{d}/kdc.log\n"
            ),
        )?;
        std::fs::write(dir.join("kadm5.acl"), "")?;
        let env = |c: &mut Command| {
            c.env("KRB5_CONFIG", dir.join("krb5.conf"))
                .env("KRB5_KDC_PROFILE", dir.join("kdc.conf"))
                .env("KRB5CCNAME", format!("FILE:{d}/ccache"))
                .env("KRB5_KTNAME", format!("FILE:{d}/keytab"));
        };
        let run = |args: &[&str], input: Option<&str>| -> Result<()> {
            let mut c = Command::new(args[0]);
            c.args(&args[1..])
                .stdin(std::process::Stdio::piped())
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::piped());
            env(&mut c);
            let mut child = c.spawn()?;
            if let Some(text) = input {
                use std::io::Write;
                child.stdin.take().unwrap().write_all(text.as_bytes())?;
            }
            let out = child.wait_with_output()?;
            ensure!(
                out.status.success(),
                "{:?}: {}",
                args,
                String::from_utf8_lossy(&out.stderr)
            );
            Ok(())
        };
        run(
            &[
                "kdb5_util",
                "create",
                "-s",
                "-r",
                "PICACHO.TEST",
                "-P",
                "master-key",
            ],
            None,
        )?;
        run(
            &["kadmin.local", "-q", "addprinc -pw user-password ana"],
            None,
        )?;
        for service in ["imap", "pop", "smtp", "sieve"] {
            run(
                &[
                    "kadmin.local",
                    "-q",
                    &format!("addprinc -randkey {service}/localhost"),
                ],
                None,
            )?;
            run(
                &[
                    "kadmin.local",
                    "-q",
                    &format!("ktadd -k {d}/keytab {service}/localhost"),
                ],
                None,
            )?;
        }
        let mut kdc = Command::new("krb5kdc");
        kdc.args(["-n", "-r", "PICACHO.TEST"])
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null());
        env(&mut kdc);
        let kdc = kdc.spawn()?;
        let realm = Realm {
            kdc,
            dir: dir.clone(),
            _alone: alone,
        };
        // The KDC listening, then the user's ticket.
        for _ in 0..100 {
            if std::net::TcpStream::connect(("127.0.0.1", port)).is_ok() {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(50));
        }
        run(&["kinit", "ana@PICACHO.TEST"], Some("user-password\n"))?;
        // SAFETY: set before the library reads its configuration, in a
        // test that runs alone (`--test-threads=1` is not needed: the
        // library is loaded on first use, below).
        unsafe {
            std::env::set_var("KRB5_CONFIG", dir.join("krb5.conf"));
            std::env::set_var("KRB5CCNAME", format!("FILE:{d}/ccache"));
            std::env::set_var("KRB5_KTNAME", format!("FILE:{d}/keytab"));
        }
        Ok(realm)
    }

    #[test]
    #[ignore = "starts a Kerberos KDC; needs MIT krb5's server tools"]
    fn kerberos_client_context_with_a_real_kdc() -> Result<()> {
        let _realm = realm()?;
        let mut client = start("imap@localhost")?;
        let mut server = imp::acceptor::Acceptor::new()?;
        let (token, mut done) = client.step(&[])?;
        assert!(!token.is_empty());
        let (mut back, mut accepted) = server.step(&token)?;
        while !done {
            let (next, finished) = client.step(&back)?;
            done = finished;
            if !next.is_empty() && !accepted {
                (back, accepted) = server.step(&next)?;
            }
        }
        assert!(accepted, "the server accepted the ticket");
        // RFC 4752's security-layer exchange.
        let offer = server.wrap(&[0x07, 0x00, 0x10, 0x00])?;
        assert_eq!(client.unwrap(&offer)?, [0x07, 0x00, 0x10, 0x00]);
        let answer = client.wrap(&[0x01, 0, 0, 0])?;
        assert_eq!(server.unwrap(&answer)?, [0x01, 0, 0, 0]);
        // RFC 5801 §5: channel bindings (GS2's header and the TLS
        // binding) must be the same on both sides to be accepted.
        let bound = b"p=tls-server-end-point,,\x01\x02\x03".to_vec();
        for (theirs, accepted) in [(bound.clone(), true), (b"p=tls-server-end-point,,\x09".to_vec(), false)] {
            let mut client = start_bound("imap@localhost", bound.clone())?;
            let mut server = imp::acceptor::Acceptor::new()?;
            server.bindings = Some(theirs);
            let (token, _) = client.step(&[])?;
            let result = server.step(&token).and_then(|(back, _)| client.step(&back).map(|_| ()));
            assert_eq!(result.is_ok(), accepted, "{result:?}");
        }
        // GS2-KRB5 end to end (RFC 5801): the SASL exchange's first
        // message is its header and the unframed token; the server frames
        // it again (RFC 2743 §3.1) and accepts it with the same bindings.
        let binding = vec![0xAB; 32];
        let mut sasl = crate::sync::sasl::Exchange::new(
            crate::sync::sasl::Mechanism::Gs2Krb5Plus,
            crate::sync::sasl::Credentials {
                user: String::new(),
                secret: String::new(),
                host: "localhost".into(),
                port: 993,
                binding: binding.clone(),
                service: "imap",
            },
        );
        let first = sasl.initial().unwrap();
        let header = b"p=tls-server-end-point,,";
        let inner = first.strip_prefix(header.as_slice()).expect("the GS2 header");
        let mut oid = vec![0x06, 0x09, 0x2a, 0x86, 0x48, 0x86, 0xf7, 0x12, 0x01, 0x02, 0x02];
        oid.extend_from_slice(inner);
        let mut framed = vec![0x60];
        match oid.len() {
            n if n < 0x80 => framed.push(n as u8),
            n if n < 0x100 => framed.extend([0x81, n as u8]),
            n => framed.extend([0x82, (n >> 8) as u8, n as u8]),
        }
        framed.extend(oid);
        let mut server = imp::acceptor::Acceptor::new()?;
        server.bindings = Some([header.as_slice(), &binding].concat());
        let (back, accepted) = server.step(&framed)?;
        assert!(accepted);
        assert!(sasl.step(&back)?.is_empty());
        sasl.finish(None)?;
        // A service the realm doesn't have fails with Kerberos' own words.
        let mut wrong = start("imap@nowhere.invalid")?;
        let error = format!("{:#}", wrong.step(&[]).err().unwrap());
        assert!(error.contains("Kerberos sign-in failed"), "{error}");
        Ok(())
    }
}
