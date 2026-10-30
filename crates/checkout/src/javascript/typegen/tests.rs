use std::path::{Path, PathBuf};

use serde_json::{Value, json};
use sha2::{Digest, Sha256};

use super::text::{convert_name, natural_compare, pascal_case, transform_comment};
use super::*;

// ---- the output of graphql-codegen ----
//
// The cases of `tests/fixtures/typegen/cases.json`: the schema of a Function API (or one made for
// the tests), the documents of a Function and its `codegen` section. What graphql-codegen wrote
// for each is in `expected.json` (its SHA-256, or the error) and, for three of them, in
// `expected/`. The README of the folder says how they were made.

fn fixtures() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/typegen")
}

fn read(path: &Path) -> String {
    std::fs::read_to_string(path).unwrap_or_else(|error| panic!("{}: {error}", path.display()))
}

fn json_file(path: &Path) -> Value {
    serde_json::from_str(&read(path)).unwrap_or_else(|error| panic!("{}: {error}", path.display()))
}

/// The schema of a case: one made for the tests, or the schema of a Function API.
fn schema_of(name: &str) -> String {
    let own = fixtures().join("schemas").join(format!("{name}.graphql"));
    if own.exists() {
        return read(&own);
    }
    read(&Path::new(env!("CARGO_MANIFEST_DIR")).join(format!("schemas/2026-10/{name}.graphql")))
}

/// What `generate` writes for a case, as a Function would run it: the plan of the `codegen`
/// section, the documents `src/*.graphql`.
fn run_case(case: &Value) -> Result<String, String> {
    let mut codegen = json!({
        "schema": "schema.graphql",
        "documents": "src/*.graphql",
        "generates": { "./generated/api.ts": { "plugins": ["typescript", "typescript-operations"] } },
        "config": { "omitOperationSuffix": true },
    });
    for (key, value) in case["codegen"].as_object().into_iter().flatten() {
        codegen[key] = value.clone();
    }
    let plan = Plan::from_package_json(&json!({ "codegen": codegen }))?.unwrap();
    assert_eq!(plan.documents, ["src/*.graphql"]);
    let documents: Vec<(String, String)> = case["documents"]
        .as_array()
        .unwrap()
        .iter()
        .map(|document| {
            let document = document.as_str().unwrap();
            let name = Path::new(document).file_name().unwrap().to_str().unwrap();
            (
                format!("src/{name}"),
                read(&fixtures().join("documents").join(document)),
            )
        })
        .collect();
    generate(
        &schema_of(case["schema"].as_str().unwrap()),
        &documents,
        &plan.options,
    )
}

fn sha256(text: &str) -> String {
    Sha256::digest(text.as_bytes())
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

fn first_difference(got: &str, expected: &str) -> String {
    let line = got
        .lines()
        .zip(expected.lines())
        .position(|(a, b)| a != b)
        .unwrap_or(got.lines().count().min(expected.lines().count()));
    format!(
        "line {}\n    got:      {}\n    expected: {}",
        line + 1,
        got.lines().nth(line).unwrap_or("<end>"),
        expected.lines().nth(line).unwrap_or("<end>"),
    )
}

#[test]
fn the_output_is_what_graphql_codegen_writes() {
    let cases = json_file(&fixtures().join("cases.json"));
    let expected = json_file(&fixtures().join("expected.json"));
    let dump = std::env::var_os("LSF_TYPEGEN_DUMP").map(PathBuf::from);
    let mut failures = Vec::new();
    let (mut compared, mut errors, mut full) = (0, 0, 0);
    let mut seen = std::collections::HashSet::new();
    for case in cases.as_array().unwrap() {
        let name = case["name"].as_str().unwrap();
        assert!(seen.insert(name), "two cases are named {name}");
        let result = run_case(case);
        if let (Some(dump), Ok(output)) = (&dump, &result) {
            std::fs::create_dir_all(dump).unwrap();
            std::fs::write(dump.join(format!("{name}.ts")), output).unwrap();
        }
        let whole = fixtures().join("expected").join(format!("{name}.ts"));
        match (result, expected.get(name)) {
            (Ok(output), None) if whole.exists() => {
                full += 1;
                let expected = read(&whole);
                if output != expected {
                    failures.push(format!("{name}: {}", first_difference(&output, &expected)));
                }
            }
            (Ok(output), Some(Value::String(hash))) => {
                if sha256(&output) != *hash {
                    failures.push(format!(
                        "{name}: SHA-256 {} instead of {hash}",
                        sha256(&output)
                    ));
                }
            }
            (Err(_), Some(Value::Object(error))) if error.contains_key("error") => errors += 1,
            (Ok(_), Some(Value::Object(error))) => {
                failures.push(format!(
                    "{name}: graphql-codegen fails ({}), this does not",
                    error["error"]
                ));
            }
            (Err(error), Some(_) | None) => {
                failures.push(format!(
                    "{name}: it fails, graphql-codegen does not: {error}"
                ));
            }
            (Ok(_), _) => failures.push(format!("{name}: nothing is expected (see README.md)")),
        }
        compared += 1;
    }
    assert!(
        failures.is_empty(),
        "{} of {compared} cases differ from graphql-codegen:\n{}",
        failures.len(),
        failures.join("\n")
    );
    assert!(
        full >= 2 && errors >= 10 && compared >= 100,
        "{full} {errors} {compared}"
    );
}

#[test]
fn every_function_api_is_covered() {
    let cases = json_file(&fixtures().join("cases.json"));
    let apis: Vec<&str> = cases
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|case| case["schema"].as_str())
        .collect();
    for api in [
        "cart_checkout_validation",
        "cart_transform",
        "delivery_customization",
        "discount",
        "order_discount",
        "payment_customization",
        "product_discount",
        "shipping_discount",
    ] {
        assert!(apis.contains(&api), "no case for the schema {api}");
    }
    // The schemas of the repository are the ones of the cases: the folder only has these.
    let schemas = Path::new(env!("CARGO_MANIFEST_DIR")).join("schemas/2026-10");
    assert_eq!(std::fs::read_dir(schemas).unwrap().count(), 8);
}

/// The cases of `tests/fixtures/typegen/fuzz.mjs` in the folder `LSF_TYPEGEN_CORPUS`: random
/// documents on a schema, and what graphql-codegen wrote for each. A case can have, in its
/// `case.json`, the `config`, the `plugins` and the `outputConfig` of its `codegen` section.
#[test]
#[ignore = "needs a folder of cases made by tests/fixtures/typegen/fuzz.mjs (LSF_TYPEGEN_CORPUS)"]
fn the_output_is_what_graphql_codegen_writes_for_random_documents() {
    let folder = std::env::var("LSF_TYPEGEN_CORPUS").expect("LSF_TYPEGEN_CORPUS is not set");
    let mut cases: Vec<PathBuf> = std::fs::read_dir(folder)
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .collect();
    cases.retain(|case| case.is_dir());
    cases.sort();
    let mut failures = Vec::new();
    for case in &cases {
        let mut documents: Vec<(String, String)> = Vec::new();
        for entry in std::fs::read_dir(case.join("src")).into_iter().flatten() {
            let path = entry.unwrap().path();
            let name = path.file_name().unwrap().to_string_lossy().to_string();
            documents.push((format!("src/{name}"), read(&path)));
        }
        let settings = match case.join("case.json") {
            path if path.exists() => json_file(&path),
            _ => json!({}),
        };
        let mut output = json!({
            "plugins": settings.get("plugins").unwrap_or(&json!(["typescript", "typescript-operations"])),
        });
        if let Some(config) = settings.get("outputConfig") {
            output["config"] = config.clone();
        }
        let package = json!({ "codegen": {
            "schema": "schema.graphql",
            "documents": "src/*.graphql",
            "generates": { "./generated/api.ts": output },
            "config": settings.get("config").unwrap_or(&json!({ "omitOperationSuffix": true })),
        } });
        let plan = Plan::from_package_json(&package).unwrap().unwrap();
        let result = generate(
            &read(&case.join("schema.graphql")),
            &documents,
            &plan.options,
        );
        let name = case.file_name().unwrap().to_string_lossy();
        match (
            result,
            std::fs::read_to_string(case.join("generated/api.ts")),
        ) {
            (Ok(output), Ok(expected)) if output != expected => {
                failures.push(format!("{name}: {}", first_difference(&output, &expected)));
            }
            (Ok(_), Err(_)) => {
                failures.push(format!("{name}: graphql-codegen fails, this does not"))
            }
            (Err(error), Ok(_)) => failures.push(format!("{name}: this fails: {error}")),
            _ => {}
        }
    }
    assert!(
        failures.is_empty(),
        "{} of {} cases differ:\n{}",
        failures.len(),
        cases.len(),
        failures.join("\n")
    );
}

// ---- the `codegen` section ----

fn plan(codegen: Value) -> Result<Option<Plan>, String> {
    Plan::from_package_json(&json!({ "codegen": codegen }))
}

fn shopify_function() -> Value {
    json!({
        "schema": "schema.graphql",
        "documents": "src/*.graphql",
        "generates": { "./generated/api.ts": { "plugins": ["typescript", "typescript-operations"] } },
        "config": { "omitOperationSuffix": true }
    })
}

#[test]
fn a_package_without_codegen_has_no_plan() {
    assert_eq!(
        Plan::from_package_json(&json!({ "name": "x", "scripts": {} })),
        Ok(None)
    );
}

#[test]
fn the_codegen_section_of_a_shopify_template_is_a_plan() {
    let plan = plan(shopify_function()).unwrap().unwrap();
    assert_eq!(plan.schema, "schema.graphql");
    assert_eq!(plan.documents, ["src/*.graphql"]);
    assert_eq!(plan.output, "./generated/api.ts");
    let options = Options {
        omit_operation_suffix: true,
        documents_required: true,
        ..Options::default()
    };
    assert_eq!(plan.options, options);
}

#[test]
fn the_defaults_are_the_ones_of_shopifys_cli() {
    let plan = plan(json!({
        "schema": "schema.graphql",
        "generates": { "generated/api.ts": ["typescript"] },
    }))
    .unwrap()
    .unwrap();
    assert_eq!(plan.documents, Vec::<String>::new());
    assert_eq!(plan.output, "generated/api.ts");
    let options = plan.options;
    assert_eq!(options.plugins, [Plugin::Typescript]);
    assert_eq!(options.default_scalar_type, "unknown");
    assert!(!options.omit_operation_suffix);
    assert!(!options.documents_required);
    let scalar = |name: &str| options.scalars[name].clone();
    assert_eq!(scalar("Decimal").input, "string");
    assert_eq!(scalar("JSON").output, "unknown");
    assert_eq!(scalar("Void").output, "null");
    assert_eq!(options.scalars.len(), 10);
}

#[test]
fn documents_are_a_path_or_a_list_of_paths() {
    let with = |documents: Value| {
        let mut codegen = shopify_function();
        codegen["documents"] = documents;
        plan(codegen).map(|plan| plan.unwrap().documents)
    };
    assert_eq!(with(json!("src/*.graphql")).unwrap(), ["src/*.graphql"]);
    assert_eq!(
        with(json!(["src/a.graphql", "queries/*.graphql"])).unwrap(),
        ["src/a.graphql", "queries/*.graphql"]
    );
    assert_eq!(with(json!([])).unwrap(), Vec::<String>::new());
    assert!(with(json!(3)).unwrap_err().contains("`codegen.documents`"));
    assert!(
        with(json!(["a", 3]))
            .unwrap_err()
            .contains("`codegen.documents`")
    );
}

#[test]
fn a_plugin_other_than_the_two_is_an_error() {
    let mut codegen = shopify_function();
    codegen["generates"] =
        json!({ "./generated/api.ts": { "plugins": ["typescript", "typed-document-node"] } });
    let error = plan(codegen).unwrap_err();
    assert!(error.contains("`typed-document-node`"), "{error}");
    assert!(
        error.contains("`typescript` and `typescript-operations`"),
        "{error}"
    );

    let mut codegen = shopify_function();
    codegen["generates"] = json!({ "./generated/": { "preset": "client" } });
    let error = plan(codegen).unwrap_err();
    assert!(error.contains("`preset`"), "{error}");
}

#[test]
fn only_one_output_is_written() {
    let mut codegen = shopify_function();
    codegen["generates"] = json!({
        "./generated/api.ts": { "plugins": ["typescript"] },
        "./generated/types.ts": { "plugins": ["typescript"] },
    });
    let error = plan(codegen).unwrap_err();
    assert!(error.contains("writes 2 files"), "{error}");

    let mut codegen = shopify_function();
    codegen["generates"] = json!({});
    assert!(plan(codegen).unwrap_err().contains("writes 0 files"));
}

#[test]
fn the_plugins_are_in_the_order_of_the_output() {
    let mut codegen = shopify_function();
    codegen["generates"] = json!({ "./api.ts": [{ "typescript-operations": {} }, "typescript"] });
    let options = plan(codegen).unwrap().unwrap().options;
    assert_eq!(options.plugins, [Plugin::Operations, Plugin::Typescript]);

    let mut codegen = shopify_function();
    codegen["generates"] = json!({ "./api.ts": { "plugins": ["typescript", "typescript"] } });
    assert!(plan(codegen).unwrap_err().contains("twice"));

    let mut codegen = shopify_function();
    codegen["generates"] =
        json!({ "./api.ts": { "plugins": [{ "typescript": { "enumsAsTypes": true } }] } });
    assert!(plan(codegen).unwrap_err().contains("its own config"));
}

#[test]
fn the_scalars_of_the_config_are_merged_with_the_ones_of_the_cli() {
    let mut codegen = shopify_function();
    codegen["config"] = json!({
        "scalars": { "Money": "number", "Decimal": { "input": "string | number", "output": "string" } },
    });
    let options = plan(codegen).unwrap().unwrap().options;
    assert_eq!(options.scalars["Money"].output, "number");
    assert_eq!(options.scalars["Decimal"].input, "string | number");
    assert_eq!(options.scalars["Decimal"].output, "string");
    assert_eq!(options.scalars["URL"].input, "string");
    assert_eq!(options.scalars.len(), 11);
}

#[test]
fn the_config_of_the_output_replaces_the_one_of_the_section_key_by_key() {
    let mut codegen = shopify_function();
    codegen["config"] = json!({
        "omitOperationSuffix": true, "enumsAsTypes": true, "scalars": { "Money": "number" },
    });
    codegen["generates"] = json!({
        "./generated/api.ts": {
            "plugins": ["typescript"],
            "config": { "enumsAsTypes": false, "immutableTypes": true, "scalars": { "Void": "never" } },
        },
    });
    let options = plan(codegen).unwrap().unwrap().options;
    assert!(options.omit_operation_suffix);
    assert!(!options.enums_as_types);
    assert!(options.immutable_types);
    // The scalars of the output are the only ones: not the ones of Shopify's CLI, not the ones of
    // the section.
    assert_eq!(options.scalars.len(), 1);
    assert_eq!(options.scalars["Void"].output, "never");
}

#[test]
fn a_null_setting_is_the_default() {
    let mut codegen = shopify_function();
    codegen["config"] = json!({ "omitOperationSuffix": null, "defaultScalarType": null });
    let options = plan(codegen).unwrap().unwrap().options;
    assert!(!options.omit_operation_suffix);
    assert_eq!(options.default_scalar_type, "any");
}

#[test]
fn every_supported_setting_is_read() {
    let mut codegen = shopify_function();
    codegen["config"] = json!({
        "omitOperationSuffix": false, "dedupeOperationSuffix": true, "operationResultSuffix": "Result",
        "enumsAsTypes": true, "enumsAsConst": true, "constEnums": true, "skipTypename": true,
        "skipTypeNameForRoot": true, "nonOptionalTypename": true, "immutableTypes": true,
        "defaultScalarType": "string", "useTypeImports": true, "preResolveTypes": true,
    });
    let options = plan(codegen).unwrap().unwrap().options;
    assert_eq!(
        options,
        Options {
            plugins: vec![Plugin::Typescript, Plugin::Operations],
            dedupe_operation_suffix: true,
            operation_result_suffix: "Result".to_string(),
            enums_as_types: true,
            enums_as_const: true,
            const_enums: true,
            skip_typename: true,
            skip_typename_for_root: true,
            non_optional_typename: true,
            immutable_types: true,
            default_scalar_type: "string".to_string(),
            documents_required: true,
            ..Options::default()
        }
    );
}

#[test]
fn a_setting_that_is_not_supported_is_an_error_that_names_it() {
    for (key, value) in [
        ("namingConvention", json!("keep")),
        ("typesPrefix", json!("I")),
        ("avoidOptionals", json!(true)),
        ("maybeValue", json!("T | undefined")),
        ("inlineFragmentTypes", json!("combine")),
        ("preResolveTypes", json!(false)),
        ("enumValues", json!({})),
    ] {
        let mut codegen = shopify_function();
        codegen["config"] = json!({ key: value });
        let error = plan(codegen).unwrap_err();
        assert!(error.contains(&format!("`{key}`")), "{error}");

        let mut codegen = shopify_function();
        codegen["generates"] =
            json!({ "./api.ts": { "plugins": ["typescript"], "config": { key: value } } });
        assert!(plan(codegen).unwrap_err().contains(&format!("`{key}`")));
    }
}

#[test]
fn a_setting_of_the_wrong_type_is_an_error() {
    let mut codegen = shopify_function();
    codegen["config"] = json!({ "enumsAsTypes": "yes" });
    assert!(plan(codegen).unwrap_err().contains("`enumsAsTypes`"));

    let mut codegen = shopify_function();
    codegen["config"] = json!({ "scalars": "./scalars.ts" });
    assert!(plan(codegen).unwrap_err().contains("`scalars`"));

    let mut codegen = shopify_function();
    codegen["config"] = json!({ "scalars": { "Money": "./money#Money" } });
    assert!(plan(codegen).unwrap_err().contains("imports its type"));
}

#[test]
fn the_section_needs_a_schema_and_an_output() {
    let mut codegen = shopify_function();
    codegen.as_object_mut().unwrap().remove("schema");
    assert!(plan(codegen).unwrap_err().contains("`codegen.schema`"));

    let mut codegen = shopify_function();
    codegen["schema"] = json!(["a.graphql", "b.graphql"]);
    assert!(plan(codegen).unwrap_err().contains("one schema file"));

    let mut codegen = shopify_function();
    codegen["schema"] = json!(["a.graphql"]);
    assert_eq!(plan(codegen).unwrap().unwrap().schema, "a.graphql");

    let mut codegen = shopify_function();
    codegen.as_object_mut().unwrap().remove("generates");
    assert!(plan(codegen).unwrap_err().contains("`codegen.generates`"));

    let mut codegen = shopify_function();
    codegen["hooks"] = json!({});
    assert!(plan(codegen).unwrap_err().contains("`codegen.hooks`"));
}

#[test]
fn no_document_is_an_error_unless_graphql_codegen_ignores_it() {
    let mut codegen = shopify_function();
    codegen["ignoreNoDocuments"] = json!(true);
    let options = plan(codegen).unwrap().unwrap().options;
    assert!(!options.documents_required);

    let plan = plan(shopify_function()).unwrap().unwrap();
    let schema = "type Query { a: Int }";
    let error = generate(schema, &[], &plan.options).unwrap_err();
    assert!(error.contains("no operation or fragment"), "{error}");
    let blank = [("src/a.graphql".to_string(), "# nothing\n".to_string())];
    assert!(generate(schema, &blank, &plan.options).is_err());
    let none = Options {
        documents_required: false,
        ..plan.options
    };
    assert!(
        generate(schema, &[], &none)
            .unwrap()
            .contains("export type Query")
    );
}

// ---- what `generate` does that the fixtures do not show ----

const SCHEMA: &str = "type Query { a: Int, b: String, u: U } type U { id: ID! }";

fn owned(documents: &[(&str, &str)]) -> Vec<(String, String)> {
    documents
        .iter()
        .map(|(path, text)| (path.to_string(), text.to_string()))
        .collect()
}

fn operations(documents: &[(&str, &str)]) -> Result<String, String> {
    let options = Options {
        plugins: vec![Plugin::Operations],
        omit_operation_suffix: true,
        ..Options::default()
    };
    generate(SCHEMA, &owned(documents), &options)
}

#[test]
fn the_documents_are_read_sorted_by_path() {
    let one = [
        ("src/b.graphql", "query Zed { a }"),
        ("src/a.graphql", "query Alpha { b }"),
    ];
    let output = operations(&one).unwrap();
    assert!(output.find("Alpha").unwrap() < output.find("Zed").unwrap());
    let two = [
        ("src/a.graphql", "query Alpha { b }"),
        ("src/b.graphql", "query Zed { a }"),
    ];
    assert_eq!(output, operations(&two).unwrap());
}

#[test]
fn an_operation_without_a_name_is_unnamed() {
    let output = operations(&[("a.graphql", "{ a }")]).unwrap();
    assert_eq!(
        output,
        "export type Unnamed_1_Variables = Exact<{ [key: string]: never; }>;\n\n\nexport type Unnamed_1_ = { __typename?: 'Query', a?: number | null };\n"
    );
    let error = operations(&[("a.graphql", "{ a }\nquery B { b }")]).unwrap_err();
    assert!(
        error.contains("This anonymous operation must be the only defined operation."),
        "{error}"
    );
    assert!(error.contains("a.graphql:1:1"), "{error}");
}

#[test]
fn a_definition_in_two_documents_must_be_the_same() {
    let same = [
        (
            "a.graphql",
            "fragment F on U { id }\nquery A { u { ...F } }",
        ),
        (
            "b.graphql",
            "fragment  F  on U {\n  id\n}\nquery B { u { ...F } }",
        ),
    ];
    let output = operations(&same).unwrap();
    assert_eq!(output.matches("export type F =").count(), 1);

    let different = [
        ("a.graphql", "query A { a }"),
        ("b.graphql", "query A { b }"),
    ];
    let error = operations(&different).unwrap_err();
    assert!(
        error.contains("the operation `A` is defined twice"),
        "{error}"
    );
    assert!(
        error.contains("a.graphql") && error.contains("b.graphql"),
        "{error}"
    );
}

#[test]
fn a_document_with_type_definitions_only_has_no_operation() {
    let output = operations(&[
        ("a.graphql", "query A { a }"),
        ("types.graphql", "type T { a: Int }"),
    ]);
    assert!(output.unwrap().contains("export type A ="));
}

#[test]
fn an_invalid_document_is_an_error_with_its_place() {
    let error = operations(&[("src/a.graphql", "query A {\n  nope\n}")]).unwrap_err();
    assert!(error.contains("src/a.graphql:2:3"), "{error}");
    assert!(
        error.contains("Cannot query field \"nope\" on type \"Query\"."),
        "{error}"
    );

    let error = operations(&[("src/a.graphql", "query A { a")]).unwrap_err();
    assert!(
        error.contains("src/a.graphql is not a valid GraphQL document"),
        "{error}"
    );
}

#[test]
fn what_graphql_codegen_does_differently_is_an_error() {
    let error = operations(&[("a.graphql", "query A { u @defer { id } }")]).unwrap_err();
    assert!(
        error.contains("`@defer`") || error.contains("@defer"),
        "{error}"
    );
    let error =
        operations(&[("a.graphql", "query A { __schema { queryType { name } } }")]).unwrap_err();
    assert!(error.contains("introspection"), "{error}");
}

#[test]
fn a_schema_that_does_not_parse_or_miss_a_type_is_an_error() {
    let options = Options::default();
    let error = generate("type Query {", &[], &options).unwrap_err();
    assert!(error.contains("the schema is not valid GraphQL"), "{error}");
    let error = generate("type Query { a: Nope }", &[], &options).unwrap_err();
    assert!(
        error.contains("`Nope`") && error.contains("`Query.a`"),
        "{error}"
    );
    let error = generate("type Query { a: Int } type Query { b: Int }", &[], &options).unwrap_err();
    assert!(error.contains("`Query` twice"), "{error}");
}

#[test]
fn extended_types_are_merged_in_the_schema() {
    let schema = "type Query { a: Int } extend type Query { b: String }";
    let output = generate(schema, &[], &Options::default()).unwrap();
    assert!(output.contains(
        "  a?: Maybe<Scalars['Int']['output']>;\n  b?: Maybe<Scalars['String']['output']>;"
    ));
}

// ---- names, order, comments ----

#[test]
fn names_are_in_pascal_case_like_change_case() {
    assert_eq!(pascal_case("cart_line"), "CartLine");
    assert_eq!(pascal_case("HTTPRequest"), "HttpRequest");
    assert_eq!(pascal_case("QDArgs"), "QdArgs");
    assert_eq!(pascal_case("ONE_TIME_PURCHASE"), "OneTimePurchase");
    assert_eq!(pascal_case("getUserV2"), "GetUserV2");
    assert_eq!(pascal_case("FOO_1BAR"), "Foo_1Bar");
    assert_eq!(pascal_case("FOO_1"), "Foo_1");
    assert_eq!(pascal_case("_under"), "Under");
    assert_eq!(pascal_case(""), "");
    // The underscores of a name stay, except for the values of an enum.
    assert_eq!(convert_name("my_query", false), "My_Query");
    assert_eq!(convert_name("Unnamed_1_", false), "Unnamed_1_");
    assert_eq!(convert_name("super_user", true), "SuperUser");
}

#[test]
fn the_schema_is_sorted_like_graphql_js_sorts_it() {
    let mut names = vec![
        "CartLinesDiscountsGenerateRunResult",
        "CartLineTarget",
        "a10",
        "a2",
        "B",
        "b",
        "_x",
        "Z9",
        "z10",
    ];
    names.sort_by(|a, b| natural_compare(a, b));
    assert_eq!(
        names,
        [
            "B",
            "CartLineTarget",
            "CartLinesDiscountsGenerateRunResult",
            "Z9",
            "_x",
            "a2",
            "a10",
            "b",
            "z10"
        ]
    );
}

#[test]
fn comments_are_written_like_graphql_codegen_writes_them() {
    assert_eq!(transform_comment("", 1), "");
    assert_eq!(transform_comment("One line", 1), "  /** One line */\n");
    assert_eq!(
        transform_comment("Two\n\nthree */ ", 0),
        "/**\n * Two\n *\n * three *\\/\n */\n"
    );
}
