//! Request bodies and query strings, normalised to JSON whatever their encoding.
//!
//! Shopify's endpoints accept the same parameters as JSON, as a URL-encoded form or as
//! multipart form data. Bracketed names (`properties[Engraving]`, `updates[]`, `items[0][id]`)
//! become nested objects and arrays.

use serde_json::{Map, Value as Json};

fn insert(target: &mut Json, path: &[String], value: Json) {
    let Some((key, rest)) = path.split_first() else {
        *target = value;
        return;
    };
    // `name[]` appends to an array.
    if key.is_empty() {
        if !target.is_array() {
            *target = Json::Array(Vec::new());
        }
        let items = target.as_array_mut().expect("just made an array");
        let mut child = Json::Null;
        insert(&mut child, rest, value);
        items.push(child);
        return;
    }
    if !target.is_object() {
        *target = Json::Object(Map::new());
    }
    let object = target.as_object_mut().expect("just made an object");
    let child = object.entry(key.clone()).or_insert(Json::Null);
    insert(child, rest, value);
}

/// `items[0][id]` → `["items", "0", "id"]`.
fn split_name(name: &str) -> Vec<String> {
    let Some(open) = name.find('[') else {
        return vec![name.to_string()];
    };
    let mut path = vec![name[..open].to_string()];
    let mut rest = &name[open..];
    while let Some(stripped) = rest.strip_prefix('[') {
        let Some(close) = stripped.find(']') else {
            break;
        };
        path.push(stripped[..close].to_string());
        rest = &stripped[close + 1..];
    }
    path
}

/// Objects whose keys are all consecutive indexes are arrays (`items[0]`, `items[1]`).
fn arrays_from_indexes(value: Json) -> Json {
    match value {
        Json::Object(map) => {
            let is_indexed = !map.is_empty()
                && map
                    .keys()
                    .enumerate()
                    .all(|(index, key)| key == &index.to_string());
            if is_indexed {
                Json::Array(
                    map.into_iter()
                        .map(|(_, value)| arrays_from_indexes(value))
                        .collect(),
                )
            } else {
                Json::Object(
                    map.into_iter()
                        .map(|(key, value)| (key, arrays_from_indexes(value)))
                        .collect(),
                )
            }
        }
        Json::Array(items) => Json::Array(items.into_iter().map(arrays_from_indexes).collect()),
        other => other,
    }
}

/// Builds the nested parameters from decoded `name=value` pairs.
pub fn nest(pairs: impl IntoIterator<Item = (String, String)>) -> Json {
    let mut out = Json::Object(Map::new());
    for (name, value) in pairs {
        insert(&mut out, &split_name(&name), Json::String(value));
    }
    arrays_from_indexes(out)
}

/// Decodes a query string into pairs.
pub fn query_pairs(query: &str) -> Vec<(String, String)> {
    form_urlencoded::parse(query.as_bytes())
        .map(|(key, value)| (key.into_owned(), value.into_owned()))
        .collect()
}

/// Parses a request body according to its content type.
pub async fn parse_body(content_type: &str, body: bytes::Bytes) -> Json {
    if body.is_empty() {
        return Json::Object(Map::new());
    }
    if content_type.starts_with("application/json") {
        return serde_json::from_slice(&body).unwrap_or(Json::Object(Map::new()));
    }
    if content_type.starts_with("multipart/form-data") {
        let Ok(boundary) = multer::parse_boundary(content_type) else {
            return Json::Object(Map::new());
        };
        let stream = futures_util::stream::once(async move { Ok::<_, std::io::Error>(body) });
        let mut multipart = multer::Multipart::new(stream, boundary);
        let mut pairs = Vec::new();
        while let Ok(Some(field)) = multipart.next_field().await {
            let Some(name) = field.name().map(str::to_string) else {
                continue;
            };
            if let Ok(text) = field.text().await {
                pairs.push((name, text));
            }
        }
        return nest(pairs);
    }
    nest(
        form_urlencoded::parse(&body)
            .map(|(key, value)| (key.into_owned(), value.into_owned()))
            .collect::<Vec<_>>(),
    )
}

/// A parameter as text, whether it arrived as a JSON string or number.
pub fn text(params: &Json, key: &str) -> Option<String> {
    match params.get(key)? {
        Json::String(text) => Some(text.clone()),
        Json::Number(number) => Some(number.to_string()),
        Json::Bool(flag) => Some(flag.to_string()),
        _ => None,
    }
}

pub fn integer(params: &Json, key: &str) -> Option<i64> {
    match params.get(key)? {
        Json::Number(number) => number
            .as_i64()
            .or_else(|| number.as_f64().map(|float| float as i64)),
        Json::String(text) => text.trim().parse().ok(),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pairs(list: &[(&str, &str)]) -> Vec<(String, String)> {
        list.iter()
            .map(|(key, value)| ((*key).to_string(), (*value).to_string()))
            .collect()
    }

    #[test]
    fn nests_bracketed_names() {
        let params = nest(pairs(&[
            ("id", "42"),
            ("properties[Engraving]", "Hi"),
            ("updates[]", "1"),
            ("updates[]", "2"),
            ("items[0][id]", "1"),
            ("items[1][id]", "2"),
        ]));
        assert_eq!(params["id"], "42");
        assert_eq!(params["properties"]["Engraving"], "Hi");
        assert_eq!(params["updates"], serde_json::json!(["1", "2"]));
        assert_eq!(params["items"][1]["id"], "2");
        assert_eq!(integer(&params, "id"), Some(42));
    }

    #[test]
    fn keeps_keyed_updates_as_an_object() {
        let params = nest(pairs(&[("updates[123]", "2"), ("updates[456]", "0")]));
        assert_eq!(params["updates"]["123"], "2");
    }
}
