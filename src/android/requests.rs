//! Requests to the Java side that are answered later, from another thread:
//! Proton's verification page, a passkey. Rust makes a `Request`, passes
//! its id to Java, and waits; Java answers with `Native.answer(id, value)`,
//! once or (Proton's page) once per message, and with a null value when it
//! has nothing more to say (the page was closed).

use jni::{
    EnvUnowned,
    objects::{JClass, JString},
};
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

/// A request waiting for Java's answers. Answers after it is dropped are
/// passed over.
pub(super) struct Request {
    id: i32,
    answers: mpsc::Receiver<Option<String>>,
}

/// What waiting for the next answer came to.
pub(super) enum Waited {
    /// Java answered: a value, or `None` when it has nothing more to say.
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

/// `Native.answer(int id, String value)`: an answer to request `id`, on
/// any Java thread; a null `value` says there is nothing more.
#[unsafe(no_mangle)]
pub extern "system" fn Java_tech_pinal_picacho_Native_answer<'caller>(
    env: EnvUnowned<'caller>,
    _class: JClass<'caller>,
    id: jni::sys::jint,
    value: JString<'caller>,
) {
    super::native(env, |_| {
        let value = (!value.is_null()).then(|| value.to_string());
        if let Some(sender) = pending().get(&id) {
            let _ = sender.send(value);
        }
        Ok(())
    });
}
