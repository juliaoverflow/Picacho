//! Picacho on Windows: what the other systems get from their desktop, their
//! bundle or their package, and the checks for the system DLLs that are
//! delay-loaded (see build.rs).

use std::path::Path;

use windows_sys::Win32::Foundation::FreeLibrary;
use windows_sys::Win32::System::LibraryLoader::{
    LOAD_LIBRARY_SEARCH_APPLICATION_DIR, LOAD_LIBRARY_SEARCH_SYSTEM32, LoadLibraryExW,
};

/// The AppUserModelID: groups Picacho's windows on the taskbar, names its
/// notifications and is what the installer's Start menu shortcut carries.
/// Matches the bundle identifier on macOS and the app ID on Linux.
pub const APP_ID: &str = "tech.pinal.picacho";

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

/// Whether `dll` can be loaded: from System32, or for WebView2's loader,
/// next to picacho.exe (where the installer puts it). A delay-loaded DLL
/// that can't be found ends the process when first called, so Picacho asks
/// first.
pub fn has_dll(dll: &str) -> bool {
    let name = wide(dll);
    // SAFETY: `name` is NUL-terminated and outlives the call; the module
    // is released again at once.
    unsafe {
        let module = LoadLibraryExW(
            name.as_ptr(),
            std::ptr::null_mut(),
            LOAD_LIBRARY_SEARCH_SYSTEM32 | LOAD_LIBRARY_SEARCH_APPLICATION_DIR,
        );
        if module.is_null() {
            return false;
        }
        FreeLibrary(module);
    }
    true
}

/// Windows Hello (webauthn.dll, Windows 10 1903 and later). Loaded from
/// System32 alone, and kept loaded: the delay-load helper, when Picacho
/// first calls into it, then finds this module rather than searching the
/// app's folder first, where a copy of Picacho run from Downloads would
/// load any webauthn.dll saved next to it.
pub fn has_windows_hello() -> bool {
    static LOADED: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *LOADED.get_or_init(|| {
        let name = wide("webauthn.dll");
        // SAFETY: `name` is NUL-terminated and outlives the call. The module
        // stays loaded for the process's life, on purpose.
        let module = unsafe {
            LoadLibraryExW(name.as_ptr(), std::ptr::null_mut(), LOAD_LIBRARY_SEARCH_SYSTEM32)
        };
        !module.is_null()
    })
}

/// Proton's human verification needs WebView2's loader (built into the
/// MSVC build, a DLL beside the app in the GNU one) and its runtime, which
/// Windows 11 has and Windows 10 gets with Edge or the Evergreen installer.
pub fn has_webview2() -> bool {
    if !cfg!(target_env = "msvc") && !has_dll("WebView2Loader.dll") {
        return false;
    }
    wry::webview_version().is_ok()
}

/// Called first in `main`: the process's AppUserModelID, so its windows,
/// tray icon and notifications are Picacho's rather than the console's or
/// PowerShell's.
pub fn set_app_identity() {
    use windows_sys::Win32::UI::Shell::SetCurrentProcessExplicitAppUserModelID;
    let id = wide(APP_ID);
    // SAFETY: `id` is NUL-terminated and outlives the call.
    unsafe {
        SetCurrentProcessExplicitAppUserModelID(id.as_ptr());
    }
    register_app_id();
}

/// Names the AppUserModelID in the registry (under the current user, as
/// GPUI does), so notifications show "Picacho" and its icon even when
/// Picacho wasn't installed with a Start menu shortcut.
fn register_app_id() {
    let key = format!(r"Software\Classes\AppUserModelId\{APP_ID}");
    let Ok(key) = windows_registry::CURRENT_USER.create(key) else {
        return;
    };
    let _ = key.set_string("DisplayName", "Picacho");
    if let Ok(exe) = std::env::current_exe() {
        // The icon resource built into picacho.exe (see build.rs).
        let _ = key.set_string("IconUri", exe.to_string_lossy().as_ref());
    }
}

/// The registry key whose command says which picacho.exe the registration
/// below is for.
const MAILTO_COMMAND: &str = r"Software\Classes\Picacho.mailto\shell\open\command";

/// The `shell\open\command` value for this executable.
fn open_command() -> Option<String> {
    let exe = std::env::current_exe().ok()?;
    Some(format!("\"{}\" \"%1\"", exe.to_string_lossy()))
}

/// The executable an `"exe" "%1"` command runs.
fn command_exe(command: &str) -> Option<std::path::PathBuf> {
    let rest = command.trim().strip_prefix('"')?;
    Some(std::path::PathBuf::from(&rest[..rest.find('"')?]))
}

/// Registers Picacho for Windows' Default apps (see `register_default_app`)
/// when it starts, unless it is already registered for this executable, or
/// another picacho.exe that still exists is (an installed copy keeps its
/// registration when a portable or development build runs). Runs on a
/// thread of its own: it only writes when something changed.
pub fn register_default_app_if_unclaimed() {
    std::thread::spawn(|| {
        let Some(ours) = open_command() else { return };
        let current = windows_registry::CURRENT_USER
            .open(MAILTO_COMMAND)
            .and_then(|key| key.get_string(""))
            .ok();
        let claimed_elsewhere = current.as_deref().is_some_and(|c| {
            !c.eq_ignore_ascii_case(&ours) && command_exe(c).is_some_and(|exe| exe.is_file())
        });
        if current.as_deref().is_some_and(|c| c.eq_ignore_ascii_case(&ours)) || claimed_elsewhere {
            return;
        }
        register_default_app();
    });
}

/// Registers Picacho under HKCU for `mailto:` links, `picacho:` widget links,
/// `.eml` and `.msg` files and Windows' Default apps list, so MSI installs and
/// portable builds alike can be chosen there without administrator rights
/// (Windows asks the person, never Picacho, to make it the default).
pub fn register_default_app() {
    let Some(command) = open_command() else {
        return;
    };
    let Ok(exe) = std::env::current_exe() else {
        return;
    };
    let icon = format!("{},0", exe.to_string_lossy());

    let set = |path: &str, name: &str, value: &str| {
        if let Ok(key) = windows_registry::CURRENT_USER.create(path) {
            let _ = key.set_string(name, value);
        }
    };

    set(r"Software\Classes\Picacho.mailto", "", "URL:MailTo Protocol");
    set(r"Software\Classes\Picacho.mailto", "URL Protocol", "");
    set(r"Software\Classes\Picacho.mailto\DefaultIcon", "", &icon);
    set(MAILTO_COMMAND, "", &command);

    // picacho:// links (the Widgets Board's taps, OAuth's return).
    set(r"Software\Classes\picacho", "", "URL:Picacho Protocol");
    set(r"Software\Classes\picacho", "URL Protocol", "");
    set(r"Software\Classes\picacho\DefaultIcon", "", &icon);
    set(r"Software\Classes\picacho\shell\open\command", "", &command);

    set(r"Software\Classes\Picacho.eml", "", "Email Message");
    set(r"Software\Classes\Picacho.eml\DefaultIcon", "", &icon);
    set(r"Software\Classes\Picacho.eml\shell\open\command", "", &command);
    set(r"Software\Classes\.eml\OpenWithProgids", "Picacho.eml", "");

    set(r"Software\Classes\Picacho.msg", "", "Outlook Email Message");
    set(r"Software\Classes\Picacho.msg\DefaultIcon", "", &icon);
    set(r"Software\Classes\Picacho.msg\shell\open\command", "", &command);
    set(r"Software\Classes\.msg\OpenWithProgids", "Picacho.msg", "");

    set(r"Software\Pinal Tech\Picacho\Capabilities", "ApplicationName", "Picacho");
    set(
        r"Software\Pinal Tech\Picacho\Capabilities",
        "ApplicationDescription",
        "Fast, native email for every inbox you have",
    );
    set(
        r"Software\Pinal Tech\Picacho\Capabilities\URLAssociations",
        "mailto",
        "Picacho.mailto",
    );
    set(
        r"Software\Pinal Tech\Picacho\Capabilities\FileAssociations",
        ".eml",
        "Picacho.eml",
    );
    set(
        r"Software\Pinal Tech\Picacho\Capabilities\FileAssociations",
        ".msg",
        "Picacho.msg",
    );
    set(r"Software\Clients\Mail\Picacho", "", "Picacho");
    set(r"Software\Clients\Mail\Picacho\Capabilities", "ApplicationName", "Picacho");
    set(
        r"Software\RegisteredApplications",
        "Picacho",
        r"Software\Pinal Tech\Picacho\Capabilities",
    );

    use windows_sys::Win32::UI::Shell::{SHCNE_ASSOCCHANGED, SHCNF_IDLIST, SHChangeNotify};
    // SAFETY: notifying Explorer with SHCNF_IDLIST and null items is documented safe.
    unsafe {
        SHChangeNotify(
            SHCNE_ASSOCCHANGED as i32,
            SHCNF_IDLIST,
            std::ptr::null(),
            std::ptr::null(),
        );
    }
}

/// Whether Picacho is currently Windows' chosen handler for `mailto:` links.
pub fn is_default_mail_app() -> bool {
    windows_registry::CURRENT_USER
        .open(r"Software\Microsoft\Windows\Shell\Associations\UrlAssociations\mailto\UserChoice")
        .ok()
        .and_then(|key| key.get_string("ProgId").ok())
        .is_some_and(|id| id.eq_ignore_ascii_case("Picacho.mailto"))
}

/// Opens Windows Settings directly to Picacho's Default apps page (where
/// Windows 11 offers a single "Set default" button for `mailto:`, `.eml` and
/// `.msg`), falling back to the top-level Default apps page on older builds.
pub fn open_default_apps() -> anyhow::Result<()> {
    use windows_sys::Win32::UI::Shell::ShellExecuteW;
    use windows_sys::Win32::UI::WindowsAndMessaging::SW_SHOWNORMAL;

    register_default_app();
    let verb = wide("open");
    for uri in [
        "ms-settings:defaultapps?registeredAppUser=Picacho",
        "ms-settings:defaultapps",
    ] {
        let target = wide(uri);
        // SAFETY: `verb` and `target` are NUL-terminated and outlive the call.
        let result = unsafe {
            ShellExecuteW(
                std::ptr::null_mut(),
                verb.as_ptr(),
                target.as_ptr(),
                std::ptr::null(),
                std::ptr::null(),
                SW_SHOWNORMAL,
            )
        };
        if result as isize > 32 {
            return Ok(());
        }
    }
    anyhow::bail!("Could not open Windows Default apps settings.")
}

/// "Show animations in Windows" (Settings → Accessibility → Visual
/// effects), asked of the system rather than read from the registry.
pub fn animations_off() -> bool {
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        SPI_GETCLIENTAREAANIMATION, SystemParametersInfoW,
    };
    let mut on: windows_sys::core::BOOL = 1;
    // SAFETY: SPI_GETCLIENTAREAANIMATION writes one BOOL to `on`.
    let asked =
        unsafe { SystemParametersInfoW(SPI_GETCLIENTAREAANIMATION, 0, (&raw mut on).cast(), 0) };
    asked != 0 && on == 0
}

/// Whether Windows' high contrast (contrast themes) is on.
pub fn high_contrast() -> bool {
    use windows_sys::Win32::UI::Accessibility::{HCF_HIGHCONTRASTON, HIGHCONTRASTW};
    use windows_sys::Win32::UI::WindowsAndMessaging::{SPI_GETHIGHCONTRAST, SystemParametersInfoW};
    let mut hc = HIGHCONTRASTW { cbSize: std::mem::size_of::<HIGHCONTRASTW>() as u32, ..Default::default() };
    // SAFETY: SPI_GETHIGHCONTRAST fills the HIGHCONTRASTW whose size it is told.
    let asked = unsafe { SystemParametersInfoW(SPI_GETHIGHCONTRAST, hc.cbSize, (&raw mut hc).cast(), 0) };
    asked != 0 && hc.dwFlags & HCF_HIGHCONTRASTON != 0
}

/// Limits `path` to the current user (and SYSTEM, which Windows' own
/// services need), as the data directory's 0700 does on Unix: its access
/// list stops inheriting from the parent, which on a roaming or shared
/// profile may let other accounts read mail.
pub fn restrict_to_user(path: &Path) -> std::io::Result<()> {
    use windows_sys::Win32::Foundation::{ERROR_SUCCESS, LocalFree};
    use windows_sys::Win32::Security::Authorization::{
        ConvertStringSecurityDescriptorToSecurityDescriptorW, SDDL_REVISION_1, SE_FILE_OBJECT,
        SetNamedSecurityInfoW,
    };
    use windows_sys::Win32::Security::{
        DACL_SECURITY_INFORMATION, GetSecurityDescriptorDacl, PROTECTED_DACL_SECURITY_INFORMATION,
        PSECURITY_DESCRIPTOR,
    };
    // Full control for the owner (the user who made it) and SYSTEM, for
    // the folder and everything made in it; nobody else.
    let sddl = wide("D:P(A;OICI;FA;;;OW)(A;OICI;FA;;;SY)");
    let target = wide(&path.to_string_lossy());
    let mut descriptor: PSECURITY_DESCRIPTOR = std::ptr::null_mut();
    // SAFETY: the strings are NUL-terminated and outlive the calls; the
    // descriptor Windows allocates is freed with LocalFree, and the DACL
    // points into it.
    unsafe {
        if ConvertStringSecurityDescriptorToSecurityDescriptorW(
            sddl.as_ptr(),
            SDDL_REVISION_1,
            &mut descriptor,
            std::ptr::null_mut(),
        ) == 0
        {
            return Err(std::io::Error::last_os_error());
        }
        let mut present = 0;
        let mut dacl = std::ptr::null_mut();
        let mut defaulted = 0;
        let found = GetSecurityDescriptorDacl(descriptor, &mut present, &mut dacl, &mut defaulted);
        let result = if found == 0 || present == 0 {
            Err(std::io::Error::last_os_error())
        } else {
            match SetNamedSecurityInfoW(
                target.as_ptr(),
                SE_FILE_OBJECT,
                DACL_SECURITY_INFORMATION | PROTECTED_DACL_SECURITY_INFORMATION,
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                dacl,
                std::ptr::null(),
            ) {
                ERROR_SUCCESS => Ok(()),
                e => Err(std::io::Error::from_raw_os_error(e as i32)),
            }
        };
        LocalFree(descriptor);
        result
    }
}

/// The real names of the webauthn.dll functions webauthn-authenticator-rs
/// calls. On the GNU target, build.rs renames its calls (`--wrap`) to
/// delay-loaded imports; lld still resolves each original name, and found
/// in the `windows` crate's library, that would import webauthn.dll the
/// usual way. Defined here, nothing does. They are never called.
#[cfg(not(target_env = "msvc"))]
mod webauthn_names {
    #[unsafe(no_mangle)]
    extern "system" fn WebAuthNAuthenticatorGetAssertion() {
        unreachable!("webauthn.dll is called through its delay-loaded import")
    }
    #[unsafe(no_mangle)]
    extern "system" fn WebAuthNAuthenticatorMakeCredential() {
        unreachable!("webauthn.dll is called through its delay-loaded import")
    }
    #[unsafe(no_mangle)]
    extern "system" fn WebAuthNFreeAssertion() {
        unreachable!("webauthn.dll is called through its delay-loaded import")
    }
    #[unsafe(no_mangle)]
    extern "system" fn WebAuthNFreeCredentialAttestation() {
        unreachable!("webauthn.dll is called through its delay-loaded import")
    }
    #[unsafe(no_mangle)]
    extern "system" fn WebAuthNGetApiVersionNumber() {
        unreachable!("webauthn.dll is called through its delay-loaded import")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn system_dlls_are_found_and_missing_ones_are_not() {
        assert!(has_dll("kernel32.dll"));
        assert!(!has_dll("picacho-no-such-library.dll"));
    }

    #[test]
    fn the_data_directory_is_the_users_alone() -> std::io::Result<()> {
        let dir = std::env::temp_dir().join(format!("picacho-acl-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir)?;
        restrict_to_user(&dir)?;
        // Still usable by its owner, as is what's made in it.
        std::fs::write(dir.join("file"), b"mail")?;
        assert_eq!(std::fs::read(dir.join("file"))?, b"mail");
        std::fs::remove_dir_all(&dir)
    }
}
