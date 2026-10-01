//! The tags of standard Liquid.

mod assign;
mod control;
mod iteration;
mod partial;
mod raw;

pub use iteration::{ForLoop, collection_items};
pub use partial::{PartialCall, parse_partial_call, parse_render};

use crate::environment::Environment;

pub(crate) fn register(env: &mut Environment) {
    env.register_tag("assign", assign::parse_assign);
    env.register_tag("capture", assign::parse_capture);
    env.register_tag("increment", assign::parse_increment);
    env.register_tag("decrement", assign::parse_decrement);
    env.register_tag("echo", assign::parse_echo);

    env.register_tag("if", control::parse_if);
    env.register_tag("unless", control::parse_unless);
    env.register_tag("case", control::parse_case);
    env.register_tag("ifchanged", control::parse_ifchanged);

    env.register_tag("for", iteration::parse_for);
    env.register_tag("break", iteration::parse_break);
    env.register_tag("continue", iteration::parse_continue);
    env.register_tag("cycle", iteration::parse_cycle);
    env.register_tag("tablerow", iteration::parse_tablerow);

    env.register_tag("render", partial::parse_render);
    env.register_tag("include", partial::parse_include);

    env.register_tag("raw", raw::parse_raw);
    env.register_tag("comment", raw::parse_comment);
    env.register_tag("doc", raw::parse_doc);
    env.register_tag("#", raw::parse_inline_comment);
}
