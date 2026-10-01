//! End-to-end rendering of a small fixture theme against the demo store.
//!
//! The expected HTML lives in `tests/snapshots/`. After an intentional change, regenerate the
//! snapshots with `UPDATE_SNAPSHOTS=1 cargo test -p slt-core --test render` and review the diff.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use slt_core::render::Target;
use slt_core::store::load::{DataSource, LoadOptions, load};
use slt_core::store::{CartLine, Store};
use slt_core::theme::Revalidate;
use slt_core::{FormResult, Rendered, Renderer, Request, Session, Theme};

struct Fixture {
    renderer: Renderer,
    store: Arc<Store>,
}

fn fixture() -> Fixture {
    let directory = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/theme");
    let env = Arc::new(slt_core::environment());
    let theme = Arc::new(Theme::open(&directory, env, Revalidate::Never).expect("fixture theme"));
    let options = LoadOptions {
        theme_locales: vec!["en".to_string(), "fr".to_string()],
    };
    let (store, diagnostics) = load(&DataSource::Demo, &options);
    assert!(diagnostics.is_empty(), "{diagnostics}");
    Fixture {
        renderer: Renderer::new(theme),
        store: Arc::new(store),
    }
}

fn request(path_and_query: &str) -> Request {
    let (path, query) = path_and_query
        .split_once('?')
        .unwrap_or((path_and_query, ""));
    let mut request = Request::new("shop.test", path);
    request.query = query
        .split('&')
        .filter(|pair| !pair.is_empty())
        .map(|pair| {
            let (key, value) = pair.split_once('=').unwrap_or((pair, ""));
            (key.to_string(), value.to_string())
        })
        .collect();
    request
}

impl Fixture {
    fn render_with(&self, request: Request, session: Session, target: &Target) -> Rendered {
        self.renderer
            .render(self.store.clone(), request, session, target)
    }

    fn page(&self, path: &str) -> Rendered {
        self.render_with(request(path), Session::initial(&self.store), &Target::Page)
    }
}

/// Compares with (or, with `UPDATE_SNAPSHOTS=1`, writes) a snapshot file.
fn assert_snapshot(name: &str, actual: &str) {
    let path: PathBuf = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/snapshots")
        .join(name);
    if std::env::var("UPDATE_SNAPSHOTS").is_ok() {
        std::fs::write(&path, actual).expect("write snapshot");
        return;
    }
    let expected = std::fs::read_to_string(&path)
        .unwrap_or_else(|_| panic!("missing snapshot {name}: run with UPDATE_SNAPSHOTS=1"));
    assert!(
        expected == actual,
        "snapshot {name} differs.\n--- expected\n{expected}\n--- actual\n{actual}"
    );
}

fn assert_clean(rendered: &Rendered) {
    let errors: Vec<String> = rendered.errors.iter().map(ToString::to_string).collect();
    assert!(errors.is_empty(), "Liquid errors: {errors:#?}");
}

#[test]
fn home_page() {
    let rendered = fixture().page("/");
    assert_clean(&rendered);
    assert_eq!(rendered.status, 200);
    assert_eq!(rendered.template, "index");
    assert_snapshot("index.html", &rendered.body);
}

#[test]
fn product_page() {
    let rendered = fixture().page("/products/organic-cotton-t-shirt");
    assert_clean(&rendered);
    assert_snapshot("product.html", &rendered.body);
}

#[test]
fn product_page_selects_the_variant_from_the_url() {
    let fixture = fixture();
    let tee = fixture
        .store
        .product_by_handle("organic-cotton-t-shirt")
        .unwrap();
    let black_m = tee
        .variants
        .iter()
        .find(|variant| variant.title == "Black / M")
        .unwrap();
    let rendered = fixture.page(&format!(
        "/products/organic-cotton-t-shirt?variant={}",
        black_m.id
    ));
    assert_clean(&rendered);
    assert!(rendered.body.contains(&format!(
        "<option value=\"{}\" selected>Black / M</option>",
        black_m.id
    )));
    assert!(rendered.body.contains("Color: White, Black*, Sage"));
}

#[test]
fn collection_page_paginates_sorts_and_filters() {
    let fixture = fixture();
    let rendered = fixture.page("/collections/all?page=2&sort_by=price-ascending");
    assert_clean(&rendered);
    assert_snapshot("collection-page-2.html", &rendered.body);

    let filtered = fixture.page("/collections/all?filter.v.availability=0");
    assert_clean(&filtered);
    // Availability is a variant-level filter: a product matches when one of its variants does.
    assert!(
        filtered.body.contains("<h1>Products (2/8)</h1>"),
        "{}",
        filtered.body
    );
    assert!(filtered.body.contains("Out of stock: 2 active"));
}

#[test]
fn liquid_template_with_translations_and_dates() {
    let rendered = fixture().page("/pages/about");
    assert_clean(&rendered);
    assert_snapshot("page.html", &rendered.body);
}

#[test]
fn translations_follow_the_locale() {
    let fixture = fixture();
    let mut request = request("/pages/about");
    request.locale = "fr".to_string();
    request.root = "/fr".to_string();
    let rendered = fixture.render_with(request, Session::initial(&fixture.store), &Target::Page);
    assert_clean(&rendered);
    assert!(rendered.body.contains("<html lang=\"fr\">"));
    assert!(rendered.body.contains("Bonjour Shop owner"));
    // Keys missing from the locale fall back to the theme's default language.
    assert!(rendered.body.contains("Tom &amp; &lt;b&gt;Jerry&lt;/b&gt;"));
    assert!(
        rendered
            .body
            .contains("href=\"http://shop.test/fr/pages/about\"")
    );
    // In French zero is singular.
    assert!(rendered.body.contains("|0 article</footer>"));
}

#[test]
fn forms_show_the_outcome_of_a_submission() {
    let fixture = fixture();
    let mut session = Session::initial(&fixture.store);
    session.form_result = Some(FormResult {
        form_type: "contact".to_string(),
        posted_successfully: false,
        errors: vec![("email".to_string(), "is invalid".to_string())],
        values: [("email".to_string(), "nope".to_string())]
            .into_iter()
            .collect(),
    });
    let rendered = fixture.render_with(request("/pages/contact"), session, &Target::Page);
    assert_clean(&rendered);
    assert_eq!(rendered.template, "page.contact");
    assert_snapshot("contact-with-errors.html", &rendered.body);
}

#[test]
fn cart_reflects_the_session() {
    let fixture = fixture();
    let tote = fixture.store.product_by_handle("canvas-tote-bag").unwrap();
    let socks = fixture
        .store
        .product_by_handle("merino-crew-socks")
        .unwrap();
    let mut session = Session::initial(&fixture.store);
    session.cart_lines = vec![
        CartLine {
            variant_id: tote.variants[0].id,
            quantity: 2,
            properties: Default::default(),
        },
        // Six pairs reach the second quantity price break.
        CartLine {
            variant_id: socks.variants[0].id,
            quantity: 6,
            properties: Default::default(),
        },
    ];
    let rendered = fixture.render_with(request("/cart"), session, &Target::Page);
    assert_clean(&rendered);
    assert_eq!(
        rendered.body,
        "\n2 x Canvas Tote Bag = $48.00\n6 x Merino Crew Socks - 36-40 = $72.00\ntotal $120.00 USD (8)\n"
    );
}

#[test]
fn section_rendering_api() {
    let fixture = fixture();
    let page = fixture.page("/");
    let id = page
        .body
        .split("id=\"shopify-section-")
        .find_map(|rest| rest.split('"').next().filter(|id| id.ends_with("__hero")))
        .expect("hero section id")
        .to_string();

    let one = fixture.render_with(
        request("/"),
        Session::initial(&fixture.store),
        &Target::Section(id.clone()),
    );
    assert_eq!(one.status, 200);
    assert!(one.body.starts_with(&format!(
        "<div id=\"shopify-section-{id}\" class=\"shopify-section hero\">"
    )));
    assert!(
        page.body.contains(&one.body),
        "a section renders the same alone as in the page"
    );

    let many = fixture.render_with(
        request("/"),
        Session::initial(&fixture.store),
        &Target::Sections(vec![id.clone(), "footer".to_string(), "nope".to_string()]),
    );
    let json: serde_json::Value = serde_json::from_str(&many.body).unwrap();
    assert_eq!(json[&id], one.body);
    assert!(
        json["footer"]
            .as_str()
            .unwrap()
            .starts_with("<div id=\"shopify-section-footer\"")
    );
    assert!(json["nope"].is_null());
}

#[test]
fn unknown_urls_render_the_404_template() {
    let fixture = fixture();
    for path in [
        "/nope",
        "/products/nope",
        "/collections/nope",
        "/pages/nope",
        "/blogs/journal/nope",
    ] {
        let rendered = fixture.page(path);
        assert_eq!(rendered.status, 404, "{path}");
        assert!(rendered.body.contains("<h1>Not here</h1>"), "{path}");
        assert!(rendered.body.contains("data-page-type=\"404\""), "{path}");
    }
    // The countries of the store, as the options of a selector.
    let body = fixture.page("/nope").body;
    assert!(
        body.contains(
            "<select><option value=\"United States\" data-provinces=\"[]\">United States</option>\n\
             <option value=\"Canada\" data-provinces=\"[]\">Canada</option>\n\
             <option value=\"France\" data-provinces=\"[]\">France</option></select>"
        ),
        "{body}"
    );
}

#[test]
fn stylesheet_and_javascript_tags_are_bundled() {
    let fixture = fixture();
    assert_eq!(
        fixture.renderer.compiled_stylesheet(),
        "/* sections/hero.liquid */\n.hero { display: block }\n"
    );
    assert_eq!(
        fixture.renderer.compiled_javascript(),
        "/* sections/hero.liquid */\n(function() {\nconsole.log('hero');\n})();\n"
    );
}

#[test]
fn gift_cards_have_a_page() {
    let fixture = fixture();
    let shop = fixture.store.shop.id;
    let card = &fixture.store.gift_cards[0];
    let rendered = fixture.page(&card.path(shop));
    assert_clean(&rendered);
    assert_eq!(rendered.template, "gift_card");
    let body = &rendered.body;
    assert!(
        body.contains("<p class=\"code\">WCGX-7X97-G74J-GDGC|GDGC</p>"),
        "{body}"
    );
    assert!(
        body.contains("<p class=\"balance\">$32.50 of $50.00 USD</p>"),
        "{body}"
    );
    assert!(
        body.contains("enabled=true expired=false expires=never"),
        "{body}"
    );
    assert!(
        body.contains("<p class=\"owner\">Jane / Jane / Gift Card</p>"),
        "{body}"
    );
    assert!(
        body.contains(&format!("href=\"http://shop.test{}\"", card.path(shop))),
        "{body}"
    );

    let expired = fixture.page(&fixture.store.gift_cards[1].path(shop));
    assert!(
        expired
            .body
            .contains("enabled=true expired=true expires=2020-12-31"),
        "{}",
        expired.body
    );
    assert!(
        expired
            .body
            .contains("<p class=\"owner\">nobody / nobody / no product</p>")
    );

    assert_eq!(
        fixture.page(&format!("/gift_cards/{shop}/unknown")).status,
        404
    );
}
