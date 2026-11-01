//! Delivery profiles: `deliveryProfileCreate`, `deliveryProfileUpdate`, `deliveryProfileRemove`,
//! `deliveryProfile(s)`, `locationsAvailableForDeliveryProfiles(Connection)`,
//! `ProductVariant.deliveryProfile`, and what a profile derives (its items, counts and
//! unassigned locations).
//!
//! As shopify.dev describes them ("Building delivery profiles", and the reference of 2026-10): a
//! profile groups variants with location groups, each shipping to zones (countries and their
//! provinces, or the rest of the world) with method definitions, each a rate (a price) or a
//! carrier service's (a participant), with price or weight conditions. The **general profile**
//! (`default: true`) holds every variant no other profile holds; it cannot be removed. The
//! seed makes it from the store data: every location in one group, shipping to the rest of the
//! world (the store data's `checkout.shipping_rates` are offered to every address) at the
//! data's shipping rates *(chosen: the name "General profile" is Shopify's; the zone's is
//! lsf's)*. Its pickup and local delivery rates are not shipping methods on Shopify (they are
//! the locations'): they stay out of it.
//!
//! - `deliveryProfileCreate` and `…Update` take groups, zones and method definitions to create,
//!   update and delete, variants and selling plan groups to associate and dissociate (a variant is
//!   in one profile: associated with one, it leaves the other). A location is in one group of a
//!   profile; a zone has a name; a method definition a name and a rate or a participant whose
//!   carrier service exists. Every update moves `version` on. The errors are `UserError`s (no
//!   code) in lsf's words (shopify.dev publishes none for these).
//! - `deliveryProfileRemove` "enqueues the removal": lsf removes it at once and answers a `Job`
//!   already done; its variants go back to the general profile.
//! - Events: `profiles/create`, `profiles/update`, `profiles/delete`.
//! - **Store sync**: the store data offers one list of shipping rates to every address. The
//!   method definitions of the general profile that ship to the rest of the world, with a flat
//!   price and no condition, are those rates: changing them (their name, price, description,
//!   active) changes the local checkout's delivery options ([`checkout_rates`]). The other zones,
//!   the other profiles, conditional rates and carrier services' rates have nowhere to go in the
//!   store data and do not reach the checkout.

use lsf_core::store::CheckoutShippingRate;
use lsf_core::store::model::{CheckoutDeliveryKind, CheckoutShippingRateInput, Money};
use lsf_core::util::stable_id;
use serde_json::{Map, Value, json};

use super::{int, keys_of, objects, push, text};
use crate::domains::catalog::{self, gids};
use crate::domains::orders::model;
use crate::graphql::{FieldContext, FieldError, Output};
use crate::resolve::{Mutation, Obj, Registry, ShopResolver, UserError, payload};
use crate::state::seed::Seed;
use crate::state::{Record, ShopState, gid, value};

pub fn register(registry: &mut Registry) {
    registry
        .mutation("deliveryProfileCreate", create)
        .mutation("deliveryProfileUpdate", update)
        .mutation("deliveryProfileRemove", remove)
        .query(
            "locationsAvailableForDeliveryProfiles",
            |r, ctx, _object| r.convert(ctx, &Value::Array(origin_locations(r.state())), true),
        )
        .query(
            "locationsAvailableForDeliveryProfilesConnection",
            |r, ctx, _object| r.connection(ctx, origin_locations(r.state())),
        )
        .field("ProductVariant", "deliveryProfile", variant_profile)
        .field("DeliveryProfile", "profileItems", profile_items)
        .field(
            "DeliveryProfile",
            "productVariantsCount",
            |r, ctx, object| {
                let count = profile_key(object).map_or(0, |key| variants_of(r.state(), key).len());
                Ok(r.count(ctx, count))
            },
        )
        .field(
            "DeliveryProfile",
            "productVariantsCountV2",
            |r, ctx, object| {
                let count = profile_key(object).map_or(0, |key| variants_of(r.state(), key).len());
                r.convert(ctx, &json!({"count": count, "capped": false}), false)
            },
        )
        .field("DeliveryProfile", "profileLocationGroups", location_groups)
        .field(
            "DeliveryProfile",
            "unassignedLocations",
            |r, ctx, object| {
                let items = profile_key(object).map(|key| unassigned(r.state(), key));
                r.convert(ctx, &Value::Array(items.unwrap_or_default()), true)
            },
        )
        .field(
            "DeliveryProfile",
            "unassignedLocationsPaginated",
            |r, ctx, object| {
                let items = profile_key(object).map(|key| unassigned(r.state(), key));
                r.connection(ctx, items.unwrap_or_default())
            },
        )
        .field(
            "DeliveryProfile",
            "activeMethodDefinitionsCount",
            |r, _ctx, object| {
                let state = r.state();
                let count = profile_key(object).map_or(0, |key| {
                    method_definitions(state, key)
                        .iter()
                        .filter(|method| {
                            state.get(method.as_str()).is_some_and(|record| {
                                record.get("active").and_then(Value::as_bool) == Some(true)
                            })
                        })
                        .count()
                });
                Ok(Output::value(count))
            },
        )
        .field(
            "DeliveryProfile",
            "originLocationCount",
            |r, _ctx, object| {
                let count = profile_key(object).map_or(0, |key| {
                    profile_locations(r.state(), key)
                        .iter()
                        .filter(|location| catalog::is_active_location(r.state(), location))
                        .count()
                });
                Ok(Output::value(count))
            },
        )
        .field(
            "DeliveryProfile",
            "locationsWithoutRatesCount",
            |r, _ctx, object| {
                let count =
                    profile_key(object).map_or(0, |key| locations_without_rates(r.state(), key));
                Ok(Output::value(count))
            },
        )
        .field("DeliveryProfile", "zoneCountryCount", |r, _ctx, object| {
            let count = profile_key(object).map_or(0, |key| countries_of(r.state(), key).len());
            Ok(Output::value(count))
        })
        .field(
            "DeliveryProfileLocationGroup",
            "countriesInAnyZone",
            countries_in_any_zone,
        )
        .field(
            "DeliveryLocationGroupZone",
            "methodDefinitionCounts",
            method_counts,
        )
        .field(
            "DeliveryLocationGroupZone",
            "methodDefinitions",
            zone_methods,
        );
}

/// The name of the general profile, as Shopify names it.
pub const GENERAL_PROFILE_NAME: &str = "General profile";

/// The record field where a method definition the seed made from a shipping rate of the store
/// data keeps that rate's handle, which the store sync writes it back under.
const RATE_HANDLE: &str = "lsfHandle";

/// The record field where a profile keeps its variants (the general profile keeps none: it holds
/// those no other profile holds).
const VARIANTS: &str = "lsfVariants";

/// The GID of the general profile the seed makes.
pub fn general_profile_gid() -> String {
    gid("DeliveryProfile", stable_id("delivery_profile", "general"))
}

/// The record types a delivery profile is made of: a change of any of them is a change of the
/// profiles (what the store sync looks at for the checkout's rates).
pub const RECORD_TYPES: &[&str] = &[
    "DeliveryProfile",
    "DeliveryLocationGroup",
    "DeliveryZone",
    "DeliveryCountry",
    "DeliveryProvince",
    "DeliveryMethodDefinition",
    "DeliveryRateDefinition",
    "DeliveryParticipant",
    "DeliveryCondition",
];

// --- The seed ---

/// The general profile: every location, the rest of the world, the store data's shipping rates.
pub fn seed(seed: &mut Seed) {
    let created = Seed::epoch_string();
    let currency = seed.currency.clone();
    let locations: Vec<String> = seed
        .state
        .all("Location")
        .filter(|location| {
            location
                .get("isFulfillmentService")
                .and_then(Value::as_bool)
                != Some(true)
        })
        .filter_map(|location| text(location, "id").map(str::to_string))
        .collect();
    let derived = |kind: &str, key: &str, type_name: &str| gid(type_name, stable_id(kind, key));
    let group = derived(
        "delivery_location_group",
        "general",
        "DeliveryLocationGroup",
    );
    let zone = derived("delivery_zone", "general/rest-of-world", "DeliveryZone");
    let country = derived(
        "delivery_country",
        "general/rest-of-world",
        "DeliveryCountry",
    );
    let mut methods = Vec::new();
    let rates: Vec<CheckoutShippingRate> = seed
        .store
        .checkout
        .shipping_rates
        .iter()
        .filter(|rate| rate.kind == CheckoutDeliveryKind::Shipping)
        .cloned()
        .collect();
    for rate in &rates {
        let method = derived(
            "delivery_method_definition",
            &format!("general/{}", rate.handle),
            "DeliveryMethodDefinition",
        );
        let definition = derived(
            "delivery_rate_definition",
            &format!("general/{}", rate.handle),
            "DeliveryRateDefinition",
        );
        seed.insert(json!({
            "__typename": "DeliveryRateDefinition",
            "id": definition,
            "price": value::money_v2(rate.price, &currency),
        }));
        seed.insert(json!({
            "__typename": "DeliveryMethodDefinition",
            "id": method,
            "name": rate.title,
            "description": rate.description,
            "active": true,
            "rateProvider": definition,
            "methodConditions": [],
            RATE_HANDLE: rate.handle,
        }));
        methods.push(method);
    }
    seed.insert(json!({
        "__typename": "DeliveryCountry",
        "id": country,
        "code": {"countryCode": null, "restOfWorld": true},
        "name": "Rest of World",
        "translatedName": "Rest of World",
        "provinces": [],
    }));
    seed.insert(json!({
        "__typename": "DeliveryZone",
        "id": zone,
        "name": "Rest of world",
        "countries": [country],
    }));
    seed.insert(json!({
        "__typename": "DeliveryLocationGroup",
        "id": group,
        "locations": locations,
    }));
    seed.insert(json!({
        "__typename": "DeliveryProfile",
        "id": general_profile_gid(),
        "name": GENERAL_PROFILE_NAME,
        "default": true,
        "version": 1,
        "coversAllItems": false,
        "createdAt": created,
        "profileLocationGroups": [{
            "locationGroup": group,
            "locationGroupZones": [{"zone": zone, "methodDefinitions": methods}],
        }],
        "sellingPlanGroups": [],
        VARIANTS: [],
    }));
}

// --- What a profile derives ---

fn profile_key(object: &Obj) -> Option<&str> {
    object.key()
}

fn default_profile(state: &ShopState) -> Option<String> {
    state
        .all("DeliveryProfile")
        .find(|profile| profile.get("default").and_then(Value::as_bool) == Some(true))
        .and_then(|profile| text(profile, "id").map(str::to_string))
}

/// The variants of a profile: its own, or for the general profile every variant no other
/// profile holds.
fn variants_of(state: &ShopState, profile: &str) -> Vec<String> {
    let Some(record) = state.get(profile) else {
        return Vec::new();
    };
    if record.get("default").and_then(Value::as_bool) != Some(true) {
        return gids(record.get(VARIANTS))
            .into_iter()
            .filter(|variant| state.contains(variant))
            .collect();
    }
    let elsewhere: Vec<String> = state
        .all("DeliveryProfile")
        .filter(|other| other.get("default").and_then(Value::as_bool) != Some(true))
        .flat_map(|other| gids(other.get(VARIANTS)))
        .collect();
    state
        .keys("ProductVariant")
        .filter(|variant| !elsewhere.iter().any(|known| known == variant))
        .map(str::to_string)
        .collect()
}

/// `ProductVariant.deliveryProfile`: the profile that holds the variant, else the general one.
fn variant_profile(
    r: &ShopResolver<'_>,
    _ctx: &FieldContext<'_>,
    object: &Obj,
) -> Result<Output<Obj>, FieldError> {
    let state = r.state();
    let Some(variant) = object.key() else {
        return Ok(Output::Null);
    };
    let profile = state
        .all("DeliveryProfile")
        .find(|profile| {
            gids(profile.get(VARIANTS))
                .iter()
                .any(|known| known == variant)
        })
        .and_then(|profile| text(profile, "id").map(str::to_string))
        .or_else(|| default_profile(state));
    Ok(profile.map_or(Output::Null, |profile| {
        Output::Object(Obj::record(&profile))
    }))
}

/// `DeliveryProfile.profileItems`: a `DeliveryProfileItem` per product, with the variants of it
/// the profile holds.
fn profile_items(
    r: &ShopResolver<'_>,
    ctx: &FieldContext<'_>,
    object: &Obj,
) -> Result<Output<Obj>, FieldError> {
    let state = r.state();
    let variants = profile_key(object)
        .map(|key| variants_of(state, key))
        .unwrap_or_default();
    let mut items: Vec<(String, Vec<String>)> = Vec::new();
    for variant in variants {
        let Some(product) = state
            .get(&variant)
            .and_then(|record| text(record, "product"))
            .map(str::to_string)
        else {
            continue;
        };
        match items.iter_mut().find(|(known, _)| *known == product) {
            Some((_, list)) => list.push(variant),
            None => items.push((product, vec![variant])),
        }
    }
    let items: Vec<Value> = items
        .into_iter()
        .map(|(product, variants)| {
            let number = crate::state::parse_gid(&product).map_or(0, |(_, n)| n);
            json!({
                "__typename": "DeliveryProfileItem",
                "id": gid("DeliveryProfileItem", number),
                "product": product,
                "variants": variants,
            })
        })
        .collect();
    r.connection(ctx, items)
}

/// The groups of a profile, as the record keeps them.
fn groups(state: &ShopState, profile: &str) -> Vec<Map<String, Value>> {
    objects(
        state
            .get(profile)
            .and_then(|record| record.get("profileLocationGroups")),
    )
}

/// `DeliveryProfile.profileLocationGroups(locationGroupId:)`.
fn location_groups(
    r: &ShopResolver<'_>,
    ctx: &FieldContext<'_>,
    object: &Obj,
) -> Result<Output<Obj>, FieldError> {
    let Some(key) = profile_key(object) else {
        return r.generic(ctx, object);
    };
    let wanted = ctx.arg_str("locationGroupId");
    let groups: Vec<Value> = groups(r.state(), key)
        .into_iter()
        .filter(|group| wanted.is_none_or(|wanted| text(group, "locationGroup") == Some(wanted)))
        .map(Value::Object)
        .collect();
    r.convert(ctx, &Value::Array(groups), true)
}

/// The locations of a profile's groups.
fn profile_locations(state: &ShopState, profile: &str) -> Vec<String> {
    groups(state, profile)
        .iter()
        .filter_map(|group| text(group, "locationGroup"))
        .flat_map(|group| keys_of(state, group, "locations"))
        .collect()
}

/// The locations a profile can ship from: active, not a fulfillment service's.
fn origin_locations(state: &ShopState) -> Vec<Value> {
    state
        .keys("Location")
        .filter(|location| {
            state.get(location).is_some_and(|record| {
                record.get("isActive").and_then(Value::as_bool) == Some(true)
                    && record.get("isFulfillmentService").and_then(Value::as_bool) != Some(true)
            })
        })
        .map(|location| json!(location))
        .collect()
}

/// The origin locations no group of a profile has.
fn unassigned(state: &ShopState, profile: &str) -> Vec<Value> {
    let assigned = profile_locations(state, profile);
    origin_locations(state)
        .into_iter()
        .filter(|location| {
            location
                .as_str()
                .is_some_and(|location| !assigned.iter().any(|known| known == location))
        })
        .collect()
}

/// The zones of a group entry, with their method definitions.
fn zones_of(group: &Map<String, Value>) -> Vec<Map<String, Value>> {
    objects(group.get("locationGroupZones"))
}

fn method_definitions(state: &ShopState, profile: &str) -> Vec<String> {
    groups(state, profile)
        .iter()
        .flat_map(zones_of)
        .flat_map(|zone| gids(zone.get("methodDefinitions")))
        .collect()
}

/// How many method definitions a profile has.
pub fn method_count(state: &ShopState, profile: &str) -> usize {
    method_definitions(state, profile).len()
}

/// The locations of a profile in groups without a method definition.
fn locations_without_rates(state: &ShopState, profile: &str) -> usize {
    groups(state, profile)
        .iter()
        .filter(|group| {
            zones_of(group)
                .iter()
                .all(|zone| gids(zone.get("methodDefinitions")).is_empty())
        })
        .filter_map(|group| text(group, "locationGroup"))
        .map(|group| keys_of(state, group, "locations").len())
        .sum()
}

/// The countries of a profile's zones, without repeats.
fn countries_of(state: &ShopState, profile: &str) -> Vec<String> {
    let mut codes: Vec<String> = Vec::new();
    for zone in groups(state, profile).iter().flat_map(zones_of) {
        let Some(zone) = text(&zone, "zone") else {
            continue;
        };
        for country in keys_of(state, zone, "countries") {
            let code = state
                .get(&country)
                .and_then(|record| record.get("code"))
                .map(|code| code.to_string())
                .unwrap_or_default();
            if !codes.contains(&code) {
                codes.push(code);
            }
        }
    }
    codes
}

/// `DeliveryProfileLocationGroup.countriesInAnyZone`.
fn countries_in_any_zone(
    r: &ShopResolver<'_>,
    ctx: &FieldContext<'_>,
    object: &Obj,
) -> Result<Output<Obj>, FieldError> {
    let state = r.state();
    let Some(group) = r.map_of(object) else {
        return r.generic(ctx, object);
    };
    let items: Vec<Value> = zones_of(group)
        .iter()
        .filter_map(|zone| text(zone, "zone"))
        .flat_map(|zone| {
            let name = state
                .get(zone)
                .and_then(|record| record.get("name").cloned())
                .unwrap_or(Value::Null);
            keys_of(state, zone, "countries")
                .into_iter()
                .map(move |country| json!({"country": country, "zone": name}))
        })
        .collect();
    r.convert(ctx, &Value::Array(items), true)
}

/// Whether a method definition's rate is a carrier service's.
fn is_participant(state: &ShopState, method: &str) -> bool {
    state
        .get(method)
        .and_then(|record| text(record, "rateProvider"))
        .and_then(|provider| state.type_of(provider))
        == Some("DeliveryParticipant")
}

/// `DeliveryLocationGroupZone.methodDefinitionCounts`.
fn method_counts(
    r: &ShopResolver<'_>,
    ctx: &FieldContext<'_>,
    object: &Obj,
) -> Result<Output<Obj>, FieldError> {
    let Some(zone) = r.map_of(object) else {
        return r.generic(ctx, object);
    };
    let methods = gids(zone.get("methodDefinitions"));
    let participants = methods
        .iter()
        .filter(|method| is_participant(r.state(), method))
        .count();
    r.convert(
        ctx,
        &json!({
            "participantDefinitionsCount": participants,
            "rateDefinitionsCount": methods.len() - participants,
        }),
        false,
    )
}

/// `DeliveryLocationGroupZone.methodDefinitions(eligible:, type:)`: `MERCHANT` the rates,
/// `PARTICIPANT` the carrier services'; `eligible` is not simulated (every method is).
fn zone_methods(
    r: &ShopResolver<'_>,
    ctx: &FieldContext<'_>,
    object: &Obj,
) -> Result<Output<Obj>, FieldError> {
    let Some(zone) = r.map_of(object) else {
        return r.generic(ctx, object);
    };
    let kind = ctx.arg_str("type");
    let items: Vec<Value> = gids(zone.get("methodDefinitions"))
        .into_iter()
        .filter(|method| match kind {
            Some("PARTICIPANT") => is_participant(r.state(), method),
            Some("MERCHANT") => !is_participant(r.state(), method),
            _ => true,
        })
        .map(Value::String)
        .collect();
    r.connection(ctx, items)
}

// --- Building and changing profiles ---

/// What the changes of one mutation need besides the shop: the errors, the shop's currency.
struct Build {
    errors: Vec<UserError>,
    currency: String,
}

impl Build {
    fn error(&mut self, path: &[String], message: &str) {
        let path: Vec<&str> = path.iter().map(String::as_str).collect();
        self.errors.push(UserError::new(&path, message));
    }
}

fn at(path: &[String], parts: &[&str]) -> Vec<String> {
    path.iter()
        .cloned()
        .chain(parts.iter().map(|part| part.to_string()))
        .collect()
}

/// A `MoneyV2` of a `MoneyInput`, in the shop's currency when it gives none.
fn money_of(input: Option<&Value>, currency: &str) -> Option<Value> {
    let input = input?;
    let amount = match input.get("amount")? {
        Value::String(text) => text.clone(),
        other => other.to_string(),
    };
    let cents = model::parse_cents(&amount)?;
    let currency = input
        .get("currencyCode")
        .and_then(Value::as_str)
        .unwrap_or(currency);
    Some(value::money_v2(cents, currency))
}

/// Makes the countries (and their provinces) of a zone input.
fn make_countries(state: &mut ShopState, input: &Map<String, Value>) -> Vec<String> {
    let mut countries = Vec::new();
    for country in objects(input.get("countries")) {
        let rest = country.get("restOfWorld").and_then(Value::as_bool) == Some(true);
        let code = text(&country, "code").map(str::to_string);
        let name = if rest {
            "Rest of World".to_string()
        } else {
            code.as_deref()
                .and_then(lsf_core::store::reference::country_name)
                .map_or_else(|| code.clone().unwrap_or_default(), str::to_string)
        };
        let mut provinces = Vec::new();
        for province in objects(country.get("provinces")) {
            let code = text(&province, "code").unwrap_or_default().to_string();
            let id = state.new_gid("DeliveryProvince");
            state.insert(value::object(json!({
                "__typename": "DeliveryProvince",
                "id": id,
                "code": code,
                "name": code,
                "translatedName": code,
            })));
            provinces.push(id);
        }
        let id = state.new_gid("DeliveryCountry");
        state.insert(value::object(json!({
            "__typename": "DeliveryCountry",
            "id": id,
            "code": {"countryCode": if rest { Value::Null } else { json!(code) }, "restOfWorld": rest},
            "name": name,
            "translatedName": name,
            "provinces": provinces,
        })));
        countries.push(id);
    }
    countries
}

/// Makes the conditions of a method definition input.
fn make_conditions(
    state: &mut ShopState,
    input: &Map<String, Value>,
    build: &Build,
) -> Vec<String> {
    let mut conditions = Vec::new();
    for (field, list) in [
        ("TOTAL_PRICE", "priceConditionsToCreate"),
        ("TOTAL_WEIGHT", "weightConditionsToCreate"),
    ] {
        for condition in objects(input.get(list)) {
            let criteria = match field {
                "TOTAL_PRICE" => money_of(condition.get("criteria"), &build.currency)
                    .map(|mut money| {
                        money["__typename"] = json!("MoneyV2");
                        money
                    })
                    .unwrap_or(Value::Null),
                _ => condition
                    .get("criteria")
                    .map(|weight| {
                        json!({"__typename": "Weight", "unit": weight.get("unit"), "value": weight.get("value")})
                    })
                    .unwrap_or(Value::Null),
            };
            let id = state.new_gid("DeliveryCondition");
            state.insert(value::object(json!({
                "__typename": "DeliveryCondition",
                "id": id,
                "field": field,
                "operator": condition.get("operator").cloned().unwrap_or(json!("GREATER_THAN_OR_EQUAL_TO")),
                "conditionCriteria": criteria,
            })));
            conditions.push(id);
        }
    }
    conditions
}

/// The rate provider of a method definition input: a rate definition (`rateDefinition`) or a
/// carrier service's participant (`participant`). `None`, with an error, when it gives neither or
/// a carrier service that is none.
fn make_provider(
    state: &mut ShopState,
    input: &Map<String, Value>,
    path: &[String],
    build: &mut Build,
) -> Option<String> {
    if let Some(rate) = input.get("rateDefinition").and_then(Value::as_object) {
        let Some(price) = money_of(rate.get("price"), &build.currency) else {
            build.error(&at(path, &["rateDefinition", "price"]), "Price is invalid");
            return None;
        };
        let id = state.new_gid("DeliveryRateDefinition");
        state.insert(value::object(json!({
            "__typename": "DeliveryRateDefinition",
            "id": id,
            "price": price,
        })));
        return Some(id);
    }
    if let Some(participant) = input.get("participant").and_then(Value::as_object) {
        let carrier = text(participant, "carrierServiceId").unwrap_or_default();
        if state.type_of(carrier) != Some("DeliveryCarrierService") {
            build.error(
                &at(path, &["participant", "carrierServiceId"]),
                "Carrier service not found",
            );
            return None;
        }
        let services: Vec<Value> = objects(participant.get("participantServices"))
            .into_iter()
            .map(|service| json!({"active": service.get("active"), "name": service.get("name")}))
            .collect();
        let id = state.new_gid("DeliveryParticipant");
        state.insert(value::object(json!({
            "__typename": "DeliveryParticipant",
            "id": id,
            "carrierService": carrier,
            "fixedFee": money_of(participant.get("fixedFee"), &build.currency),
            "percentageOfRateFee": participant.get("percentageOfRateFee").and_then(Value::as_f64).unwrap_or(0.0),
            "participantServices": services,
            "adaptToNewServicesFlag": participant.get("adaptToNewServices").and_then(Value::as_bool).unwrap_or(false),
        })));
        return Some(id);
    }
    build.error(
        path,
        "Method definition needs a rate definition or a participant",
    );
    None
}

/// Makes a method definition. `None` (with errors) when its input is not one.
fn make_method(
    state: &mut ShopState,
    input: &Map<String, Value>,
    path: &[String],
    build: &mut Build,
) -> Option<String> {
    let name = text(input, "name")
        .map(str::trim)
        .unwrap_or_default()
        .to_string();
    if name.is_empty() {
        build.error(&at(path, &["name"]), "Name can't be blank");
        return None;
    }
    let provider = make_provider(state, input, path, build)?;
    let conditions = make_conditions(state, input, build);
    let id = state.new_gid("DeliveryMethodDefinition");
    state.insert(value::object(json!({
        "__typename": "DeliveryMethodDefinition",
        "id": id,
        "name": name,
        "description": input.get("description").cloned().unwrap_or(Value::Null),
        "active": input.get("active").and_then(Value::as_bool).unwrap_or(true),
        "rateProvider": provider,
        "methodConditions": conditions,
    })));
    Some(id)
}

/// Removes a method definition with its rate provider and conditions.
fn remove_method(state: &mut ShopState, method: &str) {
    let Some(record) = state.get(method).cloned() else {
        return;
    };
    if let Some(provider) = text(&record, "rateProvider") {
        state.remove(provider);
    }
    for condition in gids(record.get("methodConditions")) {
        state.remove(&condition);
    }
    state.remove(method);
}

/// Updates a method definition of a zone from its input.
fn update_method(
    state: &mut ShopState,
    input: &Map<String, Value>,
    zone_methods: &[String],
    path: &[String],
    build: &mut Build,
) {
    let id = text(input, "id").unwrap_or_default().to_string();
    if !zone_methods.contains(&id) {
        build.error(&at(path, &["id"]), "Method definition not found");
        return;
    }
    if let Some(name) = text(input, "name")
        && name.trim().is_empty()
    {
        build.error(&at(path, &["name"]), "Name can't be blank");
        return;
    }
    let provider = (input.contains_key("rateDefinition") || input.contains_key("participant"))
        .then(|| make_provider(state, input, path, build))
        .flatten();
    let mut conditions = make_conditions(state, input, build);
    for change in objects(input.get("conditionsToUpdate")) {
        let condition = text(&change, "id").unwrap_or_default().to_string();
        let owned = state
            .get(&id)
            .is_some_and(|record| gids(record.get("methodConditions")).contains(&condition));
        if !owned {
            build.error(&at(path, &["conditionsToUpdate"]), "Condition not found");
            continue;
        }
        let field = change
            .get("field")
            .and_then(Value::as_str)
            .map(str::to_string)
            .or_else(|| {
                state
                    .get(&condition)
                    .and_then(|record| text(record, "field"))
                    .map(str::to_string)
            })
            .unwrap_or_default();
        let criteria = change.get("criteria").and_then(Value::as_f64);
        let unit = text(&change, "criteriaUnit").map(str::to_string);
        let currency = build.currency.clone();
        if let Some(record) = state.get_mut(&condition) {
            if let Some(operator) = change.get("operator").filter(|value| !value.is_null()) {
                record.insert("operator".into(), operator.clone());
            }
            record.insert("field".into(), json!(field));
            if let Some(amount) = criteria {
                let criteria = if field == "TOTAL_PRICE" {
                    let cents = (amount * 100.0).round() as i64;
                    let mut money = value::money_v2(cents, unit.as_deref().unwrap_or(&currency));
                    money["__typename"] = json!("MoneyV2");
                    money
                } else {
                    json!({"__typename": "Weight", "unit": unit.unwrap_or_else(|| "KILOGRAMS".into()), "value": amount})
                };
                record.insert("conditionCriteria".into(), criteria);
            }
        }
    }
    let old_provider = state
        .get(&id)
        .and_then(|record| text(record, "rateProvider"))
        .map(str::to_string);
    if let Some(record) = state.get_mut(&id) {
        for field in ["name", "description", "active"] {
            if let Some(given) = input.get(field).filter(|given| !given.is_null()) {
                record.insert(field.into(), given.clone());
            }
        }
        if let Some(provider) = &provider {
            record.insert("rateProvider".into(), json!(provider));
        }
        if let Some(Value::Array(list)) = record.get_mut("methodConditions") {
            list.extend(conditions.drain(..).map(Value::String));
        }
    }
    if provider.is_some()
        && let Some(old) = old_provider
    {
        state.remove(&old);
    }
}

/// Makes a zone of a group from its input: the `locationGroupZones` entry. `None` (with errors)
/// when the input is not one.
fn make_zone(
    state: &mut ShopState,
    input: &Map<String, Value>,
    path: &[String],
    build: &mut Build,
) -> Option<Value> {
    let name = text(input, "name")
        .map(str::trim)
        .unwrap_or_default()
        .to_string();
    if name.is_empty() {
        build.error(&at(path, &["name"]), "Zone name can't be blank");
        return None;
    }
    let countries = make_countries(state, input);
    let mut methods = Vec::new();
    for (index, method) in objects(input.get("methodDefinitionsToCreate"))
        .iter()
        .enumerate()
    {
        let path = at(path, &["methodDefinitionsToCreate", &index.to_string()]);
        if let Some(id) = make_method(state, method, &path, build) {
            methods.push(id);
        }
    }
    let id = state.new_gid("DeliveryZone");
    state.insert(value::object(json!({
        "__typename": "DeliveryZone",
        "id": id,
        "name": name,
        "countries": countries,
    })));
    Some(json!({"zone": id, "methodDefinitions": methods}))
}

/// Removes a zone entry's records: its zone, countries, provinces and method definitions.
fn remove_zone(state: &mut ShopState, entry: &Map<String, Value>) {
    for method in gids(entry.get("methodDefinitions")) {
        remove_method(state, &method);
    }
    let Some(zone) = text(entry, "zone").map(str::to_string) else {
        return;
    };
    for country in keys_of(state, &zone, "countries") {
        for province in keys_of(state, &country, "provinces") {
            state.remove(&province);
        }
        state.remove(&country);
    }
    state.remove(&zone);
}

/// Updates a zone entry from its input: name, countries (replaced), method definitions created
/// and updated.
fn update_zone(
    state: &mut ShopState,
    entry: &mut Map<String, Value>,
    input: &Map<String, Value>,
    path: &[String],
    build: &mut Build,
) {
    let zone = text(entry, "zone").unwrap_or_default().to_string();
    if let Some(name) = text(input, "name") {
        if name.trim().is_empty() {
            build.error(&at(path, &["name"]), "Zone name can't be blank");
            return;
        }
        if let Some(record) = state.get_mut(&zone) {
            record.insert("name".into(), json!(name.trim()));
        }
    }
    if input.contains_key("countries") {
        for country in keys_of(state, &zone, "countries") {
            for province in keys_of(state, &country, "provinces") {
                state.remove(&province);
            }
            state.remove(&country);
        }
        let countries = make_countries(state, input);
        if let Some(record) = state.get_mut(&zone) {
            record.insert("countries".into(), json!(countries));
        }
    }
    let mut methods = gids(entry.get("methodDefinitions"));
    for (index, method) in objects(input.get("methodDefinitionsToUpdate"))
        .iter()
        .enumerate()
    {
        let path = at(path, &["methodDefinitionsToUpdate", &index.to_string()]);
        update_method(state, method, &methods, &path, build);
    }
    for (index, method) in objects(input.get("methodDefinitionsToCreate"))
        .iter()
        .enumerate()
    {
        let path = at(path, &["methodDefinitionsToCreate", &index.to_string()]);
        if let Some(id) = make_method(state, method, &path, build) {
            methods.push(id);
        }
    }
    entry.insert("methodDefinitions".into(), json!(methods));
}

/// The locations of a group input that are locations of the shop (an error for the others).
fn group_locations(
    state: &ShopState,
    input: &Map<String, Value>,
    field: &str,
    path: &[String],
    build: &mut Build,
) -> Vec<String> {
    let mut locations = Vec::new();
    for (index, location) in gids(input.get(field)).into_iter().enumerate() {
        if state.type_of(&location) != Some("Location") {
            build.error(
                &at(path, &[field, &index.to_string()]),
                "Location not found",
            );
            continue;
        }
        if !locations.contains(&location) {
            locations.push(location);
        }
    }
    locations
}

/// Applies a `DeliveryProfileInput` to a profile (a new one has no groups yet).
fn apply(state: &mut ShopState, profile: &str, input: &Map<String, Value>, build: &mut Build) {
    let root = vec!["profile".to_string()];
    if let Some(name) = text(input, "name") {
        if name.trim().is_empty() {
            build.error(&at(&root, &["name"]), "Name can't be blank");
        } else if let Some(record) = state.get_mut(profile) {
            record.insert("name".into(), json!(name.trim()));
        }
    }
    if let Some(covers) = input.get("coversAllItems").and_then(Value::as_bool)
        && let Some(record) = state.get_mut(profile)
    {
        record.insert("coversAllItems".into(), json!(covers));
    }
    let mut entries = groups(state, profile);

    // Deletions first: method definitions, conditions, zones, groups.
    let deleted_methods = gids(input.get("methodDefinitionsToDelete"));
    let deleted_zones = gids(input.get("zonesToDelete"));
    let deleted_groups = gids(input.get("locationGroupsToDelete"));
    for condition in gids(input.get("conditionsToDelete")) {
        let owner = method_definitions(state, profile)
            .into_iter()
            .find(|method| {
                state
                    .get(method)
                    .is_some_and(|record| gids(record.get("methodConditions")).contains(&condition))
            });
        match owner {
            Some(method) => {
                super::pull(state, &method, "methodConditions", &condition);
                state.remove(&condition);
            }
            None => build.error(&at(&root, &["conditionsToDelete"]), "Condition not found"),
        }
    }
    let all_methods = method_definitions(state, profile);
    for method in &deleted_methods {
        if !all_methods.contains(method) {
            build.error(
                &at(&root, &["methodDefinitionsToDelete"]),
                "Method definition not found",
            );
        }
    }
    for entry in &mut entries {
        let mut zones = zones_of(entry);
        for zone in &mut zones {
            let mut methods = gids(zone.get("methodDefinitions"));
            methods.retain(|method| {
                let gone = deleted_methods.contains(method);
                if gone {
                    remove_method(state, method);
                }
                !gone
            });
            zone.insert("methodDefinitions".into(), json!(methods));
        }
        zones.retain(|zone| {
            let gone =
                text(zone, "zone").is_some_and(|id| deleted_zones.iter().any(|known| known == id));
            if gone {
                remove_zone(state, zone);
            }
            !gone
        });
        entry.insert(
            "locationGroupZones".into(),
            Value::Array(zones.into_iter().map(Value::Object).collect()),
        );
    }
    entries.retain(|entry| {
        let group = text(entry, "locationGroup").unwrap_or_default().to_string();
        let gone = deleted_groups.contains(&group);
        if gone {
            for zone in zones_of(entry) {
                remove_zone(state, &zone);
            }
            state.remove(&group);
        }
        !gone
    });

    // Groups to update (and those of the deprecated `profileLocationGroups` with an id).
    let legacy = objects(input.get("profileLocationGroups"));
    let updates: Vec<(Vec<String>, Map<String, Value>)> =
        objects(input.get("locationGroupsToUpdate"))
            .into_iter()
            .enumerate()
            .map(|(index, group)| {
                (
                    at(&root, &["locationGroupsToUpdate", &index.to_string()]),
                    group,
                )
            })
            .chain(
                legacy
                    .iter()
                    .enumerate()
                    .filter(|(_, group)| group.contains_key("id"))
                    .map(|(index, group)| {
                        (
                            at(&root, &["profileLocationGroups", &index.to_string()]),
                            group.clone(),
                        )
                    }),
            )
            .collect();
    for (path, group) in updates {
        let id = text(&group, "id").unwrap_or_default().to_string();
        let Some(entry) = entries
            .iter_mut()
            .find(|entry| text(entry, "locationGroup") == Some(id.as_str()))
        else {
            build.error(&at(&path, &["id"]), "Location group not found");
            continue;
        };
        let mut locations = keys_of(state, &id, "locations");
        let added: Vec<String> = ["locations", "locationsToAdd"]
            .iter()
            .flat_map(|field| group_locations(state, &group, field, &path, build))
            .collect();
        if group.contains_key("locations") {
            locations.clear();
        }
        for location in added {
            if !locations.contains(&location) {
                locations.push(location);
            }
        }
        let removed = gids(group.get("locationsToRemove"));
        locations.retain(|location| !removed.contains(location));
        if let Some(record) = state.get_mut(&id) {
            record.insert("locations".into(), json!(locations));
        }
        let mut zones = zones_of(entry);
        for (index, zone_input) in objects(group.get("zonesToUpdate")).iter().enumerate() {
            let zone_path = at(&path, &["zonesToUpdate", &index.to_string()]);
            let zone_id = text(zone_input, "id").unwrap_or_default();
            match zones
                .iter_mut()
                .find(|zone| text(zone, "zone") == Some(zone_id))
            {
                Some(zone) => update_zone(state, zone, zone_input, &zone_path, build),
                None => build.error(&at(&zone_path, &["id"]), "Zone not found"),
            }
        }
        for (index, zone_input) in objects(group.get("zonesToCreate")).iter().enumerate() {
            let zone_path = at(&path, &["zonesToCreate", &index.to_string()]);
            if let Some(Value::Object(zone)) = make_zone(state, zone_input, &zone_path, build) {
                zones.push(zone);
            }
        }
        entry.insert(
            "locationGroupZones".into(),
            Value::Array(zones.into_iter().map(Value::Object).collect()),
        );
    }

    // Groups to create (and those of `profileLocationGroups` without an id).
    let creations: Vec<(Vec<String>, Map<String, Value>)> =
        objects(input.get("locationGroupsToCreate"))
            .into_iter()
            .enumerate()
            .map(|(index, group)| {
                (
                    at(&root, &["locationGroupsToCreate", &index.to_string()]),
                    group,
                )
            })
            .chain(
                legacy
                    .iter()
                    .enumerate()
                    .filter(|(_, group)| !group.contains_key("id"))
                    .map(|(index, group)| {
                        (
                            at(&root, &["profileLocationGroups", &index.to_string()]),
                            group.clone(),
                        )
                    }),
            )
            .collect();
    for (path, group) in creations {
        let locations: Vec<String> = ["locations", "locationsToAdd"]
            .iter()
            .flat_map(|field| group_locations(state, &group, field, &path, build))
            .collect();
        let id = state.new_gid("DeliveryLocationGroup");
        state.insert(value::object(json!({
            "__typename": "DeliveryLocationGroup",
            "id": id,
            "locations": locations,
        })));
        let mut zones = Vec::new();
        for (index, zone_input) in objects(group.get("zonesToCreate")).iter().enumerate() {
            let zone_path = at(&path, &["zonesToCreate", &index.to_string()]);
            if let Some(zone) = make_zone(state, zone_input, &zone_path, build) {
                zones.push(zone);
            }
        }
        let mut entry = Map::new();
        entry.insert("locationGroup".into(), json!(id));
        entry.insert("locationGroupZones".into(), Value::Array(zones));
        entries.push(entry);
    }

    // A location is in one group of a profile.
    let mut seen: Vec<String> = Vec::new();
    for entry in &entries {
        let group = text(entry, "locationGroup").unwrap_or_default();
        for location in keys_of(state, group, "locations") {
            if seen.contains(&location) {
                build.error(
                    &at(&root, &["locationGroupsToCreate"]),
                    "A location can only be in one location group of a profile",
                );
                break;
            }
            seen.push(location);
        }
    }
    if let Some(record) = state.get_mut(profile) {
        record.insert(
            "profileLocationGroups".into(),
            Value::Array(entries.into_iter().map(Value::Object).collect()),
        );
    }

    // Variants and selling plan groups.
    let default = state
        .get(profile)
        .and_then(|record| record.get("default"))
        .and_then(Value::as_bool)
        == Some(true);
    for (index, variant) in gids(input.get("variantsToAssociate"))
        .into_iter()
        .enumerate()
    {
        if state.type_of(&variant) != Some("ProductVariant") {
            build.error(
                &at(&root, &["variantsToAssociate", &index.to_string()]),
                "Product variant not found",
            );
            continue;
        }
        let holders: Vec<String> = state.keys("DeliveryProfile").map(str::to_string).collect();
        for other in holders {
            super::pull(state, &other, VARIANTS, &variant);
        }
        if !default {
            push(state, profile, VARIANTS, &variant);
        }
    }
    for (index, variant) in gids(input.get("variantsToDissociate"))
        .into_iter()
        .enumerate()
    {
        if default {
            build.error(
                &at(&root, &["variantsToDissociate", &index.to_string()]),
                "Variants can't be removed from the general profile: associate them with another profile",
            );
            continue;
        }
        super::pull(state, profile, VARIANTS, &variant);
    }
    for (index, group) in gids(input.get("sellingPlanGroupsToAssociate"))
        .into_iter()
        .enumerate()
    {
        if state.type_of(&group) != Some("SellingPlanGroup") {
            build.error(
                &at(&root, &["sellingPlanGroupsToAssociate", &index.to_string()]),
                "Selling plan group not found",
            );
            continue;
        }
        push(state, profile, "sellingPlanGroups", &group);
    }
    for group in gids(input.get("sellingPlanGroupsToDissociate")) {
        super::pull(state, profile, "sellingPlanGroups", &group);
    }
}

/// Removes the method definitions whose rates a carrier service provided: what goes with a
/// carrier service deleted.
pub fn drop_carrier_service(state: &mut ShopState, carrier: &str) {
    let profiles: Vec<String> = state.keys("DeliveryProfile").map(str::to_string).collect();
    for profile in profiles {
        let gone: Vec<String> = method_definitions(state, &profile)
            .into_iter()
            .filter(|method| {
                state
                    .get(method)
                    .and_then(|record| text(record, "rateProvider"))
                    .and_then(|provider| state.get(provider))
                    .and_then(|provider| text(provider, "carrierService"))
                    == Some(carrier)
            })
            .collect();
        if gone.is_empty() {
            continue;
        }
        let mut entries = groups(state, &profile);
        for entry in &mut entries {
            let mut zones = zones_of(entry);
            for zone in &mut zones {
                let mut methods = gids(zone.get("methodDefinitions"));
                methods.retain(|method| !gone.contains(method));
                zone.insert("methodDefinitions".into(), json!(methods));
            }
            entry.insert(
                "locationGroupZones".into(),
                Value::Array(zones.into_iter().map(Value::Object).collect()),
            );
        }
        for method in &gone {
            remove_method(state, method);
        }
        bump(state, &profile, entries);
        state.emit("profiles/update", &profile);
    }
}

/// Keeps a profile's groups and moves its version on.
fn bump(state: &mut ShopState, profile: &str, entries: Vec<Map<String, Value>>) {
    if let Some(record) = state.get_mut(profile) {
        record.insert(
            "profileLocationGroups".into(),
            Value::Array(entries.into_iter().map(Value::Object).collect()),
        );
        let version = int(record, "version") + 1;
        record.insert("version".into(), json!(version));
    }
}

// --- The mutations ---

fn create(mutation: &mut Mutation<'_>, ctx: &FieldContext<'_>) -> Result<Value, FieldError> {
    let input = super::input(ctx, "profile");
    let currency = model::currency(mutation.state());
    if text(&input, "name").is_none_or(|name| name.trim().is_empty()) {
        return Ok(payload(
            json!({"profile": null}),
            &[UserError::new(&["profile", "name"], "Name can't be blank")],
        ));
    }
    let state = mutation.state_mut();
    let now = state.timestamp();
    let id = state.new_gid("DeliveryProfile");
    state.insert(value::object(json!({
        "__typename": "DeliveryProfile",
        "id": id,
        "name": "",
        "default": false,
        "version": 1,
        "coversAllItems": false,
        "createdAt": now,
        "profileLocationGroups": [],
        "sellingPlanGroups": [],
        VARIANTS: [],
    })));
    let mut build = Build {
        errors: Vec::new(),
        currency,
    };
    apply(state, &id, &input, &mut build);
    if !build.errors.is_empty() {
        return Ok(payload(json!({"profile": null}), &build.errors));
    }
    state.emit("profiles/create", &id);
    Ok(payload(json!({"profile": id}), &[]))
}

fn update(mutation: &mut Mutation<'_>, ctx: &FieldContext<'_>) -> Result<Value, FieldError> {
    let id = ctx.arg_str("id").unwrap_or_default().to_string();
    if mutation.state().type_of(&id) != Some("DeliveryProfile") {
        return Ok(payload(
            json!({"profile": null}),
            &[UserError::new(&["id"], "Profile could not be found")],
        ));
    }
    let input = super::input(ctx, "profile");
    let currency = model::currency(mutation.state());
    let state = mutation.state_mut();
    let mut build = Build {
        errors: Vec::new(),
        currency,
    };
    apply(state, &id, &input, &mut build);
    if !build.errors.is_empty() {
        return Ok(payload(json!({"profile": null}), &build.errors));
    }
    let entries = groups(state, &id);
    bump(state, &id, entries);
    state.emit("profiles/update", &id);
    Ok(payload(json!({"profile": id}), &[]))
}

/// `deliveryProfileRemove`: removed at once, its job done.
fn remove(mutation: &mut Mutation<'_>, ctx: &FieldContext<'_>) -> Result<Value, FieldError> {
    let id = ctx.arg_str("id").unwrap_or_default().to_string();
    let state = mutation.state();
    let Some(record) = (state.type_of(&id) == Some("DeliveryProfile"))
        .then(|| state.get(&id).cloned())
        .flatten()
    else {
        return Ok(payload(
            json!({"job": null}),
            &[UserError::new(&["id"], "Profile could not be found")],
        ));
    };
    if record.get("default").and_then(Value::as_bool) == Some(true) {
        return Ok(payload(
            json!({"job": null}),
            &[UserError::new(
                &["id"],
                "The general profile can't be removed",
            )],
        ));
    }
    let state = mutation.state_mut();
    state.emit("profiles/delete", &id);
    for entry in groups(state, &id) {
        for zone in zones_of(&entry) {
            remove_zone(state, &zone);
        }
        if let Some(group) = text(&entry, "locationGroup") {
            state.remove(group);
        }
    }
    state.remove(&id);
    let job = crate::resolve::done_job(state, "delivery_profile_remove");
    Ok(payload(json!({"job": job}), &[]))
}

// --- Store sync ---

/// The shipping rates the local checkout offers, as a profile in an era of the store sync gives
/// them: the store data's rates in their order, each shipping rate replaced by the method
/// definition the seed made of it in the general profile's zones that ship to the rest of the
/// world (left out when it is gone or inactive), then the other such method definitions with a
/// flat price and no condition, under a handle made of their name. `get` reads a record of the
/// era.
pub fn checkout_rates<'r>(
    get: &dyn Fn(&str) -> Option<&'r Record>,
    data: &[CheckoutShippingRate],
) -> Vec<CheckoutShippingRateInput> {
    let general = general_profile_gid();
    let methods: Vec<&Record> = get(&general)
        .map(|profile| objects(profile.get("profileLocationGroups")))
        .unwrap_or_default()
        .iter()
        .flat_map(zones_of)
        .filter(|zone| {
            text(zone, "zone").and_then(get).is_some_and(|zone| {
                gids(zone.get("countries")).iter().any(|country| {
                    get(country)
                        .and_then(|country| country.get("code"))
                        .and_then(|code| code.get("restOfWorld"))
                        .and_then(Value::as_bool)
                        == Some(true)
                })
            })
        })
        .flat_map(|zone| gids(zone.get("methodDefinitions")))
        .filter_map(|method| get(&method))
        .collect();
    let flat = |method: &Record| -> Option<i64> {
        if method.get("active").and_then(Value::as_bool) != Some(true)
            || !gids(method.get("methodConditions")).is_empty()
        {
            return None;
        }
        let provider = get(text(method, "rateProvider")?)?;
        (text(provider, "__typename") == Some("DeliveryRateDefinition"))
            .then(|| crate::domains::catalog::cents_of(provider.get("price")?.get("amount")))
            .flatten()
    };
    let input = |handle: &str, method: &Record, cents: i64| CheckoutShippingRateInput {
        handle: handle.to_string(),
        title: text(method, "name").map(str::to_string),
        price: Money::Decimal(value::decimal(cents)),
        description: text(method, "description").map(str::to_string),
        kind: None,
    };
    let mut rates = Vec::new();
    for rate in data {
        if rate.kind != CheckoutDeliveryKind::Shipping {
            rates.push(CheckoutShippingRateInput {
                handle: rate.handle.clone(),
                title: Some(rate.title.clone()),
                price: Money::Decimal(value::decimal(rate.price)),
                description: rate.description.clone(),
                kind: Some(rate.kind),
            });
            continue;
        }
        if let Some(method) = methods
            .iter()
            .find(|method| text(method, RATE_HANDLE) == Some(rate.handle.as_str()))
            && let Some(cents) = flat(method)
        {
            rates.push(input(&rate.handle, method, cents));
        }
    }
    for method in &methods {
        if text(method, RATE_HANDLE).is_some() {
            continue;
        }
        if let Some(cents) = flat(method) {
            let name = text(method, "name").unwrap_or_default();
            let mut handle = handleize(name);
            let mut n = 2;
            while rates.iter().any(|rate| rate.handle == handle) {
                handle = format!("{}-{n}", handleize(name));
                n += 1;
            }
            rates.push(input(&handle, method, cents));
        }
    }
    rates
}

/// A handle of a name: lower case, words joined by `-`.
fn handleize(name: &str) -> String {
    let mut handle = String::new();
    for c in name.chars() {
        if c.is_alphanumeric() {
            handle.extend(c.to_lowercase());
        } else if !handle.ends_with('-') && !handle.is_empty() {
            handle.push('-');
        }
    }
    let handle = handle.trim_end_matches('-').to_string();
    if handle.is_empty() {
        "rate".to_string()
    } else {
        handle
    }
}
