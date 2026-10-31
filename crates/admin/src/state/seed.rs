//! The seed of a shop: the records the store data and the app give, made once when the server
//! starts (and again when the data changes). Each family of resources seeds its own records, in
//! the module of `domains` that owns it; this module runs them in order and holds what they
//! share.

use chrono::{DateTime, TimeZone, Utc};
use indexmap::IndexSet;
use lsf_checkout::app::App;
use lsf_core::Store;
use lsf_core::store::{CheckoutMetafield, Image, Metafields};
use serde_json::Value;

use super::{Clock, Record, ShopState, value};
use crate::domains::{app, catalog, content, customers, logistics, markets, orders, subscriptions};

/// The records of the store data and of the app. The same data gives the same records, ids and
/// dates every time (dates the data does not give are fixed ones, never the clock).
pub fn seed(store: &Store, app: Option<&App>) -> ShopState {
    let mut seed = Seed::new(store, app);
    content::seed_shop(&mut seed);
    catalog::seed(&mut seed);
    customers::seed(&mut seed);
    orders::seed(&mut seed);
    subscriptions::seed(&mut seed);
    logistics::seed(&mut seed);
    content::seed(&mut seed);
    app::seed(&mut seed);
    markets::seed(&mut seed);
    content::seed_files(&mut seed);
    content::infer_metafield_definitions(&mut seed.state);
    // From here on, a change is one the Admin API made: what the store sync projects.
    seed.state.track_changes();
    seed.state
}

/// What the seed of every family reads and writes.
pub struct Seed<'a> {
    pub store: &'a Store,
    pub app: Option<&'a App>,
    /// The shop being seeded.
    pub state: ShopState,
    /// The currency of the shop: every amount of the store data is in it.
    pub currency: String,
    /// `https://<primary domain>`: the origin of the URLs records hold (product pages, files).
    pub origin: String,
    /// The numeric id of the app (`gid://shopify/App/<id>`), which the reserved namespace of its
    /// metafields names: `$app:config` is `app--<id>--config`.
    pub app_id: u64,
    /// The paths of `files/` something refers to (a file reference, a swatch image): each gets a
    /// file record, with the files the data describes.
    pub files: IndexSet<String>,
}

impl<'a> Seed<'a> {
    pub fn new(store: &'a Store, app: Option<&'a App>) -> Seed<'a> {
        let domain = store.shop.permanent_domain.to_ascii_lowercase();
        let host = store
            .shop
            .domain
            .clone()
            .filter(|domain| !domain.is_empty())
            .unwrap_or_else(|| domain.clone());
        Seed {
            store,
            app,
            state: ShopState::new(domain, Clock::of(store)),
            currency: store.shop.currency.to_ascii_uppercase(),
            origin: format!("https://{host}"),
            app_id: app::app_id(app),
            files: IndexSet::new(),
        }
    }

    /// The date of what the data gives no date for: noon on 2024-01-01, the date lsf-core
    /// gives the undated entities of the store data. Fixed, so that the records are the same
    /// every run.
    pub fn epoch() -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2024, 1, 1, 12, 0, 0)
            .single()
            .unwrap_or_default()
    }

    /// [`Seed::epoch`] as a `DateTime`.
    pub fn epoch_string() -> String {
        value::date_time(Self::epoch())
    }

    /// The GID of the file record of a path of `files/`, which the seed then creates.
    pub fn file(&mut self, path: &str) -> String {
        self.files.insert(path.to_string());
        catalog::file_gid(path)
    }

    pub fn insert(&mut self, record: Value) -> String {
        self.state.insert(value::object(record))
    }

    pub fn insert_record(&mut self, record: Record) -> String {
        self.state.insert(record)
    }

    /// A `MoneyV2` in the shop currency.
    pub fn money_v2(&self, cents: i64) -> Value {
        value::money_v2(cents, &self.currency)
    }

    /// A `MoneyBag` in the shop currency.
    pub fn money_bag(&self, cents: i64) -> Value {
        value::money_bag(cents, &self.currency)
    }

    /// A URL on the shop's primary domain: `/products/hat` → `https://shop.example/products/hat`.
    pub fn url(&self, path: &str) -> String {
        format!("{}{path}", self.origin)
    }

    /// The URL of a file of `files/`.
    pub fn file_url(&self, src: &str) -> String {
        value::file_url(&self.origin, src)
    }

    /// An `Image` of the store data, its id a GID of `id_type`.
    pub fn image(&self, image: &Image, id_type: &str) -> Value {
        value::image(&self.origin, image, id_type)
    }

    /// Seeds the metafields of an owner and answers their GIDs, for its `metafields`.
    pub fn metafields(
        &mut self,
        owner: &str,
        owner_type: &str,
        created_at: &str,
        metafields: &Metafields,
    ) -> Value {
        content::seed_metafields(self, owner, owner_type, created_at, metafields)
    }

    /// Seeds the metafields of a checkout setting (a discount, a Function) and answers their
    /// GIDs.
    pub fn checkout_metafields(
        &mut self,
        owner: &str,
        owner_type: &str,
        created_at: &str,
        metafields: &[CheckoutMetafield],
    ) -> Value {
        content::seed_checkout_metafields(self, owner, owner_type, created_at, metafields)
    }
}
