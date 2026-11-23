//! Kerberos sign-in end to end: a throwaway realm (MIT krb5kdc), and
//! loopback IMAP, POP3 and SMTP servers that accept the ticket with the
//! service's keytab through the real GSS-API. Opt-in, since it needs MIT
//! Kerberos' server tools: `cargo test -- --ignored kerberos`.
use super::*;
use crate::sync::gss::imp::acceptor::Acceptor;
use base64::{Engine as _, engine::general_purpose::STANDARD};
use protocol_tests::{backend, tls_fixture};
use std::io::{BufRead, BufReader, Write};

/// One server-side GSSAPI exchange (RFC 4752) over `read`/`write` lines
/// in the protocol's framing: the context, then the security-layer offer
/// ("no layer", 64 KiB) and the client's answer, whose user it returns.
fn accept(
    first: Option<Vec<u8>>,
    mut read: impl FnMut() -> Vec<u8>,
    mut write: impl FnMut(&[u8]),
) -> String {
    let mut server = Acceptor::new().unwrap();
    // Without an initial response, an empty challenge asks for the first token.
    let mut token = match first {
        Some(first) => first,
        None => {
            write(&[]);
            read()
        }
    };
    loop {
        let (out, done) = server.step(&token).unwrap();
        if done {
            // The last context token, if any, then the offer (one challenge
            // each; the client answers the first with nothing).
            if !out.is_empty() {
                write(&out);
                assert!(read().is_empty());
            }
            break;
        }
        write(&out);
        token = read();
    }
    write(&server.wrap(&[0x01, 0x00, 0x10, 0x00]).unwrap());
    let answer = server.unwrap(&read()).unwrap();
    assert_eq!(answer[0], 0x01, "no security layer chosen");
    String::from_utf8(answer[4..].to_vec()).unwrap()
}

#[test]
#[ignore = "starts a Kerberos KDC; needs MIT krb5's server tools"]
fn kerberos_signs_in_to_imap_pop3_and_smtp() -> Result<()> {
    let _realm = crate::sync::gss::tests::realm()?;
    let (cert, acceptor) = tls_fixture()?;
    let acceptor = Arc::new(acceptor);
    let signed_in: Arc<Mutex<Vec<String>>> = Arc::default();

    // IMAP: AUTHENTICATE GSSAPI with SASL-IR, `+ ` challenges.
    let imap = std::net::TcpListener::bind("127.0.0.1:0")?;
    let imap_port = imap.local_addr()?.port();
    let (tls, seen) = (acceptor.clone(), signed_in.clone());
    std::thread::spawn(move || {
        let socket = imap.accept().map(|(s, _)| s);
        let stream = tls.accept(socket.unwrap()).unwrap();
        stream
            .get_ref()
            .set_read_timeout(Some(std::time::Duration::from_secs(10)))
            .unwrap();
        let mut r = BufReader::new(stream);
        r.get_mut().write_all(b"* OK ready\r\n").unwrap();
        let mut line = String::new();
        loop {
            line.clear();
            if r.read_line(&mut line).unwrap_or(0) == 0 {
                return;
            }
            let (tag, command) = line.trim_end().split_once(' ').unwrap();
            let (tag, command) = (tag.to_owned(), command.to_owned());
            let upper = command.to_ascii_uppercase();
            if upper == "CAPABILITY" {
                let reply = format!(
                    "* CAPABILITY IMAP4rev1 SASL-IR AUTH=GSSAPI AUTH=PLAIN\r\n{tag} OK done\r\n"
                );
                r.get_mut().write_all(reply.as_bytes()).unwrap();
            } else if let Some(rest) = upper.strip_prefix("AUTHENTICATE GSSAPI") {
                assert!(!upper.contains("PLAIN"));
                let first = command[command.len() - rest.len()..].trim();
                let first = (!first.is_empty()).then(|| STANDARD.decode(first).unwrap());
                let reader = std::cell::RefCell::new(r);
                let user = accept(
                    first,
                    || {
                        let mut l = String::new();
                        reader.borrow_mut().read_line(&mut l).unwrap();
                        STANDARD.decode(l.trim()).unwrap_or_default()
                    },
                    |t| {
                        let line = format!("+ {}\r\n", STANDARD.encode(t));
                        reader
                            .borrow_mut()
                            .get_mut()
                            .write_all(line.as_bytes())
                            .unwrap();
                    },
                );
                r = reader.into_inner();
                seen.lock().unwrap().push(format!("imap:{user}"));
                r.get_mut()
                    .write_all(format!("{tag} OK [CAPABILITY IMAP4rev1] signed in\r\n").as_bytes())
                    .unwrap();
            } else if upper == "LOGOUT" {
                r.get_mut()
                    .write_all(format!("* BYE\r\n{tag} OK bye\r\n").as_bytes())
                    .unwrap();
                return;
            } else {
                r.get_mut()
                    .write_all(format!("{tag} OK done\r\n").as_bytes())
                    .unwrap();
            }
        }
    });

    // POP3: AUTH GSSAPI (RFC 5034), `+ ` challenges; no USER offered.
    let pop = std::net::TcpListener::bind("127.0.0.1:0")?;
    let pop_port = pop.local_addr()?.port();
    let (tls, seen) = (acceptor.clone(), signed_in.clone());
    std::thread::spawn(move || {
        let socket = pop.accept().map(|(s, _)| s);
        let socket = socket.unwrap();
        socket
            .set_read_timeout(Some(std::time::Duration::from_secs(10)))
            .unwrap();
        let stream = tls.accept(socket).unwrap();
        let mut r = BufReader::new(stream);
        r.get_mut().write_all(b"+OK ready\r\n").unwrap();
        let mut line = String::new();
        loop {
            line.clear();
            if r.read_line(&mut line).unwrap() == 0 {
                return;
            }
            let command = line.trim_end().to_owned();
            if command == "CAPA" {
                r.get_mut()
                    .write_all(b"+OK\r\nSASL GSSAPI PLAIN\r\nUIDL\r\n.\r\n")
                    .unwrap();
            } else if let Some(rest) = command.strip_prefix("AUTH GSSAPI") {
                let first = rest.trim();
                let first = (!first.is_empty()).then(|| STANDARD.decode(first).unwrap());
                let reader = std::cell::RefCell::new(r);
                let user = accept(
                    first,
                    || {
                        let mut l = String::new();
                        reader.borrow_mut().read_line(&mut l).unwrap();
                        STANDARD.decode(l.trim()).unwrap_or_default()
                    },
                    |t| {
                        let line = format!("+ {}\r\n", STANDARD.encode(t));
                        reader
                            .borrow_mut()
                            .get_mut()
                            .write_all(line.as_bytes())
                            .unwrap();
                    },
                );
                r = reader.into_inner();
                seen.lock().unwrap().push(format!("pop:{user}"));
                r.get_mut().write_all(b"+OK signed in\r\n").unwrap();
            } else if command == "QUIT" {
                r.get_mut().write_all(b"+OK bye\r\n").unwrap();
                return;
            } else {
                panic!("unexpected POP3 command {command}");
            }
        }
    });

    // SMTP on port 465 (TLS from the start): AUTH GSSAPI with its first
    // token, `334` challenges, `235` once signed in, then one message.
    let smtp = std::net::TcpListener::bind("127.0.0.1:0")?;
    let smtp_port = smtp.local_addr()?.port();
    let (tls, seen) = (acceptor.clone(), signed_in.clone());
    std::thread::spawn(move || {
        let socket = smtp.accept().map(|(s, _)| s);
        let socket = socket.unwrap();
        socket
            .set_read_timeout(Some(std::time::Duration::from_secs(10)))
            .unwrap();
        let stream = tls.accept(socket).unwrap();
        let mut r = BufReader::new(stream);
        r.get_mut().write_all(b"220 ready\r\n").unwrap();
        let mut line = String::new();
        loop {
            line.clear();
            if r.read_line(&mut line).unwrap_or(0) == 0 {
                return;
            }
            let command = line.trim_end().to_owned();
            let upper = command.to_ascii_uppercase();
            if upper.starts_with("EHLO") {
                r.get_mut()
                    .write_all(b"250-mail.example\r\n250 AUTH GSSAPI PLAIN\r\n")
                    .unwrap();
            } else if let Some(rest) = upper.strip_prefix("AUTH GSSAPI") {
                assert!(!upper.contains("PLAIN"));
                let first = command[command.len() - rest.len()..].trim();
                let first =
                    (!first.is_empty() && first != "=").then(|| STANDARD.decode(first).unwrap());
                let reader = std::cell::RefCell::new(r);
                let user = accept(
                    first,
                    || {
                        let mut l = String::new();
                        reader.borrow_mut().read_line(&mut l).unwrap();
                        STANDARD.decode(l.trim()).unwrap_or_default()
                    },
                    |t| {
                        let line = format!("334 {}\r\n", STANDARD.encode(t));
                        reader
                            .borrow_mut()
                            .get_mut()
                            .write_all(line.as_bytes())
                            .unwrap();
                    },
                );
                r = reader.into_inner();
                seen.lock().unwrap().push(format!("smtp:{user}"));
                r.get_mut().write_all(b"235 2.7.0 signed in\r\n").unwrap();
            } else if upper == "DATA" {
                r.get_mut().write_all(b"354 go\r\n").unwrap();
                loop {
                    line.clear();
                    r.read_line(&mut line).unwrap();
                    if line == ".\r\n" {
                        break;
                    }
                }
                seen.lock().unwrap().push("smtp:sent".into());
                r.get_mut().write_all(b"250 queued\r\n").unwrap();
            } else if upper == "QUIT" {
                r.get_mut().write_all(b"221 bye\r\n").unwrap();
                return;
            } else {
                r.get_mut().write_all(b"250 ok\r\n").unwrap();
            }
        }
    });

    let (mut b, mut a, path) = backend(&cert)?;
    // No password anywhere: the ticket signs in.
    b.test_secret = None;
    a.kerberos = true;
    a.username = "ana".into();
    a.incoming_port = imap_port;
    a.validate()?;
    let session = b.imap_login(&a, &Secret::default());
    assert!(session.is_ok(), "{:#}", session.err().unwrap());
    drop(session);
    let mut p = a.clone();
    p.provider = Provider::Pop3;
    p.incoming_port = pop_port;
    let pop = b.pop_login(&p, "");
    if let Err(e) = &pop {
        panic!("POP3: {e:?}");
    }
    let _ = pop.unwrap().quit();
    a.smtp_host = "localhost".into();
    a.smtp_port = smtp_port;
    a.smtp_tls = Some(true);
    crate::sync::smtp::deliver_raw(
        &a,
        "ana@example.com",
        &["bo@example.se".into()],
        b"Subject: Hello\r\n\r\nHi\r\n",
        &crate::sync::smtp::Endpoint::of(&a),
        "",
        &b.tls_connector()?,
    )?;
    // The servers each saw the ticket's user ask to act as "ana".
    std::thread::sleep(std::time::Duration::from_millis(100));
    let seen = signed_in.lock().unwrap().clone();
    assert_eq!(seen, ["imap:ana", "pop:ana", "smtp:ana", "smtp:sent"]);
    drop(b);
    std::fs::remove_dir_all(path)?;
    Ok(())
}
