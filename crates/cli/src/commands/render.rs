use std::path::Path;
use std::process::ExitCode;
use std::sync::Arc;

use lsf_core::render::Target;
use lsf_core::theme::Revalidate;
use lsf_core::{Request, Session, Store};

use crate::app::App;
use crate::output::print_diagnostics;

#[derive(clap::Args)]
pub struct Args {
    /// The path to render, with its query string: `/products/shirt?variant=123`.
    #[arg(default_value = "/")]
    path: String,

    /// Render only this section (Section Rendering API).
    #[arg(long)]
    section_id: Option<String>,

    /// The host generated URLs point to.
    #[arg(long, default_value = "localhost:9292")]
    host: String,

    /// Exit with an error when the page contains Liquid errors.
    #[arg(long)]
    strict: bool,
}

/// The request for a path given on the command line, with its query string.
pub(super) fn request_for(store: &Store, host: String, path_and_query: &str) -> Request {
    let (path, query) = path_and_query
        .split_once('?')
        .unwrap_or((path_and_query, ""));
    let mut request = Request::new(host, path);
    request.query = form_urlencoded::parse(query.as_bytes())
        .map(|(key, value)| (key.into_owned(), value.into_owned()))
        .collect();
    // A locale prefix selects the language, as on the server.
    let first = path
        .trim_start_matches('/')
        .split('/')
        .next()
        .unwrap_or_default();
    request.locale = store.primary_language().iso_code.clone();
    if let Some(language) = store
        .languages
        .iter()
        .find(|language| !language.primary && language.iso_code.eq_ignore_ascii_case(first))
    {
        request.locale = language.iso_code.clone();
        request.root = language.root_url.clone();
        let rest = path
            .trim_start_matches('/')
            .strip_prefix(first)
            .unwrap_or_default();
        request.path = if rest.is_empty() {
            "/".to_string()
        } else {
            rest.to_string()
        };
    }
    request
}

pub fn run(theme: &Path, data: Option<&Path>, args: Args) -> Result<ExitCode, String> {
    let app = App::open(theme, data, Revalidate::Never)?;
    let (store, diagnostics) = app.load_store();
    print_diagnostics(&diagnostics);
    let store = Arc::new(store);
    let request = request_for(&store, args.host, &args.path);

    let target = match args.section_id {
        Some(id) => Target::Section(id),
        None => Target::Page,
    };
    let session = Session::initial(&store);
    // `/robots.txt` is a template too, with rules of its own when the theme has none.
    let rendered = if request.path == "/robots.txt" {
        app.renderer
            .render_robots(&app.renderer.site(store, request, session))
    } else {
        app.renderer.render(store, request, session, &target)
    };
    println!("{}", rendered.body);

    let mut seen = Vec::new();
    for error in &rendered.errors {
        let message = error.to_string();
        if !seen.contains(&message) {
            eprintln!("{message}");
            seen.push(message);
        }
    }
    for warning in &rendered.warnings {
        eprintln!("warning: {warning}");
    }
    eprintln!(
        "status {} · template {} · {} Liquid error(s) · {} warning(s)",
        rendered.status,
        rendered.template,
        rendered.errors.len(),
        rendered.warnings.len()
    );
    Ok(if args.strict && !rendered.errors.is_empty() {
        ExitCode::FAILURE
    } else {
        ExitCode::SUCCESS
    })
}
