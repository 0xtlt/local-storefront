//! A Liquid template engine that mirrors the behaviour of Shopify's reference implementation
//! (the `liquid` Ruby gem): lax parsing, whitespace control, the blank-block rule, Ruby number
//! and string semantics, and error messages rendered inline.
//!
//! ```
//! use std::sync::Arc;
//! use lsf_liquid::{Context, Environment, Template};
//!
//! let env = Arc::new(Environment::standard());
//! let template = Template::parse(&env, "{{ 'hello' | upcase }} {{ 0.1 | plus: 0.2 }}").unwrap();
//! let mut ctx = Context::builder(env).build();
//! assert_eq!(template.render(&mut ctx), "HELLO 0.3");
//! ```

pub mod condition;
pub mod context;
pub mod environment;
pub mod error;
pub mod expr;
pub mod filters;
mod lax;
pub mod lexer;
pub mod number;
pub mod parser;
pub mod profiler;
pub mod tags;
pub mod template;
pub mod time;
pub mod tokenizer;
pub mod value;
pub mod variable;

pub use context::{Context, ContextBuilder, Interrupt, PartialLoader, SelfDrop};
pub use environment::{Environment, FilterFn, TagParser};
pub use error::{Error, ErrorKind, Result};
pub use expr::Expr;
pub use parser::{EndTag, Parser, TagToken};
pub use profiler::{Profile, Profiler};
pub use template::{BlockBody, Node, Tag, Template};
pub use value::{Hash, Object, Value};
pub use variable::{FilterArgs, Variable};

/// Scanners for the loosely specified parts of tag markup, exposed for custom tags.
pub mod markup {
    pub use crate::lax::{
        find_quoted_fragment, is_blank, lstrip, quoted_fragment, rstrip, scan_tag_attributes,
        skip_space, strip,
    };
}
