//! An error of a GraphQL response, in the shape graphql-ruby writes it: `message`, then
//! `locations`, `path` and `extensions` when the error has them.

use serde_json::{Map, Value, json};

use super::ast::Pos;

#[derive(Clone, Debug, PartialEq)]
pub struct Error {
    pub message: String,
    /// Where in the query the error is. graphql-ruby writes an empty list for some errors
    /// (`Unexpected end of document`), and no `locations` at all for others.
    pub locations: Option<Vec<Pos>>,
    /// The path of the error: in the response for a field error (keys and indexes), in the
    /// document for a validation error (`["query", "shop", "foo"]`).
    pub path: Option<Vec<Value>>,
    pub extensions: Option<Map<String, Value>>,
}

impl Error {
    pub fn new(message: impl Into<String>) -> Error {
        Error {
            message: message.into(),
            locations: None,
            path: None,
            extensions: None,
        }
    }

    pub fn with_locations(mut self, locations: Vec<Pos>) -> Error {
        self.locations = Some(locations);
        self
    }

    pub fn with_path(mut self, path: Vec<Value>) -> Error {
        self.path = Some(path);
        self
    }

    /// Sets an extension, creating `extensions` if there is none.
    pub fn with_extension(mut self, key: &str, value: impl Into<Value>) -> Error {
        self.extensions
            .get_or_insert_with(Map::new)
            .insert(key.to_string(), value.into());
        self
    }

    /// The code of the error (`extensions.code`), if it has one.
    pub fn code(&self) -> Option<&str> {
        self.extensions.as_ref()?.get("code")?.as_str()
    }

    pub fn to_json(&self) -> Value {
        let mut error = Map::new();
        error.insert("message".into(), Value::String(self.message.clone()));
        if let Some(locations) = &self.locations {
            let locations = locations
                .iter()
                .map(|pos| json!({"line": pos.line, "column": pos.column}))
                .collect();
            error.insert("locations".into(), Value::Array(locations));
        }
        if let Some(path) = &self.path {
            error.insert("path".into(), Value::Array(path.clone()));
        }
        if let Some(extensions) = &self.extensions {
            error.insert("extensions".into(), Value::Object(extensions.clone()));
        }
        Value::Object(error)
    }
}
