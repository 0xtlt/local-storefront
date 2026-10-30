//! The providers a Function's module imports, which the host links when it runs it: Shopify's
//! Javy plugins (QuickJS, for JavaScript Functions) and the providers of the Function Wasm API
//! (Rust Functions). They are function-runner's (its `providers/`, Apache-2.0), in one Brotli
//! stream (`providers/`, which `mise run checkout:providers` writes): their 9.7 MB, mostly the
//! same QuickJS, take 1.6 MB of lsf's binary. A provider is read from the stream once, the first
//! time a run or a build needs it; the ones the current Functions import come first, so that
//! reading them does not read the others.

use std::io::Read;
use std::sync::Mutex;

const ARCHIVE: &[u8] = include_bytes!("../providers/providers.br");

/// The providers in the order of the stream: name, size, SHA-256.
const INDEX: &str = include_str!("../providers/providers.txt");

/// The provider a module imports as `name` (`shopify_functions_javy_v4`), if lsf has it.
pub fn get(name: &str) -> Option<&'static [u8]> {
    static READ: Mutex<Vec<(&str, &[u8])>> = Mutex::new(Vec::new());
    let (name, start, size) = locate(name)?;
    let mut read = READ.lock().unwrap_or_else(|e| e.into_inner());
    if let Some((_, bytes)) = read.iter().find(|(provider, _)| *provider == name) {
        return Some(bytes);
    }
    let mut stream = brotli::Decompressor::new(ARCHIVE, 1 << 16);
    std::io::copy(&mut (&mut stream).take(start as u64), &mut std::io::sink())
        .expect("lsf's providers decompress");
    let mut bytes = vec![0; size];
    stream
        .read_exact(&mut bytes)
        .expect("lsf's providers decompress");
    // Read once for the whole process.
    let bytes: &'static [u8] = Box::leak(bytes.into_boxed_slice());
    read.push((name, bytes));
    Some(bytes)
}

/// The provider named `name` in the stream: its name, start and size.
fn locate(name: &str) -> Option<(&'static str, usize, usize)> {
    let mut start = 0;
    for (provider, size) in index() {
        if provider == name {
            return Some((provider, start, size));
        }
        start += size;
    }
    None
}

/// The names and sizes of the providers, in the order of the stream.
fn index() -> impl Iterator<Item = (&'static str, usize)> {
    INDEX
        .lines()
        .filter(|line| !line.starts_with('#') && !line.trim().is_empty())
        .map(|line| {
            let mut fields = line.split_whitespace();
            let name = fields.next().expect("a name");
            let size = fields.next().and_then(|size| size.parse().ok());
            (name, size.expect("a size"))
        })
}

#[cfg(test)]
mod tests {
    use super::*;
    use sha2::{Digest, Sha256};

    #[test]
    fn the_providers_are_function_runner_s_to_the_byte() {
        let mut total = 0;
        for line in INDEX.lines().filter(|line| !line.starts_with('#')) {
            let [name, size, sum] = line.split_whitespace().collect::<Vec<_>>()[..] else {
                panic!("{line}");
            };
            let bytes = get(name).unwrap();
            assert_eq!(bytes.len().to_string(), size, "{name}");
            let digest: String = Sha256::digest(bytes)
                .iter()
                .map(|byte| format!("{byte:02x}"))
                .collect();
            assert_eq!(digest, sum, "{name}");
            assert_eq!(&bytes[..4], b"\0asm", "{name}");
            total += bytes.len();
        }
        // Nothing is left in the stream past the last one.
        let mut all = Vec::new();
        brotli::BrotliDecompress(&mut std::io::Cursor::new(ARCHIVE), &mut all).unwrap();
        assert_eq!(all.len(), total);
        // Read once.
        let javy = get("shopify_functions_javy_v4").unwrap();
        assert!(std::ptr::eq(
            javy,
            get("shopify_functions_javy_v4").unwrap()
        ));
        assert!(get("wasi_snapshot_preview1").is_none());
    }
}
