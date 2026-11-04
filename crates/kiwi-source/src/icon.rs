//! The Home Screen icon inside an IPA, as a PNG any browser can show.
//!
//! Xcode stores the loose icon files (`AppIcon60x60@2x.png`) in Apple's "CgBI" PNG
//! variant: raw deflate data, BGRA byte order and premultiplied alpha. Browsers reject
//! those, so they are converted to standard PNGs. Icons only in `Assets.car` are not read.

use crate::{Error, Result};
use std::{
    fs::File,
    io::{BufReader, Read, Write},
    path::Path,
};

/// Largest icon file read from an archive.
const MAX_ICON_FILE: u64 = 2 * 1024 * 1024;
/// Largest icon decoded (1024 × 1024 is the App Store size).
const MAX_SIDE: u32 = 1024;
/// Icon files tried per IPA.
const MAX_CANDIDATES: usize = 16;

const SIGNATURE: &[u8; 8] = b"\x89PNG\r\n\x1a\n";

/// The app's largest loose icon as a standard PNG, or `None` when it has none Kiwi can
/// read. Run after [`crate::inspect_ipa`] accepted the archive; blocking.
pub fn ipa_icon(path: &Path) -> Result<Option<Vec<u8>>> {
    let file = File::open(path).map_err(|e| Error::new("Icon", e))?;
    let mut zip = zip::ZipArchive::new(BufReader::new(file)).map_err(|e| Error::new("Icon", e))?;
    let names: Vec<String> = zip.file_names().map(str::to_owned).collect();
    let Some(info) = names.iter().find(|n| {
        let parts: Vec<&str> = n.split('/').collect();
        parts.len() == 3
            && parts[0] == "Payload"
            && parts[1].ends_with(".app")
            && parts[2] == "Info.plist"
    }) else {
        return Ok(None);
    };
    let app_dir = info.trim_end_matches("Info.plist").to_owned();
    let plist = read_entry(&mut zip, info, MAX_ICON_FILE)?;
    crate::artifact::check_plist(&plist).map_err(|e| Error::new("Icon", e))?;
    let dict: plist::Dictionary = plist::from_bytes(&plist).map_err(|e| Error::new("Icon", e))?;
    let prefixes = icon_names(&dict);
    let mut candidates: Vec<&String> = names
        .iter()
        .filter(|n| {
            let Some(file) = n.strip_prefix(&app_dir) else {
                return false;
            };
            if file.contains('/') || !file.to_ascii_lowercase().ends_with(".png") {
                return false;
            }
            if prefixes.is_empty() {
                file.starts_with("AppIcon")
                    || ["Icon.", "Icon@", "Icon-"]
                        .iter()
                        .any(|p| file.starts_with(p))
            } else {
                prefixes.iter().any(|p| file.starts_with(p.as_str()))
            }
        })
        .collect();
    // iPhone icons first, so they win ties with iPad ones.
    candidates.sort_by_key(|n| n.contains("~ipad"));
    let mut best: Option<(u32, Vec<u8>)> = None;
    for name in candidates.into_iter().take(MAX_CANDIDATES) {
        let Ok(bytes) = read_entry(&mut zip, name, MAX_ICON_FILE) else {
            continue;
        };
        let Some(png) = browser_png(&bytes) else {
            continue;
        };
        let Some(width) = header(&png).map(|h| h.width) else {
            continue;
        };
        if best.as_ref().is_none_or(|(w, _)| width > *w) {
            best = Some((width, png));
        }
    }
    Ok(best.map(|(_, png)| png))
}

/// Icon file name prefixes the Info.plist declares (`AppIcon60x60` for
/// `AppIcon60x60@2x.png`).
fn icon_names(dict: &plist::Dictionary) -> Vec<String> {
    let mut names = Vec::new();
    let mut add = |v: Option<&plist::Value>| {
        for name in v
            .and_then(plist::Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(plist::Value::as_string)
        {
            let name = name.strip_suffix(".png").unwrap_or(name);
            if !name.is_empty() && !name.contains('/') && !names.iter().any(|n| n == name) {
                names.push(name.to_owned());
            }
        }
    };
    for key in ["CFBundleIcons", "CFBundleIcons~ipad"] {
        add(dict
            .get(key)
            .and_then(plist::Value::as_dictionary)
            .and_then(|d| d.get("CFBundlePrimaryIcon"))
            .and_then(plist::Value::as_dictionary)
            .and_then(|d| d.get("CFBundleIconFiles")));
    }
    add(dict.get("CFBundleIconFiles"));
    if let Some(name) = dict
        .get("CFBundleIconFile")
        .and_then(plist::Value::as_string)
    {
        let single = plist::Value::Array(vec![plist::Value::String(name.to_owned())]);
        add(Some(&single));
    }
    names
}

fn read_entry<R: Read + std::io::Seek>(
    zip: &mut zip::ZipArchive<R>,
    name: &str,
    max: u64,
) -> Result<Vec<u8>> {
    let entry = zip.by_name(name).map_err(|e| Error::new("Icon", e))?;
    if entry.size() > max || entry.is_symlink() {
        return Err(Error::new("Icon", "Icon file too large"));
    }
    let mut bytes = Vec::with_capacity(entry.size() as usize);
    entry
        .take(max + 1)
        .read_to_end(&mut bytes)
        .map_err(|e| Error::new("Icon", e))?;
    if bytes.len() as u64 > max {
        return Err(Error::new("Icon", "Icon file too large"));
    }
    Ok(bytes)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Header {
    width: u32,
    height: u32,
    bit_depth: u8,
    color_type: u8,
    interlace: u8,
}

struct Chunk<'a> {
    kind: [u8; 4],
    data: &'a [u8],
}

fn chunks(png: &[u8]) -> Option<Vec<Chunk<'_>>> {
    let mut rest = png.strip_prefix(SIGNATURE)?;
    let mut out = Vec::new();
    while rest.len() >= 12 {
        let len = u32::from_be_bytes(rest[..4].try_into().ok()?) as usize;
        let kind: [u8; 4] = rest[4..8].try_into().ok()?;
        let data = rest.get(8..8usize.checked_add(len)?)?;
        out.push(Chunk { kind, data });
        rest = rest.get(12 + len..)?;
        if &kind == b"IEND" {
            break;
        }
    }
    Some(out)
}

/// The IHDR of a PNG (standard or CgBI).
fn header(png: &[u8]) -> Option<Header> {
    let chunk = chunks(png)?.into_iter().find(|c| &c.kind == b"IHDR")?;
    let d = chunk.data;
    if d.len() != 13 {
        return None;
    }
    Some(Header {
        width: u32::from_be_bytes(d[0..4].try_into().ok()?),
        height: u32::from_be_bytes(d[4..8].try_into().ok()?),
        bit_depth: d[8],
        color_type: d[9],
        interlace: d[12],
    })
}

/// A PNG browsers can show: standard PNGs as they are, CgBI ones converted. `None` for
/// anything else, or images larger than [`MAX_SIDE`].
pub fn browser_png(bytes: &[u8]) -> Option<Vec<u8>> {
    let all = chunks(bytes)?;
    let h = header(bytes)?;
    if h.width == 0 || h.height == 0 || h.width > MAX_SIDE || h.height > MAX_SIDE {
        return None;
    }
    if !all.iter().any(|c| &c.kind == b"CgBI") {
        return Some(bytes.to_vec());
    }
    let pixels = decode_cgbi(&all, h)?;
    Some(encode_rgba(h.width, h.height, &pixels))
}

/// RGBA pixels of a CgBI PNG (8-bit RGBA or RGB, not interlaced).
fn decode_cgbi(chunks: &[Chunk<'_>], h: Header) -> Option<Vec<u8>> {
    let channels = match (h.bit_depth, h.color_type, h.interlace) {
        (8, 6, 0) => 4,
        (8, 2, 0) => 3,
        _ => return None,
    };
    let stride = h.width as usize * channels;
    let expected = h.height as usize * (stride + 1);
    let compressed: Vec<u8> = chunks
        .iter()
        .filter(|c| &c.kind == b"IDAT")
        .flat_map(|c| c.data.iter().copied())
        .collect();
    let mut raw = Vec::with_capacity(expected);
    flate2::read::DeflateDecoder::new(compressed.as_slice())
        .take(expected as u64 + 1)
        .read_to_end(&mut raw)
        .ok()?;
    if raw.len() != expected {
        return None;
    }
    let mut pixels = vec![0u8; h.height as usize * stride];
    let mut previous = vec![0u8; stride];
    for (row, line) in raw.chunks_exact(stride + 1).enumerate() {
        let out = &mut pixels[row * stride..(row + 1) * stride];
        out.copy_from_slice(&line[1..]);
        unfilter(line[0], out, &previous, channels)?;
        previous.copy_from_slice(out);
    }
    let mut rgba = Vec::with_capacity(h.width as usize * h.height as usize * 4);
    for px in pixels.chunks_exact(channels) {
        let (b, g, r) = (px[0], px[1], px[2]);
        let a = if channels == 4 { px[3] } else { 255 };
        let un = |c: u8| {
            if a == 0 {
                0
            } else {
                ((c as u32 * 255 + a as u32 / 2) / a as u32).min(255) as u8
            }
        };
        rgba.extend_from_slice(&[un(r), un(g), un(b), a]);
    }
    Some(rgba)
}

/// Reverses a PNG scanline filter in place; `previous` is the unfiltered line above.
fn unfilter(kind: u8, line: &mut [u8], previous: &[u8], bpp: usize) -> Option<()> {
    for i in 0..line.len() {
        let left = if i >= bpp { line[i - bpp] } else { 0 };
        let up = previous[i];
        let up_left = if i >= bpp { previous[i - bpp] } else { 0 };
        let add = match kind {
            0 => 0,
            1 => left,
            2 => up,
            3 => ((left as u16 + up as u16) / 2) as u8,
            4 => paeth(left, up, up_left),
            _ => return None,
        };
        line[i] = line[i].wrapping_add(add);
    }
    Some(())
}

fn paeth(a: u8, b: u8, c: u8) -> u8 {
    let p = a as i16 + b as i16 - c as i16;
    let (pa, pb, pc) = (
        (p - a as i16).abs(),
        (p - b as i16).abs(),
        (p - c as i16).abs(),
    );
    if pa <= pb && pa <= pc {
        a
    } else if pb <= pc {
        b
    } else {
        c
    }
}

fn encode_rgba(width: u32, height: u32, rgba: &[u8]) -> Vec<u8> {
    let stride = width as usize * 4;
    let mut encoder = flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::default());
    for line in rgba.chunks_exact(stride) {
        // Writing to a Vec cannot fail.
        let _ = encoder.write_all(&[0]);
        let _ = encoder.write_all(line);
    }
    let data = encoder.finish().unwrap_or_default();
    let mut ihdr = Vec::with_capacity(13);
    ihdr.extend_from_slice(&width.to_be_bytes());
    ihdr.extend_from_slice(&height.to_be_bytes());
    ihdr.extend_from_slice(&[8, 6, 0, 0, 0]);
    let mut out = SIGNATURE.to_vec();
    for (kind, data) in [(b"IHDR", ihdr.as_slice()), (b"IDAT", &data), (b"IEND", &[])] {
        out.extend_from_slice(&(data.len() as u32).to_be_bytes());
        out.extend_from_slice(kind);
        out.extend_from_slice(data);
        let mut crc = crc32fast::Hasher::new();
        crc.update(kind);
        crc.update(data);
        out.extend_from_slice(&crc.finalize().to_be_bytes());
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn chunk(out: &mut Vec<u8>, kind: &[u8; 4], data: &[u8]) {
        out.extend_from_slice(&(data.len() as u32).to_be_bytes());
        out.extend_from_slice(kind);
        out.extend_from_slice(data);
        out.extend_from_slice(&[0; 4]);
    }

    /// A 3 × 2 CgBI PNG: premultiplied BGRA, rows filtered with Sub and Paeth.
    pub(crate) fn cgbi() -> Vec<u8> {
        // Straight RGBA: red, half-transparent green, transparent; blue, white, grey.
        let premultiplied_bgra: [[u8; 12]; 2] = [
            [0, 0, 255, 255, 0, 128, 0, 128, 0, 0, 0, 0],
            [255, 0, 0, 255, 255, 255, 255, 255, 64, 64, 64, 255],
        ];
        let mut raw = Vec::new();
        let mut previous = [0u8; 12];
        for (row, line) in premultiplied_bgra.iter().enumerate() {
            let kind = [1u8, 4][row];
            raw.push(kind);
            for i in 0..12 {
                let left = if i >= 4 { line[i - 4] } else { 0 };
                let up_left = if i >= 4 { previous[i - 4] } else { 0 };
                let predictor = match kind {
                    1 => left,
                    _ => paeth(left, previous[i], up_left),
                };
                raw.push(line[i].wrapping_sub(predictor));
            }
            previous = *line;
        }
        let mut deflate =
            flate2::write::DeflateEncoder::new(Vec::new(), flate2::Compression::default());
        deflate.write_all(&raw).unwrap();
        let mut png = SIGNATURE.to_vec();
        chunk(&mut png, b"CgBI", &[0x50, 0, 0x20, 0x06]);
        chunk(&mut png, b"IHDR", &[0, 0, 0, 3, 0, 0, 0, 2, 8, 6, 0, 0, 0]);
        chunk(&mut png, b"IDAT", &deflate.finish().unwrap());
        chunk(&mut png, b"IEND", &[]);
        png
    }

    fn decode_standard(png: &[u8]) -> (Header, Vec<u8>) {
        let h = header(png).unwrap();
        let data: Vec<u8> = chunks(png)
            .unwrap()
            .iter()
            .filter(|c| &c.kind == b"IDAT")
            .flat_map(|c| c.data.to_vec())
            .collect();
        let mut raw = Vec::new();
        flate2::read::ZlibDecoder::new(data.as_slice())
            .read_to_end(&mut raw)
            .unwrap();
        let pixels = raw
            .chunks_exact(h.width as usize * 4 + 1)
            .flat_map(|l| {
                assert_eq!(l[0], 0);
                l[1..].to_vec()
            })
            .collect();
        (h, pixels)
    }

    #[test]
    fn converts_cgbi_to_a_standard_png() {
        let png = browser_png(&cgbi()).unwrap();
        assert!(png.starts_with(SIGNATURE));
        assert!(chunks(&png).unwrap().iter().all(|c| &c.kind != b"CgBI"));
        // The CRCs are real.
        let ihdr = &png[8..8 + 25];
        let mut crc = crc32fast::Hasher::new();
        crc.update(&ihdr[4..21]);
        assert_eq!(crc.finalize().to_be_bytes(), ihdr[21..25]);
        let (h, pixels) = decode_standard(&png);
        assert_eq!((h.width, h.height, h.color_type), (3, 2, 6));
        assert_eq!(
            pixels,
            [
                255, 0, 0, 255, 0, 255, 0, 128, 0, 0, 0, 0, //
                0, 0, 255, 255, 255, 255, 255, 255, 64, 64, 64, 255
            ]
        );
        // Standard PNGs pass through.
        assert_eq!(browser_png(&png).unwrap(), png);
    }

    #[test]
    fn rejects_what_it_cannot_show() {
        assert!(browser_png(b"<html>").is_none());
        assert!(browser_png(SIGNATURE).is_none());
        let mut huge = cgbi();
        // Width 4096.
        huge[8 + 12 + 8..8 + 12 + 12].copy_from_slice(&4096u32.to_be_bytes());
        assert!(browser_png(&huge).is_none());
        let mut truncated = cgbi();
        truncated.truncate(truncated.len() - 20);
        assert!(browser_png(&truncated).is_none());
    }

    #[test]
    fn reads_the_declared_icon_from_an_ipa() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("app.ipa");
        let mut zip = zip::ZipWriter::new(File::create(&path).unwrap());
        let options = zip::write::SimpleFileOptions::default();
        zip.start_file("Payload/A.app/Info.plist", options).unwrap();
        zip.write_all(
            br#"<?xml version="1.0" encoding="UTF-8"?><plist version="1.0"><dict>
<key>CFBundleIcons</key><dict><key>CFBundlePrimaryIcon</key><dict>
<key>CFBundleIconFiles</key><array><string>AppIcon60x60</string></array></dict></dict>
</dict></plist>"#,
        )
        .unwrap();
        zip.start_file("Payload/A.app/icon_button.png", options)
            .unwrap();
        zip.write_all(&encode_rgba(8, 8, &[0; 256])).unwrap();
        zip.start_file("Payload/A.app/AppIcon60x60@2x.png", options)
            .unwrap();
        zip.write_all(&cgbi()).unwrap();
        zip.finish().unwrap();
        let icon = ipa_icon(&path).unwrap().unwrap();
        assert_eq!(header(&icon).unwrap().width, 3);
    }
}
