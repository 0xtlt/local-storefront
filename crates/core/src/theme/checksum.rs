//! The checksum of a theme file as Shopify CLI computes it, to tell which files changed against
//! the `checksumMd5` of a store (`packages/theme/src/cli/utilities/asset-checksum.ts`):
//!
//! - a text file (by its extension, as Shopify CLI's `isTextFile`): CRLF becomes LF, then MD5;
//! - `*/settings_data.json`, which Shopify keeps minified: CRLF becomes LF, the first `/* … */`
//!   comment goes, spaces and newlines outside strings go, then MD5;
//! - any other file: the MD5 of its bytes;
//! - an empty text file: the empty string (an empty binary file is the MD5 of nothing).
//!
//! Checked on a store (`theme push`, then its `checksumMd5`): a binary file and a text file with
//! LF line ends have the checksum the store answers. The store answers the MD5 of the bytes it
//! keeps, which are the bytes pushed: for a file with CRLF line ends, or an empty one, its
//! answer differs from this checksum, as it does for a JSON template or a `settings_data.json`,
//! which the store rewrites. The Admin API of lsf answers [`md5_hex`] of the bytes, as a store.

use md5::{Digest, Md5};

/// The extensions of the files Shopify CLI reads as text: those `mrmime` gives the types its
/// `isTextFile` lists (JavaScript, JSON, Liquid, CSS, Sass, SCSS, SVG, Markdown).
const TEXT_EXTENSIONS: [&str; 12] = [
    "js", "mjs", "json", "map", "liquid", "css", "sass", "scss", "svg", "svgz", "md", "markdown",
];

/// The `checksumMd5` of the theme file `key` (`config/settings_data.json`, relative to the
/// theme and with `/`) with the content `bytes`.
pub fn checksum_md5(key: &str, bytes: &[u8]) -> String {
    if !is_text_file(key) {
        return md5_hex(bytes);
    }
    // Shopify CLI reads a text file as UTF-8, a byte that is not becoming U+FFFD.
    let text = String::from_utf8_lossy(bytes);
    if text.is_empty() {
        return String::new();
    }
    let text = text.replace("\r\n", "\n");
    if key.ends_with("/settings_data.json") {
        return md5_hex(minified_json(&text).as_bytes());
    }
    md5_hex(text.as_bytes())
}

/// Whether Shopify CLI reads the theme file `key` as text, by the extension `mrmime` reads:
/// what follows the last `.` of the key, or the whole key without one, ignoring case.
pub fn is_text_file(key: &str) -> bool {
    let key = key.trim().to_lowercase();
    let extension = key
        .rsplit_once('.')
        .map_or(key.as_str(), |(_, after)| after);
    TEXT_EXTENSIONS.contains(&extension)
}

/// The text of a `settings_data.json` as Shopify keeps it: without its first `/* … */` comment,
/// and without the spaces and newlines outside strings (a tab stays, as with Shopify CLI).
fn minified_json(text: &str) -> String {
    let mut text = text.to_string();
    if let Some(start) = text.find("/*")
        && let Some(length) = text[start + 2..].find("*/")
    {
        text.replace_range(start..start + 2 + length + 2, "");
    }
    let mut minified = String::with_capacity(text.len());
    let mut in_string = false;
    let mut was_backslash = false;
    for character in text.chars() {
        if character == '"' && !was_backslash {
            in_string = !in_string;
        }
        if !in_string && matches!(character, ' ' | '\n') {
            continue;
        }
        minified.push(character);
        was_backslash = character == '\\' && !was_backslash;
    }
    minified
}

/// The hex MD5 digest of bytes.
pub fn md5_hex(bytes: &[u8]) -> String {
    Md5::digest(bytes)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The checksums below are Shopify CLI's: those its own tests expect
    /// (`asset-checksum.test.ts`, whose fixtures are copied under `tests/fixtures`), and those
    /// its `calculateChecksum` gives the other contents.
    #[test]
    fn the_fixtures_of_shopify_cli_have_its_checksums() {
        for (key, bytes, expected) in [
            (
                "config/settings_data.json",
                &include_bytes!("../../tests/fixtures/shopify-cli-theme/config/settings_data.json")
                    [..],
                "22e69af13b7953914563c60035a831bc",
            ),
            (
                "templates/404.json",
                include_bytes!("../../tests/fixtures/shopify-cli-theme/templates/404.json"),
                "64caf742bd427adcf497bffab63df30c",
            ),
        ] {
            assert_eq!(checksum_md5(key, bytes), expected, "{key}");
        }
    }

    #[test]
    fn a_binary_file_is_the_md5_of_its_bytes() {
        for key in ["assets/crlf.png", "assets/notes.txt", "assets/js"] {
            assert_eq!(
                checksum_md5(key, b"a\r\nb\r\n"),
                "59b0d7772f0561efb95518f3cb8abc60",
                "{key}"
            );
        }
        assert_eq!(
            checksum_md5("assets/empty.png", b""),
            "d41d8cd98f00b204e9800998ecf8427e"
        );
    }

    #[test]
    fn a_text_file_has_its_crlf_read_as_lf() {
        for (key, bytes, expected) in [
            (
                "snippets/crlf.liquid",
                &b"a\r\nb\r\n"[..],
                "dd8c6a395b5dd36c56d23275028f526c",
            ),
            (
                "assets/THEME.JS",
                b"x\r\n",
                "401b30e3b8b5d629635a5c613cdb7919",
            ),
            (
                "assets/app.js.map",
                b"{\r\n}\r\n",
                "5b76b0eef9af8a2300673e0553f609f9",
            ),
            (
                "assets/icon.svg",
                b"<svg>\r\n</svg>",
                "5a0f2bc36b7bc53627098da1071f550d",
            ),
            ("README.md", b"# a\r\n", "6e980849a5f14634773f0b41cff2183c"),
            // A lone CR stays.
            (
                "assets/lone.css",
                b"a\rb\n",
                "fa12f95fa6fdcf9c39ac7000b966a42e",
            ),
            // Not minified: only a `settings_data.json` is.
            (
                "config/settings_schema.json",
                b"[ { \"name\": \"x\" } ]\r\n",
                "4787768fbd7b5929264bd539225dea99",
            ),
            // A byte that is not UTF-8 reads as U+FFFD.
            (
                "assets/bad.css",
                b"\xff\r\na",
                "0a9db1c23583f1e7664eb1eee9ba1b4a",
            ),
        ] {
            assert_eq!(checksum_md5(key, bytes), expected, "{key}");
        }
    }

    #[test]
    fn settings_data_is_minified_without_its_first_comment() {
        let settings = b"/*\r\n * comment\r\n */\r\n{\r\n  \"a\": \"x  y\",\r\n\t\"b\": \"q\\\" /* z */ \",\r\n  \"c\": [1, 2]\r\n}\r\n/* second */\r\n";
        assert_eq!(
            checksum_md5("config/settings_data.json", settings),
            "83e188b2e25453418034bb93a486388d"
        );
        assert_eq!(
            minified_json("/* a */{ \"k\": \"v w\",\n\t\"e\": \"\\\\\" }/* b */"),
            "{\"k\":\"v w\",\t\"e\":\"\\\\\"}/*b*/"
        );
        // Only spaces and newlines: nothing is left, whose MD5 is that of nothing.
        assert_eq!(
            checksum_md5("config/settings_data.json", b"   \n"),
            "d41d8cd98f00b204e9800998ecf8427e"
        );
        // Without a directory, the name is not that of the settings.
        assert_eq!(
            checksum_md5("settings_data.json", b"{ \"a\": 1 }"),
            "f404f62068b38f2bd6c126f5c14ae101"
        );
    }

    #[test]
    fn an_empty_text_file_has_no_checksum() {
        assert_eq!(checksum_md5("snippets/empty.liquid", b""), "");
        assert_eq!(checksum_md5("config/settings_data.json", b""), "");
    }

    #[test]
    fn text_files_are_those_of_shopify_cli() {
        for text in [
            "a.js",
            "a.mjs",
            "a.json",
            "a.js.map",
            "a.liquid",
            "a.css",
            "a.sass",
            "a.scss",
            "a.svg",
            "a.svgz",
            "a.md",
            "a.markdown",
            "A.CSS.LIQUID",
            "json",
        ] {
            assert!(is_text_file(text), "{text}");
        }
        for binary in [
            "a.png",
            "a.txt",
            "a.woff2",
            "a.html",
            "a.ts",
            "assets/js",
            "a.d/js",
            "a",
        ] {
            assert!(!is_text_file(binary), "{binary}");
        }
    }
}
