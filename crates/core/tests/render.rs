//! End-to-end rendering of a small fixture theme against the demo store.
//!
//! The expected HTML lives in `tests/snapshots/`. After an intentional change, regenerate the
//! snapshots with `UPDATE_SNAPSHOTS=1 cargo test -p lsf-core --test render` and review the diff.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use lsf_core::render::Target;
use lsf_core::render::page::{Page, Resource};
use lsf_core::store::load::{DataSource, LoadOptions, load};
use lsf_core::store::{CartLine, Store};
use lsf_core::theme::Revalidate;
use lsf_core::{FormResult, Rendered, Renderer, Request, Session, Theme};

struct Fixture {
    renderer: Renderer,
    store: Arc<Store>,
}

fn fixture() -> Fixture {
    let directory = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/theme");
    let env = Arc::new(lsf_core::environment());
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

    /// Renders a piece of Liquid as if it were part of the page at `path`.
    fn liquid(&self, path: &str, session: Session, source: &str) -> String {
        let site = self
            .renderer
            .site(self.store.clone(), request(path), session);
        let page = lsf_core::render::routes::resolve(&site);
        let (output, errors) = self
            .renderer
            .render_liquid(&site, page, source, &[])
            .expect("the template parses");
        assert!(errors.is_empty(), "{errors:?}");
        output
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
fn contact_forms_come_back_to_their_own_id() {
    let fixture = fixture();
    let site = fixture.renderer.site(
        fixture.store.clone(),
        request("/"),
        Session::initial(&fixture.store),
    );
    let (output, errors) = fixture
        .renderer
        .render_liquid(
            &site,
            Page::new("index", Resource::Index),
            "{% form 'contact', id: 'ContactForm', class: 'isolate' %}{% endform %}\n\
             {% form 'customer', id: 'ContactFooter', class: 'newsletter-form' %}{% endform %}\n\
             {% form 'contact' %}{% endform %}",
            &[],
        )
        .expect("the forms parse");
    assert!(errors.is_empty(), "{errors:?}");
    // The first two are what Shopify renders for the contact and newsletter forms of Dawn.
    for form in [
        r#"<form method="post" action="/contact#ContactForm" id="ContactForm" accept-charset="UTF-8" class="isolate">"#,
        r#"<form method="post" action="/contact#ContactFooter" id="ContactFooter" accept-charset="UTF-8" class="newsletter-form">"#,
        r#"<form method="post" action="/contact#contact_form" id="contact_form" accept-charset="UTF-8" class="contact-form">"#,
    ] {
        assert!(output.contains(form), "{form} is not in {output}");
    }
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
            selling_plan: None,
        },
        // Six pairs reach the second quantity price break.
        CartLine {
            variant_id: socks.variants[0].id,
            quantity: 6,
            properties: Default::default(),
            selling_plan: None,
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

    // The id of a template section names its template, so another page can ask for it: a
    // cart drawer on a product page loads a section of the cart template this way.
    let elsewhere = fixture.render_with(
        request("/pages/about"),
        Session::initial(&fixture.store),
        &Target::Section(id.clone()),
    );
    assert_eq!(elsewhere.status, 200);
    assert!(elsewhere.body.starts_with(&format!(
        "<div id=\"shopify-section-{id}\" class=\"shopify-section hero\">"
    )));
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

#[test]
fn option_values_and_filter_values_have_swatches() {
    let fixture = fixture();
    let session = || Session::initial(&fixture.store);
    let output = fixture.liquid(
        "/products/organic-cotton-t-shirt",
        session(),
        "{% for option in product.options_with_values %}{{ option.name }}:\
         {% for value in option.values %} {{ value }}={{ value.swatch.color }}/{{ value.swatch.color.rgb }}\
         {% if value.swatch.image %}+image{% endif %}{% endfor %}\n{% endfor %}",
    );
    assert_eq!(
        output,
        "Color: White=#f4f1ea/244 241 234 Black=#1c1c1c/28 28 28 Sage=#9caf88/156 175 136\n\
         Size: S=/ M=/ L=/ XL=/\n"
    );

    // A filter whose values have swatches is presented with them.
    let output = fixture.liquid(
        "/collections/all",
        session(),
        "{% for filter in collection.filters %}{% if filter.presentation == 'swatch' %}{{ filter.label }}:\
         {% for value in filter.values %} {{ value.label }}={{ value.swatch.color }}{% endfor %}\n\
         {% endif %}{% endfor %}\
         {{ collection.filters | where: 'presentation', 'text' | map: 'label' | join: ', ' }}",
    );
    assert_eq!(
        output,
        "Color: White=#f4f1ea Black=#1c1c1c Sage=#9caf88 Navy=#1f2a44 Ochre=#c98a2b\n\
         Glaze: Oat=#d9c7a7 Slate=#5b6770 Rust=#a4502f\n\
         Availability, Price, Product type, Size, Denomination"
    );
}

#[test]
fn variants_can_be_picked_up_at_the_locations_that_stock_them() {
    let fixture = fixture();
    let mug = fixture.store.product_by_handle("ceramic-mug").unwrap();
    let template = "{% assign variant = product.selected_or_first_available_variant %}\
        {% for availability in variant.store_availabilities %}\
        {{ availability.location.name }}|{{ availability.available }}|{{ availability.pick_up_enabled }}|\
        {{ availability.pick_up_time }}|{{ availability.location.address.city }}|\
        {{ availability.location.latitude }}\n{% endfor %}\
        {% for variant in product.variants %}{{ variant.store_availabilities.size }}{% endfor %}";
    // Every location stocks the first available variant. As on Shopify, the other variants
    // have no store availabilities.
    assert_eq!(
        fixture.liquid(
            "/products/ceramic-mug",
            Session::initial(&fixture.store),
            template
        ),
        "Ottawa flagship|true|true|Usually ready in 2 hours|Ottawa|45.4201\n\
         Montreal studio|true|true|Usually ready in 24 hours|Montreal|\n\
         200"
    );
    // The selected variant has them too. This one is out of stock in Montreal.
    assert_eq!(
        fixture.liquid(
            &format!("/products/ceramic-mug?variant={}", mug.variants[1].id),
            Session::initial(&fixture.store),
            template
        ),
        "Ottawa flagship|true|true|Usually ready in 2 hours|Ottawa|45.4201\n\
         Montreal studio|false|true|Usually ready in 24 hours|Montreal|\n\
         220"
    );
}

#[test]
fn products_are_sold_with_selling_plans() {
    let fixture = fixture();
    let socks = fixture
        .store
        .product_by_handle("merino-crew-socks")
        .unwrap();
    let quarterly = fixture.store.selling_plan_groups[0].plans[1].id;
    let template = "{{ product.requires_selling_plan }} {{ product.selling_plan_groups.size }} \
        {{ product.selling_plan_groups.first.name }}|{{ product.selected_selling_plan.name }}|\
        {{ product.selected_selling_plan_allocation.price }}|\
        {{ product.selected_or_first_available_selling_plan_allocation.price }}|\
        {{ product.selling_plan_groups.first.selling_plan_selected }}|\
        {{ product.selling_plan_groups.first.options.first.selected_value }}\n\
        {% for allocation in product.variants.last.selling_plan_allocations %}\
        {{ allocation.selling_plan.name }}: {{ allocation.price | money }} instead of \
        {{ allocation.compare_at_price | money }}\
        {% if allocation.selling_plan.selected %} (selected){% endif %}\n{% endfor %}";
    assert_eq!(
        fixture.liquid(
            "/products/merino-crew-socks",
            Session::initial(&fixture.store),
            template
        ),
        "false 1 Subscribe and save|||1440|false|\n\
         Deliver every month, 10% off: $14.40 instead of $16.00\n\
         Deliver every 3 months, 5% off: $15.20 instead of $16.00\n"
    );
    // `?selling_plan=` selects a plan; with `?variant=` it selects an allocation.
    assert_eq!(
        fixture.liquid(
            &format!(
                "/products/merino-crew-socks?selling_plan={quarterly}&variant={}",
                socks.variants[1].id
            ),
            Session::initial(&fixture.store),
            template
        ),
        "false 1 Subscribe and save|Deliver every 3 months, 5% off|1520|1520|true|Every 3 months\n\
         Deliver every month, 10% off: $14.40 instead of $16.00\n\
         Deliver every 3 months, 5% off: $15.20 instead of $16.00 (selected)\n"
    );

    // The `json` filter describes plans as `/products/<handle>.js` does.
    let json: serde_json::Value = serde_json::from_str(&fixture.liquid(
        "/products/merino-crew-socks",
        Session::initial(&fixture.store),
        "{{ product | json }}",
    ))
    .expect("JSON");
    let group = &json["selling_plan_groups"][0];
    assert_eq!(group["id"], fixture.store.selling_plan_groups[0].id);
    assert_eq!(
        group["options"],
        serde_json::json!([{"name": "Delivery frequency", "position": 1, "values": ["Every month", "Every 3 months"]}])
    );
    assert_eq!(
        group["selling_plans"][1],
        serde_json::json!({
            "id": quarterly,
            "name": "Deliver every 3 months, 5% off",
            "description": null,
            "options": [{"name": "Delivery frequency", "position": 1, "value": "Every 3 months"}],
            "recurring_deliveries": true,
            "price_adjustments": [{"order_count": null, "position": 1, "value_type": "percentage", "value": 5}],
            "checkout_charge": {"value_type": "percentage", "value": 100}
        })
    );
    assert_eq!(
        json["variants"][0]["selling_plan_allocations"][1],
        serde_json::json!({
            "price_adjustments": [{"position": 1, "price": 1520}],
            "price": 1520,
            "compare_at_price": 1600,
            "per_delivery_price": 1520,
            "selling_plan_id": quarterly,
            "selling_plan_group_id": fixture.store.selling_plan_groups[0].id
        })
    );

    // A product sold without selling plans.
    assert_eq!(
        fixture.liquid(
            &format!("/products/ceramic-mug?selling_plan={quarterly}"),
            Session::initial(&fixture.store),
            "{{ product.selling_plan_groups.size }}|{{ product.selected_selling_plan }}|\
             {{ product.selected_or_first_available_selling_plan_allocation }}|\
             {{ product.variants.first.selling_plan_allocations.size }}"
        ),
        "0|||0"
    );
}

#[test]
fn cart_lines_are_priced_by_their_selling_plan() {
    let fixture = fixture();
    let socks = fixture
        .store
        .product_by_handle("merino-crew-socks")
        .unwrap();
    let monthly = fixture.store.selling_plan_groups[0].plans[0].id;
    let mut session = Session::initial(&fixture.store);
    session.cart_lines = vec![
        // Quantity price breaks do not apply to a subscription: its plan sets the price.
        CartLine {
            variant_id: socks.variants[0].id,
            quantity: 6,
            properties: Default::default(),
            selling_plan: Some(monthly),
        },
        CartLine {
            variant_id: socks.variants[0].id,
            quantity: 1,
            properties: Default::default(),
            selling_plan: None,
        },
    ];
    assert_ne!(
        lsf_core::drops::cart::line_key(&session.cart_lines[0]),
        lsf_core::drops::cart::line_key(&session.cart_lines[1])
    );
    assert_eq!(
        fixture.liquid(
            "/cart",
            session,
            "{% for item in cart.items %}{{ item.quantity }} x {{ item.price }}/{{ item.original_price }} = \
             {{ item.line_price }} [{{ item.selling_plan_allocation.selling_plan.name }}]\n{% endfor %}\
             {{ cart.total_price }} {{ cart.total_discount }} {{ cart.checkout_charge_amount }}"
        ),
        "6 x 1440/1440 = 8640 [Deliver every month, 10% off]\n\
         1 x 1600/1600 = 1600 []\n\
         10240 0 10240"
    );
}
