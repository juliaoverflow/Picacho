//! Downloads of the optional local AI models. Only the model file is asked
//! for: nothing about the person or their mail is sent.
use anyhow::{Context, Result, ensure};
use std::io::Read;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

/// The file at `url`, refused if it is larger than `max` bytes. `received`
/// counts the bytes as they arrive, for showing progress. GitHub release
/// assets redirect to a download host, so redirects are followed, over
/// HTTPS only.
pub fn download(url: &str, max: u64, received: &AtomicU64) -> Result<Vec<u8>> {
    let client = reqwest::blocking::Client::builder()
        .min_tls_version(reqwest::tls::Version::TLS_1_2)
        .timeout(Duration::from_secs(120))
        .https_only(true)
        .redirect(reqwest::redirect::Policy::limited(5))
        .user_agent(concat!("Picacho/", env!("CARGO_PKG_VERSION")))
        .build()?;
    let response = client
        .get(url)
        .send()
        .context("Couldn't reach the model download")?;
    ensure!(
        response.status().is_success(),
        "The model download answered {}",
        response.status()
    );
    if let Some(length) = response.content_length() {
        ensure!(length <= max, "The model download is larger than expected");
    }
    let mut bytes = Vec::new();
    let mut body = response.take(max + 1);
    let mut chunk = [0u8; 16 * 1024];
    received.store(0, Ordering::Relaxed);
    loop {
        let n = body
            .read(&mut chunk)
            .context("The model download was interrupted")?;
        if n == 0 {
            break;
        }
        bytes.extend_from_slice(&chunk[..n]);
        received.store(bytes.len() as u64, Ordering::Relaxed);
    }
    ensure!(
        bytes.len() as u64 <= max,
        "The model download is larger than expected"
    );
    Ok(bytes)
}
