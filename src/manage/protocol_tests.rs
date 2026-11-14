use std::io::{BufRead, BufReader, Write};
use std::time::Duration;
// Local TLS fixtures exercise the production backend without real mail accounts.
use super::*;
use std::sync::atomic::{AtomicBool, Ordering};

const MESSAGE:&[u8]=b"From: Sophie Chen <sophie@example.com>\r\nTo: qa@example.com\r\nSubject: A message over real TLS\r\nDate: Sat, 24 Oct 2026 10:00:00 +0000\r\nMessage-ID: <test@example.com>\r\nMIME-Version: 1.0\r\nContent-Type: text/plain; charset=utf-8\r\n\r\nHello from the local IMAP fixture.\r\n";

pub(super) fn tls_fixture() -> Result<(Vec<u8>, native_tls::TlsAcceptor)> {
    let dir = std::env::temp_dir().join(format!("pinal-test-tls-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&dir)?;
    let run = |args: &[&str]| -> Result<()> {
        let output = std::process::Command::new("openssl")
            .args(args)
            .current_dir(&dir)
            .output()?;
        ensure!(
            output.status.success(),
            "Could not generate test certificate: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        Ok(())
    };
    run(&[
        "req",
        "-x509",
        "-newkey",
        "rsa:2048",
        "-nodes",
        "-keyout",
        "key.pem",
        "-out",
        "cert.pem",
        "-days",
        "7",
        "-subj",
        "/CN=localhost",
        "-addext",
        "subjectAltName=DNS:localhost,IP:127.0.0.1",
        "-addext",
        "extendedKeyUsage=serverAuth",
        "-addext",
        "keyUsage=digitalSignature,keyEncipherment",
        "-addext",
        "basicConstraints=critical,CA:FALSE",
    ])?;
    run(&[
        "pkcs12",
        "-export",
        "-out",
        "identity.p12",
        "-inkey",
        "key.pem",
        "-in",
        "cert.pem",
        "-passout",
        "pass:fixture-only",
    ])?;
    let cert = std::fs::read(dir.join("cert.pem"))?;
    let identity = native_tls::Identity::from_pkcs12(
        &std::fs::read(dir.join("identity.p12"))?,
        "fixture-only",
    )?;
    std::fs::remove_dir_all(dir)?;
    Ok((cert, native_tls::TlsAcceptor::new(identity)?))
}
pub(super) fn backend(cert: &[u8]) -> Result<(Backend, Account, PathBuf)> {
    let path = std::env::temp_dir().join(format!("pinal-protocol-{}", uuid::Uuid::new_v4()));
    let store = Arc::new(Store::open(&path)?);
    let mut backend = Backend::new(store);
    backend.test_certificate = Some(cert.to_vec());
    backend.test_secret = Some(Secret {
        password: "fixture-password".into(),
        ..Default::default()
    });
    let mut a = Account::new(Provider::Imap);
    a.email = "qa@example.com".into();
    a.name = "QA".into();
    a.incoming_host = "localhost".into();
    a.smtp_host = "localhost".into();
    Ok((backend, a, path))
}
use std::path::PathBuf;

/// UIDs in an IMAP set such as "1:3,7".
pub(super) fn expand(set: &str) -> Vec<u32> {
    set.split(',')
        .flat_map(|part| match part.split_once(':') {
            Some((a, b)) => (a.parse::<u32>().unwrap()..=b.parse::<u32>().unwrap()).collect(),
            None => vec![part.parse::<u32>().unwrap()],
        })
        .collect()
}

#[test]
fn imap_tls_sync_read_flags_move_and_reconcile() -> Result<()> {
    let (cert, tls_acceptor) = tls_fixture()?;
    let listener = std::net::TcpListener::bind("127.0.0.1:0")?;
    listener.set_nonblocking(true)?;
    let port = listener.local_addr()?.port();
    let stop = Arc::new(AtomicBool::new(false));
    let stopped = stop.clone();
    let moved = Arc::new(AtomicBool::new(false));
    let server_moved = moved.clone();
    let server = std::thread::spawn(move || {
        let acceptor = tls_acceptor;
        let mut read = false;
        let mut star = false;
        while !stopped.load(Ordering::Relaxed) {
            let (socket, _) = match listener.accept() {
                Ok(v) => v,
                Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                    std::thread::sleep(Duration::from_millis(10));
                    continue;
                }
                Err(e) => panic!("{e}"),
            };
            socket.set_nonblocking(false).unwrap();
            socket
                .set_read_timeout(Some(Duration::from_secs(5)))
                .unwrap();
            socket
                .set_write_timeout(Some(Duration::from_secs(5)))
                .unwrap();
            let stream = acceptor.accept(socket).unwrap();
            let mut r = BufReader::new(stream);
            r.get_mut().write_all(b"* OK local IMAP ready\r\n").unwrap();
            let mut archive = false;
            loop {
                let mut line = String::new();
                if r.read_line(&mut line).unwrap_or(0) == 0 {
                    break;
                }
                let (tag, command) = line.trim_end().split_once(' ').unwrap();
                let upper = command.to_uppercase();
                let payload = if upper.starts_with("LOGIN ") {
                    assert!(command.contains("qa@example.com"));
                    String::new()
                } else if upper.starts_with("SELECT ") {
                    archive = command.contains("Archive");
                    "* FLAGS (\\Seen \\Flagged)\r\n* 1 EXISTS\r\n* OK [UIDVALIDITY 42] stable\r\n"
                        .to_string()
                } else if upper.starts_with("UID SEARCH ") {
                    if archive == server_moved.load(Ordering::Relaxed) {
                        "* SEARCH 7\r\n".into()
                    } else {
                        "* SEARCH\r\n".into()
                    }
                } else if upper.starts_with("UID FETCH ") {
                    let raw = if upper.contains("HEADER") {
                        MESSAGE[..MESSAGE.windows(4).position(|w| w == b"\r\n\r\n").unwrap() + 4]
                            .to_vec()
                    } else {
                        MESSAGE.to_vec()
                    };
                    let section = if upper.contains("HEADER") {
                        "HEADER"
                    } else {
                        ""
                    };
                    let flags = format!(
                        "{}{}",
                        if read { "\\Seen " } else { "" },
                        if star { "\\Flagged" } else { "" }
                    );
                    format!(
                        "* 1 FETCH (UID 7 FLAGS ({flags}) RFC822.SIZE {} BODY[{section}] {{{}}}\r\n{})\r\n",
                        MESSAGE.len(),
                        raw.len(),
                        String::from_utf8_lossy(&raw)
                    )
                } else if upper.starts_with("UID STORE ") {
                    if command.contains("\\Seen") {
                        read = command.contains("+FLAGS");
                    }
                    if command.contains("\\Flagged") {
                        star = command.contains("+FLAGS");
                    }
                    String::new()
                } else if upper.starts_with("LIST ") {
                    "* LIST (\\Archive) \"/\" \"Archive\"\r\n* LIST (\\Sent) \"/\" \"Sent\"\r\n\
                      * LIST (\\HasChildren) \"/\" \"Projects\"\r\n\
                      * LIST (\\HasNoChildren) \"/\" \"Projects/2026\"\r\n\
                      * LIST (\\Noselect) \"/\" \"[Gmail]\"\r\n* LIST () \"/\" \"Trash\"\r\n"
                        .into()
                } else if upper == "CAPABILITY" {
                    "* CAPABILITY IMAP4rev1 MOVE\r\n".into()
                } else if upper.starts_with("UID MOVE ") {
                    assert!(command.contains("\"Archive\""));
                    server_moved.store(true, Ordering::Relaxed);
                    String::new()
                } else if upper == "LOGOUT" {
                    r.get_mut()
                        .write_all(format!("* BYE goodbye\r\n{tag} OK done\r\n").as_bytes())
                        .unwrap();
                    break;
                } else {
                    panic!("Unexpected IMAP command: {command}")
                };
                r.get_mut()
                    .write_all(format!("{payload}{tag} OK done\r\n").as_bytes())
                    .unwrap();
            }
        }
    });
    let (b, mut a, path) = backend(&cert)?;
    a.incoming_port = port;
    let result = (|| -> Result<()> {
        let messages = b.sync(&a, Folder::Inbox, 300)?;
        assert_eq!(messages.len(), 1);
        let m = &messages[0];
        assert!(m.id.contains(":INBOX:42:7"));
        assert_eq!(m.subject, "A message over real TLS");
        assert!(m.unread);
        assert!(m.body.is_none(), "Sync must not eagerly download bodies");
        let m = b.read(&a, m)?;
        assert!(m.body.as_deref().unwrap().contains("local IMAP fixture"));
        assert!(b.store.raw(&m.id)?.is_some());
        let m = b.change(&a, &m, Change::Read(true))?;
        assert!(!m.unread);
        let m = b.change(&a, &m, Change::Star(true))?;
        assert!(m.starred);
        let synced = b.sync(&a, Folder::Inbox, 300)?;
        assert!(!synced[0].unread);
        assert!(synced[0].starred);
        assert_eq!(
            b.store.folders(&a.id),
            vec![
                RemoteFolder {
                    name: "Projects".into(),
                    label: "Projects".into(),
                    depth: 0,
                    nestable: true,
                },
                RemoteFolder {
                    name: "Projects/2026".into(),
                    label: "2026".into(),
                    depth: 1,
                    nestable: true,
                },
            ],
            "Standard and non-selectable mailboxes are not listed as folders"
        );
        let project = Mailbox::Remote("Projects/2026".into());
        let in_project = b.sync(&a, &project, 300)?;
        assert_eq!(in_project.len(), 1);
        assert_eq!(in_project[0].folder, Folder::Other);
        assert_eq!(in_project[0].mailbox(), project);
        let everything = [(a.id.clone(), i64::MIN)];
        assert_eq!(b.store.download_counts(MAX_MESSAGE, &everything)?, (2, 1));
        assert_eq!(b.download_batch(&a, 25)?, 1);
        assert_eq!(b.store.download_counts(MAX_MESSAGE, &everything)?, (2, 0));
        assert!(b.store.raw(&in_project[0].id)?.is_some());
        assert!(b.store.message(&in_project[0].id)?.unwrap().body.is_none());
        b.change_many(&a, &in_project, Change::Star(false))?;
        b.change_many(&a, &in_project, Change::Star(true))?;
        b.mark_all_read(&a, &project)?;
        let starred = b.sync(&a, Folder::Starred, 300)?;
        assert_eq!(
            starred.len(),
            1,
            "Starred must deduplicate shared message identities across mailboxes"
        );
        b.change(&a, &m, Change::Move(Folder::Archive))?;
        assert!(moved.load(Ordering::Relaxed));
        assert!(b.sync(&a, Folder::Inbox, 300)?.is_empty());
        let archive = b.sync(&a, Folder::Archive, 300)?;
        assert_eq!(archive.len(), 1);
        assert_eq!(archive[0].folder, Folder::Archive);
        let mut stale = archive[0].clone();
        stale.id = stale.id.replace(":42:", ":41:");
        assert!(
            b.read(&a, &stale)
                .unwrap_err()
                .to_string()
                .contains("reset this mailbox")
        );
        assert!(
            b.change(&a, &stale, Change::Read(false))
                .unwrap_err()
                .to_string()
                .contains("reset this mailbox")
        );
        Ok(())
    })();
    if let Err(e) = &result {
        eprintln!("IMAP client error: {e:#}");
    }
    b.disconnect(&a.id);
    stop.store(true, Ordering::Relaxed);
    server.join().unwrap();
    drop(b);
    std::fs::remove_dir_all(path)?;
    result
}

#[test]
fn smtp_starttls_auth_delivery_and_attachment() -> Result<()> {
    let (cert, tls_acceptor) = tls_fixture()?;
    let listener = std::net::TcpListener::bind("127.0.0.1:0")?;
    let port = listener.local_addr()?.port();
    let server = std::thread::spawn(move || {
        let (socket, _) = listener.accept().unwrap();
        socket
            .set_read_timeout(Some(Duration::from_secs(5)))
            .unwrap();
        socket
            .set_write_timeout(Some(Duration::from_secs(5)))
            .unwrap();
        let mut plain = BufReader::new(socket);
        plain
            .get_mut()
            .write_all(b"220 localhost ESMTP fixture\r\n")
            .unwrap();
        let mut line = String::new();
        plain.read_line(&mut line).unwrap();
        assert!(line.starts_with("EHLO"));
        plain
            .get_mut()
            .write_all(b"250-localhost\r\n250 STARTTLS\r\n")
            .unwrap();
        line.clear();
        plain.read_line(&mut line).unwrap();
        assert_eq!(line, "STARTTLS\r\n");
        plain.get_mut().write_all(b"220 Begin TLS\r\n").unwrap();
        let mut r = BufReader::new(tls_acceptor.accept(plain.into_inner()).unwrap());
        let mut delivered = Vec::new();
        loop {
            line.clear();
            if r.read_line(&mut line).unwrap() == 0 {
                break;
            }
            let response = if line.starts_with("EHLO") {
                "250-localhost\r\n250 AUTH PLAIN LOGIN\r\n"
            } else if line.starts_with("AUTH PLAIN ") {
                use base64::Engine;
                let payload = line.split_whitespace().nth(2).unwrap();
                let decoded = base64::engine::general_purpose::STANDARD
                    .decode(payload)
                    .unwrap();
                assert!(String::from_utf8_lossy(&decoded).contains("qa@example.com"));
                "235 Authenticated\r\n"
            } else if line.starts_with("MAIL FROM:") {
                assert!(line.contains("qa@example.com"));
                "250 Sender OK\r\n"
            } else if line.starts_with("RCPT TO:") {
                assert!(line.contains("recipient@example.com"));
                "250 Recipient OK\r\n"
            } else if line == "DATA\r\n" {
                r.get_mut().write_all(b"354 Send message\r\n").unwrap();
                loop {
                    let mut data = Vec::new();
                    r.read_until(b'\n', &mut data).unwrap();
                    if data == b".\r\n" {
                        break;
                    }
                    if data.starts_with(b"..") {
                        data.remove(0);
                    }
                    delivered.extend(data);
                }
                "250 Message queued\r\n"
            } else if line == "QUIT\r\n" {
                r.get_mut().write_all(b"221 Bye\r\n").unwrap();
                break;
            } else if line == "NOOP\r\n" {
                "250 OK\r\n"
            } else {
                panic!("Unexpected SMTP command: {line}")
            };
            r.get_mut().write_all(response.as_bytes()).unwrap();
        }
        delivered
    });
    let (b, mut a, path) = backend(&cert)?;
    a.provider = Provider::Pop3;
    a.smtp_port = port;
    let attachment = path.join("hello.txt");
    std::fs::write(&attachment, b"test attachment bytes")?;
    let d = Draft {
        id: "smtp-test".into(),
        account: a.id.clone(),
        to: "Recipient <recipient@example.com>".into(),
        subject: "SMTP delivery check".into(),
        body: "Hello over verified STARTTLS.".into(),
        attachments: vec![attachment],
        ..Default::default()
    };
    b.store.save_draft(&d)?;
    let result = b.send(&a, &d);
    if let Err(e) = &result {
        eprintln!("SMTP client error: {e:#}");
    }
    let raw = server.join().unwrap();
    result?;
    let parsed = parse_message(&raw, &a, "sent", Folder::Sent, "Sent", 1)?;
    assert_eq!(parsed.subject, d.subject);
    assert!(parsed.body.unwrap().contains("verified STARTTLS"));
    assert_eq!(parsed.attachments[0].name, "hello.txt");
    assert_eq!(parsed.attachments[0].size, 21);
    assert!(b.store.drafts(&a.id)?.is_empty());
    assert_eq!(b.store.messages(&a.id, Folder::Sent, 300)?.len(), 1);
    drop(b);
    std::fs::remove_dir_all(path)?;
    Ok(())
}

#[test]
fn pop3_tls_download_is_deduplicated_and_local_moves_survive_sync() -> Result<()> {
    let (cert, acceptor) = tls_fixture()?;
    let listener = std::net::TcpListener::bind("127.0.0.1:0")?;
    let port = listener.local_addr()?.port();
    let server = std::thread::spawn(move || {
        let mut downloads = 0;
        for _ in 0..2 {
            let (socket, _) = listener.accept().unwrap();
            socket
                .set_read_timeout(Some(Duration::from_secs(5)))
                .unwrap();
            let mut r = BufReader::new(acceptor.accept(socket).unwrap());
            r.get_mut().write_all(b"+OK POP fixture\r\n").unwrap();
            loop {
                let mut line = String::new();
                r.read_line(&mut line).unwrap();
                let response = if line.starts_with("USER ") {
                    assert!(line.contains("qa@example.com"));
                    b"+OK user\r\n".to_vec()
                } else if line.starts_with("PASS ") {
                    b"+OK password\r\n".to_vec()
                } else if line == "UIDL\r\n" {
                    b"+OK ids\r\n1 stable-uid\r\n.\r\n".to_vec()
                } else if line == "RETR 1\r\n" {
                    downloads += 1;
                    [b"+OK message\r\n".as_slice(), MESSAGE, b".\r\n".as_slice()].concat()
                } else if line == "QUIT\r\n" {
                    r.get_mut().write_all(b"+OK bye\r\n").unwrap();
                    break;
                } else {
                    panic!("Unexpected POP3 command: {line}")
                };
                r.get_mut().write_all(&response).unwrap();
            }
        }
        downloads
    });
    let (b, mut a, path) = backend(&cert)?;
    a.provider = Provider::Pop3;
    a.incoming_port = port;
    let messages = b.sync(&a, Folder::Inbox, 300)?;
    assert_eq!(messages.len(), 1);
    let mail = b.change(&a, &messages[0], Change::Read(true))?;
    b.change(&a, &mail, Change::Move(Folder::Archive))?;
    assert!(b.sync(&a, Folder::Inbox, 300)?.is_empty());
    let archive = b.sync(&a, Folder::Archive, 300)?;
    assert_eq!(archive.len(), 1);
    assert!(!archive[0].unread);
    assert!(
        b.read(&a, &archive[0])?
            .body
            .unwrap()
            .contains("local IMAP fixture")
    );
    assert_eq!(
        server.join().unwrap(),
        1,
        "UIDL must prevent repeated downloads"
    );
    drop(b);
    std::fs::remove_dir_all(path)?;
    Ok(())
}

/// Proton Mail Bridge style: a plain greeting upgraded with STARTTLS, and a
/// sync window that limits the server search by date.
#[test]
fn imap_starttls_and_sync_window() -> Result<()> {
    let (cert, acceptor) = tls_fixture()?;
    let listener = std::net::TcpListener::bind("127.0.0.1:0")?;
    let port = listener.local_addr()?.port();
    let server = std::thread::spawn(move || -> Vec<String> {
        let (mut socket, _) = listener.accept().unwrap();
        socket.write_all(b"* OK Bridge ready\r\n").unwrap();
        let mut plain = BufReader::new(socket.try_clone().unwrap());
        let mut line = String::new();
        plain.read_line(&mut line).unwrap();
        let (tag, command) = line.trim_end().split_once(' ').unwrap();
        assert_eq!(command, "STARTTLS", "nothing is sent before encryption");
        socket
            .write_all(format!("{tag} OK begin TLS\r\n").as_bytes())
            .unwrap();
        let mut r = BufReader::new(acceptor.accept(socket).unwrap());
        let mut seen = vec![];
        loop {
            let mut line = String::new();
            if r.read_line(&mut line).unwrap_or(0) == 0 {
                break;
            }
            let (tag, command) = line.trim_end().split_once(' ').unwrap();
            seen.push(command.to_owned());
            let payload = if command.starts_with("SELECT") {
                "* 0 EXISTS\r\n* OK [UIDVALIDITY 9] ok\r\n"
            } else if command.starts_with("LIST") {
                "* LIST () \"/\" INBOX\r\n"
            } else if command.starts_with("UID SEARCH") {
                "* SEARCH\r\n"
            } else if command == "CAPABILITY" {
                "* CAPABILITY IMAP4rev1\r\n"
            } else {
                ""
            };
            let done = command.starts_with("LOGOUT");
            r.get_mut()
                .write_all(format!("{payload}{tag} OK done\r\n").as_bytes())
                .unwrap();
            if done {
                break;
            }
        }
        seen
    });
    let (b, mut a, path) = backend(&cert)?;
    a.service = crate::manage::model::Service::ProtonBridge;
    a.incoming_starttls = true;
    a.incoming_port = port;
    a.sync.days = 30;
    a.validate()?;
    assert!(b.sync(&a, Folder::Inbox, 300)?.is_empty());
    // The connection is kept for the next operation until signed out.
    b.disconnect(&a.id);
    let seen = server.join().unwrap();
    assert!(seen.iter().any(|c| c.starts_with("LOGIN")));
    let search = seen.iter().find(|c| c.starts_with("UID SEARCH")).unwrap();
    let since = (chrono::Utc::now() - chrono::Duration::days(30))
        .format("%-d-%b-%Y")
        .to_string();
    assert_eq!(search, &format!("UID SEARCH ALL SINCE {since}"));
    let _ = std::fs::remove_dir_all(path);
    Ok(())
}

/// Pinal mail Stalwart encrypted at rest: readable in memory once unlocked,
/// never stored readable in the local cache, and locked without the key.
#[test]
fn encrypted_pinal_mail_is_decrypted_in_memory_only() -> Result<()> {
    #[derive(serde::Deserialize)]
    struct Fixture {
        password: String,
        bundle: crate::manage::crypto::KeyBundle,
        stored: String,
    }
    use base64::Engine as _;
    let f: Fixture =
        serde_json::from_str(include_str!("../../tests/fixtures/pinal-encryption.json"))?;
    let stored = base64::engine::general_purpose::STANDARD.decode(&f.stored)?;
    let path = std::env::temp_dir().join(format!("pinal-enc-{}", uuid::Uuid::new_v4()));
    let mut backend = Backend::new(Arc::new(Store::open(&path)?));
    let account = crate::manage::model::Service::Pinal
        .account("jdoe@sonoran.exchange")
        .unwrap();
    backend.test_secret = Some(Secret {
        password: "app-password".into(),
        mail_bundle: serde_json::to_string(&f.bundle)?,
        ..Default::default()
    });
    let mail = parse_message(&stored, &account, "enc-1", Folder::Inbox, "INBOX", 1)?;
    backend.store.save_messages(std::slice::from_ref(&mail))?;
    backend.store.save_raw("enc-1", &stored)?;
    assert!(backend.encrypted(&account));

    let locked = backend.read(&account, &mail).unwrap_err();
    assert!(
        locked
            .downcast_ref::<crate::manage::crypto::Locked>()
            .is_some()
    );
    assert!(backend.unlock(&account, "wrong password").is_err());
    backend.unlock(&account, &f.password)?;

    let read = backend.read(&account, &mail)?;
    assert!(
        read.body
            .as_deref()
            .unwrap_or_default()
            .contains("Picacho reads this")
    );
    assert_eq!(read.attachments.len(), 1);
    assert_eq!(
        backend.attachment(&account, &read, read.attachments[0].part)?,
        b"%PDF secret"
    );
    let cached = backend.store.message("enc-1")?.unwrap();
    assert!(cached.body.is_none() && cached.snippet.is_empty() && cached.attachments.is_empty());
    assert_eq!(backend.store.raw("enc-1")?.unwrap(), stored);
    // A reload from the disk cache gets the body back from memory.
    let reloaded = backend.with_readable(vec![cached]);
    assert!(
        reloaded[0]
            .body
            .as_deref()
            .unwrap_or_default()
            .contains("Picacho reads this")
    );
    // Exercise HTML through real encryption/decryption as well: it must never
    // enter SQLite, including when a readable message is reloaded or locked.
    use pgp::{
        composed::{Deserializable, MessageBuilder, SignedPublicKey},
        crypto::sym::SymmetricKeyAlgorithm,
    };
    let (public, _) = SignedPublicKey::from_string(&f.bundle.keys[0].public_key)?;
    let html = "<h1>Secret HTML</h1><p>Only in memory</p>";
    let inner = format!("Content-Type: text/html; charset=utf-8\r\n\r\n{html}");
    let mut builder = MessageBuilder::from_bytes("", inner.into_bytes())
        .seipd_v1(rand08::thread_rng(), SymmetricKeyAlgorithm::AES256);
    builder.encrypt_to_key(rand08::thread_rng(), &public.public_subkeys[0])?;
    let armored = builder.to_armored_string(rand08::thread_rng(), Default::default())?;
    let raw = format!("From: Bill <bill@example.org>\r\nSubject: Encrypted HTML\r\nMIME-Version: 1.0\r\nContent-Type: multipart/encrypted; protocol=\"application/pgp-encrypted\"; boundary=enc\r\n\r\n--enc\r\nContent-Type: application/pgp-encrypted\r\n\r\nVersion: 1\r\n--enc\r\nContent-Type: application/octet-stream\r\n\r\n{armored}\r\n--enc--\r\n").into_bytes();
    let encrypted = parse_message(&raw, &account, "enc-html", Folder::Inbox, "INBOX", 2)?;
    backend.store.save_download(&encrypted, &raw)?;
    let read = backend.read(&account, &encrypted)?;
    assert_eq!(read.html.as_deref(), Some(html));
    let cached = backend.store.message("enc-html")?.unwrap();
    assert!(cached.html.is_none() && cached.body.is_none());
    assert_eq!(backend.store.raw("enc-html")?.unwrap(), raw);
    assert_eq!(
        backend.with_readable(vec![cached.clone()])[0]
            .html
            .as_deref(),
        Some(html)
    );
    // Even a denied Keychain write must discard decrypted HTML immediately.
    assert!(
        backend
            .load_credentials(&account.id, || anyhow::bail!("fixture keychain denied"))
            .is_err()
    );
    assert!(backend.lock_mail(&account).is_err());
    assert!(backend.with_readable(vec![cached])[0].html.is_none());
    let _ = std::fs::remove_dir_all(path);
    Ok(())
}

#[test]
fn catalog_lists_the_whole_mailbox_in_batches_without_refetching() -> Result<()> {
    let (cert, tls_acceptor) = tls_fixture()?;
    let listener = std::net::TcpListener::bind("127.0.0.1:0")?;
    listener.set_nonblocking(true)?;
    let port = listener.local_addr()?.port();
    let stop = Arc::new(AtomicBool::new(false));
    let stopped = stop.clone();
    let requested = Arc::new(std::sync::Mutex::new(Vec::<u32>::new()));
    let server_requested = requested.clone();
    let searches = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let server_searches = searches.clone();
    let server = std::thread::spawn(move || {
        while !stopped.load(Ordering::Relaxed) {
            let (socket, _) = match listener.accept() {
                Ok(v) => v,
                Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                    std::thread::sleep(Duration::from_millis(10));
                    continue;
                }
                Err(e) => panic!("{e}"),
            };
            socket.set_nonblocking(false).unwrap();
            socket
                .set_read_timeout(Some(Duration::from_secs(5)))
                .unwrap();
            let stream = tls_acceptor.accept(socket).unwrap();
            let mut r = BufReader::new(stream);
            r.get_mut().write_all(b"* OK local IMAP ready\r\n").unwrap();
            loop {
                let mut line = String::new();
                if r.read_line(&mut line).unwrap_or(0) == 0 {
                    break;
                }
                let (tag, command) = line.trim_end().split_once(' ').unwrap();
                let upper = command.to_uppercase();
                let payload = if upper.starts_with("LOGIN ") || upper.starts_with("UID STORE ") {
                    String::new()
                } else if upper.starts_with("SELECT ") {
                    "* 12 EXISTS\r\n* OK [UIDVALIDITY 9] stable\r\n".to_string()
                } else if upper.starts_with("UID SEARCH ") {
                    server_searches.fetch_add(1, Ordering::Relaxed);
                    format!(
                        "* SEARCH {}\r\n",
                        (1..=12)
                            .map(|u: u32| u.to_string())
                            .collect::<Vec<_>>()
                            .join(" ")
                    )
                } else if upper.starts_with("UID FETCH ") {
                    let set = command.split_whitespace().nth(2).unwrap();
                    let mut out = String::new();
                    for uid in expand(set) {
                        server_requested.lock().unwrap().push(uid);
                        let header = format!(
                            "From: Sender {uid} <s{uid}@example.com>\r\nSubject: Message {uid}\r\n\
                             Date: Mon, {uid} Nov 2026 10:00:00 +0000\r\n\r\n"
                        );
                        out.push_str(&format!(
                            "* {uid} FETCH (UID {uid} FLAGS () RFC822.SIZE 100 BODY[HEADER] {{{}}}\r\n{header})\r\n",
                            header.len()
                        ));
                    }
                    out
                } else if upper.starts_with("LIST ") {
                    "* LIST () \"/\" \"INBOX\"\r\n".into()
                } else if upper == "CAPABILITY" {
                    "* CAPABILITY IMAP4rev1\r\n".into()
                } else if upper == "LOGOUT" {
                    r.get_mut()
                        .write_all(format!("* BYE goodbye\r\n{tag} OK done\r\n").as_bytes())
                        .unwrap();
                    break;
                } else {
                    panic!("Unexpected IMAP command: {command}")
                };
                r.get_mut()
                    .write_all(format!("{payload}{tag} OK done\r\n").as_bytes())
                    .unwrap();
            }
        }
    });
    let (b, mut a, path) = backend(&cert)?;
    a.incoming_port = port;
    let result = (|| -> Result<()> {
        // A sync keeps the newest mail current; the catalog fills in the rest.
        assert_eq!(b.sync(&a, Folder::Inbox, 3)?.len(), 3);
        requested.lock().unwrap().clear();
        let mut progress = Vec::new();
        loop {
            let step = b.catalog(&a, Folder::Inbox, 4)?;
            progress.push((step.cataloged, step.total, step.added.len()));
            assert_eq!(step.layout.as_ref().map(|l| l.uids.len()), Some(12));
            if step.complete() {
                break;
            }
        }
        assert_eq!(progress, [(7, 12, 4), (11, 12, 4), (12, 12, 1)]);
        // The sync's search is the layout; batches do not search again.
        assert_eq!(searches.load(Ordering::Relaxed), 1);
        // Newest missing mail first, and nothing already cached again.
        assert_eq!(*requested.lock().unwrap(), [6, 7, 8, 9, 2, 3, 4, 5, 1]);
        let all = b.store.summaries(&a.id, Folder::Inbox)?;
        assert_eq!(all.len(), 12);
        assert_eq!(all[0].subject, "Message 12");
        assert_eq!(all[11].subject, "Message 1");
        assert!(all.iter().all(|m| m.body.is_none() && m.html.is_none()));
        // Once complete, cataloging asks for no headers.
        assert!(b.catalog(&a, Folder::Inbox, 4)?.complete());
        assert_eq!(requested.lock().unwrap().len(), 9);
        Ok(())
    })();
    b.disconnect(&a.id);
    stop.store(true, Ordering::Relaxed);
    server.join().unwrap();
    std::fs::remove_dir_all(path)?;
    result
}

/// Deleting a server folder: refused when the server withholds the right
/// (nothing changes), done when it allows it, and its cached mail forgotten.
#[test]
fn imap_folder_delete_follows_the_servers_rights() -> Result<()> {
    let (cert, acceptor) = tls_fixture()?;
    let listener = std::net::TcpListener::bind("127.0.0.1:0")?;
    let port = listener.local_addr()?.port();
    let server = std::thread::spawn(move || {
        let mut deleted = Vec::new();
        // The first sign-in lacks the right; the refusal drops its connection.
        for allowed in [false, true] {
            let (socket, _) = listener.accept().unwrap();
            socket
                .set_read_timeout(Some(Duration::from_secs(5)))
                .unwrap();
            let mut r = BufReader::new(acceptor.accept(socket).unwrap());
            r.get_mut().write_all(b"* OK local IMAP ready\r\n").unwrap();
            let mut gone = false;
            loop {
                let mut line = String::new();
                // A failed operation drops its connection without LOGOUT.
                if !matches!(r.read_line(&mut line), Ok(n) if n > 0) {
                    break;
                }
                let (tag, command) = line.trim_end().split_once(' ').unwrap();
                let upper = command.to_ascii_uppercase();
                let mut out = String::new();
                if upper.starts_with("LOGIN") {
                } else if upper == "CAPABILITY" {
                    out.push_str("* CAPABILITY IMAP4rev1\r\n");
                } else if upper.starts_with("LIST") {
                    out.push_str("* LIST () \"/\" \"INBOX\"\r\n");
                    out.push_str("* LIST (\\Trash) \"/\" \"Trash\"\r\n");
                    if !gone {
                        out.push_str("* LIST () \"/\" \"Receipts\"\r\n");
                    }
                } else if upper.starts_with("DELETE") {
                    if !allowed {
                        r.get_mut()
                            .write_all(format!("{tag} NO [NOPERM] Permission denied\r\n").as_bytes())
                            .unwrap();
                        continue;
                    }
                    deleted.push(command.to_owned());
                    gone = true;
                } else if upper.starts_with("LOGOUT") {
                    r.get_mut()
                        .write_all(format!("* BYE\r\n{tag} OK done\r\n").as_bytes())
                        .unwrap();
                    break;
                } else {
                    panic!("Unexpected IMAP command: {line}");
                }
                out.push_str(&format!("{tag} OK done\r\n"));
                r.get_mut().write_all(out.as_bytes()).unwrap();
            }
        }
        deleted
    });
    let (b, mut a, path) = backend(&cert)?;
    a.incoming_port = port;
    let cached = Mail {
        id: format!("{}:Receipts:1:1", a.id),
        account: a.id.clone(),
        folder: Folder::Other,
        remote_folder: "Receipts".into(),
        uid: 1,
        ..Mail::default()
    };
    b.store.save_messages(std::slice::from_ref(&cached))?;
    let refused = b.delete_remote_folder(&a, "Receipts").unwrap_err();
    assert!(format!("{refused:#}").contains("didn't allow"), "{refused:#}");
    assert!(b.store.message(&cached.id)?.is_some());
    let left = b.delete_remote_folder(&a, "Receipts")?;
    assert!(left.is_empty(), "{left:?}");
    assert!(b.store.message(&cached.id)?.is_none(), "its cached mail is forgotten");
    // Standard folders are never sent to the server to delete.
    let standard = b.delete_remote_folder(&a, "Trash").unwrap_err();
    assert!(format!("{standard:#}").contains("standard"), "{standard:#}");
    b.disconnect(&a.id);
    assert_eq!(server.join().unwrap(), ["DELETE \"Receipts\""]);
    drop(b);
    std::fs::remove_dir_all(path)?;
    Ok(())
}

/// Folders are made and renamed on the server in the folder list's own
/// form (parent, delimiter, modified UTF-7); a rename takes the folders
/// inside along and forgets their cached mail; refusals are explained.
#[test]
fn imap_folder_create_and_rename() -> Result<()> {
    let (cert, acceptor) = tls_fixture()?;
    let listener = std::net::TcpListener::bind("127.0.0.1:0")?;
    let port = listener.local_addr()?.port();
    let server = std::thread::spawn(move || {
        let mut folders: Vec<String> = vec!["Projects".into(), "Projects/2024".into()];
        let mut commands = Vec::new();
        // A refusal drops its connection, so the client signs in again.
        loop {
            let Ok((socket, _)) = listener.accept() else {
                break;
            };
            socket
                .set_read_timeout(Some(Duration::from_secs(5)))
                .unwrap();
            let mut r = BufReader::new(acceptor.accept(socket).unwrap());
            r.get_mut().write_all(b"* OK local IMAP ready\r\n").unwrap();
            let mut done = false;
            loop {
                let mut line = String::new();
                if !matches!(r.read_line(&mut line), Ok(n) if n > 0) {
                    break;
                }
                let (tag, command) = line.trim_end().split_once(' ').unwrap();
                let upper = command.to_ascii_uppercase();
                let mut out = String::new();
                let unquote = |s: &str| s.trim_matches('"').to_owned();
                if upper.starts_with("LOGIN") || upper.starts_with("SUBSCRIBE") {
                } else if upper == "CAPABILITY" {
                    out.push_str("* CAPABILITY IMAP4rev1\r\n");
                } else if upper.starts_with("LIST") {
                    out.push_str("* LIST () \"/\" \"INBOX\"\r\n");
                    out.push_str("* LIST (\\Trash) \"/\" \"Trash\"\r\n");
                    for f in &folders {
                        out.push_str(&format!("* LIST () \"/\" \"{f}\"\r\n"));
                    }
                } else if upper.starts_with("CREATE") {
                    commands.push(command.to_owned());
                    let name = unquote(&command[7..]);
                    if name.contains("Forbidden") {
                        r.get_mut()
                            .write_all(format!("{tag} NO [NOPERM] Permission denied\r\n").as_bytes())
                            .unwrap();
                        continue;
                    }
                    folders.push(name);
                } else if upper.starts_with("RENAME") {
                    commands.push(command.to_owned());
                    let (from, to) = command[7..].split_once("\" \"").unwrap();
                    let (from, to) = (unquote(from), unquote(to));
                    for f in &mut folders {
                        if *f == from || f.starts_with(&format!("{from}/")) {
                            *f = format!("{to}{}", &f[from.len()..]);
                        }
                    }
                } else if upper.starts_with("LOGOUT") {
                    r.get_mut()
                        .write_all(format!("* BYE\r\n{tag} OK done\r\n").as_bytes())
                        .unwrap();
                    done = true;
                    break;
                } else {
                    panic!("Unexpected IMAP command: {line}");
                }
                out.push_str(&format!("{tag} OK done\r\n"));
                r.get_mut().write_all(out.as_bytes()).unwrap();
            }
            if done {
                break;
            }
        }
        (commands, folders)
    });
    let (b, mut a, path) = backend(&cert)?;
    a.incoming_port = port;
    let made = b.create_remote_folder(&a, None, "Café")?;
    assert!(made.folders.iter().any(|f| f.name == "Caf&AOk-" && f.label == "Café"));
    let inside = b.create_remote_folder(&a, Some("Projects"), "2025")?;
    assert!(inside.folders.iter().any(|f| f.name == "Projects/2025"));
    assert_eq!(b.store.folders(&a.id), inside.folders);
    // Names the server would misread, or that are taken, never reach it.
    let taken = b.create_remote_folder(&a, Some("Projects"), "2024").unwrap_err();
    assert!(format!("{taken:#}").contains("already"), "{taken:#}");
    let refused = b.create_remote_folder(&a, None, "Forbidden").unwrap_err();
    assert!(format!("{refused:#}").contains("didn't allow"), "{refused:#}");
    let cached = Mail {
        id: format!("{}:Projects/2024:1:1", a.id),
        account: a.id.clone(),
        folder: Folder::Other,
        remote_folder: "Projects/2024".into(),
        uid: 1,
        ..Mail::default()
    };
    b.store.save_messages(std::slice::from_ref(&cached))?;
    let renamed = b.rename_remote_folder(&a, "Projects", "Work")?;
    assert_eq!(renamed.moved[0], ("Projects".into(), "Work".into()));
    assert!(renamed.moved.contains(&("Projects/2024".into(), "Work/2024".into())));
    assert!(renamed.folders.iter().any(|f| f.name == "Work/2025"));
    assert!(b.store.message(&cached.id)?.is_none(), "its cached mail is forgotten");
    let standard = b.rename_remote_folder(&a, "Trash", "Bin").unwrap_err();
    assert!(format!("{standard:#}").contains("standard"), "{standard:#}");
    // Keeping the name changes nothing on the server.
    assert!(b.rename_remote_folder(&a, "Work", "Work")?.moved.is_empty());
    b.disconnect(&a.id);
    let (commands, folders) = server.join().unwrap();
    assert_eq!(
        commands,
        [
            "CREATE \"Caf&AOk-\"",
            "CREATE \"Projects/2025\"",
            "CREATE \"Forbidden\"",
            "RENAME \"Projects\" \"Work\"",
        ]
    );
    assert_eq!(folders, ["Work", "Work/2024", "Caf&AOk-", "Work/2025"]);
    drop(b);
    std::fs::remove_dir_all(path)?;
    Ok(())
}

/// Mail moves into the account's own folders with UID MOVE, only once the
/// server still lists the folder; the moved copies leave the cache until
/// that folder syncs.
#[test]
fn imap_mail_moves_into_server_folders() -> Result<()> {
    let (cert, acceptor) = tls_fixture()?;
    let listener = std::net::TcpListener::bind("127.0.0.1:0")?;
    let port = listener.local_addr()?.port();
    let server = std::thread::spawn(move || {
        let mut moves = Vec::new();
        // A refused move drops its connection; the next signs in again.
        'connections: loop {
            let (socket, _) = listener.accept().unwrap();
            socket
                .set_read_timeout(Some(Duration::from_secs(5)))
                .unwrap();
            let mut r = BufReader::new(acceptor.accept(socket).unwrap());
            r.get_mut().write_all(b"* OK local IMAP ready\r\n").unwrap();
            loop {
                let mut line = String::new();
                if !matches!(r.read_line(&mut line), Ok(n) if n > 0) {
                    break;
                }
                let (tag, command) = line.trim_end().split_once(' ').unwrap();
                let upper = command.to_ascii_uppercase();
                let mut out = String::new();
                if upper.starts_with("LOGIN") {
                } else if upper == "CAPABILITY" {
                    out.push_str("* CAPABILITY IMAP4rev1 MOVE\r\n");
                } else if upper.starts_with("LIST") {
                    out.push_str("* LIST () \"/\" \"INBOX\"\r\n");
                    out.push_str("* LIST (\\Trash) \"/\" \"Trash\"\r\n");
                    out.push_str("* LIST () \"/\" \"Receipts\"\r\n");
                } else if upper.starts_with("SELECT") {
                    out.push_str("* 2 EXISTS\r\n* OK [UIDVALIDITY 42] stable\r\n");
                } else if upper.starts_with("UID MOVE") {
                    moves.push(command.to_owned());
                } else if upper.starts_with("LOGOUT") {
                    r.get_mut()
                        .write_all(format!("* BYE\r\n{tag} OK done\r\n").as_bytes())
                        .unwrap();
                    break 'connections;
                } else {
                    panic!("Unexpected IMAP command: {line}");
                }
                out.push_str(&format!("{tag} OK done\r\n"));
                r.get_mut().write_all(out.as_bytes()).unwrap();
            }
        }
        moves
    });
    let (b, mut a, path) = backend(&cert)?;
    a.incoming_port = port;
    let mail = |uid: u32| Mail {
        id: format!("{}:INBOX:42:{uid}", a.id),
        account: a.id.clone(),
        folder: Folder::Inbox,
        remote_folder: "INBOX".into(),
        uid,
        ..Mail::default()
    };
    let mails = [mail(7), mail(8)];
    b.store.save_messages(&mails)?;
    // Folders the server no longer lists, and standard ones, aren't sent.
    let gone = b
        .change_many(&a, &mails, Change::MoveTo("Gone".into()))
        .unwrap_err();
    assert!(format!("{gone:#}").contains("no longer"), "{gone:#}");
    let standard = b
        .change_many(&a, &mails, Change::MoveTo("Trash".into()))
        .unwrap_err();
    assert!(
        format!("{standard:#}").contains("no longer"),
        "{standard:#}"
    );
    let moved = b.change(&a, &mails[0], Change::MoveTo("Receipts".into()))?;
    assert_eq!(moved.mailbox(), Mailbox::Remote("Receipts".into()));
    b.change_many(&a, &mails[1..], Change::MoveTo("Receipts".into()))?;
    for m in &mails {
        assert!(
            b.store.message(&m.id)?.is_none(),
            "moved copies leave the cache"
        );
    }
    b.disconnect(&a.id);
    // POP3 mailboxes have no server folders.
    let mut pop = a.clone();
    pop.provider = Provider::Pop3;
    let refused = b
        .change_many(&pop, &[mail(9)], Change::MoveTo("Receipts".into()))
        .unwrap_err();
    assert!(
        format!("{refused:#}").contains("aren't kept"),
        "{refused:#}"
    );
    assert_eq!(
        server.join().unwrap(),
        ["UID MOVE 7 \"Receipts\"", "UID MOVE 8 \"Receipts\""]
    );
    drop(b);
    std::fs::remove_dir_all(path)?;
    Ok(())
}

/// Labels and Gmail ids come from the server's listing, not the message's
/// source: opening a downloaded message keeps them, in what is shown and in
/// the cache.
#[test]
fn opening_a_message_keeps_its_labels() -> Result<()> {
    let path = std::env::temp_dir().join(format!("mail-test-{}", uuid::Uuid::new_v4()));
    let b = Backend::new(Arc::new(Store::open(&path)?));
    let mut a = Account::new(Provider::Gmail);
    a.email = "qa@example.com".into();
    let mut m = parse_message(MESSAGE, &a, "labelled", Folder::Inbox, "INBOX", 1)?;
    m.labels = vec!["Receipts".into(), "Work".into()];
    m.gmail_id = 42;
    b.store.save_messages(std::slice::from_ref(&m))?;
    b.store.save_raw("labelled", MESSAGE)?;
    // An older copy, as the list may hold one, still opens with the cache's.
    let stale = Mail {
        labels: vec![],
        ..m.clone()
    };
    let read = b.read(&a, &stale)?;
    assert_eq!(read.labels, ["Receipts", "Work"]);
    let cached = b.store.message("labelled")?.unwrap();
    assert_eq!(cached.labels, ["Receipts", "Work"]);
    assert_eq!(cached.gmail_id, 42);
    drop(b);
    std::fs::remove_dir_all(path)?;
    Ok(())
}
