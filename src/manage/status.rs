//! Presentation-neutral synchronization status shared with Display.
/// A snapshot of background body downloads. Counts come from the local cache;
/// the coordinator owns task lifetime, while Display chooses how to draw it.
#[derive(Default)]
pub struct DownloadStatus {
    pub total: usize,
    pub missing: usize,
    pub active: bool,
    pub paused: bool,
}
