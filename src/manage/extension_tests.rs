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
                    p.send("* LIST () \".\" \"Projects\"\r\n");
                } else if upper == "NAMESPACE" {
                    p.send("* NAMESPACE ((\"INBOX.\" \".\")) NIL NIL\r\n");
                } else if upper.starts_with("MYRIGHTS \"PROJECTS\"") {
                    p.send("* MYRIGHTS \"Projects\" lrswik\r\n");
                } else if upper.starts_with("MYRIGHTS") {
                    p.send("* MYRIGHTS \"INBOX.Receipts\" lrswi\r\n");
                } else if upper.starts_with("CREATE") || upper.starts_with("SUBSCRIBE") {
                } else if upper == "GETQUOTAROOT INBOX" {
                    p.send("* QUOTAROOT INBOX \"\"\r\n* QUOTA \"\" (STORAGE 512 2048)\r\n");
                } else if upper == "GETQUOTAROOT \"ARCHIVE\"" {
                    // The archive has a root of its own, fuller than the
                    // inbox's: that one shows.
                    p.send("* QUOTAROOT Archive \"archive\"\r\n* QUOTA \"archive\" (STORAGE 900 1000)\r\n");
                } else if upper == "GETQUOTAROOT \"INBOX.SENT\"" {
                    p.send(&format!("{tag} NO no quota root\r\n"));
                    continue;
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
    // Where it may, it does; the rights are asked once, not per folder.
    b.create_remote_folder(&a, Some("Projects"), "2025")?;
    b.create_remote_folder(&a, Some("Projects"), "2026")?;
    assert_eq!(
        b.quota(&a)?,
        Some(Quota {
            used_kib: 900,
            limit_kib: 1000
        })
    );
    b.disconnect(&a.id);
    let (seen, compressed) = server.join().unwrap();
    let creates: Vec<_> = seen.iter().filter(|c| c.starts_with("CREATE")).collect();
    assert_eq!(
        creates,
        [
            "CREATE \"INBOX.Travel\"",
            "CREATE \"Projects.2025\"",
            "CREATE \"Projects.2026\""
        ]
    );
    let asked = seen
        .iter()
        .filter(|c| c.starts_with("MYRIGHTS \"Projects\""))
        .count();
    assert_eq!(asked, 1, "{seen:?}");
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
            assert_eq!(m.labels(), ["Caf&AOk-", "Receipts", "Tax \"24\""]);
        } else {
            // Mail apps' own keywords aren't labels.
            assert_eq!(m.labels(), ["Work"]);
        }
        let m = b.change(&a, m, Change::Label("Urgent".into(), true))?;
        assert!(m.labels().contains(&"Urgent".to_owned()), "{:?}", m.labels());
        let cached = b.store.message(&m.id)?.unwrap();
        assert!(cached.labels().contains(&"Urgent".to_owned()));
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
    // Marked Encrypt: the server's copy is replaced by one sealed to your
    // OpenPGP key, its subject hidden too; it reads back on another device.
    let me = crate::manage::openpgp::test_key("QA <qa@example.com>", "");
    let mut ring = crate::manage::openpgp::Keyring::default();
    ring.import(&me.to_armored_string(Default::default())?, "")?;
    b.set_keyring(ring)?;
    let mut sealed = b.store.draft("draft-here")?.unwrap();
    sealed.encrypt = true;
    sealed.encrypt_subject = true;
    sealed.body = "Secret plans".into();
    b.store.save_draft(&sealed)?;
    b.sync_drafts(&a)?;
    let copies = uploaded(&drafts);
    assert_eq!(copies.len(), 1, "the readable copy is replaced");
    assert!(copies[0].contains("multipart/encrypted") && copies[0].contains("Subject: ..."));
    assert!(!copies[0].contains("Secret plans") && !copies[0].contains("Written here"));
    let sealed_uid = b.store.draft("draft-here")?.unwrap().remote.unwrap().uid;
    // Another device (here: this one, without its copy) opens it.
    b.store.delete_draft("draft-here")?;
    b.sync_drafts(&a)?;
    let back = b.store.drafts(&a.id)?.into_iter().find(|d| d.subject == "Written here").expect("the sealed draft opens");
    assert!(back.body.contains("Secret plans") && back.encrypt && back.encrypt_subject, "{back:?}");
    assert_eq!(back.remote.as_ref().unwrap().uid, sealed_uid);
    // With S/MIME instead: sealed to your certificate, and read back.
    let mut certs = crate::manage::smime::Certificates::default();
    certs.import_identity(&crate::manage::smime::test_identity("qa@example.com", ""), "")?;
    b.set_certificates(certs)?;
    let mut smime = back.clone();
    smime.smime = true;
    smime.body = "Certificate secret".into();
    b.store.save_draft(&smime)?;
    b.sync_drafts(&a)?;
    let copies = uploaded(&drafts);
    assert_eq!(copies.len(), 1);
    assert!(copies[0].contains("enveloped-data") && !copies[0].contains("Certificate secret"));
    b.store.delete_draft(&back.id)?;
    b.sync_drafts(&a)?;
    let back = b.store.drafts(&a.id)?.into_iter().find(|d| d.subject == "Written here").expect("the S/MIME draft opens");
    assert!(back.smime && back.encrypt && back.body.contains("Certificate secret"), "{back:?}");
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
    b.watch_inbox(&a, move |_| {
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

/// What a scripted SMTP server saw: its commands, the message, and how the
/// sign-in went.
struct SmtpSeen {
    commands: Vec<String>,
    message: Vec<u8>,
    signed_in: bool,
}

/// A server offering `ehlo` after STARTTLS. With PIPELINING it answers MAIL
/// only once it has read `recipients` RCPTs as well, as a client that
/// pipelines sends them; one that waits for each reply stalls and fails.
fn scripted_smtp(
    ehlo: &'static str,
    recipients: usize,
    a: impl FnOnce(&mut Account),
    draft: Draft,
) -> Result<(Result<Vec<String>>, SmtpSeen)> {
    scripted_smtp_with(ehlo, recipients, a, draft, |b, a, d| b.send_noting(a, d))
}

/// As `scripted_smtp`, sending with `send` (the draft is saved first).
fn scripted_smtp_with<T>(
    ehlo: &'static str,
    recipients: usize,
    a: impl FnOnce(&mut Account),
    draft: Draft,
    send: impl FnOnce(&Backend, &Account, &Draft) -> Result<T>,
) -> Result<(Result<T>, SmtpSeen)> {
    let (cert, acceptor) = tls_fixture()?;
    let listener = TcpListener::bind("127.0.0.1:0")?;
    let port = listener.local_addr()?.port();
    let pipelining = ehlo.contains("PIPELINING");
    let server = std::thread::spawn(move || {
        let mut seen = SmtpSeen {
            commands: Vec::new(),
            message: Vec::new(),
            signed_in: false,
        };
        let Ok((socket, _)) = listener.accept() else {
            return seen;
        };
        socket
            .set_read_timeout(Some(Duration::from_secs(3)))
            .unwrap();
        let mut plain = BufReader::new(socket);
        plain.get_mut().write_all(b"220 fixture ESMTP\r\n").unwrap();
        let mut line = String::new();
        plain.read_line(&mut line).unwrap();
        plain
            .get_mut()
            .write_all(b"250-fixture\r\n250 STARTTLS\r\n")
            .unwrap();
        line.clear();
        plain.read_line(&mut line).unwrap();
        plain.get_mut().write_all(b"220 go\r\n").unwrap();
        let mut r = BufReader::new(acceptor.accept(plain.into_inner()).unwrap());
        let next = |r: &mut BufReader<native_tls::TlsStream<TcpStream>>| -> Option<String> {
            let mut line = String::new();
            (r.read_line(&mut line).unwrap_or(0) > 0).then(|| line.trim_end().to_owned())
        };
        use base64::Engine as _;
        let b64 = base64::engine::general_purpose::STANDARD;
        while let Some(command) = next(&mut r) {
            seen.commands.push(command.clone());
            let reply: String = if command.starts_with("EHLO") {
                ehlo.into()
            } else if command == "AUTH CRAM-MD5" {
                let challenge = "<1896.697170952@fixture>";
                write!(r.get_mut(), "334 {}\r\n", b64.encode(challenge)).unwrap();
                let answer = b64.decode(next(&mut r).unwrap()).unwrap();
                let answer = String::from_utf8(answer).unwrap();
                use hmac::{KeyInit, Mac};
                let mut mac =
                    <hmac::Hmac<md5::Md5> as KeyInit>::new_from_slice(b"fixture-password").unwrap();
                mac.update(challenge.as_bytes());
                let digest: String = mac
                    .finalize()
                    .into_bytes()
                    .iter()
                    .map(|b| format!("{b:02x}"))
                    .collect();
                seen.signed_in = answer == format!("qa@example.com {digest}");
                if seen.signed_in {
                    "235 ok\r\n"
                } else {
                    "535 no\r\n"
                }
                .into()
            } else if command == "AUTH LOGIN" {
                r.get_mut().write_all(b"334 VXNlcm5hbWU6\r\n").unwrap();
                let user = b64.decode(next(&mut r).unwrap()).unwrap();
                r.get_mut().write_all(b"334 UGFzc3dvcmQ6\r\n").unwrap();
                let password = b64.decode(next(&mut r).unwrap()).unwrap();
                seen.signed_in = user == b"qa@example.com" && password == b"fixture-password";
                if seen.signed_in {
                    "235 ok\r\n"
                } else {
                    "535 no\r\n"
                }
                .into()
            } else if command.starts_with("AUTH PLAIN") {
                seen.signed_in = true;
                "235 ok\r\n".into()
            } else if command.starts_with("MAIL") {
                if pipelining {
                    for _ in 0..recipients {
                        let rcpt = next(&mut r).unwrap();
                        assert!(rcpt.starts_with("RCPT"), "{rcpt}");
                        seen.commands.push(rcpt);
                    }
                    "250 ok\r\n".repeat(recipients + 1)
                } else {
                    "250 ok\r\n".into()
                }
            } else if command.starts_with("RCPT") {
                "250 ok\r\n".into()
            } else if let Some(size) = command.strip_prefix("BDAT ") {
                let n: usize = size.split(' ').next().unwrap().parse().unwrap();
                let mut chunk = vec![0; n];
                r.read_exact(&mut chunk).unwrap();
                seen.message.extend(chunk);
                "250 chunk ok\r\n".into()
            } else if command == "DATA" {
                r.get_mut().write_all(b"354 go\r\n").unwrap();
                loop {
                    let mut data = Vec::new();
                    r.read_until(b'\n', &mut data).unwrap();
                    if data == b".\r\n" {
                        break;
                    }
                    if data.starts_with(b"..") {
                        data.remove(0);
                    }
                    seen.message.extend(data);
                }
                "250 queued\r\n".into()
            } else if command == "QUIT" {
                r.get_mut().write_all(b"221 bye\r\n").unwrap();
                break;
            } else {
                "500 what\r\n".into()
            };
            r.get_mut().write_all(reply.as_bytes()).unwrap();
        }
        seen
    });
    let (b, mut account, path) = backend(&cert)?;
    account.provider = Provider::Pop3;
    account.smtp_port = port;
    a(&mut account);
    let draft = Draft {
        account: account.id.clone(),
        ..draft
    };
    b.store.save_draft(&draft)?;
    let result = send(&b, &account, &draft);
    let seen = server.join().unwrap();
    drop(b);
    std::fs::remove_dir_all(path)?;
    Ok((result, seen))
}

/// PIPELINING sends the sender and recipients at once, CHUNKING sends the
/// message in BDAT chunks without dot-stuffing, REQUIRETLS goes with MAIL
/// when asked for (and nothing is sent where it isn't offered), and the
/// best sign-in the server offers is used: CRAM-MD5 when it's all there is,
/// LOGIN before it.
#[test]
fn smtp_pipelining_chunking_requiretls_and_sign_in_methods() -> Result<()> {
    let draft = Draft {
        id: "modern".into(),
        to: "ana@example.com, bo@example.com".into(),
        subject: "Pipelined".into(),
        body: ".A line starting with a dot".into(),
        ..Draft::default()
    };
    let (result, seen) = scripted_smtp(
        "250-fixture\r\n250-PIPELINING\r\n250-CHUNKING\r\n250-REQUIRETLS\r\n250 AUTH CRAM-MD5\r\n",
        2,
        |a| a.require_tls = true,
        draft.clone(),
    )?;
    result?;
    assert!(seen.signed_in, "{:?}", seen.commands);
    let mail = seen
        .commands
        .iter()
        .find(|c| c.starts_with("MAIL"))
        .unwrap();
    assert!(mail.ends_with(" REQUIRETLS"), "{mail}");
    assert!(
        seen.commands
            .iter()
            .any(|c| c.starts_with("BDAT ") && c.ends_with(" LAST"))
    );
    assert!(!seen.commands.iter().any(|c| c == "DATA"));
    let message = String::from_utf8_lossy(&seen.message);
    assert!(message.contains("Subject: Pipelined"), "{message}");
    // BDAT sends the message as it is: the dot isn't doubled.
    assert!(message.contains("\r\n.A line starting"), "{message}");
    // Encryption all the way asked for, where the server can't promise it.
    let (result, seen) = scripted_smtp(
        "250-fixture\r\n250 AUTH PLAIN\r\n",
        1,
        |a| a.require_tls = true,
        draft.clone(),
    )?;
    assert!(format!("{:#}", result.unwrap_err()).contains("REQUIRETLS"));
    assert!(!seen.commands.iter().any(|c| c.starts_with("MAIL")));
    // An older server: one command at a time, DATA, and LOGIN over CRAM-MD5.
    let (result, seen) = scripted_smtp(
        "250-fixture\r\n250 AUTH CRAM-MD5 LOGIN\r\n",
        2,
        |_| {},
        draft,
    )?;
    result?;
    assert!(seen.signed_in);
    assert!(
        seen.commands.contains(&"AUTH LOGIN".to_owned()),
        "{:?}",
        seen.commands
    );
    assert!(seen.commands.iter().any(|c| c == "DATA"));
    assert!(String::from_utf8_lossy(&seen.message).contains("\r\n.A line starting"));
    Ok(())
}

/// Copies for Sent that couldn't be saved when a message went out wait and
/// are saved with a later check: one already there (found by Message-ID)
/// isn't saved twice, and one the server refuses waits for the next try.
#[test]
fn sent_copies_that_failed_are_saved_later_once() -> Result<()> {
    use std::sync::{Arc, Mutex};
    let (cert, acceptor) = tls_fixture()?;
    let listener = TcpListener::bind("127.0.0.1:0")?;
    let port = listener.local_addr()?.port();
    let sent: FixtureFolder = Arc::new(Mutex::new(vec![(
        1,
        b"Message-ID: <first@example.com>\r\nSubject: Already here\r\n\r\nHi\r\n".to_vec(),
    )]));
    let folder = sent.clone();
    let server = std::thread::spawn(move || {
        let mut appends = 0;
        let mut connections = 0;
        while connections < 2 {
            let Some(mut p) = Peer::accept(&listener, &acceptor) else {
                break;
            };
            connections += 1;
            while let Some((tag, command, literal)) = p.command() {
                let upper = command.to_ascii_uppercase();
                let mut sent = folder.lock().unwrap();
                if upper.starts_with("LOGIN") {
                } else if upper == "CAPABILITY" {
                    p.send("* CAPABILITY IMAP4rev1 UIDPLUS\r\n");
                } else if upper.starts_with("LIST") {
                    // No Drafts folder: the draft sync stops after the copies.
                    p.send("* LIST () \"/\" INBOX\r\n* LIST (\\Sent) \"/\" Sent\r\n");
                } else if upper.starts_with("SELECT") {
                    p.send(&format!(
                        "* {} EXISTS\r\n* OK [UIDVALIDITY 7] stable\r\n",
                        sent.len()
                    ));
                } else if let Some(id) = upper.strip_prefix("UID SEARCH HEADER MESSAGE-ID ") {
                    let id = id.trim_matches('"').to_ascii_lowercase();
                    let uids: String = sent
                        .iter()
                        .filter(|(_, raw)| {
                            String::from_utf8_lossy(raw)
                                .to_ascii_lowercase()
                                .contains(&id)
                        })
                        .map(|(u, _)| format!(" {u}"))
                        .collect();
                    p.send(&format!("* SEARCH{uids}\r\n"));
                } else if upper.starts_with("APPEND") {
                    appends += 1;
                    // The first try fails, as a full mailbox or a dropped
                    // connection would.
                    if appends == 1 {
                        p.send(&format!("{tag} NO [OVERQUOTA] try later\r\n"));
                        continue;
                    }
                    assert!(command.contains("\\Seen"), "{command}");
                    let uid = sent.len() as u32 + 1;
                    sent.push((uid, literal));
                } else if upper == "LOGOUT" {
                    p.send(&format!("* BYE\r\n{tag} OK done\r\n"));
                    break;
                } else {
                    panic!("Unexpected IMAP command: {command}");
                }
                p.ok(&tag);
            }
        }
    });
    let (b, mut a, path) = backend(&cert)?;
    a.incoming_port = port;
    b.queue_sent_copy(
        &a.id,
        b"Message-ID: <first@example.com>\r\nSubject: Already here\r\n\r\nHi\r\n",
    )?;
    b.queue_sent_copy(
        &a.id,
        b"Message-ID: <second@example.com>\r\nSubject: New\r\n\r\nHello\r\n",
    )?;
    assert_eq!(b.queued_sent_copies(&a.id), 2);
    b.sync_drafts(&a)?;
    // The first was there already; the second was refused and waits.
    assert_eq!(b.queued_sent_copies(&a.id), 1);
    assert_eq!(sent.lock().unwrap().len(), 1);
    b.disconnect(&a.id);
    b.sync_drafts(&a)?;
    assert_eq!(b.queued_sent_copies(&a.id), 0);
    let saved = sent.lock().unwrap();
    assert_eq!(saved.len(), 2);
    assert!(String::from_utf8_lossy(&saved[1].1).contains("<second@example.com>"));
    drop(saved);
    b.disconnect(&a.id);
    server.join().unwrap();
    drop(b);
    std::fs::remove_dir_all(path)?;
    Ok(())
}

/// A sending address with its own outgoing server sends through it, signed
/// in with its own name, as itself; the mailbox's server isn't used.
#[test]
fn other_addresses_send_through_their_own_server() -> Result<()> {
    let draft = Draft {
        id: "as-desk".into(),
        to: "ana@example.com".into(),
        subject: "From the desk".into(),
        body: "Hello".into(),
        identity: "desk".into(),
        ..Draft::default()
    };
    let (result, seen) = scripted_smtp(
        "250-fixture\r\n250 AUTH PLAIN\r\n",
        1,
        |a| {
            a.identities = vec![crate::manage::model::Identity {
                id: "desk".into(),
                email: "desk@shop.example".into(),
                name: "Shop Desk".into(),
                reply_to: String::new(),
                smtp: Some(crate::manage::model::OutgoingServer {
                    host: "localhost".into(),
                    port: a.smtp_port,
                    username: "desk-login".into(),
                    tls: None,
                }),
            }];
            // Nothing listens here: the mailbox's server mustn't be used.
            a.smtp_port = 1;
        },
        draft,
    )?;
    result?;
    use base64::Engine as _;
    let auth = seen
        .commands
        .iter()
        .find(|c| c.starts_with("AUTH PLAIN "))
        .unwrap();
    let credentials = base64::engine::general_purpose::STANDARD
        .decode(auth.split(' ').nth(2).unwrap())
        .unwrap();
    assert_eq!(credentials, b"\0desk-login\0fixture-password");
    let mail = seen
        .commands
        .iter()
        .find(|c| c.starts_with("MAIL"))
        .unwrap();
    assert!(mail.starts_with("MAIL FROM:<desk@shop.example>"), "{mail}");
    let message = String::from_utf8_lossy(&seen.message);
    assert!(
        message.contains("desk@shop.example") && !message.contains("qa@example.com"),
        "{message}"
    );
    Ok(())
}

/// Sending addresses are kept with their account; the account's own
/// address, a duplicate or an invalid one isn't taken.
#[test]
fn sending_addresses_are_saved_and_removed() -> Result<()> {
    use crate::manage::model::Identity;
    let (b, a, path) = backend(&[])?;
    b.store.save_account(&a)?;
    let desk = Identity {
        id: "desk".into(),
        email: " desk@shop.example ".into(),
        name: "Shop Desk".into(),
        ..Identity::default()
    };
    let saved = b.save_identity(&a.id, desk.clone(), None)?;
    assert_eq!(saved.identities.len(), 1);
    assert_eq!(saved.identities[0].email, "desk@shop.example");
    let stored = b
        .store
        .accounts()?
        .into_iter()
        .find(|x| x.id == a.id)
        .unwrap();
    assert_eq!(stored.identities, saved.identities);
    // Changing it keeps one.
    let renamed = Identity {
        name: "Front Desk".into(),
        ..desk.clone()
    };
    assert_eq!(
        b.save_identity(&a.id, renamed, None)?.identities[0].name,
        "Front Desk"
    );
    for wrong in [
        Identity {
            id: "x".into(),
            email: "DESK@shop.example".into(),
            ..Identity::default()
        },
        Identity {
            id: "y".into(),
            email: a.email.clone(),
            ..Identity::default()
        },
        Identity {
            id: "z".into(),
            email: "not an address".into(),
            ..Identity::default()
        },
    ] {
        assert!(b.save_identity(&a.id, wrong, None).is_err());
    }
    assert!(b.remove_identity(&a.id, "desk")?.identities.is_empty());
    drop(b);
    std::fs::remove_dir_all(path)?;
    Ok(())
}

/// A server listing MOVE, UIDPLUS, ESEARCH, NAMESPACE, SPECIAL-USE and
/// LIST-STATUS: searches go by ESEARCH; the listing brings each folder's
/// counters, subscriptions and a shared namespace's folders; moved and
/// saved mail is placed under the UIDs the server gave it (COPYUID,
/// APPENDUID).
#[test]
fn listing_subscriptions_shared_folders_and_new_uids() -> Result<()> {
    listing_fixture(
        "* CAPABILITY IMAP4rev1 CONDSTORE MOVE UIDPLUS ESEARCH ENABLE NAMESPACE SPECIAL-USE LIST-STATUS\r\n",
    )
}

/// The same from a server speaking only IMAP4rev2, whose base protocol has
/// all of those without listing them.
#[test]
fn imap4rev2_only_servers_get_the_same() -> Result<()> {
    listing_fixture("* CAPABILITY IMAP4rev2 CONDSTORE\r\n")
}

fn listing_fixture(capabilities: &'static str) -> Result<()> {
    let (cert, acceptor) = tls_fixture()?;
    let listener = TcpListener::bind("127.0.0.1:0")?;
    let port = listener.local_addr()?.port();
    let server = std::thread::spawn(move || {
        let mut seen = Vec::new();
        let Some(mut p) = Peer::accept(&listener, &acceptor) else {
            return seen;
        };
        while let Some((tag, command, literal)) = p.command() {
            let upper = command.to_ascii_uppercase();
            seen.push(command.clone());
            if upper == "CAPABILITY" {
                p.send(capabilities);
            } else if upper == "NAMESPACE" {
                p.send("* NAMESPACE ((\"\" \"/\")) ((\"Other Users/\" \"/\")) ((\"Shared/\" \"/\"))\r\n");
            } else if upper.starts_with("LIST \"\" \"*\"") {
                assert_eq!(
                    command,
                    "LIST \"\" \"*\" RETURN (SPECIAL-USE SUBSCRIBED STATUS (UIDVALIDITY UIDNEXT MESSAGES HIGHESTMODSEQ))"
                );
                p.send("* LIST (\\Subscribed) \"/\" INBOX\r\n");
                p.send("* STATUS INBOX (UIDVALIDITY 77 UIDNEXT 9 MESSAGES 3 HIGHESTMODSEQ 40)\r\n");
                p.send("* LIST (\\Sent \\Subscribed) \"/\" Sent\r\n");
                p.send("* LIST (\\Subscribed) \"/\" Receipts\r\n");
                p.send("* STATUS Receipts (UIDVALIDITY 77 UIDNEXT 101 MESSAGES 4 HIGHESTMODSEQ 12)\r\n");
                p.send("* LIST () \"/\" Old\r\n");
            } else if upper.starts_with("LIST \"\" \"SHARED/*\"") {
                p.send("* LIST (\\Noselect) \"/\" Shared\r\n");
                p.send("* LIST (\\Noselect) \"/\" Shared/team\r\n");
                p.send("* LIST (\\Subscribed) \"/\" Shared/team/Projects\r\n");
            } else if upper.starts_with("LIST \"\" \"OTHER USERS/*\"") {
            } else if upper.starts_with("SELECT") {
                p.send("* 3 EXISTS\r\n* OK [UIDVALIDITY 77] stable\r\n* OK [HIGHESTMODSEQ 40] ok\r\n");
            } else if upper.starts_with("UID MOVE") {
                p.send("* OK [COPYUID 77 7 101] Moved\r\n* 1 EXPUNGE\r\n");
            } else if upper.starts_with("UID SEARCH") {
                assert!(upper.starts_with("UID SEARCH RETURN (ALL) "), "{command}");
                p.send(&format!("* ESEARCH (TAG \"{tag}\") UID\r\n"));
            } else if upper.starts_with("APPEND") {
                assert!(!literal.is_empty());
                p.send(&format!("{tag} OK [APPENDUID 77 55] Saved\r\n"));
                continue;
            } else if upper == "LOGOUT" {
                p.send(&format!("* BYE\r\n{tag} OK done\r\n"));
                break;
            } else if !upper.starts_with("LOGIN") && !upper.starts_with("NOOP") {
                panic!("Unexpected IMAP command: {command}");
            }
            p.ok(&tag);
        }
        seen
    });
    let (b, mut a, path) = backend(&cert)?;
    a.incoming_port = port;
    a.sync.subscribed_only = true;
    let folders = b.with_imap(&a, |c| Ok(custom_folders(c.names(true)?)))?;
    let shown: Vec<(&str, &str, usize)> = folders
        .iter()
        .map(|f| (f.name.as_str(), f.label.as_str(), f.depth))
        .collect();
    // "Old" isn't subscribed; the shared folder sits at the top, naming
    // the path to it, its parents being placeholders.
    assert_eq!(
        shown,
        [("Receipts", "Receipts", 0), ("Shared/team/Projects", "Shared/team/Projects", 0)]
    );
    // The listing's counters tell an unchanged folder from a changed one.
    b.store.save_mailbox_state(
        &a.id,
        "Receipts",
        Some(&crate::manage::store::MailboxState {
            validity: 77,
            uid_next: 101,
            exists: 4,
            modseq: Some(12),
            days: a.sync.days,
        }),
    )?;
    assert!(b.with_imap(&a, |c| Ok(b.unchanged(c, &a, "Receipts")))?);
    assert!(!b.with_imap(&a, |c| Ok(b.unchanged(c, &a, "INBOX")))?);
    // A moved message shows in its new folder under its new UID.
    let moved = Mail {
        id: format!("{}:INBOX:77:7", a.id),
        account: a.id.clone(),
        folder: Folder::Inbox,
        remote_folder: "INBOX".into(),
        uid: 7,
        subject: "Receipt".into(),
        ..Mail::default()
    };
    b.store.save_messages(std::slice::from_ref(&moved))?;
    b.change(&a, &moved, Change::MoveTo("Receipts".into()))?;
    assert!(b.store.message(&moved.id)?.is_none());
    let placed = b
        .store
        .message(&format!("{}:Receipts:77:101", a.id))?
        .expect("placed under its new UID");
    assert_eq!((placed.uid, placed.remote_folder.as_str()), (101, "Receipts"));
    assert_eq!(placed.subject, "Receipt");
    // A sent copy saved in Sent is there at once, searched for by ESEARCH.
    let raw = b"From: qa@example.com\r\nTo: r@example.com\r\nSubject: Sent copy\r\nMessage-ID: <sent@example.com>\r\nDate: Sat, 24 Oct 2026 10:00:00 +0000\r\n\r\nHello.\r\n";
    b.with_imap(&a, |c| b.file_sent_copy(c, &a, raw))?;
    let sent = b
        .store
        .message(&format!("{}:Sent:77:55", a.id))?
        .expect("saved under its new UID");
    assert_eq!(sent.folder, Folder::Sent);
    assert!(!sent.unread);
    b.disconnect(&a.id);
    let seen = server.join().unwrap();
    assert!(seen.iter().any(|c| c == "UID MOVE 7 \"Receipts\""), "{seen:?}");
    assert!(!seen.iter().any(|c| c.starts_with("ENABLE")), "{seen:?}");
    drop(b);
    std::fs::remove_dir_all(path)?;
    Ok(())
}

/// A server offering UIDONLY (RFC 9586) has it turned on where the IMAP
/// parser reads its UIDFETCH replies (Picacho-Patches' imap-proto patch
/// 0006), and not otherwise: sync, flag changes and deletion work either
/// way, never naming a message by sequence number.
#[test]
fn uidonly_is_enabled_only_where_uidfetch_is_read() -> Result<()> {
    uidonly_fixture(crate::sync::transport::reads_uidfetch())
}

/// With UIDONLY on, messages come back as UIDFETCH (numbered by UID,
/// without a UID item) and removals as VANISHED.
#[test]
fn uidonly_servers_report_messages_by_uid() -> Result<()> {
    assert!(crate::sync::transport::reads_uidfetch());
    uidonly_fixture(true)
}

fn uidonly_fixture(expect_uidonly: bool) -> Result<()> {
    let (cert, acceptor) = tls_fixture()?;
    let listener = TcpListener::bind("127.0.0.1:0")?;
    let port = listener.local_addr()?.port();
    let server = std::thread::spawn(move || {
        let mut seen = Vec::new();
        let mut uidonly = false;
        'connections: while let Some(mut p) = Peer::accept(&listener, &acceptor) {
            while let Some((tag, command, _)) = p.command() {
                let upper = command.to_ascii_uppercase();
                seen.push(command.clone());
                // UIDONLY refuses anything numbered by sequence (RFC 9586 §3).
                let numbered = ["FETCH ", "STORE ", "SEARCH ", "COPY ", "MOVE "]
                    .iter()
                    .any(|c| upper.starts_with(c));
                if uidonly && numbered {
                    p.send(&format!("{tag} BAD [UIDREQUIRED] Use UIDs\r\n"));
                    continue;
                }
                if upper.starts_with("LOGIN") {
                } else if upper == "CAPABILITY" {
                    p.send("* CAPABILITY IMAP4rev1 ENABLE UIDONLY UIDPLUS\r\n");
                } else if upper.starts_with("ENABLE ") {
                    if upper.split_ascii_whitespace().any(|w| w == "UIDONLY") {
                        uidonly = true;
                        p.send("* ENABLED UIDONLY\r\n");
                    }
                } else if upper.starts_with("LIST") {
                    p.send("* LIST () \"/\" INBOX\r\n");
                } else if upper.starts_with("SELECT") {
                    p.send(
                        "* 1 EXISTS\r\n* OK [UIDVALIDITY 42] stable\r\n* OK [UIDNEXT 8] next\r\n",
                    );
                } else if upper.starts_with("UID SEARCH") {
                    p.send("* SEARCH 7\r\n");
                } else if upper.starts_with("UID FETCH") {
                    let header = if upper.contains("HEADER") {
                        format!(
                            " RFC822.SIZE 100 BODY[HEADER] {{{}}}\r\n{HEADER}",
                            HEADER.len()
                        )
                    } else {
                        String::new()
                    };
                    p.send(&if uidonly {
                        format!("* 7 UIDFETCH (FLAGS (\\Seen){header})\r\n")
                    } else {
                        format!("* 1 FETCH (UID 7 FLAGS (\\Seen){header})\r\n")
                    });
                } else if upper.starts_with("UID STORE") {
                    // Reported back although SILENT, as another app's
                    // change could be.
                    p.send(if uidonly {
                        "* 7 UIDFETCH (FLAGS ())\r\n"
                    } else {
                        "* 1 FETCH (UID 7 FLAGS ())\r\n"
                    });
                } else if upper.starts_with("UID EXPUNGE") {
                    p.send(if uidonly {
                        "* VANISHED 7\r\n"
                    } else {
                        "* 1 EXPUNGE\r\n"
                    });
                } else if upper == "LOGOUT" {
                    p.send(&format!("* BYE\r\n{tag} OK done\r\n"));
                    break 'connections;
                } else if !upper.starts_with("NOOP") {
                    panic!("Unexpected IMAP command: {command}");
                }
                p.ok(&tag);
            }
        }
        (seen, uidonly)
    });
    let (b, mut a, path) = backend(&cert)?;
    a.incoming_port = port;
    let synced = b.sync(&a, Folder::Inbox, 300)?;
    assert_eq!(synced.len(), 1);
    let m = &synced[0];
    assert_eq!((m.uid, m.subject.as_str(), m.unread), (7, "Receipt", false));
    let m = b.change(&a, m, Change::Read(false))?;
    assert!(m.unread);
    b.delete_forever(&a, std::slice::from_ref(&m))?;
    assert!(b.store.message(&m.id)?.is_none());
    b.disconnect(&a.id);
    let (seen, uidonly) = server.join().unwrap();
    assert_eq!(uidonly, expect_uidonly, "{seen:?}");
    assert!(seen.iter().any(|c| c == "UID EXPUNGE 7"), "{seen:?}");
    drop(b);
    std::fs::remove_dir_all(path)?;
    Ok(())
}

/// A scheduled message handed to a server with FUTURERELEASE goes now,
/// held for its time (HOLDFOR); one the server can't hold that long stays
/// scheduled here, with nothing sent.
#[test]
fn scheduled_mail_is_handed_to_a_server_that_holds_it() -> Result<()> {
    let at = chrono::Utc::now().timestamp() + 3600;
    let draft = Draft {
        id: "later".into(),
        to: "ana@example.com".into(),
        subject: "Tomorrow".into(),
        body: "Hello.".into(),
        send_at: Some(at),
        ..Default::default()
    };
    let (handed, seen) = scripted_smtp_with(
        "250-fixture\r\n250-FUTURERELEASE 604800 2030-01-01T00:00:00Z\r\n250 AUTH PLAIN\r\n",
        1,
        |a| a.server_send_later = true,
        draft.clone(),
        |b, a, d| {
            let handed = b.hand_to_server(a, &d.id)?;
            // Sent as far as Picacho is concerned: no longer a draft.
            assert!(b.store.draft(&d.id)?.is_none());
            Ok(handed)
        },
    )?;
    assert!(handed?);
    let mail = seen.commands.iter().find(|c| c.starts_with("MAIL")).unwrap();
    let seconds: u64 = mail
        .split("HOLDFOR=")
        .nth(1)
        .and_then(|s| s.split(' ').next())
        .unwrap()
        .parse()?;
    assert!((3590..=3600).contains(&seconds), "{mail}");

    // Not that far ahead: left at once, the draft still scheduled.
    let (handed, seen) = scripted_smtp_with(
        "250-fixture\r\n250-FUTURERELEASE 600\r\n250 AUTH PLAIN\r\n",
        1,
        |a| a.server_send_later = true,
        draft,
        |b, a, d| {
            let handed = b.hand_to_server(a, &d.id)?;
            assert!(b.store.draft(&d.id)?.is_some());
            Ok(handed)
        },
    )?;
    assert!(!handed?);
    assert!(!seen.commands.iter().any(|c| c.starts_with("MAIL")), "{:?}", seen.commands);
    Ok(())
}

/// The server's answers, written and read whole: commands with any number
/// of literals (`{n}` at a line's end), each given a go-ahead.
fn command_with_literals(p: &mut Peer) -> Option<(String, String, Vec<Vec<u8>>)> {
    let first = p.line()?;
    let (tag, rest) = first.trim_end().split_once(' ')?;
    let tag = tag.to_owned();
    let mut command = String::new();
    let mut literals = Vec::new();
    let mut line = rest.to_owned();
    loop {
        let size = line
            .rfind('{')
            .filter(|_| line.ends_with('}'))
            .and_then(|open| Some((open, line[open + 1..line.len() - 1].parse::<usize>().ok()?)));
        let Some((open, n)) = size else {
            command.push_str(&line);
            return Some((tag, command, literals));
        };
        command.push_str(&line[..open]);
        command.push_str("{}");
        p.send("+ go ahead\r\n");
        literals.push(p.bytes(n));
        line = p.line()?.trim_end().to_owned();
    }
}

/// A server with SASL-IR, REPLACE, MULTIAPPEND, CREATE-SPECIAL-USE and
/// STATUS=SIZE: sign-in takes one round trip, a new draft version replaces
/// the old in one command, imported mail goes in one APPEND, a missing
/// Sent folder is made marked as Sent, and storage used is added up.
#[test]
fn newer_extensions_save_round_trips() -> Result<()> {
    use std::sync::{Arc, Mutex};
    let (cert, acceptor) = tls_fixture()?;
    let listener = TcpListener::bind("127.0.0.1:0")?;
    let port = listener.local_addr()?.port();
    let seen: Arc<Mutex<Vec<String>>> = Arc::default();
    let drafts: FixtureFolder = Arc::default();
    let (log, folder) = (seen.clone(), drafts.clone());
    let server = std::thread::spawn(move || {
        let mut next = 10;
        let mut sent_made = false;
        let mut imported = 0;
        'connections: while let Some(mut p) = Peer::accept(&listener, &acceptor) {
            while let Some((tag, command, literals)) = command_with_literals(&mut p) {
                let upper = command.to_ascii_uppercase();
                log.lock().unwrap().push(command.clone());
                let mut drafts = folder.lock().unwrap();
                if upper == "CAPABILITY" {
                    p.send("* CAPABILITY IMAP4rev1 SASL-IR AUTH=PLAIN LOGINDISABLED UIDPLUS REPLACE MULTIAPPEND CREATE-SPECIAL-USE STATUS=SIZE SPECIAL-USE\r\n");
                } else if let Some(initial) = command.strip_prefix("AUTHENTICATE PLAIN ") {
                    // The credentials came with the command: no go-ahead.
                    assert_eq!(
                        crate::sync::sasl::decode(initial).unwrap(),
                        b"\0qa@example.com\0fixture-password"
                    );
                } else if upper.starts_with("LIST") {
                    p.send("* LIST () \"/\" INBOX\r\n* LIST (\\Drafts) \"/\" Drafts\r\n* LIST () \"/\" Work\r\n");
                    if sent_made {
                        p.send("* LIST (\\Sent) \"/\" Sent\r\n");
                    }
                } else if upper == "CREATE \"SENT\" (USE (\\SENT))" {
                    sent_made = true;
                } else if upper.starts_with("SUBSCRIBE") {
                } else if upper.starts_with("STATUS") {
                    let size = if upper.contains("WORK") { 2048 } else { 1024 };
                    let name = command.split('"').nth(1).unwrap_or("INBOX").to_owned();
                    p.send(&format!("* STATUS \"{name}\" (SIZE {size})\r\n"));
                } else if upper.starts_with("SELECT") {
                    p.send(&format!(
                        "* {} EXISTS\r\n* OK [UIDVALIDITY 42] stable\r\n",
                        drafts.len()
                    ));
                } else if upper.starts_with("UID SEARCH HEADER X-PICACHO-DRAFT ") {
                    let uids: String = drafts.iter().map(|(u, _)| format!(" {u}")).collect();
                    p.send(&format!("* SEARCH{uids}\r\n"));
                } else if upper.starts_with("UID SEARCH") {
                    let uids: String = drafts.iter().map(|(u, _)| format!(" {u}")).collect();
                    p.send(&format!("* SEARCH{uids}\r\n"));
                } else if upper.starts_with("UID FETCH") {
                    let set = command.split(' ').nth(2).unwrap();
                    let wanted = super::protocol_tests::expand(set);
                    for (uid, raw) in drafts.iter().filter(|(u, _)| wanted.contains(u)) {
                        let text = String::from_utf8_lossy(raw);
                        let header: String = text
                            .lines()
                            .filter(|l| l.starts_with("X-Picacho-Draft"))
                            .map(|l| format!("{l}\r\n"))
                            .collect();
                        let data = format!("{header}\r\n");
                        p.send(&format!(
                            "* {uid} FETCH (UID {uid} RFC822.SIZE {} BODY[HEADER.FIELDS (X-PICACHO-DRAFT)] {{{}}}\r\n{data})\r\n",
                            raw.len(),
                            data.len()
                        ));
                    }
                } else if let Some(rest) = upper.strip_prefix("UID REPLACE ") {
                    let old: u32 = rest.split(' ').next().unwrap().parse().unwrap();
                    assert!(command.contains("\\Draft"), "{command}");
                    drafts.retain(|(u, _)| *u != old);
                    drafts.push((next, literals[0].clone()));
                    p.send(&format!("{tag} OK [APPENDUID 42 {next}] Replaced\r\n"));
                    next += 1;
                    continue;
                } else if upper.starts_with("APPEND \"DRAFTS\"") {
                    drafts.push((next, literals[0].clone()));
                    p.send(&format!("{tag} OK [APPENDUID 42 {next}] Saved\r\n"));
                    next += 1;
                    continue;
                } else if upper.starts_with("APPEND \"WORK\"") {
                    // Every message in the one command.
                    imported += literals.len();
                    let first = 100 + imported - literals.len();
                    p.send(&format!(
                        "{tag} OK [APPENDUID 9 {first}:{}] Saved\r\n",
                        100 + imported - 1
                    ));
                    continue;
                } else if upper == "LOGOUT" {
                    p.send(&format!("* BYE\r\n{tag} OK done\r\n"));
                    break 'connections;
                } else {
                    panic!("Unexpected IMAP command: {command}");
                }
                p.ok(&tag);
            }
        }
        imported
    });
    let (b, mut a, path) = backend(&cert)?;
    a.incoming_port = port;
    // Drafts: uploaded, then replaced in one step.
    let mut d = Draft {
        id: "replaced-draft".into(),
        account: a.id.clone(),
        to: "ana@example.com".into(),
        subject: "Plans".into(),
        body: "First".into(),
        ..Draft::default()
    };
    b.store.save_draft(&d)?;
    b.sync_drafts(&a)?;
    let first = b.store.draft(&d.id)?.unwrap().remote.unwrap().uid;
    d = b.store.draft(&d.id)?.unwrap();
    d.body = "Second".into();
    b.store.save_draft(&d)?;
    b.sync_drafts(&a)?;
    let second = b.store.draft(&d.id)?.unwrap().remote.unwrap().uid;
    assert_ne!(first, second);
    {
        let drafts = drafts.lock().unwrap();
        assert_eq!(drafts.len(), 1, "replaced, not added");
        assert!(String::from_utf8_lossy(&drafts[0].1).contains("Second"));
    }
    // Import: three messages, one APPEND, listed at once.
    let dir = std::env::temp_dir().join(format!("picacho-import-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&dir)?;
    let mbox = dir.join("mail.mbox");
    let one = |n: u32| {
        format!("From qa@example.com Sat Nov 21 10:00:00 2026\nFrom: Ana <ana@example.com>\nSubject: Note {n}\nMessage-ID: <n{n}@example.com>\n\nBody {n}\n\n")
    };
    std::fs::write(&mbox, format!("{}{}{}", one(1), one(2), one(3)))?;
    let moved = b.import_messages(
        &a,
        &Mailbox::Remote("Work".into()),
        &[mbox],
        &std::sync::atomic::AtomicBool::new(false),
    )?;
    assert_eq!((moved.done, moved.skipped), (3, 0));
    // No QUOTA: the space every mailbox takes, without a limit, where the
    // IMAP parser reads STATUS=SIZE (Picacho-Patches imap-proto 0007).
    // Before it, nothing is shown, and nothing else fails.
    match b.quota(&a)? {
        Some(q) => assert_eq!((q.used_kib, q.limit_kib), (4, 0), "1 + 1 + 2 KiB"),
        None => assert!(!crate::sync::transport::reads_status_size()),
    }
    // No Sent folder: one is made, marked for its use.
    let made = b.with_imap(&a, |c| c.standard_folder(a.provider, Folder::Sent))?;
    assert_eq!(made, "Sent");
    b.disconnect(&a.id);
    let imported = server.join().unwrap();
    assert_eq!(imported, 3);
    let seen = seen.lock().unwrap();
    assert_eq!(seen.iter().filter(|c| c.starts_with("APPEND \"Work\"")).count(), 1, "{seen:?}");
    assert!(seen.iter().any(|c| c.starts_with("UID REPLACE")), "{seen:?}");
    assert!(!seen.iter().any(|c| c.starts_with("UID EXPUNGE")), "nothing to remove: {seen:?}");
    assert!(!seen.iter().any(|c| c == "AUTHENTICATE PLAIN"), "SASL-IR: {seen:?}");
    drop(b);
    std::fs::remove_dir_all(path)?;
    std::fs::remove_dir_all(dir)?;
    Ok(())
}


/// A high-priority message goes with MT-PRIORITY (RFC 6710) and the
/// headers recipients' apps read; a delivery time limit with DELIVERBY
/// (RFC 2852), at least the server's minimum.
#[test]
fn smtp_priority_and_delivery_time_limit() -> Result<()> {
    let draft = Draft {
        id: "urgent".into(),
        to: "ana@example.com".into(),
        subject: "Urgent".into(),
        body: "Now".into(),
        priority: crate::manage::model::Priority::High,
        ..Draft::default()
    };
    let (result, commands, message) = smtp_exchange(
        "250-fixture\r\n250-MT-PRIORITY MIXER\r\n250-DELIVERBY 7200\r\n250 AUTH PLAIN\r\n",
        |a| a.deliver_within_hours = 1,
        draft.clone(),
    )?;
    assert!(result?.is_empty());
    let mail = commands.iter().find(|c| c.starts_with("MAIL FROM")).unwrap();
    assert!(mail.contains(" MT-PRIORITY=4"), "{mail}");
    // One hour asked; the server takes no less than two.
    assert!(mail.contains(" BY=7200;R"), "{mail}");
    assert!(message.contains("Importance: high") && message.contains("X-Priority: 1"), "{message}");
    // A server without them is told neither; the headers still go.
    let (result, commands, message) =
        smtp_exchange("250-fixture\r\n250 AUTH PLAIN\r\n", |a| a.deliver_within_hours = 1, draft)?;
    assert!(result?.is_empty());
    let mail = commands.iter().find(|c| c.starts_with("MAIL FROM")).unwrap();
    assert!(!mail.contains("MT-PRIORITY") && !mail.contains("BY="), "{mail}");
    assert!(message.contains("Importance: high"), "{message}");
    Ok(())
}

/// A server offering IMAP4rev2 beside rev1 is switched to rev2 (ENABLE),
/// and an All Mail search goes as one MULTISEARCH with WITHIN's YOUNGER
/// and FUZZY words, its results named by each mailbox's UIDVALIDITY.
#[test]
fn rev2_multisearch_within_and_fuzzy() -> Result<()> {
    let (cert, acceptor) = tls_fixture()?;
    let listener = TcpListener::bind("127.0.0.1:0")?;
    let port = listener.local_addr()?.port();
    let server = std::thread::spawn(move || {
        let mut seen = Vec::new();
        'connections: while let Some(mut p) = Peer::accept(&listener, &acceptor) {
            while let Some((tag, command, _)) = p.command() {
                let upper = command.to_ascii_uppercase();
                seen.push(command.clone());
                if upper.starts_with("LOGIN") {
                } else if upper == "CAPABILITY" {
                    p.send("* CAPABILITY IMAP4rev1 IMAP4rev2 ENABLE SPECIAL-USE ESEARCH MULTISEARCH WITHIN SEARCH=FUZZY\r\n");
                } else if upper.starts_with("ENABLE") {
                    p.send("* ENABLED IMAP4rev2\r\n");
                } else if upper.starts_with("LIST") {
                    p.send("* LIST () \"/\" INBOX\r\n");
                    p.send("* LIST (\\Archive) \"/\" \"Archive\"\r\n");
                } else if upper == "NAMESPACE" {
                    // rev2 has it: asked once ENABLEd.
                    p.send("* NAMESPACE ((\"\" \"/\")) NIL NIL\r\n");
                } else if upper.starts_with("ESEARCH IN") {
                    p.send("* ESEARCH (TAG \"x\" MAILBOX \"INBOX\" UIDVALIDITY 7) UID COUNT 2 ALL 3:4\r\n");
                    p.send("* ESEARCH (TAG \"x\" MAILBOX \"Archive\" UIDVALIDITY 9) UID COUNT 1 ALL 5\r\n");
                } else if upper == "LOGOUT" {
                    p.send(&format!("* BYE\r\n{tag} OK done\r\n"));
                    break 'connections;
                } else {
                    panic!("Unexpected IMAP command: {command}");
                }
                p.ok(&tag);
            }
        }
        seen
    });
    let (b, mut a, path) = backend(&cert)?;
    a.incoming_port = port;
    // What the search finds is already cached, so nothing is fetched.
    for (remote, validity, uid) in [("INBOX", 7, 3), ("INBOX", 7, 4), ("Archive", 9, 5)] {
        let id = format!("{}:{remote}:{validity}:{uid}", a.id);
        let mut m = parse_message(b"Subject: Report\r\n\r\nBody", &a, &id, Folder::Inbox, remote, uid)?;
        m.date = 1_700_000_000 + i64::from(uid);
        b.store.save_messages(&[m])?;
    }
    let found = b.search_server(&a, &Folder::All.into(), "newer:2d report", 10)?;
    assert_eq!(found.total, 3);
    assert_eq!(found.mails.len(), 3);
    b.disconnect(&a.id);
    let seen = server.join().unwrap();
    let enable = seen.iter().find(|c| c.starts_with("ENABLE")).unwrap();
    assert!(enable.contains("IMAP4rev2"), "{enable}");
    let search = seen.iter().find(|c| c.starts_with("ESEARCH IN")).unwrap();
    assert!(search.contains("(mailboxes (\"INBOX\" \"Archive\"))"), "{search}");
    assert!(search.contains("YOUNGER 172800") && search.contains("FUZZY (TEXT \"report\")"), "{search}");
    assert!(!seen.iter().any(|c| c.starts_with("SELECT") || c.starts_with("EXAMINE")), "{seen:?}");
    drop(b);
    std::fs::remove_dir_all(path)?;
    Ok(())
}
