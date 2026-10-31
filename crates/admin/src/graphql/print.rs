//! Values written as graphql-ruby writes them in its messages: its printer for the values of a
//! document (`{title: "Shirt", status: ACTIVE}`), and `GraphQL::Language.serialize` for the
//! values of variables (`{title:"Shirt"}`).

use serde_json::Value;

use super::ast::{Value as Literal, ValueKind};

/// A value of a document, as graphql-ruby's printer writes it: strings in double quotes, enum
/// values bare, `[a, b]`, `{key: value}`. Numbers are written as Ruby writes the number it
/// read: `1.50` is `1.5`, `1e3` is `1000.0`.
pub fn literal(value: &Literal<'_>) -> String {
    let mut out = String::new();
    write_literal(value, &mut out);
    out
}

fn write_literal(value: &Literal<'_>, out: &mut String) {
    match &value.kind {
        ValueKind::Variable(name) => {
            out.push('$');
            out.push_str(name);
        }
        ValueKind::Int(text) => out.push_str(&int(text)),
        ValueKind::Float(text) => out.push_str(&float(text.parse().unwrap_or(f64::NAN))),
        ValueKind::String(text) => out.push_str(&string(text)),
        ValueKind::Boolean(flag) => out.push_str(if *flag { "true" } else { "false" }),
        ValueKind::Null => out.push_str("null"),
        ValueKind::Enum(name) => out.push_str(name),
        ValueKind::List(items) => {
            out.push('[');
            for (index, item) in items.iter().enumerate() {
                if index > 0 {
                    out.push_str(", ");
                }
                write_literal(item, out);
            }
            out.push(']');
        }
        ValueKind::Object(fields) => {
            out.push('{');
            for (index, field) in fields.iter().enumerate() {
                if index > 0 {
                    out.push_str(", ");
                }
                out.push_str(field.name);
                out.push_str(": ");
                write_literal(&field.value, out);
            }
            out.push('}');
        }
    }
}

/// An integer of the document as Ruby's `Integer#to_s` writes it (`-0` is `0`).
pub fn int(text: &str) -> String {
    match text.parse::<i128>() {
        Ok(number) => number.to_string(),
        Err(_) => text.to_string(),
    }
}

/// A string in double quotes, escaped as JSON (graphql-ruby's printer uses `JSON.generate`).
pub fn string(text: &str) -> String {
    serde_json::to_string(text).unwrap_or_default()
}

/// A float as Ruby's `Float#to_s` writes it: `1.5`, `100.0`, `1.0e+20`, `1.0e-05`.
pub fn float(number: f64) -> String {
    if number.is_nan() {
        return "NaN".to_string();
    }
    if number.is_infinite() {
        return if number > 0.0 {
            "Infinity"
        } else {
            "-Infinity"
        }
        .to_string();
    }
    let magnitude = number.abs();
    if number == 0.0 || (1e-4..1e16).contains(&magnitude) {
        let text = number.to_string();
        return if text.contains('.') {
            text
        } else {
            format!("{text}.0")
        };
    }
    // Rust writes the shortest digits that read back the same, as Ruby does.
    let text = format!("{number:e}");
    let (mantissa, exponent) = text.split_once('e').unwrap_or((&text, "0"));
    let mantissa = if mantissa.contains('.') {
        mantissa.to_string()
    } else {
        format!("{mantissa}.0")
    };
    let exponent: i32 = exponent.parse().unwrap_or(0);
    let sign = if exponent < 0 { '-' } else { '+' };
    format!("{mantissa}e{sign}{:02}", exponent.abs())
}

/// A JSON value as `GraphQL::Language.serialize` writes it: `{key:value}` and `[a,b]` without
/// spaces, the rest as JSON.
pub fn serialize(value: &Value) -> String {
    match value {
        Value::Object(fields) => {
            let fields: Vec<String> = fields
                .iter()
                .map(|(key, value)| format!("{key}:{}", serialize(value)))
                .collect();
            format!("{{{}}}", fields.join(","))
        }
        Value::Array(items) => {
            let items: Vec<String> = items.iter().map(serialize).collect();
            format!("[{}]", items.join(","))
        }
        other => other.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn writes_floats_as_ruby() {
        assert_eq!(float(1.5), "1.5");
        assert_eq!(float(100.0), "100.0");
        assert_eq!(float(-2500.0), "-2500.0");
        assert_eq!(float(0.0), "0.0");
        assert_eq!(float(1e20), "1.0e+20");
        assert_eq!(float(1.5e-5), "1.5e-05");
    }

    #[test]
    fn writes_literals_and_variables() {
        let document = crate::graphql::parse::parse(
            "{ a(b: {s: \"x\\\"y\", n: [1, -0, 2.50, E], v: $v, z: null}) }",
        )
        .unwrap();
        let Some(crate::graphql::ast::Definition::Operation(operation)) =
            document.definitions.first()
        else {
            panic!()
        };
        let crate::graphql::ast::Selection::Field(field) = &operation.selection_set.items[0] else {
            panic!()
        };
        assert_eq!(
            literal(&field.arguments[0].value),
            "{s: \"x\\\"y\", n: [1, 0, 2.5, E], v: $v, z: null}"
        );
        assert_eq!(
            serialize(&serde_json::json!({"a": [1, "b"], "c": null})),
            "{a:[1,\"b\"],c:null}"
        );
    }
}
