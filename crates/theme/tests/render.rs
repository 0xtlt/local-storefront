//! End-to-end rendering of a small fixture theme against the demo store.
//!
//! The expected HTML lives in `tests/snapshots/`. After an intentional change, regenerate the
//! snapshots with `UPDATE_SNAPSHOTS=1 cargo test -p lsf-theme --test render` and review the diff.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use lsf_core::session::{FormResult, Session};
use lsf_core::store::load::{DataSource, LoadOptions, load, load_with_overlay};
use lsf_core::store::{CartLine, Store};
use lsf_core::theme::Revalidate;
use lsf_theme::render::cost::Costs;
use lsf_theme::render::page::{Page, Resource};
use lsf_theme::render::{ProfileOptions, Target};
use lsf_theme::{Rendered, Renderer, Request, Theme};

struct Fixture {
    renderer: Renderer,
    store: Arc<Store>,
}

fn fixture() -> Fixture {
    let directory = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/theme");
    let env = Arc::new(lsf_theme::environment());
    let theme = Arc::new(Theme::open(&directory, env, Revalidate::Never).expect("fixture theme"));
    let options = LoadOptions {
        theme_locales: vec!["en".to_string(), "fr".to_string()],
        ..LoadOptions::default()
    };
    let (mut store, diagnostics) = load(&DataSource::Demo, &options);
    assert!(diagnostics.is_empty(), "{diagnostics}");
    // A fixed instant: the page says the shop's UTC offset, which daylight saving time moves
    // (New York, -4 h in summer, -5 h in winter), so that the snapshots hold all year.
    store.now = Some("2026-07-01T12:00:00Z".parse().expect("an RFC 3339 date"));
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
        self.liquid_for(request(path), session, source)
    }

    /// Renders a piece of Liquid as if it were part of the page of a request.
    fn liquid_for(&self, request: Request, session: Session, source: &str) -> String {
        let site = self.renderer.site(self.store.clone(), request, session);
        let page = lsf_theme::render::routes::resolve(&site);
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

/// A product with options Size (S1 to S20) and Color (C1 to C15): 300 variants.
fn product_with_300_variants() -> serde_json::Value {
    let variants: Vec<serde_json::Value> = (1..=20)
        .flat_map(|size| (1..=15).map(move |color| serde_json::json!({"options": [format!("S{size}"), format!("C{color}")]})))
        .collect();
    serde_json::json!({"title": "Big", "price": "10.00", "options": ["Size", "Color"], "variants": variants})
}

#[test]
fn product_variants_stop_at_250_and_variants_count_is_the_total() {
    // "we've restricted product.variants to return a maximum of 250 variants", and a loop over
    // it "only iterates through the first 250 variants":
    // https://shopify.dev/docs/storefronts/themes/product-merchandising/variants/support-high-variant-products
    // https://github.com/0xtlt/local-storefront/issues/82
    let overlay = serde_json::json!({"products": [product_with_300_variants()]});
    let fixture = theme_with_data(
        "variants-cap",
        &overlay,
        &[("layout/theme.liquid", "{{ content_for_layout }}")],
    );
    let session = Session::initial(&fixture.store);
    assert_eq!(
        fixture.liquid(
            "/products/big",
            session.clone(),
            "{% assign n = 0 %}{% for v in product.variants %}{% assign n = n | plus: 1 %}{% endfor %}\
             [{{ product.variants.size }}|{{ product.variants_count }}|{{ n }}|{{ product.variants.last.title }}]"
        ),
        "[250|300|250|S17 / C10]"
    );
    // The same cap in the product's JSON, which `/products/<handle>.js` answers.
    let json: serde_json::Value =
        serde_json::from_str(&fixture.liquid("/products/big", session, "{{ product | json }}"))
            .unwrap();
    assert_eq!(json["variants"].as_array().unwrap().len(), 250);
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
fn urls_built_from_the_request_lead_to_the_page_not_to_a_section_alone() {
    // A section fetched alone (`?section_id=`, `?sections=`) builds the same links as the page:
    // Shopify leaves those parameters out, puts `page` first and sorts the others by name.
    // `part.url` is "the current page URL path with the pagination parameter for the current
    // part appended": https://shopify.dev/docs/api/liquid/objects/part
    // https://github.com/0xtlt/local-storefront/issues/81
    let fixture = fixture();
    let render =
        |path: &str, source: &str| fixture.liquid(path, Session::initial(&fixture.store), source);
    let probe = "{% paginate collection.products by 1 %}next: {{ paginate.next.url }}\n\
                 previous: {{ paginate.previous.url }}{% endpaginate %}";

    assert_eq!(
        render("/collections/all?section_id=probe", probe),
        "next: /collections/all?page=2\nprevious: "
    );
    assert_eq!(
        render("/collections/all?sections=probe&page=2", probe),
        "next: /collections/all?page=3\nprevious: /collections/all?page=1"
    );
    assert_eq!(
        render(
            "/collections/all?section_id=probe&page=2&sort_by=title-ascending",
            "{% paginate collection.products by 1 %}{{ paginate.previous.url }} \
             {{ paginate.parts.first.url }} {{ paginate | default_pagination }}{% endpaginate %}"
        )
        .split("</span>")
        .next()
        .unwrap(),
        "/collections/all?page=1&sort_by=title-ascending \
         /collections/all?page=1&sort_by=title-ascending \
         <span class=\"prev\"><a href=\"/collections/all?page=1&amp;sort_by=title-ascending\" \
         title=\"\">&laquo; Previous</a>"
    );
    assert_eq!(
        render(
            "/search?q=a&section_id=probe&page=2",
            "{% paginate search.results by 1 %}{{ paginate.next.url }}{% endpaginate %}"
        ),
        "/search?page=3&q=a"
    );

    // A filter value's URLs, without the page either.
    let filters = "{% for v in collection.filters.first.values %}{{ v.url_to_add }} \
                   {{ v.url_to_remove }}\n{% endfor %}{{ collection.filters.first.url_to_remove }}";
    assert_eq!(
        render(
            "/collections/all?section_id=filters&sort_by=price-descending&page=2",
            filters
        ),
        "/collections/all?filter.v.availability=1&sort_by=price-descending \
         /collections/all?sort_by=price-descending\n\
         /collections/all?filter.v.availability=0&sort_by=price-descending \
         /collections/all?sort_by=price-descending\n\
         /collections/all?sort_by=price-descending"
    );
    // The price range's, with every other parameter sorted by name.
    assert_eq!(
        render(
            "/collections/all?sections=filters&sort_by=title-ascending&filter.v.price.gte=10\
             &filter.v.availability=1",
            "{% for f in collection.filters %}{% if f.type == 'price_range' %}\
             {{ f.url_to_remove }}{% endif %}{% endfor %}"
        ),
        "/collections/all?filter.v.availability=1&sort_by=title-ascending"
    );
}

#[test]
fn search_results_follow_sort_by() {
    // `sort_by` takes `relevance`, `price-ascending` or `price-descending`. Under a price sort the
    // non-product results (pages, articles) go to the end of the results, and `paginate` slices
    // the sorted list.
    // https://shopify.dev/docs/api/liquid/objects/search
    // https://shopify.dev/docs/storefronts/themes/navigation-search/search
    let fixture = fixture();
    let list =
        |path: &str, source: &str| fixture.liquid(path, Session::initial(&fixture.store), source);
    let prices = "{{ search.sort_by }}|{% for result in search.results %}\
                  {{ result.object_type }}:{{ result.price | default: 0 }} {% endfor %}";

    // Pages are asked for first: they still come after the products. The scented candle
    // (2800), sold out, comes last (`unavailable_products` is `last` by default).
    let relevance = list("/search?q=a&type=page,product", prices);
    assert_eq!(
        relevance,
        "relevance|page:0 page:0 page:0 product:3200 product:6800 product:2400 product:2200 \
         product:3400 product:1600 product:2500 product:2800 "
    );
    // Without `sort_by`, or with a value the search page does not know, the order is relevance.
    assert_eq!(
        list("/search?q=a&type=page,product&sort_by=relevance", prices),
        relevance
    );
    assert_eq!(
        list(
            "/search?q=a&type=page,product&sort_by=title-ascending",
            prices
        ),
        relevance
    );
    assert_eq!(
        list(
            "/search?q=a&type=page,product&sort_by=price-ascending",
            prices
        ),
        "price-ascending|product:1600 product:2200 product:2400 product:2500 product:3200 \
         product:3400 product:6800 page:0 page:0 page:0 product:2800 "
    );
    assert_eq!(
        list(
            "/search?q=a&type=page,product&sort_by=price-descending",
            prices
        ),
        "price-descending|product:6800 product:3400 product:3200 product:2500 product:2400 \
         product:2200 product:1600 page:0 page:0 page:0 product:2800 "
    );

    // The page is cut out of the sorted list.
    assert_eq!(
        list(
            "/search?q=a&type=product&sort_by=price-descending&page=2",
            "{% paginate search.results by 3 %}{% for result in search.results %}\
             {{ result.price }} {% endfor %}{% endpaginate %}"
        ),
        "2500 2400 2200 "
    );

    assert_eq!(
        list(
            "/search?q=a",
            "{% for option in search.sort_options %}{{ option.value }}={{ option.name }}; {% endfor %}\
             default {{ search.default_sort_by }}"
        ),
        "relevance=Relevance; price-ascending=Price, low to high; \
         price-descending=Price, high to low; default relevance"
    );
}

#[test]
fn storefront_filters_narrow_search_results() {
    // "Storefront filtering ... allows merchants to easily create filters for filtering
    // collection and search results pages", and "If you apply filters on the search results
    // page, then all non-product results are filtered out."
    // https://shopify.dev/docs/storefronts/themes/navigation-search/filtering/storefront-filtering
    // https://github.com/0xtlt/local-storefront/issues/158
    let fixture = fixture();
    let render =
        |path: &str, source: &str| fixture.liquid(path, Session::initial(&fixture.store), source);
    let filters = |object: &str| {
        format!(
            "{{% for f in {object}.filters %}}{{{{ f.param_name }}}}|{{{{ f.type }}}}|\
             {{{{ f.range_max }}}}|{{% for v in f.values %}}{{{{ v.value }}}}({{{{ v.count }}}}\
             {{% if v.active %}}, active{{% endif %}}){{% endfor %}}; {{% endfor %}}"
        )
    };
    // `q=a` finds the 8 products of `/collections/all`: the search has the same filters.
    let all = "{{ search.results_count }}";
    assert_eq!(render("/search?q=a&type=product", all), "8");
    for query in [
        "",
        "&filter.v.availability=1",
        "&filter.v.availability=0",
        "&filter.v.price.lte=25",
        "&filter.v.price.gte=25&filter.v.availability=1",
    ] {
        let collection = render(
            &format!("/collections/all?x=1{query}"),
            &format!(
                "{}#{{{{ collection.products_count }}}}",
                filters("collection")
            ),
        );
        let search = render(
            &format!("/search?q=a&type=product{query}"),
            &format!("{}#{{{{ search.results_count }}}}", filters("search")),
        );
        assert!(
            collection.starts_with("filter.v.availability|list|"),
            "{collection}"
        );
        assert_eq!(search, collection, "{query}");
    }
    assert_eq!(
        render("/search?q=a&type=product&filter.v.availability=0", all),
        "2"
    );

    // A filter leaves out the pages, which have no availability nor price.
    let kinds = "{% for r in search.results %}{{ r.object_type }} {% endfor %}";
    assert!(render("/search?q=a&type=page,product", kinds).contains("page "));
    let filtered = render(
        "/search?q=a&type=page,product&filter.v.availability=1",
        kinds,
    );
    assert!(!filtered.contains("page"), "{filtered}");
    assert_eq!(filtered.matches("product ").count(), 7, "{filtered}");
    // A filter's links stay on the search page, with its query.
    assert_eq!(
        render(
            "/search?q=a&type=product&filter.v.availability=1",
            "{{ search.filters[0].values[0].url_to_remove }} {{ search.filters[0].values[1].url_to_add }}"
        ),
        "/search?q=a&type=product \
         /search?filter.v.availability=1&filter.v.availability=0&q=a&type=product"
    );

    // Without products found, there is nothing to filter.
    let names = "{% for f in search.filters %}{{ f.param_name }} {% endfor %}";
    assert_eq!(render("/search?q=zzzz", names), "");
    assert_eq!(render("/search?q=a&type=page", names), "");
    assert_eq!(render("/search", names), "");
}

#[test]
fn search_hides_unavailable_products_or_puts_them_last() {
    // `unavailable_products`: "hide - Exclude unavailable products or variants. last - Show
    // unavailable products or variants after all other matching results. Defaults to last."
    // https://shopify.dev/docs/storefronts/themes/navigation-search/search
    // https://github.com/0xtlt/local-storefront/issues/160
    let fixture = fixture();
    let list =
        |path: &str, source: &str| fixture.liquid(path, Session::initial(&fixture.store), source);
    // The scented candle is the demo's sold-out product, the ceramic mug an available one.
    let results = "{{ search.results_count }}|{% for r in search.results %}\
                   {{ r.handle }}:{{ r.available }} {% endfor %}";
    let candle = "scented-candle:false ";
    let mug = "ceramic-mug:true ";
    assert_eq!(
        list(
            "/search?q=handle:ceramic-mug OR handle:scented-candle&type=product",
            results
        ),
        format!("2|{mug}{candle}")
    );
    assert_eq!(
        list(
            "/search?q=handle:ceramic-mug OR handle:scented-candle&type=product&options[unavailable_products]=last",
            results
        ),
        format!("2|{mug}{candle}")
    );
    assert_eq!(
        list(
            "/search?q=handle:ceramic-mug OR handle:scented-candle&type=product&options[unavailable_products]=hide",
            results
        ),
        format!("1|{mug}")
    );
    // `show` keeps the order of the data, where the candle comes after the mug too: the
    // candle first in a price sort shows it in its place.
    assert_eq!(
        list(
            "/search?q=handle:ceramic-mug OR handle:scented-candle&type=product&sort_by=price-descending\
             &options[unavailable_products]=show",
            results
        ),
        format!("2|{candle}{mug}")
    );
    assert_eq!(
        list(
            "/search?q=handle:ceramic-mug OR handle:scented-candle&type=product&sort_by=price-descending",
            results
        ),
        format!("2|{mug}{candle}")
    );
    // Hidden products are not counted, nor paginated.
    assert_eq!(
        list(
            "/search?q=a&type=product&options[unavailable_products]=hide",
            "{{ search.results_count }}{% for r in search.results %} {{ r.available }}{% endfor %}"
        ),
        "7 true true true true true true true"
    );
}

#[test]
fn handleize_follows_a_shopify_store() {
    // The issue's probe, with what a development store rendered (2026-11-12):
    // https://github.com/0xtlt/local-storefront/issues/85
    let fixture = fixture();
    let render = |source: &str| fixture.liquid("/", Session::initial(&fixture.store), source);
    assert_eq!(
        render(
            "[{{ \"Women's T-Shirts & Tops\" | handleize }}]\n[{{ \"L'été\" | handle }}]\n\
             [{{ 'A_B' | handleize }}]\n[{{ 'Ελληνικά Русский 日本語' | handleize }}]\n\
             [{{ '😀 smile 😀' | handleize }}]\n[{{ 'Straße Œuvre ﬁne' | handleize }}]"
        ),
        "[womens-t-shirts-tops]\n[lete]\n[a_b]\n[ελληνικά-русский-日本語]\n[😀-smile-😀]\n\
         [strasse-oeuvre-ﬁne]"
    );
    // A tag's URL is its handle, the same as `handleize` gives.
    assert_eq!(
        fixture.liquid(
            "/collections/all",
            Session::initial(&fixture.store),
            "{{ \"Women's Tops\" | link_to_tag: \"Women's Tops\" }}"
        ),
        "<a href=\"/collections/all/womens-tops\" title=\"Show products matching tag Women&#39;s Tops\">Women's Tops</a>"
    );
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
fn a_visitor_who_selected_no_country_is_in_the_shops_country() {
    // The demo shop is in Ottawa, and the United States come first in its countries: the
    // storefront opens in Canada, the country the checkout opens in.
    let fixture = fixture();
    assert_eq!(fixture.store.shop.address.country_code, "CA");
    assert_eq!(fixture.store.countries[0].iso_code, "US");
    assert_eq!(fixture.store.default_country().iso_code, "CA");
    let country = "{{ localization.country.iso_code }}";
    let output = fixture.liquid("/", Session::initial(&fixture.store), country);
    assert_eq!(output, "CA");
    assert!(fixture.page("/").body.contains("Shopify.country = \"CA\";"));
    // A selected country wins.
    let mut session = Session::initial(&fixture.store);
    session.country = Some("FR".to_string());
    assert_eq!(fixture.liquid("/", session, country), "FR");
}

#[test]
fn an_image_picked_by_its_path_has_the_size_of_its_file() {
    // Issue #116: `settings.logo` is `shopify://shop_images/logo.png`, and `files/logo.png` a
    // 400 x 500 image. Shopify gives the file's size to the setting and to `images[...]`.
    let (mut store, diagnostics) = load(&DataSource::Demo, &LoadOptions::default());
    assert!(diagnostics.is_empty(), "{diagnostics}");
    store.file_sizes.insert("logo.png".to_string(), (400, 500));
    let fixture = Fixture {
        store: Arc::new(store),
        ..fixture()
    };
    let output = fixture.liquid(
        "/",
        Session::initial(&fixture.store),
        "{{ settings.logo.width }}x{{ settings.logo.height }} {{ settings.logo.aspect_ratio }}|\
         {{ images['logo.png'].width }}x{{ images['logo.png'].height }}|\
         {{ settings.logo | image_url: width: 300 | image_tag }}",
    );
    assert!(output.starts_with("400x500 0.8|400x500|<img "), "{output}");
    assert!(output.contains(" width=\"300\" height=\"375\""), "{output}");
}

#[test]
fn a_product_image_without_alt_text_has_the_title_of_its_product() {
    // Issue #118: Shopify gives a product image without alt text the product's title, in
    // `image.alt`, `media.alt`, `image_tag` and the cart's JSON. Its `preview_image` keeps an
    // empty alt (`media.preview_image | image_url | image_tag` gives `alt=""` on Shopify).
    let overlay = serde_json::json!({"products": [
        {"title": "Widget", "price": 1000,
         "images": [{"src": "widget.jpg", "width": 1600, "height": 1600}]},
        {"title": "Gadget", "price": 1000,
         "images": [{"src": "gadget.jpg", "alt": "Gadget, front", "width": 1600, "height": 1600}]}
    ]});
    let (store, diagnostics) =
        load_with_overlay(&DataSource::Demo, &LoadOptions::default(), Some(&overlay));
    assert!(diagnostics.is_empty(), "{diagnostics}");
    let widget = store.product_by_handle("widget").expect("widget").variants[0].id;
    let fixture = Fixture {
        store: Arc::new(store),
        ..fixture()
    };
    let output = fixture.liquid(
        "/products/widget",
        Session::initial(&fixture.store),
        "{{ product.featured_image.alt }}|{{ product.featured_media.alt }}|\
         {{ product.images.first.alt }}|{{ product.media.first.preview_image.alt }}|\
         {{ all_products['gadget'].featured_image.alt }}",
    );
    assert_eq!(output, "Widget|Widget|Widget||Gadget, front");
    let tag = fixture.liquid(
        "/products/widget",
        Session::initial(&fixture.store),
        "{{ product.featured_image | image_url: width: 100 | image_tag }}",
    );
    assert!(tag.contains(" alt=\"Widget\" "), "{tag}");
    let tag = fixture.liquid(
        "/products/widget",
        Session::initial(&fixture.store),
        "{{ product.media.first.preview_image | image_url: width: 100 | image_tag }}",
    );
    assert!(tag.contains(" alt=\"\" "), "{tag}");

    let mut session = Session::initial(&fixture.store);
    session.cart_lines = vec![CartLine {
        variant_id: widget,
        quantity: 1,
        properties: Default::default(),
        selling_plan: None,
    }];
    let cart = fixture.liquid("/cart", session, "{{ cart | json }}");
    let cart: serde_json::Value = serde_json::from_str(&cart).expect("the cart's JSON");
    assert_eq!(cart["items"][0]["featured_image"]["alt"], "Widget");
}

#[test]
fn image_url_with_a_width_and_a_height_crops_from_the_center() {
    // Issue #121: Shopify adds `crop=center` when `image_url` has a width and a height and no
    // `crop` (https://shopify.dev/docs/api/liquid/filters/image_url: "The default value is
    // center"), and leaves it out with `crop: nil` and with `pad_color`. The parameters are
    // in alphabetical order.
    let fixture = fixture();
    let url = |source: &str| {
        let output = fixture.liquid(
            "/products/organic-cotton-t-shirt",
            Session::initial(&fixture.store),
            source,
        );
        let query = output.split_once('?').expect("a query").1.to_string();
        // `v=` is the file's version: the same for every URL of the image.
        query
            .split('&')
            .map(|pair| {
                if pair.starts_with("v=") {
                    "v=…"
                } else {
                    pair
                }
            })
            .collect::<Vec<_>>()
            .join("&")
    };
    let image = "product.featured_image | image_url";
    assert_eq!(
        url(&format!("{{{{ {image}: width: 300, height: 200 }}}}")),
        "crop=center&height=200&v=…&width=300"
    );
    assert_eq!(
        url(&format!(
            "{{{{ {image}: width: 300, height: 300, crop: nil }}}}"
        )),
        "height=300&v=…&width=300"
    );
    assert_eq!(
        url(&format!(
            "{{{{ {image}: width: 400, height: 400, pad_color: '000' }}}}"
        )),
        "height=400&pad_color=000&v=…&width=400"
    );
    assert_eq!(
        url(&format!(
            "{{{{ {image}: width: 400, height: 400, crop: 'bottom' }}}}"
        )),
        "crop=bottom&height=400&v=…&width=400"
    );
    assert_eq!(
        url(&format!("{{{{ {image}: width: 300 }}}}")),
        "v=…&width=300"
    );
    assert_eq!(
        url(&format!("{{{{ {image}: height: 300 }}}}")),
        "height=300&v=…"
    );
    // The tag is the box, and each width of its `srcset` keeps the crop.
    let tag = fixture.liquid(
        "/products/organic-cotton-t-shirt",
        Session::initial(&fixture.store),
        &format!("{{{{ {image}: width: 300, height: 200 | image_tag }}}}"),
    );
    assert!(tag.contains(" width=\"300\" height=\"200\""), "{tag}");
    assert!(tag.contains("crop=center&amp;height=200&amp;v="), "{tag}");
}

#[test]
fn image_tag_lists_the_width_of_image_url_and_is_the_size_it_asks_for() {
    // Issue #120, and https://shopify.dev/docs/storefronts/themes/best-practices/performance/use-filter-chains:
    // the default widths with the width of `image_url` added, or the widths of `widths:`, kept
    // up to the width of `image_url`; the tag's size is the one asked for, not the file's.
    let overlay = serde_json::json!({"products": [
        {"title": "Widget", "price": 1000,
         "images": [{"src": "widget.jpg", "alt": "W", "width": 1600, "height": 1600}]},
        {"title": "Small", "price": 1000,
         "images": [{"src": "small.jpg", "alt": "S", "width": 1024, "height": 1024}]}
    ]});
    let (store, diagnostics) =
        load_with_overlay(&DataSource::Demo, &LoadOptions::default(), Some(&overlay));
    assert!(diagnostics.is_empty(), "{diagnostics}");
    let fixture = Fixture {
        store: Arc::new(store),
        ..fixture()
    };
    // The tag with each URL reduced to its width.
    let tag = |product: &str, chain: &str| {
        let output = fixture.liquid(
            "/",
            Session::initial(&fixture.store),
            &format!("{{{{ all_products['{product}'].featured_image | {chain} }}}}"),
        );
        let mut short = String::new();
        let mut rest = output.as_str();
        while let Some(start) = rest.find("//") {
            short.push_str(&rest[..start]);
            let end = rest[start..]
                .find([' ', '"'])
                .map_or(rest.len(), |end| start + end);
            let url = &rest[start..end];
            short.push_str(url.rsplit_once("width=").map_or("…", |(_, width)| width));
            rest = &rest[end..];
        }
        short.push_str(rest);
        short
    };
    assert_eq!(
        tag("widget", "image_url: width: 400 | image_tag"),
        r#"<img src="400" alt="W" srcset="352 352w, 400 400w" width="400" height="400">"#
    );
    assert_eq!(
        tag("widget", "image_url: width: 3000 | image_tag"),
        r#"<img src="3000" alt="W" srcset="352 352w, 832 832w, 1200 1200w, 1920 1920w, 3000 3000w" width="3000" height="3000">"#
    );
    assert_eq!(
        tag(
            "widget",
            "image_url: width: 1600 | image_tag: widths: '100, 200, 2000'"
        ),
        r#"<img src="1600" alt="W" srcset="100 100w, 200 200w" width="1600" height="1600">"#
    );
    assert_eq!(
        tag(
            "small",
            "image_url: width: 1600 | image_tag: widths: '100, 200, 2000'"
        ),
        r#"<img src="1600" alt="S" srcset="100 100w, 200 200w" width="1600" height="1600">"#
    );
    // Below the smallest default width, the width asked for is the only one.
    assert_eq!(
        tag("widget", "image_url: width: 200 | image_tag"),
        r#"<img src="200" alt="W" srcset="200 200w" width="200" height="200">"#
    );
    // Nothing left after the filter: no `srcset`.
    assert_eq!(
        tag(
            "widget",
            "image_url: width: 300 | image_tag: widths: '400, 500'"
        ),
        r#"<img src="300" alt="W" width="300" height="300">"#
    );
}

#[test]
fn a_preloaded_image_tag_loads_eagerly() {
    // Issue #119: on Shopify, `image_tag: preload: true` writes `loading="eager"`, even over
    // `loading: 'lazy'`; Dawn 15's logo (`preload: true`, no `loading`) renders so on Shopify's
    // demo store. `loading` comes right after `width` and `height`, whatever the order of the
    // arguments, as in https://shopify.dev/docs/api/liquid/filters/image_tag.
    let fixture = fixture();
    let tag = |arguments: &str| {
        let output = fixture.liquid(
            "/products/organic-cotton-t-shirt",
            Session::initial(&fixture.store),
            &format!(
                "{{{{ product.featured_image | image_url: width: 400 | image_tag: {arguments} }}}}"
            ),
        );
        // What follows the size: `loading` and the attributes after it.
        output
            .split_once(" height=")
            .and_then(|(_, rest)| rest.split_once(' ').map(|(_, rest)| rest.to_string()))
            .unwrap_or_default()
    };
    assert_eq!(tag("preload: true"), "loading=\"eager\">");
    assert_eq!(tag("preload: true, loading: 'lazy'"), "loading=\"eager\">");
    assert_eq!(
        tag("class: 'card', loading: 'lazy'"),
        "loading=\"lazy\" class=\"card\">"
    );
    assert_eq!(tag("preload: false, class: 'card'"), "class=\"card\">");
    assert_eq!(tag("loading: nil, class: 'card'"), "class=\"card\">");
}

#[test]
fn image_url_raises_on_what_is_not_an_image() {
    // Issue #124: Shopify prints `invalid url input` for nil, a product or a line item without
    // an image and a URL of another site, and `image_tag` raises when `image_url` has neither a
    // width nor a height (https://shopify.dev/docs/api/liquid/filters/image_url: "If neither
    // are specified, then an error is returned"); `image_url` alone gives the URL, as in
    // Dawn's `og:image`.
    let overlay = serde_json::json!({"products": [
        {"title": "Widget", "price": 1000,
         "images": [{"src": "widget.jpg", "width": 1600, "height": 1600}]},
        {"title": "Plain", "price": 500}
    ]});
    let (store, diagnostics) =
        load_with_overlay(&DataSource::Demo, &LoadOptions::default(), Some(&overlay));
    assert!(diagnostics.is_empty(), "{diagnostics}");
    let plain = store.product_by_handle("plain").expect("plain").variants[0].id;
    let fixture = Fixture {
        store: Arc::new(store),
        ..fixture()
    };
    let mut session = Session::initial(&fixture.store);
    session.cart_lines = vec![CartLine {
        variant_id: plain,
        quantity: 1,
        properties: Default::default(),
        selling_plan: None,
    }];
    let render = |source: &str| {
        let site = fixture.renderer.site(
            fixture.store.clone(),
            request("/products/widget"),
            session.clone(),
        );
        let page = lsf_theme::render::routes::resolve(&site);
        let (output, errors) = fixture
            .renderer
            .render_liquid(&site, page, source, &[])
            .expect("the template parses");
        let errors: Vec<String> = errors.iter().map(|error| error.bare_message()).collect();
        (output, errors)
    };
    let invalid = "Liquid error (line 1): invalid url input";
    for source in [
        "{{ nothing | image_url: width: 100 }}",
        "{{ all_products['plain'].featured_image | image_url: width: 100 }}",
        "{{ all_products['plain'] | image_url: width: 100 }}",
        "{{ all_products['plain'].variants.first | image_url: width: 100 }}",
        "{{ cart.items.first.image | image_url: width: 10 }}",
        "{{ cart.items.first | image_url: width: 10 }}",
        "{{ '' | image_url: width: 100 }}",
        "{{ 'https://example.com/a.jpg' | image_url: width: 100 }}",
    ] {
        let (output, errors) = render(source);
        assert_eq!(output, invalid, "{source}");
        assert_eq!(errors, ["invalid url input"], "{source}");
    }
    let (output, errors) = render("{{ product.featured_image | image_url | image_tag }}");
    assert_eq!(
        output,
        "Liquid error (line 1): image_url must define width or height"
    );
    assert_eq!(errors.len(), 1);
    // What is an image still works: an image, its `src`, a URL of the shop's CDN, and
    // `image_url` without a size.
    let (output, errors) = render(
        "{{ product.featured_image.src | image_url: width: 100 }}|\
         {{ product.featured_image | image_url: width: 100 | image_url: width: 50 }}|\
         {{ product.featured_image | image_url }}",
    );
    assert!(errors.is_empty(), "{errors:?}");
    let urls: Vec<&str> = output.split('|').collect();
    assert!(
        urls[0].contains("/cdn/shop/files/widget.jpg?") && urls[0].ends_with("&width=100"),
        "{output}"
    );
    assert!(urls[1].ends_with("&width=50"), "{output}");
    assert!(!urls[2].contains("width="), "{output}");
}

#[test]
fn a_cart_line_of_a_product_without_an_image_has_a_featured_image_of_nulls() {
    // Issue #151: Shopify gives the line of a product without an image a `featured_image`
    // whose fields are null, in `/cart.js` and in Liquid `item | json`; `image` stays null.
    let overlay = serde_json::json!({"products": [
        {"title": "Tee", "options": ["Size"], "price": 2000,
         "variants": [{"options": ["S"], "inventory_quantity": 50}]}
    ]});
    let (store, diagnostics) =
        load_with_overlay(&DataSource::Demo, &LoadOptions::default(), Some(&overlay));
    assert!(diagnostics.is_empty(), "{diagnostics}");
    let tee = store.product_by_handle("tee").expect("tee").variants[0].id;
    let fixture = Fixture {
        store: Arc::new(store),
        ..fixture()
    };
    let mut session = Session::initial(&fixture.store);
    session.cart_lines = vec![CartLine {
        variant_id: tee,
        quantity: 1,
        properties: Default::default(),
        selling_plan: None,
    }];
    let output = fixture.liquid(
        "/cart",
        session,
        "{{ cart | json }}\n{% for item in cart.items %}{{ item | json }}{% endfor %}",
    );
    let (cart, item) = output.split_once('\n').expect("two lines");
    let cart: serde_json::Value = serde_json::from_str(cart).expect("the cart's JSON");
    let item: serde_json::Value = serde_json::from_str(item).expect("the line's JSON");
    let nulls = serde_json::json!({
        "aspect_ratio": null, "alt": null, "height": null, "url": null, "width": null
    });
    for line in [&cart["items"][0], &item] {
        assert_eq!(line["featured_image"], nulls, "{line}");
        assert_eq!(line["image"], serde_json::Value::Null);
    }
}

/// Issue #88: a line a cart transform expanded shows the price and title the transform gave
/// it, and its components in `item_components`, as Shopify's cart shows a bundle.
#[test]
fn a_line_a_cart_transform_expanded_has_its_components() {
    let fixture = fixture();
    let tote = fixture.store.product_by_handle("canvas-tote-bag").unwrap();
    let mug = fixture.store.product_by_handle("ceramic-mug").unwrap();
    let mut session = Session::initial(&fixture.store);
    session.cart_lines = vec![CartLine {
        variant_id: tote.variants[0].id,
        quantity: 1,
        properties: Default::default(),
        selling_plan: None,
    }];
    let site =
        fixture
            .renderer
            .site_with(fixture.store.clone(), request("/cart"), session, |_, _| {
                vec![Some(lsf_core::cart::TransformedLine {
                    title: Some("Duo".to_string()),
                    unit_price: 2700,
                    components: vec![lsf_core::cart::LineComponent {
                        variant_id: mug.variants[0].id,
                        quantity: 2,
                        line_price: 2700,
                    }],
                })]
            });
    let page = lsf_theme::render::routes::resolve(&site);
    let (output, errors) = fixture
        .renderer
        .render_liquid(
            &site,
            page,
            "{%- for item in cart.items %}{{ item.title }} {{ item.final_line_price }} \
             {% for c in item.item_components %}[{{ c.quantity }} x {{ c.variant.sku }} \
             {{ c.final_line_price }}]{% endfor %}{% endfor %} total:{{ cart.total_price }}",
            &[],
        )
        .expect("the template parses");
    assert!(errors.is_empty(), "{errors:?}");
    assert_eq!(
        output,
        format!("Duo 2700 [2 x {} 2700] total:2700", mug.variants[0].sku)
    );
    let json = lsf_theme::drops::cart::cart_json(&site);
    assert_eq!(json["items"][0]["has_components"], true);
    assert_eq!(json["items"][0]["final_line_price"], 2700);
    assert_eq!(json["total_price"], 2700);
}

#[test]
fn line_item_properties_are_pairs_without_a_size() {
    // Issue #152: on Shopify `item.properties | json` is a list of `[name, value]` pairs and
    // `item.properties.size` prints nothing. A loop, a lookup by name, `| size` and
    // `== empty` (the test shopify.dev's cart template uses) read the properties.
    let fixture = fixture();
    let tote = fixture
        .store
        .product_by_handle("canvas-tote-bag")
        .expect("the tote");
    let mug = fixture
        .store
        .product_by_handle("ceramic-mug")
        .expect("the mug");
    let mut session = Session::initial(&fixture.store);
    session.cart_lines = vec![
        CartLine {
            variant_id: tote.variants[0].id,
            quantity: 1,
            properties: [("Engraving", "Hi"), ("_hidden", "x")]
                .map(|(name, value)| (name.to_string(), value.to_string()))
                .into_iter()
                .collect(),
            selling_plan: None,
        },
        CartLine {
            variant_id: mug.variants[0].id,
            quantity: 1,
            properties: Default::default(),
            selling_plan: None,
        },
    ];
    let output = fixture.liquid(
        "/cart",
        session,
        "{%- for item in cart.items %}\n\
         json:{{ item.properties | json }}\n\
         size:{{ item.properties.size }}\n\
         positive:{% if item.properties.size > 0 %}yes{% else %}no{% endif %}\n\
         filter:{{ item.properties | size }} empty:{% if item.properties == empty %}yes{% else %}no{% endif %}\n\
         loop:{% for p in item.properties %}[{{ p.first }}={{ p.last }}]{% endfor %} \
         by name:{{ item.properties.Engraving }}{{ item.properties['_hidden'] }}\n\
         {%- endfor %}",
    );
    let lines: Vec<&str> = output.lines().filter(|line| !line.is_empty()).collect();
    assert_eq!(
        lines,
        [
            r#"json:[["Engraving","Hi"],["_hidden","x"]]"#,
            "size:",
            "positive:no",
            "filter:2 empty:no",
            "loop:[Engraving=Hi][_hidden=x] by name:Hix",
            "json:[]",
            "size:",
            "positive:no",
            "filter:0 empty:yes",
            "loop: by name:",
        ]
    );
}

#[test]
fn private_cart_attributes_are_not_in_liquid() {
    // #155: a cart attribute whose name starts with `__` is private: Liquid's
    // `cart.attributes` and `cart | json` leave it out, as on Shopify. One `_` is not private.
    let fixture = fixture();
    let mut session = Session::initial(&fixture.store);
    session.cart_attributes = [("gift_wrap", "yes"), ("_hidden", "h"), ("__private", "p")]
        .map(|(name, value)| (name.to_string(), value.to_string()))
        .into_iter()
        .collect();
    let output = fixture.liquid(
        "/cart",
        session,
        "attrs:{% for a in cart.attributes %}[{{ a[0] }}={{ a[1] }}]{% endfor %} \
         private:{{ cart.attributes.__private | json }} json:{{ cart.attributes | json }} \
         cart:{{ cart | json }}",
    );
    let (liquid, cart) = output.split_once(" cart:").expect("the cart's JSON");
    assert_eq!(
        liquid,
        r#"attrs:[gift_wrap=yes][_hidden=h] private:null json:{"gift_wrap":"yes","_hidden":"h"}"#
    );
    let cart: serde_json::Value = serde_json::from_str(cart).expect("the cart's JSON");
    assert_eq!(
        cart["attributes"],
        serde_json::json!({"gift_wrap": "yes", "_hidden": "h"})
    );
}

#[test]
fn search_excludes_terms_matches_title_words_and_ignores_unknown_types() {
    // #189, items 4 to 6. "Supported modifiers are - with no following whitespace, or NOT
    // followed by whitespace": https://shopify.dev/docs/api/usage/search-syntax
    let overlay = serde_json::json!({"products": [
        {"title": "The Complete Snowboard", "price": 2000},
        {"title": "Blue snowboard", "price": 2000},
        {"title": "Snowboard", "price": 2000}
    ]});
    let (store, diagnostics) =
        load_with_overlay(&DataSource::Demo, &LoadOptions::default(), Some(&overlay));
    assert!(diagnostics.is_empty(), "{diagnostics}");
    let fixture = Fixture {
        store: Arc::new(store),
        ..fixture()
    };
    let search = |query: &str| {
        fixture.liquid(
            &format!("/search?{query}"),
            Session::initial(&fixture.store),
            "{{ search.results_count }}:{% for r in search.results %}{{ r.handle }} {% endfor %}\
             [{{ search.types | join: ',' }}]",
        )
    };
    let all = "3:the-complete-snowboard blue-snowboard snowboard [product,article,page]";
    assert_eq!(search("q=snowboard"), all);
    // 4. An excluded term removes the products it matches.
    let not_blue = "2:the-complete-snowboard snowboard [product,article,page]";
    assert_eq!(search("q=snowboard -blue"), not_blue);
    assert_eq!(search("q=snowboard NOT blue"), not_blue);
    assert_eq!(search("q=-blue snowboard"), not_blue);
    assert_eq!(
        search("q=snowboard -title:complete"),
        "2:blue-snowboard snowboard [product,article,page]"
    );
    // 5. `title:` finds a word of the title, in any case.
    assert_eq!(
        search("q=title:complete"),
        "1:the-complete-snowboard [product,article,page]"
    );
    assert_eq!(search("q=title:Snowboard"), all);
    assert_eq!(search("q=title:snow"), "0:[product,article,page]");
    // 6. A type the store does not know is ignored.
    assert_eq!(search("q=snowboard&type=bogus"), all);
    assert_eq!(
        search("q=snowboard&type=product,bogus"),
        "3:the-complete-snowboard blue-snowboard snowboard [product]"
    );
}

#[test]
fn a_shop_in_a_country_the_store_does_not_sell_to_opens_in_the_first_country() {
    let (mut store, diagnostics) = load(&DataSource::Demo, &LoadOptions::default());
    assert!(diagnostics.is_empty(), "{diagnostics}");
    store.shop.address.country_code = "JP".to_string();
    assert!(store.country("JP").is_none());
    let fixture = Fixture {
        store: Arc::new(store),
        ..fixture()
    };
    let output = fixture.liquid(
        "/",
        Session::initial(&fixture.store),
        "{{ localization.country.iso_code }}",
    );
    assert_eq!(output, "US");
}

#[test]
fn a_country_is_offered_the_languages_of_its_market() {
    // https://shopify.dev/docs/api/liquid/objects/country: `available_languages` are "the
    // languages that have been added to the market that this country belongs to". On a real
    // store, the localization form of a Dawn-based theme (`localization.available_languages`)
    // lists `fr, en` in France, `de, en` in Germany and `fr, en, de` in Switzerland: the
    // languages of the visitor's market, its default first.
    let country_languages = "{% for country in localization.available_countries %}\
        {{ country.iso_code }}:{{ country.available_languages | map: 'iso_code' | join: ',' }} \
        {% endfor %}";
    let visitor_languages = "{{ localization.available_languages | map: 'iso_code' | join: ',' }}";
    // Without markets, every country is offered every language.
    let fixture = fixture();
    let session = || Session::initial(&fixture.store);
    assert_eq!(
        fixture.liquid("/", session(), country_languages),
        "CA:en,fr FR:en,fr US:en,fr "
    );
    assert_eq!(fixture.liquid("/", session(), visitor_languages), "en,fr");

    let overlay = serde_json::json!({"localization": {
        "countries": [{"iso_code": "US"}, {"iso_code": "CA"}, {"iso_code": "FR"}],
        "languages": [{"iso_code": "en"}, {"iso_code": "fr"}],
        "markets": [
            {"handle": "north-america", "countries": ["US"], "languages": ["en"]},
            {"handle": "france", "countries": ["FR"], "languages": ["fr", "en"]}
        ]
    }});
    let options = LoadOptions {
        theme_locales: vec!["en".to_string(), "fr".to_string()],
        ..LoadOptions::default()
    };
    let (store, diagnostics) = load_with_overlay(&DataSource::Demo, &options, Some(&overlay));
    assert!(diagnostics.is_empty(), "{diagnostics}");
    let fixture = Fixture {
        store: Arc::new(store),
        ..fixture
    };
    let session = |country: Option<&str>| {
        let mut session = Session::initial(&fixture.store);
        session.country = country.map(str::to_string);
        session
    };
    // Canada is in no market: it keeps every language. France lists its default first.
    assert_eq!(
        fixture.liquid("/", session(None), country_languages),
        "CA:en,fr FR:fr,en US:en "
    );
    // `localization.available_languages` follows the visitor's country: the shop's (Canada)
    // until they select one.
    assert_eq!(
        fixture.liquid("/", session(None), visitor_languages),
        "en,fr"
    );
    assert_eq!(
        fixture.liquid("/", session(Some("FR")), visitor_languages),
        "fr,en"
    );
    assert_eq!(
        fixture.liquid("/", session(Some("US")), visitor_languages),
        "en"
    );
    // `shop.published_locales` stays every language of the store.
    assert_eq!(
        fixture.liquid(
            "/",
            session(Some("FR")),
            "{{ shop.published_locales | map: 'iso_code' | join: ',' }}"
        ),
        "en,fr"
    );
}

#[test]
fn a_page_under_a_market_subfolder_links_under_it() {
    let overlay = serde_json::json!({"localization": {
        "countries": [{"iso_code": "CA"}, {"iso_code": "BE"}, {"iso_code": "NL"}],
        "languages": [{"iso_code": "en"}, {"iso_code": "fr"}],
        "markets": [{"handle": "benelux", "countries": ["BE", "NL"], "subfolder_suffix": "bnl"}]
    }});
    let options = LoadOptions {
        theme_locales: vec!["en".to_string(), "fr".to_string()],
        ..LoadOptions::default()
    };
    let (store, diagnostics) =
        lsf_core::store::load::load_with_overlay(&DataSource::Demo, &options, Some(&overlay));
    assert!(diagnostics.is_empty(), "{diagnostics}");
    let fixture = Fixture {
        store: Arc::new(store),
        ..fixture()
    };
    let mut under = request("/pages/about");
    under.locale = "fr".to_string();
    under.root = "/fr-bnl".to_string();
    under.market = Some("benelux".to_string());
    let source = "{{ localization.country.iso_code }}|{{ request.locale.root_url }}|\
        {{ localization.language.root_url }}|{{ routes.root_url }}|{{ routes.cart_url }}|\
        {% for language in localization.available_languages %}{{ language.root_url }},{% endfor %}";
    let render = |request: Request, session: Session| {
        let site = fixture
            .renderer
            .site(fixture.store.clone(), request, session);
        let page = lsf_theme::render::routes::resolve(&site);
        let (output, errors) = fixture
            .renderer
            .render_liquid(&site, page, source, &[])
            .expect("the template parses");
        assert!(errors.is_empty(), "{errors:?}");
        output
    };
    // The market's first country, every language under its subfolders.
    assert_eq!(
        render(under.clone(), Session::initial(&fixture.store)),
        "BE|/fr-bnl|/fr-bnl|/fr-bnl|/fr-bnl/cart|/en-bnl,/fr-bnl,"
    );
    // The visitor's country when the market has it.
    let mut session = Session::initial(&fixture.store);
    session.country = Some("NL".to_string());
    assert!(render(under, session.clone()).starts_with("NL|"));
    // At `/`, a country of the market is not the visitor's: the shop's.
    assert_eq!(
        render(request("/pages/about"), session),
        "CA|/|/|/|/cart|/,/fr,"
    );
    // Search engines learn every subfolder, for each country of its market.
    let head = fixture.page("/pages/about").body;
    for link in [
        "<link rel=\"alternate\" hreflang=\"en-BE\" href=\"http://shop.test/en-bnl/pages/about\">",
        "<link rel=\"alternate\" hreflang=\"fr-NL\" href=\"http://shop.test/fr-bnl/pages/about\">",
        "<link rel=\"alternate\" hreflang=\"fr\" href=\"http://shop.test/fr/pages/about\">",
    ] {
        assert!(head.contains(link), "{link} in {head}");
    }
}

/// The demo store selling to the United States and, in euros under `/en-be/`, to Belgium, with
/// `rate` and `metafields` on the market of Belgium (`extra` is merged into it).
fn european_fixture(extra: serde_json::Value) -> Fixture {
    let mut europe = serde_json::json!({"handle": "europe", "countries": ["BE"], "currency": "EUR",
        "rate": "0.9", "subfolder_suffix": "be", "metafields": {"custom": {"tier": "silver"}}});
    if let (Some(europe), serde_json::Value::Object(extra)) = (europe.as_object_mut(), extra) {
        europe.extend(extra);
    }
    let overlay = serde_json::json!({"localization": {
        "countries": [{"iso_code": "US"}, {"iso_code": "BE"}],
        "languages": [{"iso_code": "en"}, {"iso_code": "fr"}],
        "markets": [{"handle": "us", "countries": ["US"]}, europe]
    }});
    let options = LoadOptions {
        theme_locales: vec!["en".to_string(), "fr".to_string()],
        ..LoadOptions::default()
    };
    let (store, diagnostics) =
        lsf_core::store::load::load_with_overlay(&DataSource::Demo, &options, Some(&overlay));
    assert!(diagnostics.is_empty(), "{diagnostics}");
    Fixture {
        store: Arc::new(store),
        ..fixture()
    }
}

/// A request for `path` under `/en-be/`, the subfolders of the market `europe`.
fn under_en_be(path: &str) -> Request {
    let mut under = request(path);
    under.root = "/en-be".to_string();
    under.market = Some("europe".to_string());
    under
}

#[test]
fn localization_market_is_the_market_of_the_visitors_country() {
    let fixture = european_fixture(serde_json::json!({}));
    let source = "{{ localization.market.handle }}|{{ localization.market.id }}|\
        {{ localization.market.metafields.custom.tier }}|{{ localization.country.iso_code }}|\
        {% for c in localization.available_countries %}{{ c.iso_code }}:{{ c.market.handle }};{% endfor %}";
    let us = lsf_core::util::stable_id("market", "us");
    let europe = lsf_core::util::stable_id("market", "europe");
    let session = Session::initial(&fixture.store);
    // At `/`, the market of the United States; each country names its own market.
    assert_eq!(
        fixture.liquid("/", session.clone(), source),
        format!("us|{us}||US|BE:europe;US:us;")
    );
    // Under `/en-be/`, the market of Belgium, with its metafields.
    assert_eq!(
        fixture.liquid_for(under_en_be("/"), session, source),
        format!("europe|{europe}|silver|BE|BE:europe;US:us;")
    );
    // Its id is the one of the Admin API and of the Functions.
    assert_eq!(
        fixture.store.market_for("BE").gid(),
        format!("gid://shopify/Market/{europe}")
    );
}

#[test]
fn a_country_in_no_market_is_a_market_of_its_own() {
    let overlay = serde_json::json!({"localization": {
        "countries": [{"iso_code": "US"}, {"iso_code": "FR"}]
    }});
    let (store, diagnostics) = lsf_core::store::load::load_with_overlay(
        &DataSource::Demo,
        &LoadOptions::default(),
        Some(&overlay),
    );
    assert!(diagnostics.is_empty(), "{diagnostics}");
    let fixture = Fixture {
        store: Arc::new(store),
        ..fixture()
    };
    let mut session = Session::initial(&fixture.store);
    session.country = Some("FR".to_string());
    // Named after the country, as the checkout and the Admin API name it, with no metafields.
    assert_eq!(
        fixture.liquid(
            "/",
            session,
            "{{ localization.market.handle }}|{{ localization.market.id }}|\
             {{ localization.market.metafields.custom.tier }}"
        ),
        format!("france|{}|", lsf_core::util::stable_id("market", "FR"))
    );
}

#[test]
fn prices_are_in_the_currency_of_the_visitors_market() {
    let overlay = serde_json::json!({"localization": {
        "countries": [{"iso_code": "CA"}, {"iso_code": "US"}, {"iso_code": "FR"}],
        "markets": [{"handle": "europe", "countries": ["FR"], "currency": "EUR", "rate": "0.9",
            "prices": {"canvas-tote-bag": "20.00"}}]
    }});
    let (store, diagnostics) = lsf_core::store::load::load_with_overlay(
        &DataSource::Demo,
        &LoadOptions::default(),
        Some(&overlay),
    );
    assert!(diagnostics.is_empty(), "{diagnostics}");
    let fixture = Fixture {
        store: Arc::new(store),
        ..fixture()
    };
    let tote = fixture.store.product_by_handle("canvas-tote-bag").unwrap();
    let mut session = Session::initial(&fixture.store);
    session.cart_lines = vec![CartLine {
        variant_id: tote.variants[0].id,
        quantity: 2,
        properties: Default::default(),
        selling_plan: None,
    }];
    let source = "{{ product.price | money }}|{{ product.price | money_with_currency }}|\
        {{ cart.total_price | money }}|{{ cart.currency.iso_code }}|\
        {{ localization.country.currency.iso_code }}|{{ shop.currency }}|\
        {{ shop.enabled_currencies | map: 'iso_code' | join: ',' }}|\
        {{ shop.money_format }}|{{ shop.money_with_currency_format }}";
    // In the shop's currency, as before.
    assert_eq!(
        fixture.liquid("/products/organic-cotton-t-shirt", session.clone(), source),
        "$32.00|$32.00 USD|$48.00|USD|USD|USD|USD,EUR|${{amount}}|${{amount}} USD"
    );
    // In France, in euros: at the market's rate, or at its fixed price (the tote bag). The
    // money formats are those of the euro, `shop.currency` stays the shop's.
    session.country = Some("FR".to_string());
    assert_eq!(
        fixture.liquid("/products/organic-cotton-t-shirt", session.clone(), source),
        "€28,80|€28,80 EUR|€40,00|EUR|EUR|USD|USD,EUR|\
         €{{amount_with_comma_separator}}|€{{amount_with_comma_separator}} EUR"
    );
    // Shopify's scripts say it, with the rate.
    let page = fixture
        .render_with(request("/"), session, &Target::Page)
        .body;
    assert!(
        page.contains("Shopify.currency = {\"active\":\"EUR\",\"rate\":\"0.9\"};"),
        "{page}"
    );
}

#[test]
fn shop_money_formats_are_those_of_the_markets_currency() {
    // The issue's store: USD, and a market in EUR served under `/en-be/`.
    let overlay = serde_json::json!({"localization": {
        "countries": [{"iso_code": "US"}, {"iso_code": "BE"}],
        "languages": [{"iso_code": "en"}, {"iso_code": "fr"}],
        "markets": [{"handle": "us", "countries": ["US"]},
            {"handle": "europe", "countries": ["BE"], "currency": "EUR", "rate": "0.9",
             "subfolder_suffix": "be"}]
    }});
    let options = LoadOptions {
        theme_locales: vec!["en".to_string(), "fr".to_string()],
        ..LoadOptions::default()
    };
    let (store, diagnostics) =
        lsf_core::store::load::load_with_overlay(&DataSource::Demo, &options, Some(&overlay));
    assert!(diagnostics.is_empty(), "{diagnostics}");
    let fixture = Fixture {
        store: Arc::new(store),
        ..fixture()
    };
    let source = "{{ shop.money_format }}|{{ shop.money_with_currency_format }}|\
        {{ 1000 | money }}|{{ shop.currency }}|{{ shop.money_format | json }}";
    let session = Session::initial(&fixture.store);
    assert_eq!(
        fixture.liquid("/", session.clone(), source),
        "${{amount}}|${{amount}} USD|$10.00|USD|\"${{amount}}\""
    );
    for (locale, root) in [("en", "/en-be"), ("fr", "/fr-be")] {
        let mut under = request("/");
        under.locale = locale.to_string();
        under.root = root.to_string();
        under.market = Some("europe".to_string());
        assert_eq!(
            fixture.liquid_for(under, session.clone(), source),
            "€{{amount_with_comma_separator}}|€{{amount_with_comma_separator}} EUR|€10,00|USD|\
             \"€{{amount_with_comma_separator}}\"",
            "{root}"
        );
    }
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

/// `line_item.index` is the line's 0-based position: themes post `index | plus: 1` as the
/// `line` of `/cart/change.js`. The cart JSON has no such field.
#[test]
fn cart_lines_know_their_index() {
    let fixture = fixture();
    let tote = fixture.store.product_by_handle("canvas-tote-bag").unwrap();
    let mug = fixture.store.product_by_handle("ceramic-mug").unwrap();
    let mut session = Session::initial(&fixture.store);
    session.cart_lines = [tote, mug]
        .iter()
        .map(|product| CartLine {
            variant_id: product.variants[0].id,
            quantity: 1,
            properties: Default::default(),
            selling_plan: None,
        })
        .collect();
    assert_eq!(
        fixture.liquid(
            "/cart",
            session.clone(),
            "{% for item in cart.items %}{{ item.index }}:{{ item.index | plus: 1 }} {% endfor %}"
        ),
        "0:1 1:2 "
    );
    let json: serde_json::Value =
        serde_json::from_str(&fixture.liquid("/cart", session, "{{ cart | json }}")).expect("JSON");
    assert!(json["items"][1].get("index").is_none(), "{json}");
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
    // What is missing is said, with the sections the page has.
    let reason = many.not_found.as_deref().expect("a reason");
    assert!(
        reason.starts_with("no section \"nope\" on the page of template index"),
        "{reason}"
    );
    assert!(reason.contains(&id), "{reason}");
    assert!(one.not_found.is_none());

    let typo = fixture.render_with(
        request("/"),
        Session::initial(&fixture.store),
        &Target::Section(id.replace("__hero", "__heor")),
    );
    assert_eq!(typo.status, 404);
    let reason = typo.not_found.as_deref().expect("a reason");
    assert!(
        reason.contains(&format!("did you mean \"{id}\"?")),
        "{reason}"
    );

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
fn more_than_five_sections_answer_400_and_render_nothing() {
    // https://shopify.dev/docs/storefronts/themes/best-practices/performance/use-section-rendering-api:
    // "Requesting more than five sections returns 400 Bad Request and renders nothing."
    let fixture = fixture();
    let ids = |list: &str| Target::Sections(list.split(',').map(str::to_string).collect());
    let six = fixture.render_with(
        request("/"),
        Session::initial(&fixture.store),
        &ids("footer,b,c,d,e,f"),
    );
    assert_eq!(six.status, 400);
    assert_eq!(six.body, "");
    assert!(six.not_found.is_none());
    // Five render, missing ones included, and a section asked for twice counts once.
    let five = fixture.render_with(
        request("/"),
        Session::initial(&fixture.store),
        &ids("footer,b,c,d,e,footer"),
    );
    assert_eq!(five.status, 200);
    let json: serde_json::Value = serde_json::from_str(&five.body).unwrap();
    assert!(json["footer"].is_string());
    assert!(json["e"].is_null());
}

#[test]
fn a_section_rendered_for_a_variant_has_no_product_and_no_template() {
    // https://shopify.dev/docs/storefronts/themes/delivery-fulfillment/pickup-availability:
    // "You can't access the Liquid product object in the pickup availability section". A store
    // gives `product_variant` alone: `template.name` empty, `request.page_type` `variants`,
    // no `product` and no `variant`, and `request.path` without its trailing slash (issue #134).
    let fixture = theme_of(
        "variant-section",
        &[],
        &[
            (
                "layout/theme.liquid",
                "<html><body>{{ content_for_layout }}</body></html>",
            ),
            (
                "sections/probe.liquid",
                "<p>template=[{{ template.name }}] page_type=[{{ request.page_type }}] \
                 path=[{{ request.path }}] product=[{{ product.handle }}] \
                 product_variant=[{{ product_variant.id }}] variant=[{{ variant.id }}] \
                 title=[{{ product_variant.product.title }}]</p>",
            ),
            ("templates/index.json", r#"{ "sections": {}, "order": [] }"#),
            (
                "templates/product.json",
                r#"{ "sections": {}, "order": [] }"#,
            ),
        ],
    );
    let product = &fixture.store.products[0];
    let variant = product.variants.last().unwrap().id;
    let expected = format!(
        "<p>template=[] page_type=[variants] path=[/variants/{variant}] product=[] \
         product_variant=[{variant}] variant=[] title=[{}]</p>",
        product.title
    );
    for path in [
        format!("/variants/{variant}?section_id=probe"),
        format!("/variants/{variant}/?section_id=probe"),
    ] {
        let rendered = fixture.render_with(
            request(&path),
            Session::initial(&fixture.store),
            &Target::Section("probe".to_string()),
        );
        assert_eq!(rendered.status, 200, "{path}");
        assert_eq!(rendered.template, "", "{path}");
        assert!(
            rendered.body.contains(&expected),
            "{path}: {}",
            rendered.body
        );
    }
    let bundle = fixture.render_with(
        request(&format!("/variants/{variant}?sections=probe")),
        Session::initial(&fixture.store),
        &Target::Sections(vec!["probe".to_string()]),
    );
    let json: serde_json::Value = serde_json::from_str(&bundle.body).unwrap();
    assert!(
        json["probe"].as_str().unwrap().contains(&expected),
        "{}",
        bundle.body
    );
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
    // The countries of the store, as the options of a selector after Shopify's `---`, each with
    // its provinces as pairs of their name and the name to show.
    let body = fixture.page("/nope").body;
    assert!(
        body.contains(
            "<select><option value=\"---\" data-provinces=\"[]\">---</option>\n<option value=\"United States\" data-provinces=\"[[&quot;Alabama&quot;,&quot;Alabama&quot;],[&quot;Alaska&quot;,&quot;Alaska&quot;],"
        ),
        "{body}"
    );
    assert!(
        body.contains(
            "[&quot;Yukon&quot;,&quot;Yukon&quot;]]\">Canada</option>\n\
             <option value=\"France\" data-provinces=\"[]\">France</option></select>"
        ),
        "{body}"
    );
}

#[test]
fn country_selectors_list_the_provinces_the_store_ships_to() {
    let overlay = serde_json::json!({"localization": {"countries": [
        {"iso_code": "US", "provinces": ["WA", "OR", "DC"]},
        {"iso_code": "JP", "provinces": ["JP-13"]},
        {"iso_code": "FR"}
    ]}});
    let (store, diagnostics) = lsf_core::store::load::load_with_overlay(
        &DataSource::Demo,
        &LoadOptions::default(),
        Some(&overlay),
    );
    assert!(diagnostics.is_empty(), "{diagnostics}");
    let fixture = Fixture {
        store: Arc::new(store),
        ..fixture()
    };
    let session = Session::initial(&fixture.store);
    assert_eq!(
        fixture.liquid("/", session.clone(), "{{ country_option_tags }}"),
        "<option value=\"---\" data-provinces=\"[]\">---</option>\n\
         <option value=\"United States\" data-provinces=\"[[&quot;District of Columbia&quot;,&quot;Washington DC&quot;],[&quot;Oregon&quot;,&quot;Oregon&quot;],[&quot;Washington&quot;,&quot;Washington&quot;]]\">United States</option>\n\
         <option value=\"Japan\" data-provinces=\"[[&quot;Tōkyō&quot;,&quot;Tokyo&quot;]]\">Japan</option>\n\
         <option value=\"France\" data-provinces=\"[]\">France</option>"
    );
    // Sorted by name, each with the name a form sends and the name to show, as Shopify does.
    // Every country, with every province.
    let all = fixture.liquid("/", session, "{{ all_country_option_tags }}");
    assert!(
        all.contains(
            "<option value=\"Canada\" data-provinces=\"[[&quot;Alberta&quot;,&quot;Alberta&quot;],[&quot;British Columbia&quot;,&quot;British Columbia&quot;],"
        ),
        "{all}"
    );
    assert!(
        all.contains("[&quot;Wyoming&quot;,&quot;Wyoming&quot;]]\">United States</option>"),
        "{all}"
    );
    assert!(
        all.contains("<option value=\"Germany\" data-provinces=\"[]\">Germany</option>"),
        "{all}"
    );
    assert!(
        all.contains("<option value=\"United Kingdom\" data-provinces=\"[[&quot;British Forces&quot;,&quot;British Forces&quot;],[&quot;England&quot;,&quot;England&quot;],"),
        "{all}"
    );
}

#[test]
fn every_country_has_its_name_and_continent_and_a_selector_lists_them_all() {
    let overlay = serde_json::json!({"localization": {"countries": [
        {"iso_code": "US"}, {"iso_code": "AD"}, {"iso_code": "AL"}, {"iso_code": "KE"},
        {"iso_code": "MA"}, {"iso_code": "QA"}, {"iso_code": "UY"}, {"iso_code": "FR"},
        {"iso_code": "BE"}, {"iso_code": "CA"}
    ]}});
    let (store, diagnostics) = lsf_core::store::load::load_with_overlay(
        &DataSource::Demo,
        &LoadOptions::default(),
        Some(&overlay),
    );
    assert!(diagnostics.is_empty(), "{diagnostics}");
    let fixture = Fixture {
        store: Arc::new(store),
        ..fixture()
    };
    let session = Session::initial(&fixture.store);
    assert_eq!(
        fixture.liquid(
            "/",
            session.clone(),
            "{% for c in localization.available_countries %}{{ c.iso_code }}={{ c.name }}/{{ c.continent }};{% endfor %}"
        ),
        // Sorted by name, as `localization.available_countries` is on Shopify.
        "AL=Albania/Europe;AD=Andorra/Europe;BE=Belgium/Europe;CA=Canada/North America;\
         FR=France/Europe;KE=Kenya/Africa;MA=Morocco/Africa;QA=Qatar/Asia;\
         US=United States/North America;UY=Uruguay/South America;"
    );
    // Shopify's 237 countries and its `---`: 238 options, so 239 parts.
    assert_eq!(
        fixture.liquid(
            "/",
            session.clone(),
            "{{ all_country_option_tags | split: '<option' | size }}"
        ),
        "239"
    );
    // By name, accents aside, after the `---`.
    let all = fixture.liquid("/", session, "{{ all_country_option_tags }}");
    assert!(
        all.starts_with(
            "<option value=\"---\" data-provinces=\"[]\">---</option>\n\
             <option value=\"Afghanistan\" data-provinces=\"[]\">Afghanistan</option>\n\
             <option value=\"Åland Islands\" data-provinces=\"[]\">Åland Islands</option>\n\
             <option value=\"Albania\" data-provinces=\"[]\">Albania</option>\n"
        ),
        "{all}"
    );
    assert!(
        all.ends_with("<option value=\"Zimbabwe\" data-provinces=\"[]\">Zimbabwe</option>"),
        "{all}"
    );
}

#[test]
fn countries_have_their_names_in_the_language_of_the_page_and_are_sorted_by_them() {
    let overlay = serde_json::json!({
        "shop": {"address": {
            "company": "", "address1": "1 Main St", "address2": "", "city": "Portland",
            "province": "Oregon", "province_code": "OR", "zip": "97201",
            "country": "United States", "country_code": "US"
        }},
        "localization": {
            "countries": [{"iso_code": "US"}, {"iso_code": "BE"}, {"iso_code": "DE"}, {"iso_code": "CA"}],
            "languages": [{"iso_code": "en"}, {"iso_code": "fr"}, {"iso_code": "de"}]
        }
    });
    let options = LoadOptions {
        theme_locales: vec!["en".to_string(), "fr".to_string(), "de".to_string()],
        ..LoadOptions::default()
    };
    let (store, diagnostics) = load_with_overlay(&DataSource::Demo, &options, Some(&overlay));
    assert!(diagnostics.is_empty(), "{diagnostics}");
    let fixture = Fixture {
        store: Arc::new(store),
        ..fixture()
    };
    let session = || Session::initial(&fixture.store);
    let page = |locale: &str, source: &str| {
        let mut request = request("/");
        if locale != "en" {
            request.locale = locale.to_string();
            request.root = format!("/{locale}");
        }
        fixture.liquid_for(request, session(), source)
    };
    let countries = "{% for c in localization.available_countries %}{{ c.name }};{% endfor %}";
    assert_eq!(
        page("en", countries),
        "Belgium;Canada;Germany;United States;"
    );
    assert_eq!(
        page("fr", countries),
        "Allemagne;Belgique;Canada;États-Unis;"
    );
    assert_eq!(
        page("de", countries),
        "Belgien;Deutschland;Kanada;Vereinigte Staaten;"
    );
    assert_eq!(page("fr", "{{ localization.country.name }}"), "États-Unis");
    // The shop's address, and `format_address`.
    assert_eq!(
        page(
            "fr",
            "{{ shop.address.country }}|{{ shop.address.country.name }}|{{ shop.address.summary }}"
        ),
        "États-Unis|États-Unis|1 Main St, Portland, Oregon, États-Unis"
    );
    assert!(page("fr", "{{ shop.address | format_address }}").ends_with("<br>États-Unis</p>"));
    assert_eq!(page("en", "{{ shop.address.country }}"), "United States");
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
        lsf_core::cart::line_key(&session.cart_lines[0]),
        lsf_core::cart::line_key(&session.cart_lines[1])
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

#[test]
fn weights_print_as_shopify_prints_them() {
    // #128: a variant of 4536 g with the unit `lb`, as a store prints it.
    let overlay = serde_json::json!({"products": [{
        "title": "Heavy Widget",
        "price": 1000,
        "variants": [{ "weight": 4536, "weight_unit": "lb" }]
    }]});
    let (store, diagnostics) =
        load_with_overlay(&DataSource::Demo, &LoadOptions::default(), Some(&overlay));
    assert!(diagnostics.is_empty(), "{diagnostics}");
    let fixture = Fixture {
        store: Arc::new(store),
        ..fixture()
    };
    let output = fixture.liquid(
        "/products/heavy-widget",
        Session::initial(&fixture.store),
        "{%- assign v = product.variants.first -%}\
         in_unit=[{{ v.weight_in_unit }}] own_unit=[{{ v.weight | weight_with_unit: v.weight_unit }}] \
         kg=[{{ v.weight | weight_with_unit: 'kg' }}] \
         nums=[{{ 1000 | weight_with_unit: 'kg' }}|{{ 1 | weight_with_unit: 'kg' }}|\
         {{ 28 | weight_with_unit: 'oz' }}|{{ 2.5 | weight_with_unit: 'g' }}|\
         {{ 1234 | weight_with_unit: 'kg' }}|{{ -1000 | weight_with_unit: 'kg' }}]",
    );
    assert_eq!(
        output,
        "in_unit=[10.0] own_unit=[10.0 lb] kg=[4.536 kg] \
         nums=[1.0 kg|0.001 kg|1.0 oz|3 g|1.234 kg|-1.0 kg]"
    );
}

#[test]
fn robots_txt_keeps_the_line_breaks_of_its_template() {
    let fixture = fixture();
    let site = fixture.renderer.site(
        fixture.store.clone(),
        request("/robots.txt"),
        Session::initial(&fixture.store),
    );
    let rendered = fixture.renderer.render_robots(&site);
    assert_clean(&rendered);
    let body = rendered.body;

    // The objects print without a line break, as on Shopify. The lines are those of the
    // template: where `{{-` and `{%-` trim text that is only whitespace, a storefront keeps
    // its first character, here the line break.
    assert!(
        body.starts_with(
            "# we use Shopify as our ecommerce platform\n\n\
             User-agent: *\nDisallow: /a/downloads/-/*\nDisallow: /admin\n"
        ),
        "{body}"
    );
    // The rules the template adds, written with and without `-`, then the next group.
    assert!(
        body.contains(
            "-remote\nDisallow: /*?q=*\nDisallow: /*?view=\nDisallow: /*?pr_\n\
             Sitemap: http://shop.test/sitemap.xml\n\n\
             User-agent: adsbot-google\nDisallow: /checkouts/\n"
        ),
        "{body}"
    );
    // A group without a sitemap.
    assert!(
        body.contains(
            "Disallow: /services/login_with_shop\n\n\
             User-agent: Nutch\nDisallow: /\n\n\
             User-agent: AhrefsBot\nCrawl-delay: 10\nDisallow: /a/downloads/-/*\n"
        ),
        "{body}"
    );
    assert!(
        body.ends_with("User-agent: Pinterest\nCrawl-delay: 1\n\n"),
        "{body}"
    );
    // The rule the template leaves out.
    assert!(!body.contains("Disallow: /policies/\n"), "{body}");
    assert!(body.contains("Disallow: /*/policies/\n"), "{body}");
    // Never two rules on a line.
    for line in body.lines() {
        let directives: usize = ["User-agent: ", "Disallow: ", "Crawl-delay: ", "Sitemap: "]
            .iter()
            .map(|directive| line.matches(directive).count())
            .sum();
        assert!(directives <= 1, "{line}");
    }
}

#[test]
fn a_liquid_asset_gets_a_new_url_when_the_settings_change() {
    let fixture = fixture();
    let url = |store: Arc<Store>| {
        let site = fixture
            .renderer
            .site(store.clone(), request("/"), Session::initial(&store));
        let (output, errors) = fixture
            .renderer
            .render_liquid(
                &site,
                Page::new("index", Resource::Index),
                "{{ 'colors.css' | asset_url }} {{ 'base.css' | asset_url }}",
                &[],
            )
            .expect("the template parses");
        assert!(errors.is_empty(), "{errors:?}");
        let (colors, base) = output.split_once(' ').expect("two URLs");
        (colors.to_string(), base.to_string())
    };
    let (colors, base) = url(fixture.store.clone());
    assert!(
        colors.starts_with("//shop.test/cdn/shop/t/1/assets/colors.css?v="),
        "{colors}"
    );

    // `assets/colors.css.liquid` is rendered with the settings: the same settings, the same
    // URL; other settings, another URL, so that the file can be kept for a year.
    assert_eq!(url(fixture.store.clone()).0, colors);
    let options = LoadOptions {
        theme_locales: vec!["en".to_string(), "fr".to_string()],
        settings_schema: fixture
            .renderer
            .theme()
            .json("config/settings_schema.json")
            .unwrap(),
    };
    let overlay = serde_json::json!({"theme_settings": {"page_size": 3}});
    let (other, diagnostics) =
        lsf_core::store::load::load_with_overlay(&DataSource::Demo, &options, Some(&overlay));
    assert!(diagnostics.is_empty(), "{diagnostics}");
    let (other_colors, other_base) = url(Arc::new(other));
    assert_ne!(other_colors, colors);
    // A plain file only follows its own content.
    assert_eq!(other_base, base);
}

#[test]
fn a_render_says_how_long_its_parts_took() {
    let fixture = fixture();
    let kinds = |rendered: &Rendered| -> Vec<String> {
        let sections = &rendered.timings.sections;
        sections
            .iter()
            .map(|section| section.kind.clone())
            .collect()
    };

    // A page: its template, then its layout, and the sections of both as they come.
    let page = fixture.page("/");
    let timings = &page.timings;
    assert_eq!(
        kinds(&page),
        ["hero", "announcement", "announcement", "footer"]
    );
    assert_eq!(
        timings.sections[0].id,
        lsf_theme::render::template_section_id("index", "hero")
    );
    assert_eq!(timings.sections[3].id, "footer");
    let (template, layout) = (timings.template.unwrap(), timings.layout.unwrap());
    assert!(template + layout <= timings.total, "{timings:?}");
    // The sections are part of the template or of the layout.
    let of = |range: std::ops::Range<usize>| -> std::time::Duration {
        timings.sections[range]
            .iter()
            .map(|section| section.duration)
            .sum()
    };
    assert!(of(0..2) <= template && of(2..4) <= layout, "{timings:?}");
    assert!(
        timings
            .sections
            .iter()
            .all(|section| !section.duration.is_zero())
    );

    // The theme blocks of a section, each one before the blocks it holds. An app block and
    // a disabled one are not rendered.
    let hero = &timings.sections[0];
    let blocks: Vec<(&str, &str, usize)> = hero
        .blocks
        .iter()
        .map(|block| (block.kind.as_str(), block.key.as_str(), block.depth))
        .collect();
    assert_eq!(
        blocks,
        [
            ("_title", "title", 1),
            ("text", "intro", 1),
            ("group", "group", 1),
            ("text", "nested", 2)
        ]
    );
    // The id is the one the page gives the block.
    let group = &hero.blocks[2];
    assert!(
        page.body
            .contains(&format!("id=\"shopify-block-{}\"", group.id)),
        "{group:?}"
    );
    let took = |index: usize| -> std::time::Duration {
        let block = &hero.blocks[index];
        assert_eq!(block.durations.len(), 1, "{block:?}");
        block.durations[0]
    };
    // A block is part of the one it is in, and the blocks of a section are part of it.
    assert!(took(3) <= took(2), "{hero:?}");
    assert!(took(0) + took(1) + took(2) <= hero.duration, "{hero:?}");
    assert!(
        timings.sections[1..]
            .iter()
            .all(|section| section.blocks.is_empty())
    );

    // Sections asked for alone: no template, no layout.
    let footer = fixture.render_with(
        request("/"),
        Session::initial(&fixture.store),
        &Target::Section("footer".to_string()),
    );
    assert_eq!(kinds(&footer), ["footer"]);
    assert!(footer.timings.template.is_none() && footer.timings.layout.is_none());
    assert!(footer.timings.sections[0].duration <= footer.timings.total);
}

/// The frames of a profile as they nest: `a(b c(d))`. Checks that they close in order.
fn profile_shape(profile: &lsf_liquid::Profile) -> String {
    let mut shape = String::new();
    let mut open = Vec::new();
    for event in &profile.events {
        if event.open {
            if !shape.is_empty() && !shape.ends_with('(') {
                shape.push(' ');
            }
            let frame = &profile.frames[event.frame];
            shape.push_str(&frame.name);
            if let Some(line) = frame.line {
                shape.push_str(&format!(":{line}"));
            }
            shape.push('(');
            open.push(event.frame);
        } else {
            assert_eq!(open.pop(), Some(event.frame), "{shape}");
            shape.push(')');
        }
    }
    assert!(open.is_empty(), "{shape}");
    shape.replace("()", "")
}

#[test]
fn a_profile_records_what_rendered_what() {
    let fixture = fixture();
    let site = fixture.renderer.site(
        fixture.store.clone(),
        request("/"),
        Session::initial(&fixture.store),
    );
    let page = || lsf_theme::render::routes::resolve(&site);
    let plain = fixture.renderer.render_page(&site, page(), &Target::Page);
    let (rendered, profile) =
        fixture
            .renderer
            .profile_page(&site, page(), &Target::Page, &ProfileOptions::default());
    // Profiling changes nothing of what is rendered.
    assert_eq!(rendered.body, plain.body);
    assert!(rendered.errors.is_empty(), "{:?}", rendered.errors);

    let hero = lsf_theme::render::template_section_id("index", "hero");
    let second = lsf_theme::render::template_section_id("index", "second");
    let group = lsf_theme::tags::group_id_prefix("header-group");
    // The template, then the layout: under each one its sections, under a section its file,
    // and under a file the blocks and the snippets it renders.
    assert_eq!(
        profile_shape(&profile),
        format!(
            "render(\
               templates/index.json(\
                 section {hero}(sections/hero(\
                   block title(blocks/_title) \
                   block intro(blocks/text(snippets/meta)) \
                   block group(blocks/group(block nested(blocks/text(snippets/meta)))) \
                   snippets/meta)) \
                 section {second}(sections/announcement)) \
               layout/theme(\
                 section {group}announcement(sections/announcement) \
                 section footer(sections/footer)))"
        )
    );
    assert!(profile.frames.iter().all(|frame| frame.line.is_none()));

    // With lines, the tags and the outputs of each file, under the file.
    let with_lines_options = ProfileOptions {
        lines: true,
        ..ProfileOptions::default()
    };
    let (with_lines, lines) =
        fixture
            .renderer
            .profile_page(&site, page(), &Target::Page, &with_lines_options);
    assert_eq!(with_lines.body, plain.body);
    let shape = profile_shape(&lines);
    assert!(
        shape.contains("section footer(sections/footer(sections/footer:1"),
        "{shape}"
    );
    assert!(shape.contains("layout/theme(layout/theme:2 "), "{shape}");

    // A section asked for alone.
    let (_, footer) = fixture.renderer.profile_page(
        &site,
        page(),
        &Target::Section("footer".to_string()),
        &ProfileOptions::default(),
    );
    assert_eq!(
        profile_shape(&footer),
        "render(section footer(sections/footer))"
    );
}

/// How many things of each kind a profile was charged for, without the kinds that were not.
fn charged(profile: &lsf_liquid::Profile) -> Vec<(&str, u64)> {
    let total: u64 = profile
        .charges
        .iter()
        .map(|charge| charge.count * charge.each)
        .sum();
    assert_eq!(profile.points, total, "{:?}", profile.charges);
    profile
        .charges
        .iter()
        .filter(|charge| charge.count > 0)
        .map(|charge| (charge.kind.as_str(), charge.count))
        .collect()
}

#[test]
fn a_profile_counts_what_a_page_loads() {
    let fixture = fixture();
    let profile_with = |path: &str, options: &ProfileOptions| {
        let site = fixture.renderer.site(
            fixture.store.clone(),
            request(path),
            Session::initial(&fixture.store),
        );
        let page = lsf_theme::render::routes::resolve(&site);
        let (_, profile) = fixture
            .renderer
            .profile_page(&site, page, &Target::Page, options);
        profile
    };
    let profile = |path: &str| profile_with(path, &ProfileOptions::default());
    // What was loaded, without the tags and the outputs, which every page has.
    let loaded = |profile: &lsf_liquid::Profile| -> Vec<(String, u64)> {
        let charges = charged(profile);
        assert_eq!(charges[0].0, "liquid");
        assert!(charges[0].1 > 10, "{charges:?}");
        charges[1..]
            .iter()
            .map(|(kind, count)| (kind.to_string(), *count))
            .collect()
    };
    let of = |pairs: &[(&str, u64)]| -> Vec<(String, u64)> {
        pairs
            .iter()
            .map(|(kind, count)| (kind.to_string(), *count))
            .collect()
    };

    // The home page: the product of a section setting, and the one of a block setting.
    let home = profile("/");
    assert_eq!(loaded(&home), of(&[("product", 2)]));
    // A product page reads the variants of its product.
    assert_eq!(
        loaded(&profile("/products/ceramic-mug")),
        of(&[("product", 1), ("variant", 3)])
    );
    // A collection page: the collection and the products of the page, 3 of its 8.
    assert_eq!(
        loaded(&profile("/collections/all")),
        of(&[("product", 3), ("collection", 1)])
    );
    assert_eq!(loaded(&profile("/pages/about")), of(&[("page", 1)]));

    // The same render is charged the same every time.
    let again = profile("/");
    assert_eq!(again.points, home.points);
    assert_eq!(again.charges, home.charges);
    // The points of a frame are what was charged while it was open: the two products of
    // the home page are inside the section that asks for them.
    let hero = format!(
        "section {}",
        lsf_theme::render::template_section_id("index", "hero")
    );
    let frame = home
        .frames
        .iter()
        .position(|frame| frame.name == hero)
        .unwrap();
    let at: Vec<u64> = home
        .events
        .iter()
        .filter(|event| event.frame == frame)
        .map(|event| event.points)
        .collect();
    assert!(at[1] - at[0] >= 200, "{at:?}");
    assert!(at[1] - at[0] < home.points, "{at:?}");

    // Other costs, other points for the same things.
    let cheap = Costs::default().with("product=1,liquid=0").unwrap();
    let options = ProfileOptions {
        costs: cheap,
        ..ProfileOptions::default()
    };
    let cheaper = profile_with("/", &options);
    assert_eq!(cheaper.points, 2);
    assert_eq!(charged(&cheaper), charged(&home));
}

#[test]
fn a_profile_counts_the_products_of_a_collection_a_page_fetches() {
    // #129. Shopify fetches the page of a `paginate`, at most 50 products otherwise, and for
    // `collection.products` the `limit` of a loop below 50 outside `paginate`:
    // https://shopify.dev/docs/storefronts/themes/best-practices/performance/limit-product-queries-with-pagination
    let products: Vec<serde_json::Value> = (1..=60)
        .map(|n| serde_json::json!({"title": format!("P{n}"), "handle": format!("p{n}"), "price": "1.00"}))
        .collect();
    let overlay = serde_json::json!({ "products": products });
    let products_of = |name: &str, loop_markup: &str| -> (u64, usize) {
        let fixture = theme_with_data(
            name,
            &overlay,
            &[
                (
                    "layout/theme.liquid",
                    "<html><body>{{ content_for_layout }}</body></html>",
                ),
                ("templates/index.liquid", loop_markup),
            ],
        );
        let site = fixture.renderer.site(
            fixture.store.clone(),
            request("/"),
            Session::initial(&fixture.store),
        );
        let page = lsf_theme::render::routes::resolve(&site);
        let (rendered, profile) =
            fixture
                .renderer
                .profile_page(&site, page, &Target::Page, &ProfileOptions::default());
        assert_clean(&rendered);
        let products = charged(&profile)
            .into_iter()
            .find(|(kind, _)| *kind == "product")
            .map_or(0, |(_, count)| count);
        (products, rendered.body.matches("<p>").count())
    };
    let each = "<p>{{ p.title }}</p>";
    // A dynamic source is a `relative_link` too: `request.path` already has the prefix.
    // The issue's repro: a page of 4.
    assert_eq!(
        products_of(
            "profile-paginate",
            &format!(
                "{{% paginate collections.all.products by 4 %}}{{% for p in collections.all.products %}}{each}{{% endfor %}}{{% endpaginate %}}"
            )
        ),
        (4, 4)
    );
    // Without `paginate`: at most 50, whatever the size of the collection.
    assert_eq!(
        products_of(
            "profile-bare",
            &format!("{{% for p in collections.all.products %}}{each}{{% endfor %}}")
        ),
        (50, 50)
    );
    // A `limit` below 50 fetches what the loop shows, with an `offset` too.
    assert_eq!(
        products_of(
            "profile-limit",
            &format!("{{% for p in collections.all.products limit: 4 %}}{each}{{% endfor %}}")
        ),
        (4, 4)
    );
    assert_eq!(
        products_of(
            "profile-offset",
            &format!(
                "{{% for p in collections.all.products limit: 4 offset: 2 %}}{each}{{% endfor %}}"
            )
        ),
        (6, 4)
    );
    assert_eq!(
        products_of(
            "profile-paginate-limit",
            &format!(
                "{{% paginate collections.all.products by 4 %}}{{% for p in collections.all.products limit: 4 %}}{each}{{% endfor %}}{{% endpaginate %}}"
            )
        ),
        (4, 4)
    );
    // A `limit` past 50 is still 50.
    assert_eq!(
        products_of(
            "profile-limit-60",
            &format!("{{% for p in collections.all.products limit: 60 %}}{each}{{% endfor %}}")
        ),
        (50, 50)
    );
    // Reading the list otherwise than in a loop fetches the page.
    assert_eq!(
        products_of(
            "profile-first",
            "<p>{{ collections.all.products.first.title }}</p>"
        ),
        (50, 1)
    );
}

#[test]
fn a_video_file_in_a_metaobject_is_a_video() {
    let overlay = serde_json::json!({
        "metaobjects": {
            "look": [{
                "handle": "summer",
                "fields": {
                    "clip": { "type": "file_reference", "value": "looks/summer.mp4" },
                    "clips": { "type": "list.file_reference", "value": ["looks/summer.mp4"] }
                }
            }]
        }
    });
    let (store, diagnostics) = lsf_core::store::load::load_with_overlay(
        &DataSource::Demo,
        &LoadOptions::default(),
        Some(&overlay),
    );
    assert!(diagnostics.is_empty(), "{diagnostics}");
    let fixture = Fixture {
        store: Arc::new(store),
        ..fixture()
    };
    let output = fixture.liquid(
        "/",
        Session::initial(&fixture.store),
        "{% assign clip = metaobjects.look.summer.clip.value %}\
         {{ clip.media_type }}|{{ clip.sources.size }}|{{ clip.sources.first.mime_type }}|\
         {{ clip.preview_image.src }}|{{ metaobjects.look.summer.clips.value.first.media_type }}|\
         {{ clip | video_tag }}",
    );
    let (fields, tag) = output.rsplit_once('|').unwrap();
    assert_eq!(fields, "video|1|video/mp4|files/looks/summer.jpg|video");
    assert!(tag.starts_with("<video"), "{tag}");
    assert!(tag.contains("/cdn/shop/files/looks/summer.mp4"), "{tag}");
    // `metafield_tag` prints the same video, as Shopify does.
    let tag = fixture.liquid(
        "/",
        Session::initial(&fixture.store),
        "{{ metaobjects.look.summer.clip | metafield_tag }}",
    );
    assert!(tag.starts_with("<video playsinline"), "{tag}");
    assert!(tag.contains("type=\"video/mp4\""), "{tag}");
}

#[test]
fn a_variant_has_structured_data() {
    // #127: Shopify prints a schema.org `Product` for a variant, with its `Offer`, the shape of
    // an item of a `ProductGroup`'s `hasVariant`.
    let overlay = serde_json::json!({"products": [
        {
            "title": "Widget",
            "price": 1000,
            "images": [{ "src": "widget.jpg", "width": 1600, "height": 1600 }],
            "variants": [{ "sku": "W-1" }]
        },
        {
            "title": "Sized Widget",
            "price": 1200,
            "options": ["Size"],
            "variants": [{ "options": ["S"], "sku": "SW-S" }, { "options": ["M"], "sku": "SW-M" }]
        }
    ]});
    let (store, diagnostics) =
        load_with_overlay(&DataSource::Demo, &LoadOptions::default(), Some(&overlay));
    assert!(diagnostics.is_empty(), "{diagnostics}");
    let fixture = Fixture {
        store: Arc::new(store),
        ..fixture()
    };
    let render = |path: &str, source: &str| {
        let output = fixture.liquid(path, Session::initial(&fixture.store), source);
        serde_json::from_str::<serde_json::Value>(&output).expect(&output)
    };
    let widget = render(
        "/products/widget",
        "{{ product.selected_or_first_available_variant | structured_data }}",
    );
    let id = widget["offers"]["url"]
        .as_str()
        .unwrap()
        .rsplit('=')
        .next()
        .unwrap()
        .to_string();
    let keys = |value: &serde_json::Value| -> Vec<String> {
        value.as_object().unwrap().keys().cloned().collect()
    };
    assert_eq!(
        keys(&widget),
        ["@id", "@type", "image", "name", "offers", "sku"]
    );
    assert_eq!(
        widget["@id"],
        format!("/products/widget?variant={id}#variant")
    );
    assert_eq!(widget["@type"], "Product");
    assert!(
        widget["image"]
            .as_str()
            .is_some_and(|image| image.contains("/cdn/shop/files/widget.jpg?v=")
                && image.ends_with("&width=1920")),
        "{widget}"
    );
    assert_eq!(widget["name"], "Widget");
    assert_eq!(
        widget["offers"],
        serde_json::json!({
            "@id": format!("/products/widget?variant={id}#offer"),
            "@type": "Offer",
            "availability": "http://schema.org/InStock",
            "price": "10.00",
            "priceCurrency": "USD",
            "url": format!("http://shop.test/products/widget?variant={id}"),
        })
    );
    assert_eq!(widget["sku"], "W-1");

    // A variant of a product with options is named after it too.
    let sized = render(
        "/products/sized-widget",
        "{{ product.variants.first | structured_data }}",
    );
    assert_eq!(sized["name"], "Sized Widget - S");
    assert_eq!(sized["sku"], "SW-S");
    // The same as in the product's `hasVariant`.
    let group = render("/products/sized-widget", "{{ product | structured_data }}");
    assert_eq!(group["hasVariant"][0], sized);
}

#[test]
fn structured_data_leaves_out_an_empty_sku_and_image() {
    // #190: Shopify writes a one-variant product's SKU in its `Product`, before `url`, and
    // leaves out a variant's `sku` when it is empty and its `image` when it has none.
    let fixture = fixture_with_data(&serde_json::json!({"products": [
        { "title": "Plain", "price": 500, "variants": [{ "sku": "PLAIN-1" }] },
        { "title": "Bare", "price": 500 },
        {
            "title": "Tee",
            "price": 1000,
            "options": ["Size"],
            "variants": [{ "options": ["S"], "sku": "TEE-S" }, { "options": ["M"], "sku": "" }]
        }
    ]}));
    let render = |path: &str| {
        let output = fixture.liquid(
            path,
            Session::initial(&fixture.store),
            "{{ product | structured_data }}",
        );
        serde_json::from_str::<serde_json::Value>(&output).expect(&output)
    };
    let keys = |value: &serde_json::Value| -> Vec<String> {
        value.as_object().unwrap().keys().cloned().collect()
    };
    let plain = render("/products/plain");
    assert_eq!(plain["sku"], "PLAIN-1");
    assert_eq!(keys(&plain)[8..], ["offers", "sku", "url"]);
    assert!(render("/products/bare").get("sku").is_none());
    let tee = render("/products/tee");
    assert_eq!(
        keys(&tee["hasVariant"][0]),
        ["@id", "@type", "name", "offers", "sku"]
    );
    assert_eq!(
        keys(&tee["hasVariant"][1]),
        ["@id", "@type", "name", "offers"]
    );
}

#[test]
fn an_apps_own_metaobject_type_is_found_by_either_name() {
    // The store data writes an app's own type `$app:author`. A theme names it with the app's id
    // (its real one, which lsf does not know), an app's extension with `$app`: both find the
    // entry, and so does a reference to it.
    let overlay = serde_json::json!({
        "metaobjects": {
            "$app:author": [{
                "handle": "jane",
                "fields": { "name": { "type": "single_line_text_field", "value": "Jane Austen" } }
            }],
            "book": [{
                "handle": "emma",
                "fields": { "author": { "type": "metaobject_reference", "value": "$app:author/jane" } }
            }]
        }
    });
    let (store, diagnostics) = lsf_core::store::load::load_with_overlay(
        &DataSource::Demo,
        &LoadOptions::default(),
        Some(&overlay),
    );
    assert!(diagnostics.is_empty(), "{diagnostics}");
    let fixture = Fixture {
        store: Arc::new(store),
        ..fixture()
    };
    let output = fixture.liquid(
        "/",
        Session::initial(&fixture.store),
        "{{ metaobjects['app--123456--author'].jane.name }}|\
         {% for author in metaobjects['$app:author'].values %}{{ author.name }}{% endfor %}|\
         {{ metaobjects.book.emma.author.value.name }}",
    );
    assert_eq!(output, "Jane Austen|Jane Austen|Jane Austen");
}

#[test]
fn a_reference_by_gid_reads_what_validate_resolves() {
    // Shopify writes every reference as a GID: one that names an entity of the store data
    // (by its id, `metaobject.system.id` for an entry) reads it, as `type/handle` does.
    let faq = lsf_core::util::stable_id("metaobject", "faq/size");
    let (demo, _) = lsf_core::store::load::load(&DataSource::Demo, &LoadOptions::default());
    let mug = demo.product_by_handle("ceramic-mug").expect("mug").id;
    let overlay = serde_json::json!({
        "metaobjects": {
            "faq": [{ "handle": "size", "fields": { "question": "Which size?" } }],
            "guide": [{
                "handle": "fit",
                "fields": {
                    "faqs": { "type": "list.metaobject_reference", "value": [format!("gid://shopify/Metaobject/{faq}"), "faq/size"] },
                    "product": { "type": "product_reference", "value": format!("gid://shopify/Product/{mug}") }
                }
            }]
        }
    });
    let (store, diagnostics) = lsf_core::store::load::load_with_overlay(
        &DataSource::Demo,
        &LoadOptions::default(),
        Some(&overlay),
    );
    assert!(diagnostics.is_empty(), "{diagnostics}");
    let fixture = Fixture {
        store: Arc::new(store),
        ..fixture()
    };
    let output = fixture.liquid(
        "/",
        Session::initial(&fixture.store),
        "{% for faq in metaobjects.guide.fit.faqs.value %}{{ faq.question }}/{{ faq.system.id }};{% endfor %}\
         {{ metaobjects.guide.fit.product.value.handle }}",
    );
    assert_eq!(
        output,
        format!("Which size?/{faq};Which size?/{faq};ceramic-mug")
    );
}

#[test]
fn metafield_values_are_the_objects_shopify_gives() {
    let overlay = serde_json::json!({
        "metaobjects": {
            "spec": [{
                "handle": "tent",
                "fields": {
                    "shade": { "type": "color", "value": "#ff8000" },
                    "story": { "type": "article_reference", "value": "journal/how-to-care-for-linen" },
                    "same_story": { "type": "article_reference", "value": "how-to-care-for-linen" },
                    "length": { "type": "distance", "value": { "value": "2.5", "unit": "m" } },
                    "price": { "type": "money", "value": { "amount": "17.96", "currency_code": "USD" } },
                    "price_eur": { "type": "money", "value": { "amount": "5", "currency_code": "eur" } },
                    "width": { "type": "dimension", "value": { "value": "2.555", "unit": "cm" } }
                }
            }]
        }
    });
    let (store, diagnostics) = lsf_core::store::load::load_with_overlay(
        &DataSource::Demo,
        &LoadOptions::default(),
        Some(&overlay),
    );
    // Shopify refuses a money in another currency than the shop's (#195): an error of the
    // data, which still renders.
    let codes: Vec<(&str, &str)> = diagnostics
        .items
        .iter()
        .map(|diagnostic| (diagnostic.code, diagnostic.path.as_str()))
        .collect();
    assert_eq!(
        codes,
        [(
            "invalid_metafield",
            "/metaobjects/spec/0/fields/price_eur/value/currency_code"
        )],
        "{diagnostics}"
    );
    let fixture = Fixture {
        store: Arc::new(store),
        ..fixture()
    };
    let render = |source: &str| fixture.liquid("/", Session::initial(&fixture.store), source);
    let tent = "{% assign tent = metaobjects.spec.tent %}";

    // A color, with its channels.
    assert_eq!(
        render(&format!(
            "{tent}{{{{ tent.shade.value }}}} {{{{ tent.shade.value.red }}}} {{{{ tent.shade.value.rgb }}}}"
        )),
        "#ff8000 255 255 128 0"
    );
    // An article, by blog and handle or by its handle alone.
    assert_eq!(
        render(&format!(
            "{tent}{{{{ tent.story.value.title }}}}|{{{{ tent.same_story.value.title }}}}|{{{{ tent.story | metafield_text }}}}"
        )),
        "How to care for linen|How to care for linen|How to care for linen"
    );
    // Every measurement type, not only weight, volume and dimension.
    assert_eq!(
        render(&format!(
            "{tent}{{{{ tent.length.value.type }}}} {{{{ tent.length.value.value }}}} {{{{ tent.length.value.unit }}}}|{{{{ tent.length | metafield_text }}}}"
        )),
        "distance 2.5 m|2.5 m"
    );
    // Money prints and computes as cents, and has the currency.
    assert_eq!(
        render(&format!(
            "{tent}{{{{ tent.price.value }}}}|{{{{ tent.price.value | money }}}}|{{{{ tent.price.value | plus: 4 }}}}|\
             {{% if tent.price.value > 1000 %}}more{{% endif %}}|{{{{ tent.price.value.currency.iso_code }}}}"
        )),
        "1796|$17.96|1800|more|USD"
    );
    // A money keeps the currency it is written in, and is printed in it.
    assert_eq!(
        render(&format!(
            "{tent}{{{{ tent.price_eur.value }}}} {{{{ tent.price_eur.value.currency.iso_code }}}}|\
             {{{{ tent.price_eur | metafield_text }}}}|{{{{ tent.price_eur.value | money }}}}|\
             {{{{ tent.price_eur.value | money_without_trailing_zeros }}}}|\
             {{{{ tent.price.value | money_with_currency }}}}"
        )),
        "500 EUR|€5,00 EUR|€5,00|€5|$17.96 USD"
    );
    // A measurement is printed with two decimals at most.
    assert_eq!(
        render(&format!(
            "{tent}{{{{ tent.width | metafield_text }}}}|{{{{ tent.width | metafield_tag }}}}"
        )),
        "2.56 cm|<span class=\"metafield-dimension\"><span class=\"metafield-dimension_value\">2.56 </span><span class=\"metafield-dimension_unit\">cm</span></span>"
    );
}

/// A product with a list of products and a list of metaobjects, the products it names, and a
/// type of three FAQ entries.
fn reference_lists_data() -> serde_json::Value {
    serde_json::json!({
        "products": [
            { "title": "Widget", "price": "10.00", "metafields": { "custom": {
                "related": { "type": "list.product_reference", "value": ["alpha", "beta", "gamma"] },
                "faqs": { "type": "list.metaobject_reference", "value": ["faq/one", "faq/two", "faq/three"] },
                "places": { "type": "list.single_line_text_field", "value": ["Ottawa", "Vancouver"] }
            } } },
            { "title": "Alpha", "price": "10.00" },
            { "title": "Beta", "price": "10.00" },
            { "title": "Gamma", "price": "10.00" }
        ],
        "metaobjects": { "faq": [
            { "handle": "one", "fields": { "question": "How long does shipping take?" } },
            { "handle": "two", "fields": { "question": "Can I return an item?" } },
            { "handle": "three", "fields": { "question": "Do you ship to Belgium?" } }
        ] }
    })
}

#[test]
fn reference_lists_and_product_lists_are_list_drops() {
    // https://shopify.dev/docs/api/liquid/objects/metafield#metafield-determining-the-length-of-a-list-metafield:
    // a list of a reference type has `count`; the lists of other types "are rendered as
    // arrays", and only those are read by index. A `product_list` setting has `.count` too
    // (https://shopify.dev/docs/storefronts/themes/architecture/settings/input-settings#product_list).
    // On Shopify, `.first`, `.last` and `[n]` of a list drop give nothing: the `first` and
    // `last` filters and `for` reach its items.
    let picks = "{%- assign l = section.settings.products -%}\
                 <p>[{{ l.first.title }}]|[{{ l[1].title }}]|[{{ l.last.title }}]|[{{ l.count }}]|\
                 [{{ l.size }}]|[{{ l | first | map: 'title' }}]|[{{ l | last | map: 'title' }}]|\
                 [{% for p in l limit: 1 %}{{ p.title }}{% endfor %}]|[{{ l }}]</p>\
                 {% schema %}{\"name\": \"Picks\", \"tag\": null, \"settings\": \
                 [{\"type\": \"product_list\", \"id\": \"products\", \"label\": \"Products\"}]}{% endschema %}";
    let fixture = theme_with_data(
        "reference-lists",
        &reference_lists_data(),
        &[
            (
                "layout/theme.liquid",
                "<body>{{ content_for_layout }}</body>",
            ),
            (
                "templates/index.json",
                r#"{ "sections": {
                    "set": { "type": "picks", "settings": { "products": ["alpha", "beta", "gamma"] } },
                    "unset": { "type": "picks", "settings": {} }
                }, "order": ["set", "unset"] }"#,
            ),
            ("sections/picks.liquid", picks),
        ],
    );
    let render =
        |source: &str| fixture.liquid("/products/widget", Session::initial(&fixture.store), source);
    assert_eq!(
        render(
            "{%- assign r = product.metafields.custom.related.value -%}\
             [{{ r.first.handle }}]|[{{ r[1].handle }}]|[{{ r.last.handle }}]|[{{ r.count }}]|\
             [{{ r.size }}]|[{% assign x = r | first %}{{ x.handle }}]|[{{ r | last | map: 'handle' }}]|\
             [{% for p in r %}{{ p.handle }},{% endfor %}]|[{{ r }}]"
        ),
        "[]|[]|[]|[3]|[3]|[alpha]|[gamma]|[alpha,beta,gamma,]|[ProductListDrop]"
    );
    assert_eq!(
        render(
            "{%- assign q = product.metafields.custom.faqs.value -%}\
             [{{ q.first.question }}]|[{{ q[0].question }}]|[{{ q.last.question }}]|[{{ q.count }}]|\
             [{{ q.size }}]|[{{ q | first | map: 'question' }}]|[{{ q }}]"
        ),
        "[]|[]|[]|[3]|[3]|[How long does shipping take?]|[MetaobjectListDrop]"
    );
    assert_eq!(
        render(
            "{%- assign v = metaobjects.faq.values -%}\
             [{{ v.first.question }}]|[{{ v[0].question }}]|[{{ v.count }}]|[{{ v.size }}]|\
             [{% for e in v %}{{ e.system.handle }},{% endfor %}]"
        ),
        "[]|[]|[3]|[3]|[one,two,three,]"
    );
    // A list of a type that is not a reference stays an array, as on Shopify.
    assert_eq!(
        render(
            "{%- assign t = product.metafields.custom.places.value -%}\
             {{ t.first }}|{{ t[1] }}|{{ t.last }}|{{ t.size }}|[{{ t.count }}]"
        ),
        "Ottawa|Vancouver|Vancouver|2|[]"
    );
    // A `product_list` setting, set and unset: `.count` is 0 when nothing is selected.
    let rendered = fixture.page("/");
    assert_clean(&rendered);
    let lines: Vec<&str> = rendered
        .body
        .split("<p>")
        .skip(1)
        .map(|line| line.split("</p>").next().unwrap())
        .collect();
    assert_eq!(
        lines,
        [
            "[]|[]|[]|[3]|[3]|[Alpha]|[Gamma]|[Alpha]|[ProductListDrop]",
            "[]|[]|[]|[0]|[0]|[]|[]|[]|[ProductListDrop]",
        ]
    );
}

#[test]
fn a_product_list_setting_takes_the_products_of_a_list_metafield() {
    // A `product_list` setting connected to a `list.product_reference` metafield in the theme
    // editor (a dynamic source) holds the metafield's products, in its order, as on Shopify.
    let fixture = theme_with_data(
        "product-list-dynamic-source",
        &reference_lists_data(),
        &[
            (
                "layout/theme.liquid",
                "<body>{{ content_for_layout }}</body>",
            ),
            (
                "templates/product.json",
                r#"{ "sections": { "r": { "type": "related", "settings": {
                    "plist": "{{ product.metafields.custom.related.value }}"
                } } }, "order": ["r"] }"#,
            ),
            (
                "sections/related.liquid",
                "<p>{{ section.settings.plist.size }}|{{ section.settings.plist.count }}|\
                 {% for p in section.settings.plist %}{{ p.handle }},{% endfor %}</p>\
                 {% schema %}{\"name\": \"Related\", \"tag\": null, \"settings\": \
                 [{\"type\": \"product_list\", \"id\": \"plist\", \"label\": \"Products\"}]}{% endschema %}",
            ),
        ],
    );
    let rendered = fixture.page("/products/widget");
    assert_clean(&rendered);
    assert!(
        rendered.body.contains("<p>3|3|alpha,beta,gamma,</p>"),
        "{}",
        rendered.body
    );
}

#[test]
fn a_metafield_without_value_compares_filters_and_sorts_as_its_value() {
    // On Shopify, a metafield of a text, number, boolean or date type used without `.value`
    // acts as its value: in `if`, `unless`, `==`, `>`, math and date filters, `size`, and as
    // the property of `sort`, `where` and `sum`.
    let overlay = serde_json::json!({
        "metaobjects": {
            "probe": [{
                "handle": "f",
                "fields": {
                    "off": { "type": "boolean", "value": false },
                    "on": { "type": "boolean", "value": true },
                    "n": { "type": "number_integer", "value": 42 },
                    "dec": { "type": "number_decimal", "value": "1.50" },
                    "day": { "type": "date", "value": "2026-03-05" },
                    "finish": { "type": "single_line_text_field", "value": "gloss" },
                    "color": { "type": "color", "value": "#ff8000" }
                }
            }],
            "faq": [
                { "handle": "one", "fields": { "question": "How long does shipping take?", "order": { "type": "number_integer", "value": 1 } } },
                { "handle": "two", "fields": { "question": "Can I return an item?", "order": { "type": "number_integer", "value": 2 } } },
                { "handle": "three", "fields": { "question": "Do you ship to Belgium?", "order": { "type": "number_integer", "value": 3 } } }
            ]
        }
    });
    let (store, diagnostics) = lsf_core::store::load::load_with_overlay(
        &DataSource::Demo,
        &LoadOptions::default(),
        Some(&overlay),
    );
    assert!(diagnostics.is_empty(), "{diagnostics}");
    let fixture = Fixture {
        store: Arc::new(store),
        ..fixture()
    };
    let render = |source: &str| {
        fixture.liquid(
            "/",
            Session::initial(&fixture.store),
            &format!("{{%- assign f = metaobjects.probe.f -%}}{source}"),
        )
    };
    assert_eq!(
        render(
            "{% if f.off %}A{% else %}a{% endif %}|{% if f.off == false %}C{% else %}c{% endif %}|\
             {% unless f.off %}D{% endunless %}|{% if f.on == true %}E{% else %}e{% endif %}|\
             {% if f.finish == 'gloss' %}F{% else %}f{% endif %}|{% if f.dec == 1.5 %}G{% else %}g{% endif %}|\
             {% if f.n > 40 %}H{% else %}h{% endif %}|{% if f.n == 42 %}J{% else %}j{% endif %}"
        ),
        "a|C|D|E|F|G|H|J"
    );
    assert_eq!(
        render(
            "{{ f.n | plus: 1 }}|{{ f.dec | times: 2 }}|{{ f.day | date: '%d.%m.%Y' }}|{{ f.finish | size }}"
        ),
        "43|3.0|05.03.2026|5"
    );
    assert_eq!(
        render(
            "{%- assign v = metaobjects.faq.values -%}\
             {{ v | sort: 'question' | map: 'order' | join: ',' }}|\
             {{ v | where: 'order', 3 | map: 'question' }}|{{ v | map: 'order' | sum }}"
        ),
        "2,3,1|Do you ship to Belgium?|6"
    );
    // It still prints its value and has its properties, and `json` refuses it, as on Shopify.
    // A type whose value is an object (a color) stays the metafield.
    assert_eq!(
        render(
            "{{ f.n }}|{{ f.off | json }}|{{ f.n.value }}|{{ f.n.type }}|{{ f.color.value.red }}|\
             {% if f.color == '#ff8000' %}same{% else %}other{% endif %}"
        ),
        "42|{\"error\":\"json not allowed for this object\"}|42|number_integer|255|other"
    );
}

#[test]
fn metafield_values_as_the_admin_api_returns_them_render_as_their_type() {
    // https://shopify.dev/docs/apps/build/metafields/list-of-data-types: the value of a
    // metafield "is always entered and stored as a string". The Admin API returns `"true"`,
    // `"42"`, units as enums (`KILOGRAMS`), and still has the deprecated types `string`,
    // `integer` and `json_string`.
    let overlay = serde_json::json!({
        "metaobjects": {
            "probe": [{
                "handle": "f",
                "fields": {
                    "on": { "type": "boolean", "value": "true" },
                    "off": { "type": "boolean", "value": "false" },
                    "n": { "type": "number_integer", "value": "42" },
                    "mass": { "type": "weight", "value": "{\"value\":2.25,\"unit\":\"KILOGRAMS\"}" },
                    "legacy": { "type": "string", "value": "legacy" },
                    "count": { "type": "integer", "value": "7" },
                    "data": { "type": "json_string", "value": "{\"a\":[1,2]}" }
                }
            }]
        }
    });
    let (store, diagnostics) = lsf_core::store::load::load_with_overlay(
        &DataSource::Demo,
        &LoadOptions::default(),
        Some(&overlay),
    );
    assert!(diagnostics.is_empty(), "{diagnostics}");
    let fixture = Fixture {
        store: Arc::new(store),
        ..fixture()
    };
    let render = |source: &str| {
        fixture.liquid(
            "/",
            Session::initial(&fixture.store),
            &format!("{{%- assign f = metaobjects.probe.f -%}}{source}"),
        )
    };
    assert_eq!(
        render(
            "{% if f.on.value %}on{% else %}off{% endif %}|{% if f.off.value %}on{% else %}off{% endif %}|\
             {% if f.on %}on{% else %}off{% endif %}|{{ f.n.value | plus: 1 }}|{{ f.mass.value.value }}|\
             {{ f.legacy.value }}|{{ f.legacy.type }}"
        ),
        "on|off|on|43|2.25|legacy|string"
    );
    // The deprecated types give their value directly, a number for `integer` and the JSON of
    // a `json_string`.
    assert_eq!(
        render(
            "{{ f.count | plus: 1 }}|{{ f.count.value | plus: 1 }}|{{ f.data.value.a | join: ',' }}"
        ),
        "8|8|1,2"
    );
}

#[test]
fn date_metafields_print_the_stored_string() {
    // https://shopify.dev/docs/api/liquid/objects/metafield: the value of a `date` or a
    // `date_time` is "A date string. To format the string, use the date filter." A `date_time`
    // "Defaults to Greenwich Mean Time (GMT)" (list of data types): one written without an
    // offset is stored with `+00:00`. The demo shop is in America/New_York.
    let overlay = serde_json::json!({
        "metaobjects": {
            "probe": [{
                "handle": "f",
                "fields": {
                    "day": { "type": "date", "value": "2026-03-05" },
                    "at": { "type": "date_time", "value": "2026-03-05T14:30:00+01:00" },
                    "utc": { "type": "date_time", "value": "2026-03-05T23:30:00Z" },
                    "times": { "type": "list.date_time", "value": ["2026-03-05T14:30:00", "2026-03-06T09:00:00Z"] }
                }
            }]
        }
    });
    let (store, diagnostics) = lsf_core::store::load::load_with_overlay(
        &DataSource::Demo,
        &LoadOptions::default(),
        Some(&overlay),
    );
    assert!(diagnostics.is_empty(), "{diagnostics}");
    let fixture = Fixture {
        store: Arc::new(store),
        ..fixture()
    };
    let render = |source: &str| {
        fixture.liquid(
            "/",
            Session::initial(&fixture.store),
            &format!(
                "{{%- assign f = metaobjects.probe.f -%}}{{%- assign d = f.day -%}}\
                 {{%- assign t = f.at -%}}{source}"
            ),
        )
    };
    // The issue's repro, with what a store prints.
    assert_eq!(
        render("[{{ d.value }}]|[{{ d }}]|{{ d.value | json }}"),
        r#"[2026-03-05]|[2026-03-05]|"2026-03-05""#
    );
    assert_eq!(
        render("[{{ t.value }}]|[{{ t }}]|{{ t.value | json }}"),
        r#"[2026-03-05T14:30:00+01:00]|[2026-03-05T14:30:00+01:00]|"2026-03-05T14:30:00+01:00""#
    );
    assert_eq!(
        render("{{ f.utc.value }}|{{ f.times.value | join: ',' }}"),
        "2026-03-05T23:30:00Z|2026-03-05T14:30:00+00:00,2026-03-06T09:00:00Z"
    );
    // The date filter still reads the time, with or without `.value`.
    assert_eq!(
        render(
            "{{ d | date: '%d.%m.%Y' }}|{{ d.value | date: '%d.%m.%Y' }}|{{ t | date: '%s' }}|\
             {{ f.utc.value | date: '%s' }}"
        ),
        "05.03.2026|05.03.2026|1772717400|1772753400"
    );
    // `metafield_tag` gives the stored value as `datetime`, as Shopify's reference shows.
    assert_eq!(
        render("{{ d | metafield_tag }}"),
        r#"<time datetime="2026-03-05" class="metafield-date">March 5, 2026</time>"#
    );
    assert!(
        render("{{ t | metafield_tag }}").starts_with(
            r#"<time datetime="2026-03-05T14:30:00+01:00" class="metafield-date_time">"#
        ),
        "{}",
        render("{{ t | metafield_tag }}")
    );
}

#[test]
fn measurement_and_rating_metafields_print_as_a_store_prints_them() {
    // Shopify's `measurement` object (https://shopify.dev/docs/api/liquid/objects/measurement)
    // is `{"type": "volume", "unit": "mL", "value": "500.0"}`, and a store prints it as its
    // value and unit, with two decimals at most, as `metafield_text` formats measurements. A
    // `rating` prints its rating.
    let overlay = serde_json::json!({
        "metaobjects": {
            "probe": [{
                "handle": "f",
                "fields": {
                    "len": { "type": "dimension", "value": { "value": 1.5, "unit": "cm" } },
                    "mass": { "type": "weight", "value": { "value": 2.25, "unit": "kg" } },
                    "vol": { "type": "volume", "value": { "value": 500, "unit": "ml" } },
                    "cl": { "type": "volume", "value": { "value": 33, "unit": "cl" } },
                    "gal": { "type": "volume", "value": { "value": 1, "unit": "us_gal" } },
                    "stars": { "type": "rating", "value": { "scale_min": "0.0", "scale_max": "10.0", "value": "3.5" } },
                    "fine": { "type": "weight", "value": { "value": 1.234, "unit": "grams" } },
                    "long": { "type": "dimension", "value": { "value": 30, "unit": "centimeters" } }
                }
            }]
        }
    });
    let (store, diagnostics) = lsf_core::store::load::load_with_overlay(
        &DataSource::Demo,
        &LoadOptions::default(),
        Some(&overlay),
    );
    assert!(diagnostics.is_empty(), "{diagnostics}");
    let fixture = Fixture {
        store: Arc::new(store),
        ..fixture()
    };
    let render = |source: &str| {
        fixture.liquid(
            "/",
            Session::initial(&fixture.store),
            &format!("{{%- assign f = metaobjects.probe.f -%}}{source}"),
        )
    };
    // The issue's repro, with what a store prints.
    assert_eq!(
        render("{{ f.len.value }}|{{ f.mass.value }}|{{ f.vol.value }}|{{ f.stars.value }}"),
        "1.5 cm|2.25 kg|500 mL|3.5"
    );
    assert_eq!(
        render(
            "{{ f.len.value.value }} {{ f.len.value.unit }}|{{ f.vol.value.value }} {{ f.vol.value.unit }}|\
             {{ f.cl.value.unit }}|{{ f.gal.value.unit }}"
        ),
        "1.5 cm|500.0 mL|cL|gal"
    );
    // Two decimals at most; the unit names of Shopify's docs give the same short units.
    assert_eq!(
        render("{{ f.fine.value }}|{{ f.long.value }}|{{ f.fine }}|{{ f.fine | metafield_text }}"),
        "1.23 g|30 cm|1.23 g|1.23 g"
    );
    // The properties do not change: `type`, and the rating's strings.
    assert_eq!(
        render(
            "{{ f.vol.value.type }}|{{ f.stars.value.rating }}|{{ f.stars.value.scale_min }}|\
             {{ f.stars.value.scale_max }}|{{ f.stars.value | json }}"
        ),
        r#"volume|3.5|0.0|10.0|{"rating":"3.5","scale_max":"10.0","scale_min":"0.0"}"#
    );
    assert_eq!(
        render("{{ f.vol | metafield_tag }}"),
        r#"<span class="metafield-volume"><span class="metafield-volume_value">500 </span><span class="metafield-volume_unit">mL</span></span>"#
    );
}

#[test]
fn a_handle_that_names_nothing_is_the_empty_object() {
    // https://shopify.dev/docs/api/liquid/basics#empty: "An `empty` object is returned if you
    // try to access an object that is defined, but has no value", truthy, so that themes
    // compare it with `empty`. all_products: "If the product isn't found, then `empty` is
    // returned."
    let fixture = fixture();
    let render = |source: &str| {
        fixture.liquid(
            "/products/organic-cotton-t-shirt",
            Session::initial(&fixture.store),
            source,
        )
    };
    let probe = |name: &str, lookup: &str, size: &str| {
        render(&format!(
            "{{% assign x = {lookup} %}}{name}: {{% if x %}}truthy{{% else %}}falsy{{% endif %}} \
             {{% if x == empty %}}empty{{% endif %}} size={{{{ {size} }}}}"
        ))
    };
    assert_eq!(
        probe("collection", "collections['nope']", "x.products.size"),
        "collection: truthy empty size=0"
    );
    assert_eq!(
        probe("product", "all_products['nope']", "x.size"),
        "product: truthy empty size=0"
    );
    assert_eq!(
        probe("page", "pages['nope']", "x.size"),
        "page: truthy empty size=0"
    );
    assert_eq!(
        probe("blog", "blogs['nope']", "x.articles.size"),
        "blog: truthy empty size=0"
    );
    assert_eq!(
        probe("menu", "linklists['nope']", "x.links.size"),
        "menu: truthy empty size=0"
    );
    assert_eq!(
        probe("menu", "linklists['']", "x.links.size"),
        "menu: truthy empty size=0"
    );
    assert_eq!(
        probe("namespace", "product.metafields.nope", "x.size"),
        "namespace: truthy empty size=0"
    );
    // It prints nothing, its other properties are nil, and `json` refuses it, as on Shopify.
    // A missing key inside a namespace is still nil, and `size` of the lookups is not a handle.
    assert_eq!(
        render(
            "[{{ collections['nope'] }}]|[{{ collections['nope'].title }}]|\
             {{ collections['nope'] | json }}|\
             {% if product.metafields.custom.nope %}found{% else %}nil{% endif %}|\
             {{ linklists.size }}|{{ product.metafields.size }}"
        ),
        format!(
            "[]|[]|{{\"error\":\"json not allowed for this object\"}}|nil|{}|",
            fixture.store.menus.len()
        )
    );
}

#[test]
fn the_collections_list_is_the_store_s_own_collections() {
    // #96: a loop over `collections` (https://shopify.dev/docs/api/liquid/objects/collections)
    // gives the store's collections without the catch-all `all`, which `collections.all` still
    // finds; `.size` and `.first` are those of that list, as `| size` and `| first` are.
    let fixture = fixture();
    let own: Vec<&str> = fixture
        .store
        .collections
        .iter()
        .map(|collection| collection.handle.as_str())
        .filter(|handle| *handle != "all")
        .collect();
    let rendered = fixture.liquid(
        "/",
        Session::initial(&fixture.store),
        "{{ collections.size }}|{{ collections | size }}|{{ collections.first.handle }}|\
         {{ collections.last.handle }}|{% for c in collections %}{{ c.handle }},{% endfor %}|\
         {% if collections contains collections.all %}listed{% endif %}|{{ collections.all.handle }}|\
         {% paginate collections by 2 %}{{ paginate.items }}{% endpaginate %}",
    );
    assert_eq!(
        rendered,
        format!(
            "{n}|{n}|{first}|{last}|{list},||all|{n}",
            n = own.len(),
            first = own[0],
            last = own[own.len() - 1],
            list = own.join(","),
        )
    );
}

#[test]
fn a_render_looks_up_20_unique_handles_in_all_products_and_in_each_metaobject_type() {
    // #95: https://shopify.dev/docs/api/liquid/objects/all_products, "a limit of 20 unique
    // handles per page", and https://shopify.dev/docs/apps/build/metaobjects/metaobject-limits,
    // 20 handles of each type "including handles that don't match a published entry". The
    // lookup past them, and every later one, renders the error in place of the entry.
    let overlay = serde_json::json!({
        "metaobjects": {
            "chart": [{ "handle": "small", "fields": { "name": "Small" } }],
            "size": [{ "handle": "s", "fields": { "name": "S" } }]
        }
    });
    let (store, diagnostics) = lsf_core::store::load::load_with_overlay(
        &DataSource::Demo,
        &LoadOptions::default(),
        Some(&overlay),
    );
    assert!(diagnostics.is_empty(), "{diagnostics}");
    let fixture = Fixture {
        store: Arc::new(store),
        ..fixture()
    };
    let product = &fixture.store.products[0].handle;
    let site = fixture.renderer.site(
        fixture.store.clone(),
        request("/"),
        Session::initial(&fixture.store),
    );
    let page = lsf_theme::render::routes::resolve(&site);
    let source = format!(
        "{{%- for i in (1..19) -%}}{{%- assign h = 'nope-' | append: i -%}}\
         {{{{ all_products[h].title }}}}{{{{ metaobjects.chart[h].name }}}}{{%- endfor -%}}\
         [{{{{ all_products['{product}'].title }}}}|{{{{ all_products.nope-1.title }}}}]\
         [{{{{ metaobjects.chart.small.name }}}}|{{{{ metaobjects.chart.values.size }}}}]\
         [{{{{ all_products['nope-20'].title }}}}][{{{{ all_products['{product}'].title }}}}]\
         [{{{{ metaobjects.chart.nope-20.name }}}}][{{{{ metaobjects.size.s.name }}}}]"
    );
    let (output, errors) = fixture
        .renderer
        .render_liquid(&site, page, &source, &[])
        .expect("the template parses");
    let error = |list: &str| {
        format!("Liquid error (line 1): Exceeded maximum number of unique handles for {list}.")
    };
    assert_eq!(
        output,
        format!(
            "[{}|][Small|1][{all}][{all}][{chart}][S]",
            fixture.store.products[0].title,
            all = error("all_products"),
            chart = error("chart"),
        )
    );
    assert_eq!(errors.len(), 3);
}

#[test]
fn a_resource_setting_whose_handle_names_nothing_is_truthy_and_blank() {
    // https://shopify.dev/docs/storefronts/themes/architecture/settings/input-settings#product:
    // `blank` if "the selection no longer exists", and outputting the setting "will return the
    // object's handle". The value is truthy, `== blank`, `== empty` and `== nil`.
    let settings = [
        ("product", "deleted-product", ""),
        ("collection", "deleted-collection", " {{ x.products.size }}"),
        ("page", "deleted-page", ""),
        ("blog", "deleted-blog", " {{ x.articles.size }}"),
        ("article", "news/deleted-article", ""),
        ("link_list", "deleted-menu", " {{ x.links.size }}"),
    ];
    let sections: serde_json::Map<String, serde_json::Value> = settings
        .iter()
        .map(|(kind, handle, _)| {
            (
                (*kind).to_string(),
                serde_json::json!({ "type": kind, "settings": { "x": handle } }),
            )
        })
        .collect();
    let template = serde_json::json!({
        "sections": sections,
        "order": settings.iter().map(|(kind, _, _)| *kind).collect::<Vec<_>>(),
    })
    .to_string();
    let sources: Vec<(String, String)> = settings
        .iter()
        .map(|(kind, _, extra)| {
            (
                format!("sections/{kind}.liquid"),
                format!(
                    "{{% assign x = section.settings.x %}}<p>[{{{{ x }}}}] \
                     {{% if x %}}truthy{{% else %}}falsy{{% endif %}} \
                     {{% if x == blank %}}blank{{% endif %}} {{% if x == empty %}}empty{{% endif %}} \
                     {{% if x == nil %}}nil{{% endif %}}{extra}</p>\
                     {{% schema %}}{{\"name\": \"S\", \"tag\": null, \"settings\": \
                     [{{\"type\": \"{kind}\", \"id\": \"x\", \"label\": \"X\"}}]}}{{% endschema %}}"
                ),
            )
        })
        .collect();
    let mut files: Vec<(&str, &str)> = vec![
        (
            "layout/theme.liquid",
            "<body>{{ content_for_layout }}</body>",
        ),
        ("templates/index.json", &template),
    ];
    files.extend(
        sources
            .iter()
            .map(|(path, source)| (path.as_str(), source.as_str())),
    );
    let fixture = theme_of("dead-resource-settings", &[], &files);
    let rendered = fixture.page("/");
    assert_clean(&rendered);
    let lines: Vec<&str> = rendered
        .body
        .split("<p>")
        .skip(1)
        .map(|line| line.split("</p>").next().unwrap())
        .collect();
    assert_eq!(
        lines,
        [
            "[deleted-product] truthy blank empty nil",
            "[deleted-collection] truthy blank empty nil 0",
            "[deleted-page] truthy blank empty nil",
            "[deleted-blog] truthy blank empty nil 0",
            "[news/deleted-article] truthy blank empty nil",
            "[deleted-menu] truthy blank empty nil 0",
        ]
    );
}

#[test]
fn money_metafields_are_in_the_currency_of_the_visitors_market() {
    // The issue's store, with the rate of the issue's store on Shopify for the variant.
    for (rate, product_line, variant_line) in [
        (
            "0.9",
            "€9,00|€7,20 EUR|720|EUR|<span class=\"metafield-money\">€7,20 EUR</span>|€7,20 EUR",
            "€584,96|58496|EUR",
        ),
        (
            "0.918078665",
            "€9,18|€7,34 EUR|734|EUR|<span class=\"metafield-money\">€7,34 EUR</span>|€7,34 EUR",
            "€596,71|59671|EUR",
        ),
    ] {
        let overlay = serde_json::json!({
            "localization": {
                "countries": [{"iso_code": "US"}, {"iso_code": "BE"}],
                "languages": [{"iso_code": "en"}, {"iso_code": "fr"}],
                "markets": [{"handle": "us", "countries": ["US"]},
                    {"handle": "europe", "countries": ["BE"], "currency": "EUR", "rate": rate,
                     "subfolder_suffix": "be"}]
            },
            "products": [{"title": "Widget", "handle": "widget", "price": "10.00",
                "metafields": {"custom": {"lowest": {"type": "money",
                    "value": {"amount": "8.00", "currency_code": "USD"}}}},
                "variants": [{"metafields": {"custom": {"unit": {"type": "money",
                    "value": {"amount": "649.95", "currency_code": "USD"}}}}}]}]
        });
        let options = LoadOptions {
            theme_locales: vec!["en".to_string(), "fr".to_string()],
            ..LoadOptions::default()
        };
        let (store, diagnostics) =
            lsf_core::store::load::load_with_overlay(&DataSource::Demo, &options, Some(&overlay));
        assert!(diagnostics.is_empty(), "{diagnostics}");
        let fixture = Fixture {
            store: Arc::new(store),
            ..fixture()
        };
        let source = "{% assign m = product.metafields.custom.lowest %}{{ product.price | money }}|\
            {{ m.value | money_with_currency }}|{{ m.value }}|{{ m.value.currency.iso_code }}|\
            {{ m | metafield_tag }}|{{ m | metafield_text }}\
            {% assign u = product.selected_or_first_available_variant.metafields.custom.unit %}\
            #{{ u.value | money }}|{{ u.value }}|{{ u.value.currency.iso_code }}";
        let session = Session::initial(&fixture.store);
        // At `/`, in the shop's currency, as written.
        assert_eq!(
            fixture.liquid("/products/widget", session.clone(), source),
            "$10.00|$8.00 USD|800|USD|<span class=\"metafield-money\">$8.00 USD</span>|$8.00 USD\
             #$649.95|64995|USD"
        );
        // Under `/en-be/`, converted at the market's rate, like the price.
        let mut under = request("/products/widget");
        under.root = "/en-be".to_string();
        under.market = Some("europe".to_string());
        assert_eq!(
            fixture.liquid_for(under, session, source),
            format!("{product_line}#{variant_line}"),
            "rate {rate}"
        );
    }
}

#[test]
fn a_resource_setting_prints_its_handle() {
    // https://shopify.dev/docs/storefronts/themes/architecture/settings/input-settings#product:
    // "outputting the setting directly will return the object's handle". `size` is the length of
    // that handle, as on a store (issue #131); the setting is still the resource otherwise.
    let store = fixture().store;
    let blog = &store.blogs[0];
    let settings = [
        // The same product as the lookup by its handle, with the same JSON.
        (
            "product",
            store.products[0].handle.clone(),
            "{% assign p = all_products[x.handle] %}{% assign a = x | json %}\
             {% assign b = p | json %}{% if a == b and x == p %} same{% endif %}",
        ),
        ("collection", store.collections[0].handle.clone(), ""),
        ("page", store.pages[0].handle.clone(), ""),
        ("blog", blog.handle.clone(), ""),
        (
            "article",
            format!("{}/{}", blog.handle, blog.articles[0].handle),
            "",
        ),
        ("link_list", store.menus[0].handle.clone(), ""),
    ];
    let sections: serde_json::Map<String, serde_json::Value> = settings
        .iter()
        .map(|(kind, handle, _)| {
            (
                (*kind).to_string(),
                serde_json::json!({ "type": kind, "settings": { "x": handle } }),
            )
        })
        .collect();
    let template = serde_json::json!({
        "sections": sections,
        "order": settings.iter().map(|(kind, _, _)| *kind).collect::<Vec<_>>(),
    })
    .to_string();
    let sources: Vec<(String, String)> = settings
        .iter()
        .map(|(kind, _, extra)| {
            (
                format!("sections/{kind}.liquid"),
                format!(
                    "{{% assign x = section.settings.x %}}<p>[{{{{ x }}}}|{{{{ x.size }}}}|\
                     {{{{ x | size }}}}|{{{{ x | append: '' }}}}] \
                     {{% if x == blank %}}blank{{% else %}}set{{% endif %}} \
                     {{% if x.title != blank %}}titled{{% endif %}}{extra}</p>\
                     {{% schema %}}{{\"name\": \"S\", \"tag\": null, \"settings\": \
                     [{{\"type\": \"{kind}\", \"id\": \"x\", \"label\": \"X\"}}]}}{{% endschema %}}"
                ),
            )
        })
        .collect();
    let mut files: Vec<(&str, &str)> = vec![
        (
            "layout/theme.liquid",
            "<body>{{ content_for_layout }}</body>",
        ),
        ("templates/index.json", &template),
    ];
    files.extend(
        sources
            .iter()
            .map(|(path, source)| (path.as_str(), source.as_str())),
    );
    let fixture = theme_of("live-resource-settings", &[], &files);
    let rendered = fixture.page("/");
    assert_clean(&rendered);
    let lines: Vec<&str> = rendered
        .body
        .split("<p>")
        .skip(1)
        .map(|line| line.split("</p>").next().unwrap())
        .collect();
    let expected: Vec<String> = settings
        .iter()
        .map(|(kind, handle, _)| {
            let size = handle.chars().count();
            let same = if *kind == "product" { " same" } else { "" };
            format!("[{handle}|{size}|{size}|{handle}] set titled{same}")
        })
        .collect();
    assert_eq!(lines, expected);
}

#[test]
fn an_unset_setting_is_what_shopify_gives() {
    // https://shopify.dev/docs/storefronts/themes/architecture/settings/input-settings: a
    // `select` or a `radio` without `default` is its first option, a `text_alignment` is
    // `left`, text and `color_background` are empty strings and a cleared `color` stores `""`
    // and is blank; `number`, `url` and `image_picker` are nil. A store gives `""` for text,
    // color and richtext (issue #133).
    let schema = r#"{ "name": "Unset", "settings": [
      { "type": "text", "id": "text", "label": "Text" },
      { "type": "textarea", "id": "area", "label": "Area" },
      { "type": "richtext", "id": "rich", "label": "Rich" },
      { "type": "inline_richtext", "id": "inline", "label": "Inline" },
      { "type": "html", "id": "html", "label": "HTML" },
      { "type": "color", "id": "color", "label": "Color" },
      { "type": "color", "id": "cleared", "label": "Cleared" },
      { "type": "color_background", "id": "background", "label": "Background" },
      { "type": "text_alignment", "id": "align", "label": "Alignment" },
      { "type": "text_alignment", "id": "centered", "label": "Centered", "default": "center" },
      { "type": "select", "id": "choice", "label": "Choice",
        "options": [{ "value": "a", "label": "A" }, { "value": "b", "label": "B" }] },
      { "type": "select", "id": "chosen", "label": "Chosen", "default": "b",
        "options": [{ "value": "a", "label": "A" }, { "value": "b", "label": "B" }] },
      { "type": "radio", "id": "radio", "label": "Radio",
        "options": [{ "value": "x", "label": "X" }, { "value": "y", "label": "Y" }] },
      { "type": "checkbox", "id": "check", "label": "Check" },
      { "type": "number", "id": "number", "label": "Number" },
      { "type": "url", "id": "url", "label": "URL" },
      { "type": "image_picker", "id": "image", "label": "Image" },
      { "type": "liquid", "id": "liquid", "label": "Liquid" }
    ] }"#;
    let ids = [
        "text",
        "area",
        "rich",
        "inline",
        "html",
        "color",
        "cleared",
        "background",
        "align",
        "centered",
        "choice",
        "chosen",
        "radio",
        "check",
        "number",
        "url",
        "image",
        "liquid",
    ];
    let probes: String = ids
        .iter()
        .map(|id| format!("[{{{{ s.{id} | json }}}}]"))
        .collect();
    let section = format!(
        "{{%- assign s = section.settings -%}}<p>{probes}</p>\
         <p class=\"{{{{ 'text-' | append: s.align }}}}\">{{% if s.text == blank and s.color == blank %}}blank{{% endif %}}</p>\
         {{% schema %}}{schema}{{% endschema %}}"
    );
    let fixture = theme_of(
        "unset-settings",
        &[],
        &[
            (
                "layout/theme.liquid",
                "<html><body>{{ content_for_layout }}</body></html>",
            ),
            ("sections/unset.liquid", &section),
            (
                "templates/index.json",
                r#"{ "sections": { "main": { "type": "unset", "settings": { "cleared": "" } } }, "order": ["main"] }"#,
            ),
        ],
    );
    let rendered = fixture.page("/");
    assert_clean(&rendered);
    assert!(
        rendered.body.contains(
            "<p>[\"\"][\"\"][\"\"][\"\"][\"\"][\"\"][\"\"][\"\"][\"left\"][\"center\"][\"a\"][\"b\"]\
             [\"x\"][false][null][null][null][null]</p>\
             <p class=\"text-left\">blank</p>"
        ),
        "{}",
        rendered.body
    );
}

#[test]
fn a_rating_has_strings_as_on_shopify() {
    // The rating of the Admin API's `jsonValue`, the same with numbers, and the Admin API's
    // `value` (the JSON text) pasted as is.
    let overlay = serde_json::json!({
        "metaobjects": {
            "spec": [{
                "handle": "tent",
                "fields": {
                    "stars": { "type": "rating", "value": { "scale_min": "1.0", "scale_max": "5.0", "value": "4.5" } },
                    "numbers": { "type": "rating", "value": { "scale_min": 1, "scale_max": 5, "value": 4.5 } },
                    "pasted": { "type": "rating", "value": "{\"scale_min\":\"1.0\",\"scale_max\":\"5.0\",\"value\":\"4.5\"}" },
                    "several": { "type": "list.rating", "value": [{ "scale_min": "1.0", "scale_max": "10.0", "value": "7.5" }] }
                }
            }]
        }
    });
    let (store, diagnostics) = lsf_core::store::load::load_with_overlay(
        &DataSource::Demo,
        &LoadOptions::default(),
        Some(&overlay),
    );
    assert!(diagnostics.is_empty(), "{diagnostics}");
    let fixture = Fixture {
        store: Arc::new(store),
        ..fixture()
    };
    let render = |source: &str| fixture.liquid("/", Session::initial(&fixture.store), source);
    // The issue's template: `scale_max` is the string "5.0", and the range takes it, as on
    // Shopify. A float 5.0 would raise "invalid integer" there.
    let stars = |field: &str| {
        render(&format!(
            "{{%- assign m = metaobjects.spec.tent.{field} -%}}\
             [{{{{ m.value.scale_max }}}}] [{{{{ m.value.scale_max | json }}}}] \
             [{{% for i in (1..m.value.scale_max) %}}*{{% endfor %}}]"
        ))
    };
    assert_eq!(stars("stars"), "[5.0] [\"5.0\"] [*****]");
    assert_eq!(stars("pasted"), "[5.0] [\"5.0\"] [*****]");
    // Numbers are kept as they are written, as strings.
    assert_eq!(stars("numbers"), "[5] [\"5\"] [*****]");
    // The rating object of Shopify's Liquid reference: `rating`, `scale_max`, `scale_min`.
    assert_eq!(
        render("{{ metaobjects.spec.tent.stars.value | json }}"),
        r#"{"rating":"4.5","scale_max":"5.0","scale_min":"1.0"}"#
    );
    assert_eq!(
        render("{{ metaobjects.spec.tent.numbers.value | json }}"),
        r#"{"rating":"4.5","scale_max":"5","scale_min":"1"}"#
    );
    // The metafield itself is refused, as on Shopify.
    assert_eq!(
        render("{{ metaobjects.spec.tent.pasted | json }}"),
        r#"{"error":"json not allowed for this object"}"#
    );
    // Each rating of a list too.
    assert_eq!(
        render(
            "{% for r in metaobjects.spec.tent.several.value %}{{ r.rating | json }}/{{ r.scale_max | json }}{% endfor %}"
        ),
        r#""7.5"/"10.0""#
    );
    // What reads them as numbers still does.
    assert_eq!(
        render(
            "{% assign m = metaobjects.spec.tent.stars %}{{ m | metafield_text }}|\
             {{ m.value.rating | floor }}|{{ m.value.rating | modulo: 1 }}|{{ m.value.scale_max | minus: m.value.rating }}"
        ),
        "4.5|4|0.5|0.5"
    );
}

#[test]
fn metafield_tag_and_text_print_each_type_as_shopify_does() {
    // The issue's store and template; the expected lines are Shopify's, on a store.
    let overlay = serde_json::json!({
        "products": [
            {"title": "Widget", "handle": "widget", "price": "10.00", "variants": [{"sku": "W-1"}],
             "metafields": {"custom": {
                "nums": {"type": "list.number_integer", "value": [3, 1, 2]},
                "related": {"type": "list.product_reference", "value": ["alpha"]},
                "faq": {"type": "metaobject_reference", "value": "faq/one"},
                "faqs": {"type": "list.metaobject_reference", "value": ["faq/one", "faq/two"]},
                "lang": {"type": "language", "value": "pt-BR"},
                "link": {"type": "link", "value": {"text": "Docs & more", "url": "https://example.com/docs"}},
                "site": {"type": "url", "value": "https://example.com/a?b=1&c=2"},
                "stars": {"type": "rating", "value": {"scale_min": "0.0", "scale_max": "10.0", "value": "3.5"}},
                "pick": {"type": "variant_reference", "value": "W-1"},
                "care": {"type": "rich_text_field", "value": {"type": "root", "children": [
                    {"type": "heading", "level": 2, "children": [{"type": "text", "value": "Care"}]},
                    {"type": "paragraph", "children": [{"type": "text", "value": "Wipe dry."}]}]}},
                "guide": {"type": "rich_text_field", "value": {"type": "root", "children": [
                    {"type": "paragraph", "children": [{"type": "link", "url": "https://example.com/care",
                        "title": "Care guide", "target": "_blank",
                        "children": [{"type": "text", "value": "guide"}]}]}]}},
                "who": {"type": "mixed_reference", "value": "faq/one"},
                "tee": {"type": "file_reference", "value": "tee-1.jpg"}
             }}},
            {"title": "Alpha", "handle": "alpha", "price": "10.00"}
        ],
        "metaobjects": {"faq": [
            {"handle": "one", "fields": {"question": "How long does shipping take?"}},
            {"handle": "two", "fields": {"question": "Can I return an item?"}}
        ]}
    });
    let (store, diagnostics) = lsf_core::store::load::load_with_overlay(
        &DataSource::Demo,
        &LoadOptions::default(),
        Some(&overlay),
    );
    assert!(diagnostics.is_empty(), "{diagnostics}");
    let fixture = Fixture {
        store: Arc::new(store),
        ..theme_of(
            "metafield-tags",
            &[],
            &[
                ("layout/theme.liquid", "{{ content_for_layout }}"),
                (
                    "templates/product.liquid",
                    "{%- assign f = product.metafields.custom -%}\n\
                     nums: [{{ f.nums | metafield_tag }}] [{{ f.nums | metafield_text }}]\n\
                     related: [{{ f.related | metafield_tag }}]\n\
                     faq: [{{ f.faq | metafield_tag }}]\n\
                     faq field: [{{ f.faq | metafield_tag: field: 'question' }}]\n\
                     faqs: [{{ f.faqs | metafield_tag }}]\n\
                     lang: [{{ f.lang | metafield_tag }}]\n\
                     link: [{{ f.link | metafield_tag }}] [{{ f.link | metafield_text }}]\n\
                     url: [{{ f.site | metafield_tag }}]\n\
                     rating: [{{ f.stars | metafield_tag }}]\n\
                     variant: [{{ f.pick | metafield_tag }}]\n\
                     rich: [{{ f.care | metafield_text }}]\n\
                     rich link: [{{ f.guide | metafield_tag }}]\n\
                     faqs field: [{{ f.faqs | metafield_tag: field: 'question' }}]\n\
                     mixed: [{{ f.who | metafield_tag }}] [{{ f.who | metafield_text }}]\n\
                     rating text: [{{ f.stars | metafield_text }}]\n\
                     file text: [{{ f.tee | metafield_text }}] [{{ f.tee.value | image_url }}]\n\
                     variant id: {{ f.pick.value.id }}",
                ),
            ],
        )
    };
    let rendered = fixture.page("/products/widget");
    let lines: Vec<&str> = rendered.body.lines().collect();
    let variant = lines.last().unwrap().trim_start_matches("variant id: ");
    let error = |kind: &str| {
        format!(
            "Liquid error (templates/product line {}): metafield_tag requires a 'field' parameter \
             for metafields of type '{kind}'",
            if kind == "metaobject_reference" { 4 } else { 6 }
        )
    };
    let file = |line: &str| {
        let (text, url) = line
            .trim_start_matches("file text: [")
            .trim_end_matches(']')
            .split_once("] [")
            .unwrap();
        // The URL of the file, as `image_url` gives it without a size.
        (text.to_string(), url.to_string())
    };
    let (file_text, file_url) = file(lines[16]);
    assert_eq!(file_text, file_url);
    assert!(
        file_text.contains("/cdn/shop/files/tee-1.jpg"),
        "{file_text}"
    );
    assert_eq!(
        lines[..16],
        [
            "nums: [] []".to_string(),
            "related: []".to_string(),
            format!("faq: [{}]", error("metaobject_reference")),
            "faq field: [<span class=\"metafield-single_line_text_field\">How long does shipping take?</span>]"
                .to_string(),
            format!("faqs: [{}]", error("list.metaobject_reference")),
            "lang: []".to_string(),
            "link: [<a href=\"https://example.com/docs\" class=\"metafield-url\">Docs &amp; more</a>] []"
                .to_string(),
            "url: [<a href=\"https://example.com/a?b=1&amp;c=2\" class=\"metafield-url\">example.com/a?b=1&amp;c=2</a>]"
                .to_string(),
            "rating: [<span class=\"metafield-rating\">3.5</span>]".to_string(),
            format!(
                "variant: [<a href=\"/products/widget?variant={variant}\" \
                 class=\"metafield-variant_reference\">Default Title</a>]"
            ),
            "rich: [Care".to_string(),
            "Wipe dry.]".to_string(),
            "rich link: [<div class=\"metafield-rich_text_field\"><p><a href=\"https://example.com/care\" \
             target=\"_blank\" title=\"Care guide\">guide</a></p></div>]"
                .to_string(),
            "faqs field: [<ul class=\"metafield-single_line_text_field-array\">\
             <li class=\"metafield-single_line_text_field\">How long does shipping take?</li>\
             <li class=\"metafield-single_line_text_field\">Can I return an item?</li></ul>]"
                .to_string(),
            "mixed: [] []".to_string(),
            "rating text: [3.5]".to_string(),
        ]
    );
}

#[test]
fn metafield_text_escapes_text_as_metafield_tag_does() {
    // The issue's value, and Shopify's output on a store: `metafield_text` escapes the text as
    // `metafield_tag` does, a multi-line text, a list of texts and a metaobject's field too.
    let overlay = serde_json::json!({
        "products": [{"title": "Widget", "handle": "widget", "price": "10.00",
            "metafields": {"custom": {
                "name": {"type": "single_line_text_field", "value": "Tom & \"Jerry\" <b>x</b> l'été"},
                "notes": {"type": "multi_line_text_field", "value": "Line one\nLine <i>two</i>"},
                "tags": {"type": "list.single_line_text_field", "value": ["<a>", "b & c"]},
                "faq": {"type": "metaobject_reference", "value": "faq/one"}
            }}}],
        "metaobjects": {"faq": [{"handle": "one", "fields": {"question": "Is <b>it</b> & that?"}}]}
    });
    let (store, diagnostics) = lsf_core::store::load::load_with_overlay(
        &DataSource::Demo,
        &LoadOptions::default(),
        Some(&overlay),
    );
    assert!(diagnostics.is_empty(), "{diagnostics}");
    let fixture = Fixture {
        store: Arc::new(store),
        ..fixture()
    };
    let render =
        |source: &str| fixture.liquid("/products/widget", Session::initial(&fixture.store), source);
    assert_eq!(
        render(
            "{{ product.metafields.custom.name | metafield_text }}\n\
             {{ product.metafields.custom.name | metafield_tag }}"
        ),
        "Tom &amp; &quot;Jerry&quot; &lt;b&gt;x&lt;/b&gt; l&#39;été\n\
         <span class=\"metafield-single_line_text_field\">Tom &amp; &quot;Jerry&quot; \
         &lt;b&gt;x&lt;/b&gt; l&#39;été</span>"
    );
    assert_eq!(
        render("{{ product.metafields.custom.notes | metafield_text }}"),
        "Line one\nLine &lt;i&gt;two&lt;/i&gt;"
    );
    assert_eq!(
        render("{{ product.metafields.custom.tags | metafield_text }}"),
        "&lt;a&gt; and b &amp; c"
    );
    assert_eq!(
        render("{{ product.metafields.custom.faq | metafield_text: field: 'question' }}"),
        "Is &lt;b&gt;it&lt;/b&gt; &amp; that?"
    );
}

#[test]
fn reference_lists_and_metaobject_values_are_paged_as_on_shopify() {
    // The issue's store: a product whose `custom.related` lists 60 products, a type `faq` of
    // three entries and a type `note` of 60.
    let handles: Vec<String> = (1..=60).map(|i| format!("p{i:02}")).collect();
    let mut products = vec![
        serde_json::json!({"title": "Widget", "handle": "widget", "price": "10.00",
        "metafields": {"custom": {"related": {"type": "list.product_reference", "value": handles}}}}),
    ];
    products.extend(handles.iter().map(|handle| {
        serde_json::json!({"title": handle.to_uppercase(), "handle": handle, "price": "10.00"})
    }));
    let notes: Vec<serde_json::Value> = (1..=60)
        .map(|i| serde_json::json!({"handle": format!("n{i:02}"), "fields": {"text": format!("N{i}")}}))
        .collect();
    let faqs: Vec<serde_json::Value> = ["one", "two", "three"]
        .iter()
        .map(|handle| serde_json::json!({"handle": handle, "fields": {"question": handle}}))
        .collect();
    let overlay = serde_json::json!({
        "products": products,
        "metaobjects": {"faq": faqs, "note": notes}
    });
    let (store, diagnostics) = lsf_core::store::load::load_with_overlay(
        &DataSource::Demo,
        &LoadOptions::default(),
        Some(&overlay),
    );
    assert!(diagnostics.is_empty(), "{diagnostics}");
    let fixture = Fixture {
        store: Arc::new(store),
        ..fixture()
    };
    let render =
        |path: &str, source: &str| fixture.liquid(path, Session::initial(&fixture.store), source);
    // The issue's template. A loop gets the first 50 items, as on Shopify, and `size` and
    // `count` are still all of them.
    assert_eq!(
        render(
            "/products/widget",
            "{%- assign n = 0 -%}{%- for p in product.metafields.custom.related.value -%}\
             {%- assign n = n | plus: 1 -%}{%- endfor -%}loop: {{ n }}|\
             {%- assign m = 0 -%}{%- for e in metaobjects.note.values -%}{%- assign m = m | plus: 1 -%}\
             {%- endfor -%}values loop: {{ m }}|{{ metaobjects.note.values.size }}|\
             {{ product.metafields.custom.related.value.count }}|\
             {{ product.metafields.custom.related.value | size }}"
        ),
        "loop: 50|values loop: 50|60|60|60"
    );
    let related = "{% paginate product.metafields.custom.related.value by 2 %}{{ paginate.items }}|\
                   {{ paginate.pages }}|{{ paginate.current_page }}|\
                   {% for p in product.metafields.custom.related.value %}{{ p.handle }},{% endfor %}|\
                   {{ paginate.next.url }}|{{ paginate.page_param }}{% endpaginate %}";
    let faq = "{% paginate metaobjects.faq.values by 2 %}{{ paginate.items }}|{{ paginate.pages }}|\
               {% for o in metaobjects.faq.values %}{{ o.system.handle }},{% endfor %}|\
               {{ paginate.next.url }}|{{ paginate.page_param }}{% endpaginate %}";
    // Each list has a parameter of its own, `page_` and 8 hexadecimal digits.
    let param = |line: &str| line.rsplit('|').next().unwrap().to_string();
    let first = render("/products/widget", related);
    let related_param = param(&first);
    let faq_first = render("/products/widget", faq);
    let faq_param = param(&faq_first);
    for param in [&related_param, &faq_param] {
        let hash = param.strip_prefix("page_").expect("page_<hash>");
        assert!(
            hash.len() == 8 && hash.chars().all(|c| c.is_ascii_hexdigit()),
            "{param}"
        );
    }
    assert_ne!(related_param, faq_param);
    // `paginate` slices the loop inside it, and its URLs use the list's parameter.
    assert_eq!(
        first,
        format!("60|30|1|p01,p02,|/products/widget?{related_param}=2|{related_param}")
    );
    assert_eq!(
        faq_first,
        format!("3|2|one,two,|/products/widget?{faq_param}=2|{faq_param}")
    );
    // The page of the list's parameter is shown; `page` is not the list's.
    assert_eq!(
        render(
            &format!("/products/widget?{related_param}=2&page=5"),
            related
        ),
        format!("60|30|2|p03,p04,|/products/widget?{related_param}=3&page=5|{related_param}")
    );
    assert_eq!(
        render(&format!("/products/widget?{faq_param}=2"), faq),
        format!("3|2|three,||{faq_param}")
    );
    // Up to 250 items a page.
    assert_eq!(
        render(
            "/products/widget",
            "{% paginate metaobjects.note.values by 250 %}{{ paginate.page_size }}|\
             {% for o in metaobjects.note.values %}{% if forloop.last %}{{ forloop.length }}{% endif %}\
             {% endfor %}{% endpaginate %}"
        ),
        "250|60"
    );
}

#[test]
fn json_gives_metaobjects_and_namespaces_and_refuses_metafields() {
    // The issue's store, with a rich text field `answer`, as the entries of the issue's store
    // had, and a deprecated `string` and `integer`.
    let answer = |text: &str| {
        serde_json::json!({"type": "rich_text_field", "value": {"type": "root", "children": [
            {"type": "paragraph", "children": [{"type": "text", "value": text}]}]}})
    };
    let overlay = serde_json::json!({
        "products": [{"title": "Widget", "handle": "widget", "price": "10.00",
            "metafields": {"custom": {
                "faq": {"type": "metaobject_reference", "value": "faq/two"},
                "faqs": {"type": "list.metaobject_reference", "value": ["faq/one", "faq/two"]},
                "decs": {"type": "list.number_decimal", "value": ["1.5", "2.25"]}
            }, "legacy": {
                "text": {"type": "string", "value": "legacy"},
                "count": {"type": "integer", "value": 5}
            }}}],
        "metaobjects": {"faq": [
            {"handle": "one", "fields": {"question": "How long does shipping take?",
                "order": {"type": "number_integer", "value": 1}, "answer": answer("Care & use")}},
            {"handle": "two", "fields": {"question": "Can I return an item?",
                "order": {"type": "number_integer", "value": 2}, "answer": answer("Yes")}}
        ]}
    });
    let (store, _) = lsf_core::store::load::load_with_overlay(
        &DataSource::Demo,
        &LoadOptions::default(),
        Some(&overlay),
    );
    let fixture = Fixture {
        store: Arc::new(store),
        ..fixture()
    };
    let render =
        |source: &str| fixture.liquid("/products/widget", Session::initial(&fixture.store), source);
    let two = r#"{"answer":{"type":"root","children":[{"type":"paragraph","children":[{"type":"text","value":"Yes"}]}]},"order":2,"question":"Can I return an item?"}"#;
    let one = r#"{"answer":{"type":"root","children":[{"type":"paragraph","children":[{"type":"text","value":"Care \u0026amp; use"}]}]},"order":1,"question":"How long does shipping take?"}"#;
    let refused = r#"{"error":"json not allowed for this object"}"#;
    // The issue's template, line by line. The text of rich text is HTML-escaped, and `json`
    // writes `&` as `\u0026`, as it does every `&`.
    assert_eq!(render("{{ metaobjects.faq.two | json }}"), two);
    assert_eq!(
        render("{{ product.metafields.custom.faq.value | json }}"),
        two
    );
    assert_eq!(
        render("{{ product.metafields.custom.faqs.value | json }}"),
        format!("[{one},{two}]")
    );
    assert_eq!(
        render("{{ product.metafields.custom | json }}"),
        format!(r#"{{"decs":["1.5","2.25"],"faq":{two},"faqs":[{one},{two}]}}"#)
    );
    assert_eq!(
        render("{{ product.metafields.custom.decs.value | json }}"),
        r#"["1.5","2.25"]"#
    );
    assert_eq!(
        render("{{ product.metafields.custom.decs | json }}"),
        refused
    );
    // Every metafield of a current type is refused; the deprecated types give their value.
    assert_eq!(
        render(
            "{{ product.metafields.custom.faq | json }}|{{ metaobjects.faq.one.question | json }}|\
             {{ product.metafields.legacy.text | json }}|{{ product.metafields.legacy.count | json }}"
        ),
        format!(r#"{refused}|{refused}|"legacy"|5"#)
    );
    // So are `metaobject.system` and `metaobjects.<type>`, which still give their properties.
    assert_eq!(
        render(
            "{{ metaobjects.faq.one.system | json }}|{{ metaobjects.faq | json }}|\
             {{ metaobjects.faq.one.system.handle }}"
        ),
        format!("{refused}|{refused}|one")
    );
    // A decimal still computes and prints as a number, and rich text reads as before.
    assert_eq!(
        render(
            "{% assign d = product.metafields.custom.decs.value %}{{ d | first }}|\
             {{ d | first | plus: 1 }}|{% if d.last > 2 %}more{% endif %}|{{ d | join: ',' }}|\
             {{ metaobjects.faq.one.answer.value.children.first.children.first.value }}"
        ),
        "1.5|2.5|more|1.5,2.25|Care & use"
    );
}

#[test]
fn what_is_loaded_is_charged_once() {
    let overlay = serde_json::json!({
        "metaobjects": {
            "designer": [
                { "handle": "sam", "fields": { "name": "Sam" } },
                { "handle": "kim", "fields": { "name": "Kim" } }
            ]
        }
    });
    let options = LoadOptions {
        theme_locales: vec!["en".to_string(), "fr".to_string()],
        ..LoadOptions::default()
    };
    let (store, diagnostics) =
        lsf_core::store::load::load_with_overlay(&DataSource::Demo, &options, Some(&overlay));
    assert!(diagnostics.is_empty(), "{diagnostics}");
    let fixture = Fixture {
        store: Arc::new(store),
        ..fixture()
    };
    // What a piece of Liquid is charged for, on a page.
    let charges = |path: &str, source: &str| -> Vec<(String, u64)> {
        let profiler = lsf_liquid::Profiler::with_costs(false, Costs::default().pairs());
        {
            let _active = profiler.activate();
            fixture.liquid(path, Session::initial(&fixture.store), source);
        }
        charged(&profiler.finish())
            .into_iter()
            .map(|(kind, count)| (kind.to_string(), count))
            .collect()
    };
    let of = |pairs: &[(&str, u64)]| -> Vec<(String, u64)> {
        pairs
            .iter()
            .map(|(kind, count)| (kind.to_string(), *count))
            .collect()
    };
    let tee = "/products/organic-cotton-t-shirt";

    // A metafield is read once, however many times the template prints it. One that is not
    // there is looked for all the same, in a namespace that exists or not.
    assert_eq!(
        charges(
            tee,
            "{{ product.metafields.custom.care }}{{ product.metafields.custom.care }}\
             {{ product.metafields.reviews.rating_count }}\
             {{ product.metafields.custom.nothing }}{{ product.metafields.nowhere.nothing }}\
             {{ product.metafields.nowhere.nothing }}"
        ),
        of(&[("product", 1), ("metafield", 4)])
    );
    // The same metafield of another product is another one.
    assert_eq!(
        charges(
            "/collections/all",
            "{% for product in collection.products %}{{ product.metafields.custom.care }}{% endfor %}"
        ),
        of(&[("product", 8), ("collection", 1), ("metafield", 8)])
    );
    // A product named by its handle, twice.
    assert_eq!(
        charges(
            "/",
            "{{ all_products['ceramic-mug'].title }}{{ all_products['ceramic-mug'].price }}"
        ),
        of(&[("product", 1)])
    );
    // Metaobjects: each one once, and their fields are not metafields to load.
    assert_eq!(
        charges(
            "/",
            "{{ metaobjects.designer.sam.name }}\
             {% for designer in metaobjects.designer.values %}{{ designer.name }}{% endfor %}"
        ),
        of(&[("metaobject", 2)])
    );
    assert_eq!(
        charges(
            "/",
            "{{ linklists.main-menu.title }}{{ linklists['main-menu'].links.size }}"
        ),
        of(&[("menu", 1)])
    );
    // A search is charged, and so is each thing it finds.
    let search = charges("/search?q=mug", "{{ search.results_count }}");
    assert_eq!(search[search.len() - 1], ("search".to_string(), 1));
    assert!(search.contains(&("product".to_string(), 1)), "{search:?}");
    // What a template does not ask for costs nothing.
    assert_eq!(charges("/search?q=mug", "{{ shop.name }}"), of(&[]));
}

/// A fixture of a theme written in a temporary directory, `files` being `(path, content)`,
/// rendered with the given feature previews.
fn theme_of(name: &str, previews: &[lsf_theme::Preview], files: &[(&str, &str)]) -> Fixture {
    let directory = std::env::temp_dir().join(format!("lsf-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&directory);
    for (path, content) in files {
        let path = directory.join(path);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, content).unwrap();
    }
    let env = Arc::new(lsf_theme::environment_with(previews));
    let theme = Arc::new(Theme::open(&directory, env, Revalidate::Never).expect("theme"));
    Fixture {
        renderer: Renderer::new(theme),
        ..fixture()
    }
}

#[test]
fn a_file_that_is_not_utf8_renders_as_shopify_stores_it() {
    // #176: Shopify takes a section, a snippet or a JSON template saved in Latin-1, and each
    // byte that is not UTF-8 shows as U+FFFD. lsf treated such a file as missing.
    let fixture = theme_of(
        "latin1",
        &[],
        &[
            (
                "layout/theme.liquid",
                "<html><head>{{ content_for_header }}</head><body>{{ content_for_layout }}</body></html>",
            ),
            (
                "sections/uses-snippet.liquid",
                "<p>{% render 'latin' %}</p>{% schema %}{ \"name\": \"Uses snippet\", \"settings\": [] }{% endschema %}",
            ),
        ],
    );
    let directory = std::env::temp_dir().join(format!("lsf-latin1-{}", std::process::id()));
    let write = |path: &str, content: &[u8]| std::fs::write(directory.join(path), content).unwrap();
    std::fs::create_dir_all(directory.join("snippets")).unwrap();
    std::fs::create_dir_all(directory.join("templates")).unwrap();
    write(
        "sections/latin.liquid",
        b"<p>section: caf\xe9</p>{% schema %}{ \"name\": \"Latin\", \"settings\": [] }{% endschema %}",
    );
    write("snippets/latin.liquid", b"<i>snippet: caf\xe9</i>");
    write(
        "templates/index.json",
        b"{ \"sections\": { \"a\": { \"type\": \"latin\" }, \"b\": { \"type\": \"uses-snippet\" } }, \"order\": [\"a\", \"b\"], \"wrapper\": \"div.caf\xe9\" }",
    );
    let page = fixture.page("/");
    assert_clean(&page);
    assert!(
        page.body.contains("<p>section: caf\u{FFFD}</p>"),
        "{}",
        page.body
    );
    assert!(
        page.body.contains("<p><i>snippet: caf\u{FFFD}</i></p>"),
        "{}",
        page.body
    );
}

#[test]
fn a_template_composes_its_page_from_blocks() {
    let fixture = theme_of(
        "block-tag",
        &[lsf_theme::Preview::LiquidJuly26],
        &[
            (
                "layout/theme.liquid",
                "<html><head>{{ content_for_header }}</head><body>\
                 {% block 'box', tag: 'header' %}<b>{{ shop.name }}</b>{% endblock %}\
                 {{ content_for_layout }}</body></html>",
            ),
            (
                "templates/index.liquid",
                "{% assign greeting = 'hi' %}\
                 {% block 'box', alignment: 'center', items: ['a', 'b', shop.name] %}\
                 {{ greeting }}{% block 'box' %}inner{% endblock %}\
                 {% endblock %}",
            ),
            (
                "blocks/box.liquid",
                "{% assign tag = tag | default: 'section' %}\
                 <{{ tag }} data-align=\"{{ block.settings.alignment }}\" data-also=\"{{ alignment }}\" \
                 data-items=\"{{ items | join: '+' }}\" data-greeting=\"{{ greeting }}\">{{ content }}</{{ tag }}>\
                 {% if block.content != content %}block.content differs{% endif %}\
                 {% schema %}{\"name\": \"Box\", \"settings\": [{\"type\": \"select\", \"id\": \"alignment\", \
                 \"label\": \"Alignment\", \"options\": [{\"value\": \"left\", \"label\": \"Left\"}, \
                 {\"value\": \"center\", \"label\": \"Center\"}], \"default\": \"left\"}]}{% endschema %}",
            ),
        ],
    );
    let rendered = fixture.page("/");
    assert_clean(&rendered);
    // The page's body, before what the platform adds at its end.
    let body = rendered.body.split("<body>").nth(1).unwrap();
    let body = body.split("<script src=").next().unwrap();
    // No wrapper: the block's own element is the outermost. A parameter named after a setting
    // sets it, the others have their default; an array can be written in place; the body is
    // rendered where it is written, and the block does not see the caller's variables.
    assert_eq!(
        body,
        "<header data-align=\"left\" data-also=\"\" data-items=\"\" data-greeting=\"\"><b>Local Supply Co.</b></header>\
         <section data-align=\"center\" data-also=\"center\" data-items=\"a+b+Local Supply Co.\" data-greeting=\"\">\
         hi<section data-align=\"left\" data-also=\"\" data-items=\"\" data-greeting=\"\">inner</section>\
         </section>"
    );
    // Without a partial, the page gets no import map.
    assert!(!rendered.body.contains("importmap"));
}

#[test]
fn a_partial_marks_a_region_and_brings_its_runtime() {
    let fixture = theme_of(
        "partial-tag",
        &[lsf_theme::Preview::LiquidJuly26],
        &[
            (
                "layout/theme.liquid",
                "<html><head>{{ content_for_header }}</head><body>{{ content_for_layout }}</body></html>",
            ),
            (
                "templates/index.liquid",
                "{% partial 'count' %}<p>{{ shop.name }}</p>{% endpartial %}",
            ),
        ],
    );
    let rendered = fixture.page("/");
    assert_clean(&rendered);
    assert!(
        rendered.body.contains(
            "<body><!--shopify-partial name=\"count\"--><p>Local Supply Co.</p><!--/shopify-partial-->"
        ),
        "{}",
        rendered.body
    );
    // The import map comes first, so that every module of the page can use it.
    let head = rendered.body.split("<head>").nth(1).unwrap();
    assert!(
        head.starts_with("<script type=\"importmap\">{\"imports\":{\"@shopify/partial-rendering\":\"//shop.test/cdn/shopifycloud/storefront/assets/partial-rendering-"),
        "{head}"
    );
    let path = lsf_theme::render::platform::partial_rendering_path();
    let script = lsf_theme::render::platform::asset(path).expect("the runtime is served");
    assert!(script.content.contains("export const partials"));
}

#[test]
fn without_their_preview_block_and_partial_fail_as_on_a_live_store() {
    let files = [
        (
            "layout/theme.liquid",
            "<html><head>{{ content_for_header }}</head><body>{{ content_for_layout }}</body></html>",
        ),
        (
            "templates/index.liquid",
            "{% partial 'count' %}{% block 'box' %}{% endblock %}{% endpartial %}",
        ),
        ("blocks/box.liquid", "<div>{{ content }}</div>"),
    ];
    let rendered = theme_of("no-preview", &[], &files).page("/");
    assert!(
        rendered
            .body
            .contains("Liquid syntax error (templates/index line 1): Unknown tag 'partial'"),
        "{}",
        rendered.body
    );
    assert!(!rendered.body.contains("importmap"));
}

#[test]
fn a_video_setting_is_the_video_of_its_file() {
    let fixture = theme_of(
        "video-setting",
        &[],
        &[
            (
                "layout/theme.liquid",
                "<html><body>{{ content_for_layout }}</body></html>",
            ),
            (
                "templates/index.json",
                r#"{"sections": {"hero": {"type": "hero", "settings": {"clip": "shopify://files/videos/Clip.MOV"}}}, "order": ["hero"]}"#,
            ),
            (
                "sections/hero.liquid",
                "{% assign clip = section.settings.clip %}\
                 {{ clip.media_type }}|{{ clip.sources.first.mime_type }}|{{ clip.sources.first.format }}|\
                 {{ clip.preview_image.src }}|{{ clip.aspect_ratio }}\
                 {% schema %}{\"name\": \"Hero\", \"settings\": [{\"type\": \"video\", \"id\": \"clip\", \"label\": \"Clip\"}]}{% endschema %}",
            ),
        ],
    );
    let rendered = fixture.page("/");
    assert_clean(&rendered);
    // A `.mov` is announced as the MP4 Shopify makes of it; its poster is the `.jpg` of the
    // same name, whatever the case of the extension.
    assert!(
        rendered
            .body
            .contains("video|video/mp4|mp4|files/videos/Clip.jpg|1.7777777777777777"),
        "{}",
        rendered.body
    );
}

#[test]
fn a_called_block_takes_its_parameters_when_the_tag_opens() {
    let liquid_july = [lsf_theme::Preview::LiquidJuly26];
    let layout = (
        "layout/theme.liquid",
        "<html><body>{{ content_for_layout }}</body></html>",
    );
    // A setting called `content` keeps its default: only parameters set settings, and the
    // body is not one. The parameters are read before the body runs.
    let text = (
        "blocks/text.liquid",
        "[{{ block.settings.content }}|{{ n }}|{{ items | join: ',' }}|{{ content }}|{% content_for 'blocks' %}]\
         {% schema %}{\"name\": \"Text\", \"settings\": [{\"type\": \"text\", \"id\": \"content\", \"label\": \"Content\", \"default\": \"Hello\"}]}{% endschema %}",
    );
    let fixture = theme_of(
        "called-block",
        &liquid_july,
        &[
            layout,
            text,
            (
                "templates/index.liquid",
                "{% assign n = 1 %}{% block 'text', n: n, items: [['a', 'b'], 'c'] %}{% assign n = 2 %}{{ n }}{% endblock %}",
            ),
        ],
    );
    let rendered = fixture.page("/");
    assert_clean(&rendered);
    assert!(
        rendered.body.contains("<body>[Hello|1|a,b,c|2|]"),
        "{}",
        rendered.body
    );

    // A block nested in the body of another is part of it in a profile.
    let nested = theme_of(
        "called-block-profile",
        &liquid_july,
        &[
            layout,
            text,
            (
                "templates/index.liquid",
                "{% block 'text' %}{% block 'text' %}{% endblock %}{% endblock %}",
            ),
        ],
    );
    let site = nested.renderer.site(
        nested.store.clone(),
        request("/"),
        Session::initial(&nested.store),
    );
    let (_, profile) = nested.renderer.profile_page(
        &site,
        lsf_theme::render::routes::resolve(&site),
        &Target::Page,
        &ProfileOptions::default(),
    );
    assert_eq!(
        profile_shape(&profile),
        "render(templates/index(block text(block text(blocks/text) blocks/text)) layout/theme)"
    );
}

#[test]
fn a_called_block_is_refused_where_shopify_refuses_it() {
    let liquid_july = [lsf_theme::Preview::LiquidJuly26];
    let fixture = theme_of(
        "called-block-errors",
        &liquid_july,
        &[
            (
                "layout/theme.liquid",
                "<html><body>{{ content_for_layout }}</body></html>",
            ),
            (
                "templates/index.liquid",
                "{% block 'outer' %}{% endblock %}|{% block 'missing' %}{% endblock %}",
            ),
            ("blocks/outer.liquid", "{% block 'inner' %}{% endblock %}"),
            ("blocks/inner.liquid", "inner"),
        ],
    );
    let rendered = fixture.page("/");
    // Only layouts and templates may call a block; and the block has to exist.
    assert!(
        rendered.body.contains(
            "Liquid error (blocks/outer line 1): the block tag can only be used in layout/ and templates/ files"
        ),
        "{}",
        rendered.body
    );
    assert!(
        rendered.body.contains(
            "Liquid error (templates/index line 1): Could not find asset blocks/missing.liquid"
        ),
        "{}",
        rendered.body
    );
    // An array written in place is a value, not the start of a lookup.
    let env = Arc::new(lsf_theme::environment_with(&liquid_july));
    let Err(error) =
        lsf_liquid::Template::parse(&env, "{% block 'x', a: ['b'].first %}{% endblock %}")
    else {
        panic!("an array followed by a lookup parses");
    };
    assert!(
        error.to_string().contains("cannot be followed by"),
        "{error}"
    );
}

#[test]
fn an_article_handle_that_two_blogs_have_names_neither() {
    let overlay = serde_json::json!({
        "blogs": [{
            "title": "News",
            "articles": [{
                "title": "How to care for linen",
                "author": "Sam",
                "published_at": "2024-01-10T09:00:00Z",
                "content": "<p>Again.</p>"
            }]
        }],
        "metaobjects": {
            "note": [{
                "handle": "n",
                "fields": {
                    "loose": { "type": "article_reference", "value": "how-to-care-for-linen" },
                    "exact": { "type": "article_reference", "value": "news/how-to-care-for-linen" }
                }
            }]
        }
    });
    let (store, diagnostics) = lsf_core::store::load::load_with_overlay(
        &DataSource::Demo,
        &LoadOptions::default(),
        Some(&overlay),
    );
    // `validate` says so: the loose handle names no article.
    assert_eq!(diagnostics.error_count(), 1, "{diagnostics}");
    let error = &diagnostics.items[0];
    assert_eq!(error.path, "/metaobjects/note/0/fields/loose/value");
    assert!(
        error.message.starts_with("several blogs have an article"),
        "{diagnostics}"
    );
    let fixture = Fixture {
        store: Arc::new(store),
        ..fixture()
    };
    let output = fixture.liquid(
        "/",
        Session::initial(&fixture.store),
        "{% assign n = metaobjects.note.n %}[{{ n.loose.value.title }}]{{ n.exact.value.url }}",
    );
    assert_eq!(output, "[]/blogs/news/how-to-care-for-linen");
}

#[test]
fn a_block_cannot_be_called_from_the_liquid_of_a_setting() {
    let fixture = theme_of(
        "called-block-setting",
        &[lsf_theme::Preview::LiquidJuly26],
        &[
            (
                "layout/theme.liquid",
                "<html><body>{{ content_for_layout }}</body></html>",
            ),
            (
                "templates/index.json",
                r#"{"sections": {"c": {"type": "custom", "settings": {"code": "{% block 'b' %}{% endblock %}"}}}, "order": ["c"]}"#,
            ),
            (
                "sections/custom.liquid",
                "{{ section.settings.code }}{% schema %}{\"name\": \"Custom\", \"settings\": [{\"type\": \"liquid\", \"id\": \"code\", \"label\": \"Code\"}]}{% endschema %}",
            ),
            ("blocks/b.liquid", "<b>called</b>"),
        ],
    );
    let rendered = fixture.page("/");
    assert!(
        !rendered.body.contains("<b>called</b>"),
        "{}",
        rendered.body
    );
    assert!(
        rendered
            .body
            .contains("the block tag can only be used in layout/ and templates/ files"),
        "{}",
        rendered.body
    );
}

/// A theme in a temporary directory rendered against the demo store plus `overlay` (store data).
fn theme_with_data(name: &str, overlay: &serde_json::Value, files: &[(&str, &str)]) -> Fixture {
    let fixture = theme_of(name, &[], files);
    let (store, diagnostics) = lsf_core::store::load::load_with_overlay(
        &DataSource::Demo,
        &LoadOptions::default(),
        Some(overlay),
    );
    assert!(diagnostics.is_empty(), "{diagnostics}");
    Fixture {
        store: Arc::new(store),
        ..fixture
    }
}

#[test]
fn metaobject_settings_are_metaobjects() {
    // https://shopify.dev/docs/storefronts/themes/architecture/settings/input-settings#metaobject
    // and #metaobject_list: the setting gives a `metaobject` (or an array of them), and
    // `metaobject_type` says which type the picker offers. Themes store the handle alone.
    let overlay = serde_json::json!({
        "metaobjects": {
            "recipe": [
                { "handle": "pancakes", "fields": { "name": { "type": "single_line_text_field", "value": "Pancakes" } } },
                { "handle": "waffles", "fields": { "name": { "type": "single_line_text_field", "value": "Waffles" } } }
            ]
        }
    });
    let gid = format!(
        "gid://shopify/Metaobject/{}",
        lsf_core::util::stable_id("metaobject", "recipe/waffles")
    );
    let template = serde_json::json!({
        "sections": { "main": { "type": "recipes", "blocks": {
            "handle": { "type": "card", "settings": { "recipe": "pancakes" } },
            "dotted": { "type": "card", "settings": { "recipe": "recipe.waffles" } },
            "slashed": { "type": "card", "settings": { "recipe": "recipe/pancakes" } },
            "gid": { "type": "card", "settings": { "recipe": gid } },
            "unknown": { "type": "card", "settings": { "recipe": "nope" } },
            "list": { "type": "card", "settings": { "recipes": ["waffles", "nope", gid, "pancakes"] } },
            "dynamic": { "type": "card", "settings": {
                "recipe": "{{ metaobjects.recipe.waffles }}",
                "recipes": "{{ metaobjects.recipe.values }}"
            } }
        }, "block_order": ["handle", "dotted", "slashed", "gid", "unknown", "list", "dynamic"] } },
        "order": ["main"]
    })
    .to_string();
    let fixture = theme_with_data(
        "metaobject-settings",
        &overlay,
        &[
            (
                "layout/theme.liquid",
                "<html><head>{{ content_for_header }}</head><body>{{ content_for_layout }}</body></html>",
            ),
            ("templates/index.json", &template),
            ("sections/recipes.liquid", "{% content_for 'blocks' %}\n"),
            (
                "blocks/card.liquid",
                "[{{ block.settings.recipe.name.value }}|{{ block.settings.recipe.system.handle }}|\
                 {% for p in block.settings.recipes %}{{ p.name.value }},{% endfor %}]\
                 {% schema %}{\"name\": \"Card\", \"tag\": null, \"settings\": [\
                 {\"type\": \"metaobject\", \"id\": \"recipe\", \"label\": \"Recipe\", \"metaobject_type\": \"recipe\"},\
                 {\"type\": \"metaobject_list\", \"id\": \"recipes\", \"label\": \"Recipes\", \"metaobject_type\": \"recipe\"}\
                 ]}{% endschema %}",
            ),
        ],
    );
    let rendered = fixture.page("/");
    assert_clean(&rendered);
    let body = rendered.body.split("<body>").nth(1).unwrap();
    let cards: Vec<&str> = body
        .split('[')
        .skip(1)
        .map(|card| card.split(']').next().unwrap())
        .collect();
    assert_eq!(
        cards,
        [
            // A handle, `type.handle`, `type/handle` and the id of the entry; an unknown one is blank.
            "Pancakes|pancakes|",
            "Waffles|waffles|",
            "Pancakes|pancakes|",
            "Waffles|waffles|",
            "||",
            // The list keeps its order and leaves out what it does not find.
            "||Waffles,Waffles,Pancakes,",
            // A dynamic source gives the entries themselves, which are kept as they are.
            "Waffles|waffles|Pancakes,Waffles,",
        ]
    );
}

#[test]
fn the_entries_of_a_metaobject_type_have_pages() {
    // https://shopify.dev/docs/storefronts/themes/architecture/templates/metaobject: a type
    // whose definition has the online store capability has `templates/metaobject/<type>.json`,
    // rendered with the entry as `metaobject` at `/pages/<type>/<handle>`. Without the
    // capability (here: without that template) the entry has no page and no `system.url`.
    let overlay = serde_json::json!({
        "metaobjects": {
            "event": [
                { "handle": "summer", "fields": { "title": { "type": "single_line_text_field", "value": "Summer fair" } } },
                { "handle": "winter", "template_suffix": "alt", "fields": { "title": { "type": "single_line_text_field", "value": "Winter fair" } } }
            ],
            "note": [{ "handle": "n", "fields": {} }]
        }
    });
    let section = "<h1>{{ metaobject.title.value }}</h1><i>{{ metaobject.system.handle }}</i>\
                   <u>{{ metaobject.system.url }}</u><s>{{ request.page_type }}|{{ template.directory }}|{{ template.name }}|{{ template.suffix }}</s>\
                   <a href=\"{{ section.settings.link }}\">{{ page_title }}</a>\n\
                   {% schema %}{\"name\": \"Event\", \"settings\": [{\"type\": \"url\", \"id\": \"link\"}]}{% endschema %}";
    let template = serde_json::json!({
        "sections": { "main": { "type": "event", "settings": { "link": "shopify://metaobjects/event/winter" } } },
        "order": ["main"]
    })
    .to_string();
    let fixture = theme_with_data(
        "metaobject-pages",
        &overlay,
        &[
            (
                "layout/theme.liquid",
                "<html><head>{{ content_for_header }}</head><body>{{ content_for_layout }}</body></html>",
            ),
            ("sections/event.liquid", section),
            ("templates/metaobject/event.json", &template),
            (
                "templates/metaobject/event.alt.json",
                &template.replace("winter", "summer"),
            ),
        ],
    );
    let body = |path: &str| {
        let rendered = fixture.page(path);
        (rendered.status, rendered.template.clone(), rendered.body)
    };
    let (status, template, html) = body("/pages/event/summer");
    assert_eq!(status, 200, "{html}");
    assert_eq!(template, "metaobject/event");
    assert!(
        html.contains(
            "<h1>Summer fair</h1><i>summer</i><u>/pages/event/summer</u>\
             <s>metaobject|metaobject|event|</s>\
             <a href=\"/pages/event/winter\">Summer fair</a>"
        ),
        "{html}"
    );
    // The entry's template suffix, and `?view=`.
    let (status, template, html) = body("/pages/event/winter");
    assert_eq!((status, template.as_str()), (200, "metaobject/event.alt"));
    assert!(
        html.contains("<s>metaobject|metaobject|event|alt</s>"),
        "{html}"
    );
    assert!(html.contains("<a href=\"/pages/event/summer\">"), "{html}");
    assert_eq!(
        body("/pages/event/summer?view=alt").1,
        "metaobject/event.alt"
    );
    // An unknown entry, and a type without a template, are not found.
    assert_eq!(body("/pages/event/nope").0, 404);
    assert_eq!(body("/pages/note/n").0, 404);
    assert_eq!(body("/pages/nope/summer").0, 404);
    // `system.url` is blank for the type without pages, and follows the locale.
    let output = fixture.liquid(
        "/",
        Session::initial(&fixture.store),
        "[{{ metaobjects.note.n.system.url }}][{{ metaobjects.event.summer.system.url }}]",
    );
    assert_eq!(output, "[][/pages/event/summer]");
    let mut french = request("/pages/event/summer");
    french.root = "/fr".to_string();
    let rendered = fixture.render_with(french, Session::initial(&fixture.store), &Target::Page);
    assert_eq!(rendered.status, 200);
    assert!(
        rendered.body.contains("<u>/fr/pages/event/summer</u>"),
        "{}",
        rendered.body
    );
}

#[test]
fn the_pages_of_a_metaobject_type_are_at_the_url_handle_of_its_definition() {
    // https://shopify.dev/docs/apps/build/metaobjects/use-metaobject-capabilities: "The
    // urlHandle is set to content resulting in the following URL for metaobject entries:
    // /pages/content/{entry-handle}". The type's own name is no URL then.
    let overlay = serde_json::json!({
        "metaobject_definitions": { "info": { "url_handle": "infos" } },
        "metaobjects": { "info": [{ "handle": "about", "fields": {} }] }
    });
    let fixture = theme_with_data(
        "metaobject-url-handle",
        &overlay,
        &[
            ("layout/theme.liquid", "{{ content_for_layout }}"),
            (
                "templates/metaobject/info.liquid",
                "{{ metaobject.system.url }}|{{ request.path }}|{{ canonical_url }}",
            ),
        ],
    );
    let page = fixture.page("/pages/infos/about");
    assert_eq!(
        (page.status, page.template.as_str()),
        (200, "metaobject/info")
    );
    assert_eq!(
        page.body,
        "/pages/infos/about|/pages/infos/about|http://shop.test/pages/infos/about"
    );
    assert_eq!(fixture.page("/pages/info/about").status, 404);
}

#[test]
fn a_url_setting_entered_as_text_keeps_its_path_on_a_localized_route() {
    // https://shopify.dev/docs/api/liquid/objects/url: "URLs that point to store resources, the
    // home page, and the collection pages include the path prefix of the current locale",
    // "relative paths and anchors are returned exactly as they were entered". A path entered as
    // text is a `relative_link` "even if the path matches a resource", except `/`, a
    // `home_link`. The `default` can only be `/collections` or `/collections/all`, the
    // collection pages (input-settings#url).
    let overlay = serde_json::json!({"localization": {
        "countries": [{"iso_code": "US"}, {"iso_code": "BE"}],
        "languages": [{"iso_code": "en"}, {"iso_code": "fr"}],
        "markets": [{"handle": "us", "countries": ["US"]},
            {"handle": "europe", "countries": ["BE"], "subfolder_suffix": "be"}]
    }});
    let schema = serde_json::json!([
        {"name": "theme_info"},
        {"name": "Links", "settings": [
            {"type": "url", "id": "typed_link", "label": "Typed"},
            {"type": "url", "id": "picked_link", "label": "Picked"},
            {"type": "url", "id": "external_link", "label": "External"},
            {"type": "url", "id": "anchor_link", "label": "Anchor"},
            {"type": "url", "id": "home_link", "label": "Home"},
            {"type": "url", "id": "catalog_link", "label": "Catalog", "default": "/collections/all"},
            {"type": "url", "id": "collections_link", "label": "Collections", "default": "/collections"}
        ]}
    ])
    .to_string();
    let data = serde_json::json!({"current": {
        "typed_link": "/pages/contact",
        "picked_link": "shopify://collections/all",
        "external_link": "https://example.com/x",
        "anchor_link": "#faq",
        "home_link": "/"
    }})
    .to_string();
    let section = "<i>{{ section.settings.link }}|{{ section.settings.dynamic }}|\
                   {% for block in section.blocks %}{{ block.settings.link }}{% endfor %}</i>\n\
                   {% schema %}{\"name\": \"Links\", \"settings\": [{\"type\": \"url\", \"id\": \"link\"}, \
                   {\"type\": \"url\", \"id\": \"dynamic\"}], \"blocks\": [{\"type\": \"link\", \"name\": \"Link\", \
                   \"settings\": [{\"type\": \"url\", \"id\": \"link\"}]}]}{% endschema %}";
    let template = serde_json::json!({
        "sections": { "links": {
            "type": "links",
            "settings": { "link": "/collections/all", "dynamic": "{{ request.path }}" },
            "blocks": { "b": { "type": "link", "settings": { "link": "shopify://pages/contact" } } },
            "block_order": ["b"]
        } },
        "order": ["links"]
    })
    .to_string();
    let fixture = theme_with_data(
        "url-settings-typed",
        &overlay,
        &[
            ("config/settings_schema.json", &schema),
            ("config/settings_data.json", &data),
            (
                "layout/theme.liquid",
                "<html><head>{{ content_for_header }}</head><body>{{ content_for_layout }}</body></html>",
            ),
            ("sections/links.liquid", section),
            ("templates/index.json", &template),
        ],
    );
    let source = "{{ settings.typed_link }}|{{ settings.picked_link }}|{{ settings.external_link }}|\
                  {{ settings.anchor_link }}|{{ settings.home_link }}|{{ settings.catalog_link }}|\
                  {{ settings.collections_link }}";
    let session = Session::initial(&fixture.store);
    let render = |root: &str, market: Option<&str>| {
        let mut under = request("/");
        under.root = root.to_string();
        under.market = market.map(str::to_string);
        if root == "/fr" {
            under.locale = "fr".to_string();
        }
        let settings = fixture.liquid_for(under.clone(), session.clone(), source);
        let page = fixture.render_with(under, session.clone(), &Target::Page);
        assert_eq!(page.status, 200, "{}", page.body);
        let section = page
            .body
            .split_once("<i>")
            .and_then(|(_, rest)| rest.split_once("</i>"))
            .map(|(inside, _)| inside.to_string())
            .unwrap_or_else(|| page.body.clone());
        (settings, section)
    };
    // A dynamic source is a `relative_link` too: `request.path` already has the prefix.
    // The issue's repro: `/pages/contact|/fr/collections/all` on `/fr/`.
    assert_eq!(
        render("/fr", None),
        (
            "/pages/contact|/fr/collections/all|https://example.com/x|#faq|/fr|\
             /fr/collections/all|/fr/collections"
                .to_string(),
            "/collections/all|/fr|/fr/pages/contact".to_string()
        )
    );
    // The same on a market subfolder.
    assert_eq!(
        render("/en-be", Some("europe")),
        (
            "/pages/contact|/en-be/collections/all|https://example.com/x|#faq|/en-be|\
             /en-be/collections/all|/en-be/collections"
                .to_string(),
            "/collections/all|/en-be|/en-be/pages/contact".to_string()
        )
    );
    // Without a prefix, every value is its path.
    assert_eq!(
        render("", None),
        (
            "/pages/contact|/collections/all|https://example.com/x|#faq|/|/collections/all|\
             /collections"
                .to_string(),
            "/collections/all|/|/pages/contact".to_string()
        )
    );
}

#[test]
fn a_metaobject_shows_its_translation_on_a_localized_route() {
    // https://shopify.dev/docs/apps/build/metaobjects/use-metaobject-capabilities: the fields of
    // a translatable type are translated per locale; a field without a translation shows its
    // value in the primary language.
    let rich_text = |text: &str| {
        serde_json::json!({"type": "root", "children": [
            {"type": "paragraph", "children": [{"type": "text", "value": text}]},
        ]})
    };
    let overlay = serde_json::json!({
        "metaobjects": {
            "faq": [
                {
                    "handle": "shipping",
                    "fields": {
                        "title": "Shipping",
                        "answer": { "type": "rich_text_field", "value": rich_text("Two days.") },
                        "position": 1
                    },
                    "translations": {
                        "fr": { "title": "Livraison", "answer": rich_text("Deux jours.") }
                    }
                },
                { "handle": "returns", "fields": { "title": "Returns" } }
            ]
        }
    });
    let section = "<h1>{{ metaobject.title.value }}</h1>{{ metaobject.answer | metafield_tag }}\
                   <i>{{ metaobject.position.value }}</i><title>{{ page_title }}</title>";
    let template =
        serde_json::json!({"sections": {"main": {"type": "faq"}}, "order": ["main"]}).to_string();
    let fixture = theme_with_data(
        "metaobject-translations",
        &overlay,
        &[
            (
                "layout/theme.liquid",
                "<html><head>{{ content_for_header }}</head><body>{{ content_for_layout }}</body></html>",
            ),
            ("sections/faq.liquid", section),
            ("templates/metaobject/faq.json", &template),
        ],
    );
    let french = |path: &str| {
        let mut request = request(path);
        request.locale = "fr".to_string();
        request.root = "/fr".to_string();
        request
    };

    let english = fixture.page("/pages/faq/shipping");
    assert_clean(&english);
    assert!(
        english.body.contains("<h1>Shipping</h1>"),
        "{}",
        english.body
    );
    assert!(english.body.contains("Two days."), "{}", english.body);
    assert!(
        english.body.contains("<title>Shipping</title>"),
        "{}",
        english.body
    );

    let rendered = fixture.render_with(
        french("/pages/faq/shipping"),
        Session::initial(&fixture.store),
        &Target::Page,
    );
    assert_clean(&rendered);
    assert_eq!(rendered.status, 200);
    assert!(
        rendered.body.contains("<h1>Livraison</h1>"),
        "{}",
        rendered.body
    );
    assert!(rendered.body.contains("Deux jours."), "{}", rendered.body);
    assert!(!rendered.body.contains("Two days."), "{}", rendered.body);
    // A field without a translation, and one that cannot have any, show their own value.
    assert!(rendered.body.contains("<i>1</i>"), "{}", rendered.body);
    assert!(
        rendered.body.contains("<title>Livraison</title>"),
        "{}",
        rendered.body
    );

    // Wherever Liquid reads the entry: `metaobjects`, a reference.
    let titles = "{% for faq in metaobjects.faq.values %}{{ faq.title }};{% endfor %}\
                  {{ metaobjects.faq.shipping.title.value }}";
    let session = || Session::initial(&fixture.store);
    assert_eq!(
        fixture.liquid_for(french("/"), session(), titles),
        "Livraison;Returns;Livraison"
    );
    assert_eq!(
        fixture.liquid("/", session(), titles),
        "Shipping;Returns;Shipping"
    );
}

#[test]
fn a_broken_json_template_is_an_error_of_the_page() {
    let fixture = theme_of(
        "broken-json-template",
        &[],
        &[
            (
                "layout/theme.liquid",
                "<html><body>{{ content_for_layout }}</body></html>",
            ),
            (
                "templates/index.json",
                "{\n  \"sections\": {\n    \"a\": {\"type\": \"main\" \"x\": 1}\n  }\n}",
            ),
            (
                "templates/page.json",
                "{\"sections\": {\"a\": {\"type\": \"main-prodcut\"}}, \"order\": [\"a\"]}",
            ),
            ("sections/main.liquid", "<p>main</p>"),
            ("sections/main-product.liquid", "<p>product</p>"),
        ],
    );
    let rendered = fixture.page("/");
    assert_eq!(rendered.errors.len(), 1, "{}", rendered.body);
    let message = rendered.errors[0].to_string();
    assert!(message.contains("templates/index.json line 3"), "{message}");
    assert!(message.contains("invalid JSON"), "{message}");
    assert!(
        rendered.body.contains("<pre>Liquid error"),
        "{}",
        rendered.body
    );

    // A section type without a file: the error names the template and the closest type.
    let site = fixture.renderer.site(
        fixture.store.clone(),
        request("/pages/contact"),
        Session::initial(&fixture.store),
    );
    let page = Page::new("page", Resource::Index);
    let rendered = fixture.renderer.render_page(&site, page, &Target::Page);
    assert_eq!(rendered.errors.len(), 1, "{}", rendered.body);
    let message = rendered.errors[0].to_string();
    assert!(message.contains("templates/page.json line 1"), "{message}");
    assert!(
        message
            .contains("'main-prodcut' is not a valid section type (did you mean 'main-product'?)"),
        "{message}"
    );
}

#[test]
fn a_locale_of_the_theme_the_store_does_not_sell_in_says_so() {
    let fixture = theme_of(
        "missing-language",
        &[],
        &[
            (
                "layout/theme.liquid",
                "<html><body>{{ content_for_layout }}</body></html>",
            ),
            ("templates/index.liquid", "home"),
            ("locales/en.default.json", "{}"),
            ("locales/de.json", "{}"),
        ],
    );
    let rendered = fixture.page("/de/");
    assert_eq!(rendered.status, 404);
    assert_eq!(
        rendered.not_found.as_deref(),
        Some(
            "no page at \"/de/\": the theme has a de locale but the store data has no German \
             language; add {\"iso_code\": \"de\"} to localization.languages"
        )
    );
    // A path that names no locale is only missing.
    assert_eq!(
        fixture.page("/nope").not_found.as_deref(),
        Some("no page at \"/nope\"")
    );
}

#[test]
fn the_payment_button_speaks_the_language_of_the_storefront() {
    let fixture = theme_of(
        "payment-button",
        &[],
        &[("templates/index.liquid", "{{ product | payment_button }}")],
    );
    let button = |locale: &str| {
        let mut request = request("/");
        request.locale = locale.to_string();
        let rendered =
            fixture.render_with(request, Session::initial(&fixture.store), &Target::Page);
        assert_clean(&rendered);
        rendered.body
    };
    assert!(button("en").contains(">Buy it now</button>"));
    assert!(button("fr").contains(">Acheter maintenant</button>"));
    assert!(button("de").contains(">Jetzt kaufen</button>"));
    assert!(button("pt-BR").contains(">Comprar agora</button>"));
    assert!(button("fr-CA").contains(">Acheter maintenant</button>"));
    // A language Shopify's button has no words for is in English.
    assert!(button("eo").contains(">Buy it now</button>"));
}

#[test]
fn a_schema_shopify_refuses_is_an_error_of_the_page_that_renders_it() {
    let fixture = theme_of(
        "schema-errors",
        &[],
        &[
            (
                "layout/theme.liquid",
                "<html><body>{{ content_for_layout }}</body></html>",
            ),
            (
                "templates/index.json",
                r#"{"sections": {"a": {"type": "hero"}, "b": {"type": "hero"}, "c": {"type": "list", "blocks": {"x": {"type": "item"}}, "block_order": ["x"]}}, "order": ["a", "b", "c"]}"#,
            ),
            (
                "sections/hero.liquid",
                "<h1>{{ section.settings.title }}</h1>\n{% schema %}\n{\"name\": \"Hero\", \"settings\": [{\"type\": \"text\", \"id\": \"title\", \"label\": \"Title\", \"default\": \"Hello\"} {}]}\n{% endschema %}",
            ),
            (
                "sections/list.liquid",
                "{% content_for 'blocks' %}{% schema %}{\"name\": \"List\", \"blocks\": [{\"type\": \"@theme\"}]}{% endschema %}",
            ),
            (
                "blocks/item.liquid",
                "<i>{{ block.settings.size }}</i>\n{% schema %}\n{\"name\": \"Item\", \"settings\": [\n{\"type\": \"range\", \"id\": \"size\", \"label\": \"Size\", \"min\": 0, \"max\": 10, \"step\": 2, \"default\": 3}\n]}\n{% endschema %}",
            ),
        ],
    );
    let rendered = fixture.page("/");
    let errors: Vec<String> = rendered.errors.iter().map(ToString::to_string).collect();
    // Once each, though the hero renders twice; where the file has the problem.
    assert_eq!(errors.len(), 2, "{errors:#?}");
    assert!(
        errors[0]
            .starts_with("Liquid error (sections/hero line 3): the {% schema %} is not valid JSON"),
        "{errors:#?}"
    );
    assert!(
        errors[1].starts_with("Liquid error (blocks/item line 4): {% schema %}: the default 3 of setting \"size\" is not a step of the range"),
        "{errors:#?}"
    );
    // The settings have no default, as on a store that would have taken the file.
    assert!(rendered.body.contains("<h1></h1>"), "{}", rendered.body);
}

#[test]
fn a_theme_folder_that_does_not_exist_says_so() {
    let missing = std::env::temp_dir().join(format!("lsf-no-theme-{}", std::process::id()));
    let env = Arc::new(lsf_theme::environment());
    let error = Theme::open(&missing, env.clone(), Revalidate::Never)
        .err()
        .expect("an error")
        .to_string();
    assert!(error.contains("does not exist"), "{error}");
    // A folder that is not a theme is told apart.
    let error = Theme::open(std::env::temp_dir(), env, Revalidate::Never)
        .err()
        .expect("an error")
        .to_string();
    assert!(
        error.contains("does not look like a Shopify theme"),
        "{error}"
    );
}

#[test]
fn a_page_says_which_of_lsfs_default_menus_it_renders() {
    // The data has a footer menu and no main menu: lsf makes up the main menu.
    let removed = lsf_core::store::load::Removals {
        menus: vec!["main-menu".to_string()],
        ..Default::default()
    };
    let overlay = lsf_core::store::load::Overlay {
        name: "(test)".to_string(),
        removed,
        data: serde_json::json!({}),
    };
    let (store, diagnostics) = lsf_core::store::load::load_with_overlays(
        &DataSource::Demo,
        &LoadOptions::default(),
        &[overlay],
    );
    assert!(diagnostics.is_empty(), "{diagnostics}");
    let fixture = Fixture {
        store: Arc::new(store),
        ..theme_of(
            "default-menus",
            &[],
            &[
                (
                    "layout/theme.liquid",
                    "<html><body>{{ content_for_layout }}\
                     <footer>{{ linklists.footer.links.size }}</footer></body></html>",
                ),
                // A `link_list` setting and a lookup by handle are the same menu, said once.
                (
                    "templates/index.json",
                    r#"{"sections": {"header": {"type": "header", "settings": {"menu": "main-menu"}}}, "order": ["header"]}"#,
                ),
                (
                    "sections/header.liquid",
                    "{% for link in section.settings.menu.links %}[{{ link.title }}]{% endfor %}\
                     {{ linklists['main-menu'].title }}\
                     {% schema %}{\"name\": \"Header\", \"settings\": [{\"type\": \"link_list\", \"id\": \"menu\", \"label\": \"Menu\", \"default\": \"main-menu\"}]}{% endschema %}",
                ),
                ("templates/page.liquid", "{{ page.title }}"),
            ],
        )
    };
    let home = fixture.page("/");
    assert_clean(&home);
    assert!(
        home.body.contains("[Home][Catalog]Main menu"),
        "{}",
        home.body
    );
    assert_eq!(
        home.default_menus,
        [
            "menu \"main-menu\" is lsf's default (Home, Catalog), not the store's: the store data \
             has none with this handle. Add it to \"menus\" to render the store's links"
        ]
    );
    // The footer is the data's, and a page without the main menu has nothing to say.
    let page = fixture.page("/pages/contact");
    assert_clean(&page);
    assert!(page.default_menus.is_empty(), "{:?}", page.default_menus);
}

#[test]
fn a_static_block_type_that_differs_by_case_renders_its_file() {
    // Shopify renders `blocks/text.liquid` for `type: 'TEXT'`; lsf finds it on every file
    // system, not only on those that ignore case.
    let fixture = theme_of(
        "block-type-case",
        &[],
        &[
            (
                "layout/theme.liquid",
                "<html><head>{{ content_for_header }}</head><body>{{ content_for_layout }}</body></html>",
            ),
            (
                "templates/index.json",
                r#"{ "sections": { "main": { "type": "holder" } }, "order": ["main"] }"#,
            ),
            (
                "sections/holder.liquid",
                "{% content_for 'block', type: 'TEXT', id: 't' %}\
                 {% schema %}{\"name\": \"Holder\", \"settings\": []}{% endschema %}",
            ),
            (
                "blocks/text.liquid",
                "<p>text block {{ block.type }}</p>{% schema %}{\"name\": \"Text\", \"settings\": []}{% endschema %}",
            ),
        ],
    );
    let page = fixture.page("/");
    assert_clean(&page);
    assert!(
        page.body.contains("<p>text block text</p>"),
        "{}",
        page.body
    );
}

#[test]
fn a_block_rendered_again_gets_a_numbered_id() {
    // On Shopify, the second render of a block on a page has the id of the first with `-1`,
    // the third with `-2`, and so on: Horizon's product card, a static block rendered once
    // per product, is `<prefix>__static-product-card`, then `...-1`, `...-2`, and so are the
    // blocks it holds. A block rendered once keeps its id, and `section.blocks` gives the ids
    // without a number (Horizon looks a block up there with `find_index: 'id', block.id`).
    let fixture = theme_of(
        "block-id-suffix",
        &[],
        &[
            (
                "layout/theme.liquid",
                "<html><head>{{ content_for_header }}</head><body>{{ content_for_layout }}</body></html>",
            ),
            (
                "templates/index.json",
                r#"{
                  "sections": {
                    "main": {
                      "type": "cards",
                      "blocks": {
                        "card": {
                          "type": "card",
                          "static": true,
                          "blocks": { "price": { "type": "text" } },
                          "block_order": ["price"]
                        },
                        "intro": { "type": "text" }
                      },
                      "block_order": ["intro"]
                    },
                    "pair": {
                      "type": "boxes",
                      "blocks": { "left": { "type": "box" }, "right": { "type": "box" } },
                      "block_order": ["left", "right"]
                    }
                  },
                  "order": ["main", "pair"]
                }"#,
            ),
            (
                "sections/cards.liquid",
                "{% for i in (1..3) %}{% content_for 'block', type: 'card', id: 'card' %}{% endfor %}\
                 {% content_for 'blocks' %}{% content_for 'blocks' %}\
                 {% for block in section.blocks %}<li>{{ block.id }}</li>{% endfor %}\
                 {% schema %}{\"name\": \"Cards\", \"blocks\": [{\"type\": \"@theme\"}]}{% endschema %}",
            ),
            (
                "sections/boxes.liquid",
                "{% content_for 'blocks' %}\
                 {% schema %}{\"name\": \"Boxes\", \"blocks\": [{\"type\": \"@theme\"}]}{% endschema %}",
            ),
            (
                "blocks/box.liquid",
                "{% content_for 'block', type: 'text', id: 'title' %}\
                 {% schema %}{\"name\": \"Box\", \"tag\": null, \"blocks\": [{\"type\": \"@theme\"}]}{% endschema %}",
            ),
            (
                "blocks/card.liquid",
                "<p>{{ block.id }}</p>{% content_for 'blocks' %}\
                 {% schema %}{\"name\": \"Card\", \"blocks\": [{\"type\": \"@theme\"}]}{% endschema %}",
            ),
            (
                "blocks/text.liquid",
                "<span>{{ block.id }}</span>{% schema %}{\"name\": \"Text\", \"tag\": null}{% endschema %}",
            ),
        ],
    );
    let section = lsf_theme::render::template_section_id("index", "main");
    let prefix = format!("A{}", &lsf_core::util::short_hash(&section)[..15]);
    let card = |suffix: &str| {
        format!(
            "<div id=\"shopify-block-{prefix}__card{suffix}\" class=\"shopify-block\">\
             <p>{prefix}__card{suffix}</p><span>{prefix}__price{suffix}</span></div>"
        )
    };
    let expected = format!(
        "<div id=\"shopify-section-{section}\" class=\"shopify-section\">{}{}{}\
         <span>{prefix}__intro</span><span>{prefix}__intro-1</span>\
         <li>{prefix}__intro</li></div>",
        card(""),
        card("-1"),
        card("-2"),
    );

    let page = fixture.page("/");
    assert_clean(&page);
    assert!(page.body.contains(&expected), "{}", page.body);

    // Two blocks that give a static block the same id each render it once: neither is
    // numbered. Shopify gives every block a prefix of its own; lsf's comes from the section,
    // so the two have the same id.
    let pair = lsf_theme::render::template_section_id("index", "pair");
    let title = format!(
        "<span>A{}__title</span>",
        &lsf_core::util::short_hash(&pair)[..15]
    );
    assert!(
        page.body.contains(&format!(
            "<div id=\"shopify-section-{pair}\" class=\"shopify-section\">{title}{title}</div>"
        )),
        "{}",
        page.body
    );

    // The Section Rendering API renders the section alone: its numbers start again.
    let alone = fixture.render_with(
        request("/"),
        Session::initial(&fixture.store),
        &Target::Section(section.clone()),
    );
    assert_eq!(alone.body, expected);
    // A section asked for twice in `?sections=` is rendered once: Shopify answers
    // `?sections=<id>,<id>` with one key, its blocks numbered from the first render.
    let twice = fixture.render_with(
        request("/"),
        Session::initial(&fixture.store),
        &Target::Sections(vec![section.clone(), section.clone()]),
    );
    let json: serde_json::Value = serde_json::from_str(&twice.body).unwrap();
    assert_eq!(json.as_object().map(serde_json::Map::len), Some(1));
    assert_eq!(json[&section], expected);

    // The timings count a block once, under the id of its first render, with each render.
    let timed: Vec<(&str, usize)> = page.timings.sections[0]
        .blocks
        .iter()
        .map(|block| (block.id.as_str(), block.durations.len()))
        .collect();
    let (card, price, intro) = (
        format!("{prefix}__card"),
        format!("{prefix}__price"),
        format!("{prefix}__intro"),
    );
    assert_eq!(
        timed,
        [(card.as_str(), 3), (price.as_str(), 3), (intro.as_str(), 2)]
    );
}

#[test]
fn a_color_scheme_gives_its_colors_through_the_setting_and_by_id() {
    let fixture = theme_of(
        "color-scheme-settings",
        &[],
        &[
            (
                "config/settings_schema.json",
                r##"[
                  { "name": "theme_info" },
                  { "name": "Colors", "settings": [
                    { "type": "color_scheme", "id": "scheme", "label": "Scheme", "default": "scheme-1" },
                    { "type": "color_scheme_group", "id": "color_schemes",
                      "definition": [
                        { "type": "color", "id": "background", "label": "Background", "default": "#FFFFFF" },
                        { "type": "color", "id": "text", "label": "Text", "default": "#121212" }
                      ],
                      "role": { "text": "text", "background": { "solid": "background", "gradient": "background" } } }
                  ] }
                ]"##,
            ),
            (
                "config/settings_data.json",
                r##"{ "current": {
                  "scheme": "scheme-2",
                  "color_schemes": {
                    "scheme-1": { "settings": { "background": "#FFFFFF", "text": "#121212" } },
                    "scheme-2": { "settings": { "background": "#334FB4", "text": "#FFFFFF" } }
                  }
                } }"##,
            ),
            ("layout/theme.liquid", "{{ content_for_layout }}"),
            (
                "templates/index.liquid",
                "id=[{{ settings.scheme.id }}]\n\
                 background=[{{ settings.scheme.settings.background }}]\n\
                 text-rgb=[{{ settings.scheme.settings.text.rgb }}]\n\
                 by-key=[{{ settings.color_schemes['scheme-1'].settings.text }}]\n\
                 by-name=[{{ settings.color_schemes.scheme-2.id }}]\n\
                 count=[{{ settings.color_schemes.size }} {{ settings.color_schemes.first.id }} {{ settings.color_schemes[1].id }}]\n\
                 loop=[{% for s in settings.color_schemes %}{{ s.id }}:{{ s.settings.background }};{% endfor %}]\n\
                 {% section 'scheme' %}",
            ),
            (
                "sections/scheme.liquid",
                "section=[{{ section.settings.scheme }} {{ section.settings.scheme.settings.text }}]\
                 {% schema %}{\"name\": \"Scheme\", \"settings\": [{\"type\": \"color_scheme\", \
                 \"id\": \"scheme\", \"label\": \"Scheme\", \"default\": \"scheme-1\"}]}{% endschema %}",
            ),
        ],
    );
    let rendered = fixture.page("/");
    assert_clean(&rendered);
    let body = rendered.body.split("<script").next().unwrap();
    assert!(
        body.starts_with(
            "id=[scheme-2]\n\
             background=[#334FB4]\n\
             text-rgb=[255 255 255]\n\
             by-key=[#121212]\n\
             by-name=[scheme-2]\n\
             count=[2 scheme-1 scheme-2]\n\
             loop=[scheme-1:#FFFFFF;scheme-2:#334FB4;]\n"
        ),
        "{body}"
    );
    assert!(body.contains("section=[scheme-1 #121212]"), "{body}");
}

#[test]
fn theme_json_with_trailing_commas_and_unknown_escapes_renders_as_on_shopify() {
    let files = [
        (
            "sections/comma.liquid",
            "<p>comma: [{{ section.settings.t }}]</p>\n{% schema %}\n{\n  \"name\": \"Comma\",\n  \
             \"settings\": [\n    { \"type\": \"text\", \"id\": \"t\", \"label\": \"T\", \"default\": \"dflt\" },\n  ],\n}\n\
             {% endschema %}",
        ),
        (
            "sections/escape.liquid",
            "<p>escape: [{{ section.settings.q }}]</p>{% schema %}{ \"name\": \"Escape\", \"settings\": \
             [{ \"type\": \"text\", \"id\": \"q\", \"label\": \"Q\", \"default\": \"\\“q\\”\" }] }{% endschema %}",
        ),
        (
            "sections/globals.liquid",
            "<p>setting: [{{ settings.label }}]</p><p>locale: [{{ 'general.hi' | t }}]</p>\
             {% schema %}{ \"name\": \"Globals\", \"settings\": [] }{% endschema %}",
        ),
        (
            "config/settings_schema.json",
            "[\n  { \"name\": \"theme_info\" },\n  { \"name\": \"Probe\", \"settings\": [\n    \
             { \"type\": \"text\", \"id\": \"label\", \"label\": \"Label\", \"default\": \"from schema\" },\n  ] }\n]",
        ),
        (
            "config/settings_data.json",
            "{ \"current\": { \"label\": \"from data\", }, }",
        ),
        (
            "locales/en.default.json",
            "{ \"general\": { \"hi\": \"Hello\" } }",
        ),
        (
            "locales/fr.json",
            "{\n  \"general\": { \"hi\": \"Bonjour\", },\n}",
        ),
        (
            "templates/index.json",
            "{ \"sections\": { \"a\": { \"type\": \"comma\" }, \"b\": { \"type\": \"escape\" }, \
             \"c\": { \"type\": \"globals\" } }, \"order\": [\"a\", \"b\", \"c\"] }",
        ),
        (
            "templates/list-collections.json",
            "{\n  \"sections\": { \"a\": { \"type\": \"comma\" } },\n  \"order\": [\"a\"],\n}",
        ),
        (
            "layout/theme.liquid",
            "<body>{{ content_for_layout }}</body>",
        ),
    ];
    let fixture = theme_of("lenient-json", &[], &files);
    let paragraphs = |rendered: &Rendered| -> Vec<String> {
        rendered
            .body
            .split("<p>")
            .skip(1)
            .map(|p| p.split("</p>").next().unwrap().to_string())
            .collect()
    };
    let home = fixture.page("/");
    assert_clean(&home);
    assert!(home.warnings.is_empty(), "{:?}", home.warnings);
    assert_eq!(
        paragraphs(&home),
        [
            "comma: [dflt]",
            "escape: [“q”]",
            "setting: [from data]",
            "locale: [Hello]"
        ]
    );
    let mut french = request("/");
    french.locale = "fr".to_string();
    french.root = "/fr".to_string();
    let french = fixture.render_with(french, Session::initial(&fixture.store), &Target::Page);
    assert_eq!(paragraphs(&french)[3], "locale: [Bonjour]");
    let collections = fixture.page("/collections");
    assert_clean(&collections);
    assert_eq!(paragraphs(&collections), ["comma: [dflt]"]);

    // A byte order mark: Shopify refuses the file, and the render says it left it out.
    let mut files = files.to_vec();
    let bom = "\u{feff}{ \"current\": { \"label\": \"from data\" } }";
    files[4] = ("config/settings_data.json", bom);
    let fixture = theme_of("lenient-json-bom", &[], &files);
    let home = fixture.page("/");
    assert_eq!(paragraphs(&home)[2], "setting: [from schema]");
    assert_eq!(
        home.warnings,
        [
            "config/settings_data.json: invalid JSON at line 1, column 1: the file starts with a \
          UTF-8 byte order mark (BOM), which Shopify refuses: the file is left out of the page"
        ]
    );
}

#[test]
fn closest_is_the_nearest_resource_setting_then_the_template() {
    let template = r#"{
      "sections": {
        "main": {
          "type": "feat",
          "settings": { "product": "canvas-tote-bag" },
          "blocks": {
            "a": { "type": "card", "settings": {} },
            "b": { "type": "plain", "settings": {} },
            "g": {
              "type": "group",
              "settings": { "product": "ceramic-mug" },
              "blocks": { "c": { "type": "card", "settings": {} }, "d": { "type": "plain", "settings": {} } },
              "block_order": ["c", "d"]
            }
          },
          "block_order": ["a", "b", "g"]
        },
        "param": { "type": "param" }
      },
      "order": ["main", "param"]
    }"#;
    let fixture = theme_of(
        "closest-settings",
        &[],
        &[
            (
                "layout/theme.liquid",
                "<body>{{ content_for_layout }}</body>",
            ),
            (
                "sections/feat.liquid",
                "<p>section=[{{ closest.product.handle }}]</p>{% content_for 'blocks' %}\
                 {% schema %}{\"name\": \"Feat\", \"settings\": [{\"type\": \"product\", \"id\": \"product\", \
                 \"label\": \"Product\"}], \"blocks\": [{\"type\": \"@theme\"}]}{% endschema %}",
            ),
            (
                "sections/param.liquid",
                "{% content_for 'block', type: 'plain', id: 'p', closest.product: all_products['merino-crew-socks'] %}\
                 {% content_for 'block', type: 'card', id: 'q', closest.product: all_products['merino-crew-socks'] %}\
                 {% schema %}{\"name\": \"Param\", \"settings\": []}{% endschema %}",
            ),
            (
                "blocks/card.liquid",
                "<p>card {{ block.id | split: '__' | last }}=[{{ closest.product.handle }}]</p>\
                 {% schema %}{\"name\": \"Card\", \"settings\": [{\"type\": \"product\", \"id\": \"product\", \
                 \"label\": \"Product\"}]}{% endschema %}",
            ),
            (
                "blocks/plain.liquid",
                "<p>plain {{ block.id | split: '__' | last }}=[{{ closest.product.handle }}]</p>\
                 {% schema %}{\"name\": \"Plain\", \"settings\": []}{% endschema %}",
            ),
            (
                "blocks/group.liquid",
                "<p>group {{ block.id | split: '__' | last }}=[{{ closest.product.handle }}]</p>{% content_for 'blocks' %}\
                 {% schema %}{\"name\": \"Group\", \"settings\": [{\"type\": \"product\", \"id\": \"product\", \
                 \"label\": \"Product\"}], \"blocks\": [{\"type\": \"@theme\"}]}{% endschema %}",
            ),
            ("templates/product.json", template),
            ("templates/index.json", template),
        ],
    );
    // The page's product, then the home page, which has none: the same, from the settings.
    for path in ["/products/linen-overshirt", "/"] {
        let rendered = fixture.page(path);
        assert_clean(&rendered);
        let lines: Vec<&str> = rendered
            .body
            .split("<p>")
            .skip(1)
            .map(|line| line.split("</p>").next().unwrap())
            .collect();
        assert_eq!(
            lines,
            [
                "section=[canvas-tote-bag]",
                // A block's own product setting is its closest, even unset.
                "card a=[]",
                "plain b=[canvas-tote-bag]",
                "group g=[ceramic-mug]",
                "card c=[]",
                "plain d=[ceramic-mug]",
                // `closest.product:` of `content_for`, under the block's own setting.
                "plain p=[merino-crew-socks]",
                "card q=[]",
            ],
            "{path}"
        );
    }
}
/// The fixture theme against the demo store plus `overlay` (store data).
fn fixture_with_data(overlay: &serde_json::Value) -> Fixture {
    let (store, diagnostics) = lsf_core::store::load::load_with_overlay(
        &DataSource::Demo,
        &LoadOptions::default(),
        Some(overlay),
    );
    assert!(diagnostics.is_empty(), "{diagnostics}");
    Fixture {
        store: Arc::new(store),
        ..fixture()
    }
}

#[test]
fn options_by_name_finds_an_option_in_any_case() {
    // #125, and https://shopify.dev/docs/api/liquid/objects/product#product-options_by_name:
    // "When accessing a specific option, the name is case-insensitive."
    let fixture = fixture_with_data(&serde_json::json!({"products": [{
        "title": "Case Tee",
        "price": 1000,
        "options": ["Color"],
        "variants": [{ "options": ["Red"] }, { "options": ["Blue"] }]
    }]}));
    let output = fixture.liquid(
        "/products/case-tee",
        Session::initial(&fixture.store),
        "[{{ product.options_by_name['Color'].name }}][{{ product.options_by_name['color'].name }}]\
         [{{ product.options_by_name['COLOR'].values.size }}][{{ product.options_by_name.color.name }}]\
         [{{ product.options_by_name['size'].name }}][{{ product.options_by_name.size }}]\
         {% for pair in product.options_by_name %}[{{ pair[0] }}={{ pair[1].values | join: ',' }}]{% endfor %}",
    );
    assert_eq!(output, "[Color][Color][2][Color][][1][Color=Red,Blue]");
}

#[test]
fn a_color_metafield_prints_as_it_is_stored() {
    // #195: Shopify keeps the case of a stored color, in a metafield as in a setting.
    let fixture = fixture_with_data(&serde_json::json!({"products": [{
        "title": "Tint",
        "price": 1000,
        "metafields": { "custom": {
            "tint": { "type": "color", "value": "#FF00aa" },
            "tints": { "type": "list.color", "value": ["#ff0000", "#00FF00"] }
        } }
    }]}));
    let output = fixture.liquid(
        "/products/tint",
        Session::initial(&fixture.store),
        "{% assign m = product.metafields.custom.tint %}{{ m.value }}|{{ product.metafields.custom.tints.value | join: ',' }}|\
         {{ m | metafield_tag }}|{{ m.value | json }}|{% if m.value == '#FF00aa' %}equal{% endif %}|\
         {{ m.value.red }}|{{ m.value | color_to_rgb }}",
    );
    assert_eq!(
        output,
        "#FF00aa|#ff0000,#00FF00|<span class=\"metafield-color\">#FF00aa</span>|\"#FF00aa\"|equal|\
         255|rgb(255, 0, 170)"
    );
}

#[test]
fn a_metaobject_is_found_in_any_case() {
    // #195: Shopify finds an entry whatever the case of its type or handle, in Liquid and in
    // its page's URL, whose `system.url` and canonical URL are in lower case.
    let fixture = theme_with_data(
        "metaobject-case",
        &serde_json::json!({
            "metaobjects": { "faq": [{ "handle": "one", "fields": { "question": "How long?" } }] }
        }),
        &[
            (
                "layout/theme.liquid",
                "{{ content_for_layout }}|{{ canonical_url }}",
            ),
            (
                "templates/metaobject/faq.liquid",
                "{{ metaobject.question }}|{{ metaobject.system.url }}",
            ),
        ],
    );
    let output = fixture.liquid(
        "/",
        Session::initial(&fixture.store),
        "[{{ metaobjects.faq['ONE'].question }}]|[{{ metaobjects.FAQ.one.question }}]|\
         [{{ metaobjects.Faq.values.size }}]",
    );
    assert_eq!(output, "[How long?]|[How long?]|[1]");
    let page = fixture.page("/pages/FAQ/ONE");
    assert_eq!(page.status, 200);
    assert_eq!(
        page.body,
        "How long?|/pages/faq/one|http://shop.test/pages/faq/one"
    );
}

#[test]
fn a_tag_page_counts_the_whole_collection_in_all_products_count() {
    // #190: https://shopify.dev/docs/api/liquid/objects/collection#collection-all_products_count,
    // "This includes products that have been filtered out of the current view".
    let fixture = fixture_with_data(&serde_json::json!({
        "products": [
            { "title": "Widget", "price": 1000, "tags": ["Premium", "Snow"] },
            { "title": "Gadget", "price": 2000, "tags": ["Snow"] },
            { "title": "Gizmo", "price": 3000, "tags": ["Premium"] }
        ],
        "collections": [{ "handle": "things", "title": "Things", "products": ["widget", "gadget", "gizmo"] }]
    }));
    let counts = |path: &str| {
        fixture.liquid(
            path,
            Session::initial(&fixture.store),
            "[{{ collection.all_products_count }}|{{ collection.products_count }}]",
        )
    };
    assert_eq!(counts("/collections/things"), "[3|3]");
    assert_eq!(counts("/collections/things/snow"), "[3|2]");
}

#[test]
fn an_external_video_has_the_video_s_aspect_ratio() {
    // #190: Shopify gives a YouTube video `aspect_ratio` 1.77, and its 480 x 360 thumbnail
    // 1.3333333333333333.
    let fixture = fixture_with_data(&serde_json::json!({"products": [{
        "title": "Clip",
        "price": 500,
        "media": [{
            "media_type": "external_video", "host": "youtube", "external_id": "dQw4w9WgXcQ",
            "preview_image": { "src": "poster.jpg", "width": 480, "height": 360 }
        }]
    }]}));
    let output = fixture.liquid(
        "/products/clip",
        Session::initial(&fixture.store),
        "{% for m in product.media %}{{ m.aspect_ratio }}|{{ m.preview_image.aspect_ratio }}{% endfor %}",
    );
    assert_eq!(output, "1.77|1.3333333333333333");
}

#[test]
fn image_url_checks_its_arguments_as_shopify_does() {
    // #190: https://shopify.dev/docs/api/liquid/filters/image_url. A width over 5760 is 5760,
    // one under 1 Shopify's error; `pad_color` is `hex3` or `hex6`; `format: 'jpg'` of a JPEG
    // converts nothing ("png to jpg, png to pjpg, jpg to pjpg"), and is left out.
    let fixture = fixture_with_data(&serde_json::json!({"products": [
        { "title": "Widget", "price": 1000, "images": [{ "src": "widget.jpg", "width": 1600, "height": 1600 }] },
        { "title": "Sticker", "price": 1000, "images": [{ "src": "sticker.png", "width": 1600, "height": 1600 }] }
    ]}));
    let render = |path: &str, arguments: &str| {
        let site = fixture.renderer.site(
            fixture.store.clone(),
            request(path),
            Session::initial(&fixture.store),
        );
        let page = lsf_theme::render::routes::resolve(&site);
        let source = format!("{{{{ product.featured_image | image_url: {arguments} }}}}");
        let (output, _) = fixture
            .renderer
            .render_liquid(&site, page, &source, &[])
            .expect("the template parses");
        // The query without `v=`, the file's version.
        match output.split_once('?') {
            Some((base, query)) => {
                let query: Vec<&str> = query
                    .split('&')
                    .filter(|pair| !pair.starts_with("v="))
                    .collect();
                format!("{}?{}", base.rsplit('/').next().unwrap(), query.join("&"))
            }
            None => output,
        }
    };
    let widget = |arguments: &str| render("/products/widget", arguments);
    assert_eq!(widget("width: 6000"), "widget.jpg?width=5760");
    assert_eq!(
        widget("width: 0"),
        "Liquid error (line 1): Width must be between 1 and 5760"
    );
    assert_eq!(
        widget("height: -2"),
        "Liquid error (line 1): Height must be between 1 and 5760"
    );
    assert_eq!(
        widget("width: 300, height: 100, pad_color: '#ff0000'"),
        "Liquid error (line 1): Color padding must be between 000000 and ffffff"
    );
    assert_eq!(
        widget("width: 300, height: 100, pad_color: 'FF0000'"),
        "widget.jpg?height=100&pad_color=FF0000&width=300"
    );
    assert_eq!(widget("width: 300, format: 'jpg'"), "widget.jpg?width=300");
    assert_eq!(
        widget("width: 300, format: 'pjpg'"),
        "widget.jpg?format=pjpg&width=300"
    );
    assert_eq!(
        render("/products/sticker", "width: 300, format: 'jpg'"),
        "sticker.png?format=jpg&width=300"
    );
}

#[test]
fn placeholder_svg_tag_refuses_a_name_shopify_does_not_have() {
    // #190: https://shopify.dev/docs/api/liquid/filters/placeholder_svg_tag "Accepts the
    // following placeholder names"; Shopify renders `Unknown SVG placeholder 'nope'` for others.
    let fixture = fixture();
    let site = fixture.renderer.site(
        fixture.store.clone(),
        request("/"),
        Session::initial(&fixture.store),
    );
    let page = lsf_theme::render::routes::resolve(&site);
    let (output, errors) = fixture
        .renderer
        .render_liquid(
            &site,
            page,
            "[{{ 'nope' | placeholder_svg_tag }}][{{ '' | placeholder_svg_tag }}]\
             [{{ 'detailed-apparel-1' | placeholder_svg_tag | slice: 0, 4 }}]",
            &[],
        )
        .expect("the template parses");
    assert_eq!(
        output,
        "[Liquid error (line 1): Unknown SVG placeholder 'nope']\
         [Liquid error (line 1): Unknown SVG placeholder '']\
         [<svg]"
    );
    assert_eq!(errors.len(), 2);
}

#[test]
fn the_title_of_a_search_counts_its_results() {
    // #190: Shopify's `page_title` of `/search?q=snowboard` is
    // `Search: 14 results found for &quot;snowboard&quot;`.
    let fixture = fixture_with_data(&serde_json::json!({"products": [
        { "title": "Quoted Widget", "price": 1000 },
        { "title": "Quoted Gadget", "price": 1000 }
    ]}));
    let title =
        |path: &str| fixture.liquid(path, Session::initial(&fixture.store), "{{ page_title }}");
    assert_eq!(
        title("/search?q=quoted"),
        "Search: 2 results found for &quot;quoted&quot;"
    );
    assert_eq!(
        title("/search?q=gadget"),
        "Search: 1 result found for &quot;gadget&quot;"
    );
    assert_eq!(title("/search"), "Search");
}

#[test]
fn collection_sort_options_has_most_relevant() {
    // #190: the nine options of https://shopify.dev/docs/api/liquid/objects/collection#collection-sort_options,
    // `most-relevant` second.
    let fixture = fixture();
    let output = fixture.liquid(
        "/collections/all",
        Session::initial(&fixture.store),
        "{% for o in collection.sort_options %}{{ o.value }}={{ o.name }};{% endfor %}",
    );
    assert_eq!(
        output,
        "manual=Featured;most-relevant=Most relevant;best-selling=Best selling;\
         title-ascending=Alphabetically, A-Z;title-descending=Alphabetically, Z-A;\
         price-ascending=Price, low to high;price-descending=Price, high to low;\
         created-ascending=Date, old to new;created-descending=Date, new to old;"
    );
}

#[test]
fn canonical_url_keeps_the_tags_and_the_query() {
    // #113: on Shopify a tag page is its own canonical, and the search, vendor and type pages
    // keep their `q`, the vendor's lower-cased. Sorting is left out.
    let fixture = fixture_with_data(&serde_json::json!({
        "products": [
            { "title": "Widget", "vendor": "Acme Labs", "type": "Snowboard", "price": 1000, "tags": ["Premium", "Snow"] },
            { "title": "Gadget", "price": 2000, "tags": ["Snow"] }
        ],
        "collections": [{ "handle": "things", "title": "Things", "products": ["widget", "gadget"] }]
    }));
    // The query as the server decodes it: `request` does not.
    let canonical = |path: &str| {
        fixture.liquid(
            path,
            Session::initial(&fixture.store),
            "{{ canonical_url }}",
        )
    };
    let origin = "http://shop.test";
    for (path, expected) in [
        ("/collections/things/snow", "/collections/things/snow"),
        (
            "/collections/things/premium+snow",
            "/collections/things/premium+snow",
        ),
        (
            "/collections/things?sort_by=price-ascending",
            "/collections/things",
        ),
        (
            "/collections/vendors?q=Acme Labs",
            "/collections/vendors?q=acme+labs",
        ),
        (
            "/collections/types?q=Snowboard",
            "/collections/types?q=snowboard",
        ),
        ("/search?q=widget", "/search?q=widget"),
        ("/search", "/search"),
    ] {
        assert_eq!(canonical(path), format!("{origin}{expected}"), "{path}");
    }
}

#[test]
fn current_tags_holds_the_tags_as_the_products_write_them() {
    // #115: on `/collections/<handle>/<tag>`, Shopify's `current_tags` holds the tags as they
    // are written on the products (`Snow`), so that the test of
    // https://shopify.dev/docs/storefronts/themes/navigation-search/filtering/tag-filtering,
    // `current_tags contains tag` over `collection.all_tags`, finds the active ones.
    let fixture = fixture_with_data(&serde_json::json!({
        "products": [
            { "title": "Widget", "price": 1000, "tags": ["Premium", "Snow"] },
            { "title": "Gadget", "price": 2000, "tags": ["Snow"] },
            { "title": "Gizmo", "price": 3000, "tags": ["Premium"] }
        ],
        "collections": [{ "handle": "things", "title": "Things", "products": ["widget", "gadget", "gizmo"] }]
    }));
    let tags = |path: &str| {
        fixture.liquid(
            path,
            Session::initial(&fixture.store),
            "[{{ current_tags | join: ',' }}]\
             [{% for t in collection.all_tags %}{% if current_tags contains t %}{{ t }};{% endif %}{% endfor %}]\
             [{{ collection.products.size }}]",
        )
    };
    assert_eq!(tags("/collections/things/snow"), "[Snow][Snow;][2]");
    assert_eq!(
        tags("/collections/things/premium+snow"),
        "[Premium,Snow][Premium;Snow;][1]"
    );
    // A tag no product has stays as the URL writes it.
    assert_eq!(tags("/collections/things/nope"), "[nope][][0]");
}

#[test]
fn page_title_and_page_description_are_the_search_engine_listing() {
    // #100: Shopify keeps a resource's search engine listing in the metafields
    // `global.title_tag` and `global.description_tag`
    // (https://shopify.dev/docs/apps/build/marketing-analytics/optimize-storefront-seo), and
    // `page_title` and `page_description` give it. Without them, the title and description.
    let seo = |title: &str, description: &str| serde_json::json!({ "global": { "title_tag": title, "description_tag": description } });
    let fixture = fixture_with_data(&serde_json::json!({
        "products": [
            {
                "title": "Seo Tee",
                "description": "<p>A plain tee.</p>",
                "price": 1000,
                "metafields": seo("Tee SEO title", "Tee & SEO description")
            },
            { "title": "Plain Tee", "description": "<p>A plain tee.</p>", "price": 1000 }
        ],
        "collections": [{
            "handle": "seo-shirts",
            "title": "Shirts",
            "products": ["seo-tee"],
            "metafields": { "global": { "title_tag": "Shirts SEO title" } }
        }],
        "pages": [{
            "title": "About",
            "handle": "seo-about",
            "content": "<p>About us.</p>",
            "metafields": seo("About SEO title", "About SEO description")
        }]
    }));
    let listing = |path: &str| {
        fixture.liquid(
            path,
            Session::initial(&fixture.store),
            "{{ page_title }}|{{ page_description }}",
        )
    };
    assert_eq!(
        listing("/products/seo-tee"),
        "Tee SEO title|Tee &amp; SEO description"
    );
    assert_eq!(listing("/products/plain-tee"), "Plain Tee|A plain tee.");
    assert_eq!(listing("/collections/seo-shirts"), "Shirts SEO title|");
    assert_eq!(
        listing("/pages/seo-about"),
        "About SEO title|About SEO description"
    );
}

#[test]
fn a_product_media_in_json_is_the_media_object() {
    // #99: `{{ product.media | json }}` and `{{ product.featured_media | json }}` give media
    // objects, as in `{{ product | json }}` (https://shopify.dev/docs/api/liquid/filters/json),
    // while `product.images` and `product.featured_image` stay URLs.
    let fixture = fixture_with_data(&serde_json::json!({"products": [{
        "title": "Media Tee",
        "price": 1000,
        "images": [{ "src": "products/tee.jpg", "alt": "Tee", "width": 800, "height": 400 }]
    }]}));
    let output = fixture.liquid(
        "/products/media-tee",
        Session::initial(&fixture.store),
        "{{ product.media | json }}|{{ product.featured_media | json }}|\
         {{ product.featured_image | json }}|{{ product.images | json }}|{{ product | json }}",
    );
    let parts: Vec<serde_json::Value> = output
        .split('|')
        .map(|part| serde_json::from_str(part).expect("json"))
        .collect();
    let media = &parts[0][0];
    assert_eq!(media["media_type"], "image");
    assert_eq!(media["alt"], "Tee");
    assert_eq!(media["position"], 1);
    assert_eq!(media["width"], 800);
    assert_eq!(media["aspect_ratio"], 2.0);
    assert_eq!(media["preview_image"]["src"], media["src"]);
    assert_eq!(parts[1], *media);
    assert_eq!(parts[4]["media"][0], *media);
    assert_eq!(parts[2], media["src"]);
    assert_eq!(parts[3][0], media["src"]);
}

#[test]
fn an_option_in_json_is_its_name_position_and_values() {
    // #98: `{{ product.options_with_values | json }}` on a store gives each option as
    // `{"name","position","values"}`, its values as strings.
    let fixture = fixture_with_data(&serde_json::json!({"products": [{
        "title": "Json Tee",
        "price": 1000,
        "options": ["Color", "Size"],
        "variants": [{ "options": ["Black", "S"] }, { "options": ["White", "M"] }]
    }]}));
    let output = fixture.liquid(
        "/products/json-tee",
        Session::initial(&fixture.store),
        "{{ product.options_with_values | json }}\n{{ product.options_with_values.first | json }}",
    );
    assert_eq!(
        output,
        "[{\"name\":\"Color\",\"position\":1,\"values\":[\"Black\",\"White\"]},\
         {\"name\":\"Size\",\"position\":2,\"values\":[\"S\",\"M\"]}]\n\
         {\"name\":\"Color\",\"position\":1,\"values\":[\"Black\",\"White\"]}"
    );
}

#[test]
fn a_t_key_saved_as_a_setting_value_is_printed_as_it_is() {
    // https://shopify.dev/docs/storefronts/themes/architecture/locales/schema-locale-files:
    // `t:` translates the attributes of a schema, a `default` among them. A value saved in a
    // JSON template is no schema attribute: Shopify prints it as it is.
    let fixture = theme_of(
        "saved-t-key",
        &[],
        &[
            ("layout/theme.liquid", "{{ content_for_layout }}"),
            (
                "sections/heading.liquid",
                "<p>{{ section.settings.text }}|{{ section.settings.fallback }}</p>\
                 {% schema %}{\"name\": \"Heading\", \"settings\": [\
                 {\"type\": \"text\", \"id\": \"text\", \"label\": \"Text\"},\
                 {\"type\": \"text\", \"id\": \"fallback\", \"label\": \"Fallback\", \"default\": \"t:sections.heading.default\"}\
                 ]}{% endschema %}",
            ),
            (
                "templates/index.json",
                r#"{"sections": {"main": {"type": "heading", "settings": {"text": "t:sections.heading.default"}}}, "order": ["main"]}"#,
            ),
            (
                "locales/en.default.schema.json",
                r#"{"sections": {"heading": {"default": "Hello"}}}"#,
            ),
        ],
    );
    let page = fixture.page("/");
    assert!(
        page.body
            .contains("<p>t:sections.heading.default|Hello</p>"),
        "{}",
        page.body
    );
}
