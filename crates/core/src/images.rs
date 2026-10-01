//! The local stand-in for Shopify's image CDN: resizes, crops and re-encodes images on demand,
//! and draws placeholders for images the fixtures mention but that do not exist on disk.

use std::io::Cursor;
use std::path::Path;

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
    /// `jpg`, `png`, `webp` or `gif`. Defaults to the format of the source.
    pub format: Option<String>,
    pub quality: Option<u8>,
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

fn encode(
    image: DynamicImage,
    format: &str,
    quality: Option<u8>,
) -> Result<(Vec<u8>, &'static str), String> {
    let mut out = Cursor::new(Vec::new());
    let content_type = match format {
        "png" => {
            image
                .write_to(&mut out, ImageFormat::Png)
                .map_err(|error| error.to_string())?;
            "image/png"
        }
        "webp" => {
            image
                .write_to(&mut out, ImageFormat::WebP)
                .map_err(|error| error.to_string())?;
            "image/webp"
        }
        "gif" => {
            image
                .write_to(&mut out, ImageFormat::Gif)
                .map_err(|error| error.to_string())?;
            "image/gif"
        }
        _ => {
            let encoder =
                image::codecs::jpeg::JpegEncoder::new_with_quality(&mut out, quality.unwrap_or(82));
            image
                .to_rgb8()
                .write_with_encoder(encoder)
                .map_err(|error| error.to_string())?;
            "image/jpeg"
        }
    };
    Ok((out.into_inner(), content_type))
}

fn output_format(extension: &str, transform: &Transform) -> String {
    match transform.format.as_deref() {
        Some("pjpg" | "jpg" | "jpeg") => "jpg".to_string(),
        Some(format @ ("png" | "webp" | "gif")) => format.to_string(),
        _ => match extension.to_lowercase().as_str() {
            "png" => "png".to_string(),
            "webp" => "webp".to_string(),
            "gif" => "gif".to_string(),
            _ => "jpg".to_string(),
        },
    }
}

/// Resizes and re-encodes an image file. Returns the bytes and their content type.
pub fn transform_file(
    path: &Path,
    transform: &Transform,
) -> Result<(Vec<u8>, &'static str), String> {
    let extension = path
        .extension()
        .map(|extension| extension.to_string_lossy().to_lowercase())
        .unwrap_or_default();
    let mut image = image::open(path).map_err(|error| format!("{}: {error}", path.display()))?;
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
    encode(
        resized,
        &output_format(&extension, transform),
        transform.quality,
    )
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
    let extension = src.rsplit('.').next().unwrap_or("jpg");
    encode(
        DynamicImage::ImageRgb8(canvas),
        &output_format(extension, transform),
        transform.quality,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

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
        let (bytes, content_type) =
            placeholder("products/tee.jpg", (1600, 2000), &transform).unwrap();
        assert_eq!(content_type, "image/jpeg");
        let decoded = image::load_from_memory(&bytes).unwrap();
        assert_eq!((decoded.width(), decoded.height()), (200, 250));
    }
}
