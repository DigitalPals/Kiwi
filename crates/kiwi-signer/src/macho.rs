//! Reads the entitlements embedded in a Mach-O code signature (what
//! `codesign -d --entitlements -` prints) without running codesign.

use std::{
    fs::File,
    io::{self, Read, Seek, SeekFrom},
    path::Path,
};

use plist::Dictionary;

use crate::{MAX_PLIST_BYTES, check_plist};

const FAT_MAGIC: u32 = 0xcafe_babe;
const FAT_MAGIC_64: u32 = 0xcafe_babf;
const MH_MAGIC: u32 = 0xfeed_face;
const MH_MAGIC_64: u32 = 0xfeed_facf;
const LC_CODE_SIGNATURE: u32 = 0x1d;
const CSMAGIC_EMBEDDED_SIGNATURE: u32 = 0xfade_0cc0;
const CSMAGIC_EMBEDDED_ENTITLEMENTS: u32 = 0xfade_7171;
const CSSLOT_ENTITLEMENTS: u32 = 5;
const CPU_TYPE_ARM64: u32 = 0x0100_000c;
const MAX_LOAD_COMMANDS: u32 = 1 << 24;
const MAX_SIGNATURE: u32 = 256 << 20;

/// The XML entitlements of the binary at `path`. `None` when it is not a Mach-O, is
/// unsigned, or has no (readable) entitlements; prefers the arm64 slice.
pub(crate) fn entitlements(path: &Path) -> io::Result<Option<Dictionary>> {
    let mut file = File::open(path)?;
    let len = file.metadata()?.len();
    let mut head = [0u8; 8];
    if len < 8 {
        return Ok(None);
    }
    file.read_exact(&mut head)?;
    let mut slices = match be32(&head, 0) {
        Some(magic @ (FAT_MAGIC | FAT_MAGIC_64)) => {
            let count = be32(&head, 4).unwrap_or(0).min(32) as usize;
            let entry_size = if magic == FAT_MAGIC { 20 } else { 32 };
            let table = read_at(&mut file, 8, count * entry_size)?;
            table
                .chunks_exact(entry_size)
                .filter_map(|entry| {
                    let cputype = be32(entry, 0)?;
                    let offset = if magic == FAT_MAGIC {
                        u64::from(be32(entry, 8)?)
                    } else {
                        be64(entry, 8)?
                    };
                    Some((cputype, offset))
                })
                .collect()
        }
        _ => vec![(0, 0)],
    };
    slices.sort_by_key(|&(cputype, _)| cputype != CPU_TYPE_ARM64);
    for (_, offset) in slices {
        if let Some(xml) = slice_entitlements(&mut file, offset, len)? {
            if check_plist(&xml).is_err() {
                return Ok(None);
            }
            return Ok(plist::from_bytes(&xml).ok());
        }
    }
    Ok(None)
}

fn slice_entitlements(file: &mut File, base: u64, len: u64) -> io::Result<Option<Vec<u8>>> {
    if base.saturating_add(32) > len {
        return Ok(None);
    }
    let header = read_at(file, base, 32)?;
    let header_size = match le32(&header, 0) {
        Some(MH_MAGIC_64) => 32,
        Some(MH_MAGIC) => 28,
        _ => return Ok(None),
    };
    let (Some(ncmds), Some(cmds_size)) = (le32(&header, 16), le32(&header, 20)) else {
        return Ok(None);
    };
    if cmds_size > MAX_LOAD_COMMANDS || base + header_size + u64::from(cmds_size) > len {
        return Ok(None);
    }
    let cmds = read_at(file, base + header_size, cmds_size as usize)?;
    let mut at = 0usize;
    for _ in 0..ncmds {
        let (Some(cmd), Some(size)) = (le32(&cmds, at), le32(&cmds, at + 4)) else {
            break;
        };
        if cmd == LC_CODE_SIGNATURE {
            let (Some(offset), Some(size)) = (le32(&cmds, at + 8), le32(&cmds, at + 12)) else {
                break;
            };
            let start = base + u64::from(offset);
            if size > MAX_SIGNATURE || start + u64::from(size) > len {
                return Ok(None);
            }
            return superblob_entitlements(file, start, size);
        }
        if size < 8 {
            break;
        }
        at += size as usize;
    }
    Ok(None)
}

/// Seek directly to the XML slot. Code directories and CMS signatures can occupy
/// hundreds of MiB, but are not needed to recover the original entitlements.
fn superblob_entitlements<R: Read + Seek>(
    file: &mut R,
    start: u64,
    len: u32,
) -> io::Result<Option<Vec<u8>>> {
    if len < 12 {
        return Ok(None);
    }
    let header = read_at(file, start, 12)?;
    if be32(&header, 0) != Some(CSMAGIC_EMBEDDED_SIGNATURE) {
        return Ok(None);
    }
    let count = be32(&header, 8).unwrap_or(0).min(64).min((len - 12) / 8);
    let index = read_at(file, start + 12, count as usize * 8)?;
    for slot in index.chunks_exact(8) {
        if be32(slot, 0) != Some(CSSLOT_ENTITLEMENTS) {
            continue;
        }
        let offset = u64::from(be32(slot, 4).unwrap_or(u32::MAX));
        if offset + 8 > u64::from(len) {
            continue;
        }
        let entry = read_at(file, start + offset, 8)?;
        let size = be32(&entry, 4).unwrap_or(0);
        if be32(&entry, 0) != Some(CSMAGIC_EMBEDDED_ENTITLEMENTS)
            || size < 8
            || u64::from(size) > MAX_PLIST_BYTES + 8
            || offset + u64::from(size) > u64::from(len)
        {
            continue;
        }
        return read_at(file, start + offset + 8, size as usize - 8).map(Some);
    }
    Ok(None)
}

fn read_at<R: Read + Seek>(file: &mut R, offset: u64, len: usize) -> io::Result<Vec<u8>> {
    let mut buf = vec![0; len];
    file.seek(SeekFrom::Start(offset))?;
    file.read_exact(&mut buf)?;
    Ok(buf)
}

fn be32(b: &[u8], at: usize) -> Option<u32> {
    Some(u32::from_be_bytes(b.get(at..at + 4)?.try_into().ok()?))
}

fn be64(b: &[u8], at: usize) -> Option<u64> {
    Some(u64::from_be_bytes(b.get(at..at + 8)?.try_into().ok()?))
}

fn le32(b: &[u8], at: usize) -> Option<u32> {
    Some(u32::from_le_bytes(b.get(at..at + 4)?.try_into().ok()?))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::{entitlements_xml, fat, macho};
    use plist::Value;
    use std::io::Write;

    fn read(bytes: &[u8]) -> Option<Dictionary> {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("bin");
        std::fs::write(&path, bytes).unwrap();
        entitlements(&path).unwrap()
    }

    #[test]
    fn reads_thin_binary_entitlements() {
        let xml = entitlements_xml(&[
            ("application-identifier", "OLD.com.example".into()),
            (
                "com.apple.security.application-groups",
                Value::Array(vec!["group.com.example".into()]),
            ),
        ]);
        let ent = read(&macho(CPU_TYPE_ARM64, Some(&xml))).unwrap();
        assert_eq!(
            ent.get("application-identifier").and_then(Value::as_string),
            Some("OLD.com.example")
        );
        assert_eq!(ent.len(), 2);
    }

    #[test]
    fn unsigned_or_foreign_files_have_none() {
        assert_eq!(read(&macho(CPU_TYPE_ARM64, None)), None);
        assert_eq!(read(b"#!/bin/sh\necho not a binary\n"), None);
        assert_eq!(read(b"tiny"), None);
        let mut truncated = macho(CPU_TYPE_ARM64, Some(&entitlements_xml(&[])));
        truncated.truncate(0x108);
        assert_eq!(read(&truncated), None);
    }

    #[test]
    fn reads_only_the_entitlements_from_a_large_signature() {
        struct CountReads {
            file: File,
            bytes: usize,
        }
        impl Read for CountReads {
            fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
                let count = self.file.read(buf)?;
                self.bytes += count;
                Ok(count)
            }
        }
        impl Seek for CountReads {
            fn seek(&mut self, pos: SeekFrom) -> io::Result<u64> {
                self.file.seek(pos)
            }
        }

        let xml = entitlements_xml(&[("application-identifier", "TEAM.example".into())]);
        let binary = macho(CPU_TYPE_ARM64, Some(&xml));
        let mut file = tempfile::tempfile().unwrap();
        file.write_all(&binary).unwrap();
        // A sparse file represents a large code directory without allocating its bytes.
        file.set_len(0x100 + u64::from(MAX_SIGNATURE)).unwrap();
        let mut counted = CountReads { file, bytes: 0 };
        assert_eq!(
            superblob_entitlements(&mut counted, 0x100, MAX_SIGNATURE).unwrap(),
            Some(xml.clone())
        );
        assert_eq!(counted.bytes, 12 + 2 * 8 + 8 + xml.len());
    }

    #[test]
    fn ignores_entitlement_slots_outside_the_signature() {
        let xml = entitlements_xml(&[("ok", true.into())]);
        let binary = macho(CPU_TYPE_ARM64, Some(&xml));
        let len = (binary.len() - 0x100) as u32;
        for offset in [len - 4, len, u32::MAX] {
            let mut malformed = binary.clone();
            malformed[0x118..0x11c].copy_from_slice(&offset.to_be_bytes());
            // Bytes exist after the signature, but they may not be read as part of it.
            malformed.extend_from_slice(&binary);
            assert_eq!(read(&malformed), None);
        }
        for size in [0, 7, len, u32::MAX] {
            let mut malformed = binary.clone();
            malformed[0x128..0x12c].copy_from_slice(&size.to_be_bytes());
            malformed.extend_from_slice(&binary);
            assert_eq!(read(&malformed), None);
        }
    }

    #[test]
    fn hostile_entitlements_are_ignored() {
        let deep = format!(
            "<plist><dict><key>a</key>{}</dict></plist>",
            "<array>".repeat(100_000)
        );
        assert_eq!(read(&macho(CPU_TYPE_ARM64, Some(deep.as_bytes()))), None);
        let mut big = entitlements_xml(&[("a", true.into())]);
        big.resize(MAX_PLIST_BYTES as usize + 1, b' ');
        assert_eq!(read(&macho(CPU_TYPE_ARM64, Some(&big))), None);
    }

    #[test]
    fn prefers_arm64_slice_of_fat_binary() {
        let x86 = entitlements_xml(&[("x86", true.into())]);
        let arm = entitlements_xml(&[("arm64", true.into())]);
        let binary = fat(&[
            (0x0100_0007, macho(0x0100_0007, Some(&x86))),
            (CPU_TYPE_ARM64, macho(CPU_TYPE_ARM64, Some(&arm))),
        ]);
        let ent = read(&binary).unwrap();
        assert!(ent.contains_key("arm64"));
        let unsigned_arm = fat(&[
            (0x0100_0007, macho(0x0100_0007, Some(&x86))),
            (CPU_TYPE_ARM64, macho(CPU_TYPE_ARM64, None)),
        ]);
        assert!(read(&unsigned_arm).unwrap().contains_key("x86"));
    }
}
