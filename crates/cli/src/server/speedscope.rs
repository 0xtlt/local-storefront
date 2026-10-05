//! speedscope (<https://github.com/jlfwong/speedscope>), the viewer `shopify theme profile`
//! opens, served from the binary under `/__lsf/speedscope/` to show a profile as a flame
//! graph. The files are its release build: see `assets/speedscope/NOTICE.md`.

use super::reply::{Reply, cache, content_type};

macro_rules! files {
    ($($name:literal),* $(,)?) => {
        &[$(($name, include_bytes!(concat!("../../assets/speedscope/", $name)) as &[u8])),*]
    };
}

const FILES: &[(&str, &[u8])] = files![
    "index.html",
    "speedscope.6f107512.js",
    "import.bcbb2033.js",
    "demangle-cpp.1768f4cc.js",
    "source-map.438fa06b.js",
    "reset.8c46b7a1.css",
    "source-code-pro.52b1676f.css",
    "SourceCodePro-Regular.ttf.f546cbe0.woff2",
    "favicon-16x16.f74b3187.png",
    "favicon-32x32.bc503437.png",
    // The licenses of the viewer and of its font go where the viewer goes.
    "LICENSE",
    "source-code-pro.LICENSE.md",
];

/// Where the files of the viewer are served.
const BASE: &str = "/__lsf/speedscope/";

/// The page of the viewer as it is released: its files are named next to it.
const INDEX: &str = include_str!("../../assets/speedscope/index.html");

/// One file of the viewer.
pub fn file(name: &str) -> Reply {
    match FILES.iter().find(|(known, _)| *known == name) {
        Some((name, bytes)) if name.contains("LICENSE") => {
            Reply::new(200, "text/plain; charset=utf-8", *bytes)
        }
        // Every other file but the page has a hash of its content in its name.
        Some((name, bytes)) if *name == "index.html" => Reply::new(200, content_type(name), *bytes),
        Some((name, bytes)) => Reply::new(200, content_type(name), *bytes).cached(cache::PLATFORM),
        None => Reply::not_found(),
    }
}

/// The page of the viewer, showing the profile it fetches from `profile_url`, a URL of this
/// server. The page can be served at any address, which stays the one of the browser.
///
/// The viewer only reads where its profile is after the `#` of the address, once, when its
/// script starts. The address is given one for that moment and gets it taken away after.
pub fn page(profile_url: &str) -> Reply {
    // A string of JavaScript, which cannot end the script it is written in.
    let url = serde_json::Value::from(profile_url)
        .to_string()
        .replace('<', "\\u003c");
    let set = format!(
        "<script>var page=location.pathname+location.search;\
         history.replaceState(null,\"\",page+\"#profileURL=\"+encodeURIComponent({url}))</script>"
    );
    let clear = "<script>history.replaceState(null,\"\",page)</script>";
    let html = INDEX
        .replace("href=\"", &format!("href=\"{BASE}"))
        .replace("src=\"", &format!("src=\"{BASE}"));
    // Around the script of the viewer: the one tag of the page that names a file to run.
    let html = match html.split_once("<script src=") {
        Some((before, viewer)) => match viewer.split_once("</script>") {
            Some((viewer, after)) => {
                format!("{before}{set}<script src={viewer}</script>{clear}{after}")
            }
            None => html,
        },
        None => html,
    };
    Reply::html(200, html)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_page_tells_the_viewer_where_its_profile_is_and_keeps_its_address() {
        let reply = page("/__lsf/profile?path=%2F&format=speedscope");
        let html = String::from_utf8(reply.body).unwrap();
        // The address gets a `#` the viewer reads, the viewer starts, the `#` goes away.
        let set = "history.replaceState(null,\"\",page+\"#profileURL=\"+\
                   encodeURIComponent(\"/__lsf/profile?path=%2F&format=speedscope\"))</script>";
        let viewer = "<script src=\"/__lsf/speedscope/speedscope.6f107512.js\"></script>";
        let clear = "<script>history.replaceState(null,\"\",page)</script>";
        assert!(html.contains(&format!("{set}{viewer}{clear}")), "{html}");
        // The files of the page are named from the root: the page is served elsewhere.
        assert_eq!(html.matches("=\"/__lsf/speedscope/").count(), 5, "{html}");
        assert!(
            !html.contains("href=\"reset") && !html.contains("src=\"speedscope"),
            "{html}"
        );
        for part in html.split('"').filter(|part| part.starts_with(BASE)) {
            assert_eq!(file(part.trim_start_matches(BASE)).status, 200, "{part}");
        }

        // Nothing of the address of a profile can end the script or the string it is in.
        let reply = page("/a\"</script><script>alert(1)//\\");
        let html = String::from_utf8(reply.body).unwrap();
        assert_eq!(html.matches("<script").count(), 4, "{html}");
        assert!(
            html.contains(
                "encodeURIComponent(\"/a\\\"\\u003c/script>\\u003cscript>alert(1)//\\\\\"))"
            ),
            "{html}"
        );
    }
}
