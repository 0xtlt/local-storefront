//! Compression of what the server sends: Brotli or gzip, for the clients that accept one and
//! the content that gets smaller, as a Shopify storefront answers.

use std::io::Write;

use super::reply::Reply;

/// Smaller responses are sent as they are: there is nothing to gain.
const MIN_SIZE: usize = 1024;

// Fast levels: every response is compressed as it goes, and to the same machine nothing is
// slow to send. On a 400 KB page, Brotli takes about half a millisecond at this quality and
// leaves an eighth of the page; three times as long would only save another 2%.
const BROTLI_QUALITY: u32 = 1;
const BROTLI_WINDOW: u32 = 22;
const GZIP_LEVEL: u32 = 3;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Encoding {
    Brotli,
    Gzip,
}

impl Encoding {
    /// The name of the encoding in `Content-Encoding`.
    fn name(self) -> &'static str {
        match self {
            Encoding::Brotli => "br",
            Encoding::Gzip => "gzip",
        }
    }

    fn compress(self, input: &[u8]) -> std::io::Result<Vec<u8>> {
        let mut output = Vec::with_capacity(input.len() / 4);
        match self {
            Encoding::Brotli => {
                let mut writer =
                    brotli::CompressorWriter::new(&mut output, 4096, BROTLI_QUALITY, BROTLI_WINDOW);
                writer.write_all(input)?;
                writer.flush()?;
                drop(writer);
            }
            Encoding::Gzip => {
                let mut encoder = flate2::write::GzEncoder::new(
                    &mut output,
                    flate2::Compression::new(GZIP_LEVEL),
                );
                encoder.write_all(input)?;
                encoder.finish()?;
            }
        }
        Ok(output)
    }
}

/// The encoding to answer a request with, from its `Accept-Encoding`: Brotli when the client
/// takes it, gzip otherwise, none when it takes neither. A weight (`br;q=0.5`) decides between
/// the two, and `q=0` refuses one.
pub fn negotiate(accept_encoding: Option<&str>) -> Option<Encoding> {
    let (mut brotli, mut gzip, mut any) = (None, None, None);
    for part in accept_encoding?.split(',') {
        let mut pieces = part.split(';');
        let name = pieces
            .next()
            .unwrap_or_default()
            .trim()
            .to_ascii_lowercase();
        let weight = pieces
            .find_map(|piece| piece.trim().strip_prefix("q="))
            .map_or(1.0, |weight| weight.trim().parse::<f32>().unwrap_or(0.0));
        match name.as_str() {
            "br" => brotli = Some(weight),
            "gzip" | "x-gzip" => gzip = Some(weight),
            "*" => any = Some(weight),
            _ => {}
        }
    }
    let brotli = brotli.or(any).unwrap_or(0.0);
    let gzip = gzip.or(any).unwrap_or(0.0);
    if brotli > 0.0 && brotli >= gzip {
        Some(Encoding::Brotli)
    } else if gzip > 0.0 {
        Some(Encoding::Gzip)
    } else {
        None
    }
}

/// Whether content of this type gets smaller: text, JSON, scripts, XML, SVG and the fonts and
/// images that are not compressed already.
fn compressible(content_type: &str) -> bool {
    let kind = content_type
        .split(';')
        .next()
        .unwrap_or_default()
        .trim()
        .to_ascii_lowercase();
    kind.starts_with("text/")
        || kind.ends_with("/json")
        || kind.ends_with("+json")
        || kind.ends_with("/xml")
        || kind.ends_with("+xml")
        || matches!(
            kind.as_str(),
            "application/javascript"
                | "application/x-mpegurl"
                | "application/wasm"
                | "font/ttf"
                | "font/otf"
                | "application/vnd.ms-fontobject"
                | "image/x-icon"
                | "image/bmp"
        )
}

fn header<'a>(reply: &'a Reply, name: &str) -> Option<&'a str> {
    reply
        .headers
        .iter()
        .find(|(key, _)| key.eq_ignore_ascii_case(name))
        .map(|(_, value)| value.as_str())
}

/// Compresses a reply with the encoding its request accepts.
///
/// Images, videos and the other formats that are compressed already are left alone, like
/// replies without a body and small ones. A reply that could be compressed says that it
/// depends on `Accept-Encoding`, whether this one was or not.
pub fn apply(mut reply: Reply, encoding: Option<Encoding>) -> Reply {
    if !compressible(header(&reply, "content-type").unwrap_or_default())
        || header(&reply, "content-encoding").is_some()
        || reply.body.is_empty()
    {
        return reply;
    }
    // One header, as a storefront sends it: `Vary: Accept-Encoding, Accept` for a page.
    match reply
        .headers
        .iter_mut()
        .find(|(name, _)| name.eq_ignore_ascii_case("vary"))
    {
        Some((_, value)) => *value = format!("Accept-Encoding, {value}"),
        None => reply
            .headers
            .push(("vary".to_string(), "Accept-Encoding".to_string())),
    }
    let Some(encoding) = encoding else {
        return reply;
    };
    if reply.body.len() < MIN_SIZE {
        return reply;
    }
    match encoding.compress(&reply.body) {
        Ok(compressed) if compressed.len() < reply.body.len() => {
            reply.body = compressed;
            reply.header("content-encoding", encoding.name())
        }
        _ => reply,
    }
}

#[cfg(test)]
pub(crate) fn decompress(encoding: Encoding, input: &[u8]) -> Vec<u8> {
    use std::io::Read;
    let mut output = Vec::new();
    match encoding {
        Encoding::Brotli => {
            brotli::Decompressor::new(input, 4096)
                .read_to_end(&mut output)
                .expect("valid Brotli");
        }
        Encoding::Gzip => {
            flate2::read::GzDecoder::new(input)
                .read_to_end(&mut output)
                .expect("valid gzip");
        }
    }
    output
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn brotli_is_preferred_and_weights_are_followed() {
        // What browsers send.
        assert_eq!(
            negotiate(Some("gzip, deflate, br, zstd")),
            Some(Encoding::Brotli)
        );
        assert_eq!(negotiate(Some("gzip, deflate")), Some(Encoding::Gzip));
        assert_eq!(negotiate(Some("x-gzip")), Some(Encoding::Gzip));
        assert_eq!(negotiate(Some("BR")), Some(Encoding::Brotli));
        assert_eq!(negotiate(Some("*")), Some(Encoding::Brotli));
        // Neither, or nothing said.
        assert_eq!(negotiate(Some("identity")), None);
        assert_eq!(negotiate(Some("zstd, deflate")), None);
        assert_eq!(negotiate(Some("")), None);
        assert_eq!(negotiate(None), None);
        // Weights.
        assert_eq!(
            negotiate(Some("br;q=0.5, gzip;q=0.8")),
            Some(Encoding::Gzip)
        );
        assert_eq!(negotiate(Some("br; q=0, gzip")), Some(Encoding::Gzip));
        assert_eq!(negotiate(Some("br;q=0, gzip;q=0")), None);
        assert_eq!(negotiate(Some("gzip;q=0, *")), Some(Encoding::Brotli));
        assert_eq!(negotiate(Some("*;q=0")), None);
        assert_eq!(negotiate(Some("br;q=nonsense, gzip")), Some(Encoding::Gzip));
    }

    #[test]
    fn only_content_that_gets_smaller_is_compressed() {
        for kind in [
            "text/html; charset=utf-8",
            "text/css; charset=utf-8",
            "text/javascript; charset=utf-8",
            "application/json; charset=utf-8",
            "application/ld+json",
            "application/xml",
            "image/svg+xml",
            "font/ttf",
            "Text/Plain",
        ] {
            assert!(compressible(kind), "{kind}");
        }
        for kind in [
            "image/png",
            "image/jpeg",
            "image/webp",
            "font/woff2",
            "video/mp4",
            "application/octet-stream",
            "application/pdf",
            "",
        ] {
            assert!(!compressible(kind), "{kind}");
        }
    }

    fn page() -> Vec<u8> {
        "<p>The same paragraph, again and again.</p>\n"
            .repeat(200)
            .into_bytes()
    }

    #[test]
    fn replies_are_compressed_for_the_clients_that_accept_it() {
        for encoding in [Encoding::Brotli, Encoding::Gzip] {
            let reply = apply(
                Reply::html(200, String::from_utf8(page()).unwrap()),
                Some(encoding),
            );
            assert_eq!(header(&reply, "content-encoding"), Some(encoding.name()));
            assert_eq!(header(&reply, "vary"), Some("Accept-Encoding"));
            assert!(reply.body.len() < page().len() / 10, "{}", reply.body.len());
            assert_eq!(decompress(encoding, &reply.body), page());
        }

        // A client that accepts nothing gets the page as it is, and a cache is told why.
        let plain = apply(Reply::html(200, String::from_utf8(page()).unwrap()), None);
        assert_eq!(header(&plain, "content-encoding"), None);
        assert_eq!(header(&plain, "vary"), Some("Accept-Encoding"));
        assert_eq!(plain.body, page());
    }

    #[test]
    fn some_replies_are_left_alone() {
        let untouched = |reply: Reply| {
            let body = reply.body.clone();
            let reply = apply(reply, Some(Encoding::Brotli));
            assert_eq!(header(&reply, "content-encoding"), None);
            assert_eq!(reply.body, body);
            reply
        };
        // Small.
        let small = untouched(Reply::json(200, &serde_json::json!({"item_count": 0})));
        assert_eq!(header(&small, "vary"), Some("Accept-Encoding"));
        // Compressed already.
        let image = untouched(Reply::new(200, "image/png", page()));
        assert_eq!(header(&image, "vary"), None);
        // Without a body.
        untouched(Reply::redirect("/cart"));
        // Encoded by whoever built it.
        let encoded = apply(
            Reply::html(200, String::from_utf8(page()).unwrap()).header("Content-Encoding", "gzip"),
            Some(Encoding::Brotli),
        );
        assert_eq!(header(&encoded, "content-encoding"), Some("gzip"));
        assert_eq!(encoded.body, page());
    }
}
