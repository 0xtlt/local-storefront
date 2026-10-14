//! The tags of standard Liquid.

mod assign;
mod control;
mod iteration;
mod partial;
mod raw;

pub use iteration::{ForLoop, collection_items};
pub use partial::{
    PartialCall, check_render, parse_partial_call, parse_render, print_partial_call,
    strict2_partial_call,
};

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

    // How Shopify's strict parser reads each tag's markup (see `crate::strict2`).
    env.register_syntax("assign", assign::check_assign);
    env.register_syntax("capture", assign::check_capture);
    env.register_syntax("increment", assign::check_counter);
    env.register_syntax("decrement", assign::check_counter);
    env.register_syntax("echo", crate::strict2::check_output);
    env.register_syntax("if", control::check_condition);
    env.register_syntax("elsif", control::check_condition);
    env.register_syntax("unless", control::check_condition);
    env.register_syntax("case", control::check_case);
    env.register_syntax("when", control::check_when);
    env.register_syntax("for", iteration::check_for);
    env.register_syntax("cycle", iteration::check_cycle);
    env.register_syntax("tablerow", iteration::check_tablerow);
    env.register_syntax("render", partial::check_render);
    env.register_syntax("include", partial::check_include);
}
