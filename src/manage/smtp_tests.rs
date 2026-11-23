//! Sending over SMTP against loopback fixtures: Bcc copies, BURL, and what
//! each transaction carries.
use super::protocol_tests::{backend, tls_fixture};
use super::*;
use std::{
    io::{BufRead, BufReader, Read, Write},
    net::{TcpListener, TcpStream},
    sync::{Arc, Mutex},
    time::Duration,
};

/// One SMTP transaction a fixture took: its MAIL FROM, RCPT TOs and message.
#[derive(Clone, Debug, Default)]
pub(super) struct Transaction {
    pub(super) mail: String,
    pub(super) rcpt: Vec<String>,
    pub(super) message: Vec<u8>,
}
impl Transaction {
    pub(super) fn text(&self) -> String {
        String::from_utf8_lossy(&self.message).into_owned()
    }
    /// The message's header section.
    pub(super) fn headers(&self) -> String {
        let text = self.text();
        text.split("\r\n\r\n").next().unwrap_or_default().to_owned()
    }
}

/// What an SMTP fixture saw over all its connections.
#[derive(Clone, Debug, Default)]
pub(super) struct Seen {
    pub(super) commands: Vec<String>,
    pub(super) transactions: Vec<Transaction>,
}

/// A STARTTLS submission server (port 587 style) offering `ehlo` that takes
/// any number of connections until `stop` is set and it sees no more.
/// Replies to each command with what `reply` returns (`None`: the usual
/// success), so tests can script refusals.
pub(super) struct SmtpFixture {
    pub(super) port: u16,
    pub(super) cert: Vec<u8>,
    pub(super) seen: Arc<Mutex<Seen>>,
    /// Serves the same certificate, for another fixture server.
    pub(super) acceptor: native_tls::TlsAcceptor,
    stop: Arc<std::sync::atomic::AtomicBool>,
    thread: Option<std::thread::JoinHandle<()>>,
}
pub(super) type Script = Arc<dyn Fn(&str) -> Option<String> + Send + Sync>;
impl SmtpFixture {
    pub(super) fn start(ehlo: &'static str, reply: Script) -> Result<Self> {
        let (cert, acceptor) = tls_fixture()?;
        let listener = TcpListener::bind("127.0.0.1:0")?;
        listener.set_nonblocking(true)?;
        let port = listener.local_addr()?.port();
        let seen = Arc::new(Mutex::new(Seen::default()));
        let stop = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let (seen2, stop2) = (seen.clone(), stop.clone());
        let shared = acceptor.clone();
        let thread = std::thread::spawn(move || {
            loop {
                let socket = match listener.accept() {
                    Ok((socket, _)) => socket,
                    Err(_) if stop2.load(std::sync::atomic::Ordering::SeqCst) => break,
                    Err(_) => {
                        std::thread::sleep(Duration::from_millis(10));
                        continue;
                    }
                };
                socket.set_nonblocking(false).unwrap();
                socket.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
                serve(socket, &acceptor, ehlo, &reply, &seen2);
            }
        });
        Ok(Self {
            port,
            cert,
            seen,
            acceptor: shared,
            stop,
            thread: Some(thread),
        })
    }
    /// Stops taking connections and returns what it saw.
    pub(super) fn finish(mut self) -> Seen {
        self.stop.store(true, std::sync::atomic::Ordering::SeqCst);
        if let Some(t) = self.thread.take() {
            t.join().unwrap();
        }
        self.seen.lock().unwrap().clone()
    }
}

fn serve(
    socket: TcpStream,
    acceptor: &native_tls::TlsAcceptor,
    ehlo: &str,
    reply: &Script,
    seen: &Mutex<Seen>,
) {
    let mut plain = BufReader::new(socket);
    plain.get_mut().write_all(b"220 fixture ESMTP\r\n").unwrap();
    let mut line = String::new();
    plain.read_line(&mut line).unwrap();
    plain.get_mut().write_all(b"250-fixture\r\n250 STARTTLS\r\n").unwrap();
    line.clear();
    plain.read_line(&mut line).unwrap();
    plain.get_mut().write_all(b"220 go\r\n").unwrap();
    let Ok(tls) = acceptor.accept(plain.into_inner()) else {
        return;
    };
    let mut r = BufReader::new(tls);
    let mut current = Transaction::default();
    loop {
        line.clear();
        if r.read_line(&mut line).unwrap_or(0) == 0 {
            break;
        }
        let command = line.trim_end().to_owned();
        seen.lock().unwrap().commands.push(command.clone());
        let upper = command.to_ascii_uppercase();
        let scripted = reply(&command);
        let answer: String = if let Some(s) = scripted.clone().filter(|_| !upper.starts_with("BDAT") && upper != "DATA") {
            s
        } else if upper.starts_with("EHLO") {
            ehlo.into()
        } else if upper.starts_with("AUTH") {
            "235 2.7.0 ok\r\n".into()
        } else if upper.starts_with("MAIL") {
            current = Transaction {
                mail: command.clone(),
                ..Default::default()
            };
            "250 2.1.0 ok\r\n".into()
        } else if upper.starts_with("RCPT") {
            current.rcpt.push(command.clone());
            "250 2.1.5 ok\r\n".into()
        } else if upper == "RSET" {
            current = Transaction::default();
            "250 2.0.0 ok\r\n".into()
        } else if let Some(rest) = upper.strip_prefix("BDAT ") {
            let n: usize = rest.split(' ').next().unwrap().parse().unwrap();
            let mut chunk = vec![0; n];
            r.read_exact(&mut chunk).unwrap();
            current.message.extend(chunk);
            if rest.ends_with(" LAST") {
                seen.lock().unwrap().transactions.push(std::mem::take(&mut current));
            }
            scripted.unwrap_or_else(|| "250 2.0.0 queued\r\n".into())
        } else if upper.starts_with("BURL ") {
            current.message = command.as_bytes().to_vec();
            seen.lock().unwrap().transactions.push(std::mem::take(&mut current));
            "250 2.0.0 fetched and queued\r\n".into()
        } else if upper == "DATA" {
            r.get_mut().write_all(b"354 go\r\n").unwrap();
            loop {
                let mut data = Vec::new();
                r.read_until(b'\n', &mut data).unwrap();
                if data == b".\r\n" || data.is_empty() {
                    break;
                }
                if data.starts_with(b"..") {
                    data.remove(0);
                }
                current.message.extend(data);
            }
            seen.lock().unwrap().transactions.push(std::mem::take(&mut current));
            scripted.unwrap_or_else(|| "250 2.0.0 queued\r\n".into())
        } else if upper == "QUIT" {
            r.get_mut().write_all(b"221 bye\r\n").unwrap();
            break;
        } else {
            "500 5.5.1 what\r\n".into()
        };
        r.get_mut().write_all(answer.as_bytes()).unwrap();
    }
}

pub(super) fn no_script() -> Script {
    Arc::new(|_| None)
}

/// A POP3 account (no IMAP Sent folder: the sent copy is kept here) whose
/// outgoing server is `fixture`.
pub(super) fn smtp_account(fixture: &SmtpFixture) -> Result<(Backend, Account, std::path::PathBuf)> {
    let (b, mut a, path) = backend(&fixture.cert)?;
    a.provider = Provider::Pop3;
    a.smtp_port = fixture.port;
    Ok((b, a, path))
}

/// The copy of `id`'s draft kept in Sent here (POP3 accounts keep it).
fn sent_copy(b: &Backend, a: &Account, draft: &str) -> Result<String> {
    let raw = b
        .store
        .raw(&format!("{}:sent:{draft}", a.id))?
        .context("no Sent copy")?;
    Ok(String::from_utf8_lossy(&raw).into_owned())
}

/// RFC 5322 §3.6.3: unencrypted mail with Bcc goes out once, to everyone
/// on the envelope, with no Bcc field; the Sent copy keeps the field.
#[test]
fn bcc_never_goes_out_but_stays_in_sent() -> Result<()> {
    let fixture = SmtpFixture::start("250-fixture\r\n250 AUTH PLAIN\r\n", no_script())?;
    let (b, a, path) = smtp_account(&fixture)?;
    let d = Draft {
        id: "bcc-plain".into(),
        account: a.id.clone(),
        to: "Ana <ana@example.com>".into(),
        cc: "cy@example.com".into(),
        bcc: "Bo <bo@example.com>, di@example.com".into(),
        subject: "Plain with Bcc".into(),
        body: "Hello".into(),
        ..Draft::default()
    };
    b.store.save_draft(&d)?;
    b.send(&a, &d)?;
    let sent = sent_copy(&b, &a, &d.id)?;
    let seen = fixture.finish();
    assert_eq!(seen.transactions.len(), 1, "{seen:?}");
    let t = &seen.transactions[0];
    assert_eq!(t.rcpt.len(), 4, "{:?}", t.rcpt);
    let headers = t.headers();
    assert!(!headers.to_ascii_lowercase().contains("\r\nbcc:"), "{headers}");
    assert!(!headers.contains("bo@example.com") && !headers.contains("di@example.com"));
    let sent_headers = sent.split("\r\n\r\n").next().unwrap_or_default();
    assert!(
        sent_headers.contains("Bcc: Bo <bo@example.com>, di@example.com"),
        "{sent_headers}"
    );
    // The Sent copy is otherwise the message as sent.
    assert!(sent.ends_with(&t.text()), "{sent}");
    drop(b);
    std::fs::remove_dir_all(path)?;
    Ok(())
}

fn keyring_for(people: &[&str]) -> Result<(crate::manage::openpgp::Keyring, Vec<pgp::composed::SignedSecretKey>)> {
    use crate::manage::openpgp::{Keyring, test_key};
    use pgp::composed::SignedPublicKey;
    let me = test_key("QA <qa@example.com>", "");
    let mut ring = Keyring::default();
    ring.import(&me.to_armored_string(Default::default())?, "")?;
    let mut keys = Vec::new();
    for who in people {
        let key = test_key(who, "");
        ring.import(&SignedPublicKey::from(key.clone()).to_armored_string(Default::default())?, "")?;
        keys.push(key);
    }
    Ok((ring, keys))
}

/// Whether `key` opens `raw`.
fn opens(raw: &[u8], key: &pgp::composed::SignedSecretKey) -> bool {
    let mut ring = crate::manage::openpgp::Keyring::default();
    ring.import(&key.to_armored_string(Default::default()).unwrap(), "").unwrap();
    crate::manage::openpgp::open(raw, &ring).is_some_and(|(_, v)| v.decrypted)
}

/// Encrypted with Bcc: To and Cc get one copy, encrypted to them, and each
/// Bcc recipient a copy of their own, encrypted to them alone. Every copy
/// shows the same To and Cc, none has a Bcc field or names another Bcc
/// recipient, and the Sent copy keeps Bcc.
#[test]
fn encrypted_bcc_recipients_each_get_their_own_copy() -> Result<()> {
    let fixture = SmtpFixture::start("250-fixture\r\n250 AUTH PLAIN\r\n", no_script())?;
    let (b, a, path) = smtp_account(&fixture)?;
    let (ring, keys) = keyring_for(&[
        "Ana <ana@example.com>",
        "Bo <bo@example.com>",
        "Di <di@example.com>",
    ])?;
    b.set_keyring(ring)?;
    let d = Draft {
        id: "bcc-sealed".into(),
        account: a.id.clone(),
        to: "Ana <ana@example.com>".into(),
        bcc: "bo@example.com, di@example.com".into(),
        subject: "Sealed with Bcc".into(),
        body: "Only for some.".into(),
        encrypt: true,
        ..Draft::default()
    };
    b.store.save_draft(&d)?;
    b.send(&a, &d)?;
    let sent = sent_copy(&b, &a, &d.id)?;
    let seen = fixture.finish();
    assert_eq!(seen.transactions.len(), 3, "{seen:?}");
    let [ana, bo, di] = [&keys[0], &keys[1], &keys[2]];
    // To's copy: to Ana alone, and only Ana's key opens it.
    let main = &seen.transactions[0];
    assert_eq!(main.rcpt, ["RCPT TO:<ana@example.com>"]);
    assert!(opens(&main.message, ana) && !opens(&main.message, bo) && !opens(&main.message, di));
    // Each Bcc copy: to that one recipient, and only their key opens it.
    for (t, (address, key, other)) in seen.transactions[1..]
        .iter()
        .zip([("bo@example.com", bo, di), ("di@example.com", di, bo)])
    {
        assert_eq!(t.rcpt, [format!("RCPT TO:<{address}>")]);
        assert!(opens(&t.message, key), "{address}");
        assert!(!opens(&t.message, ana) && !opens(&t.message, other), "{address}");
    }
    for t in &seen.transactions {
        let headers = t.headers();
        assert!(headers.contains("To: Ana <ana@example.com>"), "{headers}");
        assert!(!headers.to_ascii_lowercase().contains("\r\nbcc:"), "{headers}");
        assert!(!headers.contains("bo@example.com") && !headers.contains("di@example.com"), "{headers}");
        assert!(headers.contains("multipart/encrypted"), "{headers}");
    }
    // The same message to everyone: one Message-ID, at the sender's domain.
    let id = |t: &Transaction| {
        t.headers()
            .lines()
            .find(|l| l.starts_with("Message-ID:"))
            .map(str::to_owned)
    };
    let main_id = id(main).unwrap_or_default();
    assert!(main_id.ends_with("@example.com>"), "{main_id}");
    assert!(seen.transactions.iter().all(|t| id(t).as_deref() == Some(main_id.as_str())));
    assert!(sent.starts_with("Bcc: bo@example.com, di@example.com\r\n"), "{sent}");
    drop(b);
    std::fs::remove_dir_all(path)?;
    Ok(())
}

/// Encrypted with only Bcc recipients: no copy goes to an empty To/Cc
/// list; each Bcc recipient gets theirs.
#[test]
fn encrypted_mail_to_bcc_alone_goes_to_each() -> Result<()> {
    let fixture = SmtpFixture::start("250-fixture\r\n250 AUTH PLAIN\r\n", no_script())?;
    let (b, a, path) = smtp_account(&fixture)?;
    let (ring, _) = keyring_for(&["Bo <bo@example.com>", "Di <di@example.com>"])?;
    b.set_keyring(ring)?;
    let d = Draft {
        id: "bcc-only".into(),
        account: a.id.clone(),
        bcc: "bo@example.com, di@example.com".into(),
        subject: "Only Bcc".into(),
        body: "Hi".into(),
        encrypt: true,
        ..Draft::default()
    };
    b.store.save_draft(&d)?;
    b.send(&a, &d)?;
    let sent = sent_copy(&b, &a, &d.id)?;
    let seen = fixture.finish();
    let rcpts: Vec<_> = seen.transactions.iter().map(|t| t.rcpt.clone()).collect();
    assert_eq!(
        rcpts,
        [vec!["RCPT TO:<bo@example.com>".to_owned()], vec!["RCPT TO:<di@example.com>".to_owned()]]
    );
    assert!(sent.starts_with("Bcc: bo@example.com, di@example.com\r\n"), "{sent}");
    drop(b);
    std::fs::remove_dir_all(path)?;
    Ok(())
}

/// Encrypted mail with Bcc through Proton's or a JMAP server's API is
/// refused before anything is sent, with its own error; without Encrypt,
/// or without Bcc, the refusal doesn't apply.
#[test]
fn encrypted_bcc_is_refused_where_the_service_sends_one_copy() -> Result<()> {
    let (b, mut a, path) = backend(&[])?;
    let d = Draft {
        id: "api-bcc".into(),
        account: a.id.clone(),
        to: "ana@example.com".into(),
        bcc: "bo@example.com".into(),
        subject: "Sealed".into(),
        body: "Hi".into(),
        encrypt: true,
        ..Draft::default()
    };
    for (provider, name) in [(Provider::Proton, "Proton"), (Provider::Jmap, "this JMAP server")] {
        a.provider = provider;
        // Nothing listens on port 1: any network call would fail otherwise.
        a.incoming_port = 1;
        a.smtp_port = 1;
        let error = b.send(&a, &d).unwrap_err();
        let refusal = error
            .downcast_ref::<crate::sync::smtp::EncryptedBccUnsupported>()
            .unwrap_or_else(|| panic!("{error:#}"));
        assert_eq!(refusal.service, name);
        assert!(error.to_string().contains("turn off Encrypt"), "{error}");
        // Without Encrypt, or without Bcc, it goes on to the service (and
        // fails here only for want of one).
        for other in [
            Draft { encrypt: false, ..d.clone() },
            Draft { bcc: String::new(), ..d.clone() },
        ] {
            let error = b.send(&a, &other).unwrap_err();
            assert!(
                error.downcast_ref::<crate::sync::smtp::EncryptedBccUnsupported>().is_none(),
                "{error:#}"
            );
        }
    }
    drop(b);
    std::fs::remove_dir_all(path)?;
    Ok(())
}

/// An IMAP server with URLAUTH and UIDPLUS for BURL: it files appended
/// messages in Sent and authorizes URLs to them.
fn burl_imap(
    listener: TcpListener,
    acceptor: native_tls::TlsAcceptor,
    stop: Arc<std::sync::atomic::AtomicBool>,
) -> std::thread::JoinHandle<Vec<(String, Vec<u8>)>> {
    std::thread::spawn(move || {
        listener.set_nonblocking(true).unwrap();
        let mut seen = Vec::new();
        let mut uid = 0;
        loop {
            let socket = match listener.accept() {
                Ok((socket, _)) => socket,
                Err(_) if stop.load(std::sync::atomic::Ordering::SeqCst) => break,
                Err(_) => {
                    std::thread::sleep(Duration::from_millis(10));
                    continue;
                }
            };
            socket.set_nonblocking(false).unwrap();
            socket.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
            let Ok(tls) = acceptor.accept(socket) else { continue };
            let mut r = BufReader::new(tls);
            r.get_mut().write_all(b"* OK fixture ready\r\n").unwrap();
            let mut line = String::new();
            loop {
                line.clear();
                if r.read_line(&mut line).unwrap_or(0) == 0 {
                    break;
                }
                let trimmed = line.trim_end().to_owned();
                let Some((tag, command)) = trimmed.split_once(' ') else { break };
                let upper = command.to_ascii_uppercase();
                let mut literal = Vec::new();
                if let Some(open) = command.rfind('{')
                    && command.ends_with('}')
                    && let Ok(n) = command[open + 1..command.len() - 1].trim_end_matches('+').parse::<usize>()
                {
                    if !command.ends_with("+}") {
                        r.get_mut().write_all(b"+ go ahead\r\n").unwrap();
                    }
                    literal = vec![0; n];
                    r.read_exact(&mut literal).unwrap();
                    let mut rest = String::new();
                    r.read_line(&mut rest).unwrap();
                }
                seen.push((command.to_owned(), literal.clone()));
                let reply = if upper == "CAPABILITY" {
                    "* CAPABILITY IMAP4rev1 UIDPLUS URLAUTH\r\n".to_owned()
                } else if upper.starts_with("LIST") {
                    "* LIST () \"/\" INBOX\r\n* LIST (\\Sent) \"/\" Sent\r\n".to_owned()
                } else if upper.starts_with("APPEND") {
                    uid += 1;
                    format!("{tag} OK [APPENDUID 7 {uid}] Saved\r\n")
                } else if upper.starts_with("GENURLAUTH") {
                    format!(
                        "* GENURLAUTH \"imap://qa%40example.com@localhost/Sent;UIDVALIDITY=7/;UID={uid};urlauth=submit+qa%40example.com:internal:0123456789abcdef0123456789abcdef\"\r\n"
                    )
                } else if upper == "LOGOUT" {
                    r.get_mut().write_all(format!("* BYE\r\n{tag} OK done\r\n").as_bytes()).unwrap();
                    break;
                } else {
                    String::new()
                };
                if !reply.contains(&format!("{tag} ")) {
                    r.get_mut().write_all(reply.as_bytes()).unwrap();
                    r.get_mut().write_all(format!("{tag} OK done\r\n").as_bytes()).unwrap();
                } else {
                    r.get_mut().write_all(reply.as_bytes()).unwrap();
                }
            }
        }
        seen
    })
}

/// A large message without Bcc goes by BURL (filed in Sent, then fetched
/// by the submission server); with Bcc it is sent as usual, so no copy
/// carries Bcc, and Sent keeps it (RFC 4468 §3, RFC 5322 §3.6.3).
#[test]
#[ignore = "needs the imap-proto with Picacho patch 0007 (GENURLAUTH) in Cargo.lock"]
fn burl_is_used_only_without_bcc() -> Result<()> {
    assert!(crate::sync::transport::reads_urlauth(), "this build reads GENURLAUTH");
    let smtp = SmtpFixture::start(
        "250-fixture\r\n250-CHUNKING\r\n250-BURL imap\r\n250 AUTH PLAIN\r\n",
        no_script(),
    )?;
    let listener = TcpListener::bind("127.0.0.1:0")?;
    let imap_port = listener.local_addr()?.port();
    let stop = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let imap = burl_imap(listener, smtp.acceptor.clone(), stop.clone());
    let (b, mut a, path) = backend(&smtp.cert)?;
    a.provider = Provider::Imap;
    a.smtp_port = smtp.port;
    a.incoming_port = imap_port;
    let attachment = path.join("large.bin");
    std::fs::write(&attachment, vec![b'x'; 1024 * 1024 + 10])?;
    let d = Draft {
        id: "burl".into(),
        account: a.id.clone(),
        to: "ana@example.com".into(),
        subject: "Large".into(),
        body: "Attached.".into(),
        attachments: vec![attachment.clone()],
        ..Draft::default()
    };
    b.store.save_draft(&d)?;
    b.send(&a, &d)?;
    let with_bcc = Draft {
        id: "burl-bcc".into(),
        bcc: "bo@example.com".into(),
        ..d.clone()
    };
    b.store.save_draft(&with_bcc)?;
    b.send(&a, &with_bcc)?;
    b.disconnect(&a.id);
    stop.store(true, std::sync::atomic::Ordering::SeqCst);
    let imap = imap.join().unwrap();
    let seen = smtp.finish();
    let burls = seen.commands.iter().filter(|c| c.starts_with("BURL ")).count();
    assert_eq!(burls, 1, "{:?}", seen.commands);
    assert_eq!(seen.transactions.len(), 2);
    // Without Bcc: by reference.
    assert!(seen.transactions[0].text().starts_with("BURL imap://"));
    // With Bcc: the message itself, without Bcc, to both.
    let t = &seen.transactions[1];
    assert_eq!(t.rcpt.len(), 2, "{:?}", t.rcpt);
    let headers = t.headers();
    assert!(headers.contains("Subject: Large"), "{headers}");
    assert!(!headers.to_ascii_lowercase().contains("\r\nbcc:") && !headers.contains("bo@example.com"));
    // Filed in Sent: the BURL copy before sending, the other after, with
    // its Bcc.
    let appends: Vec<String> = imap
        .iter()
        .filter(|(c, _)| c.to_ascii_uppercase().starts_with("APPEND"))
        .map(|(_, l)| String::from_utf8_lossy(l).into_owned())
        .collect();
    assert_eq!(appends.len(), 2, "{imap:?}");
    assert!(!appends[0].contains("Bcc:"));
    assert!(appends[1].starts_with("Bcc: bo@example.com\r\n"), "{}", &appends[1][..200]);
    drop(b);
    std::fs::remove_dir_all(path)?;
    Ok(())
}

/// The BURL decision alone, which runs in every build: a draft with any
/// Bcc recipient isn't sent by reference.
#[test]
fn burl_is_skipped_for_drafts_with_bcc() {
    let d = Draft {
        to: "ana@example.com".into(),
        ..Draft::default()
    };
    assert!(burl_allowed(&d));
    assert!(burl_allowed(&Draft { bcc: "  ".into(), ..d.clone() }));
    assert!(!burl_allowed(&Draft { bcc: "bo@example.com".into(), ..d.clone() }));
    // A Bcc that can't be read isn't sent by reference either.
    assert!(!burl_allowed(&Draft { bcc: "<<<".into(), ..d }));
}

/// RFC 1870 §4: the size MAIL FROM declares is the size the server gets,
/// by DATA and by BDAT alike. (The composer's MIME already ends every line
/// with CRLF; `data_size_counts_line_endings_as_sent` covers bare ones.)
#[test]
fn declared_size_matches_what_is_sent() -> Result<()> {
    for ehlo in [
        "250-fixture\r\n250-SIZE 10000000\r\n250 AUTH PLAIN\r\n",
        "250-fixture\r\n250-SIZE 10000000\r\n250-CHUNKING\r\n250 AUTH PLAIN\r\n",
    ] {
        let fixture = SmtpFixture::start(ehlo, no_script())?;
        let (b, a, path) = smtp_account(&fixture)?;
        let forwarded = path.join("original.eml");
        std::fs::write(&forwarded, "From: x@example.com\nSubject: old\n\nline one\nline two\n".repeat(20))?;
        let d = Draft {
            id: format!("size-{}", ehlo.len()),
            account: a.id.clone(),
            to: "ana@example.com".into(),
            subject: "Size".into(),
            body: "See attached.".into(),
            attachments: vec![forwarded],
            ..Draft::default()
        };
        b.store.save_draft(&d)?;
        b.send(&a, &d)?;
        let seen = fixture.finish();
        let t = &seen.transactions[0];
        let declared: usize = t
            .mail
            .split(' ')
            .find_map(|p| p.strip_prefix("SIZE="))
            .context("no SIZE")?
            .parse()?;
        assert_eq!(declared, t.message.len(), "{}", t.mail);
        drop(b);
        std::fs::remove_dir_all(path)?;
    }
    Ok(())
}

/// RFC 4865 §3: a server whose max-future-release-date-time is before
/// the message is due isn't handed it, though its interval would allow
/// it; the draft stays scheduled here.
#[test]
fn scheduled_mail_isnt_handed_over_past_the_servers_last_date() -> Result<()> {
    let at = chrono::Utc::now().timestamp() + 3 * 86_400;
    let end = (chrono::Utc::now() + chrono::Duration::days(1)).format("%Y-%m-%dT%H:%M:%SZ");
    let ehlo: &'static str = Box::leak(
        format!("250-fixture\r\n250-FUTURERELEASE 604800 {end}\r\n250 AUTH PLAIN\r\n").into_boxed_str(),
    );
    let fixture = SmtpFixture::start(ehlo, no_script())?;
    let (b, mut a, path) = smtp_account(&fixture)?;
    a.server_send_later = true;
    let d = Draft {
        id: "far".into(),
        account: a.id.clone(),
        to: "ana@example.com".into(),
        subject: "Later".into(),
        body: "Hi".into(),
        send_at: Some(at),
        ..Draft::default()
    };
    b.store.save_draft(&d)?;
    assert!(!b.hand_to_server(&a, &d.id)?);
    assert!(b.store.draft(&d.id)?.is_some());
    let seen = fixture.finish();
    assert!(!seen.commands.iter().any(|c| c.starts_with("MAIL")), "{:?}", seen.commands);
    drop(b);
    std::fs::remove_dir_all(path)?;
    Ok(())
}
