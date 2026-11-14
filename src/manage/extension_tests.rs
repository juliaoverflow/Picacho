//! IMAP extensions and SMTP options over loopback TLS: what the app sends
//! to servers that offer them, and how it behaves on servers that don't.
use super::protocol_tests::{backend, tls_fixture};
use super::*;
use flate2::{Compress, Compression, Decompress, FlushCompress, FlushDecompress};
use std::{
    io::{BufRead, BufReader, Read, Write},
    net::{TcpListener, TcpStream},
    time::Duration,
};

/// The server side of one fixture connection: TLS, optionally compressed
/// once COMPRESS is accepted, reading IMAP commands with their literals.
struct Peer {
    tls: native_tls::TlsStream<TcpStream>,
    plain: Vec<u8>,
    deflate: Option<(Compress, Decompress)>,
}
impl Peer {
    fn accept(listener: &TcpListener, acceptor: &native_tls::TlsAcceptor) -> Option<Self> {
        let (socket, _) = listener.accept().ok()?;
        socket.set_read_timeout(Some(Duration::from_secs(5))).ok()?;
        let tls = acceptor.accept(socket).ok()?;
        let mut peer = Self {
            tls,
            plain: Vec::new(),
            deflate: None,
        };
        peer.send("* OK fixture ready\r\n");
        Some(peer)
    }
    fn fill(&mut self) -> bool {
        let mut buf = [0; 16 * 1024];
        let n = match self.tls.read(&mut buf) {
            Ok(0) | Err(_) => return false,
            Ok(n) => n,
        };
        match &mut self.deflate {
            None => self.plain.extend_from_slice(&buf[..n]),
            Some((_, inflate)) => {
                let mut input = &buf[..n];
                loop {
                    let mut out = vec![0; 64 * 1024];
                    let (before_in, before_out) = (inflate.total_in(), inflate.total_out());
                    inflate
                        .decompress(input, &mut out, FlushDecompress::Sync)
                        .unwrap();
                    let used = (inflate.total_in() - before_in) as usize;
                    let made = (inflate.total_out() - before_out) as usize;
                    self.plain.extend_from_slice(&out[..made]);
                    input = &input[used..];
                    if (input.is_empty() && made < out.len()) || (used == 0 && made == 0) {
                        break;
                    }
                }
            }
        }
        true
    }
    fn line(&mut self) -> Option<String> {
        loop {
            if let Some(i) = self.plain.iter().position(|b| *b == b'\n') {
                let line: Vec<u8> = self.plain.drain(..=i).collect();
                return Some(String::from_utf8_lossy(&line).into_owned());
            }
            if !self.fill() {
                return None;
            }
        }
    }
    fn bytes(&mut self, n: usize) -> Vec<u8> {
        while self.plain.len() < n && self.fill() {}
        let n = n.min(self.plain.len());
        self.plain.drain(..n).collect()
    }
    fn send(&mut self, text: &str) {
        self.send_bytes(text.as_bytes());
    }
    fn send_bytes(&mut self, data: &[u8]) {
        match &mut self.deflate {
            None => self.tls.write_all(data).unwrap(),
            Some((deflate, _)) => {
                let mut out = Vec::new();
                let mut consumed = 0;
                loop {
                    out.reserve(data.len() + 1024);
                    let before = deflate.total_in();
                    deflate
                        .compress_vec(&data[consumed..], &mut out, FlushCompress::Sync)
                        .unwrap();
                    consumed += (deflate.total_in() - before) as usize;
                    if consumed == data.len() && out.len() < out.capacity() {
                        break;
                    }
                }
                self.tls.write_all(&out).unwrap();
            }
        }
    }
    fn compress(&mut self) {
        self.deflate = Some((
            Compress::new(Compression::default(), false),
            Decompress::new(false),
        ));
    }
    /// The next command as (tag, command); a trailing literal (`{n}`) is
    /// accepted with a continuation and returned too.
    fn command(&mut self) -> Option<(String, String, Vec<u8>)> {
        let line = self.line()?;
        let line = line.trim_end();
        let (tag, command) = line.split_once(' ')?;
        let (tag, mut command) = (tag.to_owned(), command.to_owned());
        let mut literal = Vec::new();
        if let Some(open) = command.rfind('{')
            && command.ends_with('}')
            && let Ok(n) = command[open + 1..command.len() - 1].parse::<usize>()
        {
            self.send("+ go ahead\r\n");
            literal = self.bytes(n);
            // The rest of the command line (normally nothing) after it.
            let rest = self.line().unwrap_or_default();
            command.truncate(open);
            command.push_str(rest.trim_end());
        }
        Some((tag, command, literal))
    }
    fn ok(&mut self, tag: &str) {
        self.send(&format!("{tag} OK done\r\n"));
    }
}

/// A server with ID, COMPRESS, NAMESPACE, QUOTA, ACL and SPECIAL-USE: the
/// app introduces itself, compresses the session, puts new folders in the
/// personal namespace, reports storage, refuses what ACL says it can't do,
/// and signs in with the account's user name.
#[test]
fn capable_server_extensions_are_used() -> Result<()> {
    let (cert, acceptor) = tls_fixture()?;
    let listener = TcpListener::bind("127.0.0.1:0")?;
    let port = listener.local_addr()?.port();
    let server = std::thread::spawn(move || {
        let mut seen = Vec::new();
        let mut compressed = 0;
        'connections: while let Some(mut p) = Peer::accept(&listener, &acceptor) {
            while let Some((tag, command, _)) = p.command() {
                let upper = command.to_ascii_uppercase();
                seen.push(command.clone());
                if upper.starts_with("LOGIN") {
                    assert!(command.starts_with("LOGIN \"qa-user\""), "{command}");
                } else if upper == "CAPABILITY" {
                    p.send("* CAPABILITY IMAP4rev1 ID COMPRESS=DEFLATE NAMESPACE QUOTA ACL SPECIAL-USE APPENDLIMIT=100000\r\n");
                } else if upper.starts_with("ID ") {
                    assert!(command.contains("\"Picacho\""), "{command}");
                    p.send("* ID NIL\r\n");
                } else if upper == "COMPRESS DEFLATE" {
                    p.ok(&tag);
                    p.compress();
                    compressed += 1;
                    continue;
                } else if upper.starts_with("LIST") {
                    assert_eq!(command, "LIST \"\" \"*\" RETURN (SPECIAL-USE)");
                    p.send("* LIST (\\HasChildren) \".\" INBOX\r\n");
                    p.send("* LIST (\\Sent) \".\" \"INBOX.Sent\"\r\n");
                    p.send("* LIST () \".\" \"INBOX.Receipts\"\r\n");
                    p.send("* LIST () \".\" \"Archive\"\r\n");
                } else if upper == "NAMESPACE" {
                    p.send("* NAMESPACE ((\"INBOX.\" \".\")) NIL NIL\r\n");
                } else if upper.starts_with("MYRIGHTS") {
                    p.send("* MYRIGHTS \"INBOX.Receipts\" lrswi\r\n");
                } else if upper.starts_with("CREATE") || upper.starts_with("SUBSCRIBE") {
                } else if upper == "GETQUOTAROOT INBOX" {
                    p.send("* QUOTAROOT INBOX \"\"\r\n* QUOTA \"\" (STORAGE 512 2048)\r\n");
                } else if upper == "LOGOUT" {
                    p.send(&format!("* BYE\r\n{tag} OK done\r\n"));
                    break 'connections;
                } else {
                    panic!("Unexpected IMAP command: {command}");
                }
                p.ok(&tag);
            }
        }
        (seen, compressed)
    });
    let (b, mut a, path) = backend(&cert)?;
    a.incoming_port = port;
    a.username = "qa-user".into();
    // NAMESPACE puts new folders inside the inbox, although a folder
    // ("Archive") sits beside it.
    b.create_remote_folder(&a, None, "Travel")?;
    // Without the right to make folders inside it, nothing is sent.
    let refused = b
        .create_remote_folder(&a, Some("INBOX.Receipts"), "2026")
        .unwrap_err();
    assert!(format!("{refused:#}").contains("permission"), "{refused:#}");
    assert_eq!(
        b.quota(&a)?,
        Some(Quota {
            used_kib: 512,
            limit_kib: 2048
        })
    );
    b.disconnect(&a.id);
    let (seen, compressed) = server.join().unwrap();
    let creates: Vec<_> = seen.iter().filter(|c| c.starts_with("CREATE")).collect();
    assert_eq!(creates, ["CREATE \"INBOX.Travel\""]);
    // Each sign-in introduced itself and compressed what followed.
    let logins = seen.iter().filter(|c| c.starts_with("LOGIN")).count();
    assert_eq!(compressed, logins);
    assert_eq!(seen.iter().filter(|c| c.starts_with("ID ")).count(), logins);
    drop(b);
    std::fs::remove_dir_all(path)?;
    Ok(())
}

/// Without MOVE, mail moves by COPY and removing just those messages (UID
/// EXPUNGE). Permanent deletion and emptying Trash do the same. Without
/// UIDPLUS, where EXPUNGE would also remove other deleted mail, nothing is
/// changed.
#[test]
fn moves_and_deletion_expunge_only_the_named_mail() -> Result<()> {
    let (cert, acceptor) = tls_fixture()?;
    let listener = TcpListener::bind("127.0.0.1:0")?;
    let port = listener.local_addr()?.port();
    let server = std::thread::spawn(move || {
        let mut seen = Vec::new();
        for uidplus in [true, false] {
            let Some(mut p) = Peer::accept(&listener, &acceptor) else {
                break;
            };
            while let Some((tag, command, _)) = p.command() {
                let upper = command.to_ascii_uppercase();
                if upper == "CAPABILITY" {
                    p.send(if uidplus {
                        "* CAPABILITY IMAP4rev1 UIDPLUS\r\n"
                    } else {
                        "* CAPABILITY IMAP4rev1\r\n"
                    });
                } else if upper.starts_with("LIST") {
                    p.send("* LIST () \"/\" INBOX\r\n* LIST (\\Trash) \"/\" Trash\r\n* LIST () \"/\" Receipts\r\n");
                } else if upper.starts_with("SELECT") {
                    p.send("* 3 EXISTS\r\n* OK [UIDVALIDITY 42] stable\r\n");
                } else if upper == "UID SEARCH ALL" {
                    p.send("* SEARCH 4 5\r\n");
                } else if upper == "UID SEARCH DELETED" {
                    // Another app left a message marked for deletion.
                    p.send("* SEARCH 3\r\n");
                } else if upper == "LOGOUT" {
                    p.send(&format!("* BYE\r\n{tag} OK done\r\n"));
                    break;
                } else if !upper.starts_with("LOGIN") {
                    seen.push(command);
                }
                p.ok(&tag);
            }
        }
        seen
    });
    let (b, mut a, path) = backend(&cert)?;
    a.incoming_port = port;
    let mail = |folder: Folder, remote: &str, uid: u32| Mail {
        id: format!("{}:{remote}:42:{uid}", a.id),
        account: a.id.clone(),
        folder,
        remote_folder: remote.into(),
        uid,
        ..Mail::default()
    };
    let (inbox, trash, other) = (
        mail(Folder::Inbox, "INBOX", 7),
        mail(Folder::Trash, "Trash", 9),
        mail(Folder::Inbox, "INBOX", 8),
    );
    b.store
        .save_messages(&[inbox.clone(), trash.clone(), other.clone()])?;
    b.change(&a, &inbox, Change::MoveTo("Receipts".into()))?;
    b.delete_forever(&a, std::slice::from_ref(&trash))?;
    assert!(b.store.message(&trash.id)?.is_none());
    assert_eq!(b.empty_mailbox(&a, Folder::Trash)?, 2);
    b.disconnect(&a.id);
    // Without UIDPLUS another message is already marked: refused, unchanged.
    let refused = b
        .change(&a, &other, Change::MoveTo("Receipts".into()))
        .unwrap_err();
    assert!(
        format!("{refused:#}").contains("already marked"),
        "{refused:#}"
    );
    assert!(b.store.message(&other.id)?.is_some());
    let seen = server.join().unwrap();
    assert_eq!(
        seen,
        [
            "UID COPY 7 \"Receipts\"",
            "UID STORE 7 +FLAGS.SILENT (\\Deleted)",
            "UID EXPUNGE 7",
            "UID STORE 9 +FLAGS.SILENT (\\Deleted)",
            "UID EXPUNGE 9",
            "UID STORE 4:5 +FLAGS.SILENT (\\Deleted)",
            "UID EXPUNGE 4:5",
        ]
    );
    drop(b);
    std::fs::remove_dir_all(path)?;
    Ok(())
}

const HEADER: &str = "From: Ana <ana@example.com>\r\nSubject: Receipt\r\nDate: Sat, 24 Oct 2026 10:00:00 +0000\r\nMessage-ID: <r@example.com>\r\n\r\n";

/// Labels sync from Gmail (X-GM-LABELS) and from keywords elsewhere, and
/// are set with the matching STORE. Keywords that aren't atoms, and new
/// keywords where the server doesn't keep them, are refused unsent.
#[test]
fn labels_come_from_gmail_labels_or_keywords() -> Result<()> {
    for gmail in [true, false] {
        let (cert, acceptor) = tls_fixture()?;
        let listener = TcpListener::bind("127.0.0.1:0")?;
        let port = listener.local_addr()?.port();
        let server = std::thread::spawn(move || {
            let mut stores = Vec::new();
            'connections: while let Some(mut p) = Peer::accept(&listener, &acceptor) {
                while let Some((tag, command, _)) = p.command() {
                    let upper = command.to_ascii_uppercase();
                    if upper.starts_with("LOGIN") {
                    } else if upper == "CAPABILITY" {
                        p.send(if gmail {
                            "* CAPABILITY IMAP4rev1 X-GM-EXT-1 MOVE\r\n"
                        } else {
                            "* CAPABILITY IMAP4rev1 MOVE\r\n"
                        });
                    } else if upper.starts_with("LIST") {
                        p.send("* LIST () \"/\" INBOX\r\n");
                    } else if upper.starts_with("SELECT") {
                        p.send("* 1 EXISTS\r\n* OK [UIDVALIDITY 42] stable\r\n* OK [UIDNEXT 8] next\r\n");
                        // Only the first keyword server keeps new keywords.
                        p.send(if gmail || stores.is_empty() {
                            "* OK [PERMANENTFLAGS (\\Seen \\Flagged \\*)] kept\r\n"
                        } else {
                            "* OK [PERMANENTFLAGS (\\Seen \\Flagged)] kept\r\n"
                        });
                    } else if upper.starts_with("UID SEARCH") {
                        p.send("* SEARCH 7\r\n");
                    } else if upper.starts_with("UID FETCH") {
                        let labels = if gmail {
                            " X-GM-MSGID 99 X-GM-LABELS (\\Important \"Receipts\" Caf&AOk- \"Tax \\\"24\\\"\")"
                        } else {
                            ""
                        };
                        let flags = if gmail {
                            "\\Seen"
                        } else {
                            "\\Seen $Forwarded Work"
                        };
                        if upper.contains("HEADER") {
                            p.send(&format!(
                                "* 1 FETCH (UID 7 FLAGS ({flags}){labels} RFC822.SIZE 100 BODY[HEADER] {{{}}}\r\n{HEADER})\r\n",
                                HEADER.len()
                            ));
                        } else {
                            p.send(&format!("* 1 FETCH (UID 7 FLAGS ({flags}){labels})\r\n"));
                        }
                    } else if upper.starts_with("UID STORE") {
                        stores.push(command);
                    } else if upper == "LOGOUT" {
                        p.send(&format!("* BYE\r\n{tag} OK done\r\n"));
                        break 'connections;
                    } else {
                        panic!("Unexpected IMAP command: {command}");
                    }
                    p.ok(&tag);
                }
            }
            stores
        });
        let (b, mut a, path) = backend(&cert)?;
        a.incoming_port = port;
        if gmail {
            a.provider = Provider::Gmail;
            a.service = Service::Gmail;
        }
        let synced = b.sync(&a, Folder::Inbox, 300)?;
        let m = &synced[0];
        if gmail {
            assert_eq!(m.labels, ["Caf&AOk-", "Receipts", "Tax \"24\""]);
        } else {
            // Mail apps' own keywords aren't labels.
            assert_eq!(m.labels, ["Work"]);
        }
        let m = b.change(&a, m, Change::Label("Urgent".into(), true))?;
        assert!(m.labels.contains(&"Urgent".to_owned()), "{:?}", m.labels);
        let cached = b.store.message(&m.id)?.unwrap();
        assert!(cached.labels.contains(&"Urgent".to_owned()));
        if gmail {
            // Labels travel as Gmail names them (modified UTF-7).
            b.change(&a, &m, Change::Label("Caf&AOk-".into(), false))?;
        } else {
            let spaced = b
                .change(&a, &m, Change::Label("two words".into(), true))
                .unwrap_err();
            assert!(
                format!("{spaced:#}").contains("can't contain"),
                "{spaced:#}"
            );
            // The server stops keeping new keywords.
            b.disconnect(&a.id);
            let kept = b
                .change(&a, &m, Change::Label("Later".into(), true))
                .unwrap_err();
            assert!(
                format!("{kept:#}").contains("doesn't allow new labels"),
                "{kept:#}"
            );
            // Removing still works.
            b.change(&a, &m, Change::Label("Urgent".into(), false))?;
        }
        b.disconnect(&a.id);
        let stores = server.join().unwrap();
        if gmail {
            assert_eq!(
                stores,
                [
                    "UID STORE 7 +X-GM-LABELS (\"Urgent\")",
                    "UID STORE 7 -X-GM-LABELS (\"Caf&AOk-\")",
                ]
            );
        } else {
            assert_eq!(
                stores,
                [
                    "UID STORE 7 +FLAGS.SILENT (Urgent)",
                    "UID STORE 7 -FLAGS.SILENT (Urgent)",
                ]
            );
        }
        drop(b);
        std::fs::remove_dir_all(path)?;
    }
    Ok(())
}

/// Drafts go both ways with the server's Drafts folder: written here, they
/// are uploaded (a new version replaces the old); written elsewhere, they
/// appear here; deleted or sent on one side, they go from the other.
#[test]
fn drafts_sync_with_the_drafts_folder() -> Result<()> {
    use std::sync::{Arc, Mutex};
    let (cert, acceptor) = tls_fixture()?;
    let listener = TcpListener::bind("127.0.0.1:0")?;
    let port = listener.local_addr()?.port();
    // The server's Drafts folder, shared with the test.
    let foreign = "From: QA <qa@example.com>\r\nTo: bo@example.com\r\nSubject: From my phone\r\nMIME-Version: 1.0\r\nContent-Type: multipart/mixed; boundary=b\r\n\r\n--b\r\nContent-Type: text/plain\r\n\r\nWritten elsewhere.\r\n--b\r\nContent-Type: image/png; name=dot.png\r\nContent-Disposition: attachment; filename=dot.png\r\nContent-Transfer-Encoding: base64\r\n\r\naGVsbG8=\r\n--b--\r\n";
    let drafts: FixtureFolder = Arc::new(Mutex::new(vec![(1, foreign.as_bytes().to_vec())]));
    let folder = drafts.clone();
    // Whether the server searches by header; without, the app reads the
    // newest messages' headers instead.
    let header_search = Arc::new(std::sync::atomic::AtomicBool::new(true));
    let searches = header_search.clone();
    // Another app saves a draft just as Picacho uploads its first.
    let racer = "From: QA <qa@example.com>\r\nSubject: Saved elsewhere at once\r\n\r\nHi\r\n";
    let server = std::thread::spawn(move || {
        let mut next = 2;
        let mut raced = false;
        'connections: while let Some(mut p) = Peer::accept(&listener, &acceptor) {
            while let Some((tag, command, literal)) = p.command() {
                let upper = command.to_ascii_uppercase();
                let mut drafts = folder.lock().unwrap();
                if upper.starts_with("LOGIN") || upper.starts_with("UID STORE") {
                } else if upper == "CAPABILITY" {
                    p.send("* CAPABILITY IMAP4rev1 UIDPLUS\r\n");
                } else if upper.starts_with("LIST") {
                    p.send("* LIST () \"/\" INBOX\r\n* LIST (\\Drafts) \"/\" Drafts\r\n");
                } else if upper.starts_with("SELECT") {
                    p.send(&format!(
                        "* {} EXISTS\r\n* OK [UIDVALIDITY 42] stable\r\n",
                        drafts.len()
                    ));
                } else if upper == "UID SEARCH ALL" {
                    let uids: String = drafts.iter().map(|(u, _)| format!(" {u}")).collect();
                    p.send(&format!("* SEARCH{uids}\r\n"));
                } else if let Some(id) = upper.strip_prefix("UID SEARCH HEADER X-PICACHO-DRAFT ") {
                    if !searches.load(std::sync::atomic::Ordering::SeqCst) {
                        p.send(&format!("{tag} BAD not searchable\r\n"));
                        continue;
                    }
                    let id = id.trim_matches('"').to_ascii_lowercase();
                    let uids: String = drafts
                        .iter()
                        .filter(|(_, raw)| {
                            String::from_utf8_lossy(raw)
                                .to_ascii_lowercase()
                                .contains(&format!("x-picacho-draft: {id}"))
                        })
                        .map(|(u, _)| format!(" {u}"))
                        .collect();
                    p.send(&format!("* SEARCH{uids}\r\n"));
                } else if let Some(range) = upper.strip_prefix("UID SEARCH UID ") {
                    let from: u32 = range.trim_end_matches(":*").parse().unwrap();
                    let highest = drafts.iter().map(|(u, _)| *u).max();
                    let uids: String = drafts
                        .iter()
                        .map(|(u, _)| *u)
                        .filter(|u| *u >= from || Some(*u) == highest)
                        .map(|u| format!(" {u}"))
                        .collect();
                    p.send(&format!("* SEARCH{uids}\r\n"));
                } else if upper.starts_with("UID FETCH") {
                    let set = command.split(' ').nth(2).unwrap();
                    let wanted = super::protocol_tests::expand(set);
                    let headers_only = upper.contains("HEADER.FIELDS");
                    for (uid, raw) in drafts.iter().filter(|(u, _)| wanted.contains(u)) {
                        let data: Vec<u8> = if headers_only {
                            let text = String::from_utf8_lossy(raw);
                            let header: String = text
                                .lines()
                                .filter(|l| l.starts_with("X-Picacho-Draft"))
                                .map(|l| format!("{l}\r\n"))
                                .collect();
                            format!("{header}\r\n").into_bytes()
                        } else {
                            raw.clone()
                        };
                        let section = if headers_only {
                            "BODY[HEADER.FIELDS (X-PICACHO-DRAFT)]"
                        } else {
                            "BODY[]"
                        };
                        p.send(&format!(
                            "* {uid} FETCH (UID {uid} {section} {{{}}}\r\n",
                            data.len()
                        ));
                        p.send_bytes(&data);
                        p.send(")\r\n");
                    }
                } else if upper.starts_with("APPEND") {
                    assert!(command.contains("\\Draft"), "{command}");
                    drafts.push((next, literal));
                    next += 1;
                    if !raced {
                        raced = true;
                        drafts.push((next, racer.as_bytes().to_vec()));
                        next += 1;
                    }
                } else if let Some(set) = upper.strip_prefix("UID EXPUNGE ") {
                    let gone = super::protocol_tests::expand(set);
                    drafts.retain(|(u, _)| !gone.contains(u));
                } else if upper == "LOGOUT" {
                    p.send(&format!("* BYE\r\n{tag} OK done\r\n"));
                    break 'connections;
                } else {
                    panic!("Unexpected IMAP command: {command}");
                }
                p.ok(&tag);
            }
        }
    });
    let (b, mut a, path) = backend(&cert)?;
    a.incoming_port = port;
    let mine = Draft {
        id: "draft-here".into(),
        account: a.id.clone(),
        to: "ana@example.com".into(),
        bcc: "secret@example.com".into(),
        subject: "Written here".into(),
        body: "First version".into(),
        ..Draft::default()
    };
    b.store.save_draft(&mine)?;
    assert_eq!(b.sync_drafts(&a)?, 1, "the draft written elsewhere appears");
    let local = b.store.drafts(&a.id)?;
    assert_eq!(local.len(), 2);
    let phone = local.iter().find(|d| d.subject == "From my phone").unwrap();
    assert_eq!(phone.to, "bo@example.com");
    assert!(phone.body.contains("Written elsewhere."));
    assert_eq!(std::fs::read(&phone.attachments[0])?, b"hello");
    let uploaded = |drafts: &FixtureFolder| {
        drafts
            .lock()
            .unwrap()
            .iter()
            .filter(|(_, raw)| String::from_utf8_lossy(raw).contains("X-Picacho-Draft: draft-here"))
            .map(|(_, raw)| String::from_utf8_lossy(raw).into_owned())
            .collect::<Vec<_>>()
    };
    let copies = uploaded(&drafts);
    assert_eq!(copies.len(), 1);
    assert!(copies[0].contains("First version") && copies[0].contains("Bcc: secret@example.com"));
    // The other app's draft, saved at once with a higher UID, isn't taken
    // for Picacho's upload.
    let racer_uid = |drafts: &FixtureFolder| {
        drafts
            .lock()
            .unwrap()
            .iter()
            .find(|(_, raw)| raw.as_slice() == racer.as_bytes())
            .map(|(u, _)| *u)
    };
    let raced = racer_uid(&drafts).expect("the other app's draft is there");
    let remote = b.store.draft("draft-here")?.unwrap().remote.unwrap();
    assert_ne!(remote.uid, raced);
    // The next sync takes it in as a draft written elsewhere.
    assert_eq!(b.sync_drafts(&a)?, 1);
    assert_eq!(drafts.lock().unwrap().len(), 3);
    // Unchanged drafts aren't uploaded again; a new version replaces the
    // old, also on a server that can't search by header.
    b.sync_drafts(&a)?;
    assert_eq!(drafts.lock().unwrap().len(), 3);
    header_search.store(false, std::sync::atomic::Ordering::SeqCst);
    let mut edited = b.store.draft("draft-here")?.unwrap();
    edited.body = "Second version".into();
    b.store.save_draft(&edited)?;
    b.sync_drafts(&a)?;
    let copies = uploaded(&drafts);
    assert_eq!(copies.len(), 1);
    assert!(copies[0].contains("Second version"));
    assert_eq!(racer_uid(&drafts), Some(raced), "another app's draft stays");
    // Deleted here: gone from the server.
    b.discard_draft(&phone.id)?;
    let saved_elsewhere = b
        .store
        .drafts(&a.id)?
        .into_iter()
        .find(|d| d.subject == "Saved elsewhere at once")
        .unwrap();
    b.discard_draft(&saved_elsewhere.id)?;
    b.sync_drafts(&a)?;
    assert_eq!(drafts.lock().unwrap().len(), 1);
    // Sent from another device: gone from here, as it was unchanged.
    drafts.lock().unwrap().clear();
    assert_eq!(b.sync_drafts(&a)?, 1);
    assert!(b.store.drafts(&a.id)?.is_empty());
    b.disconnect(&a.id);
    server.join().unwrap();
    drop(b);
    std::fs::remove_dir_all(path)?;
    Ok(())
}

/// A fixture mailbox: (uid, message) pairs.
type FixtureFolder = std::sync::Arc<std::sync::Mutex<Vec<(u32, Vec<u8>)>>>;

/// One SMTP exchange with a fixture server, returning the commands it saw
/// and the message.
fn smtp_exchange(
    ehlo: &'static str,
    a: impl FnOnce(&mut Account),
    draft: Draft,
) -> Result<(Result<Vec<String>>, Vec<String>, String)> {
    let (cert, acceptor) = tls_fixture()?;
    let listener = TcpListener::bind("127.0.0.1:0")?;
    let port = listener.local_addr()?.port();
    let server = std::thread::spawn(move || {
        let mut commands = Vec::new();
        let mut message = Vec::new();
        let Ok((socket, _)) = listener.accept() else {
            return (commands, String::new());
        };
        socket
            .set_read_timeout(Some(Duration::from_secs(5)))
            .unwrap();
        // Plain until STARTTLS, as on port 587.
        let mut plain = BufReader::new(socket);
        plain.get_mut().write_all(b"220 fixture ESMTP\r\n").unwrap();
        let mut line = String::new();
        plain.read_line(&mut line).unwrap();
        assert!(line.starts_with("EHLO"));
        plain
            .get_mut()
            .write_all(b"250-fixture\r\n250 STARTTLS\r\n")
            .unwrap();
        line.clear();
        plain.read_line(&mut line).unwrap();
        assert_eq!(line, "STARTTLS\r\n");
        plain.get_mut().write_all(b"220 go\r\n").unwrap();
        let mut r = BufReader::new(acceptor.accept(plain.into_inner()).unwrap());
        loop {
            line.clear();
            if r.read_line(&mut line).unwrap_or(0) == 0 {
                break;
            }
            commands.push(line.trim_end().to_owned());
            let reply: &str = if line.starts_with("EHLO") {
                ehlo
            } else if line.starts_with("AUTH") {
                "235 ok\r\n"
            } else if line.starts_with("MAIL") || line.starts_with("RCPT") {
                "250 ok\r\n"
            } else if line == "DATA\r\n" {
                r.get_mut().write_all(b"354 go\r\n").unwrap();
                loop {
                    let mut data = Vec::new();
                    r.read_until(b'\n', &mut data).unwrap();
                    if data == b".\r\n" {
                        break;
                    }
                    message.extend(data);
                }
                "250 queued\r\n"
            } else if line == "QUIT\r\n" {
                r.get_mut().write_all(b"221 bye\r\n").unwrap();
                break;
            } else {
                "500 what\r\n"
            };
            r.get_mut().write_all(reply.as_bytes()).unwrap();
        }
        (commands, String::from_utf8_lossy(&message).into_owned())
    });
    let (b, mut account, path) = backend(&cert)?;
    account.provider = Provider::Pop3;
    account.smtp_port = port;
    a(&mut account);
    let draft = Draft {
        account: account.id.clone(),
        ..draft
    };
    let attachment = path.join("dot.png");
    std::fs::write(&attachment, b"\x89PNG")?;
    let draft = Draft {
        attachments: vec![attachment],
        ..draft
    };
    let result = b.send_noting(&account, &draft);
    let (commands, message) = server.join().unwrap();
    drop(b);
    std::fs::remove_dir_all(path)?;
    Ok((result, commands, message))
}

/// The server's SIZE limit is read and kept to; delivery receipts (DSN),
/// read receipts, Reply-To and attachment types go out as asked.
#[test]
fn smtp_limits_receipts_and_reply_to() -> Result<()> {
    let draft = Draft {
        id: "receipts".into(),
        to: "ana@example.com".into(),
        subject: "Receipts".into(),
        body: "Hello".into(),
        reply_to: "Desk <desk@example.com>".into(),
        ..Draft::default()
    };
    let (result, commands, message) = smtp_exchange(
        "250-fixture\r\n250-SIZE 10000000\r\n250-DSN\r\n250 AUTH PLAIN\r\n",
        |a| {
            a.read_receipts = true;
            a.delivery_receipts = true;
            a.username = "qa-login".into();
        },
        draft.clone(),
    )?;
    assert!(result?.is_empty());
    let mail = commands
        .iter()
        .find(|c| c.starts_with("MAIL FROM"))
        .unwrap();
    assert!(
        mail.contains("SIZE=") && mail.contains("RET=HDRS") && mail.contains("ENVID=receipts"),
        "{mail}"
    );
    let rcpt = commands.iter().find(|c| c.starts_with("RCPT TO")).unwrap();
    assert!(rcpt.contains("NOTIFY=SUCCESS,FAILURE,DELAY"), "{rcpt}");
    let auth = commands
        .iter()
        .find(|c| c.starts_with("AUTH PLAIN"))
        .unwrap();
    use base64::Engine;
    let credentials = base64::engine::general_purpose::STANDARD
        .decode(auth.split(' ').nth(2).unwrap())
        .unwrap();
    assert!(String::from_utf8_lossy(&credentials).contains("qa-login"));
    assert!(
        message.contains("Reply-To: Desk <desk@example.com>"),
        "{message}"
    );
    assert!(message.contains("Disposition-Notification-To: <qa@example.com>"));
    assert!(message.contains("Content-Type: image/png"), "{message}");
    // Too large for the server: refused before anything is sent.
    let (result, commands, _) = smtp_exchange(
        "250-fixture\r\n250-SIZE 100\r\n250 AUTH PLAIN\r\n",
        |_| {},
        draft.clone(),
    )?;
    let refused = result.unwrap_err();
    assert!(
        format!("{refused:#}").contains("accepts up to"),
        "{refused:#}"
    );
    assert!(!commands.iter().any(|c| c.starts_with("MAIL")));
    // Receipts asked for where the server can't: sent anyway, with a note.
    let (result, commands, _) = smtp_exchange(
        "250-fixture\r\n250 AUTH PLAIN\r\n",
        |a| a.delivery_receipts = true,
        draft,
    )?;
    assert!(result?[0].contains("delivery receipts"));
    assert!(!commands.iter().any(|c| c.contains("NOTIFY")));
    Ok(())
}

/// The inbox watch waits in IDLE and reports what the server announces,
/// then ends promptly when stopped.
#[test]
fn idle_reports_new_mail() -> Result<()> {
    let (cert, acceptor) = tls_fixture()?;
    let listener = TcpListener::bind("127.0.0.1:0")?;
    let port = listener.local_addr()?.port();
    // How many IDLEs the server has had, for the test to check.
    let idles = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let counted = idles.clone();
    let server = std::thread::spawn(move || {
        let mut seen = Vec::new();
        let Some(mut p) = Peer::accept(&listener, &acceptor) else {
            return seen;
        };
        while let Some((tag, command, _)) = p.command() {
            let upper = command.to_ascii_uppercase();
            seen.push(command.clone());
            if upper == "CAPABILITY" {
                p.send("* CAPABILITY IMAP4rev1 IDLE\r\n");
            } else if upper.starts_with("EXAMINE") {
                p.send("* 1 EXISTS\r\n* OK [UIDVALIDITY 42] stable\r\n");
            } else if upper == "IDLE" {
                p.send("+ idling\r\n");
                let n = counted.fetch_add(1, std::sync::atomic::Ordering::SeqCst) + 1;
                std::thread::sleep(Duration::from_millis(200));
                if n == 1 {
                    // A keepalive in this server's words: no new mail.
                    p.send("* OK Still connected\r\n");
                } else if n == 2 {
                    p.send("* 2 EXISTS\r\n");
                }
                // Waits for DONE (or the client going away).
                match p.line() {
                    Some(done) => {
                        seen.push(done.trim_end().to_owned());
                        p.ok(&tag);
                    }
                    None => break,
                }
                continue;
            } else if upper == "LOGOUT" {
                p.send(&format!("* BYE\r\n{tag} OK done\r\n"));
                break;
            }
            p.ok(&tag);
        }
        seen
    });
    let (b, mut a, path) = backend(&cert)?;
    a.incoming_port = port;
    let b = Arc::new(b);
    let (tx, rx) = std::sync::mpsc::channel();
    b.watch_inbox(&a, move || {
        let _ = tx.send(());
    });
    rx.recv_timeout(Duration::from_secs(10))
        .expect("the new mail was reported");
    assert!(
        idles.load(std::sync::atomic::Ordering::SeqCst) >= 2,
        "the keepalive wasn't reported as new mail"
    );
    assert_eq!(b.watching(), [a.id.clone()]);
    let started = std::time::Instant::now();
    b.stop_watching(&a.id);
    let seen = server.join().unwrap();
    assert!(
        started.elapsed() < Duration::from_secs(5),
        "stopping waits for nothing"
    );
    assert!(seen.contains(&"EXAMINE \"INBOX\"".to_owned()), "{seen:?}");
    assert!(seen.iter().filter(|c| *c == "IDLE").count() >= 1);
    assert!(
        seen.contains(&"DONE".to_owned()),
        "the first IDLE ended to report: {seen:?}"
    );
    drop(b);
    std::fs::remove_dir_all(path)?;
    Ok(())
}

/// Attachments of a server draft that share a name are each kept, as
/// their own files.
#[test]
fn same_named_attachments_get_their_own_files() {
    let mut used = std::collections::HashSet::new();
    assert_eq!(distinct_name("scan.pdf", &mut used), "scan.pdf");
    assert_eq!(distinct_name("Scan.PDF", &mut used), "Scan (2).PDF");
    assert_eq!(distinct_name("scan.pdf", &mut used), "scan (3).pdf");
    assert_eq!(distinct_name("notes", &mut used), "notes");
    assert_eq!(distinct_name("notes", &mut used), "notes (2)");
}
