//! Pictures no longer shown, whose textures leave the windows' sprite
//! atlases on the next frame. GPUI keeps a picture's texture in every
//! window's atlas it was drawn in until told to drop it, and its decoded
//! asset until removed: an attachment previewed, or a picture pasted into
//! a draft, otherwise stayed in memory (and video memory) for the session.
//! Views that let go of pictures retire them here, from wherever (a view
//! being dropped has no window); the main window releases them as it draws
//! (`release`).
use gpui::{App, RenderImage, Window};
use std::cell::RefCell;
use std::sync::Arc;

thread_local! {
    static RENDERED: RefCell<Vec<Arc<RenderImage>>> = const { RefCell::new(Vec::new()) };
    static IMAGES: RefCell<Vec<Arc<gpui::Image>>> = const { RefCell::new(Vec::new()) };
}

/// A decoded picture (`RenderImage`) no longer shown.
pub(crate) fn retire_rendered(image: Arc<RenderImage>) {
    RENDERED.with(|r| r.borrow_mut().push(image));
}

/// A picture given as its file's bytes (`gpui::Image`, decoded by GPUI's
/// asset cache) no longer shown.
pub(crate) fn retire_image(image: Arc<gpui::Image>) {
    IMAGES.with(|r| r.borrow_mut().push(image));
}

/// Drops what was retired from every window's atlas, and GPUI's cache.
pub(crate) fn release(window: &mut Window, cx: &mut App) {
    let rendered = RENDERED.with(|r| std::mem::take(&mut *r.borrow_mut()));
    for image in rendered {
        cx.drop_image(image, Some(window));
    }
    let images = IMAGES.with(|r| std::mem::take(&mut *r.borrow_mut()));
    for image in images {
        if let Some(rendered) = image.clone().get_render_image(window, cx) {
            cx.drop_image(rendered, Some(window));
        }
        image.remove_asset(cx);
    }
}
