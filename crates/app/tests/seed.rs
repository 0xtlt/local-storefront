//! The seed of a shop from the store data and the app: the records of each family.

mod support;

use lsf_app::admin::domains::{app, catalog, content, customers, orders};
use lsf_app::admin::state::{Record, ShopState, gid, gid_type, parse_gid};
use lsf_core::Store;
use lsf_core::util::stable_id;
use serde_json::{Value, json};

fn record<'s>(state: &'s ShopState, gid: &str) -> &'s Record {
    state.get(gid).unwrap_or_else(|| panic!("no record {gid}"))
}

fn strings(value: &Value) -> Vec<&str> {
    value
        .as_array()
        .map(|items| items.iter().filter_map(Value::as_str).collect())
        .unwrap_or_default()
}

fn demo() -> (Store, ShopState) {
    let store = support::demo_store();
    let state = ShopState::seeded(&store, None);
    (store, state)
}

fn rich() -> (Store, lsf_app::config::App, ShopState) {
    let store = support::fixture_store("rich");
    let app = support::fixture_app();
    let state = ShopState::seeded(&store, Some(&app));
    (store, app, state)
}

#[test]
fn the_demo_store_seeds_its_catalog() {
    let (store, state) = demo();
    assert_eq!(state.count("Product"), store.products.len());

    let tee = store.product_by_handle("organic-cotton-t-shirt").unwrap();
    let product = record(&state, &catalog::product_gid(tee));
    assert_eq!(product["status"], json!("ACTIVE"));
    assert_eq!(product["handle"], json!("organic-cotton-t-shirt"));
    assert_eq!(
        product["onlineStoreUrl"],
        json!(format!(
            "https://{}/products/organic-cotton-t-shirt",
            store.shop.permanent_domain
        ))
    );
    assert_eq!(strings(&product["variants"]).len(), tee.variants.len());
    assert_eq!(
        product["priceRangeV2"]["minVariantPrice"],
        json!({"amount": lsf_app::admin::state::value::decimal(tee.price_min()), "currencyCode": "USD"})
    );
    let total: i64 = tee
        .variants
        .iter()
        .map(|variant| variant.inventory_quantity)
        .sum();
    assert_eq!(product["totalInventory"], json!(total));
    assert_eq!(product["hasOnlyDefaultVariant"], json!(false));
    // The options and their values are records.
    let option = record(&state, strings(&product["options"])[0]);
    assert_eq!(option["name"], json!(tee.options[0].name));
    assert_eq!(
        strings(&option["optionValues"]).len(),
        tee.options[0].values.len()
    );

    // A variant, its inventory item and its levels at the two locations of the demo store.
    let first = &tee.variants[0];
    let variant = record(&state, &catalog::variant_gid(first));
    assert_eq!(
        variant["price"],
        json!(lsf_app::admin::state::value::money(first.price))
    );
    assert_eq!(variant["sku"], json!(first.sku));
    assert_eq!(variant["position"], json!(1));
    assert_eq!(variant["product"], json!(catalog::product_gid(tee)));
    assert_eq!(
        variant["selectedOptions"][0]["name"],
        json!(tee.options[0].name)
    );
    let item = record(&state, variant["inventoryItem"].as_str().unwrap());
    assert_eq!(item["tracked"], json!(first.inventory_tracked));
    let levels = strings(&item["inventoryLevels"]);
    assert_eq!(levels.len(), store.locations.len());
    let available: i64 = levels
        .iter()
        .map(|level| catalog::available_of(record(&state, level)))
        .sum();
    assert_eq!(available, first.inventory_quantity);
    assert_eq!(
        variant["inventoryQuantity"],
        json!(first.inventory_quantity)
    );
    assert!(levels[0].contains("?inventory_item_id="));
    assert_eq!(parse_gid(levels[0]).unwrap().0, "InventoryLevel");

    // The images of a product are `MediaImage` records.
    let media = strings(&product["media"]);
    assert!(!media.is_empty());
    assert_eq!(record(&state, media[0])["mediaContentType"], json!("IMAGE"));
    assert_eq!(product["featuredMedia"], json!(media[0]));

    // `all`, which lsf-core adds, is not a collection of the Admin API.
    let handles: Vec<&str> = state
        .all("Collection")
        .filter_map(|collection| collection["handle"].as_str())
        .collect();
    assert!(handles.contains(&"apparel"));
    assert!(!handles.contains(&"all"));

    // Every product is published on the Online Store.
    let publication = record(&state, &catalog::online_store_publication_gid());
    assert_eq!(
        strings(&publication["products"]).len(),
        store.products.len()
    );

    // Selling plans: the group of the socks, monthly and quarterly.
    let group = state.all("SellingPlanGroup").next().unwrap();
    assert_eq!(group["name"], json!("Subscribe and save"));
    let socks = store.product_by_handle("merino-crew-socks").unwrap();
    assert_eq!(strings(&group["products"]), [catalog::product_gid(socks)]);
    let plans: Vec<&Record> = strings(&group["sellingPlans"])
        .into_iter()
        .map(|plan| record(&state, plan))
        .collect();
    assert_eq!(plans[0]["billingPolicy"]["interval"], json!("MONTH"));
    assert_eq!(plans[1]["billingPolicy"]["intervalCount"], json!(3));
    assert_eq!(
        plans[0]["pricingPolicies"][0]["adjustmentValue"]["percentage"],
        json!(10.0)
    );
    assert_eq!(
        record(&state, &catalog::product_gid(socks))["sellingPlanGroups"],
        json!([group["id"]])
    );

    // Gift cards.
    let card = state.all("GiftCard").next().unwrap();
    assert_eq!(card["lastCharacters"], json!("GDGC"));
    assert_eq!(
        card["balance"],
        json!({"amount": "32.5", "currencyCode": "USD"})
    );
    let jane = store.customer_by_email("jane.doe@example.com").unwrap();
    assert_eq!(card["customer"], json!(customers::customer_gid(jane)));
    let expired = state.all("GiftCard").nth(1).unwrap();
    assert_eq!(expired["expiresOn"], json!("2020-12-31"));
    assert_eq!(expired["isRedeemable"], json!(false));
}

#[test]
fn the_demo_store_seeds_its_customers_and_orders() {
    let (store, state) = demo();
    let jane = store.customer_by_email("jane.doe@example.com").unwrap();
    let customer = record(&state, &customers::customer_gid(jane));
    assert_eq!(customer["email"], json!("jane.doe@example.com"));
    assert_eq!(customer["displayName"], json!("Jane Doe"));
    assert_eq!(customer["numberOfOrders"], json!("2"));
    assert_eq!(customer["state"], json!("ENABLED"));
    assert_eq!(
        customer["defaultAddress"]["id"],
        json!(format!(
            "gid://shopify/MailingAddress/{}?model_name=CustomerAddress",
            jane.addresses[0].id
        ))
    );
    let order_ids = strings(&customer["orders"]);
    assert_eq!(order_ids.len(), 2);
    // What she spent is what her orders cost, as the storefront counts it.
    let spent: f64 = order_ids
        .iter()
        .map(|order| {
            record(&state, order)["totalPriceSet"]["shopMoney"]["amount"]
                .as_str()
                .unwrap()
                .parse::<f64>()
                .unwrap()
        })
        .sum();
    assert_eq!(
        customer["amountSpent"]["amount"]
            .as_str()
            .unwrap()
            .parse::<f64>()
            .unwrap(),
        spent
    );

    let first = &jane.orders[0];
    let order = record(&state, &orders::order_gid(first));
    assert_eq!(order["name"], json!(first.name));
    assert_eq!(order["number"], json!(first.order_number));
    assert_eq!(order["customer"], json!(customers::customer_gid(jane)));
    assert_eq!(order["displayFinancialStatus"], json!("PAID"));
    assert_eq!(order["displayFulfillmentStatus"], json!("FULFILLED"));
    let lines = strings(&order["lineItems"]);
    assert_eq!(lines.len(), first.line_items.len());
    let line = record(&state, lines[0]);
    let (product, variant) = first.line_items[0].variant;
    let product = &store.products[product];
    assert_eq!(
        line["variant"],
        json!(catalog::variant_gid(&product.variants[variant]))
    );
    assert_eq!(line["product"], json!(catalog::product_gid(product)));
    assert_eq!(line["quantity"], json!(first.line_items[0].quantity));
    // A fulfilled, paid order has its fulfillment, its closed fulfillment order and its sale.
    let fulfillment = record(&state, strings(&order["fulfillments"])[0]);
    assert_eq!(fulfillment["status"], json!("SUCCESS"));
    let fulfillment_order = record(&state, strings(&order["fulfillmentOrders"])[0]);
    assert_eq!(fulfillment_order["status"], json!("CLOSED"));
    let sale = record(&state, strings(&order["transactions"])[0]);
    assert_eq!(sale["kind"], json!("SALE"));
    assert_eq!(sale["amountSet"], order["totalPriceSet"]);

    // B2B: the company, its locations, its contact and their role.
    let company = state.all("Company").next().unwrap();
    assert_eq!(company["name"], json!("Northwind Hotels"));
    assert_eq!(strings(&company["locations"]).len(), 2);
    let alex = store.customer_by_email("alex.morgan@example.com").unwrap();
    let contact = record(&state, &customers::company_contact_gid(alex));
    assert_eq!(contact["customer"], json!(customers::customer_gid(alex)));
    assert_eq!(contact["isMainContact"], json!(true));
    assert_eq!(company["mainContact"], contact["id"]);
    let assignment = record(&state, strings(&contact["roleAssignments"])[0]);
    assert_eq!(
        record(&state, assignment["role"].as_str().unwrap())["name"],
        json!("Ordering only")
    );
    assert_eq!(
        record(&state, &customers::customer_gid(alex))["companyContactProfiles"],
        json!([contact["id"]])
    );
}

#[test]
fn the_demo_store_seeds_its_content() {
    let (store, state) = demo();
    let shop = state.shop().unwrap();
    assert_eq!(shop["id"], json!(gid("Shop", store.shop.id)));
    assert_eq!(shop["name"], json!("Local Supply Co."));
    assert_eq!(shop["myshopifyDomain"], json!(store.shop.permanent_domain));
    assert_eq!(state.domain(), store.shop.permanent_domain);
    assert_eq!(shop["currencyCode"], json!("USD"));
    assert_eq!(shop["ianaTimezone"], json!("America/New_York"));
    assert!(matches!(
        shop["timezoneAbbreviation"].as_str(),
        Some("EST" | "EDT")
    ));
    assert_eq!(shop["enabledPresentmentCurrencies"], json!(["USD"]));
    assert_eq!(strings(&shop["shopPolicies"]).len(), 4);
    let domain = record(&state, shop["primaryDomain"].as_str().unwrap());
    assert_eq!(domain["host"], json!(store.shop.permanent_domain));

    // The shop's metafield, its definition inferred.
    let metafield = record(&state, strings(&shop["metafields"])[0]);
    assert_eq!(metafield["namespace"], json!("custom"));
    assert_eq!(metafield["key"], json!("announcement"));
    assert_eq!(metafield["type"], json!("single_line_text_field"));
    assert_eq!(
        metafield["value"],
        json!("Free shipping on orders over $75")
    );
    assert_eq!(metafield["owner"], shop["id"]);
    assert_eq!(metafield["ownerType"], json!("SHOP"));
    let definition = record(&state, metafield["definition"].as_str().unwrap());
    assert_eq!(definition["ownerType"], json!("SHOP"));
    assert_eq!(definition["type"]["name"], json!("single_line_text_field"));
    assert_eq!(definition["type"]["category"], json!("TEXT"));
    assert_eq!(definition["metafields"], json!([metafield["id"]]));

    assert_eq!(state.count("Page"), store.pages.len());
    let blog = state.all("Blog").next().unwrap();
    let article = record(&state, strings(&blog["articles"])[0]);
    assert_eq!(article["blog"], blog["id"]);
    assert_eq!(state.count("Comment"), 1);

    let menu = record(&state, &content::menu_gid("main-menu"));
    assert_eq!(menu["isDefault"], json!(true));
    let shop_link = &menu["items"][1];
    assert_eq!(shop_link["type"], json!("CATALOG"));
    let apparel = &shop_link["items"][0];
    assert_eq!(apparel["type"], json!("COLLECTION"));
    assert_eq!(
        apparel["resourceId"],
        json!(catalog::collection_gid(
            store.collection_by_handle("apparel").unwrap()
        ))
    );

    // The demo store declares no market: each country is one, as the checkout sees it.
    let markets: Vec<&str> = state.keys("Market").collect();
    assert_eq!(markets.len(), 3);
    assert_eq!(markets[0], gid("Market", stable_id("market", "US")));
    assert_eq!(record(&state, markets[1])["handle"], json!("canada"));

    let english = state.get("en").unwrap();
    assert_eq!(english["__typename"], json!("ShopLocale"));
    assert_eq!(english["primary"], json!(true));
}

#[test]
fn without_an_app_the_installation_has_every_scope() {
    let (_, state) = demo();
    let installation = state.all("AppInstallation").next().unwrap();
    let scopes = installation["accessScopes"].as_array().unwrap();
    assert_eq!(scopes.len(), app::ALL_SCOPES.len());
    assert!(scopes.contains(&json!({"handle": "write_products"})));
    let local = record(&state, installation["app"].as_str().unwrap());
    assert_eq!(local["apiKey"], json!(app::LOCAL_CLIENT_ID));
    assert_eq!(state.count("ShopifyFunction"), 0);
}

#[test]
fn a_shop_the_app_was_just_installed_on_has_no_owner_of_its_functions() {
    // The fixture app's own store data: settings for its Functions, no owner (`enabled`).
    let (store, _) = lsf_core::store::load::load_with_overlay(
        &lsf_core::store::load::DataSource::Demo,
        &Default::default(),
        Some(&json!({
            "checkout": {"functions": {"payment-rules": {"metafields": {"$app:config.settings": {"type": "json", "value": {"hideName": "Card"}}}}}}
        })),
    );
    let app = support::fixture_app();
    let state = ShopState::seeded(&store, Some(&app));
    for type_name in [
        "PaymentCustomization",
        "DeliveryCustomization",
        "Validation",
        "CartTransform",
    ] {
        assert_eq!(state.count(type_name), 0, "{type_name}");
    }
    // The Functions are there, named as `shopify.extension.toml` names them.
    let payment = state
        .all("ShopifyFunction")
        .find(|function| function["handle"] == json!("payment-rules"))
        .unwrap();
    assert_eq!(payment["title"], json!("Payment rules"));
}

#[test]
fn the_ids_are_those_the_checkout_gives() {
    let (store, app, state) = rich();
    // The owners of discount Functions: their position in `checkout.discounts`.
    let automatic = record(&state, "gid://shopify/DiscountAutomaticNode/1");
    assert_eq!(automatic["automaticDiscount"]["title"], json!("Volume"));
    let code = record(&state, "gid://shopify/DiscountCodeNode/2");
    assert_eq!(code["codeDiscount"]["codes"][0]["code"], json!("WELCOME10"));
    // The other owners: the position of their Function in the app.
    let position = |handle: &str| {
        app.functions
            .iter()
            .position(|function| function.handle == handle)
            .unwrap() as u64
            + 1
    };
    for (type_name, handle) in [
        ("PaymentCustomization", "payment-rules"),
        ("DeliveryCustomization", "delivery-rules"),
        ("Validation", "order-limits"),
        ("CartTransform", "bundles"),
    ] {
        let owner = record(&state, &gid(type_name, position(handle)));
        assert_eq!(owner["__typename"], json!(type_name));
    }
    // A buyer's contact: the customer's id.
    let buyer = store.customer_by_email("buyer@acme.example").unwrap();
    assert!(state.contains(&gid("CompanyContact", buyer.id)));
    // Markets: the handle of a declared one, the upper-case code of a country of its own.
    assert!(state.contains(&gid("Market", stable_id("market", "europe"))));
    assert!(state.contains(&gid("Market", stable_id("market", "GB"))));
    // Metaobjects: what lsf-core resolves a `gid://shopify/Metaobject/…` to.
    let ines = content::metaobject_gid("designer", "ines");
    assert_eq!(
        store
            .metaobject_by_reference(&ines, None)
            .map(|entry| entry.handle.as_str()),
        Some("ines")
    );
    // Selling plans, variants, products: lsf-core's ids.
    let plan = &store.selling_plan_groups[0].plans[0];
    assert!(state.contains(&gid("SellingPlan", plan.id)));
}

#[test]
fn the_rich_fixture_seeds_metafields_metaobjects_and_files() {
    let (store, _, state) = rich();
    let dress = store.product_by_handle("linen-dress").unwrap();
    let product = record(&state, &catalog::product_gid(dress));
    let metafield = |key: &str| {
        strings(&product["metafields"])
            .into_iter()
            .map(|id| record(&state, id))
            .find(|metafield| metafield["key"] == json!(key))
            .unwrap_or_else(|| panic!("no metafield {key}"))
    };
    let coffee = catalog::product_gid(store.product_by_handle("coffee-subscription").unwrap());
    let pairs = metafield("pairs_with");
    assert_eq!(pairs["value"], json!(coffee));
    assert_eq!(pairs["reference"], json!(coffee));
    let related = metafield("related");
    assert_eq!(related["references"].as_array().unwrap().len(), 2);
    assert_eq!(related["jsonValue"][0], json!(coffee));
    assert_eq!(
        related["value"].as_str().unwrap(),
        related["jsonValue"].to_string()
    );
    let guide = metafield("size_guide");
    let file = record(&state, guide["reference"].as_str().unwrap());
    assert_eq!(file["__typename"], json!("GenericFile"));
    assert_eq!(file["alt"], json!("Size guide"));
    assert_eq!(file["mimeType"], json!("application/pdf"));
    let price = metafield("price_hint");
    assert_eq!(
        price["jsonValue"],
        json!({"amount": "80.00", "currency_code": "EUR"})
    );
    assert_eq!(metafield("specs")["type"], json!("json"));
    // The reserved namespace of the app, as Shopify writes it.
    let hidden = metafield("hidden");
    assert_eq!(
        hidden["namespace"],
        json!(format!(
            "app--{}--config",
            app::app_id(Some(&support::fixture_app()))
        ))
    );
    assert_eq!(hidden["jsonValue"], json!(false));
    assert_eq!(hidden["definition"], Value::Null);
    let shop = state.shop().unwrap();
    let founded = record(&state, strings(&shop["metafields"])[0]);
    assert_eq!(
        (founded["value"].clone(), founded["jsonValue"].clone()),
        (json!("2019"), json!(2019))
    );

    // Metaobjects and their definitions, inferred from their fields.
    let ines = record(&state, &content::metaobject_gid("designer", "ines"));
    assert_eq!(ines["displayName"], json!("Inès"));
    let portrait = ines["fields"]
        .as_array()
        .unwrap()
        .iter()
        .find(|field| field["key"] == json!("portrait"))
        .unwrap();
    assert_eq!(
        record(&state, portrait["reference"].as_str().unwrap())["__typename"],
        json!("MediaImage")
    );
    let definition = record(&state, ines["definition"].as_str().unwrap());
    assert_eq!(definition["type"], json!("designer"));
    assert_eq!(definition["displayNameKey"], json!("name"));
    let keys: Vec<&Value> = definition["fieldDefinitions"]
        .as_array()
        .unwrap()
        .iter()
        .map(|field| &field["key"])
        .collect();
    assert_eq!(
        keys,
        [
            &json!("name"),
            &json!("bio"),
            &json!("portrait"),
            &json!("signature_piece"),
            &json!("years")
        ]
    );
    assert_eq!(definition["metaobjectsCount"], json!(2));
    // Its web pages are at the URL handle of the data, else at the type.
    assert_eq!(
        definition["capabilities"]["onlineStore"]["data"]["urlHandle"],
        json!("designers")
    );
    let faq_definition = record(&state, &content::metaobject_definition_gid("faq"));
    assert_eq!(
        faq_definition["capabilities"]["onlineStore"]["data"]["urlHandle"],
        json!("faq")
    );
    let faq = record(&state, &content::metaobject_gid("faq", "returns"));
    assert_eq!(faq["fields"][2]["references"].as_array().unwrap().len(), 2);

    // Files: those the data describes, and those something refers to (a swatch image).
    assert_eq!(state.count("GenericFile"), 1);
    let floral = catalog::file_gid("swatches/floral.png");
    assert_eq!(record(&state, &floral)["__typename"], json!("MediaImage"));
    let clip = record(&state, &catalog::file_gid("lookbook/clip.mp4"));
    assert_eq!(clip["sources"][0]["width"], json!(1280));

    // Product media of every kind.
    let kinds: Vec<&str> = strings(&product["media"])
        .into_iter()
        .map(|id| gid_type(id).unwrap())
        .collect();
    assert_eq!(kinds, ["MediaImage", "Video", "ExternalVideo", "Model3d"]);
    let youtube = record(&state, strings(&product["media"])[2]);
    assert_eq!(
        youtube["embedUrl"],
        json!("https://www.youtube.com/embed/dQw4w9WgXcQ")
    );
}

#[test]
fn the_rich_fixture_seeds_b2b_gift_cards_and_selling_plans() {
    let (store, _, state) = rich();
    let company = state.all("Company").next().unwrap();
    assert_eq!(company["externalId"], json!("ACME-1"));
    assert_eq!(
        company["totalSpent"],
        json!({"amount": "1250.0", "currencyCode": "EUR"})
    );
    let lyon = record(&state, strings(&company["locations"])[0]);
    assert_eq!(
        lyon["ordersCount"],
        json!({"count": 4, "precision": "EXACT"})
    );
    assert_eq!(lyon["taxSettings"]["taxRegistrationId"], json!("FR123"));
    assert_eq!(lyon["shippingAddress"]["companyName"], json!("Acme Retail"));
    assert_eq!(strings(&lyon["roleAssignments"]).len(), 1);
    let berlin = record(&state, strings(&company["locations"])[1]);
    assert_eq!(berlin["locale"], json!("de"));
    assert_eq!(strings(&berlin["roleAssignments"]).len(), 0);
    let buyer = store.customer_by_email("buyer@acme.example").unwrap();
    let contact = record(&state, &customers::company_contact_gid(buyer));
    assert_eq!(contact["title"], json!("Purchasing manager"));

    let card = state.all("GiftCard").next().unwrap();
    assert_eq!(card["recipientAttributes"]["preferredName"], json!("Zo"));
    assert_eq!(card["recipientAttributes"]["message"], json!("Enjoy!"));
    let disabled = state.all("GiftCard").nth(1).unwrap();
    assert_eq!(
        disabled["initialValue"],
        json!({"amount": "25.0", "currencyCode": "GBP"})
    );
    assert_eq!(disabled["isRedeemable"], json!(false));

    let plans: Vec<&Record> = state.all("SellingPlan").collect();
    let subscription = plans[0];
    assert_eq!(subscription["category"], json!("SUBSCRIPTION"));
    assert_eq!(subscription["billingPolicy"]["interval"], json!("WEEK"));
    assert_eq!(subscription["billingPolicy"]["intervalCount"], json!(2));
    let recurring = &subscription["pricingPolicies"][1];
    assert_eq!(
        recurring["__typename"],
        json!("SellingPlanRecurringPricingPolicy")
    );
    assert_eq!(recurring["afterCycle"], json!(3));
    assert_eq!(
        recurring["adjustmentValue"],
        json!({"__typename": "MoneyV2", "amount": "15.0", "currencyCode": "EUR"})
    );
    let preorder = plans[1];
    assert_eq!(preorder["category"], json!("PRE_ORDER"));
    assert_eq!(
        preorder["billingPolicy"]["__typename"],
        json!("SellingPlanFixedBillingPolicy")
    );
    assert_eq!(
        preorder["billingPolicy"]["checkoutCharge"]["value"]["percentage"],
        json!(20.0)
    );
    assert_eq!(
        preorder["billingPolicy"]["remainingBalanceChargeTrigger"],
        json!("ON_FULFILLMENT")
    );

    // The stock of the dress spreads over the two locations that have it in stock; the boots,
    // oversold, are at the first one.
    let dress = store.product_by_handle("linen-dress").unwrap();
    let item = record(&state, &catalog::inventory_item_gid(dress.variants[0].id));
    let quantities: Vec<i64> = strings(&item["inventoryLevels"])
        .into_iter()
        .map(|level| catalog::available_of(record(&state, level)))
        .collect();
    assert_eq!(quantities, [4, 3]);
    let boots = store.product_by_handle("preorder-boots").unwrap();
    let product = record(&state, &catalog::product_gid(boots));
    assert_eq!(product["totalInventory"], json!(-3));
    assert_eq!(product["hasOutOfStockVariants"], json!(true));
    assert_eq!(product["hasOnlyDefaultVariant"], json!(true));
}

#[test]
fn the_orders_of_the_rich_fixture_follow_their_state() {
    let (store, _, state) = rich();
    let zoe = store.customer_by_email("zoe@example.com").unwrap();
    let order = |index: usize| record(&state, &orders::order_gid(&zoe.orders[index]));
    let kinds = |order: &Record| -> Vec<String> {
        strings(&order["transactions"])
            .into_iter()
            .map(|id| record(&state, id)["kind"].as_str().unwrap().to_string())
            .collect()
    };

    // Paid, fulfilled, with a closed return of one dress.
    let first = order(0);
    assert_eq!(
        first["subtotalPriceSet"]["shopMoney"]["amount"],
        json!("196.0")
    );
    assert_eq!(
        first["totalPriceSet"]["shopMoney"]["amount"],
        json!("231.0")
    );
    assert_eq!(first["shippingLine"]["title"], json!("Colissimo"));
    assert_eq!(kinds(first), ["SALE"]);
    assert_eq!(first["returnStatus"], json!("RETURNED"));
    let sent_back = record(&state, strings(&first["returns"])[0]);
    assert_eq!(sent_back["status"], json!("CLOSED"));
    let line = record(&state, strings(&sent_back["returnLineItems"])[0]);
    assert_eq!(line["__typename"], json!("ReturnLineItem"));
    assert_eq!(line["quantity"], json!(1));
    let fulfilled = record(&state, line["fulfillmentLineItem"].as_str().unwrap());
    assert_eq!(
        fulfilled["lineItem"],
        json!(strings(&first["lineItems"])[0])
    );

    // Refunded, half fulfilled.
    let second = order(1);
    assert_eq!(second["displayFinancialStatus"], json!("REFUNDED"));
    assert_eq!(
        second["displayFulfillmentStatus"],
        json!("PARTIALLY_FULFILLED")
    );
    assert_eq!(kinds(second), ["SALE", "REFUND"]);
    assert_eq!(second["totalRefundedSet"], second["totalPriceSet"]);
    let line = record(&state, strings(&second["lineItems"])[0]);
    assert_eq!(line["unfulfilledQuantity"], json!(2));
    assert_eq!(second["note"], json!("Leave at the door"));

    // Cancelled, unpaid, with a return requested of a line never fulfilled.
    let third = order(2);
    assert_eq!(third["cancelledAt"], third["createdAt"]);
    assert_eq!(third["displayFinancialStatus"], json!("PENDING"));
    assert!(kinds(third).is_empty());
    assert_eq!(
        record(&state, strings(&third["fulfillmentOrders"])[0])["status"],
        json!("CANCELLED")
    );
    let requested = record(&state, strings(&third["returns"])[0]);
    let line = record(&state, strings(&requested["returnLineItems"])[0]);
    assert_eq!(line["__typename"], json!("UnverifiedReturnLineItem"));

    // Authorized: capturable, nothing received.
    let fourth = order(3);
    assert_eq!(kinds(fourth), ["AUTHORIZATION"]);
    assert_eq!(fourth["capturable"], json!(true));
    assert_eq!(
        fourth["totalReceivedSet"]["shopMoney"]["amount"],
        json!("0.0")
    );

    // The customer counts every order, and spends what those not cancelled cost.
    let customer = record(&state, &customers::customer_gid(zoe));
    assert_eq!(customer["numberOfOrders"], json!("4"));
    assert_eq!(customer["lastOrder"], fourth["id"]);
    assert_eq!(
        customer["emailMarketingConsent"]["marketingState"],
        json!("SUBSCRIBED")
    );
}

#[test]
fn the_app_owns_its_functions_and_their_owners() {
    let (_, app, state) = rich();
    let installation = state.all("AppInstallation").next().unwrap();
    let scopes: Vec<&str> = installation["accessScopes"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|scope| scope["handle"].as_str())
        .collect();
    assert_eq!(
        scopes,
        app.config
            .scopes
            .iter()
            .map(String::as_str)
            .collect::<Vec<_>>()
    );
    let app_record = record(&state, installation["app"].as_str().unwrap());
    assert_eq!(
        app_record["apiKey"],
        json!("11111111111111111111111111111111")
    );
    assert_eq!(app_record["installation"], installation["id"]);

    // A Function is kept under its id, a UUID.
    let id = app::function_id(Some(&app), "volume-discount");
    let function = record(&state, &id);
    assert_eq!(function["id"], json!(id));
    assert_eq!(function["apiType"], json!("discounts"));
    assert_eq!(function["app"], app_record["id"]);
    let payment = state.all("PaymentCustomization").next().unwrap();
    assert_eq!(
        payment["functionId"],
        json!(app::function_id(Some(&app), "payment-rules"))
    );
    // As the data declares them: enabled, or not.
    assert_eq!(payment["enabled"], json!(true));
    let delivery = state.all("DeliveryCustomization").next().unwrap();
    assert_eq!(delivery["enabled"], json!(false));
    assert_eq!(payment["shopifyFunction"], payment["functionId"]);
    let rules = record(&state, strings(&payment["metafields"])[0]);
    assert_eq!(rules["jsonValue"], json!({"hide": ["cod"]}));
    assert_eq!(rules["ownerType"], json!("PAYMENT_CUSTOMIZATION"));
    let limits = record(
        &state,
        strings(&state.all("Validation").next().unwrap()["metafields"])[0],
    );
    assert_eq!(
        (limits["type"].clone(), limits["value"].clone()),
        (json!("number_integer"), json!("5"))
    );

    // The discounts run the Function, with the metafields of the data.
    let automatic = record(&state, "gid://shopify/DiscountAutomaticNode/1");
    let discount = &automatic["automaticDiscount"];
    assert_eq!(discount["__typename"], json!("DiscountAutomaticApp"));
    assert_eq!(discount["appDiscountType"]["functionId"], json!(id));
    assert_eq!(discount["discountClasses"], json!(["PRODUCT", "ORDER"]));
    let settings = record(&state, strings(&automatic["metafields"])[0]);
    assert_eq!(settings["key"], json!("settings"));
    assert_eq!(settings["owner"], automatic["id"]);
    assert_eq!(settings["ownerType"], json!("DISCOUNT"));
    let code = record(&state, "gid://shopify/DiscountCodeNode/2");
    assert_eq!(
        code["codeDiscount"]["discountClasses"],
        json!(["PRODUCT", "ORDER", "SHIPPING"])
    );
    let featured = record(&state, strings(&code["metafields"])[0]);
    assert_eq!(
        featured["reference"].as_str().map(gid_type),
        Some(Some("Product"))
    );
}

#[test]
fn the_same_data_gives_the_same_records() {
    let (store, app, state) = rich();
    let again = ShopState::seeded(&store, Some(&app));
    let records = |state: &ShopState| -> Vec<(String, Record)> {
        state
            .iter()
            .map(|(key, record)| (key.to_string(), record.clone()))
            .collect()
    };
    assert_eq!(records(&state), records(&again));
    // The frozen clock of the data is the clock of the shop.
    assert_eq!(state.timestamp(), "2026-03-15T10:00:00Z");
}

#[test]
fn products_off_the_storefront_are_listed_with_their_status() {
    let overlay = json!({
        "products": [
            {"title": "Draft dress", "price": 100, "status": "draft"},
            {"title": "Hidden mug", "price": 100, "published": false}
        ],
        "checkout": {"discounts": [{"function": "volume-discount", "id": 9}]}
    });
    let (store, diagnostics) = lsf_core::store::load::load_with_overlay(
        &lsf_core::store::load::DataSource::Demo,
        &lsf_core::store::load::LoadOptions::default(),
        Some(&overlay),
    );
    assert!(!diagnostics.has_errors(), "{diagnostics}");
    let state = ShopState::seeded(&store, None);
    let draft = record(&state, &catalog::product_gid(&store.unlisted_products[0]));
    assert_eq!(draft["status"], json!("DRAFT"));
    assert!(draft["publishedAt"].is_string());
    assert_eq!(draft["onlineStoreUrl"], Value::Null);
    let hidden = record(&state, &catalog::product_gid(&store.unlisted_products[1]));
    assert_eq!(hidden["status"], json!("ACTIVE"));
    assert_eq!(hidden["publishedAt"], Value::Null);
    let publication = record(&state, &catalog::online_store_publication_gid());
    let published = strings(&publication["products"]);
    assert!(published.contains(&draft["id"].as_str().unwrap_or_default()));
    assert!(!published.contains(&hidden["id"].as_str().unwrap_or_default()));
    assert_eq!(state.count("Product"), store.products.len() + 2);
    // A discount with an id of its own has it in the Admin API.
    assert!(state.contains("gid://shopify/DiscountAutomaticNode/9"));
}
