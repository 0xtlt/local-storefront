//! Per-render state shared by tags and filters through the Liquid context's registers.

use std::sync::{Arc, Mutex};

use slt_liquid::{Context, Error, Result, Value};

use super::page::Page;
use crate::site::Site;
use crate::theme::Layout;

#[derive(Default)]
struct Inner {
    /// Set by `{% layout %}` in a Liquid template.
    layout_override: Option<Layout>,
    /// `Link` headers requested by `preload_tag` and `image_tag: preload: true`.
    preloads: Vec<String>,
    /// How many sections have been rendered for the current location (`section.index`).
    section_counts: Vec<(String, usize)>,
    /// Nesting of `content_for`, to stop runaway recursion.
    block_depth: usize,
}

pub struct RenderState {
    pub site: Arc<Site>,
    pub page: Page,
    inner: Mutex<Inner>,
}

/// The deepest blocks can nest. Shopify allows 8 levels.
pub const MAX_BLOCK_DEPTH: usize = 16;

impl RenderState {
    pub fn new(site: Arc<Site>, page: Page) -> Self {
        RenderState {
            site,
            page,
            inner: Mutex::new(Inner::default()),
        }
    }

    /// The state of the render a tag or filter runs in.
    pub fn of(ctx: &Context) -> Result<&RenderState> {
        ctx.register::<RenderState>()
            .ok_or_else(|| Error::standard("this tag or filter needs a theme to render"))
    }

    fn inner(&self) -> std::sync::MutexGuard<'_, Inner> {
        self.inner.lock().expect("render state poisoned")
    }

    pub fn set_layout(&self, layout: Layout) {
        self.inner().layout_override = Some(layout);
    }

    pub fn layout_override(&self) -> Option<Layout> {
        self.inner().layout_override.clone()
    }

    pub fn add_preload(&self, header: String) {
        let mut inner = self.inner();
        if !inner.preloads.contains(&header) {
            inner.preloads.push(header);
        }
    }

    pub fn preloads(&self) -> Vec<String> {
        self.inner().preloads.clone()
    }

    /// The 1-based index of the next section rendered in a location (`template`, `header`, ...).
    pub fn next_section_index(&self, location: &str) -> usize {
        let mut inner = self.inner();
        match inner
            .section_counts
            .iter_mut()
            .find(|(name, _)| name == location)
        {
            Some((_, count)) => {
                *count += 1;
                *count
            }
            None => {
                inner.section_counts.push((location.to_string(), 1));
                1
            }
        }
    }

    /// Enters a nested block render; fails when blocks nest unreasonably deep.
    pub fn enter_block(&self) -> Result<()> {
        let mut inner = self.inner();
        if inner.block_depth >= MAX_BLOCK_DEPTH {
            return Err(Error::standard("blocks are nested too deeply"));
        }
        inner.block_depth += 1;
        Ok(())
    }

    pub fn leave_block(&self) {
        let mut inner = self.inner();
        inner.block_depth = inner.block_depth.saturating_sub(1);
    }
}

/// A value from a keyword argument list, by name.
pub fn named<'a>(pairs: &'a [(String, Value)], key: &str) -> Option<&'a Value> {
    pairs
        .iter()
        .rev()
        .find(|(name, _)| name == key)
        .map(|(_, value)| value)
}
