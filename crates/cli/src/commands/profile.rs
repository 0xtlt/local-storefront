use std::path::Path;
use std::process::ExitCode;
use std::sync::Arc;

use lsf_core::Session;
use lsf_core::render::cost::Costs;
use lsf_core::render::{ProfileOptions, Target};
use lsf_core::theme::Revalidate;

use crate::app::App;
use crate::output::print_diagnostics;
use crate::profile::{DEFAULT_RUNS, MAX_RUNS, Unit, measure, speedscope, text};

#[derive(clap::Args)]
pub struct Args {
    /// The path to profile, with its query string: `/collections/all?sort_by=price-ascending`.
    #[arg(default_value = "/")]
    path: String,

    /// Profile only this section (Section Rendering API).
    #[arg(long)]
    section_id: Option<String>,

    /// Record every tag and every output too, by file and line. There are many of them, and
    /// timing each one makes the render slower: read such a profile for where the time goes
    /// in a file, not for how long the page takes.
    #[arg(long)]
    lines: bool,

    /// Print the profile in the format of speedscope instead of the report, as
    /// `shopify theme profile --json` does. Save it to a file and drop the file on
    /// https://www.speedscope.app to see the flame graph.
    #[arg(long)]
    json: bool,

    /// How many times the page is rendered. The profile is the one of the render in the
    /// middle, by how long they took: a single render can be slow for reasons of its own.
    #[arg(long, default_value_t = DEFAULT_RUNS, value_parser = clap::value_parser!(u32).range(1..=i64::from(MAX_RUNS)))]
    runs: u32,

    /// Count in points instead of time: what the page would cost a Shopify storefront,
    /// where loading a product or a metafield is slow, rather than what it takes here.
    /// A tag or an output rendered is 1 point, and each thing that is loaded costs more.
    /// The points are a model, the same at every render.
    #[arg(long)]
    points: bool,

    /// Change what a kind of thing costs in points: `--cost product=50,metafield=5`. Kinds:
    /// liquid, product, variant, collection, metafield, metaobject, page, blog, article,
    /// menu, search.
    #[arg(long, value_name = "RULES")]
    cost: Vec<String>,

    /// Show every row of the report. Without this option, what took less than a hundredth
    /// of the render is summed up.
    #[arg(long)]
    all: bool,
}

pub fn run(theme: &Path, data: Option<&Path>, args: Args) -> Result<ExitCode, String> {
    let app = App::open(theme, data, Revalidate::Never)?;
    let (store, diagnostics) = app.load_store();
    print_diagnostics(&diagnostics);
    let store = Arc::new(store);
    let request = super::render::request_for(&store, DEFAULT_HOST.to_string(), &args.path);
    let target = match args.section_id {
        Some(id) => Target::Section(id),
        None => Target::Page,
    };
    let site = app
        .renderer
        .site(store.clone(), request, Session::initial(&store));
    let options = ProfileOptions {
        lines: args.lines,
        costs: Costs::default()
            .with(&args.cost.join(","))
            .map_err(|problem| format!("--cost: {problem}"))?,
    };
    let unit = if args.points {
        Unit::Points
    } else {
        Unit::Time
    };
    let measured = measure(&app.renderer, &site, &target, &options, args.runs);
    if args.json {
        println!("{}", speedscope(&measured.profile, &args.path, unit));
    } else {
        print!("{}", text(&args.path, &measured, unit, args.all));
    }
    Ok(ExitCode::SUCCESS)
}

/// The host generated URLs point to: nothing of a profile shows it.
const DEFAULT_HOST: &str = "localhost:9292";
