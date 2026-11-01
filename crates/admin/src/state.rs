//! The state of a shop: its records, seeded from the store data, and the events of what changed.
//!
//! A record is a JSON object in the shape of its GraphQL type, keyed by its GID. [`ShopState`]
//! holds the records of one shop, its clock and its pending events; [`Shops`] holds every shop of
//! the server, by domain, each seeded from the same store data. What a record of each type holds
//! and how its id is made: the "Records" section of `crates/admin/docs/design.md`.

pub mod seed;
pub mod value;

use std::collections::HashMap;

use chrono::{DateTime, NaiveDate, SubsecRound, Utc};
use chrono_tz::Tz;
use indexmap::{IndexMap, IndexSet};
use lsf_checkout::app::App;
use lsf_core::Store;
use serde_json::{Map, Value};

/// A record: a JSON object in the shape of its GraphQL type, with its GID in `id` and its type in
/// `__typename`.
pub type Record = Map<String, Value>;

/// The GID of an entity: `gid://shopify/Product/1`.
pub fn gid(type_name: &str, id: u64) -> String {
    format!("gid://shopify/{type_name}/{id}")
}

/// The type and the numeric id of a GID, when it is one of the form `gid://shopify/<Type>/<id>`
/// (a query string after the id, as some GIDs have, is ignored).
pub fn parse_gid(gid: &str) -> Option<(&str, u64)> {
    let rest = gid.strip_prefix("gid://shopify/")?;
    let (type_name, id) = rest.split_once('/')?;
    let id = id.split_once('?').map_or(id, |(id, _)| id);
    Some((type_name, id.parse().ok()?))
}

/// The number of a GID (`gid://shopify/Order/7` is 7), 0 when it is not one.
pub fn gid_number(gid: &str) -> u64 {
    parse_gid(gid).map_or(0, |(_, number)| number)
}

/// The type a GID names, whatever its id looks like: `Product` for `gid://shopify/Product/1`.
pub fn gid_type(gid: &str) -> Option<&str> {
    let rest = gid.strip_prefix("gid://shopify/")?;
    let (type_name, _) = rest.split_once('/')?;
    (!type_name.is_empty()).then_some(type_name)
}

/// Something that changed in a shop, which webhooks report.
#[derive(Clone, Debug, PartialEq)]
pub struct Event {
    /// The webhook topic, as Shopify writes it in `X-Shopify-Topic`: `products/update`.
    pub topic: String,
    /// The shop it happened in: `my-shop.myshopify.com`.
    pub shop: String,
    /// The record it is about.
    pub gid: String,
    /// When it happened, by the clock of the shop.
    pub at: DateTime<Utc>,
    /// A unique id, `X-Shopify-Event-Id`: random, so that a backend that remembers the events it
    /// processed (across server runs, or a reseed) never takes a new one for a duplicate.
    pub id: String,
    /// The record as it was when the event happened: what a `…/delete` topic reports, since the
    /// record is gone by the time the webhook is built. For another topic, the record before the
    /// change, when the payload says what changed ([`ShopState::emit_with`]).
    pub snapshot: Option<Record>,
}

/// The clock of a shop.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Clock {
    /// The `now` of the store data: every change happens at that instant, so a test reads the
    /// same dates every run.
    Frozen(DateTime<Utc>),
    /// The time of the machine, to the second (Shopify's `DateTime`s have no fraction).
    System,
}

impl Clock {
    /// The clock the store data asks for.
    pub fn of(store: &Store) -> Clock {
        store.now.map_or(Clock::System, Clock::Frozen)
    }

    pub fn now(self) -> DateTime<Utc> {
        match self {
            Clock::Frozen(now) => now.trunc_subsecs(0),
            Clock::System => Utc::now().trunc_subsecs(0),
        }
    }
}

/// A record and its type, as the state keeps it.
#[derive(Clone, Debug, PartialEq)]
struct Entry {
    type_name: String,
    record: Record,
}

/// What changed in a record since the seed: see [`ShopState::changes`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ChangeKind {
    /// Created since the seed.
    Inserted,
    /// Seeded, and no longer the same.
    Changed,
    /// Seeded, and gone.
    Removed,
}

/// A record that changed since the seed: its key, how it changed, and what it was in the seed
/// (`None` for a record created since).
#[derive(Clone, Copy, Debug)]
pub struct Change<'s> {
    pub key: &'s str,
    pub kind: ChangeKind,
    pub original: Option<&'s Record>,
}

/// The records of one shop, its clock and the events of what changed.
///
/// Records are kept in creation order: the order of the store data for the seeded ones, then the
/// order the Admin API created the others in. That is the order [`ShopState::all`] lists, and the
/// order a connection shows before a `sortKey` applies; the root connections of Shopify sort by
/// `ID` by default (`locations` and `markets` by `NAME`, `orders` by `PROCESSED_AT`), which the
/// resolver applies on top. There is no locking here: the `api` layer keeps a shop behind a
/// mutex.
#[derive(Clone, Debug)]
pub struct ShopState {
    domain: String,
    records: IndexMap<String, Entry>,
    /// The keys of the records of each type, in creation order.
    by_type: HashMap<String, IndexSet<String>>,
    /// The next numeric id of each group of types (see [`counter_group`]).
    counters: HashMap<String, u64>,
    clock: Clock,
    events: Vec<Event>,
    /// How many events the shop emitted, ever (a reset does not restart it). Event ids are random
    /// (`Event::id`), not made from it.
    emitted: u64,
    /// The records as they were before their first change since the seed, by key (`None` for a
    /// record that did not exist), once [`ShopState::track_changes`] turned tracking on: what
    /// the store sync projects, and what it compares with to drop the changes that were undone.
    originals: Option<IndexMap<String, Option<Record>>>,
    /// The files the shop keeps beside its records, by key: the bytes of a staged upload, the
    /// result of a bulk operation. Not records: no GraphQL type, no event, no sync.
    blobs: HashMap<String, Blob>,
    /// How many times a record was written (added, removed, or handed out to change), ever:
    /// what tells whether a request changed the shop, without comparing records. A reset does
    /// not restart it.
    revision: u64,
    /// What a mutation keeps for the requests that follow and that is not a record, by key: the
    /// idempotency keys of the `@idempotent` mutations and what they answered. A failed mutation
    /// puts them back with the shop; a reset forgets them.
    memos: HashMap<String, Value>,
    /// What the store data says of the shop that no record holds, by name: the checkout's tax
    /// rate (`taxRateBps`), which draft orders compute their taxes with. Set by the seed, kept
    /// by a reset.
    settings: Map<String, Value>,
    /// What undoes the writes made since the open savepoints, while there are some
    /// ([`ShopState::savepoint`]).
    journal: Option<Journal>,
}

/// The undo log of a shop: each write made since the oldest open savepoint, with what it
/// overwrote, so that a failed mutation puts back what it touched and nothing else. Copying the
/// shop instead costs what the whole shop costs (400 ms for 198,000 records); this costs what
/// the mutation wrote.
#[derive(Clone, Debug, Default)]
struct Journal {
    /// What undoes each write, in the order of the writes.
    steps: Vec<Undo>,
    /// The step that keeps the value of a record before an in-place write, by key: a record
    /// written again within the same savepoint needs no second copy.
    saved: HashMap<String, usize>,
    /// Where each open savepoint starts in `steps`, the innermost last (a bulk operation runs
    /// a mutation per line within its own).
    starts: Vec<usize>,
}

/// What undoes one write ([`Journal`]).
#[derive(Clone, Debug)]
enum Undo {
    /// A record was changed in place, or replaced by one of the same type: its entry before.
    Changed {
        key: String,
        before: Entry,
    },
    /// A record was added, at the end: remove it.
    Added {
        key: String,
    },
    /// A record was replaced by one of another type: its entry before, and its position among
    /// the keys of that type.
    Retyped {
        key: String,
        before: Entry,
        position: usize,
    },
    /// A record was removed: it, and its positions among the records and among those of its
    /// type.
    Removed {
        key: String,
        entry: Entry,
        index: usize,
        position: Option<usize>,
    },
    /// A counter moved: its value before.
    Counter {
        group: String,
        before: Option<u64>,
    },
    /// A side entry was set: its value before.
    Blob {
        key: String,
        before: Option<Blob>,
    },
    Memo {
        key: String,
        before: Option<Value>,
    },
    Setting {
        key: String,
        before: Option<Value>,
    },
}

/// Where a mutation started writing ([`ShopState::savepoint`]): what [`ShopState::rollback`]
/// goes back to, besides the records the journal keeps.
#[derive(Clone, Copy, Debug)]
#[must_use = "a savepoint is closed by `rollback` or `release`"]
pub struct Savepoint {
    steps: usize,
    /// How many savepoints were open before this one: what a rollback after a panic closes
    /// the inner ones it left open down to.
    depth: usize,
    events: usize,
    emitted: u64,
    revision: u64,
    originals: Option<usize>,
    clock: Clock,
}

/// A file a shop keeps beside its records ([`ShopState::put_blob`]): what a client uploaded to a
/// staged upload target, or what a bulk operation wrote. Cloning it is cheap, so that copying a
/// shop (another shop made from the seed, the undo log of a mutation) does not copy the bytes.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Blob {
    pub bytes: std::sync::Arc<[u8]>,
    /// The MIME type it was uploaded or written with: `text/jsonl`, `image/png`.
    pub content_type: String,
}

impl Blob {
    pub fn new(bytes: impl Into<Vec<u8>>, content_type: impl Into<String>) -> Blob {
        Blob {
            bytes: std::sync::Arc::from(bytes.into()),
            content_type: content_type.into(),
        }
    }
}

impl ShopState {
    /// An empty shop.
    pub fn new(domain: impl Into<String>, clock: Clock) -> ShopState {
        ShopState {
            domain: domain.into(),
            records: IndexMap::new(),
            by_type: HashMap::new(),
            counters: HashMap::new(),
            clock,
            events: Vec::new(),
            emitted: 0,
            originals: None,
            blobs: HashMap::new(),
            revision: 0,
            memos: HashMap::new(),
            settings: Map::new(),
            journal: None,
        }
    }

    /// Keeps a file beside the records, under `key` (replacing the one there). A reset forgets
    /// it, as it forgets what the Admin API changed.
    pub fn put_blob(&mut self, key: impl Into<String>, blob: Blob) {
        let key = key.into();
        let before = self.blobs.insert(key.clone(), blob);
        self.log(|| Undo::Blob { key, before });
    }

    /// The file kept under `key`.
    pub fn blob(&self, key: &str) -> Option<&Blob> {
        self.blobs.get(key)
    }

    /// A shop seeded from the store data and the app: see [`seed::seed`].
    pub fn seeded(store: &Store, app: Option<&App>) -> ShopState {
        seed::seed(store, app)
    }

    /// Changes what the seed holds from outside of it (what the server knows and the store data
    /// does not: the directory of the local theme). What `amend` writes is not a change of the
    /// Admin API: store sync does not see it.
    pub fn amend_seed(&mut self, amend: impl FnOnce(&mut ShopState)) {
        let originals = self.originals.take();
        amend(self);
        self.originals = originals;
    }

    /// A setting of the shop the seed made: see the `settings` field.
    pub fn setting(&self, name: &str) -> Option<&Value> {
        self.settings.get(name)
    }

    /// Sets a setting of the shop (the seed does).
    pub fn set_setting(&mut self, name: &str, value: Value) {
        let before = self.settings.insert(name.to_string(), value);
        self.log(|| Undo::Setting {
            key: name.to_string(),
            before,
        });
    }

    /// The `*.myshopify.com` domain of the shop.
    pub fn domain(&self) -> &str {
        &self.domain
    }

    /// The GID of the `Shop` record.
    pub fn shop_gid(&self) -> Option<&str> {
        self.keys("Shop").next()
    }

    /// The `Shop` record.
    pub fn shop(&self) -> Option<&Record> {
        self.shop_gid().and_then(|gid| self.get(gid))
    }

    pub fn clock(&self) -> Clock {
        self.clock
    }

    pub fn set_clock(&mut self, clock: Clock) {
        self.clock = clock;
    }

    /// The time of the shop: the store data's `now` when it has one, the system clock otherwise.
    pub fn now(&self) -> DateTime<Utc> {
        self.clock.now()
    }

    /// [`ShopState::now`] as a GraphQL `DateTime`: `2026-01-01T00:00:00Z`.
    pub fn timestamp(&self) -> String {
        value::date_time(self.now())
    }

    /// The shop's time zone, `Shop.ianaTimezone`: `None` when it names none chrono-tz knows.
    pub fn iana_time_zone(&self) -> Option<Tz> {
        self.shop()
            .and_then(|shop| shop.get("ianaTimezone"))
            .and_then(Value::as_str)
            .and_then(|zone| zone.parse::<Tz>().ok())
    }

    /// The zone of the shop's days: [`ShopState::iana_time_zone`], UTC without one.
    pub fn time_zone(&self) -> Tz {
        self.iana_time_zone().unwrap_or(Tz::UTC)
    }

    /// The shop's day of an instant, in [`ShopState::time_zone`].
    pub fn day_of(&self, at: DateTime<Utc>) -> NaiveDate {
        at.with_timezone(&self.time_zone()).date_naive()
    }

    /// The shop's date today: [`ShopState::day_of`] its clock.
    pub fn today(&self) -> NaiveDate {
        self.day_of(self.now())
    }

    /// The record of a GID (or of the key a record without a GID is kept under, see
    /// [`ShopState::insert_as`]).
    pub fn get(&self, gid: &str) -> Option<&Record> {
        self.records.get(gid).map(|entry| &entry.record)
    }

    /// The record of a GID, to change. With tracking on, the record counts as changed from now
    /// on (it is kept as it was, and compared with when the changes are read).
    pub fn get_mut(&mut self, gid: &str) -> Option<&mut Record> {
        self.note(gid);
        self.save_entry(gid);
        self.records.get_mut(gid).map(|entry| &mut entry.record)
    }

    pub fn contains(&self, gid: &str) -> bool {
        self.records.contains_key(gid)
    }

    /// The type of the record of a GID: its `__typename`.
    pub fn type_of(&self, gid: &str) -> Option<&str> {
        self.records.get(gid).map(|entry| entry.type_name.as_str())
    }

    /// The record of a GID when it is one of `type_name` ([`ShopState::type_of`]).
    pub fn get_of(&self, gid: &str, type_name: &str) -> Option<&Record> {
        self.records
            .get(gid)
            .filter(|entry| entry.type_name == type_name)
            .map(|entry| &entry.record)
    }

    /// Whether the record of a GID is one of `type_name` ([`ShopState::type_of`]).
    pub fn is_of(&self, gid: &str, type_name: &str) -> bool {
        self.type_of(gid) == Some(type_name)
    }

    /// Adds a record, or replaces the record of the same GID where it was. The key is the
    /// record's `id`; a record without one gets a new GID of its `__typename`
    /// ([`ShopState::new_gid`]) as its `id`. The type is `__typename`, else the type of the GID.
    /// Returns the GID.
    pub fn insert(&mut self, mut record: Record) -> String {
        let type_name = record_type(&record);
        let key = match record.get("id").and_then(Value::as_str) {
            Some(id) => id.to_string(),
            None => {
                let id = self.new_gid(&type_name);
                record.insert("id".to_string(), Value::String(id.clone()));
                id
            }
        };
        self.insert_as(key, record)
    }

    /// Adds a record under `key`, for the records whose `id` is not a GID or that have none
    /// (`ShopifyFunction`, whose id is a UUID; `ShopLocale`, kept under its locale).
    pub fn insert_as(&mut self, key: impl Into<String>, record: Record) -> String {
        let key = key.into();
        self.note(&key);
        let type_name = record_type(&record).to_string();
        if let Some((gid_type, id)) = parse_gid(&key) {
            let group = counter_group(gid_type);
            let before = self.counters.get(&group).copied();
            let next = before.unwrap_or(1).max(id.saturating_add(1));
            if before != Some(next) {
                self.counters.insert(group.clone(), next);
                self.log(|| Undo::Counter { group, before });
            }
        }
        let previous_type = self.records.get(&key).map(|entry| entry.type_name.clone());
        match previous_type {
            None => self.log(|| Undo::Added { key: key.clone() }),
            Some(previous) if previous == type_name => self.save_entry(&key),
            Some(previous) => {
                let position = self
                    .by_type
                    .get_mut(&previous)
                    .and_then(|keys| keys.shift_remove_full(&key))
                    .map_or(0, |(position, _)| position);
                if let Some(journal) = &mut self.journal
                    && let Some(before) = self.records.get(&key)
                {
                    journal.steps.push(Undo::Retyped {
                        key: key.clone(),
                        before: before.clone(),
                        position,
                    });
                }
            }
        }
        self.by_type
            .entry(type_name.clone())
            .or_default()
            .insert(key.clone());
        self.records
            .insert(key.clone(), Entry { type_name, record });
        key
    }

    /// Removes a record. What refers to it is left as it is: a reference to a record that is
    /// gone resolves to `null`.
    pub fn remove(&mut self, gid: &str) -> Option<Record> {
        if self.records.contains_key(gid) {
            self.note(gid);
        }
        let (index, key, entry) = self.records.shift_remove_full(gid)?;
        let position = self
            .by_type
            .get_mut(&entry.type_name)
            .and_then(|keys| keys.shift_remove_full(gid))
            .map(|(position, _)| position);
        if let Some(journal) = &mut self.journal {
            journal.steps.push(Undo::Removed {
                key,
                entry: entry.clone(),
                index,
                position,
            });
        }
        Some(entry.record)
    }

    /// Emits `topic` for the record, then removes it: the event keeps the record as it was.
    pub fn delete(&mut self, gid: &str, topic: &str) -> Option<Record> {
        if !self.contains(gid) {
            return None;
        }
        self.emit(topic, gid);
        self.remove(gid)
    }

    /// The GIDs of the records of a type, in creation order.
    pub fn keys<'s>(&'s self, type_name: &str) -> impl Iterator<Item = &'s str> + 's {
        self.by_type
            .get(type_name)
            .into_iter()
            .flat_map(|keys| keys.iter().map(String::as_str))
    }

    /// The records of a type, in creation order.
    pub fn all<'s>(&'s self, type_name: &str) -> impl Iterator<Item = &'s Record> + 's {
        self.keys(type_name).filter_map(|key| self.get(key))
    }

    /// The records of any of `types`, in creation order across them: the members of an interface
    /// or a union (`files`: `MediaImage`, `Video`, `GenericFile`, …).
    pub fn all_of<'s>(&'s self, types: &'s [&str]) -> impl Iterator<Item = &'s Record> + 's {
        self.records
            .values()
            .filter(|entry| types.contains(&entry.type_name.as_str()))
            .map(|entry| &entry.record)
    }

    /// The number of records of a type.
    pub fn count(&self, type_name: &str) -> usize {
        self.by_type.get(type_name).map_or(0, IndexSet::len)
    }

    /// The number of records of every type.
    pub fn len(&self) -> usize {
        self.records.len()
    }

    pub fn is_empty(&self) -> bool {
        self.records.is_empty()
    }

    /// Every record, in creation order, with its key.
    pub fn iter(&self) -> impl Iterator<Item = (&str, &Record)> {
        self.records
            .iter()
            .map(|(key, entry)| (key.as_str(), &entry.record))
    }

    /// The types that have records.
    pub fn types(&self) -> impl Iterator<Item = &str> {
        self.by_type
            .iter()
            .filter(|(_, keys)| !keys.is_empty())
            .map(|(type_name, _)| type_name.as_str())
    }

    /// A new GID for a record of `type_name`, from a counter that starts above every id of the
    /// type the state holds. The discount nodes (`DiscountAutomaticNode`, `DiscountCodeNode`,
    /// `DiscountNode`) share a counter, as they share their ids on Shopify.
    pub fn new_gid(&mut self, type_name: &str) -> String {
        let group = counter_group(type_name);
        let before = self.counters.get(&group).copied();
        let id = before.unwrap_or(1);
        self.counters.insert(group.clone(), id + 1);
        self.log(|| Undo::Counter { group, before });
        gid(type_name, id)
    }

    /// Sets `updatedAt` of a record to now, when the record has that field. Returns whether the
    /// record exists.
    pub fn touch(&mut self, gid: &str) -> bool {
        let now = self.timestamp();
        match self.get_mut(gid) {
            Some(record) => {
                if record.contains_key("updatedAt") {
                    record.insert("updatedAt".to_string(), Value::String(now));
                }
                true
            }
            None => false,
        }
    }

    /// Records that `topic` happened to a record. A `…/delete` event (and
    /// `inventory_levels/disconnect`, whose level goes too) keeps a copy of the record: emit it
    /// before removing the record (or use [`ShopState::delete`]).
    pub fn emit(&mut self, topic: &str, gid: &str) -> &Event {
        let snapshot = (topic.ends_with("/delete") || topic == "inventory_levels/disconnect")
            .then(|| self.get(gid).cloned())
            .flatten();
        self.emitted += 1;
        let event = Event {
            topic: topic.to_string(),
            shop: self.domain.clone(),
            gid: gid.to_string(),
            at: self.now(),
            id: crate::webhooks::sign::new_id(),
            snapshot,
        };
        self.events.push(event);
        &self.events[self.events.len() - 1]
    }

    /// Records that `topic` happened to a record, with the record the event keeps given rather
    /// than copied: the record before the change, for a topic whose payload says what changed
    /// (`customer.tags_added` names the tags the record did not have), or a record about to go
    /// with what its payload embeds (a role assignment revoked, a contact deleted with its
    /// company inline), or something that is not a record of its own (a redeem code of a
    /// discount, whose `gid` is its id; a product on a publication, for
    /// `product_publications/create`, whose snapshot holds both). See `Event::snapshot`.
    pub fn emit_with(&mut self, topic: &str, gid: &str, snapshot: Record) -> &Event {
        self.emit(topic, gid);
        let last = self.events.len() - 1;
        self.events[last].snapshot = Some(snapshot);
        &self.events[last]
    }

    /// The events not taken yet, in the order they happened.
    pub fn events(&self) -> &[Event] {
        &self.events
    }

    /// Takes the events emitted since the last call.
    pub fn take_events(&mut self) -> Vec<Event> {
        std::mem::take(&mut self.events)
    }

    /// How many events are pending: where the events of what runs next start.
    pub fn event_count(&self) -> usize {
        self.events.len()
    }

    /// Merges the events emitted since `start` that repeat a topic and a record into the first
    /// of them, which keeps its place: one mutation sends one webhook per topic and record (an
    /// order that commits then ships units of one level, a product and its variant given
    /// metafields at once), as Shopify does. Its payload is built from the record as the
    /// mutation left it; the snapshot kept is the first's (the record before the changes), or,
    /// when the key names no record, the last's (the snapshot is then the state reported).
    /// The first of each topic and record is found through a map, so that a mutation that emits
    /// many events (an import of 100,000 redirects) merges them in one pass.
    pub fn merge_events_from(&mut self, start: usize) {
        let mut merged: Vec<Event> = Vec::new();
        let mut firsts: HashMap<(String, String), usize> = HashMap::new();
        for event in self.events.drain(start.min(self.events.len())..) {
            let key = (event.topic.clone(), event.gid.clone());
            match firsts.get(&key) {
                Some(&index) => {
                    let first = &mut merged[index];
                    if !self.records.contains_key(&event.gid) || first.snapshot.is_none() {
                        first.snapshot = event.snapshot.or(first.snapshot.take());
                    }
                }
                None => {
                    firsts.insert(key, merged.len());
                    merged.push(event);
                }
            }
        }
        self.events.extend(merged);
    }

    /// The memos whose key starts with `prefix`, in no order: for a family that keeps a memo
    /// per record and must know, cheaply, whether any is there.
    pub fn memos_with_prefix<'s>(
        &'s self,
        prefix: &'s str,
    ) -> impl Iterator<Item = (&'s str, &'s Value)> + 's {
        self.memos
            .iter()
            .filter(move |(key, _)| key.starts_with(prefix))
            .map(|(key, value)| (key.as_str(), value))
    }

    /// What a mutation kept under `key` (see [`ShopState::set_memo`]).
    pub fn memo(&self, key: &str) -> Option<&Value> {
        self.memos.get(key)
    }

    /// Keeps `value` under `key` for the requests that follow: what is not a record, such as an
    /// idempotency key and the answer it gave. Not a change of the records: the store sync does
    /// not see it.
    pub fn set_memo(&mut self, key: impl Into<String>, value: Value) {
        let key = key.into();
        let before = self.memos.insert(key.clone(), value);
        self.log(|| Undo::Memo { key, before });
    }

    /// Opens a savepoint: from now on each write keeps what it overwrote, until
    /// [`ShopState::rollback`] puts it back or [`ShopState::release`] keeps the writes. What a
    /// mutation does before it fails is undone this way ([`crate::resolve::Mutation`]): the
    /// records it wrote, created and removed (in their places), the counters of new ids, the
    /// events it emitted, the side entries (blobs, memos, settings), the changes tracked for the
    /// store sync, the revision and the clock. Savepoints nest (a bulk operation runs a mutation
    /// per line within its own) and close in the reverse order. The events taken meanwhile
    /// ([`ShopState::take_events`]) are not put back, nor what a reset or [`Shops`] does.
    pub fn savepoint(&mut self) -> Savepoint {
        let journal = self.journal.get_or_insert_with(Journal::default);
        let depth = journal.starts.len();
        journal.starts.push(journal.steps.len());
        Savepoint {
            steps: journal.steps.len(),
            depth,
            events: self.events.len(),
            emitted: self.emitted,
            revision: self.revision,
            originals: self.originals.as_ref().map(IndexMap::len),
            clock: self.clock,
        }
    }

    /// Keeps the writes made since `savepoint`. They stay in the journal while an outer
    /// savepoint is open, which may still undo them.
    pub fn release(&mut self, savepoint: Savepoint) {
        let Some(journal) = &mut self.journal else {
            return;
        };
        debug_assert_eq!(
            journal.starts.last(),
            Some(&savepoint.steps),
            "the innermost savepoint"
        );
        journal.starts.pop();
        if journal.starts.is_empty() {
            self.journal = None;
        }
    }

    /// Puts the shop back as it was when `savepoint` was opened (see [`ShopState::savepoint`]),
    /// and closes it.
    pub fn rollback(&mut self, savepoint: Savepoint) {
        let Some(mut journal) = self.journal.take() else {
            return;
        };
        debug_assert_eq!(
            journal.starts.last(),
            Some(&savepoint.steps),
            "the innermost savepoint"
        );
        while journal.steps.len() > savepoint.steps {
            let Some(step) = journal.steps.pop() else {
                break;
            };
            self.undo(step);
        }
        journal.saved.retain(|_, step| *step < savepoint.steps);
        journal.starts.pop();
        if !journal.starts.is_empty() {
            self.journal = Some(journal);
        }
        self.events.truncate(savepoint.events);
        self.emitted = savepoint.emitted;
        self.revision = savepoint.revision;
        match (&mut self.originals, savepoint.originals) {
            (Some(originals), Some(len)) => originals.truncate(len),
            (originals, None) => *originals = None,
            (None, Some(_)) => {}
        }
        self.clock = savepoint.clock;
    }

    /// [`ShopState::rollback`] after a panic: the savepoints opened after `savepoint` that the
    /// panic left open are closed first, and undone with it.
    pub fn rollback_unwound(&mut self, savepoint: Savepoint) {
        if let Some(journal) = &mut self.journal {
            journal.starts.truncate(savepoint.depth + 1);
        }
        self.rollback(savepoint);
    }

    /// Runs `change` on the shop and, when it panics, puts the shop back as it was before it
    /// (see [`ShopState::savepoint`]) and answers the panic: a bug in a mutation, or in what
    /// follows one, then leaves no half-made change behind for every later request to trip on.
    pub fn unwinding<T>(
        &mut self,
        change: impl FnOnce(&mut ShopState) -> T,
    ) -> Result<T, Box<dyn std::any::Any + Send>> {
        let savepoint = self.savepoint();
        match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| change(self))) {
            Ok(value) => {
                self.release(savepoint);
                Ok(value)
            }
            Err(panic) => {
                self.rollback_unwound(savepoint);
                Err(panic)
            }
        }
    }

    /// Adds a step to the journal, when a savepoint is open.
    fn log(&mut self, step: impl FnOnce() -> Undo) {
        if let Some(journal) = &mut self.journal {
            journal.steps.push(step());
        }
    }

    /// Keeps the entry of `key` before an in-place write, unless the innermost savepoint keeps
    /// it already.
    fn save_entry(&mut self, key: &str) {
        let Some(journal) = &mut self.journal else {
            return;
        };
        let start = journal.starts.last().copied().unwrap_or(0);
        if journal.saved.get(key).is_some_and(|&step| step >= start) {
            return;
        }
        let Some(entry) = self.records.get(key) else {
            return;
        };
        journal.saved.insert(key.to_string(), journal.steps.len());
        journal.steps.push(Undo::Changed {
            key: key.to_string(),
            before: entry.clone(),
        });
    }

    /// Undoes one write: the shop is as it was right after it, the later ones being undone.
    fn undo(&mut self, step: Undo) {
        match step {
            Undo::Changed { key, before } => {
                if let Some(entry) = self.records.get_mut(&key) {
                    *entry = before;
                }
            }
            Undo::Added { key } => {
                if let Some(entry) = self.records.shift_remove(&key)
                    && let Some(keys) = self.by_type.get_mut(&entry.type_name)
                {
                    keys.shift_remove(&key);
                }
            }
            Undo::Retyped {
                key,
                before,
                position,
            } => {
                if let Some(entry) = self.records.get_mut(&key) {
                    if let Some(keys) = self.by_type.get_mut(&entry.type_name) {
                        keys.shift_remove(&key);
                    }
                    let keys = self.by_type.entry(before.type_name.clone()).or_default();
                    keys.shift_insert(position.min(keys.len()), key);
                    *entry = before;
                }
            }
            Undo::Removed {
                key,
                entry,
                index,
                position,
            } => {
                if let Some(position) = position {
                    let keys = self.by_type.entry(entry.type_name.clone()).or_default();
                    keys.shift_insert(position.min(keys.len()), key.clone());
                }
                let index = index.min(self.records.len());
                self.records.shift_insert(index, key, entry);
            }
            Undo::Counter { group, before } => match before {
                Some(next) => {
                    self.counters.insert(group, next);
                }
                None => {
                    self.counters.remove(&group);
                }
            },
            Undo::Blob { key, before } => restore(&mut self.blobs, key, before),
            Undo::Memo { key, before } => restore(&mut self.memos, key, before),
            // The key set last is the map's last: removing it keeps the order of the others.
            Undo::Setting { key, before } => match before {
                Some(value) => {
                    self.settings.insert(key, value);
                }
                None => {
                    self.settings.remove(&key);
                }
            },
        }
    }

    /// Starts tracking the changes made to the records from now on, forgetting those tracked so
    /// far: the seed calls it once the records of the data are made, so that what
    /// [`ShopState::changes`] lists is what the Admin API changed since.
    pub fn track_changes(&mut self) {
        self.originals = Some(IndexMap::new());
    }

    /// Keeps the record of `key` as it is, before its first change since tracking started.
    /// Cheap: one copy of a record the first time it changes, nothing afterwards.
    fn note(&mut self, key: &str) {
        self.revision += 1;
        let Some(originals) = &mut self.originals else {
            return;
        };
        if !originals.contains_key(key) {
            let original = self.records.get(key).map(|entry| entry.record.clone());
            originals.insert(key.to_string(), original);
        }
    }

    /// A number that grows whenever a record is written: added, removed, or handed out by
    /// [`ShopState::get_mut`] (which may leave it as it was). The same number before and after a
    /// request means that the request changed nothing.
    pub fn revision(&self) -> u64 {
        self.revision
    }

    /// Whether a record may have changed since the seed: it was changed, created or removed,
    /// possibly back to what it was. Without tracking, nothing has.
    pub fn touched(&self) -> bool {
        self.originals
            .as_ref()
            .is_some_and(|originals| !originals.is_empty())
    }

    /// The records that changed since the seed, in the order they were first changed: created,
    /// changed (and not back to what they were), or removed. A record created then removed is
    /// not listed. Without tracking, none.
    pub fn changes(&self) -> Vec<Change<'_>> {
        let Some(originals) = &self.originals else {
            return Vec::new();
        };
        originals
            .iter()
            .filter_map(|(key, original)| {
                let current = self.get(key);
                let kind = match (original, current) {
                    (None, Some(_)) => ChangeKind::Inserted,
                    (Some(_), None) => ChangeKind::Removed,
                    (Some(before), Some(now)) if before != now => ChangeKind::Changed,
                    _ => return None,
                };
                Some(Change {
                    key,
                    kind,
                    original: original.as_ref(),
                })
            })
            .collect()
    }

    /// The record of a key as the seed made it: the record itself when it did not change, its
    /// copy when it did, `None` when the seed did not make it.
    pub fn original(&self, key: &str) -> Option<&Record> {
        match self
            .originals
            .as_ref()
            .and_then(|originals| originals.get(key))
        {
            Some(original) => original.as_ref(),
            None => self.get(key),
        }
    }

    /// Whether the record of a key differs from the seed's (created, changed or removed).
    pub fn changed(&self, key: &str) -> bool {
        match self
            .originals
            .as_ref()
            .and_then(|originals| originals.get(key))
        {
            Some(original) => original.as_ref() != self.get(key),
            None => false,
        }
    }

    /// Everything the shop holds, in a form two shops can be compared by (the orders included:
    /// records, keys by type, counters, events, side entries, tracked changes): what the tests
    /// of a rollback compare.
    #[cfg(test)]
    pub(crate) fn picture(&self) -> String {
        fn sorted<T: std::fmt::Debug>(items: impl Iterator<Item = T>) -> Vec<String> {
            let mut items: Vec<String> = items.map(|item| format!("{item:?}")).collect();
            items.sort();
            items
        }
        let by_type = sorted(
            self.by_type
                .iter()
                .filter(|(_, keys)| !keys.is_empty())
                .map(|(type_name, keys)| (type_name, keys.iter().collect::<Vec<_>>())),
        );
        format!(
            "domain {:?}\nrecords {:?}\nby type {by_type:?}\ncounters {:?}\nclock {:?}\n\
             events {:?}\nemitted {}\noriginals {:?}\nblobs {:?}\nrevision {}\nmemos {:?}\n\
             settings {:?}",
            self.domain,
            self.records.iter().collect::<Vec<_>>(),
            sorted(self.counters.iter()),
            self.clock,
            self.events,
            self.emitted,
            self.originals
                .as_ref()
                .map(|originals| originals.iter().collect::<Vec<_>>()),
            sorted(self.blobs.iter()),
            self.revision,
            sorted(self.memos.iter()),
            self.settings,
        )
    }

    /// Moves the shop to another domain: what a shop created from the seed of another one needs.
    /// The `Shop` record gets the id of the new domain, and every value naming the old domain
    /// (`myshopifyDomain`, the host of the primary domain when it is the myshopify one, URLs on
    /// it) or the old `Shop` GID is rewritten.
    fn rehost(&mut self, domain: &str) {
        let old_domain = std::mem::replace(&mut self.domain, domain.to_string());
        let old_origin = format!("https://{old_domain}");
        let new_origin = format!("https://{domain}");
        let old_shop = self.shop_gid().map(str::to_string);
        let new_shop = gid("Shop", lsf_core::util::stable_id("shop", domain));
        let rewrite = |text: &str| -> Option<String> {
            if Some(text) == old_shop.as_deref() {
                Some(new_shop.clone())
            } else if text == old_domain {
                Some(domain.to_string())
            } else {
                text.strip_prefix(&old_origin)
                    .filter(|rest| rest.is_empty() || rest.starts_with(['/', '?']))
                    .map(|rest| format!("{new_origin}{rest}"))
            }
        };
        for entry in self.records.values_mut() {
            for value in entry.record.values_mut() {
                rewrite_strings(value, &rewrite);
            }
        }
        // Moving is not a change the store sync projects: what was tracked stays as it was.
        let originals = self.originals.take();
        if let Some(old_shop) = old_shop
            && let Some(record) = self.remove(&old_shop)
        {
            self.insert(record);
        }
        self.originals = originals;
    }

    /// The shop as it is after a reset: `seed`, under this shop's domain, with the events gone
    /// and the event sequence kept. The revision moves on: a reset is a write.
    fn reset_from(&mut self, seed: &ShopState) {
        let domain = self.domain.clone();
        let (emitted, revision) = (self.emitted, self.revision);
        *self = seed.clone();
        self.emitted = emitted;
        self.revision = revision + 1;
        if domain != self.domain {
            self.rehost(&domain);
        }
    }
}

/// Puts back the value a key of a side map had (`None`: it had none).
fn restore<V>(map: &mut HashMap<String, V>, key: String, before: Option<V>) {
    match before {
        Some(value) => {
            map.insert(key, value);
        }
        None => {
            map.remove(&key);
        }
    }
}

/// The type of a record: its `__typename`, else the type of its GID.
fn record_type(record: &Record) -> String {
    record
        .get("__typename")
        .and_then(Value::as_str)
        .or_else(|| record.get("id").and_then(Value::as_str).and_then(gid_type))
        .unwrap_or_default()
        .to_string()
}

/// The counter a type takes its new ids from: its own, except the discount nodes, which share
/// one.
fn counter_group(type_name: &str) -> String {
    match type_name {
        "DiscountAutomaticNode" | "DiscountCodeNode" | "DiscountNode" => "DiscountNode".to_string(),
        // `Catalog/<n>` names any catalog: they share their ids.
        "MarketCatalog" | "CompanyLocationCatalog" | "AppCatalog" | "Catalog" => {
            "Catalog".to_string()
        }
        other => other.to_string(),
    }
}

fn rewrite_strings(value: &mut Value, rewrite: &impl Fn(&str) -> Option<String>) {
    match value {
        Value::String(text) => {
            if let Some(new) = rewrite(text) {
                *text = new;
            }
        }
        Value::Array(items) => items
            .iter_mut()
            .for_each(|item| rewrite_strings(item, rewrite)),
        Value::Object(map) => map
            .values_mut()
            .for_each(|item| rewrite_strings(item, rewrite)),
        _ => {}
    }
}

/// The shops of the server, by domain (`my-shop.myshopify.com`).
///
/// The default shop is the store data's (`shop.permanent_domain`). Others are created for test
/// isolation, each a copy of the same seed. No locking here: the `api` layer wraps each shop in
/// a mutex.
#[derive(Clone, Debug)]
pub struct Shops {
    /// The records the store data and the app give, under the default domain: every shop starts
    /// as a copy of them.
    seed: ShopState,
    shops: IndexMap<String, ShopState>,
}

impl Shops {
    /// The default shop, seeded from the store data and the app.
    pub fn new(store: &Store, app: Option<&App>) -> Shops {
        let seed = ShopState::seeded(store, app);
        let mut shops = IndexMap::new();
        shops.insert(seed.domain().to_string(), seed.clone());
        Shops { seed, shops }
    }

    /// The domain of the default shop: `shop.permanent_domain` of the store data.
    pub fn default_domain(&self) -> &str {
        self.seed.domain()
    }

    pub fn default_shop(&self) -> Option<&ShopState> {
        self.shops.get(self.seed.domain())
    }

    pub fn default_shop_mut(&mut self) -> Option<&mut ShopState> {
        self.shops.get_mut(self.seed.domain())
    }

    pub fn get(&self, domain: &str) -> Option<&ShopState> {
        self.shops.get(&domain.to_ascii_lowercase())
    }

    pub fn get_mut(&mut self, domain: &str) -> Option<&mut ShopState> {
        self.shops.get_mut(&domain.to_ascii_lowercase())
    }

    /// The domains of the shops, the default one first, then in creation order.
    pub fn domains(&self) -> impl Iterator<Item = &str> {
        self.shops.keys().map(String::as_str)
    }

    pub fn len(&self) -> usize {
        self.shops.len()
    }

    pub fn is_empty(&self) -> bool {
        self.shops.is_empty()
    }

    /// Creates a shop seeded from the same data: `test-1` (or `test-1.myshopify.com`) makes
    /// `test-1.myshopify.com`. An error when the name is not a valid shop name, or is taken.
    pub fn create(&mut self, name: &str) -> Result<&mut ShopState, String> {
        let domain = shop_domain(name)?;
        if self.shops.contains_key(&domain) {
            return Err(format!("the shop {domain} already exists"));
        }
        let mut shop = self.seed.clone();
        shop.rehost(&domain);
        Ok(self.shops.entry(domain).or_insert(shop))
    }

    /// A shop seeded from the same data under the domain of `name`, as [`Shops::create`] makes
    /// it, but not kept here: for a server that keeps each shop behind a lock of its own.
    pub fn new_shop(&self, name: &str) -> Result<ShopState, String> {
        let domain = shop_domain(name)?;
        let mut shop = self.seed.clone();
        if domain != self.seed.domain() {
            shop.rehost(&domain);
        }
        Ok(shop)
    }

    /// Puts a shop kept outside back to the seed, as [`Shops::reset`] does to those kept here:
    /// its domain and its event sequence stay.
    pub fn reset_shop(&self, shop: &mut ShopState) {
        shop.reset_from(&self.seed);
    }

    /// Forgets a shop other than the default one. Returns whether it existed.
    pub fn remove(&mut self, domain: &str) -> bool {
        let domain = domain.to_ascii_lowercase();
        domain != self.seed.domain() && self.shops.shift_remove(&domain).is_some()
    }

    /// Puts every shop back to the seed: what the Admin API changed is dropped, the shops
    /// created stay (seeded again), and the pending events are dropped.
    pub fn reset(&mut self) {
        let seed = &self.seed;
        for shop in self.shops.values_mut() {
            shop.reset_from(seed);
        }
    }

    /// Changes the seed, and every shop kept here, with what the server knows and the store data
    /// does not (the directory of the local theme): not a change of the Admin API
    /// ([`ShopState::amend_seed`]). A reseed starts from the store data again: amend it again.
    pub fn amend(&mut self, amend: impl Fn(&mut ShopState)) {
        self.seed.amend_seed(&amend);
        for shop in self.shops.values_mut() {
            shop.amend_seed(&amend);
        }
    }

    /// Seeds again from new store data (the data changed on disk), then resets every shop. The
    /// default shop follows `shop.permanent_domain` if it changed.
    pub fn reseed(&mut self, store: &Store, app: Option<&App>) {
        let seed = ShopState::seeded(store, app);
        let old_default = self.seed.domain().to_string();
        if seed.domain() != old_default
            && let Some(shop) = self.shops.shift_remove(&old_default)
        {
            let mut moved = shop;
            moved.domain = seed.domain().to_string();
            self.shops.insert(seed.domain().to_string(), moved);
            self.shops.move_index(self.shops.len() - 1, 0);
        }
        self.seed = seed;
        self.reset();
    }
}

/// The domain of a shop name: `test-1` → `test-1.myshopify.com`.
pub fn shop_domain(name: &str) -> Result<String, String> {
    let name = name.trim().to_ascii_lowercase();
    let handle = name.strip_suffix(".myshopify.com").unwrap_or(&name);
    let valid = !handle.is_empty()
        && handle.len() <= 60
        && handle
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-')
        && !handle.starts_with('-')
        && !handle.ends_with('-');
    if valid {
        Ok(format!("{handle}.myshopify.com"))
    } else {
        Err(format!(
            "\"{name}\" is not a shop name: use lowercase letters, digits and hyphens, e.g. \"test-1\""
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn record(value: Value) -> Record {
        match value {
            Value::Object(map) => map,
            _ => unreachable!(),
        }
    }

    #[test]
    fn reads_the_days_of_the_shop_in_its_time_zone() {
        let now = DateTime::parse_from_rfc3339("2026-03-01T02:00:00Z")
            .unwrap()
            .with_timezone(&Utc);
        let mut state = ShopState::new("a.myshopify.com", Clock::Frozen(now));
        assert_eq!(state.iana_time_zone(), None);
        assert_eq!(state.today().to_string(), "2026-03-01");
        let shop = state.insert(record(json!({
            "__typename": "Shop",
            "id": "gid://shopify/Shop/1",
            "ianaTimezone": "America/New_York",
        })));
        assert_eq!(state.time_zone(), chrono_tz::America::New_York);
        assert_eq!(state.today().to_string(), "2026-02-28");
        state.get_mut(&shop).unwrap()["ianaTimezone"] = json!("Mars/Olympus");
        assert_eq!(state.iana_time_zone(), None);
        assert_eq!(state.time_zone(), Tz::UTC);
    }

    #[test]
    fn reads_gids() {
        assert_eq!(gid("Product", 7), "gid://shopify/Product/7");
        assert_eq!(parse_gid("gid://shopify/Product/7"), Some(("Product", 7)));
        assert_eq!(
            parse_gid("gid://shopify/ProductImage/3?product_id=7"),
            Some(("ProductImage", 3))
        );
        assert_eq!(parse_gid("gid://other/Product/7"), None);
        assert_eq!(parse_gid("gid://shopify/Product/x"), None);
        assert_eq!(gid_number("gid://shopify/Order/7"), 7);
        assert_eq!(
            gid_number("gid://shopify/MailingAddress/3?model_name=Address"),
            3
        );
        assert_eq!(gid_number("gid://shopify/Order/x"), 0);
        assert_eq!(gid_number(""), 0);
        assert_eq!(
            gid_type("gid://shopify/ShopifyFunction/a-b"),
            Some("ShopifyFunction")
        );
        assert_eq!(gid_type("fr"), None);
    }

    /// Two runs of the same data (two shops seeded alike) do not repeat event ids: a backend that
    /// remembers the events it processed would drop the second run's webhooks.
    #[test]
    fn event_ids_differ_between_runs_of_the_same_data() {
        let mut first = ShopState::new("a.myshopify.com", Clock::System);
        let mut second = ShopState::new("a.myshopify.com", Clock::System);
        let a = first
            .emit("products/update", "gid://shopify/Product/1")
            .id
            .clone();
        let b = second
            .emit("products/update", "gid://shopify/Product/1")
            .id
            .clone();
        assert_ne!(a, b);
        assert_eq!(a.len(), 36);
    }

    #[test]
    fn keeps_records_by_type_in_creation_order() {
        let mut state = ShopState::new("a.myshopify.com", Clock::System);
        state.insert(record(
            json!({"__typename": "Product", "id": "gid://shopify/Product/9"}),
        ));
        state.insert(record(
            json!({"__typename": "Product", "id": "gid://shopify/Product/3"}),
        ));
        state.insert(record(
            json!({"__typename": "Collection", "id": "gid://shopify/Collection/5"}),
        ));
        let ids: Vec<&str> = state.keys("Product").collect();
        assert_eq!(ids, ["gid://shopify/Product/9", "gid://shopify/Product/3"]);
        assert_eq!(state.count("Product"), 2);
        assert_eq!(state.all_of(&["Collection", "Product"]).count(), 3);
        // A new id is above every id of the type.
        assert_eq!(state.new_gid("Product"), "gid://shopify/Product/10");
        assert_eq!(state.new_gid("Product"), "gid://shopify/Product/11");
        assert_eq!(state.new_gid("Order"), "gid://shopify/Order/1");
        // A record without an id gets a new one.
        let new = state.insert(record(json!({"__typename": "Product", "title": "New"})));
        assert_eq!(new, "gid://shopify/Product/12");
        assert_eq!(state.get(&new).unwrap()["id"], json!(new));
        // Replacing keeps the position; removing forgets it.
        state.insert(record(
            json!({"__typename": "Product", "id": "gid://shopify/Product/9", "title": "Nine"}),
        ));
        assert_eq!(
            state.keys("Product").next(),
            Some("gid://shopify/Product/9")
        );
        assert!(state.remove("gid://shopify/Product/3").is_some());
        assert_eq!(state.count("Product"), 2);
        assert!(state.get("gid://shopify/Product/3").is_none());
    }

    #[test]
    fn discount_nodes_share_their_ids() {
        let mut state = ShopState::new("a.myshopify.com", Clock::System);
        state.insert(record(
            json!({"__typename": "DiscountCodeNode", "id": "gid://shopify/DiscountCodeNode/2"}),
        ));
        assert_eq!(
            state.new_gid("DiscountAutomaticNode"),
            "gid://shopify/DiscountAutomaticNode/3"
        );
    }

    #[test]
    fn touches_and_emits_at_the_frozen_clock() {
        let now = DateTime::parse_from_rfc3339("2026-03-01T10:00:00.5Z")
            .unwrap()
            .with_timezone(&Utc);
        let mut state = ShopState::new("a.myshopify.com", Clock::Frozen(now));
        let gid = state.insert(record(json!({
            "__typename": "Product", "id": "gid://shopify/Product/1",
            "title": "Hat", "updatedAt": "2024-01-01T00:00:00Z"
        })));
        assert!(state.touch(&gid));
        assert_eq!(
            state.get(&gid).unwrap()["updatedAt"],
            json!("2026-03-01T10:00:00Z")
        );
        assert!(!state.touch("gid://shopify/Product/2"));

        let first = state.emit("products/update", &gid).clone();
        assert_eq!(first.shop, "a.myshopify.com");
        assert_eq!(first.snapshot, None);
        let deleted = state.delete(&gid, "products/delete").unwrap();
        assert_eq!(deleted["title"], json!("Hat"));
        let events = state.take_events();
        assert_eq!(events.len(), 2);
        assert_eq!(events[1].snapshot.as_ref().unwrap()["title"], json!("Hat"));
        assert_ne!(events[0].id, events[1].id);
        assert_eq!(events[0].id.len(), 36);
        assert!(state.take_events().is_empty());
    }

    #[test]
    fn tracks_what_changed_since_the_seed() {
        let mut state = ShopState::new("a.myshopify.com", Clock::System);
        let product = |id: u64, title: &str| {
            record(json!({"__typename": "Product", "id": gid("Product", id), "title": title}))
        };
        state.insert(product(1, "One"));
        state.insert(product(2, "Two"));
        state.insert(product(3, "Three"));
        state.insert(product(4, "Four"));
        // Before tracking starts, nothing counts.
        assert!(!state.touched());
        state.track_changes();
        assert!(!state.touched() && state.changes().is_empty());

        state.get_mut("gid://shopify/Product/1").unwrap()["title"] = json!("Uno");
        // Changed, then back to what it was: not a change.
        state.get_mut("gid://shopify/Product/2").unwrap()["title"] = json!("Dos");
        state.get_mut("gid://shopify/Product/2").unwrap()["title"] = json!("Two");
        // Read through `get_mut` without a change: not a change either.
        let _ = state.get_mut("gid://shopify/Product/4");
        state.remove("gid://shopify/Product/3");
        let created = state.insert(record(json!({"__typename": "Product", "title": "New"})));
        // Created then removed: nothing to say.
        let gone = state.insert(record(json!({"__typename": "Product", "title": "Gone"})));
        state.remove(&gone);

        assert!(state.touched());
        let changes: Vec<(&str, ChangeKind)> = state
            .changes()
            .iter()
            .map(|change| (change.key, change.kind))
            .collect();
        assert_eq!(
            changes,
            [
                ("gid://shopify/Product/1", ChangeKind::Changed),
                ("gid://shopify/Product/3", ChangeKind::Removed),
                (created.as_str(), ChangeKind::Inserted),
            ]
        );
        assert_eq!(
            state.original("gid://shopify/Product/1").unwrap()["title"],
            json!("One")
        );
        assert_eq!(
            state.original("gid://shopify/Product/3").unwrap()["title"],
            json!("Three")
        );
        assert_eq!(
            state.original("gid://shopify/Product/4").unwrap()["title"],
            json!("Four")
        );
        assert!(state.original(&created).is_none());
        assert!(state.changed("gid://shopify/Product/1"));
        assert!(!state.changed("gid://shopify/Product/2"));
        assert!(!state.changed("gid://shopify/Product/4"));

        // Reading leaves the revision as it is; any write moves it on.
        let revision = state.revision();
        let _ = state.get("gid://shopify/Product/1");
        let _ = state.changes();
        assert_eq!(state.revision(), revision);
        let _ = state.get_mut("gid://shopify/Product/4");
        assert!(state.revision() > revision);
    }

    #[test]
    fn a_rollback_puts_back_every_write_since_its_savepoint() {
        let mut state = ShopState::new("a.myshopify.com", Clock::System);
        for id in 1..=6 {
            state.insert(record(json!({
                "__typename": "Product", "id": gid("Product", id), "title": format!("P{id}"),
                "updatedAt": "2024-01-01T00:00:00Z"
            })));
        }
        state.insert(record(
            json!({"__typename": "Collection", "id": "gid://shopify/Collection/1"}),
        ));
        state.set_setting("taxRateBps", json!(2000));
        state.set_memo("kept", json!(1));
        state.put_blob("kept", Blob::new("a", "text/plain"));
        state.track_changes();
        state.get_mut("gid://shopify/Product/1").unwrap()["title"] = json!("Changed before");
        state.emit("products/update", "gid://shopify/Product/1");
        let before = state.picture();

        let savepoint = state.savepoint();
        // Every kind of write, some twice, in an order that moves positions.
        state.get_mut("gid://shopify/Product/2").unwrap()["title"] = json!("Two");
        state.get_mut("gid://shopify/Product/2").unwrap()["title"] = json!("Deux");
        state.touch("gid://shopify/Product/3");
        state.remove("gid://shopify/Product/4");
        state.delete("gid://shopify/Product/2", "products/delete");
        let created = state.insert(record(json!({"__typename": "Product", "title": "New"})));
        state.get_mut(&created).unwrap()["title"] = json!("Newer");
        state.insert(record(json!({
            "__typename": "Product", "id": "gid://shopify/Product/2", "title": "Back"
        })));
        state.insert(record(json!({
            "__typename": "Collection", "id": "gid://shopify/Product/5", "title": "Retyped"
        })));
        state.insert(record(
            json!({"__typename": "Order", "id": "gid://shopify/Order/40"}),
        ));
        state.new_gid("Customer");
        state.emit_with("customer.tags_added", &created, Record::new());
        state.set_setting("taxRateBps", json!(0));
        state.set_setting("new", json!(true));
        state.set_memo("kept", json!(2));
        state.set_memo("new", json!(3));
        state.put_blob("kept", Blob::new("b", "text/plain"));
        state.put_blob("new", Blob::new("c", "text/plain"));
        state.set_clock(Clock::Frozen(Utc::now()));
        // A nested savepoint kept, then one undone: the outer rollback undoes both.
        let inner = state.savepoint();
        state.get_mut("gid://shopify/Product/6").unwrap()["title"] = json!("Inner");
        state.remove("gid://shopify/Collection/1");
        state.release(inner);
        let inner = state.savepoint();
        let mid = state.picture();
        state.get_mut("gid://shopify/Product/6").unwrap()["title"] = json!("Undone");
        state.remove("gid://shopify/Product/1");
        state.rollback(inner);
        assert_eq!(state.picture(), mid, "the inner rollback");
        state.get_mut("gid://shopify/Product/6").unwrap()["title"] = json!("Again");
        assert_ne!(state.picture(), before);
        state.rollback(savepoint);

        assert_eq!(state.picture(), before);
        assert!(state.journal.is_none());
        // New ids start where they did, and the next event has the next sequence.
        assert_eq!(state.new_gid("Product"), "gid://shopify/Product/7");
        assert_eq!(state.changes().len(), 1);
    }

    #[test]
    fn a_released_savepoint_keeps_the_writes() {
        let mut state = ShopState::new("a.myshopify.com", Clock::System);
        state.insert(record(
            json!({"__typename": "Product", "id": "gid://shopify/Product/1", "title": "One"}),
        ));
        let savepoint = state.savepoint();
        state.get_mut("gid://shopify/Product/1").unwrap()["title"] = json!("Kept");
        state.emit("products/update", "gid://shopify/Product/1");
        state.release(savepoint);
        assert!(state.journal.is_none());
        assert_eq!(
            state.get("gid://shopify/Product/1").unwrap()["title"],
            json!("Kept")
        );
        assert_eq!(state.events().len(), 1);
    }

    #[test]
    fn a_shop_starts_without_changes_and_a_reset_forgets_them() {
        let store = lsf_core::store::load::load(
            &lsf_core::store::load::DataSource::Demo,
            &lsf_core::store::load::LoadOptions::default(),
        )
        .0;
        let mut shops = Shops::new(&store, None);
        let shop = shops.default_shop_mut().unwrap();
        assert!(!shop.touched());
        let product = shop.keys("Product").next().unwrap().to_string();
        shop.get_mut(&product).unwrap()["title"] = json!("Changed");
        assert_eq!(shop.changes().len(), 1);
        // A shop made from the seed is moved to its domain without a change.
        let other = shops.create("test-1").unwrap();
        assert!(other.changes().is_empty());
        shops.reset();
        assert!(!shops.default_shop().unwrap().touched());
    }

    #[test]
    fn names_shops() {
        assert_eq!(shop_domain("test-1").unwrap(), "test-1.myshopify.com");
        assert_eq!(
            shop_domain("Test-1.myshopify.com").unwrap(),
            "test-1.myshopify.com"
        );
        assert!(shop_domain("").is_err());
        assert!(shop_domain("a b").is_err());
        assert!(shop_domain("other.example.com").is_err());
    }
}
