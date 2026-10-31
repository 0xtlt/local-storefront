//! Store sync: what the Admin API changed, written back as store data, so that the storefront
//! and the checkout see it. A product the backend creates is on the theme's pages, a customer it
//! creates can log in at the checkout, a metafield it sets is read by Liquid and by the Function
//! inputs, an app discount it creates runs its Function at the checkout.
//!
//! [`overlay`] projects the records of a shop that changed since the seed (see
//! [`ShopState::changes`]) onto the store data they were seeded from: the entities to upsert, in
//! the format of a data file, and the entities to remove, as a [`lsf_core::store::load::Overlay`]
//! that `load_with_overlays` applies on top of the files. Only what changed is projected: an
//! entity whose records changed (a product when one of its variants, inventory levels or
//! metafields did), and the entities that name, by a key that changed or that is gone, one that
//! changed (a collection listing a product whose handle changed). Each is projected twice, from
//! the records as they are and as the seed made them: when both give the same store data, the
//! change does not show in the store (a derived field, a change undone) and is left out.
//!
//! What syncs, what does not, the ids, and the steps the server takes: the "Store sync" section
//! of `crates/admin/docs/design.md`.

use std::collections::{HashMap, HashSet};

use indexmap::{IndexMap, IndexSet};
use lsf_core::Store;
use lsf_core::store::load::{Overlay, Removals};
use lsf_core::store::model::{self, Money};
use serde::Serialize;
use serde_json::{Map, Value, json};

use crate::domains::{catalog, content};
use crate::state::{ChangeKind, Record, ShopState, gid, gid_type, parse_gid};

/// What diagnostics name the overlay of the Admin API's changes.
pub const OVERLAY_NAME: &str = "(admin api)";

/// The changes the Admin API made to a shop since the seed, as an overlay of the store data the
/// shop was seeded from (`store`: the store loaded from the files, without any overlay). `None`
/// when nothing that the store data expresses changed: the store stays as the files make it.
///
/// Cheap when nothing changed (one check); otherwise it reads the records once to know what
/// owns what, then projects the entities that changed.
pub fn overlay(state: &ShopState, store: &Store) -> Option<Overlay> {
    if !state.touched() {
        return None;
    }
    let changes = state.changes();
    if changes.is_empty() {
        return None;
    }
    let sync = Sync::new(state, store, &changes);
    sync.project(&changes)
}

/// The types of the records that are an entity of the store data, which the other records belong
/// to (a variant to its product, a metafield to its owner).
const ROOTS: [&str; 19] = [
    "Product",
    "Collection",
    "Page",
    "Blog",
    "Menu",
    "Customer",
    "Company",
    "GiftCard",
    "Location",
    "Metaobject",
    "SellingPlanGroup",
    "Market",
    "Shop",
    "DiscountAutomaticNode",
    "DiscountCodeNode",
    "CartTransform",
    "DeliveryCustomization",
    "PaymentCustomization",
    "Validation",
];

/// The owners of a Function that is not a discount, whose metafields are its settings.
const FUNCTION_OWNERS: [&str; 4] = [
    "CartTransform",
    "DeliveryCustomization",
    "PaymentCustomization",
    "Validation",
];

/// The fields that point from a record to the record it belongs to.
const PARENT_FIELDS: [(&str, &str); 10] = [
    ("ProductVariant", "product"),
    ("InventoryItem", "variant"),
    ("InventoryLevel", "item"),
    ("Metafield", "owner"),
    ("Article", "blog"),
    ("Comment", "article"),
    ("CompanyLocation", "company"),
    ("CompanyContact", "customer"),
    ("CompanyContactRoleAssignment", "companyContact"),
    ("Order", "customer"),
];

/// The lists of records a record holds, which belong to it.
const CHILD_LISTS: [(&str, &str); 11] = [
    ("Product", "variants"),
    ("Product", "options"),
    ("Product", "media"),
    ("ProductOption", "optionValues"),
    ("InventoryItem", "inventoryLevels"),
    ("Blog", "articles"),
    ("Article", "comments"),
    ("Company", "locations"),
    ("Order", "lineItems"),
    ("SellingPlanGroup", "sellingPlans"),
    ("Shop", "shopPolicies"),
];

/// The records as they are now, or as the seed made them.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Era {
    Seed,
    Now,
}

/// The records of one era, with what the projection looks up in them.
struct View<'s> {
    state: &'s ShopState,
    era: Era,
    /// The metafields of each owner, in creation order.
    metafields: HashMap<&'s str, Vec<&'s str>>,
    /// The collections that list each product, in creation order.
    collections_of: HashMap<&'s str, Vec<&'s str>>,
    /// The selling plan groups that list each product.
    groups_of: HashMap<&'s str, Vec<&'s str>>,
    /// The company contacts of each customer.
    contacts_of: HashMap<&'s str, Vec<&'s str>>,
}

impl<'s> View<'s> {
    fn new(state: &'s ShopState, era: Era, removed: &HashMap<String, Vec<&'s str>>) -> View<'s> {
        let mut view = View {
            state,
            era,
            metafields: HashMap::new(),
            collections_of: HashMap::new(),
            groups_of: HashMap::new(),
            contacts_of: HashMap::new(),
        };
        let mut metafields: HashMap<&'s str, Vec<&'s str>> = HashMap::new();
        for key in view.keys("Metafield", removed) {
            if let Some(owner) = view.get(key).and_then(|record| text(record, "owner")) {
                metafields.entry(owner).or_default().push(key);
            }
        }
        let mut collections_of: HashMap<&'s str, Vec<&'s str>> = HashMap::new();
        for key in view.keys("Collection", removed) {
            for product in view
                .get(key)
                .map(|record| list(record, "products"))
                .into_iter()
                .flatten()
            {
                collections_of.entry(product).or_default().push(key);
            }
        }
        let mut groups_of: HashMap<&'s str, Vec<&'s str>> = HashMap::new();
        for key in view.keys("SellingPlanGroup", removed) {
            for product in view
                .get(key)
                .map(|record| list(record, "products"))
                .into_iter()
                .flatten()
            {
                groups_of.entry(product).or_default().push(key);
            }
        }
        let mut contacts_of: HashMap<&'s str, Vec<&'s str>> = HashMap::new();
        for key in view.keys("CompanyContact", removed) {
            if let Some(customer) = view.get(key).and_then(|record| text(record, "customer")) {
                contacts_of.entry(customer).or_default().push(key);
            }
        }
        view.metafields = metafields;
        view.collections_of = collections_of;
        view.groups_of = groups_of;
        view.contacts_of = contacts_of;
        view
    }

    fn get(&self, key: &str) -> Option<&'s Record> {
        match self.era {
            Era::Now => self.state.get(key),
            Era::Seed => self.state.original(key),
        }
    }

    /// The keys of the records of a type in this era: those there now, and in the seed's era
    /// also those removed since.
    fn keys(&self, type_name: &str, removed: &HashMap<String, Vec<&'s str>>) -> Vec<&'s str> {
        let mut keys: Vec<&'s str> = self.state.keys(type_name).collect();
        if self.era == Era::Seed {
            keys.extend(removed.get(type_name).into_iter().flatten().copied());
        }
        keys.into_iter()
            .filter(|key| self.get(key).is_some())
            .collect()
    }

    fn metafields_of(&self, owner: &str) -> &[&'s str] {
        self.metafields.get(owner).map_or(&[], Vec::as_slice)
    }

    /// Whether a product is on the storefront: active and published on the Online Store.
    fn product_listed(&self, product: &str) -> bool {
        self.get(product).is_some_and(|record| {
            text(record, "status").is_none_or(|status| status == "ACTIVE")
                && text(record, "publishedAt").is_some()
        })
    }

    /// Whether a variant can be the line of an order on the storefront: it exists, and its
    /// product is on the storefront.
    fn variant_listed(&self, variant: &str) -> bool {
        self.get(variant)
            .and_then(|record| text(record, "product"))
            .is_some_and(|product| self.product_listed(product))
    }

    fn published(&self, key: &str) -> bool {
        self.get(key)
            .is_some_and(|record| flag(record, "isPublished") != Some(false))
    }

    fn location_active(&self, key: &str) -> bool {
        self.get(key)
            .is_some_and(|record| flag(record, "isActive") != Some(false))
    }
}

/// An entity of the store data, projected from its records.
enum Projected {
    Product(model::ProductInput),
    Collection(model::CollectionInput),
    Page(model::PageInput),
    Blog(model::BlogInput),
    Menu(String, model::MenuInput),
    Customer(model::CustomerInput),
    Company(model::CompanyInput),
    GiftCard(model::GiftCardInput),
    Location(model::LocationInput),
    Metaobject(String, model::MetaobjectInput),
    SellingPlanGroup(model::SellingPlanGroupInput),
}

impl Projected {
    /// The key an overlay replaces and removes the entity by.
    fn key(&self) -> String {
        match self {
            Projected::Product(product) => product.handle.clone().unwrap_or_default(),
            Projected::Collection(collection) => collection.handle.clone().unwrap_or_default(),
            Projected::Page(page) => page.handle.clone().unwrap_or_default(),
            Projected::Blog(blog) => blog.handle.clone().unwrap_or_default(),
            Projected::Menu(handle, _) => handle.clone(),
            Projected::Customer(customer) => customer.email.to_lowercase(),
            Projected::Company(company) => company.name.to_lowercase(),
            Projected::GiftCard(card) => card.code.to_uppercase(),
            Projected::Location(location) => location.name.to_lowercase(),
            Projected::Metaobject(kind, entry) => format!("{kind}/{}", entry.handle),
            Projected::SellingPlanGroup(group) => group.name.to_lowercase(),
        }
    }

    fn json(&self) -> Value {
        fn to(value: &impl Serialize) -> Value {
            serde_json::to_value(value).unwrap_or(Value::Null)
        }
        match self {
            Projected::Product(value) => to(value),
            Projected::Collection(value) => to(value),
            Projected::Page(value) => to(value),
            Projected::Blog(value) => to(value),
            Projected::Menu(handle, value) => json!([handle, to(value)]),
            Projected::Customer(value) => to(value),
            Projected::Company(value) => to(value),
            Projected::GiftCard(value) => to(value),
            Projected::Location(value) => to(value),
            Projected::Metaobject(kind, value) => json!([kind, to(value)]),
            Projected::SellingPlanGroup(value) => to(value),
        }
    }

    /// Whether the overlay replaces the entity by its id, so that a new key leaves it where it
    /// is: the entities of the data that have an `id`.
    fn replaced_by_id(&self) -> bool {
        matches!(
            self,
            Projected::Product(_)
                | Projected::Collection(_)
                | Projected::Page(_)
                | Projected::Blog(_)
                | Projected::Customer(_)
                | Projected::Company(_)
                | Projected::Location(_)
        )
    }

    fn remove(&self, removals: &mut Removals) {
        let key = match self {
            Projected::Customer(customer) => customer.email.clone(),
            Projected::Company(company) => company.name.clone(),
            Projected::GiftCard(card) => card.code.clone(),
            Projected::Location(location) => location.name.clone(),
            Projected::SellingPlanGroup(group) => group.name.clone(),
            Projected::Metaobject(kind, entry) => {
                removals
                    .metaobjects
                    .entry(kind.clone())
                    .or_default()
                    .push(entry.handle.clone());
                return;
            }
            other => other.key(),
        };
        let list = match self {
            Projected::Product(_) => &mut removals.products,
            Projected::Collection(_) => &mut removals.collections,
            Projected::Page(_) => &mut removals.pages,
            Projected::Blog(_) => &mut removals.blogs,
            Projected::Menu(..) => &mut removals.menus,
            Projected::Customer(_) => &mut removals.customers,
            Projected::Company(_) => &mut removals.companies,
            Projected::GiftCard(_) => &mut removals.gift_cards,
            Projected::Location(_) => &mut removals.locations,
            Projected::SellingPlanGroup(_) => &mut removals.selling_plan_groups,
            Projected::Metaobject(..) => return,
        };
        if !list.contains(&key) {
            list.push(key);
        }
    }

    fn add(self, input: &mut model::StoreInput) {
        match self {
            Projected::Product(value) => input.products.push(value),
            Projected::Collection(value) => input.collections.push(value),
            Projected::Page(value) => input.pages.push(value),
            Projected::Blog(value) => input.blogs.push(value),
            Projected::Menu(handle, value) => {
                input.menus.insert(handle, value);
            }
            Projected::Customer(value) => input.customers.push(value),
            Projected::Company(value) => input.companies.push(value),
            Projected::GiftCard(value) => input.gift_cards.push(value),
            Projected::Location(value) => input.locations.push(value),
            Projected::Metaobject(kind, value) => {
                input.metaobjects.entry(kind).or_default().push(value)
            }
            Projected::SellingPlanGroup(value) => input.selling_plan_groups.push(value),
        }
    }
}

/// The projection of one shop.
struct Sync<'s> {
    state: &'s ShopState,
    store: &'s Store,
    now: View<'s>,
    seed: View<'s>,
    /// The record each record belongs to, from the lists that hold them (`Product.variants`).
    parents: HashMap<&'s str, &'s str>,
    /// `app--<id>`: the reserved namespace of the app, `$app` in the store data.
    app_namespace: Option<String>,
    /// Whether the store data has locations once the projection applies, so that variants can
    /// name them.
    locations_in_data: bool,
}

impl<'s> Sync<'s> {
    fn new(state: &'s ShopState, store: &'s Store, changes: &[crate::state::Change<'s>]) -> Self {
        // The keys of the records removed since the seed, by type: the seed's era still has them.
        let mut removed: HashMap<String, Vec<&'s str>> = HashMap::new();
        for change in changes {
            if change.kind == ChangeKind::Removed
                && let Some(type_name) = change
                    .original
                    .and_then(|record| type_of(change.key, record))
            {
                removed
                    .entry(type_name.to_string())
                    .or_default()
                    .push(change.key);
            }
        }
        let mut parents = HashMap::new();
        let mut index = |record: &'s Record, key: &'s str| {
            let Some(type_name) = type_of(key, record) else {
                return;
            };
            for (parent_type, field) in CHILD_LISTS {
                if parent_type == type_name {
                    for child in list(record, field) {
                        parents.entry(child).or_insert(key);
                    }
                }
            }
        };
        for (key, record) in state.iter() {
            index(record, key);
        }
        for change in changes {
            if let Some(original) = change.original {
                index(original, change.key);
            }
        }
        let app_namespace = state
            .keys("App")
            .next()
            .and_then(parse_gid)
            .map(|(_, id)| format!("app--{id}"));
        let locations_in_data = !store.locations.is_empty()
            || changes.iter().any(|change| {
                state
                    .get(change.key)
                    .or(change.original)
                    .and_then(|record| type_of(change.key, record))
                    == Some("Location")
            });
        Sync {
            state,
            store,
            now: View::new(state, Era::Now, &removed),
            seed: View::new(state, Era::Seed, &removed),
            parents,
            app_namespace,
            locations_in_data,
        }
    }

    fn view(&self, era: Era) -> &View<'s> {
        match era {
            Era::Now => &self.now,
            Era::Seed => &self.seed,
        }
    }

    /// The entity of the store data a record belongs to: its type and key.
    fn root_of(&self, key: &'s str) -> Option<(&'s str, &'s str)> {
        let mut key = key;
        for _ in 0..8 {
            let record = self.now.get(key).or_else(|| self.seed.get(key))?;
            let type_name = type_of(key, record)?;
            if ROOTS.contains(&type_name) {
                return Some((type_name, key));
            }
            let pointer = PARENT_FIELDS
                .iter()
                .find(|(child, _)| *child == type_name)
                .and_then(|(_, field)| text(record, field));
            key = pointer.or_else(|| self.parents.get(key).copied())?;
        }
        None
    }

    fn project(&self, changes: &[crate::state::Change<'s>]) -> Option<Overlay> {
        let mut roots: IndexSet<(&'s str, &'s str)> = IndexSet::new();
        for change in changes {
            if let Some(root) = self.root_of(change.key) {
                roots.insert(root);
            }
        }

        let mut input = model::StoreInput::default();
        let mut removals = Removals::default();
        let mut shop_patch = Map::new();
        let mut done: HashSet<&'s str> = HashSet::new();
        let mut discounts = false;
        let mut functions: IndexSet<String> = IndexSet::new();
        let mut markets = false;

        // Entities, then the entities that name one whose key changed or that is gone, until
        // there are no more. Those are written even when what they project is the same: the data
        // names the entity by the key that changed (a handle, a SKU), they by what does not (an
        // id).
        let mut queue: Vec<(&'s str, &'s str)> = roots.into_iter().collect();
        let mut naming: HashSet<&'s str> = HashSet::new();
        let mut variants = self.moved_variants();
        while !queue.is_empty() {
            let mut moved: HashSet<&'s str> = std::mem::take(&mut variants).into_iter().collect();
            let mut members: Vec<&'s str> = Vec::new();
            for (type_name, key) in std::mem::take(&mut queue) {
                if !done.insert(key) {
                    continue;
                }
                match type_name {
                    "Shop" => self.shop(&mut shop_patch, &mut removals),
                    "Market" => markets = true,
                    "DiscountAutomaticNode" | "DiscountCodeNode" => discounts = true,
                    owner if FUNCTION_OWNERS.contains(&owner) => {
                        for era in [Era::Now, Era::Seed] {
                            if let Some(handle) = self.function_of(self.view(era), key) {
                                functions.insert(handle);
                            }
                        }
                    }
                    _ => {
                        let now = self.entity(Era::Now, type_name, key);
                        let seed = self.entity(Era::Seed, type_name, key);
                        let same = match (&now, &seed) {
                            (Some(now), Some(seed)) => {
                                !naming.contains(key) && now.json() == seed.json()
                            }
                            (None, None) => true,
                            _ => false,
                        };
                        if same {
                            continue;
                        }
                        if let Some(seed) = &seed {
                            match &now {
                                // Gone: removed. Renamed: replaced where it is when it has an
                                // id, removed and added again otherwise.
                                None => seed.remove(&mut removals),
                                Some(now) if now.key() != seed.key() && !now.replaced_by_id() => {
                                    seed.remove(&mut removals)
                                }
                                Some(_) => {}
                            }
                            if now.as_ref().is_none_or(|now| now.key() != seed.key()) {
                                moved.insert(key);
                            }
                        }
                        members.extend(self.membership_changes(type_name, key));
                        if let Some(now) = now {
                            now.add(&mut input);
                        }
                    }
                }
            }
            // A product that joined or left a collection or a group is written again: it may name
            // them itself.
            for product in members {
                if !done.contains(product) {
                    queue.push(("Product", product));
                }
            }
            if !moved.is_empty() {
                for root in self.naming(&moved, &done) {
                    naming.insert(root.1);
                    queue.push(root);
                }
            }
        }

        if markets {
            input.localization = self.localization();
        }
        let mut checkout = model::CheckoutInput::default();
        if discounts {
            let now = self.discounts(Era::Now);
            if serde_json::to_value(&now).ok()
                != serde_json::to_value(self.discounts(Era::Seed)).ok()
            {
                checkout.discounts = Some(now);
            }
        }
        for handle in functions {
            let now = self.function_settings(Era::Now, &handle);
            let seed = self.function_settings(Era::Seed, &handle);
            if serde_json::to_value(&now).ok() != serde_json::to_value(&seed).ok() {
                checkout.functions.insert(handle, now);
            }
        }
        if checkout.discounts.is_some() || !checkout.functions.is_empty() {
            input.checkout = Some(checkout);
        }

        let mut data = serde_json::to_value(&input).unwrap_or(Value::Null);
        if let Value::Object(map) = &mut data {
            if !shop_patch.is_empty() {
                map.insert("shop".to_string(), Value::Object(shop_patch));
            }
            map.retain(|_, value| !is_empty_section(value));
            strip_nulls(&mut data);
        }
        let data_is_empty = data.as_object().is_none_or(Map::is_empty);
        if data_is_empty && removals.is_empty() {
            return None;
        }
        Some(Overlay {
            name: OVERLAY_NAME.to_string(),
            removed: removals,
            data: if data_is_empty { Value::Null } else { data },
        })
    }

    /// The products that joined or left a collection or a selling plan group, between the seed
    /// and now.
    fn membership_changes(&self, type_name: &str, key: &str) -> Vec<&'s str> {
        if type_name != "Collection" && type_name != "SellingPlanGroup" {
            return Vec::new();
        }
        let products = |view: &View<'s>| -> HashSet<&'s str> {
            view.get(key)
                .map(|record| list(record, "products").collect())
                .unwrap_or_default()
        };
        let (now, seed) = (products(&self.now), products(&self.seed));
        now.symmetric_difference(&seed).copied().collect()
    }

    /// The variants whose SKU changed or that are gone: orders and metafields of the data may
    /// name them by their SKU.
    fn moved_variants(&self) -> Vec<&'s str> {
        self.state
            .changes()
            .into_iter()
            .filter(|change| {
                change
                    .original
                    .and_then(|record| type_of(change.key, record))
                    == Some("ProductVariant")
            })
            .filter(|change| {
                let sku = |view: &View<'s>| view.get(change.key).map(|record| text(record, "sku"));
                change.kind == ChangeKind::Removed || sku(&self.now) != sku(&self.seed)
            })
            .map(|change| change.key)
            .collect()
    }

    /// The entities that name one of `moved` (the records of the entities whose key changed or
    /// that are gone, and the variants whose SKU changed): their data names them by their key,
    /// which they must be written again with.
    fn naming(&self, moved: &HashSet<&'s str>, done: &HashSet<&'s str>) -> Vec<(&'s str, &'s str)> {
        let mut found = IndexSet::new();
        for (key, record) in self.state.iter() {
            if moved.contains(key) || !mentions(record, moved) {
                continue;
            }
            if let Some(root) = self.root_of(key)
                && !done.contains(root.1)
                && !moved.contains(root.1)
            {
                found.insert(root);
            }
        }
        // A gift card names the product it was bought as, which the records do not keep.
        for card in &self.store.gift_cards {
            let Some(product) = card
                .product
                .and_then(|index| self.store.products.get(index))
            else {
                continue;
            };
            let product = catalog::product_gid(product);
            let card = catalog::gift_card_gid(card);
            if moved.contains(product.as_str())
                && let Some(key) = self.state.keys("GiftCard").find(|key| *key == card)
                && !done.contains(key)
            {
                found.insert(("GiftCard", key));
            }
        }
        found.into_iter().collect()
    }

    fn entity(&self, era: Era, type_name: &str, key: &str) -> Option<Projected> {
        let view = self.view(era);
        match type_name {
            "Product" => self.product(view, key).map(Projected::Product),
            "Collection" => self.collection(view, key).map(Projected::Collection),
            "Page" => self.page(view, key).map(Projected::Page),
            "Blog" => self.blog(view, key).map(Projected::Blog),
            "Menu" => self
                .menu(view, key)
                .map(|(handle, menu)| Projected::Menu(handle, menu)),
            "Customer" => self.customer(view, key).map(Projected::Customer),
            "Company" => self.company(view, key).map(Projected::Company),
            "GiftCard" => self.gift_card(view, key).map(Projected::GiftCard),
            "Location" => self.location(view, key).map(Projected::Location),
            "Metaobject" => self
                .metaobject(view, key)
                .map(|(kind, entry)| Projected::Metaobject(kind, entry)),
            "SellingPlanGroup" => self
                .selling_plan_group(view, key)
                .map(Projected::SellingPlanGroup),
            _ => None,
        }
    }

    // ---- metafields and references --------------------------------------------------------

    /// The namespace of a metafield as the store data writes it: the app's reserved namespace
    /// `app--<id>--config` is `$app:config`.
    fn namespace(&self, namespace: &str) -> String {
        if let Some(app) = &self.app_namespace {
            if namespace == app {
                return "$app".to_string();
            }
            if let Some(name) = namespace
                .strip_prefix(app.as_str())
                .and_then(|rest| rest.strip_prefix("--"))
            {
                return format!("$app:{name}");
            }
        }
        namespace.to_string()
    }

    /// The metafields of an owner, as the store data writes them.
    fn metafields(&self, view: &View<'s>, owner: &str) -> model::Metafields {
        let mut metafields = model::Metafields::new();
        for (namespace, key, field) in self.metafield_inputs(view, owner) {
            metafields.entry(namespace).or_default().insert(key, field);
        }
        metafields
    }

    /// The metafields of a checkout setting (a discount, a Function), by `namespace.key`.
    fn checkout_metafields(
        &self,
        view: &View<'s>,
        owner: &str,
    ) -> IndexMap<String, model::MetafieldInput> {
        self.metafield_inputs(view, owner)
            .into_iter()
            .map(|(namespace, key, field)| (format!("{namespace}.{key}"), field))
            .collect()
    }

    fn metafield_inputs(
        &self,
        view: &View<'s>,
        owner: &str,
    ) -> Vec<(String, String, model::MetafieldInput)> {
        view.metafields_of(owner)
            .iter()
            .filter_map(|key| view.get(key))
            .filter_map(|record| {
                let namespace = self.namespace(text(record, "namespace")?);
                let key = text(record, "key")?.to_string();
                let kind = text(record, "type")?;
                let value = self.value(view, kind, record.get("value")?)?;
                Some((
                    namespace,
                    key,
                    model::MetafieldInput::Typed(model::TypedMetafield {
                        kind: kind.to_string(),
                        value,
                    }),
                ))
            })
            .collect()
    }

    /// A metafield or metaobject field value of the Admin API (the string Admin returns) as the
    /// store data writes it: JSON for the types that hold JSON, a number or a boolean for those
    /// types, and references by the key the data names them by (a handle, a path of `files/`,
    /// `type/handle`, an email), which Liquid and the Functions read.
    fn value(&self, view: &View<'s>, kind: &str, value: &Value) -> Option<Value> {
        let text = match value {
            Value::String(text) => text.clone(),
            Value::Null => return None,
            other => other.to_string(),
        };
        let mut json = content::json_value(kind, &text);
        if kind.strip_prefix("list.").unwrap_or(kind) == "money" {
            // The data writes an amount in the shop's currency without it.
            let currency = self
                .state
                .shop()
                .and_then(|shop| text_of(shop, "currencyCode"));
            let strip = |money: &mut Value| {
                if let Value::Object(money) = money
                    && money.get("currency_code").and_then(Value::as_str) == currency
                {
                    money.remove("currency_code");
                }
            };
            match &mut json {
                Value::Array(items) => items.iter_mut().for_each(strip),
                money => strip(money),
            }
        }
        if !kind.ends_with("_reference") {
            return Some(json);
        }
        let base = kind.strip_prefix("list.").unwrap_or(kind);
        Some(match json {
            Value::Array(items) => Value::Array(
                items
                    .iter()
                    .filter_map(|item| {
                        let gid = item.as_str()?;
                        // A reference to what is gone names nothing.
                        view.get(gid)?;
                        Some(self.reference(view, base, gid))
                    })
                    .collect(),
            ),
            Value::String(gid) => self.reference(view, base, &gid),
            other => other,
        })
    }

    /// The key the store data names a record by, in a reference: the GID itself when it names
    /// something the data has no key for.
    fn reference(&self, view: &View<'s>, kind: &str, gid: &str) -> Value {
        let named = view.get(gid).and_then(|record| {
            Some(match gid_type(gid)? {
                "Product" | "Collection" | "Page" | "Blog" => json!(text(record, "handle")?),
                "ProductVariant" => json!(parse_gid(gid)?.1),
                "Article" => {
                    let blog = view.get(text(record, "blog")?)?;
                    json!(format!(
                        "{}/{}",
                        text(blog, "handle")?,
                        text(record, "handle")?
                    ))
                }
                "Metaobject" => json!(format!(
                    "{}/{}",
                    text(record, "type")?,
                    text(record, "handle")?
                )),
                "MediaImage" | "Video" | "GenericFile" if kind == "file_reference" => {
                    json!(file_path(record)?)
                }
                "Customer" => json!(text(record, "email")?),
                "Company" => json!(text(record, "name")?),
                _ => return None,
            })
        });
        named.unwrap_or_else(|| json!(gid))
    }

    // ---- catalog -------------------------------------------------------------------------

    fn base_product(&self, id: u64) -> Option<&'s lsf_core::store::Product> {
        self.store
            .products
            .iter()
            .chain(&self.store.unlisted_products)
            .find(|product| product.id == id)
    }

    fn product(&self, view: &View<'s>, key: &str) -> Option<model::ProductInput> {
        let record = view.get(key)?;
        let id = parse_gid(key)?.1;
        let base = self.base_product(id);
        let options = list(record, "options")
            .filter_map(|option| view.get(option))
            .map(|option| {
                let values = list(option, "optionValues")
                    .filter_map(|value| view.get(value))
                    .filter_map(|value| {
                        let name = text(value, "name")?.to_string();
                        let swatch = value.get("swatch").filter(|swatch| !swatch.is_null());
                        Some(match swatch.and_then(|swatch| self.swatch(view, swatch)) {
                            Some(swatch) => {
                                model::OptionValueInput::Detailed(model::OptionValueDetail {
                                    name,
                                    swatch: Some(swatch),
                                })
                            }
                            None => model::OptionValueInput::Name(name),
                        })
                    })
                    .collect::<Vec<_>>();
                // The values in the order the option keeps them.
                let order: Vec<&str> = list(option, "values").collect();
                let mut values = values;
                values.sort_by_key(|value| {
                    order
                        .iter()
                        .position(|name| *name == value.name())
                        .unwrap_or(usize::MAX)
                });
                model::OptionInput::Detailed(model::OptionDetail {
                    name: text(option, "name").unwrap_or_default().to_string(),
                    values,
                })
            })
            .collect();
        let media: Vec<(String, model::MediaInput)> = list(record, "media")
            .filter_map(|media| Some((media.to_string(), self.media(view, media, base)?)))
            .collect();
        let variants = list(record, "variants")
            .filter_map(|variant| self.variant(view, variant, base, &media))
            .collect();
        let collections = view
            .collections_of
            .get(key)
            .into_iter()
            .flatten()
            .filter_map(|collection| view.get(collection))
            .filter_map(|collection| text(collection, "handle").map(str::to_string))
            .collect();
        let selling_plan_groups = view
            .groups_of
            .get(key)
            .into_iter()
            .flatten()
            .filter_map(|group| view.get(group))
            .filter_map(|group| text(group, "name").map(str::to_string))
            .collect();
        let status = match text(record, "status") {
            Some("DRAFT") => model::ProductStatus::Draft,
            Some("ARCHIVED") => model::ProductStatus::Archived,
            _ => model::ProductStatus::Active,
        };
        let published_at = string(record, "publishedAt");
        Some(model::ProductInput {
            id: Some(id),
            title: string(record, "title").unwrap_or_default(),
            handle: Some(text(record, "handle")?.to_string()),
            description: string(record, "descriptionHtml")
                .or_else(|| string(record, "bodyHtml"))
                .unwrap_or_default(),
            vendor: string(record, "vendor"),
            product_type: string(record, "productType").unwrap_or_default(),
            tags: list(record, "tags").map(str::to_string).collect(),
            options,
            variants,
            media: media.into_iter().map(|(_, media)| media).collect(),
            collections,
            template_suffix: string(record, "templateSuffix"),
            gift_card: flag(record, "isGiftCard").unwrap_or(false),
            created_at: string(record, "createdAt"),
            published: published_at.is_none().then_some(false),
            published_at,
            updated_at: string(record, "updatedAt"),
            metafields: self.metafields(view, key),
            selling_plan_groups,
            requires_selling_plan: flag(record, "requiresSellingPlan").unwrap_or(false),
            status,
            ..model::ProductInput::default()
        })
    }

    fn swatch(&self, view: &View<'s>, swatch: &Value) -> Option<model::SwatchInput> {
        let color = swatch
            .get("color")
            .and_then(Value::as_str)
            .map(str::to_string);
        let image = swatch
            .get("image")
            .and_then(Value::as_str)
            .and_then(|file| view.get(file))
            .and_then(file_path)
            .map(model::ImageInput::Src);
        match (color, image) {
            (None, None) => None,
            (Some(color), None) => Some(model::SwatchInput::Color(color)),
            (color, image) => Some(model::SwatchInput::Detailed(model::SwatchDetail {
                color,
                image,
            })),
        }
    }

    /// A piece of product media. Images must be files of `files/`: those the Admin API took from
    /// elsewhere are left out.
    fn media(
        &self,
        view: &View<'s>,
        key: &str,
        base: Option<&lsf_core::store::Product>,
    ) -> Option<model::MediaInput> {
        let record = view.get(key)?;
        let id = parse_gid(key)?.1;
        let base = base.and_then(|product| product.media.iter().find(|media| media.id == id));
        let alt = string(record, "alt");
        let preview = record
            .get("preview")
            .and_then(|preview| preview.get("image"))
            .filter(|image| !image.is_null());
        let source = |source: &Value| -> Option<model::MediaSource> {
            let url = source.get("url")?.as_str()?;
            Some(model::MediaSource {
                url: files_path(url).unwrap_or_else(|| url.to_string()),
                mime_type: source
                    .get("mimeType")
                    .and_then(Value::as_str)
                    .map(str::to_string),
                format: source
                    .get("format")
                    .and_then(Value::as_str)
                    .map(str::to_string),
                width: source
                    .get("width")
                    .and_then(Value::as_u64)
                    .map(|n| n as u32),
                height: source
                    .get("height")
                    .and_then(Value::as_u64)
                    .map(|n| n as u32),
            })
        };
        let detail = |media_type: model::MediaType| model::MediaDetail {
            media_type,
            id: Some(id),
            alt: alt.clone(),
            preview_image: preview
                .and_then(image_input)
                .map(model::ImageInput::Detailed),
            sources: record
                .get("sources")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
                .filter_map(source)
                .collect(),
            duration: None,
            host: None,
            external_id: None,
            aspect_ratio: base.map(|media| media.aspect_ratio),
        };
        Some(match text(record, "__typename")? {
            "MediaImage" => {
                let image = record
                    .get("image")
                    .filter(|image| !image.is_null())
                    .or(preview)?;
                let mut detail = image_input(image)?;
                detail.alt = alt.or(detail.alt);
                detail.focal_point = base
                    .and_then(|media| media.preview.as_ref())
                    .and_then(|image| image.focal_point)
                    .map(|(x, y)| model::FocalPoint { x, y });
                model::MediaInput::Image(model::ImageInput::Detailed(detail))
            }
            "Video" => model::MediaInput::Other(model::MediaDetail {
                duration: record.get("duration").and_then(Value::as_u64),
                ..detail(model::MediaType::Video)
            }),
            "Model3d" => model::MediaInput::Other(detail(model::MediaType::Model)),
            "ExternalVideo" => {
                let host = match text(record, "host") {
                    Some("VIMEO") => model::VideoHost::Vimeo,
                    _ => model::VideoHost::Youtube,
                };
                let url = text(record, "originUrl").or_else(|| text(record, "embedUrl"))?;
                model::MediaInput::Other(model::MediaDetail {
                    host: Some(host),
                    external_id: Some(external_video_id(url)),
                    sources: Vec::new(),
                    ..detail(model::MediaType::ExternalVideo)
                })
            }
            _ => return None,
        })
    }

    fn variant(
        &self,
        view: &View<'s>,
        key: &str,
        base: Option<&lsf_core::store::Product>,
        media: &[(String, model::MediaInput)],
    ) -> Option<model::VariantInput> {
        let record = view.get(key)?;
        let id = parse_gid(key)?.1;
        let base =
            base.and_then(|product| product.variants.iter().find(|variant| variant.id == id));
        let item_key = text(record, "inventoryItem");
        let item = item_key.and_then(|item| view.get(item));
        let tracked = item.and_then(|item| flag(item, "tracked")).unwrap_or(false);
        let policy = match text(record, "inventoryPolicy") {
            Some("CONTINUE") => model::InventoryPolicy::Continue,
            _ => model::InventoryPolicy::Deny,
        };
        let levels: Vec<(&str, &Record)> = item
            .map(|item| list(item, "inventoryLevels").collect::<Vec<_>>())
            .unwrap_or_default()
            .into_iter()
            .filter_map(|level| Some((level, view.get(level)?)))
            .collect();
        let quantity = if levels.is_empty() {
            record
                .get("inventoryQuantity")
                .and_then(Value::as_i64)
                .unwrap_or(0)
        } else {
            levels
                .iter()
                .map(|(_, level)| catalog::available_of(level))
                .sum()
        };
        let derived = !tracked || quantity > 0 || policy == model::InventoryPolicy::Continue;
        let available = flag(record, "availableForSale").filter(|available| *available != derived);
        let weight = item.and_then(|item| item.get("measurement")?.get("weight"));
        let (grams, unit) = match weight {
            Some(weight) => {
                let (unit, factor) = match weight.get("unit").and_then(Value::as_str) {
                    Some("GRAMS") => ("g", 1.0),
                    Some("OUNCES") => ("oz", 28.349_523_125),
                    Some("POUNDS") => ("lb", 453.592_37),
                    _ => ("kg", 1000.0),
                };
                let value = weight.get("value").and_then(Value::as_f64).unwrap_or(0.0);
                (
                    Some((value * factor).round().max(0.0) as u64),
                    Some(unit.to_string()),
                )
            }
            None => (None, None),
        };
        let image = list(record, "media")
            .next()
            .and_then(|media_key| media.iter().find(|(key, _)| key == media_key))
            .and_then(|(_, media)| match media {
                model::MediaInput::Image(image) => Some(image.detail().src),
                model::MediaInput::Other(_) => None,
            });
        // Where the variant is stocked: as the data says while its inventory is as the seed made
        // it, from its inventory levels once the Admin API changed them.
        let inventory_changed = item_key.is_some_and(|item| self.state.changed(item))
            || levels.iter().any(|(level, _)| self.state.changed(level));
        let store_availabilities: Option<Vec<(String, bool)>> = if !self.locations_in_data {
            None
        } else if let (Some(base), false) = (base, inventory_changed && view.era == Era::Now) {
            Some(
                base.store_availabilities
                    .iter()
                    .filter_map(|(index, in_stock)| {
                        let location = self.store.locations.get(*index)?;
                        let key = catalog::location_gid(location);
                        let record = view.get(&key).filter(|_| view.location_active(&key))?;
                        Some((text(record, "name")?.to_string(), *in_stock))
                    })
                    .collect(),
            )
        } else {
            Some(
                levels
                    .iter()
                    .filter_map(|(_, level)| {
                        let location =
                            text(level, "location").filter(|key| view.location_active(key))?;
                        let name = text(view.get(location)?, "name")?.to_string();
                        let in_stock = catalog::available_of(level) > 0
                            || !tracked
                            || policy == model::InventoryPolicy::Continue;
                        Some((name, in_stock))
                    })
                    .collect(),
            )
        };
        let quantity_rule = base
            .map(|variant| &variant.quantity_rule)
            .filter(|rule| rule.min != 1 || rule.max.is_some() || rule.increment != 1)
            .map(|rule| model::QuantityRule {
                min: Some(rule.min),
                max: rule.max,
                increment: Some(rule.increment),
            });
        let measurement = record
            .get("unitPriceMeasurement")
            .filter(|value| !value.is_null());
        Some(model::VariantInput {
            id: Some(id),
            title: string(record, "title"),
            options: record
                .get("selectedOptions")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
                .filter_map(|option| option.get("value")?.as_str())
                .map(str::to_string)
                .collect(),
            price: Some(Money::Cents(
                catalog::cents_of(record.get("price")).unwrap_or(0),
            )),
            compare_at_price: catalog::cents_of(record.get("compareAtPrice")).map(Money::Cents),
            sku: string(record, "sku"),
            barcode: string(record, "barcode"),
            available,
            inventory_quantity: Some(quantity),
            inventory_tracked: Some(tracked),
            inventory_policy: policy,
            weight: grams,
            weight_unit: unit,
            requires_shipping: item.and_then(|item| flag(item, "requiresShipping")),
            taxable: flag(record, "taxable"),
            image,
            unit_price: record.get("unitPrice").and_then(money_v2).map(Money::Cents),
            unit_price_measurement: measurement.map(|measurement| model::UnitPriceMeasurement {
                measured_type: measurement
                    .get("measuredType")
                    .and_then(Value::as_str)
                    .map(str::to_ascii_lowercase),
                quantity_value: measurement
                    .get("quantityValue")
                    .and_then(Value::as_f64)
                    .unwrap_or(1.0),
                quantity_unit: measurement
                    .get("quantityUnit")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_ascii_lowercase(),
                reference_value: measurement.get("referenceValue").and_then(Value::as_f64),
                reference_unit: measurement
                    .get("referenceUnit")
                    .and_then(Value::as_str)
                    .map(str::to_ascii_lowercase),
            }),
            quantity_rule,
            quantity_price_breaks: base
                .map(|variant| {
                    variant
                        .quantity_price_breaks
                        .iter()
                        .map(|(minimum_quantity, price)| model::QuantityPriceBreak {
                            minimum_quantity: *minimum_quantity,
                            price: Money::Cents(*price),
                        })
                        .collect()
                })
                .unwrap_or_default(),
            store_availabilities: store_availabilities.map(|stocked| stocked.into_iter().collect()),
            metafields: self.metafields(view, key),
        })
    }

    fn collection(&self, view: &View<'s>, key: &str) -> Option<model::CollectionInput> {
        let record = view.get(key)?;
        let sort_order = match text(record, "sortOrder") {
            Some("BEST_SELLING") => model::SortOrder::BestSelling,
            Some("ALPHA_ASC") => model::SortOrder::TitleAscending,
            Some("ALPHA_DESC") => model::SortOrder::TitleDescending,
            Some("PRICE_ASC") => model::SortOrder::PriceAscending,
            Some("PRICE_DESC") => model::SortOrder::PriceDescending,
            Some("CREATED") => model::SortOrder::CreatedAscending,
            Some("CREATED_DESC") => model::SortOrder::CreatedDescending,
            _ => model::SortOrder::Manual,
        };
        Some(model::CollectionInput {
            id: Some(parse_gid(key)?.1),
            title: string(record, "title").unwrap_or_default(),
            handle: Some(text(record, "handle")?.to_string()),
            description: string(record, "descriptionHtml").unwrap_or_default(),
            image: record
                .get("image")
                .filter(|image| !image.is_null())
                .and_then(image_input)
                .map(model::ImageInput::Detailed),
            products: list(record, "products")
                .filter_map(|product| view.get(product))
                .filter_map(|product| text(product, "handle").map(str::to_string))
                .collect(),
            sort_order,
            template_suffix: string(record, "templateSuffix"),
            published_at: string(record, "createdAt"),
            updated_at: string(record, "updatedAt"),
            metafields: self.metafields(view, key),
            ..model::CollectionInput::default()
        })
    }

    fn location(&self, view: &View<'s>, key: &str) -> Option<model::LocationInput> {
        let record = view.get(key)?;
        if !view.location_active(key) {
            return None;
        }
        let id = parse_gid(key)?.1;
        let base = self
            .store
            .locations
            .iter()
            .find(|location| location.id == id);
        let address = record.get("address").filter(|address| !address.is_null());
        let pickup = record
            .get("localPickupSettingsV2")
            .and_then(|settings| settings.get("pickupTime"))
            .and_then(Value::as_str);
        let pick_up_time = pickup.map(|time| {
            // The words of the data when they say the same, Shopify's otherwise.
            base.map(|location| location.pick_up_time.clone())
                .filter(|words| catalog::pickup_time(words) == Some(time))
                .unwrap_or_else(|| pickup_words(time).to_string())
        });
        let field = |name: &str| {
            address
                .and_then(|address| address.get(name))
                .and_then(Value::as_str)
        };
        Some(model::LocationInput {
            id: Some(id),
            name: text(record, "name")?.to_string(),
            address: address.map(|_| model::AddressInput {
                address1: field("address1").map(str::to_string),
                address2: field("address2").map(str::to_string),
                city: field("city").map(str::to_string),
                province: field("province").map(str::to_string),
                province_code: field("provinceCode").map(str::to_string),
                country: field("country").map(str::to_string),
                country_code: field("countryCode").map(str::to_string),
                zip: field("zip").map(str::to_string),
                phone: field("phone").map(str::to_string),
                id: base.map(|location| location.address.id),
                ..model::AddressInput::default()
            }),
            pick_up_enabled: Some(pickup.is_some()),
            pick_up_time,
            physical_storefront: base.map(|location| location.physical_storefront),
            latitude: address.and_then(|address| address.get("latitude")?.as_f64()),
            longitude: address.and_then(|address| address.get("longitude")?.as_f64()),
            metafields: self.metafields(view, key),
        })
    }

    fn gift_card(&self, view: &View<'s>, key: &str) -> Option<model::GiftCardInput> {
        let record = view.get(key)?;
        let id = parse_gid(key)?.1;
        // The code is the key of a card in the data, which the Admin API never shows again
        // after creating it: only the cards of the data sync.
        let base = self.store.gift_cards.iter().find(|card| card.id == id)?;
        let recipient = record
            .get("recipientAttributes")
            .filter(|value| !value.is_null());
        let recipient_email = recipient
            .and_then(|recipient| recipient.get("recipient")?.as_str())
            .and_then(|customer| view.get(customer))
            .and_then(|customer| string(customer, "email"));
        let balance = record.get("balance");
        Some(model::GiftCardInput {
            code: base.code.clone(),
            initial_value: Money::Cents(
                record
                    .get("initialValue")
                    .and_then(money_v2)
                    .unwrap_or(base.initial_value),
            ),
            balance: balance.and_then(money_v2).map(Money::Cents),
            currency: balance
                .and_then(|money| money.get("currencyCode")?.as_str())
                .map(str::to_string),
            expires_on: string(record, "expiresOn"),
            enabled: flag(record, "enabled"),
            customer: text(record, "customer")
                .and_then(|customer| view.get(customer))
                .and_then(|customer| string(customer, "email")),
            message: recipient
                .and_then(|recipient| recipient.get("message")?.as_str())
                .map(str::to_string)
                .or_else(|| (!base.message.is_empty()).then(|| base.message.clone())),
            recipient: match (recipient, &base.recipient) {
                (Some(attributes), base_recipient) => Some(model::GiftCardRecipientInput {
                    name: base_recipient
                        .as_ref()
                        .map(|r| r.name.clone())
                        .filter(|n| !n.is_empty()),
                    email: recipient_email,
                    nickname: attributes
                        .get("preferredName")
                        .and_then(Value::as_str)
                        .map(str::to_string),
                }),
                (None, Some(base)) => Some(model::GiftCardRecipientInput {
                    name: (!base.name.is_empty()).then(|| base.name.clone()),
                    email: (!base.email.is_empty()).then(|| base.email.clone()),
                    nickname: (!base.nickname.is_empty()).then(|| base.nickname.clone()),
                }),
                (None, None) => None,
            },
            send_on: recipient
                .and_then(|recipient| recipient.get("sendNotificationAt")?.as_str())
                .map(str::to_string)
                .or_else(|| base.send_on.map(crate::state::value::date_time)),
            product: base
                .product
                .and_then(|index| self.store.products.get(index))
                .map(catalog::product_gid)
                .and_then(|product| view.get(&product))
                .and_then(|product| string(product, "handle")),
            properties: base.properties.clone(),
            template_suffix: string(record, "templateSuffix"),
        })
    }

    fn selling_plan_group(
        &self,
        view: &View<'s>,
        key: &str,
    ) -> Option<model::SellingPlanGroupInput> {
        let record = view.get(key)?;
        let plans = list(record, "sellingPlans")
            .filter_map(|plan| Some((plan, view.get(plan)?)))
            .map(|(plan_key, plan)| {
                let policies: Vec<&Value> = plan
                    .get("pricingPolicies")
                    .and_then(Value::as_array)
                    .map(|policies| policies.iter().collect())
                    .unwrap_or_default();
                let price_adjustments = policies
                    .iter()
                    .enumerate()
                    .filter_map(|(index, policy)| {
                        let (value_type, value) = adjustment(policy)?;
                        let order_count = policies
                            .get(index + 1)
                            .and_then(|next| next.get("afterCycle")?.as_u64())
                            .map(|count| count as u32);
                        Some(model::PriceAdjustmentInput {
                            value_type,
                            value,
                            order_count,
                        })
                    })
                    .collect();
                let billing = plan.get("billingPolicy");
                let recurring = billing
                    .and_then(|billing| billing.get("__typename")?.as_str())
                    .is_none_or(|kind| kind == "SellingPlanRecurringBillingPolicy");
                let checkout_charge = billing
                    .and_then(|billing| billing.get("checkoutCharge"))
                    .filter(|_| !recurring)
                    .and_then(|charge| {
                        let value = charge.get("value")?;
                        Some(match charge.get("type")?.as_str()? {
                            "PRICE" => model::CheckoutChargeInput {
                                value_type: model::CheckoutChargeType::Price,
                                value: Money::Cents(money_v2(value)?),
                            },
                            _ => model::CheckoutChargeInput {
                                value_type: model::CheckoutChargeType::Percentage,
                                value: Money::Cents(
                                    value.get("percentage")?.as_f64()?.round() as i64
                                ),
                            },
                        })
                    });
                model::SellingPlanInput {
                    id: parse_gid(plan_key).map(|(_, id)| id),
                    name: string(plan, "name").unwrap_or_default(),
                    description: string(plan, "description"),
                    options: list(plan, "options").map(str::to_string).collect(),
                    price_adjustments,
                    recurring_deliveries: Some(recurring),
                    checkout_charge,
                }
            })
            .collect();
        Some(model::SellingPlanGroupInput {
            name: text(record, "name")?.to_string(),
            app_id: string(record, "appId"),
            options: list(record, "options").map(str::to_string).collect(),
            selling_plans: plans,
            products: list(record, "products")
                .filter_map(|product| view.get(product))
                .filter_map(|product| string(product, "handle"))
                .collect(),
        })
    }

    // ---- customers -----------------------------------------------------------------------

    fn customer(&self, view: &View<'s>, key: &str) -> Option<model::CustomerInput> {
        let record = view.get(key)?;
        let id = parse_gid(key)?.1;
        // A customer of the data has an email: one without is not on the storefront.
        let email = text(record, "email")?.to_string();
        let base = self
            .store
            .customers
            .iter()
            .find(|customer| customer.id == id);
        let default_address = record
            .get("defaultAddress")
            .and_then(|address| address.get("id"))
            .and_then(Value::as_str);
        let mut addresses: Vec<&Value> = record
            .get("addressesV2")
            .or_else(|| record.get("addresses"))
            .and_then(Value::as_array)
            .map(|addresses| addresses.iter().collect())
            .unwrap_or_default();
        // The default address is the first one in the data.
        if let Some(position) = addresses
            .iter()
            .position(|address| address.get("id").and_then(Value::as_str) == default_address)
        {
            let default = addresses.remove(position);
            addresses.insert(0, default);
        }
        let consent = record
            .get("emailMarketingConsent")
            .or_else(|| record.get("defaultEmailAddress"))
            .and_then(|consent| consent.get("marketingState"))
            .and_then(Value::as_str);
        // The company the customer buys for: the one of their contact, and the locations their
        // roles give them.
        let contact = view
            .contacts_of
            .get(key)
            .into_iter()
            .flatten()
            .find_map(|contact| view.get(contact));
        let company = contact
            .and_then(|contact| text(contact, "company"))
            .and_then(|company| view.get(company));
        let company_locations = contact
            .map(|contact| {
                list(contact, "roleAssignments")
                    .filter_map(|assignment| view.get(assignment))
                    .filter_map(|assignment| view.get(text(assignment, "companyLocation")?))
                    .filter_map(|location| string(location, "name"))
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        Some(model::CustomerInput {
            id: Some(id),
            email,
            first_name: string(record, "firstName"),
            last_name: string(record, "lastName"),
            phone: string(record, "phone"),
            tags: list(record, "tags").map(str::to_string).collect(),
            accepts_marketing: consent == Some("SUBSCRIBED"),
            has_account: Some(text(record, "state") == Some("ENABLED")),
            tax_exempt: flag(record, "taxExempt").unwrap_or(false),
            addresses: addresses.into_iter().map(mailing_address).collect(),
            orders: base
                .map(|customer| self.orders(view, customer))
                .unwrap_or_default(),
            company: company.and_then(|company| string(company, "name")),
            company_locations: if company.is_some() {
                company_locations
            } else {
                Vec::new()
            },
            company_contact: contact.zip(company).and_then(|(contact, company)| {
                let details = model::CompanyContactInput {
                    title: string(contact, "title"),
                    locale: string(contact, "locale"),
                    created_at: string(contact, "createdAt"),
                    updated_at: string(contact, "updatedAt"),
                };
                // Without a title or a language, and with the company's dates, the data says
                // nothing of the contact.
                let defaults = details.title.is_none()
                    && details.locale.is_none()
                    && details.created_at == string(company, "createdAt")
                    && details.updated_at == details.created_at;
                (!defaults).then_some(details)
            }),
            metafields: self.metafields(view, key),
            ..model::CustomerInput::default()
        })
    }

    /// The orders of a customer of the data, as the data has them (the Admin API's changes to
    /// orders do not sync), naming their variants by id; the lines of variants that are no longer
    /// on the storefront are left out.
    fn orders(
        &self,
        view: &View<'s>,
        customer: &lsf_core::store::Customer,
    ) -> Vec<model::OrderInput> {
        let variant_id = |(product, variant): (usize, usize)| {
            self.store
                .products
                .get(product)?
                .variants
                .get(variant)
                .map(|variant| variant.id)
        };
        let address = |address: &lsf_core::store::Address| model::AddressInput {
            id: Some(address.id),
            first_name: some(&address.first_name),
            last_name: some(&address.last_name),
            company: some(&address.company),
            address1: some(&address.address1),
            address2: some(&address.address2),
            city: some(&address.city),
            province: some(&address.province),
            province_code: some(&address.province_code),
            country: some(&address.country),
            country_code: some(&address.country_code),
            zip: some(&address.zip),
            phone: some(&address.phone),
        };
        customer
            .orders
            .iter()
            .map(|order| {
                let kept = |variant: u64| view.variant_listed(&gid("ProductVariant", variant));
                let line_items = order
                    .line_items
                    .iter()
                    .filter_map(|line| Some((variant_id(line.variant)?, line.quantity)))
                    .filter(|(variant, _)| kept(*variant))
                    .map(|(variant, quantity)| model::OrderLineInput {
                        variant: model::VariantRef::Id(variant),
                        quantity,
                    })
                    .collect();
                let returns = order
                    .returns
                    .iter()
                    .map(|sent| model::OrderReturnInput {
                        status: Some(sent.status.clone()),
                        line_items: sent
                            .lines
                            .iter()
                            .filter_map(|(index, quantity)| {
                                let variant = variant_id(order.line_items.get(*index)?.variant)?;
                                kept(variant).then_some(model::OrderLineInput {
                                    variant: model::VariantRef::Id(variant),
                                    quantity: *quantity,
                                })
                            })
                            .collect(),
                    })
                    .collect();
                model::OrderInput {
                    id: Some(order.id),
                    name: Some(order.name.clone()),
                    created_at: Some(crate::state::value::date_time(order.created_at)),
                    financial_status: Some(order.financial_status.clone()),
                    fulfillment_status: Some(order.fulfillment_status.clone()),
                    line_items,
                    shipping_address: order.shipping_address.as_ref().map(address),
                    billing_address: order.billing_address.as_ref().map(address),
                    shipping_price: Some(Money::Cents(order.shipping_price)),
                    tax_price: Some(Money::Cents(order.tax_price)),
                    cancelled: order.cancelled,
                    note: some(&order.note),
                    returns,
                }
            })
            .collect()
    }

    fn company(&self, view: &View<'s>, key: &str) -> Option<model::CompanyInput> {
        let record = view.get(key)?;
        let id = parse_gid(key)?.1;
        let base = self.store.companies.iter().find(|company| company.id == id);
        let company_name = text(record, "name")?;
        let locations = list(record, "locations")
            .filter_map(|location| Some((location, view.get(location)?)))
            .filter_map(|(location_key, location)| {
                let location_id = parse_gid(location_key)?.1;
                let base_location = base.and_then(|company| {
                    company
                        .locations
                        .iter()
                        .find(|candidate| candidate.id == location_id)
                });
                let address = location
                    .get("shippingAddress")
                    .filter(|address| !address.is_null());
                let field = |name: &str| {
                    address
                        .and_then(|address| address.get(name))
                        .and_then(Value::as_str)
                        .map(str::to_string)
                };
                Some(model::CompanyLocationInput {
                    id: Some(location_id),
                    name: text(location, "name")?.to_string(),
                    external_id: string(location, "externalId"),
                    shipping_address: address.map(|_| model::AddressInput {
                        id: base_location
                            .and_then(|location| location.shipping_address.as_ref())
                            .map(|address| address.id),
                        first_name: field("firstName"),
                        last_name: field("lastName"),
                        company: field("companyName").filter(|name| name != company_name),
                        address1: field("address1"),
                        address2: field("address2"),
                        city: field("city"),
                        province: field("province"),
                        province_code: field("zoneCode"),
                        country: field("country"),
                        country_code: field("countryCode"),
                        zip: field("zip"),
                        phone: field("phone"),
                    }),
                    tax_registration_id: location
                        .get("taxSettings")
                        .and_then(|settings| settings.get("taxRegistrationId"))
                        .and_then(Value::as_str)
                        .or_else(|| text(location, "taxRegistrationId"))
                        .map(str::to_string),
                    locale: string(location, "locale"),
                    created_at: string(location, "createdAt"),
                    updated_at: string(location, "updatedAt"),
                    orders_count: location
                        .get("ordersCount")
                        .and_then(|count| count.get("count"))
                        .and_then(Value::as_u64)
                        .map(|count| count as u32),
                    total_spent: location
                        .get("totalSpent")
                        .and_then(money_v2)
                        .map(Money::Cents),
                    metafields: self.metafields(view, location_key),
                })
            })
            .collect();
        Some(model::CompanyInput {
            id: Some(id),
            name: company_name.to_string(),
            external_id: string(record, "externalId"),
            locations,
            created_at: string(record, "createdAt"),
            updated_at: string(record, "updatedAt"),
            metafields: self.metafields(view, key),
        })
    }

    // ---- content -------------------------------------------------------------------------

    fn page(&self, view: &View<'s>, key: &str) -> Option<model::PageInput> {
        let record = view.get(key)?;
        // An unpublished page is not on the storefront.
        if !view.published(key) {
            return None;
        }
        let id = parse_gid(key)?.1;
        let base = self.store.pages.iter().find(|page| page.id == id);
        Some(model::PageInput {
            id: Some(id),
            title: string(record, "title").unwrap_or_default(),
            handle: Some(text(record, "handle")?.to_string()),
            content: string(record, "body").unwrap_or_default(),
            author: base.and_then(|page| some(&page.author)),
            template_suffix: string(record, "templateSuffix"),
            published_at: string(record, "publishedAt"),
            updated_at: string(record, "updatedAt"),
            metafields: self.metafields(view, key),
            ..model::PageInput::default()
        })
    }

    fn blog(&self, view: &View<'s>, key: &str) -> Option<model::BlogInput> {
        let record = view.get(key)?;
        let (comments_enabled, moderated) = match text(record, "commentPolicy") {
            Some("MODERATED") => (true, true),
            Some("AUTO_PUBLISHED") => (true, false),
            _ => (false, false),
        };
        let articles = list(record, "articles")
            .filter(|article| view.published(article))
            .filter_map(|article| self.article(view, article))
            .collect();
        Some(model::BlogInput {
            id: Some(parse_gid(key)?.1),
            title: string(record, "title").unwrap_or_default(),
            handle: Some(text(record, "handle")?.to_string()),
            articles,
            comments_enabled,
            moderated,
            template_suffix: string(record, "templateSuffix"),
            metafields: self.metafields(view, key),
            ..model::BlogInput::default()
        })
    }

    fn article(&self, view: &View<'s>, key: &str) -> Option<model::ArticleInput> {
        let record = view.get(key)?;
        let comments = list(record, "comments")
            .filter_map(|comment| Some((comment, view.get(comment)?)))
            .filter(|(_, comment)| {
                text(comment, "status").is_none_or(|status| status == "PUBLISHED")
            })
            .filter_map(|(comment_key, comment)| {
                let author = comment.get("author");
                Some(model::CommentInput {
                    id: parse_gid(comment_key).map(|(_, id)| id),
                    author: author
                        .and_then(|author| author.get("name")?.as_str())
                        .unwrap_or_default()
                        .to_string(),
                    email: author
                        .and_then(|author| author.get("email")?.as_str())
                        .map(str::to_string),
                    content: string(comment, "bodyHtml").or_else(|| string(comment, "body"))?,
                    created_at: string(comment, "createdAt"),
                })
            })
            .collect();
        Some(model::ArticleInput {
            id: Some(parse_gid(key)?.1),
            title: string(record, "title").unwrap_or_default(),
            handle: Some(text(record, "handle")?.to_string()),
            author: record
                .get("author")
                .and_then(|author| author.get("name")?.as_str())
                .map(str::to_string),
            content: string(record, "body").unwrap_or_default(),
            excerpt: string(record, "summary"),
            image: record
                .get("image")
                .filter(|image| !image.is_null())
                .and_then(image_input)
                .map(model::ImageInput::Detailed),
            tags: list(record, "tags").map(str::to_string).collect(),
            comments,
            template_suffix: string(record, "templateSuffix"),
            created_at: string(record, "createdAt"),
            published_at: string(record, "publishedAt"),
            updated_at: string(record, "updatedAt"),
            metafields: self.metafields(view, key),
        })
    }

    fn menu(&self, view: &View<'s>, key: &str) -> Option<(String, model::MenuInput)> {
        let record = view.get(key)?;
        let handle = text(record, "handle")?.to_string();
        let links = record
            .get("items")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .map(|item| self.link(view, item))
            .collect();
        Some((
            handle,
            model::MenuInput {
                title: string(record, "title"),
                links,
                ..model::MenuInput::default()
            },
        ))
    }

    /// A menu item: its resource by its key when the resource is on the storefront, its URL
    /// otherwise.
    fn link(&self, view: &View<'s>, item: &Value) -> model::LinkInput {
        let field = |name: &str| item.get(name).and_then(Value::as_str);
        let resource = field("resourceId");
        let record = resource.and_then(|resource| view.get(resource));
        let handle = || record.and_then(|record| string(record, "handle"));
        let mut link = model::LinkInput {
            title: field("title").unwrap_or_default().to_string(),
            links: item
                .get("items")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
                .map(|child| self.link(view, child))
                .collect(),
            ..model::LinkInput::default()
        };
        match field("type") {
            Some("PRODUCT") if resource.is_some_and(|key| view.product_listed(key)) => {
                link.product = handle();
            }
            Some("COLLECTION") if record.is_some() => link.collection = handle(),
            Some("PAGE") if resource.is_some_and(|key| view.published(key)) => link.page = handle(),
            Some("BLOG") if record.is_some() => link.blog = handle(),
            Some("ARTICLE") if resource.is_some_and(|key| view.published(key)) => {
                link.article = record.and_then(|article| {
                    let blog = view.get(text(article, "blog")?)?;
                    Some(format!(
                        "{}/{}",
                        text(blog, "handle")?,
                        text(article, "handle")?
                    ))
                });
            }
            Some("SHOP_POLICY") => {
                link.policy = record
                    .and_then(|policy| text(policy, "type"))
                    .and_then(policy_handle)
                    .map(str::to_string);
            }
            _ => {}
        }
        let targeted = link.product.is_some()
            || link.collection.is_some()
            || link.page.is_some()
            || link.blog.is_some()
            || link.article.is_some()
            || link.policy.is_some();
        if !targeted {
            link.url = field("url").map(|url| strip_origin(url, self.origin()));
        }
        link
    }

    /// `https://<primary domain>`, which the URLs of records are on.
    fn origin(&self) -> Option<&'s str> {
        self.state.shop().and_then(|shop| text(shop, "url"))
    }

    fn metaobject(&self, view: &View<'s>, key: &str) -> Option<(String, model::MetaobjectInput)> {
        let record = view.get(key)?;
        let capabilities = record.get("capabilities");
        // A draft entry is not on the storefront.
        let status = capabilities
            .and_then(|capabilities| capabilities.get("publishable")?.get("status")?.as_str());
        if status == Some("DRAFT") {
            return None;
        }
        let fields = record
            .get("fields")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(|field| {
                let key = field.get("key")?.as_str()?.to_string();
                let kind = field.get("type")?.as_str()?;
                let value = self.value(view, kind, field.get("value")?)?;
                Some((
                    key,
                    model::MetafieldInput::Typed(model::TypedMetafield {
                        kind: kind.to_string(),
                        value,
                    }),
                ))
            })
            .collect();
        Some((
            text(record, "type")?.to_string(),
            model::MetaobjectInput {
                handle: text(record, "handle")?.to_string(),
                fields,
                template_suffix: capabilities
                    .and_then(|capabilities| {
                        capabilities
                            .get("onlineStore")?
                            .get("templateSuffix")?
                            .as_str()
                    })
                    .filter(|suffix| !suffix.is_empty())
                    .map(str::to_string),
            },
        ))
    }

    /// What changed of the shop itself: its name, email and description, its policies, and its
    /// metafields (`shop` is merged field by field: a metafield it no longer has is a removal).
    fn shop(&self, patch: &mut Map<String, Value>, removals: &mut Removals) {
        let (Some(now), Some(seed)) = (
            self.state.shop(),
            self.state.shop_gid().and_then(|key| self.seed.get(key)),
        ) else {
            return;
        };
        for (field, name) in [
            ("name", "name"),
            ("email", "email"),
            ("description", "description"),
        ] {
            if now.get(field) != seed.get(field)
                && let Some(value) = string(now, field)
            {
                patch.insert(name.to_string(), json!(value));
            }
        }
        let Some(key) = self.state.shop_gid() else {
            return;
        };
        let now_fields = self.metafields(&self.now, key);
        let seed_fields = self.metafields(&self.seed, key);
        let mut metafields = Map::new();
        for (namespace, fields) in &now_fields {
            for (field_key, field) in fields {
                let before = seed_fields
                    .get(namespace)
                    .and_then(|fields| fields.get(field_key));
                let value = serde_json::to_value(field).unwrap_or(Value::Null);
                if before.map(|before| serde_json::to_value(before).unwrap_or(Value::Null))
                    != Some(value.clone())
                {
                    let entry = metafields
                        .entry(namespace.clone())
                        .or_insert_with(|| Value::Object(Map::new()));
                    if let Value::Object(entry) = entry {
                        entry.insert(field_key.clone(), value);
                    }
                }
            }
        }
        for (namespace, fields) in &seed_fields {
            for field_key in fields.keys() {
                if now_fields
                    .get(namespace)
                    .and_then(|fields| fields.get(field_key))
                    .is_none()
                {
                    removals
                        .shop_metafields
                        .push(format!("{namespace}.{field_key}"));
                }
            }
        }
        if !metafields.is_empty() {
            patch.insert("metafields".to_string(), Value::Object(metafields));
        }
        let policies =
            |view: &View<'s>| -> IndexMap<&'static str, (Option<String>, Option<String>)> {
                view.get(key)
                    .map(|shop| list(shop, "shopPolicies").collect::<Vec<_>>())
                    .unwrap_or_default()
                    .into_iter()
                    .filter_map(|policy| view.get(policy))
                    .filter_map(|policy| {
                        let field = policy_field(text(policy, "type")?)?;
                        Some((field, (string(policy, "title"), string(policy, "body"))))
                    })
                    .collect()
            };
        let (now_policies, seed_policies) = (policies(&self.now), policies(&self.seed));
        let mut changed = Map::new();
        for (field, (title, body)) in &now_policies {
            if seed_policies.get(field) != Some(&(title.clone(), body.clone())) {
                changed.insert(
                    field.to_string(),
                    json!({"title": title, "body": body.clone().unwrap_or_default()}),
                );
            }
        }
        if !changed.is_empty() {
            patch.insert("policies".to_string(), Value::Object(changed));
        }
    }

    /// The countries and languages of the data, with the markets as the Admin API has them: those
    /// declared in the data and those it created, with their countries and metafields. A
    /// country's market of its own is not declared: what the Admin API changes of it does not
    /// sync.
    fn localization(&self) -> Option<model::LocalizationInput> {
        let removed = self.removed_of(&["Market"]);
        let markets = |view: &View<'s>| -> Vec<model::MarketInput> {
            view.keys("Market", &removed)
                .into_iter()
                .filter_map(|key| Some((key, view.get(key)?)))
                .filter(|(key, record)| {
                    let handle = text(record, "handle").unwrap_or_default();
                    let declared = *key == content::market_gid(handle)
                        || self
                            .store
                            .markets
                            .iter()
                            .any(|market| content::market_gid(&market.handle) == *key);
                    let enabled = flag(record, "enabled") != Some(false)
                        && text(record, "status").is_none_or(|status| status == "ACTIVE");
                    declared && enabled
                })
                .filter_map(|(key, record)| {
                    Some(model::MarketInput {
                        handle: text(record, "handle")?.to_string(),
                        countries: record
                            .get("regions")
                            .and_then(Value::as_array)
                            .into_iter()
                            .flatten()
                            .filter_map(|region| region.get("code")?.as_str())
                            .map(str::to_string)
                            .collect(),
                        metafields: self.metafields(view, key),
                    })
                })
                .collect()
        };
        let now = markets(&self.now);
        if serde_json::to_value(&now).ok() == serde_json::to_value(markets(&self.seed)).ok() {
            return None;
        }
        let mut countries: Vec<model::CountryInput> = self
            .store
            .countries
            .iter()
            .map(|country| model::CountryInput {
                iso_code: country.iso_code.clone(),
                name: Some(country.name.clone()),
                currency: Some(country.currency.clone()),
                unit_system: Some(country.unit_system.clone()),
                popular: country.popular,
            })
            .collect();
        // A market can only group countries the store sells to: those it adds are sold to.
        for code in now.iter().flat_map(|market| &market.countries) {
            if !countries
                .iter()
                .any(|country| country.iso_code.eq_ignore_ascii_case(code))
            {
                countries.push(model::CountryInput {
                    iso_code: code.clone(),
                    name: None,
                    currency: None,
                    unit_system: None,
                    popular: false,
                });
            }
        }
        Some(model::LocalizationInput {
            countries,
            languages: self
                .store
                .languages
                .iter()
                .map(|language| model::LanguageInput {
                    iso_code: language.iso_code.clone(),
                    name: Some(language.name.clone()),
                    endonym_name: Some(language.endonym_name.clone()),
                })
                .collect(),
            markets: now,
        })
    }

    // ---- the app -------------------------------------------------------------------------

    /// The handle of the Function a record runs, from the `ShopifyFunction` its `functionId` (or
    /// `shopifyFunction`) names.
    fn function_of(&self, view: &View<'s>, key: &str) -> Option<String> {
        let record = view.get(key)?;
        let function = text(record, "functionId").or_else(|| text(record, "shopifyFunction"))?;
        string(self.state.get(function)?, "handle")
    }

    /// The app discounts that run at the checkout: each automatic or code app discount that is
    /// active by the shop's clock, with the id the Admin API gave it, in creation order. The
    /// others (Shopify's own discount types) have nothing to run.
    fn discounts(&self, era: Era) -> Vec<model::CheckoutDiscountInput> {
        let view = self.view(era);
        let now = self.state.timestamp();
        let removed = self.removed_of(&["DiscountAutomaticNode", "DiscountCodeNode"]);
        let mut keys: Vec<&'s str> = view.keys("DiscountAutomaticNode", &removed);
        keys.extend(view.keys("DiscountCodeNode", &removed));
        keys.sort_by_key(|key| parse_gid(key).map_or(0, |(_, id)| id));
        keys.into_iter()
            .filter_map(|key| {
                let record = view.get(key)?;
                let details = record
                    .get("automaticDiscount")
                    .or_else(|| record.get("codeDiscount"))
                    .and_then(Value::as_object)?;
                let kind = details.get("__typename").and_then(Value::as_str)?;
                if kind != "DiscountAutomaticApp" && kind != "DiscountCodeApp" {
                    return None;
                }
                let starts = details.get("startsAt").and_then(Value::as_str);
                let ends = details.get("endsAt").and_then(Value::as_str);
                let active = starts.is_none_or(|starts| starts <= now.as_str())
                    && ends.is_none_or(|ends| ends > now.as_str());
                if !active {
                    return None;
                }
                let number = parse_gid(key).map(|(_, id)| id);
                // The Function of the app it runs; a discount of the data whose Function the app
                // does not have keeps the handle the data gives (the checkout says it is missing).
                let function = details
                    .get("appDiscountType")
                    .and_then(|kind| kind.get("functionId"))
                    .or_else(|| details.get("functionId"))
                    .and_then(Value::as_str)
                    .and_then(|function| self.state.get(function))
                    .and_then(|function| string(function, "handle"))
                    .or_else(|| {
                        let discount = self
                            .store
                            .checkout
                            .discounts
                            .iter()
                            .find(|discount| Some(discount.id) == number)?;
                        Some(discount.function.clone())
                    })?;
                let code = details
                    .get("codes")
                    .and_then(Value::as_array)
                    .and_then(|codes| codes.first())
                    .and_then(|code| code.get("code")?.as_str())
                    .map(str::to_string);
                if kind == "DiscountCodeApp" && code.is_none() {
                    return None;
                }
                let mut classes: Vec<model::CheckoutDiscountClass> = details
                    .get("discountClasses")
                    .and_then(Value::as_array)
                    .into_iter()
                    .flatten()
                    .filter_map(|class| match class.as_str()? {
                        "PRODUCT" => Some(model::CheckoutDiscountClass::Product),
                        "ORDER" => Some(model::CheckoutDiscountClass::Order),
                        "SHIPPING" => Some(model::CheckoutDiscountClass::Shipping),
                        _ => None,
                    })
                    .collect();
                // All three is what the data says by saying none.
                if classes.len() == 3 {
                    classes.clear();
                }
                Some(model::CheckoutDiscountInput {
                    id: number,
                    function,
                    title: details
                        .get("title")
                        .and_then(Value::as_str)
                        .map(str::to_string),
                    code,
                    classes,
                    metafields: self.checkout_metafields(view, key),
                })
            })
            .collect()
    }

    /// The settings of a Function: the metafields of the enabled objects that run it, in the
    /// order they were created, a later one's taking the place of an earlier one's.
    fn function_settings(&self, era: Era, handle: &str) -> model::CheckoutFunctionInput {
        let view = self.view(era);
        let removed = self.removed_of(&FUNCTION_OWNERS);
        let mut metafields = IndexMap::new();
        for type_name in FUNCTION_OWNERS {
            for key in view.keys(type_name, &removed) {
                let Some(record) = view.get(key) else {
                    continue;
                };
                if flag(record, "enabled") == Some(false)
                    || self.function_of(view, key).as_deref() != Some(handle)
                {
                    continue;
                }
                metafields.extend(self.checkout_metafields(view, key));
            }
        }
        model::CheckoutFunctionInput { metafields }
    }

    /// The keys of the records of some types removed since the seed, by type.
    fn removed_of(&self, types: &[&str]) -> HashMap<String, Vec<&'s str>> {
        let mut removed: HashMap<String, Vec<&'s str>> = HashMap::new();
        for change in self.state.changes() {
            if change.kind != ChangeKind::Removed {
                continue;
            }
            if let Some(type_name) = change
                .original
                .and_then(|record| type_of(change.key, record))
                && types.contains(&type_name)
            {
                removed
                    .entry(type_name.to_string())
                    .or_default()
                    .push(change.key);
            }
        }
        removed
    }
}

// ---- reading records ----------------------------------------------------------------------

/// The type of a record: its `__typename`, else the type its GID names.
fn type_of<'r>(key: &'r str, record: &'r Record) -> Option<&'r str> {
    text(record, "__typename").or_else(|| gid_type(key))
}

/// [`text`], for where a variable is named `text`.
fn text_of<'r>(record: &'r Record, field: &str) -> Option<&'r str> {
    text(record, field)
}

/// A string field of a record, when it is a non-empty string.
fn text<'r>(record: &'r Record, field: &str) -> Option<&'r str> {
    record
        .get(field)
        .and_then(Value::as_str)
        .filter(|text| !text.is_empty())
}

fn string(record: &Record, field: &str) -> Option<String> {
    text(record, field).map(str::to_string)
}

fn flag(record: &Record, field: &str) -> Option<bool> {
    record.get(field).and_then(Value::as_bool)
}

/// The strings of a list field of a record: GIDs, tags.
fn list<'r>(record: &'r Record, field: &str) -> impl Iterator<Item = &'r str> + 'r {
    record
        .get(field)
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
}

/// The cents of a `MoneyV2` (`{"amount": "19.5", …}`) or of a `Money` string.
fn money_v2(value: &Value) -> Option<i64> {
    match value {
        Value::Object(money) => catalog::cents_of(money.get("amount")),
        other => catalog::cents_of(Some(other)),
    }
}

/// A text of the store, or nothing when it is empty.
fn some(text: &str) -> Option<String> {
    (!text.is_empty()).then(|| text.to_string())
}

/// Whether a value holds one of `targets` as a string, anywhere in it.
fn mentions(record: &Record, targets: &HashSet<&str>) -> bool {
    fn walk(value: &Value, targets: &HashSet<&str>) -> bool {
        match value {
            Value::String(text) => targets.contains(text.as_str()),
            Value::Array(items) => items.iter().any(|item| walk(item, targets)),
            Value::Object(map) => map.values().any(|item| walk(item, targets)),
            _ => false,
        }
    }
    record.values().any(|value| walk(value, targets))
}

/// The path in `files/` of a URL lsf serves a file at (`https://<domain>/cdn/shop/files/<path>?v=…`).
fn files_path(url: &str) -> Option<String> {
    let (_, rest) = url.split_once("/cdn/shop/files/")?;
    let path = rest.split(['?', '#']).next().unwrap_or(rest);
    (!path.is_empty()).then(|| percent_decode(path))
}

/// The path in `files/` of a file record (`MediaImage`, `Video`, `GenericFile`).
fn file_path(record: &Record) -> Option<String> {
    let url = record
        .get("image")
        .and_then(|image| image.get("url"))
        .or_else(|| {
            record
                .get("originalSource")
                .and_then(|source| source.get("url"))
        })
        .or_else(|| record.get("url"))
        .and_then(Value::as_str)?;
    files_path(url)
}

/// An `Image` of a record as an image of the data: its file in `files/`, its alt text, size and
/// the number of its id.
fn image_input(image: &Value) -> Option<model::ImageDetail> {
    let src = files_path(image.get("url")?.as_str()?)?;
    Some(model::ImageDetail {
        src,
        alt: image
            .get("altText")
            .and_then(Value::as_str)
            .map(str::to_string),
        width: image.get("width").and_then(Value::as_u64).map(|n| n as u32),
        height: image
            .get("height")
            .and_then(Value::as_u64)
            .map(|n| n as u32),
        id: image
            .get("id")
            .and_then(Value::as_str)
            .and_then(parse_gid)
            .map(|(_, id)| id),
        focal_point: None,
    })
}

/// A `MailingAddress` of a customer as an address of the data, with the id of its GID.
fn mailing_address(address: &Value) -> model::AddressInput {
    let field = |name: &str| {
        address
            .get(name)
            .and_then(Value::as_str)
            .filter(|text| !text.is_empty())
            .map(str::to_string)
    };
    model::AddressInput {
        id: field("id").as_deref().and_then(parse_gid).map(|(_, id)| id),
        first_name: field("firstName"),
        last_name: field("lastName"),
        company: field("company"),
        address1: field("address1"),
        address2: field("address2"),
        city: field("city"),
        province: field("province"),
        province_code: field("provinceCode"),
        country: field("country"),
        country_code: field("countryCodeV2").or_else(|| field("countryCode")),
        zip: field("zip"),
        phone: field("phone"),
    }
}

/// The id of an external video, from its URL on its host.
fn external_video_id(url: &str) -> String {
    let url = url.split('#').next().unwrap_or(url);
    if let Some((_, query)) = url.split_once('?')
        && let Some(id) = query.split('&').find_map(|pair| pair.strip_prefix("v="))
    {
        return id.to_string();
    }
    let path = url.split('?').next().unwrap_or(url);
    path.trim_end_matches('/')
        .rsplit('/')
        .next()
        .unwrap_or_default()
        .to_string()
}

/// A price adjustment of a selling plan's pricing policy.
fn adjustment(policy: &Value) -> Option<(model::AdjustmentType, Money)> {
    let value = policy.get("adjustmentValue")?;
    Some(match policy.get("adjustmentType")?.as_str()? {
        "PERCENTAGE" => (
            model::AdjustmentType::Percentage,
            Money::Cents(value.get("percentage")?.as_f64()?.round() as i64),
        ),
        "FIXED_AMOUNT" => (
            model::AdjustmentType::FixedAmount,
            Money::Cents(money_v2(value)?),
        ),
        "PRICE" => (model::AdjustmentType::Price, Money::Cents(money_v2(value)?)),
        _ => return None,
    })
}

/// The handle of a shop policy, from its `ShopPolicyType`.
fn policy_handle(kind: &str) -> Option<&'static str> {
    Some(match kind {
        "PRIVACY_POLICY" => "privacy-policy",
        "REFUND_POLICY" => "refund-policy",
        "SHIPPING_POLICY" => "shipping-policy",
        "TERMS_OF_SERVICE" => "terms-of-service",
        "SUBSCRIPTION_POLICY" => "subscription-policy",
        _ => return None,
    })
}

/// The field of `shop.policies` of the data a `ShopPolicyType` is.
fn policy_field(kind: &str) -> Option<&'static str> {
    Some(match kind {
        "PRIVACY_POLICY" => "privacy_policy",
        "REFUND_POLICY" => "refund_policy",
        "SHIPPING_POLICY" => "shipping_policy",
        "TERMS_OF_SERVICE" => "terms_of_service",
        "SUBSCRIPTION_POLICY" => "subscription_policy",
        _ => return None,
    })
}

/// How Shopify words a `DeliveryLocalPickupTime`, which the data keeps as words.
fn pickup_words(time: &str) -> &'static str {
    match time {
        "ONE_HOUR" => "Usually ready in 1 hour",
        "TWO_HOURS" => "Usually ready in 2 hours",
        "FOUR_HOURS" => "Usually ready in 4 hours",
        "TWO_TO_FOUR_DAYS" => "Usually ready in 2-4 days",
        "FIVE_OR_MORE_DAYS" => "Usually ready in 5 or more days",
        _ => "Usually ready in 24 hours",
    }
}

/// A URL of a record as a menu of the data writes it: a path when it is on the shop's domain.
fn strip_origin(url: &str, origin: Option<&str>) -> String {
    match origin.and_then(|origin| url.strip_prefix(origin)) {
        Some("") => "/".to_string(),
        Some(path) if path.starts_with('/') => path.to_string(),
        _ => url.to_string(),
    }
}

/// `%XX` escapes decoded.
fn percent_decode(text: &str) -> String {
    let bytes = text.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b'%'
            && let Some(byte) = text
                .get(index + 1..index + 3)
                .and_then(|hex| u8::from_str_radix(hex, 16).ok())
        {
            out.push(byte);
            index += 3;
        } else {
            out.push(bytes[index]);
            index += 1;
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// Whether a top-level section of the projected data says nothing: it is left out.
fn is_empty_section(value: &Value) -> bool {
    match value {
        Value::Null => true,
        Value::Array(items) => items.is_empty(),
        Value::Object(map) => map.is_empty(),
        _ => false,
    }
}

/// Drops the `null` fields of objects, which the data reads as absent: the overlay is easier to
/// read without them. (No value of the projected data is a meaningful `null`.)
fn strip_nulls(value: &mut Value) {
    match value {
        Value::Object(map) => {
            map.retain(|_, item| !item.is_null());
            map.values_mut().for_each(strip_nulls);
        }
        Value::Array(items) => items.iter_mut().for_each(strip_nulls),
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use lsf_core::store::load::{DataSource, LoadOptions, load_with_overlays};

    /// Every entity of a store, projected from the records the seed made of it: the store data
    /// of the whole store.
    fn everything(state: &ShopState, store: &Store) -> Value {
        let sync = Sync::new(state, store, &[]);
        let mut input = model::StoreInput::default();
        for type_name in ROOTS {
            for key in state.keys(type_name) {
                if let Some(entity) = sync.entity(Era::Now, type_name, key) {
                    entity.add(&mut input);
                }
            }
        }
        let mut functions = IndexMap::new();
        for type_name in FUNCTION_OWNERS {
            for key in state.keys(type_name) {
                if let Some(handle) = sync.function_of(&sync.now, key) {
                    let settings = sync.function_settings(Era::Now, &handle);
                    if !settings.metafields.is_empty() {
                        functions.insert(handle, settings);
                    }
                }
            }
        }
        input.checkout = Some(model::CheckoutInput {
            discounts: Some(sync.discounts(Era::Now)),
            functions,
            ..model::CheckoutInput::default()
        });
        let mut data = serde_json::to_value(&input).unwrap_or(Value::Null);
        strip_nulls(&mut data);
        data
    }

    /// What is compared of two stores: everything the data says, in the order it is.
    fn summary(store: &Store) -> Vec<String> {
        vec![
            format!("{:#?}", store.products),
            format!("{:#?}", store.unlisted_products),
            format!("{:#?}", store.collections),
            format!("{:#?}", store.pages),
            format!("{:#?}", store.blogs),
            format!("{:#?}", store.menus),
            format!("{:#?}", store.customers),
            format!("{:#?}", store.companies),
            format!("{:#?}", store.gift_cards),
            format!("{:#?}", store.locations),
            format!("{:#?}", store.selling_plan_groups),
            format!("{:#?}", store.metaobjects),
            format!("{:#?}", store.markets),
            format!("{:#?}", store.checkout),
        ]
    }

    #[test]
    fn the_whole_store_projected_from_the_seed_is_the_store() {
        let fixtures = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures");
        let app = lsf_checkout::app::load(&fixtures.join("app")).expect("the fixture app");
        for (source, app) in [
            (DataSource::Directory(fixtures.join("rich")), Some(&app)),
            (DataSource::Demo, None),
        ] {
            let (store, diagnostics) = load_with_overlays(&source, &LoadOptions::default(), &[]);
            assert!(!diagnostics.has_errors(), "{diagnostics}");
            let state = ShopState::seeded(&store, app);
            let overlay = Overlay::new(OVERLAY_NAME, everything(&state, &store));
            let (again, diagnostics) = load_with_overlays(
                &source,
                &LoadOptions::default(),
                std::slice::from_ref(&overlay),
            );
            let own: Vec<String> = diagnostics
                .items
                .iter()
                .filter(|item| {
                    item.file == OVERLAY_NAME
                        || item.severity == lsf_core::diagnostics::Severity::Error
                })
                .map(ToString::to_string)
                .collect();
            assert!(own.is_empty(), "{own:#?}");
            for (before, after) in summary(&store).iter().zip(summary(&again)) {
                if *before != after {
                    let line = before
                        .lines()
                        .zip(after.lines())
                        .position(|(a, b)| a != b)
                        .unwrap_or(0);
                    let context = |text: &str| {
                        text.lines()
                            .skip(line.saturating_sub(12))
                            .take(16)
                            .collect::<Vec<_>>()
                            .join("\n")
                    };
                    panic!(
                        "{source:?}: differs at line {line}\n--- before\n{}\n--- after\n{}",
                        context(before),
                        context(&after)
                    );
                }
            }
        }
    }

    #[test]
    fn reads_the_paths_of_files() {
        assert_eq!(
            files_path("https://shop.example/cdn/shop/files/products/a%20b.jpg?v=12").as_deref(),
            Some("products/a b.jpg")
        );
        assert_eq!(files_path("https://cdn.example.com/a.jpg"), None);
        assert_eq!(
            external_video_id("https://www.youtube.com/watch?v=dQw4w9WgXcQ"),
            "dQw4w9WgXcQ"
        );
        assert_eq!(external_video_id("https://vimeo.com/76979871"), "76979871");
        assert_eq!(
            strip_origin("https://a.example/collections/x", Some("https://a.example")),
            "/collections/x"
        );
        assert_eq!(
            strip_origin("https://a.example", Some("https://a.example")),
            "/"
        );
        assert_eq!(
            strip_origin("https://other.example/x", Some("https://a.example")),
            "https://other.example/x"
        );
    }

    #[test]
    fn strips_nulls_and_empty_sections() {
        let mut value = json!({"a": null, "b": [{"c": null, "d": 1}], "e": {"f": null}});
        strip_nulls(&mut value);
        assert_eq!(value, json!({"b": [{"d": 1}], "e": {}}));
        assert!(is_empty_section(&json!([])));
        assert!(!is_empty_section(&json!(false)));
    }
}
