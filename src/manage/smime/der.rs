//! DER (ITU-T X.690) reading for the CMS structures smime.rs takes apart:
//! signer infos, certificates and compressed content. The input is a
//! stranger's mail, so every length is checked against what is left, the
//! nesting is limited, and nothing here can panic or index unchecked.
//!
//! Only DER is read. BER's indefinite lengths (X.690 §8.1.3.6) are refused
//! with `Error::Indefinite`; smime.rs first has OpenSSL parse the message
//! and write it back as DER, which turns BER into DER.

/// How deep constructed values may nest. CMS SignedData with certificates
/// reaches about a dozen levels; past this, the input is hostile.
pub(super) const MAX_DEPTH: usize = 32;

/// X.690 §8.1.2.5: bit 6 of the identifier octet marks a constructed value.
const CONSTRUCTED: u8 = 0x20;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Error {
    /// The input ends inside an identifier, a length or a value.
    Truncated,
    /// A length longer than four octets, or not in its shortest form
    /// (X.690 §10.1: DER lengths are minimal).
    BadLength,
    /// BER's indefinite length (X.690 §8.1.3.6), which DER forbids.
    Indefinite,
    /// A tag number of 31 or more (X.690 §8.1.2.4), which CMS never uses.
    HighTag,
    /// Nesting deeper than `MAX_DEPTH`.
    TooDeep,
    /// A value with another tag than the structure has there.
    Unexpected { expected: u8, found: u8 },
    /// The children of a primitive value were asked for.
    NotConstructed,
    /// Bytes after the value that should have been the last.
    Trailing,
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Truncated => write!(f, "the DER input is truncated"),
            Self::BadLength => write!(f, "a DER length is overlong or not minimal"),
            Self::Indefinite => write!(f, "an indefinite (BER) length isn't DER"),
            Self::HighTag => write!(f, "a high-numbered DER tag isn't supported"),
            Self::TooDeep => write!(f, "the DER input nests too deeply"),
            Self::Unexpected { expected, found } => {
                write!(f, "DER tag {found:#04x} where {expected:#04x} was expected")
            }
            Self::NotConstructed => write!(f, "a primitive DER value has no children"),
            Self::Trailing => write!(f, "bytes follow the last DER value"),
        }
    }
}

impl std::error::Error for Error {}

/// One tag-length-value, borrowed from the input.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct Tlv<'a> {
    /// The identifier octet: class, constructed bit and tag number.
    pub tag: u8,
    /// The contents octets.
    pub value: &'a [u8],
    /// The whole encoding, identifier and length included, as it came.
    pub raw: &'a [u8],
    depth: usize,
}

impl<'a> Tlv<'a> {
    pub fn constructed(&self) -> bool {
        self.tag & CONSTRUCTED != 0
    }
    /// A reader over this constructed value's children, one level deeper.
    pub fn children(&self) -> Result<Reader<'a>, Error> {
        if !self.constructed() {
            return Err(Error::NotConstructed);
        }
        let depth = self.depth + 1;
        if depth > MAX_DEPTH {
            return Err(Error::TooDeep);
        }
        Ok(Reader {
            rest: self.value,
            depth,
        })
    }
}

/// Reads consecutive values from a slice.
#[derive(Clone, Debug)]
pub(super) struct Reader<'a> {
    rest: &'a [u8],
    depth: usize,
}

impl<'a> Reader<'a> {
    pub fn new(input: &'a [u8]) -> Self {
        Self {
            rest: input,
            depth: 0,
        }
    }
    pub fn is_empty(&self) -> bool {
        self.rest.is_empty()
    }
    /// The next value's identifier octet, if there is one.
    pub fn peek_tag(&self) -> Option<u8> {
        self.rest.first().copied()
    }
    /// The next value.
    pub fn read(&mut self) -> Result<Tlv<'a>, Error> {
        let input = self.rest;
        let (&tag, after_tag) = input.split_first().ok_or(Error::Truncated)?;
        // X.690 §8.1.2.4: all five low bits set means the tag number
        // continues in further octets.
        if tag & 0x1f == 0x1f {
            return Err(Error::HighTag);
        }
        let (&first, after_first) = after_tag.split_first().ok_or(Error::Truncated)?;
        let (len, after_len) = if first < 0x80 {
            // X.690 §8.1.3.4: the short form.
            (usize::from(first), after_first)
        } else if first == 0x80 {
            return Err(Error::Indefinite);
        } else {
            // X.690 §8.1.3.5: the long form, its first octet counting the
            // octets that follow. 0xff is reserved; more than four would be
            // a value larger than any message this reads.
            let count = usize::from(first & 0x7f);
            if count > 4 {
                return Err(Error::BadLength);
            }
            let digits = after_first.get(..count).ok_or(Error::Truncated)?;
            let len = digits
                .iter()
                .fold(0usize, |acc, &d| (acc << 8) | usize::from(d));
            // X.690 §10.1: the fewest octets, so no leading zero and no
            // long form for what the short form holds.
            if digits.first() == Some(&0) || len < 0x80 {
                return Err(Error::BadLength);
            }
            (len, after_first.get(count..).ok_or(Error::Truncated)?)
        };
        let value = after_len.get(..len).ok_or(Error::Truncated)?;
        let rest = after_len.get(len..).ok_or(Error::Truncated)?;
        let header = input.len() - after_len.len();
        let raw = input.get(..header + len).ok_or(Error::Truncated)?;
        self.rest = rest;
        Ok(Tlv {
            tag,
            value,
            raw,
            depth: self.depth,
        })
    }
    /// The next value, which must have `tag`.
    pub fn read_tag(&mut self, tag: u8) -> Result<Tlv<'a>, Error> {
        match self.peek_tag() {
            Some(found) if found != tag => Err(Error::Unexpected {
                expected: tag,
                found,
            }),
            _ => self.read(),
        }
    }
    /// The next value when it has `tag` (an OPTIONAL field), else nothing.
    pub fn optional(&mut self, tag: u8) -> Result<Option<Tlv<'a>>, Error> {
        if self.peek_tag() == Some(tag) {
            self.read().map(Some)
        } else {
            Ok(None)
        }
    }
    /// Checks nothing is left.
    pub fn finish(self) -> Result<(), Error> {
        if self.rest.is_empty() {
            Ok(())
        } else {
            Err(Error::Trailing)
        }
    }
}

/// The one value `input` holds, with nothing after it.
pub(super) fn single(input: &[u8]) -> Result<Tlv<'_>, Error> {
    let mut reader = Reader::new(input);
    let tlv = reader.read()?;
    reader.finish()?;
    Ok(tlv)
}

/// One value: `tag`, the DER length (X.690 §10.1: its shortest form) and
/// `parts` as its contents, in order.
pub(super) fn encode(tag: u8, parts: &[&[u8]]) -> Vec<u8> {
    let len: usize = parts.iter().map(|p| p.len()).sum();
    let mut out = Vec::with_capacity(len + 6);
    out.push(tag);
    if len < 0x80 {
        out.push(len as u8);
    } else {
        let digits = len.to_be_bytes();
        let first = digits.iter().position(|&d| d != 0).unwrap_or(digits.len() - 1);
        let digits = digits.get(first..).unwrap_or_default();
        out.push(0x80 | digits.len() as u8);
        out.extend_from_slice(digits);
    }
    for part in parts {
        out.extend_from_slice(part);
    }
    out
}

pub(super) const SEQUENCE: u8 = 0x30;
pub(super) const SET: u8 = 0x31;
const INTEGER: u8 = 0x02;
const OCTET_STRING: u8 = 0x04;
const OID: u8 = 0x06;
/// `[0]` and `[1]`, constructed (EXPLICIT, or IMPLICIT SET OF).
const CONTEXT_0: u8 = 0xa0;
const CONTEXT_1: u8 = 0xa1;
/// `[0]` IMPLICIT, primitive: SignerIdentifier's subjectKeyIdentifier.
const CONTEXT_0_PRIMITIVE: u8 = 0x80;

/// RFC 5652 §5.1: id-signedData, 1.2.840.113549.1.7.2.
pub(super) const SIGNED_DATA: &[u8] = &[0x2a, 0x86, 0x48, 0x86, 0xf7, 0x0d, 0x01, 0x07, 0x02];
/// RFC 3274 §1.1: id-ct-compressedData, 1.2.840.113549.1.9.16.1.9.
pub(super) const COMPRESSED_DATA: &[u8] =
    &[0x2a, 0x86, 0x48, 0x86, 0xf7, 0x0d, 0x01, 0x09, 0x10, 0x01, 0x09];
/// RFC 3274 §1.1: id-alg-zlibCompress, 1.2.840.113549.1.9.16.3.8.
const ZLIB_COMPRESS: &[u8] = &[0x2a, 0x86, 0x48, 0x86, 0xf7, 0x0d, 0x01, 0x09, 0x10, 0x03, 0x08];

/// RFC 5652 §3: ContentInfo's contentType and its `[0] EXPLICIT` content.
pub(super) fn content_info(input: &[u8]) -> Result<(&[u8], Tlv<'_>), Error> {
    let mut info = single(input)?.children()?;
    let oid = info.read_tag(OID)?.value;
    let mut explicit = info.read_tag(CONTEXT_0)?.children()?;
    info.finish()?;
    let content = explicit.read()?;
    explicit.finish()?;
    Ok((oid, content))
}

/// RFC 5652 §5.1: a SignedData, its fields kept as their raw encodings.
pub(super) struct SignedData<'a> {
    version: Tlv<'a>,
    digest_algorithms: Tlv<'a>,
    encap_content_info: Tlv<'a>,
    certificates: Option<Tlv<'a>>,
    crls: Option<Tlv<'a>>,
    pub signer_infos: Vec<Tlv<'a>>,
}

impl<'a> SignedData<'a> {
    /// The SignedData a DER ContentInfo holds.
    pub fn parse(input: &'a [u8]) -> Result<Self, Error> {
        let (oid, content) = content_info(input)?;
        if oid != SIGNED_DATA {
            return Err(Error::Unexpected {
                expected: OID,
                found: content.tag,
            });
        }
        let mut fields = content.children()?;
        let version = fields.read_tag(INTEGER)?;
        let digest_algorithms = fields.read_tag(SET)?;
        let encap_content_info = fields.read_tag(SEQUENCE)?;
        let certificates = fields.optional(CONTEXT_0)?;
        let crls = fields.optional(CONTEXT_1)?;
        let mut signers = fields.read_tag(SET)?.children()?;
        fields.finish()?;
        let mut signer_infos = Vec::new();
        while !signers.is_empty() {
            signer_infos.push(signers.read_tag(SEQUENCE)?);
        }
        Ok(Self {
            version,
            digest_algorithms,
            encap_content_info,
            certificates,
            crls,
            signer_infos,
        })
    }
    /// The certificates it carries, each as its DER encoding. RFC 5652
    /// §10.2.2: other choices (attribute certificates, `[n]`-tagged) are
    /// skipped.
    pub fn certificates(&self) -> Result<Vec<&'a [u8]>, Error> {
        let mut out = Vec::new();
        if let Some(certificates) = self.certificates {
            let mut choices = certificates.children()?;
            while !choices.is_empty() {
                let choice = choices.read()?;
                if choice.tag == SEQUENCE {
                    out.push(choice.raw);
                }
            }
        }
        Ok(out)
    }
    /// The encapsulated content (RFC 5652 §5.2 eContent), where it isn't
    /// detached.
    pub fn content(&self) -> Result<Option<&'a [u8]>, Error> {
        let mut encap = self.encap_content_info.children()?;
        encap.read_tag(OID)?;
        let Some(explicit) = encap.optional(CONTEXT_0)? else {
            return Ok(None);
        };
        encap.finish()?;
        let mut explicit = explicit.children()?;
        let content = explicit.read_tag(OCTET_STRING)?;
        explicit.finish()?;
        Ok(Some(content.value))
    }
    /// A ContentInfo with this SignedData's version, digest algorithms,
    /// content, certificates and CRLs, and only `signer_infos` (each a raw
    /// SignerInfo): OpenSSL checks every signer at once, so each is checked
    /// alone in one. RFC 5652 §5.1 makes signerInfos a SET OF, so they are
    /// written in DER order (X.690 §11.6).
    pub fn with_signers(&self, signer_infos: &[&[u8]]) -> Vec<u8> {
        let mut sorted = signer_infos.to_vec();
        sorted.sort();
        let signer_infos = encode(SET, &sorted);
        let mut fields: Vec<&[u8]> = vec![
            self.version.raw,
            self.digest_algorithms.raw,
            self.encap_content_info.raw,
        ];
        fields.extend(self.certificates.map(|c| c.raw));
        fields.extend(self.crls.map(|c| c.raw));
        fields.push(&signer_infos);
        let signed_data = encode(SEQUENCE, &fields);
        let oid = encode(OID, &[SIGNED_DATA]);
        encode(SEQUENCE, &[&oid, &encode(CONTEXT_0, &[&signed_data])])
    }
}

/// RFC 5652 §5.3: who made a signature.
#[derive(Debug, PartialEq, Eq)]
pub(super) enum SignerId<'a> {
    /// The issuer's Name (its DER) and the serial number's content octets.
    IssuerAndSerial { issuer: &'a [u8], serial: &'a [u8] },
    SubjectKeyId(&'a [u8]),
}

/// A SignerInfo's SignerIdentifier.
pub(super) fn signer_id<'a>(signer_info: &Tlv<'a>) -> Result<SignerId<'a>, Error> {
    let mut fields = signer_info.children()?;
    fields.read_tag(INTEGER)?;
    if let Some(key_id) = fields.optional(CONTEXT_0_PRIMITIVE)? {
        return Ok(SignerId::SubjectKeyId(key_id.value));
    }
    let mut issuer_and_serial = fields.read_tag(SEQUENCE)?.children()?;
    let issuer = issuer_and_serial.read_tag(SEQUENCE)?.raw;
    let serial = issuer_and_serial.read_tag(INTEGER)?.value;
    issuer_and_serial.finish()?;
    Ok(SignerId::IssuerAndSerial { issuer, serial })
}

/// RFC 5280 §4.1: a certificate's issuer (its DER) and serial number's
/// content octets, to match against a SignerIdentifier.
pub(super) fn issuer_and_serial(certificate: &[u8]) -> Result<(&[u8], &[u8]), Error> {
    let mut certificate = single(certificate)?.children()?;
    let mut tbs = certificate.read_tag(SEQUENCE)?.children()?;
    tbs.optional(CONTEXT_0)?;
    let serial = tbs.read_tag(INTEGER)?.value;
    tbs.read_tag(SEQUENCE)?;
    let issuer = tbs.read_tag(SEQUENCE)?.raw;
    Ok((issuer, serial))
}

/// RFC 3274 §1.1: the zlib stream a DER CompressedData ContentInfo holds.
pub(super) fn compressed_content(input: &[u8]) -> Result<&[u8], Error> {
    let (oid, content) = content_info(input)?;
    if oid != COMPRESSED_DATA {
        return Err(Error::Unexpected {
            expected: OID,
            found: content.tag,
        });
    }
    let mut fields = content.children()?;
    fields.read_tag(INTEGER)?;
    let mut algorithm = fields.read_tag(SEQUENCE)?.children()?;
    let algorithm_oid = algorithm.read_tag(OID)?;
    if algorithm_oid.value != ZLIB_COMPRESS {
        return Err(Error::Unexpected {
            expected: OID,
            found: algorithm_oid.tag,
        });
    }
    let mut encap = fields.read_tag(SEQUENCE)?.children()?;
    fields.finish()?;
    encap.read_tag(OID)?;
    let mut explicit = encap.read_tag(CONTEXT_0)?.children()?;
    encap.finish()?;
    let content = explicit.read_tag(OCTET_STRING)?;
    explicit.finish()?;
    Ok(content.value)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn der_encode_writes_minimal_lengths_the_reader_reads_back() {
        for len in [0usize, 1, 0x7f, 0x80, 0xff, 0x100, 0xffff, 0x10000] {
            let value = vec![9u8; len];
            let encoded = encode(0x04, &[&value[..len / 2], &value[len / 2..]]);
            let tlv = single(&encoded).unwrap();
            assert_eq!((tlv.tag, tlv.value.len()), (0x04, len), "{len}");
        }
        assert_eq!(encode(0x30, &[&[0x05, 0x00]]), [0x30, 0x02, 0x05, 0x00]);
        assert_eq!(&encode(0x04, &[&[0u8; 0x80]])[..3], &[0x04, 0x81, 0x80]);
    }

    /// Every value under `tlv`, depth first, as far as the reader allows.
    fn walk(tlv: Tlv<'_>, seen: &mut usize) -> Result<(), Error> {
        *seen += 1;
        if tlv.constructed() {
            let mut children = tlv.children()?;
            while !children.is_empty() {
                walk(children.read()?, seen)?;
            }
        }
        Ok(())
    }

    fn walk_all(input: &[u8]) -> Result<usize, Error> {
        let mut seen = 0;
        walk(single(input)?, &mut seen)?;
        Ok(seen)
    }

    #[test]
    fn der_reads_nested_values_and_keeps_their_raw_bytes() {
        // SEQUENCE { INTEGER 5, SEQUENCE { OCTET STRING "ab" }, [0] NULL-ish }
        let input = [
            0x30, 0x0b, 0x02, 0x01, 0x05, 0x30, 0x04, 0x04, 0x02, b'a', b'b', 0x80, 0x00,
        ];
        let top = single(&input).unwrap();
        assert_eq!(top.tag, SEQUENCE);
        assert_eq!(top.raw, &input[..]);
        let mut children = top.children().unwrap();
        let int = children.read_tag(INTEGER).unwrap();
        assert_eq!(int.value, &[5]);
        assert_eq!(int.raw, &[0x02, 0x01, 0x05]);
        let inner = children.read_tag(SEQUENCE).unwrap();
        assert_eq!(inner.raw, &[0x30, 0x04, 0x04, 0x02, b'a', b'b']);
        assert_eq!(children.optional(0xa0).unwrap(), None);
        let implicit = children.optional(0x80).unwrap().unwrap();
        assert!(implicit.value.is_empty() && !implicit.constructed());
        assert_eq!(implicit.children().err(), Some(Error::NotConstructed));
        children.finish().unwrap();
        assert_eq!(walk_all(&input), Ok(5));
    }

    #[test]
    fn der_reads_long_form_lengths() {
        let mut input = vec![0x04, 0x81, 0x80];
        input.extend(std::iter::repeat_n(7u8, 0x80));
        assert_eq!(single(&input).unwrap().value.len(), 0x80);
        let mut input = vec![0x04, 0x82, 0x01, 0x00];
        input.extend(std::iter::repeat_n(7u8, 0x100));
        assert_eq!(single(&input).unwrap().value.len(), 0x100);
    }

    #[test]
    fn der_refuses_truncated_input() {
        let whole = [0x30, 0x06, 0x02, 0x01, 0x05, 0x04, 0x01, 0xff];
        assert!(single(&whole).is_ok());
        for end in 0..whole.len() {
            let cut = &whole[..end];
            assert_eq!(single(cut).and_then(|_| Ok(())), Err(Error::Truncated), "{end}");
        }
        // The outer length fits, an inner one runs past its parent.
        assert_eq!(walk_all(&[0x30, 0x03, 0x02, 0x05, 0x05]), Err(Error::Truncated));
        // A long-form length whose octets are cut off.
        assert_eq!(single(&[0x04, 0x84, 0x01, 0x00]), Err(Error::Truncated));
    }

    #[test]
    fn der_refuses_overlong_and_non_minimal_lengths() {
        // A length that claims more than the input has, with every size of
        // length field up to four octets, and the largest of all.
        assert_eq!(single(&[0x04, 0x7f, 0x00]), Err(Error::Truncated));
        assert_eq!(single(&[0x04, 0x84, 0xff, 0xff, 0xff, 0xff, 0x00]), Err(Error::Truncated));
        // Five or more length octets, and the reserved 0xff.
        assert_eq!(
            single(&[0x04, 0x85, 0x01, 0x00, 0x00, 0x00, 0x00]),
            Err(Error::BadLength)
        );
        assert_eq!(single(&[0x04, 0xff]), Err(Error::BadLength));
        // X.690 §10.1: a leading zero octet, or the long form for a short length.
        assert_eq!(single(&[0x04, 0x82, 0x00, 0x01, 0x00]), Err(Error::BadLength));
        assert_eq!(single(&[0x04, 0x81, 0x01, 0x00]), Err(Error::BadLength));
    }

    #[test]
    fn der_cleanly_refuses_ber_indefinite_lengths() {
        // SEQUENCE (indefinite) { INTEGER 5 } end-of-contents.
        let ber = [0x30, 0x80, 0x02, 0x01, 0x05, 0x00, 0x00];
        assert_eq!(single(&ber), Err(Error::Indefinite));
        // Inside a definite parent too.
        assert_eq!(
            walk_all(&[0x30, 0x06, 0x30, 0x80, 0x05, 0x00, 0x00, 0x00]),
            Err(Error::Indefinite)
        );
    }

    #[test]
    fn der_refuses_deep_nesting_without_overflowing_the_stack() {
        // 5,000 nested SEQUENCEs, each with a valid minimal length.
        let mut input: Vec<u8> = vec![0x05, 0x00];
        for _ in 0..5_000 {
            let len = input.len();
            let mut next = if len < 0x80 {
                vec![SEQUENCE, len as u8]
            } else if len < 0x100 {
                vec![SEQUENCE, 0x81, len as u8]
            } else {
                vec![SEQUENCE, 0x82, (len >> 8) as u8, len as u8]
            };
            next.extend_from_slice(&input);
            input = next;
        }
        assert_eq!(walk_all(&input), Err(Error::TooDeep));
        // Exactly the limit is read.
        let mut input: Vec<u8> = vec![0x05, 0x00];
        for _ in 0..MAX_DEPTH {
            let mut next = vec![SEQUENCE, input.len() as u8];
            next.extend_from_slice(&input);
            input = next;
        }
        assert_eq!(walk_all(&input), Ok(MAX_DEPTH + 1));
    }

    #[test]
    fn der_refuses_high_tags_trailing_bytes_and_wrong_tags() {
        assert_eq!(single(&[0x1f, 0x81, 0x00, 0x00]), Err(Error::HighTag));
        assert_eq!(single(&[0x05, 0x00, 0x00]), Err(Error::Trailing));
        assert_eq!(single(&[]), Err(Error::Truncated));
        let mut reader = Reader::new(&[0x02, 0x01, 0x05]);
        assert_eq!(
            reader.read_tag(SEQUENCE),
            Err(Error::Unexpected {
                expected: SEQUENCE,
                found: INTEGER
            })
        );
        // A wrong tag reads nothing.
        assert_eq!(reader.read_tag(INTEGER).unwrap().value, &[5]);
    }

    /// Random and mutated input never panics.
    #[test]
    fn der_survives_mutated_input() {
        let base = [
            0x30, 0x0b, 0x02, 0x01, 0x05, 0x30, 0x04, 0x04, 0x02, b'a', b'b', 0x80, 0x00,
        ];
        let mut state = 0x2545_f491_4f6c_dd1du64;
        let mut next = || {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            state
        };
        for _ in 0..20_000 {
            let mut input = base.to_vec();
            for _ in 0..(next() % 4 + 1) {
                let at = (next() % input.len() as u64) as usize;
                input[at] = next() as u8;
            }
            input.truncate((next() % (input.len() as u64 + 1)) as usize);
            let _ = walk_all(&input);
        }
    }
}
