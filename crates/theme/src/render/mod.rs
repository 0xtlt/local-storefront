//! The rendering pipeline: request → page → template sections → layout.

pub mod cost;
pub mod globals;
pub mod hints;
pub mod page;
pub mod platform;
pub mod routes;
pub mod section;
pub mod seo;
pub mod settings;
pub mod state;

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use chrono::Utc;
use lsf_core::session::Session;
use lsf_core::store::Store;
use lsf_core::util::stable_id;
use lsf_liquid::filters::escape_html;
use lsf_liquid::{Context, Environment, PartialLoader, Profile, Profiler, Template, Value};

use self::cost::Costs;
use self::globals::Globals;
use self::page::{Page, Resource};
use self::section::{Placement, render_section};
use self::state::RenderState;
use crate::site::{Request, Site};
use crate::theme::{Layout, SectionInstance, TemplateJson, Theme};

/// The Liquid environment themes are parsed with: standard Liquid plus Shopify's tags and
/// filters, trimming whitespace the way storefronts do. What a live store has: no preview.
pub fn environment() -> Environment {
    environment_with(&[])
}

/// [`environment`], with the tags of the given feature previews.
pub fn environment_with(previews: &[crate::Preview]) -> Environment {
    let mut env = Environment::standard();
    env.set_bug_compatible_whitespace_trimming(true);
    crate::tags::register(&mut env, previews);
    crate::filters::register(&mut env);
    env
}

/// Loads the snippets `render` and `include` refer to.
struct ThemePartials {
    theme: Arc<Theme>,
    /// The snippets this render has loaded. A page renders the same few snippets hundreds of
    /// times; asking the theme each time takes locks that every other render takes too.
    loaded: Mutex<HashMap<String, Arc<Template>>>,
}

impl PartialLoader for ThemePartials {
    fn load(&self, name: &str) -> lsf_liquid::Result<Arc<Template>> {
        if let Some(template) = self.loaded.lock().expect("partials poisoned").get(name) {
            return Ok(template.clone());
        }
        let path = format!("snippets/{name}.liquid");
        match self.theme.liquid(&path)? {
            Some(file) => {
                self.loaded
                    .lock()
                    .expect("partials poisoned")
                    .insert(name.to_string(), file.template.clone());
                Ok(file.template.clone())
            }
            None => Err(lsf_liquid::Error::file_system(format!(
                "Could not find asset {path}"
            ))),
        }
    }
}

/// The frame of a profile that holds every other: the render as a whole.
pub const PROFILE_ROOT: &str = "render";

/// The most sections one request of the Section Rendering API renders, `?sections=` or the
/// `sections` of a cart request: "Requesting more than five sections returns 400 Bad Request
/// and renders nothing" (shopify.dev, use the Section Rendering API).
pub const MAX_SECTIONS: usize = 5;

/// The number of sections `ids` ask for: a section asked for twice is rendered once, and
/// counts once.
pub fn distinct_sections(ids: &[String]) -> usize {
    let mut distinct: Vec<&str> = Vec::with_capacity(ids.len());
    for id in ids {
        if !distinct.contains(&id.as_str()) {
            distinct.push(id);
        }
    }
    distinct.len()
}

/// Whether `ids` ask for more sections than [`MAX_SECTIONS`].
pub fn too_many_sections(ids: &[String]) -> bool {
    distinct_sections(ids) > MAX_SECTIONS
}

/// How a page is profiled.
#[derive(Clone, Debug, Default)]
pub struct ProfileOptions {
    /// Record every tag and every output of the templates too, which makes the render
    /// slower than it is.
    pub lines: bool,
    /// What each kind of thing costs, in points.
    pub costs: Costs,
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
    /// The entries of the `Link` header requested by `preload_tag` and friends.
    pub preloads: Vec<String>,
    /// The Liquid errors printed into the page.
    pub errors: Vec<lsf_liquid::Error>,
    /// Non-fatal problems: unknown filters, missing translations, missing assets.
    pub warnings: Vec<String>,
    /// What the page left out and why: the app blocks it names that did not render.
    pub notices: Vec<String>,
    /// What the page renders of the menus lsf made up, which the store data does not have: a
    /// note per menu ([`default_menu_note`]), in the order the page first used them. Not an
    /// error: a Shopify store cannot delete these menus, only their links are lsf's.
    pub default_menus: Vec<String>,
    /// The template that rendered the page, e.g. `product.alternate`.
    pub template: String,
    /// Why a page is the 404 page: what its URL asks for and the store does not have.
    pub not_found: Option<String>,
    /// How long it took.
    pub timings: Timings,
}

/// What a page says of a menu lsf made up that it renders: whose links they are, and how to
/// render the store's.
pub fn default_menu_note(menu: &lsf_core::store::Menu) -> String {
    format!(
        "menu \"{}\" is lsf's default ({}), not the store's: the store data has none with this \
         handle. Add it to \"menus\" to render the store's links",
        menu.handle,
        menu.link_titles()
    )
}

/// The notes of the menus lsf made up that a render used.
fn default_menu_notes(site: &Site) -> Vec<String> {
    site.default_menus()
        .iter()
        .filter_map(|handle| site.store.menu(handle))
        .map(default_menu_note)
        .collect()
}

/// How long a render took, and the parts of it.
#[derive(Clone, Debug, Default)]
pub struct Timings {
    /// The whole render.
    pub total: Duration,
    /// The template: what goes into `content_for_layout`. Only when a page was rendered.
    pub template: Option<Duration>,
    /// The layout around it. Only when a page was rendered.
    pub layout: Option<Duration>,
    /// Every section, in the order they were rendered. Their time is part of the template's
    /// or of the layout's, whichever they are in.
    pub sections: Vec<SectionTiming>,
}

#[derive(Clone, Debug)]
pub struct SectionTiming {
    /// The id of the section on the page: the one of `#shopify-section-<id>`.
    pub id: String,
    /// The type of the section: the name of its file in `sections/`.
    pub kind: String,
    pub duration: Duration,
    /// Its theme blocks, each one before the blocks it holds. Their time is part of the
    /// section's. The blocks a section writes itself, from `section.blocks`, are not apart
    /// from it: nothing tells where one ends.
    pub blocks: Vec<BlockTiming>,
}

/// A theme block of a section: a file of `blocks/`, rendered by `content_for`.
#[derive(Clone, Debug)]
pub struct BlockTiming {
    /// The id of the block on the page: the one of `#shopify-block-<id>` the first time it
    /// renders. The renders after it are `<id>-1`, `<id>-2`, and so on.
    pub id: String,
    /// Its name in the section: the key it has in the JSON template, or the `id` a static
    /// block is given.
    pub key: String,
    /// The type of the block: the name of its file in `blocks/`.
    pub kind: String,
    /// 1 for a block of the section, 2 for a block of such a block, and so on.
    pub depth: usize,
    /// Every time it was rendered: a block in a loop is rendered once per turn. The time of
    /// the blocks it holds is part of its own.
    pub durations: Vec<Duration>,
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

/// Says which of the theme's settings and translations files of the page could not be read
/// (a byte order mark, which Shopify refuses too): the render goes on without them, so their
/// settings and translations are missing from the page.
fn warn_dropped_files(ctx: &Context, site: &Site) {
    let theme = &site.theme;
    let mut paths = vec![
        "config/settings_schema.json".to_string(),
        "config/settings_data.json".to_string(),
    ];
    for locale in [site.request.locale.clone(), theme.default_locale()] {
        if let Some(path) = theme.translations_path(&locale)
            && !paths.contains(&path)
        {
            paths.push(path);
        }
    }
    for path in paths {
        if let Err(lsf_core::Error::Json { message, .. }) = theme.json(&path) {
            ctx.warn(format!(
                "{path}: {message}: the file is left out of the page"
            ));
        }
    }
}

/// The line of a JSON parse error: `invalid JSON at line 3, column 5: …` → 3.
fn json_error_line(message: &str) -> Option<u32> {
    let rest = &message[message.find("at line ")? + "at line ".len()..];
    rest.split(|c: char| !c.is_ascii_digit())
        .next()?
        .parse()
        .ok()
}

/// The page of a render that spent its budget (`lsf_liquid::RenderLimits`): Liquid's error,
/// and what lsf counted.
fn budget_page(template: &str, score: u64, took: std::time::Duration) -> String {
    let limits = lsf_liquid::RenderLimits::default();
    format!(
        "<!doctype html>\n<html><head><meta charset=\"utf-8\"><title>Liquid error</title></head><body>\n\
         <h1>Liquid error: Memory limits exceeded</h1>\n\
         <p>The render of <code>{}</code> stopped after a render score of {score} in {:.1} s: \
         lsf stops a render past {} (each body counts its nodes, each loop over a range its \
         items) or {} s, as Shopify's renderer stops a page that does not end.</p>\n</body></html>\n",
        escape_html(template),
        took.as_secs_f64(),
        limits.score,
        limits.time.map_or(0, |time| time.as_secs()),
    )
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
        self.site_with(store, request, session, |_, _| Vec::new())
    }

    /// [`Renderer::site`], with the lines of its cart as the cart transforms of the shop make
    /// them ([`Site::cart_transforms`]): `transform` gets the store as the site presents it (in
    /// the visitor's currency) and the session.
    pub fn site_with(
        &self,
        store: Arc<Store>,
        request: Request,
        session: Session,
        transform: impl FnOnce(&Store, &Session) -> Vec<Option<lsf_core::cart::TransformedLine>>,
    ) -> Arc<Site> {
        let now = store.now.unwrap_or_else(Utc::now);
        let mut site = Site::new(self.theme.clone(), store, request, session, now);
        site.cart_transforms = transform(&site.store, &site.session);
        Arc::new(site)
    }

    fn context(
        &self,
        site: &Arc<Site>,
        page: &Page,
        profiler: Option<Arc<Profiler>>,
    ) -> (Context, Arc<RenderState>, Arc<Globals>) {
        let state = Arc::new(RenderState::new(site.clone(), page.clone()));
        let globals = Arc::new(Globals::new(site.clone(), page.clone()));
        let mut builder = Context::builder(self.env.clone())
            .globals(globals.clone())
            .partials(Arc::new(ThemePartials {
                theme: self.theme.clone(),
                loaded: Mutex::new(HashMap::new()),
            }))
            .register(state.clone())
            .time_zone(site.store.shop.timezone);
        // Without a fixed `now`, `'now'` reads the clock at each use, as it does on Shopify.
        if let Some(now) = site.store.now {
            builder = builder.now(now);
        }
        if let Some(profiler) = profiler {
            builder = builder.profiler(profiler);
        }
        let ctx = builder.build();
        globals.set("settings", settings::theme_settings(&state, &ctx));
        warn_dropped_files(&ctx, site);
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
        let started = Instant::now();
        let site = self.site(store, request, session);
        let page = routes::resolve(&site);
        let mut rendered = self.render_page(&site, page, target);
        rendered.timings.total = started.elapsed();
        rendered
    }

    /// Renders an already resolved page. Used for the endpoints that render a section against
    /// a resource of their own (product recommendations, predictive search).
    pub fn render_page(&self, site: &Arc<Site>, page: Page, target: &Target) -> Rendered {
        self.render_page_with(site, page, target, None, None)
    }

    /// Renders a page whose template is `source` rather than a file of the theme: the Liquid an
    /// app proxy answers with `Content-Type: application/liquid`. As a Liquid template of the
    /// theme, it goes into `content_for_layout` of `layout/theme.liquid`, or of the layout its
    /// `{% layout %}` names (`none` for none); without the layout file, the content is the page.
    /// `name` is what its Liquid errors are reported under.
    pub fn render_source(
        &self,
        site: &Arc<Site>,
        page: Page,
        source: &str,
        name: &str,
    ) -> Rendered {
        self.render_page_with(site, page, &Target::Page, None, Some((source, name)))
    }

    /// Renders a page as [`Renderer::render_page`] does, and records what the render spends
    /// its time in: the template and the layout, every section and every theme block, and
    /// every snippet, each one inside what rendered it. Next to the time, the profile counts
    /// what the render would cost a storefront, in the points of [`cost`].
    pub fn profile_page(
        &self,
        site: &Arc<Site>,
        page: Page,
        target: &Target,
        options: &ProfileOptions,
    ) -> (Rendered, Profile) {
        let profiler = Profiler::with_costs(options.lines, options.costs.pairs());
        let rendered = {
            // What the objects of the page load is charged to the profiler of the thread.
            let _active = profiler.activate();
            let _span = profiler.span(PROFILE_ROOT, None);
            self.render_page_with(site, page, target, Some(profiler.clone()), None)
        };
        (rendered, profiler.finish())
    }

    fn render_page_with(
        &self,
        site: &Arc<Site>,
        page: Page,
        target: &Target,
        profiler: Option<Arc<Profiler>>,
        source: Option<(&str, &str)>,
    ) -> Rendered {
        let started = Instant::now();
        let (mut ctx, state, globals) = self.context(site, &page, profiler);
        // What Shopify preloads on its own comes first, then what the theme asks for.
        let mut preloads = Vec::new();
        let mut timings = Timings::default();
        // The sections the Section Rendering API asks for and the page does not have.
        let mut missing_sections = Vec::new();
        let (status, content_type, body) = match target {
            Target::Page => {
                let template_started = Instant::now();
                let (content, layout) = match source {
                    Some((source, name)) => self.render_source_template(&ctx, source, name),
                    None => self.render_template(&ctx, site, &page),
                };
                timings.template = Some(template_started.elapsed());
                globals.set("content_for_layout", Value::from(content.clone()));
                let layout = state.layout_override().unwrap_or(layout);
                let layout_started = Instant::now();
                let html = self.render_layout(&mut ctx, &layout, &page, content);
                timings.layout = Some(layout_started.elapsed());
                preloads = hints::of_head(&html, &site.request.host);
                // Shopify preloads the stylesheet it adds to the page itself.
                if let Some(url) = platform::compiled_stylesheet_url(site)
                    && html.contains(&url)
                {
                    state.add_preload(&url, &[("as", Some("style")), ("rel", Some("preload"))]);
                }
                // The app embed blocks, and the scripts and stylesheets of the app blocks.
                let html = crate::app_extensions::render::decorate(&ctx, html);
                (
                    page.status,
                    "text/html; charset=utf-8",
                    platform::decorate(site, &page, html),
                )
            }
            Target::Section(id) => match self.render_section_by_id(&ctx, site, &page, id) {
                Some(html) => (200, "text/html; charset=utf-8", html),
                None => {
                    missing_sections.push(id.clone());
                    (404, "text/html; charset=utf-8", String::new())
                }
            },
            Target::Sections(ids) if too_many_sections(ids) => {
                state.notice(format!(
                    "{} sections asked for: the Section Rendering API renders {MAX_SECTIONS} at \
                     most, and answers 400 with nothing to more",
                    distinct_sections(ids)
                ));
                (400, "text/html; charset=utf-8", String::new())
            }
            Target::Sections(ids) => {
                // A section asked for twice is rendered once, as on Shopify: one key, its
                // blocks numbered from their first render.
                let mut sections = serde_json::Map::new();
                for id in ids {
                    if sections.contains_key(id) {
                        continue;
                    }
                    let html = self.render_section_by_id(&ctx, site, &page, id);
                    if html.is_none() {
                        missing_sections.push(id.clone());
                    }
                    sections.insert(
                        id.clone(),
                        html.map_or(serde_json::Value::Null, serde_json::Value::String),
                    );
                }
                (
                    200,
                    "application/json; charset=utf-8",
                    serde_json::Value::Object(sections).to_string(),
                )
            }
        };
        // A render that spent its budget never ended on Shopify either: what it rendered is cut
        // where it stopped, and the page says why rather than show half of itself.
        let (status, content_type, body) = if ctx.budget_exceeded() {
            (
                500,
                "text/html; charset=utf-8",
                budget_page(&page.template.full(), ctx.render_score(), started.elapsed()),
            )
        } else {
            (status, content_type, body)
        };
        Rendered {
            status,
            content_type,
            body,
            preloads: {
                for hint in state.preloads() {
                    if !preloads.contains(&hint) {
                        preloads.push(hint);
                    }
                }
                preloads
            },
            errors: ctx.errors(),
            warnings: ctx.warnings(),
            notices: state.notices(),
            default_menus: default_menu_notes(site),
            template: page.template.full(),
            not_found: match (&page.resource, target) {
                (Resource::NotFound(reason), Target::Page) => Some(reason.clone()),
                _ if !missing_sections.is_empty() => {
                    Some(self.missing_sections_reason(site, &page, &missing_sections))
                }
                _ => None,
            },
            timings: Timings {
                total: started.elapsed(),
                sections: state.section_timings(),
                ..timings
            },
        }
    }

    /// The JSON template of the page, with the overrides of its contextual files for the
    /// visitor.
    fn template_json(&self, site: &Site, page: &Page) -> Option<TemplateJson> {
        site.template_json(&format!("templates/{}.json", page.template.full()))
            .ok()
            .flatten()
    }

    /// Renders the template of the page: the content that goes into `content_for_layout`.
    fn render_template(&self, ctx: &Context, site: &Site, page: &Page) -> (String, Layout) {
        let name = page.template.full();
        let default_layout = if page.template.name == "password" {
            Layout::Named("password".to_string())
        } else {
            Layout::Default
        };

        // A JSON template lists sections.
        match site.template_json(&format!("templates/{name}.json")) {
            Ok(Some(template)) => {
                // A JSON template is no Liquid file: in a profile it is named as one.
                let _span = ctx
                    .profiler()
                    .map(|profiler| profiler.span(&format!("templates/{name}.json"), None));
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
                        // A section of the template that does not render (its type has no
                        // file) is the template's error: it says where.
                        Err(error) => {
                            let path = format!("templates/{name}.json");
                            let line = self.json_line(&path, &instance.kind);
                            let error = error.with_template(Some(path.into())).with_line(line);
                            content.push_str(&ctx.handle_error(error, line));
                        }
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
            // Shopify refuses to save such a file: here it is an error of the page, counted
            // with its Liquid errors, so that `--strict` and the `X-LSF-Liquid-Errors` header
            // see it.
            Err(error) => {
                let path = format!("templates/{name}.json");
                let message = match error {
                    lsf_core::error::Error::Json { message, .. } => message,
                    other => other.to_string(),
                };
                let line = json_error_line(&message).unwrap_or(1);
                let error = lsf_liquid::Error::standard(message)
                    .with_template(Some(path.into()))
                    .with_line(line);
                return (
                    format!("<pre>{}</pre>", escape_html(&ctx.handle_error(error, line))),
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

    /// The line of a JSON file of the theme where a section of type `kind` is placed, for the
    /// errors of that section. 1 when it cannot be told.
    fn json_line(&self, path: &str, kind: &str) -> u32 {
        let needle = format!("\"{kind}\"");
        self.theme
            .files()
            .read(path)
            .and_then(|source| {
                source
                    .lines()
                    .position(|line| line.contains("\"type\"") && line.contains(&needle))
            })
            .map_or(1, |index| index as u32 + 1)
    }

    /// Renders Liquid given as text the way a Liquid template of the theme is rendered: in a
    /// scope of its own, inside the default layout unless its `{% layout %}` names another.
    fn render_source_template(&self, ctx: &Context, source: &str, name: &str) -> (String, Layout) {
        let content = match Template::parse_named(&self.env, source, Some(name)) {
            Ok(template) => match ctx.isolated() {
                Ok(mut inner) => template.render(&mut inner),
                Err(error) => ctx.handle_error(error, 1),
            },
            Err(error) => ctx.handle_error(error, 1),
        };
        (content, Layout::Default)
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
            Resource::NotFound(_) => "<h1>404 Not Found</h1>".to_string(),
            _ => format!(
                "<!-- lsf: the theme has no templates/{}.json or .liquid -->",
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
    fn render_section_by_id(
        &self,
        ctx: &Context,
        site: &Site,
        page: &Page,
        id: &str,
    ) -> Option<String> {
        let name = page.template.full();
        let render = |placement: Placement<'_>, instance: &SectionInstance| -> String {
            match render_section(ctx, &placement, instance) {
                Ok(html) => html,
                Err(error) => ctx.handle_error(error, 1),
            }
        };

        if let Some(template) = self.template_json(site, page) {
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
        // The id of a template section names its template: a page may ask for a section of
        // another template, as a cart drawer does when it loads recommendations.
        if id.starts_with("template--") {
            for other in self.theme.template_names() {
                if other == name {
                    continue;
                }
                let Some(key) = id.strip_prefix(&template_section_id(&other, "")) else {
                    continue;
                };
                let instance = site
                    .template_json(&format!("templates/{other}.json"))
                    .ok()
                    .flatten()
                    .and_then(|template| template.sections.get(key).cloned())?;
                return Some(render(
                    Placement {
                        id: id.to_string(),
                        location: "template",
                        group: None,
                    },
                    &instance,
                ));
            }
        }
        for file in self.theme.files().list("sections").iter() {
            let Some(group_name) = file
                .strip_suffix(".json")
                .filter(|_| !crate::theme::template_json::is_contextual(file))
            else {
                continue;
            };
            let prefix = crate::tags::group_id_prefix(group_name);
            let Some(key) = id.strip_prefix(&prefix) else {
                continue;
            };
            let group = site
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

    /// Why the Section Rendering API found no section: the ids asked for, and the ids the page
    /// has (its template's sections, those of the section groups).
    fn missing_sections_reason(&self, site: &Site, page: &Page, missing: &[String]) -> String {
        let name = page.template.full();
        let mut ids = Vec::new();
        if let Some(template) = self.template_json(site, page) {
            for key in &template.order {
                ids.push(template_section_id(&name, key));
            }
        }
        for file in self.theme.files().list("sections").iter() {
            let Some(group_name) = file
                .strip_suffix(".json")
                .filter(|_| !crate::theme::template_json::is_contextual(file))
            else {
                continue;
            };
            if let Ok(Some(group)) = site.template_json(&format!("sections/{file}")) {
                let prefix = crate::tags::group_id_prefix(group_name);
                ids.extend(group.order.iter().map(|key| format!("{prefix}{key}")));
            }
        }
        let quoted: Vec<String> = missing.iter().map(|id| format!("\"{id}\"")).collect();
        let mut reason = format!(
            "no section {} on the page of template {name}",
            quoted.join(", ")
        );
        let hint = missing
            .iter()
            .find_map(|id| lsf_core::util::closest_match(id, ids.iter().map(String::as_str)));
        if let Some(close) = hint {
            reason.push_str(&format!(" (did you mean \"{close}\"?)"));
        }
        if ids.is_empty() {
            reason.push_str("; it has no JSON template or section group sections");
        } else {
            reason.push_str(&format!("; its sections are {}", ids.join(", ")));
        }
        reason.push_str(", or the name of any file of sections/");
        reason
    }

    /// Renders a piece of Liquid as if it were part of the page: with the page's global objects
    /// plus the given variables. Returns the output and the Liquid errors it contains.
    pub fn render_liquid(
        &self,
        site: &Arc<Site>,
        page: Page,
        source: &str,
        variables: &[(String, Value)],
    ) -> std::result::Result<(String, Vec<lsf_liquid::Error>), lsf_liquid::Error> {
        let template = Template::parse(&self.env, source)?;
        let (ctx, _, _) = self.context(site, &page, None);
        let mut inner = ctx.isolated()?;
        for (name, value) in variables {
            inner.set(name.clone(), value.clone());
        }
        let output = template.render(&mut inner);
        Ok((output, ctx.errors()))
    }

    /// Renders `/robots.txt`: `templates/robots.txt.liquid` when the theme has one, Shopify's
    /// default otherwise. Either way it is plain text, without a layout.
    pub fn render_robots(&self, site: &Arc<Site>) -> Rendered {
        let started = Instant::now();
        let page = Page::new("robots.txt", Resource::Index);
        let (mut ctx, _, _) = self.context(site, &page, None);
        let body = match self.theme.liquid("templates/robots.txt.liquid") {
            Ok(Some(file)) => file.template.render(&mut ctx),
            Ok(None) => seo::default_robots(site),
            Err(error) => error.to_string(),
        };
        Rendered {
            status: 200,
            content_type: "text/plain; charset=utf-8",
            body,
            preloads: Vec::new(),
            errors: ctx.errors(),
            warnings: ctx.warnings(),
            notices: Vec::new(),
            default_menus: default_menu_notes(site),
            template: page.template.full(),
            not_found: None,
            timings: Timings {
                total: started.elapsed(),
                ..Timings::default()
            },
        }
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
        let (mut ctx, _, _) = self.context(&site, &page, None);
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
        for file in theme.files().list(directory).iter() {
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
        lsf_core::urls::version_of(&bundles)
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
