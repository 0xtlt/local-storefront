//! The set of tags and filters templates are parsed and rendered with.

use std::collections::HashMap;

use crate::context::Context;
use crate::error::Result;
use crate::parser::{Parser, TagToken};
use crate::template::Tag;
use crate::value::Value;
use crate::variable::FilterArgs;

/// A filter: `input | name: args`.
pub type FilterFn = fn(&Value, &FilterArgs, &Context) -> Result<Value>;

/// Builds a tag from its markup. Block tags pull their body from the parser.
pub type TagParser = fn(&mut Parser<'_, '_>, &TagToken<'_>) -> Result<Box<dyn Tag>>;

#[derive(Clone, Default)]
pub struct Environment {
    tags: HashMap<String, TagParser>,
    filters: HashMap<String, FilterFn>,
    bug_compatible_whitespace_trimming: bool,
}

impl Environment {
    /// An environment without any tag or filter.
    pub fn empty() -> Self {
        Environment::default()
    }

    /// The tags and filters of standard Liquid.
    pub fn standard() -> Self {
        let mut env = Environment::default();
        crate::tags::register(&mut env);
        crate::filters::register(&mut env);
        env
    }

    /// Trims whitespace as Shopify's storefronts do rather than as the language says, like
    /// the reference implementation's parse option of the same name.
    ///
    /// `{{-` and `{%-` remove the whitespace before them. When the text before one is nothing
    /// but whitespace, a storefront keeps its first character: in `{% if a %}\n  {{- b }}`
    /// the line break stays. This started as a bug of the renderer Shopify ran themes with,
    /// and themes came to rely on it, a `robots.txt.liquid` for the line breaks of its rules.
    pub fn set_bug_compatible_whitespace_trimming(&mut self, enabled: bool) {
        self.bug_compatible_whitespace_trimming = enabled;
    }

    pub fn bug_compatible_whitespace_trimming(&self) -> bool {
        self.bug_compatible_whitespace_trimming
    }

    pub fn register_tag(&mut self, name: &str, parser: TagParser) {
        self.tags.insert(name.to_string(), parser);
    }

    pub fn register_filter(&mut self, name: &str, filter: FilterFn) {
        self.filters.insert(name.to_string(), filter);
    }

    pub fn tag(&self, name: &str) -> Option<TagParser> {
        self.tags.get(name).copied()
    }

    pub fn filter(&self, name: &str) -> Option<FilterFn> {
        self.filters.get(name).copied()
    }

    pub fn tag_names(&self) -> Vec<&str> {
        let mut names: Vec<&str> = self.tags.keys().map(String::as_str).collect();
        names.sort_unstable();
        names
    }

    pub fn filter_names(&self) -> Vec<&str> {
        let mut names: Vec<&str> = self.filters.keys().map(String::as_str).collect();
        names.sort_unstable();
        names
    }
}
