//! The files of the store data a server watches (`--watch`): which changed since it last read
//! them, and so what it reads again. A change of checkout scenarios alone reads the scenarios
//! again, and the shops of the Admin API keep what the backend changed in them; a change of the
//! store's own data seeds them again ([`lsf_core::store::load::role_of`] says which is which).

use std::collections::BTreeMap;
use std::path::Path;
use std::time::SystemTime;

use lsf_core::store::load::{FileRole, role_of};

/// The files of a data directory, by their path in it (`/` between folders): their size and
/// when they were modified. Names that start with a dot are left out, as the store leaves them.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct DataFiles(BTreeMap<String, (u64, u128)>);

/// How a file changed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum How {
    Added,
    Changed,
    Removed,
}

/// A file that changed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Changed {
    pub path: String,
    pub how: How,
}

/// What changed in the data directory, and so what the server reads again.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum DataChange {
    /// Nothing the store reads (`schema/`, `README.md`, another folder): nothing.
    Unread,
    /// Checkout scenarios, and nothing else the store reads: they are read again, on top of
    /// the same store data, and the shops of the Admin API keep their state.
    Scenarios(Vec<Changed>),
    /// The store's own data (and the scenarios that changed with it): everything is read
    /// again, and the shops of the Admin API are seeded again from it.
    Store(Vec<Changed>),
}

impl DataFiles {
    /// The files of `root` as they are now. Nothing for a directory that cannot be read.
    pub(crate) fn read(root: &Path) -> DataFiles {
        fn walk(root: &Path, prefix: &str, files: &mut BTreeMap<String, (u64, u128)>) {
            let Ok(entries) = std::fs::read_dir(root) else {
                return;
            };
            for entry in entries.filter_map(Result::ok) {
                let name = entry.file_name().to_string_lossy().into_owned();
                if name.starts_with('.') {
                    continue;
                }
                let Ok(metadata) = entry.metadata() else {
                    continue;
                };
                let path = format!("{prefix}{name}");
                if metadata.is_dir() {
                    walk(&entry.path(), &format!("{path}/"), files);
                } else {
                    let modified = metadata
                        .modified()
                        .ok()
                        .and_then(|time| time.duration_since(SystemTime::UNIX_EPOCH).ok())
                        .map_or(0, |since| since.as_nanos());
                    files.insert(path, (metadata.len(), modified));
                }
            }
        }
        let mut files = BTreeMap::new();
        walk(root, "", &mut files);
        DataFiles(files)
    }

    /// What changed from these files to `now`, and so what is read again.
    pub(crate) fn change_to(&self, now: &DataFiles) -> DataChange {
        let mut changed: Vec<Changed> = Vec::new();
        for (path, file) in &now.0 {
            match self.0.get(path) {
                None => changed.push(Changed {
                    path: path.clone(),
                    how: How::Added,
                }),
                Some(before) if before != file => changed.push(Changed {
                    path: path.clone(),
                    how: How::Changed,
                }),
                Some(_) => {}
            }
        }
        for path in self.0.keys().filter(|path| !now.0.contains_key(*path)) {
            changed.push(Changed {
                path: path.clone(),
                how: How::Removed,
            });
        }
        changed.retain(|change| role_of(&change.path) != FileRole::Unread);
        if changed
            .iter()
            .any(|change| role_of(&change.path) == FileRole::Store)
        {
            DataChange::Store(changed)
        } else if changed.is_empty() {
            DataChange::Unread
        } else {
            DataChange::Scenarios(changed)
        }
    }
}

/// The files that changed, for a line of the terminal: `checkouts/b2b.json added,
/// store.json changed`, the first three, then how many more.
pub(crate) fn describe(changed: &[Changed]) -> String {
    const SHOWN: usize = 3;
    let mut words: Vec<String> = changed
        .iter()
        .take(SHOWN)
        .map(|change| {
            let how = match change.how {
                How::Added => "added",
                How::Changed => "changed",
                How::Removed => "removed",
            };
            format!("{} {how}", change.path)
        })
        .collect();
    if changed.len() > SHOWN {
        words.push(format!("{} more", changed.len() - SHOWN));
    }
    words.join(", ")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn files(entries: &[(&str, u64, u128)]) -> DataFiles {
        DataFiles(
            entries
                .iter()
                .map(|(path, size, modified)| (path.to_string(), (*size, *modified)))
                .collect(),
        )
    }

    fn change(path: &str, how: How) -> Changed {
        Changed {
            path: path.to_string(),
            how,
        }
    }

    #[test]
    fn a_change_of_scenarios_alone_is_told_from_one_of_the_store_data() {
        let before = files(&[
            ("store.json", 10, 1),
            ("products/tee.json", 20, 1),
            ("checkouts/b2b.json", 30, 1),
            ("README.md", 40, 1),
        ]);
        // A scenario added, another changed, the README rewritten: the scenarios alone.
        let now = files(&[
            ("store.json", 10, 1),
            ("products/tee.json", 20, 1),
            ("checkouts/b2b.json", 31, 2),
            ("checkouts/new.json", 2, 2),
            ("README.md", 41, 2),
        ]);
        assert_eq!(
            before.change_to(&now),
            DataChange::Scenarios(vec![
                change("checkouts/b2b.json", How::Changed),
                change("checkouts/new.json", How::Added),
            ])
        );
        // A product removed with them: the store data.
        let now = files(&[("store.json", 10, 1), ("checkouts/new.json", 2, 2)]);
        assert_eq!(
            before.change_to(&now),
            DataChange::Store(vec![
                change("checkouts/new.json", How::Added),
                change("checkouts/b2b.json", How::Removed),
                change("products/tee.json", How::Removed),
            ])
        );
        // What the store does not read: nothing.
        let now = files(&[
            ("store.json", 10, 1),
            ("products/tee.json", 20, 1),
            ("checkouts/b2b.json", 30, 1),
            ("README.md", 40, 3),
            ("schema/product.schema.json", 5, 3),
        ]);
        assert_eq!(before.change_to(&now), DataChange::Unread);
        // An image of `files/`: the store data (the sizes of its images).
        let now = files(&[
            ("store.json", 10, 1),
            ("products/tee.json", 20, 1),
            ("checkouts/b2b.json", 30, 1),
            ("README.md", 40, 1),
            ("files/logo.png", 7, 3),
        ]);
        assert_eq!(
            before.change_to(&now),
            DataChange::Store(vec![change("files/logo.png", How::Added)])
        );
    }

    #[test]
    fn the_files_that_changed_are_named_three_at_most() {
        let changed = [
            change("checkouts/a.json", How::Added),
            change("store.json", How::Changed),
            change("products/tee.json", How::Removed),
            change("checkouts/b.json", How::Added),
            change("checkouts/c.json", How::Added),
        ];
        assert_eq!(describe(&changed[..1]), "checkouts/a.json added");
        assert_eq!(
            describe(&changed),
            "checkouts/a.json added, store.json changed, products/tee.json removed, 2 more"
        );
    }

    #[test]
    fn the_files_are_read_with_their_path_in_the_directory() {
        let root = std::env::temp_dir().join(format!("lsf-data-files-{}", std::process::id()));
        std::fs::create_dir_all(root.join("checkouts")).unwrap();
        std::fs::create_dir_all(root.join(".git")).unwrap();
        std::fs::write(root.join("store.json"), "{}").unwrap();
        std::fs::write(root.join("checkouts/b2b.json"), "{}").unwrap();
        std::fs::write(root.join(".git/HEAD"), "ref").unwrap();
        std::fs::write(root.join(".DS_Store"), "").unwrap();
        let read = DataFiles::read(&root);
        let paths: Vec<&str> = read.0.keys().map(String::as_str).collect();
        assert_eq!(paths, ["checkouts/b2b.json", "store.json"]);
        std::fs::write(root.join("checkouts/b2b.json"), "{\"title\": \"B2B\"}").unwrap();
        assert_eq!(
            read.change_to(&DataFiles::read(&root)),
            DataChange::Scenarios(vec![change("checkouts/b2b.json", How::Changed)])
        );
        std::fs::remove_dir_all(&root).ok();
    }
}
