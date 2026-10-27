//! The files of the checkout page and of the sandbox its extensions run in, embedded in the
//! binary. The server serves them under `/__lsf/checkout/assets/`.
//!
//! The tests of these scripts (`assets/tests/`) and the preview (`assets/preview/`) are not
//! embedded. The scripts of the sandbox are classic scripts, not modules: the frame has an
//! opaque origin, which cannot load modules.

macro_rules! embed {
    ($($name:literal),* $(,)?) => {
        /// The name of every embedded file.
        pub const NAMES: &[&str] = &[$($name),*];

        /// The content of an embedded file, by its name (`checkout.js`).
        pub fn get(name: &str) -> Option<&'static [u8]> {
            match name {
                $($name => Some(include_bytes!(concat!("../assets/", $name)).as_slice()),)*
                _ => None,
            }
        }
    };
}

embed!(
    "api-versions.js",
    "checkout.css",
    "checkout.html",
    "checkout.js",
    "extension-host.js",
    "format.js",
    "operations.js",
    "sandbox-api.js",
    "sandbox-components.js",
    "sandbox.html",
    "sandbox.js",
    "tokens.css",
    "vdom.js",
    "views.js",
);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_file_of_the_page_is_embedded() {
        let directory = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("assets");
        let mut on_disk: Vec<String> = std::fs::read_dir(&directory)
            .expect("the assets directory")
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
        assert!(get("tests/checkout.test.mjs").is_none());
        assert!(get("preview/serve.mjs").is_none());
    }

    #[test]
    fn the_scripts_of_the_sandbox_are_classic_scripts() {
        for name in ["sandbox-api.js", "sandbox-components.js", "sandbox.js"] {
            let source = String::from_utf8(get(name).unwrap().to_vec()).unwrap();
            for line in source.lines() {
                let line = line.trim_start();
                assert!(
                    !line.starts_with("import ") && !line.starts_with("export "),
                    "{name} is loaded as a classic script: {line}"
                );
            }
        }
    }
}
