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
