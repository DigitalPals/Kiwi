//! The App Store Connect API key (`AuthKey_<id>.p8`, a PKCS#8 P-256 key) and the ES256
//! tokens it signs.

use std::time::{Duration, SystemTime, UNIX_EPOCH};

use base64::Engine;
use base64::engine::general_purpose::{STANDARD, URL_SAFE_NO_PAD as B64};
use p256::ecdsa::signature::Signer;
use p256::ecdsa::{Signature, SigningKey};

use crate::{Error, Result};

/// Tokens may live at most 20 minutes; Kiwi makes a new one per request.
pub const TOKEN_LIFETIME: Duration = Duration::from_secs(15 * 60);
const AUDIENCE: &str = "appstoreconnect-v1";

const SEQUENCE: u8 = 0x30;
const INTEGER: u8 = 0x02;
const OCTET_STRING: u8 = 0x04;
const OID: u8 = 0x06;
/// 1.2.840.10045.2.1 (ecPublicKey)
const OID_EC: &[u8] = &[0x2a, 0x86, 0x48, 0xce, 0x3d, 0x02, 0x01];
/// 1.2.840.10045.3.1.7 (prime256v1)
const OID_P256: &[u8] = &[0x2a, 0x86, 0x48, 0xce, 0x3d, 0x03, 0x01, 0x07];

/// One DER element: (tag, content, rest).
fn element(input: &[u8]) -> Option<(u8, &[u8], &[u8])> {
    let (&tag, rest) = input.split_first()?;
    let (&first, rest) = rest.split_first()?;
    let (len, rest) = if first < 0x80 {
        (usize::from(first), rest)
    } else {
        let n = usize::from(first & 0x7f);
        if n == 0 || n > 4 || rest.len() < n {
            return None;
        }
        let len = rest[..n]
            .iter()
            .fold(0usize, |acc, b| (acc << 8) | usize::from(*b));
        (len, &rest[n..])
    };
    (rest.len() >= len).then(|| (tag, &rest[..len], &rest[len..]))
}

fn expect(input: &[u8], tag: u8) -> Option<(&[u8], &[u8])> {
    match element(input)? {
        (t, content, rest) if t == tag => Some((content, rest)),
        _ => None,
    }
}

/// The 32-byte private scalar of a PKCS#8 (`BEGIN PRIVATE KEY`) or SEC1 (`BEGIN EC PRIVATE
/// KEY`) P-256 key.
fn scalar(der: &[u8]) -> Option<Vec<u8>> {
    let (body, _) = expect(der, SEQUENCE)?;
    let (version, rest) = expect(body, INTEGER)?;
    if version == [1] {
        // SEC1 ECPrivateKey.
        let (key, _) = expect(rest, OCTET_STRING)?;
        return Some(key.to_vec());
    }
    if version != [0] {
        return None;
    }
    let (algorithm, rest) = expect(rest, SEQUENCE)?;
    let (oid, params) = expect(algorithm, OID)?;
    if oid != OID_EC {
        return None;
    }
    if let Some((curve, _)) = expect(params, OID)
        && curve != OID_P256
    {
        return None;
    }
    let (inner, _) = expect(rest, OCTET_STRING)?;
    scalar(inner)
}

/// Reads a `.p8` file's text (PEM) or bare base64 DER.
pub fn parse_private_key(text: &str) -> Result<SigningKey> {
    let invalid =
        || Error::Key("The private key is not an App Store Connect API key (.p8 file).".into());
    let base64: String = text
        .lines()
        .map(str::trim)
        .filter(|l| !l.starts_with("-----"))
        .collect();
    let der = STANDARD.decode(base64.as_bytes()).map_err(|_| invalid())?;
    let secret = scalar(&der).ok_or_else(invalid)?;
    SigningKey::from_slice(&secret).map_err(|_| invalid())
}

/// An ES256 JWT for the App Store Connect API (team key: `iss` is the issuer id).
pub fn token(key: &SigningKey, key_id: &str, issuer_id: &str, now: SystemTime) -> String {
    let iat = now.duration_since(UNIX_EPOCH).unwrap_or_default().as_secs();
    let header = serde_json::json!({ "alg": "ES256", "kid": key_id, "typ": "JWT" });
    let claims = serde_json::json!({
        "iss": issuer_id,
        "iat": iat,
        "exp": iat + TOKEN_LIFETIME.as_secs(),
        "aud": AUDIENCE,
    });
    let input = format!(
        "{}.{}",
        B64.encode(header.to_string()),
        B64.encode(claims.to_string())
    );
    let signature: Signature = key.sign(input.as_bytes());
    format!("{input}.{}", B64.encode(signature.to_bytes()))
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use p256::ecdsa::VerifyingKey;
    use p256::ecdsa::signature::Verifier;

    pub const SECRET: [u8; 32] = [
        0x51, 0x9b, 0x42, 0x3d, 0x71, 0x5f, 0x8b, 0x58, 0x1f, 0x4f, 0xa8, 0xee, 0x59, 0xf4, 0x77,
        0x1a, 0x5b, 0x44, 0xc8, 0x13, 0x0b, 0x4e, 0x3e, 0xac, 0xca, 0x54, 0xa5, 0x6d, 0xda, 0x72,
        0xb4, 0x64,
    ];

    fn tlv(tag: u8, content: &[u8]) -> Vec<u8> {
        let mut out = vec![tag];
        if content.len() < 0x80 {
            out.push(content.len() as u8);
        } else {
            out.extend([0x81, content.len() as u8]);
        }
        out.extend_from_slice(content);
        out
    }

    /// A `.p8` file as App Store Connect hands it out.
    pub fn p8() -> String {
        let ec = tlv(
            SEQUENCE,
            &[tlv(INTEGER, &[1]), tlv(OCTET_STRING, &SECRET)].concat(),
        );
        let algorithm = tlv(SEQUENCE, &[tlv(OID, OID_EC), tlv(OID, OID_P256)].concat());
        let der = tlv(
            SEQUENCE,
            &[tlv(INTEGER, &[0]), algorithm, tlv(OCTET_STRING, &ec)].concat(),
        );
        let body = STANDARD.encode(der);
        let lines: Vec<&str> = body
            .as_bytes()
            .chunks(64)
            .map(|c| std::str::from_utf8(c).unwrap())
            .collect();
        format!(
            "-----BEGIN PRIVATE KEY-----\n{}\n-----END PRIVATE KEY-----\n",
            lines.join("\n")
        )
    }

    #[test]
    fn reads_p8_keys() {
        let key = parse_private_key(&p8()).unwrap();
        assert_eq!(key.to_bytes().as_slice(), SECRET);
        // Windows line endings and surrounding whitespace are fine.
        let crlf = format!("  {}  ", p8().replace('\n', "\r\n"));
        assert_eq!(parse_private_key(&crlf).unwrap().to_bytes(), key.to_bytes());
        for bad in [
            "",
            "not base64!",
            "-----BEGIN PRIVATE KEY-----\nAAAA\n-----END PRIVATE KEY-----",
        ] {
            assert!(
                matches!(parse_private_key(bad), Err(Error::Key(_))),
                "{bad}"
            );
        }
    }

    #[test]
    fn signs_verifiable_tokens() {
        let key = parse_private_key(&p8()).unwrap();
        let now = UNIX_EPOCH + Duration::from_secs(1_800_000_000);
        let jwt = token(
            &key,
            "2X9R4HXF34",
            "57246542-96fe-1a63-e053-0824d011072a",
            now,
        );
        let parts: Vec<&str> = jwt.split('.').collect();
        assert_eq!(parts.len(), 3);
        let header: serde_json::Value =
            serde_json::from_slice(&B64.decode(parts[0]).unwrap()).unwrap();
        assert_eq!(header["alg"], "ES256");
        assert_eq!(header["kid"], "2X9R4HXF34");
        let claims: serde_json::Value =
            serde_json::from_slice(&B64.decode(parts[1]).unwrap()).unwrap();
        assert_eq!(claims["iss"], "57246542-96fe-1a63-e053-0824d011072a");
        assert_eq!(claims["aud"], "appstoreconnect-v1");
        assert_eq!(claims["iat"], 1_800_000_000u64);
        assert_eq!(claims["exp"], 1_800_000_900u64);
        let signature = Signature::from_slice(&B64.decode(parts[2]).unwrap()).unwrap();
        VerifyingKey::from(&key)
            .verify(format!("{}.{}", parts[0], parts[1]).as_bytes(), &signature)
            .unwrap();
    }
}
