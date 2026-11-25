//! Unlocking with the body rather than a passcode, for the app lock: Touch
//! ID, Face ID or Optic ID on macOS and iOS (LocalAuthentication), the
//! phone's fingerprint or face unlock on Android 10 and later
//! (`android.hardware.biometrics`, through the Java side). Elsewhere there
//! is none, and the lock asks for its passcode.
//!
//! The system draws and runs its own prompt; the answer arrives on another
//! thread, through the returned channel.
use futures::channel::oneshot;

/// The kind of biometrics this device has, enrolled and ready.
// Each system constructs only its own kinds.
#[cfg_attr(not(any(target_os = "macos", target_os = "ios")), allow(dead_code))]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Biometry {
    TouchId,
    FaceId,
    OpticId,
    /// Android's: a fingerprint or the face, whichever the phone has.
    Android,
}

impl Biometry {
    /// What Settings and the lock screen call it.
    pub fn label(self) -> &'static str {
        match self {
            Self::TouchId => t!("biometric-touch-id"),
            Self::FaceId => t!("biometric-face-id"),
            Self::OpticId => t!("biometric-optic-id"),
            Self::Android => t!("biometric-fingerprint-or-face"),
        }
    }
    /// The label at the start of a sentence or on a button.
    pub fn title(self) -> &'static str {
        match self {
            Self::Android => t!("biometric-fingerprint-or-face-2"),
            other => other.label(),
        }
    }
    pub fn icon(self) -> &'static str {
        match self {
            Self::TouchId | Self::Android => "fingerprint",
            Self::FaceId | Self::OpticId => "face-id",
        }
    }
}

/// How asking ended, when it didn't unlock.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Failure {
    /// The person cancelled, chose the passcode, or the system took the
    /// prompt away (another app came forward): nothing to report.
    Cancelled,
    /// It couldn't be used (too many tries, no longer enrolled); why.
    Failed(String),
}

pub type Answer = Result<(), Failure>;

/// The biometrics this device can unlock with now, if any.
pub fn available() -> Option<Biometry> {
    platform::available()
}

/// Asks the system to check the person, saying why (`reason`).
pub fn authenticate(reason: &str) -> oneshot::Receiver<Answer> {
    let (tx, rx) = oneshot::channel();
    platform::authenticate(reason, tx);
    rx
}

#[cfg(any(target_os = "macos", target_os = "ios"))]
mod platform {
    use super::{Answer, Biometry, Failure};
    use block2::RcBlock;
    use futures::channel::oneshot;
    use objc2::runtime::Bool;
    use objc2_foundation::{NSError, NSString};
    use objc2_local_authentication::{LABiometryType, LAContext, LAPolicy};
    use std::sync::Mutex;

    // LAError's codes for a prompt that ended without a failure to report:
    // the person cancelled (-2) or chose the fallback (-3), the system
    // (-4) or the app (-9) took it away.
    const CANCELLED: [isize; 4] = [-2, -3, -4, -9];

    pub fn available() -> Option<Biometry> {
        // Safety: a new context, asked on this thread; nothing else holds it.
        unsafe {
            let context = LAContext::new();
            context
                .canEvaluatePolicy_error(LAPolicy::DeviceOwnerAuthenticationWithBiometrics)
                .ok()?;
            match context.biometryType() {
                LABiometryType::TouchID => Some(Biometry::TouchId),
                LABiometryType::FaceID => Some(Biometry::FaceId),
                LABiometryType::OpticID => Some(Biometry::OpticId),
                _ => None,
            }
        }
    }

    pub fn authenticate(reason: &str, tx: oneshot::Sender<Answer>) {
        let tx = Mutex::new(Some(tx));
        // Safety: LocalAuthentication calls the reply once, on a queue of
        // its own; the block only touches what it owns. The context is
        // kept alive by the block until then.
        unsafe {
            let context = LAContext::new();
            // No "Enter Password" button: the lock screen's own passcode
            // field is the fallback, and the system's password isn't the
            // app's passcode.
            context.setLocalizedFallbackTitle(Some(&NSString::from_str("")));
            let kept = context.clone();
            let reply = RcBlock::new(move |ok: Bool, error: *mut NSError| {
                let _ = &kept;
                let answer = if ok.as_bool() {
                    Ok(())
                } else if error.is_null() {
                    Err(Failure::Cancelled)
                } else {
                    let error = &*error;
                    if CANCELLED.contains(&error.code()) {
                        Err(Failure::Cancelled)
                    } else {
                        Err(Failure::Failed(error.localizedDescription().to_string()))
                    }
                };
                if let Some(tx) = tx.lock().unwrap_or_else(|e| e.into_inner()).take() {
                    let _ = tx.send(answer);
                }
            });
            context.evaluatePolicy_localizedReason_reply(
                LAPolicy::DeviceOwnerAuthenticationWithBiometrics,
                &NSString::from_str(reason),
                &reply,
            );
        }
    }
}

#[cfg(target_os = "android")]
mod platform {
    use super::{Answer, Biometry};
    use futures::channel::oneshot;

    pub fn available() -> Option<Biometry> {
        crate::android::biometrics_available().then_some(Biometry::Android)
    }

    pub fn authenticate(reason: &str, tx: oneshot::Sender<Answer>) {
        let reason = reason.to_owned();
        // The Java side answers on its UI thread; this one waits for it.
        let _ = std::thread::Builder::new()
            .name("biometrics".into())
            .spawn(move || {
                let _ = tx.send(crate::android::authenticate_biometrics(&reason));
            });
    }
}

#[cfg(not(any(target_os = "macos", target_os = "ios", target_os = "android")))]
mod platform {
    use super::{Answer, Biometry, Failure};
    use futures::channel::oneshot;

    pub fn available() -> Option<Biometry> {
        None
    }

    pub fn authenticate(_reason: &str, tx: oneshot::Sender<Answer>) {
        let _ = tx.send(Err(Failure::Failed(
            t!("biometric-this-computer-has-no-biometrics-picacho").into(),
        )));
    }
}
