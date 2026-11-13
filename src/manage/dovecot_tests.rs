//! Opt-in comparison with Dovecot and Python's independent protocol clients.
use super::*;
use sha2::{Digest, Sha256};
/// A SHA-256 digest in lowercase hex.
fn sha256_hex(data: impl AsRef<[u8]>) -> String {
    Sha256::digest(data).iter().map(|b| format!("{b:02x}")).collect()
}
use std::{
    io::{BufRead, BufReader, Write},
    process::{Child, Command, Stdio},
};

struct Server(Child);
impl Drop for Server {
    fn drop(&mut self) {
        // Closing stdin lets the fixture clean up its workers and temporary mail.
        self.0.stdin.take();
        let _ = self.0.wait();
    }
}
fn observe(config: &serde_json::Value) -> Result<serde_json::Value> {
    let mut child = Command::new("python3")
        .args(["scripts/local-mail-server.py", "inspect"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()?;
    child
        .stdin
        .take()
        .unwrap()
        .write_all(serde_json::to_string(config)?.as_bytes())?;
    let result = child.wait_with_output()?;
    ensure!(
        result.status.success(),
        "Independent protocol client failed"
    );
    Ok(serde_json::from_slice(&result.stdout)?)
}
#[test]
#[ignore = "requires local Dovecot 2.4, Python 3, OpenSSL and loopback sockets"]
fn dovecot_matches_independent_imap_and_pop_clients() -> Result<()> {
    let child = Command::new("python3")
        .arg("scripts/local-mail-server.py")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()?;
    let mut server = Server(child);
    let mut line = String::new();
    BufReader::new(server.0.stdout.take().unwrap()).read_line(&mut line)?;
    let config: serde_json::Value = serde_json::from_str(&line)?;
    let cert = std::fs::read(
        config["certificate"]
            .as_str()
            .context("Missing test certificate")?,
    )?;
    let (b, mut a, path) = protocol_tests::backend(&cert)?;
    a.incoming_port = config["imap"].as_u64().unwrap() as u16;
    let mails = b.sync(&a, Folder::Inbox, 300)?;
    assert_eq!(mails.len(), 3);
    let reference = observe(&config)?;
    for mail in &mails {
        let expected = reference["imap"]
            .as_array()
            .unwrap()
            .iter()
            .find(|r| r["uid"] == mail.uid)
            .unwrap();
        let read = b.read(&a, mail)?;
        assert!(read.body.unwrap().contains(".dot"));
        let raw = b.store.raw(&mail.id)?.unwrap();
        assert_eq!(
            sha256_hex(&raw),
            expected["sha256"].as_str().unwrap()
        );
    }
    b.change_many(&a, &mails[..1], Change::Read(true))?;
    b.change_many(&a, &mails[..1], Change::Star(true))?;
    let flags = observe(&config)?;
    let updated = flags["imap"]
        .as_array()
        .unwrap()
        .iter()
        .find(|r| r["uid"] == mails[0].uid)
        .unwrap();
    assert!(updated["flags"].as_str().unwrap().contains("\\Seen"));
    assert!(updated["flags"].as_str().unwrap().contains("\\Flagged"));

    let mut pop = a.clone();
    pop.id = uuid::Uuid::new_v4().to_string();
    pop.provider = Provider::Pop3;
    pop.incoming_port = config["pop3"].as_u64().unwrap() as u16;
    let downloaded = b.sync(&pop, Folder::Inbox, 300)?;
    assert_eq!(downloaded.len(), 3);
    for mail in &downloaded {
        let expected = flags["pop"]
            .as_array()
            .unwrap()
            .iter()
            .find(|r| mail.id.ends_with(r["uid"].as_str().unwrap()))
            .unwrap();
        assert_eq!(
            sha256_hex(b.store.raw(&mail.id)?.unwrap()),
            expected["sha256"].as_str().unwrap()
        );
    }
    b.change_many(&pop, &downloaded[..1], Change::Move(Folder::Archive))?;
    assert_eq!(b.sync(&pop, Folder::Inbox, 300)?.len(), 2);
    assert_eq!(b.store.messages(&pop.id, Folder::Archive, 300)?.len(), 1);
    b.change_many(&a, &mails[..1], Change::Move(Folder::Archive))?;
    assert_eq!(b.sync(&a, Folder::Inbox, 300)?.len(), 2);
    assert_eq!(b.sync(&a, Folder::Archive, 300)?.len(), 1);
    assert_eq!(observe(&config)?["imap"].as_array().unwrap().len(), 2);
    drop(b);
    std::fs::remove_dir_all(path)?;
    Ok(())
}
