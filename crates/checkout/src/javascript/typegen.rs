//! The GraphQL typegen of a JavaScript Function: the TypeScript that graphql-codegen writes
//! (`generated/api.ts`) for the Function API schema and the input queries, with the `typescript`
//! and `typescript-operations` plugins and the defaults of Shopify's CLI.
//!
//! The output is the same text, byte for byte, as `shopify app function typegen` writes: the
//! tests compare it with graphql-codegen's. What graphql-codegen does that this does not is an
//! error, never a different output: a setting of the `codegen` section that is not supported, a
//! document that uses `@defer` or an introspection field, a schema with an `extend` that
//! graphql-codegen merges in a way this does not. Documents that graphql-codegen refuses are
//! refused here too (see [`validate`] for the rules that are checked).

mod model;
mod operations;
mod text;
mod types;
mod validate;

use std::collections::{BTreeMap, HashMap};

use graphql_parser::Pos;
use graphql_parser::query as q;
use serde_json::{Map, Value};

use model::Model;
use operations::{Definition, Operations, Source};
use types::Types;

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

/// The TypeScript type of a scalar, for the values that go in (arguments, variables) and the
/// values that come out.
#[derive(Clone, Debug, PartialEq)]
pub struct Scalar {
    pub input: String,
    pub output: String,
}

/// A plugin of graphql-codegen the typegen runs.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Plugin {
    /// `typescript`: the types of the schema.
    Typescript,
    /// `typescript-operations`: the types of the operations and the fragments.
    Operations,
}

/// The options of graphql-codegen that change what is written, with the defaults of Shopify's CLI
/// (which sets `defaultScalarType` and maps the scalars of the Function APIs).
#[derive(Clone, Debug, PartialEq)]
pub struct Options {
    /// The plugins, in the order of the output; both by default.
    pub plugins: Vec<Plugin>,
    /// `scalars`: the TypeScript type of the scalars of the schema. A scalar that is not here is
    /// `default_scalar_type`.
    pub scalars: BTreeMap<String, Scalar>,
    /// `defaultScalarType`.
    pub default_scalar_type: String,
    /// `omitOperationSuffix`: `CartInput`, not `CartInputQuery`.
    pub omit_operation_suffix: bool,
    /// `dedupeOperationSuffix`: no second `Query` in `CartQueryQuery`.
    pub dedupe_operation_suffix: bool,
    /// `operationResultSuffix`.
    pub operation_result_suffix: String,
    /// `enumsAsTypes`: a union of strings for an enum, not a TypeScript `enum`.
    pub enums_as_types: bool,
    /// `enumsAsConst`: a `const` object and a type for an enum.
    pub enums_as_const: bool,
    /// `constEnums`: `const enum`.
    pub const_enums: bool,
    /// `skipTypename`: no `__typename` unless a query selects it.
    pub skip_typename: bool,
    /// `skipTypeNameForRoot`: no `__typename` for the root type of an operation either.
    pub skip_typename_for_root: bool,
    /// `nonOptionalTypename`: `__typename: 'X'`, not `__typename?: 'X'`.
    pub non_optional_typename: bool,
    /// `immutableTypes`: `readonly` fields and `ReadonlyArray`.
    pub immutable_types: bool,
    /// Whether it is an error that no document was found: the `documents` of the `codegen`
    /// section are not empty, and `ignoreNoDocuments` is not set.
    pub documents_required: bool,
}

impl Default for Options {
    /// Shopify's CLI defaults, and no `codegen.config`.
    fn default() -> Options {
        let string = |name: &str| Scalar {
            input: name.to_string(),
            output: name.to_string(),
        };
        let scalars = [
            ("Date", "string"),
            ("DateTime", "string"),
            ("DateTimeWithoutTimezone", "string"),
            ("Decimal", "string"),
            ("Handle", "string"),
            ("ID", "string"),
            ("JSON", "unknown"),
            ("TimeWithoutTimezone", "string"),
            ("URL", "string"),
            ("Void", "null"),
        ]
        .into_iter()
        .map(|(name, ty)| (name.to_string(), string(ty)))
        .collect();
        Options {
            plugins: vec![Plugin::Typescript, Plugin::Operations],
            scalars,
            default_scalar_type: "unknown".to_string(),
            omit_operation_suffix: false,
            dedupe_operation_suffix: false,
            operation_result_suffix: String::new(),
            enums_as_types: false,
            enums_as_const: false,
            const_enums: false,
            skip_typename: false,
            skip_typename_for_root: false,
            non_optional_typename: false,
            immutable_types: false,
            documents_required: false,
        }
    }
}

impl Options {
    pub(crate) fn add_typename(&self) -> bool {
        !self.skip_typename
    }
}

/// The keys of the `codegen` section that are only about how the CLI runs, not about what it
/// writes.
const HARMLESS_KEYS: [&str; 4] = ["overwrite", "silent", "verbose", "debug"];

type OutputSpec = (Vec<Plugin>, Option<Map<String, Value>>);

impl Plan {
    /// The plan of the `codegen` section of a `package.json`; `None` when it has none.
    ///
    /// The options are what graphql-codegen ends up with: the defaults of Shopify's CLI, over
    /// them the `config` of the section (its `scalars` merged with the defaults), over that the
    /// `config` of the output, key by key.
    pub fn from_package_json(package: &Value) -> Result<Option<Plan>, String> {
        let Some(codegen) = package.get("codegen") else {
            return Ok(None);
        };
        let codegen = codegen
            .as_object()
            .ok_or("`codegen` in package.json must be an object")?;
        for key in codegen.keys() {
            if ![
                "schema",
                "documents",
                "generates",
                "config",
                "ignoreNoDocuments",
            ]
            .contains(&key.as_str())
                && !HARMLESS_KEYS.contains(&key.as_str())
            {
                return Err(format!(
                    "`codegen.{key}` in package.json is not supported by lsf's typegen"
                ));
            }
        }
        if codegen.get("overwrite") == Some(&Value::Bool(false)) {
            return Err(
                "`codegen.overwrite: false` in package.json is not supported by lsf's typegen"
                    .to_string(),
            );
        }
        let schema = match codegen.get("schema") {
            Some(Value::String(schema)) => schema.clone(),
            Some(Value::Array(schemas)) if schemas.len() == 1 && schemas[0].is_string() => {
                schemas[0].as_str().unwrap().to_string()
            }
            Some(_) => {
                return Err(
                    "`codegen.schema` in package.json must be the path of one schema file"
                        .to_string(),
                );
            }
            None => return Err("`codegen.schema` is missing in package.json".to_string()),
        };
        let documents = match codegen.get("documents") {
            None | Some(Value::Null) => Vec::new(),
            Some(Value::String(path)) => vec![path.clone()],
            Some(Value::Array(paths)) => paths
                .iter()
                .map(|path| {
                    path.as_str().map(str::to_string).ok_or_else(|| {
                        "`codegen.documents` in package.json must be paths".to_string()
                    })
                })
                .collect::<Result<_, _>>()?,
            Some(_) => {
                return Err(
                    "`codegen.documents` in package.json must be a path or a list of paths"
                        .to_string(),
                );
            }
        };
        let generates = codegen
            .get("generates")
            .and_then(Value::as_object)
            .ok_or("`codegen.generates` in package.json must list the file to write")?;
        if generates.len() != 1 {
            return Err(format!(
                "`codegen.generates` in package.json writes {} files; lsf's typegen writes one",
                generates.len()
            ));
        }
        let (output, spec) = generates.iter().next().unwrap();
        let (plugins, output_config) = output_spec(output, spec)?;

        // The config the way Shopify's CLI merges it, then the config of the output over it.
        let mut options = Options {
            plugins,
            ..Options::default()
        };
        if let Some(config) = codegen.get("config") {
            let config = config
                .as_object()
                .ok_or("`codegen.config` in package.json must be an object")?;
            apply_config(&mut options, config, true)?;
        }
        if let Some(config) = output_config {
            apply_config(&mut options, &config, false)?;
        }
        let ignore_no_documents = match codegen.get("ignoreNoDocuments") {
            None | Some(Value::Null) => false,
            Some(Value::Bool(flag)) => *flag,
            Some(_) => return Err("`codegen.ignoreNoDocuments` must be true or false".to_string()),
        };
        options.documents_required = !documents.is_empty() && !ignore_no_documents;
        Ok(Some(Plan {
            schema,
            documents,
            output: output.clone(),
            options,
        }))
    }
}

/// The plugins of an output, and its `config`.
fn output_spec(output: &str, spec: &Value) -> Result<OutputSpec, String> {
    let (plugins, config) = match spec {
        Value::Array(plugins) => (plugins, None),
        Value::Object(spec) => {
            for key in spec.keys() {
                if key != "plugins" && key != "config" {
                    return Err(format!(
                        "`codegen.generates` of `{output}` has `{key}` in package.json, which is not supported by lsf's typegen"
                    ));
                }
            }
            let plugins = spec
                .get("plugins")
                .and_then(Value::as_array)
                .ok_or_else(|| format!("`codegen.generates` of `{output}` lists no `plugins`"))?;
            let config = match spec.get("config") {
                None | Some(Value::Null) => None,
                Some(Value::Object(config)) => Some(config.clone()),
                Some(_) => {
                    return Err(format!(
                        "`config` of `{output}` in `codegen.generates` must be an object"
                    ));
                }
            };
            (plugins, config)
        }
        _ => {
            return Err(format!(
                "`codegen.generates` of `{output}` must list plugins"
            ));
        }
    };
    let mut found = Vec::new();
    for plugin in plugins {
        let (name, plugin_config) = match plugin {
            Value::String(name) => (name.as_str(), None),
            Value::Object(object) if object.len() == 1 => {
                let (name, config) = object.iter().next().unwrap();
                (name.as_str(), Some(config))
            }
            _ => {
                return Err(format!(
                    "a plugin of `{output}` in `codegen.generates` must be a name"
                ));
            }
        };
        let plugin = match name {
            "typescript" => Plugin::Typescript,
            "typescript-operations" => Plugin::Operations,
            other => {
                return Err(format!(
                    "the plugin `{other}` of `codegen.generates` is not supported: lsf's typegen runs `typescript` and `typescript-operations`"
                ));
            }
        };
        if plugin_config.is_some_and(|config| {
            !config.is_null() && config.as_object().is_none_or(|object| !object.is_empty())
        }) {
            return Err(format!(
                "the plugin `{name}` has its own config in `codegen.generates`; put the options in `config`"
            ));
        }
        if found.contains(&plugin) {
            return Err(format!(
                "the plugin `{name}` is listed twice in `codegen.generates`"
            ));
        }
        found.push(plugin);
    }
    if found.is_empty() {
        return Err(format!("`codegen.generates` of `{output}` lists no plugin"));
    }
    Ok((found, config))
}

/// Applies the keys of a `config` over the options. The `scalars` of the config of the section
/// are merged with the scalars that are there (Shopify's CLI does that); those of the config of an
/// output replace them (`merge_scalars` is false), as in graphql-codegen.
fn apply_config(
    options: &mut Options,
    config: &Map<String, Value>,
    merge_scalars: bool,
) -> Result<(), String> {
    let flag = |key: &str, value: &Value| -> Result<Option<bool>, String> {
        match value {
            Value::Null => Ok(None),
            Value::Bool(flag) => Ok(Some(*flag)),
            _ => Err(format!(
                "`{key}` in the codegen config must be true or false"
            )),
        }
    };
    let text = |key: &str, value: &Value| -> Result<Option<String>, String> {
        match value {
            Value::Null => Ok(None),
            Value::String(text) => Ok(Some(text.clone())),
            _ => Err(format!("`{key}` in the codegen config must be a string")),
        }
    };
    for (key, value) in config {
        match key.as_str() {
            "omitOperationSuffix" => {
                options.omit_operation_suffix = flag(key, value)?.unwrap_or(false)
            }
            "dedupeOperationSuffix" => {
                options.dedupe_operation_suffix = flag(key, value)?.unwrap_or(false)
            }
            "enumsAsTypes" => options.enums_as_types = flag(key, value)?.unwrap_or(false),
            "enumsAsConst" => options.enums_as_const = flag(key, value)?.unwrap_or(false),
            "constEnums" => options.const_enums = flag(key, value)?.unwrap_or(false),
            "skipTypename" => options.skip_typename = flag(key, value)?.unwrap_or(false),
            "skipTypeNameForRoot" => {
                options.skip_typename_for_root = flag(key, value)?.unwrap_or(false)
            }
            "nonOptionalTypename" => {
                options.non_optional_typename = flag(key, value)?.unwrap_or(false)
            }
            "immutableTypes" => options.immutable_types = flag(key, value)?.unwrap_or(false),
            "operationResultSuffix" => {
                options.operation_result_suffix = text(key, value)?.unwrap_or_default();
            }
            "defaultScalarType" => {
                // An empty string is falsy for graphql-codegen: the default is `any`.
                options.default_scalar_type = text(key, value)?
                    .filter(|text| !text.is_empty())
                    .unwrap_or_else(|| "any".to_string());
            }
            "scalars" => {}
            // These two do not change what is written when nothing is imported.
            "useTypeImports" | "emitLegacyCommonJSImports" => {
                flag(key, value)?;
            }
            "preResolveTypes" => {
                if flag(key, value)? == Some(false) {
                    return Err(unsupported(key));
                }
            }
            _ => return Err(unsupported(key)),
        }
    }
    if let Some(scalars) = config.get("scalars") {
        let base = if merge_scalars {
            options.scalars.clone()
        } else {
            BTreeMap::new()
        };
        options.scalars = parse_scalars(scalars, &base)?;
    }
    Ok(())
}

fn unsupported(key: &str) -> String {
    format!("`{key}` in the codegen config is not supported by lsf's typegen")
}

/// The `scalars` of a config over `base`.
fn parse_scalars(
    value: &Value,
    base: &BTreeMap<String, Scalar>,
) -> Result<BTreeMap<String, Scalar>, String> {
    let object = value
        .as_object()
        .ok_or("`scalars` in the codegen config must map scalar names to types")?;
    let mut scalars = base.clone();
    for (name, value) in object {
        let scalar = match value {
            Value::String(ty) => Scalar {
                input: ty.clone(),
                output: ty.clone(),
            },
            Value::Object(object) => match (object.get("input"), object.get("output")) {
                (Some(Value::String(input)), Some(Value::String(output))) => Scalar {
                    input: input.clone(),
                    output: output.clone(),
                },
                _ => {
                    return Err(format!(
                        "the scalar `{name}` in the codegen config needs an `input` and an `output` type"
                    ));
                }
            },
            _ => {
                return Err(format!(
                    "the scalar `{name}` in the codegen config must be a type"
                ));
            }
        };
        if scalar.input.contains('#') || scalar.output.contains('#') {
            return Err(format!(
                "the scalar `{name}` in the codegen config imports its type from a module, which lsf's typegen does not support"
            ));
        }
        scalars.insert(name.clone(), scalar);
    }
    Ok(scalars)
}

/// The TypeScript for `schema` (GraphQL SDL) and the operations of `documents` (path and text).
///
/// graphql-codegen reads the documents sorted by the path of their file (compared as text), and
/// so does this: the order of `documents` does not matter, and the paths must all be relative to
/// the same folder. The caller expands the globs and reads the files.
pub fn generate(
    schema: &str,
    documents: &[(String, String)],
    options: &Options,
) -> Result<String, String> {
    let model = Model::parse(schema)?;
    let scalars = types::scalar_map(&model, options);
    let sources = load_documents(documents)?;
    if sources.is_empty() && options.documents_required {
        return Err(
            "no operation or fragment was found in the documents of `codegen.documents`"
                .to_string(),
        );
    }
    validate::validate(&model, &sources)?;

    let mut prepend: Vec<String> = Vec::new();
    let mut contents = Vec::new();
    for plugin in &options.plugins {
        match plugin {
            Plugin::Typescript => {
                prepend.extend(types::HEADER.iter().map(|line| line.to_string()));
                let types = Types {
                    model: &model,
                    options,
                    scalars: &scalars,
                };
                contents.push(types.content());
            }
            Plugin::Operations => {
                let mut operations = Operations::new(&model, options, &scalars, &sources);
                contents.push(operations.content(&sources)?);
            }
        }
    }
    Ok(prepend
        .into_iter()
        .chain(contents)
        .filter(|part| !part.is_empty())
        .collect::<Vec<_>>()
        .join("\n"))
}

/// The operations and fragments of the documents, in the order graphql-codegen reads them: the
/// files sorted by path, a definition that is in two files word for word only once.
fn load_documents(documents: &[(String, String)]) -> Result<Vec<Source>, String> {
    let mut documents: Vec<&(String, String)> = documents.iter().collect();
    documents.sort_by(|a, b| a.0.cmp(&b.0));

    // The first place of a definition of each name, and its text; two definitions of one name
    // must be the same text.
    let mut seen: HashMap<(bool, String), (String, String)> = HashMap::new();
    let mut sources = Vec::new();
    for (position, (path, text)) in documents.iter().enumerate() {
        let definitions = parse_document(path, text, position)?;
        for definition in definitions {
            let (is_fragment, name) = match &definition {
                Definition::Operation(operation) => (false, operation_name(operation)),
                Definition::Fragment(fragment) => (true, Some(fragment.name.clone())),
            };
            if let Some(name) = name {
                let printed = definition.to_string();
                match seen.get(&(is_fragment, name.clone())) {
                    Some((_, first)) if *first == printed => continue,
                    Some((first_path, _)) => {
                        return Err(format!(
                            "the {} `{name}` is defined twice, differently: in {first_path} and in {path}",
                            if is_fragment { "fragment" } else { "operation" }
                        ));
                    }
                    None => {
                        seen.insert((is_fragment, name), (path.clone(), printed));
                    }
                }
            }
            sources.push(Source {
                path: path.clone(),
                definition,
            });
        }
    }
    Ok(sources)
}

fn operation_name(operation: &q::OperationDefinition<'static, String>) -> Option<String> {
    match operation {
        q::OperationDefinition::SelectionSet(_) => None,
        q::OperationDefinition::Query(query) => query.name.clone(),
        q::OperationDefinition::Mutation(mutation) => mutation.name.clone(),
        q::OperationDefinition::Subscription(subscription) => subscription.name.clone(),
    }
}

/// The definitions of a document. A document of type definitions only has none (graphql-codegen
/// reads only operations and fragments from a document).
///
/// The location of every selection set is made unique among the documents (`position` is the
/// number of the document): graphql-codegen caches the type of a selection set by where it is.
fn parse_document(path: &str, text: &str, position: usize) -> Result<Vec<Definition>, String> {
    if is_blank(text) {
        return Ok(Vec::new());
    }
    let document = match q::parse_query::<String>(text) {
        Ok(document) => document.into_static(),
        Err(error) => {
            if graphql_parser::parse_schema::<String>(text).is_ok() {
                return Ok(Vec::new());
            }
            return Err(format!("{path} is not a valid GraphQL document: {error}"));
        }
    };
    let offset = (position + 1) * 1_000_000;
    let mut definitions = document.definitions;
    for definition in &mut definitions {
        match definition {
            Definition::Operation(operation) => match operation {
                q::OperationDefinition::SelectionSet(set) => relocate(set, offset),
                q::OperationDefinition::Query(query) => relocate(&mut query.selection_set, offset),
                q::OperationDefinition::Mutation(mutation) => {
                    relocate(&mut mutation.selection_set, offset)
                }
                q::OperationDefinition::Subscription(subscription) => {
                    relocate(&mut subscription.selection_set, offset)
                }
            },
            Definition::Fragment(fragment) => relocate(&mut fragment.selection_set, offset),
        }
    }
    Ok(definitions)
}

/// Whether a document holds nothing but white space, commas and comments.
fn is_blank(text: &str) -> bool {
    text.lines().all(|line| {
        let code = line.split('#').next().unwrap_or("");
        code.chars()
            .all(|c| c.is_whitespace() || c == ',' || c == '\u{feff}')
    })
}

fn relocate(set: &mut q::SelectionSet<'static, String>, offset: usize) {
    set.span = (
        Pos {
            line: set.span.0.line + offset,
            column: set.span.0.column,
        },
        Pos {
            line: set.span.1.line + offset,
            column: set.span.1.column,
        },
    );
    for selection in &mut set.items {
        match selection {
            q::Selection::Field(field) => relocate(&mut field.selection_set, offset),
            q::Selection::InlineFragment(inline) => relocate(&mut inline.selection_set, offset),
            q::Selection::FragmentSpread(_) => {}
        }
    }
}

#[cfg(test)]
mod tests;
