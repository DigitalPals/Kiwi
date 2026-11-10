use std::{collections::HashMap, process::Command, sync::atomic::AtomicBool, time::Duration};

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::{
    Error, Result,
    cert::{parse_certificate, pem_certificates, sha1_hex},
    process, require_macos,
};

/// `security` blocks while the keychain waits for an answer to a prompt.
const SECURITY_TIMEOUT: Duration = Duration::from_secs(30);

/// A code signing identity (certificate + private key) in the keychain.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Identity {
    /// Upper-case hex SHA-1 of the certificate, as accepted by `codesign -s`.
    pub sha1: String,
    pub name: String,
    pub team_id: Option<String>,
    pub expires_at: Option<DateTime<Utc>>,
}

/// Lists valid code signing identities with team id and expiry from their certificates.
pub fn list_identities() -> Result<Vec<Identity>> {
    require_macos()?;
    let mut identities =
        parse_find_identity(&security(&["find-identity", "-v", "-p", "codesigning"])?);
    if identities.is_empty() {
        return Ok(identities);
    }
    let certs: HashMap<_, _> = pem_certificates(&security(&["find-certificate", "-a", "-p"])?)
        .iter()
        .filter_map(|der| Some((sha1_hex(der), parse_certificate(der)?)))
        .collect();
    for identity in &mut identities {
        if let Some(cert) = certs.get(&identity.sha1) {
            identity.team_id.clone_from(&cert.org_unit);
            identity.expires_at = cert.not_after;
        }
    }
    Ok(identities)
}

fn security(args: &[&str]) -> Result<String> {
    let out = process::output(
        Command::new("/usr/bin/security").args(args),
        &format!("security {}", args[0]),
        SECURITY_TIMEOUT,
        &AtomicBool::new(false),
    )?;
    if !out.status.success() {
        return Err(Error::Tool(format!(
            "security {} failed: {}",
            args[0],
            String::from_utf8_lossy(&out.stderr).trim()
        )));
    }
    Ok(String::from_utf8_lossy(&out.stdout).into_owned())
}

/// Parses `security find-identity -v` lines like
/// `  1) 45ADD8…0965 "Apple Development: Name (ABCDE12345)"`.
fn parse_find_identity(output: &str) -> Vec<Identity> {
    let mut identities: Vec<Identity> = Vec::new();
    for line in output.lines() {
        let Some((index, rest)) = line.trim().split_once(") ") else {
            continue;
        };
        let Some((sha1, name)) = rest.split_once(' ') else {
            continue;
        };
        let Some(name) = name.strip_prefix('"').and_then(|n| n.strip_suffix('"')) else {
            continue;
        };
        let valid = index.bytes().all(|b| b.is_ascii_digit())
            && sha1.len() == 40
            && sha1.bytes().all(|b| b.is_ascii_hexdigit());
        if valid && !identities.iter().any(|i| i.sha1.eq_ignore_ascii_case(sha1)) {
            identities.push(Identity {
                sha1: sha1.to_ascii_uppercase(),
                name: name.to_owned(),
                team_id: None,
                expires_at: None,
            });
        }
    }
    identities
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_find_identity_output() {
        let out = "  1) 0123456789ABCDEF0123456789ABCDEF01234567 \"Apple Development: Jane Doe (ABCDE12345)\"\n\
                   \x20 2) 89abcdef0123456789abcdef0123456789abcdef \"Apple Distribution: Jane Doe (TEAM123456)\"\n\
                   \x20 3) 0123456789ABCDEF0123456789ABCDEF01234567 \"Apple Development: Jane Doe (ABCDE12345)\"\n\
                   \x20 4) 1111111111111111111111111111111111111111 \"Revoked\" (CSSMERR_TP_CERT_REVOKED)\n\
                   \x20    2 valid identities found\n";
        let ids = parse_find_identity(out);
        assert_eq!(ids.len(), 2);
        assert_eq!(ids[0].sha1, "0123456789ABCDEF0123456789ABCDEF01234567");
        assert_eq!(ids[0].name, "Apple Development: Jane Doe (ABCDE12345)");
        assert_eq!(ids[1].sha1, "89ABCDEF0123456789ABCDEF0123456789ABCDEF");
        assert!(parse_find_identity("     0 valid identities found\n").is_empty());
    }

    #[cfg(not(target_os = "macos"))]
    #[test]
    fn unsupported_off_macos() {
        assert!(matches!(list_identities(), Err(Error::Unsupported(_))));
    }
}
