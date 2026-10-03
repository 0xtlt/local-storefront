//! Cached access to the files of a theme directory.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, RwLock};
use std::time::{Duration, Instant, SystemTime};

/// How often what is cached about a file (its content, whether it exists, its directory's
/// listing) is checked against the file system.
#[derive(Clone, Copy, Debug)]
pub enum Revalidate {
    /// Re-check the modification time, at most once per interval. Suits development, where
    /// theme files change while the server runs.
    Every(Duration),
    /// Read each file once. Suits test runs against a theme that does not change.
    Never,
}

struct Entry {
    content: Option<Arc<str>>,
    modified: Option<SystemTime>,
    checked: Instant,
}

/// Something read from the file system (a directory listing, whether a file exists), and when.
struct Checked<T> {
    value: T,
    checked: Instant,
}

type CheckedCache<T> = RwLock<HashMap<String, Checked<T>>>;

struct Version {
    modified: Option<SystemTime>,
    version: u64,
    checked: Instant,
}

pub struct ThemeFiles {
    root: PathBuf,
    revalidate: Revalidate,
    /// When this was created: what `expired` counts from.
    opened: Instant,
    /// Nanoseconds after `opened` before which what was checked is not to be trusted.
    expired: AtomicU64,
    cache: RwLock<HashMap<String, Entry>>,
    // Rendering a page asks for listings, existence and versions hundreds of times. Each one
    // is a system call, and the kernel serves those on one directory one at a time, so they
    // are cached like file contents: concurrent renders must not queue on the file system.
    listings: CheckedCache<Arc<[String]>>,
    existence: CheckedCache<bool>,
    versions: RwLock<HashMap<String, Version>>,
}

impl ThemeFiles {
    pub fn new(root: impl Into<PathBuf>, revalidate: Revalidate) -> Self {
        ThemeFiles {
            root: root.into(),
            revalidate,
            opened: Instant::now(),
            expired: AtomicU64::new(0),
            cache: RwLock::new(HashMap::new()),
            listings: RwLock::new(HashMap::new()),
            existence: RwLock::new(HashMap::new()),
            versions: RwLock::new(HashMap::new()),
        }
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    /// Resolves a theme-relative path, refusing anything that escapes the theme directory.
    pub fn resolve(&self, relative: &str) -> Option<PathBuf> {
        let relative = Path::new(relative);
        let escapes = relative.components().any(|component| {
            !matches!(
                component,
                std::path::Component::Normal(_) | std::path::Component::CurDir
            )
        });
        (!escapes).then(|| self.root.join(relative))
    }

    /// Whether something checked at `checked` can be used without looking at the file system.
    fn is_fresh(&self, checked: Instant) -> bool {
        match self.revalidate {
            Revalidate::Never => true,
            Revalidate::Every(interval) => {
                checked.elapsed() < interval && !self.expired_since(checked)
            }
        }
    }

    /// Says that the files changed: everything is checked against the file system on its next
    /// use, without waiting for the interval. For whoever sees a change before the interval is
    /// over, like live reload, which renders the page again at once. Files that are read only
    /// once (`Revalidate::Never`) stay as they were read.
    pub fn expire(&self) {
        let now = self.opened.elapsed().as_nanos() as u64;
        self.expired.fetch_max(now.max(1), Ordering::Relaxed);
    }

    /// Whether [`ThemeFiles::expire`] was called after `checked`.
    pub fn expired_since(&self, checked: Instant) -> bool {
        let checked = checked.saturating_duration_since(self.opened).as_nanos() as u64;
        checked < self.expired.load(Ordering::Relaxed)
    }

    /// The cached answer for `key` while it is fresh, otherwise what `look` finds now.
    fn checked<T: Clone>(&self, cache: &CheckedCache<T>, key: &str, look: impl FnOnce() -> T) -> T {
        if let Ok(cache) = cache.read()
            && let Some(entry) = cache.get(key)
            && self.is_fresh(entry.checked)
        {
            return entry.value.clone();
        }
        let value = look();
        if let Ok(mut cache) = cache.write() {
            cache.insert(
                key.to_string(),
                Checked {
                    value: value.clone(),
                    checked: Instant::now(),
                },
            );
        }
        value
    }

    /// The content of a theme file (`snippets/price.liquid`), or `None` when it does not exist.
    /// The returned `Arc` is stable while the file is unchanged, so it doubles as a cache key.
    pub fn read(&self, relative: &str) -> Option<Arc<str>> {
        if let Ok(cache) = self.cache.read()
            && let Some(entry) = cache.get(relative)
            && self.is_fresh(entry.checked)
        {
            return entry.content.clone();
        }
        let path = self.resolve(relative)?;
        let modified = std::fs::metadata(&path)
            .ok()
            .and_then(|meta| meta.modified().ok());
        let mut cache = self.cache.write().ok()?;
        if let Some(entry) = cache.get_mut(relative)
            && entry.modified == modified
        {
            entry.checked = Instant::now();
            return entry.content.clone();
        }
        let content: Option<Arc<str>> = match modified {
            Some(_) => std::fs::read_to_string(&path).ok().map(Arc::from),
            None => None,
        };
        cache.insert(
            relative.to_string(),
            Entry {
                content: content.clone(),
                modified,
                checked: Instant::now(),
            },
        );
        content
    }

    pub fn exists(&self, relative: &str) -> bool {
        let Some(path) = self.resolve(relative) else {
            return false;
        };
        self.checked(&self.existence, relative, || path.is_file())
    }

    /// The file names directly inside a theme directory, sorted.
    pub fn list(&self, directory: &str) -> Arc<[String]> {
        let Some(path) = self.resolve(directory) else {
            return Arc::default();
        };
        self.checked(&self.listings, directory, || {
            let mut names: Vec<String> = std::fs::read_dir(path)
                .into_iter()
                .flatten()
                .filter_map(|entry| entry.ok())
                .filter(|entry| entry.file_type().is_ok_and(|kind| kind.is_file()))
                .filter_map(|entry| entry.file_name().into_string().ok())
                .filter(|name| !name.starts_with('.'))
                .collect();
            names.sort();
            names.into()
        })
    }

    /// A number that changes when the file's content changes, used for `?v=` cache busting.
    /// It derives from the content rather than the modification time, so that the same theme
    /// renders the same HTML on every machine.
    pub fn version(&self, relative: &str) -> u64 {
        let Some(path) = self.resolve(relative) else {
            return 0;
        };
        if let Ok(versions) = self.versions.read()
            && let Some(cached) = versions.get(relative)
            && self.is_fresh(cached.checked)
        {
            return cached.version;
        }
        let modified = std::fs::metadata(&path)
            .ok()
            .and_then(|meta| meta.modified().ok());
        if let Ok(mut versions) = self.versions.write()
            && let Some(cached) = versions.get_mut(relative)
            && cached.modified == modified
        {
            cached.checked = Instant::now();
            return cached.version;
        }
        let version = match (modified, std::fs::read(&path)) {
            (Some(_), Ok(bytes)) => {
                // FNV-1a over the content, folded into the range of a Unix timestamp.
                let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
                for byte in &bytes {
                    hash ^= u64::from(*byte);
                    hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
                }
                1_600_000_000 + hash % 100_000_000
            }
            _ => 0,
        };
        if let Ok(mut versions) = self.versions.write() {
            versions.insert(
                relative.to_string(),
                Version {
                    modified,
                    version,
                    checked: Instant::now(),
                },
            );
        }
        version
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A theme directory with one snippet. Then a second snippet appears and the first changes.
    fn changing_theme(name: &str, revalidate: Revalidate) -> (PathBuf, ThemeFiles, u64) {
        let root = std::env::temp_dir().join(format!("lsf-files-{name}-{}", std::process::id()));
        std::fs::create_dir_all(root.join("snippets")).unwrap();
        std::fs::write(root.join("snippets/a.liquid"), "a").unwrap();
        let files = ThemeFiles::new(root.clone(), revalidate);
        assert_eq!(*files.list("snippets"), ["a.liquid".to_string()]);
        assert!(files.exists("snippets/a.liquid"));
        assert!(!files.exists("snippets/b.liquid"));
        let version = files.version("snippets/a.liquid");

        std::fs::write(root.join("snippets/b.liquid"), "b").unwrap();
        std::fs::write(root.join("snippets/a.liquid"), "changed").unwrap();
        (root, files, version)
    }

    #[test]
    fn static_files_are_looked_at_once() {
        let (root, files, version) = changing_theme("static", Revalidate::Never);
        assert_eq!(*files.list("snippets"), ["a.liquid".to_string()]);
        assert!(!files.exists("snippets/b.liquid"));
        assert_eq!(files.version("snippets/a.liquid"), version);
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn expired_files_are_looked_at_again_before_their_interval() {
        let (root, files, version) =
            changing_theme("expired", Revalidate::Every(Duration::from_secs(60)));
        // Within the interval, the file system is not looked at.
        assert!(!files.exists("snippets/b.liquid"));
        assert_eq!(*files.list("snippets"), ["a.liquid".to_string()]);
        assert_eq!(files.version("snippets/a.liquid"), version);

        files.expire();
        assert_eq!(files.read("snippets/b.liquid").as_deref(), Some("b"));
        assert_eq!(
            *files.list("snippets"),
            ["a.liquid".to_string(), "b.liquid".to_string()]
        );
        assert!(files.exists("snippets/b.liquid"));
        assert_ne!(files.version("snippets/a.liquid"), version);
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn revalidated_files_follow_the_file_system() {
        let (root, files, _) = changing_theme("revalidated", Revalidate::Every(Duration::ZERO));
        assert_eq!(
            *files.list("snippets"),
            ["a.liquid".to_string(), "b.liquid".to_string()]
        );
        assert!(files.exists("snippets/b.liquid"));
        let _ = std::fs::remove_dir_all(root);
    }
}
