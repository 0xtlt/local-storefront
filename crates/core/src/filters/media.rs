//! Image, video and 3D model filters.

use std::any::Any;
use std::borrow::Cow;

use slt_liquid::filters::escape_html;
use slt_liquid::number::to_integer;
use slt_liquid::{Context, Environment, Error, FilterArgs, Object, Result, Value};

use super::{html_attributes, site};
use crate::drops::media::{ImageDrop, MediaDrop, focal_point_css, image_base_url, source_url};
use crate::drops::metafield::image_from_src;
use crate::render::state::RenderState;
use crate::site::Site;
use crate::store::{Image, MediaKind, VideoHost};
use crate::urls;

/// The widths Shopify puts in a default `srcset`.
const DEFAULT_SRCSET_WIDTHS: [u32; 4] = [352, 832, 1200, 1920];

/// The largest dimension the image CDN accepts.
const MAX_DIMENSION: i64 = 5760;

/// The result of `image_url`: prints as the URL, and remembers the image it came from so that
/// `image_tag` can work out dimensions, `alt` and `srcset`.
pub struct ImageUrl {
    pub url: String,
    /// The URL without transformation parameters.
    pub base: String,
    pub image: Image,
    pub alt: String,
    pub width: Option<u32>,
    pub height: Option<u32>,
    /// The other transformation parameters (`crop`, `format`, ...), repeated in `srcset` URLs.
    pub params: Vec<(String, String)>,
}

impl Object for ImageUrl {
    fn type_name(&self) -> &str {
        "image_url"
    }

    fn to_value(&self) -> Option<Value> {
        Some(Value::from(&self.url))
    }

    fn render(&self) -> Cow<'_, str> {
        Cow::Borrowed(&self.url)
    }

    fn as_any(&self) -> &dyn Any {
        self
    }
}

/// The image a value stands for, with the alt text to use for it.
///
/// Accepts an image, anything that has one (product, variant, collection, article, line item,
/// media) and a path relative to the CDN's shop directory (`files/shirt.jpg`).
pub fn resolve_image(site: &std::sync::Arc<Site>, input: &Value) -> Option<(Image, String)> {
    match input {
        Value::Object(object) => {
            if let Some(drop) = input.downcast::<ImageDrop>() {
                return Some((drop.image.clone(), drop.image.alt.clone()));
            }
            if let Some(url) = input.downcast::<ImageUrl>() {
                return Some((url.image.clone(), url.alt.clone()));
            }
            if let Some(media) = input.downcast::<MediaDrop>() {
                return media
                    .media
                    .preview
                    .clone()
                    .map(|image| (image, media.media.alt.clone()));
            }
            for key in ["featured_image", "image", "preview_image"] {
                let candidate = object.get(key).unwrap_or(Value::Nil);
                if let Some(drop) = candidate.downcast::<ImageDrop>() {
                    // The resource's title stands in for a missing alt text.
                    let alt = if drop.image.alt.is_empty() {
                        object
                            .get("title")
                            .map(|title| title.to_str().into_owned())
                            .unwrap_or_default()
                    } else {
                        drop.image.alt.clone()
                    };
                    return Some((drop.image.clone(), alt));
                }
            }
            None
        }
        Value::Str(path) if !path.is_empty() => {
            let path = path.split('?').next().unwrap_or(path);
            let src = path
                .rsplit_once("/cdn/shop/")
                .map_or(path, |(_, rest)| rest)
                .trim_start_matches("files/");
            Some((image_from_src(site, src), String::new()))
        }
        _ => None,
    }
}

fn dimension(args: &FilterArgs, name: &str) -> Result<Option<u32>> {
    match args.named(name) {
        None | Some(Value::Nil) => Ok(None),
        Some(value) => {
            let size = to_integer(value)?;
            if !(1..=MAX_DIMENSION).contains(&size) {
                return Err(Error::argument(format!(
                    "{name} must be between 1 and {MAX_DIMENSION}"
                )));
            }
            Ok(Some(size as u32))
        }
    }
}

fn image_url(input: &Value, args: &FilterArgs, ctx: &Context) -> Result<Value> {
    let site = site(ctx)?;
    if input.is_nil() {
        return Ok(Value::Nil);
    }
    let Some((image, alt)) = resolve_image(site, input) else {
        return Ok(Value::Nil);
    };
    let width = dimension(args, "width")?;
    let height = dimension(args, "height")?;
    let mut params: Vec<(String, String)> = Vec::new();
    for (key, value) in &args.named {
        if matches!(key.as_str(), "width" | "height") || value.is_nil() {
            continue;
        }
        params.push((key.clone(), urls::encode_component(&value.to_str())));
    }
    let base = image_base_url(site, &image);
    let url = build_url(&base, width, height, &params);
    Ok(Value::object(ImageUrl {
        url,
        base,
        image,
        alt,
        width,
        height,
        params,
    }))
}

fn build_url(
    base: &str,
    width: Option<u32>,
    height: Option<u32>,
    params: &[(String, String)],
) -> String {
    let mut all: Vec<(&str, String)> = params
        .iter()
        .map(|(key, value)| (key.as_str(), value.clone()))
        .collect();
    if let Some(width) = width {
        all.push(("width", width.to_string()));
    }
    if let Some(height) = height {
        all.push(("height", height.to_string()));
    }
    urls::with_sorted_params(base, &all)
}

/// The size the transformed image is displayed at: `(width, height)`.
fn output_size(url: &ImageUrl) -> (u32, u32) {
    let (natural_width, natural_height) = (
        f64::from(url.image.width.max(1)),
        f64::from(url.image.height.max(1)),
    );
    let ratio = natural_width / natural_height;
    let cropped = url.params.iter().any(|(key, _)| key == "crop");
    let (width, height) = match (url.width, url.height) {
        (Some(width), Some(height)) if cropped => (
            f64::from(width).min(natural_width),
            f64::from(height).min(natural_height),
        ),
        (Some(width), Some(height)) => {
            // Without a crop the image is scaled to fit inside the box.
            let scale = (f64::from(width) / natural_width)
                .min(f64::from(height) / natural_height)
                .min(1.0);
            (natural_width * scale, natural_height * scale)
        }
        (Some(width), None) => {
            let width = f64::from(width).min(natural_width);
            (width, width / ratio)
        }
        (None, Some(height)) => {
            let height = f64::from(height).min(natural_height);
            (height * ratio, height)
        }
        (None, None) => (natural_width, natural_height),
    };
    (width.round() as u32, height.round() as u32)
}

fn srcset(url: &ImageUrl, widths: &[u32]) -> String {
    widths
        .iter()
        .map(|&width| {
            // A fixed height scales with the width so that every candidate keeps the ratio.
            let height = match (url.width, url.height) {
                (Some(base_width), Some(base_height)) => Some(
                    (f64::from(width) * f64::from(base_height) / f64::from(base_width)).round()
                        as u32,
                ),
                _ => None,
            };
            format!(
                "{} {width}w",
                build_url(&url.base, Some(width), height, &url.params)
            )
        })
        .collect::<Vec<_>>()
        .join(", ")
}

fn parse_widths(value: &Value) -> Vec<u32> {
    match value {
        Value::Array(items) => items
            .iter()
            .filter_map(|item| to_integer(item).ok())
            .map(|width| width as u32)
            .collect(),
        other => other
            .to_str()
            .split(',')
            .filter_map(|part| part.trim().parse::<u32>().ok())
            .collect(),
    }
}

fn image_tag(input: &Value, args: &FilterArgs, ctx: &Context) -> Result<Value> {
    let Some(url) = input.downcast::<ImageUrl>() else {
        // A plain URL: nothing is known about the image behind it.
        let src = input.to_str();
        if src.is_empty() {
            return Ok(Value::empty_string());
        }
        return Ok(Value::from(format!(
            "<img src=\"{}\"{}>",
            escape_html(&src),
            html_attributes(&args.named, &["preload", "widths"])
        )));
    };
    let (output_width, output_height) = output_size(url);

    let alt = match args.named("alt") {
        Some(alt) => alt.to_str().into_owned(),
        None => url.alt.clone(),
    };
    let mut html = format!(
        "<img src=\"{}\" alt=\"{}\"",
        escape_html(&url.url),
        escape_html(&alt)
    );

    // srcset: an explicit one wins, then explicit widths, then Shopify's defaults up to the
    // requested width.
    let width_attribute = args
        .named("width")
        .filter(|width| !width.is_nil())
        .and_then(|width| to_integer(width).ok());
    let srcset_value = match args.named("srcset") {
        Some(Value::Nil) => None,
        Some(custom) => Some(custom.to_str().into_owned()),
        None => {
            let widths = match args.named("widths") {
                Some(widths) => parse_widths(widths),
                None => {
                    let limit = url.width.unwrap_or(output_width);
                    let mut widths: Vec<u32> = DEFAULT_SRCSET_WIDTHS
                        .iter()
                        .copied()
                        .filter(|width| *width < limit)
                        .collect();
                    if let Some(width) = width_attribute {
                        widths.push(width as u32);
                    }
                    if widths.is_empty() {
                        widths.push(limit);
                    }
                    widths.sort_unstable();
                    widths.dedup();
                    widths
                }
            };
            Some(srcset(url, &widths))
        }
    };
    if let Some(srcset_value) = &srcset_value {
        html.push_str(&format!(" srcset=\"{}\"", escape_html(srcset_value)));
    }

    // Dimensions: both by default; when either is given explicitly, only what is given.
    let explicit_width = args.named("width");
    let explicit_height = args.named("height");
    if explicit_width.is_none() && explicit_height.is_none() {
        html.push_str(&format!(
            " width=\"{output_width}\" height=\"{output_height}\""
        ));
    } else {
        if let Some(width) = explicit_width.filter(|width| !width.is_nil()) {
            html.push_str(&format!(" width=\"{}\"", escape_html(&width.to_str())));
        }
        if let Some(height) = explicit_height.filter(|height| !height.is_nil()) {
            html.push_str(&format!(" height=\"{}\"", escape_html(&height.to_str())));
        }
    }

    html.push_str(&html_attributes(
        &args.named,
        &[
            "width", "height", "alt", "srcset", "widths", "preload", "style",
        ],
    ));

    let mut style = args
        .named("style")
        .map(|style| style.to_str().into_owned())
        .unwrap_or_default();
    if let Some(point) = url.image.focal_point {
        style.push_str(&format!("object-position:{};", focal_point_css(point)));
    }
    if !style.is_empty() {
        html.push_str(&format!(" style=\"{}\"", escape_html(&style)));
    }

    let preload = args.named("preload").is_some_and(Value::is_truthy);
    if preload {
        let mut header = format!("<{}>; rel=preload; as=image", url.url);
        if let Some(srcset_value) = &srcset_value {
            header.push_str(&format!("; imagesrcset={srcset_value}"));
        }
        if let Some(sizes) = args.named("sizes") {
            header.push_str(&format!("; imagesizes={}", sizes.to_str()));
        }
        RenderState::of(ctx)?.add_preload(header);
    } else if args.named("loading").is_none() {
        // Images in sections further down the page are lazy-loaded by default.
        let below_the_fold =
            matches!(ctx.find_variable("section").get("index"), Value::Int(index) if index > 3);
        if below_the_fold {
            html.push_str(" loading=\"lazy\"");
        }
    }
    html.push('>');
    Ok(Value::from(html))
}

/// The pixel sizes of the named sizes the legacy `img_url` filter accepts.
fn named_size(name: &str) -> Option<&'static str> {
    Some(match name {
        "pico" => "16x16",
        "icon" => "32x32",
        "thumb" => "50x50",
        "small" => "100x100",
        "compact" => "160x160",
        "medium" => "240x240",
        "large" => "480x480",
        "grande" => "600x600",
        _ => return None,
    })
}

/// Parses the size suffix of a legacy image URL (`large`, `480x`, `x480`, `480x480`) into the
/// box it requests. Used by the server to honour those URLs.
pub fn legacy_size(suffix: &str) -> Option<(Option<u32>, Option<u32>)> {
    let size = named_size(suffix).unwrap_or(suffix);
    let (width, height) = size.split_once('x')?;
    let parse = |part: &str| -> Option<Option<u32>> {
        if part.is_empty() {
            Some(None)
        } else {
            part.parse::<u32>().ok().map(Some)
        }
    };
    let (width, height) = (parse(width)?, parse(height)?);
    (width.is_some() || height.is_some()).then_some((width, height))
}

fn legacy_url(
    site: &std::sync::Arc<Site>,
    input: &Value,
    args: &FilterArgs,
    default_size: Option<&str>,
) -> Option<String> {
    let (image, _) = resolve_image(site, input)?;
    let base = image_base_url(site, &image);
    let size = match args.get(0) {
        Some(size) => Some(size.to_str().into_owned()),
        None => default_size.map(str::to_string),
    };
    let Some(size) = size.filter(|size| !matches!(size.as_str(), "master" | "original" | ""))
    else {
        return Some(base);
    };
    let mut suffix = size.clone();
    // Crop and scale only apply to explicit pixel sizes.
    if args.get(0).is_some() && named_size(&size).is_none() {
        if let Some(crop) = args.named("crop") {
            suffix.push_str(&format!("_crop_{}", crop.to_str()));
        }
        if let Some(scale) = args
            .named("scale")
            .and_then(|scale| to_integer(scale).ok())
            .filter(|scale| *scale > 1)
        {
            suffix.push_str(&format!("@{scale}x"));
        }
    }
    Some(urls::with_size_suffix(&base, &suffix))
}

fn img_url(input: &Value, args: &FilterArgs, ctx: &Context) -> Result<Value> {
    Ok(legacy_url(site(ctx)?, input, args, Some("small")).map_or(Value::Nil, Value::from))
}

fn collection_img_url(input: &Value, args: &FilterArgs, ctx: &Context) -> Result<Value> {
    Ok(legacy_url(site(ctx)?, input, args, None).map_or(Value::Nil, Value::from))
}

fn img_tag(input: &Value, args: &FilterArgs, ctx: &Context) -> Result<Value> {
    let site = site(ctx)?;
    // `img_tag: alt, class, size`.
    let size_args = FilterArgs {
        positional: args.get(2).cloned().into_iter().collect(),
        named: Vec::new(),
    };
    let src = match legacy_url(site, input, &size_args, Some("small")) {
        Some(url)
            if input.downcast::<ImageUrl>().is_none()
                && !matches!(input, Value::Str(text) if text.contains("//")) =>
        {
            url
        }
        _ => input.to_str().into_owned(),
    };
    let alt = args
        .get(0)
        .map(|alt| alt.to_str().into_owned())
        .unwrap_or_default();
    let class = args
        .get(1)
        .map(|class| format!(" class=\"{}\"", escape_html(&class.to_str())))
        .unwrap_or_default();
    Ok(Value::from(format!(
        "<img src=\"{}\" alt=\"{}\"{class} />",
        escape_html(&src),
        escape_html(&alt)
    )))
}

/// The poster of a media, at the legacy size given by `image_size` (default `small`).
fn poster_url(site: &std::sync::Arc<Site>, media: &MediaDrop, args: &FilterArgs) -> Option<String> {
    let image = media.media.preview.as_ref()?;
    let size = args
        .named("image_size")
        .map(|size| size.to_str().into_owned())
        .unwrap_or_else(|| "small".to_string());
    Some(urls::with_size_suffix(&image_base_url(site, image), &size))
}

fn video_html(
    site: &std::sync::Arc<Site>,
    media: &MediaDrop,
    args: &FilterArgs,
    controls: bool,
) -> String {
    let MediaKind::Video { sources, .. } = &media.media.kind else {
        return String::new();
    };
    let mut html = String::from("<video playsinline=\"playsinline\"");
    let mut named = args.named.clone();
    if controls && !named.iter().any(|(key, _)| key == "controls") {
        named.push(("controls".to_string(), Value::Bool(true)));
    }
    html.push_str(&html_attributes(&named, &["image_size", "preload"]));
    let preload = args
        .named("preload")
        .map(|preload| preload.to_str().into_owned())
        .unwrap_or_else(|| "metadata".to_string());
    html.push_str(&format!(" preload=\"{}\"", escape_html(&preload)));
    if !media.media.alt.is_empty() {
        html.push_str(&format!(
            " aria-label=\"{}\"",
            escape_html(&media.media.alt)
        ));
    }
    let poster = poster_url(site, media, args);
    if let Some(poster) = &poster {
        html.push_str(&format!(" poster=\"{}\"", escape_html(poster)));
    }
    html.push('>');
    for source in sources {
        html.push_str(&format!(
            "<source src=\"{}\" type=\"{}\">",
            escape_html(&source_url(site, &source.url)),
            escape_html(&source.mime_type)
        ));
    }
    if let Some(poster) = &poster {
        html.push_str(&format!("<img src=\"{}\">", escape_html(poster)));
    }
    html.push_str("</video>");
    html
}

fn video_tag(input: &Value, args: &FilterArgs, ctx: &Context) -> Result<Value> {
    Ok(match input.downcast::<MediaDrop>() {
        Some(media) => Value::from(video_html(site(ctx)?, media, args, false)),
        None => Value::empty_string(),
    })
}

/// The result of `external_video_url`: prints as the URL and keeps the video's title.
struct ExternalVideoUrl {
    url: String,
    title: String,
}

impl Object for ExternalVideoUrl {
    fn type_name(&self) -> &str {
        "external_video_url"
    }

    fn to_value(&self) -> Option<Value> {
        Some(Value::from(&self.url))
    }

    fn as_any(&self) -> &dyn Any {
        self
    }
}

fn embed_url(
    site: &Site,
    media: &MediaDrop,
    named: &[(String, Value)],
) -> Option<ExternalVideoUrl> {
    let MediaKind::ExternalVideo { host, external_id } = &media.media.kind else {
        return None;
    };
    let mut params: Vec<(String, String)> = match host {
        VideoHost::Youtube => vec![
            ("controls".to_string(), "1".to_string()),
            ("enablejsapi".to_string(), "1".to_string()),
            ("modestbranding".to_string(), "1".to_string()),
            (
                "origin".to_string(),
                urls::encode_component(&site.request.origin()),
            ),
            ("playsinline".to_string(), "1".to_string()),
            ("rel".to_string(), "0".to_string()),
        ],
        VideoHost::Vimeo => vec![
            ("byline".to_string(), "0".to_string()),
            ("controls".to_string(), "1".to_string()),
            ("playsinline".to_string(), "1".to_string()),
            ("title".to_string(), "0".to_string()),
        ],
    };
    for (key, value) in named {
        params.retain(|(existing, _)| existing != key);
        params.push((key.clone(), urls::encode_component(&value.to_str())));
    }
    params.sort_by(|a, b| a.0.cmp(&b.0));
    let query: Vec<String> = params
        .iter()
        .map(|(key, value)| format!("{key}={value}"))
        .collect();
    let base = match host {
        VideoHost::Youtube => format!("https://www.youtube.com/embed/{external_id}"),
        VideoHost::Vimeo => format!("https://player.vimeo.com/video/{external_id}"),
    };
    Some(ExternalVideoUrl {
        url: format!("{base}?{}", query.join("&")),
        title: media.media.alt.clone(),
    })
}

fn external_video_url(input: &Value, args: &FilterArgs, ctx: &Context) -> Result<Value> {
    let site = site(ctx)?;
    Ok(input
        .downcast::<MediaDrop>()
        .and_then(|media| embed_url(site, media, &args.named).map(Value::object))
        .unwrap_or(Value::Nil))
}

fn iframe(url: &str, title: &str, args: &FilterArgs) -> String {
    format!(
        "<iframe frameborder=\"0\" allow=\"accelerometer; autoplay; encrypted-media; gyroscope; picture-in-picture\" allowfullscreen=\"allowfullscreen\"{} src=\"{}\" title=\"{}\"></iframe>",
        html_attributes(&args.named, &[]),
        escape_html(url),
        escape_html(title)
    )
}

fn external_video_tag(input: &Value, args: &FilterArgs, ctx: &Context) -> Result<Value> {
    if let Some(url) = input.downcast::<ExternalVideoUrl>() {
        return Ok(Value::from(iframe(&url.url, &url.title, args)));
    }
    let site = site(ctx)?;
    Ok(input
        .downcast::<MediaDrop>()
        .and_then(|media| embed_url(site, media, &[]))
        .map_or_else(Value::empty_string, |url| {
            Value::from(iframe(&url.url, &url.title, args))
        }))
}

fn model_html(site: &std::sync::Arc<Site>, media: &MediaDrop, args: &FilterArgs) -> String {
    let MediaKind::Model { sources } = &media.media.kind else {
        return String::new();
    };
    let source = sources
        .iter()
        .find(|source| source.format == "glb")
        .or_else(|| sources.first())
        .map(|source| source_url(site, &source.url))
        .unwrap_or_default();
    let poster = poster_url(site, media, args)
        .map(|poster| format!(" poster=\"{}\"", escape_html(&poster)))
        .unwrap_or_default();
    format!(
        "<model-viewer{} src=\"{}\" camera-controls=\"true\" style=\"--poster-color: transparent;\" data-shopify-feature=\"1.12\" alt=\"{}\"{poster}></model-viewer>",
        html_attributes(&args.named, &["image_size"]),
        escape_html(&source),
        escape_html(&media.media.alt)
    )
}

fn model_viewer_tag(input: &Value, args: &FilterArgs, ctx: &Context) -> Result<Value> {
    Ok(match input.downcast::<MediaDrop>() {
        Some(media) => Value::from(model_html(site(ctx)?, media, args)),
        None => Value::empty_string(),
    })
}

fn media_tag(input: &Value, args: &FilterArgs, ctx: &Context) -> Result<Value> {
    let site = site(ctx)?;
    if let Some(media) = input.downcast::<MediaDrop>() {
        return Ok(Value::from(match &media.media.kind {
            MediaKind::Video { .. } => video_html(site, media, args, true),
            MediaKind::Model { .. } => model_html(site, media, args),
            MediaKind::ExternalVideo { .. } => match embed_url(site, media, &[]) {
                Some(url) => iframe(&url.url, &url.title, args),
                None => String::new(),
            },
            MediaKind::Image => String::new(),
        }));
    }
    if let Some(image) = input.downcast::<ImageDrop>() {
        let size = args
            .named("image_size")
            .map(|size| size.to_str().into_owned())
            .unwrap_or_else(|| "small".to_string());
        let url = urls::with_size_suffix(&image_base_url(site, &image.image), &size);
        return Ok(Value::from(format!(
            "<img src=\"{}\" alt=\"{}\"{}>",
            escape_html(&url),
            escape_html(&image.image.alt),
            html_attributes(&args.named, &["image_size"])
        )));
    }
    Ok(Value::empty_string())
}

pub(super) fn register(env: &mut Environment) {
    env.register_filter("image_url", image_url);
    env.register_filter("image_tag", image_tag);
    env.register_filter("img_url", img_url);
    env.register_filter("product_img_url", img_url);
    env.register_filter("article_img_url", img_url);
    env.register_filter("collection_img_url", collection_img_url);
    env.register_filter("img_tag", img_tag);
    env.register_filter("video_tag", video_tag);
    env.register_filter("external_video_url", external_video_url);
    env.register_filter("external_video_tag", external_video_tag);
    env.register_filter("model_viewer_tag", model_viewer_tag);
    env.register_filter("media_tag", media_tag);
}
