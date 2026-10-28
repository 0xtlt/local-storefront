//! The npm packages lsf builds extensions with when an app has no `node_modules`: what a
//! Function's entry imports (`@shopify/shopify_function`), and what a Preact checkout UI
//! extension does (`preact`, `@preact/signals`, `@shopify/ui-extensions`), at the versions
//! Shopify's templates install. Only the files those entries reach are kept, with each
//! package's `package.json` and license (all MIT, but `@shopify/shopify_function`: Apache-2.0).
//!
//! `tools/checkout/javascript_packages.mjs` writes them (`crates/checkout/javascript-packages`)
//! and their list (`files`). A package in the app's own `node_modules` comes first, as with
//! esbuild.

mod files;

use std::hash::Hasher;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

pub use files::VERSIONS;

/// A `node_modules` folder that holds the packages: written once per process, in the system's
/// temporary folder, in a folder named after its content (two versions of lsf do not share one).
pub fn root() -> Result<&'static Path, String> {
    static ROOT: OnceLock<Result<PathBuf, String>> = OnceLock::new();
    ROOT.get_or_init(write)
        .as_ref()
        .map(PathBuf::as_path)
        .map_err(Clone::clone)
}

/// Whether `path` is one of these packages' files: the bundle names them as esbuild would, by
/// their path in `node_modules`.
pub fn relative(path: &Path) -> Option<String> {
    let root = root().ok()?;
    let relative = path.strip_prefix(root).ok()?;
    let parts: Vec<String> = relative
        .components()
        .map(|part| part.as_os_str().to_string_lossy().into_owned())
        .collect();
    Some(format!("node_modules/{}", parts.join("/")))
}

fn write() -> Result<PathBuf, String> {
    let mut hash = Fnv::default();
    for (path, bytes) in files::FILES {
        hash.write(path.as_bytes());
        hash.write(bytes);
    }
    let base = std::env::temp_dir().join(format!("lsf-packages-{:016x}", hash.finish()));
    let root = base.join("node_modules");
    let complete = base.join("complete");
    if complete.is_file() {
        return Ok(root);
    }
    let failed = |error: std::io::Error| format!("cannot write lsf's npm packages: {error}");
    // Written aside, then renamed: another lsf may write the same at the same time.
    let partial = base.with_file_name(format!(
        "{}-{}",
        base.file_name().expect("a name").to_string_lossy(),
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&partial);
    for (path, bytes) in files::FILES {
        let file = partial.join("node_modules").join(path);
        std::fs::create_dir_all(file.parent().expect("a folder")).map_err(failed)?;
        std::fs::write(&file, bytes).map_err(failed)?;
    }
    std::fs::write(partial.join("complete"), "").map_err(failed)?;
    if std::fs::rename(&partial, &base).is_err() {
        let _ = std::fs::remove_dir_all(&partial);
        if !complete.is_file() {
            return Err(format!(
                "cannot write lsf's npm packages in {}",
                base.display()
            ));
        }
    }
    Ok(root)
}

/// FNV-1a: the same hash in every build of lsf, unlike the standard library's.
struct Fnv(u64);

impl Default for Fnv {
    fn default() -> Self {
        Fnv(0xcbf2_9ce4_8422_2325)
    }
}

impl Hasher for Fnv {
    fn finish(&self) -> u64 {
        self.0
    }

    fn write(&mut self, bytes: &[u8]) {
        for byte in bytes {
            self.0 ^= u64::from(*byte);
            self.0 = self.0.wrapping_mul(0x0100_0000_01b3);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_file_of_the_packages_is_embedded() {
        let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("javascript-packages/node_modules");
        let mut on_disk = Vec::new();
        let mut stack = vec![dir.clone()];
        while let Some(folder) = stack.pop() {
            for entry in std::fs::read_dir(&folder).unwrap().flatten() {
                let path = entry.path();
                if path.is_dir() {
                    stack.push(path);
                } else {
                    let relative = path.strip_prefix(&dir).unwrap();
                    on_disk.push(relative.to_string_lossy().replace('\\', "/"));
                }
            }
        }
        on_disk.sort();
        let embedded: Vec<&str> = files::FILES.iter().map(|(path, _)| *path).collect();
        assert_eq!(on_disk, embedded, "run `mise run checkout:packages`");
    }

    #[test]
    fn the_packages_are_written_once_as_a_node_modules() {
        let root = root().unwrap();
        assert_eq!(root.file_name().unwrap(), "node_modules");
        for (path, bytes) in files::FILES {
            assert_eq!(&std::fs::read(root.join(path)).unwrap(), bytes, "{path}");
        }
        assert_eq!(
            relative(&root.join("preact/dist/preact.module.js")).unwrap(),
            "node_modules/preact/dist/preact.module.js"
        );
    }
}
