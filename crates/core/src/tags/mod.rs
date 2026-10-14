//! Shopify's theme tags: `section`, `sections`, `schema`, `style`, `stylesheet`, `javascript`,
//! `layout`, `content_for`, `form` and `paginate`; `block` and `partial` with the Liquid July '26
//! preview.

mod block;
mod content_for;
mod form;
mod paginate;
mod partial;
mod section;
mod simple;

use lsf_liquid::Environment;

use crate::Preview;

pub use form::FormDrop;
pub use paginate::paginate_url;
pub use section::{group_id_prefix, static_instance};

pub fn register(env: &mut Environment, previews: &[Preview]) {
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
    // How Shopify's strict parser reads their markup (see `lsf_liquid::strict2`). `block` and
    // `partial` are parsed strictly already.
    env.register_syntax("section", section::check);
    env.register_syntax("sections", section::check);
    env.register_syntax("layout", simple::check_layout);
    env.register_syntax("render", simple::check_render);
    env.register_syntax("content_for", content_for::check);
    env.register_syntax("form", form::check);
    env.register_syntax("paginate", paginate::check);
    if previews.contains(&Preview::LiquidJuly26) {
        env.register_tag("block", block::parse);
        env.register_tag("partial", partial::parse);
    }
}

#[cfg(test)]
mod tests {
    use lsf_liquid::strict2;

    fn fixed(source: &str) -> (Vec<String>, String) {
        let env = crate::render::environment_with(&[]);
        let audit = strict2::audit(&env, source);
        let messages = audit
            .findings
            .iter()
            .map(|finding| finding.issue.message.clone())
            .collect();
        (messages, strict2::apply(source, &audit.findings))
    }

    #[test]
    fn shopify_tags_follow_the_strict_parser() {
        let valid = [
            "{% section 'header' %}",
            "{% sections 'footer-group' %}",
            "{% layout none %}{% layout 'theme' %}",
            "{% render block %}",
            "{% form 'product', product, class: 'x' %}{% endform %}",
            "{% paginate collection.products by 12, window_size: 2 %}{% endpaginate %}",
            "{% content_for 'block', type: 'a', id: 'b', closest.product: p %}",
        ];
        for source in valid {
            assert_eq!(fixed(source), (Vec::new(), source.to_string()), "{source}");
        }
        let bare = "Bare bracket access is not allowed. Use self['...'] instead".to_string();
        assert_eq!(
            fixed("{% section 'a' 'b' %}"),
            (
                vec!["Expected end_of_string but found string".to_string()],
                "{% section \"a' 'b\" %}".to_string()
            )
        );
        assert_eq!(
            fixed("{% form 'product', [p] %}{% endform %}"),
            (
                vec![bare.clone()],
                "{% form 'product', self[p] %}{% endform %}".to_string()
            )
        );
        assert_eq!(
            fixed("{% content_for 'blocks', a: [b] %}"),
            (
                vec![bare],
                "{% content_for 'blocks', a: self[b] %}".to_string()
            )
        );
    }
}
