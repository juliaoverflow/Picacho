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
