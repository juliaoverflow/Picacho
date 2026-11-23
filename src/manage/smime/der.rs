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

#[cfg(test)]
mod tests {
    use super::*;

    const SEQUENCE: u8 = 0x30;
    const INTEGER: u8 = 0x02;

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
