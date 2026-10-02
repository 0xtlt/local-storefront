//! The rendering pipeline: request → page → template sections → layout.

pub mod globals;
pub mod page;
pub mod routes;
pub mod section;
pub mod settings;
pub mod state;

use std::sync::Arc;

use chrono::Utc;
use slt_liquid::filters::escape_html;
use slt_liquid::{Context, Environment, PartialLoader, Template, Value};

use self::globals::Globals;
use self::page::{Page, Resource};
use self::section::{Placement, render_section};
use self::state::RenderState;
use crate::site::{Request, Session, Site};
use crate::store::Store;
use crate::theme::{Layout, SectionInstance, TemplateJson, Theme};
use crate::util::stable_id;

/// The Liquid environment themes are parsed with: standard Liquid plus Shopify's tags and
/// filters.
pub fn environment() -> Environment {
    let mut env = Environment::standard();
    crate::tags::register(&mut env);
    crate::filters::register(&mut env);
    env
}

/// Loads the snippets `render` and `include` refer to.
struct ThemePartials {
    theme: Arc<Theme>,
}

impl PartialLoader for ThemePartials {
    fn load(&self, name: &str) -> slt_liquid::Result<Arc<Template>> {
        let path = format!("snippets/{name}.liquid");
        match self.theme.liquid(&path)? {
            Some(file) => Ok(file.template.clone()),
            None => Err(slt_liquid::Error::file_system(format!(
                "Could not find asset {path}"
            ))),
        }
    }
}

/// What to render for a request.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Target {
    /// The whole page, layout included.
    Page,
    /// One section, as the Section Rendering API's `section_id` parameter asks for.
    Section(String),
    /// Several sections as a JSON object of id → HTML (`sections` parameter).
    Sections(Vec<String>),
}

#[derive(Debug)]
pub struct Rendered {
    pub status: u16,
    pub content_type: &'static str,
    pub body: String,
    /// `Link` headers requested by `preload_tag` and friends.
    pub preloads: Vec<String>,
    /// The Liquid errors printed into the page.
    pub errors: Vec<slt_liquid::Error>,
    /// Non-fatal problems: unknown filters, missing translations, missing assets.
    pub warnings: Vec<String>,
    /// The template that rendered the page, e.g. `product.alternate`.
    pub template: String,
}

pub struct Renderer {
    theme: Arc<Theme>,
    env: Arc<Environment>,
}

/// The id of a section of a JSON template: `template--<number>__<key>`.
pub fn template_section_id(template: &str, key: &str) -> String {
    format!("template--{}__{key}", stable_id("template", template))
}

/// Parses a JSON template's `wrapper` (`div#id.class[attr=value]`) into opening and closing
/// tags.
fn wrapper_tags(wrapper: &str) -> (String, String) {
    let tag_end = wrapper.find(['#', '.', '[']).unwrap_or(wrapper.len());
    let tag = if tag_end == 0 {
        "div"
    } else {
        &wrapper[..tag_end]
    };
    let mut id = None;
    let mut classes = Vec::new();
    let mut attributes = String::new();
    let mut rest = &wrapper[tag_end..];
    while let Some(marker) = rest.chars().next() {
        let body = &rest[1..];
        match marker {
            '[' => {
                let end = body.find(']').unwrap_or(body.len());
                let (name, value) = body[..end].split_once('=').unwrap_or((&body[..end], ""));
                attributes.push_str(&format!(
                    " {name}=\"{}\"",
                    escape_html(value.trim_matches(['"', '\'']))
                ));
                rest = body.get(end + 1..).unwrap_or_default();
            }
            _ => {
                let end = body.find(['#', '.', '[']).unwrap_or(body.len());
                if marker == '#' {
                    id = Some(&body[..end]);
                } else {
                    classes.push(&body[..end]);
                }
                rest = &body[end..];
            }
        }
    }
    let mut open = format!("<{tag}");
    if let Some(id) = id {
        open.push_str(&format!(" id=\"{}\"", escape_html(id)));
    }
    if !classes.is_empty() {
        open.push_str(&format!(" class=\"{}\"", escape_html(&classes.join(" "))));
    }
    open.push_str(&attributes);
    open.push('>');
    (open, format!("</{tag}>"))
}

impl Renderer {
    pub fn new(theme: Arc<Theme>) -> Self {
        Renderer {
            env: theme.environment().clone(),
            theme,
        }
    }

    pub fn theme(&self) -> &Arc<Theme> {
        &self.theme
    }

    /// The immutable description of one render.
    pub fn site(&self, store: Arc<Store>, request: Request, session: Session) -> Arc<Site> {
        let now = store.now.unwrap_or_else(Utc::now);
        Arc::new(Site {
            theme: self.theme.clone(),
            store,
            request,
            session,
            now,
        })
    }

    fn context(&self, site: &Arc<Site>, page: &Page) -> (Context, Arc<RenderState>, Arc<Globals>) {
        let state = Arc::new(RenderState::new(site.clone(), page.clone()));
        let globals = Arc::new(Globals::new(site.clone(), page.clone()));
        let ctx = Context::builder(self.env.clone())
            .globals(globals.clone())
            .partials(Arc::new(ThemePartials {
                theme: self.theme.clone(),
            }))
            .register(state.clone())
            .now(site.now)
            .time_zone(site.store.shop.timezone)
            .build();
        globals.set("settings", settings::theme_settings(&state, &ctx));
        (ctx, state, globals)
    }

    /// Renders a request.
    pub fn render(
        &self,
        store: Arc<Store>,
        request: Request,
        session: Session,
        target: &Target,
    ) -> Rendered {
        let site = self.site(store, request, session);
        let page = routes::resolve(&site);
        self.render_page(&site, page, target)
    }

    /// Renders an already resolved page. Used for the endpoints that render a section against
    /// a resource of their own (product recommendations, predictive search).
    pub fn render_page(&self, site: &Arc<Site>, page: Page, target: &Target) -> Rendered {
        let (mut ctx, state, globals) = self.context(site, &page);
        let (status, content_type, body) = match target {
            Target::Page => {
                let (content, layout) = self.render_template(&ctx, &page);
                globals.set("content_for_layout", Value::from(content.clone()));
                let layout = state.layout_override().unwrap_or(layout);
                (
                    page.status,
                    "text/html; charset=utf-8",
                    self.render_layout(&mut ctx, &layout, &page, content),
                )
            }
            Target::Section(id) => match self.render_section_by_id(&ctx, &page, id) {
                Some(html) => (200, "text/html; charset=utf-8", html),
                None => (404, "text/html; charset=utf-8", String::new()),
            },
            Target::Sections(ids) => {
                let sections: serde_json::Map<String, serde_json::Value> = ids
                    .iter()
                    .map(|id| {
                        let html = self.render_section_by_id(&ctx, &page, id);
                        (
                            id.clone(),
                            html.map_or(serde_json::Value::Null, serde_json::Value::String),
                        )
                    })
                    .collect();
                (
                    200,
                    "application/json; charset=utf-8",
                    serde_json::Value::Object(sections).to_string(),
                )
            }
        };
        Rendered {
            status,
            content_type,
            body,
            preloads: state.preloads(),
            errors: ctx.errors(),
            warnings: ctx.warnings(),
            template: page.template.full(),
        }
    }

    fn template_json(&self, page: &Page) -> Option<TemplateJson> {
        self.theme
            .template_json(&format!("templates/{}.json", page.template.full()))
            .ok()
            .flatten()
    }

    /// Renders the template of the page: the content that goes into `content_for_layout`.
    fn render_template(&self, ctx: &Context, page: &Page) -> (String, Layout) {
        let name = page.template.full();
        let default_layout = if page.template.name == "password" {
            Layout::Named("password".to_string())
        } else {
            Layout::Default
        };

        // A JSON template lists sections.
        match self.theme.template_json(&format!("templates/{name}.json")) {
            Ok(Some(template)) => {
                let mut content = String::new();
                for key in &template.order {
                    let Some(instance) = template
                        .sections
                        .get(key)
                        .filter(|section| !section.disabled)
                    else {
                        continue;
                    };
                    let placement = Placement {
                        id: template_section_id(&name, key),
                        location: "template",
                        group: None,
                    };
                    match render_section(ctx, &placement, instance) {
                        Ok(html) => content.push_str(&html),
                        Err(error) => content.push_str(&ctx.handle_error(error, 1)),
                    }
                }
                if let Some(wrapper) = &template.wrapper {
                    let (open, close) = wrapper_tags(wrapper);
                    content = format!("{open}{content}{close}");
                }
                let layout = match template.layout {
                    Layout::Default => default_layout,
                    other => other,
                };
                return (content, layout);
            }
            Ok(None) => {}
            Err(error) => {
                return (
                    format!("<pre>{}</pre>", escape_html(&error.to_string())),
                    default_layout,
                );
            }
        }

        // A Liquid template is rendered directly.
        match self.theme.liquid(&format!("templates/{name}.liquid")) {
            Ok(Some(file)) => match ctx.isolated() {
                Ok(mut inner) => (file.template.render(&mut inner), default_layout),
                Err(error) => (ctx.handle_error(error, 1), default_layout),
            },
            Ok(None) => (self.builtin_template(ctx, page), default_layout),
            Err(error) => (ctx.handle_error(error, 1), default_layout),
        }
    }

    /// What Shopify renders for pages a theme has no template for.
    fn builtin_template(&self, ctx: &Context, page: &Page) -> String {
        match &page.resource {
            Resource::Policy(_) => {
                let policy = ctx.find_variable("policy");
                format!(
                    "<div class=\"shopify-policy__container\">\n  <div class=\"shopify-policy__title\">\n    <h1>{}</h1>\n  </div>\n  <div class=\"shopify-policy__body\">\n    <div class=\"rte\">{}</div>\n  </div>\n</div>",
                    escape_html(&policy.get("title").to_str()),
                    policy.get("body").to_str()
                )
            }
            Resource::NotFound => "<h1>404 Not Found</h1>".to_string(),
            _ => format!(
                "<!-- slt: the theme has no templates/{}.json or .liquid -->",
                escape_html(&page.template.full())
            ),
        }
    }

    fn render_layout(
        &self,
        ctx: &mut Context,
        layout: &Layout,
        page: &Page,
        content: String,
    ) -> String {
        let name = match layout {
            Layout::None => return content,
            Layout::Default => "theme",
            Layout::Named(name) => name.as_str(),
        };
        let _ = page;
        match self.theme.liquid(&format!("layout/{name}.liquid")) {
            Ok(Some(file)) => file.template.render(ctx),
            // Without the layout the content is still worth showing.
            Ok(None) => {
                ctx.warn(format!("layout/{name}.liquid does not exist"));
                content
            }
            Err(error) => format!("{}{content}", ctx.handle_error(error, 1)),
        }
    }

    /// Renders one section by id, wherever it is defined: the page's JSON template, a section
    /// group, or a section file rendered with its default settings.
    fn render_section_by_id(&self, ctx: &Context, page: &Page, id: &str) -> Option<String> {
        let name = page.template.full();
        let render = |placement: Placement<'_>, instance: &SectionInstance| -> String {
            match render_section(ctx, &placement, instance) {
                Ok(html) => html,
                Err(error) => ctx.handle_error(error, 1),
            }
        };

        if let Some(template) = self.template_json(page) {
            for (key, instance) in &template.sections {
                if template_section_id(&name, key) == id {
                    return Some(render(
                        Placement {
                            id: id.to_string(),
                            location: "template",
                            group: None,
                        },
                        instance,
                    ));
                }
            }
        }
        for file in self.theme.files().list("sections") {
            let Some(group_name) = file.strip_suffix(".json") else {
                continue;
            };
            let prefix = crate::tags::group_id_prefix(group_name);
            let Some(key) = id.strip_prefix(&prefix) else {
                continue;
            };
            let group = self
                .theme
                .template_json(&format!("sections/{file}"))
                .ok()
                .flatten()?;
            let instance = group.sections.get(key)?;
            let location = group
                .group_type
                .clone()
                .unwrap_or_else(|| "custom".to_string());
            return Some(render(
                Placement {
                    id: id.to_string(),
                    location: &location,
                    group: Some(group_name),
                },
                instance,
            ));
        }
        if self.theme.files().exists(&format!("sections/{id}.liquid")) {
            let state = RenderState::of(ctx).ok()?;
            let instance = crate::tags::static_instance(state, id);
            return Some(render(
                Placement {
                    id: id.to_string(),
                    location: "static",
                    group: None,
                },
                &instance,
            ));
        }
        None
    }

    /// Renders a piece of Liquid as if it were part of the page: with the page's global objects
    /// plus the given variables. Returns the output and the Liquid errors it contains.
    pub fn render_liquid(
        &self,
        site: &Arc<Site>,
        page: Page,
        source: &str,
        variables: &[(String, Value)],
    ) -> std::result::Result<(String, Vec<slt_liquid::Error>), slt_liquid::Error> {
        let template = Template::parse(&self.env, source)?;
        let (ctx, _, _) = self.context(site, &page);
        let mut inner = ctx.isolated()?;
        for (name, value) in variables {
            inner.set(name.clone(), value.clone());
        }
        let output = template.render(&mut inner);
        Ok((output, ctx.errors()))
    }

    /// Renders a `.liquid` asset (`assets/theme.css.liquid`), which can read `settings`.
    pub fn render_asset(
        &self,
        store: Arc<Store>,
        request: Request,
        source: &str,
        name: &str,
    ) -> String {
        let site = self.site(store, request, Session::default());
        let page = Page::new("index", Resource::Index);
        let (mut ctx, _, _) = self.context(&site, &page);
        match Template::parse_named(&self.env, source, Some(name)) {
            Ok(template) => template.render(&mut ctx),
            Err(error) => format!("/* {error} */"),
        }
    }

    /// The bundle of every `{% stylesheet %}` tag in the theme's sections, blocks and snippets.
    pub fn compiled_stylesheet(&self) -> String {
        compiled(&self.theme, "stylesheet", |css| css.to_string())
    }

    /// The bundle of every `{% javascript %}` tag, each in its own scope.
    pub fn compiled_javascript(&self) -> String {
        compiled(&self.theme, "javascript", |js| {
            format!("(function() {{\n{js}\n}})();")
        })
    }
}

fn compiled(theme: &Theme, tag: &str, wrap: impl Fn(&str) -> String) -> String {
    let mut out = String::new();
    for directory in ["sections", "blocks", "snippets"] {
        for file in theme.files().list(directory) {
            if !file.ends_with(".liquid") {
                continue;
            }
            let path = format!("{directory}/{file}");
            let Some(source) = theme.files().read(&path) else {
                continue;
            };
            for body in crate::theme::schema::extract_blocks(&source, tag) {
                if body.trim().is_empty() {
                    continue;
                }
                out.push_str(&format!(
                    "/* {path} */\n{}\n",
                    wrap(body.trim_matches('\n'))
                ));
            }
        }
    }
    out
}

/// The cache-busting version of the compiled bundles: it changes when their content does.
pub fn compiled_version(theme: &Theme) -> u64 {
    theme.cached_version("compiled_assets", || {
        let bundles = format!(
            "{}{}",
            compiled(theme, "stylesheet", str::to_string),
            compiled(theme, "javascript", str::to_string)
        );
        crate::urls::version_of(&bundles)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_template_wrappers() {
        assert_eq!(
            wrapper_tags("div#main.a.b[data-x=1]"),
            (
                "<div id=\"main\" class=\"a b\" data-x=\"1\">".to_string(),
                "</div>".to_string()
            )
        );
        assert_eq!(
            wrapper_tags("section"),
            ("<section>".to_string(), "</section>".to_string())
        );
    }
}
