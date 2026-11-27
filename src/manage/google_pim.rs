//! Google's calendars and contacts for a Google account signed in with
//! OAuth, through Google's own APIs (`sync/google.rs`): what calendars
//! (`calendar.rs`) and contacts (`contacts.rs`) use when their source is
//! a Google account.
use super::*;
use crate::sync::google;

impl Backend {
    /// Runs `work` with a session of the Google account's APIs, signed in
    /// with its token (renewed as needed), the account's work waiting.
    pub(super) fn with_google<T>(&self, a: &Account, work: impl FnOnce(&google::Session) -> Result<T>) -> Result<T> {
        let token = self.mail_account_secret(&a.id)?;
        let lock = self.account_lock(&a.id);
        let _operation = lock.lock().unwrap();
        let session = google::Session::open(
            &gmail::api_base(a, google::CALENDAR_BASE),
            &gmail::api_base(a, google::PEOPLE_BASE),
            &token,
            self.tls_connector()?,
        )?;
        work(&session)
    }
}
