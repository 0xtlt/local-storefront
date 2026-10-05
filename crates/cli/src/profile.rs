//! Profiling a page, for `lsf profile` and for `/__lsf/profile`: what rendered what as a tree
//! of text, and the format of speedscope, for a flame graph. Both in the time the render
//! took here, or in the points of what it would cost a storefront.

use std::fmt::Write;
use std::sync::Arc;
use std::time::Duration;

use lsf_core::Site;
use lsf_core::render::{ProfileOptions, Rendered, Renderer, Target, cost, routes};
use lsf_liquid::profiler::{Event, Frame, Profile};
use serde_json::{Value as Json, json};

/// The renders that come before the ones that are measured: the first ones read and parse
/// the files of the theme.
const WARM_UP: usize = 3;

/// How many times a page is rendered when nothing else is asked for.
pub const DEFAULT_RUNS: u32 = 15;

/// The most renders one profile is made of.
pub const MAX_RUNS: u32 = 1000;

/// What a profile is read in.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Unit {
    /// The time the render took on this machine.
    Time,
    /// What the render would cost a storefront: see [`cost`].
    Points,
}

impl Unit {
    /// Where an event is on the clock of the unit: nanoseconds, or points.
    fn at(self, event: &Event) -> u64 {
        match self {
            Unit::Time => nanos(event.at),
            Unit::Points => event.points,
        }
    }

    /// A value of the unit as a report shows it: milliseconds, or points.
    fn show(self, value: u64) -> String {
        match self {
            Unit::Time => format!("{:.3}", value as f64 / 1e6),
            Unit::Points => value.to_string(),
        }
    }

    /// What is too little to be shown on its own row.
    fn least(self) -> u64 {
        match self {
            Unit::Time => 500,
            Unit::Points => 1,
        }
    }
}

fn nanos(duration: Duration) -> u64 {
    u64::try_from(duration.as_nanos()).unwrap_or(u64::MAX)
}

/// A page rendered several times, and the render in the middle by how long they took: a
/// single render can be slow for reasons of its own.
pub struct Measured {
    pub rendered: Rendered,
    pub profile: Profile,
    pub runs: u32,
    pub fastest: Duration,
    pub slowest: Duration,
    /// Whether tags and outputs were recorded.
    pub lines: bool,
}

pub fn measure(
    renderer: &Renderer,
    site: &Arc<Site>,
    target: &Target,
    options: &ProfileOptions,
    runs: u32,
) -> Measured {
    let runs = runs.clamp(1, MAX_RUNS);
    for _ in 0..WARM_UP {
        renderer.render_page(site, routes::resolve(site), target);
    }
    let mut renders: Vec<(Rendered, Profile)> = (0..runs)
        .map(|_| renderer.profile_page(site, routes::resolve(site), target, options))
        .collect();
    renders.sort_by_key(|(_, profile)| profile.total);
    let fastest = renders[0].1.total;
    let slowest = renders[renders.len() - 1].1.total;
    let (rendered, profile) = renders.swap_remove(renders.len() / 2);
    Measured {
        rendered,
        profile,
        runs,
        fastest,
        slowest,
        lines: options.lines,
    }
}

/// What was measured, said in a few lines, then the report of the profile.
pub fn text(path: &str, measured: &Measured, unit: Unit, all: bool) -> String {
    let template = &measured.rendered.template;
    let mut out = String::new();
    match unit {
        Unit::Time => {
            let ms = |duration: Duration| Unit::Time.show(nanos(duration));
            let _ = writeln!(
                out,
                "{path} \u{b7} template {template} \u{b7} {} ms",
                ms(measured.profile.total)
            );
            let _ = writeln!(
                out,
                "The render in the middle of {}, which took from {} to {} ms.",
                measured.runs,
                ms(measured.fastest),
                ms(measured.slowest)
            );
            out.push_str(
                "Times are in ms, for all the calls of a row. \"own\" leaves out what is under the row.\n",
            );
            if measured.lines {
                out.push_str(
                    "Timing every line makes the render slower: compare the rows with each other.\n",
                );
            }
        }
        Unit::Points => {
            let _ = writeln!(
                out,
                "{path} \u{b7} template {template} \u{b7} {} points",
                measured.profile.points
            );
            out.push_str(
                "Points are a model of what the page costs a Shopify storefront, not a measure:\n\
                 what they are made of is at the end. They are the same at every render.\n\
                 Points are for all the calls of a row. \"own\" leaves out what is under the row.\n",
            );
        }
    }
    if !measured.rendered.errors.is_empty() {
        let _ = writeln!(
            out,
            "The page has {} Liquid error(s).",
            measured.rendered.errors.len()
        );
    }
    out.push('\n');
    out.push_str(&report(&measured.profile, unit, all));
    if unit == Unit::Points {
        out.push('\n');
        out.push_str(&charges(&measured.profile));
    }
    out
}

/// What the points of a profile are made of: how many things of each kind, and their cost.
fn charges(profile: &Profile) -> String {
    let mut out = String::from("What the points are made of:\n\n  points   count   each\n");
    for charge in &profile.charges {
        if charge.count == 0 {
            continue;
        }
        let _ = writeln!(
            out,
            "{:>8} {:>7} {:>6}  {:<10}  {}",
            charge.count * charge.each,
            charge.count,
            charge.each,
            charge.kind,
            cost::describe(&charge.kind),
        );
    }
    out
}

/// What a frame is called: a template, or a line of one.
fn label(frame: &Frame) -> String {
    match frame.line {
        Some(line) => format!("{}:{line}", frame.name),
        None => frame.name.clone(),
    }
}

/// The Liquid file of a frame, when it is one or a line of one. The frames a render names
/// itself (`section <id>`, `block <key>`, a JSON template) have none.
fn file(frame: &Frame) -> Option<String> {
    let is_template = frame.name.contains('/') && !frame.name.contains([' ', '.']);
    is_template.then(|| format!("{}.liquid", frame.name))
}

/// The profile in the file format of speedscope (<https://www.speedscope.app>), which is
/// also what `shopify theme profile --json` prints. The file holds the profile twice, in
/// time and in points: speedscope shows the `active` one and lets the reader change.
pub fn speedscope(profile: &Profile, name: &str, active: Unit) -> Json {
    let frames: Vec<Json> = profile
        .frames
        .iter()
        .map(|frame| {
            let mut entry = json!({ "name": label(frame) });
            if let Some(file) = file(frame) {
                entry["file"] = json!(file);
            }
            if let Some(line) = frame.line {
                entry["line"] = json!(line);
            }
            entry
        })
        .collect();
    let recorded = |unit: Unit, title: &str, unit_name: &str, end: u64| -> Json {
        let events: Vec<Json> = profile
            .events
            .iter()
            .map(|event| {
                json!({
                    "type": if event.open { "O" } else { "C" },
                    "frame": event.frame,
                    "at": unit.at(event),
                })
            })
            .collect();
        json!({
            "type": "evented",
            "name": format!("{name} ({title})"),
            "unit": unit_name,
            "startValue": 0,
            "endValue": end,
            "events": events,
        })
    };
    json!({
        "$schema": "https://www.speedscope.app/file-format-schema.json",
        "name": name,
        "exporter": concat!("lsf ", env!("CARGO_PKG_VERSION")),
        "activeProfileIndex": if active == Unit::Time { 0 } else { 1 },
        "shared": { "frames": frames },
        "profiles": [
            recorded(Unit::Time, "time", "nanoseconds", nanos(profile.total)),
            recorded(Unit::Points, "points", "none", profile.points),
        ],
    })
}

/// A frame under the frame that rendered it: every time it was, counted together.
struct Node {
    frame: usize,
    calls: u64,
    /// In nanoseconds or in points.
    total: u64,
    /// The part of `total` that went to the frames under it.
    inside: u64,
    children: Vec<usize>,
}

impl Node {
    fn new(frame: usize) -> Node {
        Node {
            frame,
            calls: 0,
            total: 0,
            inside: 0,
            children: Vec::new(),
        }
    }

    /// What the frame took itself, without the frames under it.
    fn own(&self) -> u64 {
        self.total.saturating_sub(self.inside)
    }
}

/// The frames of a profile as a tree. The first node stands for the recording itself: the
/// frames that nothing rendered are under it.
fn tree(profile: &Profile, unit: Unit) -> Vec<Node> {
    let mut nodes = vec![Node::new(usize::MAX)];
    // The nodes that are open, with where each one was opened.
    let mut open: Vec<(usize, u64)> = Vec::new();
    for event in &profile.events {
        let parent = open.last().map_or(0, |(node, _)| *node);
        if event.open {
            let known = nodes[parent]
                .children
                .iter()
                .copied()
                .find(|child| nodes[*child].frame == event.frame);
            let node = known.unwrap_or_else(|| {
                nodes.push(Node::new(event.frame));
                let node = nodes.len() - 1;
                nodes[parent].children.push(node);
                node
            });
            open.push((node, unit.at(event)));
        } else if let Some((node, opened)) = open.pop() {
            let took = unit.at(event).saturating_sub(opened);
            nodes[node].calls += 1;
            nodes[node].total += took;
            let parent = open.last().map_or(0, |(node, _)| *node);
            nodes[parent].inside += took;
        }
    }
    nodes
}

/// How many rows the table of own times has, unless every row is asked for.
const SLOWEST: usize = 15;

/// The profile as text: the tree of what rendered what, the heaviest first, then the frames
/// that took the most on their own. Unless `all`, what took less than a hundredth of the
/// render is summed up in one row per parent.
pub fn report(profile: &Profile, unit: Unit, all: bool) -> String {
    let nodes = tree(profile, unit);
    let floor = if all { 0 } else { nodes[0].inside / 100 };
    let mut out = String::new();
    out.push_str("   total      own  calls\n");
    write_children(&mut out, profile, &nodes, unit, 0, 0, floor);

    // Own time or points by frame, wherever it was rendered from.
    let mut own: Vec<(u64, u64, usize)> = Vec::new();
    for node in &nodes[1..] {
        match own.iter_mut().find(|(_, _, frame)| *frame == node.frame) {
            Some((value, calls, _)) => {
                *value += node.own();
                *calls += node.calls;
            }
            None => own.push((node.own(), node.calls, node.frame)),
        }
    }
    own.sort_by(|a, b| b.0.cmp(&a.0).then_with(|| a.2.cmp(&b.2)));
    let shown = if all {
        own.len()
    } else {
        SLOWEST.min(own.len())
    };
    out.push_str(match unit {
        Unit::Time => "\nSlowest on their own:\n\n",
        Unit::Points => "\nCostliest on their own:\n\n",
    });
    out.push_str("     own  calls\n");
    for (value, calls, frame) in &own[..shown] {
        let name = label(&profile.frames[*frame]);
        let _ = writeln!(out, "{:>8} {calls:>6}  {name}", unit.show(*value));
    }
    out
}

fn write_children(
    out: &mut String,
    profile: &Profile,
    nodes: &[Node],
    unit: Unit,
    parent: usize,
    depth: usize,
    floor: u64,
) {
    let mut children = nodes[parent].children.clone();
    children.sort_by(|a, b| {
        let (a, b) = (&nodes[*a], &nodes[*b]);
        b.total.cmp(&a.total).then_with(|| a.frame.cmp(&b.frame))
    });
    let indent = "  ".repeat(depth);
    let (mut hidden, mut hidden_total) = (0, 0);
    for child in children {
        let node = &nodes[child];
        if node.total < floor {
            hidden += 1;
            hidden_total += node.total;
            continue;
        }
        let _ = writeln!(
            out,
            "{:>8} {:>8} {:>6}  {indent}{}",
            unit.show(node.total),
            unit.show(node.own()),
            node.calls,
            label(&profile.frames[node.frame]),
        );
        write_children(out, profile, nodes, unit, child, depth + 1, floor);
    }
    // What rounds to nothing is not worth a row.
    if hidden > 0 && hidden_total >= unit.least() {
        let _ = writeln!(
            out,
            "{:>8} {:>8} {:>6}  {indent}... {hidden} more",
            unit.show(hidden_total),
            "",
            "",
        );
    }
}

#[cfg(test)]
mod tests {
    use lsf_liquid::profiler::{Charge, Event};

    use super::*;

    /// A profile from what happened: `("name", line, at in microseconds, opens)`. One point
    /// is charged every 100 microseconds.
    fn profile(events: &[(&str, Option<u32>, u64, bool)]) -> Profile {
        let mut frames: Vec<Frame> = Vec::new();
        let events = events
            .iter()
            .map(|(name, line, at, open)| {
                let frame = Frame {
                    name: name.to_string(),
                    line: *line,
                };
                let index = frames
                    .iter()
                    .position(|known| *known == frame)
                    .unwrap_or_else(|| {
                        frames.push(frame);
                        frames.len() - 1
                    });
                Event {
                    frame: index,
                    at: Duration::from_micros(*at),
                    points: *at / 100,
                    open: *open,
                }
            })
            .collect::<Vec<_>>();
        let total = events.last().map_or(Duration::ZERO, |event| event.at);
        Profile {
            frames,
            points: events.last().map_or(0, |event| event.points),
            events,
            total,
            charges: vec![
                Charge {
                    kind: "liquid".to_string(),
                    count: 20,
                    each: 1,
                },
                Charge {
                    kind: "product".to_string(),
                    count: 1,
                    each: 20,
                },
                Charge {
                    kind: "metaobject".to_string(),
                    count: 0,
                    each: 20,
                },
            ],
        }
    }

    /// A page of two product cards, each with its price, and a footer.
    fn page() -> Profile {
        profile(&[
            ("render", None, 0, true),
            ("section main", None, 100, true),
            ("sections/main", None, 200, true),
            ("snippets/card", None, 300, true),
            ("snippets/price", None, 400, true),
            ("snippets/price", None, 900, false),
            ("snippets/card", None, 1300, false),
            ("snippets/card", None, 1300, true),
            ("snippets/price", None, 1400, true),
            ("snippets/price", None, 1700, false),
            ("snippets/card", None, 2300, false),
            ("sections/main", None, 2500, false),
            ("section main", None, 2600, false),
            ("section footer", None, 2600, true),
            ("snippets/price", Some(3), 2600, true),
            ("snippets/price", Some(3), 2620, false),
            ("section footer", None, 2640, false),
            ("render", None, 4000, false),
        ])
    }

    #[test]
    fn the_report_is_a_tree_then_the_slowest_frames() {
        assert_eq!(
            report(&page(), Unit::Time, true),
            "   total      own  calls
   4.000    1.460      1  render
   2.500    0.200      1    section main
   2.300    0.300      1      sections/main
   2.000    1.200      2        snippets/card
   0.800    0.800      2          snippets/price
   0.040    0.020      1    section footer
   0.020    0.020      1      snippets/price:3

Slowest on their own:

     own  calls
   1.460      1  render
   1.200      2  snippets/card
   0.800      2  snippets/price
   0.300      1  sections/main
   0.200      1  section main
   0.020      1  section footer
   0.020      1  snippets/price:3
"
        );
    }

    #[test]
    fn what_took_little_is_summed_up() {
        // The floor is a hundredth of the render: 40 microseconds of 4 milliseconds. The
        // footer is at it and stays; what it rendered is under it.
        let report = report(&page(), Unit::Time, false);
        let tree: Vec<&str> = report.lines().take_while(|line| !line.is_empty()).collect();
        let summed_up = format!("{:>8} {:>8} {:>6}      ... 1 more", "0.020", "", "");
        assert_eq!(
            tree[5..],
            [
                "   0.800    0.800      2          snippets/price",
                "   0.040    0.020      1    section footer",
                summed_up.as_str(),
            ]
        );
    }

    #[test]
    fn speedscope_gets_frames_and_events() {
        let exported = speedscope(&page(), "/products/mug", Unit::Time);
        assert_eq!(
            exported["$schema"],
            "https://www.speedscope.app/file-format-schema.json"
        );
        assert_eq!(
            exported["shared"]["frames"],
            json!([
                { "name": "render" },
                { "name": "section main" },
                { "name": "sections/main", "file": "sections/main.liquid" },
                { "name": "snippets/card", "file": "snippets/card.liquid" },
                { "name": "snippets/price", "file": "snippets/price.liquid" },
                { "name": "section footer" },
                { "name": "snippets/price:3", "file": "snippets/price.liquid", "line": 3 },
            ])
        );
        let recorded = &exported["profiles"][0];
        assert_eq!(recorded["type"], "evented");
        assert_eq!(recorded["name"], "/products/mug (time)");
        assert_eq!(recorded["unit"], "nanoseconds");
        assert_eq!(
            (&recorded["startValue"], &recorded["endValue"]),
            (&json!(0), &json!(4_000_000))
        );
        let events = recorded["events"].as_array().unwrap();
        assert_eq!(events.len(), 18);
        assert_eq!(events[1], json!({ "type": "O", "frame": 1, "at": 100_000 }));
        assert_eq!(
            events[17],
            json!({ "type": "C", "frame": 0, "at": 4_000_000 })
        );
    }

    #[test]
    fn the_same_profile_is_read_in_points() {
        // The page was charged a point every 100 microseconds.
        assert_eq!(
            report(&page(), Unit::Points, false),
            "   total      own  calls
      40       15      1  render
      25        2      1    section main
      23        3      1      sections/main
      20       12      2        snippets/card
       8        8      2          snippets/price
       0        0      1    section footer
       0        0      1      snippets/price:3

Costliest on their own:

     own  calls
      15      1  render
      12      2  snippets/card
       8      2  snippets/price
       3      1  sections/main
       2      1  section main
       0      1  section footer
       0      1  snippets/price:3
"
        );
        // What the points are made of: the kinds that were charged, with their cost.
        assert_eq!(
            charges(&page()),
            "What the points are made of:

  points   count   each
      20      20      1  liquid      A tag or an output rendered.
      20       1     20  product     A product loaded.
"
        );
    }

    #[test]
    fn speedscope_gets_the_profile_in_time_and_in_points() {
        let exported = speedscope(&page(), "/products/mug", Unit::Points);
        // The reader changes from one to the other in the viewer: this one is shown first.
        assert_eq!(exported["activeProfileIndex"], 1);
        assert_eq!(
            speedscope(&page(), "/products/mug", Unit::Time)["activeProfileIndex"],
            0
        );
        let profiles = exported["profiles"].as_array().unwrap();
        assert_eq!(profiles.len(), 2);
        let points = &profiles[1];
        assert_eq!(points["name"], "/products/mug (points)");
        assert_eq!(points["unit"], "none");
        assert_eq!(
            (&points["startValue"], &points["endValue"]),
            (&json!(0), &json!(40))
        );
        let events = points["events"].as_array().unwrap();
        assert_eq!(events.len(), 18);
        assert_eq!(events[1], json!({ "type": "O", "frame": 1, "at": 1 }));
        assert_eq!(events[17], json!({ "type": "C", "frame": 0, "at": 40 }));
        // The same frames for both.
        assert_eq!(profiles[0]["events"].as_array().unwrap().len(), 18);
    }

    /// The README gives the cost of each kind of thing: it must say what the code does.
    #[test]
    fn the_readme_lists_the_costs() {
        let readme = include_str!("../../../README.md");
        for (kind, points, what) in cost::KINDS {
            let row = format!("| `{kind}` | {points} | {what} |");
            assert!(readme.contains(&row), "the README has no row:\n{row}");
        }
    }
}
