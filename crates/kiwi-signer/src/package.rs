//! Zips a signed `Payload` into an IPA in pure Rust, keeping symlinks and permissions (what
//! `zip -qry` does on macOS).

use std::{
    fs::{self, File},
    io::{self, BufWriter},
    os::unix::fs::PermissionsExt,
    path::Path,
    sync::atomic::{AtomicBool, Ordering},
};

use zip::{CompressionMethod, ZipWriter, write::SimpleFileOptions};

use crate::{Error, Result};

/// Writes `root/Payload` (directories, files with their modes, symlinks as links) to the
/// zip archive `output`, in name order. Checks `cancel` between entries.
#[cfg_attr(target_os = "macos", allow(dead_code))]
pub(crate) fn zip_payload(root: &Path, output: &Path, cancel: &AtomicBool) -> Result<()> {
    let file = File::create(output).map_err(Error::io_path("Could not create", output))?;
    let mut zip = ZipWriter::new(BufWriter::with_capacity(1 << 20, file));
    add_dir(&mut zip, root, Path::new("Payload"), cancel)?;
    zip.finish()
        .map_err(|e| Error::Tool(format!("Packaging the IPA failed: {e}")))?;
    Ok(())
}

fn add_dir<W: io::Write + io::Seek>(
    zip: &mut ZipWriter<W>,
    root: &Path,
    rel: &Path,
    cancel: &AtomicBool,
) -> Result<()> {
    let failed = |e: &dyn std::fmt::Display| {
        Error::Tool(format!(
            "Packaging the IPA failed at {}: {e}",
            rel.display()
        ))
    };
    let dir = root.join(rel);
    zip.add_directory(name(rel)?, options(0o755))
        .map_err(|e| failed(&e))?;
    let mut entries = fs::read_dir(&dir)
        .and_then(|entries| entries.collect::<io::Result<Vec<_>>>())
        .map_err(Error::io_path("Could not read", &dir))?;
    entries.sort_by_key(|e| e.file_name());
    for entry in entries {
        if cancel.load(Ordering::Relaxed) {
            return Err(Error::Cancelled);
        }
        let rel = rel.join(entry.file_name());
        let path = root.join(&rel);
        let meta = fs::symlink_metadata(&path).map_err(Error::io_path("Could not read", &path))?;
        let failed = |e: &dyn std::fmt::Display| {
            Error::Tool(format!(
                "Packaging the IPA failed at {}: {e}",
                rel.display()
            ))
        };
        if meta.is_dir() {
            add_dir(zip, root, &rel, cancel)?;
        } else if meta.is_symlink() {
            let target = fs::read_link(&path).map_err(Error::io_path("Could not read", &path))?;
            let target = target
                .to_str()
                .ok_or_else(|| failed(&"the link is not UTF-8"))?;
            zip.add_symlink(name(&rel)?, target, options(0o755))
                .map_err(|e| failed(&e))?;
        } else if meta.is_file() {
            let options = options(meta.permissions().mode() & 0o777)
                .compression_method(CompressionMethod::Deflated)
                .large_file(meta.len() >= u64::from(u32::MAX));
            zip.start_file(name(&rel)?, options)
                .map_err(|e| failed(&e))?;
            let mut file = File::open(&path).map_err(Error::io_path("Could not read", &path))?;
            io::copy(&mut file, zip).map_err(|e| failed(&e))?;
        }
    }
    Ok(())
}

fn options(mode: u32) -> SimpleFileOptions {
    SimpleFileOptions::default()
        .compression_method(CompressionMethod::Stored)
        .unix_permissions(mode)
}

fn name(rel: &Path) -> Result<String> {
    rel.to_str().map(str::to_owned).ok_or_else(|| {
        Error::InvalidIpa(format!(
            "{} has a name that is not UTF-8",
            rel.to_string_lossy()
        ))
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{io::Read, os::unix::fs::symlink};

    #[test]
    fn keeps_modes_links_and_order() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("root");
        let app = root.join("Payload/Demo.app");
        fs::create_dir_all(app.join("Frameworks/Kit.framework")).unwrap();
        fs::write(app.join("Demo"), vec![7u8; 10_000]).unwrap();
        fs::set_permissions(app.join("Demo"), fs::Permissions::from_mode(0o755)).unwrap();
        fs::write(app.join("Info.plist"), b"plist").unwrap();
        fs::set_permissions(app.join("Info.plist"), fs::Permissions::from_mode(0o644)).unwrap();
        fs::write(app.join("Frameworks/Kit.framework/Kit"), b"kit").unwrap();
        symlink("Frameworks/Kit.framework", app.join("Current")).unwrap();
        let output = dir.path().join("out.ipa");
        zip_payload(&root, &output, &AtomicBool::new(false)).unwrap();

        let mut zip = zip::ZipArchive::new(File::open(&output).unwrap()).unwrap();
        let names: Vec<_> = zip.file_names().map(str::to_owned).collect();
        let mut sorted = names.clone();
        sorted.sort();
        assert_eq!(
            sorted,
            [
                "Payload/",
                "Payload/Demo.app/",
                "Payload/Demo.app/Current",
                "Payload/Demo.app/Demo",
                "Payload/Demo.app/Frameworks/",
                "Payload/Demo.app/Frameworks/Kit.framework/",
                "Payload/Demo.app/Frameworks/Kit.framework/Kit",
                "Payload/Demo.app/Info.plist",
            ]
        );
        let mut demo = zip.by_name("Payload/Demo.app/Demo").unwrap();
        assert_eq!(demo.unix_mode().unwrap() & 0o777, 0o755);
        assert_eq!(demo.compression(), CompressionMethod::Deflated);
        let mut bytes = Vec::new();
        demo.read_to_end(&mut bytes).unwrap();
        assert_eq!(bytes, vec![7u8; 10_000]);
        drop(demo);
        assert_eq!(
            zip.by_name("Payload/Demo.app/Info.plist")
                .unwrap()
                .unix_mode()
                .unwrap()
                & 0o777,
            0o644
        );
        let mut link = zip.by_name("Payload/Demo.app/Current").unwrap();
        assert!(link.is_symlink());
        let mut target = String::new();
        link.read_to_string(&mut target).unwrap();
        assert_eq!(target, "Frameworks/Kit.framework");
        drop(link);

        // What Kiwi extracts from its own output is what it packaged.
        let back = dir.path().join("back");
        let extracted = crate::archive::extract(&output, &back, &AtomicBool::new(false)).unwrap();
        assert_eq!(fs::read(extracted.join("Current/Kit")).unwrap(), b"kit");
    }

    #[test]
    fn cancels() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("root");
        fs::create_dir_all(root.join("Payload/Demo.app")).unwrap();
        fs::write(root.join("Payload/Demo.app/Demo"), b"x").unwrap();
        let err =
            zip_payload(&root, &dir.path().join("out.ipa"), &AtomicBool::new(true)).unwrap_err();
        assert!(matches!(err, Error::Cancelled));
    }
}
