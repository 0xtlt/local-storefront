//! What a field the record has no value for answers: `null` when it can, else the emptiest value
//! of its type, so that a query never fails on what lsf does not simulate.

use serde_json::{Value, json};

use super::Obj;
use crate::graphql::Output;
use crate::schema::{Kind, Schema, TypeRef};
use crate::state::Record;

/// The default of a value of type `ty`: `null` for a nullable type; for a non-null one, an empty
/// list, a scalar's zero, the first value of an enum, or an object of defaults (the first
/// possible type of an interface or a union). A connection is answered by the caller, which
/// pages over nothing.
pub(super) fn output(schema: &Schema, ty: &TypeRef) -> Output<Obj> {
    let TypeRef::NonNull(inner) = ty else {
        return Output::Null;
    };
    if inner.is_list() {
        return Output::List(Vec::new());
    }
    let named = inner.named();
    let Some(def) = schema.get(named) else {
        return Output::Null;
    };
    match def.kind {
        Kind::Scalar => Output::Value(scalar(named)),
        Kind::Enum => def
            .enum_values
            .keys()
            .next()
            .map_or(Output::Null, |value| Output::value(value.clone())),
        Kind::Object => Output::Object(Obj::built(named, Record::new())),
        Kind::Interface | Kind::Union => schema
            .possible_types(named)
            .first()
            .map_or(Output::Null, |object| {
                Output::Object(Obj::built(object, Record::new()))
            }),
        Kind::InputObject => Output::Null,
    }
}

/// The zero of a scalar.
fn scalar(name: &str) -> Value {
    match name {
        "Int" => json!(0),
        "Float" => json!(0.0),
        "Boolean" => json!(false),
        "Money" => json!("0.00"),
        "Decimal" => json!("0.0"),
        "UnsignedInt64" | "BigInt" => json!("0"),
        "DateTime" => json!(crate::state::seed::Seed::epoch_string()),
        "Date" => json!("2024-01-01"),
        "JSON" => json!({}),
        _ => json!(""),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn non_null(name: &str) -> TypeRef {
        TypeRef::NonNull(Box::new(TypeRef::Named(name.to_string())))
    }

    #[test]
    fn non_null_fields_get_the_emptiest_value() {
        let schema = crate::schema::admin();
        let value = |ty: TypeRef| match output(schema, &ty) {
            Output::Value(value) => value,
            other => panic!("{other:?}"),
        };
        assert_eq!(value(non_null("String")), json!(""));
        assert_eq!(value(non_null("Money")), json!("0.00"));
        assert_eq!(value(non_null("ProductStatus")), json!("ACTIVE"));
        assert_eq!(value(non_null("DateTime")), json!("2024-01-01T12:00:00Z"));
        assert!(matches!(
            output(schema, &TypeRef::Named("String".into())),
            Output::Null
        ));
        assert!(matches!(
            output(schema, &TypeRef::NonNull(Box::new(TypeRef::List(Box::new(non_null("String")))))),
            Output::List(items) if items.is_empty()
        ));
        let Output::Object(Obj::Inline(seo)) = output(schema, &non_null("SEO")) else {
            panic!("an object of defaults");
        };
        assert_eq!(seo.type_name, "SEO");
        assert!(!seo.reported);
    }
}
