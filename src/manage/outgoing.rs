//! What a draft's attachments become as it is sent. Photos larger than the
//! draft's picture size are made smaller (their orientation applied, their
//! metadata, such as where they were taken, left behind). A message too
//! large for its outgoing server, with cloud storage set up (Settings →
//! Composing), has its largest files shared as links instead: uploaded to
//! Nextcloud or a WebDAV folder, and listed with their links in the text.
//!
//! The draft itself is unchanged: what is sent is a copy, its smaller
//! pictures in a folder of their own beside the database (sealed while
//! the database is encrypted), removed once sent.
use crate::manage::composing::{CloudKind, CloudStorage, ComposeOptions};
use crate::manage::model::Draft;
use crate::manage::richtext::{self as rt, html as rhtml};
use anyhow::{Context, Result, bail, ensure};
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::time::Duration;

/// The keychain entry with the cloud storage's password.
pub const CLOUD_SECRET: &str = "cloud-storage";

/// A file shared as a link in place of its attachment.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Shared {
    pub name: String,
    pub size: u64,
    pub url: String,
}

/// A draft as it is sent, and the files made for it.
pub struct Prepared {
    pub draft: Draft,
    /// Attachments shared as links.
    pub shared: Vec<Shared>,
    /// Pictures made smaller.
    pub resized: usize,
    folder: Option<PathBuf>,
}

impl Drop for Prepared {
    fn drop(&mut self) {
        if let Some(folder) = self.folder.take() {
            let _ = std::fs::remove_dir_all(folder);
        }
    }
}

/// Roughly how large `d` is as a message: its text, and its attachments
/// as base64 makes them (4 bytes for 3, line breaks).
pub fn estimate(d: &Draft) -> usize {
    let files: u64 = d.attachments.iter().map(|p| crate::manage::sealed_files::len(p).unwrap_or(0)).sum();
    d.body.len() * 2 + d.html.len() + (files as usize) * 137 / 100 + 8 * 1024
}

/// `d` ready to send: its pictures at its picture size, and, when
/// `share_over` is given, as many of its largest attachments shared
/// through `share` (local file, name → link) as it takes to bring it under
/// that many bytes. Encrypted messages are never shared as links: the
/// files would be readable where they're stored. `dir` is where smaller
/// pictures are kept until sent (the database's folder).
pub fn prepare(
    dir: &Path,
    options: &ComposeOptions,
    d: &Draft,
    share_over: Option<usize>,
    share: &dyn Fn(&Path, &str) -> Result<String>,
) -> Result<Prepared> {
    let mut prepared = Prepared { draft: d.clone(), shared: vec![], resized: 0, folder: None };
    let size = d.image_size.unwrap_or(options.image_size);
    if let Some(max) = size.max_side() {
        for i in 0..prepared.draft.attachments.len() {
            let path = prepared.draft.attachments[i].clone();
            let Some(smaller) = smaller_picture(&path, max) else { continue };
            let folder = match &prepared.folder {
                Some(f) => f.clone(),
                None => {
                    let f = dir.join("outgoing").join(uuid::Uuid::new_v4().simple().to_string());
                    std::fs::create_dir_all(&f)?;
                    prepared.folder = Some(f.clone());
                    f
                }
            };
            // The same name: a formatted message's pictures name their file.
            let Some(name) = path.file_name() else { continue };
            let target = folder.join(name);
            crate::manage::sealed_files::write(&target, &smaller)?;
            prepared.draft.attachments[i] = target;
            prepared.resized += 1;
        }
    }
    let Some(limit) = share_over.filter(|_| !d.encrypt) else {
        return Ok(prepared);
    };
    if estimate(&prepared.draft) <= limit {
        return Ok(prepared);
    }
    // Pictures shown in the text stay: the text names them.
    let shown = if d.formatted && !d.html.is_empty() { rhtml::parse(&d.html, true).file_images() } else { vec![] };
    let mut files: Vec<(PathBuf, PathBuf, u64)> = prepared
        .draft
        .attachments
        .iter()
        .zip(&d.attachments)
        .filter(|(p, _)| !p.file_name().is_some_and(|n| shown.iter().any(|s| *s == n.to_string_lossy())))
        .map(|(p, original)| (p.clone(), original.clone(), crate::manage::sealed_files::len(p).unwrap_or(0)))
        .collect();
    files.sort_by_key(|f| std::cmp::Reverse(f.2));
    let mut shared = Vec::new();
    for (path, _, len) in files {
        if estimate(&prepared.draft) <= limit {
            break;
        }
        let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        let url = share(&path, &name).with_context(|| t!("manage-outgoing-cloud-couldnt-share", name = name))?;
        prepared.draft.attachments.retain(|p| p != &path);
        shared.push(Shared { name, size: len, url });
    }
    if !shared.is_empty() {
        add_links(&mut prepared.draft, &shared);
    }
    prepared.shared = shared;
    Ok(prepared)
}

/// The picture at `path` made at most `max` pixels a side, if it is a
/// photo larger than that: JPEG and WebP pictures, and PNGs over a
/// megabyte (screenshots smaller than that stay sharp). `None` when it
/// stays as it is (smaller already, not a picture, or not smaller made so).
pub fn smaller_picture(path: &Path, max: u32) -> Option<Vec<u8>> {
    let ext = path.extension()?.to_string_lossy().to_ascii_lowercase();
    if !matches!(ext.as_str(), "jpg" | "jpeg" | "png" | "webp") {
        return None;
    }
    let len = crate::manage::sealed_files::len(path).ok()?;
    if ext == "png" && len < 1024 * 1024 {
        return None;
    }
    let bytes = crate::manage::sealed_files::read(path).ok()?;
    downscale(&bytes, max)
}

/// `bytes` (a JPEG, PNG or WebP) made at most `max` pixels a side, in the
/// same format, its orientation applied; `None` if it is that small
/// already or wouldn't be smaller.
pub fn downscale(bytes: &[u8], max: u32) -> Option<Vec<u8>> {
    use image::{ImageDecoder as _, ImageReader};
    let limits = || {
        let mut l = image::Limits::default();
        l.max_image_width = Some(20_000);
        l.max_image_height = Some(20_000);
        l.max_alloc = Some(512 * 1024 * 1024);
        l
    };
    let mut reader = ImageReader::new(std::io::Cursor::new(bytes)).with_guessed_format().ok()?;
    reader.limits(limits());
    let format = reader.format()?;
    let (w, h) = reader.into_dimensions().ok()?;
    if w.max(h) <= max {
        return None;
    }
    let mut reader = ImageReader::new(std::io::Cursor::new(bytes)).with_guessed_format().ok()?;
    reader.limits(limits());
    let mut decoder = reader.into_decoder().ok()?;
    let orientation = decoder.orientation().unwrap_or(image::metadata::Orientation::NoTransforms);
    let mut picture = image::DynamicImage::from_decoder(decoder).ok()?;
    picture.apply_orientation(orientation);
    let picture = picture.resize(max, max, image::imageops::FilterType::CatmullRom);
    let alpha = picture.color().has_alpha();
    let mut out = Vec::new();
    match format {
        image::ImageFormat::Jpeg => {
            let encoder = image::codecs::jpeg::JpegEncoder::new_with_quality(&mut out, 85);
            picture.to_rgb8().write_with_encoder(encoder).ok()?;
        }
        image::ImageFormat::Png => {
            let encoder = image::codecs::png::PngEncoder::new_with_quality(
                &mut out,
                image::codecs::png::CompressionType::Best,
                image::codecs::png::FilterType::Adaptive,
            );
            if alpha {
                picture.to_rgba8().write_with_encoder(encoder).ok()?;
            } else {
                picture.to_rgb8().write_with_encoder(encoder).ok()?;
            }
        }
        image::ImageFormat::WebP => {
            let encoder = image::codecs::webp::WebPEncoder::new_lossless(&mut out);
            if alpha {
                picture.to_rgba8().write_with_encoder(encoder).ok()?;
            } else {
                picture.to_rgb8().write_with_encoder(encoder).ok()?;
            }
        }
        _ => return None,
    }
    (out.len() < bytes.len()).then_some(out)
}

/// A size as people read it, in the language of outgoing mail.
fn size_text(bytes: u64) -> String {
    if bytes >= 1024 * 1024 {
        format!("{:.1} MB", bytes as f64 / 1_048_576.)
    } else {
        format!("{} KB", bytes.div_ceil(1024).max(1))
    }
}

/// Lists `shared` with their links in `d`'s text, before its signature and
/// any quoted message: in its HTML, its text, or its Markdown.
pub fn add_links(d: &mut Draft, shared: &[Shared]) {
    let heading = crate::i18n::in_outgoing(|| t!("outgoing-cloud-shared-as-links").to_owned());
    if d.formatted && !d.html.is_empty() {
        let mut doc = rhtml::parse(&d.html, true);
        let at = doc.signature_range().map(|r| r.start).or(doc.quote_start()).unwrap_or(doc.blocks.len());
        let mut html = format!("<p>{}</p><ul>", escape(&heading));
        for s in shared {
            html.push_str(&format!("<li><a href=\"{}\">{}</a> ({})</li>", escape(&s.url), escape(&s.name), size_text(s.size)));
        }
        html.push_str("</ul>");
        let mut blocks: Vec<Rc<rt::Block>> = rhtml::parse(&html, false).blocks;
        // A blank line keeps it apart from what follows.
        if at < doc.blocks.len() {
            blocks.push(Rc::new(rt::Block::paragraph("div", "")));
        }
        doc.blocks.splice(at..at, blocks);
        d.html = rhtml::to_html(&doc);
        d.body = doc.to_plain();
        return;
    }
    // Markdown (a formatted draft without HTML) or plain text.
    let lines: Vec<String> = shared
        .iter()
        .map(|s| {
            if d.formatted {
                format!("- [{}]({}) ({})", s.name.replace(['[', ']'], ""), s.url, size_text(s.size))
            } else {
                format!("- {} ({}): {}", s.name, size_text(s.size), s.url)
            }
        })
        .collect();
    let section = format!("{heading}\n{}", lines.join("\n"));
    d.body = insert_before_tail(&d.body, &section);
}

/// `body` with `section` before its signature or quoted message (or at its
/// end), a blank line around it.
fn insert_before_tail(body: &str, section: &str) -> String {
    let mut at = None;
    let mut offset = 0;
    for line in body.split_inclusive('\n') {
        let text = line.trim_end_matches(['\n', '\r']);
        if text == "-- " || crate::manage::composing::introduces_quote(text) || crate::manage::composing::starts_forward(text) {
            at = Some(offset);
            break;
        }
        offset += line.len();
    }
    match at {
        Some(at) => {
            let before = body[..at].trim_end();
            format!("{before}\n\n{section}\n\n{}", &body[at..])
        }
        None => format!("{}\n\n{section}\n", body.trim_end()),
    }
}

fn escape(s: &str) -> String {
    s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;").replace('"', "&quot;")
}

/// Uploads files to the cloud storage and makes links to them.
pub struct Cloud {
    http: reqwest::blocking::Client,
    storage: CloudStorage,
    password: String,
    /// The folder this message's files go in (made on the first).
    folder: std::cell::RefCell<Option<Vec<String>>>,
}

impl Cloud {
    pub fn new(storage: &CloudStorage, password: &str, tls: native_tls::TlsConnector) -> Result<Self> {
        ensure!(storage.configured(), t!("manage-outgoing-cloud-not-set-up"));
        let http = reqwest::blocking::Client::builder()
            .use_preconfigured_tls(tls)
            .timeout(Duration::from_secs(600))
            .connect_timeout(Duration::from_secs(15))
            // Redirects only on the same server, so the password stays there.
            .redirect(reqwest::redirect::Policy::custom(|attempt| {
                let same = attempt.previous().first().is_some_and(|first| {
                    first.host_str() == attempt.url().host_str() && first.port_or_known_default() == attempt.url().port_or_known_default()
                });
                if same && attempt.previous().len() < 5 { attempt.follow() } else { attempt.stop() }
            }))
            .user_agent(concat!("Picacho/", env!("CARGO_PKG_VERSION")))
            // The password never goes over plain http.
            .https_only(!cfg!(test))
            .build()?;
        Ok(Self { http, storage: storage.clone(), password: password.to_owned(), folder: Default::default() })
    }

    fn base(&self) -> Result<url::Url> {
        let mut text = self.storage.url.trim().to_owned();
        if !text.contains("://") {
            text = format!("https://{text}");
        }
        let url = url::Url::parse(&text).with_context(|| t!("manage-outgoing-cloud-the-address-isnt-valid"))?;
        ensure!(matches!(url.scheme(), "https" | "http"), t!("manage-outgoing-cloud-the-address-isnt-valid"));
        Ok(url)
    }

    /// The WebDAV folder files go under, with `segments` after it.
    fn dav(&self, segments: &[String]) -> Result<url::Url> {
        let mut url = self.base()?;
        {
            let mut path = url.path_segments_mut().map_err(|_| anyhow::anyhow!(t!("manage-outgoing-cloud-the-address-isnt-valid")))?;
            path.pop_if_empty();
            if self.storage.kind == CloudKind::Nextcloud {
                path.extend(["remote.php", "dav", "files", self.storage.user.trim()]);
            }
            path.extend(segments);
        }
        Ok(url)
    }

    fn request(&self, method: &str, url: url::Url) -> Result<reqwest::blocking::RequestBuilder> {
        let method = reqwest::Method::from_bytes(method.as_bytes())?;
        Ok(self.http.request(method, url).basic_auth(self.storage.user.trim(), Some(&self.password)))
    }

    fn check_status(response: reqwest::blocking::Response, ok: &[u16]) -> Result<reqwest::blocking::Response> {
        let status = response.status().as_u16();
        if ok.contains(&status) || response.status().is_success() {
            return Ok(response);
        }
        match status {
            401 | 403 => bail!(t!("manage-outgoing-cloud-sign-in-failed")),
            507 => bail!(t!("manage-outgoing-cloud-full")),
            _ => bail!(t!("manage-outgoing-cloud-server-answered", status = status)),
        }
    }

    /// Checks the storage can be reached and signed in to.
    pub fn check(&self) -> Result<()> {
        let response = self.request("PROPFIND", self.dav(&[])?)?.header("Depth", "0").send()?;
        Self::check_status(response, &[207]).map(|_| ())
    }

    /// The folder this message's files go in: the storage's folder, and in
    /// it one for today and this message.
    fn folder(&self) -> Result<Vec<String>> {
        if let Some(f) = self.folder.borrow().clone() {
            return Ok(f);
        }
        let mut segments: Vec<String> = self.storage.folder.split('/').map(str::trim).filter(|s| !s.is_empty() && *s != "." && *s != "..").map(str::to_owned).collect();
        if segments.is_empty() {
            segments.push("Picacho".into());
        }
        let today = chrono::Local::now().format("%Y-%m-%d").to_string();
        segments.push(format!("{today}-{}", &uuid::Uuid::new_v4().simple().to_string()[..8]));
        for depth in 1..=segments.len() {
            let response = self.request("MKCOL", self.dav(&segments[..depth])?)?.send()?;
            // 405: it's there already.
            Self::check_status(response, &[201, 405])?;
        }
        *self.folder.borrow_mut() = Some(segments.clone());
        Ok(segments)
    }

    /// Uploads `local` as `name` and returns its link.
    pub fn share(&self, local: &Path, name: &str) -> Result<String> {
        let name = crate::manage::backend::opened::safe_file_name(name);
        let folder = self.folder()?;
        let mut segments = folder.clone();
        segments.push(name.clone());
        let bytes = crate::manage::sealed_files::read(local)?;
        let url = self.dav(&segments)?;
        let response = self.request("PUT", url.clone())?.body(bytes).send()?;
        Self::check_status(response, &[200, 201, 204])?;
        match self.storage.kind {
            CloudKind::Nextcloud => self.nextcloud_link(&segments),
            _ => {
                let public = self.storage.public_url.trim();
                if public.is_empty() {
                    return Ok(url.to_string());
                }
                let mut link = url::Url::parse(public).with_context(|| t!("manage-outgoing-cloud-the-address-isnt-valid"))?;
                link.path_segments_mut()
                    .map_err(|_| anyhow::anyhow!(t!("manage-outgoing-cloud-the-address-isnt-valid")))?
                    .pop_if_empty()
                    .extend(&segments);
                Ok(link.to_string())
            }
        }
    }

    /// A public, read-only link to a file Nextcloud has (its sharing API).
    fn nextcloud_link(&self, segments: &[String]) -> Result<String> {
        let mut url = self.base()?;
        url.path_segments_mut()
            .map_err(|_| anyhow::anyhow!(t!("manage-outgoing-cloud-the-address-isnt-valid")))?
            .pop_if_empty()
            .extend(["ocs", "v2.php", "apps", "files_sharing", "api", "v1", "shares"]);
        url.query_pairs_mut().append_pair("format", "json");
        let path = format!("/{}", segments.join("/"));
        let mut form = vec![("path", path), ("shareType", "3".into()), ("permissions", "1".into())];
        if self.storage.expire_days > 0 {
            let until = chrono::Local::now().date_naive() + chrono::Days::new(u64::from(self.storage.expire_days));
            form.push(("expireDate", until.format("%Y-%m-%d").to_string()));
        }
        let response = self
            .request("POST", url)?
            .header("OCS-APIRequest", "true")
            .header("Accept", "application/json")
            .form(&form)
            .send()?;
        let status = response.status();
        let answer: serde_json::Value = response.json().unwrap_or_default();
        // Refused (a policy wants links to have passwords, sharing is off):
        // what Nextcloud says why.
        if let Some(message) = answer["ocs"]["meta"]["message"].as_str().filter(|m| !m.trim().is_empty() && !status.is_success()) {
            bail!(t!("manage-outgoing-cloud-server-said", message = message.trim()));
        }
        if !status.is_success() {
            bail!(t!("manage-outgoing-cloud-server-answered", status = status.as_u16()));
        }
        nextcloud_url(&answer).with_context(|| t!("manage-outgoing-cloud-no-link"))
    }
}

/// Google Drive's and Microsoft Graph's addresses.
pub const DRIVE_BASE: &str = "https://www.googleapis.com";
pub const GRAPH_BASE: &str = "https://graph.microsoft.com/v1.0";

/// Uploads files to a mail account's Google Drive or OneDrive, signed in
/// with its token, and makes links anyone with them can open.
pub struct AccountCloud {
    http: reqwest::blocking::Client,
    storage: CloudStorage,
    token: String,
    /// `DRIVE_BASE` or `GRAPH_BASE` (another server in tests).
    base: String,
    /// Drive: the folder's id. OneDrive: this message's folder's path.
    folder: std::cell::RefCell<Option<Vec<String>>>,
}

/// Bytes OneDrive takes in one piece of an upload: a multiple of 320 KiB.
const ONEDRIVE_PIECE: usize = 10 * 320 * 1024;

impl AccountCloud {
    pub fn new(storage: &CloudStorage, token: &str, base: &str, tls: native_tls::TlsConnector) -> Result<Self> {
        ensure!(storage.kind.uses_account() && !token.is_empty(), t!("manage-outgoing-cloud-not-set-up"));
        let http = reqwest::blocking::Client::builder()
            .use_preconfigured_tls(tls)
            .timeout(Duration::from_secs(600))
            .connect_timeout(Duration::from_secs(15))
            .user_agent(concat!("Picacho/", env!("CARGO_PKG_VERSION")))
            .https_only(!cfg!(test))
            .build()?;
        Ok(Self { http, storage: storage.clone(), token: token.to_owned(), base: base.trim_end_matches('/').to_owned(), folder: Default::default() })
    }

    fn folder_name(&self) -> String {
        let name = self.storage.folder.trim().trim_matches('/');
        if name.is_empty() { "Picacho".into() } else { name.to_owned() }
    }

    fn send(request: reqwest::blocking::RequestBuilder) -> Result<serde_json::Value> {
        let response = request.send()?;
        if !response.status().is_success() {
            return Err(Self::refused(response));
        }
        Ok(response.json().unwrap_or_default())
    }

    /// What a refusal says: signed out, full, or the service's message.
    fn refused(response: reqwest::blocking::Response) -> anyhow::Error {
        let status = response.status().as_u16();
        let answer: serde_json::Value = response.json().unwrap_or_default();
        let message = answer["error"]["message"].as_str().unwrap_or_default().trim().to_owned();
        match status {
            401 => anyhow::anyhow!(t!("manage-outgoing-cloud-sign-in-again")),
            403 if message.is_empty() => anyhow::anyhow!(t!("manage-outgoing-cloud-sign-in-again")),
            507 => anyhow::anyhow!(t!("manage-outgoing-cloud-full")),
            _ if !message.is_empty() => anyhow::anyhow!(t!("manage-outgoing-cloud-server-said", message = message)),
            code => anyhow::anyhow!(t!("manage-outgoing-cloud-server-answered", status = code)),
        }
    }

    /// Checks the storage can be reached with the account's sign-in.
    pub fn check(&self) -> Result<()> {
        let url = match self.storage.kind {
            CloudKind::GoogleDrive => format!("{}/drive/v3/about?fields=user", self.base),
            _ => format!("{}/me/drive", self.base),
        };
        Self::send(self.http.get(url).bearer_auth(&self.token)).map(|_| ())
    }

    pub fn share(&self, local: &Path, name: &str) -> Result<String> {
        let name = crate::manage::backend::opened::safe_file_name(name);
        let bytes = crate::manage::sealed_files::read(local)?;
        match self.storage.kind {
            CloudKind::GoogleDrive => self.share_drive(&name, bytes),
            _ => self.share_onedrive(&name, bytes),
        }
    }

    /// Drive's folder for Picacho's files (with `drive.file`, Drive shows
    /// Picacho only what Picacho made): found, or made.
    fn drive_folder(&self) -> Result<String> {
        if let Some(f) = self.folder.borrow().as_ref().and_then(|f| f.first().cloned()) {
            return Ok(f);
        }
        let name = self.folder_name().replace('\\', "\\\\").replace('\'', "\\'");
        let query = format!("name = '{name}' and mimeType = 'application/vnd.google-apps.folder' and trashed = false");
        let mut url = url::Url::parse(&format!("{}/drive/v3/files", self.base))?;
        url.query_pairs_mut().append_pair("q", &query).append_pair("fields", "files(id)").append_pair("spaces", "drive");
        let found = Self::send(self.http.get(url).bearer_auth(&self.token))?;
        let id = match found["files"][0]["id"].as_str() {
            Some(id) => id.to_owned(),
            None => {
                let made = Self::send(
                    self.http
                        .post(format!("{}/drive/v3/files?fields=id", self.base))
                        .bearer_auth(&self.token)
                        .json(&serde_json::json!({"name": self.folder_name(), "mimeType": "application/vnd.google-apps.folder"})),
                )?;
                made["id"].as_str().context(t!("manage-outgoing-cloud-no-link"))?.to_owned()
            }
        };
        *self.folder.borrow_mut() = Some(vec![id.clone()]);
        Ok(id)
    }

    fn share_drive(&self, name: &str, bytes: Vec<u8>) -> Result<String> {
        let folder = self.drive_folder()?;
        // A resumable upload: the file's details, then its bytes where told.
        let response = self
            .http
            .post(format!("{}/upload/drive/v3/files?uploadType=resumable&fields=id,webViewLink", self.base))
            .bearer_auth(&self.token)
            .header("X-Upload-Content-Length", bytes.len().to_string())
            .json(&serde_json::json!({"name": name, "parents": [folder]}))
            .send()?;
        if !response.status().is_success() {
            return Err(Self::refused(response));
        }
        let location = response
            .headers()
            .get("location")
            .and_then(|v| v.to_str().ok())
            .context(t!("manage-outgoing-cloud-no-link"))?
            .to_owned();
        let file = Self::send(self.http.put(location).bearer_auth(&self.token).body(bytes))?;
        let id = file["id"].as_str().context(t!("manage-outgoing-cloud-no-link"))?.to_owned();
        Self::send(
            self.http
                .post(format!("{}/drive/v3/files/{id}/permissions", self.base))
                .bearer_auth(&self.token)
                .json(&serde_json::json!({"role": "reader", "type": "anyone"})),
        )?;
        Ok(file["webViewLink"].as_str().map(str::to_owned).unwrap_or_else(|| format!("https://drive.google.com/file/d/{id}/view")))
    }

    fn share_onedrive(&self, name: &str, bytes: Vec<u8>) -> Result<String> {
        let folder = {
            let mut f = self.folder.borrow_mut();
            f.get_or_insert_with(|| {
                let mut segments: Vec<String> = self.folder_name().split('/').map(str::trim).filter(|s| !s.is_empty() && *s != "..").map(str::to_owned).collect();
                let today = chrono::Local::now().format("%Y-%m-%d").to_string();
                segments.push(format!("{today}-{}", &uuid::Uuid::new_v4().simple().to_string()[..8]));
                segments
            })
            .clone()
        };
        // /me/drive/root:/<folder>/<name>:/createUploadSession
        let mut url = url::Url::parse(&self.base)?;
        {
            let mut path = url.path_segments_mut().map_err(|_| anyhow::anyhow!(t!("manage-outgoing-cloud-the-address-isnt-valid")))?;
            path.pop_if_empty().extend(["me", "drive", "root:"]);
            path.extend(&folder);
            path.extend([format!("{name}:"), "createUploadSession".to_owned()]);
        }
        let session = Self::send(
            self.http
                .post(url)
                .bearer_auth(&self.token)
                .json(&serde_json::json!({"item": {"@microsoft.graph.conflictBehavior": "rename"}})),
        )?;
        let upload = session["uploadUrl"].as_str().context(t!("manage-outgoing-cloud-no-link"))?.to_owned();
        let total = bytes.len();
        let mut item = serde_json::Value::Null;
        let mut at = 0;
        // The upload address is its own authorization: no token is sent.
        while at < total || total == 0 {
            let end = (at + ONEDRIVE_PIECE).min(total);
            let piece = bytes[at..end].to_vec();
            let range = if total == 0 { "bytes */0".to_owned() } else { format!("bytes {at}-{}/{total}", end - 1) };
            item = Self::send(self.http.put(&upload).header("Content-Range", range).body(piece))?;
            at = end;
            if total == 0 {
                break;
            }
        }
        let id = item["id"].as_str().context(t!("manage-outgoing-cloud-no-link"))?.to_owned();
        let mut asked = serde_json::json!({"type": "view", "scope": "anonymous"});
        if self.storage.expire_days > 0 {
            let until = chrono::Utc::now() + chrono::Duration::days(i64::from(self.storage.expire_days));
            asked["expirationDateTime"] = until.to_rfc3339_opts(chrono::SecondsFormat::Secs, true).into();
        }
        let link_url = format!("{}/me/drive/items/{id}/createLink", self.base);
        // Organizations may allow links only within them.
        let link = match Self::send(self.http.post(&link_url).bearer_auth(&self.token).json(&asked)) {
            Ok(link) => link,
            Err(_) => {
                asked["scope"] = "organization".into();
                if let Some(o) = asked.as_object_mut() {
                    o.remove("expirationDateTime");
                }
                Self::send(self.http.post(&link_url).bearer_auth(&self.token).json(&asked))?
            }
        };
        link["link"]["webUrl"].as_str().map(str::to_owned).context(t!("manage-outgoing-cloud-no-link"))
    }
}

/// Where a message's large files are shared from.
pub enum Sharer {
    Dav(Cloud),
    Account(AccountCloud),
}

impl Sharer {
    pub fn share(&self, local: &Path, name: &str) -> Result<String> {
        match self {
            Sharer::Dav(c) => c.share(local, name),
            Sharer::Account(c) => c.share(local, name),
        }
    }
    pub fn check(&self) -> Result<()> {
        match self {
            Sharer::Dav(c) => c.check(),
            Sharer::Account(c) => c.check(),
        }
    }
}

/// The link in a Nextcloud share's answer (OCS: `ocs.data.url`).
fn nextcloud_url(answer: &serde_json::Value) -> Option<String> {
    let data = &answer["ocs"]["data"];
    let data = if data.is_array() { &data[0] } else { data };
    data["url"].as_str().filter(|u| u.starts_with("https://") || u.starts_with("http://")).map(str::to_owned)
}

#[cfg(test)]
/// A Nextcloud server for one test: what it was asked, by method and
/// path, and the answers it gives.
pub(crate) fn test_nextcloud(answer_share: &'static str) -> (String, std::sync::Arc<std::sync::Mutex<Vec<String>>>) {
    test_server(move |method, _, _| match method {
        "MKCOL" | "PUT" => ("201 Created".into(), String::new(), String::new()),
        "POST" => ("200 OK".into(), String::new(), answer_share.to_owned()),
        _ => ("405 Method Not Allowed".into(), String::new(), String::new()),
    })
}

/// A web server for one test, answering each request with `answer`
/// (method, path, its own address → status, more headers, body), and what
/// it was asked: "METHOD path body-length auth" (auth: basic, bearer or
/// none), with its Content-Range after where it had one.
#[cfg(test)]
pub(crate) fn test_server(
    answer: impl Fn(&str, &str, &str) -> (String, String, String) + Send + Sync + 'static,
) -> (String, std::sync::Arc<std::sync::Mutex<Vec<String>>>) {
    use std::io::{BufRead, BufReader, Read, Write};
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let address = format!("http://{}", listener.local_addr().unwrap());
    let seen = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
    let log = seen.clone();
    let answer = std::sync::Arc::new(answer);
    let own = address.clone();
    std::thread::spawn(move || {
        for stream in listener.incoming().flatten() {
            let (log, answer, own) = (log.clone(), answer.clone(), own.clone());
            std::thread::spawn(move || {
                let mut reader = BufReader::new(stream.try_clone().unwrap());
                let mut stream = stream;
                loop {
                    let mut line = String::new();
                    if reader.read_line(&mut line).unwrap_or(0) == 0 {
                        return;
                    }
                    let mut length = 0;
                    let mut auth = "none";
                    let mut range = String::new();
                    loop {
                        let mut header = String::new();
                        reader.read_line(&mut header).unwrap();
                        let h = header.trim().to_ascii_lowercase();
                        if h.is_empty() {
                            break;
                        }
                        if let Some(v) = h.strip_prefix("content-length:") {
                            length = v.trim().parse().unwrap();
                        }
                        if let Some(v) = h.strip_prefix("content-range:") {
                            range = format!(" {}", v.trim());
                        }
                        if h.starts_with("authorization: basic") {
                            auth = "basic";
                        } else if h.starts_with("authorization: bearer") {
                            auth = "bearer";
                        }
                    }
                    let mut body = vec![0; length];
                    reader.read_exact(&mut body).unwrap();
                    let mut words = line.split_whitespace();
                    let (method, path) = (words.next().unwrap().to_owned(), words.next().unwrap().to_owned());
                    log.lock().unwrap().push(format!("{method} {path} {} {auth}{range}", body.len()));
                    let (status, headers, text) = answer(&method, &path, &own);
                    let _ = write!(
                        stream,
                        "HTTP/1.1 {status}\r\nContent-Length: {}\r\nContent-Type: application/json\r\n{headers}\r\n{text}",
                        text.len()
                    );
                }
            });
        }
    });
    (address, seen)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn photo(w: u32, h: u32) -> Vec<u8> {
        let mut img = image::RgbImage::new(w, h);
        for (x, y, p) in img.enumerate_pixels_mut() {
            *p = image::Rgb([(x * 255 / w) as u8, (y * 255 / h) as u8, 128]);
        }
        let mut out = Vec::new();
        image::codecs::jpeg::JpegEncoder::new_with_quality(&mut out, 95).encode_image(&img).unwrap();
        out
    }

    #[test]
    fn large_photos_are_made_smaller_and_small_ones_left() {
        let big = photo(1600, 1200);
        let smaller = downscale(&big, 800).expect("made smaller");
        assert!(smaller.len() < big.len());
        assert_eq!(image::load_from_memory(&smaller).unwrap().width(), 800);
        assert_eq!(image::load_from_memory(&smaller).unwrap().height(), 600);
        assert!(downscale(&photo(400, 300), 800).is_none());
        assert!(downscale(b"not a picture", 800).is_none());
    }

    #[test]
    fn pictures_and_large_files_are_prepared_for_sending() {
        let dir = std::env::temp_dir().join(format!("picacho-outgoing-{}", uuid::Uuid::new_v4().simple()));
        std::fs::create_dir_all(&dir).unwrap();
        let photo_path = dir.join("beach.jpg");
        std::fs::write(&photo_path, photo(3000, 2000)).unwrap();
        let big = dir.join("video.mov");
        std::fs::write(&big, vec![7u8; 300_000]).unwrap();
        let d = Draft {
            id: "x".into(),
            body: "Hi\n\n-- \nSam".into(),
            attachments: vec![photo_path.clone(), big.clone()],
            ..Draft::default()
        };
        let options = ComposeOptions::default();
        let uploaded = std::cell::RefCell::new(Vec::new());
        let share = |p: &Path, name: &str| {
            uploaded.borrow_mut().push(p.to_path_buf());
            Ok(format!("https://cloud.example.org/s/{name}"))
        };
        let prepared = prepare(&dir, &options, &d, Some(200_000), &share).unwrap();
        assert_eq!(prepared.resized, 1);
        let sent_photo = &prepared.draft.attachments[0];
        assert_ne!(sent_photo, &photo_path);
        assert_eq!(sent_photo.file_name(), photo_path.file_name(), "the same name");
        assert_eq!(image::image_dimensions(sent_photo).unwrap(), (2048, 1365));
        assert_eq!(uploaded.borrow().as_slice(), std::slice::from_ref(&big));
        assert_eq!(prepared.draft.attachments.len(), 1);
        assert_eq!(prepared.shared[0].url, "https://cloud.example.org/s/video.mov");
        assert!(
            prepared.draft.body.starts_with("Hi\n\nFiles shared as links:\n- video.mov (293 KB): https://cloud.example.org/s/video.mov\n\n-- \nSam"),
            "{}",
            prepared.draft.body
        );
        let folder = sent_photo.parent().unwrap().to_path_buf();
        drop(prepared);
        assert!(!folder.exists(), "its smaller pictures go once sent");
        // Encrypted: nothing is shared; at its actual size: nothing resized.
        let encrypted = Draft { encrypt: true, image_size: Some(crate::manage::composing::ImageSize::Actual), ..d.clone() };
        let prepared = prepare(&dir, &options, &encrypted, Some(1), &share).unwrap();
        assert!(prepared.shared.is_empty() && prepared.resized == 0);
        assert_eq!(prepared.draft.attachments, encrypted.attachments);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn files_are_uploaded_to_nextcloud_and_shared_as_links() {
        let (address, seen) = test_nextcloud(r#"{"ocs":{"meta":{"status":"ok"},"data":{"url":"https://cloud.test/s/Xy7"}}}"#);
        let dir = std::env::temp_dir().join(format!("picacho-cloud-{}", uuid::Uuid::new_v4().simple()));
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("big file.zip");
        std::fs::write(&file, vec![1u8; 4096]).unwrap();
        let storage = CloudStorage {
            kind: CloudKind::Nextcloud,
            url: address,
            user: "sam".into(),
            folder: "Mail/Large".into(),
            expire_days: 7,
            ..CloudStorage::default()
        };
        let tls = native_tls::TlsConnector::new().unwrap();
        let cloud = Cloud::new(&storage, "app-password", tls).unwrap();
        let link = cloud.share(&file, "big file.zip").unwrap();
        assert_eq!(link, "https://cloud.test/s/Xy7");
        let seen = seen.lock().unwrap().clone();
        assert_eq!(seen[0], "MKCOL /remote.php/dav/files/sam/Mail 0 basic");
        assert_eq!(seen[1], "MKCOL /remote.php/dav/files/sam/Mail/Large 0 basic");
        assert!(seen[2].starts_with("MKCOL /remote.php/dav/files/sam/Mail/Large/"), "{seen:?}");
        assert!(seen[3].starts_with("PUT /remote.php/dav/files/sam/Mail/Large/") && seen[3].ends_with("/big%20file.zip 4096 basic"), "{seen:?}");
        assert!(seen[4].starts_with("POST /ocs/v2.php/apps/files_sharing/api/v1/shares?format=json"), "{seen:?}");
        // A second file goes in the same folder.
        let _ = cloud.share(&file, "again.zip").unwrap();
        let later = cloud.folder.borrow().clone().unwrap();
        assert_eq!(later.len(), 3);
        let _ = std::fs::remove_dir_all(&dir);
        // Refused: Nextcloud's reason.
        let (address, _) = test_nextcloud(r#"{"ocs":{"meta":{"status":"failure","message":"Passwords are enforced for link shares"},"data":[]}}"#);
        let cloud = Cloud::new(&CloudStorage { url: address, ..storage }, "x", native_tls::TlsConnector::new().unwrap()).unwrap();
        let dir = std::env::temp_dir().join(format!("picacho-cloud-{}", uuid::Uuid::new_v4().simple()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("a.zip"), b"a").unwrap();
        // (200 with a failure: no link in it.)
        assert!(cloud.share(&dir.join("a.zip"), "a.zip").is_err());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn files_go_to_google_drive_shared_with_anyone_with_the_link() {
        let (base, seen) = test_server(|method, path, own| match (method, path) {
            ("GET", p) if p.starts_with("/drive/v3/files?") => ("200 OK".into(), String::new(), r#"{"files":[]}"#.into()),
            ("POST", "/drive/v3/files?fields=id") => ("200 OK".into(), String::new(), r#"{"id":"folder1"}"#.into()),
            ("POST", p) if p.starts_with("/upload/drive/v3/files?uploadType=resumable") => {
                ("200 OK".into(), format!("Location: {own}/upload/session/7\r\n"), String::new())
            }
            ("PUT", "/upload/session/7") => ("200 OK".into(), String::new(), r#"{"id":"file9","webViewLink":"https://drive.google.com/file/d/file9/view"}"#.into()),
            ("POST", "/drive/v3/files/file9/permissions") => ("200 OK".into(), String::new(), r#"{"id":"anyoneWithLink"}"#.into()),
            _ => ("404 Not Found".into(), String::new(), r#"{"error":{"message":"no such thing"}}"#.into()),
        });
        let dir = std::env::temp_dir().join(format!("picacho-drive-{}", uuid::Uuid::new_v4().simple()));
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("talk.mp4");
        std::fs::write(&file, vec![3u8; 5000]).unwrap();
        let storage = CloudStorage { kind: CloudKind::GoogleDrive, account: "acct".into(), ..CloudStorage::default() };
        let cloud = AccountCloud::new(&storage, "token", &base, native_tls::TlsConnector::new().unwrap()).unwrap();
        assert_eq!(cloud.share(&file, "talk.mp4").unwrap(), "https://drive.google.com/file/d/file9/view");
        let seen = seen.lock().unwrap().clone();
        assert!(seen[0].starts_with("GET /drive/v3/files?q=name") && seen[0].ends_with("bearer"), "{seen:?}");
        assert_eq!(seen[1].split(' ').next(), Some("POST"));
        assert!(seen[3].starts_with("PUT /upload/session/7 5000"), "{seen:?}");
        assert!(seen[4].starts_with("POST /drive/v3/files/file9/permissions"), "{seen:?}");
        // The folder is found once.
        cloud.share(&file, "again.mp4").unwrap();
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn files_go_to_onedrive_in_pieces_and_get_a_link() {
        let (base, seen) = test_server(|method, path, own| match (method, path) {
            ("POST", p) if p.ends_with(":/createUploadSession") => ("200 OK".into(), String::new(), format!(r#"{{"uploadUrl":"{own}/up/1"}}"#)),
            ("PUT", "/up/1") => ("202 Accepted".into(), String::new(), r#"{"nextExpectedRanges":["x"]}"#.into()),
            ("POST", p) if p.ends_with("/createLink") => ("200 OK".into(), String::new(), r#"{"link":{"webUrl":"https://1drv.ms/u/s!abc"}}"#.into()),
            _ => ("404 Not Found".into(), String::new(), String::new()),
        });
        let dir = std::env::temp_dir().join(format!("picacho-onedrive-{}", uuid::Uuid::new_v4().simple()));
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("big.iso");
        let total = ONEDRIVE_PIECE + 1000;
        std::fs::write(&file, vec![1u8; total]).unwrap();
        let storage = CloudStorage { kind: CloudKind::OneDrive, account: "acct".into(), expire_days: 30, ..CloudStorage::default() };
        let cloud = AccountCloud::new(&storage, "token", &base, native_tls::TlsConnector::new().unwrap()).unwrap();
        // The last piece's answer is the file: no id here, so no link.
        assert!(cloud.share(&file, "big.iso").is_err());
        let seen = seen.lock().unwrap().clone();
        assert!(seen[0].starts_with("POST /me/drive/root:/Picacho/") && seen[0].contains("/big.iso:/createUploadSession") && seen[0].ends_with("bearer"), "{seen:?}");
        assert_eq!(seen[1], format!("PUT /up/1 {ONEDRIVE_PIECE} none bytes 0-{}/{total}", ONEDRIVE_PIECE - 1), "the upload address is its own authorization");
        assert_eq!(seen[2], format!("PUT /up/1 1000 none bytes {ONEDRIVE_PIECE}-{}/{total}", total - 1));
        let _ = std::fs::remove_dir_all(&dir);
        // A complete one.
        let (base, seen) = test_server(|method, path, own| match (method, path) {
            ("POST", p) if p.ends_with(":/createUploadSession") => ("200 OK".into(), String::new(), format!(r#"{{"uploadUrl":"{own}/up/2"}}"#)),
            ("PUT", "/up/2") => ("201 Created".into(), String::new(), r#"{"id":"ITEM1"}"#.into()),
            ("POST", "/me/drive/items/ITEM1/createLink") => ("200 OK".into(), String::new(), r#"{"link":{"webUrl":"https://1drv.ms/u/s!abc"}}"#.into()),
            _ => ("404 Not Found".into(), String::new(), String::new()),
        });
        let dir = std::env::temp_dir().join(format!("picacho-onedrive-{}", uuid::Uuid::new_v4().simple()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("a.zip"), b"abc").unwrap();
        let cloud = AccountCloud::new(&storage, "token", &base, native_tls::TlsConnector::new().unwrap()).unwrap();
        assert_eq!(cloud.share(&dir.join("a.zip"), "a.zip").unwrap(), "https://1drv.ms/u/s!abc");
        assert!(seen.lock().unwrap().iter().any(|r| r.starts_with("POST /me/drive/items/ITEM1/createLink")));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn links_go_in_formatted_messages_above_the_signature() {
        let mut d = Draft { formatted: true, html: "<p>Hi</p><p>--&nbsp;</p><div>Sam</div>".into(), ..Draft::default() };
        add_links(&mut d, &[Shared { name: "a&b.pdf".into(), size: 3 * 1024 * 1024, url: "https://x.test/s/1".into() }]);
        assert!(d.html.contains("<li><a href=\"https://x.test/s/1\">a&amp;b.pdf</a> (3.0 MB)</li>"), "{}", d.html);
        assert!(d.html.find("x.test").unwrap() < d.html.find("Sam").unwrap());
        assert!(d.body.contains("a&b.pdf"));
        let answer: serde_json::Value = serde_json::json!({"ocs": {"meta": {"status": "ok"}, "data": {"url": "https://cloud.test/s/abc"}}});
        assert_eq!(nextcloud_url(&answer).as_deref(), Some("https://cloud.test/s/abc"));
        assert_eq!(nextcloud_url(&serde_json::json!({"ocs": {"data": []}})), None);
    }
}
