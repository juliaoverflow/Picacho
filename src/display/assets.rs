use gpui::{AssetSource, SharedString};
use std::borrow::Cow;

pub struct Assets;
impl AssetSource for Assets {
    fn load(&self, path: &str) -> anyhow::Result<Option<Cow<'static, [u8]>>> {
        if let Some(name) = path.strip_prefix("brand/") {
            return Ok(brand(name).map(Cow::Borrowed));
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
