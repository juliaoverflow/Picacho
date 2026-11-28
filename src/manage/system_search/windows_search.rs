//! Windows Search: one small `.eml` file per message (`super::eml`) in a
//! folder of Picacho's under the local application data folder (never the
//! roaming one, which would carry them to every computer signed in), added
//! to Windows Search's indexed locations through its crawl scope manager.
//!
//! Windows indexes `.eml` files with its own MIME filter: their subject,
//! people, date and text are found from the Start menu and File Explorer.
//! A result opens with the system's app for email files; with Picacho,
//! it opens the message it stands for (`super::link_for_file`).
//!
//! Adding the folder needs Windows Search's service. Where it isn't
//! running, or doesn't let Picacho add a location (some managed
//! computers), the files are still kept, and Settings says how to add
//! the folder in Indexing Options.
use super::Change;
use anyhow::{Context as _, Result};
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};
use windows::Win32::Foundation::{E_ACCESSDENIED, REGDB_E_CLASSNOTREG};
use windows::Win32::System::Com::{
    CLSCTX_LOCAL_SERVER, COINIT_MULTITHREADED, CoCreateInstance, CoInitializeEx, CoUninitialize,
};
use windows::Win32::System::Search::{
    CSearchManager, FF_INDEXCOMPLEXURLS, ISearchCrawlScopeManager, ISearchManager,
};
use windows::core::{HSTRING, w};

/// COM, on the worker's thread, for as long as it runs.
pub struct Apartment(bool);

impl Apartment {
    pub fn new() -> Self {
        // SAFETY: paired with CoUninitialize in Drop when it succeeded.
        Self(unsafe { CoInitializeEx(None, COINIT_MULTITHREADED) }.is_ok())
    }
}

impl Drop for Apartment {
    fn drop(&mut self) {
        if self.0 {
            // SAFETY: CoInitializeEx succeeded on this thread.
            unsafe { CoUninitialize() };
        }
    }
}

/// This store's folder of files for Windows Search (`init`).
static FOLDER: OnceLock<PathBuf> = OnceLock::new();
/// Why the folder isn't in Windows Search's indexed locations, when it
/// couldn't be added; cleared once it is in them.
static WARNING: Mutex<Option<String>> = Mutex::new(None);
/// The folder was found in (or added to) the indexed locations by this process.
static SCOPED: Mutex<bool> = Mutex::new(false);

/// The folder for the store in `store_dir`: one of its own under the
/// local application data folder, named by a digest of the store's path,
/// so that two stores (another `--data-dir`) never share one.
pub fn folder_for(store_dir: &Path) -> Option<PathBuf> {
    use sha2::Digest as _;
    let local = directories::ProjectDirs::from("tech", "pinal", "picacho")?.data_local_dir().to_owned();
    let canonical = std::fs::canonicalize(store_dir).unwrap_or_else(|_| store_dir.to_owned());
    let digest = sha2::Sha256::digest(canonical.to_string_lossy().to_lowercase().as_bytes());
    let tag: String = digest[..6].iter().map(|b| format!("{b:02x}")).collect();
    Some(local.join("Search").join(tag))
}

/// Settles the folder for the store in `store_dir` (the first call wins).
pub fn init(store_dir: &Path) {
    if let Some(folder) = folder_for(store_dir) {
        let _ = FOLDER.set(folder);
    }
}

/// The folder of files for Windows Search, once `init` settled it.
pub fn folder() -> Option<&'static Path> {
    FOLDER.get().map(PathBuf::as_path)
}

fn folder_or_err() -> Result<&'static Path> {
    folder().ok_or_else(|| anyhow::anyhow!(t!("manage-system-search-no-folder")))
}

/// Why the files aren't being indexed, if they aren't.
pub fn warning() -> Option<String> {
    WARNING.lock().unwrap_or_else(|e| e.into_inner()).clone()
}

/// Windows Search's address of `folder` (`file:///C:\…\`).
fn scope_url(folder: &Path) -> HSTRING {
    let mut path = folder.to_string_lossy().into_owned();
    if !path.ends_with('\\') {
        path.push('\\');
    }
    HSTRING::from(format!("file:///{path}"))
}

/// The system index's crawl scope manager.
fn scope_manager() -> windows::core::Result<ISearchCrawlScopeManager> {
    // SAFETY: COM calls on a thread in COM (`Apartment`); the interfaces
    // are released as they go out of scope.
    unsafe {
        let manager: ISearchManager = CoCreateInstance(&CSearchManager, None, CLSCTX_LOCAL_SERVER)?;
        let catalog = manager.GetCatalog(w!("SystemIndex"))?;
        catalog.GetCrawlScopeManager()
    }
}

/// Puts `folder` in the indexed locations, if it isn't there yet.
fn add_scope(folder: &Path) -> windows::core::Result<()> {
    let url = scope_url(folder);
    let scope = scope_manager()?;
    // SAFETY: as in `scope_manager`; `url` outlives the calls.
    unsafe {
        if scope.IncludedInCrawlScope(&url)?.as_bool() {
            return Ok(());
        }
        scope.AddUserScopeRule(&url, true, false, FF_INDEXCOMPLEXURLS.0 as u32)?;
        scope.SaveAll()
    }
}

/// What Settings says when the folder couldn't be added.
fn scope_warning(folder: &Path, e: &windows::core::Error) -> String {
    let path = folder.display().to_string();
    if e.code() == REGDB_E_CLASSNOTREG || e.code().0 as u32 == 0x8008_0005 || e.code().0 as u32 == 0x8007_0422 {
        // Not installed, or its service is off (CO_E_SERVER_EXEC_FAILURE,
        // ERROR_SERVICE_DISABLED).
        t!("manage-system-search-windows-search-off", path = path).to_owned()
    } else if e.code() == E_ACCESSDENIED {
        t!("manage-system-search-windows-search-denied", path = path).to_owned()
    } else {
        t!("manage-system-search-windows-search-failed", path = path, e = e.message()).to_owned()
    }
}

/// Makes the folder (this user's alone) and puts it in Windows Search's
/// indexed locations.
pub fn prepare() -> Result<()> {
    let folder = folder_or_err()?;
    std::fs::create_dir_all(folder)
        .with_context(|| t!("manage-system-search-couldnt-make-folder", path = folder.display().to_string()))?;
    if let Err(e) = crate::display::windows::restrict_to_user(folder) {
        eprintln!("Picacho: couldn't limit the system search folder to this user: {e}");
    }
    let mut scoped = SCOPED.lock().unwrap_or_else(|e| e.into_inner());
    if !*scoped {
        match add_scope(folder) {
            Ok(()) => {
                *scoped = true;
                *WARNING.lock().unwrap_or_else(|e| e.into_inner()) = None;
            }
            // The files are kept all the same: added by hand in
            // Indexing Options, they are found.
            Err(e) => {
                eprintln!("Picacho: Windows Search didn't add {}: {e}", folder.display());
                *WARNING.lock().unwrap_or_else(|e| e.into_inner()) = Some(scope_warning(folder, &e));
            }
        }
    }
    Ok(())
}

/// Writes, renames and removes the files `changes` call for. Returns the
/// name each message now has there (`None`: none).
pub fn apply(changes: &[Change]) -> Result<Vec<Option<String>>> {
    let folder = folder_or_err()?;
    let mut names = Vec::with_capacity(changes.len());
    for change in changes {
        let before = change.listed.as_ref().map(|(_, name)| name.as_str());
        let Some(entry) = &change.entry else {
            if let Some(name) = before {
                remove(&folder.join(name))?;
            }
            names.push(None);
            continue;
        };
        let name = super::file_name(entry);
        let path = folder.join(&name);
        write(&path, &super::eml(entry), entry.message.date)?;
        if let Some(old) = before.filter(|old| *old != name) {
            remove(&folder.join(old))?;
        }
        names.push(Some(name));
    }
    Ok(names)
}

/// Writes one file whole (beside it first, then renamed over it, so the
/// indexer never reads half of one), dated as its message.
fn write(path: &Path, bytes: &[u8], date: i64) -> Result<()> {
    use std::io::Write as _;
    let partial = path.with_extension("partial");
    {
        let mut file = std::fs::File::create(&partial)?;
        file.write_all(bytes)?;
        // Search results sort and show by date: the message's.
        if let Some(when) = u64::try_from(date)
            .ok()
            .filter(|d| *d > 0)
            .and_then(|d| std::time::UNIX_EPOCH.checked_add(std::time::Duration::from_secs(d)))
        {
            let _ = file.set_modified(when);
        }
    }
    std::fs::rename(&partial, path).inspect_err(|_| {
        let _ = std::fs::remove_file(&partial);
    })?;
    Ok(())
}

fn remove(path: &Path) -> Result<()> {
    match std::fs::remove_file(path) {
        Err(e) if e.kind() != std::io::ErrorKind::NotFound => Err(e.into()),
        _ => Ok(()),
    }
}

/// Whether files from before are still there (to be removed).
pub fn has_leftovers() -> bool {
    folder().is_some_and(Path::exists)
}

/// Removes every file, and the folder from the indexed locations.
pub fn clear() -> Result<()> {
    let Some(folder) = folder() else {
        return Ok(());
    };
    // Out of the indexed locations first: removing the files then takes
    // them out of the index at once, and nothing is indexed again.
    // Best effort: a location left behind holds nothing.
    let url = scope_url(folder);
    if let Ok(scope) = scope_manager() {
        // SAFETY: as in `scope_manager`; `url` outlives the calls.
        unsafe {
            if scope.RemoveScopeRule(&url).is_ok() {
                let _ = scope.SaveAll();
            }
        }
    }
    *SCOPED.lock().unwrap_or_else(|e| e.into_inner()) = false;
    *WARNING.lock().unwrap_or_else(|e| e.into_inner()) = None;
    match std::fs::remove_dir_all(folder) {
        Err(e) if e.kind() != std::io::ErrorKind::NotFound => {
            Err(e).with_context(|| t!("manage-system-search-couldnt-remove-folder", path = folder.display().to_string()))
        }
        _ => Ok(()),
    }
}
