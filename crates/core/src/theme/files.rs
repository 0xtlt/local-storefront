//! Cached access to the files of a theme directory.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, RwLock};
use std::time::{Duration, Instant, SystemTime};

/// How often a cached file is checked against the file system.
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

pub struct ThemeFiles {
    root: PathBuf,
    revalidate: Revalidate,
    cache: RwLock<HashMap<String, Entry>>,
    versions: RwLock<HashMap<String, (Option<SystemTime>, u64)>>,
}

impl ThemeFiles {
    pub fn new(root: impl Into<PathBuf>, revalidate: Revalidate) -> Self {
        ThemeFiles {
            root: root.into(),
            revalidate,
            cache: RwLock::new(HashMap::new()),
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

    fn is_fresh(&self, entry: &Entry) -> bool {
        match self.revalidate {
            Revalidate::Never => true,
            Revalidate::Every(interval) => entry.checked.elapsed() < interval,
        }
    }

    /// The content of a theme file (`snippets/price.liquid`), or `None` when it does not exist.
    /// The returned `Arc` is stable while the file is unchanged, so it doubles as a cache key.
    pub fn read(&self, relative: &str) -> Option<Arc<str>> {
        if let Ok(cache) = self.cache.read()
            && let Some(entry) = cache.get(relative)
            && self.is_fresh(entry)
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
        self.resolve(relative).is_some_and(|path| path.is_file())
    }

    /// The file names directly inside a theme directory, sorted.
    pub fn list(&self, directory: &str) -> Vec<String> {
        let Some(path) = self.resolve(directory) else {
            return Vec::new();
        };
        let mut names: Vec<String> = std::fs::read_dir(path)
            .into_iter()
            .flatten()
            .filter_map(|entry| entry.ok())
            .filter(|entry| entry.file_type().is_ok_and(|kind| kind.is_file()))
            .filter_map(|entry| entry.file_name().into_string().ok())
            .filter(|name| !name.starts_with('.'))
            .collect();
        names.sort();
        names
    }

    /// A number that changes when the file's content changes, used for `?v=` cache busting.
    /// It derives from the content rather than the modification time, so that the same theme
    /// renders the same HTML on every machine.
    pub fn version(&self, relative: &str) -> u64 {
        let Some(path) = self.resolve(relative) else {
            return 0;
        };
        let modified = std::fs::metadata(&path)
            .ok()
            .and_then(|meta| meta.modified().ok());
        if let Ok(versions) = self.versions.read()
            && let Some((cached_modified, version)) = versions.get(relative)
            && *cached_modified == modified
        {
            return *version;
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
            versions.insert(relative.to_string(), (modified, version));
        }
        version
    }
}
