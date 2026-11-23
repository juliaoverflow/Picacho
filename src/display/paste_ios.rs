//! iOS's pasteboard, for pasting with formatting. GPUI reads only its plain
//! string; apps copy formatting alongside it as HTML (`public.html`), a web
//! archive (Safari) or RTF (Notes, Pages, Mail), each turned into HTML here.
use super::Run;
use objc2::rc::Retained;
use objc2::runtime::{AnyClass, AnyObject, Bool};
use objc2::{Encode, Encoding, RefEncode, class, msg_send};
use std::ffi::{CStr, CString, c_void};
use std::ptr::null_mut;

#[repr(C)]
#[derive(Clone, Copy)]
struct NSRange {
    location: usize,
    length: usize,
}

// Safety: matches Foundation's `struct _NSRange { NSUInteger; NSUInteger; }`.
unsafe impl Encode for NSRange {
    const ENCODING: Encoding = Encoding::Struct("_NSRange", &[usize::ENCODING, usize::ENCODING]);
}
unsafe impl RefEncode for NSRange {
    const ENCODING_REF: Encoding = Encoding::Pointer(&Self::ENCODING);
}

#[link(name = "UIKit", kind = "framework")]
unsafe extern "C" {
    static NSDocumentTypeDocumentAttribute: *const AnyObject;
    static NSRTFTextDocumentType: *const AnyObject;
    static NSRTFDTextDocumentType: *const AnyObject;
    static NSHTMLTextDocumentType: *const AnyObject;
    static NSFontAttributeName: *const AnyObject;
    static NSLinkAttributeName: *const AnyObject;
    static NSStrikethroughStyleAttributeName: *const AnyObject;
}

/// `UIFontDescriptorSymbolicTraits`.
const TRAIT_ITALIC: u32 = 1 << 0;
const TRAIT_BOLD: u32 = 1 << 1;
const TRAIT_MONO_SPACE: u32 = 1 << 10;

/// The pasteboard's formatting as HTML, when it has some.
pub(super) fn clipboard_html() -> Option<String> {
    // UIKit's pasteboard and its attributed-string document conversions
    // (which use WebKit for HTML) belong to the main thread; the composer
    // pastes from it.
    objc2::MainThreadMarker::new()?;
    objc2::rc::autoreleasepool(|_| {
        // Safety: UIKit and Foundation messages with their documented
        // argument and return types; objects come from the autorelease pool
        // above or are owned by a `Retained` that releases them.
        unsafe {
            let board: *mut AnyObject = msg_send![class!(UIPasteboard), generalPasteboard];
            if board.is_null() {
                return None;
            }
            if let Some(html) = data(board, "public.html").and_then(|d| utf8(d)) {
                return Some(html);
            }
            let archive = data(board, "com.apple.webarchive")
                .or_else(|| data(board, "Apple Web Archive pasteboard type"));
            if let Some(html) = archive.and_then(|d| web_archive_html(d)) {
                return Some(html);
            }
            for (kind, document) in [
                ("public.rtf", NSRTFTextDocumentType),
                ("com.apple.flat-rtfd", NSRTFDTextDocumentType),
            ] {
                let Some(d) = data(board, kind) else { continue };
                let Some(text) = attributed_string(d, document) else { continue };
                // UIKit's own HTML of it first: headings, quotes, lists and
                // tables kept, which styled runs alone lose.
                if let Some(html) = attributed_html(&text) {
                    return Some(html);
                }
                if let Some(runs) = attributed_runs(&text) {
                    return Some(super::runs_to_html(&runs));
                }
            }
            None
        }
    })
}

/// An autoreleased `NSString`.
unsafe fn ns_string(s: &str) -> *mut AnyObject {
    let c = CString::new(s).unwrap_or_default();
    unsafe { msg_send![class!(NSString), stringWithUTF8String: c.as_ptr()] }
}

unsafe fn to_string(s: *mut AnyObject) -> Option<String> {
    if s.is_null() {
        return None;
    }
    let utf8: *const std::ffi::c_char = unsafe { msg_send![s, UTF8String] };
    (!utf8.is_null()).then(|| {
        unsafe { CStr::from_ptr(utf8) }
            .to_string_lossy()
            .into_owned()
    })
}

unsafe fn is_kind(object: *mut AnyObject, class: &AnyClass) -> bool {
    if object.is_null() {
        return false;
    }
    let kind: Bool = unsafe { msg_send![object, isKindOfClass: class] };
    kind.as_bool()
}

/// The pasteboard's data of a type (a UTI), if it has that type.
unsafe fn data(board: *mut AnyObject, kind: &str) -> Option<*mut AnyObject> {
    let data: *mut AnyObject = unsafe { msg_send![board, dataForPasteboardType: ns_string(kind)] };
    (!data.is_null()).then_some(data)
}

unsafe fn utf8(data: *mut AnyObject) -> Option<String> {
    let len: usize = unsafe { msg_send![data, length] };
    let bytes: *const c_void = unsafe { msg_send![data, bytes] };
    if len == 0 || bytes.is_null() {
        return None;
    }
    let bytes = unsafe { std::slice::from_raw_parts(bytes.cast::<u8>(), len) };
    let text = String::from_utf8_lossy(bytes).into_owned();
    (!text.trim().is_empty()).then_some(text)
}

/// A web archive's main page: a property list whose `WebMainResource` has
/// the HTML as `WebResourceData`.
unsafe fn web_archive_html(data: *mut AnyObject) -> Option<String> {
    unsafe {
        let plist: *mut AnyObject = msg_send![
            class!(NSPropertyListSerialization),
            propertyListWithData: data,
            options: 0usize,
            format: null_mut::<usize>(),
            error: null_mut::<*mut AnyObject>()
        ];
        if !is_kind(plist, class!(NSDictionary)) {
            return None;
        }
        let main: *mut AnyObject = msg_send![plist, objectForKey: ns_string("WebMainResource")];
        if !is_kind(main, class!(NSDictionary)) {
            return None;
        }
        let html: *mut AnyObject = msg_send![main, objectForKey: ns_string("WebResourceData")];
        if !is_kind(html, class!(NSData)) {
            return None;
        }
        utf8(html)
    }
}

/// RTF (or RTFD) read as an attributed string, owned (released when
/// dropped).
unsafe fn attributed_string(
    data: *mut AnyObject,
    document: *const AnyObject,
) -> Option<Retained<AnyObject>> {
    unsafe {
        let options: *mut AnyObject = msg_send![
            class!(NSDictionary),
            dictionaryWithObject: document,
            forKey: NSDocumentTypeDocumentAttribute
        ];
        // `alloc` and `init…` each hand over a +1 reference: `init…` takes
        // the allocation's and returns its own, or releases it and returns
        // nil when the data doesn't parse.
        let text: *mut AnyObject = msg_send![class!(NSAttributedString), alloc];
        let text: *mut AnyObject = msg_send![
            text,
            initWithData: data,
            options: options,
            documentAttributes: null_mut::<*mut AnyObject>(),
            error: null_mut::<*mut AnyObject>()
        ];
        Retained::from_raw(text)
    }
}

/// An attributed string as HTML, by UIKit's own conversion
/// (`NSHTMLTextDocumentType`, written as UTF-8, its default for HTML).
unsafe fn attributed_html(text: &AnyObject) -> Option<String> {
    unsafe {
        // NSRange counts UTF-16 units, as `length` does.
        let len: usize = msg_send![text, length];
        let options: *mut AnyObject = msg_send![
            class!(NSDictionary),
            dictionaryWithObject: NSHTMLTextDocumentType,
            forKey: NSDocumentTypeDocumentAttribute
        ];
        // Autoreleased, so it lives until the pool in `clipboard_html` ends.
        let html: *mut AnyObject = msg_send![
            text,
            dataFromRange: NSRange { location: 0, length: len },
            documentAttributes: options,
            error: null_mut::<*mut AnyObject>()
        ];
        if html.is_null() { None } else { utf8(html) }
    }
}

/// An attributed string as its styled runs.
unsafe fn attributed_runs(text: &AnyObject) -> Option<Vec<Run>> {
    let runs = unsafe { runs(text) };
    (!runs.is_empty()).then_some(runs)
}

unsafe fn runs(text: &AnyObject) -> Vec<Run> {
    let mut out = vec![];
    unsafe {
        let string: *mut AnyObject = msg_send![text, string];
        let len: usize = msg_send![text, length];
        let mut at = 0;
        while at < len {
            let mut range = NSRange {
                location: at,
                length: 0,
            };
            let attributes: *mut AnyObject =
                msg_send![text, attributesAtIndex: at, effectiveRange: &mut range as *mut NSRange];
            if range.length == 0 {
                break;
            }
            let part: *mut AnyObject = msg_send![string, substringWithRange: range];
            let mut run = Run {
                text: to_string(part).unwrap_or_default(),
                ..Run::default()
            };
            if !attributes.is_null() {
                let font: *mut AnyObject = msg_send![attributes, objectForKey: NSFontAttributeName];
                if !font.is_null() {
                    let descriptor: *mut AnyObject = msg_send![font, fontDescriptor];
                    if !descriptor.is_null() {
                        let traits: u32 = msg_send![descriptor, symbolicTraits];
                        run.bold = traits & TRAIT_BOLD != 0;
                        run.italic = traits & TRAIT_ITALIC != 0;
                        run.code = traits & TRAIT_MONO_SPACE != 0;
                    }
                }
                let link: *mut AnyObject = msg_send![attributes, objectForKey: NSLinkAttributeName];
                run.link = if is_kind(link, class!(NSURL)) {
                    let s: *mut AnyObject = msg_send![link, absoluteString];
                    to_string(s)
                } else if is_kind(link, class!(NSString)) {
                    to_string(link)
                } else {
                    None
                };
                let strike: *mut AnyObject =
                    msg_send![attributes, objectForKey: NSStrikethroughStyleAttributeName];
                if is_kind(strike, class!(NSNumber)) {
                    let style: isize = msg_send![strike, integerValue];
                    run.strike = style != 0;
                }
            }
            out.push(run);
            at = range.location + range.length;
        }
    }
    out
}
