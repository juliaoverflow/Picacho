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

const READER_CSS: &str = "body { margin: 12px; font: 14px sans-serif; overflow-wrap: anywhere; } img { max-width: 100%; height: auto; } iframe, object, embed, form, input, button, textarea, select { display: none !important; }";

const LIGHT_CSS: &str = "html { background: white; color: #202124; }";
/// For emails with their own dark styles, which Blitz then applies.
const DARK_CSS: &str = "html { background: #1f2023; color: #e8eaed; }";
/// Blocked remote images keep their size as an empty outline, so the layout
/// is the one the sender designed. Tracking pixels stay invisible.
const BLOCKED_CSS: &str = "img[src^='http' i], img[src^='//'] { outline: 1px dashed rgba(128, 128, 128, 0.55); outline-offset: -1px; } img[src^='http' i]:not([width]):not([height]), img[src^='//']:not([width]):not([height]) { min-width: 16px; min-height: 16px; } img[width='0'], img[width='1'], img[height='0'], img[height='1'] { outline: none !important; }";

/// How the body is colored.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Tone {
    Light,
    /// The email's own `prefers-color-scheme: dark` styles.
    NativeDark,
    /// Light styles with every painted color's lightness inverted.
    InvertedDark,
}
impl Tone {
    pub fn for_email(html: &str, dark: bool) -> Self {
        if !dark {
            Tone::Light
        } else if html.contains("prefers-color-scheme") && html.contains("dark") {
            Tone::NativeDark
        } else {
            Tone::InvertedDark
        }
    }
}

pub(super) struct EmailDocument {
    pub(super) doc: HtmlDocument,
    tone: Tone,
    cancelled: Arc<AtomicBool>,
    pub(super) changed: Arc<AtomicBool>,
}
impl Drop for EmailDocument {
    fn drop(&mut self) {
        self.cancelled.store(true, Ordering::Relaxed);
    }
}
impl EmailDocument {
    #[cfg(test)]
    pub(super) fn new(html: &str, blocked: bool) -> Self {
        Self::with_tone(html, blocked, Tone::Light)
    }
    pub(super) fn with_tone(html: &str, blocked: bool, tone: Tone) -> Self {
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
                ua_stylesheets: [
                    Some(UserAgentStylesheet::default_sheet()),
                    Some(UserAgentStylesheet::parse(if tone == Tone::NativeDark {
                        DARK_CSS
                    } else {
                        LIGHT_CSS
                    })),
                    Some(UserAgentStylesheet::parse(READER_CSS)),
                    blocked.then(|| UserAgentStylesheet::parse(BLOCKED_CSS)),
                ]
                .into_iter()
                .flatten()
                .collect(),
                ..Default::default()
            },
        );
        Self {
            doc,
            tone,
            cancelled,
            changed,
        }
    }
    pub(super) fn layout(&mut self, width: u32) -> f32 {
        self.doc.set_viewport(Viewport::new(
            width.max(1),
            600,
            1.,
            if self.tone == Tone::NativeDark {
                ColorScheme::Dark
            } else {
                ColorScheme::Light
            },
        ));
        self.doc.handle_messages();
        self.doc.resolve(0.);
        self.content_height()
    }
    /// The horizontal extent of the email's content column, (left, width).
    /// Most templates center a fixed-width table in a page-wide background;
    /// this follows the tallest child down from the body while it fills
    /// its parent, and stops at the first one that is clearly narrower.
    pub(super) fn content_column(&self) -> (f32, f32) {
        let full = self.doc.root_element().final_layout().size.width.max(1.);
        let Some(mut node) = self
            .doc
            .query_selector("body")
            .ok()
            .flatten()
            .and_then(|id| self.doc.get_node(id))
        else {
            return (0., full);
        };
        for _ in 0..12 {
            let size = node.final_layout().size;
            let Some(child) = node
                .children
                .iter()
                .filter_map(|id| self.doc.get_node(*id))
                .filter(|n| n.is_element())
                .max_by(|a, b| {
                    a.final_layout()
                        .size
                        .height
                        .total_cmp(&b.final_layout().size.height)
                })
            else {
                break;
            };
            let child_size = child.final_layout().size;
            if child_size.height < size.height * 0.6 || child_size.width < 200. {
                break;
            }
            node = child;
            if child_size.width < full * 0.9 {
                break;
            }
        }
        let width = node.final_layout().size.width;
        if width < full * 0.9 && width >= 200. {
            (node.absolute_position(0., 0.).x, width)
        } else {
            (0., full)
        }
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
        self.pixels_at(width, height, scale, 0., offset as f32 / scale)
    }
    /// Paints the region whose top-left is (`x`, `y`) in document pixels.
    pub(super) fn pixels_at(
        &mut self,
        width: u32,
        height: u32,
        scale: f32,
        x: f32,
        y: f32,
    ) -> Vec<u8> {
        let mut renderer = VelloCpuImageRenderer::new(width, height);
        let mut pixels = Vec::new();
        self.doc
            .set_viewport_scroll(blitz_dom::Point { x: 0., y: y as f64 });
        // Keep Blitz's layout/culling in CSS pixels. Apply DPI scaling to the
        // recorded vector commands so geometry and text remain crisp at every scale.
        let mut scene = anyrender::Scene::new();
        blitz_paint::paint_scene(
            &mut scene,
            &mut self.doc,
            1.,
            (width as f32 / scale + x).ceil() as u32,
            (height as f32 / scale).ceil() as u32,
            0,
            0,
        );
        if self.tone == Tone::InvertedDark {
            darken_scene(&mut scene);
        }
        renderer.render_to_vec(
            |painter| {
                painter.append_scene(
                    scene,
                    kurbo::Affine::scale(scale as f64) * kurbo::Affine::translate((-x as f64, 0.)),
                )
            },
            &mut pixels,
        );
        self.doc
            .set_viewport_scroll(blitz_dom::Point { x: 0., y: 0. });
        // Vello produces premultiplied RGBA; GPUI's atlas expects straight BGRA.
        to_straight_bgra(&mut pixels);
        pixels
    }
}

/// The page color behind the email for a tone, as 0xRRGGBB.
pub fn page_color(tone: Tone) -> u32 {
    match tone {
        Tone::Light => 0xffffff,
        Tone::NativeDark => 0x1f2023,
        Tone::InvertedDark => {
            let [r, g, b, _] = dark_color([1., 1., 1., 1.], Role::Surface);
            let c = |v: f32| (v * 255.).round() as u32;
            c(r) << 16 | c(g) << 8 | c(b)
        }
    }
}

/// Recolors a painted scene for dark reading, the way mail apps do: light
/// page and panel backgrounds turn dark and dark text turns light, while
/// saturated or mid-tone surfaces (brand banners, buttons) and the light
/// text on them keep their colors, only slightly dimmed. Hue and chroma are
/// kept throughout. Images keep their own colors, as photos and logos must.
fn darken_scene(scene: &mut anyrender::Scene) {
    use anyrender::{Paint, recording::RenderCommand};
    let recolor = |brush: &mut Paint, role: Role| match brush {
        Paint::Solid(color) => color.components = dark_color(color.components, role),
        Paint::Gradient(gradient) => {
            for stop in gradient.stops.iter_mut() {
                let mut color: peniko::Color = stop.color.to_alpha_color();
                color.components = dark_color(color.components, role);
                stop.color = peniko::color::DynamicColor::from_alpha_color(color);
            }
        }
        _ => {}
    };
    for command in &mut scene.commands {
        match command {
            RenderCommand::Fill(fill) => recolor(&mut fill.brush, Role::Surface),
            RenderCommand::Stroke(stroke) => recolor(&mut stroke.brush, Role::Line),
            RenderCommand::GlyphRun(run) => recolor(&mut run.brush, Role::Text),
            RenderCommand::BoxShadow(shadow) => {
                shadow.brush.components = dark_color(shadow.brush.components, Role::Surface)
            }
            _ => {}
        }
    }
}

/// What a painted color is for, which decides how dark mode treats it.
#[derive(Clone, Copy, PartialEq)]
pub(super) enum Role {
    Surface,
    Line,
    Text,
}

// OKLab's published matrices, digit for digit.
#[allow(clippy::excessive_precision)]
fn to_oklab([r, g, b]: [f32; 3]) -> [f32; 3] {
    let linear = |c: f32| {
        if c <= 0.04045 {
            c / 12.92
        } else {
            ((c + 0.055) / 1.055).powf(2.4)
        }
    };
    let (r, g, b) = (linear(r), linear(g), linear(b));
    let l = (0.4122214708 * r + 0.5363325363 * g + 0.0514459929 * b).cbrt();
    let m = (0.2119034982 * r + 0.6806995451 * g + 0.1073969566 * b).cbrt();
    let s = (0.0883024619 * r + 0.2817188376 * g + 0.6299787005 * b).cbrt();
    [
        0.2104542553 * l + 0.7936177850 * m - 0.0040720468 * s,
        1.9779984951 * l - 2.4285922050 * m + 0.4505937099 * s,
        0.0259040371 * l + 0.7827717662 * m - 0.8086757660 * s,
    ]
}
#[allow(clippy::excessive_precision)]
fn from_oklab([lightness, oa, ob]: [f32; 3]) -> [f32; 3] {
    let l = (lightness + 0.3963377774 * oa + 0.2158037573 * ob).powi(3);
    let m = (lightness - 0.1055613458 * oa - 0.0638541728 * ob).powi(3);
    let s = (lightness - 0.0894841775 * oa - 1.2914855480 * ob).powi(3);
    let gamma = |c: f32| {
        let c = c.clamp(0., 1.);
        if c <= 0.0031308 {
            c * 12.92
        } else {
            1.055 * c.powf(1. / 2.4) - 0.055
        }
    };
    [
        gamma(4.0767416621 * l - 3.3077115913 * m + 0.2309699292 * s),
        gamma(-1.2684380046 * l + 2.6097574011 * m - 0.3413193965 * s),
        gamma(-0.0041960863 * l - 0.7034186147 * m + 1.7076147010 * s),
    ]
}

/// A straight sRGB color for dark reading (see `darken_scene`). Lightness
/// is OKLab's, so "light" and "dark" match what the eye sees.
pub(super) fn dark_color([r, g, b, alpha]: [f32; 4], role: Role) -> [f32; 4] {
    let [l, a, bb] = to_oklab([r, g, b]);
    let chroma = (a * a + bb * bb).sqrt();
    // Flipped lightness: white becomes a soft charcoal, black near-white.
    let flipped = 0.95 - 0.73 * l;
    let next = match role {
        // Grey and pale tinted surfaces are page and panel backgrounds:
        // darken them. Saturated surfaces, even light ones, are brand color:
        // keep, a little dimmer.
        Role::Surface if chroma < 0.04 || (l > 0.85 && chroma < 0.1) => flipped.min(l),
        Role::Surface => l * 0.9,
        // Dark text turns light; light text sits on a kept color: keep it.
        Role::Text if l < 0.6 => flipped.max(l),
        Role::Text => l,
        // Rules and borders keep their contrast with what is around them.
        Role::Line => flipped,
    };
    let [r, g, b] = from_oklab([next, a, bb]);
    [r, g, b, alpha]
}

/// Converts premultiplied RGBA to straight BGRA in place. Email backgrounds
/// are opaque almost everywhere, so those pixels only swap two channels,
/// a word at a time.
fn to_straight_bgra(pixels: &mut [u8]) {
    let (head, words, tail) = bytemuck::pod_align_to_mut::<u8, u32>(pixels);
    if !head.is_empty() || !tail.is_empty() {
        for pixel in pixels.chunks_exact_mut(4) {
            unpremultiply(pixel);
        }
        return;
    }
    for word in words {
        let p = u32::from_le(*word);
        if p >> 24 == 0xff {
            *word = (p & 0xff00_ff00 | (p & 0xff) << 16 | (p >> 16) & 0xff).to_le();
        } else {
            let mut bytes = word.to_ne_bytes();
            unpremultiply(&mut bytes);
            *word = u32::from_ne_bytes(bytes);
        }
    }
}
fn unpremultiply(pixel: &mut [u8]) {
    let alpha = pixel[3] as u32;
    for c in &mut pixel[..3] {
        *c = (*c as u32 * 255).checked_div(alpha).unwrap_or(0).min(255) as u8;
    }
    pixel.swap(0, 2);
}

/// What a secondary click landed on, for the reader's context menu.
#[derive(Clone, Debug, Default)]
pub struct ContextTarget {
    pub link: Option<String>,
    pub image: Option<String>,
}

impl EmailDocument {
    /// The cursor the author's CSS (or the content under the pointer) asks for.
    pub(super) fn cursor(&self) -> gpui::CursorStyle {
        use cursor_icon::CursorIcon as I;
        use gpui::CursorStyle as C;
        match self.doc.get_cursor() {
            Some(I::Pointer) => C::PointingHand,
            Some(I::Text) => C::IBeam,
            Some(I::VerticalText) => C::IBeamCursorForVerticalLayout,
            Some(I::Crosshair) => C::Crosshair,
            Some(I::Grab) => C::OpenHand,
            Some(I::Grabbing) => C::ClosedHand,
            Some(I::NotAllowed | I::NoDrop) => C::OperationNotAllowed,
            Some(I::ContextMenu) => C::ContextualMenu,
            Some(I::Alias) => C::DragLink,
            Some(I::Copy) => C::DragCopy,
            Some(I::EwResize | I::ColResize) => C::ResizeLeftRight,
            Some(I::NsResize | I::RowResize) => C::ResizeUpDown,
            _ => C::Arrow,
        }
    }
    /// The link and image under a point in document coordinates.
    pub(super) fn context_at(&self, x: f32, y: f32) -> ContextTarget {
        let mut target = ContextTarget::default();
        let Some(hit) = self.doc.hit(x, y) else {
            return target;
        };
        let mut next = self.doc.get_node(hit.node_id);
        while let Some(node) = next {
            if let Some(element) = node.element_data() {
                let tag: &str = &element.name.local;
                if target.link.is_none() && element.is_link() {
                    target.link = element.attr(blitz_dom::local_name!("href")).map(Into::into);
                }
                if target.image.is_none() && tag == "img" {
                    target.image = element
                        .attr(blitz_dom::local_name!("src"))
                        .filter(|s| !s.starts_with("data:"))
                        .map(Into::into);
                }
            }
            next = node.parent.and_then(|id| self.doc.get_node(id));
        }
        // Only addresses that can be opened or copied usefully.
        target.link = target.link.filter(|href| {
            url::Url::parse(href).is_ok_and(|u| resources::remote_url(&u) || u.scheme() == "mailto")
        });
        target
    }
}
