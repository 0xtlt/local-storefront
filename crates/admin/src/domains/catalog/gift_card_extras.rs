//! The rest of gift cards: gift card products (`giftCardProductSet`) and the notifications of a
//! card to its customer and its recipient (`giftCardSendNotificationToCustomer`,
//! `giftCardSendNotificationToRecipient`).
//!
//! As shopify.dev's reference of 2026-10 describes them:
//!
//! - `giftCardProductSet` is `productSet` for a gift card product: the product is a gift card,
//!   its variants are not taxable, need no shipping and are not tracked ("applies gift card
//!   variant defaults (non-taxable, no shipping required, inventory untracked)"), lists are
//!   replaced as `productSet` replaces them. It runs `productSet` with those defaults, and
//!   answers its product, operation and errors (codes `GiftCardProductSetUserErrorCode` shares
//!   with it, else `INVALID_INPUT`). A product that is not a gift card is refused
//!   (`PRODUCT_MUST_BE_GIFT_CARD`); `issuanceCurrency` and `crossCurrencyRedeemable` are taken
//!   on creation only.
//! - The notifications put the card's email in the shop's outbox (`crate::outbox`): to its
//!   customer, to its recipient with their message. lsf sends nothing; the card is answered as
//!   it is. A card that is disabled or expired is refused (`INVALID`, lsf's wording).

use serde_json::{Map, Value, json};

use super::gift_cards::{redeemable, today};
use crate::graphql::{self, FieldContext, FieldError, Request};
use crate::outbox::{self, Entry};
use crate::resolve::{Mutation, Registry, UserError, payload};
use crate::state::{Record, ShopState, value};

pub fn register(registry: &mut Registry) {
    registry
        .mutation("giftCardProductSet", product_set)
        .mutation("giftCardSendNotificationToCustomer", |m, ctx| {
            notify(m, ctx, false)
        })
        .mutation("giftCardSendNotificationToRecipient", |m, ctx| {
            notify(m, ctx, true)
        });
}

/// The codes of `GiftCardProductSetUserErrorCode`: a code of `productSet` it does not have is
/// `INVALID_INPUT`.
const CODES: &[&str] = &[
    "BARCODE_CHECKSUM_VALIDATION_FAILED",
    "BARCODE_VALUE_TOO_LONG",
    "BLANK_BARCODE_VALUE",
    "CANNOT_COMBINE_LINKED_AND_NONLINKED_OPTION_VALUES",
    "CAPABILITY_VIOLATION",
    "DUPLICATED_METAFIELD_VALUE",
    "DUPLICATED_OPTION_NAME",
    "DUPLICATED_OPTION_VALUE",
    "DUPLICATED_VALUE",
    "DUPLICATE_BARCODE_VALUE",
    "DUPLICATE_LINKED_OPTION",
    "GENERIC_ERROR",
    "GIFT_CARDS_NOT_ACTIVATED",
    "GIFT_CARD_ATTRIBUTE_CANNOT_BE_CHANGED",
    "HANDLE_NOT_UNIQUE",
    "ID_NOT_ALLOWED",
    "INCOMPATIBLE_BARCODE_FIELDS",
    "INPUT_MISMATCH",
    "INVALID_BARCODE_CHARACTERS",
    "INVALID_BARCODE_LENGTH",
    "INVALID_BARCODE_PREFIX",
    "INVALID_BARCODE_SEPARATORS",
    "INVALID_BARCODE_TYPE",
    "INVALID_INPUT",
    "INVALID_METAFIELD",
    "INVALID_METAFIELD_VALUE_FOR_LINKED_OPTION",
    "INVALID_PRODUCT",
    "INVALID_VARIANT",
    "JOB_ERROR",
    "LINKED_METAFIELD_DEFINITION_NOT_FOUND",
    "LINKED_OPTIONS_NOT_SUPPORTED_FOR_SHOP",
    "METAFIELD_MISMATCH",
    "MISSING_FIELD_REQUIRED",
    "NOT_FOUND",
    "OPTIONS_OVER_LIMIT",
    "OPTION_DOES_NOT_EXIST",
    "OPTION_VALUES_MISSING",
    "OPTION_VALUES_OVER_LIMIT",
    "OPTION_VALUE_DOES_NOT_EXIST",
    "PRODUCT_DOES_NOT_EXIST",
    "PRODUCT_MUST_BE_GIFT_CARD",
    "PRODUCT_OPTIONS_INPUT_MISSING",
    "PRODUCT_SUSPENDED",
    "PRODUCT_VARIANT_DOES_NOT_EXIST",
    "TOO_MANY_BARCODES",
    "VARIANTS_INPUT_MISSING",
    "VARIANTS_OVER_LIMIT",
];

/// The product an identifier names, when there is one.
fn identified(state: &ShopState, identifier: &Map<String, Value>) -> Option<String> {
    if let Some(id) = identifier.get("id").and_then(Value::as_str) {
        return (state.type_of(id) == Some("Product")).then(|| id.to_string());
    }
    let handle = identifier.get("handle").and_then(Value::as_str)?;
    super::records::find_by_handle(state, "Product", handle)
}

/// The `ProductSetInput` of a `GiftCardProductSetInput`: a gift card, its variants with the
/// gift card defaults.
fn product_input(input: &Map<String, Value>) -> Map<String, Value> {
    let mut product = input.clone();
    for field in ["issuanceCurrency", "crossCurrencyRedeemable"] {
        product.remove(field);
    }
    product.insert("giftCard".into(), json!(true));
    if let Some(Value::Array(variants)) = product.get_mut("variants") {
        for variant in variants.iter_mut().filter_map(Value::as_object_mut) {
            variant.insert("taxable".into(), json!(false));
            variant.insert(
                "inventoryItem".into(),
                json!({"tracked": false, "requiresShipping": false}),
            );
            variant.insert("inventoryPolicy".into(), json!("CONTINUE"));
        }
    }
    product
}

/// `giftCardProductSet`.
fn product_set(mutation: &mut Mutation<'_>, ctx: &FieldContext<'_>) -> Result<Value, FieldError> {
    let empty = json!({"product": null, "productSetOperation": null});
    let failed = |field: &[&str], message: &str, code: &str| {
        Ok(payload(
            empty.clone(),
            &[UserError::new(field, message).code(code)],
        ))
    };
    let input = crate::domains::orders::model::input_of(ctx.arg("input"));
    let identifier = crate::domains::orders::model::input_of(ctx.arg("identifier"));
    let existing = identified(mutation.state(), &identifier);
    if let Some(product) = &existing {
        if mutation
            .state()
            .get(product)
            .and_then(|record| record.get("isGiftCard"))
            .and_then(Value::as_bool)
            != Some(true)
        {
            return failed(
                &["identifier"],
                "The specified product is not a gift card. The giftCardProductSet mutation can only be used with gift card products.",
                "PRODUCT_MUST_BE_GIFT_CARD",
            );
        }
        for field in ["issuanceCurrency", "crossCurrencyRedeemable"] {
            if input.get(field).is_some_and(|value| !value.is_null()) {
                return failed(
                    &["input", field],
                    &format!("{field} can only be set when the gift card product is created"),
                    "INVALID_INPUT",
                );
            }
        }
    }
    let request = Request {
        query: "mutation($input: ProductSetInput!, $identifier: ProductSetIdentifiers, $synchronous: Boolean) {
            productSet(input: $input, identifier: $identifier, synchronous: $synchronous) {
              product { id } productSetOperation { id } userErrors { field message code } } }"
            .to_string(),
        variables: value::object(json!({
            "input": product_input(&input),
            "identifier": if identifier.is_empty() { Value::Null } else { Value::Object(identifier.clone()) },
            "synchronous": ctx.arg_bool("synchronous").unwrap_or(true),
        })),
        operation_name: None,
    };
    let answer = {
        let mut resolver = mutation.resolver();
        graphql::run(crate::schema::admin(), &mut resolver, &request, None).to_json()
    };
    let set = &answer["data"]["productSet"];
    if set.is_null() {
        let message = answer["errors"][0]["message"]
            .as_str()
            .unwrap_or("Something went wrong, please try again.")
            .to_string();
        return failed(&["input"], &message, "GENERIC_ERROR");
    }
    let errors: Vec<UserError> = set["userErrors"]
        .as_array()
        .into_iter()
        .flatten()
        .map(|error| {
            let field: Vec<String> = error["field"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(Value::as_str)
                .map(str::to_string)
                .collect();
            let field: Vec<&str> = field.iter().map(String::as_str).collect();
            let code = error["code"]
                .as_str()
                .filter(|code| CODES.contains(code))
                .unwrap_or("INVALID_INPUT");
            UserError::new(&field, error["message"].as_str().unwrap_or_default()).code(code)
        })
        .collect();
    if !errors.is_empty() {
        return Ok(payload(empty, &errors));
    }
    let product = set["product"]["id"].as_str().map(str::to_string);
    if existing.is_none()
        && let Some(product) = &product
    {
        let state = mutation.state_mut();
        let currency = crate::domains::orders::model::currency(state);
        if let Some(record) = state.get_mut(product) {
            record.insert(
                "lsfGiftCardSettings".into(),
                json!({
                    "issuanceCurrency": input.get("issuanceCurrency").and_then(Value::as_str).unwrap_or(&currency),
                    "crossCurrencyRedeemable": input.get("crossCurrencyRedeemable").and_then(Value::as_bool).unwrap_or(false),
                }),
            );
        }
    }
    Ok(payload(
        json!({
            "product": product,
            "productSetOperation": set["productSetOperation"]["id"].as_str(),
        }),
        &[],
    ))
}

/// `giftCardSendNotificationToCustomer` (`recipient` false) and
/// `giftCardSendNotificationToRecipient`.
fn notify(
    mutation: &mut Mutation<'_>,
    ctx: &FieldContext<'_>,
    recipient: bool,
) -> Result<Value, FieldError> {
    let failed = |message: &str, code: &str| {
        Ok(payload(
            json!({"giftCard": null}),
            &[UserError::new(&["id"], message).code(code)],
        ))
    };
    let id = ctx.arg_str("id").unwrap_or_default().to_string();
    let state = mutation.state();
    let Some(card) = state.get_of(&id, "GiftCard").cloned() else {
        return failed("The gift card could not be found.", "GIFT_CARD_NOT_FOUND");
    };
    let to = if recipient {
        card.get("recipientAttributes")
            .and_then(|attributes| attributes.get("recipient"))
            .and_then(Value::as_str)
    } else {
        card.get("customer").and_then(Value::as_str)
    };
    let email = to
        .and_then(|customer| state.get(customer))
        .and_then(|customer| customer.get("email"))
        .and_then(Value::as_str)
        .filter(|email| !email.is_empty())
        .map(str::to_string);
    let Some(email) = email else {
        return if recipient {
            failed("The recipient could not be found.", "RECIPIENT_NOT_FOUND")
        } else {
            failed("The customer could not be found.", "CUSTOMER_NOT_FOUND")
        };
    };
    if !redeemable(&card, &today(state)) {
        return failed(
            "The gift card is disabled, expired or spent: there is nothing to notify",
            "INVALID",
        );
    }
    let (subject, body) = card_text(state, &card, recipient);
    let what = if recipient {
        "gift_card_recipient"
    } else {
        "gift_card_created"
    };
    outbox::record(
        mutation.state_mut(),
        Entry::email(what, &email)
            .subject(subject)
            .body(body)
            .about(&id),
    );
    Ok(payload(json!({"giftCard": id}), &[]))
}

/// The subject and the text of a gift card's email: its balance and its last characters (the
/// code itself is not kept, as Shopify answers it once), and for its recipient their message.
fn card_text(state: &ShopState, card: &Record, recipient: bool) -> (String, String) {
    let shop = state
        .shop()
        .and_then(|shop| shop.get("name"))
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    let balance = card
        .get("balance")
        .and_then(|balance| balance.get("amount"))
        .and_then(Value::as_str)
        .unwrap_or("0.0");
    let currency = card
        .get("balance")
        .and_then(|balance| balance.get("currencyCode"))
        .and_then(Value::as_str)
        .unwrap_or_default();
    let last = card
        .get("lastCharacters")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let mut body = format!("Your {shop} gift card ending in {last} holds {balance} {currency}.");
    if recipient && let Some(attributes) = card.get("recipientAttributes") {
        if let Some(name) = attributes.get("preferredName").and_then(Value::as_str) {
            body = format!("{name},\n{body}");
        }
        if let Some(message) = attributes.get("message").and_then(Value::as_str) {
            body.push('\n');
            body.push_str(message);
        }
    }
    let subject = if recipient {
        format!("You received a {shop} gift card")
    } else {
        format!("Your {shop} gift card")
    };
    (subject, body)
}

#[cfg(test)]
mod tests {
    use serde_json::{Value, json};

    use crate::domains::catalog::mutation_tests::{demo, run_with};

    const SET: &str = "mutation($input: GiftCardProductSetInput!, $identifier: ProductSetIdentifiers) {
        giftCardProductSet(input: $input, identifier: $identifier) {
          product { id title isGiftCard variants(first: 5) { nodes { price taxable inventoryItem { tracked requiresShipping } } } }
          userErrors { field message code } } }";

    #[test]
    fn a_gift_card_product_is_set_with_the_gift_card_defaults() {
        let mut shop = demo();
        let answer = run_with(
            &mut shop,
            SET,
            json!({"input": {
                "title": "Store gift card",
                "productOptions": [{"name": "Denomination", "values": [{"name": "25"}, {"name": "50"}]}],
                "variants": [
                    {"optionValues": [{"optionName": "Denomination", "name": "25"}], "price": "25.00"},
                    {"optionValues": [{"optionName": "Denomination", "name": "50"}], "price": "50.00"},
                ],
                "issuanceCurrency": "USD",
            }}),
        );
        let set = &answer["data"]["giftCardProductSet"];
        assert_eq!(set["userErrors"], json!([]), "{answer}");
        let product = &set["product"];
        assert_eq!(product["isGiftCard"], true);
        assert_eq!(
            product["variants"]["nodes"][1],
            json!({"price": "50.00", "taxable": false, "inventoryItem": {"tracked": false, "requiresShipping": false}})
        );
        let id = product["id"].as_str().unwrap().to_string();
        let events: Vec<String> = shop
            .take_events()
            .into_iter()
            .map(|event| event.topic)
            .collect();
        assert!(
            events.contains(&"products/create".to_string()),
            "{events:?}"
        );
        // Updated by its id: the title changes, the issuance currency cannot.
        let updated = run_with(
            &mut shop,
            SET,
            json!({"identifier": {"id": id}, "input": {"title": "Gift card"}}),
        );
        assert_eq!(
            updated["data"]["giftCardProductSet"]["product"]["title"],
            "Gift card"
        );
        let refused = run_with(
            &mut shop,
            SET,
            json!({"identifier": {"id": id}, "input": {"issuanceCurrency": "CAD"}}),
        );
        assert_eq!(
            refused["data"]["giftCardProductSet"]["userErrors"][0]["code"],
            "INVALID_INPUT"
        );
    }

    #[test]
    fn a_product_that_is_not_a_gift_card_is_refused_and_unchanged() {
        let mut shop = demo();
        let product = shop
            .keys("Product")
            .find(|key| shop.get(key).unwrap()["isGiftCard"] == json!(false))
            .unwrap()
            .to_string();
        let before = shop.get(&product).cloned();
        let answer = run_with(
            &mut shop,
            SET,
            json!({"identifier": {"id": product}, "input": {"title": "Not a card"}}),
        );
        let set = &answer["data"]["giftCardProductSet"];
        assert_eq!(set["product"], Value::Null);
        assert_eq!(set["userErrors"][0]["code"], "PRODUCT_MUST_BE_GIFT_CARD");
        assert_eq!(shop.get(&product).cloned(), before);
        assert!(shop.take_events().is_empty());
    }

    #[test]
    fn a_cards_notifications_go_to_the_outbox() {
        let mut shop = demo();
        // The demo's first card is jane's, for her: a customer and a recipient.
        let card = shop.keys("GiftCard").next().unwrap().to_string();
        let to_customer = run_with(
            &mut shop,
            "mutation($id: ID!) { giftCardSendNotificationToCustomer(id: $id) { giftCard { id customer { id } } userErrors { field message code } } }",
            json!({"id": card}),
        );
        assert_eq!(
            to_customer["data"]["giftCardSendNotificationToCustomer"]["userErrors"],
            json!([]),
            "{to_customer}"
        );
        let to_recipient = run_with(
            &mut shop,
            "mutation($id: ID!) { giftCardSendNotificationToRecipient(id: $id) { giftCard { id } userErrors { field message code } } }",
            json!({"id": card}),
        );
        assert_eq!(
            to_recipient["data"]["giftCardSendNotificationToRecipient"]["userErrors"],
            json!([])
        );
        let emails = crate::outbox::entries(&shop, Some(crate::outbox::Channel::Email));
        assert_eq!(emails[0]["what"], "gift_card_created");
        assert_eq!(emails[1]["what"], "gift_card_recipient");
        assert!(
            emails[1]["body"]
                .as_str()
                .unwrap()
                .contains("Happy birthday")
        );
        // A card without a recipient, and one that is expired, are refused.
        let expired = shop
            .keys("GiftCard")
            .find(|key| {
                !super::redeemable(shop.get(key).unwrap(), &super::today(&shop))
                    && shop.get(key).unwrap()["customer"].is_string()
            })
            .map(str::to_string);
        if let Some(expired) = expired {
            let refused = run_with(
                &mut shop,
                "mutation($id: ID!) { giftCardSendNotificationToCustomer(id: $id) { giftCard { id } userErrors { code } } }",
                json!({"id": expired}),
            );
            assert_eq!(
                refused["data"]["giftCardSendNotificationToCustomer"]["userErrors"][0]["code"],
                "INVALID"
            );
        }
        let missing = run_with(
            &mut shop,
            "mutation { giftCardSendNotificationToRecipient(id: \"gid://shopify/GiftCard/1\") { giftCard { id } userErrors { code } } }",
            json!({}),
        );
        assert_eq!(
            missing["data"]["giftCardSendNotificationToRecipient"]["userErrors"][0]["code"],
            "GIFT_CARD_NOT_FOUND"
        );
        assert_eq!(
            crate::outbox::entries(&shop, Some(crate::outbox::Channel::Email)).len(),
            2
        );
    }
}
