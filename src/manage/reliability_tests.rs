//! Fault-injection tests for snapshot safety and recovery over real local TLS.
use super::*;
use protocol_tests::{backend, tls_fixture};
use std::{
    io::{BufRead, BufReader, Write},
    net::TcpStream,
    thread::{self, JoinHandle},
    time::Duration,
};

struct Server {
    port: u16,
    stop: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
}
impl Server {
    fn new(mut serve: impl FnMut(TcpStream) + Send + 'static) -> Result<Self> {
        let listener = std::net::TcpListener::bind("127.0.0.1:0")?;
        listener.set_nonblocking(true)?;
        let port = listener.local_addr()?.port();
        let stop = Arc::new(AtomicBool::new(false));
        let stopped = stop.clone();
        let thread = thread::spawn(move || {
            while !stopped.load(Ordering::Relaxed) {
                match listener.accept() {
                    Ok((socket, _)) => {
                        // Accepted sockets inherit the listener's non-blocking
                        // mode on macOS, which breaks the TLS handshake.
                        socket.set_nonblocking(false).unwrap();
                        socket
                            .set_read_timeout(Some(Duration::from_secs(3)))
                            .unwrap();
                        socket
                            .set_write_timeout(Some(Duration::from_secs(3)))
                            .unwrap();
                        serve(socket);
                    }
                    Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                        thread::sleep(Duration::from_millis(5))
                    }
                    Err(e) => panic!("fixture accept: {e}"),
                }
            }
        });
        Ok(Self {
            port,
            stop,
            thread: Some(thread),
        })
    }
}
impl Drop for Server {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        self.thread.take().unwrap().join().unwrap();
    }
}
#[derive(Clone, Copy, Default)]
struct ImapFault {
    no_validity: bool,
    missing_header: bool,
    unsolicited: bool,
    vanished: bool,
    reject_fetch: bool,
    disconnect_fetch: bool,
    recover_after_disconnect: bool,
    disconnect_logout: bool,
    validity: u32,
    reject_second_store: bool,
}
fn imap_server(fault: ImapFault) -> Result<(Vec<u8>, Server)> {
    let (cert, acceptor) = tls_fixture()?;
    let mut connections = 0;
    let server = Server::new(move |socket| {
        connections += 1;
        let Ok(stream) = acceptor.accept(socket) else {
            return;
        };
        let mut r = BufReader::new(stream);
        if r.get_mut().write_all(b"* OK fixture\r\n").is_err() {
            return;
        }
        let mut fetched = false;
        let mut stores = 0;
        loop {
            let mut line = String::new();
            if r.read_line(&mut line).unwrap_or(0) == 0 {
                return;
            }
            let (tag, command) = line.trim_end().split_once(' ').unwrap();
            let command = command.to_uppercase();
            let mut status = "OK";
            let payload = if command.starts_with("LOGIN ") {
                String::new()
            } else if command.starts_with("LIST ") {
                "* LIST (\\Archive) \"/\" \"Archive\"\r\n".into()
            } else if command == "CAPABILITY" {
                "* CAPABILITY IMAP4rev1 MOVE\r\n".into()
            } else if command.starts_with("SELECT ") {
                if fault.no_validity {
                    "* 1 EXISTS\r\n".into()
                } else {
                    format!(
                        "* 1 EXISTS\r\n* OK [UIDVALIDITY {}] stable\r\n",
                        if fault.validity == 0 {
                            42
                        } else {
                            fault.validity
                        }
                    )
                }
            } else if command.starts_with("UID SEARCH ") {
                if fault.vanished && fetched {
                    "* SEARCH\r\n".into()
                } else {
                    "* SEARCH 7\r\n".into()
                }
            } else if command.starts_with("UID FETCH ") {
                fetched = true;
                if fault.disconnect_fetch && (!fault.recover_after_disconnect || connections == 1) {
                    return;
                }
                if fault.reject_fetch {
                    status = "NO";
                    String::new()
                } else if fault.missing_header || fault.vanished {
                    String::new()
                } else {
                    let header = "From: qa@example.com\r\nSubject: Fresh snapshot\r\n\r\n";
                    format!(
                        "{}* 1 FETCH (UID 7 FLAGS () RFC822.SIZE 100 BODY[HEADER] {{{}}}\r\n{header})\r\n",
                        if fault.unsolicited {
                            "* 2 FETCH (UID 99 FLAGS (\\Seen))\r\n"
                        } else {
                            ""
                        },
                        header.len()
                    )
                }
            } else if command.starts_with("UID STORE ") {
                stores += 1;
                if stores == 2 && fault.reject_second_store {
                    status = "NO";
                }
                String::new()
            } else if command == "LOGOUT" {
                if fault.disconnect_logout {
                    return;
                }
                let _ = r
                    .get_mut()
                    .write_all(format!("* BYE bye\r\n{tag} OK done\r\n").as_bytes());
                return;
            } else {
                panic!("unexpected fixture command: {command}");
            };
            if r.get_mut()
                .write_all(format!("{payload}{tag} {status} done\r\n").as_bytes())
                .is_err()
            {
                return;
            }
        }
    })?;
    Ok((cert, server))
}
fn seed(b: &Backend, a: &Account, remote: &str, uid: u32, validity: u32) -> Result<Mail> {
    let id = format!("{}:{remote}:{validity}:{uid}", a.id);
    let mail = parse_message(
        b"Subject: Cached message\r\n\r\nOld body",
        a,
        &id,
        Folder::Inbox,
        remote,
        uid,
    )?;
    b.store
        .save_download(&mail, b"Subject: Cached message\r\n\r\nOld body")?;
    Ok(mail)
}
#[test]
fn failed_or_incomplete_snapshots_keep_cached_mail_and_bodies() -> Result<()> {
    for fault in [
        ImapFault {
            no_validity: true,
            ..Default::default()
        },
        ImapFault {
            missing_header: true,
            ..Default::default()
        },
        ImapFault {
            reject_fetch: true,
            ..Default::default()
        },
        ImapFault {
            disconnect_fetch: true,
            ..Default::default()
        },
    ] {
        let (cert, server) = imap_server(fault)?;
        let (b, mut a, path) = backend(&cert)?;
        a.incoming_port = server.port;
        let old = seed(&b, &a, "INBOX", 8, 42)?;
        assert!(b.sync(&a, Folder::Inbox, 300).is_err());
        assert_eq!(b.store.messages(&a.id, Folder::Inbox, 300)?.len(), 1);
        assert!(b.store.raw(&old.id)?.is_some());
        drop(b);
        std::fs::remove_dir_all(path)?;
    }
    Ok(())
}
#[test]
fn unsolicited_flags_are_not_headers_and_logout_is_best_effort() -> Result<()> {
    let (cert, server) = imap_server(ImapFault {
        unsolicited: true,
        disconnect_logout: true,
        ..Default::default()
    })?;
    let (b, mut a, path) = backend(&cert)?;
    a.incoming_port = server.port;
    let mails = b.sync(&a, Folder::Inbox, 300)?;
    assert_eq!(mails.len(), 1);
    assert_eq!(mails[0].uid, 7);
    b.change_many(&a, &mails, Change::Read(true))?;
    assert!(!b.store.message(&mails[0].id)?.unwrap().unread);
    drop(b);
    std::fs::remove_dir_all(path)?;
    Ok(())
}
#[test]
fn expunge_between_search_and_fetch_reconciles_without_phantom_mail() -> Result<()> {
    let (cert, server) = imap_server(ImapFault {
        vanished: true,
        ..Default::default()
    })?;
    let (b, mut a, path) = backend(&cert)?;
    a.incoming_port = server.port;
    let old = seed(&b, &a, "INBOX", 7, 42)?;
    assert!(b.sync(&a, Folder::Inbox, 300)?.is_empty());
    assert!(b.store.message(&old.id)?.is_none());
    drop(b);
    std::fs::remove_dir_all(path)?;
    Ok(())
}
#[test]
fn uidvalidity_rollover_invalidates_all_views_and_refuses_stale_changes() -> Result<()> {
    let (cert, server) = imap_server(ImapFault {
        validity: 43,
        ..Default::default()
    })?;
    let (b, mut a, path) = backend(&cert)?;
    a.incoming_port = server.port;
    let mut old = seed(&b, &a, "INBOX", 7, 42)?;
    old.starred = true;
    b.store.save_messages(&[old.clone()])?;
    assert!(
        b.change_many(&a, std::slice::from_ref(&old), Change::Read(true))
            .is_err()
    );
    let fresh = b.sync(&a, Folder::Inbox, 300)?;
    assert!(fresh[0].id.ends_with(":43:7"));
    assert!(b.store.raw(&old.id)?.is_none());
    assert!(b.store.messages(&a.id, Folder::Starred, 300)?.is_empty());
    drop(b);
    std::fs::remove_dir_all(path)?;
    Ok(())
}
#[test]
fn confirmed_changes_survive_a_later_mailbox_failure() -> Result<()> {
    let (cert, server) = imap_server(ImapFault {
        reject_second_store: true,
        ..Default::default()
    })?;
    let (b, mut a, path) = backend(&cert)?;
    a.incoming_port = server.port;
    let first = seed(&b, &a, "INBOX", 7, 42)?;
    let second = seed(&b, &a, "Archive", 8, 42)?;
    assert!(
        b.change_many(&a, &[first.clone(), second.clone()], Change::Read(true))
            .is_err()
    );
    assert!(!b.store.message(&first.id)?.unwrap().unread);
    assert!(b.store.message(&second.id)?.unwrap().unread);
    drop(b);
    std::fs::remove_dir_all(path)?;
    Ok(())
}

fn pop_server(
    uidl: &'static str,
    interrupt_first: bool,
) -> Result<(Vec<u8>, Server, Arc<std::sync::atomic::AtomicUsize>)> {
    let (cert, acceptor) = tls_fixture()?;
    let downloads = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let count = downloads.clone();
    let server = Server::new(move |socket| {
        let Ok(stream) = acceptor.accept(socket) else {
            return;
        };
        let mut r = BufReader::new(stream);
        if r.get_mut().write_all(b"+OK ready\r\n").is_err() {
            return;
        }
        loop {
            let mut line = String::new();
            if r.read_line(&mut line).unwrap_or(0) == 0 {
                return;
            }
            let command = line.trim_end();
            let response = if command.starts_with("USER ") || command.starts_with("PASS ") {
                "+OK\r\n".to_owned()
            } else if command == "UIDL" {
                format!("+OK\r\n{uidl}.\r\n")
            } else if let Some(number) = command.strip_prefix("RETR ") {
                let attempt = count.fetch_add(1, Ordering::Relaxed);
                if interrupt_first && attempt == 0 {
                    let _ = r
                        .get_mut()
                        .write_all(b"+OK\r\nSubject: Interrupted\r\n\r\nPartial");
                    return;
                }
                format!("+OK\r\nSubject: Message {number}\r\n\r\n..dot\r\n.\r\n")
            } else if command == "QUIT" {
                return;
            }
            // Cleanup failure is harmless: no DELE was issued.
            else {
                panic!("Unexpected POP3 command: {command}");
            };
            if r.get_mut().write_all(response.as_bytes()).is_err() {
                return;
            }
        }
    })?;
    Ok((cert, server, downloads))
}
#[test]
fn pop3_repairs_partial_cache_and_sorts_uidl_by_message_number() -> Result<()> {
    let (cert, server, downloads) = pop_server("2 uid-two\r\n1 uid-one\r\n", false)?;
    let (b, mut a, path) = backend(&cert)?;
    a.provider = Provider::Pop3;
    a.incoming_port = server.port;
    let id = format!("{}:pop:uid-two", a.id);
    let mut partial = parse_message(
        b"Subject: Old\r\n\r\n",
        &a,
        &id,
        Folder::Archive,
        "INBOX",
        2,
    )?;
    partial.unread = false;
    partial.starred = true;
    b.store.save_messages(&[partial])?; // Simulate an older build interrupted between summary/raw writes.
    assert!(b.sync(&a, Folder::Inbox, 1)?.is_empty());
    let repaired = b.store.message(&id)?.unwrap();
    assert_eq!(repaired.subject, "Message 2");
    assert_eq!(repaired.folder, Folder::Archive);
    assert!(!repaired.unread && repaired.starred);
    assert!(b.store.raw(&id)?.is_some());
    b.sync(&a, Folder::Inbox, 1)?;
    assert_eq!(downloads.load(Ordering::Relaxed), 1);
    drop(b);
    std::fs::remove_dir_all(path)?;
    Ok(())
}
#[test]
fn pop3_interrupted_retr_does_not_create_a_download_identity() -> Result<()> {
    let (cert, server, downloads) = pop_server("1 uid-one\r\n", true)?;
    let (b, mut a, path) = backend(&cert)?;
    a.provider = Provider::Pop3;
    a.incoming_port = server.port;
    // A malformed/truncated response is kept as an error, then an explicit
    // refresh safely retries without having marked the UIDL as downloaded.
    assert!(b.sync(&a, Folder::Inbox, 300).is_err());
    assert!(b.store.messages(&a.id, Folder::Inbox, 300)?.is_empty());
    let mails = b.sync(&a, Folder::Inbox, 300)?;
    assert_eq!(mails.len(), 1);
    assert_eq!(downloads.load(Ordering::Relaxed), 2);
    assert!(b.read(&a, &mails[0])?.body.unwrap().contains(".dot"));
    drop(b);
    std::fs::remove_dir_all(path)?;
    Ok(())
}
#[test]
fn pop3_rejects_duplicate_or_invalid_uidl_before_retrieving_any_mail() -> Result<()> {
    for rows in [
        "1 duplicate\r\n2 duplicate\r\n",
        "1 first\r\n1 second\r\n",
        "0 invalid\r\n",
        "1 invalid extra\r\n",
    ] {
        let (cert, server, downloads) = pop_server(rows, false)?;
        let (b, mut a, path) = backend(&cert)?;
        a.provider = Provider::Pop3;
        a.incoming_port = server.port;
        assert!(b.sync(&a, Folder::Inbox, 300).is_err());
        assert_eq!(downloads.load(Ordering::Relaxed), 0);
        drop(b);
        std::fs::remove_dir_all(path)?;
    }
    Ok(())
}

#[test]
fn read_only_sync_reconnects_once_after_a_transient_disconnect() -> Result<()> {
    let (cert, server) = imap_server(ImapFault {
        disconnect_fetch: true,
        recover_after_disconnect: true,
        ..Default::default()
    })?;
    let (b, mut a, path) = backend(&cert)?;
    a.incoming_port = server.port;
    let messages = b.sync(&a, Folder::Inbox, 300)?;
    assert_eq!(messages.len(), 1);
    assert_eq!(messages[0].subject, "Fresh snapshot");
    drop(b);
    std::fs::remove_dir_all(path)?;
    Ok(())
}

#[test]
fn credentials_cache_denial_until_explicit_retry_and_isolate_accounts() -> Result<()> {
    let (b, _, path) = backend(&[])?;
    let calls = std::cell::Cell::new(0);
    let read = || -> Result<Secret> {
        calls.set(calls.get() + 1);
        anyhow::bail!("Keychain access denied (fixture)")
    };
    for _ in 0..5 {
        let error = b.load_credentials("blocked", read).unwrap_err();
        assert_eq!(
            error.downcast_ref::<AccessFailure>(),
            Some(&AccessFailure::Keychain)
        );
        assert!(format!("{error:#}").contains("Keychain access denied"));
    }
    assert_eq!(
        calls.get(),
        1,
        "background tasks must not repeat system prompts"
    );
    let available = || {
        Ok(Secret {
            password: "fixture".into(),
            ..Default::default()
        })
    };
    assert_eq!(
        b.load_credentials("healthy", available)?.password,
        "fixture"
    );
    b.retry_credentials("blocked");
    assert_eq!(
        b.load_credentials("blocked", available)?.password,
        "fixture"
    );
    b.retry_credentials("healthy");
    assert_eq!(b.load_credentials("healthy", read)?.password, "fixture");
    assert_eq!(
        calls.get(),
        1,
        "a refresh keeps already accessible credentials"
    );
    drop(b);
    std::fs::remove_dir_all(path)?;
    Ok(())
}

#[test]
fn rejected_imap_login_is_actionable_and_failed_reconnect_keeps_local_data() -> Result<()> {
    let (cert, acceptor) = tls_fixture()?;
    let attempts = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let count = attempts.clone();
    let server = Server::new(move |socket| {
        count.fetch_add(1, Ordering::Relaxed);
        let Ok(stream) = acceptor.accept(socket) else {
            return;
        };
        let mut r = BufReader::new(stream);
        r.get_mut().write_all(b"* OK fixture\r\n").unwrap();
        let mut line = String::new();
        r.read_line(&mut line).unwrap();
        let tag = line.split_whitespace().next().unwrap();
        write!(
            r.get_mut(),
            "{tag} NO [AUTHENTICATIONFAILED] Invalid app password\r\n"
        )
        .unwrap();
    })?;
    let (b, mut a, path) = backend(&cert)?;
    a.incoming_port = server.port;
    b.store.save_account(&a)?;
    let mut cached = crate::manage::model::demo_mail().remove(0);
    cached.account = a.id.clone();
    b.store.save_messages(&[cached])?;
    let draft = Draft {
        id: "unsent".into(),
        account: a.id.clone(),
        body: "Keep me".into(),
        ..Default::default()
    };
    b.store.save_draft(&draft)?;
    let error = b.sync(&a, Folder::Inbox, 300).unwrap_err();
    assert_eq!(
        error.downcast_ref::<AccessFailure>(),
        Some(&AccessFailure::Authentication)
    );
    assert!(!crate::sync::retry::transient(&error));
    assert_eq!(
        attempts.load(Ordering::Relaxed),
        1,
        "do not replay rejected authentication"
    );
    assert!(format!("{error:#}").contains("Invalid app password"));
    assert!(
        b.reconnect(&a, "invalid".into(), Arc::new(AtomicBool::new(false)))
            .is_err()
    );
    assert_eq!(b.store.accounts()?.len(), 1);
    assert_eq!(b.store.messages(&a.id, Folder::Inbox, 300)?.len(), 1);
    assert_eq!(b.store.drafts(&a.id)?[0].body, "Keep me");
    drop(b);
    std::fs::remove_dir_all(path)?;
    Ok(())
}
