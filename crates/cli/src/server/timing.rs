//! How long the server took. Each response says it in its `Server-Timing` header, where a
//! Shopify storefront says it and where browsers look for it, and `/__lsf/timings` adds up
//! the renders so far.

use std::collections::HashMap;
use std::sync::Mutex;
use std::time::Duration;

use lsf_core::render::{Rendered, Target, Timings};
use serde_json::{Value as Json, json};

use super::reply::Reply;

const HEADER: &str = "server-timing";

/// A duration in milliseconds, to the microsecond, written as short as it can be.
fn milliseconds(duration: Duration) -> String {
    let text = format!("{:.3}", duration.as_secs_f64() * 1000.0);
    text.trim_end_matches('0').trim_end_matches('.').to_string()
}

/// One entry of the header: `name;dur=1.5`, with a description when there is one.
fn entry(name: &str, duration: Duration, description: Option<&str>) -> String {
    let mut entry = format!("{name};dur={}", milliseconds(duration));
    if let Some(description) = description {
        entry.push_str(";desc=\"");
        for character in description.chars() {
            match character {
                '"' | '\\' => entry.extend(['\\', character]),
                // A header carries nothing else.
                _ if character.is_ascii() && !character.is_ascii_control() => entry.push(character),
                _ => entry.push('?'),
            }
        }
        entry.push('"');
    }
    entry
}

/// The most the header says about the sections and the blocks of a render. Node reads 16 KB
/// of headers and no more, all of them together: a page of many blocks must stay one that a
/// test can fetch.
const DETAIL_BUDGET: usize = 6 * 1024;

/// The entries of a render: the whole of it, then its template and its layout when a page was
/// rendered. With `detail`, every section as well, described by its type and its id, and
/// after each one its theme blocks.
pub fn of_render(timings: &Timings, detail: bool) -> String {
    let mut entries = vec![entry("render", timings.total, None)];
    if let Some(template) = timings.template {
        entries.push(entry("template", template, None));
    }
    if let Some(layout) = timings.layout {
        entries.push(entry("layout", layout, None));
    }
    if detail {
        entries.extend(of_sections(timings));
    }
    entries.join(", ")
}

/// A name after a type, unless they are the same: a section rendered by name
/// (`{% section 'header' %}`) has its type for id, and a static block often has its type
/// for key.
fn described(kind: &str, name: &str) -> String {
    if name == kind {
        kind.to_string()
    } else {
        format!("{kind} {name}")
    }
}

/// One entry per section, and under each one entry per theme block: a dash for each level
/// it is nested at, its type, its key, and how many times it was rendered when more than
/// once. The time of a block is the time of all of these. When there are more blocks than
/// the header has room for, the fastest are left out, and an entry says how many.
fn of_sections(timings: &Timings) -> Vec<String> {
    // Each entry, with its time when it is one of a block.
    let mut entries: Vec<(String, Option<Duration>)> = Vec::new();
    for section in &timings.sections {
        let description = described(&section.kind, &section.id);
        entries.push((entry("section", section.duration, Some(&description)), None));
        for block in &section.blocks {
            let total = block.durations.iter().sum();
            let mut description = "- ".repeat(block.depth);
            description.push_str(&described(&block.kind, &block.key));
            if block.durations.len() > 1 {
                description.push_str(&format!(" x{}", block.durations.len()));
            }
            entries.push((entry("block", total, Some(&description)), Some(total)));
        }
    }

    let separator = ", ".len();
    let mut size: usize = entries.iter().map(|(text, _)| text.len() + separator).sum();
    let mut fastest: Vec<(Duration, usize)> = entries
        .iter()
        .enumerate()
        .filter_map(|(index, (_, block))| Some(((*block)?, index)))
        .collect();
    fastest.sort();
    let mut omitted = vec![false; entries.len()];
    for (_, index) in fastest {
        if size <= DETAIL_BUDGET {
            break;
        }
        size -= entries[index].0.len() + separator;
        omitted[index] = true;
    }
    let count = omitted.iter().filter(|omitted| **omitted).count();
    let mut kept: Vec<String> = entries
        .into_iter()
        .zip(omitted)
        .filter(|(_, omitted)| !omitted)
        .map(|((text, _), _)| text)
        .collect();
    if count > 0 {
        kept.push(format!(
            "omitted;desc=\"the {count} fastest blocks: see /__lsf/timings\""
        ));
    }
    kept
}

/// Writes the header of a response: what the request took as a whole, under the name Shopify
/// gives it, then what the handler said about its render, then what was done to the response
/// after it. The delay of a throttle is named, and is not part of `processing`.
pub fn complete(
    mut reply: Reply,
    processing: Duration,
    compress: Option<Duration>,
    throttle: Duration,
) -> Reply {
    let mut entries = vec![entry("processing", processing, None)];
    if let Some(index) = reply.headers.iter().position(|(name, _)| name == HEADER) {
        entries.push(reply.headers.remove(index).1);
    }
    if let Some(compress) = compress {
        entries.push(entry("compress", compress, None));
    }
    if !throttle.is_zero() {
        entries.push(entry("throttle", throttle, None));
    }
    reply.header(HEADER, entries.join(", "))
}

/// How many buckets share each power of two: a bucket is 1/32 of its lower bound wide, so the
/// middle of one is within 1.6% of every duration in it.
const STEPS: u64 = 32;
const STEP_BITS: u32 = STEPS.trailing_zeros();

/// The bucket of a duration in nanoseconds. Under 64 ns each one has its own.
fn bucket(nanos: u64) -> usize {
    if nanos < STEPS {
        return nanos as usize;
    }
    let shift = nanos.ilog2() - STEP_BITS;
    // The leading bits of the duration: 32 to 63.
    let leading = nanos >> shift;
    (u64::from(shift) * STEPS + leading) as usize
}

/// The shortest duration of a bucket, in nanoseconds.
fn lower_bound(bucket: usize) -> u64 {
    let bucket = bucket as u64;
    if bucket < STEPS {
        return bucket;
    }
    (STEPS + bucket % STEPS) << (bucket / STEPS - 1)
}

/// The durations of one thing. They are counted in buckets rather than kept, so that a server
/// which renders for days stays as small as one which just started.
#[derive(Default)]
struct Durations {
    count: u64,
    total: Duration,
    min: Duration,
    max: Duration,
    /// How many durations fell in each bucket. As long as the longest one needs.
    buckets: Vec<u32>,
}

impl Durations {
    fn add(&mut self, duration: Duration) {
        self.min = if self.count == 0 {
            duration
        } else {
            self.min.min(duration)
        };
        self.max = self.max.max(duration);
        self.count += 1;
        self.total += duration;
        let bucket = bucket(u64::try_from(duration.as_nanos()).unwrap_or(u64::MAX));
        if bucket >= self.buckets.len() {
            self.buckets.resize(bucket + 1, 0);
        }
        self.buckets[bucket] = self.buckets[bucket].saturating_add(1);
    }

    /// The duration that `fraction` of them do not exceed: the middle of its bucket, unless
    /// that is outside of what was seen.
    fn percentile(&self, fraction: f64) -> Duration {
        let rank = ((self.count as f64 * fraction).ceil() as u64).max(1);
        let mut seen = 0;
        for (bucket, count) in self.buckets.iter().enumerate() {
            seen += u64::from(*count);
            if seen >= rank {
                let low = lower_bound(bucket);
                let width = lower_bound(bucket + 1).saturating_sub(low);
                return Duration::from_nanos(low + width / 2).clamp(self.min, self.max);
            }
        }
        self.max
    }

    /// Adds the figures to an entry of the report, in milliseconds.
    fn report(&self, mut entry: Json) -> Json {
        let ms = |duration: Duration| (duration.as_secs_f64() * 1e6).round() / 1e3;
        let mean = self.total.as_nanos() / u128::from(self.count.max(1));
        let mean = Duration::from_nanos(u64::try_from(mean).unwrap_or(u64::MAX));
        entry["count"] = json!(self.count);
        entry["total"] = json!(ms(self.total));
        entry["mean"] = json!(ms(mean));
        entry["p50"] = json!(ms(self.percentile(0.5)));
        entry["p95"] = json!(ms(self.percentile(0.95)));
        entry["min"] = json!(ms(self.min));
        entry["max"] = json!(ms(self.max));
        entry
    }
}

/// A theme block and every time it was rendered.
struct Block {
    kind: String,
    /// The id of its section.
    section: String,
    durations: Durations,
}

#[derive(Default)]
struct Inner {
    /// The pages rendered, by template.
    templates: HashMap<String, Durations>,
    /// The sections rendered, by id, with their type.
    sections: HashMap<String, (String, Durations)>,
    /// The theme blocks rendered, by id.
    blocks: HashMap<String, Block>,
}

/// How long the templates, the sections and the theme blocks took since the server started,
/// or since the figures were last cleared.
#[derive(Default)]
pub struct Totals(Mutex<Inner>);

/// The entries of a report, what took the most time in all first.
fn ranked(mut entries: Vec<(Duration, &String, Json)>) -> Json {
    entries.sort_by(|a, b| b.0.cmp(&a.0).then_with(|| a.1.cmp(b.1)));
    entries.into_iter().map(|(_, _, entry)| entry).collect()
}

impl Totals {
    /// Counts a render: its sections and their blocks, and its template when a whole page
    /// was rendered.
    pub fn add(&self, rendered: &Rendered, target: &Target) {
        let mut inner = self.0.lock().expect("timings poisoned");
        if *target == Target::Page {
            inner
                .templates
                .entry(rendered.template.clone())
                .or_default()
                .add(rendered.timings.total);
        }
        for section in &rendered.timings.sections {
            inner
                .sections
                .entry(section.id.clone())
                .or_insert_with(|| (section.kind.clone(), Durations::default()))
                .1
                .add(section.duration);
            for block in &section.blocks {
                let durations = &mut inner
                    .blocks
                    .entry(block.id.clone())
                    .or_insert_with(|| Block {
                        kind: block.kind.clone(),
                        section: section.id.clone(),
                        durations: Durations::default(),
                    })
                    .durations;
                for duration in &block.durations {
                    durations.add(*duration);
                }
            }
        }
    }

    pub fn clear(&self) {
        *self.0.lock().expect("timings poisoned") = Inner::default();
    }

    /// The report of `/__lsf/timings`.
    pub fn to_json(&self) -> Json {
        let inner = self.0.lock().expect("timings poisoned");
        let templates = inner
            .templates
            .iter()
            .map(|(template, durations)| {
                let entry = durations.report(json!({ "template": template }));
                (durations.total, template, entry)
            })
            .collect();
        let sections = inner
            .sections
            .iter()
            .map(|(id, (kind, durations))| {
                let entry = durations.report(json!({ "id": id, "type": kind }));
                (durations.total, id, entry)
            })
            .collect();
        let blocks = inner
            .blocks
            .iter()
            .map(|(id, block)| {
                let named = json!({ "id": id, "type": block.kind, "section": block.section });
                (block.durations.total, id, block.durations.report(named))
            })
            .collect();
        json!({
            "templates": ranked(templates),
            "sections": ranked(sections),
            "blocks": ranked(blocks),
        })
    }
}

#[cfg(test)]
mod tests {
    use lsf_core::render::{BlockTiming, SectionTiming};

    use super::*;

    #[test]
    fn durations_are_written_in_milliseconds() {
        let micros = Duration::from_micros;
        assert_eq!(entry("render", micros(60_000), None), "render;dur=60");
        assert_eq!(entry("render", micros(14_732), None), "render;dur=14.732");
        assert_eq!(entry("render", micros(100), None), "render;dur=0.1");
        assert_eq!(entry("render", Duration::ZERO, None), "render;dur=0");
        assert_eq!(
            entry("section", micros(1500), Some("hero \"é\\")),
            "section;dur=1.5;desc=\"hero \\\"?\\\\\""
        );
    }

    #[test]
    fn the_header_names_the_request_then_the_render() {
        let ms = Duration::from_millis;
        let reply = Reply::html(200, "page").header(HEADER, "render;dur=3");
        let reply = complete(reply, ms(5), Some(ms(1)), ms(300));
        let headers: Vec<&str> = reply
            .headers
            .iter()
            .filter(|(name, _)| name == HEADER)
            .map(|(_, value)| value.as_str())
            .collect();
        assert_eq!(
            headers,
            ["processing;dur=5, render;dur=3, compress;dur=1, throttle;dur=300"]
        );

        // What renders nothing, is not compressed and is not delayed.
        let reply = complete(Reply::text(200, "ok"), ms(2), None, Duration::ZERO);
        assert_eq!(reply.headers.last().unwrap().1, "processing;dur=2");
    }

    fn block(key: &str, kind: &str, depth: usize, micros: &[u64]) -> BlockTiming {
        BlockTiming {
            id: format!("Aabc__{key}"),
            key: key.to_string(),
            kind: kind.to_string(),
            depth,
            durations: micros.iter().copied().map(Duration::from_micros).collect(),
        }
    }

    fn section(id: &str, kind: &str, micros: u64, blocks: Vec<BlockTiming>) -> SectionTiming {
        SectionTiming {
            id: id.to_string(),
            kind: kind.to_string(),
            duration: Duration::from_micros(micros),
            blocks,
        }
    }

    #[test]
    fn sections_are_followed_by_their_blocks() {
        let timings = Timings {
            total: Duration::from_micros(9000),
            template: Some(Duration::from_micros(6000)),
            layout: Some(Duration::from_micros(2000)),
            sections: vec![
                section(
                    "template--1__list",
                    "product-list",
                    5000,
                    vec![
                        // Rendered once per product, with the blocks it holds.
                        block("card", "product-card", 1, &[1000, 1500, 500]),
                        block("price", "price", 2, &[100, 200, 100]),
                        block("text_x1", "text", 1, &[250]),
                    ],
                ),
                section("footer", "footer", 300, Vec::new()),
            ],
        };
        assert_eq!(
            of_render(&timings, false),
            "render;dur=9, template;dur=6, layout;dur=2"
        );
        assert_eq!(
            of_render(&timings, true).split(", ").collect::<Vec<_>>(),
            [
                "render;dur=9",
                "template;dur=6",
                "layout;dur=2",
                "section;dur=5;desc=\"product-list template--1__list\"",
                "block;dur=3;desc=\"- product-card card x3\"",
                "block;dur=0.4;desc=\"- - price x3\"",
                "block;dur=0.25;desc=\"- text text_x1\"",
                "section;dur=0.3;desc=\"footer\"",
            ]
        );
    }

    #[test]
    fn a_page_of_many_blocks_keeps_a_header_that_can_be_read() {
        // The later a block comes, the longer it took.
        let blocks = (0..500)
            .map(|index| block(&format!("text_{index:03}"), "text", 1, &[index + 1]))
            .collect();
        let timings = Timings {
            total: Duration::from_millis(200),
            template: None,
            layout: None,
            sections: vec![section("main", "main", 150_000, blocks)],
        };
        let header = of_render(&timings, true);
        assert!(header.len() <= DETAIL_BUDGET + 200, "{}", header.len());
        let entries: Vec<&str> = header.split(", ").collect();
        // The section stays, and the slowest blocks, in the order they were rendered.
        assert_eq!(entries[1], "section;dur=150;desc=\"main\"");
        assert_eq!(
            entries[entries.len() - 2],
            "block;dur=0.5;desc=\"- text text_499\""
        );
        let kept = entries.len() - 3;
        assert!((100..500).contains(&kept), "{kept}");
        assert_eq!(
            entries[2],
            format!(
                "block;dur={};desc=\"- text text_{}\"",
                milliseconds(Duration::from_micros(501 - kept as u64)),
                500 - kept
            )
        );
        // And the header says what it leaves out.
        assert_eq!(
            entries[entries.len() - 1],
            format!(
                "omitted;desc=\"the {} fastest blocks: see /__lsf/timings\"",
                500 - kept
            )
        );
    }

    #[test]
    fn buckets_follow_each_other() {
        for nanos in 0..4096 {
            let index = bucket(nanos);
            assert!(lower_bound(index) <= nanos, "{nanos}");
            assert!(nanos < lower_bound(index + 1), "{nanos}");
        }
        for index in 0..1900 {
            let (low, high) = (lower_bound(index), lower_bound(index + 1));
            assert!(low < high, "{index}");
            assert_eq!(bucket(low), index);
            assert_eq!(bucket(high - 1), index);
            // No wider than 1/32 of what it holds.
            assert!((high - low) * STEPS <= low.max(STEPS), "{index}");
        }
        assert_eq!(bucket(u64::MAX), 1919);
    }

    #[test]
    fn percentiles_are_close_to_the_durations() {
        let micros = Duration::from_micros;
        let mut durations = Durations::default();
        // 1 to 1000 microseconds, in an order of no meaning.
        for index in 0..1000u64 {
            durations.add(micros(index * 387 % 1000 + 1));
        }
        assert_eq!(durations.count, 1000);
        assert_eq!((durations.min, durations.max), (micros(1), micros(1000)));
        assert_eq!(durations.total, micros(500_500));
        for (fraction, expected) in [(0.5, 500.0), (0.95, 950.0), (0.99, 990.0)] {
            let found = durations.percentile(fraction).as_secs_f64() * 1e6;
            assert!(
                (found - expected).abs() <= expected * 0.02,
                "{found} for {expected}"
            );
        }
        // Never outside of what was seen.
        let mut one = Durations::default();
        one.add(micros(1234));
        assert_eq!(one.percentile(0.5), micros(1234));
        assert_eq!(one.percentile(0.95), micros(1234));
        // A day-long one does not make the others coarser.
        durations.add(Duration::from_secs(86_400));
        assert_eq!(durations.percentile(1.0), Duration::from_secs(86_400));
        let median = durations.percentile(0.5).as_secs_f64() * 1e6;
        assert!((median - 500.0).abs() <= 10.0, "{median}");
    }

    #[test]
    fn the_report_ranks_what_took_the_most_time() {
        let mut slow = Durations::default();
        slow.add(Duration::from_micros(2500));
        slow.add(Duration::from_micros(1500));
        let mut fast = Durations::default();
        fast.add(Duration::from_micros(10));
        let (a, b) = ("a".to_string(), "b".to_string());
        let report = ranked(vec![
            (fast.total, &a, fast.report(json!({ "template": "a" }))),
            (slow.total, &b, slow.report(json!({ "template": "b" }))),
        ]);
        assert_eq!(
            report,
            json!([
                { "template": "b", "count": 2, "total": 4.0, "mean": 2.0, "p50": 1.5,
                  "p95": 2.5, "min": 1.5, "max": 2.5 },
                { "template": "a", "count": 1, "total": 0.01, "mean": 0.01, "p50": 0.01,
                  "p95": 0.01, "min": 0.01, "max": 0.01 },
            ])
        );
    }
}
