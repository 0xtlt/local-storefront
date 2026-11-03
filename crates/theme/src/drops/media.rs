//! `image`, `media` (image, video, external video, model) and their parts.

use std::any::Any;
use std::borrow::Cow;

use lsf_liquid::number::float_to_s;
use lsf_liquid::{Object, Value};
use serde_json::json;

use super::{Memo, SiteRef, hash};
use crate::store::{Image, Media, MediaKind, MediaSource, VideoHost};
use crate::urls;

/// The path Shopify prints for an image: relative to the CDN's shop directory.
pub fn image_path(image: &Image) -> String {
    format!("files/{}", image.src)
}

/// The full URL of an image, without transformation parameters.
pub fn image_base_url(site: &SiteRef, image: &Image) -> String {
    urls::file_url(site, &image.src)
}

/// `25.0% 75.5%`, the CSS `object-position` of a focal point.
pub fn focal_point_css(point: (f64, f64)) -> String {
    let round = |value: f64| (value * 10_000.0).round() / 10_000.0;
    format!(
        "{}% {}%",
        float_to_s(round(point.0)),
        float_to_s(round(point.1))
    )
}

pub struct ImageDrop {
    pub site: SiteRef,
    pub image: Image,
    /// Set for product images.
    pub product: Option<usize>,
    /// 1-based position among the product's media.
    pub position: Option<usize>,
    memo: Memo,
}

impl ImageDrop {
    pub fn value(site: &SiteRef, image: &Image) -> Value {
        Value::object(ImageDrop {
            site: site.clone(),
            image: image.clone(),
            product: None,
            position: None,
            memo: Memo::default(),
        })
    }

    pub fn optional(site: &SiteRef, image: Option<&Image>) -> Value {
        image.map_or(Value::Nil, |image| Self::value(site, image))
    }

    pub fn for_product(site: &SiteRef, image: &Image, product: usize, position: usize) -> Value {
        Value::object(ImageDrop {
            site: site.clone(),
            image: image.clone(),
            product: Some(product),
            position: Some(position),
            memo: Memo::default(),
        })
    }

    /// The variants this image is attached to.
    fn variant_indexes(&self) -> Vec<usize> {
        let (Some(product), Some(position)) = (self.product, self.position) else {
            return Vec::new();
        };
        self.site.store.products[product]
            .variants
            .iter()
            .enumerate()
            .filter(|(_, variant)| variant.media_index == Some(position - 1))
            .map(|(index, _)| index)
            .collect()
    }
}

impl Object for ImageDrop {
    fn type_name(&self) -> &str {
        "image"
    }

    fn get(&self, key: &str) -> Option<Value> {
        let image = &self.image;
        Some(match key {
            "src" => Value::from(image_path(image)),
            "width" => Value::Int(i64::from(image.width)),
            "height" => Value::Int(i64::from(image.height)),
            "aspect_ratio" => Value::Float(image.aspect_ratio()),
            "alt" => Value::from(&image.alt),
            "id" => Value::Int(image.id as i64),
            "media_type" => Value::str("image"),
            "position" => self.position.map_or(Value::Nil, Value::from),
            "product_id" => self.product.map_or(Value::Nil, |index| {
                Value::Int(self.site.store.products[index].id as i64)
            }),
            "attached_to_variant?" => Value::Bool(!self.variant_indexes().is_empty()),
            "variants" => self.memo.get("variants", || {
                let product = self.product.unwrap_or(0);
                Value::array(
                    self.variant_indexes()
                        .into_iter()
                        .map(|variant| {
                            super::product::VariantDrop::value(&self.site, product, variant)
                        })
                        .collect(),
                )
            }),
            "preview_image" => self
                .memo
                .get("preview_image", || ImageDrop::value(&self.site, image)),
            "presentation" => hash([(
                "focal_point",
                image
                    .focal_point
                    .map_or(Value::Nil, |point| Value::object(FocalPoint(point))),
            )]),
            _ => return None,
        })
    }

    fn to_value(&self) -> Option<Value> {
        Some(Value::from(image_path(&self.image)))
    }

    fn render(&self) -> Cow<'_, str> {
        Cow::Owned(image_path(&self.image))
    }

    fn to_json(&self) -> serde_json::Value {
        serde_json::Value::String(image_base_url(&self.site, &self.image))
    }

    fn identity(&self) -> Option<String> {
        Some(format!("image:{}", self.image.id))
    }

    fn as_any(&self) -> &dyn Any {
        self
    }
}

pub struct FocalPoint(pub (f64, f64));

impl Object for FocalPoint {
    fn type_name(&self) -> &str {
        "focal_point"
    }

    fn get(&self, key: &str) -> Option<Value> {
        Some(match key {
            "x" => Value::Float(self.0.0),
            "y" => Value::Float(self.0.1),
            _ => return None,
        })
    }

    fn render(&self) -> Cow<'_, str> {
        Cow::Owned(focal_point_css(self.0))
    }

    fn as_any(&self) -> &dyn Any {
        self
    }
}

/// The URL of a media source: a path inside `files/` or an absolute URL.
pub fn source_url(site: &SiteRef, url: &str) -> String {
    if url.starts_with("http://") || url.starts_with("https://") || url.starts_with("//") {
        url.to_string()
    } else {
        urls::file_url(site, url)
    }
}

fn source_value(site: &SiteRef, source: &MediaSource) -> Value {
    hash([
        ("url", Value::from(source_url(site, &source.url))),
        ("mime_type", Value::from(&source.mime_type)),
        ("format", Value::from(&source.format)),
        ("width", Value::Int(i64::from(source.width))),
        ("height", Value::Int(i64::from(source.height))),
    ])
}

/// A product media that is not an image: `video`, `external_video` or `model`.
pub struct MediaDrop {
    pub site: SiteRef,
    pub media: Media,
}

impl MediaDrop {
    /// The drop for any media: images get an image drop, as in Shopify.
    pub fn value(site: &SiteRef, media: &Media, product: Option<usize>) -> Value {
        match (&media.kind, &media.preview) {
            (MediaKind::Image, Some(image)) => match product {
                Some(product) => ImageDrop::for_product(site, image, product, media.position),
                None => ImageDrop::value(site, image),
            },
            _ => Value::object(MediaDrop {
                site: site.clone(),
                media: media.clone(),
            }),
        }
    }
}

impl Object for MediaDrop {
    fn type_name(&self) -> &str {
        self.media.media_type()
    }

    fn get(&self, key: &str) -> Option<Value> {
        let media = &self.media;
        Some(match key {
            "id" => Value::Int(media.id as i64),
            "position" => Value::from(media.position),
            "media_type" => Value::str(media.media_type()),
            "alt" => Value::from(&media.alt),
            "aspect_ratio" => Value::Float(media.aspect_ratio),
            "preview_image" => ImageDrop::optional(&self.site, media.preview.as_ref()),
            "sources" => match &media.kind {
                MediaKind::Video { sources, .. } | MediaKind::Model { sources } => Value::array(
                    sources
                        .iter()
                        .map(|source| source_value(&self.site, source))
                        .collect(),
                ),
                _ => return None,
            },
            "duration" => match &media.kind {
                MediaKind::Video { duration, .. } => Value::Int(*duration as i64),
                _ => return None,
            },
            "host" => match &media.kind {
                MediaKind::ExternalVideo { host, .. } => Value::str(match host {
                    VideoHost::Youtube => "youtube",
                    VideoHost::Vimeo => "vimeo",
                }),
                _ => return None,
            },
            "external_id" => match &media.kind {
                MediaKind::ExternalVideo { external_id, .. } => Value::from(external_id),
                _ => return None,
            },
            _ => return None,
        })
    }

    fn to_json(&self) -> serde_json::Value {
        let media = &self.media;
        let mut out = json!({
            "alt": media.alt,
            "id": media.id,
            "position": media.position,
            "media_type": media.media_type(),
            "aspect_ratio": media.aspect_ratio,
        });
        if let Some(image) = &media.preview {
            out["preview_image"] = json!({
                "aspect_ratio": image.aspect_ratio(),
                "height": image.height,
                "width": image.width,
                "src": image_base_url(&self.site, image),
            });
        }
        match &media.kind {
            MediaKind::Video { sources, duration } => {
                out["duration"] = json!(duration);
                out["sources"] = sources
                    .iter()
                    .map(|source| {
                        json!({
                            "format": source.format,
                            "height": source.height,
                            "mime_type": source.mime_type,
                            "url": source_url(&self.site, &source.url),
                            "width": source.width,
                        })
                    })
                    .collect();
            }
            MediaKind::Model { sources } => {
                out["sources"] = sources
                    .iter()
                    .map(|source| json!({"format": source.format, "mime_type": source.mime_type, "url": source_url(&self.site, &source.url)}))
                    .collect();
            }
            MediaKind::ExternalVideo { host, external_id } => {
                out["external_id"] = json!(external_id);
                out["host"] = json!(match host {
                    VideoHost::Youtube => "youtube",
                    VideoHost::Vimeo => "vimeo",
                });
            }
            MediaKind::Image => {}
        }
        out
    }

    fn identity(&self) -> Option<String> {
        Some(format!("media:{}", self.media.id))
    }

    fn as_any(&self) -> &dyn Any {
        self
    }
}

/// The JSON of an image media, as it appears in a product's `media` array.
pub fn image_media_json(site: &SiteRef, media: &Media, image: &Image) -> serde_json::Value {
    json!({
        "alt": if image.alt.is_empty() { serde_json::Value::Null } else { json!(image.alt) },
        "id": media.id,
        "position": media.position,
        "preview_image": {
            "aspect_ratio": image.aspect_ratio(),
            "height": image.height,
            "width": image.width,
            "src": image_base_url(site, image),
        },
        "aspect_ratio": image.aspect_ratio(),
        "height": image.height,
        "media_type": "image",
        "src": image_base_url(site, image),
        "width": image.width,
    })
}
