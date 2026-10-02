//! Everything Shopify would serve from its CDN: theme assets, the bundles built from
//! `{% stylesheet %}` and `{% javascript %}` tags, images and other files.

use std::sync::Arc;

use lsf_core::filters::html_payment_icon;
use lsf_core::images::{Transform, placeholder, transform_file};

use super::reply::{Reply, content_type};
use super::{Incoming, ServerState};

/// Images are cached in memory, up to this many variants.
const IMAGE_CACHE_LIMIT: usize = 800;

const RASTER_EXTENSIONS: [&str; 6] = ["jpg", "jpeg", "png", "gif", "webp", "bmp"];

fn extension(path: &str) -> String {
    path.rsplit('.')
        .next()
        .unwrap_or_default()
        .to_ascii_lowercase()
}

pub fn handle(state: &ServerState, incoming: &Incoming) -> Reply {
    let path = incoming.path.as_str();
    // The scripts of the platform: the local counterparts of what Shopify's CDN serves.
    if let Some(script) = lsf_core::render::platform::asset(path.trim_start_matches("/cdn/")) {
        return Reply::new(200, "text/javascript; charset=utf-8", script);
    }
    let segments: Vec<&str> = path.trim_start_matches("/cdn/").split('/').collect();
    match segments.as_slice() {
        ["shop", "t", _, "assets", rest @ ..] => asset(state, incoming, &rest.join("/")),
        ["shop", "t", _, "compiled_assets", "styles.css"] => Reply::new(
            200,
            "text/css; charset=utf-8",
            state.app.renderer.compiled_stylesheet(),
        )
        .immutable(),
        ["shop", "t", _, "compiled_assets", "scripts.js"] => Reply::new(
            200,
            "text/javascript; charset=utf-8",
            state.app.renderer.compiled_javascript(),
        )
        .immutable(),
        [
            "shop",
            "files" | "products" | "collections" | "articles",
            rest @ ..,
        ] => {
            // Older stores keep product, collection and article images in their own folders;
            // locally everything lives in `files/`.
            let folder = segments[1];
            let name = rest.join("/");
            let src = if folder == "files" {
                name
            } else {
                format!("{folder}/{name}")
            };
            file(state, incoming, &src)
        }
        ["fonts", rest @ ..] => match rest.last() {
            Some(name) => font(state, name),
            None => Reply::not_found(),
        },
        [
            "shopifycloud",
            "storefront",
            "assets",
            "payment_icons",
            name,
        ] => {
            let handle = name.trim_end_matches(".svg");
            Reply::new(200, "image/svg+xml", html_payment_icon(handle, None)).immutable()
        }
        // Shopify's shared images (the gift card illustration, ...) are not available offline:
        // a neutral drawing keeps the layout without a broken image.
        ["shopifycloud", ..] | ["s", "global", ..] if path.ends_with(".svg") => Reply::new(
            200,
            "image/svg+xml",
            "<svg xmlns=\"http://www.w3.org/2000/svg\" viewBox=\"0 0 334 200\" role=\"img\">\
             <rect width=\"334\" height=\"200\" rx=\"12\" fill=\"#e7e5e1\"/>\
             <rect x=\"24\" y=\"132\" width=\"120\" height=\"14\" rx=\"7\" fill=\"#c9c6c0\"/>\
             <rect x=\"24\" y=\"156\" width=\"72\" height=\"14\" rx=\"7\" fill=\"#c9c6c0\"/></svg>",
        )
        .header("x-lsf-placeholder", "1"),
        // Shopify's shared scripts and styles are not available offline: answer with an empty
        // file of the right type rather than a 404 that would show up as a page error.
        ["shopifycloud", ..] | ["s", "global", ..] => Reply::new(
            200,
            content_type(path),
            format!("/* {path} is served by Shopify and is not available locally */"),
        ),
        _ => Reply::not_found(),
    }
}

/// Serves a font of Shopify's font library: the real file when it was put into
/// `files/fonts/`, otherwise a blank font that makes browsers use the theme's fallback fonts.
fn font(state: &ServerState, name: &str) -> Reply {
    if name.contains("..") {
        return Reply::not_found();
    }
    let local = state
        .app
        .data_dir()
        .map(|directory| directory.join("files/fonts").join(name))
        .and_then(|path| std::fs::read(path).ok());
    match local {
        Some(bytes) => Reply::new(200, content_type(name), bytes).immutable(),
        None => Reply::new(200, "font/ttf", lsf_core::fonts::blank_font())
            .immutable()
            .header("x-lsf-placeholder", "1"),
    }
}

fn asset(state: &ServerState, incoming: &Incoming, name: &str) -> Reply {
    let theme = &state.app.theme;
    let files = theme.files();
    let Some(path) = files.resolve(&format!("assets/{name}")) else {
        return Reply::not_found();
    };
    if let Ok(bytes) = std::fs::read(&path) {
        return Reply::new(200, content_type(name), bytes).immutable();
    }
    // `theme.css.liquid` is requested as `theme.css` and rendered with the theme settings.
    if let Some(source) = files.read(&format!("assets/{name}.liquid")) {
        let loaded = state.loaded();
        let request = state.storefront_request(&loaded.store, incoming, "/", Vec::new());
        let rendered = state.app.renderer.render_asset(
            loaded.store.clone(),
            request,
            &source,
            &format!("assets/{name}"),
        );
        return Reply::new(200, content_type(name), rendered);
    }
    // `asset_img_url` asks for a resized variant: `logo_small.png`.
    let mut transform = Transform::default();
    if let Some(original) = transform.strip_legacy_suffix(name)
        && let Some(path) = files
            .resolve(&format!("assets/{original}"))
            .filter(|path| path.is_file())
    {
        return match transform_file(&path, &transform) {
            Ok((bytes, kind)) => Reply::new(200, kind, bytes).immutable(),
            Err(error) => Reply::text(500, error),
        };
    }
    Reply::not_found()
}

/// Serves a file of the data directory's `files/`, transformed when it is an image and the URL
/// asks for it. Images that the fixtures mention but that do not exist get a placeholder.
fn file(state: &ServerState, incoming: &Incoming, src: &str) -> Reply {
    if src.split('/').any(|segment| segment == "..") {
        return Reply::not_found();
    }
    let query = incoming
        .query
        .iter()
        .map(|(key, value)| (key.as_str(), value.as_str()));
    let mut transform = Transform::from_query(query);
    let directory = state
        .app
        .data_dir()
        .map(|directory| directory.join("files"));
    let exists = |src: &str| {
        directory
            .as_ref()
            .map(|directory| directory.join(src))
            .filter(|path| path.is_file())
    };

    // A legacy size suffix (`shirt_large.jpg`) names a variant of `shirt.jpg`.
    let mut src = src.to_string();
    if exists(&src).is_none()
        && let Some((folder, name)) = src
            .rsplit_once('/')
            .map(|(folder, name)| (format!("{folder}/"), name))
            .or(Some((String::new(), src.as_str())))
        && let Some(original) = transform.strip_legacy_suffix(name)
    {
        src = format!("{folder}{original}");
    }

    let kind = extension(&src);
    let is_raster = RASTER_EXTENSIONS.contains(&kind.as_str());
    let path = exists(&src);

    // Anything that is not a raster image is served as it is.
    if !is_raster {
        return match path.and_then(|path| std::fs::read(path).ok()) {
            Some(bytes) => Reply::new(200, content_type(&src), bytes).immutable(),
            None => Reply::not_found(),
        };
    }
    if let Some(path) = &path
        && transform == Transform::default()
    {
        return match std::fs::read(path) {
            Ok(bytes) => Reply::new(200, content_type(&src), bytes).immutable(),
            Err(_) => Reply::not_found(),
        };
    }

    let cache_key = format!("{src}?{transform:?}");
    if let Some(cached) = state
        .image_cache
        .lock()
        .expect("image cache poisoned")
        .get(&cache_key)
        .cloned()
    {
        return Reply::new(200, cached.1, cached.0.clone()).immutable();
    }
    let result = match &path {
        Some(path) => transform_file(path, &transform),
        None => {
            let declared = state
                .loaded()
                .store
                .image_size(&src)
                .unwrap_or((1200, 1200));
            placeholder(&src, declared, &transform)
        }
    };
    match result {
        Ok((bytes, kind)) => {
            let entry = Arc::new((bytes, kind));
            let mut cache = state.image_cache.lock().expect("image cache poisoned");
            if cache.len() >= IMAGE_CACHE_LIMIT {
                cache.clear();
            }
            cache.insert(cache_key, entry.clone());
            let reply = Reply::new(200, entry.1, entry.0.clone()).immutable();
            if path.is_none() {
                reply.header("x-lsf-placeholder", "1")
            } else {
                reply
            }
        }
        Err(error) => Reply::text(500, error),
    }
}
