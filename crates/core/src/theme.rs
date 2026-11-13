//! A theme as files on disk: the directories it is made of, cached access to its files, the
//! files the Admin API writes over them, and which theme of the shop it is.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, RwLock};
use std::time::{Duration, Instant, SystemTime};

/// The theme of the shop the storefront serves, as `Shopify.theme`, the `theme` object and the
/// asset URLs (`/cdn/shop/t/<id>/…`) name it: the shop's main theme. Without one, the theme
/// directory: [`crate::urls::THEME_ID`], the name of its `config/settings_schema.json`, `main`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ThemeIdentity {
    /// The number of its `OnlineStoreTheme` GID.
    pub id: u64,
    /// Its name in the admin; `None` for the name of the files' `theme_info`.
    pub name: Option<String>,
    /// `main`, as `Shopify.theme.role` writes it.
    pub role: String,
}

/// The directories a theme is made of.
pub const THEME_DIRECTORIES: [&str; 8] = [
    "assets",
    "blocks",
    "config",
    "layout",
    "locales",
    "sections",
    "snippets",
    "templates",
];

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

/// The path of `relative` in the theme directory `root`, or `None` when it could name something
/// else: a component that is not a plain name (`..`, a root, a drive), or a name with a
/// backslash, a colon, NUL or a control character. Those are refused on every system, so that
/// what a path reads is the same on all of them: on Windows `\` separates components and
/// `a.css:x` is a stream of `a.css`.
pub fn resolve_in(root: &Path, relative: &str) -> Option<PathBuf> {
    let refused = |character: char| matches!(character, '\\' | ':') || character.is_control();
    if relative.chars().any(refused) {
        return None;
    }
    let relative = Path::new(relative);
    let escapes = relative.components().any(|component| {
        !matches!(
            component,
            std::path::Component::Normal(_) | std::path::Component::CurDir
        )
    });
    (!escapes).then(|| root.join(relative))
}

/// Whether a theme file is Liquid or JSON: text that Shopify CLI uploads as UTF-8, a byte that
/// is not UTF-8 becoming U+FFFD.
fn is_theme_text(relative: &str) -> bool {
    relative.ends_with(".liquid") || relative.ends_with(".json")
}

/// The text of the file at `path`. A Liquid or JSON file that is not UTF-8 (a Latin-1 `é`) is
/// read as Shopify stores it: each byte that is not UTF-8 is U+FFFD. Other files are text only
/// when they are UTF-8.
fn read_text(path: &Path, relative: &str) -> Option<Arc<str>> {
    let bytes = std::fs::read(path).ok()?;
    match String::from_utf8(bytes) {
        Ok(text) => Some(Arc::from(text)),
        Err(error) if is_theme_text(relative) => Some(Arc::from(
            String::from_utf8_lossy(error.as_bytes()).as_ref(),
        )),
        Err(_) => None,
    }
}

/// The line of the first byte of `bytes` that is not UTF-8, `None` when they are UTF-8: a
/// theme file that was saved in another encoding.
pub fn first_non_utf8_line(bytes: &[u8]) -> Option<usize> {
    let error = std::str::from_utf8(bytes).err()?;
    let valid = &bytes[..error.valid_up_to()];
    Some(valid.iter().filter(|byte| **byte == b'\n').count() + 1)
}

/// Files that are served in place of those of the directory, or on top of them: what the
/// Admin API wrote into the theme the storefront serves (`themeFilesUpsert` on the local
/// theme), or the whole of another theme it published. The directory is never written to.
#[derive(Clone, Debug, Default)]
pub struct FileOverlay {
    /// By theme-relative path: the file's content, or `None` for a file deleted from the
    /// directory's.
    pub files: HashMap<String, Option<OverlayFile>>,
    /// Whether the overlay is the whole theme: the directory's files are not served at all.
    pub exclusive: bool,
    /// What tells one overlay from the next: a new one is applied when it differs.
    pub generation: u64,
}

/// The content of a file of an overlay.
#[derive(Clone, Debug)]
pub struct OverlayFile {
    pub bytes: Arc<[u8]>,
    /// The content as text when it is UTF-8: what Liquid, JSON and locales are read from. The
    /// same `Arc` for the life of the overlay, so it doubles as a cache key, as a file's does.
    pub text: Option<Arc<str>>,
}

impl OverlayFile {
    pub fn new(bytes: impl Into<Arc<[u8]>>) -> OverlayFile {
        let bytes: Arc<[u8]> = bytes.into();
        let text = std::str::from_utf8(&bytes).ok().map(Arc::from);
        OverlayFile { bytes, text }
    }
}

impl FileOverlay {
    /// What the overlay says of a path: `Some(Some(file))` it has the file, `Some(None)` the file
    /// is not there (deleted, or not in an exclusive overlay), `None` the directory says.
    fn entry(&self, relative: &str) -> Option<Option<&OverlayFile>> {
        match self.files.get(relative) {
            Some(file) => Some(file.as_ref()),
            None if self.exclusive => Some(None),
            None => None,
        }
    }
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
    /// The files of the Admin API, on top of the directory's ([`ThemeFiles::set_overlay`]).
    /// `has_overlay` is what a read looks at first: without an overlay, which is the common
    /// case, a read costs one atomic load more than it did before overlays.
    overlay: RwLock<Option<Arc<FileOverlay>>>,
    has_overlay: AtomicBool,
    /// Nanoseconds after `opened` at which the overlay last changed: what the theme's derived
    /// values check, even for files read once.
    overlay_changed: AtomicU64,
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
            overlay: RwLock::new(None),
            has_overlay: AtomicBool::new(false),
            overlay_changed: AtomicU64::new(0),
        }
    }

    /// Serves `overlay` on top of the directory's files (or instead of them, when it is
    /// exclusive), until another replaces it; `None` serves the directory alone again. What
    /// is derived from the files is computed again from then on.
    pub fn set_overlay(&self, overlay: Option<Arc<FileOverlay>>) {
        let present = overlay.is_some();
        if let Ok(mut current) = self.overlay.write() {
            *current = overlay;
        }
        self.has_overlay.store(present, Ordering::Release);
        let now = self.opened.elapsed().as_nanos() as u64;
        self.overlay_changed
            .fetch_max(now.max(1), Ordering::Relaxed);
        self.expire();
    }

    /// The overlay served, if any.
    pub fn overlay(&self) -> Option<Arc<FileOverlay>> {
        if !self.has_overlay.load(Ordering::Acquire) {
            return None;
        }
        self.overlay.read().ok()?.clone()
    }

    /// What the overlay says of `relative`, through `answer` (the file, or `None` for a file it
    /// deletes or an exclusive overlay lacks); `None` when the directory decides.
    fn in_overlay<T>(
        &self,
        relative: &str,
        answer: impl FnOnce(Option<&OverlayFile>) -> T,
    ) -> Option<T> {
        self.overlay()?.entry(relative).map(answer)
    }

    /// The generation of the overlay served: `0` without one.
    pub fn overlay_generation(&self) -> u64 {
        self.overlay().map_or(0, |overlay| overlay.generation)
    }

    /// Whether the overlay changed after `checked`.
    pub fn overlay_changed_since(&self, checked: Instant) -> bool {
        let checked = checked.saturating_duration_since(self.opened).as_nanos() as u64;
        checked < self.overlay_changed.load(Ordering::Relaxed)
    }

    /// The bytes of a file: the overlay's, else the directory's. What a file that is not text
    /// (an image of `assets/`) is served from.
    pub fn bytes(&self, relative: &str) -> Option<Arc<[u8]>> {
        if let Some(bytes) = self.in_overlay(relative, |file| file.map(|file| file.bytes.clone())) {
            return bytes;
        }
        let path = self.resolve(relative)?;
        std::fs::read(path).ok().map(Arc::from)
    }

    /// Whether the overlay decides what a path is: the directory's file at it is not served
    /// (it was written over, deleted, or the overlay is the whole theme).
    pub fn overlaid(&self, relative: &str) -> bool {
        self.in_overlay(relative, |_| ()).is_some()
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    /// Resolves a theme-relative path, refusing anything that escapes the theme directory
    /// ([`resolve_in`]).
    pub fn resolve(&self, relative: &str) -> Option<PathBuf> {
        resolve_in(&self.root, relative)
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
        if let Some(text) =
            self.in_overlay(relative, |file| file.and_then(|file| file.text.clone()))
        {
            return text;
        }
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
            Some(_) => read_text(&path, relative),
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
        if let Some(exists) = self.in_overlay(relative, |file| file.is_some()) {
            return exists;
        }
        let Some(path) = self.resolve(relative) else {
            return false;
        };
        self.checked(&self.existence, relative, || path.is_file())
    }

    /// The file names directly inside a theme directory, sorted: the overlay's files there, and
    /// the directory's that it does not delete.
    pub fn list(&self, directory: &str) -> Arc<[String]> {
        let listed = self.list_directory(directory);
        let Some(overlay) = self.overlay() else {
            return listed;
        };
        let prefix = format!("{}/", directory.trim_end_matches('/'));
        let mut names: Vec<String> = if overlay.exclusive {
            Vec::new()
        } else {
            listed.to_vec()
        };
        for (path, file) in &overlay.files {
            let Some(name) = path
                .strip_prefix(&prefix)
                .filter(|name| !name.contains('/'))
            else {
                continue;
            };
            match file {
                Some(_) if !names.iter().any(|known| known == name) => names.push(name.to_string()),
                None => names.retain(|known| known != name),
                _ => {}
            }
        }
        names.sort();
        names.into()
    }

    /// The file names directly inside a directory of the theme's directory, sorted.
    fn list_directory(&self, directory: &str) -> Arc<[String]> {
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
        if let Some(version) = self.in_overlay(relative, |file| {
            file.map_or(0, |file| content_version(&file.bytes))
        }) {
            return version;
        }
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
            (Some(_), Ok(bytes)) => content_version(&bytes),
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

/// The `?v=` of a file: FNV-1a over its content, folded into the range of a Unix timestamp.
fn content_version(bytes: &[u8]) -> u64 {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for byte in bytes {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    1_600_000_000 + hash % 100_000_000
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_path_resolves_only_to_a_plain_name_in_the_directory_with_either_separator() {
        let root = Path::new("theme");
        assert_eq!(
            resolve_in(root, "assets/base.css"),
            Some(root.join("assets/base.css"))
        );
        assert_eq!(
            resolve_in(root, "./assets/base.css"),
            Some(root.join("./assets/base.css"))
        );
        for escaping in [
            "../x",
            "assets/../../x",
            "assets/..\\..\\x",
            "assets\\..\\..\\x",
            "..\\x",
            "/etc/passwd",
            "\\etc\\passwd",
            "C:\\x",
            "C:x",
            "assets/base.css:stream",
            "assets/a\0b",
            "assets/a\nb",
            "assets/a\u{7f}b",
        ] {
            assert_eq!(resolve_in(root, escaping), None, "{escaping:?}");
        }
    }

    #[test]
    fn a_liquid_or_json_file_that_is_not_utf8_is_read_as_shopify_stores_it() {
        // Latin-1 `é` (0xE9): Shopify CLI uploads it as U+FFFD, and the page shows `caf�`.
        let root = std::env::temp_dir().join(format!("lsf-latin1-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join("snippets")).unwrap();
        std::fs::create_dir_all(root.join("assets")).unwrap();
        std::fs::write(root.join("snippets/latin.liquid"), b"<i>caf\xe9</i>\n").unwrap();
        std::fs::write(root.join("assets/image.png"), b"\x89PNG\xff").unwrap();
        let files = ThemeFiles::new(root.clone(), Revalidate::Never);
        assert_eq!(
            files.read("snippets/latin.liquid").as_deref(),
            Some("<i>caf\u{FFFD}</i>\n")
        );
        // A file that is not Liquid or JSON is not text.
        assert_eq!(files.read("assets/image.png"), None);
        assert_eq!(first_non_utf8_line(b"a\nb\ncaf\xe9"), Some(3));
        assert_eq!(first_non_utf8_line("café".as_bytes()), None);
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn an_overlay_is_read_on_top_of_the_directory_and_never_written_to_it() {
        let (root, files, _) = changing_theme("overlay", Revalidate::Never);
        let mut overlay = FileOverlay {
            generation: 1,
            ..FileOverlay::default()
        };
        overlay.files.insert(
            "snippets/a.liquid".into(),
            Some(OverlayFile::new(b"from the admin".to_vec())),
        );
        overlay.files.insert(
            "snippets/c.liquid".into(),
            Some(OverlayFile::new(b"new".to_vec())),
        );
        overlay.files.insert("snippets/b.liquid".into(), None);
        files.set_overlay(Some(Arc::new(overlay.clone())));
        assert_eq!(
            files.read("snippets/a.liquid").as_deref(),
            Some("from the admin")
        );
        assert_eq!(
            *files.list("snippets"),
            ["a.liquid".to_string(), "c.liquid".to_string()]
        );
        assert!(!files.exists("snippets/b.liquid"));
        assert_eq!(files.overlay_generation(), 1);
        // The directory is as it was.
        assert_eq!(
            std::fs::read_to_string(root.join("snippets/a.liquid")).unwrap(),
            "changed"
        );

        // Exclusive: only the overlay's files are the theme.
        overlay.exclusive = true;
        overlay.generation = 2;
        files.set_overlay(Some(Arc::new(overlay)));
        assert_eq!(
            *files.list("snippets"),
            ["a.liquid".to_string(), "c.liquid".to_string()]
        );
        files.set_overlay(None);
        assert_eq!(files.read("snippets/a.liquid").as_deref(), Some("changed"));
        assert_eq!(files.overlay_generation(), 0);
        let _ = std::fs::remove_dir_all(root);
    }

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
