//! What a version shows differently from the records: its adapters.
//!
//! The shops keep one set of records, in one shape: that of [`super::DEFAULT`], which the seed,
//! the mutations and the payload builders write and read. A version is a view of them:
//!
//! - **What exists** is its schema's business: a field, an argument, an enum value or a mutation
//!   the version does not have is refused by validation, as Shopify refuses it (`Field
//!   'automaticDiscounts' doesn't exist on type 'QueryRoot'`), before any resolver runs. A
//!   resolver or a mutation registered for something only some versions have is only reached in
//!   those; nothing to gate by hand.
//! - **What reads differently** is an [`Adapter`] of the version, declared in its entry of
//!   `KNOWN` (`super::KNOWN`): a field renamed or reshaped, an enum value removed, an input given in
//!   another shape, a webhook payload with other keys. The resolvers and the builders stay those of
//!   the records.
//! - **What behaves differently** (a mutation whose rules changed) reads the request's version:
//!   `m.version().since("2027-01")`. Rare; an adapter is preferred when the difference is one of
//!   shape.
//!
//! An adapter maps any record to the version's view, whatever version wrote it: it is total
//! (a value the version has passes through), so that it holds when the default version moves.

use serde_json::{Map, Value};

use crate::state::Record;

/// One difference between a version's view and the records.
#[derive(Clone, Copy, Debug)]
pub enum Adapter {
    /// `type_name.field` in this version is `read` of the record (or inline object) it is a
    /// field of, rather than its value under `field`: the value that is then converted by the
    /// field's type, as a stored one is (a GID becomes its record, an object an inline one), or
    /// `None` when there is none. A renamed field reads the record's name for it, a reshaped one
    /// computes its value, a removed enum value maps to the one that replaced it.
    Field {
        type_name: &'static str,
        field: &'static str,
        read: fn(&Record) -> Option<Value>,
    },
    /// The arguments of `type_name.field` (a mutation's input) as this version gives them,
    /// rewritten into the shape the records' resolver reads before it runs.
    Arguments {
        type_name: &'static str,
        field: &'static str,
        write: fn(&mut Map<String, Value>),
    },
    /// The payload of a webhook `topic` in this version, from the one the builders make.
    Payload {
        topic: &'static str,
        write: fn(&mut Value),
    },
}

/// The adapter of `type_name.field` that reads it from the record, if any.
pub fn field(
    adapters: &[Adapter],
    type_name: &str,
    field: &str,
) -> Option<fn(&Record) -> Option<Value>> {
    adapters.iter().find_map(|adapter| match adapter {
        Adapter::Field {
            type_name: of,
            field: name,
            read,
        } if *of == type_name && *name == field => Some(*read),
        _ => None,
    })
}

/// The adapter of the arguments of `type_name.field`, if any.
pub fn arguments(
    adapters: &[Adapter],
    type_name: &str,
    field: &str,
) -> Option<fn(&mut Map<String, Value>)> {
    adapters.iter().find_map(|adapter| match adapter {
        Adapter::Arguments {
            type_name: of,
            field: name,
            write,
        } if *of == type_name && *name == field => Some(*write),
        _ => None,
    })
}

/// The payload of `topic` as this version sends it: `payload` with its adapters applied. The
/// same value when the version has none for the topic.
pub fn payload(adapters: &[Adapter], topic: &str, payload: &Value) -> Value {
    let mut payload = payload.clone();
    for adapter in adapters {
        if let Adapter::Payload { topic: of, write } = adapter
            && *of == topic
        {
            write(&mut payload);
        }
    }
    payload
}

/// Whether the version has an adapter for the payload of `topic`.
pub fn adapts_payload(adapters: &[Adapter], topic: &str) -> bool {
    adapters
        .iter()
        .any(|adapter| matches!(adapter, Adapter::Payload { topic: of, .. } if *of == topic))
}
