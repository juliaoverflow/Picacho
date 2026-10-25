use gpui::{AssetSource, SharedString};
use std::borrow::Cow;

pub struct Assets;
impl AssetSource for Assets {
    fn load(&self, path: &str) -> anyhow::Result<Option<Cow<'static, [u8]>>> {
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
            "mail/download.svg" => Some("<path d='M12 3v12m-5-5 5 5 5-5M4 16v5h16v-5'/>"),
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
