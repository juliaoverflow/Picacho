//! NTLM over HTTP (MS-NLMP, as IIS takes it for Exchange Web Services):
//! a NEGOTIATE message, the server's CHALLENGE, and an AUTHENTICATE
//! message with an NTLMv2 response. Only NTLMv2: LM and NTLMv1 answers,
//! which can be cracked from a capture, are never sent.
//!
//! The password is used here to make the response and kept nowhere else.
use anyhow::{Result, bail, ensure};
use hmac::{KeyInit as _, Mac as _};

type HmacMd5 = hmac::Hmac<md5::Md5>;

const NEGOTIATE_UNICODE: u32 = 0x0000_0001;
const REQUEST_TARGET: u32 = 0x0000_0004;
const NEGOTIATE_NTLM: u32 = 0x0000_0200;
const NEGOTIATE_ALWAYS_SIGN: u32 = 0x0000_8000;
const NEGOTIATE_EXTENDED_SESSIONSECURITY: u32 = 0x0008_0000;
const NEGOTIATE_TARGET_INFO: u32 = 0x0080_0000;
const NEGOTIATE_128: u32 = 0x2000_0000;
const NEGOTIATE_56: u32 = 0x8000_0000;
const FLAGS: u32 = NEGOTIATE_UNICODE
    | REQUEST_TARGET
    | NEGOTIATE_NTLM
    | NEGOTIATE_ALWAYS_SIGN
    | NEGOTIATE_EXTENDED_SESSIONSECURITY
    | NEGOTIATE_TARGET_INFO
    | NEGOTIATE_128
    | NEGOTIATE_56;
const SIGNATURE: &[u8; 8] = b"NTLMSSP\0";

/// MD4 (RFC 1320), which only the NT password hash still needs.
pub fn md4(input: &[u8]) -> [u8; 16] {
    let mut message = input.to_vec();
    let bits = (input.len() as u64).wrapping_mul(8);
    message.push(0x80);
    while message.len() % 64 != 56 {
        message.push(0);
    }
    message.extend_from_slice(&bits.to_le_bytes());
    let (mut a, mut b, mut c, mut d) = (0x6745_2301u32, 0xefcd_ab89u32, 0x98ba_dcfeu32, 0x1032_5476u32);
    for block in message.chunks(64) {
        let x: Vec<u32> = block.chunks(4).map(|w| u32::from_le_bytes([w[0], w[1], w[2], w[3]])).collect();
        let (aa, bb, cc, dd) = (a, b, c, d);
        let f = |x: u32, y: u32, z: u32| (x & y) | (!x & z);
        let g = |x: u32, y: u32, z: u32| (x & y) | (x & z) | (y & z);
        let h = |x: u32, y: u32, z: u32| x ^ y ^ z;
        for &i in &[0usize, 4, 8, 12] {
            a = a.wrapping_add(f(b, c, d)).wrapping_add(x[i]).rotate_left(3);
            d = d.wrapping_add(f(a, b, c)).wrapping_add(x[i + 1]).rotate_left(7);
            c = c.wrapping_add(f(d, a, b)).wrapping_add(x[i + 2]).rotate_left(11);
            b = b.wrapping_add(f(c, d, a)).wrapping_add(x[i + 3]).rotate_left(19);
        }
        for &i in &[0usize, 1, 2, 3] {
            let k = 0x5a82_7999;
            a = a.wrapping_add(g(b, c, d)).wrapping_add(x[i]).wrapping_add(k).rotate_left(3);
            d = d.wrapping_add(g(a, b, c)).wrapping_add(x[i + 4]).wrapping_add(k).rotate_left(5);
            c = c.wrapping_add(g(d, a, b)).wrapping_add(x[i + 8]).wrapping_add(k).rotate_left(9);
            b = b.wrapping_add(g(c, d, a)).wrapping_add(x[i + 12]).wrapping_add(k).rotate_left(13);
        }
        for &i in &[0usize, 2, 1, 3] {
            let k = 0x6ed9_eba1;
            a = a.wrapping_add(h(b, c, d)).wrapping_add(x[i]).wrapping_add(k).rotate_left(3);
            d = d.wrapping_add(h(a, b, c)).wrapping_add(x[i + 8]).wrapping_add(k).rotate_left(9);
            c = c.wrapping_add(h(d, a, b)).wrapping_add(x[i + 4]).wrapping_add(k).rotate_left(11);
            b = b.wrapping_add(h(c, d, a)).wrapping_add(x[i + 12]).wrapping_add(k).rotate_left(15);
        }
        a = a.wrapping_add(aa);
        b = b.wrapping_add(bb);
        c = c.wrapping_add(cc);
        d = d.wrapping_add(dd);
    }
    let mut out = [0u8; 16];
    for (i, w) in [a, b, c, d].iter().enumerate() {
        out[i * 4..i * 4 + 4].copy_from_slice(&w.to_le_bytes());
    }
    out
}

fn utf16(s: &str) -> Vec<u8> {
    s.encode_utf16().flat_map(u16::to_le_bytes).collect()
}
fn hmac_md5(key: &[u8], parts: &[&[u8]]) -> [u8; 16] {
    let mut mac = HmacMd5::new_from_slice(key).expect("HMAC takes any key");
    for p in parts {
        mac.update(p);
    }
    mac.finalize().into_bytes().into()
}

/// Who signs in: `DOMAIN\user`, `user@domain` (a UPN, sent whole) or
/// `user`, and the password.
#[derive(Clone)]
pub struct Identity {
    pub user: String,
    pub domain: String,
    pub password: String,
}
impl Identity {
    pub fn new(login: &str, password: &str) -> Self {
        let (domain, user) = match login.split_once('\\') {
            Some((d, u)) => (d.to_owned(), u.to_owned()),
            None => (String::new(), login.to_owned()),
        };
        Self { user, domain, password: password.to_owned() }
    }
}
impl std::fmt::Debug for Identity {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Identity {{ user: {:?}, domain: {:?} }}", self.user, self.domain)
    }
}

/// The NEGOTIATE message (MS-NLMP §2.2.1.1), without domain or workstation.
pub fn negotiate() -> Vec<u8> {
    let mut m = SIGNATURE.to_vec();
    m.extend_from_slice(&1u32.to_le_bytes());
    m.extend_from_slice(&FLAGS.to_le_bytes());
    m.extend_from_slice(&[0; 16]); // domain and workstation fields, empty
    m
}

/// What a CHALLENGE message (§2.2.1.2) carries that the answer needs.
#[derive(Debug, PartialEq, Eq)]
pub struct Challenge {
    pub flags: u32,
    pub server_challenge: [u8; 8],
    pub target_info: Vec<u8>,
}
pub fn parse_challenge(m: &[u8]) -> Result<Challenge> {
    ensure!(m.len() >= 32 && &m[..8] == SIGNATURE, "the server's NTLM challenge isn't one");
    ensure!(u32::from_le_bytes(m[8..12].try_into()?) == 2, "the server's NTLM message isn't a challenge");
    let flags = u32::from_le_bytes(m[20..24].try_into()?);
    let mut server_challenge = [0u8; 8];
    server_challenge.copy_from_slice(&m[24..32]);
    let target_info = if m.len() >= 48 {
        let len = u16::from_le_bytes(m[40..42].try_into()?) as usize;
        let at = u32::from_le_bytes(m[44..48].try_into()?) as usize;
        m.get(at..at + len).map(<[u8]>::to_vec).unwrap_or_default()
    } else {
        Vec::new()
    };
    if flags & NEGOTIATE_UNICODE == 0 {
        bail!("the server's NTLM doesn't take Unicode names");
    }
    Ok(Challenge { flags, server_challenge, target_info })
}

/// The time in `target_info` (MsvAvTimestamp), if the server gave one.
fn timestamp_in(target_info: &[u8]) -> Option<[u8; 8]> {
    let mut at = 0;
    while at + 4 <= target_info.len() {
        let id = u16::from_le_bytes([target_info[at], target_info[at + 1]]);
        let len = u16::from_le_bytes([target_info[at + 2], target_info[at + 3]]) as usize;
        let value = target_info.get(at + 4..at + 4 + len)?;
        match id {
            0 => return None,
            7 if len == 8 => return value.try_into().ok(),
            _ => at += 4 + len,
        }
    }
    None
}

/// The NTLMv2 answer (§3.3.2) for `challenge`: its NT response, at
/// `time` (Windows FILETIME) with `client_challenge`.
pub fn ntlmv2_response(id: &Identity, challenge: &Challenge, time: [u8; 8], client_challenge: [u8; 8]) -> Vec<u8> {
    let nt_hash = md4(&utf16(&id.password));
    let user_domain = [utf16(&id.user.to_uppercase()), utf16(&id.domain)].concat();
    let key = hmac_md5(&nt_hash, &[&user_domain]);
    let mut blob = vec![1, 1, 0, 0, 0, 0, 0, 0];
    blob.extend_from_slice(&time);
    blob.extend_from_slice(&client_challenge);
    blob.extend_from_slice(&[0; 4]);
    blob.extend_from_slice(&challenge.target_info);
    blob.extend_from_slice(&[0; 4]);
    let proof = hmac_md5(&key, &[&challenge.server_challenge, &blob]);
    [proof.to_vec(), blob].concat()
}

/// RFC 2744 §3.11's `gss_channel_bindings_struct` with no addresses and
/// `application_data` (for HTTPS, `tls-server-end-point:` and the hash of
/// the server's certificate), hashed with MD5 as MS-NLMP §2.2.2.1's
/// MsvAvChannelBindings has it.
pub fn channel_bindings_hash(application_data: &[u8]) -> [u8; 16] {
    use md5::Digest as _;
    let mut h = md5::Md5::new();
    // Initiator and acceptor: address type and length, both zero.
    h.update([0u8; 16]);
    h.update((application_data.len() as u32).to_le_bytes());
    h.update(application_data);
    h.finalize().into()
}

/// The server's target info with what Extended Protection checks added
/// (MS-NLMP §2.2.2.1): the TLS channel's bindings (MsvAvChannelBindings),
/// where known, and the service the answer is for (MsvAvTargetName,
/// `HTTP/host`), so the answer can't be relayed to another server or over
/// another TLS connection.
pub fn bound_target_info(target_info: &[u8], bindings: Option<&[u8]>, spn: Option<&str>) -> Vec<u8> {
    let mut out = Vec::with_capacity(target_info.len() + 64);
    let mut at = 0;
    while at + 4 <= target_info.len() {
        let id = u16::from_le_bytes([target_info[at], target_info[at + 1]]);
        let len = u16::from_le_bytes([target_info[at + 2], target_info[at + 3]]) as usize;
        if id == 0 || at + 4 + len > target_info.len() {
            break;
        }
        // The server's own are kept, but ours replace any such it sent.
        if id != 0x0009 && id != 0x000A {
            out.extend_from_slice(&target_info[at..at + 4 + len]);
        }
        at += 4 + len;
    }
    let mut pair = |id: u16, value: &[u8]| {
        out.extend_from_slice(&id.to_le_bytes());
        out.extend_from_slice(&(value.len() as u16).to_le_bytes());
        out.extend_from_slice(value);
    };
    if let Some(data) = bindings {
        pair(0x000A, &channel_bindings_hash(data));
    }
    if let Some(spn) = spn {
        pair(0x0009, &utf16(spn));
    }
    pair(0x0000, &[]);
    out
}

/// The AUTHENTICATE message (§2.2.1.3) answering `challenge`, bound to the
/// TLS channel (`bindings`, its application data) and to the service
/// (`spn`, `HTTP/host`) where given.
pub fn authenticate(id: &Identity, challenge: &Challenge, bindings: Option<&[u8]>, spn: Option<&str>) -> Vec<u8> {
    let time = timestamp_in(&challenge.target_info).unwrap_or_else(|| {
        // Windows time: 100 ns since 1601.
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.as_nanos() / 100) as u64;
        (now + 116_444_736_000_000_000).to_le_bytes()
    });
    let client_challenge: [u8; 8] = rand::random();
    let bound = Challenge {
        flags: challenge.flags,
        server_challenge: challenge.server_challenge,
        target_info: bound_target_info(&challenge.target_info, bindings, spn),
    };
    let nt = ntlmv2_response(id, &bound, time, client_challenge);
    // LMv2 is left empty (24 zeros): with a timestamp, MS-NLMP says not to send it.
    let lm = vec![0u8; 24];
    let domain = utf16(&id.domain);
    let user = utf16(&id.user);
    let workstation = utf16("PICACHO");
    let flags = challenge.flags & FLAGS | NEGOTIATE_UNICODE;
    let header_len = 64usize;
    let fields = [&lm, &nt, &domain, &user, &workstation];
    let mut m = SIGNATURE.to_vec();
    m.extend_from_slice(&3u32.to_le_bytes());
    let mut offset = header_len;
    let mut payload = Vec::new();
    for f in fields {
        m.extend_from_slice(&(f.len() as u16).to_le_bytes());
        m.extend_from_slice(&(f.len() as u16).to_le_bytes());
        m.extend_from_slice(&(offset as u32).to_le_bytes());
        offset += f.len();
        payload.extend_from_slice(f);
    }
    // No session key (no signing or sealing over HTTP).
    m.extend_from_slice(&0u16.to_le_bytes());
    m.extend_from_slice(&0u16.to_le_bytes());
    m.extend_from_slice(&(offset as u32).to_le_bytes());
    m.extend_from_slice(&flags.to_le_bytes());
    debug_assert_eq!(m.len(), header_len);
    m.extend_from_slice(&payload);
    m
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn md4_matches_rfc_1320() {
        let hex = |b: [u8; 16]| b.iter().map(|x| format!("{x:02x}")).collect::<String>();
        assert_eq!(hex(md4(b"")), "31d6cfe0d16ae931b73c59d7e0c089c0");
        assert_eq!(hex(md4(b"abc")), "a448017aaf21d8525fc10ae87aa6729d");
        assert_eq!(hex(md4(b"12345678901234567890123456789012345678901234567890123456789012345678901234567890")), "e33b4ddc9c38f2199c3e7b164fcc0536");
    }

    /// MS-NLMP §4.2.4.2.2: the NTLMv2 response for its sample user.
    #[test]
    fn ntlmv2_matches_ms_nlmp() {
        let id = Identity { user: "User".into(), domain: "Domain".into(), password: "Password".into() };
        assert_eq!(
            md4(&utf16("Password")),
            [0xa4, 0xf4, 0x9c, 0x40, 0x65, 0x10, 0xbd, 0xca, 0xb6, 0x82, 0x4e, 0xe7, 0xc3, 0x0f, 0xd8, 0x52]
        );
        // §4.2.4.1.1 target info: NetBIOS domain "Domain", server "Server".
        let mut target_info = Vec::new();
        for (kind, value) in [(2u16, "Domain"), (1u16, "Server")] {
            let v = utf16(value);
            target_info.extend_from_slice(&kind.to_le_bytes());
            target_info.extend_from_slice(&(v.len() as u16).to_le_bytes());
            target_info.extend_from_slice(&v);
        }
        target_info.extend_from_slice(&[0, 0, 0, 0]);
        let challenge = Challenge {
            flags: FLAGS,
            server_challenge: [0x01, 0x23, 0x45, 0x67, 0x89, 0xab, 0xcd, 0xef],
            target_info,
        };
        let response = ntlmv2_response(&id, &challenge, [0; 8], [0xaa; 8]);
        // §4.2.4.2.2 NTProofStr.
        assert_eq!(
            &response[..16],
            &[0x68, 0xcd, 0x0a, 0xb8, 0x51, 0xe5, 0x1c, 0x96, 0xaa, 0xbc, 0x92, 0x7b, 0xeb, 0xef, 0x6a, 0x1c]
        );
    }

    #[test]
    fn messages_have_their_shape() -> Result<()> {
        let n = negotiate();
        assert_eq!(&n[..8], SIGNATURE);
        let mut c = SIGNATURE.to_vec();
        c.extend_from_slice(&2u32.to_le_bytes());
        c.extend_from_slice(&[0; 8]);
        c.extend_from_slice(&FLAGS.to_le_bytes());
        c.extend_from_slice(&[9; 8]);
        c.extend_from_slice(&[0; 8]);
        let info = [7u8, 0, 8, 0, 1, 2, 3, 4, 5, 6, 7, 8, 0, 0, 0, 0];
        c.extend_from_slice(&(info.len() as u16).to_le_bytes());
        c.extend_from_slice(&(info.len() as u16).to_le_bytes());
        c.extend_from_slice(&48u32.to_le_bytes());
        c.extend_from_slice(&info);
        let challenge = parse_challenge(&c)?;
        assert_eq!(challenge.server_challenge, [9; 8]);
        assert_eq!(timestamp_in(&challenge.target_info), Some([1, 2, 3, 4, 5, 6, 7, 8]));
        let m = authenticate(&Identity::new("CORP\\ana", "pw"), &challenge, None, None);
        assert_eq!(u32::from_le_bytes(m[8..12].try_into()?), 3);
        let user_len = u16::from_le_bytes(m[36..38].try_into()?) as usize;
        let user_at = u32::from_le_bytes(m[40..44].try_into()?) as usize;
        assert_eq!(&m[user_at..user_at + user_len], utf16("ana").as_slice());
        assert!(parse_challenge(b"nonsense").is_err());
        Ok(())
    }

    #[test]
    fn answers_are_bound_to_the_channel_and_service() {
        // The server's pairs are kept and ours come before the end.
        let server = [7u8, 0, 8, 0, 1, 2, 3, 4, 5, 6, 7, 8, 0, 0, 0, 0];
        let bound = bound_target_info(&server, Some(b"tls-server-end-point:abc"), Some("HTTP/mail.corp.example"));
        assert_eq!(&bound[..12], &server[..12]);
        assert_eq!(&bound[12..16], &[0x0A, 0, 16, 0]);
        assert_eq!(&bound[16..32], &channel_bindings_hash(b"tls-server-end-point:abc"));
        let spn = utf16("HTTP/mail.corp.example");
        assert_eq!(&bound[32..36], &[0x09, 0, spn.len() as u8, 0]);
        assert_eq!(&bound[36..36 + spn.len()], spn.as_slice());
        assert_eq!(&bound[36 + spn.len()..], &[0, 0, 0, 0]);
        // A gss_channel_bindings_struct: 16 zero bytes for the addresses,
        // then the data's length and the data.
        let mut flat = vec![0u8; 16];
        flat.extend_from_slice(&3u32.to_le_bytes());
        flat.extend_from_slice(b"xyz");
        use md5::Digest as _;
        let expected: [u8; 16] = md5::Md5::digest(&flat).into();
        assert_eq!(channel_bindings_hash(b"xyz"), expected);
        // Nothing known: just the end.
        assert_eq!(bound_target_info(&[], None, None), vec![0, 0, 0, 0]);
    }
}
