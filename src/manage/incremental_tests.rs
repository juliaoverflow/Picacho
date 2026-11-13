//! Incremental synchronization against a scripted CONDSTORE + Gmail server:
//! later checks ask only for what changed, and Gmail mail downloads once.
use super::*;
use protocol_tests::{backend, expand, tls_fixture};
use std::io::{BufRead, BufReader, Write};
use std::sync::Mutex as StdMutex;
use std::time::Duration;

#[derive(Clone)]
struct Message {
    uid: u32,
    seen: bool,
    gmail_id: u64,
    modseq: u64,
}
struct ServerBox {
    messages: Vec<Message>,
    uid_next: u32,
    modseq: u64,
}
#[derive(Default)]
struct World {
    boxes: HashMap<String, ServerBox>,
    /// Commands received, uppercased, after sign-in.
    log: Vec<String>,
    logins: usize,
    /// Reply NO [THROTTLED] to every FETCH.
    throttle: bool,
}
fn header(uid: u32) -> String {
    format!(
        "From: Sender {uid} <s{uid}@example.com>\r\nSubject: Message {uid}\r\n\
         Date: Wed, {uid} Nov 2026 10:00:00 +0000\r\n\r\n"
    )
}
fn raw(uid: u32) -> String {
    format!("{}Body of message {uid}\r\n", header(uid))
}

/// Serves one connection at a time until `stop`.
fn serve(
    world: Arc<StdMutex<World>>,
    stop: Arc<AtomicBool>,
) -> Result<(Vec<u8>, u16, std::thread::JoinHandle<()>)> {
    let (cert, acceptor) = tls_fixture()?;
    let listener = std::net::TcpListener::bind("127.0.0.1:0")?;
    listener.set_nonblocking(true)?;
    let port = listener.local_addr()?.port();
    let thread = std::thread::spawn(move || {
        while !stop.load(Ordering::Relaxed) {
            let socket = match listener.accept() {
                Ok((socket, _)) => socket,
                Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                    std::thread::sleep(Duration::from_millis(5));
                    continue;
                }
                Err(e) => panic!("{e}"),
            };
            socket.set_nonblocking(false).unwrap();
            socket
                .set_read_timeout(Some(Duration::from_secs(3)))
                .unwrap();
            let Ok(stream) = acceptor.accept(socket) else {
                continue;
            };
            let mut r = BufReader::new(stream);
            r.get_mut().write_all(b"* OK fixture\r\n").unwrap();
            let mut selected = String::new();
            loop {
                let mut line = String::new();
                if r.read_line(&mut line).unwrap_or(0) == 0 {
                    break;
                }
                let (tag, command) = line.trim_end().split_once(' ').unwrap();
                let upper = command.to_uppercase();
                let mut w = world.lock().unwrap();
                let mut status = "OK";
                let payload = if upper.starts_with("LOGIN ") {
                    w.logins += 1;
                    String::new()
                } else if upper == "CAPABILITY" {
                    "* CAPABILITY IMAP4rev1 MOVE CONDSTORE X-GM-EXT-1\r\n".into()
                } else if upper.starts_with("LIST ") {
                    "* LIST (\\HasNoChildren) \"/\" \"INBOX\"\r\n\
                     * LIST (\\All \\HasNoChildren) \"/\" \"[Gmail]/All Mail\"\r\n"
                        .into()
                } else if upper == "LOGOUT" {
                    let _ = r
                        .get_mut()
                        .write_all(format!("* BYE bye\r\n{tag} OK done\r\n").as_bytes());
                    break;
                } else if upper == "NOOP" {
                    String::new()
                } else {
                    w.log.push(upper.clone());
                    if upper.starts_with("SELECT ") {
                        selected = command[7..]
                            .split(" (")
                            .next()
                            .unwrap()
                            .trim_matches('"')
                            .to_owned();
                        let b = &w.boxes[&selected];
                        format!(
                            "* {} EXISTS\r\n* OK [UIDVALIDITY 5] ok\r\n* OK [UIDNEXT {}] ok\r\n\
                             * OK [HIGHESTMODSEQ {}] ok\r\n",
                            b.messages.len(),
                            b.uid_next,
                            b.modseq
                        )
                    } else if upper.starts_with("UID SEARCH ") {
                        let b = &w.boxes[&selected];
                        let uids: Vec<String> =
                            b.messages.iter().map(|m| m.uid.to_string()).collect();
                        format!("* SEARCH {}\r\n", uids.join(" "))
                    } else if upper.starts_with("UID FETCH ") && w.throttle {
                        status = "NO [THROTTLED] slow down,";
                        String::new()
                    } else if upper.starts_with("UID FETCH ") {
                        let set = command.split_whitespace().nth(2).unwrap().to_owned();
                        let b = &w.boxes[&selected];
                        let max = b.messages.iter().map(|m| m.uid).max().unwrap_or(0);
                        let wanted: Vec<u32> = if let Some(from) = set.strip_suffix(":*") {
                            let from: u32 = from.parse().unwrap();
                            // "n:*" always includes the newest message.
                            b.messages
                                .iter()
                                .map(|m| m.uid)
                                .filter(|u| *u >= from || *u == max)
                                .collect()
                        } else {
                            expand(&set)
                        };
                        let since = upper
                            .split("CHANGEDSINCE ")
                            .nth(1)
                            .map(|s| s.trim_end_matches(')').parse::<u64>().unwrap());
                        let mut out = String::new();
                        for (seq, m) in b.messages.iter().enumerate() {
                            if !wanted.contains(&m.uid) || since.is_some_and(|s| m.modseq <= s) {
                                continue;
                            }
                            let flags = if m.seen { "\\Seen" } else { "" };
                            let mut items = format!("UID {} FLAGS ({flags})", m.uid);
                            if upper.contains("X-GM-MSGID") {
                                items += &format!(" X-GM-MSGID {}", m.gmail_id);
                            }
                            if upper.contains("RFC822.SIZE") {
                                items += &format!(
                                    " RFC822.SIZE {} INTERNALDATE \"05-Nov-2026 10:00:00 +0000\"",
                                    raw(m.uid).len()
                                );
                            }
                            if upper.contains("HEADER.FIELDS") {
                                let h = header(m.uid);
                                items += &format!(
                                    " BODY[HEADER.FIELDS (FROM TO SUBJECT DATE MESSAGE-ID CONTENT-TYPE LIST-UNSUBSCRIBE LIST-UNSUBSCRIBE-POST)] {{{}}}\r\n{h}",
                                    h.len()
                                );
                            } else if upper.contains("BODY.PEEK[]") {
                                let body = raw(m.uid);
                                items += &format!(" BODY[] {{{}}}\r\n{body}", body.len());
                            }
                            if since.is_some() {
                                items += &format!(" MODSEQ ({})", m.modseq);
                            }
                            out += &format!("* {} FETCH ({items})\r\n", seq + 1);
                        }
                        out
                    } else {
                        panic!("Unexpected IMAP command: {command}")
                    }
                };
                drop(w);
                if r.get_mut()
                    .write_all(format!("{payload}{tag} {status} done\r\n").as_bytes())
                    .is_err()
                {
                    break;
                }
            }
        }
    });
    Ok((cert, port, thread))
}
fn mailbox(uids: &[u32], gmail_base: u64) -> ServerBox {
    ServerBox {
        messages: uids
            .iter()
            .map(|uid| Message {
                uid: *uid,
                seen: false,
                gmail_id: gmail_base + u64::from(*uid),
                modseq: 10,
            })
            .collect(),
        uid_next: uids.iter().max().map_or(1, |u| u + 1),
        modseq: 10,
    }
}
fn commands(world: &StdMutex<World>) -> Vec<String> {
    std::mem::take(&mut world.lock().unwrap().log)
}

#[test]
fn later_checks_ask_only_for_what_changed() -> Result<()> {
    let world = Arc::new(StdMutex::new(World::default()));
    world
        .lock()
        .unwrap()
        .boxes
        .insert("INBOX".into(), mailbox(&[1, 2, 3], 1000));
    let stop = Arc::new(AtomicBool::new(false));
    let (cert, port, server) = serve(world.clone(), stop.clone())?;
    let (b, mut a, path) = backend(&cert)?;
    a.provider = Provider::Gmail;
    a.incoming_port = port;
    let result = (|| -> Result<()> {
        // First check: one search, and lean headers with Gmail ids.
        assert_eq!(b.sync(&a, Folder::Inbox, 300)?.len(), 3);
        let first = commands(&world);
        assert_eq!(
            first.iter().filter(|c| c.starts_with("UID SEARCH")).count(),
            1
        );
        assert!(
            first
                .iter()
                .any(|c| c.contains("HEADER.FIELDS") && c.contains("X-GM-MSGID"))
        );
        assert!(!first.iter().any(|c| c.contains("BODY.PEEK[HEADER]")));
        let cached = b.store.messages(&a.id, Folder::Inbox, 300)?;
        assert!(cached.iter().all(|m| m.gmail_id > 1000));

        // Nothing changed: SELECT is all it takes.
        assert_eq!(b.sync(&a, Folder::Inbox, 300)?.len(), 3);
        assert_eq!(commands(&world), ["SELECT \"INBOX\" (CONDSTORE)"]);

        // New mail and a flag change: only those, no search.
        {
            let mut w = world.lock().unwrap();
            let inbox = w.boxes.get_mut("INBOX").unwrap();
            inbox.messages.push(Message {
                uid: 4,
                seen: false,
                gmail_id: 1004,
                modseq: 12,
            });
            inbox.messages[1].seen = true;
            inbox.messages[1].modseq = 12;
            inbox.uid_next = 5;
            inbox.modseq = 12;
        }
        let synced = b.sync(&a, Folder::Inbox, 300)?;
        assert_eq!(synced.len(), 4);
        let log = commands(&world);
        assert!(!log.iter().any(|c| c.starts_with("UID SEARCH")), "{log:?}");
        assert!(log.iter().any(|c| c.starts_with("UID FETCH 4:*")));
        assert!(log.iter().any(|c| c.contains("(CHANGEDSINCE 10)")));
        let second = synced.iter().find(|m| m.uid == 2).unwrap();
        assert!(!second.unread, "flag change since the last check applies");
        assert!(synced.iter().find(|m| m.uid == 1).unwrap().unread);

        // A removal changes the count: one search reconciles it.
        world
            .lock()
            .unwrap()
            .boxes
            .get_mut("INBOX")
            .unwrap()
            .messages
            .remove(0);
        let synced = b.sync(&a, Folder::Inbox, 300)?;
        assert_eq!(synced.len(), 3);
        assert!(synced.iter().all(|m| m.uid != 1));
        assert_eq!(
            commands(&world)
                .iter()
                .filter(|c| c.starts_with("UID SEARCH"))
                .count(),
            1
        );

        // The layout drawn before headers load matches the server.
        let layout = b.layout(&a.id, &Folder::Inbox.into()).unwrap();
        assert_eq!(*layout.uids, [2, 3, 4]);
        // Everything above happened on one signed-in connection.
        assert_eq!(world.lock().unwrap().logins, 1);
        Ok(())
    })();
    b.disconnect(&a.id);
    stop.store(true, Ordering::Relaxed);
    server.join().unwrap();
    drop(b);
    std::fs::remove_dir_all(path)?;
    result
}

#[test]
fn placeholders_fill_on_demand_and_gmail_mail_downloads_once() -> Result<()> {
    let world = Arc::new(StdMutex::new(World::default()));
    {
        let mut w = world.lock().unwrap();
        // The same three Gmail messages, under Inbox and All Mail.
        w.boxes.insert("INBOX".into(), mailbox(&[1, 2, 3], 1000));
        w.boxes.insert(
            "[Gmail]/All Mail".into(),
            mailbox(&[1, 2, 3, 4, 5, 6], 1000),
        );
    }
    let stop = Arc::new(AtomicBool::new(false));
    let (cert, port, server) = serve(world.clone(), stop.clone())?;
    let (b, mut a, path) = backend(&cert)?;
    a.provider = Provider::Gmail;
    a.incoming_port = port;
    a.sync.offline_days = 0;
    let result = (|| -> Result<()> {
        // A sync lists the newest; the rest are placeholders in arrival order.
        let archive: Mailbox = Folder::Archive.into();
        b.sync(&a, archive.clone(), 2)?;
        let (rows, layout) = b.listed(&a.id, &archive)?;
        let layout = layout.unwrap();
        assert_eq!(
            rows.iter().map(|m| m.uid).collect::<Vec<_>>(),
            [6, 5, 4, 3, 2, 1]
        );
        assert_eq!(rows.iter().filter(|m| m.pending).count(), 4);
        assert!(rows.iter().all(|m| m.pending == (m.uid <= 4)));

        // Scrolling to the oldest rows lists exactly those.
        commands(&world);
        let filled = b.fill(&a, &archive, &layout, &[1, 2])?;
        assert_eq!(filled.len(), 2);
        let log = commands(&world);
        assert_eq!(log.len(), 1, "already selected: {log:?}");
        assert!(log[0].starts_with("UID FETCH 1:2 "));
        let (rows, _) = b.listed(&a.id, &archive)?;
        assert_eq!(rows.iter().filter(|m| m.pending).count(), 2);

        // Bodies: each Gmail message downloads once, whichever label it is under.
        b.sync(&a, Folder::Inbox, 300)?;
        commands(&world);
        let mut downloaded = 0;
        loop {
            let n = b.download_batch(&a, 25)?;
            if n == 0 {
                break;
            }
            downloaded += n;
        }
        let fetched: usize = commands(&world)
            .iter()
            .filter(|c| c.contains("BODY.PEEK[]"))
            .map(|c| expand(c.split_whitespace().nth(2).unwrap()).len())
            .sum();
        // Inbox 1-3 and All Mail 1, 2, 5, 6 are listed: 7 rows, 5 messages.
        assert_eq!(downloaded, 7);
        assert_eq!(
            fetched, 5,
            "copies under another label are not downloaded again"
        );
        Ok(())
    })();
    b.disconnect(&a.id);
    stop.store(true, Ordering::Relaxed);
    server.join().unwrap();
    drop(b);
    std::fs::remove_dir_all(path)?;
    result
}

#[test]
fn server_limits_pause_background_work() -> Result<()> {
    let world = Arc::new(StdMutex::new(World::default()));
    world
        .lock()
        .unwrap()
        .boxes
        .insert("INBOX".into(), mailbox(&[1, 2], 1000));
    let stop = Arc::new(AtomicBool::new(false));
    let (cert, port, server) = serve(world.clone(), stop.clone())?;
    let (b, mut a, path) = backend(&cert)?;
    a.provider = Provider::Gmail;
    a.incoming_port = port;
    a.sync.offline_days = 0;
    let result = (|| -> Result<()> {
        b.sync(&a, Folder::Inbox, 300)?;
        world.lock().unwrap().throttle = true;
        let error = b.download_batch(&a, 25).unwrap_err();
        let limit = *error
            .downcast_ref::<crate::manage::status::Throttled>()
            .expect("typed as a server limit");
        assert!(!limit.budget);
        assert!(b.throttled_until(&a.id).is_some());
        // While paused, background work does not contact the server.
        commands(&world);
        assert!(b.download_batch(&a, 25).is_err());
        assert!(b.catalog(&a, Folder::Inbox, 10).is_err());
        assert!(commands(&world).is_empty());
        assert!(
            crate::manage::status::AccountIssue::new(&error)
                .guidance(&crate::manage::status::IssueSource::Download)
                .contains("limiting")
        );
        Ok(())
    })();
    b.disconnect(&a.id);
    stop.store(true, Ordering::Relaxed);
    server.join().unwrap();
    drop(b);
    std::fs::remove_dir_all(path)?;
    result
}

#[test]
fn a_relaunch_lists_a_mailbox_in_the_same_order_as_after_sync() -> Result<()> {
    let world = Arc::new(StdMutex::new(World::default()));
    world
        .lock()
        .unwrap()
        .boxes
        .insert("INBOX".into(), mailbox(&[1, 2, 3], 1000));
    let stop = Arc::new(AtomicBool::new(false));
    let (cert, port, server) = serve(world.clone(), stop.clone())?;
    let (b, mut a, path) = backend(&cert)?;
    a.incoming_port = port;
    let result = (|| -> Result<()> {
        let inbox: Mailbox = Folder::Inbox.into();
        b.sync(&a, inbox.clone(), 300)?;
        // The newest arrival carries the oldest date (moved-in mail).
        let mut moved = b
            .store
            .summaries(&a.id, inbox.clone())?
            .into_iter()
            .find(|m| m.uid == 3)
            .unwrap();
        moved.date = 1;
        b.store.save_messages(&[moved])?;
        let uids = |rows: Vec<Mail>| rows.iter().map(|m| m.uid).collect::<Vec<_>>();
        let synced = uids(b.listed(&a.id, &inbox)?.0);
        assert_eq!(synced, [3, 2, 1]);
        // A new launch has no layout in memory until its first sync.
        let relaunched = Backend::new(b.store.clone());
        let (rows, layout) = relaunched.listed(&a.id, &inbox)?;
        assert!(layout.is_none());
        assert_eq!(uids(rows), synced);
        Ok(())
    })();
    b.disconnect(&a.id);
    stop.store(true, Ordering::Relaxed);
    server.join().unwrap();
    drop(b);
    std::fs::remove_dir_all(path)?;
    result
}
