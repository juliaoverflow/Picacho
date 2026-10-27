//! Bounds response memory and total command time underneath verified TLS.
use std::{
    io::{self, Read, Write},
    net::TcpStream,
    time::{Duration, Instant},
};

const RESPONSE_LIMIT: usize = 32 * 1024 * 1024;
const COMMAND_TIMEOUT: Duration = Duration::from_secs(90);
const IDLE_TIMEOUT: Duration = Duration::from_secs(30);

/// Applied before TLS so both implicit TLS and STARTTLS use the same limits.
/// The budget includes TLS framing. Each outgoing command starts a new budget;
/// the protocol layer still enforces the stricter 25 MB decoded message limit.
#[derive(Debug)]
pub(crate) struct Wire {
    socket: TcpStream,
    remaining: usize,
    deadline: Instant,
}
impl Wire {
    pub(crate) fn new(socket: TcpStream) -> Self {
        Self {
            socket,
            remaining: RESPONSE_LIMIT,
            deadline: Instant::now() + COMMAND_TIMEOUT,
        }
    }
}
impl Read for Wire {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        if buf.is_empty() {
            return Ok(0);
        }
        if self.remaining == 0 {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "Mail server response exceeds the 32 MB session-command limit",
            ));
        }
        let remaining_time = self
            .deadline
            .checked_duration_since(Instant::now())
            .filter(|d| !d.is_zero())
            .ok_or_else(|| {
                io::Error::new(io::ErrorKind::TimedOut, "Mail server command timed out")
            })?;
        self.socket
            .set_read_timeout(Some(remaining_time.min(IDLE_TIMEOUT)))?;
        let capacity = buf.len().min(self.remaining);
        let read = self.socket.read(&mut buf[..capacity])?;
        self.remaining -= read;
        Ok(read)
    }
}
impl Write for Wire {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        let written = self.socket.write(buf)?;
        if written > 0 {
            self.remaining = RESPONSE_LIMIT;
            self.deadline = Instant::now() + COMMAND_TIMEOUT;
        }
        Ok(written)
    }
    fn flush(&mut self) -> io::Result<()> {
        self.socket.flush()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn response_budget_and_command_deadline_are_enforced() -> io::Result<()> {
        let listener = std::net::TcpListener::bind("127.0.0.1:0")?;
        let mut client = Wire::new(TcpStream::connect(listener.local_addr()?)?);
        let (mut server, _) = listener.accept()?;
        server.write_all(b"12345")?;
        client.remaining = 4;
        let mut data = [0; 4];
        client.read_exact(&mut data)?;
        assert_eq!(&data, b"1234");
        assert_eq!(
            client.read(&mut data).unwrap_err().kind(),
            io::ErrorKind::InvalidData
        );
        client.write_all(b"next command\r\n")?;
        assert_eq!(client.read(&mut data)?, 1);
        client.deadline = Instant::now() - Duration::from_millis(1);
        assert_eq!(
            client.read(&mut data).unwrap_err().kind(),
            io::ErrorKind::TimedOut
        );
        Ok(())
    }
}
