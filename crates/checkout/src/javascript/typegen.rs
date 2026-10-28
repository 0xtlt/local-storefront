//! The GraphQL typegen of a JavaScript Function: the TypeScript that graphql-codegen writes
//! (`generated/api.ts`) for the Function API schema and the input queries, with the `typescript`
//! and `typescript-operations` plugins and the defaults of Shopify's CLI.

use serde_json::Value;

/// What the typegen of a Function writes, from the `codegen` section of its `package.json`.
#[derive(Clone, Debug, PartialEq)]
pub struct Plan {
    /// The schema, relative to the Function's folder (`schema.graphql`).
    pub schema: String,
    /// The documents, globs relative to the Function's folder (`src/*.graphql`).
    pub documents: Vec<String>,
    /// The file it writes, relative to the Function's folder (`./generated/api.ts`).
    pub output: String,
    pub options: Options,
}

/// The options of graphql-codegen that change what is written.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Options {}

impl Plan {
    /// The plan of the `codegen` section of a `package.json`; `None` when it has none.
    pub fn from_package_json(_package: &Value) -> Result<Option<Plan>, String> {
        Err("the typegen is not written yet".to_string())
    }
}

/// The TypeScript for `schema` (GraphQL SDL) and the operations of `documents` (path and text).
pub fn generate(
    _schema: &str,
    _documents: &[(String, String)],
    _options: &Options,
) -> Result<String, String> {
    Err("the typegen is not written yet".to_string())
}
