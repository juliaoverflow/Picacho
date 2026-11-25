//! An anyrender scene that writes PDF (with krilla): what Blitz paints of
//! an email becomes vector paths, text in the email's own fonts (embedded,
//! subset, and mapped back to characters so the PDF's text can be copied
//! and searched) and images, rather than pixels.
//!
//! Coordinates arrive in the document's CSS pixels; the page's transform
//! (pushed first, popped last) maps them to the page. Each command's own
//! transform is applied to its geometry, so clips, which krilla keeps until
//! popped, need no transform of their own on the stack.
use anyrender::{Glyph, NormalizedCoord, Paint, PaintRef, PaintScene, RenderContext};
use krilla::color::rgb;
use krilla::geom::{Path, PathBuilder, Size, Transform};
use krilla::num::NormalizedF32;
use krilla::paint::{Fill, FillRule, LinearGradient, RadialGradient, SpreadMethod, Stop, Stroke, StrokeDash, SweepGradient};
use krilla::surface::Surface;
use kurbo::{Affine, PathEl, Rect, Shape};
use peniko::{BlendMode, Color, Extend, FontData, ImageBrushRef, StyleRef};
use std::collections::HashMap;
use std::sync::Arc;

/// Fonts and images already given to the PDF, by their data's identity, so
/// each is embedded once however often it's drawn.
#[derive(Default)]
pub(super) struct Resources {
    fonts: HashMap<(u64, u32, Vec<NormalizedCoord>), Option<PdfFont>>,
    images: HashMap<u64, Option<krilla::image::Image>>,
}

/// A font as the PDF has it, and the characters its glyphs stand for.
#[derive(Clone)]
struct PdfFont {
    font: krilla::text::Font,
    chars: Arc<HashMap<u32, char>>,
}

impl Resources {
    fn font(&mut self, data: &FontData, coords: &[NormalizedCoord]) -> Option<PdfFont> {
        let key = (data.data.id(), data.index, coords.to_vec());
        self.fonts
            .entry(key)
            .or_insert_with(|| {
                let bytes: Arc<dyn AsRef<[u8]> + Send + Sync> = Arc::new(data.data.clone());
                let font_ref = skrifa::FontRef::from_index(data.data.as_ref(), data.index).ok()?;
                // The lowest character for each glyph.
                let mut chars = HashMap::new();
                for (code, glyph) in skrifa::MetadataProvider::charmap(&font_ref).mappings() {
                    if let Some(c) = char::from_u32(code) {
                        chars.entry(glyph.to_u32()).or_insert(c);
                    }
                }
                let font = if coords.iter().any(|c| *c != 0) {
                    // Normalized coordinates back to the axes' own values.
                    let axes = skrifa::MetadataProvider::axes(&font_ref);
                    let settings: Vec<(krilla::text::Tag, f32)> = axes
                        .iter()
                        .zip(coords)
                        .map(|(axis, &n)| {
                            let n = n as f32 / 16384.;
                            let value = if n < 0. {
                                axis.default_value() + n * (axis.default_value() - axis.min_value())
                            } else {
                                axis.default_value() + n * (axis.max_value() - axis.default_value())
                            };
                            (krilla::text::Tag::new(&axis.tag().to_be_bytes()), value)
                        })
                        .collect();
                    krilla::text::Font::new_variable(bytes.into(), data.index, &settings)?
                } else {
                    krilla::text::Font::new(bytes.into(), data.index)?
                };
                Some(PdfFont {
                    font,
                    chars: Arc::new(chars),
                })
            })
            .clone()
    }

    fn image(&mut self, image: &peniko::ImageData) -> Option<krilla::image::Image> {
        self.images
            .entry(image.data.id())
            .or_insert_with(|| pdf_image(image))
            .clone()
    }
}

/// An image for the PDF: photographs (opaque, and not small) as JPEG,
/// the rest losslessly with their transparency.
fn pdf_image(image: &peniko::ImageData) -> Option<krilla::image::Image> {
    let (w, h) = (image.width, image.height);
    let bytes = image.data.as_ref();
    if w == 0 || h == 0 || bytes.len() < (w as usize) * (h as usize) * 4 {
        return None;
    }
    let mut rgba = bytes[..(w as usize) * (h as usize) * 4].to_vec();
    for px in rgba.as_chunks_mut::<4>().0 {
        if image.format == peniko::ImageFormat::Bgra8 {
            px.swap(0, 2);
        }
        if image.alpha_type == peniko::ImageAlphaType::AlphaPremultiplied && px[3] > 0 && px[3] < 255 {
            let a = px[3] as u32;
            for c in &mut px[..3] {
                *c = ((*c as u32 * 255 + a / 2) / a).min(255) as u8;
            }
        }
    }
    let opaque = rgba.as_chunks::<4>().0.iter().all(|p| p[3] == 255);
    if opaque && w as u64 * h as u64 >= 128 * 128 {
        let rgb: Vec<u8> = rgba
            .as_chunks::<4>()
            .0
            .iter()
            .flat_map(|p| [p[0], p[1], p[2]])
            .collect();
        let mut jpeg = Vec::new();
        let encoded = image::codecs::jpeg::JpegEncoder::new_with_quality(&mut jpeg, 88)
            .encode(&rgb, w, h, image::ExtendedColorType::Rgb8);
        if encoded.is_ok()
            && let Ok(img) = krilla::image::Image::from_jpeg(jpeg.into(), true)
        {
            return Some(img);
        }
    }
    Some(krilla::image::Image::from_rgba8(rgba, w, h))
}

/// The krilla transform of a kurbo one.
fn transform(a: Affine) -> Transform {
    let [sx, ky, kx, sy, tx, ty] = a.as_coeffs().map(|v| v as f32);
    Transform::from_row(sx, ky, kx, sy, tx, ty)
}

/// `shape` as a PDF path, through `a`.
fn path(shape: &impl Shape, a: Affine) -> Option<Path> {
    let mut b = PathBuilder::new();
    let p = |pt: kurbo::Point| {
        let pt = a * pt;
        (pt.x as f32, pt.y as f32)
    };
    for el in shape.path_elements(0.1) {
        match el {
            PathEl::MoveTo(a0) => {
                let (x, y) = p(a0);
                b.move_to(x, y)
            }
            PathEl::LineTo(a0) => {
                let (x, y) = p(a0);
                b.line_to(x, y)
            }
            PathEl::QuadTo(a0, a1) => {
                let ((x1, y1), (x, y)) = (p(a0), p(a1));
                b.quad_to(x1, y1, x, y)
            }
            PathEl::CurveTo(a0, a1, a2) => {
                let ((x1, y1), (x2, y2), (x, y)) = (p(a0), p(a1), p(a2));
                b.cubic_to(x1, y1, x2, y2, x, y)
            }
            PathEl::ClosePath => b.close(),
        }
    }
    b.finish()
}

fn unit(v: f32) -> NormalizedF32 {
    NormalizedF32::new(v.clamp(0., 1.)).unwrap_or(NormalizedF32::ONE)
}

/// A color as the PDF's RGB, and its opacity.
fn color(c: Color) -> (rgb::Color, f32) {
    let [r, g, b, a] = c.to_rgba8().to_u8_array();
    (rgb::Color::new(r, g, b), a as f32 / 255.)
}

fn spread(extend: Extend) -> SpreadMethod {
    match extend {
        Extend::Pad => SpreadMethod::Pad,
        Extend::Repeat => SpreadMethod::Repeat,
        Extend::Reflect => SpreadMethod::Reflect,
    }
}

/// A brush as a PDF paint and its opacity; images aren't paints here (they
/// are drawn), nor are custom paints, which nothing in an email makes.
fn paint(brush: &PaintRef<'_>, brush_transform: Option<Affine>, alpha: f32) -> Option<(krilla::paint::Paint, f32)> {
    match brush {
        Paint::Solid(c) => {
            let (c, a) = color(*c);
            Some((c.into(), a * alpha))
        }
        Paint::Gradient(g) => {
            let stops: Vec<Stop> = g
                .stops
                .iter()
                .map(|s| {
                    let (c, a) = color(s.color.to_alpha_color::<peniko::color::Srgb>());
                    Stop {
                        offset: unit(s.offset),
                        color: c.into(),
                        opacity: unit(a),
                    }
                })
                .collect();
            let t = transform(brush_transform.unwrap_or(Affine::IDENTITY));
            let paint: krilla::paint::Paint = match &g.kind {
                peniko::GradientKind::Linear(l) => LinearGradient {
                    x1: l.start.x as f32,
                    y1: l.start.y as f32,
                    x2: l.end.x as f32,
                    y2: l.end.y as f32,
                    transform: t,
                    spread_method: spread(g.extend),
                    stops,
                    anti_alias: true,
                }
                .into(),
                peniko::GradientKind::Radial(r) => RadialGradient {
                    fx: r.start_center.x as f32,
                    fy: r.start_center.y as f32,
                    fr: r.start_radius,
                    cx: r.end_center.x as f32,
                    cy: r.end_center.y as f32,
                    cr: r.end_radius,
                    transform: t,
                    spread_method: spread(g.extend),
                    stops,
                    anti_alias: true,
                }
                .into(),
                peniko::GradientKind::Sweep(s) => SweepGradient {
                    cx: s.center.x as f32,
                    cy: s.center.y as f32,
                    start_angle: s.start_angle.to_degrees(),
                    end_angle: s.end_angle.to_degrees(),
                    transform: t,
                    spread_method: spread(g.extend),
                    stops,
                    anti_alias: true,
                }
                .into(),
            };
            Some((paint, alpha))
        }
        _ => None,
    }
}

fn fill_rule(f: peniko::Fill) -> FillRule {
    match f {
        peniko::Fill::NonZero => FillRule::NonZero,
        peniko::Fill::EvenOdd => FillRule::EvenOdd,
    }
}

fn pdf_stroke(s: &kurbo::Stroke, paint: krilla::paint::Paint, opacity: f32) -> Stroke {
    use krilla::paint::{LineCap, LineJoin};
    Stroke {
        paint,
        width: s.width as f32,
        miter_limit: s.miter_limit as f32,
        line_cap: match s.start_cap {
            kurbo::Cap::Butt => LineCap::Butt,
            kurbo::Cap::Square => LineCap::Square,
            kurbo::Cap::Round => LineCap::Round,
        },
        line_join: match s.join {
            kurbo::Join::Bevel => LineJoin::Bevel,
            kurbo::Join::Miter => LineJoin::Miter,
            kurbo::Join::Round => LineJoin::Round,
        },
        opacity: unit(opacity),
        dash: (!s.dash_pattern.is_empty()).then(|| StrokeDash {
            array: s.dash_pattern.iter().map(|d| *d as f32).collect(),
            offset: s.dash_offset as f32,
        }),
    }
}

fn blend(mode: BlendMode) -> krilla::blend::BlendMode {
    use krilla::blend::BlendMode as B;
    use peniko::Mix;
    match mode.mix {
        Mix::Multiply => B::Multiply,
        Mix::Screen => B::Screen,
        Mix::Overlay => B::Overlay,
        Mix::Darken => B::Darken,
        Mix::Lighten => B::Lighten,
        Mix::ColorDodge => B::ColorDodge,
        Mix::ColorBurn => B::ColorBurn,
        Mix::HardLight => B::HardLight,
        Mix::SoftLight => B::SoftLight,
        Mix::Difference => B::Difference,
        Mix::Exclusion => B::Exclusion,
        Mix::Hue => B::Hue,
        Mix::Saturation => B::Saturation,
        Mix::Color => B::Color,
        Mix::Luminosity => B::Luminosity,
        _ => B::Normal,
    }
}

/// A glyph placed by krilla: its id, advance and offset (in ems), and the
/// characters it stands for in the run's text.
struct Placed {
    id: u32,
    range: std::ops::Range<usize>,
    advance: f32,
    y_offset: f32,
}
impl krilla::text::Glyph for Placed {
    fn glyph_id(&self) -> krilla::text::GlyphId {
        krilla::text::GlyphId::new(self.id)
    }
    fn text_range(&self) -> std::ops::Range<usize> {
        self.range.clone()
    }
    fn x_advance(&self, size: f32) -> f32 {
        self.advance * size
    }
    fn x_offset(&self, _: f32) -> f32 {
        0.
    }
    fn y_offset(&self, size: f32) -> f32 {
        self.y_offset * size
    }
    fn y_advance(&self, _: f32) -> f32 {
        0.
    }
    fn location(&self) -> Option<krilla::surface::Location> {
        None
    }
}

/// One page's surface, taking Blitz's painting.
pub(super) struct PdfScene<'s, 'p> {
    pub(super) surface: &'s mut Surface<'p>,
    pub(super) resources: &'s mut Resources,
    /// How many of krilla's pushes each open layer made.
    layers: Vec<usize>,
}

impl<'s, 'p> PdfScene<'s, 'p> {
    pub(super) fn new(surface: &'s mut Surface<'p>, resources: &'s mut Resources) -> Self {
        Self {
            surface,
            resources,
            layers: Vec::new(),
        }
    }

    /// Pops what layers left open.
    pub(super) fn finish(mut self) {
        while self.layers.pop().is_some_and(|n| {
            for _ in 0..n {
                self.surface.pop();
            }
            true
        }) {}
    }

    fn draw_image(&mut self, image: ImageBrushRef<'_>, transform: Affine, brush_transform: Option<Affine>, shape: &impl Shape) {
        let data = image.image;
        let Some(pdf) = self.resources.image(data) else { return };
        let Some(clip) = path(shape, transform) else { return };
        let Some(size) = Size::from_wh(data.width as f32, data.height as f32) else { return };
        self.surface.push_clip_path(&clip, &FillRule::NonZero);
        let alpha = image.sampler.alpha;
        if alpha < 1. {
            self.surface.push_opacity(unit(alpha));
        }
        let base = transform * brush_transform.unwrap_or(Affine::IDENTITY);
        // Repeated backgrounds: a tile for each place the shape shows.
        let tiled = image.sampler.x_extend != Extend::Pad || image.sampler.y_extend != Extend::Pad;
        let mut offsets = vec![(0., 0.)];
        if tiled && let Some(inverse) = Some(base.inverse()) {
            let area = (inverse * shape.bounding_box().to_path(0.1)).bounding_box();
            let (w, h) = (data.width as f64, data.height as f64);
            let xs: Vec<f64> = if image.sampler.x_extend != Extend::Pad {
                let first = (area.x0 / w).floor() as i64;
                let last = (area.x1 / w).ceil() as i64;
                (first..last).take(400).map(|i| i as f64 * w).collect()
            } else {
                vec![0.]
            };
            let ys: Vec<f64> = if image.sampler.y_extend != Extend::Pad {
                let first = (area.y0 / h).floor() as i64;
                let last = (area.y1 / h).ceil() as i64;
                (first..last).take(400).map(|i| i as f64 * h).collect()
            } else {
                vec![0.]
            };
            offsets = ys
                .iter()
                .flat_map(|y| xs.iter().map(move |x| (*x, *y)))
                .take(2000)
                .collect();
        }
        for (x, y) in offsets {
            self.surface
                .push_transform(&transform_of(base * Affine::translate((x, y))));
            self.surface.draw_image(pdf.clone(), size);
            self.surface.pop();
        }
        if alpha < 1. {
            self.surface.pop();
        }
        self.surface.pop();
    }
}

fn transform_of(a: Affine) -> Transform {
    transform(a)
}

impl RenderContext for PdfScene<'_, '_> {}

impl PaintScene for PdfScene<'_, '_> {
    fn reset(&mut self) {}

    fn push_layer(
        &mut self,
        fill: peniko::Fill,
        blend_mode: impl Into<BlendMode>,
        alpha: f32,
        transform: Affine,
        clip: &impl Shape,
        _filter: Option<Arc<anyrender::Filter>>,
        _backdrop_filter: Option<Arc<anyrender::Filter>>,
    ) {
        let mut pushed = 0;
        if let Some(clip) = path(clip, transform) {
            self.surface.push_clip_path(&clip, &fill_rule(fill));
            pushed += 1;
        }
        let mode = blend(blend_mode.into());
        if mode != krilla::blend::BlendMode::Normal {
            self.surface.push_blend_mode(mode);
            pushed += 1;
        }
        if alpha < 1. {
            self.surface.push_opacity(unit(alpha));
            pushed += 1;
        }
        self.layers.push(pushed);
    }

    fn push_clip_layer(&mut self, fill: peniko::Fill, transform: Affine, clip: &impl Shape) {
        let pushed = match path(clip, transform) {
            Some(clip) => {
                self.surface.push_clip_path(&clip, &fill_rule(fill));
                1
            }
            // An empty clip shows nothing: clipped to nothing.
            None => match path(&Rect::new(0., 0., 0.001, 0.001), Affine::IDENTITY) {
                Some(nothing) => {
                    self.surface.push_clip_path(&nothing, &FillRule::NonZero);
                    1
                }
                None => 0,
            },
        };
        self.layers.push(pushed);
    }

    fn pop_layer(&mut self) {
        if let Some(n) = self.layers.pop() {
            for _ in 0..n {
                self.surface.pop();
            }
        }
    }

    fn stroke<'a>(
        &mut self,
        style: &kurbo::Stroke,
        transform: Affine,
        brush: impl Into<PaintRef<'a>>,
        brush_transform: Option<Affine>,
        shape: &impl Shape,
    ) {
        let brush = brush.into();
        let Some((paint, opacity)) = paint(&brush, brush_transform, 1.) else { return };
        let Some(p) = path(shape, Affine::IDENTITY) else { return };
        self.surface.push_transform(&transform_of(transform));
        self.surface.set_fill(None);
        self.surface.set_stroke(Some(pdf_stroke(style, paint, opacity)));
        self.surface.draw_path(&p);
        self.surface.set_stroke(None);
        self.surface.pop();
    }

    fn fill<'a>(
        &mut self,
        style: peniko::Fill,
        transform: Affine,
        brush: impl Into<PaintRef<'a>>,
        brush_transform: Option<Affine>,
        shape: &impl Shape,
    ) {
        let brush = brush.into();
        if let Paint::Image(image) = brush {
            self.draw_image(image, transform, brush_transform, shape);
            return;
        }
        let Some((paint, opacity)) = paint(&brush, brush_transform, 1.) else { return };
        if opacity <= 0. {
            return;
        }
        let Some(p) = path(shape, Affine::IDENTITY) else { return };
        self.surface.push_transform(&transform_of(transform));
        self.surface.set_stroke(None);
        self.surface.set_fill(Some(Fill {
            paint,
            opacity: unit(opacity),
            rule: fill_rule(style),
        }));
        self.surface.draw_path(&p);
        self.surface.set_fill(None);
        self.surface.pop();
    }

    fn draw_glyphs<'a, 's: 'a>(
        &'s mut self,
        font: &'a FontData,
        font_size: f32,
        _hint: bool,
        normalized_coords: &'a [NormalizedCoord],
        embolden: kurbo::Vec2,
        style: impl Into<StyleRef<'a>>,
        brush: impl Into<PaintRef<'a>>,
        brush_alpha: f32,
        transform: Affine,
        glyph_transform: Option<Affine>,
        glyphs: impl Iterator<Item = Glyph> + Clone,
    ) {
        let glyphs: Vec<Glyph> = glyphs.collect();
        let (Some(first), true) = (glyphs.first().copied(), font_size > 0.) else { return };
        let Some(pdf_font) = self.resources.font(font, normalized_coords) else { return };
        let brush = brush.into();
        let Some((paint, opacity)) = paint(&brush, None, brush_alpha) else { return };
        if opacity <= 0. {
            return;
        }
        // The run's text, a character for each glyph that has one.
        let mut text = String::new();
        let mut placed = Vec::with_capacity(glyphs.len());
        for (i, g) in glyphs.iter().enumerate() {
            let start = text.len();
            if let Some(c) = pdf_font.chars.get(&g.id) {
                text.push(*c);
            }
            let next_x = glyphs.get(i + 1).map(|n| n.x).unwrap_or(g.x);
            placed.push(Placed {
                id: g.id,
                range: start..text.len(),
                advance: (next_x - g.x) / font_size,
                y_offset: (first.y - g.y) / font_size,
            });
        }
        // A glyph transform (synthetic italics' skew) about the baseline.
        let origin = Affine::translate((first.x as f64, first.y as f64));
        let run = match glyph_transform {
            Some(g) => transform * origin * g * origin.inverse(),
            None => transform,
        };
        let style = style.into();
        let bold = embolden.x.max(embolden.y) as f32;
        let fill = || Fill {
            paint: paint.clone(),
            opacity: unit(opacity),
            rule: FillRule::NonZero,
        };
        match style {
            StyleRef::Fill(_) => {
                self.surface.set_fill(Some(fill()));
                // Synthetic bold: an outline as wide as it's emboldened.
                self.surface.set_stroke((bold > 0.).then(|| Stroke {
                    paint: paint.clone(),
                    width: bold * 2.,
                    opacity: unit(opacity),
                    ..Stroke::default()
                }));
            }
            StyleRef::Stroke(s) => {
                self.surface.set_fill(None);
                self.surface.set_stroke(Some(pdf_stroke(s, paint.clone(), opacity)));
            }
        }
        self.surface.push_transform(&transform_of(run));
        self.surface.draw_glyphs(
            krilla::geom::Point::from_xy(first.x, first.y),
            &placed,
            pdf_font.font.clone(),
            &text,
            font_size,
            false,
        );
        self.surface.pop();
        self.surface.set_fill(None);
        self.surface.set_stroke(None);
    }

    fn draw_box_shadow(&mut self, transform: Affine, rect: Rect, brush: Color, radius: f64, std_dev: f64) {
        // No blur in PDF: rings fading out over the blur's reach.
        let (c, a) = color(brush);
        let steps = if std_dev > 0.5 { 6 } else { 1 };
        let reach = std_dev * 2.;
        for i in 0..steps {
            let grow = reach * (1. - i as f64 / steps as f64);
            let shape = rect.inflate(grow, grow).to_rounded_rect(radius + grow);
            let Some(p) = path(&shape, Affine::IDENTITY) else { continue };
            self.surface.push_transform(&transform_of(transform));
            self.surface.set_stroke(None);
            self.surface.set_fill(Some(Fill {
                paint: c.into(),
                opacity: unit(a / steps as f32),
                rule: FillRule::NonZero,
            }));
            self.surface.draw_path(&p);
            self.surface.set_fill(None);
            self.surface.pop();
        }
    }
}
