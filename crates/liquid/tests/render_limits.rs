//! The render budget: a template whose loops never end stops with Ruby Liquid's resource limit
//! error, said once, instead of holding its thread for ever.

use std::sync::Arc;
use std::time::{Duration, Instant};

use lsf_liquid::{Context, Environment, RenderLimits, Template};

fn render(source: &str, limits: RenderLimits) -> (String, Context, Duration) {
    let env = Arc::new(Environment::standard());
    let template = Template::parse(&env, source).expect("the template parses");
    let mut ctx = Context::builder(env).limits(limits).build();
    let started = Instant::now();
    let out = template.render(&mut ctx);
    (out, ctx, started.elapsed())
}

/// `(1..1000000000)` nested never answered, and allocated every item of the range first.
#[test]
fn loops_that_never_end_stop_at_the_budget() {
    let (out, ctx, took) = render(
        "before{% for i in (1..1000000000) %}{% for j in (1..1000000000) %}{{ j }}{% endfor %}{% endfor %}after",
        RenderLimits::default(),
    );
    assert!(took < Duration::from_secs(2), "{took:?}");
    assert_eq!(
        out.matches("Liquid error").count(),
        1,
        "said once: {}",
        &out[..out.len().min(300)]
    );
    assert!(out.contains("Memory limits exceeded"));
    assert!(out.starts_with("before"));
    assert!(!out.ends_with("after"), "nothing renders after it");
    assert!(ctx.budget_exceeded());
    assert_eq!(ctx.errors().len(), 1);
}

#[test]
fn a_render_stops_at_its_score() {
    let (out, ctx, _) = render(
        "{% for i in (1..300) %}{% for j in (1..300) %}.{% endfor %}{% endfor %}",
        RenderLimits {
            score: 10_000,
            time: None,
        },
    );
    assert!(ctx.budget_exceeded());
    assert!(
        out.ends_with("Liquid error (line 1): Memory limits exceeded"),
        "{out}"
    );
    // Each pass of the outer loop scores 600: the 300 items of the inner range and the node of
    // each of its 300 bodies.
    let dots = out.matches('.').count();
    assert!((4_500..=5_100).contains(&dots), "{dots} dots");
}

#[test]
fn a_render_stops_at_its_time() {
    let (out, ctx, took) = render(
        "{% for i in (1..5000) %}{% for j in (1..5000) %}{% assign x = j %}{% endfor %}{% endfor %}done",
        RenderLimits {
            score: u64::MAX,
            time: Some(Duration::from_millis(100)),
        },
    );
    assert!(ctx.budget_exceeded(), "{took:?}");
    assert!(took < Duration::from_secs(2), "{took:?}");
    assert!(out.contains("Memory limits exceeded"));
    assert!(!out.contains("done"));
}

#[test]
fn a_render_within_its_budget_is_untouched() {
    let (out, ctx, _) = render(
        "{% for i in (1..3) %}{% if i > 1 %},{% endif %}{{ i }}{% endfor %}{% tablerow n in (1..2) %}{{ n }}{% endtablerow %}",
        RenderLimits::default(),
    );
    assert!(out.starts_with("1,2,3<tr class=\"row1\">"), "{out}");
    assert!(!ctx.budget_exceeded());
    assert!(ctx.errors().is_empty());
    assert!(ctx.render_score() > 0);
}
