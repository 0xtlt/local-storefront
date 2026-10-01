//! Small helpers shared across the crate.

/// Turns a string into a Shopify handle: lowercase ASCII letters and digits separated by
/// single hyphens. Accented Latin letters are transliterated (`Crème brûlée` → `creme-brulee`).
pub fn handleize(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    let mut pending_hyphen = false;
    for c in input.chars() {
        let mapped = transliterate(c);
        for c in mapped.chars() {
            if c.is_ascii_alphanumeric() {
                if pending_hyphen && !out.is_empty() {
                    out.push('-');
                }
                pending_hyphen = false;
                out.push(c.to_ascii_lowercase());
            } else {
                pending_hyphen = true;
            }
        }
    }
    out
}

fn transliterate(c: char) -> String {
    let replacement = match c {
        'à' | 'á' | 'â' | 'ã' | 'ä' | 'å' | 'ā' | 'ă' | 'ą' | 'À' | 'Á' | 'Â' | 'Ã' | 'Ä' | 'Å'
        | 'Ā' | 'Ă' | 'Ą' => "a",
        'æ' | 'Æ' => "ae",
        'ç' | 'ć' | 'č' | 'ĉ' | 'ċ' | 'Ç' | 'Ć' | 'Č' | 'Ĉ' | 'Ċ' => "c",
        'ď' | 'đ' | 'Ď' | 'Đ' | 'ð' | 'Ð' => "d",
        'è' | 'é' | 'ê' | 'ë' | 'ē' | 'ĕ' | 'ė' | 'ę' | 'ě' | 'È' | 'É' | 'Ê' | 'Ë' | 'Ē' | 'Ĕ'
        | 'Ė' | 'Ę' | 'Ě' => "e",
        'ĝ' | 'ğ' | 'ġ' | 'ģ' | 'Ĝ' | 'Ğ' | 'Ġ' | 'Ģ' => "g",
        'ĥ' | 'ħ' | 'Ĥ' | 'Ħ' => "h",
        'ì' | 'í' | 'î' | 'ï' | 'ĩ' | 'ī' | 'ĭ' | 'į' | 'ı' | 'Ì' | 'Í' | 'Î' | 'Ï' | 'Ĩ' | 'Ī'
        | 'Ĭ' | 'Į' | 'İ' => "i",
        'ĵ' | 'Ĵ' => "j",
        'ķ' | 'Ķ' => "k",
        'ĺ' | 'ļ' | 'ľ' | 'ŀ' | 'ł' | 'Ĺ' | 'Ļ' | 'Ľ' | 'Ŀ' | 'Ł' => "l",
        'ñ' | 'ń' | 'ņ' | 'ň' | 'Ñ' | 'Ń' | 'Ņ' | 'Ň' => "n",
        'ò' | 'ó' | 'ô' | 'õ' | 'ö' | 'ø' | 'ō' | 'ŏ' | 'ő' | 'Ò' | 'Ó' | 'Ô' | 'Õ' | 'Ö' | 'Ø'
        | 'Ō' | 'Ŏ' | 'Ő' => "o",
        'œ' | 'Œ' => "oe",
        'ŕ' | 'ŗ' | 'ř' | 'Ŕ' | 'Ŗ' | 'Ř' => "r",
        'ś' | 'ŝ' | 'ş' | 'š' | 'Ś' | 'Ŝ' | 'Ş' | 'Š' => "s",
        'ß' => "ss",
        'ţ' | 'ť' | 'ŧ' | 'Ţ' | 'Ť' | 'Ŧ' => "t",
        'þ' | 'Þ' => "th",
        'ù' | 'ú' | 'û' | 'ü' | 'ũ' | 'ū' | 'ŭ' | 'ů' | 'ű' | 'ų' | 'Ù' | 'Ú' | 'Û' | 'Ü' | 'Ũ'
        | 'Ū' | 'Ŭ' | 'Ů' | 'Ű' | 'Ų' => "u",
        'ŵ' | 'Ŵ' => "w",
        'ý' | 'ÿ' | 'ŷ' | 'Ý' | 'Ÿ' | 'Ŷ' => "y",
        'ź' | 'ż' | 'ž' | 'Ź' | 'Ż' | 'Ž' => "z",
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

fn edit_distance(a: &str, b: &str) -> usize {
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

#[cfg(test)]
mod tests {
    use super::*;

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
