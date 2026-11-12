use std::path::Path;
use std::process::ExitCode;

use lsf_core::theme::Revalidate;

use crate::app::App;
use crate::server::{ServeOptions, ServerState, routes};

/// The options of a server that a command runs without serving: it answers what the command
/// asks, and prints nothing.
pub(super) fn quiet_options() -> ServeOptions {
    ServeOptions {
        polaris: Default::default(),
        admin: None,
        live_reload: false,
        watch: false,
        quiet: true,
        compress: false,
        minify: false,
        throttle: Default::default(),
        timings: false,
        customer: None,
        checkout: None,
    }
}

#[derive(clap::Args)]
pub struct Args {
    /// Also list the states a page has beyond its URL: each route in every other language of
    /// the store (`/fr/…`), a search that finds a product, and the sections the theme's
    /// JavaScript fetches (product recommendations, predictive search).
    #[arg(long)]
    all: bool,
}

/// The sections of the JSON templates of `template` whose type reads `object` (`recommendations`):
/// their ids, as the Section Rendering API names them.
fn sections_reading(theme: &lsf_theme::Theme, template: &str, object: &str) -> Vec<String> {
    let Ok(Some(json)) = theme.template_json(&format!("templates/{template}.json")) else {
        return Vec::new();
    };
    let mut ids: Vec<String> = json
        .sections
        .iter()
        .filter(|(_, section)| {
            theme
                .files()
                .read(&format!("sections/{}.liquid", section.kind))
                .is_some_and(|source| source.contains(&format!("{object}.")))
        })
        .map(|(key, _)| lsf_theme::render::template_section_id(template, key))
        .collect();
    ids.sort();
    ids
}

/// What `--all` adds to the routes: a search that finds the first product, the sections the
/// theme fetches, and every route in each other language of the store.
fn all_routes(state: &ServerState, mut routes: Vec<String>) -> Vec<String> {
    let loaded = state.loaded();
    let store = &loaded.store;
    let theme = &state.app.theme;
    let encode = |text: &str| form_urlencoded::byte_serialize(text.as_bytes()).collect::<String>();
    if let Some(product) = store.products.first() {
        let query = encode(&product.title);
        routes.push(format!("/search?q={query}"));
        // The product recommendations of a product page, fetched once it is shown.
        for id in sections_reading(theme, "product", "recommendations") {
            routes.push(format!(
                "/recommendations/products?product_id={}&limit=4&intent=related&section_id={}",
                product.id,
                encode(&id)
            ));
        }
        // The predictive search of the header, fetched as the visitor types.
        if theme.files().exists("sections/predictive-search.liquid") {
            routes.push(format!(
                "/search/suggest?q={query}&section_id=predictive-search"
            ));
        }
    }
    let primary = routes.clone();
    // Every language at `/` (`/fr`), then every language under each market's subfolders
    // (`/en-ca`, `/fr-ca`).
    let mut roots: Vec<String> = store
        .languages
        .iter()
        .filter(|language| !language.primary)
        .map(|language| language.root_url.trim_end_matches('/').to_string())
        .collect();
    for market in store
        .markets
        .iter()
        .filter(|market| market.subfolder_suffix.is_some())
    {
        roots.extend(
            store
                .presence_languages(Some(market))
                .into_iter()
                .map(|language| store.locale_root(language, Some(market))),
        );
    }
    for root in roots {
        routes.extend(primary.iter().map(|route| {
            if route == "/" {
                root.clone()
            } else {
                format!("{root}{route}")
            }
        }));
    }
    routes
}

pub fn run(
    theme: &Path,
    data: Option<&Path>,
    previews: &[lsf_theme::Preview],
    args: Args,
) -> Result<ExitCode, String> {
    let app = App::open(theme, data, Revalidate::Never, previews)?;
    let (state, _) = ServerState::new(app, quiet_options());
    let mut listed = routes(&state);
    if args.all {
        listed = all_routes(&state, listed);
    }
    for route in listed {
        println!("{route}");
    }
    Ok(ExitCode::SUCCESS)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn all_routes_add_the_languages_a_search_and_the_sections_the_theme_fetches() {
        let root = std::env::temp_dir().join(format!("lsf-routes-all-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        for (path, content) in [
            ("layout/theme.liquid", "{{ content_for_layout }}"),
            ("templates/index.liquid", "home"),
            (
                "templates/product.json",
                r#"{"sections": {"main": {"type": "main"}, "related": {"type": "related"}}, "order": ["main", "related"]}"#,
            ),
            ("sections/main.liquid", "{{ product.title }}"),
            (
                "sections/related.liquid",
                "{% for p in recommendations.products %}{{ p.title }}{% endfor %}",
            ),
            (
                "sections/predictive-search.liquid",
                "{{ predictive_search.performed }}",
            ),
        ] {
            let path = root.join(path);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, content).unwrap();
        }
        let app = App::open(&root, None, Revalidate::Never, &[]).unwrap();
        let (state, _) = ServerState::new(app, quiet_options());
        let some = routes(&state);
        let all = all_routes(&state, some.clone());
        let related = lsf_theme::render::template_section_id("product", "related");
        let product = state.loaded().store.products[0].clone();
        let added = [
            format!("/search?q={}", product.title.replace(' ', "+")),
            format!(
                "/recommendations/products?product_id={}&limit=4&intent=related&section_id={}",
                product.id,
                form_urlencoded::byte_serialize(related.as_bytes()).collect::<String>()
            ),
            format!(
                "/search/suggest?q={}&section_id=predictive-search",
                product.title.replace(' ', "+")
            ),
        ];
        assert_eq!(all[..some.len()], some[..]);
        assert_eq!(all[some.len()..some.len() + 3], added[..], "{all:#?}");
        // The demo store has French: every route again under /fr.
        let french = &all[some.len() + 3..];
        assert_eq!(french.len(), some.len() + 3);
        assert_eq!(french[0], "/fr");
        assert!(french.iter().all(|route| route.starts_with("/fr")));
        // Each one renders, the main section of the product page reads no recommendations.
        assert!(!all.iter().any(|route| route.contains("__main")));
        for route in &all {
            let (status, errors, body) = state.answer_get(
                route,
                "shop.test",
                state.initial_session(&state.loaded().store),
            );
            assert!(status < 400 && errors == 0, "{route}: {status} {body}");
        }
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn the_account_is_rendered_as_the_server_answers_it() {
        let theme = Path::new(env!("CARGO_MANIFEST_DIR")).join("../theme/tests/fixtures/theme");
        let app = App::open(&theme, None, Revalidate::Never, &[]).unwrap();
        let (state, _) = ServerState::new(app, quiet_options());
        let store = state.loaded().store.clone();
        let mut session = state.initial_session(&store);
        session.customer_id = Some(store.customers[0].id);
        let (status, _, body) = state.answer_get("/account", "shop.test", session);
        assert_eq!(status, 200);
        assert!(
            body.contains(&format!(
                "data-lsf-customer=\"{}\"",
                store.customers[0].email
            )),
            "{body}"
        );
    }
}
