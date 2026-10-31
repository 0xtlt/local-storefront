//! Store sync as a server runs it: what the Admin API changed in the default shop, projected
//! onto the store the shops were seeded from ([`crate::sync::overlay`]) when the server asks for
//! it, and kept until the shop changes again.
//!
//! A change only counts itself ([`StoreSync::changed`], an atomic increment under the shop's
//! lock): the Admin request that made it pays nothing more. The projection waits for the next
//! request that reads the store ([`StoreSync::update`]), so that a burst of mutations is
//! projected once, and the store built once.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Duration;

use lsf_core::Store;
use lsf_core::diagnostics::Diagnostics;
use lsf_core::store::load::Overlay;
use serde_json::{Map, Value, json};

use crate::state::ShopState;

/// What the Admin API changed in the default shop, as store data.
#[derive(Clone, Debug, Default)]
pub struct Synced {
    /// The count of changes ([`crate::api::AdminApi::store_changes`]) the overlay was made at.
    pub changes: u64,
    /// Moves on when the overlay changes, and only then: what a store built with the overlay
    /// is known by. `0` is the store of the files, before the Admin API changed anything.
    pub generation: u64,
    /// The overlay of the store data the shop was seeded from: `None` when nothing that the
    /// store data expresses changed, and the store is the files'.
    pub overlay: Option<Arc<Overlay>>,
}

/// The store a server built last with an overlay, for `GET /__lsf/admin`.
#[derive(Clone, Debug)]
struct Built {
    generation: u64,
    took: Duration,
    /// What the store data said of the overlay, as JSON.
    diagnostics: Vec<Value>,
}

struct Inner {
    /// The store the shops were seeded from: the files, without any overlay.
    base: Arc<Store>,
    /// The overlay as last made.
    synced: Synced,
    built: Option<Built>,
}

/// The store sync of a server's default shop: off until the server gives it the store the
/// shops were seeded from ([`StoreSync::start`]).
///
/// Locks: the default shop's, then this one's; never the other way around.
#[derive(Default)]
pub(super) struct StoreSync {
    /// How many times the default shop changed (or was put back to the seed): read without a
    /// lock, so that a server knows at once whether its store may be behind.
    changes: AtomicU64,
    inner: Mutex<Option<Inner>>,
}

impl StoreSync {
    fn inner(&self) -> MutexGuard<'_, Option<Inner>> {
        self.inner
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    /// How many times the default shop changed.
    pub(super) fn changes(&self) -> u64 {
        self.changes.load(Ordering::Acquire)
    }

    /// Counts a change of the default shop, while it is locked.
    pub(super) fn changed(&self) {
        self.changes.fetch_add(1, Ordering::AcqRel);
    }

    /// Whether the server turned sync on.
    pub(super) fn is_on(&self) -> bool {
        self.inner().is_some()
    }

    /// Turns sync on over `base`, or moves it to a new base after a reseed: under the default
    /// shop's lock. The overlay is made again on the next [`StoreSync::update`].
    pub(super) fn start(&self, base: Arc<Store>) {
        let mut inner = self.inner();
        match inner.as_mut() {
            Some(inner) => inner.base = base,
            None => {
                *inner = Some(Inner {
                    base,
                    synced: Synced {
                        changes: self.changes(),
                        generation: 0,
                        overlay: None,
                    },
                    built: None,
                });
                return;
            }
        }
        drop(inner);
        self.changed();
    }

    /// The overlay of `state` (the default shop, locked by the caller), made again when the
    /// shop changed since it was last made. `None` while sync is off.
    pub(super) fn update(&self, state: &ShopState) -> Option<Synced> {
        let changes = self.changes();
        let base = {
            let inner = self.inner();
            let inner = inner.as_ref()?;
            if inner.synced.changes == changes {
                return Some(inner.synced.clone());
            }
            inner.base.clone()
        };
        // Made without this lock: `GET /__lsf/admin` reads it meanwhile. The shop's lock keeps
        // the shop, the base and the count as they were read.
        let overlay = crate::sync::overlay(state, &base);
        let mut inner = self.inner();
        let inner = inner.as_mut()?;
        if inner.synced.overlay.as_deref() != overlay.as_ref() {
            inner.synced.generation += 1;
            inner.synced.overlay = overlay.map(Arc::new);
        }
        inner.synced.changes = changes;
        Some(inner.synced.clone())
    }

    /// Notes what the server built with the overlay of `generation`: how long it took, and what
    /// the store data said of the overlay.
    pub(super) fn built(&self, generation: u64, took: Duration, diagnostics: &Diagnostics) {
        if let Some(inner) = self.inner().as_mut() {
            inner.built = Some(Built {
                generation,
                took,
                diagnostics: diagnostics
                    .items
                    .iter()
                    .filter_map(|item| serde_json::to_value(item).ok())
                    .collect(),
            });
        }
    }

    /// `storeSync` of `GET /__lsf/admin`.
    pub(super) fn status(&self) -> Value {
        let changes = self.changes();
        let inner = self.inner();
        let Some(inner) = inner.as_ref() else {
            return json!({"enabled": false});
        };
        let synced = &inner.synced;
        json!({
            "enabled": true,
            "generation": synced.generation,
            // The shop changed since the overlay was made: the next page projects it.
            "pending": synced.changes != changes,
            "overlay": synced.overlay.as_deref().map(summary),
            "built": inner.built.as_ref().map(|built| json!({
                "generation": built.generation,
                "ms": (built.took.as_secs_f64() * 10_000.0).round() / 10.0,
                "diagnostics": built.diagnostics,
            })),
        })
    }
}

/// What an overlay writes, in short: how many entities of each section of the store data, and
/// the keys of those it removes.
fn summary(overlay: &Overlay) -> Value {
    let mut written = Map::new();
    if let Value::Object(sections) = &overlay.data {
        for (section, value) in sections {
            let count = match value {
                Value::Array(items) => items.len(),
                Value::Object(fields) => fields.len(),
                _ => 1,
            };
            written.insert(section.clone(), json!(count));
        }
    }
    let removed = match serde_json::to_value(&overlay.removed) {
        Ok(Value::Object(lists)) => lists
            .into_iter()
            .filter(|(_, keys)| match keys {
                Value::Array(keys) => !keys.is_empty(),
                Value::Object(keys) => !keys.is_empty(),
                _ => false,
            })
            .collect(),
        _ => Map::new(),
    };
    json!({"written": written, "removed": removed})
}
