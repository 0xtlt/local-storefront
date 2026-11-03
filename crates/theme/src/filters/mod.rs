//! Shopify's Liquid filters.

mod cart;
mod color;
mod font;
mod html;
mod i18n;
pub mod media;
mod metafield;
mod misc;
pub mod money;
mod url;

use lsf_liquid::filters::escape_html;
use lsf_liquid::{Context, Environment, Result, Value};

use crate::render::state::RenderState;
use crate::site::Site;

pub use html::payment_icon_svg as html_payment_icon;
pub use i18n::translate;
pub use misc::to_script_safe_json as misc_json;

pub fn register(env: &mut Environment) {
    money::register(env);
    i18n::register(env);
    url::register(env);
    html::register(env);
    media::register(env);
    color::register(env);
    font::register(env);
    misc::register(env);
    cart::register(env);
    metafield::register(env);
}

/// The site of the render a filter runs in.
pub(crate) fn site(ctx: &Context) -> Result<&std::sync::Arc<Site>> {
    Ok(&RenderState::of(ctx)?.site)
}

/// Renders extra HTML attributes from keyword arguments: ` class="a" data-x="b"`.
/// A `nil` value drops the attribute; `true` repeats the name, as Shopify does for booleans.
pub(crate) fn html_attributes(named: &[(String, Value)], skip: &[&str]) -> String {
    let mut out = String::new();
    for (key, value) in named {
        if skip.contains(&key.as_str()) {
            continue;
        }
        match value {
            Value::Nil | Value::Bool(false) => {}
            Value::Bool(true) => out.push_str(&format!(" {key}=\"{key}\"")),
            other => out.push_str(&format!(" {key}=\"{}\"", escape_html(&other.to_str()))),
        }
    }
    out
}
