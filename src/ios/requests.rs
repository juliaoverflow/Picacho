//! Requests to the Swift side that are answered later, from another thread:
//! Proton's verification page. Rust makes a `Request`, passes its id to
//! Swift, and waits; Swift answers with `picacho_ios_answer(id, …)`, once or
//! (Proton's page) once per message, and with no value when it has nothing
//! more to say (the page was closed). As Android's (`android::requests`).

use std::{
    collections::HashMap,
    sync::{
        LazyLock, Mutex,
        atomic::{AtomicBool, AtomicI32, Ordering},
        mpsc,
    },
    time::{Duration, Instant},
};

/// The requests waiting for answers, by id.
static PENDING: LazyLock<Mutex<HashMap<i32, mpsc::Sender<Option<String>>>>> =
    LazyLock::new(Default::default);
static NEXT: AtomicI32 = AtomicI32::new(1);

/// The longest answer kept: Proton's page posts small JSON messages.
const MOST: usize = 64 * 1024;

/// A request waiting for Swift's answers. Answers after it is dropped are
/// passed over.
pub(super) struct Request {
    id: i32,
    answers: mpsc::Receiver<Option<String>>,
}

/// What waiting for the next answer came to.
pub(super) enum Waited {
    /// Swift answered: a value, or `None` when it has nothing more to say.
    Answer(Option<String>),
    /// The sign-in was cancelled.
    Cancelled,
    /// No answer came in time.
    TimedOut,
}

impl Request {
    pub(super) fn new() -> Self {
        let id = NEXT.fetch_add(1, Ordering::Relaxed);
        let (sender, answers) = mpsc::channel();
        pending().insert(id, sender);
        Self { id, answers }
    }

    pub(super) fn id(&self) -> i32 {
        self.id
    }

    /// Waits for the next answer until `deadline`, or until `cancel`.
    pub(super) fn next(&self, cancel: &AtomicBool, deadline: Instant) -> Waited {
        loop {
            if cancel.load(Ordering::Relaxed) {
                return Waited::Cancelled;
            }
            let now = Instant::now();
            if now >= deadline {
                return Waited::TimedOut;
            }
            let wait = (deadline - now).min(Duration::from_millis(100));
            match self.answers.recv_timeout(wait) {
                Ok(answer) => return Waited::Answer(answer),
                Err(mpsc::RecvTimeoutError::Timeout) => {}
                Err(mpsc::RecvTimeoutError::Disconnected) => return Waited::Answer(None),
            }
        }
    }
}

impl Drop for Request {
    fn drop(&mut self) {
        pending().remove(&self.id);
    }
}

fn pending() -> std::sync::MutexGuard<'static, HashMap<i32, mpsc::Sender<Option<String>>>> {
    PENDING.lock().unwrap_or_else(|e| e.into_inner())
}

/// An answer to request `id`, from any thread: `len` UTF-8 bytes at `value`,
/// or nothing more to say when `value` is null.
///
/// # Safety
/// `value` is null or points to `len` readable bytes for the call.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn picacho_ios_answer(id: i32, value: *const u8, len: usize) {
    let value = if value.is_null() {
        None
    } else if len > MOST {
        return;
    } else {
        // Safety: the caller passes `len` readable bytes.
        let bytes = unsafe { std::slice::from_raw_parts(value, len) };
        Some(String::from_utf8_lossy(bytes).into_owned())
    };
    if let Some(sender) = pending().get(&id) {
        let _ = sender.send(value);
    }
}
