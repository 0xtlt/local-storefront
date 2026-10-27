use serde_json::json;

use super::Schema;
use crate::version::{ApiVersion, FunctionApi};

fn query(schema: &Schema, text: &str) -> Result<(), String> {
    let document = graphql_parser::parse_query::<String>(text).unwrap();
    schema.validate_query(&document)
}

fn latest(api: FunctionApi) -> &'static Schema {
    ApiVersion::latest().schema(api)
}

// A field that exists in one release and not in the other, as Shopify adds them (the
// `Customer.createdAt` of 2026-10 against the schema of 2026-07). The two supported releases
// have the same Function schemas today (docs/api-versions.md), so this is a pair of small
// schemas.
const OLD: &str = "
schema { query: Input }
type Input { customer: Customer }
type Customer { id: ID! }
";
const NEW: &str = "
schema { query: Input }
type Input { customer: Customer }
type Customer { id: ID!, createdAt: DateTime! }
scalar DateTime
";

#[test]
fn a_field_missing_in_a_version_is_an_error() {
    let (old, new) = (Schema::parse(OLD).unwrap(), Schema::parse(NEW).unwrap());
    let text = "{ customer { id createdAt } }";
    assert_eq!(query(&new, text), Ok(()));
    assert_eq!(
        query(&old, text).unwrap_err(),
        "`customer.createdAt` does not exist on `Customer` (it has id)"
    );
}

#[test]
fn queries_are_checked_field_by_field() {
    let schema = latest(FunctionApi::PaymentCustomization);
    let ok = r#"query Input($ns: String) {
        cart { cost { subtotalAmount { amount } } lines { id merchandise { __typename ... on ProductVariant { id product { hasAnyTag(tags: ["a"]) } } } } }
        paymentMethods { id name }
        paymentCustomization { metafield(namespace: "$app", key: "k") { value } }
        ...Shop
    }
    fragment Shop on Input { shop { localTime { date } } }"#;
    assert_eq!(query(schema, ok), Ok(()));

    let error = query(schema, "{ cart { lines { quantity { x } } } }").unwrap_err();
    assert_eq!(
        error,
        "`cart.lines.quantity` is a `Int` and has no fields to select"
    );
    let error = query(schema, "{ cart { lines } }").unwrap_err();
    assert_eq!(
        error,
        "`cart.lines` is a `CartLine`: select some of its fields"
    );
}

#[test]
fn arguments_are_checked() {
    let schema = latest(FunctionApi::PaymentCustomization);
    let error = query(
        schema,
        r#"{ cart { attribute(key: "a", other: 1) { value } } }"#,
    )
    .unwrap_err();
    assert_eq!(error, "`cart.attribute` has no argument `other`");
    let error = query(
        schema,
        r#"{ paymentCustomization { metafield(namespace: "x") { value } } }"#,
    )
    .unwrap_err();
    assert_eq!(
        error,
        "`paymentCustomization.metafield` needs the argument `key`"
    );
}

#[test]
fn the_api_decides_the_root_fields() {
    // The discount owner is in the discount API, the payment customization in its own.
    let discount = latest(FunctionApi::Discount);
    assert_eq!(query(discount, "{ discount { discountClasses } }"), Ok(()));
    let error = query(discount, "{ paymentMethods { id } }").unwrap_err();
    assert!(
        error.starts_with("`paymentMethods` does not exist on `Input` (it has "),
        "{error}"
    );
    assert_eq!(
        query(
            latest(FunctionApi::PaymentCustomization),
            "{ paymentMethods { id } }"
        ),
        Ok(())
    );
}

#[test]
fn type_conditions_and_fragments_are_checked() {
    let schema = latest(FunctionApi::Discount);
    let error = query(
        schema,
        "{ cart { lines { merchandise { ... on Nothing { id } } } } }",
    )
    .unwrap_err();
    assert_eq!(
        error,
        "`cart.lines.merchandise`: the type `Nothing` does not exist"
    );
    let error = query(
        schema,
        "{ cart { lines { merchandise { ... on Cart { lines { id } } } } } }",
    )
    .unwrap_err();
    assert_eq!(
        error,
        "`cart.lines.merchandise`: `... on Cart` can never apply to `Merchandise`"
    );
    let error = query(schema, "{ ...Missing }").unwrap_err();
    assert_eq!(error, "the fragment `Missing` is not defined");
    // Fragments that spread each other do not loop.
    assert_eq!(
        query(
            schema,
            "{ ...A } fragment A on Input { cart { lines { id } } ...B } fragment B on Input { ...A }"
        ),
        Ok(())
    );
    let error = query(schema, "mutation { cart { lines { id } } }").unwrap_err();
    assert_eq!(error, "only queries are allowed");
}

#[test]
fn every_problem_of_a_query_is_listed() {
    let error = query(
        latest(FunctionApi::Discount),
        "{ nothing cart { note lines { colour } } }",
    )
    .unwrap_err();
    assert_eq!(error.matches("does not exist").count(), 3, "{error}");
    assert!(
        error.contains("`nothing`")
            && error.contains("`cart.note`")
            && error.contains("`cart.lines.colour`")
    );
}

fn output(api: FunctionApi, result: &str, value: serde_json::Value) -> Result<(), String> {
    latest(api).validate_output(result, &value)
}

#[test]
fn an_output_has_the_operations_of_its_target() {
    use FunctionApi::*;
    assert_eq!(
        output(
            DeliveryCustomization,
            "CartDeliveryOptionsTransformRunResult",
            json!({"operations": [
                {"deliveryOptionHide": {"deliveryOptionHandle": "express"}},
                {"deliveryOptionRename": {"deliveryOptionHandle": "standard", "title": "Slow"}},
                {"deliveryOptionMove": {"deliveryOptionHandle": "standard", "index": 0}},
            ]})
        ),
        Ok(())
    );
    // The legacy names and shapes are not the ones of the current target...
    let error = output(
        DeliveryCustomization,
        "CartDeliveryOptionsTransformRunResult",
        json!({"operations": [{"hide": {"deliveryOptionHandle": "express"}}]}),
    )
    .unwrap_err();
    assert!(
        error.contains("`operations[0]` has no field `hide`"),
        "{error}"
    );
    assert!(
        error.contains("must have exactly one field of `Operation`, it has 0"),
        "{error}"
    );
    // ...and `FunctionRunResult` takes no operation of this API any more.
    let error = output(
        DeliveryCustomization,
        "FunctionRunResult",
        json!({"operations": [{"deliveryOptionHide": {"deliveryOptionHandle": "express"}}]}),
    )
    .unwrap_err();
    assert!(
        error.contains("`operations[0]` has no field `deliveryOptionHide`"),
        "{error}"
    );
}

#[test]
fn the_legacy_payment_result_only_sets_payment_terms() {
    use FunctionApi::*;
    let schema = latest(PaymentCustomization);
    assert_eq!(
        schema.operations("FunctionRunResult"),
        Some(vec!["paymentTermsSet"])
    );
    assert_eq!(
        schema.operations("CartPaymentMethodsTransformRunResult"),
        Some(vec![
            "orderReviewAdd",
            "paymentMethodHide",
            "paymentMethodMove",
            "paymentMethodRename",
            "paymentTermsSet"
        ])
    );
    assert_eq!(
        latest(DeliveryCustomization).operations("FunctionRunResult"),
        Some(vec![])
    );
    assert_eq!(
        latest(ProductDiscount).operations("FunctionRunResult"),
        None
    );
    // `paymentTermsSet: {}` removes the terms, it is not a `@oneOf`.
    assert_eq!(
        output(
            PaymentCustomization,
            "CartPaymentMethodsTransformRunResult",
            json!({"operations": [{"paymentTermsSet": {}}, {"paymentTermsSet": {"paymentTerms": null}}]})
        ),
        Ok(())
    );
    assert_eq!(
        output(
            PaymentCustomization,
            "FunctionRunResult",
            json!({"operations": [{"paymentTermsSet": {"paymentTerms": {"net": {"dueInDays": 7}}}}]})
        ),
        Ok(())
    );
}

#[test]
fn one_of_objects_take_exactly_one_field() {
    use FunctionApi::*;
    let two = json!({"operations": [{
        "paymentMethodHide": {"paymentMethodId": "1"},
        "paymentMethodMove": {"paymentMethodId": "1", "index": 0}
    }]});
    let error = output(
        PaymentCustomization,
        "CartPaymentMethodsTransformRunResult",
        two,
    )
    .unwrap_err();
    assert_eq!(
        error,
        "`operations[0]` must have exactly one field of `Operation`, it has 2"
    );
    let none = json!({"operations": [{}]});
    assert!(
        output(
            PaymentCustomization,
            "CartPaymentMethodsTransformRunResult",
            none
        )
        .is_err()
    );
}

#[test]
fn required_fields_scalars_and_enums_are_checked() {
    use FunctionApi::*;
    let result = "CartPaymentMethodsTransformRunResult";
    let error = output(PaymentCustomization, result, json!({})).unwrap_err();
    assert_eq!(error, "`operations` is required");
    let error = output(
        PaymentCustomization,
        result,
        json!({"operations": [{"paymentMethodMove": {"paymentMethodId": "1"}}]}),
    )
    .unwrap_err();
    assert_eq!(error, "`operations[0].paymentMethodMove.index` is required");
    let error = output(
        PaymentCustomization,
        result,
        json!({"operations": [{"paymentMethodMove": {"paymentMethodId": "1", "index": "0"}}]}),
    )
    .unwrap_err();
    assert_eq!(
        error,
        "`operations[0].paymentMethodMove.index` must be an integer, got \"0\""
    );
    let error = output(
        PaymentCustomization,
        result,
        json!({"operations": [{"paymentMethodHide": {"paymentMethodId": "1", "placements": ["HERE"]}}]}),
    )
    .unwrap_err();
    assert!(
        error.contains("must be one of ACCELERATED_CHECKOUT, PAYMENT_METHOD"),
        "{error}"
    );
    let error = output(PaymentCustomization, result, json!({"operations": null})).unwrap_err();
    assert_eq!(error, "`operations` is null but cannot be");
    let error = output(PaymentCustomization, result, json!([])).unwrap_err();
    assert!(error.contains("must be an object"), "{error}");
}

#[test]
fn decimals_ids_and_nullable_fields() {
    use FunctionApi::*;
    let line = |update: serde_json::Value| json!({"operations": [{"lineUpdate": update}]});
    let result = "CartTransformRunResult";
    // A decimal is a string or a number; an ID is a string; optional fields may be null.
    for amount in [json!("7.50"), json!(7.5), json!(7)] {
        let value = line(json!({
            "cartLineId": "gid://shopify/CartLine/1", "title": null, "image": null,
            "price": {"adjustment": {"fixedPricePerUnit": {"amount": amount}}}
        }));
        assert_eq!(output(CartTransform, result, value), Ok(()));
    }
    let bad = line(
        json!({"cartLineId": "1", "price": {"adjustment": {"fixedPricePerUnit": {"amount": "abc"}}}}),
    );
    assert!(
        output(CartTransform, result, bad)
            .unwrap_err()
            .contains("must be a decimal")
    );
    let bad = line(json!({"cartLineId": 1}));
    assert_eq!(
        output(CartTransform, result, bad),
        Ok(()),
        "a number is coerced into an ID"
    );
    let bad = line(json!({"cartLineId": true}));
    assert!(
        output(CartTransform, result, bad)
            .unwrap_err()
            .contains("must be a string")
    );
}

#[test]
fn legacy_discount_results_keep_their_shapes() {
    use FunctionApi::*;
    let legacy = json!({
        "discountApplicationStrategy": "FIRST",
        "discounts": [{
            "targets": [{"productVariant": {"id": "gid://shopify/ProductVariant/1"}}],
            "value": {"percentage": {"value": "10"}},
            "message": "Ten off"
        }]
    });
    assert_eq!(
        output(ProductDiscount, "FunctionRunResult", legacy.clone()),
        Ok(())
    );
    let current = json!({"operations": [{"productDiscountsAdd": {
        "selectionStrategy": "FIRST",
        "candidates": [{
            "targets": [{"cartLine": {"id": "gid://shopify/CartLine/1"}}],
            "value": {"percentage": {"value": 10}},
            "message": "Ten off"
        }]
    }}]});
    assert_eq!(
        output(
            Discount,
            "CartLinesDiscountsGenerateRunResult",
            current.clone()
        ),
        Ok(())
    );
    // Each one against the other's type.
    assert!(output(Discount, "CartLinesDiscountsGenerateRunResult", legacy).is_err());
    assert!(output(ProductDiscount, "FunctionRunResult", current).is_err());
}

#[test]
fn the_validation_results_of_both_generations() {
    use FunctionApi::*;
    assert_eq!(
        output(
            CartCheckoutValidation,
            "FunctionRunResult",
            json!({"errors": [{"localizedMessage": "No", "target": "$.cart"}]})
        ),
        Ok(())
    );
    assert_eq!(
        output(
            CartCheckoutValidation,
            "CartValidationsGenerateRunResult",
            json!({"operations": [{"validationAdd": {"errors": [{"message": "No", "target": "$.cart"}]}}]})
        ),
        Ok(())
    );
}

#[test]
fn every_embedded_schema_parses_and_has_its_result_types() {
    for version in ApiVersion::all() {
        for api in FunctionApi::ALL {
            let schema = version.schema(api);
            assert!(schema.has_type("Input"), "{version} {api}");
            assert!(
                schema.has_type("FunctionRunResult") || api == FunctionApi::Discount,
                "{version} {api}"
            );
        }
    }
}

#[test]
fn every_target_names_a_result_type_of_its_schema() {
    for target in [
        "cart.transform.run",
        "purchase.cart-transform.run",
        "cart.lines.discounts.generate.run",
        "cart.delivery-options.discounts.generate.run",
        "purchase.product-discount.run",
        "purchase.order-discount.run",
        "purchase.shipping-discount.run",
        "cart.delivery-options.transform.run",
        "purchase.delivery-customization.run",
        "cart.payment-methods.transform.run",
        "purchase.payment-customization.run",
        "cart.validations.generate.run",
        "purchase.validation.run",
    ] {
        let spec = crate::version::target_spec(target).unwrap_or_else(|| panic!("{target}"));
        for version in ApiVersion::all() {
            assert!(
                version.schema(spec.api).has_type(spec.result),
                "{target}: {version} has no {}",
                spec.result
            );
        }
        assert_eq!(spec.legacy, target.starts_with("purchase."), "{target}");
    }
    assert!(crate::version::target_spec("cart.lines.discounts.generate.fetch").is_none());
}

/// Accepts the output, then normalizes it.
fn normalized(schema: &Schema, result: &str, mut value: serde_json::Value) -> serde_json::Value {
    schema.validate_output(result, &value).unwrap();
    schema.normalize_output(result, &mut value);
    value
}

#[test]
fn a_single_value_where_the_schema_has_a_list_becomes_a_list_of_one() {
    use FunctionApi::*;
    let schema = latest(CartCheckoutValidation);
    let out = normalized(
        schema,
        "CartValidationsGenerateRunResult",
        json!({"operations": {"validationAdd": {"errors": {"message": "No", "target": "$.cart"}}}}),
    );
    assert_eq!(
        out,
        json!({"operations": [{"validationAdd": {"errors": [{"message": "No", "target": "$.cart"}]}}]})
    );
    // Lists stay what they are, so does the legacy shape.
    let list = json!({"operations": [{"validationAdd": {"errors": [{"message": "No", "target": "$.cart"}]}}]});
    assert_eq!(
        normalized(schema, "CartValidationsGenerateRunResult", list.clone()),
        list
    );
    let legacy = normalized(
        schema,
        "FunctionRunResult",
        json!({"errors": {"localizedMessage": "No", "target": "$.cart"}}),
    );
    assert_eq!(
        legacy,
        json!({"errors": [{"localizedMessage": "No", "target": "$.cart"}]})
    );
}

#[test]
fn nested_lists_and_null_are_normalized_level_by_level() {
    let schema = Schema::parse(
        "
schema { query: Input }
type Input { id: ID }
input Result { grid: [[Cell!]!], names: [String!], any: JSON, next: Result }
input Cell { values: [Int!] }
scalar JSON
",
    )
    .unwrap();
    let out = normalized(
        &schema,
        "Result",
        json!({
            "grid": {"values": 1},
            "names": "a",
            "any": {"keep": 1},
            "next": {"grid": [{"values": [1, 2]}, [{"values": 3}]], "names": null},
        }),
    );
    assert_eq!(
        out,
        json!({
            "grid": [[{"values": [1]}]],
            "names": ["a"],
            "any": {"keep": 1},
            "next": {"grid": [[{"values": [1, 2]}], [{"values": [3]}]], "names": null},
        })
    );
}

#[test]
fn the_defaults_of_arguments_are_listed() {
    let schema = Schema::parse(
        "schema { query: Input }
         type Input { tags(list: [String!]! = [\"a\"], n: Int = 3, flag: Boolean = true, key: String!, other: String): Int }",
    )
    .unwrap();
    let mut defaults = schema.argument_defaults("Input", "tags");
    defaults.sort_by_key(|(name, _)| *name);
    assert_eq!(
        defaults,
        [
            ("flag", json!(true)),
            ("list", json!(["a"])),
            ("n", json!(3)),
        ]
    );
    assert!(schema.argument_defaults("Input", "nope").is_empty());
    assert!(schema.argument_defaults("Nope", "tags").is_empty());
    assert_eq!(
        latest(FunctionApi::Discount).argument_defaults("ProductVariant", "inCollections"),
        [("ids", json!([]))]
    );
}
