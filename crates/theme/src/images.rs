//! The local stand-in for Shopify's image CDN: resizes, crops and re-encodes images on demand,
//! in the lightest format the client reads, and draws placeholders for images the fixtures
//! mention but that do not exist on disk.

use std::ffi::c_int;
use std::io::Cursor;
use std::path::Path;

use image::codecs::avif::AvifEncoder;
use image::codecs::jpeg::JpegEncoder;
use image::imageops::FilterType;
use image::{DynamicImage, ImageFormat, Rgb, RgbImage};

use crate::drops::color::Color;
use crate::filters::media::legacy_size;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Crop {
    Top,
    Center,
    Bottom,
    Left,
    Right,
}

/// A transformation requested through the URL (`?width=400&height=400&crop=center`).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Transform {
    pub width: Option<u32>,
    pub height: Option<u32>,
    pub crop: Option<Crop>,
    /// `(left, top, width, height)` of the region to keep, in source pixels.
    pub region: Option<(u32, u32, u32, u32)>,
    pub pad_color: Option<[u8; 3]>,
    /// `pjpg`, `jpg` or `png`: the format for the clients that read neither WebP nor AVIF.
    /// Defaults to the format of the source.
    pub format: Option<String>,
    pub quality: Option<u8>,
}

/// The formats a client reads besides the classic ones, as its `Accept` header says.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Accepted {
    pub webp: bool,
    pub avif: bool,
}

impl Accepted {
    /// Reads an `Accept` header the way Shopify's CDN does: a client that names `image/webp`
    /// reads WebP, one that names `image/avif` as well reads AVIF, and the weights (`;q=`) do
    /// not count.
    pub fn from_header(accept: Option<&str>) -> Accepted {
        let accept = accept.unwrap_or_default().to_ascii_lowercase();
        let webp = accept.contains("image/webp");
        Accepted {
            webp,
            avif: webp && accept.contains("image/avif"),
        }
    }
}

/// The largest dimension served, like Shopify's CDN.
const MAX_DIMENSION: u32 = 5760;

impl Transform {
    /// Reads the transformation from decoded query parameters.
    pub fn from_query<'a>(query: impl IntoIterator<Item = (&'a str, &'a str)>) -> Transform {
        let mut transform = Transform::default();
        let mut region = [None; 4];
        for (key, value) in query {
            let number = value.parse::<u32>().ok();
            match key {
                "width" => transform.width = number.map(|width| width.clamp(1, MAX_DIMENSION)),
                "height" => transform.height = number.map(|height| height.clamp(1, MAX_DIMENSION)),
                "crop" => {
                    transform.crop = match value {
                        "top" => Some(Crop::Top),
                        "center" => Some(Crop::Center),
                        "bottom" => Some(Crop::Bottom),
                        "left" => Some(Crop::Left),
                        "right" => Some(Crop::Right),
                        _ => None,
                    }
                }
                "crop_left" => region[0] = number,
                "crop_top" => region[1] = number,
                "crop_width" => region[2] = number,
                "crop_height" => region[3] = number,
                "pad_color" => {
                    transform.pad_color =
                        Color::parse(&format!("#{}", value.trim_start_matches('#'))).map(|color| {
                            [
                                color.red.round() as u8,
                                color.green.round() as u8,
                                color.blue.round() as u8,
                            ]
                        });
                }
                "format" => transform.format = Some(value.to_lowercase()),
                "quality" => {
                    transform.quality = value
                        .parse::<u8>()
                        .ok()
                        .map(|quality| quality.clamp(1, 100))
                }
                _ => {}
            }
        }
        if let [Some(left), Some(top), Some(width), Some(height)] = region {
            transform.region = Some((left, top, width, height));
        }
        transform
    }

    /// Applies the size suffix of a legacy image URL (`shirt_480x480_crop_center@2x.jpg`).
    /// Returns the file name without the suffix when one was recognised.
    pub fn strip_legacy_suffix(&mut self, file_name: &str) -> Option<String> {
        let (stem, extension) = file_name.rsplit_once('.')?;
        let (stem, scale) = match stem.rsplit_once('@') {
            Some((rest, scale)) if scale.ends_with('x') => {
                (rest, scale.trim_end_matches('x').parse::<u32>().ok())
            }
            _ => (stem, None),
        };
        let (stem, crop) = match stem.rsplit_once("_crop_") {
            Some((rest, crop)) => (rest, Some(crop)),
            None => (stem, None),
        };
        let (base, suffix) = stem.rsplit_once('_')?;
        let (width, height) = legacy_size(suffix)?;
        let scale = scale.unwrap_or(1).clamp(1, 3);
        self.width = width.map(|width| width * scale);
        self.height = height.map(|height| height * scale);
        self.crop = match crop {
            Some("top") => Some(Crop::Top),
            Some("bottom") => Some(Crop::Bottom),
            Some("left") => Some(Crop::Left),
            Some("right") => Some(Crop::Right),
            Some(_) => Some(Crop::Center),
            None => None,
        };
        Some(format!("{base}.{extension}"))
    }
}

/// The size an image of `(width, height)` ends up with after a transformation, and whether it
/// has to be cropped to it. Images are never enlarged.
fn output_size(source: (u32, u32), transform: &Transform) -> (u32, u32, bool) {
    let (source_width, source_height) = (f64::from(source.0.max(1)), f64::from(source.1.max(1)));
    let ratio = source_width / source_height;
    let fill = transform.crop.is_some() || transform.pad_color.is_some();
    let (width, height) = match (transform.width, transform.height) {
        (Some(width), Some(height)) if fill => {
            // Keep the requested ratio, shrinking the box if the source is smaller than it.
            let scale = (source_width / f64::from(width))
                .min(source_height / f64::from(height))
                .min(1.0);
            (f64::from(width) * scale, f64::from(height) * scale)
        }
        (Some(width), Some(height)) => {
            let scale = (f64::from(width) / source_width)
                .min(f64::from(height) / source_height)
                .min(1.0);
            (source_width * scale, source_height * scale)
        }
        (Some(width), None) => {
            let width = f64::from(width).min(source_width);
            (width, width / ratio)
        }
        (None, Some(height)) => {
            let height = f64::from(height).min(source_height);
            (height * ratio, height)
        }
        (None, None) => (source_width, source_height),
    };
    (
        (width.round() as u32).max(1),
        (height.round() as u32).max(1),
        fill && transform.width.is_some() && transform.height.is_some(),
    )
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Format {
    Jpeg,
    Png,
    Gif,
    WebP,
    Avif,
}

/// The qualities Shopify's CDN appears to encode with when the URL gives none: with them, the
/// files weigh about what its own do.
const JPEG_QUALITY: u8 = 85;
const WEBP_QUALITY: u8 = 90;
const AVIF_QUALITY: u8 = 85;

/// The fastest setting of the AVIF encoder: an image is encoded while a page waits for it.
const AVIF_SPEED: u8 = 10;

impl Format {
    fn content_type(self) -> &'static str {
        match self {
            Format::Jpeg => "image/jpeg",
            Format::Png => "image/png",
            Format::Gif => "image/gif",
            Format::WebP => "image/webp",
            Format::Avif => "image/avif",
        }
    }
}

/// The content type of a file that is served as it is.
fn content_type(extension: &str) -> &'static str {
    match extension {
        "png" => "image/png",
        "gif" => "image/gif",
        "webp" => "image/webp",
        "bmp" => "image/bmp",
        _ => "image/jpeg",
    }
}

/// Encodes a lossy WebP with libwebp: the `image` crate only writes lossless ones.
fn encode_webp(image: &DynamicImage, quality: u8) -> Result<Vec<u8>, String> {
    let failed = || "the image could not be encoded as WebP".to_string();
    let width = c_int::try_from(image.width()).map_err(|_| failed())?;
    let height = c_int::try_from(image.height()).map_err(|_| failed())?;
    let quality = f32::from(quality);
    let mut output: *mut u8 = std::ptr::null_mut();
    // SAFETY: the pixels are `height` rows of `stride` bytes, which is what the encoder is told
    // to read, and they live until it returns.
    let size = if image.color().has_alpha() {
        let pixels = image.to_rgba8();
        unsafe {
            libwebp_sys::WebPEncodeRGBA(
                pixels.as_ptr(),
                width,
                height,
                width * 4,
                quality,
                &mut output,
            )
        }
    } else {
        let pixels = image.to_rgb8();
        unsafe {
            libwebp_sys::WebPEncodeRGB(
                pixels.as_ptr(),
                width,
                height,
                width * 3,
                quality,
                &mut output,
            )
        }
    };
    if size == 0 || output.is_null() {
        return Err(failed());
    }
    // SAFETY: the encoder succeeded, so `output` points to `size` bytes that it allocated. They
    // are copied, then given back to it.
    let bytes = unsafe { std::slice::from_raw_parts(output, size) }.to_vec();
    unsafe { libwebp_sys::WebPFree(output.cast()) };
    Ok(bytes)
}

fn encode(image: &DynamicImage, format: Format, quality: Option<u8>) -> Result<Vec<u8>, String> {
    let mut out = Cursor::new(Vec::new());
    let written = match format {
        Format::Png => image.write_to(&mut out, ImageFormat::Png),
        Format::Gif => image.write_to(&mut out, ImageFormat::Gif),
        Format::Jpeg => image
            .to_rgb8()
            .write_with_encoder(JpegEncoder::new_with_quality(
                &mut out,
                quality.unwrap_or(JPEG_QUALITY),
            )),
        Format::WebP => return encode_webp(image, quality.unwrap_or(WEBP_QUALITY)),
        Format::Avif => {
            let encoder = AvifEncoder::new_with_speed_quality(
                &mut out,
                AVIF_SPEED,
                quality.unwrap_or(AVIF_QUALITY),
            );
            if image.color().has_alpha() {
                image.to_rgba8().write_with_encoder(encoder)
            } else {
                image.to_rgb8().write_with_encoder(encoder)
            }
        }
    };
    written.map_err(|error| error.to_string())?;
    Ok(out.into_inner())
}

/// The format for the clients that read neither WebP nor AVIF: the one of the source, or the
/// one the URL asks for.
fn classic_format(extension: &str, transform: &Transform, has_alpha: bool) -> Format {
    match transform.format.as_deref() {
        // A JPEG has no transparency: Shopify keeps the format of an image that has some.
        Some("pjpg" | "jpg" | "jpeg") if !has_alpha => Format::Jpeg,
        Some("png") => Format::Png,
        _ => match extension {
            "png" => Format::Png,
            "gif" => Format::Gif,
            "webp" if has_alpha => Format::Png,
            _ => Format::Jpeg,
        },
    }
}

/// Whether a file is served as it is: nothing is asked of it, and the client reads its format
/// and no lighter one. A GIF is never converted, as it may be animated.
pub fn served_as_is(extension: &str, transform: &Transform, accepted: Accepted) -> bool {
    *transform == Transform::default()
        && (extension == "gif" || (!accepted.webp && extension != "webp"))
}

/// Encodes an image for a client, the way Shopify's CDN chooses a format: the classic one for
/// the clients that read nothing else, and otherwise the lightest of that one, WebP and AVIF.
/// `original` is the file itself, when nothing was asked of it.
fn lightest(
    image: &DynamicImage,
    original: Option<Vec<u8>>,
    extension: &str,
    transform: &Transform,
    accepted: Accepted,
) -> Result<(Vec<u8>, &'static str), String> {
    let classic = classic_format(extension, transform, image.color().has_alpha());
    // `format: 'pjpg'` asks for a progressive JPEG, which neither WebP nor AVIF is.
    let converts = classic != Format::Gif
        && !(classic == Format::Jpeg && transform.format.as_deref() == Some("pjpg"));
    let mut candidates = Vec::with_capacity(3);
    let mut has_webp = false;
    match original {
        Some(bytes) if extension != "webp" || accepted.webp => {
            has_webp = extension == "webp";
            candidates.push((bytes, content_type(extension)));
        }
        _ => candidates.push((
            encode(image, classic, transform.quality)?,
            classic.content_type(),
        )),
    }
    for (format, wanted) in [
        (Format::WebP, accepted.webp && !has_webp),
        (Format::Avif, accepted.avif),
    ] {
        // An image too large for a format is served in another one.
        if wanted
            && converts
            && let Ok(bytes) = encode(image, format, transform.quality)
        {
            candidates.push((bytes, format.content_type()));
        }
    }
    // The first of the lightest: the classic format when nothing beats it.
    candidates
        .into_iter()
        .min_by_key(|(bytes, _)| bytes.len())
        .ok_or_else(|| "the image could not be encoded".to_string())
}

/// Resizes and re-encodes an image file for a client. Returns the bytes and their content type.
pub fn transform_file(
    path: &Path,
    transform: &Transform,
    accepted: Accepted,
) -> Result<(Vec<u8>, &'static str), String> {
    let extension = path
        .extension()
        .map(|extension| extension.to_string_lossy().to_lowercase())
        .unwrap_or_default();
    let failed = |error: &dyn std::fmt::Display| format!("{}: {error}", path.display());
    let bytes = std::fs::read(path).map_err(|error| failed(&error))?;
    if served_as_is(&extension, transform, accepted) {
        return Ok((bytes, content_type(&extension)));
    }
    let format = ImageFormat::from_path(path)
        .or_else(|_| image::guess_format(&bytes))
        .map_err(|error| failed(&error))?;
    let mut image =
        image::load_from_memory_with_format(&bytes, format).map_err(|error| failed(&error))?;
    let original = (*transform == Transform::default()).then_some(bytes);
    if let Some((left, top, width, height)) = transform.region {
        let left = left.min(image.width().saturating_sub(1));
        let top = top.min(image.height().saturating_sub(1));
        image = image.crop_imm(
            left,
            top,
            width.min(image.width() - left).max(1),
            height.min(image.height() - top).max(1),
        );
    }
    let (width, height, fill) = output_size((image.width(), image.height()), transform);
    let resized = if (width, height) == (image.width(), image.height()) {
        image
    } else if !fill {
        image.resize_exact(width, height, FilterType::CatmullRom)
    } else if let Some(color) = transform.pad_color.filter(|_| transform.crop.is_none()) {
        // Fit inside the box and pad the rest.
        let fitted = image.resize(width, height, FilterType::CatmullRom);
        let mut canvas = RgbImage::from_pixel(width, height, Rgb(color));
        let (x, y) = ((width - fitted.width()) / 2, (height - fitted.height()) / 2);
        image::imageops::overlay(&mut canvas, &fitted.to_rgb8(), i64::from(x), i64::from(y));
        DynamicImage::ImageRgb8(canvas)
    } else {
        // Cover the box, then cut what sticks out on the side opposite to the anchor.
        let scale = (f64::from(width) / f64::from(image.width()))
            .max(f64::from(height) / f64::from(image.height()));
        let cover_width = ((f64::from(image.width()) * scale).round() as u32).max(width);
        let cover_height = ((f64::from(image.height()) * scale).round() as u32).max(height);
        let covered = image.resize_exact(cover_width, cover_height, FilterType::CatmullRom);
        let (x, y) = match transform.crop.unwrap_or(Crop::Center) {
            Crop::Top => ((cover_width - width) / 2, 0),
            Crop::Bottom => ((cover_width - width) / 2, cover_height - height),
            Crop::Left => (0, (cover_height - height) / 2),
            Crop::Right => (cover_width - width, (cover_height - height) / 2),
            Crop::Center => ((cover_width - width) / 2, (cover_height - height) / 2),
        };
        covered.crop_imm(x, y, width, height)
    };
    lightest(&resized, original, &extension, transform, accepted)
}

/// A 5×7 bitmap font for the placeholder labels: digits, capitals and a few symbols.
fn glyph(c: char) -> [u8; 7] {
    match c.to_ascii_uppercase() {
        '0' => [0x0e, 0x11, 0x13, 0x15, 0x19, 0x11, 0x0e],
        '1' => [0x04, 0x0c, 0x04, 0x04, 0x04, 0x04, 0x0e],
        '2' => [0x0e, 0x11, 0x01, 0x02, 0x04, 0x08, 0x1f],
        '3' => [0x1f, 0x02, 0x04, 0x02, 0x01, 0x11, 0x0e],
        '4' => [0x02, 0x06, 0x0a, 0x12, 0x1f, 0x02, 0x02],
        '5' => [0x1f, 0x10, 0x1e, 0x01, 0x01, 0x11, 0x0e],
        '6' => [0x06, 0x08, 0x10, 0x1e, 0x11, 0x11, 0x0e],
        '7' => [0x1f, 0x01, 0x02, 0x04, 0x08, 0x08, 0x08],
        '8' => [0x0e, 0x11, 0x11, 0x0e, 0x11, 0x11, 0x0e],
        '9' => [0x0e, 0x11, 0x11, 0x0f, 0x01, 0x02, 0x0c],
        'A' => [0x0e, 0x11, 0x11, 0x1f, 0x11, 0x11, 0x11],
        'B' => [0x1e, 0x11, 0x11, 0x1e, 0x11, 0x11, 0x1e],
        'C' => [0x0e, 0x11, 0x10, 0x10, 0x10, 0x11, 0x0e],
        'D' => [0x1c, 0x12, 0x11, 0x11, 0x11, 0x12, 0x1c],
        'E' => [0x1f, 0x10, 0x10, 0x1e, 0x10, 0x10, 0x1f],
        'F' => [0x1f, 0x10, 0x10, 0x1e, 0x10, 0x10, 0x10],
        'G' => [0x0e, 0x11, 0x10, 0x17, 0x11, 0x11, 0x0f],
        'H' => [0x11, 0x11, 0x11, 0x1f, 0x11, 0x11, 0x11],
        'I' => [0x0e, 0x04, 0x04, 0x04, 0x04, 0x04, 0x0e],
        'J' => [0x07, 0x02, 0x02, 0x02, 0x02, 0x12, 0x0c],
        'K' => [0x11, 0x12, 0x14, 0x18, 0x14, 0x12, 0x11],
        'L' => [0x10, 0x10, 0x10, 0x10, 0x10, 0x10, 0x1f],
        'M' => [0x11, 0x1b, 0x15, 0x15, 0x11, 0x11, 0x11],
        'N' => [0x11, 0x11, 0x19, 0x15, 0x13, 0x11, 0x11],
        'O' => [0x0e, 0x11, 0x11, 0x11, 0x11, 0x11, 0x0e],
        'P' => [0x1e, 0x11, 0x11, 0x1e, 0x10, 0x10, 0x10],
        'Q' => [0x0e, 0x11, 0x11, 0x11, 0x15, 0x12, 0x0d],
        'R' => [0x1e, 0x11, 0x11, 0x1e, 0x14, 0x12, 0x11],
        'S' => [0x0f, 0x10, 0x10, 0x0e, 0x01, 0x01, 0x1e],
        'T' => [0x1f, 0x04, 0x04, 0x04, 0x04, 0x04, 0x04],
        'U' => [0x11, 0x11, 0x11, 0x11, 0x11, 0x11, 0x0e],
        'V' => [0x11, 0x11, 0x11, 0x11, 0x11, 0x0a, 0x04],
        'W' => [0x11, 0x11, 0x11, 0x15, 0x15, 0x15, 0x0a],
        'X' => [0x11, 0x11, 0x0a, 0x04, 0x0a, 0x11, 0x11],
        'Y' => [0x11, 0x11, 0x11, 0x0a, 0x04, 0x04, 0x04],
        'Z' => [0x1f, 0x01, 0x02, 0x04, 0x08, 0x10, 0x1f],
        '-' => [0x00, 0x00, 0x00, 0x1f, 0x00, 0x00, 0x00],
        '_' => [0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x1f],
        '.' => [0x00, 0x00, 0x00, 0x00, 0x00, 0x0c, 0x0c],
        '/' => [0x01, 0x01, 0x02, 0x04, 0x08, 0x10, 0x10],
        _ => [0; 7],
    }
}

fn draw_text(canvas: &mut RgbImage, text: &str, x: u32, y: u32, scale: u32, color: Rgb<u8>) {
    for (index, c) in text.chars().enumerate() {
        let rows = glyph(c);
        let origin_x = x + index as u32 * 6 * scale;
        for (row, bits) in rows.iter().enumerate() {
            for column in 0..5u32 {
                if bits & (0x10 >> column) == 0 {
                    continue;
                }
                for dy in 0..scale {
                    for dx in 0..scale {
                        let (px, py) =
                            (origin_x + column * scale + dx, y + row as u32 * scale + dy);
                        if px < canvas.width() && py < canvas.height() {
                            canvas.put_pixel(px, py, color);
                        }
                    }
                }
            }
        }
    }
}

/// Draws a placeholder for an image that only exists in the fixtures: a soft gradient whose hue
/// derives from the file name, labelled with the name and the size it stands for.
pub fn placeholder(
    src: &str,
    declared: (u32, u32),
    transform: &Transform,
    accepted: Accepted,
) -> Result<(Vec<u8>, &'static str), String> {
    let source = match transform.region {
        Some((_, _, width, height)) => (width.max(1), height.max(1)),
        None => declared,
    };
    let (width, height, _) = output_size(source, transform);
    let hash = u64::from_str_radix(&crate::util::short_hash(src)[..8], 16).unwrap_or(0);
    let hue = (hash % 360) as f64;
    let top = Color::from_hsl(hue, 0.32, 0.86, 1.0);
    let bottom = Color::from_hsl(hue + 24.0, 0.36, 0.72, 1.0);
    let mut canvas = RgbImage::new(width, height);
    for y in 0..height {
        let t = f64::from(y) / f64::from(height.max(1));
        let mix = |a: f64, b: f64| (a + (b - a) * t).round() as u8;
        let row = Rgb([
            mix(top.red, bottom.red),
            mix(top.green, bottom.green),
            mix(top.blue, bottom.blue),
        ]);
        for x in 0..width {
            canvas.put_pixel(x, y, row);
        }
    }
    // A label, when there is room for one.
    let name = src.rsplit('/').next().unwrap_or(src);
    let stem = name.rsplit_once('.').map_or(name, |(stem, _)| stem);
    let ink = Color::from_hsl(hue, 0.30, 0.34, 1.0);
    let ink = Rgb([
        ink.red.round() as u8,
        ink.green.round() as u8,
        ink.blue.round() as u8,
    ]);
    for (line, text) in [stem.to_string(), format!("{}X{}", declared.0, declared.1)]
        .iter()
        .enumerate()
    {
        let scale = (width / (text.chars().count() as u32 * 6).max(1) * 7 / 10).clamp(1, 6);
        let text_width = text.chars().count() as u32 * 6 * scale;
        if text_width + 8 > width || 20 * scale > height {
            continue;
        }
        let x = (width - text_width) / 2;
        let y = height / 2 + (line as u32 * 10 * scale) - 8 * scale;
        draw_text(
            &mut canvas,
            text,
            x,
            y.min(height.saturating_sub(8 * scale)),
            scale,
            ink,
        );
    }
    let extension = src.rsplit('.').next().unwrap_or("jpg").to_lowercase();
    lightest(
        &DynamicImage::ImageRgb8(canvas),
        None,
        &extension,
        transform,
        accepted,
    )
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use image::Rgba;

    use super::*;

    const CLASSIC: Accepted = Accepted {
        webp: false,
        avif: false,
    };
    const WEBP: Accepted = Accepted {
        webp: true,
        avif: false,
    };
    const MODERN: Accepted = Accepted {
        webp: true,
        avif: true,
    };

    /// A picture with the gradients and the texture of a photograph, and as much `grain` as
    /// asked. `alpha` fades it out towards the right.
    fn photograph(width: u32, height: u32, grain: f64, alpha: bool) -> DynamicImage {
        let picture = image::RgbaImage::from_fn(width, height, |x, y| {
            let fade = 255 - x * 255 / width;
            let speck =
                (x.wrapping_mul(2654435761) ^ y.wrapping_mul(40503)).wrapping_mul(2246822519) >> 24;
            let noise = (f64::from(speck) / 255.0 - 0.5) * grain;
            let (x, y) = (f64::from(x), f64::from(y));
            let wave = ((x / 7.0).sin() + (y / 5.0).cos()) * 24.0 + noise;
            Rgba([
                (x / f64::from(width) * 180.0 + wave + 50.0) as u8,
                (y / f64::from(height) * 150.0 + wave + 50.0) as u8,
                ((x + y) / f64::from(width + height) * 140.0 - wave + 50.0) as u8,
                if alpha { fade as u8 } else { 255 },
            ])
        });
        if alpha {
            DynamicImage::ImageRgba8(picture)
        } else {
            DynamicImage::ImageRgb8(DynamicImage::ImageRgba8(picture).to_rgb8())
        }
    }

    /// A file of the tests' own directory.
    fn file(name: &str) -> PathBuf {
        let directory = std::env::temp_dir().join(format!("lsf-images-{}", std::process::id()));
        std::fs::create_dir_all(&directory).unwrap();
        directory.join(name)
    }

    /// Saves an image in the format its name says.
    fn saved(name: &str, image: &DynamicImage) -> PathBuf {
        let path = file(name);
        image.save(&path).unwrap();
        path
    }

    fn query(pairs: &[(&'static str, &'static str)]) -> Transform {
        Transform::from_query(pairs.iter().copied())
    }

    #[test]
    fn reads_what_a_client_accepts_as_shopify_does() {
        let accepted = |accept| Accepted::from_header(Some(accept));
        assert_eq!(
            accepted("image/avif,image/webp,image/apng,image/svg+xml,image/*,*/*;q=0.8"),
            MODERN
        );
        assert_eq!(accepted("image/webp,*/*"), WEBP);
        assert_eq!(accepted("Image/WebP"), WEBP);
        // Naming a format is enough, whatever its weight.
        assert_eq!(accepted("image/avif;q=0,image/webp;q=0.1"), MODERN);
        // AVIF goes to the clients that read WebP too.
        assert_eq!(accepted("image/avif"), CLASSIC);
        assert_eq!(accepted("image/png,image/svg+xml,image/*;q=0.8"), CLASSIC);
        assert_eq!(accepted("*/*"), CLASSIC);
        assert_eq!(Accepted::from_header(None), CLASSIC);
    }

    #[test]
    fn serves_the_lightest_format_a_client_reads() {
        let path = saved("photo.jpg", &photograph(480, 360, 0.0, false));
        let half = query(&[("width", "240")]);
        let (jpeg, kind) = transform_file(&path, &half, CLASSIC).unwrap();
        assert_eq!(kind, "image/jpeg");
        let (webp, kind) = transform_file(&path, &half, WEBP).unwrap();
        assert_eq!(kind, "image/webp");
        assert!(webp.len() < jpeg.len());
        let decoded = image::load_from_memory(&webp).unwrap();
        assert_eq!((decoded.width(), decoded.height()), (240, 180));
        // AVIF is not lighter for this picture: the client that reads it gets the WebP.
        assert_eq!(
            transform_file(&path, &half, MODERN).unwrap(),
            (webp, "image/webp")
        );

        // It is for a picture with grain.
        let grainy = saved("grainy.jpg", &photograph(480, 360, 20.0, false));
        let (webp, kind) = transform_file(&grainy, &half, WEBP).unwrap();
        assert_eq!(kind, "image/webp");
        let (avif, kind) = transform_file(&grainy, &half, MODERN).unwrap();
        assert_eq!(kind, "image/avif");
        assert_eq!(&avif[4..12], b"ftypavif");
        assert!(avif.len() < webp.len());

        // The quality of the URL is the one of every format.
        let low = query(&[("width", "240"), ("quality", "30")]);
        for accepted in [CLASSIC, WEBP, MODERN] {
            let (light, _) = transform_file(&grainy, &low, accepted).unwrap();
            let (heavy, _) = transform_file(&grainy, &half, accepted).unwrap();
            assert!(light.len() < heavy.len(), "{accepted:?}");
        }

        // A file of which nothing is asked is sent as it is, unless a lighter format is read.
        let original = std::fs::read(&path).unwrap();
        let untouched = Transform::default();
        assert_eq!(
            transform_file(&path, &untouched, CLASSIC).unwrap(),
            (original.clone(), "image/jpeg")
        );
        let (lighter, kind) = transform_file(&path, &untouched, MODERN).unwrap();
        assert_ne!(kind, "image/jpeg");
        assert!(lighter.len() < original.len());

        // A format that weighs more than the file is not served: a checkerboard is light as
        // a well compressed PNG, and heavy in the formats made for photographs.
        let checkerboard = file("checkerboard.png");
        let encoder = image::codecs::png::PngEncoder::new_with_quality(
            std::fs::File::create(&checkerboard).unwrap(),
            image::codecs::png::CompressionType::Best,
            image::codecs::png::FilterType::NoFilter,
        );
        RgbImage::from_fn(32, 32, |x, y| {
            Rgb(if (x + y) % 2 == 0 {
                [255, 255, 255]
            } else {
                [20, 20, 20]
            })
        })
        .write_with_encoder(encoder)
        .unwrap();
        assert_eq!(
            transform_file(&checkerboard, &untouched, MODERN).unwrap(),
            (std::fs::read(&checkerboard).unwrap(), "image/png")
        );
    }

    #[test]
    fn keeps_the_format_an_image_needs() {
        // A progressive JPEG is asked for: it is one for every client.
        let photo = saved("progressive.jpg", &photograph(240, 180, 0.0, false));
        let progressive = query(&[("width", "120"), ("format", "pjpg")]);
        assert_eq!(
            transform_file(&photo, &progressive, MODERN).unwrap().1,
            "image/jpeg"
        );
        // `jpg` only names the classic format.
        let jpg = query(&[("width", "120"), ("format", "jpg")]);
        assert_ne!(
            transform_file(&photo, &jpg, MODERN).unwrap().1,
            "image/jpeg"
        );

        // An opaque PNG becomes a JPEG when the URL asks for one, a transparent one does not.
        let opaque = saved("opaque.png", &photograph(240, 180, 0.0, false));
        let faded = saved("faded.png", &photograph(240, 180, 0.0, true));
        assert_eq!(
            transform_file(&opaque, &jpg, CLASSIC).unwrap().1,
            "image/jpeg"
        );
        assert_eq!(
            transform_file(&faded, &jpg, CLASSIC).unwrap().1,
            "image/png"
        );
        let (webp, kind) = transform_file(&faded, &jpg, WEBP).unwrap();
        assert_eq!(kind, "image/webp");
        assert!(image::load_from_memory(&webp).unwrap().color().has_alpha());

        // A WebP file is only for the clients that read WebP.
        let modern = saved("modern.webp", &photograph(120, 90, 0.0, false));
        let untouched = Transform::default();
        assert_eq!(
            transform_file(&modern, &untouched, CLASSIC).unwrap().1,
            "image/jpeg"
        );
        assert_eq!(
            transform_file(&modern, &untouched, WEBP).unwrap(),
            (std::fs::read(&modern).unwrap(), "image/webp")
        );
        let transparent = saved("transparent.webp", &photograph(120, 90, 0.0, true));
        assert_eq!(
            transform_file(&transparent, &untouched, CLASSIC).unwrap().1,
            "image/png"
        );

        // A GIF may be animated: it stays one.
        let gif = saved("animation.gif", &photograph(120, 90, 0.0, false));
        let small = query(&[("width", "60")]);
        assert_eq!(transform_file(&gif, &small, MODERN).unwrap().1, "image/gif");
        assert_eq!(
            transform_file(&gif, &untouched, MODERN).unwrap(),
            (std::fs::read(&gif).unwrap(), "image/gif")
        );
    }

    #[test]
    fn computes_output_sizes() {
        let transform = |query: &[(&str, &str)]| Transform::from_query(query.iter().copied());
        assert_eq!(
            output_size((1600, 2000), &transform(&[("width", "400")])),
            (400, 500, false)
        );
        assert_eq!(
            output_size((1600, 2000), &transform(&[("height", "400")])),
            (320, 400, false)
        );
        assert_eq!(
            output_size(
                (1600, 2000),
                &transform(&[("width", "400"), ("height", "400")])
            ),
            (320, 400, false)
        );
        assert_eq!(
            output_size(
                (1600, 2000),
                &transform(&[("width", "400"), ("height", "400"), ("crop", "center")])
            ),
            (400, 400, true)
        );
        // Never enlarged.
        assert_eq!(
            output_size((100, 50), &transform(&[("width", "400")])),
            (100, 50, false)
        );
    }

    #[test]
    fn parses_legacy_suffixes() {
        let mut transform = Transform::default();
        assert_eq!(
            transform
                .strip_legacy_suffix("shirt_480x480_crop_top@2x.jpg")
                .as_deref(),
            Some("shirt.jpg")
        );
        assert_eq!(
            (transform.width, transform.height, transform.crop),
            (Some(960), Some(960), Some(Crop::Top))
        );
        let mut transform = Transform::default();
        assert_eq!(
            transform.strip_legacy_suffix("shirt_large.jpg").as_deref(),
            Some("shirt.jpg")
        );
        assert_eq!(transform.width, Some(480));
        assert_eq!(
            Transform::default().strip_legacy_suffix("my_shirt.jpg"),
            None
        );
    }

    #[test]
    fn draws_placeholders() {
        let transform = Transform::from_query([("width", "200")]);
        let (bytes, content_type) = placeholder(
            "products/tee.jpg",
            (1600, 2000),
            &transform,
            Accepted::default(),
        )
        .unwrap();
        assert_eq!(content_type, "image/jpeg");
        let decoded = image::load_from_memory(&bytes).unwrap();
        assert_eq!((decoded.width(), decoded.height()), (200, 250));

        // A placeholder is as light as an image of the store would be.
        let (lighter, content_type) =
            placeholder("products/tee.jpg", (1600, 2000), &transform, MODERN).unwrap();
        assert_ne!(content_type, "image/jpeg");
        assert!(lighter.len() < bytes.len());
    }
}
