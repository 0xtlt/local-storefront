//! The catalog's mutations, through queries over the demo store: what each changes, what it
//! answers, the events it emits, and that a failure changes nothing.

use serde_json::{Value, json};

use crate::graphql::{self, Request};
use crate::resolve::{Registry, ShopResolver};
use crate::state::ShopState;

pub fn demo() -> ShopState {
    let (store, _) = lsf_core::store::load::load(
        &lsf_core::store::load::DataSource::Demo,
        &Default::default(),
    );
    ShopState::seeded(&store, None)
}

/// The answer to a query, `extensions` without `cost`.
pub fn run(shop: &mut ShopState, query: &str) -> Value {
    run_with(shop, query, Value::Null)
}

/// The answer to a query with variables.
pub fn run_with(shop: &mut ShopState, query: &str, variables: Value) -> Value {
    let mut resolver = ShopResolver::new(shop, Registry::standard());
    let mut request = Request::new(query);
    if let Value::Object(variables) = variables {
        request.variables = variables;
    }
    let mut answer = graphql::run(crate::schema::admin(), &mut resolver, &request, None);
    answer.extensions.extend(resolver.extensions());
    answer.extensions.remove("cost");
    answer.to_json()
}

/// The topics emitted since the last call.
pub fn topics(shop: &mut ShopState) -> Vec<String> {
    shop.take_events()
        .into_iter()
        .map(|event| event.topic)
        .collect()
}

/// The GID of the product of a handle.
pub fn product(shop: &ShopState, handle: &str) -> String {
    shop.keys("Product")
        .find(|key| shop.get(key).unwrap()["handle"] == json!(handle))
        .unwrap_or_else(|| panic!("no product {handle}"))
        .to_string()
}

/// The variants of a product.
pub fn variants(shop: &ShopState, product: &str) -> Vec<String> {
    super::gids(shop.get(product).unwrap().get("variants"))
}

/// The payload of a mutation, its user errors checked to be none.
pub fn ok<'a>(answer: &'a Value, name: &str) -> &'a Value {
    let payload = &answer["data"][name];
    assert!(
        answer.get("errors").is_none(),
        "{name} answered errors: {answer}"
    );
    let errors = payload
        .get("userErrors")
        .or_else(|| payload.get("mediaUserErrors"))
        .cloned()
        .unwrap_or(json!([]));
    assert_eq!(errors, json!([]), "{name}: {answer}");
    payload
}

/// That a failed mutation changed nothing: no event, and the records it names as they were.
pub fn unchanged(shop: &mut ShopState, before: &ShopState, keys: &[&str]) {
    assert!(shop.events().is_empty(), "events: {:?}", shop.events());
    for key in keys {
        assert_eq!(shop.get(key), before.get(key), "{key} changed");
    }
}

#[test]
fn bulk_creates_variants_with_their_values_and_inventory() {
    let mut shop = demo();
    let mug = product(&shop, "ceramic-mug");
    let location = shop.keys("Location").next().unwrap().to_string();
    let answer = run_with(
        &mut shop,
        r#"mutation($id: ID!, $variants: [ProductVariantsBulkInput!]!) {
            productVariantsBulkCreate(productId: $id, variants: $variants) {
                productVariants { title price compareAtPrice sku selectedOptions { name value }
                    inventoryQuantity inventoryItem { tracked inventoryLevels(first: 5) { nodes { location { id } quantities(names: ["available", "on_hand"]) { name quantity } } } } }
                product { options { name values } priceRangeV2 { maxVariantPrice { amount } } totalInventory }
                userErrors { field message code }
            } }"#,
        json!({"id": mug, "variants": [{
            "price": 42.5, "compareAtPrice": "50.00",
            "optionValues": [{"optionName": "Glaze", "name": "Cobalt"}],
            "inventoryItem": {"sku": "MUG-CBT", "tracked": true},
            "inventoryQuantities": [{"availableQuantity": 7, "locationId": location}],
        }]}),
    );
    let payload = ok(&answer, "productVariantsBulkCreate");
    let variant = &payload["productVariants"][0];
    assert_eq!(variant["title"], json!("Cobalt"));
    assert_eq!(variant["price"], json!("42.50"));
    assert_eq!(variant["compareAtPrice"], json!("50.00"));
    assert_eq!(variant["sku"], json!("MUG-CBT"));
    assert_eq!(variant["inventoryQuantity"], json!(7));
    assert_eq!(variant["inventoryItem"]["tracked"], json!(true));
    let level = &variant["inventoryItem"]["inventoryLevels"]["nodes"][0];
    assert_eq!(level["location"]["id"], json!(location));
    assert_eq!(
        level["quantities"],
        json!([{"name": "available", "quantity": 7}, {"name": "on_hand", "quantity": 7}])
    );
    let options = &payload["product"]["options"][0];
    assert_eq!(
        options["values"].as_array().unwrap().last(),
        Some(&json!("Cobalt"))
    );
    assert_eq!(
        payload["product"]["priceRangeV2"]["maxVariantPrice"]["amount"],
        json!("42.5")
    );
    assert_eq!(
        topics(&mut shop),
        [
            "inventory_items/create",
            "inventory_levels/connect",
            "products/update"
        ]
    );

    // The same values again: refused, nothing changed.
    let before = shop.clone();
    let again = run_with(
        &mut shop,
        r#"mutation($id: ID!) { productVariantsBulkCreate(productId: $id, variants: [{optionValues: [{optionName: "Glaze", name: "Cobalt"}]}]) {
            productVariants { id } userErrors { field message code } } }"#,
        json!({"id": mug}),
    );
    assert_eq!(
        again["data"]["productVariantsBulkCreate"],
        json!({"productVariants": null, "userErrors": [{
            "field": ["variants", "0"], "message": "The variant 'Cobalt' already exists.", "code": "VARIANT_ALREADY_EXISTS"
        }]})
    );
    unchanged(&mut shop, &before, &[&mug]);
    // An option the product does not have, and a product that does not exist.
    let unknown = run_with(
        &mut shop,
        r#"mutation($id: ID!) { productVariantsBulkCreate(productId: $id, variants: [{optionValues: [{optionName: "Size", name: "L"}]}]) {
            userErrors { field code } } }"#,
        json!({"id": mug}),
    );
    assert_eq!(
        unknown["data"]["productVariantsBulkCreate"]["userErrors"][0]["code"],
        json!("OPTION_VALUES_FOR_NUMBER_OF_UNKNOWN_OPTIONS")
    );
    let missing = run(
        &mut shop,
        r#"mutation { productVariantsBulkCreate(productId: "gid://shopify/Product/1", variants: []) { userErrors { field message code } } }"#,
    );
    assert_eq!(
        missing["data"]["productVariantsBulkCreate"]["userErrors"],
        json!([{"field": ["productId"], "message": "Product does not exist", "code": "PRODUCT_DOES_NOT_EXIST"}])
    );
    unchanged(&mut shop, &before, &[&mug]);
}

#[test]
fn bulk_create_replaces_the_standalone_default_variant() {
    let mut shop = demo();
    let created = run(
        &mut shop,
        r#"mutation { productCreate(product: {title: "Poster"}) { product { id } } }"#,
    );
    let poster = created["data"]["productCreate"]["product"]["id"]
        .as_str()
        .unwrap()
        .to_string();
    shop.take_events();
    let default = variants(&shop, &poster)[0].clone();
    let answer = run_with(
        &mut shop,
        r#"mutation($id: ID!) { productVariantsBulkCreate(productId: $id, variants: [
            {optionValues: [{optionName: "Title", name: "A3"}], price: "10"},
            {optionValues: [{optionName: "Title", name: "A2"}], price: "15"}]) {
            product { hasOnlyDefaultVariant variants(first: 5) { nodes { title position } } } userErrors { message } } }"#,
        json!({"id": poster}),
    );
    let product = &ok(&answer, "productVariantsBulkCreate")["product"];
    assert_eq!(product["hasOnlyDefaultVariant"], json!(false));
    assert_eq!(
        product["variants"]["nodes"],
        json!([{"title": "A3", "position": 1}, {"title": "A2", "position": 2}])
    );
    assert!(!shop.contains(&default));
    let topics = topics(&mut shop);
    assert!(
        topics.contains(&"inventory_items/delete".to_string()),
        "{topics:?}"
    );
}

#[test]
fn bulk_updates_prices_inventory_and_options() {
    let mut shop = demo();
    let beanie = product(&shop, "wool-beanie");
    let [navy, ochre] = <[String; 2]>::try_from(variants(&shop, &beanie)).unwrap();
    let location = shop.keys("Location").next().unwrap().to_string();
    let before_quantity = shop.get(&navy).unwrap()["inventoryQuantity"]
        .as_i64()
        .unwrap();
    let answer = run_with(
        &mut shop,
        r#"mutation($id: ID!, $variants: [ProductVariantsBulkInput!]!) {
            productVariantsBulkUpdate(productId: $id, variants: $variants) {
                productVariants { id price inventoryPolicy sku inventoryQuantity }
                product { priceRangeV2 { minVariantPrice { amount } } }
                userErrors { field message code }
            } }"#,
        json!({"id": beanie, "variants": [
            {"id": navy, "price": "19.99", "inventoryPolicy": "CONTINUE", "inventoryItem": {"sku": "BNE-NAVY-2"},
             "quantityAdjustments": [{"locationId": location, "adjustment": 3, "changeFromQuantity": null}]},
        ]}),
    );
    let payload = ok(&answer, "productVariantsBulkUpdate");
    let variant = &payload["productVariants"][0];
    assert_eq!(variant["price"], json!("19.99"));
    assert_eq!(variant["inventoryPolicy"], json!("CONTINUE"));
    assert_eq!(variant["sku"], json!("BNE-NAVY-2"));
    assert_eq!(variant["inventoryQuantity"], json!(before_quantity + 3));
    assert_eq!(
        payload["product"]["priceRangeV2"]["minVariantPrice"]["amount"],
        json!("19.99")
    );
    assert_eq!(
        topics(&mut shop),
        [
            "inventory_items/update",
            "inventory_levels/update",
            "products/update"
        ]
    );

    // `inventoryQuantities` are for creating; a variant of another product.
    let before = shop.clone();
    let mug_variant = variants(&shop, &product(&shop, "ceramic-mug"))[0].clone();
    let refused = run_with(
        &mut shop,
        r#"mutation($id: ID!, $variants: [ProductVariantsBulkInput!]!) {
            productVariantsBulkUpdate(productId: $id, variants: $variants) { productVariants { id } userErrors { field code } } }"#,
        json!({"id": beanie, "variants": [
            {"id": ochre, "price": "5", "inventoryQuantities": [{"availableQuantity": 1, "locationId": location}]},
            {"id": mug_variant, "price": "-1"},
        ]}),
    );
    assert_eq!(
        refused["data"]["productVariantsBulkUpdate"]["userErrors"],
        json!([
            {"field": ["variants", "0", "inventoryQuantities"], "code": "NO_INVENTORY_QUANTITIES_ON_VARIANTS_UPDATE"},
            {"field": ["variants", "1", "id"], "code": "MUST_BE_FOR_THIS_PRODUCT"},
            {"field": ["variants", "1", "price"], "code": "GREATER_THAN_OR_EQUAL_TO"},
        ])
    );
    unchanged(&mut shop, &before, &[&beanie, &ochre]);

    // With `allowPartialUpdates`, the valid variant is updated and the other reported.
    let partial = run_with(
        &mut shop,
        r#"mutation($id: ID!, $variants: [ProductVariantsBulkInput!]!) {
            productVariantsBulkUpdate(productId: $id, variants: $variants, allowPartialUpdates: true) {
                productVariants { price } userErrors { field code } } }"#,
        json!({"id": beanie, "variants": [{"id": ochre, "price": "21"}, {"id": mug_variant, "price": "1"}]}),
    );
    assert_eq!(
        partial["data"]["productVariantsBulkUpdate"]["productVariants"],
        json!([{"price": "21.00"}])
    );
    assert_eq!(shop.get(&ochre).unwrap()["price"], json!("21.00"));
}

#[test]
fn bulk_deletes_and_reorders_variants() {
    let mut shop = demo();
    let mug = product(&shop, "ceramic-mug");
    let [oat, slate, rust] = <[String; 3]>::try_from(variants(&shop, &mug)).unwrap();
    let item = shop.get(&rust).unwrap()["inventoryItem"]
        .as_str()
        .unwrap()
        .to_string();
    let reordered = run_with(
        &mut shop,
        r#"mutation($id: ID!, $positions: [ProductVariantPositionInput!]!) {
            productVariantsBulkReorder(productId: $id, positions: $positions) {
                product { variants(first: 5) { nodes { id position } } } userErrors { field message } } }"#,
        json!({"id": mug, "positions": [{"id": rust, "position": 1}]}),
    );
    let nodes = &ok(&reordered, "productVariantsBulkReorder")["product"]["variants"]["nodes"];
    assert_eq!(
        nodes,
        &json!([{"id": rust, "position": 1}, {"id": oat, "position": 2}, {"id": slate, "position": 3}])
    );
    assert_eq!(topics(&mut shop), ["products/update"]);

    let deleted = run_with(
        &mut shop,
        r#"mutation($id: ID!, $ids: [ID!]!) { productVariantsBulkDelete(productId: $id, variantsIds: $ids) {
            product { variants(first: 5) { nodes { id } } } userErrors { field message } } }"#,
        json!({"id": mug, "ids": [rust]}),
    );
    ok(&deleted, "productVariantsBulkDelete");
    assert!(!shop.contains(&rust) && !shop.contains(&item));
    assert_eq!(
        topics(&mut shop),
        ["inventory_items/delete", "products/update"]
    );

    // A variant of another product (shopify.dev's example), and every variant.
    let before = shop.clone();
    let tee_variant = variants(&shop, &product(&shop, "organic-cotton-t-shirt"))[0].clone();
    let foreign = run_with(
        &mut shop,
        r#"mutation($id: ID!, $ids: [ID!]!) { productVariantsBulkDelete(productId: $id, variantsIds: $ids) { userErrors { field message code } } }"#,
        json!({"id": mug, "ids": [oat, tee_variant]}),
    );
    assert_eq!(
        foreign["data"]["productVariantsBulkDelete"]["userErrors"],
        json!([{"field": ["variantsIds", "1"], "message": "At least one variant does not belong to the product", "code": "AT_LEAST_ONE_VARIANT_DOES_NOT_BELONG_TO_THE_PRODUCT"}])
    );
    let all = run_with(
        &mut shop,
        r#"mutation($id: ID!, $ids: [ID!]!) { productVariantsBulkDelete(productId: $id, variantsIds: $ids) { userErrors { code } } }"#,
        json!({"id": mug, "ids": [oat, slate]}),
    );
    assert_eq!(
        all["data"]["productVariantsBulkDelete"]["userErrors"],
        json!([{"code": "CANNOT_DELETE_LAST_VARIANT"}])
    );
    unchanged(&mut shop, &before, &[&mug, &oat, &slate]);
}

#[test]
fn options_are_created_updated_reordered_and_deleted() {
    let mut shop = demo();
    let beanie = product(&shop, "wool-beanie");
    let created = run_with(
        &mut shop,
        r#"mutation($id: ID!) { productOptionsCreate(productId: $id, variantStrategy: CREATE, options: [{name: "Size", values: [{name: "S"}, {name: "L"}]}]) {
            product { options { name position values } variants(first: 10) { nodes { title } } } userErrors { field message code } } }"#,
        json!({"id": beanie}),
    );
    let product = &ok(&created, "productOptionsCreate")["product"];
    assert_eq!(
        product["options"][1],
        json!({"name": "Size", "position": 2, "values": ["S", "L"]})
    );
    let titles: Vec<&str> = product["variants"]["nodes"]
        .as_array()
        .unwrap()
        .iter()
        .map(|node| node["title"].as_str().unwrap())
        .collect();
    assert_eq!(titles, ["Navy / S", "Ochre / S", "Navy / L", "Ochre / L"]);
    let events = topics(&mut shop);
    assert_eq!(events.last().map(String::as_str), Some("products/update"));
    assert_eq!(
        events
            .iter()
            .filter(|topic| *topic == "inventory_items/create")
            .count(),
        2
    );

    // An existing name, and too many options.
    let before = shop.clone();
    let taken = run_with(
        &mut shop,
        r#"mutation($id: ID!) { productOptionsCreate(productId: $id, options: [{name: "Size", values: [{name: "M"}]}, {name: "A", values: [{name: "x"}]}, {name: "B", values: [{name: "y"}]}]) {
            userErrors { field message code } } }"#,
        json!({"id": beanie}),
    );
    assert_eq!(
        taken["data"]["productOptionsCreate"]["userErrors"],
        json!([
            {"field": ["options"], "message": "Can only specify a maximum of 3 options", "code": "OPTIONS_OVER_LIMIT"},
            {"field": ["options", "0"], "message": "Option 'Size' already exists.", "code": "OPTION_ALREADY_EXISTS"},
        ])
    );
    unchanged(&mut shop, &before, &[&beanie]);

    // Rename the option and a value, add one.
    let size = super::gids(shop.get(&beanie).unwrap().get("options"))[1].clone();
    let small = super::gids(shop.get(&size).unwrap().get("optionValues"))[0].clone();
    let updated = run_with(
        &mut shop,
        r#"mutation($id: ID!, $option: OptionUpdateInput!, $update: [OptionValueUpdateInput!], $add: [OptionValueCreateInput!]) {
            productOptionUpdate(productId: $id, option: $option, optionValuesToUpdate: $update, optionValuesToAdd: $add) {
                product { options { name values } variants(first: 10) { nodes { title } } } userErrors { field message code } } }"#,
        json!({"id": beanie, "option": {"id": size, "name": "Fit"}, "update": [{"id": small, "name": "Small"}], "add": [{"name": "XL"}]}),
    );
    let product = &ok(&updated, "productOptionUpdate")["product"];
    assert_eq!(
        product["options"][1],
        json!({"name": "Fit", "values": ["Small", "L", "XL"]})
    );
    assert_eq!(
        product["variants"]["nodes"][0]["title"],
        json!("Navy / Small")
    );
    let duplicate = run_with(
        &mut shop,
        r#"mutation($id: ID!, $option: OptionUpdateInput!) {
            productOptionUpdate(productId: $id, option: $option, optionValuesToAdd: [{name: "Large"}, {name: "xl"}]) { userErrors { field message code } } }"#,
        json!({"id": beanie, "option": {"id": size}}),
    );
    assert_eq!(
        duplicate["data"]["productOptionUpdate"]["userErrors"],
        json!([{"field": ["optionValuesToAdd", "1", "name"], "message": "Option value already exists.", "code": "OPTION_VALUE_ALREADY_EXISTS"}])
    );
    shop.take_events();

    // Reorder: the options and their values, the variants follow.
    let reordered = run_with(
        &mut shop,
        r#"mutation($id: ID!) { productOptionsReorder(productId: $id, options: [{name: "Fit", values: [{name: "L"}, {name: "Small"}, {name: "XL"}]}, {name: "Color"}]) {
            product { options { name position } variants(first: 10) { nodes { title } } } userErrors { field message code } } }"#,
        json!({"id": beanie}),
    );
    let product = &ok(&reordered, "productOptionsReorder")["product"];
    assert_eq!(product["options"][0], json!({"name": "Fit", "position": 1}));
    assert_eq!(product["variants"]["nodes"][0]["title"], json!("L / Navy"));
    let missing = run_with(
        &mut shop,
        r#"mutation($id: ID!) { productOptionsReorder(productId: $id, options: [{name: "Fit", values: [{name: "L"}]}, {name: "Color"}]) {
            userErrors { field message code } } }"#,
        json!({"id": beanie}),
    );
    assert_eq!(
        missing["data"]["productOptionsReorder"]["userErrors"][0],
        json!({"field": ["options"], "message": "Missing option value 'Small'.", "code": "MISSING_OPTION_VALUE"})
    );

    // Delete: refused with several values by default, done with `POSITION`.
    let refused = run_with(
        &mut shop,
        r#"mutation($id: ID!, $options: [ID!]!) { productOptionsDelete(productId: $id, options: $options) { deletedOptionsIds userErrors { field message code } } }"#,
        json!({"id": beanie, "options": [size]}),
    );
    assert_eq!(
        refused["data"]["productOptionsDelete"]["userErrors"],
        json!([{"field": ["options"], "message": "Cannot delete an option with multiple values.", "code": "CANNOT_DELETE_OPTION_WITH_MULTIPLE_VALUES"}])
    );
    let deleted = run_with(
        &mut shop,
        r#"mutation($id: ID!, $options: [ID!]!) { productOptionsDelete(productId: $id, options: $options, strategy: POSITION) {
            deletedOptionsIds product { options { name } variants(first: 10) { nodes { title } } } userErrors { message } } }"#,
        json!({"id": beanie, "options": [size]}),
    );
    let payload = ok(&deleted, "productOptionsDelete");
    assert_eq!(payload["deletedOptionsIds"], json!([size]));
    assert_eq!(payload["product"]["options"], json!([{"name": "Color"}]));
    assert_eq!(
        payload["product"]["variants"]["nodes"],
        json!([{"title": "Navy"}, {"title": "Ochre"}])
    );
}

#[test]
fn media_is_created_from_urls_attached_reordered_and_deleted() {
    let mut shop = demo();
    let tote = product(&shop, "canvas-tote-bag");
    let created = run_with(
        &mut shop,
        r#"mutation($id: ID!) { productCreateMedia(productId: $id, media: [
            {originalSource: "https://example.com/side.jpg", alt: "Side", mediaContentType: IMAGE},
            {originalSource: "not a url", mediaContentType: IMAGE},
            {originalSource: "https://www.youtube.com/watch?v=4L8VbGRibj8", mediaContentType: EXTERNAL_VIDEO}]) {
            media { alt mediaContentType status ... on MediaImage { image { url } } ... on ExternalVideo { embedUrl } }
            mediaUserErrors { field message } } }"#,
        json!({"id": tote}),
    );
    let payload = &created["data"]["productCreateMedia"];
    // The valid media are created even when one is not (shopify.dev's example).
    assert_eq!(
        payload["mediaUserErrors"],
        json!([{"field": ["media", "1", "originalSource"], "message": "Image URL is invalid"}])
    );
    assert_eq!(
        payload["media"],
        json!([
            {"alt": "Side", "mediaContentType": "IMAGE", "status": "READY", "image": {"url": "https://example.com/side.jpg"}},
            {"alt": null, "mediaContentType": "EXTERNAL_VIDEO", "status": "READY", "embedUrl": "https://www.youtube.com/embed/4L8VbGRibj8"},
        ])
    );
    assert_eq!(topics(&mut shop), ["products/update"]);
    let media = super::gids(shop.get(&tote).unwrap().get("media"));
    let side = media[media.len() - 2].clone();
    let video = media.last().unwrap().clone();
    let variant = variants(&shop, &tote)[0].clone();

    let detached = run_with(
        &mut shop,
        r#"mutation($id: ID!, $pairs: [ProductVariantDetachMediaInput!]!) { productVariantDetachMedia(productId: $id, variantMedia: $pairs) { userErrors { field code } } }"#,
        json!({"id": tote, "pairs": [{"variantId": variant, "mediaIds": [side]}]}),
    );
    assert_eq!(
        detached["data"]["productVariantDetachMedia"]["userErrors"][0]["code"],
        json!("MEDIA_IS_NOT_ATTACHED_TO_VARIANT")
    );
    let had_media = !super::gids(shop.get(&variant).unwrap().get("media")).is_empty();
    if had_media {
        let current = super::gids(shop.get(&variant).unwrap().get("media"));
        run_with(
            &mut shop,
            r#"mutation($id: ID!, $pairs: [ProductVariantDetachMediaInput!]!) { productVariantDetachMedia(productId: $id, variantMedia: $pairs) { userErrors { code } } }"#,
            json!({"id": tote, "pairs": [{"variantId": variant, "mediaIds": current}]}),
        );
    }
    let attached = run_with(
        &mut shop,
        r#"mutation($id: ID!, $pairs: [ProductVariantAppendMediaInput!]!) { productVariantAppendMedia(productId: $id, variantMedia: $pairs) {
            productVariants { media(first: 2) { nodes { id } } image { url } } userErrors { field message } } }"#,
        json!({"id": tote, "pairs": [{"variantId": variant, "mediaIds": [side]}]}),
    );
    let variants_payload = &ok(&attached, "productVariantAppendMedia")["productVariants"];
    assert_eq!(variants_payload[0]["media"]["nodes"], json!([{"id": side}]));
    assert_eq!(
        variants_payload[0]["image"]["url"],
        json!("https://example.com/side.jpg")
    );
    shop.take_events();

    let reordered = run_with(
        &mut shop,
        r#"mutation($id: ID!, $moves: [MoveInput!]!) { productReorderMedia(id: $id, moves: $moves) { job { done } mediaUserErrors { message } } }"#,
        json!({"id": tote, "moves": [{"id": video, "newPosition": "0"}]}),
    );
    assert_eq!(
        ok(&reordered, "productReorderMedia")["job"],
        json!({"done": true})
    );
    assert_eq!(super::gids(shop.get(&tote).unwrap().get("media"))[0], video);
    assert_eq!(shop.get(&tote).unwrap()["featuredMedia"], json!(video));

    let deleted = run_with(
        &mut shop,
        r#"mutation($id: ID!, $ids: [ID!]!) { productDeleteMedia(productId: $id, mediaIds: $ids) { deletedMediaIds mediaUserErrors { field message } } }"#,
        json!({"id": tote, "ids": [side]}),
    );
    assert_eq!(
        ok(&deleted, "productDeleteMedia")["deletedMediaIds"],
        json!([side])
    );
    assert!(!shop.contains(&side));
    assert_eq!(shop.get(&variant).unwrap()["image"], Value::Null);
    let unknown = run_with(
        &mut shop,
        r#"mutation($id: ID!) { productDeleteMedia(productId: $id, mediaIds: ["gid://shopify/Video/-1"]) { deletedMediaIds mediaUserErrors { field message } } }"#,
        json!({"id": tote}),
    );
    assert_eq!(
        unknown["data"]["productDeleteMedia"],
        json!({"deletedMediaIds": null, "mediaUserErrors": [{"field": ["mediaIds"], "message": "Media id gid://shopify/Video/-1 does not exist"}]})
    );
}

#[test]
fn product_create_and_update_take_media_and_metafields() {
    let mut shop = demo();
    let created = run(
        &mut shop,
        r#"mutation { productCreate(product: {title: "Helmet Nova", metafields: [{namespace: "custom", key: "material", type: "single_line_text_field", value: "Carbon"}]},
            media: [{originalSource: "https://cdn.example.com/helmet.png", alt: "Gray helmet", mediaContentType: IMAGE}]) {
            product { id media(first: 2) { nodes { alt mediaContentType } } featuredMedia { alt } metafield(namespace: "custom", key: "material") { value type } }
            userErrors { field message } } }"#,
    );
    let product = &ok(&created, "productCreate")["product"];
    assert_eq!(
        product["media"]["nodes"],
        json!([{"alt": "Gray helmet", "mediaContentType": "IMAGE"}])
    );
    assert_eq!(product["featuredMedia"]["alt"], json!("Gray helmet"));
    assert_eq!(
        product["metafield"],
        json!({"value": "Carbon", "type": "single_line_text_field"})
    );
    let id = product["id"].as_str().unwrap().to_string();
    // The new image has the id lsf-core gives the first image of the product.
    let image = super::gids(shop.get(&id).unwrap().get("media"))[0].clone();
    assert_eq!(
        image,
        crate::state::gid(
            "MediaImage",
            lsf_core::util::stable_id("media", "helmet-nova/0")
        )
    );
    shop.take_events();
    let updated = run_with(
        &mut shop,
        r#"mutation($id: ID!) { productUpdate(product: {id: $id, metafields: [{namespace: "custom", key: "material", value: "Kevlar"}]}) {
            product { metafield(namespace: "custom", key: "material") { value } } userErrors { message } } }"#,
        json!({"id": id}),
    );
    assert_eq!(
        ok(&updated, "productUpdate")["product"]["metafield"]["value"],
        json!("Kevlar")
    );
    let blank = run_with(
        &mut shop,
        r#"mutation($id: ID!) { productUpdate(product: {id: $id, metafields: [{namespace: "custom", key: "", type: "single_line_text_field", value: "x"}]}) { userErrors { field message } } }"#,
        json!({"id": id}),
    );
    assert_eq!(
        blank["data"]["productUpdate"]["userErrors"],
        json!([{"field": ["metafields", "0", "key"], "message": "Key can't be blank"}])
    );
}

#[test]
fn product_set_creates_and_replaces_options_and_variants() {
    let mut shop = demo();
    let location = shop.keys("Location").next().unwrap().to_string();
    let created = run_with(
        &mut shop,
        r#"mutation($input: ProductSetInput!) { productSet(input: $input) {
            product { id title handle options { name values } variants(first: 5) { nodes { id title price sku inventoryQuantity } } totalInventory }
            productSetOperation { id } userErrors { field message code } } }"#,
        json!({"input": {
            "title": "Winter hat", "tags": ["winter"],
            "productOptions": [{"name": "Color", "position": 1, "values": [{"name": "Grey"}, {"name": "Black"}]}],
            "variants": [
                {"optionValues": [{"optionName": "Color", "name": "Grey"}], "price": 79.99, "sku": "HAT-GRY",
                 "inventoryQuantities": [{"locationId": location, "name": "available", "quantity": 12}]},
                {"optionValues": [{"optionName": "Color", "name": "Black"}], "price": 69.99},
            ],
        }}),
    );
    let payload = ok(&created, "productSet");
    let product = &payload["product"];
    assert_eq!(product["handle"], json!("winter-hat"));
    assert_eq!(
        product["options"],
        json!([{"name": "Color", "values": ["Grey", "Black"]}])
    );
    assert_eq!(product["variants"]["nodes"][0]["sku"], json!("HAT-GRY"));
    assert_eq!(
        product["variants"]["nodes"][0]["inventoryQuantity"],
        json!(12)
    );
    assert_eq!(product["variants"]["nodes"][1]["price"], json!("69.99"));
    assert_eq!(product["totalInventory"], json!(12));
    let id = product["id"].as_str().unwrap().to_string();
    let grey = product["variants"]["nodes"][0]["id"]
        .as_str()
        .unwrap()
        .to_string();
    let black = product["variants"]["nodes"][1]["id"]
        .as_str()
        .unwrap()
        .to_string();
    let events = topics(&mut shop);
    assert_eq!(events[0], "products/create");
    assert_eq!(
        events
            .iter()
            .filter(|topic| *topic == "inventory_items/create")
            .count(),
        2
    );

    // Upsert by id: the Grey variant updated, Black removed, White created.
    let updated = run_with(
        &mut shop,
        r#"mutation($id: ID!, $input: ProductSetInput!) { productSet(identifier: {id: $id}, input: $input) {
            product { title tags options { name values } variants(first: 5) { nodes { id title price } } } userErrors { field message code } } }"#,
        json!({"id": id, "input": {
            "productOptions": [{"name": "Color", "values": [{"name": "Grey"}, {"name": "White"}]}],
            "variants": [
                {"id": grey, "optionValues": [{"optionName": "Color", "name": "Grey"}], "price": "75.00"},
                {"optionValues": [{"optionName": "Color", "name": "White"}], "price": "80.00"},
            ],
        }}),
    );
    let product = &ok(&updated, "productSet")["product"];
    assert_eq!(product["title"], json!("Winter hat"), "left out, kept");
    assert_eq!(product["tags"], json!(["winter"]));
    assert_eq!(
        product["options"],
        json!([{"name": "Color", "values": ["Grey", "White"]}])
    );
    assert_eq!(product["variants"]["nodes"][0]["id"], json!(grey));
    assert_eq!(product["variants"]["nodes"][0]["price"], json!("75.00"));
    assert_eq!(product["variants"]["nodes"][1]["title"], json!("White"));
    assert!(!shop.contains(&black));
    let events = topics(&mut shop);
    assert!(events.contains(&"inventory_items/delete".to_string()));
    assert!(events.contains(&"products/update".to_string()));

    // Back to the default variant (shopify.dev's example), asynchronously.
    let reset = run_with(
        &mut shop,
        r#"mutation($id: ID!) { productSet(synchronous: false, identifier: {id: $id}, input: {productOptions: [], variants: []}) {
            product { id } productSetOperation { status product { hasOnlyDefaultVariant } } userErrors { message } } }"#,
        json!({"id": id}),
    );
    let payload = ok(&reset, "productSet");
    assert_eq!(payload["product"], Value::Null);
    assert_eq!(
        payload["productSetOperation"],
        json!({"status": "COMPLETE", "product": {"hasOnlyDefaultVariant": true}})
    );

    // Upsert by handle creates; an id of no product is refused.
    let by_handle = run(
        &mut shop,
        r#"mutation { productSet(identifier: {handle: "rain-cap"}, input: {title: "Rain cap"}) { product { handle } userErrors { message } } }"#,
    );
    assert_eq!(
        ok(&by_handle, "productSet")["product"]["handle"],
        json!("rain-cap")
    );
    shop.take_events();
    let before = shop.clone();
    let unknown = run(
        &mut shop,
        r#"mutation { productSet(identifier: {id: "gid://shopify/Product/1"}, input: {title: "X"}) { product { id } userErrors { field message code } } }"#,
    );
    assert_eq!(
        unknown["data"]["productSet"]["userErrors"],
        json!([{"field": ["input", "id"], "message": "Product does not exist", "code": "PRODUCT_DOES_NOT_EXIST"}])
    );
    let missing = run(
        &mut shop,
        r#"mutation { productSet(input: {title: "Odd", productOptions: [{name: "Size", values: [{name: "S"}]}]}) { userErrors { field code } } }"#,
    );
    assert_eq!(
        missing["data"]["productSet"]["userErrors"],
        json!([{"field": ["input", "variants"], "code": "VARIANTS_INPUT_MISSING"}])
    );
    assert_eq!(shop.count("Product"), before.count("Product"));
    assert!(shop.events().is_empty());
}

#[test]
fn product_duplicate_copies_options_variants_and_metafields() {
    let mut shop = demo();
    let tee = product(&shop, "organic-cotton-t-shirt");
    let answer = run_with(
        &mut shop,
        r#"mutation($id: ID!) { productDuplicate(productId: $id, newTitle: "Tee (copy)", newStatus: DRAFT) {
            newProduct { title handle status publishedAt tags options { name values }
                variants(first: 20) { nodes { title price sku inventoryQuantity } } }
            userErrors { field message } } }"#,
        json!({"id": tee}),
    );
    let copy = &ok(&answer, "productDuplicate")["newProduct"];
    assert_eq!(copy["title"], json!("Tee (copy)"));
    assert_eq!(copy["handle"], json!("tee-copy"));
    assert_eq!(copy["status"], json!("DRAFT"));
    assert_eq!(copy["publishedAt"], Value::Null);
    let original = run_with(
        &mut shop,
        r#"query($id: ID!) { product(id: $id) { tags options { name values } variants(first: 20) { nodes { title price sku } } } }"#,
        json!({"id": tee}),
    );
    let original = &original["data"]["product"];
    assert_eq!(copy["tags"], original["tags"]);
    assert_eq!(copy["options"], original["options"]);
    let titles = |product: &Value| -> Vec<(Value, Value, Value)> {
        product["variants"]["nodes"]
            .as_array()
            .unwrap()
            .iter()
            .map(|node| {
                (
                    node["title"].clone(),
                    node["price"].clone(),
                    node["sku"].clone(),
                )
            })
            .collect()
    };
    assert_eq!(titles(copy), titles(original));
    assert!(
        copy["variants"]["nodes"]
            .as_array()
            .unwrap()
            .iter()
            .all(|node| node["inventoryQuantity"] == json!(0))
    );
    assert_eq!(topics(&mut shop)[0], "products/create");
    let missing = run(
        &mut shop,
        r#"mutation { productDuplicate(productId: "gid://shopify/Product/1", newTitle: "X") { newProduct { id } userErrors { field message } } }"#,
    );
    assert_eq!(
        missing["data"]["productDuplicate"],
        json!({"newProduct": null, "userErrors": [{"field": ["productId"], "message": "Product does not exist"}]})
    );
}

#[test]
fn collections_custom_and_smart() {
    let mut shop = demo();
    let mug = product(&shop, "ceramic-mug");
    let tote = product(&shop, "canvas-tote-bag");
    let created = run_with(
        &mut shop,
        r#"mutation($input: CollectionInput!) { collectionCreate(input: $input) {
            collection { id title handle sortOrder ruleSet { appliedDisjunctively } productsCount { count } products(first: 5) { nodes { id } } }
            userErrors { field message } } }"#,
        json!({"input": {"title": "Picks", "descriptionHtml": "<p>Ours.</p>", "products": [mug, tote]}}),
    );
    let collection = &ok(&created, "collectionCreate")["collection"];
    assert_eq!(collection["handle"], json!("picks"));
    assert_eq!(collection["sortOrder"], json!("MANUAL"));
    assert_eq!(collection["ruleSet"], Value::Null);
    assert_eq!(collection["productsCount"]["count"], json!(2));
    let picks = collection["id"].as_str().unwrap().to_string();
    assert!(super::gids(shop.get(&mug).unwrap().get("collections")).contains(&picks));
    assert_eq!(topics(&mut shop), ["collections/create"]);

    // Add (once), reorder, remove.
    let tee = product(&shop, "organic-cotton-t-shirt");
    let added = run_with(
        &mut shop,
        r#"mutation($id: ID!, $products: [ID!]!) { collectionAddProducts(id: $id, productIds: $products) { collection { productsCount { count } } userErrors { message } } }"#,
        json!({"id": picks, "products": [tee, mug]}),
    );
    assert_eq!(
        ok(&added, "collectionAddProducts")["collection"]["productsCount"]["count"],
        json!(3)
    );
    let reordered = run_with(
        &mut shop,
        r#"mutation($id: ID!, $moves: [MoveInput!]!) { collectionReorderProducts(id: $id, moves: $moves) { job { done } userErrors { field message } } }"#,
        json!({"id": picks, "moves": [{"id": tee, "newPosition": "0"}]}),
    );
    ok(&reordered, "collectionReorderProducts");
    assert_eq!(
        super::gids(shop.get(&picks).unwrap().get("products")),
        [tee.clone(), mug.clone(), tote.clone()]
    );
    let removed = run_with(
        &mut shop,
        r#"mutation($id: ID!, $products: [ID!]!) { collectionRemoveProducts(id: $id, productIds: $products) { job { done } userErrors { message } } }"#,
        json!({"id": picks, "products": [mug]}),
    );
    ok(&removed, "collectionRemoveProducts");
    assert!(!super::gids(shop.get(&mug).unwrap().get("collections")).contains(&picks));
    assert_eq!(
        topics(&mut shop),
        [
            "collections/update",
            "collections/update",
            "collections/update"
        ]
    );

    // A smart collection: its products are those its rules match, and follow product changes.
    let smart = run(
        &mut shop,
        r#"mutation { collectionCreate(input: {title: "On sale", ruleSet: {appliedDisjunctively: true, rules: [
            {column: TAG, relation: EQUALS, condition: "sale"}, {column: VARIANT_PRICE, relation: GREATER_THAN, condition: "1000"}]}}) {
            collection { id sortOrder ruleSet { appliedDisjunctively rules { column relation condition } } products(first: 10) { nodes { handle } } } userErrors { message } } }"#,
    );
    let collection = &ok(&smart, "collectionCreate")["collection"];
    assert_eq!(collection["sortOrder"], json!("MOST_RELEVANT"));
    let handles: Vec<&str> = collection["products"]["nodes"]
        .as_array()
        .unwrap()
        .iter()
        .map(|node| node["handle"].as_str().unwrap())
        .collect();
    assert!(
        handles.contains(&"linen-overshirt") && handles.contains(&"scented-candle"),
        "{handles:?}"
    );
    assert!(!handles.contains(&"ceramic-mug"));
    let sale = collection["id"].as_str().unwrap().to_string();
    shop.take_events();
    // Tagging the mug puts it in; a manual add is refused.
    run_with(
        &mut shop,
        r#"mutation($id: ID!) { productUpdate(product: {id: $id, tags: ["kitchen", "sale"]}) { userErrors { message } } }"#,
        json!({"id": mug}),
    );
    assert!(super::gids(shop.get(&sale).unwrap().get("products")).contains(&mug));
    assert!(super::gids(shop.get(&mug).unwrap().get("collections")).contains(&sale));
    assert_eq!(topics(&mut shop), ["products/update"]);
    let before = shop.clone();
    let refused = run_with(
        &mut shop,
        r#"mutation($id: ID!, $products: [ID!]!) { collectionAddProductsV2(id: $id, productIds: $products) { job { id } userErrors { field message code } } }"#,
        json!({"id": sale, "products": [tote]}),
    );
    assert_eq!(
        refused["data"]["collectionAddProductsV2"]["userErrors"],
        json!([{"field": ["id"], "message": "Can't manually add products to a smart collection", "code": "CANT_ADD_TO_SMART_COLLECTION"}])
    );
    let custom_rules = run_with(
        &mut shop,
        r#"mutation($id: ID!) { collectionUpdate(input: {id: $id, ruleSet: {appliedDisjunctively: false, rules: [{column: TITLE, relation: CONTAINS, condition: "x"}]}}) { userErrors { field message } } }"#,
        json!({"id": picks}),
    );
    assert_eq!(
        custom_rules["data"]["collectionUpdate"]["userErrors"],
        json!([{"field": ["id"], "message": "Cannot update rule set of a custom collection"}])
    );
    unchanged(&mut shop, &before, &[&sale, &picks, &tote]);

    // Changing the rules answers a job, the membership is computed again.
    let updated = run_with(
        &mut shop,
        r#"mutation($id: ID!) { collectionUpdate(input: {id: $id, title: "Mugs", ruleSet: {appliedDisjunctively: false, rules: [{column: TITLE, relation: CONTAINS, condition: "mug"}]}}) {
            collection { title products(first: 5) { nodes { handle } } } job { done } userErrors { message } } }"#,
        json!({"id": sale}),
    );
    let payload = ok(&updated, "collectionUpdate");
    assert_eq!(payload["job"], json!({"done": true}));
    assert_eq!(
        payload["collection"]["products"]["nodes"],
        json!([{"handle": "ceramic-mug"}])
    );
    shop.take_events();

    let deleted = run_with(
        &mut shop,
        r#"mutation($id: ID!) { collectionDelete(input: {id: $id}) { deletedCollectionId userErrors { message } } }"#,
        json!({"id": picks}),
    );
    assert_eq!(
        ok(&deleted, "collectionDelete")["deletedCollectionId"],
        json!(picks)
    );
    assert!(!super::gids(shop.get(&tee).unwrap().get("collections")).contains(&picks));
    assert_eq!(topics(&mut shop), ["collections/delete"]);
    let gone = run_with(
        &mut shop,
        r#"mutation($id: ID!) { collectionDelete(input: {id: $id}) { deletedCollectionId userErrors { field message } } }"#,
        json!({"id": picks}),
    );
    assert_eq!(
        gone["data"]["collectionDelete"],
        json!({"deletedCollectionId": null, "userErrors": [{"field": ["id"], "message": "Collection does not exist"}]})
    );
}
