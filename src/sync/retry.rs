//! Retry classification for read-only synchronization. Authentication failures,
//! invalid responses and rejected commands require an explicit later refresh.
pub(crate) fn transient(error: &anyhow::Error) -> bool {
    error.chain().any(|cause| {
        if let Some(error) = cause.downcast_ref::<std::io::Error>() {
            return matches!(
                error.kind(),
                std::io::ErrorKind::TimedOut
                    | std::io::ErrorKind::WouldBlock
                    | std::io::ErrorKind::ConnectionReset
                    | std::io::ErrorKind::ConnectionAborted
                    | std::io::ErrorKind::BrokenPipe
                    | std::io::ErrorKind::UnexpectedEof
            );
        }
        matches!(
            cause.downcast_ref::<imap::Error>(),
            Some(imap::Error::ConnectionLost)
        )
    })
}

/// A server that couldn't be reached at all: its name didn't resolve or no
/// connection could be made. Nothing was sent; trying later may work.
#[derive(Debug)]
pub(crate) struct Unreachable(pub(crate) String);
impl std::fmt::Display for Unreachable {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}
impl std::error::Error for Unreachable {}

/// Whether trying again later may work: a lost connection, or a server
/// that couldn't be reached.
pub(crate) fn later(error: &anyhow::Error) -> bool {
    transient(error) || error.chain().any(|cause| cause.is::<Unreachable>())
}
