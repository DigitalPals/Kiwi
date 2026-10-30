//! X.509 certificate details (subject CN/OU, expiry) and PEM/SHA-1 helpers.

use base64::{Engine, prelude::BASE64_STANDARD};
use chrono::{DateTime, NaiveDateTime, Utc};
use sha1::{Digest, Sha1};

use crate::der::{GENERALIZED_TIME, INTEGER, OID, Reader, SEQUENCE, SET, UTC_TIME, context};

const OID_COMMON_NAME: &[u8] = &[0x55, 0x04, 0x03];
const OID_ORG_UNIT: &[u8] = &[0x55, 0x04, 0x0b];

#[derive(Debug, Default, PartialEq)]
pub(crate) struct CertInfo {
    pub common_name: Option<String>,
    /// Apple puts the team id in the subject's organizational unit.
    pub org_unit: Option<String>,
    pub not_after: Option<DateTime<Utc>>,
}

/// Upper-case hex SHA-1 of DER bytes, the form `security` and `codesign` use.
pub(crate) fn sha1_hex(der: &[u8]) -> String {
    hex::encode_upper(Sha1::digest(der))
}

pub(crate) fn parse_certificate(der: &[u8]) -> Option<CertInfo> {
    let mut cert = Reader::new(Reader::new(der).read(SEQUENCE)?);
    let mut tbs = Reader::new(cert.read(SEQUENCE)?);
    tbs.optional(context(0)); // version
    tbs.read(INTEGER)?; // serial number
    tbs.read(SEQUENCE)?; // signature algorithm
    tbs.read(SEQUENCE)?; // issuer
    let mut validity = Reader::new(tbs.read(SEQUENCE)?);
    validity.any()?; // not before
    let not_after = validity
        .any()
        .and_then(|(tag, value)| parse_time(tag, value));
    let mut info = CertInfo {
        not_after,
        ..CertInfo::default()
    };
    let mut names = Reader::new(tbs.read(SEQUENCE)?);
    while !names.is_empty() {
        let mut set = Reader::new(names.read(SET)?);
        while !set.is_empty() {
            let mut attribute = Reader::new(set.read(SEQUENCE)?);
            let oid = attribute.read(OID)?;
            let (_, value) = attribute.any()?;
            let value = Some(String::from_utf8_lossy(value).into_owned());
            match oid {
                OID_COMMON_NAME => info.common_name = value,
                OID_ORG_UNIT => info.org_unit = value,
                _ => {}
            }
        }
    }
    Some(info)
}

fn parse_time(tag: u8, value: &[u8]) -> Option<DateTime<Utc>> {
    let s = std::str::from_utf8(value).ok()?.strip_suffix('Z')?;
    let full = match tag {
        // RFC 5280: two-digit years 50..99 are 19xx, 00..49 are 20xx.
        UTC_TIME if s.len() == 12 => format!("{}{s}", if s < "50" { "20" } else { "19" }),
        GENERALIZED_TIME if s.len() == 14 => s.to_owned(),
        _ => return None,
    };
    NaiveDateTime::parse_from_str(&full, "%Y%m%d%H%M%S")
        .ok()
        .map(|t| t.and_utc())
}

/// Decodes every `CERTIFICATE` block of PEM text.
pub(crate) fn pem_certificates(text: &str) -> Vec<Vec<u8>> {
    text.split("-----BEGIN CERTIFICATE-----")
        .skip(1)
        .filter_map(|block| {
            let (body, _) = block.split_once("-----END CERTIFICATE-----")?;
            let b64: String = body.split_whitespace().collect();
            BASE64_STANDARD.decode(b64).ok()
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::certificate;
    use chrono::TimeZone;

    #[test]
    fn parses_subject_and_expiry() {
        let der = certificate("Apple Development: Test (ABCDE12345)", "TEAM123456", false);
        let info = parse_certificate(&der).unwrap();
        assert_eq!(
            info.common_name.as_deref(),
            Some("Apple Development: Test (ABCDE12345)")
        );
        assert_eq!(info.org_unit.as_deref(), Some("TEAM123456"));
        assert_eq!(
            info.not_after,
            Some(Utc.with_ymd_and_hms(2027, 10, 25, 12, 0, 0).unwrap())
        );
    }

    #[test]
    fn parses_generalized_time() {
        let der = certificate("Long", "TEAM", true);
        let info = parse_certificate(&der).unwrap();
        assert_eq!(
            info.not_after,
            Some(Utc.with_ymd_and_hms(2051, 1, 2, 3, 4, 5).unwrap())
        );
    }

    #[test]
    fn utc_time_century() {
        let t = parse_time(UTC_TIME, b"991231235959Z").unwrap();
        assert_eq!(t, Utc.with_ymd_and_hms(1999, 12, 31, 23, 59, 59).unwrap());
        assert!(parse_time(UTC_TIME, b"2701011200Z").is_none());
    }

    #[test]
    fn rejects_garbage() {
        assert!(parse_certificate(b"not a certificate").is_none());
        assert!(parse_certificate(&[0x30, 0x00]).is_none());
    }

    #[test]
    fn decodes_pem() {
        let der = certificate("A", "B", false);
        let b64 = BASE64_STANDARD.encode(&der);
        let lines: Vec<&str> = b64
            .as_bytes()
            .chunks(64)
            .map(|c| std::str::from_utf8(c).unwrap())
            .collect();
        let pem = format!(
            "SHA-1 hash: X\n-----BEGIN CERTIFICATE-----\n{}\n-----END CERTIFICATE-----\n\
             -----BEGIN CERTIFICATE-----\nbroken",
            lines.join("\n")
        );
        assert_eq!(pem_certificates(&pem), vec![der.clone()]);
        assert_eq!(sha1_hex(&der).len(), 40);
        assert_eq!(sha1_hex(b"abc"), "A9993E364706816ABA3E25717850C26C9CD0D89D");
    }
}
