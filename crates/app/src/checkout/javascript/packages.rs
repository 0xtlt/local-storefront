//! The npm packages lsf builds extensions with when an app has no `node_modules`: what a
//! Function's entry imports (`@shopify/shopify_function`), and what a Preact checkout UI
//! extension does (`preact`, `@preact/signals`, `@shopify/ui-extensions`), at the versions
//! Shopify's templates install. Only the files those entries reach are kept, with each
//! package's `package.json` and license (all MIT, but `@shopify/shopify_function`: Apache-2.0).
//!
//! `tools/checkout/javascript_packages.mjs` writes them (`crates/app/javascript-packages`)
//! and their list (`files`). A package in the app's own `node_modules` comes first, as with
//! esbuild.

mod files;

use std::hash::Hasher;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{Duration, Instant};

pub use files::VERSIONS;

/// How long a check of the files holds: a server that runs for days checks them again, in case
/// something emptied the folder meanwhile.
const RECHECK: Duration = Duration::from_secs(60);

/// A `node_modules` folder that holds the packages, in the user's cache folder
/// (`~/Library/Caches/lsf/` on macOS, `~/.cache/lsf/` on Linux), else the system's temporary
/// folder, in a folder named after its content (two versions of lsf do not share one).
///
/// The folder is used only when each file is there with its bytes: a cleaner of temporary files
/// that deletes the files and keeps the folders (macOS does) leaves a folder that is written
/// again, not one that breaks every build.
pub fn root() -> Result<PathBuf, String> {
    static ROOT: Mutex<Option<(PathBuf, Instant)>> = Mutex::new(None);
    let mut cached = ROOT.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
    if let Some((root, checked)) = cached.as_ref() {
        // Within a minute of a check, the folder is taken as it was. The time is that of the
        // check, not of the last use: a server used every few seconds still checks again.
        if checked.elapsed() < RECHECK {
            return Ok(root.clone());
        }
        if root.parent().is_some_and(|base| complete(base, false)) {
            let root = root.clone();
            *cached = Some((root.clone(), Instant::now()));
            return Ok(root);
        }
    }
    let root = write()?;
    *cached = Some((root.clone(), Instant::now()));
    Ok(root)
}

/// Whether `path` is one of these packages' files: the bundle names them as esbuild would, by
/// their path in `node_modules`.
pub fn relative(path: &Path) -> Option<String> {
    let root = root().ok()?;
    let relative = path.strip_prefix(&root).ok()?;
    let parts: Vec<String> = relative
        .components()
        .map(|part| part.as_os_str().to_string_lossy().into_owned())
        .collect();
    Some(format!("node_modules/{}", parts.join("/")))
}

/// The hash of the packages, which names their folder and is written in its marker.
fn content_hash() -> u64 {
    let mut hash = Fnv::default();
    for (path, bytes) in files::FILES {
        hash.write(path.as_bytes());
        hash.write(bytes);
    }
    hash.finish()
}

/// What the marker of a complete folder holds.
fn marker() -> String {
    format!("{:016x} {}\n", content_hash(), files::FILES.len())
}

/// Whether the folder `base` holds the packages: its marker, then each file, by its size, or
/// by its bytes with `bytes`.
fn complete(base: &Path, bytes: bool) -> bool {
    if std::fs::read_to_string(base.join("complete"))
        .ok()
        .as_deref()
        != Some(&marker())
    {
        return false;
    }
    let root = base.join("node_modules");
    files::FILES.iter().all(|(path, content)| {
        let file = root.join(path);
        if bytes {
            std::fs::read(&file).is_ok_and(|read| read == *content)
        } else {
            std::fs::metadata(&file).is_ok_and(|meta| meta.len() == content.len() as u64)
        }
    })
}

fn write() -> Result<PathBuf, String> {
    let name = format!("packages-{:016x}", content_hash());
    let mut parents = Vec::new();
    if let Some(cache) = lsf_core::util::user_cache_dir() {
        parents.push(cache.join("lsf"));
    }
    parents.push(std::env::temp_dir().join("lsf"));
    let mut error = String::new();
    for parent in parents {
        match install(&parent, &name) {
            Ok(root) => return Ok(root),
            Err(failed) => error = failed,
        }
    }
    Err(error)
}

/// Writes the packages in `parent/name`, unless they are there already. A folder without its
/// marker, or with a file missing or changed, is replaced. Another lsf may do the same at the
/// same time: the files are written aside then renamed, and one lsf at a time replaces a folder
/// (a lock next to it), so that none deletes a folder another just wrote.
fn install(parent: &Path, name: &str) -> Result<PathBuf, String> {
    let base = parent.join(name);
    let root = base.join("node_modules");
    if complete(&base, true) {
        return Ok(root);
    }
    let failed = |error: std::io::Error| {
        format!(
            "cannot write lsf's npm packages in {}: {error}",
            base.display()
        )
    };
    std::fs::create_dir_all(parent).map_err(failed)?;
    let lock = std::fs::File::create(parent.join(format!("{name}.lock"))).map_err(failed)?;
    // A file system without locks writes without one: the rename below still keeps a folder
    // whole.
    let _ = lock.lock();
    if complete(&base, true) {
        return Ok(root);
    }
    let unique = format!(
        "{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |time| time.as_nanos())
    );
    if base.exists() {
        // Moved aside first: a folder that cannot be deleted whole still frees its name.
        let stale = parent.join(format!("{name}.stale-{unique}"));
        if std::fs::rename(&base, &stale).is_ok() {
            let _ = std::fs::remove_dir_all(&stale);
        } else {
            let _ = std::fs::remove_dir_all(&base);
        }
    }
    let partial = parent.join(format!("{name}.partial-{unique}"));
    let written = (|| {
        for (path, bytes) in files::FILES {
            let file = partial.join("node_modules").join(path);
            std::fs::create_dir_all(file.parent().expect("a folder"))?;
            std::fs::write(&file, bytes)?;
        }
        std::fs::write(partial.join("complete"), marker())?;
        std::fs::rename(&partial, &base)
    })();
    if let Err(error) = written {
        let _ = std::fs::remove_dir_all(&partial);
        if !complete(&base, true) {
            return Err(failed(error));
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
        assert!(root.parent().unwrap().join("complete").is_file());
        for (path, bytes) in files::FILES {
            assert_eq!(&std::fs::read(root.join(path)).unwrap(), bytes, "{path}");
        }
        assert_eq!(
            relative(&root.join("preact/dist/preact.module.js")).unwrap(),
            "node_modules/preact/dist/preact.module.js"
        );
    }

    /// macOS's cleaner of temporary files deletes the files and keeps the folders: the folder is
    /// written again, not taken as complete or refused.
    #[test]
    fn a_folder_whose_files_were_deleted_is_written_again() {
        let parent = std::env::temp_dir().join(format!("lsf-packages-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&parent);
        let root = install(&parent, "packages").unwrap();
        let delete_files = |dir: &Path| {
            let mut stack = vec![dir.to_path_buf()];
            while let Some(folder) = stack.pop() {
                for entry in std::fs::read_dir(&folder).unwrap().flatten() {
                    if entry.path().is_dir() {
                        stack.push(entry.path());
                    } else {
                        std::fs::remove_file(entry.path()).unwrap();
                    }
                }
            }
        };
        delete_files(&parent.join("packages"));
        assert!(!complete(&parent.join("packages"), false));
        assert_eq!(install(&parent, "packages").unwrap(), root);
        assert!(complete(&parent.join("packages"), true));

        // One file gone, or changed, with the marker kept.
        let (path, _) = files::FILES[0];
        std::fs::remove_file(root.join(path)).unwrap();
        assert!(!complete(&parent.join("packages"), false));
        install(&parent, "packages").unwrap();
        std::fs::write(root.join(path), b"changed").unwrap();
        install(&parent, "packages").unwrap();
        assert!(complete(&parent.join("packages"), true));

        // A marker of another content.
        std::fs::write(parent.join("packages/complete"), "").unwrap();
        install(&parent, "packages").unwrap();
        assert!(complete(&parent.join("packages"), true));

        // Several lsf at once.
        delete_files(&parent.join("packages"));
        let threads: Vec<_> = (0..4)
            .map(|_| {
                let parent = parent.clone();
                std::thread::spawn(move || install(&parent, "packages").unwrap())
            })
            .collect();
        for thread in threads {
            assert_eq!(thread.join().unwrap(), root);
        }
        assert!(complete(&parent.join("packages"), true));
        let left: Vec<_> = std::fs::read_dir(&parent)
            .unwrap()
            .flatten()
            .map(|entry| entry.file_name().to_string_lossy().into_owned())
            .filter(|name| name != "packages" && name != "packages.lock")
            .collect();
        assert!(left.is_empty(), "{left:?}");
        std::fs::remove_dir_all(&parent).unwrap();
    }
}
