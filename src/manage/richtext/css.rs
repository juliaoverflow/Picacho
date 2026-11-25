//! The little CSS the formatted composer reads from inline `style`
//! attributes: declarations, colors and lengths.

/// A color, with alpha.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Color {
    pub r: u8,
    pub g: u8,
    pub b: u8,
    pub a: u8,
}

impl Color {
    pub const fn rgb(r: u8, g: u8, b: u8) -> Self {
        Self { r, g, b, a: 255 }
    }
    /// As CSS writes it: `#rrggbb`, or `rgba()` when not opaque.
    pub fn to_css(self) -> String {
        if self.a == 255 {
            format!("#{:02x}{:02x}{:02x}", self.r, self.g, self.b)
        } else {
            format!("rgba({},{},{},{:.2})", self.r, self.g, self.b, self.a as f32 / 255.)
        }
    }
    /// Relative luminance, 0 (black) to 1 (white).
    pub fn luminance(self) -> f32 {
        let f = |c: u8| {
            let c = c as f32 / 255.;
            if c <= 0.039_28 { c / 12.92 } else { ((c + 0.055) / 1.055).powf(2.4) }
        };
        0.2126 * f(self.r) + 0.7152 * f(self.g) + 0.0722 * f(self.b)
    }
}

/// A CSS color: `#rgb`, `#rrggbb` (and with alpha), `rgb()`/`rgba()`, or
/// one of the common names. `None` for anything else (`inherit`,
/// `currentColor`, system colors).
pub fn color(value: &str) -> Option<Color> {
    let v = value.trim().trim_end_matches("!important").trim().to_ascii_lowercase();
    if let Some(hex) = v.strip_prefix('#') {
        let digit = |i: usize| u8::from_str_radix(hex.get(i..i + 1)?, 16).ok();
        let pair = |i: usize| u8::from_str_radix(hex.get(i..i + 2)?, 16).ok();
        return match hex.len() {
            3 | 4 => Some(Color {
                r: digit(0)? * 17,
                g: digit(1)? * 17,
                b: digit(2)? * 17,
                a: if hex.len() == 4 { digit(3)? * 17 } else { 255 },
            }),
            6 | 8 => Some(Color {
                r: pair(0)?,
                g: pair(2)?,
                b: pair(4)?,
                a: if hex.len() == 8 { pair(6)? } else { 255 },
            }),
            _ => None,
        };
    }
    if let Some(args) = v.strip_prefix("rgba(").or_else(|| v.strip_prefix("rgb(")) {
        let args = args.strip_suffix(')')?;
        let parts: Vec<&str> = args
            .split([',', ' ', '/'])
            .map(str::trim)
            .filter(|p| !p.is_empty())
            .collect();
        if parts.len() < 3 {
            return None;
        }
        let channel = |p: &str| -> Option<u8> {
            if let Some(pct) = p.strip_suffix('%') {
                Some((pct.parse::<f32>().ok()?.clamp(0., 100.) * 2.55).round() as u8)
            } else {
                Some(p.parse::<f32>().ok()?.clamp(0., 255.).round() as u8)
            }
        };
        let alpha = match parts.get(3) {
            Some(p) => {
                let a = if let Some(pct) = p.strip_suffix('%') {
                    pct.parse::<f32>().ok()? / 100.
                } else {
                    p.parse::<f32>().ok()?
                };
                (a.clamp(0., 1.) * 255.).round() as u8
            }
            None => 255,
        };
        return Some(Color {
            r: channel(parts[0])?,
            g: channel(parts[1])?,
            b: channel(parts[2])?,
            a: alpha,
        });
    }
    let named = match v.as_str() {
        "black" => (0, 0, 0),
        "white" => (255, 255, 255),
        "red" => (255, 0, 0),
        "green" => (0, 128, 0),
        "blue" => (0, 0, 255),
        "yellow" => (255, 255, 0),
        "orange" => (255, 165, 0),
        "purple" => (128, 0, 128),
        "gray" | "grey" => (128, 128, 128),
        "silver" => (192, 192, 192),
        "maroon" => (128, 0, 0),
        "navy" => (0, 0, 128),
        "teal" => (0, 128, 128),
        "olive" => (128, 128, 0),
        "lime" => (0, 255, 0),
        "aqua" | "cyan" => (0, 255, 255),
        "fuchsia" | "magenta" => (255, 0, 255),
        "darkgray" | "darkgrey" => (169, 169, 169),
        "lightgray" | "lightgrey" => (211, 211, 211),
        "darkblue" => (0, 0, 139),
        "darkred" => (139, 0, 0),
        "darkgreen" => (0, 100, 0),
        "brown" => (165, 42, 42),
        "pink" => (255, 192, 203),
        "transparent" => return Some(Color { r: 0, g: 0, b: 0, a: 0 }),
        _ => return None,
    };
    Some(Color::rgb(named.0, named.1, named.2))
}

/// A font size in pixels, `inherited` being the size it is relative to
/// (em, %, and the keywords `smaller`/`larger`).
pub fn font_size(value: &str, inherited: f32) -> Option<f32> {
    let v = value.trim().trim_end_matches("!important").trim().to_ascii_lowercase();
    let number = |s: &str| s.trim().parse::<f32>().ok().filter(|n| n.is_finite() && *n >= 0.);
    let px = if let Some(n) = v.strip_suffix("px") {
        number(n)?
    } else if let Some(n) = v.strip_suffix("pt") {
        number(n)? * 4. / 3.
    } else if let Some(n) = v.strip_suffix("rem") {
        number(n)? * 16.
    } else if let Some(n) = v.strip_suffix("em") {
        number(n)? * inherited
    } else if let Some(n) = v.strip_suffix('%') {
        number(n)? * inherited / 100.
    } else {
        match v.as_str() {
            "xx-small" => 9.,
            "x-small" => 10.,
            "small" => 13.,
            "medium" => 16.,
            "large" => 18.,
            "x-large" => 24.,
            "xx-large" => 32.,
            "smaller" => inherited * 0.83,
            "larger" => inherited * 1.2,
            _ => return None,
        }
    };
    (px > 0.).then_some(px.clamp(6., 96.))
}

/// The size `<font size="…">` gives, 1 to 7 (and `+n`/`-n` from 3).
pub fn font_tag_size(value: &str) -> Option<f32> {
    let v = value.trim();
    let n: i32 = if let Some(rel) = v.strip_prefix('+') {
        3 + rel.parse::<i32>().ok()?
    } else if let Some(rel) = v.strip_prefix('-') {
        3 - rel.parse::<i32>().ok()?
    } else {
        v.parse().ok()?
    };
    Some(match n.clamp(1, 7) {
        1 => 10.,
        2 => 13.,
        3 => 16.,
        4 => 18.,
        5 => 24.,
        6 => 32.,
        _ => 48.,
    })
}

/// The declarations of a `style` attribute, in order, names lowercase.
pub fn declarations(style: &str) -> Vec<(String, String)> {
    let mut out = Vec::new();
    // Semicolons inside quotes or parentheses don't end a declaration.
    let mut current = String::new();
    let mut quote: Option<char> = None;
    let mut depth = 0usize;
    let push = |current: &mut String, out: &mut Vec<(String, String)>| {
        if let Some((name, value)) = current.split_once(':') {
            let name = name.trim().to_ascii_lowercase();
            let value = value.trim().to_owned();
            if !name.is_empty() && !value.is_empty() {
                out.push((name, value));
            }
        }
        current.clear();
    };
    for ch in style.chars() {
        match (quote, ch) {
            (Some(q), c) if c == q => quote = None,
            (Some(_), _) => {}
            (None, '"' | '\'') => quote = Some(ch),
            (None, '(') => depth += 1,
            (None, ')') => depth = depth.saturating_sub(1),
            (None, ';') if depth == 0 => {
                push(&mut current, &mut out);
                continue;
            }
            _ => {}
        }
        current.push(ch);
    }
    push(&mut current, &mut out);
    out
}

/// Declarations written back as a `style` attribute.
pub fn style_attr(declarations: &[(String, String)]) -> String {
    declarations
        .iter()
        .map(|(n, v)| format!("{n}:{v}"))
        .collect::<Vec<_>>()
        .join(";")
}

/// A value that must not reach a sent message or a draft: script URLs and
/// old browsers' CSS expressions.
pub fn unsafe_value(value: &str) -> bool {
    let v: String = value
        .chars()
        .filter(|c| !c.is_whitespace() && !c.is_control())
        .collect::<String>()
        .to_ascii_lowercase();
    v.contains("javascript:")
        || v.contains("vbscript:")
        || v.contains("expression(")
        || v.contains("-moz-binding")
        || v.contains("behavior:")
}

/// The first family of a `font-family` value, without quotes.
pub fn first_family(value: &str) -> Option<String> {
    let first = value.split(',').next()?.trim().trim_matches(['"', '\'']).trim();
    (!first.is_empty()).then(|| first.to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn colors_are_read_in_every_common_form() {
        assert_eq!(color("#f00"), Some(Color::rgb(255, 0, 0)));
        assert_eq!(color("#00ff00"), Some(Color::rgb(0, 255, 0)));
        assert_eq!(color("rgb(1, 2, 3)"), Some(Color::rgb(1, 2, 3)));
        assert_eq!(color("rgba(1,2,3,0.5)").unwrap().a, 128);
        assert_eq!(color("Navy"), Some(Color::rgb(0, 0, 128)));
        assert_eq!(color("windowtext"), None);
        assert_eq!(Color::rgb(255, 0, 0).to_css(), "#ff0000");
    }

    #[test]
    fn sizes_resolve_to_pixels() {
        assert_eq!(font_size("12px", 16.), Some(12.));
        assert_eq!(font_size("12pt", 16.), Some(16.));
        assert_eq!(font_size("150%", 16.), Some(24.));
        assert_eq!(font_size("2em", 10.), Some(20.));
        assert_eq!(font_tag_size("+1"), Some(18.));
    }

    #[test]
    fn declarations_keep_quoted_semicolons() {
        let d = declarations("font-family: \"A;B\", serif; COLOR: red;; background: url(a;b)");
        assert_eq!(d[0], ("font-family".into(), "\"A;B\", serif".into()));
        assert_eq!(d[1], ("color".into(), "red".into()));
        assert_eq!(d[2], ("background".into(), "url(a;b)".into()));
        assert!(unsafe_value("url(java\nscript:alert(1))"));
    }
}
