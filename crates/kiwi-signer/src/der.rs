//! Minimal DER reader for the structures Kiwi needs (CMS content, X.509 subject and validity).

pub(crate) const BOOLEAN: u8 = 0x01;
pub(crate) const INTEGER: u8 = 0x02;
pub(crate) const OCTET_STRING: u8 = 0x04;
pub(crate) const OID: u8 = 0x06;
pub(crate) const UTC_TIME: u8 = 0x17;
pub(crate) const GENERALIZED_TIME: u8 = 0x18;
pub(crate) const SEQUENCE: u8 = 0x30;
pub(crate) const SET: u8 = 0x31;
const OCTET_STRING_CONSTRUCTED: u8 = 0x24;

/// Tag of a constructed context-specific element `[n]`.
pub(crate) const fn context(n: u8) -> u8 {
    0xa0 | n
}

/// pkcs7-signedData, 1.2.840.113549.1.7.2
const OID_SIGNED_DATA: &[u8] = &[0x2a, 0x86, 0x48, 0x86, 0xf7, 0x0d, 0x01, 0x07, 0x02];

pub(crate) struct Reader<'a>(&'a [u8]);

impl<'a> Reader<'a> {
    pub(crate) fn new(data: &'a [u8]) -> Self {
        Self(data)
    }

    pub(crate) fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    /// Reads the next element and returns its tag and contents.
    pub(crate) fn any(&mut self) -> Option<(u8, &'a [u8])> {
        let (&tag, rest) = self.0.split_first()?;
        if tag & 0x1f == 0x1f {
            return None; // high tag numbers do not occur in the structures we read
        }
        let (&first, rest) = rest.split_first()?;
        let (len, rest) = if first < 0x80 {
            (usize::from(first), rest)
        } else {
            // 0x80 (indefinite length) is BER, not DER.
            let n = usize::from(first & 0x7f);
            if n == 0 || n > 4 || rest.len() < n {
                return None;
            }
            let len = rest[..n]
                .iter()
                .fold(0usize, |acc, &b| acc << 8 | usize::from(b));
            (len, &rest[n..])
        };
        if rest.len() < len {
            return None;
        }
        let (content, rest) = rest.split_at(len);
        self.0 = rest;
        Some((tag, content))
    }

    /// Reads the next element, which must have `tag`.
    pub(crate) fn read(&mut self, tag: u8) -> Option<&'a [u8]> {
        match self.any()? {
            (t, content) if t == tag => Some(content),
            _ => None,
        }
    }

    /// Reads the next element only if it has `tag`.
    pub(crate) fn optional(&mut self, tag: u8) -> Option<&'a [u8]> {
        if self.0.first() == Some(&tag) {
            self.read(tag)
        } else {
            None
        }
    }
}

/// Returns the encapsulated content of a CMS SignedData message (the plist of a
/// `.mobileprovision`). The signature is not verified.
pub(crate) fn cms_content(bytes: &[u8]) -> Option<Vec<u8>> {
    let mut info = Reader::new(Reader::new(bytes).read(SEQUENCE)?);
    if info.read(OID)? != OID_SIGNED_DATA {
        return None;
    }
    let mut signed = Reader::new(Reader::new(info.read(context(0))?).read(SEQUENCE)?);
    signed.read(INTEGER)?; // version
    signed.read(SET)?; // digest algorithms
    let mut encap = Reader::new(signed.read(SEQUENCE)?);
    encap.read(OID)?; // content type
    match Reader::new(encap.read(context(0))?).any()? {
        (OCTET_STRING, data) => Some(data.to_vec()),
        (OCTET_STRING_CONSTRUCTED, chunks) => {
            let mut chunks = Reader::new(chunks);
            let mut out = Vec::new();
            while !chunks.is_empty() {
                out.extend_from_slice(chunks.read(OCTET_STRING)?);
            }
            Some(out)
        }
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::{cms, tlv};

    #[test]
    fn reads_short_and_long_lengths() {
        let long = vec![7u8; 300];
        let data = [tlv(INTEGER, &[1]), tlv(OCTET_STRING, &long)].concat();
        let mut r = Reader::new(&data);
        assert_eq!(r.read(INTEGER), Some(&[1u8][..]));
        assert_eq!(r.optional(SET), None);
        assert_eq!(r.read(OCTET_STRING).map(<[u8]>::len), Some(300));
        assert!(r.is_empty());
    }

    #[test]
    fn rejects_truncated_and_indefinite() {
        assert!(Reader::new(&[0x30, 0x05, 0x01]).any().is_none());
        assert!(Reader::new(&[0x30, 0x80, 0x00, 0x00]).any().is_none());
        assert!(Reader::new(&[0x30, 0x84, 0xff]).any().is_none());
    }

    #[test]
    fn extracts_cms_content() {
        assert_eq!(cms_content(&cms(b"hello")).as_deref(), Some(&b"hello"[..]));
        assert_eq!(cms_content(b"<?xml"), None);
        assert_eq!(cms_content(&tlv(SEQUENCE, &tlv(OID, &[1, 2]))), None);
    }

    #[test]
    fn extracts_chunked_cms_content() {
        let chunks = [tlv(OCTET_STRING, b"hel"), tlv(OCTET_STRING, b"lo")].concat();
        let blob = crate::testutil::cms_with_content(&tlv(OCTET_STRING_CONSTRUCTED, &chunks));
        assert_eq!(cms_content(&blob).as_deref(), Some(&b"hello"[..]));
    }
}
