//! The `expect` of a checkout scenario, checked against the checkout its Functions computed:
//! what `lsf app check` fails on when a Function runs without an error but decides wrong.
//!
//! A list (`payment_methods`, `express_methods`, `delivery_options`, `discounts`, `lines`,
//! `errors`) is checked with `includes` (each entry is in it), `excludes` (none is) and `equals`
//! (the whole list, in order). An entry is a string, which names an entry of the list (a
//! payment method by its handle or its name), or the fields it must have. `totals` are checked
//! exactly. Each entry of `includes` and of `excludes`, each `equals` and each amount is one
//! expectation.

use lsf_core::store::model::{ExpectList, Expected, Money, ScenarioExpectInput};
use serde::Serialize;
use serde_json::{Map, Value as Json};

use super::types::{
    AppliedDiscount, Cents, CheckoutError, CheckoutResult, DeliveryOption, PaymentMethod,
    ResultLine,
};

/// An expectation the checkout does not meet.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Unmet {
    /// What was checked: the list and the rule (`express_methods.excludes`), or the amount
    /// (`totals.total`).
    pub assertion: String,
    /// What the scenario expects: the entry of `includes` or `excludes`, the list of `equals`,
    /// the amount. Amounts are decimal strings (`"5.00"`).
    pub expected: Json,
    /// What the checkout has: the list, each entry with the fields the expectation names (one
    /// field: the values alone); the amount.
    pub actual: Json,
}

/// What checking the `expect` of a scenario found.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Outcome {
    /// How many expectations were checked, met or not.
    pub checked: usize,
    pub unmet: Vec<Unmet>,
}

/// Checks `expect` against the checkout the Functions computed.
pub fn evaluate(expect: &ScenarioExpectInput, result: &CheckoutResult) -> Outcome {
    let mut outcome = Outcome::default();
    if let Some(list) = &expect.payment_methods {
        check_list(
            "payment_methods",
            list,
            &result.payment_methods,
            &mut outcome,
        );
    }
    if let Some(list) = &expect.express_methods {
        check_list(
            "express_methods",
            list,
            &result.express_methods,
            &mut outcome,
        );
    }
    if let Some(list) = &expect.delivery_options {
        let options: Vec<DeliveryOption> = result
            .delivery_groups
            .iter()
            .flat_map(|group| group.options.iter().cloned())
            .collect();
        check_list("delivery_options", list, &options, &mut outcome);
    }
    if let Some(list) = &expect.discounts {
        check_list("discounts", list, &result.discounts, &mut outcome);
    }
    if let Some(list) = &expect.lines {
        check_list("lines", list, &result.lines, &mut outcome);
    }
    if let Some(list) = &expect.errors {
        check_list("errors", list, &result.errors, &mut outcome);
    }
    if let Some(totals) = &expect.totals {
        let cost = &result.cost;
        for (field, expected, actual) in [
            ("subtotal", &totals.subtotal, cost.subtotal),
            ("discounts", &totals.discounts, cost.discounts),
            ("shipping", &totals.shipping, cost.shipping),
            ("tax", &totals.tax, cost.tax),
            ("total", &totals.total, cost.total),
            ("due_now", &totals.due_now, cost.due_now),
            ("due_later", &totals.due_later, cost.due_later),
        ] {
            let Some(expected) = expected else {
                continue;
            };
            outcome.checked += 1;
            let expected = money(expected);
            let actual = amount(actual);
            if expected != actual {
                outcome.unmet.push(Unmet {
                    assertion: format!("totals.{field}"),
                    expected,
                    actual,
                });
            }
        }
    }
    outcome
}

/// An entry of a list of the checkout, as `expect` reads it.
trait Entry {
    /// The fields an entry of `expect` can give.
    type Fields;
    /// The fields a string names the entry by, in the order `actual` shows them.
    const NAMES: &'static [&'static str];
    /// Every field `expect` can give, in the order `actual` shows them.
    const FIELDS: &'static [&'static str];

    /// The fields an entry of `expect` gives, with their values as `expected` shows them.
    fn given(fields: &Self::Fields) -> Vec<(&'static str, Json)>;

    /// The value of a field of the entry, as `actual` shows it; `None` when it has none (a
    /// line without a SKU, an automatic discount's code).
    fn value(&self, field: &str) -> Option<Json>;

    /// Whether the field of the entry is what `expect` gives.
    fn has(&self, field: &str, expected: &Json) -> bool {
        self.value(field).as_ref() == Some(expected)
    }
}

impl Entry for PaymentMethod {
    type Fields = lsf_core::store::model::ExpectedPaymentMethod;
    const NAMES: &'static [&'static str] = &["handle", "name"];
    const FIELDS: &'static [&'static str] = &["handle", "name"];

    fn given(fields: &Self::Fields) -> Vec<(&'static str, Json)> {
        [
            ("handle", text(&fields.handle)),
            ("name", text(&fields.name)),
        ]
        .into_iter()
        .filter_map(|(field, value)| Some((field, value?)))
        .collect()
    }

    fn value(&self, field: &str) -> Option<Json> {
        match field {
            "handle" => Some(Json::from(self.handle.as_str())),
            "name" => Some(Json::from(self.name.as_str())),
            _ => None,
        }
    }
}

impl Entry for DeliveryOption {
    type Fields = lsf_core::store::model::ExpectedDeliveryOption;
    const NAMES: &'static [&'static str] = &["handle", "title"];
    const FIELDS: &'static [&'static str] = &["handle", "title", "cost"];

    fn given(fields: &Self::Fields) -> Vec<(&'static str, Json)> {
        [
            ("handle", text(&fields.handle)),
            ("title", text(&fields.title)),
            ("cost", fields.cost.as_ref().map(money)),
        ]
        .into_iter()
        .filter_map(|(field, value)| Some((field, value?)))
        .collect()
    }

    fn value(&self, field: &str) -> Option<Json> {
        match field {
            "handle" => Some(Json::from(self.handle.as_str())),
            "title" => Some(Json::from(self.title.as_str())),
            "cost" => Some(amount(self.cost)),
            _ => None,
        }
    }
}

impl Entry for AppliedDiscount {
    type Fields = lsf_core::store::model::ExpectedDiscount;
    const NAMES: &'static [&'static str] = &["title", "code"];
    const FIELDS: &'static [&'static str] = &["title", "code", "class", "amount"];

    fn given(fields: &Self::Fields) -> Vec<(&'static str, Json)> {
        [
            ("title", text(&fields.title)),
            ("code", text(&fields.code)),
            (
                "class",
                fields
                    .class
                    .map(|class| serde_json::to_value(class).expect("an enum serializes")),
            ),
            ("amount", fields.amount.as_ref().map(money)),
        ]
        .into_iter()
        .filter_map(|(field, value)| Some((field, value?)))
        .collect()
    }

    fn value(&self, field: &str) -> Option<Json> {
        match field {
            "title" => Some(Json::from(self.title.as_str())),
            "code" => self.code.as_deref().map(Json::from),
            "class" => Some(serde_json::to_value(self.class).expect("an enum serializes")),
            "amount" => Some(amount(self.amount)),
            _ => None,
        }
    }
}

impl Entry for ResultLine {
    type Fields = lsf_core::store::model::ExpectedLine;
    const NAMES: &'static [&'static str] = &["sku", "title"];
    const FIELDS: &'static [&'static str] = &[
        "sku",
        "title",
        "quantity",
        "price",
        "total",
        "components",
        "attributes",
    ];

    fn given(fields: &Self::Fields) -> Vec<(&'static str, Json)> {
        [
            ("sku", text(&fields.sku)),
            ("title", text(&fields.title)),
            ("quantity", fields.quantity.map(Json::from)),
            ("price", fields.price.as_ref().map(money)),
            ("total", fields.total.as_ref().map(money)),
            (
                "components",
                fields
                    .components
                    .as_ref()
                    .map(|names| names.iter().map(|name| Json::from(name.as_str())).collect()),
            ),
            (
                "attributes",
                fields.attributes.as_ref().map(|attributes| {
                    attributes
                        .iter()
                        .map(|(key, value)| (key.clone(), Json::from(value.as_str())))
                        .collect::<Map<String, Json>>()
                        .into()
                }),
            ),
        ]
        .into_iter()
        .filter_map(|(field, value)| Some((field, value?)))
        .collect()
    }

    fn value(&self, field: &str) -> Option<Json> {
        match field {
            "sku" => self.sku.as_deref().map(Json::from),
            "title" => Some(Json::from(self.title.as_str())),
            "quantity" => Some(Json::from(self.quantity)),
            "price" => Some(amount(self.unit_price)),
            "total" => Some(amount(self.total)),
            // Each by its SKU, or its title when it has none.
            "components" => Some(
                self.components
                    .iter()
                    .map(|component| {
                        Json::from(component.sku.as_deref().unwrap_or(&component.title))
                    })
                    .collect(),
            ),
            "attributes" => Some(
                self.attributes
                    .iter()
                    .map(|pair| (pair.key.clone(), Json::from(pair.value.as_str())))
                    .collect::<Map<String, Json>>()
                    .into(),
            ),
            _ => None,
        }
    }

    fn has(&self, field: &str, expected: &Json) -> bool {
        match (field, expected) {
            // In order, each by its SKU or its title.
            ("components", Json::Array(names)) => {
                names.len() == self.components.len()
                    && names
                        .iter()
                        .zip(&self.components)
                        .all(|(name, component)| component.named(name))
            }
            // Those given, with these values; the line can have others.
            ("attributes", Json::Object(expected)) => expected.iter().all(|(key, value)| {
                self.attributes
                    .iter()
                    .any(|pair| &pair.key == key && value.as_str() == Some(pair.value.as_str()))
            }),
            _ => self.value(field).as_ref() == Some(expected),
        }
    }
}

impl ResultLine {
    /// Whether `name` is the SKU or the title of the line.
    fn named(&self, name: &Json) -> bool {
        let Some(name) = name.as_str() else {
            return false;
        };
        self.sku.as_deref() == Some(name) || self.title == name
    }
}

impl Entry for CheckoutError {
    type Fields = lsf_core::store::model::ExpectedError;
    const NAMES: &'static [&'static str] = &["message"];
    const FIELDS: &'static [&'static str] = &["message", "target"];

    fn given(fields: &Self::Fields) -> Vec<(&'static str, Json)> {
        [
            ("message", text(&fields.message)),
            ("target", text(&fields.target)),
        ]
        .into_iter()
        .filter_map(|(field, value)| Some((field, value?)))
        .collect()
    }

    fn value(&self, field: &str) -> Option<Json> {
        match field {
            "message" => Some(Json::from(self.message.as_str())),
            "target" => self.target.as_deref().map(Json::from),
            _ => None,
        }
    }
}

/// Whether an entry of the list is the one `expected` describes.
fn matches<E: Entry>(entry: &E, expected: &Expected<E::Fields>) -> bool {
    match expected {
        Expected::Name(name) => {
            let name = Json::from(name.as_str());
            E::NAMES
                .iter()
                .any(|field| entry.value(field).as_ref() == Some(&name))
        }
        Expected::Fields(fields) => E::given(fields)
            .iter()
            .all(|(field, value)| entry.has(field, value)),
    }
}

/// An entry of `expect` as `expected` shows it.
fn written<E: Entry>(expected: &Expected<E::Fields>) -> Json {
    match expected {
        Expected::Name(name) => Json::from(name.as_str()),
        Expected::Fields(fields) => Json::Object(
            E::given(fields)
                .into_iter()
                .map(|(field, value)| (field.to_string(), value))
                .collect(),
        ),
    }
}

/// The list of the checkout as `actual` shows it: each entry with the fields the entries of
/// `expect` name (those a string names it by, for a string), or its value alone for one field.
fn shown<E: Entry>(entries: &[E], expected: &[&Expected<E::Fields>]) -> Json {
    let mut named: Vec<&str> = Vec::new();
    for item in expected {
        match item {
            Expected::Name(_) => named.extend(E::NAMES),
            Expected::Fields(fields) => {
                named.extend(E::given(fields).iter().map(|(field, _)| *field))
            }
        }
    }
    if named.is_empty() {
        named.extend(E::NAMES);
    }
    let fields: Vec<&str> = E::FIELDS
        .iter()
        .copied()
        .filter(|field| named.contains(field))
        .collect();
    entries
        .iter()
        .map(|entry| match fields.as_slice() {
            [field] => entry.value(field).unwrap_or(Json::Null),
            _ => Json::Object(
                fields
                    .iter()
                    .filter_map(|field| Some((field.to_string(), entry.value(field)?)))
                    .collect(),
            ),
        })
        .collect()
}

fn check_list<E: Entry>(
    name: &str,
    list: &ExpectList<E::Fields>,
    entries: &[E],
    outcome: &mut Outcome,
) {
    for item in &list.includes {
        outcome.checked += 1;
        if !entries.iter().any(|entry| matches(entry, item)) {
            outcome.unmet.push(Unmet {
                assertion: format!("{name}.includes"),
                expected: written::<E>(item),
                actual: shown(entries, &[item]),
            });
        }
    }
    for item in &list.excludes {
        outcome.checked += 1;
        if entries.iter().any(|entry| matches(entry, item)) {
            outcome.unmet.push(Unmet {
                assertion: format!("{name}.excludes"),
                expected: written::<E>(item),
                actual: shown(entries, &[item]),
            });
        }
    }
    if let Some(equals) = &list.equals {
        outcome.checked += 1;
        let same = equals.len() == entries.len()
            && equals
                .iter()
                .zip(entries)
                .all(|(item, entry)| matches(entry, item));
        if !same {
            let items: Vec<&Expected<E::Fields>> = equals.iter().collect();
            outcome.unmet.push(Unmet {
                assertion: format!("{name}.equals"),
                expected: equals.iter().map(written::<E>).collect(),
                actual: shown(entries, &items),
            });
        }
    }
}

fn text(value: &Option<String>) -> Option<Json> {
    value.as_deref().map(Json::from)
}

/// An amount as `expect` writes it: a decimal string, `"-5.00"`.
pub fn amount(cents: Cents) -> Json {
    let sign = if cents < 0 { "-" } else { "" };
    let cents = cents.unsigned_abs();
    Json::from(format!("{sign}{}.{:02}", cents / 100, cents % 100))
}

/// An amount of `expect`, as [`amount`] writes it. One the store data refused (it overflows)
/// is shown as written, and equals nothing.
fn money(money: &Money) -> Json {
    match money.cents() {
        Some(cents) => amount(cents),
        None => serde_json::to_value(money).expect("an amount serializes"),
    }
}

#[cfg(test)]
mod tests;
