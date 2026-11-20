//! Proton push notifications, received the way Proton's Android app gets
//! them: one cloud-messaging (Firebase) registration for this installation,
//! one device key, and every Proton session that wants notifications
//! registered with Proton for that device (`protected_email_bridge::push`).
//!
//! One client serves every Proton account in the process: a single
//! cloud-messaging connection and device token, with each session
//! registered against it. Proton's pushes name the session they are for;
//! that picks the account. Pushes for sessions not registered here (an
//! account that turned notifications off since) are dropped.
//!
//! The cloud-messaging credentials (device token, Android ID, web-push keys),
//! the device key and the record of pushes already shown live in the
//! keychain, so a restarted process (the Android mail service, the desktop
//! background process) reuses the same token and registration rather than
//! making a new one each run.
//!
//! The Firebase project is the operator's, set at build time or at run time
//! through `PICACHO_PROTON_FCM_*` (see `firebase`). Without at least its API
//! key, app id and project id, push isn't offered.
//!
//! Nothing here touches the database or the UI: `manage` says which
//! sessions to register and shows what arrives.
use anyhow::{Context, Result, anyhow};
use protected_email_bridge::{
    api::ApiClient,
    push::{
        DeviceKeyManager, DeviceRegistrationRecord, Presentation, PushEnvelope, Registrar,
        SecureStore,
        fcm::{FcmAndroidApp, FcmConfig, FcmPushProvider},
        registration::{
            ApiConnectivity, RegistrationObserver, SessionContext, SessionProvider, StoredSession,
        },
        seen::SeenNotifications,
    },
};
use std::sync::{Arc, Mutex, OnceLock};
use tokio::sync::{mpsc, watch};
use zeroize::Zeroizing;

/// A build-time setting, which the same variable at run time overrides.
macro_rules! setting {
    ($name:literal) => {
        std::env::var($name)
            .ok()
            .or_else(|| option_env!($name).map(str::to_owned))
            .map(|v| v.trim().to_owned())
            .filter(|v| !v.is_empty())
    };
}

/// The Firebase project the device registers with, from the environment:
///
/// * `PICACHO_PROTON_FCM_API_KEY`: the project's web API key;
/// * `PICACHO_PROTON_FCM_APP_ID`: the Android app's id (`1:<number>:android:<hex>`);
/// * `PICACHO_PROTON_FCM_PROJECT_ID`: the project id;
/// * `PICACHO_PROTON_FCM_SENDER_ID`: the sender id (project number); taken
///   from the app id when not set;
/// * `PICACHO_PROTON_FCM_PACKAGE`: the Android package name, for projects
///   whose key is restricted to Android apps;
/// * `PICACHO_PROTON_FCM_CERT_SHA1`: that package's signing-certificate
///   SHA-1 (hex, with or without colons);
/// * `PICACHO_PROTON_FCM_VAPID_KEY`: a web-push VAPID key, if the project
///   wants its own.
///
/// `None` without the API key, app id and project id. The package identity
/// is sent only when both the package and the certificate hash are set.
pub fn firebase() -> Option<FcmConfig> {
    let api_key = setting!("PICACHO_PROTON_FCM_API_KEY")?;
    let app_id = setting!("PICACHO_PROTON_FCM_APP_ID")?;
    let project_id = setting!("PICACHO_PROTON_FCM_PROJECT_ID")?;
    let mut config = FcmConfig::new(api_key, app_id.clone(), project_id);
    let sender = setting!("PICACHO_PROTON_FCM_SENDER_ID").or_else(|| sender_of(&app_id));
    let package = setting!("PICACHO_PROTON_FCM_PACKAGE");
    let cert = setting!("PICACHO_PROTON_FCM_CERT_SHA1").map(|c| cert_hex(&c));
    if let (Some(gcm_sender_id), Some(android_package), Some(android_package_cert)) =
        (sender, package, cert)
    {
        config.android_app = Some(FcmAndroidApp {
            gcm_sender_id,
            android_package,
            android_package_cert,
        });
    }
    config.vapid_key = setting!("PICACHO_PROTON_FCM_VAPID_KEY");
    Some(config)
}

/// Whether this build (or run) has a Firebase project to receive with.
pub fn available() -> bool {
    firebase().is_some()
}

/// The project number in a Firebase app id (`1:<number>:android:<hex>`).
fn sender_of(app_id: &str) -> Option<String> {
    let number = app_id.split(':').nth(1)?;
    (!number.is_empty() && number.bytes().all(|b| b.is_ascii_digit())).then(|| number.to_owned())
}

/// A certificate hash as Firebase wants it: hex digits only, upper case
/// (keytool prints `AB:CD:…`).
fn cert_hex(cert: &str) -> String {
    cert.chars()
        .filter(char::is_ascii_hexdigit)
        .collect::<String>()
        .to_ascii_uppercase()
}

/// A push failure worth knowing about. Never includes what a push said.
pub(crate) fn note(message: &str) {
    #[cfg(any(target_os = "android", target_os = "ios"))]
    log::warn!("Proton push: {message}");
    #[cfg(not(any(target_os = "android", target_os = "ios")))]
    eprintln!("Picacho: Proton push: {message}");
}

/// The push client's secrets, each its own keychain entry beside the
/// accounts' (`proton-push:<name>`).
struct Keychain;
impl Keychain {
    fn entry(name: &str) -> String {
        format!("proton-push:{name}")
    }
}
fn store_error(e: anyhow::Error) -> protected_email_bridge::Error {
    protected_email_bridge::Error::Other(format!("{e:#}"))
}
/// Runs a keychain call where it may block. The library asks the store from
/// inside the Proton runtime (registration, received pushes), and some
/// keychains block on a runtime of their own (Linux's Secret Service, over
/// zbus), which Tokio refuses inside another: those calls go to a thread
/// of their own.
fn off_runtime<T: Send>(work: impl FnOnce() -> T + Send) -> T {
    if tokio::runtime::Handle::try_current().is_err() {
        return work();
    }
    std::thread::scope(|scope| {
        std::thread::Builder::new()
            .name("proton-push-keychain".into())
            .spawn_scoped(scope, work)
            .expect("start a keychain thread")
            .join()
            .unwrap_or_else(|panic| std::panic::resume_unwind(panic))
    })
}
impl SecureStore for Keychain {
    fn get(&self, name: &str) -> protected_email_bridge::Result<Option<Zeroizing<String>>> {
        let entry = Self::entry(name);
        off_runtime(|| crate::manage::store::read_secret_text(&entry))
            .map(|v| v.map(Zeroizing::new))
            .map_err(store_error)
    }
    fn put(&self, name: &str, value: &str) -> protected_email_bridge::Result<()> {
        let entry = Self::entry(name);
        off_runtime(|| crate::manage::store::write_secret_text(&entry, value)).map_err(store_error)
    }
    fn delete(&self, name: &str) -> protected_email_bridge::Result<()> {
        let entry = Self::entry(name);
        off_runtime(|| crate::manage::store::delete_secret(&entry)).map_err(store_error)
    }
}

/// A signed-in Proton session to register for pushes.
#[derive(Clone)]
pub struct Registration {
    /// Picacho's id for the account.
    pub account: String,
    /// The session's id, which Proton's pushes name.
    pub uid: String,
    /// Proton's id for the user.
    pub user_id: String,
    /// The session's client, which registers the device.
    pub client: ApiClient,
}

/// The registered sessions, as the library's registration process sees
/// them.
struct Sessions {
    list: Mutex<Vec<Registration>>,
    changes: watch::Sender<u64>,
}
impl SessionProvider for Sessions {
    fn stored_sessions(&self) -> protected_email_bridge::Result<Vec<StoredSession>> {
        Ok(self
            .list
            .lock()
            .unwrap()
            .iter()
            .map(|r| StoredSession {
                uid: r.uid.clone(),
                user_id: r.user_id.clone(),
                fully_signed_in: true,
            })
            .collect())
    }
    fn context(&self, session: &StoredSession) -> Option<SessionContext> {
        self.list
            .lock()
            .unwrap()
            .iter()
            .find(|r| r.uid == session.uid)
            .map(|r| SessionContext::Ready(r.client.clone()))
    }
    fn subscribe(&self) -> watch::Receiver<u64> {
        self.changes.subscribe()
    }
}

/// Reports each session's registration to `Status`, by account.
struct Observer;
impl RegistrationObserver for Observer {
    fn registered(&self, session_uid: &str) {
        if let Some(account) = account_of(session_uid) {
            report(&account, Ok(()));
        }
    }
    fn registration_failed(
        &self,
        session_uid: Option<&str>,
        error: &protected_email_bridge::Error,
    ) {
        note(&format!(
            "Proton didn't register this device for pushes: {error}"
        ));
        // Without a session, the step failed before reaching one (reading
        // the sessions, which can't fail here): nothing to attribute.
        if let Some(account) = session_uid.and_then(account_of) {
            report(&account, Err(error.to_string()));
        }
    }
}

/// The account (Picacho's id) whose session this is.
fn account_of(session_uid: &str) -> Option<String> {
    client()
        .sessions
        .list
        .lock()
        .unwrap()
        .iter()
        .find(|r| r.uid == session_uid)
        .map(|r| r.account.clone())
}

fn report(account: &str, result: std::result::Result<(), String>) {
    let status = client().status.lock().unwrap().clone();
    if let Some(status) = status {
        status(account, result);
    }
}

/// Shows a push: the account's id and what to show.
pub type Deliver = Arc<dyn Fn(&str, Presentation) + Send + Sync>;
/// Hears whether an account's session was registered with Proton for
/// pushes: the account's id, and the error if Proton refused it.
pub type Status = Arc<dyn Fn(&str, std::result::Result<(), String>) + Send + Sync>;

/// The running client: the cloud-messaging connection, the registration
/// process and the task that opens what arrives.
struct Running {
    provider: FcmPushProvider,
    registrar: tokio::task::JoinHandle<()>,
    receiver: tokio::task::JoinHandle<()>,
}
impl Drop for Running {
    fn drop(&mut self) {
        self.provider.stop();
        self.registrar.abort();
        self.receiver.abort();
    }
}

/// The one push client of the process.
struct Client {
    sessions: Arc<Sessions>,
    keys: Arc<DeviceKeyManager>,
    seen: SeenNotifications,
    /// The device's registration record, from the cloud-messaging
    /// connection; the registration process follows it.
    records: watch::Sender<Option<DeviceRegistrationRecord>>,
    /// Set once the accounts that want pushes have had their sessions
    /// opened (or failed to): until then pushes wait, as a push for a
    /// session not yet open would otherwise be dropped.
    ready: watch::Sender<bool>,
    deliver: Mutex<Option<Deliver>>,
    status: Mutex<Option<Status>>,
    running: Mutex<Option<Running>>,
}

fn client() -> &'static Client {
    static CLIENT: OnceLock<Client> = OnceLock::new();
    CLIENT.get_or_init(|| {
        let store: Arc<dyn SecureStore> = Arc::new(Keychain);
        Client {
            sessions: Arc::new(Sessions {
                list: Mutex::new(Vec::new()),
                changes: watch::channel(0).0,
            }),
            keys: Arc::new(DeviceKeyManager::new(store.clone())),
            seen: SeenNotifications::new(store),
            records: watch::channel(None).0,
            ready: watch::channel(false).0,
            deliver: Mutex::new(None),
            status: Mutex::new(None),
            running: Mutex::new(None),
        }
    })
}

/// Starts receiving, if it isn't already: connects to cloud messaging
/// (registering this installation the first time, reusing the saved token
/// afterwards) and registers the sessions `set_sessions` names with Proton.
/// What arrives goes to `deliver` once `set_ready` is called; each
/// session's registration is reported to `status`.
pub fn start(deliver: Deliver, status: Status) -> Result<()> {
    let c = client();
    *c.deliver.lock().unwrap() = Some(deliver);
    *c.status.lock().unwrap() = Some(status);
    let mut running = c.running.lock().unwrap();
    if running.is_some() {
        return Ok(());
    }
    let config = firebase().context("This copy of Picacho has no Firebase project for push.")?;
    let runtime = super::proton::runtime();
    let _entered = runtime.enter();
    // Made (and kept) before anything registers, so the device key is the
    // keychain's from now on.
    c.keys
        .get_or_create()
        .map_err(|e| anyhow!("Couldn't make the push device key: {e}"))?;
    let (envelopes, received) = mpsc::unbounded_channel();
    let provider = FcmPushProvider::start(
        config,
        Arc::new(Keychain),
        Arc::new(|record| {
            client().records.send_if_modified(|current| {
                if current.as_ref() == Some(&record) {
                    false
                } else {
                    *current = Some(record);
                    true
                }
            });
        }),
        envelopes,
    )
    .map_err(|e| anyhow!("Couldn't start receiving pushes: {e}"))?;
    let registrar = Registrar::new(
        c.sessions.clone(),
        c.keys.clone(),
        Arc::new(ApiConnectivity::new(
            ApiClient::new(super::proton::deployment()).map_err(|e| anyhow!("{e}"))?,
        )),
        c.records.subscribe(),
        Some(Arc::new(Observer)),
    )
    .spawn();
    let receiver = runtime.spawn(receive(received));
    *running = Some(Running {
        provider,
        registrar,
        receiver,
    });
    Ok(())
}

/// Stops receiving. The saved token and device key stay, for the next
/// `start`.
pub fn stop() {
    let c = client();
    let running = c.running.lock().unwrap().take();
    drop(running);
    set_sessions(Vec::new());
    c.ready.send_replace(false);
}

/// Whether pushes are being received.
pub fn running() -> bool {
    client().running.lock().unwrap().is_some()
}

/// The sessions to register (and take pushes for), replacing the last
/// list. Sessions new to the list are registered with Proton.
pub fn set_sessions(list: Vec<Registration>) {
    let c = client();
    let mut current = c.sessions.list.lock().unwrap();
    let key = |l: &[Registration]| {
        let mut k: Vec<(String, String)> = l
            .iter()
            .map(|r| (r.account.clone(), r.uid.clone()))
            .collect();
        k.sort();
        k
    };
    let changed = key(&current) != key(&list);
    *current = list;
    drop(current);
    if changed {
        c.sessions.changes.send_modify(|v| *v = v.wrapping_add(1));
    }
}

/// Registers again the sessions not registered yet, such as one Proton
/// refused before.
pub fn retry() {
    if running() {
        client()
            .sessions
            .changes
            .send_modify(|v| *v = v.wrapping_add(1));
    }
}

/// Lets pushes through to `deliver` (see `Client::ready`).
pub fn set_ready() {
    client().ready.send_replace(true);
}

/// Opens each push once the accounts are ready, and hands it on.
async fn receive(mut envelopes: mpsc::UnboundedReceiver<PushEnvelope>) {
    let mut ready = client().ready.subscribe();
    while let Some(envelope) = envelopes.recv().await {
        if ready.wait_for(|r| *r).await.is_err() {
            return;
        }
        process(&envelope);
    }
}

/// One push: skipped if it was handled before (in this run or an earlier
/// one), decrypted with the device key, matched to its account by session,
/// shown, and remembered either way, so a redelivery isn't handled again.
fn process(envelope: &PushEnvelope) {
    let c = client();
    if c.seen.contains(envelope) {
        return;
    }
    match envelope.open(&c.keys) {
        Ok(notification) => {
            let account = c
                .sessions
                .list
                .lock()
                .unwrap()
                .iter()
                .find(|r| !envelope.uid.is_empty() && r.uid == envelope.uid)
                .map(|r| r.account.clone());
            let deliver = c.deliver.lock().unwrap().clone();
            let presentation = Presentation::of(&notification);
            if let (Some(account), Some(deliver)) = (account, deliver)
                && presentation != Presentation::Ignore
            {
                deliver(&account, presentation);
            }
        }
        // Never says what the push said.
        Err(_) => note("dropped a push that couldn't be opened"),
    }
    if let Err(e) = c.seen.insert(envelope) {
        note(&format!("couldn't remember a handled push: {e}"));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sender_comes_from_the_app_id() {
        assert_eq!(
            sender_of("1:123456789012:android:abcdef0123456789").as_deref(),
            Some("123456789012")
        );
        assert_eq!(sender_of("nonsense"), None);
        assert_eq!(sender_of("1::android:ab"), None);
    }

    /// A refused registration is reported for the account whose session
    /// it was; one that names no session isn't attributed.
    #[test]
    fn refused_registrations_are_reported_for_their_account() {
        let reports: Arc<Mutex<Vec<(String, std::result::Result<(), String>)>>> = Arc::default();
        let heard = reports.clone();
        *client().status.lock().unwrap() = Some(Arc::new(move |account, result| {
            heard.lock().unwrap().push((account.to_owned(), result));
        }));
        let session = |account: &str, uid: &str| Registration {
            account: account.into(),
            uid: uid.into(),
            user_id: format!("user-{account}"),
            client: ApiClient::new(super::super::proton::deployment()).unwrap(),
        };
        set_sessions(vec![session("first", "uid-1"), session("second", "uid-2")]);
        let refused = protected_email_bridge::Error::Other("Invalid input".into());
        Observer.registration_failed(Some("uid-2"), &refused);
        Observer.registration_failed(None, &refused);
        Observer.registered("uid-1");
        set_sessions(Vec::new());
        *client().status.lock().unwrap() = None;
        let reports = reports.lock().unwrap();
        assert_eq!(reports.len(), 2);
        assert_eq!(reports[0].0, "second");
        assert!(reports[0].1.as_ref().unwrap_err().contains("Invalid input"));
        assert_eq!(reports[1], ("first".to_owned(), Ok(())));
    }

    #[test]
    fn certificate_hash_loses_its_colons() {
        assert_eq!(cert_hex("ab:cd:0F 12"), "ABCD0F12");
    }

    /// A keychain that blocks on a runtime of its own (as Linux's does)
    /// works when asked from a task of the Proton runtime.
    #[test]
    fn keychain_calls_from_the_runtime_may_block_on_their_own() {
        let nested = || {
            tokio::runtime::Builder::new_current_thread()
                .build()
                .unwrap()
                .block_on(async { 7 })
        };
        let got = super::super::proton::runtime()
            .block_on(async move { tokio::spawn(async move { off_runtime(nested) }).await })
            .unwrap();
        assert_eq!(got, 7);
        assert_eq!(off_runtime(|| 8), 8);
    }
}
