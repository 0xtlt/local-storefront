//! Polaris, the web components of an embedded app's pages (`<s-page>`, `<s-button>`, …), which
//! the app loads from Shopify's CDN: lsf serves the CDN's paths at `/__lsf/cdn/<path>`, and its
//! Polaris mode (`--polaris`, `/__lsf/polaris`) decides what they answer.
//!
//! - `online`, the default: a redirect to `https://cdn.shopify.com/<path>`, the query kept.
//! - `offline`: the copy in the binary (`assets/shopify-cdn`, see its README: Polaris 1.x and 2.x,
//!   their icons and their fonts), with `https://cdn.shopify.com/` in its scripts and stylesheets
//!   pointed at `/__lsf/cdn/` of this server, so that what Polaris loads comes from here too. The
//!   unversioned names (`polaris.js`, `polaris-2.js`) are the copies the CDN served at those names
//!   when they were fetched. A path the copy does not have is a 404 that says so.
//!
//! An app points its Polaris script at `/__lsf/cdn/shopifycloud/polaris-2.js` (the `polarisUrl` of
//! the React Router template) and gets Shopify's online, lsf's offline.

use std::sync::atomic::{AtomicBool, Ordering};

use percent_encoding::{AsciiSet, CONTROLS, utf8_percent_encode};
use serde_json::{Value as Json, json};

use super::reply::{Reply, cache, content_type};
use super::{Incoming, ServerState};

mod files;

/// Where the paths of the CDN are served.
pub const PREFIX: &str = "/__lsf/cdn/";

/// Shopify's CDN.
const CDN: &str = "https://cdn.shopify.com/";

/// What `/__lsf/cdn/` answers.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, clap::ValueEnum)]
pub enum Mode {
    /// Redirect to Shopify's CDN.
    #[default]
    Online,
    /// Serve lsf's copy, without the internet.
    Offline,
}

impl Mode {
    pub fn as_str(self) -> &'static str {
        match self {
            Mode::Online => "online",
            Mode::Offline => "offline",
        }
    }

    fn parse(text: &str) -> Option<Mode> {
        match text.trim().to_ascii_lowercase().as_str() {
            "online" => Some(Mode::Online),
            "offline" => Some(Mode::Offline),
            _ => None,
        }
    }
}

/// The Polaris mode of a server: the one it started with (`--polaris`), and the one it is in,
/// which the control API changes.
pub struct Polaris {
    default: Mode,
    offline: AtomicBool,
}

impl Polaris {
    pub fn new(default: Mode) -> Polaris {
        Polaris {
            default,
            offline: AtomicBool::new(default == Mode::Offline),
        }
    }

    pub fn mode(&self) -> Mode {
        if self.offline.load(Ordering::Relaxed) {
            Mode::Offline
        } else {
            Mode::Online
        }
    }

    pub fn set(&self, mode: Mode) {
        self.offline.store(mode == Mode::Offline, Ordering::Relaxed);
    }

    /// Back to the mode the server started with.
    pub fn reset(&self) -> Mode {
        self.set(self.default);
        self.default
    }
}

/// A copy of Polaris: its path on the CDN, the version it says it is, and the other paths the
/// CDN served it at when it was fetched.
pub struct Script {
    pub path: &'static str,
    pub version: &'static str,
    pub aliases: &'static [&'static str],
}

/// The bytes of the copy at `path` (a path on the CDN, an alias included), and the path of the
/// file they are.
fn copy_of(path: &str) -> Option<(&'static str, &'static [u8])> {
    let path = files::SCRIPTS
        .iter()
        .find(|script| script.aliases.contains(&path))
        .map_or(path, |script| script.path);
    let index = files::FILES
        .binary_search_by(|(known, _)| (*known).cmp(path))
        .ok()?;
    Some(files::FILES[index])
}

/// What a path keeps of itself in a URL: the characters a URL cannot have as they are.
const PATH: &AsciiSet = &CONTROLS
    .add(b' ')
    .add(b'"')
    .add(b'#')
    .add(b'%')
    .add(b'<')
    .add(b'>')
    .add(b'?')
    .add(b'\\')
    .add(b'^')
    .add(b'`')
    .add(b'{')
    .add(b'|')
    .add(b'}');

/// `GET /__lsf/cdn/<path>`: a redirect to Shopify's CDN, or lsf's copy, by the mode.
pub fn cdn(state: &ServerState, incoming: &Incoming) -> Reply {
    if !matches!(incoming.method.as_str(), "GET" | "HEAD") {
        return Reply::text(405, "Only GET and HEAD\n").header("allow", "GET, HEAD");
    }
    let path = incoming
        .path
        .strip_prefix(PREFIX)
        .unwrap_or_default()
        .trim_start_matches('/');
    let reply = match state.polaris.mode() {
        Mode::Online => {
            let mut location = format!("{CDN}{}", utf8_percent_encode(path, PATH));
            if !incoming.raw_query.is_empty() {
                location.push('?');
                location.push_str(&incoming.raw_query);
            }
            // Never kept: the mode may change, and the same URL then answers the copy.
            Reply::redirect(&location).cached("no-store")
        }
        Mode::Offline => match copy_of(path) {
            Some((file, bytes)) => offline(incoming, file, bytes),
            None => Reply::text(404, missing(path)).cached("no-store"),
        },
    };
    // Loaded by the app's pages, on the app's origin: scripts, fonts and `fetch()`es of icons.
    reply
        .header("access-control-allow-origin", "*")
        .header("cross-origin-resource-policy", "cross-origin")
        .header("x-content-type-options", "nosniff")
}

/// A file of the copy. The scripts and the stylesheets get the CDN's URLs in them pointed at
/// this server, and are fetched again each time: their content depends on the address the
/// server is reached at, and the same URL redirects once the mode is `online`. The icons and the
/// fonts have a hash or a date in their name, and are only loaded by the copy.
fn offline(incoming: &Incoming, file: &str, bytes: &'static [u8]) -> Reply {
    let kind = content_type(file);
    let rewritten = (file.ends_with(".js") || file.ends_with(".css"))
        .then(|| std::str::from_utf8(bytes).ok())
        .flatten();
    match rewritten {
        Some(text) => {
            let here = format!("{}{PREFIX}", super::checkout::origin_of(incoming));
            Reply::new(200, kind, text.replace(CDN, &here)).cached("no-cache")
        }
        None => Reply::new(200, kind, bytes).cached(cache::IMMUTABLE),
    }
}

/// The 404 of a path the copy does not have.
fn missing(path: &str) -> String {
    let scripts: Vec<String> = files::SCRIPTS
        .iter()
        .map(|script| {
            let mut names: Vec<&str> = script.aliases.to_vec();
            names.push(script.path);
            let names: Vec<&str> = names
                .iter()
                .map(|name| name.trim_start_matches("shopifycloud/"))
                .collect();
            format!("{} ({})", script.version, names.join(", "))
        })
        .collect();
    format!(
        "lsf has no offline copy of {CDN}{path}\n\n\
         lsf's Polaris mode is offline: {PREFIX}shopifycloud/ serves its copy of Polaris {}, with \
         their icons and fonts. Online (--polaris online, or PUT /__lsf/polaris \
         {{\"mode\": \"online\"}}), {PREFIX}<path> redirects to {CDN}<path>.\n",
        scripts.join(" and "),
    )
}

/// The control API of the mode: `GET`, `PUT` (`POST`) and `DELETE /__lsf/polaris`. `None` for
/// another path.
pub fn control(state: &ServerState, incoming: &Incoming, segments: &[&str]) -> Option<Reply> {
    if segments != ["polaris"] {
        return None;
    }
    let reply = match incoming.method.as_str() {
        "GET" => Reply::json(200, &status(state)),
        "PUT" | "POST" => {
            let params = incoming.all_params();
            match params
                .get("mode")
                .and_then(Json::as_str)
                .and_then(Mode::parse)
            {
                Some(mode) => {
                    state.polaris.set(mode);
                    Reply::json(200, &json!({ "ok": true, "mode": mode.as_str() }))
                }
                None => Reply::json(
                    422,
                    &json!({
                        "ok": false,
                        "error": format!(
                            "mode: {} is not a Polaris mode: \"online\" or \"offline\"",
                            params.get("mode").map_or("nothing".to_string(), Json::to_string)
                        ),
                    }),
                ),
            }
        }
        "DELETE" => {
            let mode = state.polaris.reset();
            Reply::json(200, &json!({ "ok": true, "mode": mode.as_str() }))
        }
        _ => Reply::text(405, "Only GET, PUT, POST and DELETE\n")
            .header("allow", "GET, PUT, POST, DELETE"),
    };
    // lsf's App Bridge reads it from the app's page, on the app's origin.
    Some(reply.header("access-control-allow-origin", "*"))
}

/// `GET /__lsf/polaris`.
fn status(state: &ServerState) -> Json {
    json!({
        "mode": state.polaris.mode().as_str(),
        "default": state.polaris.default.as_str(),
        "cdn": PREFIX,
        "scripts": files::SCRIPTS.iter().map(|script| json!({
            "version": script.version,
            "path": script.path,
            "aliases": script.aliases,
        })).collect::<Vec<_>>(),
        "files": files::FILES.len(),
    })
}

/// What the server prints of its Polaris mode when it starts offline: online, `/__lsf/cdn/` is
/// Shopify's CDN, nothing to say.
pub fn banner(state: &ServerState, origin: &str) -> Option<String> {
    if state.polaris.mode() != Mode::Offline {
        return None;
    }
    let versions: Vec<&str> = files::SCRIPTS.iter().map(|script| script.version).collect();
    Some(format!(
        "polaris: offline: {origin}{PREFIX}shopifycloud/polaris-2.js (lsf's copy of Polaris {})",
        versions.join(" and ")
    ))
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;
    use std::path::Path;

    use sha2::{Digest, Sha256};

    use super::*;

    fn directory() -> std::path::PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR")).join("assets/shopify-cdn")
    }

    fn on_disk(root: &Path, directory: &Path, found: &mut Vec<String>) {
        for entry in std::fs::read_dir(directory).expect("assets/shopify-cdn") {
            let path = entry.expect("an entry").path();
            if path.is_dir() {
                on_disk(root, &path, found);
            } else {
                let relative = path.strip_prefix(root).expect("under the root");
                found.push(relative.to_string_lossy().replace('\\', "/"));
            }
        }
    }

    #[test]
    fn the_list_the_files_and_their_sums_agree() {
        let root = directory();
        let mut found = Vec::new();
        on_disk(&root, &root, &mut found);
        found.retain(|path| path != "README.md" && path != "SHA256SUMS");
        found.sort();
        let listed: Vec<String> = files::FILES
            .iter()
            .map(|(path, _)| path.to_string())
            .collect();
        assert_eq!(found, listed, "run tools/polaris/refresh.mjs");
        // Sorted, for the binary search.
        assert!(files::FILES.windows(2).all(|pair| pair[0].0 < pair[1].0));

        let sums = std::fs::read_to_string(root.join("SHA256SUMS")).expect("SHA256SUMS");
        let sums: BTreeMap<&str, &str> = sums
            .lines()
            .map(|line| {
                let (sum, path) = line.split_once("  ").expect("<sum>  <path>");
                (path, sum)
            })
            .collect();
        assert_eq!(sums.len(), files::FILES.len());
        for (path, bytes) in files::FILES {
            let sum: String = Sha256::digest(bytes)
                .iter()
                .map(|byte| format!("{byte:02x}"))
                .collect();
            assert_eq!(sums.get(path).copied(), Some(sum.as_str()), "{path}");
        }
    }

    #[test]
    fn every_script_and_alias_is_a_file_of_the_copy() {
        assert!(files::SCRIPTS.len() >= 2);
        for script in files::SCRIPTS {
            let (file, bytes) = copy_of(script.path).expect(script.path);
            assert_eq!(file, script.path);
            let text = std::str::from_utf8(bytes).expect("a script is text");
            assert!(
                text.contains(&format!("\"{}\"", script.version)),
                "{} says {}",
                script.path,
                script.version
            );
            for alias in script.aliases {
                assert_eq!(
                    copy_of(alias).map(|(file, _)| file),
                    Some(script.path),
                    "{alias}"
                );
            }
        }
        for (alias, file) in [
            ("shopifycloud/polaris.js", "shopifycloud/polaris-1.1.js"),
            ("shopifycloud/polaris-1.js", "shopifycloud/polaris-1.1.js"),
            ("shopifycloud/polaris-1.1.js", "shopifycloud/polaris-1.1.js"),
            ("shopifycloud/polaris-2.js", "shopifycloud/polaris-2.0.js"),
            ("shopifycloud/polaris-2.0.js", "shopifycloud/polaris-2.0.js"),
        ] {
            assert_eq!(
                copy_of(alias).map(|(found, _)| found),
                Some(file),
                "{alias}"
            );
        }
        assert!(copy_of("shopifycloud/polaris-1.0.js").is_none());
        assert!(copy_of("shopifycloud/app-bridge.js").is_none());
        assert!(copy_of("../README.md").is_none());
    }

    #[test]
    fn what_the_scripts_load_from_the_cdn_is_in_the_copy() {
        // Every URL of the CDN in a script or a stylesheet is a file of the copy, or the prefix
        // of its icons; each stylesheet's fonts are files of the copy.
        for (path, bytes) in files::FILES {
            if !(path.ends_with(".js") || path.ends_with(".css")) {
                continue;
            }
            let text = std::str::from_utf8(bytes).expect("text");
            for (at, _) in text.match_indices(CDN) {
                let url: String = text[at + CDN.len()..]
                    .chars()
                    .take_while(|c| !matches!(c, '"' | '\'' | '`' | ')'))
                    .collect();
                assert!(
                    url.ends_with('/') && url.starts_with("shopifycloud/admin-ui-foundations/")
                        || copy_of(&url).is_some(),
                    "{path} loads {CDN}{url}, which lsf has no copy of"
                );
            }
            if path.ends_with(".css") {
                let folder = path.rsplit_once('/').map_or("", |(folder, _)| folder);
                for part in text.split("url(").skip(1) {
                    let name = part
                        .split(')')
                        .next()
                        .unwrap_or_default()
                        .trim_matches(['\'', '"', ' ']);
                    assert!(
                        copy_of(&format!("{folder}/{name}")).is_some(),
                        "{path}: {name}"
                    );
                }
            }
        }
        // The icons the scripts name: a sample, in each of the three folders.
        let icons = files::FILES
            .iter()
            .filter(|(path, _)| path.ends_with(".svg"))
            .count();
        assert!(icons > 800, "{icons} icons");
        for folder in ["icons", "internal-only", "experimental"] {
            let prefix = format!("shopifycloud/admin-ui-foundations/{folder}/");
            assert!(
                files::FILES
                    .iter()
                    .any(|(path, _)| path.starts_with(&prefix)),
                "{folder}"
            );
        }
    }

    #[test]
    fn the_mode_starts_as_told_and_goes_back_to_it() {
        let polaris = Polaris::new(Mode::Online);
        assert_eq!(polaris.mode(), Mode::Online);
        polaris.set(Mode::Offline);
        assert_eq!(polaris.mode(), Mode::Offline);
        assert_eq!(polaris.reset(), Mode::Online);
        assert_eq!(polaris.mode(), Mode::Online);
        let polaris = Polaris::new(Mode::Offline);
        assert_eq!(polaris.mode(), Mode::Offline);
        polaris.set(Mode::Online);
        assert_eq!(polaris.reset(), Mode::Offline);
        assert_eq!(Mode::parse(" Offline "), Some(Mode::Offline));
        assert_eq!(Mode::parse("cdn"), None);
    }
}
