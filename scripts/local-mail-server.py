"""Isolated TLS gateway to Dovecot 2.4's unprivileged protocol workers.

The gateway accepts only synthetic fixture logins; Dovecot implements every
mailbox command. No system service, system user, or real mailbox is modified.
The calling Rust test holds stdin open and closes it to shut down the fixture.
Use the `inspect` mode for a comparison with Python's independent IMAP/POP clients.
"""

import atexit
import getpass
import hashlib
import imaplib
import json
import os
from pathlib import Path
import poplib
import shutil
import signal
import socket
import ssl
import subprocess
import sys
import tempfile
import threading


def inspect_mailbox(config):
    """Return server UIDs/flags and hashes of bytes read by independent clients."""
    context = ssl.create_default_context(cafile=config["certificate"])
    client = imaplib.IMAP4_SSL(
        "localhost", config["imap"], ssl_context=context, timeout=10
    )
    client.login("qa@example.com", "fixture-password")
    client.select("INBOX")
    _, rows = client.uid("SEARCH", None, "ALL")
    messages = []
    for uid in rows[0].decode().split():
        _, fetched = client.uid("FETCH", uid, "(UID FLAGS BODY.PEEK[])")
        metadata, raw = next(row for row in fetched if isinstance(row, tuple))
        messages.append({
            "uid": int(uid),
            "flags": metadata.decode(),
            "sha256": hashlib.sha256(raw).hexdigest(),
        })
    client.logout()

    pop = poplib.POP3_SSL("localhost", config["pop3"], context=context, timeout=10)
    pop.user("qa@example.com")
    pop.pass_("fixture-password")
    bodies = []
    for row in pop.uidl()[1]:
        number, uid = row.decode().split()
        raw = b"\r\n".join(pop.retr(int(number))[1]) + b"\r\n"
        bodies.append({"uid": uid, "sha256": hashlib.sha256(raw).hexdigest()})
    pop.quit()
    return {"imap": messages, "pop": bodies}


class LocalMailServer:
    def __init__(self):
        prefix = Path(os.environ.get("PICACHO_DOVECOT_PREFIX", "/usr"))
        self.libexec = prefix / "lib/dovecot"
        self.doveconf = prefix / "bin/doveconf"
        if not self.doveconf.exists():
            raise SystemExit(
                "Install Dovecot 2.4 and OpenSSL, or set PICACHO_DOVECOT_PREFIX "
                "to an extracted installation prefix."
            )
        self.prefix = prefix
        self.root = Path(tempfile.mkdtemp(prefix="picacho-dovecot-"))
        atexit.register(shutil.rmtree, self.root, ignore_errors=True)
        self.children = []
        self.listeners = []
        for folder in ["", ".Archive", ".Sent", ".Trash"]:
            for leaf in ["cur", "new", "tmp"]:
                (self.root / "mail" / folder / leaf).mkdir(parents=True, exist_ok=True)
        for index in range(1, 4):
            (self.root / "mail/new" / str(index)).write_bytes((
                "From: sender@example.com\r\nTo: qa@example.com\r\n"
                f"Subject: Dovecot {index}\r\n"
                f"Message-ID: <dovecot-{index}@example.com>\r\n"
                "Date: Mon, 26 Oct 2026 10:00:00 +0000\r\n"
                f"Content-Type: text/plain\r\n\r\nMessage {index}.\r\n.dot\r\n"
            ).encode())

        self.certificate = self.root / "cert.pem"
        key = self.root / "key.pem"
        subprocess.run([
            "openssl", "req", "-x509", "-newkey", "rsa:2048", "-nodes",
            "-keyout", str(key), "-out", str(self.certificate), "-days", "1",
            "-subj", "/CN=localhost",
            "-addext", "subjectAltName=DNS:localhost,IP:127.0.0.1",
            "-addext", "basicConstraints=critical,CA:FALSE",
        ], check=True, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
        self.tls = ssl.SSLContext(ssl.PROTOCOL_TLS_SERVER)
        self.tls.load_cert_chain(self.certificate, key)
        self.config = self.root / "dovecot.conf"
        self.config.write_text(
            "dovecot_config_version = 2.4.1\n"
            "dovecot_storage_version = 2.4.1\n"
            f"base_dir = {self.root}/run\nlibexec_dir = {self.libexec}\n"
            f"mail_driver = maildir\nmail_path = {self.root}/mail\n"
            "log_path = /dev/stderr\n"
            f"default_login_user = {getpass.getuser()}\n"
            f"default_internal_user = {getpass.getuser()}\n"
            "auth_allow_cleartext = yes\nssl = no\n"
        )
        (self.root / "run").mkdir()

    def authenticate_fixture(self, tls, protocol):
        """Handle only the synthetic pre-worker login, over verified TLS."""
        reader = tls.makefile("rb", buffering=0)
        greeting = (
            b"* OK local Dovecot gateway\r\n"
            if protocol == "imap" else b"+OK local Dovecot gateway\r\n"
        )
        tls.sendall(greeting)
        while line := reader.readline(8192):
            fields = line.split()
            command = fields[1].upper() if protocol == "imap" else fields[0].upper()
            if protocol == "imap" and command == b"CAPABILITY":
                tls.sendall(b"* CAPABILITY IMAP4rev1\r\n" + fields[0] + b" OK capabilities\r\n")
            elif protocol == "imap" and command == b"LOGIN":
                tls.sendall(fields[0] + b" OK fixture authenticated\r\n")
                return True
            elif protocol == "pop3" and command == b"USER":
                tls.sendall(b"+OK\r\n")
            elif protocol == "pop3" and command == b"PASS":
                tls.sendall(b"+OK\r\n")
                return True
            else:
                raise RuntimeError(f"Unexpected fixture login command {command!r}")
        return False

    def handle(self, connection, protocol):
        worker = None
        try:
            with self.tls.wrap_socket(connection, server_side=True) as tls:
                tls.settimeout(10)
                if not self.authenticate_fixture(tls, protocol):
                    return
                environment = os.environ.copy()
                environment.update(
                    USER=getpass.getuser(), HOME=str(self.root),
                    LD_LIBRARY_PATH=f"{self.libexec}:{self.prefix}/lib/x86_64-linux-gnu",
                )
                worker = subprocess.Popen([
                    str(self.doveconf), "-c", str(self.config), "-F",
                    str(self.libexec / protocol),
                ], stdin=subprocess.PIPE, stdout=subprocess.PIPE,
                    stderr=subprocess.DEVNULL, env=environment)
                self.children.append(worker)
                # IMAP workers greet PREAUTH. POP workers expect commands directly.
                if protocol == "imap":
                    worker.stdout.readline()

                def forward_responses():
                    try:
                        while data := os.read(worker.stdout.fileno(), 65536):
                            tls.sendall(data)
                    except (OSError, ssl.SSLError):
                        pass  # The client may have already closed the session.

                forwarding = threading.Thread(target=forward_responses, daemon=True)
                forwarding.start()
                while data := tls.recv(65536):
                    worker.stdin.write(data)
                    worker.stdin.flush()
                forwarding.join(timeout=1)
        except (OSError, ssl.SSLError):
            pass
        finally:
            if worker is not None:
                worker.terminate()
                worker.wait(timeout=5)
            connection.close()

    def serve(self, protocol):
        listener = socket.socket()
        listener.bind(("127.0.0.1", 0))
        listener.listen()
        self.listeners.append(listener)

        def accept_connections():
            while True:
                connection, _ = listener.accept()
                threading.Thread(
                    target=self.handle, args=(connection, protocol), daemon=True
                ).start()

        threading.Thread(target=accept_connections, daemon=True).start()
        return listener.getsockname()[1]

    def close(self):
        for worker in self.children:
            if worker.poll() is None:
                worker.terminate()
                worker.wait(timeout=5)


def main():
    if sys.argv[1:] == ["inspect"]:
        print(json.dumps(inspect_mailbox(json.load(sys.stdin))))
        return
    signal.signal(signal.SIGTERM, lambda *_: sys.exit(0))
    server = LocalMailServer()
    try:
        print(json.dumps({
            "imap": server.serve("imap"), "pop3": server.serve("pop3"),
            "certificate": str(server.certificate),
        }), flush=True)
        sys.stdin.read()
    finally:
        server.close()


if __name__ == "__main__":
    main()
