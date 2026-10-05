//! What a render spends its time in, and what it costs. A render that is given a [`Profiler`]
//! records every template it goes through (a layout, a section, a snippet), each one inside
//! the one that rendered it, and on demand every tag and every output of these templates.
//!
//! Next to the time, a profile counts points: what the host says each thing costs. A tag or
//! an output rendered is one kind of thing ([`NODE_KIND`]); the host charges the others (a
//! product it loads, a search it runs) as they happen. Points say what a render would cost
//! where these things are slow, whatever they take here.

use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::hash::{BuildHasher, Hash};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// The kind of thing a tag or an output rendered is, in the costs of a profiler.
pub const NODE_KIND: &str = "liquid";

/// Something that takes time: a template, a line of one, or what the host names.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Frame {
    /// The name of a template (`snippets/price`), or of what the host measures.
    pub name: String,
    /// The line of the tag or of the output, when the frame is one of them.
    pub line: Option<u32>,
}

/// A frame that starts or ends. A frame ends after every frame that started in it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Event {
    /// The index of the frame in [`Profile::frames`].
    pub frame: usize,
    /// When, from the start of the recording.
    pub at: Duration,
    /// The points charged so far.
    pub points: u64,
    pub open: bool,
}

/// How many things of a kind a render was charged for.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Charge {
    pub kind: String,
    pub count: u64,
    /// The points of one.
    pub each: u64,
}

/// What a [`Profiler`] recorded.
#[derive(Clone, Debug, Default)]
pub struct Profile {
    pub frames: Vec<Frame>,
    pub events: Vec<Event>,
    /// How long the recording lasted.
    pub total: Duration,
    /// The points charged in all.
    pub points: u64,
    /// What the points are made of, in the order of the costs the profiler was given.
    pub charges: Vec<Charge>,
}

/// The frames of one name: the one without a line, and the one of each line.
#[derive(Default)]
struct Named {
    whole: Option<usize>,
    lines: HashMap<u32, usize>,
}

#[derive(Default)]
struct Inner {
    frames: Vec<Frame>,
    by_name: HashMap<String, Named>,
    events: Vec<Event>,
    /// How many things of each kind were charged, as the costs are ordered.
    counts: Vec<u64>,
    /// What was charged once already: a kind and what tells one thing of it from another.
    charged: HashSet<(usize, u64)>,
}

pub struct Profiler {
    started: Instant,
    lines: bool,
    /// The kinds of things that cost points, and the points of one.
    costs: Vec<(String, u64)>,
    /// The points of a tag or of an output.
    node_cost: u64,
    nodes: AtomicU64,
    points: AtomicU64,
    /// Hashes what tells the things of a kind apart.
    hasher: std::collections::hash_map::RandomState,
    inner: Mutex<Inner>,
}

thread_local! {
    /// The profiler of the render this thread is doing, for what is charged from where the
    /// context of the render cannot be reached.
    static ACTIVE: RefCell<Option<Arc<Profiler>>> = const { RefCell::new(None) };
}

/// Makes a profiler the one [`charge`] and [`charge_once`] charge on this thread, until it
/// is dropped.
pub struct Active {
    previous: Option<Arc<Profiler>>,
}

impl Drop for Active {
    fn drop(&mut self) {
        ACTIVE.with(|active| *active.borrow_mut() = self.previous.take());
    }
}

/// Charges the render this thread is profiling for one thing of a kind. Does nothing when
/// the thread profiles nothing.
pub fn charge(kind: &str) {
    ACTIVE.with(|active| {
        if let Some(profiler) = active.borrow().as_ref() {
            profiler.charge(kind, None);
        }
    });
}

/// Charges for a thing the first time only: a product that is loaded once, however many
/// times the templates ask for it. `key` tells it from the other things of its kind.
pub fn charge_once(kind: &str, key: &impl Hash) {
    ACTIVE.with(|active| {
        if let Some(profiler) = active.borrow().as_ref() {
            profiler.charge(kind, Some(profiler.hasher.hash_one(key)));
        }
    });
}

impl Profiler {
    /// A profiler of time alone: nothing costs a point.
    ///
    /// With `lines`, the tags and the outputs of the templates are recorded too. There are
    /// many more of them than of templates, and timing each one makes the render slower:
    /// what such a profile says of the whole is more than the render takes without it.
    pub fn new(lines: bool) -> Arc<Profiler> {
        Profiler::with_costs(lines, &[])
    }

    /// A profiler that counts points too: `costs` are the kinds of things a render is
    /// charged for and the points of one. What is charged of another kind is not counted.
    pub fn with_costs(lines: bool, costs: &[(&str, u64)]) -> Arc<Profiler> {
        Arc::new(Profiler {
            started: Instant::now(),
            lines,
            node_cost: costs
                .iter()
                .find(|(kind, _)| *kind == NODE_KIND)
                .map_or(0, |(_, cost)| *cost),
            costs: costs
                .iter()
                .map(|(kind, cost)| (kind.to_string(), *cost))
                .collect(),
            nodes: AtomicU64::new(0),
            points: AtomicU64::new(0),
            hasher: std::collections::hash_map::RandomState::new(),
            inner: Mutex::new(Inner {
                counts: vec![0; costs.len()],
                ..Inner::default()
            }),
        })
    }

    /// Whether tags and outputs are recorded.
    pub fn lines(&self) -> bool {
        self.lines
    }

    /// Makes this profiler the one of the thread: see [`Active`].
    pub fn activate(self: &Arc<Self>) -> Active {
        Active {
            previous: ACTIVE.with(|active| active.borrow_mut().replace(self.clone())),
        }
    }

    // A render is done by one thread: the counters are only atomic to be shared.
    fn add_points(&self, points: u64) {
        let so_far = self.points.load(Ordering::Relaxed);
        self.points.store(so_far + points, Ordering::Relaxed);
    }

    /// Counts a tag or an output that is rendered.
    pub fn node(&self) {
        let so_far = self.nodes.load(Ordering::Relaxed);
        self.nodes.store(so_far + 1, Ordering::Relaxed);
        self.add_points(self.node_cost);
    }

    /// Charges for one thing of a kind. With a key, only the first time the key is seen.
    fn charge(&self, kind: &str, key: Option<u64>) {
        let Some(index) = self.costs.iter().position(|(known, _)| known == kind) else {
            return;
        };
        let mut inner = self.inner.lock().expect("profiler poisoned");
        if let Some(key) = key
            && !inner.charged.insert((index, key))
        {
            return;
        }
        inner.counts[index] += 1;
        self.add_points(self.costs[index].1);
    }

    /// Starts a frame, which ends when the span is dropped.
    pub fn span(self: &Arc<Self>, name: &str, line: Option<u32>) -> Span {
        let mut inner = self.inner.lock().expect("profiler poisoned");
        let known = inner.by_name.get(name).and_then(|named| match line {
            Some(line) => named.lines.get(&line).copied(),
            None => named.whole,
        });
        let frame = match known {
            Some(frame) => frame,
            None => {
                let frame = inner.frames.len();
                inner.frames.push(Frame {
                    name: name.to_string(),
                    line,
                });
                let named = inner.by_name.entry(name.to_string()).or_default();
                match line {
                    Some(line) => {
                        named.lines.insert(line, frame);
                    }
                    None => named.whole = Some(frame),
                }
                frame
            }
        };
        // Read last, so that the time it took to find the frame is not part of it.
        let at = self.started.elapsed();
        inner.events.push(Event {
            frame,
            at,
            points: self.points.load(Ordering::Relaxed),
            open: true,
        });
        Span {
            profiler: self.clone(),
            frame,
        }
    }

    /// Ends the recording and returns it.
    pub fn finish(&self) -> Profile {
        let total = self.started.elapsed();
        let mut inner = self.inner.lock().expect("profiler poisoned");
        let nodes = self.nodes.load(Ordering::Relaxed);
        let charges = self
            .costs
            .iter()
            .zip(&inner.counts)
            .map(|((kind, each), count)| Charge {
                kind: kind.clone(),
                count: if kind == NODE_KIND { nodes } else { *count },
                each: *each,
            })
            .collect();
        Profile {
            frames: std::mem::take(&mut inner.frames),
            events: std::mem::take(&mut inner.events),
            total,
            points: self.points.load(Ordering::Relaxed),
            charges,
        }
    }
}

/// A frame being recorded. Dropping it ends the frame.
pub struct Span {
    profiler: Arc<Profiler>,
    frame: usize,
}

impl Drop for Span {
    fn drop(&mut self) {
        let at = self.profiler.started.elapsed();
        if let Ok(mut inner) = self.profiler.inner.lock() {
            inner.events.push(Event {
                frame: self.frame,
                at,
                points: self.profiler.points.load(Ordering::Relaxed),
                open: false,
            });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Context, Environment, PartialLoader, Result, Template};

    struct Snippets(Arc<Environment>);

    impl PartialLoader for Snippets {
        fn load(&self, name: &str) -> Result<Arc<Template>> {
            let source = match name {
                "card" => "[{% render 'price' %}]",
                _ => "{{ 1 | plus: 1 }}",
            };
            let name = format!("snippets/{name}");
            Template::parse_named(&self.0, source, Some(&name)).map(Arc::new)
        }
    }

    /// The frames of a profile as they nest: `a(b(c) b(c))`.
    fn shape(profile: &Profile) -> String {
        let mut shape = String::new();
        let mut open = Vec::new();
        let mut last = Duration::ZERO;
        for event in &profile.events {
            assert!(event.at >= last, "{profile:?}");
            last = event.at;
            if event.open {
                if shape.ends_with(')') || shape.chars().last().is_some_and(char::is_alphanumeric) {
                    shape.push(' ');
                }
                let frame = &profile.frames[event.frame];
                shape.push_str(&frame.name);
                if let Some(line) = frame.line {
                    shape.push_str(&format!(":{line}"));
                }
                shape.push('(');
                open.push(event.frame);
            } else {
                assert_eq!(open.pop(), Some(event.frame), "{profile:?}");
                shape.push(')');
            }
        }
        assert!(open.is_empty(), "{profile:?}");
        assert!(last <= profile.total);
        shape.replace("()", "")
    }

    fn render(source: &str, lines: bool) -> (String, Profile) {
        let env = Arc::new(Environment::standard());
        let template = Template::parse_named(&env, source, Some("templates/index")).unwrap();
        let profiler = Profiler::new(lines);
        let mut ctx = Context::builder(env.clone())
            .partials(Arc::new(Snippets(env)))
            .profiler(profiler.clone())
            .build();
        (template.render(&mut ctx), profiler.finish())
    }

    #[test]
    fn templates_are_recorded_inside_the_one_that_renders_them() {
        let source = "{% for i in (1..2) %}{% render 'card' %}{% endfor %}\n{% render 'price' %}";
        let (output, profile) = render(source, false);
        assert_eq!(output, "[2][2]\n2");
        assert_eq!(
            shape(&profile),
            "templates/index(snippets/card(snippets/price) snippets/card(snippets/price) \
             snippets/price)"
        );
        // One frame per template, however many times it is rendered.
        assert_eq!(profile.frames.len(), 3);
    }

    #[test]
    fn lines_are_recorded_on_demand() {
        let source = "{% if true %}\n{{ 'a' }}{% render 'price' %}\n{% endif %}";
        let (output, profile) = render(source, true);
        assert_eq!(output, "\na2\n");
        assert_eq!(
            shape(&profile),
            "templates/index(templates/index:1(templates/index:2 \
             templates/index:2(snippets/price(snippets/price:1))))"
        );
    }

    #[test]
    fn points_are_charged_to_the_frame_they_happen_in() {
        let env = Arc::new(Environment::standard());
        let source = "{% render 'card' %}{% render 'price' %}";
        let template = Template::parse_named(&env, source, Some("templates/index")).unwrap();
        let costs = [(NODE_KIND, 1), ("product", 20), ("search", 100)];
        let profiler = Profiler::with_costs(false, &costs);
        let mut ctx = Context::builder(env.clone())
            .partials(Arc::new(Snippets(env)))
            .profiler(profiler.clone())
            .build();
        {
            let _active = profiler.activate();
            // Before any frame: the same product twice, and another.
            charge_once("product", &7);
            charge_once("product", &7);
            charge_once("product", &8);
            // Every search costs, and what has no cost is not counted.
            charge("search");
            charge("search");
            charge("unknown");
            template.render(&mut ctx);
        }
        // Nothing is charged once the profiler is not the one of the thread any more.
        charge("search");
        let profile = profiler.finish();
        assert_eq!(
            profile.charges,
            [
                // Two `render` in the template, one in `card`, one output in each `price`.
                Charge {
                    kind: "liquid".to_string(),
                    count: 5,
                    each: 1
                },
                Charge {
                    kind: "product".to_string(),
                    count: 2,
                    each: 20
                },
                Charge {
                    kind: "search".to_string(),
                    count: 2,
                    each: 100
                },
            ]
        );
        assert_eq!(profile.points, 5 + 40 + 200);
        // The clock of points of each frame: what was charged before it, then what is in it.
        let points: Vec<(String, bool, u64)> = profile
            .events
            .iter()
            .map(|event| {
                let name = profile.frames[event.frame].name.clone();
                (name, event.open, event.points)
            })
            .collect();
        let at = |name: &str, open: bool, points: u64| (name.to_string(), open, points);
        assert_eq!(
            points,
            [
                at("templates/index", true, 240),
                at("snippets/card", true, 241),
                at("snippets/price", true, 242),
                at("snippets/price", false, 243),
                at("snippets/card", false, 243),
                at("snippets/price", true, 244),
                at("snippets/price", false, 245),
                at("templates/index", false, 245),
            ]
        );
    }

    #[test]
    fn a_render_without_a_profiler_records_nothing() {
        let env = Arc::new(Environment::standard());
        let template = Template::parse_named(&env, "{{ 1 }}", Some("templates/index")).unwrap();
        let mut ctx = Context::builder(env).build();
        assert!(ctx.profiler().is_none());
        assert_eq!(template.render(&mut ctx), "1");
    }
}
