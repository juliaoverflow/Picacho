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
            } else if command == "CAPA" {
                // An older server without CAPA: USER/PASS it is.
                "-ERR unknown command\r\n".to_owned()
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
        loop {
            let mut line = String::new();
            if r.read_line(&mut line).unwrap_or(0) == 0 {
                return;
            }
            let tag = line.split_whitespace().next().unwrap();
            if line.trim_end().ends_with(" CAPABILITY") {
                write!(r.get_mut(), "* CAPABILITY IMAP4rev1\r\n{tag} OK\r\n").unwrap();
                continue;
            }
            write!(
                r.get_mut(),
                "{tag} NO [AUTHENTICATIONFAILED] Invalid app password\r\n"
            )
            .unwrap();
            return;
        }
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

/// A POP3 server for the deletion settings: `messages` (UIDL, subject) in
/// message-number order, recording every DELE and QUIT and applying the
/// deletions only at a QUIT that `confirm_quit` lets succeed.
struct PopMailbox {
    messages: Vec<(String, String)>,
    deleted_at_quit: Vec<String>,
    log: Vec<String>,
    drop_before_quit_reply: bool,
    interrupt_retr: bool,
}
fn deleting_pop_server(mailbox: Arc<std::sync::Mutex<PopMailbox>>) -> Result<(Vec<u8>, Server)> {
    let (cert, acceptor) = tls_fixture()?;
    let server = Server::new(move |socket| {
        let Ok(stream) = acceptor.accept(socket) else {
            return;
        };
        let mut r = BufReader::new(stream);
        if r.get_mut().write_all(b"+OK ready\r\n").is_err() {
            return;
        }
        // This session's snapshot, and the numbers it marked.
        let messages = mailbox.lock().unwrap().messages.clone();
        let mut marked: Vec<u32> = Vec::new();
        loop {
            let mut line = String::new();
            if r.read_line(&mut line).unwrap_or(0) == 0 {
                return;
            }
            let command = line.trim_end().to_owned();
            mailbox.lock().unwrap().log.push(command.clone());
            let response = if command.starts_with("USER ") || command.starts_with("PASS ") {
                "+OK\r\n".to_owned()
            } else if command == "CAPA" {
                "+OK\r\nUSER\r\nUIDL\r\n.\r\n".to_owned()
            } else if command == "UIDL" {
                let mut rows = String::from("+OK\r\n");
                for (i, (uid, _)) in messages.iter().enumerate() {
                    rows.push_str(&format!("{} {uid}\r\n", i + 1));
                }
                rows + ".\r\n"
            } else if let Some(n) = command.strip_prefix("RETR ") {
                let n: usize = n.parse().unwrap();
                if mailbox.lock().unwrap().interrupt_retr {
                    let _ = r.get_mut().write_all(b"+OK\r\nSubject: Cut\r\n\r\nPart");
                    return;
                }
                format!("+OK\r\nSubject: {}\r\n\r\nBody\r\n.\r\n", messages[n - 1].1)
            } else if let Some(n) = command.strip_prefix("DELE ") {
                let n: u32 = n.parse().unwrap();
                if marked.contains(&n) {
                    "-ERR already deleted\r\n".to_owned()
                } else {
                    marked.push(n);
                    "+OK marked\r\n".to_owned()
                }
            } else if command == "QUIT" {
                let mut m = mailbox.lock().unwrap();
                if m.drop_before_quit_reply {
                    // The connection ends before UPDATE: nothing is deleted.
                    return;
                }
                let gone: Vec<String> = marked
                    .iter()
                    .map(|n| messages[*n as usize - 1].0.clone())
                    .collect();
                m.messages.retain(|(uid, _)| !gone.contains(uid));
                m.deleted_at_quit.extend(gone);
                drop(m);
                let _ = r.get_mut().write_all(b"+OK bye\r\n");
                return;
            } else {
                panic!("Unexpected POP3 command: {command}");
            };
            if r.get_mut().write_all(response.as_bytes()).is_err() {
                return;
            }
        }
    })?;
    Ok((cert, server))
}
fn pop_mailbox(uids: &[&str]) -> Arc<std::sync::Mutex<PopMailbox>> {
    Arc::new(std::sync::Mutex::new(PopMailbox {
        messages: uids
            .iter()
            .map(|u| (u.to_string(), format!("Message {u}")))
            .collect(),
        deleted_at_quit: Vec::new(),
        log: Vec::new(),
        drop_before_quit_reply: false,
        interrupt_retr: false,
    }))
}
#[test]
fn pop3_never_deletes_from_the_server_by_default() -> Result<()> {
    let mailbox = pop_mailbox(&["a", "b"]);
    let (cert, server) = deleting_pop_server(mailbox.clone())?;
    let (b, mut a, path) = backend(&cert)?;
    a.provider = Provider::Pop3;
    a.incoming_port = server.port;
    assert_eq!(a.sync.pop_delete_after_days, None);
    assert!(!a.sync.pop_delete_with_local);
    assert_eq!(b.sync(&a, Folder::Inbox, 300)?.len(), 2);
    let mails = b.store.messages(&a.id, Folder::Inbox, 300)?;
    b.delete_forever(&a, &mails[..1])?;
    b.sync(&a, Folder::Inbox, 300)?;
    let m = mailbox.lock().unwrap();
    assert!(m.log.iter().all(|c| !c.starts_with("DELE")), "{:?}", m.log);
    assert_eq!(m.messages.len(), 2);
    drop(m);
    drop(b);
    std::fs::remove_dir_all(path)?;
    Ok(())
}
#[test]
fn pop3_deletes_after_download_only_what_is_saved_here() -> Result<()> {
    let mailbox = pop_mailbox(&["a", "b"]);
    let (cert, server) = deleting_pop_server(mailbox.clone())?;
    let (b, mut a, path) = backend(&cert)?;
    a.provider = Provider::Pop3;
    a.incoming_port = server.port;
    a.sync.pop_delete_after_days = Some(0);
    // An interrupted download saves nothing, so nothing is deleted.
    mailbox.lock().unwrap().interrupt_retr = true;
    assert!(b.sync(&a, Folder::Inbox, 300).is_err());
    assert!(
        mailbox
            .lock()
            .unwrap()
            .log
            .iter()
            .all(|c| !c.starts_with("DELE"))
    );
    mailbox.lock().unwrap().interrupt_retr = false;
    // Downloaded and saved in this check: deleted at its QUIT.
    let mails = b.sync(&a, Folder::Inbox, 300)?;
    assert_eq!(mails.len(), 2);
    for m in &mails {
        assert!(b.store.raw(&m.id)?.is_some());
    }
    assert_eq!(mailbox.lock().unwrap().deleted_at_quit, vec!["a", "b"]);
    // The mail stays here, and isn't downloaded or deleted again.
    let before = mailbox.lock().unwrap().log.len();
    assert_eq!(b.sync(&a, Folder::Inbox, 300)?.len(), 2);
    let m = mailbox.lock().unwrap();
    assert!(
        m.log[before..]
            .iter()
            .all(|c| !c.starts_with("DELE") && !c.starts_with("RETR"))
    );
    drop(m);
    drop(b);
    std::fs::remove_dir_all(path)?;
    Ok(())
}
#[test]
fn pop3_deletion_is_unconfirmed_until_quit_succeeds() -> Result<()> {
    let mailbox = pop_mailbox(&["a"]);
    let (cert, server) = deleting_pop_server(mailbox.clone())?;
    let (b, mut a, path) = backend(&cert)?;
    a.provider = Provider::Pop3;
    a.incoming_port = server.port;
    a.sync.pop_delete_after_days = Some(0);
    mailbox.lock().unwrap().drop_before_quit_reply = true;
    // The mail is saved; the check reports that deleting wasn't confirmed.
    assert!(b.sync(&a, Folder::Inbox, 300).is_err());
    let id = format!("{}:pop:a", a.id);
    assert!(b.store.raw(&id)?.is_some());
    assert_eq!(mailbox.lock().unwrap().messages.len(), 1);
    // The next check tries again, without downloading it again.
    mailbox.lock().unwrap().drop_before_quit_reply = false;
    b.sync(&a, Folder::Inbox, 300)?;
    let m = mailbox.lock().unwrap();
    assert_eq!(m.deleted_at_quit, vec!["a"]);
    assert_eq!(m.log.iter().filter(|c| c.starts_with("RETR")).count(), 1);
    drop(m);
    drop(b);
    std::fs::remove_dir_all(path)?;
    Ok(())
}
#[test]
fn pop3_deletes_after_days_counted_from_download() -> Result<()> {
    let mailbox = pop_mailbox(&["old", "new"]);
    let (cert, server) = deleting_pop_server(mailbox.clone())?;
    let (b, mut a, path) = backend(&cert)?;
    a.provider = Provider::Pop3;
    a.incoming_port = server.port;
    a.sync.pop_delete_after_days = Some(7);
    b.sync(&a, Folder::Inbox, 300)?;
    assert!(mailbox.lock().unwrap().deleted_at_quit.is_empty());
    // "old" was first seen downloaded eight days ago.
    let key = format!("pop_downloaded:{}", a.id);
    let mut seen: std::collections::BTreeMap<String, i64> =
        serde_json::from_str(&b.store.preference(&key).unwrap())?;
    assert_eq!(seen.len(), 2);
    *seen.get_mut("old").unwrap() -= 8 * 86_400;
    b.store
        .set_preference(&key, &serde_json::to_string(&seen)?)?;
    b.sync(&a, Folder::Inbox, 300)?;
    assert_eq!(mailbox.lock().unwrap().deleted_at_quit, vec!["old"]);
    // The deleted one's record goes at the next check; the mail stays here.
    b.sync(&a, Folder::Inbox, 300)?;
    let seen: std::collections::BTreeMap<String, i64> =
        serde_json::from_str(&b.store.preference(&key).unwrap())?;
    assert_eq!(seen.keys().collect::<Vec<_>>(), vec!["new"]);
    assert!(b.store.raw(&format!("{}:pop:old", a.id))?.is_some());
    drop(b);
    std::fs::remove_dir_all(path)?;
    Ok(())
}
#[test]
fn pop3_deletes_with_local_deletions_when_asked() -> Result<()> {
    let mailbox = pop_mailbox(&["a", "b"]);
    let (cert, server) = deleting_pop_server(mailbox.clone())?;
    let (b, mut a, path) = backend(&cert)?;
    a.provider = Provider::Pop3;
    a.incoming_port = server.port;
    a.sync.pop_delete_with_local = true;
    b.sync(&a, Folder::Inbox, 300)?;
    let gone = b.store.message(&format!("{}:pop:a", a.id))?.unwrap();
    b.delete_forever(&a, &[gone])?;
    b.sync(&a, Folder::Inbox, 300)?;
    let m = mailbox.lock().unwrap();
    assert_eq!(m.deleted_at_quit, vec!["a"]);
    assert_eq!(m.messages.len(), 1);
    drop(m);
    // Its record is forgotten once the server no longer has it.
    b.sync(&a, Folder::Inbox, 300)?;
    assert!(
        b.store
            .preference(&format!("pop_erased:{}", a.id))
            .is_none_or(|s| s == "[]")
    );
    drop(b);
    std::fs::remove_dir_all(path)?;
    Ok(())
}

/// A POP3 server on port 110 style: plain until STLS, then TLS. It lists
/// TOP, so older mail is listed by headers; opening one downloads it whole.
/// The third session is refused with LOGIN-DELAY, which pauses background
/// checks as a server limit rather than as a refused sign-in.
#[test]
fn pop3_starttls_top_headers_and_login_delay() -> Result<()> {
    let (cert, acceptor) = tls_fixture()?;
    let seen = Arc::new(Mutex::new(Vec::<String>::new()));
    let log = seen.clone();
    let mut sessions = 0;
    let server = Server::new(move |socket| {
        sessions += 1;
        let mut plain = BufReader::new(socket);
        let _ = plain.get_mut().write_all(b"+OK ready\r\n");
        // Before TLS: CAPA, then STLS; nothing else is accepted.
        loop {
            let mut line = String::new();
            if plain.read_line(&mut line).unwrap_or(0) == 0 {
                return;
            }
            let command = line.trim_end().to_owned();
            log.lock().unwrap().push(format!("plain {command}"));
            match command.as_str() {
                "CAPA" => {
                    let _ = plain
                        .get_mut()
                        .write_all(b"+OK\r\nSTLS\r\nUSER\r\nTOP\r\nUIDL\r\nRESP-CODES\r\n.\r\n");
                }
                "STLS" => {
                    let _ = plain.get_mut().write_all(b"+OK begin TLS\r\n");
                    break;
                }
                other => panic!("Sent in the clear: {other}"),
            }
        }
        let Ok(stream) = acceptor.accept(plain.into_inner()) else {
            return;
        };
        let mut r = BufReader::new(stream);
        let third = sessions == 3;
        loop {
            let mut line = String::new();
            if r.read_line(&mut line).unwrap_or(0) == 0 {
                return;
            }
            let command = line.trim_end().to_owned();
            log.lock().unwrap().push(command.clone());
            let reply = match command.as_str() {
                "CAPA" => "+OK\r\nUSER\r\nTOP\r\nUIDL\r\nRESP-CODES\r\n.\r\n".to_owned(),
                c if c.starts_with("USER ") => "+OK\r\n".to_owned(),
                c if c.starts_with("PASS ") && third => {
                    "-ERR [LOGIN-DELAY] wait a few minutes\r\n".to_owned()
                }
                c if c.starts_with("PASS ") => "+OK\r\n".to_owned(),
                "UIDL" => "+OK\r\n1 one\r\n2 two\r\n.\r\n".to_owned(),
                "LIST" => "+OK\r\n1 120\r\n2 4000\r\n.\r\n".to_owned(),
                c if c.starts_with("TOP ") => {
                    let n = c.split(' ').nth(1).unwrap();
                    format!("+OK\r\nSubject: Message {n}\r\nFrom: a@example.com\r\n\r\n.\r\n")
                }
                c if c.starts_with("RETR ") => {
                    let n = c.split(' ').nth(1).unwrap();
                    format!("+OK\r\nSubject: Message {n}\r\nFrom: a@example.com\r\n\r\nThe whole text.\r\n.\r\n")
                }
                "QUIT" => return,
                other => panic!("Unexpected POP3 command: {other}"),
            };
            if r.get_mut().write_all(reply.as_bytes()).is_err() {
                return;
            }
        }
    })?;
    let (b, mut a, path) = backend(&cert)?;
    a.provider = Provider::Pop3;
    a.incoming_port = server.port;
    a.incoming_starttls = true;
    a.validate()?;
    // Listed by their headers: no RETR.
    let listed = b.catalog(&a, Folder::Inbox, 50)?;
    assert_eq!(listed.total, 2);
    let first = b.store.message(&format!("{}:pop:one", a.id))?.unwrap();
    assert_eq!(first.subject, "Message 1");
    assert!(first.body.is_none() && b.store.raw(&first.id)?.is_none());
    assert_eq!(b.store.message(&format!("{}:pop:two", a.id))?.unwrap().size, 4000);
    // Opening it downloads it whole, by its UIDL identity.
    let read = b.read(&a, &first)?;
    assert!(read.body.unwrap().contains("The whole text."));
    assert!(b.store.raw(&first.id)?.is_some());
    // The server asks to wait: background work pauses, the sign-in isn't
    // called wrong.
    let error = b.catalog(&a, Folder::Inbox, 50).unwrap_err();
    assert!(error.downcast_ref::<crate::manage::status::Throttled>().is_some(), "{error:#}");
    assert!(error.downcast_ref::<AccessFailure>().is_none(), "{error:#}");
    let seen = seen.lock().unwrap();
    assert!(seen.iter().any(|c| c == "TOP 1 0"), "{seen:?}");
    assert_eq!(seen.iter().filter(|c| c.starts_with("RETR")).count(), 1, "{seen:?}");
    assert!(
        seen.iter().filter(|c| c.starts_with("plain ")).all(|c| c == "plain CAPA" || c == "plain STLS"),
        "{seen:?}"
    );
    drop(server);
    drop(b);
    std::fs::remove_dir_all(path)?;
    Ok(())
}
