//! Opens an uploaded `.p12` (PKCS#12) signing identity: the certificate, its private key and
//! the [`Identity`] Kiwi shows for it.

use std::panic::{AssertUnwindSafe, catch_unwind};

use apple_codesign::{AppleCodesignError, cryptography::InMemoryPrivateKey};
use x509_certificate::{CapturedX509Certificate, Sign};

use crate::{
    Error, Identity, Result,
    cert::{parse_certificate, sha1_hex},
    der::{OID, Reader, SEQUENCE},
    files::MAX_P12_BYTES,
};

/// SHA-1 (1.3.14.3.2.26), the only MAC the PKCS#12 reader supports.
const OID_SHA1: &[u8] = &[0x2b, 0x0e, 0x03, 0x02, 0x1a];

/// A signing certificate with its private key.
pub(crate) struct SigningKey {
    pub cert: CapturedX509Certificate,
    pub key: InMemoryPrivateKey,
    pub identity: Identity,
    /// Team id from the certificate (its organizational unit).
    pub team_id: String,
}

fn invalid(message: impl Into<String>) -> Error {
    Error::InvalidIdentity(message.into())
}

/// Opens `p12` with `password` and checks it holds a code signing certificate with a team
/// id and the private key that belongs to it. Expiry is not checked here.
pub(crate) fn open(p12: &[u8], password: &str) -> Result<SigningKey> {
    if p12.len() > MAX_P12_BYTES {
        return Err(invalid(format!(
            "The .p12 file is larger than {} KiB",
            MAX_P12_BYTES >> 10
        )));
    }
    if mac_algorithm(p12).is_some_and(|oid| oid != OID_SHA1) {
        return Err(invalid(
            "This .p12 uses AES encryption (the default of OpenSSL 3), which Kiwi can't open. Export it from Keychain Access, or convert it with: openssl pkcs12 -export -keypbe PBE-SHA1-3DES -certpbe PBE-SHA1-3DES -macalg sha1",
        ));
    }
    // The PKCS#12 parser asserts on unexpected structures; this is untrusted input.
    let (cert, key) = catch_unwind(AssertUnwindSafe(|| {
        apple_codesign::cryptography::parse_pfx_data(p12, password).map_err(|e| match e {
            AppleCodesignError::PfxBadPassword => invalid("The password is wrong"),
            AppleCodesignError::PfxParseError(_) => {
                invalid("The file is not a .p12 signing identity, or the password is wrong")
            }
            other => invalid(format!(
                "The private key in the .p12 is not supported ({other})"
            )),
        })
    }))
    .map_err(|_| invalid("Kiwi could not read the .p12 file"))??;
    if key.public_key_data() != cert.public_key_data() {
        return Err(invalid(
            "The private key in the .p12 does not belong to its certificate. Export a single identity: the certificate with its private key.",
        ));
    }
    let der = cert
        .encode_der()
        .map_err(|e| invalid(format!("The certificate could not be read ({e})")))?;
    let info =
        parse_certificate(&der).ok_or_else(|| invalid("The certificate could not be read"))?;
    if info.code_signing != Some(true) {
        return Err(invalid(
            "The certificate in the .p12 is not a code signing certificate. Use an Apple Development or Apple Distribution certificate.",
        ));
    }
    let team_id = info
        .org_unit
        .clone()
        .filter(|ou| !ou.is_empty())
        .ok_or_else(|| {
            invalid("The certificate has no team id. Use an Apple Development or Apple Distribution certificate.")
        })?;
    let identity = Identity {
        sha1: sha1_hex(&der),
        name: info
            .common_name
            .clone()
            .unwrap_or_else(|| format!("Signing identity ({team_id})")),
        team_id: Some(team_id.clone()),
        expires_at: info.not_after,
    };
    Ok(SigningKey {
        cert,
        key,
        identity,
        team_id,
    })
}

/// The OID of the MAC digest of a DER PKCS#12 file; `None` when it can't be read this way
/// (the PKCS#12 parser then decides).
fn mac_algorithm(p12: &[u8]) -> Option<&[u8]> {
    let mut pfx = Reader::new(Reader::new(p12).read(SEQUENCE)?);
    pfx.any()?; // version
    pfx.read(SEQUENCE)?; // authSafe
    let mut mac_data = Reader::new(pfx.read(SEQUENCE)?);
    let mut digest_info = Reader::new(mac_data.read(SEQUENCE)?);
    let mut algorithm = Reader::new(digest_info.read(SEQUENCE)?);
    algorithm.read(OID)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::{TEAM, signing_identity};

    #[test]
    fn opens_a_code_signing_identity() {
        let test = signing_identity(TEAM, "secret");
        let key = open(&test.p12, "secret").unwrap();
        assert_eq!(key.team_id, TEAM);
        assert_eq!(key.identity.team_id.as_deref(), Some(TEAM));
        assert_eq!(key.identity.sha1, sha1_hex(&test.certificate));
        assert_eq!(
            key.identity.name,
            format!("Apple Development: Kiwi Test ({TEAM})")
        );
        assert!(key.identity.expires_at.is_some());
    }

    #[test]
    fn opens_and_signs_with_rsa_identities() {
        // Apple issues RSA keys. (Generating one takes a few seconds in debug builds.)
        let test = crate::testutil::signing_identity_with(TEAM, "", true);
        let key = open(&test.p12, "").unwrap();
        assert!(matches!(key.key, InMemoryPrivateKey::Rsa(_)));
        assert_eq!(key.identity.sha1, sha1_hex(&test.certificate));
        let other = signing_identity(TEAM, "");
        let mismatched = crate::testutil::p12_with(&test.certificate, &other.private_key, "");
        assert!(open(&mismatched, "").is_err());

        use crate::{bundle::CodeKind, sign::Toolchain};
        use std::{path::Path, sync::atomic::AtomicBool};
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("Lib.dylib"),
            crate::testutil::ios_macho(true),
        )
        .unwrap();
        let tools = crate::rcodesign::Rcodesign::new(&key);
        tools
            .sign(
                dir.path(),
                Path::new("Lib.dylib"),
                None,
                &AtomicBool::new(false),
            )
            .unwrap();
        tools
            .check(&dir.path().join("Lib.dylib"), CodeKind::Dylib)
            .unwrap();
    }

    #[test]
    fn explains_unusable_files() {
        let test = signing_identity(TEAM, "secret");
        let err = |p12: &[u8], password: &str| match open(p12, password) {
            Err(e) => e.to_string(),
            Ok(_) => panic!("opened"),
        };
        assert_eq!(err(&test.p12, "wrong"), "The password is wrong");
        assert!(err(b"not a p12", "secret").starts_with("The file is not a .p12"));
        assert!(err(&vec![0; MAX_P12_BYTES + 1], "").contains("larger than"));

        let no_team = signing_identity("", "secret");
        assert!(err(&no_team.p12, "secret").contains("no team id"));

        let mismatched = crate::testutil::p12_with(
            &test.certificate,
            &signing_identity(TEAM, "x").private_key,
            "secret",
        );
        assert!(err(&mismatched, "secret").contains("does not belong to its certificate"));
    }

    #[test]
    fn rejects_aes_encrypted_files_with_advice() {
        // An OpenSSL 3 style PFX: version, authSafe, then a SHA-256 MAC.
        use crate::testutil::tlv;
        let sha256 = [0x60, 0x86, 0x48, 0x01, 0x65, 0x03, 0x04, 0x02, 0x01];
        let mac = tlv(
            SEQUENCE,
            &[
                tlv(
                    SEQUENCE,
                    &[tlv(SEQUENCE, &tlv(OID, &sha256)), tlv(0x04, &[0; 32])].concat(),
                ),
                tlv(0x04, &[1; 8]),
                tlv(0x02, &[0x08, 0x00]),
            ]
            .concat(),
        );
        let pfx = tlv(
            SEQUENCE,
            &[tlv(0x02, &[3]), tlv(SEQUENCE, &[]), mac].concat(),
        );
        let err = open(&pfx, "secret").err().unwrap().to_string();
        assert!(err.contains("AES encryption"), "{err}");
        assert_eq!(mac_algorithm(&pfx), Some(&sha256[..]));
        let test = signing_identity(TEAM, "secret");
        assert_eq!(mac_algorithm(&test.p12), Some(OID_SHA1));
    }
}
