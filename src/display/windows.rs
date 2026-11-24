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
