//! Internal protocol facade for the Manage coordinator.
pub(crate) use super::{imap::*, mime::*, pop::*};
pub const MAX_MESSAGE: usize = 25 * 1024 * 1024;
#[cfg(test)]
mod tests {
    use super::*;
    use crate::manage::model::*;
    use anyhow::Result;
    use std::{
        io::{BufRead, BufReader, Write},
        net::TcpStream,
    };
    #[test]
    fn parses_mime_body_and_attachment() -> Result<()> {
        let raw=b"From: =?UTF-8?Q?Sophie_Chen?= <sophie@example.com>\r\nTo: me@example.com\r\nSubject: =?UTF-8?Q?Hello_=E2=9C=93?=\r\nMIME-Version: 1.0\r\nContent-Type: multipart/mixed; boundary=x\r\n\r\n--x\r\nContent-Type: text/plain; charset=utf-8\r\n\r\nHi there!\r\n--x\r\nContent-Type: text/plain; name=hello.txt\r\nContent-Disposition: attachment; filename=hello.txt\r\nContent-Transfer-Encoding: base64\r\n\r\naGVsbG8=\r\n--x--\r\n";
        let m = parse_message(
            raw,
            &Account::new(Provider::Imap),
            "id",
            Folder::Inbox,
            "INBOX",
            1,
        )?;
        assert_eq!(m.sender, "Sophie Chen");
        assert_eq!(m.subject, "Hello ✓");
        assert!(m.body.unwrap().contains("Hi there!"));
        assert_eq!(m.attachments[0].name, "hello.txt");
        assert_eq!(m.attachments[0].size, 5);
        Ok(())
    }
    #[test]
    fn html_does_not_display_script() {
        assert_eq!(
            html_to_text(
                "<p>Hello &amp; welcome</p><script>bad()</script><style>css</style><p>World</p>"
            ),
            "Hello & welcome\nWorld"
        );
    }
    #[test]
    fn pop3_loopback_uidl_and_dot_unstuffing() -> Result<()> {
        let listener = std::net::TcpListener::bind("127.0.0.1:0")?;
        let address = listener.local_addr()?;
        let server = std::thread::spawn(move || {
            let (mut s, _) = listener.accept().unwrap();
            s.write_all(b"+OK ready\r\n").unwrap();
            let mut r = BufReader::new(s);
            let mut line = String::new();
            r.read_line(&mut line).unwrap();
            assert_eq!(line, "UIDL\r\n");
            r.get_mut()
                .write_all(b"+OK\r\n1 abc\r\n2 def\r\n.\r\n")
                .unwrap();
            line.clear();
            r.read_line(&mut line).unwrap();
            assert_eq!(line, "RETR 2\r\n");
            r.get_mut()
                .write_all(b"+OK\r\nSubject: Test\r\n\r\n..hello\r\n.\r\n")
                .unwrap();
        });
        let mut p = PopSession::new(TcpStream::connect(address)?)?;
        assert_eq!(p.uidl()?, vec![(1, "abc".into()), (2, "def".into())]);
        p.command("RETR 2")?;
        assert!(String::from_utf8(p.multiline(1024)?)?.contains("\r\n.hello\r\n"));
        server.join().unwrap();
        Ok(())
    }
    #[test]
    fn rejects_pop_command_injection() -> Result<()> {
        let mut p = PopSession::new(std::io::Cursor::new(b"+OK\r\n".to_vec()))?;
        assert!(p.command("USER a\r\nDELE 1").is_err());
        Ok(())
    }
}
