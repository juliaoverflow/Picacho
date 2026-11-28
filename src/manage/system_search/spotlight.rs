//! Spotlight: each message as a Core Spotlight item in Picacho's own index
//! (`CSSearchableIndex.defaultSearchableIndex`), named by its
//! `picacho://widget/message` link, in a domain named by its account.
//!
//! Choosing one in Spotlight opens Picacho (or brings it forward) with a
//! user activity whose identifier is that link. On macOS the application
//! delegate passes it on (`handle_activities`): to the window's process, or
//! to the background one, which opens the window with it. On iOS the scene
//! does (packaging/ios/main.m).
//!
//! Core Spotlight is loaded when first needed, so builds need no more
//! than the frameworks every Mac has. It works for an app bundle (with a
//! bundle identifier); elsewhere (`cargo run`) it says it isn't available.
use super::Change;
use anyhow::Result;
use block2::RcBlock;
use objc2::rc::{Allocated, Retained, autoreleasepool};
use objc2::runtime::AnyClass;
use objc2::runtime::AnyObject;
#[cfg(target_os = "macos")]
use objc2::runtime::{Bool, Imp, Sel};
use objc2::msg_send;
#[cfg(target_os = "macos")]
use objc2::{ClassType as _, Encode as _, sel};
use objc2_foundation::{NSArray, NSError, NSString};
use std::ffi::CStr;
#[cfg(target_os = "macos")]
use std::ffi::CString;
use std::sync::OnceLock;
use std::time::Duration;

/// How long a call into Core Spotlight is waited for.
const WAIT: Duration = Duration::from_secs(60);
/// The content type of email messages (`UTTypeEmailMessage`).
const EMAIL_TYPE: &str = "public.email-message";
/// The activity Spotlight opens an app with for one of its items
/// (`CSSearchableItemActionType`), and the key of the item's identifier in
/// its user info (`CSSearchableItemActivityIdentifier`), as Core Spotlight
/// has them, or as documented where it can't be asked.
#[cfg(target_os = "macos")]
const ACTION_TYPE: &str = "com.apple.corespotlightitem";
#[cfg(target_os = "macos")]
const ACTIVITY_IDENTIFIER: &str = "kCSSearchableItemActivityIdentifier";

/// Core Spotlight, loaded once and kept loaded.
fn framework() -> Option<&'static libloading::Library> {
    static FRAMEWORK: OnceLock<Option<libloading::Library>> = OnceLock::new();
    FRAMEWORK
        .get_or_init(|| {
            // SAFETY: a system framework, whose initializers only register
            // its Objective-C classes.
            unsafe { libloading::Library::new("/System/Library/Frameworks/CoreSpotlight.framework/CoreSpotlight").ok() }
        })
        .as_ref()
}

/// An `NSString *const` Core Spotlight exports, as a Rust string.
#[cfg(target_os = "macos")]
fn constant(name: &CStr, fallback: &str) -> String {
    let found = framework().and_then(|lib| {
        // SAFETY: the symbol is an `NSString *const` (a pointer to an
        // immutable string object that lives as long as the framework).
        unsafe {
            let symbol = lib.get::<*const *const NSString>(name.to_bytes_with_nul()).ok()?;
            let string = (**symbol).as_ref()?;
            Some(string.to_string())
        }
    });
    found.unwrap_or_else(|| fallback.to_owned())
}

fn class(name: &CStr) -> Result<&'static AnyClass> {
    framework();
    AnyClass::get(name).ok_or_else(|| anyhow::anyhow!(t!("manage-system-search-spotlight-unavailable")))
}

/// Picacho's searchable index, where this Mac can index.
fn index() -> Result<Retained<AnyObject>> {
    let cls = class(c"CSSearchableIndex")?;
    // SAFETY: class methods of CSSearchableIndex, as declared in
    // <CoreSpotlight/CSSearchableIndex.h>.
    unsafe {
        let available: bool = msg_send![cls, isIndexingAvailable];
        if !available {
            anyhow::bail!(t!("manage-system-search-spotlight-unavailable"));
        }
        let index: Option<Retained<AnyObject>> = msg_send![cls, defaultSearchableIndex];
        index.ok_or_else(|| anyhow::anyhow!(t!("manage-system-search-spotlight-unavailable")))
    }
}

/// Calls `call` with a completion handler, and waits for it.
fn wait(call: impl FnOnce(&block2::Block<dyn Fn(*mut NSError)>)) -> Result<()> {
    let (tx, rx) = std::sync::mpsc::sync_channel::<Option<String>>(1);
    let done = RcBlock::new(move |error: *mut NSError| {
        // SAFETY: Core Spotlight passes a valid NSError or nil.
        let message = unsafe { error.as_ref() }.map(|e| e.localizedDescription().to_string());
        let _ = tx.try_send(message);
    });
    call(&done);
    match rx.recv_timeout(WAIT) {
        Ok(None) => Ok(()),
        Ok(Some(message)) => Err(anyhow::anyhow!(t!("manage-system-search-spotlight-said", e = message))),
        Err(_) => Err(anyhow::anyhow!(t!("manage-system-search-spotlight-didnt-answer"))),
    }
}

fn strings(values: &[String]) -> Retained<NSArray<NSString>> {
    let values: Vec<Retained<NSString>> = values.iter().map(|v| NSString::from_str(v)).collect();
    NSArray::from_retained_slice(&values)
}

fn date(seconds: f64) -> Option<Retained<AnyObject>> {
    let cls = AnyClass::get(c"NSDate")?;
    // SAFETY: +[NSDate dateWithTimeIntervalSince1970:] returns a date.
    unsafe { msg_send![cls, dateWithTimeIntervalSince1970: seconds] }
}

/// One message as a searchable item.
fn item(entry: &super::Entry) -> Option<Retained<AnyObject>> {
    let m = &entry.message;
    let set_cls = AnyClass::get(c"CSSearchableItemAttributeSet")?;
    let item_cls = AnyClass::get(c"CSSearchableItem")?;
    // SAFETY: CSSearchableItemAttributeSet's and CSSearchableItem's
    // initializers and properties, as declared in Core Spotlight's headers
    // (macOS 10.13 and later); every argument is an object of the declared
    // class, or nil where it may be.
    unsafe {
        let set: Allocated<AnyObject> = msg_send![set_cls, alloc];
        let set: Option<Retained<AnyObject>> =
            msg_send![set, initWithItemContentType: &*NSString::from_str(EMAIL_TYPE)];
        let set = set?;
        let subject = m.subject.trim();
        let title = if subject.is_empty() { t!("manage-system-search-no-subject") } else { subject };
        let _: () = msg_send![&*set, setTitle: &*NSString::from_str(title)];
        let _: () = msg_send![&*set, setDisplayName: &*NSString::from_str(title)];
        let summary: String = m.text.split_whitespace().take(60).collect::<Vec<_>>().join(" ");
        let _: () = msg_send![&*set, setContentDescription: &*NSString::from_str(&summary)];
        let _: () = msg_send![&*set, setTextContent: &*NSString::from_str(&m.text)];
        let author = super::person(&m.sender, &m.from);
        let (mut author_names, author_addresses) = super::people(&author);
        if author_names.is_empty() {
            author_names = author_addresses.clone();
        }
        let _: () = msg_send![&*set, setAuthorNames: &*strings(&author_names)];
        let _: () = msg_send![&*set, setAuthorEmailAddresses: &*strings(&author_addresses)];
        let recipients = [m.to.as_str(), m.cc.as_str()].join(", ");
        let (recipient_names, recipient_addresses) = super::people(&recipients);
        let _: () = msg_send![&*set, setRecipientNames: &*strings(&recipient_names)];
        let _: () = msg_send![&*set, setRecipientEmailAddresses: &*strings(&recipient_addresses)];
        let everyone: Vec<String> = author_addresses.iter().chain(&recipient_addresses).cloned().collect();
        let _: () = msg_send![&*set, setEmailAddresses: &*strings(&everyone)];
        if let Some(when) = date(m.date as f64) {
            let _: () = msg_send![&*set, setContentCreationDate: &*when];
            let _: () = msg_send![&*set, setContentModificationDate: &*when];
        }
        let item: Allocated<AnyObject> = msg_send![item_cls, alloc];
        let item: Option<Retained<AnyObject>> = msg_send![
            item,
            initWithUniqueIdentifier: &*NSString::from_str(&entry.link),
            domainIdentifier: &*NSString::from_str(&m.account),
            attributeSet: &*set
        ];
        let item = item?;
        // Items expire after a month unless told otherwise: mail stays as
        // long as it's here.
        if let Some(cls) = AnyClass::get(c"NSDate") {
            let never: Option<Retained<AnyObject>> = msg_send![cls, distantFuture];
            if let Some(never) = never {
                let _: () = msg_send![&*item, setExpirationDate: &*never];
            }
        }
        Some(item)
    }
}

/// Nothing to make ready: Core Spotlight is asked as items are given.
pub fn prepare() -> Result<()> {
    index().map(drop)
}

/// Gives Spotlight the messages `changes` call for, and takes the others
/// out. Returns the name (link) each message now has there.
pub fn apply(changes: &[Change]) -> Result<Vec<Option<String>>> {
    autoreleasepool(|_| {
        let index = index()?;
        let mut names = Vec::with_capacity(changes.len());
        let mut gone: Vec<String> = Vec::new();
        let mut items: Vec<Retained<AnyObject>> = Vec::new();
        for change in changes {
            let before = change.listed.as_ref().map(|(_, name)| name.clone());
            match change.entry.as_ref().and_then(|e| Some((e, item(e)?))) {
                Some((entry, item)) => {
                    // Named anew (moved to another account): the old one goes.
                    if let Some(old) = before.filter(|old| *old != entry.link) {
                        gone.push(old);
                    }
                    items.push(item);
                    names.push(Some(entry.link.clone()));
                }
                None => {
                    gone.extend(before);
                    names.push(None);
                }
            }
        }
        if !gone.is_empty() {
            let ids = strings(&gone);
            // SAFETY: -[CSSearchableIndex deleteSearchableItemsWithIdentifiers:completionHandler:].
            wait(|done| unsafe {
                let _: () = msg_send![&*index, deleteSearchableItemsWithIdentifiers: &*ids, completionHandler: done];
            })?;
        }
        if !items.is_empty() {
            let items = NSArray::from_retained_slice(&items);
            // SAFETY: -[CSSearchableIndex indexSearchableItems:completionHandler:].
            wait(|done| unsafe {
                let _: () = msg_send![&*index, indexSearchableItems: &*items, completionHandler: done];
            })?;
        }
        Ok(names)
    })
}

/// Takes all of Picacho's mail out of Spotlight.
pub fn clear() -> Result<()> {
    autoreleasepool(|_| {
        let index = index()?;
        // SAFETY: -[CSSearchableIndex deleteAllSearchableItemsWithCompletionHandler:].
        wait(|done| unsafe {
            let _: () = msg_send![&*index, deleteAllSearchableItemsWithCompletionHandler: done];
        })
    })
}

/// Spotlight's index isn't looked through for what's left: what Picacho
/// listed is in the store.
pub fn has_leftovers() -> bool {
    false
}

pub fn warning() -> Option<String> {
    None
}

#[cfg(target_os = "macos")]
/// What a Spotlight result chosen opens (`handle_activities`).
static OPEN: OnceLock<fn(String)> = OnceLock::new();
#[cfg(target_os = "macos")]
/// The delegate's own handler of other activities, if it had one.
static PREVIOUS: OnceLock<Imp> = OnceLock::new();

#[cfg(target_os = "macos")]
type ContinueActivity =
    unsafe extern "C-unwind" fn(*mut AnyObject, Sel, *mut AnyObject, *mut AnyObject, *mut AnyObject) -> Bool;

#[cfg(target_os = "macos")]
/// The link of the message a Spotlight result stands for, when `activity`
/// is a Spotlight result's.
///
/// # Safety
/// `activity` is an `NSUserActivity` or null.
unsafe fn link_of(activity: *mut AnyObject) -> Option<String> {
    // SAFETY: NSUserActivity's `activityType` and `userInfo`, and
    // NSDictionary's `objectForKey:`; the value is checked to be a string.
    unsafe {
        let activity = activity.as_ref()?;
        let kind: Option<Retained<NSString>> = msg_send![activity, activityType];
        static ACTION: OnceLock<String> = OnceLock::new();
        let action = ACTION.get_or_init(|| constant(c"CSSearchableItemActionType", ACTION_TYPE));
        if kind?.to_string() != *action {
            return None;
        }
        let info: Option<Retained<AnyObject>> = msg_send![activity, userInfo];
        static KEY: OnceLock<String> = OnceLock::new();
        let key = KEY.get_or_init(|| constant(c"CSSearchableItemActivityIdentifier", ACTIVITY_IDENTIFIER));
        let value: Option<Retained<AnyObject>> = msg_send![&*info?, objectForKey: &*NSString::from_str(key)];
        let value = value?;
        let is_string: bool = msg_send![&*value, isKindOfClass: NSString::class()];
        if !is_string {
            return None;
        }
        Some(Retained::cast_unchecked::<NSString>(value).to_string())
    }
}

#[cfg(target_os = "macos")]
/// `application:continueUserActivity:restorationHandler:`.
unsafe extern "C-unwind" fn continue_activity(
    this: *mut AnyObject,
    cmd: Sel,
    app: *mut AnyObject,
    activity: *mut AnyObject,
    handler: *mut AnyObject,
) -> Bool {
    // SAFETY: AppKit passes an NSUserActivity.
    if let Some(link) = unsafe { link_of(activity) } {
        if let Some(open) = OPEN.get() {
            open(link);
        }
        return Bool::YES;
    }
    match PREVIOUS.get() {
        // SAFETY: the method this one replaced, of the same signature.
        Some(previous) => unsafe {
            let previous: ContinueActivity = std::mem::transmute::<Imp, ContinueActivity>(*previous);
            previous(this, cmd, app, activity, handler)
        },
        None => Bool::NO,
    }
}

#[cfg(target_os = "macos")]
/// Has the application delegate class `delegate` (GPUI's, or the
/// background process's) pass Spotlight results chosen to `open`, with the
/// link of the message each stands for; other activities go where they
/// went. Once per process. Returns whether it was set up.
pub fn handle_activities(delegate: &CStr, open: fn(String)) -> bool {
    if OPEN.set(open).is_err() {
        return false;
    }
    let Some(cls) = AnyClass::get(delegate) else {
        return false;
    };
    // BOOL, then self, _cmd, the application, the activity and the block.
    let Ok(types) = CString::new(format!("{}@:@@@?", Bool::ENCODING)) else {
        return false;
    };
    let method = continue_activity as ContinueActivity;
    // SAFETY: a method of the declared signature, on a registered class;
    // the type encoding matches it.
    unsafe {
        let imp = std::mem::transmute::<ContinueActivity, Imp>(method);
        let cls = cls as *const AnyClass as *mut AnyClass;
        let selector = sel!(application:continueUserActivity:restorationHandler:);
        if let Some(previous) = objc2::ffi::class_replaceMethod(cls, selector, imp, types.as_ptr()) {
            let _ = PREVIOUS.set(previous);
        }
    }
    true
}
