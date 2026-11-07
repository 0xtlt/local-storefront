//! The files of the admin an embedded app is previewed in, embedded in the binary: the page
//! (`admin.html`, `admin.css`, `admin.js`), served under `/__lsf/admin/assets/`, and lsf's App
//! Bridge (`app-bridge.js`), which the app's pages load from `/__lsf/admin/app-bridge.js`.
//!
//! Their tests (`assets/tests/admin.test.mjs`, `assets/tests/app-bridge.test.mjs`) are not
//! embedded. `app-bridge.js` is a classic script, not a module: it must run before the rest of
//! the app's page, as Shopify's does, and read `document.currentScript`.

macro_rules! embed {
    ($($name:literal),* $(,)?) => {
        /// The name of every embedded file.
        pub const NAMES: &[&str] = &[$($name),*];

        /// The content of an embedded file, by its name (`admin.js`).
        pub fn get(name: &str) -> Option<&'static [u8]> {
            match name {
                $($name => Some(include_bytes!(concat!("../../assets/admin/", $name)).as_slice()),)*
                _ => None,
            }
        }
    };
}

embed!("admin.css", "admin.html", "admin.js", "app-bridge.js");

/// The content type of an embedded file.
pub fn content_type(name: &str) -> &'static str {
    match name.rsplit_once('.').map(|(_, extension)| extension) {
        Some("js") => "text/javascript; charset=utf-8",
        Some("css") => "text/css; charset=utf-8",
        Some("html") => "text/html; charset=utf-8",
        _ => "application/octet-stream",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_file_of_the_admin_is_embedded() {
        let directory = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("assets/admin");
        let mut on_disk: Vec<String> = std::fs::read_dir(&directory)
            .expect("the assets/admin directory")
            .filter_map(Result::ok)
            .filter(|entry| entry.path().is_file())
            .map(|entry| entry.file_name().to_string_lossy().into_owned())
            .collect();
        on_disk.sort();
        let mut embedded: Vec<&str> = NAMES.to_vec();
        embedded.sort();
        assert_eq!(on_disk, embedded);
        for name in NAMES {
            assert!(!get(name).expect(name).is_empty(), "{name}");
        }
        assert!(get("../checkout.js").is_none());
    }

    #[test]
    fn app_bridge_is_a_classic_script() {
        let source = String::from_utf8(get("app-bridge.js").unwrap().to_vec()).unwrap();
        for line in source.lines() {
            let line = line.trim_start();
            assert!(
                !line.starts_with("import ") && !line.starts_with("export "),
                "app-bridge.js is loaded as a classic script: {line}"
            );
        }
    }
}
