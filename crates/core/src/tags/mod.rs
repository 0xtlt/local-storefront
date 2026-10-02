//! Shopify's theme tags: `section`, `sections`, `schema`, `style`, `stylesheet`, `javascript`,
//! `layout`, `content_for`, `form` and `paginate`.

mod content_for;
mod form;
mod paginate;
mod section;
mod simple;

use lsf_liquid::Environment;

pub use form::FormDrop;
pub use paginate::paginate_url;
pub use section::{group_id_prefix, static_instance};

pub fn register(env: &mut Environment) {
    env.register_tag("section", section::parse_section);
    env.register_tag("sections", section::parse_sections);
    env.register_tag("schema", simple::parse_schema);
    env.register_tag("stylesheet", simple::parse_stylesheet);
    env.register_tag("javascript", simple::parse_javascript);
    env.register_tag("style", simple::parse_style);
    env.register_tag("layout", simple::parse_layout);
    env.register_tag("render", simple::parse_render);
    env.register_tag("content_for", content_for::parse);
    env.register_tag("form", form::parse);
    env.register_tag("paginate", paginate::parse);
}
