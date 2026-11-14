use gpui::{AssetSource, SharedString};
use std::borrow::Cow;

pub struct Assets;
impl AssetSource for Assets {
    fn load(&self, path: &str) -> anyhow::Result<Option<Cow<'static, [u8]>>> {
        if let Some(name) = path.strip_prefix("brand/") {
            return Ok(brand(name).map(Cow::Borrowed));
        }
        if let Some(name) = path
            .strip_prefix("avatars/")
            .and_then(|n| n.strip_suffix(".jpg"))
        {
            return Ok(preset(name).map(Cow::Borrowed));
        }
        let shape = match path {
            "mail/logo.svg" => Some(
                "<path fill='currentColor' stroke='none' d='M2 17v-5a10 10 0 0 1 20 0v5a20 10 0 0 0-6-3v-2a4 4 0 0 0-8 0v2a20 10 0 0 0-6 3Zm0 5a10 5 0 0 1 20 0Z'/>",
            ),
            "mail/mail.svg" => {
                Some("<rect x='3' y='5' width='18' height='14' rx='2'/><path d='m3 6 9 7 9-7'/>")
            }
            "mail/compose.svg" => Some(
                "<path d='M12 4H5a2 2 0 0 0-2 2v13a2 2 0 0 0 2 2h13a2 2 0 0 0 2-2v-7M16 3a2.1 2.1 0 0 1 3 3L10 15l-4 1 1-4Z'/>",
            ),
            "mail/send.svg" => Some("<path d='m22 2-7 20-4-9-9-4Z M22 2 11 13'/>"),
            "mail/archive.svg" => Some(
                "<rect x='3' y='3' width='18' height='4' rx='1'/><path d='M5 7v13h14V7M10 11h4'/>",
            ),
            "mail/trash.svg" => {
                Some("<path d='M3 6h18M9 6V3h6v3M5 6l1 15h12l1-15M10 10v7M14 10v7'/>")
            }
            "mail/reply.svg" => Some("<path d='m9 5-7 7 7 7M2 12h12a7 7 0 0 1 7 7'/>"),
            "mail/reply-all.svg" => {
                Some("<path d='m7 5-6 7 6 7M13 5l-6 7 6 7M7 12h8a7 7 0 0 1 7 7'/>")
            }
            "mail/forward.svg" => Some("<path d='m15 5 7 7-7 7M22 12H10a7 7 0 0 0-7 7'/>"),
            "mail/refresh.svg" => Some(
                "<path d='M20 7a9 9 0 0 0-15-2L2 8M2 3v5h5M4 17a9 9 0 0 0 15 2l3-3M22 21v-5h-5'/>",
            ),
            "mail/paperclip.svg" => Some(
                "<path d='m21 11-9 9a6 6 0 0 1-8-8L14 2a4 4 0 0 1 6 6L10 18a2 2 0 0 1-3-3l9-9'/>",
            ),
            "mail/shield.svg" => {
                Some("<path d='m12 2 9 4v6c0 5-9 10-9 10S3 17 3 12V6Z'/><path d='m8 12 3 3 5-6'/>")
            }
            "mail/spam.svg" => Some("<path d='m8 2-6 6v8l6 6h8l6-6V8l-6-6Z M12 7v6M12 17h.01'/>"),
            "mail/warning.svg" => Some(
                "<path d='m10.3 3.8-8 14A2 2 0 0 0 4 21h16a2 2 0 0 0 1.7-3.2l-8-14a2 2 0 0 0-3.4 0Z'/><path d='M12 9v4M12 17h.01'/>",
            ),
            "mail/download.svg" => Some("<path d='M12 3v12m-5-5 5 5 5-5M4 16v5h16v-5'/>"),
            "mail/star-filled.svg" => Some(
                "<path fill='currentColor' d='m12 2.8 2.8 5.7 6.3.9-4.55 4.43 1.07 6.27L12 17.13 6.38 20.1l1.07-6.27L2.9 9.4l6.3-.9Z'/>",
            ),
            "mail/mail-open.svg" => Some(
                "<path d='M3 10v9a2 2 0 0 0 2 2h14a2 2 0 0 0 2-2v-9L12 3Z'/><path d='m3 10 9 6 9-6'/>",
            ),
            "mail/select.svg" => {
                Some("<rect x='3' y='3' width='18' height='18' rx='4'/><path d='m8 12 3 3 5-6'/>")
            }
            "mail/monitor.svg" => {
                Some("<rect x='2' y='4' width='20' height='13' rx='2'/><path d='M8 21h8M12 17v4'/>")
            }
            "mail/image.svg" => Some(
                "<rect x='3' y='3' width='18' height='18' rx='2'/><circle cx='9' cy='9' r='2'/><path d='m21 15-3.1-3.1a2 2 0 0 0-2.8 0L6 21'/>",
            ),
            "mail/image-off.svg" => Some(
                "<path d='M2 2l20 20M10.4 3H19a2 2 0 0 1 2 2v8.6M21 18.4V19a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2V5c0-.6.2-1 .6-1.4M6 21l7.6-7.6'/>",
            ),
            "mail/code.svg" => Some("<path d='m16 18 6-6-6-6M8 6l-6 6 6 6'/>"),
            "mail/text.svg" => Some("<path d='M4 6h16M4 12h16M4 18h10'/>"),
            "mail/formatted.svg" => {
                Some("<rect x='3' y='3' width='18' height='18' rx='2'/><path d='M3 9h18M9 21V9'/>")
            }
            "mail/lock.svg" => Some(
                "<rect x='4' y='10' width='16' height='11' rx='2'/><path d='M8 10V7a4 4 0 0 1 8 0v3'/>",
            ),
            "mail/filter.svg" => Some(
                "<path d='M4 5h16M7 12h10M10 19h4'/><circle cx='8' cy='5' r='2'/><circle cx='16' cy='12' r='2'/>",
            ),
            "mail/bold.svg" => {
                Some("<path d='M7 5h6a3.5 3.5 0 0 1 0 7H7zM7 12h7a3.5 3.5 0 0 1 0 7H7z'/>")
            }
            "mail/italic.svg" => Some("<path d='M14 5h-4M14 19h-4M14.5 5l-5 14'/>"),
            "mail/underline.svg" => Some("<path d='M7 4v7a5 5 0 0 0 10 0V4M5 20h14'/>"),
            "mail/strike.svg" => Some(
                "<path d='M5 12h14M16.5 7.5A4 3 0 0 0 12 5c-2.5 0-4.5 1.3-4.5 3.2 0 1.2.8 2.2 2.3 2.8M8 16.5c.6 1.5 2.2 2.5 4.2 2.5 2.6 0 4.6-1.4 4.6-3.4 0-.7-.2-1.3-.6-1.8'/>",
            ),
            "mail/list.svg" => Some(
                "<path d='M9 6h11M9 12h11M9 18h11'/><circle cx='4.5' cy='6' r='1' fill='currentColor'/><circle cx='4.5' cy='12' r='1' fill='currentColor'/><circle cx='4.5' cy='18' r='1' fill='currentColor'/>",
            ),
            "mail/list-ordered.svg" => Some(
                "<path d='M10 6h10M10 12h10M10 18h10M4 4.5h1.2V9M3.6 9h2.8M3.6 14.6a1.4 1.4 0 0 1 2.6.7c0 1-2.6 2-2.6 3.2h2.8'/>",
            ),
            "mail/quote.svg" => Some(
                "<path d='M5 7h5v5H6.5A1.5 1.5 0 0 0 5 13.5V17M14 7h5v5h-3.5a1.5 1.5 0 0 0-1.5 1.5V17'/>",
            ),
            "mail/link.svg" => Some(
                "<path d='M10 14a4 4 0 0 0 5.7 0l3-3a4 4 0 0 0-5.7-5.7l-1 1M14 10a4 4 0 0 0-5.7 0l-3 3a4 4 0 0 0 5.7 5.7l1-1'/>",
            ),
            "mail/heading.svg" => Some("<path d='M6 4v16M18 4v16M6 12h12'/>"),
            "mail/save.svg" => Some(
                "<path d='M5 3h11l3 3v13a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2V5a2 2 0 0 1 2-2Z'/><path d='M7 3v5h8V3M7 21v-7h10v7'/>",
            ),
            "mail/popout.svg" => Some(
                "<rect x='3' y='7' width='14' height='14' rx='2'/><path d='M10 3h11v11M21 3l-9 9'/>",
            ),
            "mail/expand.svg" => Some("<path d='M15 3h6v6M9 21H3v-6M21 3l-7 7M3 21l7-7'/>"),
            "mail/shrink.svg" => Some("<path d='M4 14h6v6M20 10h-6V4M14 10l7-7M3 21l7-7'/>"),
            "mail/dock.svg" => Some("<path d='M5 19h14'/>"),
            "mail/undock.svg" => Some("<path d='M5 5h14'/>"),
            "mail/preview.svg" => Some(
                "<path d='M2 12s3.6-7 10-7 10 7 10 7-3.6 7-10 7S2 12 2 12Z'/><circle cx='12' cy='12' r='3'/>",
            ),
            "mail/format.svg" => Some("<path d='M4 20h4M6 20 12 4l6 16M8.3 14h7.4M16 20h4'/>"),
            "mail/contact.svg" => {
                Some("<circle cx='12' cy='8' r='4'/><path d='M4 21a8 8 0 0 1 16 0'/>")
            }
            "mail/thread.svg" => Some(
                "<path d='M4 5h12a2 2 0 0 1 2 2v6a2 2 0 0 1-2 2H9l-4 3v-3H4a2 2 0 0 1-2-2V7a2 2 0 0 1 2-2Z'/><path d='M22 9v8a2 2 0 0 1-2 2h-1v3l-4-3h-3'/>",
            ),
            "mail/upload.svg" => Some("<path d='M12 16V4m-5 5 5-5 5 5M4 16v5h16v-5'/>"),
            "mail/cloud.svg" => Some(
                "<path d='M7 18a5 5 0 0 1-.8-9.94A7 7 0 0 1 19.4 9.6 4.3 4.3 0 0 1 18.5 18Z'/>",
            ),
            "mail/grid.svg" => Some(
                "<rect x='3' y='3' width='7' height='7' rx='1.5'/><rect x='14' y='3' width='7' height='7' rx='1.5'/><rect x='3' y='14' width='7' height='7' rx='1.5'/><rect x='14' y='14' width='7' height='7' rx='1.5'/>",
            ),
            "mail/alias.svg" => Some(
                "<circle cx='12' cy='12' r='4'/><path d='M16 8v5a3 3 0 0 0 6 0v-1a10 10 0 1 0-4 8'/>",
            ),
            "mail/inboxes.svg" => Some(
                "<path d='M22 12h-6l-2 3h-4l-2-3H2'/><path d='M5.5 5.1 2 12v6a2 2 0 0 0 2 2h16a2 2 0 0 0 2-2v-6l-3.5-6.9A2 2 0 0 0 16.8 4H7.2a2 2 0 0 0-1.7 1.1Z'/><path d='M7 1h10'/>",
            ),
            "mail/pin.svg" => Some(
                "<path d='M12 17v5M9 10.8a2 2 0 0 1-1.1 1.8l-1.8.9A2 2 0 0 0 5 15.2V17h14v-1.8a2 2 0 0 0-1.1-1.7l-1.8-.9A2 2 0 0 1 15 10.8V7a1 1 0 0 1 1-1 2 2 0 0 0 0-4H8a2 2 0 0 0 0 4 1 1 0 0 1 1 1Z'/>",
            ),
            "mail/subscriptions.svg" => Some(
                "<path d='M4 4h12v15a2 2 0 0 0 2 2H6a2 2 0 0 1-2-2Z'/><path d='M16 8h4v11a2 2 0 0 1-4 0M8 8h4M8 12h4M8 16h3'/>",
            ),
            "mail/ban.svg" => Some("<circle cx='12' cy='12' r='9'/><path d='m5.6 5.6 12.8 12.8'/>"),
            "mail/tag.svg" => Some(
                "<path d='M12.6 2.6A2 2 0 0 0 11.2 2H4a2 2 0 0 0-2 2v7.2a2 2 0 0 0 .6 1.4l8.7 8.7a2.4 2.4 0 0 0 3.4 0l6.6-6.6a2.4 2.4 0 0 0 0-3.4Z'/><circle cx='7.5' cy='7.5' r='.5'/>",
            ),
            _ => None,
        };
        if let Some(shape) = shape {
            return Ok(Some(Cow::Owned(format!("<svg xmlns='http://www.w3.org/2000/svg' viewBox='0 0 24 24' fill='none' stroke='currentColor' stroke-width='1.65' stroke-linecap='round' stroke-linejoin='round'>{shape}</svg>").into_bytes())));
        }
        gpui_component_assets::Assets.load(path)
    }
    fn list(&self, path: &str) -> anyhow::Result<Vec<SharedString>> {
        gpui_component_assets::Assets.list(path)
    }
}

/// Full-color marks drawn with `img`: the Picacho and Pinal logos and tiles for
/// each mail service in the provider list.
fn brand(name: &str) -> Option<&'static [u8]> {
    let svg: &'static str = match name {
        "picacho.svg" => return Some(include_bytes!("../../assets/brand/picacho.svg")),
        "pinal.svg" => return Some(include_bytes!("../../assets/brand/pinal.svg")),
        "gmail.svg" => {
            "<svg xmlns='http://www.w3.org/2000/svg' viewBox='0 0 48 48'><rect width='48' height='48' rx='12' fill='#fff'/><rect x='.5' y='.5' width='47' height='47' rx='11.5' fill='none' stroke='#dadce0'/><path d='M11 35V17' stroke='#4285F4' stroke-width='5' stroke-linecap='round'/><path d='M37 35V17' stroke='#34A853' stroke-width='5' stroke-linecap='round'/><path d='M11 17l13 10 13-10' fill='none' stroke='#EA4335' stroke-width='5' stroke-linecap='round' stroke-linejoin='round'/></svg>"
        }
        "outlook.svg" => {
            "<svg xmlns='http://www.w3.org/2000/svg' viewBox='0 0 48 48'><rect width='48' height='48' rx='12' fill='#0F6CBD'/><rect x='24' y='14' width='15' height='20' rx='2' fill='#28A8EA'/><rect x='9' y='12' width='22' height='24' rx='4' fill='#0A4F8F'/><ellipse cx='20' cy='24' rx='5.5' ry='7' fill='none' stroke='#fff' stroke-width='3'/></svg>"
        }
        "icloud.svg" => {
            "<svg xmlns='http://www.w3.org/2000/svg' viewBox='0 0 48 48'><defs><linearGradient id='g' x1='0' y1='0' x2='0' y2='1'><stop offset='0' stop-color='#4FB5F6'/><stop offset='1' stop-color='#1C7FE0'/></linearGradient></defs><rect width='48' height='48' rx='12' fill='url(#g)'/><path d='M16 33a6 6 0 0 1-.8-11.95A8.5 8.5 0 0 1 31.5 19.6 6.7 6.7 0 0 1 32.5 33Z' fill='#fff'/></svg>"
        }
        "fastmail.svg" => {
            "<svg xmlns='http://www.w3.org/2000/svg' viewBox='0 0 48 48'><rect width='48' height='48' rx='12' fill='#0067B9'/><rect x='10' y='14' width='28' height='20' rx='3' fill='#fff'/><path d='M11 16l13 10 13-10' fill='none' stroke='#0067B9' stroke-width='2.6' stroke-linejoin='round'/><circle cx='36' cy='33' r='6' fill='#F5A623'/></svg>"
        }
        "zoho.svg" => {
            "<svg xmlns='http://www.w3.org/2000/svg' viewBox='0 0 48 48'><rect width='48' height='48' rx='12' fill='#fff'/><rect x='.5' y='.5' width='47' height='47' rx='11.5' fill='none' stroke='#dadce0'/><rect x='9' y='17' width='7' height='14' rx='1.5' fill='#E42527'/><rect x='17.5' y='15' width='7' height='14' rx='1.5' fill='#089949' transform='rotate(-8 21 22)'/><rect x='26' y='17' width='7' height='14' rx='1.5' fill='#226DB4'/><rect x='34' y='17' width='6' height='14' rx='1.5' fill='#F9B21D'/></svg>"
        }
        "proton.svg" => {
            "<svg xmlns='http://www.w3.org/2000/svg' viewBox='0 0 48 48'><rect width='48' height='48' rx='12' fill='#6D4AFF'/><path d='M15 36V12h11a8 8 0 0 1 0 16h-5v8Z' fill='#fff'/><path d='M21 17h5a3 3 0 0 1 0 6h-5Z' fill='#6D4AFF'/></svg>"
        }
        "other.svg" => {
            "<svg xmlns='http://www.w3.org/2000/svg' viewBox='0 0 48 48'><rect width='48' height='48' rx='12' fill='#8A94A6'/><circle cx='24' cy='24' r='5.5' fill='none' stroke='#fff' stroke-width='3'/><path d='M29.5 24v2.5a4 4 0 0 0 8 0V24a13.5 13.5 0 1 0-5.3 10.7' fill='none' stroke='#fff' stroke-width='3' stroke-linecap='round'/></svg>"
        }
        _ => return None,
    };
    Some(svg.as_bytes())
}

/// Preset account pictures (CC0 photographs; see assets/avatars/README.md),
/// as (name, label, JPEG bytes). Drawn with `img("avatars/<name>.jpg")`.
pub const PRESETS: [(&str, &str, &[u8]); 10] = [
    (
        "tulip",
        "Tulips",
        include_bytes!("../../assets/avatars/tulip.jpg"),
    ),
    (
        "butterfly",
        "Butterfly",
        include_bytes!("../../assets/avatars/butterfly.jpg"),
    ),
    (
        "beach",
        "Beach",
        include_bytes!("../../assets/avatars/beach.jpg"),
    ),
    (
        "leaf",
        "Dewdrops",
        include_bytes!("../../assets/avatars/leaf.jpg"),
    ),
    (
        "kitten",
        "Kitten",
        include_bytes!("../../assets/avatars/kitten.jpg"),
    ),
    (
        "penguin",
        "Penguin",
        include_bytes!("../../assets/avatars/penguin.jpg"),
    ),
    (
        "chess",
        "Chess",
        include_bytes!("../../assets/avatars/chess.jpg"),
    ),
    (
        "guitar",
        "Guitar",
        include_bytes!("../../assets/avatars/guitar.jpg"),
    ),
    (
        "duck",
        "Rubber Duck",
        include_bytes!("../../assets/avatars/duck.jpg"),
    ),
    (
        "sunflower",
        "Sunflower",
        include_bytes!("../../assets/avatars/sunflower.jpg"),
    ),
];

pub fn preset(name: &str) -> Option<&'static [u8]> {
    PRESETS
        .iter()
        .find(|(n, ..)| *n == name)
        .map(|(_, _, b)| *b)
}
