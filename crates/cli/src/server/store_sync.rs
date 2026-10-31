//! Store sync: what the backend changes through the Admin API shows in the storefront and the
//! checkout. A product it creates is on the theme's pages, a customer it creates can log in, a
//! metafield it sets is read by Liquid and by the Function inputs, an app discount it creates
//! runs its Function at the checkout.
//!
//! The shops of the Admin API are seeded from the store of the files (the *base*, which
//! [`Loaded`] keeps, with the data the files gave). A change of the default shop is only counted
//! on the Admin request's path; the next request that reads the store sees the count move, asks
//! the Admin API for the overlay of its changes (made then, under the shop's lock), and when it
//! is not the one the store was built with, builds the store again: the data the files gave
//! when the base was loaded (never the files again) with the overlay on top, swapped in whole,
//! as a reload of the files is. A burst of mutations is built once, by the first page
//! that follows it; a page rendered meanwhile sees the store before or after, never half of it.
//! Pages served without a mutation in between pay one atomic read.
//!
//! See "Store sync" in `crates/admin/docs/design.md`, and "What the storefront and the checkout
//! see" in `docs/admin.md`.

use std::collections::HashSet;
use std::sync::atomic::Ordering;
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Instant;

use lsf_admin::api::Synced;
use lsf_core::Store;
use lsf_core::diagnostics::Diagnostics;
use lsf_core::store::load::{OVERLAY_FILE, Overlay};
use serde_json::Value as Json;

use super::{Incoming, Loaded, ServerState};

/// What the Admin API changed, as the store of a [`Loaded`] has it.
#[derive(Clone, Debug, Default)]
pub struct Applied {
    /// The generation of the overlay ([`Synced::generation`]): `0` for the files alone.
    pub generation: u64,
    pub overlay: Option<Arc<Overlay>>,
    /// What the store data said of the overlay: the problems of the store with it that the
    /// files alone do not have.
    pub diagnostics: Diagnostics,
}

/// The data of a session that has its own (`PUT /__lsf/session` with `data`). Its store is the
/// files, then what the Admin API changed, then this data: it is built again when what the
/// Admin API changed is.
#[derive(Clone, Debug)]
pub struct SessionData {
    pub data: Arc<Json>,
    /// The generation of the Admin API's changes the session's store was built with.
    pub generation: u64,
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

impl ServerState {
    /// Brings the store up to date with what the Admin API changed in its default shop, and
    /// the store of the session `incoming` names when that session has data of its own. Run
    /// before a request reads the store; does nothing (one atomic read) when the default shop
    /// did not change since it last ran.
    pub(super) fn sync_admin_changes(&self, incoming: Option<&Incoming>) {
        let Some(admin) = &self.admin else {
            return;
        };
        if admin.api.store_changes() != self.admin_changes.load(Ordering::Acquire) {
            let _rebuilding = lock(&self.rebuilding);
            // Another request may have built it while this one waited.
            if admin.api.store_changes() != self.admin_changes.load(Ordering::Acquire)
                && let Some(synced) = admin.api.synced()
            {
                let loaded = self.loaded();
                if synced.generation != loaded.synced.generation {
                    let rebuilt = Arc::new(self.with_admin_changes(&loaded, &synced));
                    *self.loaded.write().expect("store lock poisoned") = rebuilt;
                }
                self.sync_theme(&admin.api);
                self.admin_changes.store(synced.changes, Ordering::Release);
            }
        }
        if let Some(id) = incoming.and_then(|incoming| self.named_session(incoming)) {
            self.sync_session(&id);
        }
    }

    /// What the Admin API changed of the theme the storefront serves: the files written over
    /// the theme directory or deleted from it, or the whole of another theme it published
    /// (`lsf_admin::domains::themes`), the script tags `content_for_header` loads, and which
    /// theme `Shopify.theme` names (its id, its name). The directory is never written to.
    /// Applied when they changed, while `rebuilding` is held: after a change of the default
    /// shop, and after a reseed (`ServerState::reload`).
    pub(super) fn sync_theme(&self, api: &lsf_admin::api::AdminApi) {
        let files = self.app.theme.files();
        let overlay = api.storefront_overlay();
        let generation = overlay.as_ref().map_or(0, |overlay| overlay.generation);
        if generation != files.overlay_generation() {
            files.set_overlay(overlay.map(Arc::new));
        }
        let scripts = api.storefront_scripts();
        if *self.app.theme.script_tags() != *scripts {
            self.app.theme.set_script_tags(scripts);
        }
        // Which theme `Shopify.theme`, the `theme` object and the asset URLs name.
        let identity = api.storefront_theme();
        if self.app.theme.identity().as_deref() != identity.as_ref() {
            self.app.theme.set_identity(identity);
        }
    }

    /// The store of the files of `files` (its base, the data its files gave, its diagnostics,
    /// its fingerprint) with what the Admin API changed on top, as `synced` says: built from
    /// that data, not from the files on disk, which may have changed since without a reload. The store data's problems with the
    /// overlay are printed when they are new, and the build is told to the Admin API for
    /// `GET /__lsf/admin`. Called while `rebuilding` is held.
    pub(super) fn with_admin_changes(&self, files: &Loaded, synced: &Synced) -> Loaded {
        let started = Instant::now();
        let (store, diagnostics) = match &synced.overlay {
            None => (files.base.clone(), Diagnostics::new()),
            Some(overlay) => {
                let (store, all) = files
                    .source
                    .build(&files.load_options, std::slice::from_ref(overlay.as_ref()));
                (Arc::new(store), new_problems(all, &files.diagnostics))
            }
        };
        let took = started.elapsed();
        if let Some(admin) = &self.admin {
            admin.api.sync_built(synced.generation, took, &diagnostics);
        }
        let previous = &self.loaded().synced.diagnostics;
        if !self.options.quiet && !diagnostics.is_empty() && !same(previous, &diagnostics) {
            eprintln!(
                "admin:  the store data refuses part of what the Admin API changed (the storefront shows the rest):"
            );
            crate::output::print_diagnostics(&diagnostics);
        }
        Loaded {
            store,
            diagnostics: files.diagnostics.clone(),
            fingerprint: files.fingerprint,
            base: files.base.clone(),
            source: files.source.clone(),
            load_options: files.load_options.clone(),
            synced: Applied {
                generation: synced.generation,
                overlay: synced.overlay.clone(),
                diagnostics,
            },
        }
    }

    /// The store of a session with data of its own: the data the files of `loaded` gave, what
    /// the Admin API changed (as `loaded` has it), then `data`. Its problems are those of the files, of the Admin API's
    /// overlay, and of `data` under [`OVERLAY_FILE`].
    pub(super) fn session_store(&self, loaded: &Loaded, data: &Json) -> (Store, Diagnostics) {
        let mut overlays: Vec<Overlay> = loaded
            .synced
            .overlay
            .iter()
            .map(|overlay| Overlay::clone(overlay))
            .collect();
        overlays.push(Overlay::new(OVERLAY_FILE, data.clone()));
        loaded.source.build(&loaded.load_options, &overlays)
    }

    /// Builds the store of the session `id` again when it has data of its own and the Admin
    /// API's changes moved on since it was built. What the data says of the new store is not
    /// told: it was checked when it was set, and the store is built from what is valid.
    fn sync_session(&self, id: &str) {
        let stale = |generation: u64| {
            let sessions = self.sessions.lock().expect("sessions poisoned");
            sessions
                .get(id)
                .and_then(|entry| entry.data.as_ref())
                .filter(|data| data.generation != generation)
                .map(|data| data.data.clone())
        };
        if stale(self.loaded().synced.generation).is_none() {
            return;
        }
        let _rebuilding = lock(&self.rebuilding);
        let loaded = self.loaded();
        let Some(data) = stale(loaded.synced.generation) else {
            return;
        };
        let (store, _) = self.session_store(&loaded, &data);
        let mut sessions = self.sessions.lock().expect("sessions poisoned");
        if let Some(entry) = sessions.get_mut(id)
            && let Some(own) = &mut entry.data
            // The data was set again meanwhile: its store is the one built with it.
            && Arc::ptr_eq(&own.data, &data)
        {
            own.generation = loaded.synced.generation;
            entry.store = Some(Arc::new(store));
        }
    }
}

/// The problems of a store with an overlay that the store of the files alone does not have:
/// those the overlay was refused for, and those it made in the files (a removed entity that a
/// file still names).
fn new_problems(all: Diagnostics, files: &Diagnostics) -> Diagnostics {
    let known: HashSet<String> = files.items.iter().map(ToString::to_string).collect();
    Diagnostics {
        items: all
            .items
            .into_iter()
            .filter(|item| {
                item.file == lsf_admin::sync::OVERLAY_NAME || !known.contains(&item.to_string())
            })
            .collect(),
    }
}

/// Whether two lists of problems say the same, so that the same ones are printed once.
fn same(a: &Diagnostics, b: &Diagnostics) -> bool {
    a.items.len() == b.items.len()
        && a.items
            .iter()
            .zip(&b.items)
            .all(|(a, b)| a.to_string() == b.to_string())
}
