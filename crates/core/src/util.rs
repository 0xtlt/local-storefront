//! Small helpers shared across the crate.

/// The command that runs lsf, as the hints of the tool write it (`` `lsf app build x` ``):
/// `LSF_COMMAND`, which the npm launcher sets to `npx local-storefront`, else `lsf`.
pub fn command() -> String {
    command_of(std::env::var("LSF_COMMAND").ok().as_deref())
}

/// [`command`] for a value of `LSF_COMMAND`: blank is unset.
pub fn command_of(variable: Option<&str>) -> String {
    variable
        .map(str::trim)
        .filter(|command| !command.is_empty())
        .unwrap_or("lsf")
        .to_string()
}

/// `1 Function`, `2 Functions`: a number and what it counts, in the singular or the plural.
pub fn count(n: usize, one: &str, many: &str) -> String {
    format!("{n} {}", if n == 1 { one } else { many })
}

/// Turns a string into a Shopify handle, as the `handleize` filter does and as Shopify makes the
/// handle of a resource from its title: lowercase, with a single hyphen for each run of spaces
/// and punctuation, and none at the ends. Latin accents are folded (`Crème brûlée` →
/// `creme-brulee`, `Straße` → `strasse`), a straight apostrophe is removed
/// (`Women's` → `womens`), and underscores and the letters of other scripts, emoji and
/// ligatures are kept (`A_B` → `a_b`, `Ελληνικά 日本語` → `ελληνικά-日本語`).
pub fn handleize(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    let mut pending_hyphen = false;
    for c in input.chars().flat_map(char::to_lowercase) {
        for c in transliterate(c).chars() {
            if c == '\'' {
                continue;
            }
            if separates_words(c) {
                pending_hyphen = true;
            } else {
                if pending_hyphen && !out.is_empty() {
                    out.push('-');
                }
                pending_hyphen = false;
                out.push(c);
            }
        }
    }
    out
}

/// What a handle replaces with a hyphen: spaces, control characters and punctuation (ASCII's
/// symbols included, `&` or `+`), but the underscore.
fn separates_words(c: char) -> bool {
    use unicode_general_category::{GeneralCategory, get_general_category};
    if c == '_' {
        return false;
    }
    if c.is_ascii() {
        return !c.is_ascii_alphanumeric();
    }
    c.is_whitespace()
        || c.is_control()
        || matches!(
            get_general_category(c),
            GeneralCategory::ConnectorPunctuation
                | GeneralCategory::DashPunctuation
                | GeneralCategory::OpenPunctuation
                | GeneralCategory::ClosePunctuation
                | GeneralCategory::InitialPunctuation
                | GeneralCategory::FinalPunctuation
                | GeneralCategory::OtherPunctuation
                | GeneralCategory::SpaceSeparator
                | GeneralCategory::LineSeparator
                | GeneralCategory::ParagraphSeparator
        )
}

/// A Latin letter with an accent, lowercase, as the plain letters it is folded to.
fn transliterate(c: char) -> String {
    let replacement = match c {
        'à' | 'á' | 'â' | 'ã' | 'ä' | 'å' | 'ā' | 'ă' | 'ą' => "a",
        'æ' => "ae",
        'ç' | 'ć' | 'č' | 'ĉ' | 'ċ' => "c",
        'ď' | 'đ' | 'ð' => "d",
        'è' | 'é' | 'ê' | 'ë' | 'ē' | 'ĕ' | 'ė' | 'ę' | 'ě' => "e",
        'ĝ' | 'ğ' | 'ġ' | 'ģ' => "g",
        'ĥ' | 'ħ' => "h",
        'ì' | 'í' | 'î' | 'ï' | 'ĩ' | 'ī' | 'ĭ' | 'į' | 'ı' => "i",
        'ĵ' => "j",
        'ķ' => "k",
        'ĺ' | 'ļ' | 'ľ' | 'ŀ' | 'ł' => "l",
        'ñ' | 'ń' | 'ņ' | 'ň' => "n",
        'ò' | 'ó' | 'ô' | 'õ' | 'ö' | 'ø' | 'ō' | 'ŏ' | 'ő' => "o",
        'œ' => "oe",
        'ŕ' | 'ŗ' | 'ř' => "r",
        'ś' | 'ŝ' | 'ş' | 'š' => "s",
        'ß' => "ss",
        'ţ' | 'ť' | 'ŧ' => "t",
        'þ' => "th",
        'ù' | 'ú' | 'û' | 'ü' | 'ũ' | 'ū' | 'ŭ' | 'ů' | 'ű' | 'ų' => "u",
        'ŵ' => "w",
        'ý' | 'ÿ' | 'ŷ' => "y",
        'ź' | 'ż' | 'ž' => "z",
        other => return other.to_string(),
    };
    replacement.to_string()
}

/// A stable 13-digit id derived from a kind and a key, so the same data always produces the
/// same ids. The range stays well below 2^53, so ids survive a round trip through JavaScript.
pub fn stable_id(kind: &str, key: &str) -> u64 {
    // FNV-1a.
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for byte in kind.bytes().chain([0u8]).chain(key.bytes()) {
        hash ^= u64::from(byte);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    1_000_000_000_000 + hash % 8_000_000_000_000
}

/// A stable hash rendered as a short string, for cache-busting versions and generated ids.
pub fn short_hash(input: &str) -> String {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for byte in input.bytes() {
        hash ^= u64::from(byte);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    format!("{hash:016x}")
}

/// `main-menu` → `Main menu`.
pub fn humanize(handle: &str) -> String {
    let spaced = handle.replace(['-', '_'], " ");
    let mut chars = spaced.chars();
    match chars.next() {
        Some(first) => first.to_uppercase().chain(chars).collect(),
        None => String::new(),
    }
}

/// Finds the candidate closest to `input`, for "did you mean" hints.
pub fn closest_match<'a>(
    input: &str,
    candidates: impl IntoIterator<Item = &'a str>,
) -> Option<&'a str> {
    let mut best: Option<(usize, &str)> = None;
    for candidate in candidates {
        let distance = edit_distance(input, candidate);
        if best.is_none_or(|(best_distance, _)| distance < best_distance) {
            best = Some((distance, candidate));
        }
    }
    let (distance, candidate) = best?;
    // Only suggest when the names are genuinely close.
    let longest = input.chars().count().max(candidate.chars().count());
    let threshold = if longest <= 4 {
        1
    } else {
        (longest / 3).max(2)
    };
    (distance <= threshold).then_some(candidate)
}

/// The Levenshtein distance between two strings, in characters.
pub fn edit_distance(a: &str, b: &str) -> usize {
    let a: Vec<char> = a.chars().collect();
    let b: Vec<char> = b.chars().collect();
    let mut previous: Vec<usize> = (0..=b.len()).collect();
    for (i, ca) in a.iter().enumerate() {
        let mut current = vec![i + 1];
        for (j, cb) in b.iter().enumerate() {
            let substitution = previous[j] + usize::from(ca != cb);
            current.push(substitution.min(previous[j + 1] + 1).min(current[j] + 1));
        }
        previous = current;
    }
    previous[b.len()]
}

/// The user's cache folder: `~/Library/Caches` on macOS, `$XDG_CACHE_HOME` or `~/.cache` on
/// Linux, `%LOCALAPPDATA%` on Windows. Unlike the system's temporary folder, nothing empties it
/// behind lsf's back.
pub fn user_cache_dir() -> Option<std::path::PathBuf> {
    let var = |name: &str| {
        std::env::var_os(name)
            .filter(|value| !value.is_empty())
            .map(std::path::PathBuf::from)
    };
    if cfg!(windows) {
        var("LOCALAPPDATA")
    } else if cfg!(target_os = "macos") {
        var("HOME").map(|home| home.join("Library").join("Caches"))
    } else {
        var("XDG_CACHE_HOME").or_else(|| var("HOME").map(|home| home.join(".cache")))
    }
}

/// [`std::fs::canonicalize`], without the `\\?\` it puts before every path on Windows:
/// `\\?\C:\app` is `C:\app`, `\\?\UNC\server\share\app` is `\\server\share\app`, as Node's
/// `fs.realpath` (libuv), which Shopify's CLI uses, writes them. lsf shows the paths it
/// canonicalizes. Every path lsf canonicalizes goes through it (`clippy.toml` forbids the
/// others), so that two of them compare alike. A path Windows would read as another file without
/// the prefix keeps it ([`without_verbatim_prefix`]).
pub fn canonicalize(path: &std::path::Path) -> std::io::Result<std::path::PathBuf> {
    #[allow(clippy::disallowed_methods)]
    let path = std::fs::canonicalize(path)?;
    Ok(match path.to_str().and_then(without_verbatim_prefix) {
        Some(simple) => std::path::PathBuf::from(simple),
        None => path,
    })
}

/// A verbatim path of Windows (`\\?\C:\app`, `\\?\UNC\server\share\app`) without its prefix
/// (`C:\app`, `\\server\share\app`), when both name the same file: when no part of it is one
/// Windows reads otherwise without the prefix, as a device (`CON`, `nul.txt`), a name that ends
/// in a dot or a space (dropped), `.` and `..` (followed), or a character a name cannot have.
/// `None` for any other path, a path of macOS or Linux among them. A path longer than Windows'
/// 260 characters loses the prefix too: Rust's file functions put it back from 248 characters
/// (`maybe_verbatim` in the standard library).
pub fn without_verbatim_prefix(path: &str) -> Option<String> {
    let rest = path.strip_prefix(r"\\?\")?;
    let (simple, names) = if let Some(unc) = rest.strip_prefix(r"UNC\") {
        // `\\server\share`: the two names before the path are names too.
        let mut root = unc.split('\\');
        if root.next().is_none_or(str::is_empty) || root.next().is_none_or(str::is_empty) {
            return None;
        }
        (format!(r"\\{unc}"), unc)
    } else {
        let bytes = rest.as_bytes();
        if bytes.len() < 3 || !bytes[0].is_ascii_alphabetic() || &bytes[1..3] != br":\" {
            // `\\?\Volume{…}\`, `\\?\GLOBALROOT\…`, a drive without its root: no shorter form.
            return None;
        }
        (rest.to_string(), &rest[3..])
    };
    let mut parts = names.split('\\').peekable();
    while let Some(name) = parts.next() {
        // `C:\` ends with an empty name; one anywhere else is `\\` in the path.
        if name.is_empty() && parts.peek().is_none() {
            continue;
        }
        if !is_plain_name(name) {
            return None;
        }
    }
    Some(simple)
}

/// Whether Windows reads `name`, a part of a path, as it is written, without the `\\?\` prefix.
fn is_plain_name(name: &str) -> bool {
    const DEVICES: [&str; 4] = ["CON", "PRN", "AUX", "NUL"];
    if name.is_empty()
        || name.len() > 255
        || name.ends_with(['.', ' '])
        || name
            .chars()
            .any(|c| c < ' ' || matches!(c, '<' | '>' | ':' | '"' | '/' | '|' | '?' | '*'))
    {
        return false;
    }
    // `CON`, `con.txt`, `COM1 .log`, `LPT³`: a device, whatever follows the first dot.
    let stem = name.split('.').next().unwrap_or(name).trim_end();
    let numbered = |prefix: &str| {
        stem.get(..3)
            .is_some_and(|start| start.eq_ignore_ascii_case(prefix))
            && matches!(
                &stem[3..],
                "1" | "2" | "3" | "4" | "5" | "6" | "7" | "8" | "9" | "¹" | "²" | "³"
            )
    };
    !(DEVICES
        .iter()
        .any(|device| stem.eq_ignore_ascii_case(device))
        || numbered("COM")
        || numbered("LPT"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_verbatim_path_of_windows_is_shown_without_its_prefix() {
        let simple = |path: &str| without_verbatim_prefix(path);
        assert_eq!(
            simple(r"\\?\C:\work\demo-app\extensions\payment-rules\dist\function.wasm").as_deref(),
            Some(r"C:\work\demo-app\extensions\payment-rules\dist\function.wasm")
        );
        assert_eq!(simple(r"\\?\C:\").as_deref(), Some(r"C:\"));
        assert_eq!(simple(r"\\?\d:\demo-app").as_deref(), Some(r"d:\demo-app"));
        assert_eq!(
            simple(r"\\?\UNC\server\share\demo-app").as_deref(),
            Some(r"\\server\share\demo-app")
        );
        // A long path too: Rust's file functions put the prefix back.
        let long = format!(r"\\?\C:\{}", ["folder"; 60].join(r"\"));
        assert_eq!(simple(&long).as_deref(), Some(&long[4..]));
        // Names Windows reads as written: a dot inside, spaces inside, accents, `COM10`.
        assert_eq!(
            simple(r"\\?\C:\My Apps\démo.app\COM10\console\nul-rules").as_deref(),
            Some(r"C:\My Apps\démo.app\COM10\console\nul-rules")
        );
    }

    #[test]
    fn a_verbatim_path_keeps_its_prefix_where_windows_would_read_another_file() {
        for path in [
            // Devices, whatever their case or extension.
            r"\\?\C:\demo-app\CON",
            r"\\?\C:\demo-app\nul.txt",
            r"\\?\C:\demo-app\Com1",
            r"\\?\C:\demo-app\lpt9.log",
            r"\\?\C:\demo-app\AUX .json",
            r"\\?\C:\demo-app\COM²",
            // Names Windows trims, follows or rejects.
            r"\\?\C:\demo-app\dist.",
            r"\\?\C:\demo-app\dist ",
            r"\\?\C:\demo-app\..\x",
            r"\\?\C:\demo-app\.\x",
            r"\\?\C:\demo-app\\x",
            r"\\?\C:\demo-app\a:b",
            r"\\?\C:\demo-app\a/b",
            r"\\?\C:\demo-app\a?b",
            r"\\?\UNC\\share",
            r"\\?\UNC\server",
            r"\\?\UNC\server\",
            r"\\?\UNC\server\nul",
            // No shorter form.
            r"\\?\Volume{0b1c2d3e-0000-0000-0000-100000000000}\demo-app",
            r"\\?\GLOBALROOT\Device\demo-app",
            r"\\?\C:",
            r"\\?\C:demo-app",
            r"\\?\1:\demo-app",
        ] {
            assert_eq!(without_verbatim_prefix(path), None, "{path}");
        }
        let long = format!(r"\\?\C:\{}", "x".repeat(256));
        assert_eq!(without_verbatim_prefix(&long), None);
    }

    #[test]
    fn other_paths_are_left_as_they_are() {
        for path in [
            "/home/dev/demo-app",
            "/private/var/demo-app",
            r"C:\demo-app",
            r"\\server\share\demo-app",
            r"\\.\COM1",
            "",
        ] {
            assert_eq!(without_verbatim_prefix(path), None, "{path}");
        }
    }

    #[test]
    fn a_canonical_path_is_absolute_and_has_no_verbatim_prefix() {
        let here = canonicalize(std::path::Path::new(".")).unwrap();
        assert!(here.is_absolute(), "{}", here.display());
        assert!(
            !here.to_string_lossy().starts_with(r"\\?\"),
            "{}",
            here.display()
        );
        assert!(canonicalize(std::path::Path::new("no such folder, surely")).is_err());
    }

    #[test]
    fn the_command_is_lsf_unless_the_launcher_names_another() {
        assert_eq!(command_of(None), "lsf");
        assert_eq!(command_of(Some("  ")), "lsf");
        assert_eq!(
            command_of(Some("npx local-storefront")),
            "npx local-storefront"
        );
    }

    #[test]
    fn a_count_is_singular_for_one_only() {
        assert_eq!(count(0, "Function", "Functions"), "0 Functions");
        assert_eq!(count(1, "Function", "Functions"), "1 Function");
        assert_eq!(count(2, "UI extension", "UI extensions"), "2 UI extensions");
    }

    #[test]
    fn handleizes() {
        assert_eq!(handleize("Blue Shirt"), "blue-shirt");
        assert_eq!(
            handleize("  100% Cotton -- T-Shirt! "),
            "100-cotton-t-shirt"
        );
        assert_eq!(handleize("Crème brûlée"), "creme-brulee");
        assert_eq!(handleize("Straße"), "strasse");
    }

    #[test]
    fn handleizes_as_a_shopify_store_does() {
        // What a development store's `handleize` gave for each input (2026-11-12):
        // https://github.com/0xtlt/local-storefront/issues/85
        // A straight apostrophe is removed, a curly one is a hyphen; underscores, other scripts,
        // emoji and ligatures are kept; Latin accents are folded.
        for (input, handle) in [
            ("Women's T-Shirts & Tops", "womens-t-shirts-tops"),
            ("L'été", "lete"),
            ("A_B", "a_b"),
            ("Ελληνικά Русский 日本語", "ελληνικά-русский-日本語"),
            ("😀 smile 😀", "😀-smile-😀"),
            ("Straße Œuvre ﬁne", "strasse-oeuvre-ﬁne"),
            ("L'été à Paris", "lete-a-paris"),
            ("it's 100% (new)!", "its-100-new"),
            ("  --A__B--  ", "a__b"),
            ("ı İ", "i-i\u{307}"),
            ("Été 2026", "ete-2026"),
            ("Crème Brûlée", "creme-brulee"),
            ("Women’s Tops", "women-s-tops"),
        ] {
            assert_eq!(handleize(input), handle, "{input:?}");
        }
    }

    #[test]
    fn ids_are_stable_and_distinct() {
        assert_eq!(stable_id("product", "a"), stable_id("product", "a"));
        assert_ne!(stable_id("product", "a"), stable_id("variant", "a"));
        assert!(stable_id("product", "a") < (1u64 << 53));
    }

    #[test]
    fn suggests_close_names() {
        assert_eq!(
            closest_match("titel", ["title", "handle", "price"]),
            Some("title")
        );
        assert_eq!(closest_match("zzz", ["title", "handle"]), None);
    }
}
