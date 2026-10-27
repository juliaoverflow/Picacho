//! Blitz document and software painting, independent of GPUI and mailbox state.
use super::resources;
use anyrender::{ImageRenderer, PaintScene};
use anyrender_vello_cpu::VelloCpuImageRenderer;
use blitz_dom::{DocumentConfig, UserAgentStylesheet};
use blitz_html::HtmlDocument;
use blitz_traits::shell::{ColorScheme, Viewport};
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};

const READER_CSS: &str = "html { background: white; color: #202124; } body { margin: 12px; font: 14px sans-serif; overflow-wrap: anywhere; } img { max-width: 100%; height: auto; } iframe, object, embed, form, input, button, textarea, select { display: none !important; }";

pub(super) struct EmailDocument {
    pub(super) doc: HtmlDocument,
    cancelled: Arc<AtomicBool>,
    pub(super) changed: Arc<AtomicBool>,
}
impl Drop for EmailDocument {
    fn drop(&mut self) {
        self.cancelled.store(true, Ordering::Relaxed);
    }
}
impl EmailDocument {
    pub(super) fn new(html: &str, blocked: bool) -> Self {
        let cancelled = Arc::new(AtomicBool::new(false));
        let changed = Arc::new(AtomicBool::new(false));
        let resources = resources::Resources::new(blocked, changed.clone(), cancelled.clone());
        let doc = HtmlDocument::from_html(
            html,
            DocumentConfig {
                // Relative resources have no implicit sender-controlled origin.
                base_url: Some("about:blank".into()),
                net_provider: Some(Arc::new(resources)),
                navigation_provider: Some(Arc::new(resources::Links)),
                ua_stylesheets: vec![
                    UserAgentStylesheet::default_sheet(),
                    UserAgentStylesheet::parse(READER_CSS),
                ],
                ..Default::default()
            },
        );
        Self {
            doc,
            cancelled,
            changed,
        }
    }
    pub(super) fn layout(&mut self, width: u32) -> f32 {
        self.doc
            .set_viewport(Viewport::new(width.max(1), 600, 1., ColorScheme::Light));
        self.doc.handle_messages();
        self.doc.resolve(0.);
        self.content_height()
    }
    pub(super) fn content_height(&self) -> f32 {
        let layout = self.doc.root_element().final_layout();
        // Taffy's scroll_height is the maximum *offset*, not the document height.
        layout
            .size
            .height
            .max(layout.scrollable_overflow_rect.bottom)
            .max(1.)
    }
    pub(super) fn pixels(&mut self, width: u32, height: u32, scale: f32, offset: u32) -> Vec<u8> {
        let mut renderer = VelloCpuImageRenderer::new(width, height);
        let mut pixels = Vec::new();
        self.doc.set_viewport_scroll(blitz_dom::Point {
            x: 0.,
            y: offset as f64 / scale as f64,
        });
        // Keep Blitz's layout/culling in CSS pixels. Apply DPI scaling to the
        // recorded vector commands so geometry and text remain crisp at every scale.
        let mut scene = anyrender::Scene::new();
        blitz_paint::paint_scene(
            &mut scene,
            &mut self.doc,
            1.,
            (width as f32 / scale).ceil() as u32,
            (height as f32 / scale).ceil() as u32,
            0,
            0,
        );
        renderer.render_to_vec(
            |painter| painter.append_scene(scene, kurbo::Affine::scale(scale as f64)),
            &mut pixels,
        );
        self.doc
            .set_viewport_scroll(blitz_dom::Point { x: 0., y: 0. });
        // Vello produces premultiplied RGBA; GPUI's atlas expects straight BGRA.
        for pixel in pixels.chunks_exact_mut(4) {
            let alpha = pixel[3] as u32;
            for c in &mut pixel[..3] {
                *c = (*c as u32 * 255).checked_div(alpha).unwrap_or(0).min(255) as u8;
            }
            pixel.swap(0, 2);
        }
        pixels
    }
}
