//! The files of a ZIP archive: what `themeCreate` reads a theme from ("a ZIP file hosted at a
//! public URL or files previously uploaded using `stagedUploadsCreate`").
//!
//! Enough of the format (PKWARE's APPNOTE) for a theme: the central directory, entries stored
//! or deflated, no ZIP64, no encryption. What a theme ZIP made by Shopify CLI, GitHub or the
//! admin's "Download theme file" holds. Archives of the theme's directory itself
//! (`dawn-main/layout/theme.liquid`) are read without that first directory.

use std::io::Read;

/// The most an archive may hold once uncompressed: what keeps a hostile archive from filling
/// the memory.
pub const MAX_UNCOMPRESSED: usize = 200 << 20;

/// Why an archive cannot be read.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ZipError {
    /// Not a ZIP archive, or one lsf cannot read.
    Invalid(String),
    /// No file in it.
    Empty,
    /// More than [`MAX_UNCOMPRESSED`] once uncompressed.
    TooLarge,
}

fn u16_at(bytes: &[u8], at: usize) -> Option<u16> {
    Some(u16::from_le_bytes(bytes.get(at..at + 2)?.try_into().ok()?))
}

fn u32_at(bytes: &[u8], at: usize) -> Option<u32> {
    Some(u32::from_le_bytes(bytes.get(at..at + 4)?.try_into().ok()?))
}

/// The files of an archive, `(path, bytes)` in its order, directories left out.
pub fn files(bytes: &[u8]) -> Result<Vec<(String, Vec<u8>)>, ZipError> {
    let invalid = |why: &str| ZipError::Invalid(why.to_string());
    // The end of central directory record: 22 bytes, then a comment of up to 65535.
    let search_from = bytes.len().saturating_sub(22 + 65_535);
    let end = (search_from..bytes.len().saturating_sub(21))
        .rev()
        .find(|&at| u32_at(bytes, at) == Some(0x0605_4b50))
        .ok_or_else(|| invalid("no end of central directory: not a ZIP archive"))?;
    let count = u16_at(bytes, end + 10).ok_or_else(|| invalid("truncated"))? as usize;
    let mut at = u32_at(bytes, end + 16).ok_or_else(|| invalid("truncated"))? as usize;
    let mut files = Vec::new();
    let mut total = 0usize;
    for _ in 0..count {
        if u32_at(bytes, at) != Some(0x0201_4b50) {
            return Err(invalid("a central directory entry is broken"));
        }
        let field = |offset: usize| u16_at(bytes, at + offset).map(usize::from);
        let flags = field(8).ok_or_else(|| invalid("truncated"))?;
        let method = field(10).ok_or_else(|| invalid("truncated"))?;
        let compressed = u32_at(bytes, at + 20).ok_or_else(|| invalid("truncated"))? as usize;
        let size = u32_at(bytes, at + 24).ok_or_else(|| invalid("truncated"))? as usize;
        let name_length = field(28).ok_or_else(|| invalid("truncated"))?;
        let extra_length = field(30).ok_or_else(|| invalid("truncated"))?;
        let comment_length = field(32).ok_or_else(|| invalid("truncated"))?;
        let local = u32_at(bytes, at + 42).ok_or_else(|| invalid("truncated"))? as usize;
        let name = bytes
            .get(at + 46..at + 46 + name_length)
            .ok_or_else(|| invalid("truncated"))?;
        let name = String::from_utf8_lossy(name).replace('\\', "/");
        at += 46 + name_length + extra_length + comment_length;
        if name.ends_with('/') {
            continue;
        }
        if flags & 1 != 0 {
            return Err(invalid("the archive is encrypted"));
        }
        total = total.saturating_add(size);
        if total > MAX_UNCOMPRESSED || size == u32::MAX as usize {
            return Err(ZipError::TooLarge);
        }
        if u32_at(bytes, local) != Some(0x0403_4b50) {
            return Err(invalid("a local file header is broken"));
        }
        let data = local
            + 30
            + u16_at(bytes, local + 26).ok_or_else(|| invalid("truncated"))? as usize
            + u16_at(bytes, local + 28).ok_or_else(|| invalid("truncated"))? as usize;
        let raw = bytes
            .get(data..data + compressed)
            .ok_or_else(|| invalid("an entry is truncated"))?;
        let content = match method {
            0 => raw.to_vec(),
            8 => {
                let mut out = Vec::with_capacity(size);
                flate2::read::DeflateDecoder::new(raw)
                    .take(size as u64 + 1)
                    .read_to_end(&mut out)
                    .map_err(|_| invalid("an entry does not inflate"))?;
                out
            }
            other => {
                return Err(ZipError::Invalid(format!(
                    "{name} is compressed with method {other}, which lsf does not read (stored and deflated only)"
                )));
            }
        };
        files.push((name, content));
    }
    if files.is_empty() {
        return Err(ZipError::Empty);
    }
    Ok(strip_root(files))
}

/// The files without the directory every one of them is in, when the archive is of the theme's
/// directory (`dawn-main/layout/theme.liquid`) rather than of its content.
fn strip_root(files: Vec<(String, Vec<u8>)>) -> Vec<(String, Vec<u8>)> {
    let first = files
        .first()
        .and_then(|(path, _)| path.split_once('/'))
        .map(|(root, _)| format!("{root}/"));
    let Some(root) = first else {
        return files;
    };
    let theme_directory = lsf_core::theme::THEME_DIRECTORIES
        .iter()
        .any(|directory| root.trim_end_matches('/') == *directory);
    if theme_directory || !files.iter().all(|(path, _)| path.starts_with(&root)) {
        return files;
    }
    files
        .into_iter()
        .map(|(path, bytes)| (path[root.len()..].to_string(), bytes))
        .collect()
}

/// An archive of `files`, stored (not compressed): what the tests make themes from.
#[cfg(test)]
pub fn stored(files: &[(&str, &[u8])]) -> Vec<u8> {
    let mut out = Vec::new();
    let mut central = Vec::new();
    for (name, content) in files {
        let offset = out.len() as u32;
        let crc = crc32(content);
        let header = |signature: u32, central: bool| {
            let mut header = Vec::new();
            header.extend_from_slice(&signature.to_le_bytes());
            if central {
                header.extend_from_slice(&20u16.to_le_bytes());
            }
            header.extend_from_slice(&20u16.to_le_bytes());
            header.extend_from_slice(&0u16.to_le_bytes());
            header.extend_from_slice(&0u16.to_le_bytes());
            header.extend_from_slice(&0u32.to_le_bytes());
            header.extend_from_slice(&crc.to_le_bytes());
            header.extend_from_slice(&(content.len() as u32).to_le_bytes());
            header.extend_from_slice(&(content.len() as u32).to_le_bytes());
            header.extend_from_slice(&(name.len() as u16).to_le_bytes());
            header.extend_from_slice(&0u16.to_le_bytes());
            if central {
                header.extend_from_slice(&0u16.to_le_bytes());
                header.extend_from_slice(&0u16.to_le_bytes());
                header.extend_from_slice(&0u16.to_le_bytes());
                header.extend_from_slice(&0u32.to_le_bytes());
                header.extend_from_slice(&offset.to_le_bytes());
            }
            header.extend_from_slice(name.as_bytes());
            header
        };
        out.extend(header(0x0403_4b50, false));
        out.extend_from_slice(content);
        central.extend(header(0x0201_4b50, true));
    }
    let directory_offset = out.len() as u32;
    let directory_size = central.len() as u32;
    out.extend(central);
    out.extend_from_slice(&0x0605_4b50u32.to_le_bytes());
    out.extend_from_slice(&[0, 0, 0, 0]);
    out.extend_from_slice(&(files.len() as u16).to_le_bytes());
    out.extend_from_slice(&(files.len() as u16).to_le_bytes());
    out.extend_from_slice(&directory_size.to_le_bytes());
    out.extend_from_slice(&directory_offset.to_le_bytes());
    out.extend_from_slice(&0u16.to_le_bytes());
    out
}

#[cfg(test)]
fn crc32(bytes: &[u8]) -> u32 {
    let mut crc = 0xffff_ffffu32;
    for byte in bytes {
        crc ^= u32::from(*byte);
        for _ in 0..8 {
            crc = if crc & 1 != 0 {
                (crc >> 1) ^ 0xedb8_8320
            } else {
                crc >> 1
            };
        }
    }
    !crc
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_a_stored_archive_and_strips_its_root_directory() {
        let archive = stored(&[
            (
                "dawn-main/layout/theme.liquid",
                b"<html>{{ content_for_layout }}</html>",
            ),
            ("dawn-main/templates/index.json", b"{}"),
        ]);
        let files = files(&archive).unwrap();
        assert_eq!(
            files
                .iter()
                .map(|(path, _)| path.as_str())
                .collect::<Vec<_>>(),
            ["layout/theme.liquid", "templates/index.json"]
        );
        assert_eq!(files[1].1, b"{}");
    }

    #[test]
    fn reads_a_deflated_entry() {
        use std::io::Write;
        let mut encoder =
            flate2::write::DeflateEncoder::new(Vec::new(), flate2::Compression::default());
        encoder.write_all(b"hello hello hello").unwrap();
        let deflated = encoder.finish().unwrap();
        let mut archive = stored(&[("layout/theme.liquid", &deflated)]);
        // Method 8 (deflate), and the uncompressed size, in both headers.
        archive[8] = 8;
        let central = archive
            .windows(4)
            .position(|window| window == 0x0201_4b50u32.to_le_bytes())
            .unwrap();
        archive[central + 10] = 8;
        let size = (b"hello hello hello".len() as u32).to_le_bytes();
        archive[22..26].copy_from_slice(&size);
        archive[central + 24..central + 28].copy_from_slice(&size);
        assert_eq!(files(&archive).unwrap()[0].1, b"hello hello hello");
    }

    #[test]
    fn refuses_what_is_not_an_archive() {
        assert!(matches!(files(b"not a zip"), Err(ZipError::Invalid(_))));
        assert_eq!(files(&stored(&[])), Err(ZipError::Empty));
    }
}
