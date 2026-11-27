//! Development aid (Linux): with `PICACHO_STALLS=1`, catches the window's
//! thread when it stops responding and writes down where it is.
//!
//! The window's thread notes the time every 25 ms; a watchdog thread that
//! sees no note for 250 ms runs `eu-stack` (elfutils) on the window's
//! thread, then again each second it stays stuck, and notes how long the
//! stall lasted once it ends. Everything goes to `/tmp/picacho-stalls.txt`
//! (or `PICACHO_STALLS_FILE`). Picacho lets `eu-stack` look at it
//! (`PR_SET_PTRACER`) only while this is on. Build with symbols to read the
//! stacks: `CARGO_PROFILE_RELEASE_STRIP=false
//! CARGO_PROFILE_RELEASE_DEBUG=line-tables-only cargo build --release`.
use gpui::App;
use std::io::Write;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

/// When the window's thread last noted the time, in ms since `EPOCH` (0:
/// not yet).
static BEAT: AtomicU64 = AtomicU64::new(0);
static EPOCH: std::sync::LazyLock<Instant> = std::sync::LazyLock::new(Instant::now);
/// How often the window's thread notes the time.
const BEAT_EVERY: Duration = Duration::from_millis(25);
/// How long without a note counts as a stall.
const STALL: u64 = 250;
/// While a stall lasts, its stack is taken again this often.
const AGAIN: u64 = 1000;

fn now() -> u64 {
    EPOCH.elapsed().as_millis() as u64
}

/// Starts watching, if `PICACHO_STALLS` is set.
pub(super) fn start(cx: &mut App) {
    if std::env::var_os("PICACHO_STALLS").is_none() {
        return;
    }
    let path = std::env::var_os("PICACHO_STALLS_FILE")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| std::env::temp_dir().join("picacho-stalls.txt"));
    // Safety: allows any process of this user to trace this one, which
    // eu-stack (started below, not our parent) needs under Yama.
    unsafe {
        libc::prctl(libc::PR_SET_PTRACER, libc::PR_SET_PTRACER_ANY, 0, 0, 0);
    }
    let _ = std::fs::write(&path, "");
    eprintln!("Picacho: writing window stalls to {}", path.display());
    BEAT.store(now().max(1), Ordering::Relaxed);
    cx.spawn(async move |cx| {
        loop {
            cx.background_executor().timer(BEAT_EVERY).await;
            BEAT.store(now().max(1), Ordering::Relaxed);
        }
    })
    .detach();
    let pid = std::process::id().to_string();
    let _ = std::thread::Builder::new()
        .name("picacho-stalls".into())
        .spawn(move || watch(&pid, &path));
}

fn watch(pid: &str, path: &std::path::Path) {
    let write = |text: &str| {
        if let Ok(mut f) = std::fs::OpenOptions::new().append(true).create(true).open(path) {
            let _ = f.write_all(text.as_bytes());
        }
    };
    // The beat the current stall started after, and when its stack was
    // last taken.
    let mut stalled: Option<(u64, u64)> = None;
    loop {
        std::thread::sleep(Duration::from_millis(50));
        let (beat, at) = (BEAT.load(Ordering::Relaxed), now());
        let late = at.saturating_sub(beat);
        match stalled {
            Some((from, _)) if beat != from => {
                write(&format!("-- stall ended after {} ms\n\n", beat.saturating_sub(from)));
                stalled = None;
            }
            Some((from, taken)) if at.saturating_sub(taken) >= AGAIN => {
                stalled = Some((from, at));
                write(&stack(pid, at, late));
            }
            None if late >= STALL => {
                stalled = Some((beat, at));
                write(&stack(pid, at, late));
            }
            _ => {}
        }
    }
}

/// The window's thread's stack now, as eu-stack shows it.
fn stack(pid: &str, at: u64, late: u64) -> String {
    let shown = std::process::Command::new("eu-stack")
        .args(["-1", "-i", "-n", "80", "-p", pid])
        .output();
    let body = match shown {
        Ok(out) => String::from_utf8_lossy(&out.stdout).into_owned(),
        Err(e) => format!("(eu-stack couldn't run: {e})\n"),
    };
    format!("== {:.1} s after launch, window thread stuck for {late} ms\n{body}", at as f64 / 1000.)
}
