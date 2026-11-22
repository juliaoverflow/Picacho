//! Keeping Picacho's memory small while it runs in the background.
//!
//! On Linux, glibc's allocator gives each thread that allocates an arena of
//! its own, up to eight per core, and keeps memory freed in them rather than
//! returning it. Picacho has dozens of threads (GPUI's workers, one per core,
//! and D-Bus's), so after a while with the window open its arenas hold
//! hundreds of megabytes of freed memory that stay with the process once the
//! window closes. The background is a process of its own for that reason
//! (`display::resident`); fewer arenas, and handing what's free back to the
//! system after background checks, keep both processes smaller.

/// Arenas glibc may make: enough for the few threads busy at once.
#[cfg(all(target_os = "linux", target_env = "gnu"))]
const ARENAS: i32 = 4;

/// Called first thing in `main`, before other threads start.
pub fn configure() {
    #[cfg(all(target_os = "linux", target_env = "gnu"))]
    {
        // Set by the person (or a packager): theirs wins.
        if std::env::var_os("MALLOC_ARENA_MAX").is_none() {
            // Safety: mallopt only changes the allocator's settings.
            unsafe { libc::mallopt(libc::M_ARENA_MAX, ARENAS) };
        }
    }
    // Tokio runtimes made without a size (zbus's, for D-Bus) start a worker
    // per core; D-Bus needs a couple. Picacho's own runtimes set theirs.
    if std::env::var_os("TOKIO_WORKER_THREADS").is_none() {
        // Safety: no other threads are running yet to read the environment.
        unsafe { std::env::set_var("TOKIO_WORKER_THREADS", "2") };
    }
}

/// Hands memory the allocator holds but no longer uses back to the system.
/// It takes a few milliseconds; call it after a lot was freed, not often.
pub fn release_unused() {
    #[cfg(all(target_os = "linux", target_env = "gnu"))]
    // Safety: malloc_trim only returns free pages to the system.
    unsafe {
        libc::malloc_trim(0);
    }
}

/// Hands freed memory back to the system before long, off the calling
/// thread, at most every `EVERY`: for the window's process, after it lets go
/// of a lot (an email's document, a mailbox's list). Without it glibc kept
/// what an email or a long list had used, and the window stayed as large as
/// its largest moment. Throttled so that opening one message after another
/// costs no more than a trim every few seconds.
pub fn release_unused_soon() {
    #[cfg(all(target_os = "linux", target_env = "gnu"))]
    {
        use std::sync::atomic::{AtomicU64, Ordering};
        use std::time::{Duration, Instant};
        const EVERY: Duration = Duration::from_secs(10);
        static START: std::sync::LazyLock<Instant> = std::sync::LazyLock::new(Instant::now);
        // Milliseconds after `START` of the last trim, plus one (0: none yet).
        static LAST: AtomicU64 = AtomicU64::new(0);
        let now = START.elapsed().as_millis() as u64 + 1;
        let last = LAST.load(Ordering::Relaxed);
        if last != 0 && now - last < EVERY.as_millis() as u64 {
            return;
        }
        if LAST.compare_exchange(last, now, Ordering::Relaxed, Ordering::Relaxed).is_err() {
            return;
        }
        let _ = std::thread::Builder::new()
            .name("picacho-trim".into())
            .spawn(release_unused);
    }
}
