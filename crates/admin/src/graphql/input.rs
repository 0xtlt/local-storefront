//! Input values: the literals of a document and the values of variables, checked against their
//! type as graphql-ruby checks them (its `LiteralValidator` for literals, `validate_input` for
//! variables, with the same explanations), and coerced to the JSON a resolver reads.
//!
//! The scalars of the Admin API, and what lsf accepts for them (Shopify does not publish its
//! parsers; these follow the descriptions of the schema):
//!
//! | Scalar | Accepted input | Given to the resolver as |
//! |---|---|---|
//! | `Int` | an integer from -2^31 to 2^31 - 1 | a number |
//! | `Float` | a number | a number |
//! | `String`, `HTML`, `FormattedString`, `StorefrontID` | a string | the string |
//! | `Boolean` | `true` or `false` | the boolean |
//! | `ID` | a GID (`gid://shopify/Product/1`); an integer is read as its digits, and fails | the string |
//! | `DateTime` | ISO 8601: `2026-01-01T10:00:00Z`, with an offset, fractions of seconds, or a date alone | the string |
//! | `Date` | `YYYY-MM-DD` | the string |
//! | `Decimal`, `Money` | a decimal number, as a string (`"19.99"`) or a number | a string |
//! | `BigInt` | an integer, as a number or a string of digits | a string |
//! | `UnsignedInt64` | an integer from 0 to 2^64 - 1, as a number or a string | a string |
//! | `URL` | an absolute URI (`https://…`, `mailto:…`), without white space | the string |
//! | `Color` | `#RGB` or `#RRGGBB` | the string |
//! | `UtcOffset` | `+HH:MM` or `-HH:MM` | the string |
//! | `ARN` | `arn:<partition>:<service>:<region>:<account>:<resource>` | the string |
//! | `JSON` | any JSON value | the value |
//!
//! A value that is not one of these fails with graphql-ruby's `Could not coerce value … to …`,
//! except an `ID` that is not a GID, which fails as Shopify's does: ``Invalid global id `123` ``.

use serde_json::{Map, Number, Value};

use super::ast::{Value as Literal, ValueKind};
use super::print;
use crate::schema::{Kind, Schema, TypeRef};

/// What is wrong with an input value, in the shape of graphql-ruby's `problems`: where in the
/// value (`["lines", 0, "merchandiseId"]`), and why.
#[derive(Clone, Debug, PartialEq)]
pub struct Problem {
    pub path: Vec<Value>,
    pub explanation: String,
    /// The message of a coercion error (a scalar's own error), which graphql-ruby repeats.
    pub message: Option<String>,
}

impl Problem {
    fn new(explanation: impl Into<String>) -> Problem {
        Problem {
            path: Vec::new(),
            explanation: explanation.into(),
            message: None,
        }
    }

    fn at(mut self, key: impl Into<Value>) -> Problem {
        self.path.insert(0, key.into());
        self
    }

    pub fn to_json(&self) -> Value {
        let mut problem = Map::new();
        problem.insert("path".into(), Value::Array(self.path.clone()));
        problem.insert("explanation".into(), self.explanation.clone().into());
        if let Some(message) = &self.message {
            problem.insert("message".into(), message.clone().into());
        }
        Value::Object(problem)
    }
}

/// Why a scalar refused a value.
#[derive(Debug, PartialEq)]
enum ScalarError {
    /// graphql-ruby's `Could not coerce value … to …`.
    Invalid,
    /// A coercion error of the scalar, with its message.
    Coercion(String),
}

impl ScalarError {
    fn problem(self, type_name: &str, value: &Value) -> Problem {
        match self {
            ScalarError::Invalid => Problem::new(format!(
                "Could not coerce value {} to {type_name}",
                print::serialize(value)
            )),
            ScalarError::Coercion(message) => Problem {
                path: Vec::new(),
                explanation: message.clone(),
                message: Some(message),
            },
        }
    }
}

/// Checks a value against a scalar of the schema and gives its coerced value.
fn scalar(name: &str, value: &Value) -> Result<Value, ScalarError> {
    let text = value.as_str();
    let invalid = Err(ScalarError::Invalid);
    match name {
        "Int" => match value.as_i64() {
            Some(n) if value.is_i64() && i32::try_from(n).is_ok() => Ok(value.clone()),
            _ => invalid,
        },
        "Float" => match value.as_f64() {
            Some(n) if n.is_finite() => Ok(value.clone()),
            _ => invalid,
        },
        "Boolean" => match value {
            Value::Bool(_) => Ok(value.clone()),
            _ => invalid,
        },
        "ID" => {
            let id = match value {
                Value::String(text) => text.clone(),
                Value::Number(number) if number.is_i64() || number.is_u64() => number.to_string(),
                _ => return invalid,
            };
            if is_gid(&id) {
                Ok(Value::String(id))
            } else {
                Err(ScalarError::Coercion(format!("Invalid global id `{id}`")))
            }
        }
        "DateTime" => match text {
            Some(text) if is_date_time(text) || is_date(text) => Ok(value.clone()),
            _ => invalid,
        },
        "Date" => match text {
            Some(text) if is_date(text) => Ok(value.clone()),
            _ => invalid,
        },
        "Decimal" | "Money" => decimal(value).ok_or(ScalarError::Invalid),
        "BigInt" => integer(value, false).ok_or(ScalarError::Invalid),
        "UnsignedInt64" => integer(value, true).ok_or(ScalarError::Invalid),
        "URL" => match text {
            Some(text) if is_url(text) => Ok(value.clone()),
            _ => invalid,
        },
        "Color" => match text {
            Some(text) if is_color(text) => Ok(value.clone()),
            _ => invalid,
        },
        "UtcOffset" => match text {
            Some(text) if is_utc_offset(text) => Ok(value.clone()),
            _ => invalid,
        },
        "ARN" => match text {
            Some(text) if text.starts_with("arn:") && text.split(':').count() >= 6 => {
                Ok(value.clone())
            }
            _ => invalid,
        },
        "JSON" => Ok(value.clone()),
        // String, HTML, FormattedString, StorefrontID, and any scalar lsf does not know.
        _ => match value {
            Value::String(_) => Ok(value.clone()),
            _ if name == "String"
                || matches!(name, "HTML" | "FormattedString" | "StorefrontID") =>
            {
                invalid
            }
            _ => Ok(value.clone()),
        },
    }
}

/// `gid://<app>/<Type>/<id>`: a GID as Shopify's GIDs are (a query string may follow the id).
pub(super) fn is_gid(text: &str) -> bool {
    let Some(rest) = text.strip_prefix("gid://") else {
        return false;
    };
    let mut parts = rest.splitn(3, '/');
    let (Some(app), Some(ty), Some(id)) = (parts.next(), parts.next(), parts.next()) else {
        return false;
    };
    !app.is_empty()
        && !ty.is_empty()
        && ty.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
        && !id.is_empty()
        && !id.starts_with('?')
        && !text.chars().any(char::is_whitespace)
}

fn digits(text: &str, count: usize) -> bool {
    text.len() == count && text.bytes().all(|b| b.is_ascii_digit())
}

/// `YYYY-MM-DD`, a real day.
fn is_date(text: &str) -> bool {
    chrono::NaiveDate::parse_from_str(text, "%Y-%m-%d").is_ok() && text.len() == 10
}

/// ISO 8601 date and time: `YYYY-MM-DDTHH:MM[:SS[.fff]]` with `Z` or an offset, or none (read
/// in UTC, as Shopify reads it).
fn is_date_time(text: &str) -> bool {
    let Some((date, time)) = text.split_once(['T', 't', ' ']) else {
        return false;
    };
    if !is_date(date) {
        return false;
    }
    let (clock, zone) = match time.find(['Z', 'z', '+', '-']) {
        Some(index) => time.split_at(index),
        None => (time, ""),
    };
    let zone_ok = zone.is_empty()
        || zone.eq_ignore_ascii_case("z")
        || is_utc_offset(zone)
        || (zone.len() == 5 && digits(&zone[1..], 4));
    let (clock, fraction) = clock.split_once('.').unwrap_or((clock, "0"));
    let parts: Vec<&str> = clock.split(':').collect();
    let clock_ok = matches!(parts.len(), 2 | 3)
        && parts.iter().all(|part| digits(part, 2))
        && parts[0] <= "23"
        && parts[1] <= "59"
        && parts.get(2).is_none_or(|seconds| *seconds <= "60");
    zone_ok && clock_ok && !fraction.is_empty() && fraction.bytes().all(|b| b.is_ascii_digit())
}

fn is_utc_offset(text: &str) -> bool {
    let bytes = text.as_bytes();
    text.len() == 6
        && matches!(bytes[0], b'+' | b'-')
        && digits(&text[1..3], 2)
        && bytes[3] == b':'
        && digits(&text[4..6], 2)
}

fn is_color(text: &str) -> bool {
    text.strip_prefix('#')
        .is_some_and(|hex| matches!(hex.len(), 3 | 6) && hex.bytes().all(|b| b.is_ascii_hexdigit()))
}

/// An absolute URI: a scheme (`https:`), then something, without white space.
fn is_url(text: &str) -> bool {
    let Some((scheme, rest)) = text.split_once(':') else {
        return false;
    };
    let mut chars = scheme.chars();
    chars.next().is_some_and(|c| c.is_ascii_alphabetic())
        && chars.all(|c| c.is_ascii_alphanumeric() || matches!(c, '+' | '-' | '.'))
        && !rest.is_empty()
        && rest != "//"
        && !text.chars().any(char::is_whitespace)
}

/// A decimal, as a string: `"19.99"`, `"-1"`, `"1e3"` are; `"abc"` and `""` are not.
fn decimal(value: &Value) -> Option<Value> {
    let text = match value {
        Value::String(text) => text.trim().to_string(),
        Value::Number(number) => number.to_string(),
        _ => return None,
    };
    let unsigned = text.strip_prefix(['-', '+']).unwrap_or(&text);
    let (mantissa, exponent) = match unsigned.split_once(['e', 'E']) {
        Some((mantissa, exponent)) => (mantissa, Some(exponent)),
        None => (unsigned, None),
    };
    let (whole, fraction) = mantissa.split_once('.').unwrap_or((mantissa, ""));
    let ok = (!whole.is_empty() || !fraction.is_empty())
        && whole.bytes().all(|b| b.is_ascii_digit())
        && fraction.bytes().all(|b| b.is_ascii_digit())
        && exponent.is_none_or(|exponent| {
            let exponent = exponent.strip_prefix(['-', '+']).unwrap_or(exponent);
            !exponent.is_empty() && exponent.bytes().all(|b| b.is_ascii_digit())
        });
    ok.then_some(Value::String(text))
}

/// An integer as a string of digits; `unsigned` keeps it from 0 to 2^64 - 1.
fn integer(value: &Value, unsigned: bool) -> Option<Value> {
    let text = match value {
        Value::String(text) => text.clone(),
        Value::Number(number) if number.is_i64() || number.is_u64() => number.to_string(),
        Value::Number(number) => {
            // An integer too large for 64 bits, read with arbitrary precision.
            let text = number.to_string();
            if text.bytes().all(|b| b.is_ascii_digit() || b == b'-') {
                text
            } else {
                return None;
            }
        }
        _ => return None,
    };
    if unsigned {
        text.parse::<u64>().ok()?;
    } else {
        let digits = text.strip_prefix('-').unwrap_or(&text);
        if digits.is_empty() || !digits.bytes().all(|b| b.is_ascii_digit()) {
            return None;
        }
    }
    Some(Value::String(text))
}

// ---- literals ----

/// A constant literal as the JSON value graphql-ruby hands a scalar: `None` for an enum value
/// or a variable, which no scalar takes.
fn scalar_json(value: &Literal<'_>) -> Option<Value> {
    Some(match &value.kind {
        ValueKind::Int(text) | ValueKind::Float(text) => number(text)?,
        ValueKind::String(text) => Value::String(text.clone()),
        ValueKind::Boolean(flag) => Value::Bool(*flag),
        ValueKind::Null => Value::Null,
        ValueKind::List(items) => {
            Value::Array(items.iter().map(scalar_json).collect::<Option<_>>()?)
        }
        ValueKind::Object(fields) => Value::Object(
            fields
                .iter()
                .map(|field| Some((field.name.to_string(), scalar_json(&field.value)?)))
                .collect::<Option<_>>()?,
        ),
        ValueKind::Enum(_) | ValueKind::Variable(_) => return None,
    })
}

/// A number of the document as JSON. An integer keeps every digit.
fn number(text: &str) -> Option<Value> {
    if let Ok(n) = text.parse::<i64>() {
        return Some(Value::from(n));
    }
    if let Ok(n) = text.parse::<u64>() {
        return Some(Value::from(n));
    }
    if let Ok(number) = serde_json::from_str::<Number>(text) {
        return Some(Value::Number(number));
    }
    Number::from_f64(text.parse().ok()?).map(Value::Number)
}

/// Checks a literal against a type, as graphql-ruby's `LiteralValidator` does: `Ok` when it
/// fits, else the first problem when there is one to tell (a variable inside fits: its usage is
/// checked apart).
pub fn validate_literal(
    schema: &Schema,
    value: &Literal<'_>,
    ty: Option<&TypeRef>,
) -> Result<(), Option<Problem>> {
    let Some(ty) = ty else {
        return Err(None);
    };
    if value.is_null() {
        return if ty.is_non_null() { Err(None) } else { Ok(()) };
    }
    match ty {
        TypeRef::NonNull(inner) => validate_literal(schema, value, Some(inner)),
        TypeRef::List(item) => match &value.kind {
            ValueKind::List(items) => items
                .iter()
                .try_for_each(|item_value| validate_literal(schema, item_value, Some(item))),
            _ => validate_literal(schema, value, Some(item)),
        },
        TypeRef::Named(name) => {
            if value.variable().is_some() {
                return Ok(());
            }
            let Some(def) = schema.get(name) else {
                return Err(None);
            };
            match def.kind {
                Kind::Scalar => {
                    let json = scalar_json(value).ok_or(None)?;
                    scalar(name, &json)
                        .map(drop)
                        .map_err(|error| Some(error.problem(name, &json)))
                }
                Kind::Enum => match value.kind {
                    ValueKind::Enum(enum_value) if def.enum_values.contains_key(enum_value) => {
                        Ok(())
                    }
                    ValueKind::Enum(enum_value) => Err(Some(Problem::new(format!(
                        "Expected {} to be one of: {}",
                        print::string(enum_value),
                        enum_names(def)
                    )))),
                    _ => Err(None),
                },
                Kind::InputObject => {
                    let ValueKind::Object(fields) = &value.kind else {
                        return Err(None);
                    };
                    for field in def.input_fields.values() {
                        if field.is_required() && !fields.iter().any(|f| f.name == field.name) {
                            return Err(None);
                        }
                    }
                    if def.one_of && fields.len() != 1 {
                        return Err(Some(Problem::new(format!(
                            "`{}` is a OneOf type, so only one argument may be given (instead of {})",
                            def.name,
                            fields.len()
                        ))));
                    }
                    fields.iter().try_for_each(|field| {
                        validate_literal(
                            schema,
                            &field.value,
                            def.input_field(field.name).map(|f| &f.ty),
                        )
                    })
                }
                _ => Err(None),
            }
        }
    }
}

fn enum_names(def: &crate::schema::TypeDef) -> String {
    def.enum_values
        .keys()
        .map(String::as_str)
        .collect::<Vec<_>>()
        .join(", ")
}

/// The value of a literal for a resolver, coerced to its type: enum values as their names,
/// a single value given for a list as a list of one, the defaults of an input object's fields
/// filled in. `None` when the literal is a variable that has no value, which leaves the argument
/// out (its default applies).
pub fn coerce_literal(
    schema: &Schema,
    value: &Literal<'_>,
    ty: &TypeRef,
    variables: &Map<String, Value>,
) -> Option<Value> {
    if let Some(name) = value.variable() {
        return variables
            .get(name)
            .map(|value| coerce_value(schema, value, ty));
    }
    if value.is_null() {
        return Some(Value::Null);
    }
    Some(match ty {
        TypeRef::NonNull(inner) => return coerce_literal(schema, value, inner, variables),
        TypeRef::List(item) => match &value.kind {
            ValueKind::List(items) => Value::Array(
                items
                    .iter()
                    .map(|item_value| {
                        coerce_literal(schema, item_value, item, variables).unwrap_or(Value::Null)
                    })
                    .collect(),
            ),
            _ => Value::Array(vec![coerce_literal(schema, value, item, variables)?]),
        },
        TypeRef::Named(name) => {
            let Some(def) = schema.get(name) else {
                return scalar_json(value);
            };
            match (&value.kind, def.kind) {
                (ValueKind::Enum(enum_value), _) => Value::String(enum_value.to_string()),
                (ValueKind::Object(fields), Kind::InputObject) => {
                    let mut object = Map::new();
                    for field in def.input_fields.values() {
                        let given = fields.iter().find(|f| f.name == field.name);
                        let coerced = given
                            .and_then(|f| coerce_literal(schema, &f.value, &field.ty, variables));
                        if let Some(coerced) = coerced.or_else(|| field.default_value.clone()) {
                            object.insert(field.name.clone(), coerced);
                        }
                    }
                    Value::Object(object)
                }
                _ => {
                    let json = scalar_json(value).unwrap_or(Value::Null);
                    scalar(name, &json).unwrap_or(json)
                }
            }
        }
    })
}

// ---- variables ----

/// Checks the value of a variable against its type, as graphql-ruby's `validate_input` does:
/// every problem, each with where it is in the value.
pub fn validate_value(schema: &Schema, value: &Value, ty: &TypeRef) -> Vec<Problem> {
    let mut problems = Vec::new();
    check(schema, value, ty, &mut problems);
    problems
}

fn check(schema: &Schema, value: &Value, ty: &TypeRef, problems: &mut Vec<Problem>) {
    if value.is_null() {
        if ty.is_non_null() {
            problems.push(Problem::new("Expected value to not be null"));
        }
        return;
    }
    match ty {
        TypeRef::NonNull(inner) => check(schema, value, inner, problems),
        TypeRef::List(item) => {
            let items = match value {
                Value::Array(items) => items.as_slice(),
                single => std::slice::from_ref(single),
            };
            for (index, item_value) in items.iter().enumerate() {
                let mut inner = Vec::new();
                check(schema, item_value, item, &mut inner);
                problems.extend(inner.into_iter().map(|problem| problem.at(index)));
            }
        }
        TypeRef::Named(name) => {
            let Some(def) = schema.get(name) else {
                return;
            };
            match def.kind {
                Kind::Scalar => {
                    if let Err(error) = scalar(name, value) {
                        problems.push(error.problem(name, value));
                    }
                }
                Kind::Enum => {
                    let known = value
                        .as_str()
                        .is_some_and(|name| def.enum_values.contains_key(name));
                    if !known {
                        problems.push(Problem::new(format!(
                            "Expected {} to be one of: {}",
                            print::serialize(value),
                            enum_names(def)
                        )));
                    }
                }
                Kind::InputObject => {
                    let Value::Object(fields) = value else {
                        problems.push(Problem::new(format!(
                            "Expected {} to be a key-value object.",
                            value
                        )));
                        return;
                    };
                    for (key, field_value) in fields {
                        match def.input_field(key) {
                            None => problems.push(
                                Problem::new(format!("Field is not defined on {}", def.name))
                                    .at(key.clone()),
                            ),
                            Some(field) => {
                                let mut inner = Vec::new();
                                check(schema, field_value, &field.ty, &mut inner);
                                problems.extend(
                                    inner.into_iter().map(|problem| problem.at(key.clone())),
                                );
                            }
                        }
                    }
                    for field in def.input_fields.values() {
                        if field.is_required() && !fields.contains_key(&field.name) {
                            problems.push(
                                Problem::new("Expected value to not be null")
                                    .at(field.name.clone()),
                            );
                        }
                    }
                    if def.one_of {
                        if fields.len() == 1 {
                            if let Some((key, Value::Null)) = fields.iter().next() {
                                problems.push(Problem::new(format!(
                                    "'{}' requires exactly one argument, but '{key}' was `null`.",
                                    def.name
                                )));
                            }
                        } else {
                            problems.push(Problem::new(format!(
                                "'{}' requires exactly one argument, but {} were provided.",
                                def.name,
                                fields.len()
                            )));
                        }
                    }
                }
                _ => {}
            }
        }
    }
}

/// The value of a variable, valid for its type, coerced as [`coerce_literal`] coerces literals.
pub fn coerce_value(schema: &Schema, value: &Value, ty: &TypeRef) -> Value {
    if value.is_null() {
        return Value::Null;
    }
    match ty {
        TypeRef::NonNull(inner) => coerce_value(schema, value, inner),
        TypeRef::List(item) => match value {
            Value::Array(items) => Value::Array(
                items
                    .iter()
                    .map(|item_value| coerce_value(schema, item_value, item))
                    .collect(),
            ),
            single => Value::Array(vec![coerce_value(schema, single, item)]),
        },
        TypeRef::Named(name) => match schema.get(name).map(|def| (def, def.kind)) {
            Some((def, Kind::InputObject)) => {
                let Value::Object(fields) = value else {
                    return value.clone();
                };
                let mut object = Map::new();
                for field in def.input_fields.values() {
                    let coerced = fields
                        .get(&field.name)
                        .map(|given| coerce_value(schema, given, &field.ty))
                        .or_else(|| field.default_value.clone());
                    if let Some(coerced) = coerced {
                        object.insert(field.name.clone(), coerced);
                    }
                }
                Value::Object(object)
            }
            Some((_, Kind::Scalar)) => scalar(name, value).unwrap_or_else(|_| value.clone()),
            _ => value.clone(),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::schema::admin;
    use serde_json::json;

    fn ty(text: &str) -> TypeRef {
        let source = format!("query($v: {text}) {{ a }}");
        let document = crate::graphql::parse::parse(&source).unwrap();
        document.operations().next().unwrap().variables[0]
            .ty
            .clone()
    }

    #[test]
    fn checks_scalars() {
        assert_eq!(scalar("Int", &json!(1)), Ok(json!(1)));
        assert_eq!(scalar("Int", &json!(1.5)), Err(ScalarError::Invalid));
        assert_eq!(
            scalar("Int", &json!(4_000_000_000_i64)),
            Err(ScalarError::Invalid)
        );
        assert_eq!(scalar("Float", &json!(1)), Ok(json!(1)));
        assert_eq!(
            scalar("ID", &json!("gid://shopify/Product/1")),
            Ok(json!("gid://shopify/Product/1"))
        );
        assert_eq!(
            scalar("ID", &json!("gid://shopify/ProductImage/3?product_id=1")),
            Ok(json!("gid://shopify/ProductImage/3?product_id=1"))
        );
        assert_eq!(
            scalar("ID", &json!(123)),
            Err(ScalarError::Coercion("Invalid global id `123`".into()))
        );
        assert_eq!(
            scalar("ID", &json!("gid://shopify/Product/")),
            Err(ScalarError::Coercion(
                "Invalid global id `gid://shopify/Product/`".into()
            ))
        );
        assert!(scalar("DateTime", &json!("2026-01-01T10:00:00Z")).is_ok());
        assert!(scalar("DateTime", &json!("2026-01-01T10:00:00.123+02:00")).is_ok());
        assert!(scalar("DateTime", &json!("2026-01-01")).is_ok());
        assert!(scalar("DateTime", &json!("2026-13-01T10:00:00Z")).is_err());
        assert!(scalar("DateTime", &json!("yesterday")).is_err());
        assert!(scalar("Date", &json!("2026-02-29")).is_err());
        assert_eq!(scalar("Decimal", &json!(19.99)), Ok(json!("19.99")));
        assert_eq!(scalar("Money", &json!("19.99")), Ok(json!("19.99")));
        assert!(scalar("Money", &json!("19,99")).is_err());
        assert_eq!(scalar("UnsignedInt64", &json!(7)), Ok(json!("7")));
        assert!(scalar("UnsignedInt64", &json!(-7)).is_err());
        assert_eq!(scalar("BigInt", &json!("-12")), Ok(json!("-12")));
        assert!(scalar("URL", &json!("https://example.com/a?b")).is_ok());
        assert!(scalar("URL", &json!("example.com")).is_err());
        assert!(scalar("Color", &json!("#6A8D48")).is_ok());
        assert!(scalar("Color", &json!("6A8D48")).is_err());
        assert!(scalar("UtcOffset", &json!("-05:00")).is_ok());
        assert!(
            scalar(
                "ARN",
                &json!("arn:aws:events:us-east-1::event-source/aws.partner/shopify.com/1/x")
            )
            .is_ok()
        );
        assert_eq!(scalar("JSON", &json!({"a": [1]})), Ok(json!({"a": [1]})));
        assert_eq!(scalar("String", &json!(1)), Err(ScalarError::Invalid));
    }

    #[test]
    fn explains_invalid_variables_as_graphql_ruby() {
        let schema = admin();
        let problems = |value: Value, text: &str| -> Vec<Value> {
            validate_value(schema, &value, &ty(text))
                .iter()
                .map(Problem::to_json)
                .collect()
        };
        assert_eq!(
            problems(json!(null), "ID!"),
            [json!({"path": [], "explanation": "Expected value to not be null"})]
        );
        assert_eq!(
            problems(json!("1"), "ID!"),
            [
                json!({"path": [], "explanation": "Invalid global id `1`", "message": "Invalid global id `1`"})
            ]
        );
        assert_eq!(
            problems(json!(["gid://shopify/Product/1", 2.5]), "[ID!]!"),
            [json!({"path": [1], "explanation": "Could not coerce value 2.5 to ID"})]
        );
        assert_eq!(
            problems(json!("FOO"), "ProductStatus"),
            [
                json!({"path": [], "explanation": "Expected \"FOO\" to be one of: ACTIVE, ARCHIVED, DRAFT, UNLISTED"})
            ]
        );
        assert_eq!(
            problems(json!({"amount": "1.0", "nope": 1}), "MoneyInput!"),
            [
                json!({"path": ["nope"], "explanation": "Field is not defined on MoneyInput"}),
                json!({"path": ["currencyCode"], "explanation": "Expected value to not be null"}),
            ]
        );
        assert_eq!(
            problems(json!("x"), "MoneyInput"),
            [json!({"path": [], "explanation": "Expected \"x\" to be a key-value object."})]
        );
        assert_eq!(
            problems(
                json!({"id": "gid://shopify/Product/1", "handle": "a"}),
                "ProductIdentifierInput"
            ),
            [
                json!({"path": [], "explanation": "'ProductIdentifierInput' requires exactly one argument, but 2 were provided."})
            ]
        );
        assert!(problems(json!("gid://shopify/Product/1"), "[ID!]").is_empty());
    }

    #[test]
    fn coerces_values_for_resolvers() {
        let schema = admin();
        assert_eq!(
            coerce_value(schema, &json!("gid://shopify/Product/1"), &ty("[ID!]")),
            json!(["gid://shopify/Product/1"])
        );
        assert_eq!(
            coerce_value(
                schema,
                &json!({"amount": 5, "currencyCode": "EUR"}),
                &ty("MoneyInput")
            ),
            json!({"amount": "5", "currencyCode": "EUR"})
        );
    }
}
