//! Synthetic fixtures: DER, certificates, profiles, Mach-O binaries and IPAs. Other crates'
//! tests get them with the `test-support` feature.
#![allow(missing_docs, clippy::missing_panics_doc)]

use std::io::{Cursor, Write};

use chrono::TimeZone;
use plist::{Dictionary, Value};
use zip::{ZipWriter, write::SimpleFileOptions};

use crate::{
    Identity, Profile,
    cert::sha1_hex,
    der::{GENERALIZED_TIME, INTEGER, OCTET_STRING, OID, SEQUENCE, SET, UTC_TIME, context},
    parse_profile,
};

pub const TEAM: &str = "TEAM123456";

pub fn tlv(tag: u8, content: &[u8]) -> Vec<u8> {
    let len = content.len();
    let mut out = vec![tag];
    if len < 0x80 {
        out.push(len as u8);
    } else {
        let bytes: Vec<u8> = len
            .to_be_bytes()
            .into_iter()
            .skip_while(|&b| b == 0)
            .collect();
        out.push(0x80 | bytes.len() as u8);
        out.extend(bytes);
    }
    out.extend_from_slice(content);
    out
}

/// CMS SignedData whose encapsulated content is the given (already encoded) element.
pub fn cms_with_content(econtent: &[u8]) -> Vec<u8> {
    let data_oid = [0x2a, 0x86, 0x48, 0x86, 0xf7, 0x0d, 0x01, 0x07, 0x01];
    let signed_oid = [0x2a, 0x86, 0x48, 0x86, 0xf7, 0x0d, 0x01, 0x07, 0x02];
    let encap = tlv(
        SEQUENCE,
        &[tlv(OID, &data_oid), tlv(context(0), econtent)].concat(),
    );
    let signed = tlv(
        SEQUENCE,
        &[tlv(INTEGER, &[1]), tlv(SET, &[]), encap, tlv(SET, &[])].concat(),
    );
    tlv(
        SEQUENCE,
        &[tlv(OID, &signed_oid), tlv(context(0), &signed)].concat(),
    )
}

pub fn cms(content: &[u8]) -> Vec<u8> {
    cms_with_content(&tlv(OCTET_STRING, content))
}

fn name_attribute(oid: &[u8], tag: u8, value: &str) -> Vec<u8> {
    tlv(
        SET,
        &tlv(
            SEQUENCE,
            &[tlv(OID, oid), tlv(tag, value.as_bytes())].concat(),
        ),
    )
}

pub fn certificate(common_name: &str, org_unit: &str, generalized: bool) -> Vec<u8> {
    certificate_with(common_name, org_unit, generalized, None)
}

/// A certificate with the given (already encoded) extensions.
pub fn certificate_with_extensions(
    common_name: &str,
    org_unit: &str,
    extensions: &[u8],
) -> Vec<u8> {
    certificate_with(common_name, org_unit, false, Some(extensions))
}

fn certificate_with(
    common_name: &str,
    org_unit: &str,
    generalized: bool,
    extensions: Option<&[u8]>,
) -> Vec<u8> {
    let alg = tlv(
        SEQUENCE,
        &tlv(OID, &[0x2a, 0x86, 0x48, 0x86, 0xf7, 0x0d, 0x01, 0x01, 0x0b]),
    );
    let issuer = tlv(
        SEQUENCE,
        &name_attribute(&[0x55, 0x04, 0x03], 0x0c, "Test WWDR"),
    );
    let not_after = if generalized {
        tlv(GENERALIZED_TIME, b"20510102030405Z")
    } else {
        tlv(UTC_TIME, b"271025120000Z")
    };
    let validity = tlv(
        SEQUENCE,
        &[tlv(UTC_TIME, b"251025120000Z"), not_after].concat(),
    );
    let subject = tlv(
        SEQUENCE,
        &[
            name_attribute(
                &[0x09, 0x92, 0x26, 0x89, 0x93, 0xf2, 0x2c, 0x64, 0x01, 0x01],
                0x0c,
                "USER123456",
            ),
            name_attribute(&[0x55, 0x04, 0x03], 0x0c, common_name),
            name_attribute(&[0x55, 0x04, 0x0b], 0x13, org_unit),
            name_attribute(&[0x55, 0x04, 0x0a], 0x0c, "Test User"),
        ]
        .concat(),
    );
    let tbs = tlv(
        SEQUENCE,
        &[
            tlv(context(0), &tlv(INTEGER, &[2])),
            tlv(INTEGER, &[0x01, 0x23]),
            alg.clone(),
            issuer,
            validity,
            subject,
            tlv(SEQUENCE, &[]),
            extensions.map_or_else(Vec::new, |e| tlv(context(3), &tlv(SEQUENCE, e))),
        ]
        .concat(),
    );
    tlv(SEQUENCE, &[tbs, alg, tlv(0x03, &[0, 1, 2, 3])].concat())
}

pub fn identity_certificate() -> Vec<u8> {
    certificate("Apple Development: Test User (USER123456)", TEAM, false)
}

pub fn identity() -> Identity {
    Identity {
        sha1: sha1_hex(&identity_certificate()),
        name: "Apple Development: Test User (USER123456)".into(),
        team_id: Some(TEAM.into()),
        expires_at: Some(
            chrono::Utc
                .with_ymd_and_hms(2027, 10, 25, 12, 0, 0)
                .unwrap(),
        ),
    }
}

pub struct ProfileSpec {
    /// DER of the certificate the profile allows (default: [`identity_certificate`]).
    pub certificate: Option<Vec<u8>>,
    pub uuid: String,
    pub name: String,
    pub app_id: Option<String>,
    pub expires: &'static str,
    pub platforms: Vec<&'static str>,
    pub development: bool,
    pub entitlements: Vec<(&'static str, Value)>,
}

impl ProfileSpec {
    pub fn wildcard() -> Self {
        Self {
            certificate: None,
            uuid: "00000000-0000-4000-8000-000000000001".into(),
            name: "iOS Team Provisioning Profile: *".into(),
            app_id: Some(format!("{TEAM}.*")),
            expires: "2027-10-26T18:52:06Z",
            platforms: vec!["iOS", "xrOS"],
            development: true,
            entitlements: Vec::new(),
        }
    }

    pub fn explicit(bundle_id: &str) -> Self {
        Self {
            uuid: format!("explicit-{bundle_id}"),
            name: format!("iOS Team Provisioning Profile: {bundle_id}"),
            app_id: Some(format!("{TEAM}.{bundle_id}")),
            ..Self::wildcard()
        }
    }
}

pub fn date(s: &str) -> Value {
    Value::Date(plist::Date::from_xml_format(s).unwrap())
}

pub fn profile_bytes(spec: &ProfileSpec) -> Vec<u8> {
    let mut ent = Dictionary::new();
    if let Some(app_id) = &spec.app_id {
        ent.insert("application-identifier".into(), app_id.clone().into());
    }
    ent.insert(
        "keychain-access-groups".into(),
        Value::Array(vec![format!("{TEAM}.*").into(), "com.apple.token".into()]),
    );
    ent.insert("get-task-allow".into(), spec.development.into());
    ent.insert("com.apple.developer.team-identifier".into(), TEAM.into());
    for (key, value) in &spec.entitlements {
        ent.insert((*key).into(), value.clone());
    }
    let strings = |items: &[&str]| Value::Array(items.iter().map(|s| (*s).into()).collect());
    let mut root = Dictionary::new();
    root.insert("AppIDName".into(), "XC Wildcard".into());
    root.insert("CreationDate".into(), date("2026-10-26T18:52:06Z"));
    root.insert("Platform".into(), strings(&spec.platforms));
    root.insert(
        "DeveloperCertificates".into(),
        Value::Array(vec![
            Value::Data(
                spec.certificate
                    .clone()
                    .unwrap_or_else(identity_certificate),
            ),
            Value::Data(b"other".to_vec()),
        ]),
    );
    root.insert("Entitlements".into(), ent.into());
    root.insert("ExpirationDate".into(), date(spec.expires));
    root.insert("Name".into(), spec.name.clone().into());
    root.insert(
        "ProvisionedDevices".into(),
        strings(&[
            "00000000-0000000000000001",
            "0000000000000000000000000000000000000002",
        ]),
    );
    root.insert("TeamIdentifier".into(), strings(&[TEAM]));
    root.insert("TeamName".into(), "Test User".into());
    root.insert("UUID".into(), spec.uuid.clone().into());
    root.insert("Version".into(), 1.into());
    let mut xml = Vec::new();
    Value::Dictionary(root).to_writer_xml(&mut xml).unwrap();
    cms(&xml)
}

pub fn profile(spec: &ProfileSpec) -> Profile {
    parse_profile(&profile_bytes(spec)).unwrap()
}

pub fn entitlements_xml(entries: &[(&str, Value)]) -> Vec<u8> {
    let dict: Dictionary = entries
        .iter()
        .map(|(k, v)| ((*k).to_owned(), v.clone()))
        .collect();
    let mut xml = Vec::new();
    Value::Dictionary(dict).to_writer_xml(&mut xml).unwrap();
    xml
}

/// Thin little-endian 64-bit Mach-O, with an embedded signature carrying
/// `entitlements` when given.
pub fn macho(cputype: u32, entitlements: Option<&[u8]>) -> Vec<u8> {
    let le = |v: u32| v.to_le_bytes();
    let be = |v: u32| v.to_be_bytes();
    // LC_UUID (24 bytes) then LC_CODE_SIGNATURE (16 bytes).
    let mut cmds = [le(0x1b), le(24)].concat();
    cmds.extend([0u8; 16]);
    let sig_offset = 0x100u32;
    let signature = entitlements.map(|xml| {
        let directory = [be(0xfade_0c02), be(8)].concat();
        let ent = [&be(0xfade_7171)[..], &be(8 + xml.len() as u32), xml].concat();
        let header_len = 12 + 2 * 8;
        let mut blob = [be(0xfade_0cc0), be(0), be(2)].concat();
        blob.extend([be(0), be(header_len)].concat());
        blob.extend([be(5), be(header_len + directory.len() as u32)].concat());
        blob.extend(directory);
        blob.extend(ent);
        let len = blob.len() as u32;
        blob[4..8].copy_from_slice(&be(len));
        blob
    });
    let ncmds = if let Some(sig) = &signature {
        cmds.extend([le(0x1d), le(16), le(sig_offset), le(sig.len() as u32)].concat());
        2
    } else {
        1
    };
    let mut out = [
        le(0xfeed_facf),
        le(cputype),
        le(0),
        le(2),
        le(ncmds),
        le(cmds.len() as u32),
        le(0),
        le(0),
    ]
    .concat();
    out.extend(cmds);
    out.resize(sig_offset as usize, 0);
    if let Some(sig) = signature {
        out.extend(sig);
    }
    out
}

/// Universal binary of the given slices (32-bit fat header, 0x1000 alignment).
pub fn fat(slices: &[(u32, Vec<u8>)]) -> Vec<u8> {
    let be = |v: u32| v.to_be_bytes();
    let mut out = [be(0xcafe_babe), be(slices.len() as u32)].concat();
    let mut offset = 0x1000u32;
    let mut body = Vec::new();
    for (cputype, slice) in slices {
        out.extend(
            [
                be(*cputype),
                be(0),
                be(offset),
                be(slice.len() as u32),
                be(12),
            ]
            .concat(),
        );
        body.resize((offset - 0x1000) as usize, 0);
        body.extend(slice);
        offset = 0x1000 + body.len().next_multiple_of(0x1000) as u32;
    }
    out.resize(0x1000, 0);
    out.extend(body);
    out
}

pub fn plist_bytes(dict: Dictionary, binary: bool) -> Vec<u8> {
    let mut out = Vec::new();
    if binary {
        Value::Dictionary(dict).to_writer_binary(&mut out).unwrap();
    } else {
        Value::Dictionary(dict).to_writer_xml(&mut out).unwrap();
    }
    out
}

/// Builds zip archives entry by entry.
pub struct Zip(ZipWriter<Cursor<Vec<u8>>>);

impl Default for Zip {
    fn default() -> Self {
        Self::new()
    }
}

impl Zip {
    pub fn new() -> Self {
        Self(ZipWriter::new(Cursor::new(Vec::new())))
    }

    pub fn file(mut self, name: &str, data: &[u8], mode: u32) -> Self {
        let options = SimpleFileOptions::default().unix_permissions(mode);
        self.0.start_file(name, options).unwrap();
        self.0.write_all(data).unwrap();
        self
    }

    pub fn dir(mut self, name: &str) -> Self {
        self.0
            .add_directory(name, SimpleFileOptions::default().unix_permissions(0o755))
            .unwrap();
        self
    }

    pub fn symlink(mut self, name: &str, target: &str) -> Self {
        self.0
            .add_symlink(name, target, SimpleFileOptions::default())
            .unwrap();
        self
    }

    pub fn finish(self) -> Vec<u8> {
        self.0.finish().unwrap().into_inner()
    }
}

/// A self-signed Apple Development style identity, exported as Keychain Access does.
#[cfg(not(target_os = "macos"))]
pub struct TestIdentity {
    pub p12: Vec<u8>,
    /// DER of the certificate.
    pub certificate: Vec<u8>,
    /// PKCS#8 DER of the private key.
    pub private_key: Vec<u8>,
}

/// Creates a code signing certificate for `team` (its organizational unit) with a new P-256
/// key, valid for a year, in a `.p12` protected by `password`.
#[cfg(not(target_os = "macos"))]
pub fn signing_identity(team: &str, password: &str) -> TestIdentity {
    signing_identity_with(team, password, false)
}

/// Like [`signing_identity`], with a 2048-bit RSA key (as Apple issues) when `rsa`.
#[cfg(not(target_os = "macos"))]
pub fn signing_identity_with(team: &str, password: &str, rsa: bool) -> TestIdentity {
    use apple_codesign::{CertificateProfile, create_self_signed_code_signing_certificate};
    use x509_certificate::{EcdsaCurve, KeyAlgorithm};
    let algorithm = if rsa {
        KeyAlgorithm::Rsa
    } else {
        KeyAlgorithm::Ecdsa(EcdsaCurve::Secp256r1)
    };
    let (cert, key) = create_self_signed_code_signing_certificate(
        algorithm,
        CertificateProfile::AppleDevelopment,
        team,
        "Kiwi Test",
        "NL",
        chrono::Duration::days(365),
    )
    .unwrap();
    let certificate = cert.encode_der().unwrap();
    let private_key = key.to_pkcs8_one_asymmetric_key_der().to_vec();
    TestIdentity {
        p12: p12_with(&certificate, &private_key, password),
        certificate,
        private_key,
    }
}

/// A `.p12` with one certificate and one key (legacy encryption, SHA-1 MAC).
#[cfg(not(target_os = "macos"))]
pub fn p12_with(certificate: &[u8], private_key: &[u8], password: &str) -> Vec<u8> {
    p12::PFX::new(certificate, private_key, None, password, "Kiwi Test")
        .unwrap()
        .to_der()
}

/// A structurally complete arm64 iOS Mach-O (segments, `__LINKEDIT`, build version) that
/// signers can sign: an executable, or a dylib when `dylib`.
#[cfg(not(target_os = "macos"))]
pub fn ios_macho(dylib: bool) -> Vec<u8> {
    use apple_codesign::{MachoTarget, Platform, macho_builder::MachOBuilder};
    const MH_EXECUTE: u32 = 2;
    const MH_DYLIB: u32 = 6;
    MachOBuilder::new_aarch64(if dylib { MH_DYLIB } else { MH_EXECUTE })
        .macho_target(MachoTarget {
            platform: Platform::IOs,
            minimum_os_version: semver::Version::new(16, 0, 0),
            sdk_version: semver::Version::new(18, 0, 0),
        })
        .write_macho()
        .unwrap()
}
