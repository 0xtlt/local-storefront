use std::path::Path;
use std::process::ExitCode;
use std::sync::Arc;

use lsf_core::theme::Revalidate;
use lsf_core::{Session, Store};
use lsf_theme::Request;
use lsf_theme::render::Target;

use crate::app::App;
use crate::output::{count, print_diagnostics};

#[derive(clap::Args)]
pub struct Args {
    /// The path to render, with its query string: `/products/shirt?variant=123`.
    #[arg(default_value = "/", value_parser = crate::paths::route)]
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

    /// Render as this customer is logged in: the email of a customer of the data, `default`
    /// for the first one, or `none`. Without it, the store data decides (`session.customer`).
    #[arg(long, env = "LSF_CUSTOMER", value_name = "EMAIL")]
    customer: Option<String>,

    /// Put a variant in the cart: its SKU, its id or the handle of its product, with a quantity
    /// after `=` (`--cart TEE-BLK-M=2`). Repeat it for several lines.
    #[arg(long, value_name = "VARIANT[=QUANTITY]")]
    cart: Vec<String>,

    /// The session to render with, as `PUT /__lsf/session` takes it (`customer`, `cart`,
    /// `country`, `company_location`): JSON, or the path of a JSON file. `--customer` and
    /// `--cart` apply on top of it.
    #[arg(long, value_name = "JSON|FILE")]
    session: Option<String>,

    /// How to print the Liquid errors, on stderr.
    #[arg(long, value_enum, default_value = "text")]
    format: Format,
}

#[derive(Clone, Copy, PartialEq, Eq, Default, clap::ValueEnum)]
pub enum Format {
    /// For a person: the error as Liquid prints it.
    #[default]
    Text,
    /// GitHub Actions annotations (`::error file=…,line=…::message`), on stderr: a workflow
    /// shows each on its line of the pull request.
    Github,
}

/// The file of the theme a template name of an error is (`sections/header` →
/// `sections/header.liquid`), as an annotation names it.
fn source_file(theme: &Path, template: &str) -> Option<String> {
    [
        format!("{template}.liquid"),
        template.to_string(),
        format!("{template}.json"),
    ]
    .into_iter()
    .find(|candidate| theme.join(candidate).is_file())
    .map(|candidate| crate::output::annotation_path(theme, &candidate))
}

/// Prints an annotation of `render` on stderr: the HTML is on stdout.
fn annotate(
    level: &'static str,
    file: Option<String>,
    line: Option<usize>,
    path: &str,
    message: String,
) {
    eprintln!(
        "{}",
        crate::output::Annotation {
            level,
            file,
            line,
            title: format!("lsf render {path}"),
            message,
        }
    );
}

/// The session `--session`, `--customer` and `--cart` describe, in the data format.
fn session_input(args: &Args) -> Result<Option<lsf_core::store::model::SessionInput>, String> {
    use lsf_core::store::model::{CartInput, CartLineInput, SessionInput};
    if args.session.is_none() && args.customer.is_none() && args.cart.is_empty() {
        return Ok(None);
    }
    let mut input = match &args.session {
        None => SessionInput::default(),
        Some(text) => {
            let source = if text.trim_start().starts_with('{') {
                text.clone()
            } else {
                std::fs::read_to_string(&*crate::paths::given(text))
                    .map_err(|error| format!("cannot read the session {text}: {error}"))?
            };
            let json = lsf_core::json::parse_lenient("--session", &source)
                .map_err(|error| error.to_string())?;
            serde_json::from_value(json)
                .map_err(|error| format!("--session is not a session: {error}"))?
        }
    };
    if let Some(customer) = &args.customer {
        input.customer = Some(customer.clone());
    }
    for line in &args.cart {
        let (variant, quantity) = match line.rsplit_once('=') {
            Some((variant, quantity)) => (
                variant,
                quantity.trim().parse::<u32>().map_err(|_| {
                    format!("--cart {line}: the quantity after = must be a whole number")
                })?,
            ),
            None => (line.as_str(), 1),
        };
        let variant = match variant.trim().parse::<u64>() {
            Ok(id) => serde_json::json!(id),
            Err(_) => serde_json::json!(variant.trim()),
        };
        let line: CartLineInput =
            serde_json::from_value(serde_json::json!({"variant": variant, "quantity": quantity}))
                .map_err(|error| format!("--cart {line}: {error}"))?;
        input
            .cart
            .get_or_insert_with(CartInput::default)
            .items
            .push(line);
    }
    Ok(Some(input))
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

/// What the server answers itself, rather than with a page of the theme, for a path without
/// its locale: how the summary names it.
fn answered_by_the_server(path: &str) -> Option<&'static str> {
    match path {
        // It stands in for the customer accounts Shopify hosts.
        "/account" => Some("a page of lsf's (the customer account), not of the theme"),
        _ if path.starts_with("/account/") => {
            Some("a page of lsf's (the customer account), not of the theme")
        }
        // A section rendered against their own objects (`recommendations`, `predictive_search`).
        "/recommendations/products" => Some("product recommendations"),
        "/search/suggest" => Some("predictive search"),
        _ => None,
    }
}

/// What to render: `--section-id`, else the sections the query asks for (`section_id`,
/// `sections`), as the Section Rendering API does, else the page.
fn target(args: &Args, request: &Request) -> Target {
    if let Some(id) = &args.section_id {
        return Target::Section(id.clone());
    }
    if let Some((_, id)) = request.query.iter().find(|(key, _)| key == "section_id") {
        return Target::Section(id.clone());
    }
    let ids: Vec<String> = request
        .query
        .iter()
        .filter(|(key, _)| key == "sections" || key == "sections[]")
        .flat_map(|(_, value)| value.split(',').map(str::trim).map(str::to_string))
        .filter(|id| !id.is_empty())
        .collect();
    if ids.is_empty() {
        Target::Page
    } else {
        Target::Sections(ids)
    }
}

pub fn run(
    theme: &Path,
    data: Option<&Path>,
    previews: &[lsf_theme::Preview],
    args: Args,
) -> Result<ExitCode, String> {
    let app = App::open(theme, data, Revalidate::Never, previews)?;
    crate::app::note_missing_previews(&app.theme, previews);
    let (store, diagnostics) = app.load_store();
    print_diagnostics(&diagnostics);
    let store = Arc::new(store);
    let request = request_for(&store, args.host.clone(), &args.path);

    let target = target(&args, &request);
    let mut session = Session::initial(&store);
    if let Some(input) = session_input(&args)? {
        let mut problems = lsf_core::diagnostics::Diagnostics::new();
        lsf_core::store::build::apply_session_input(
            &store,
            &input,
            &mut session,
            &mut problems,
            "the session",
            "",
        );
        if problems.has_errors() {
            print_diagnostics(&problems);
            return Err("the session of --session, --customer or --cart names what the store data does not have".to_string());
        }
    }
    // What the server answers itself rather than with a page of the theme: answered as `serve`
    // answers it.
    if let Some(what) = answered_by_the_server(&request.path) {
        let options = crate::server::ServeOptions {
            // Its Liquid errors go to stderr, as those of a page do.
            quiet: false,
            ..super::routes::quiet_options()
        };
        let (state, _) = crate::server::ServerState::new(app, options);
        let path = match &args.section_id {
            Some(id) if !args.path.contains("section_id=") => format!(
                "{}{}section_id={}",
                args.path,
                if args.path.contains('?') { "&" } else { "?" },
                lsf_core::urls::encode_component(id)
            ),
            _ => args.path.clone(),
        };
        let (status, errors, body) = state.answer_get(&path, &args.host, session);
        println!("{body}");
        if args.format == Format::Github && (errors > 0 || status >= 400) {
            annotate(
                if args.strict { "error" } else { "warning" },
                None,
                None,
                &args.path,
                format!(
                    "status {status}, {}",
                    count(errors, "Liquid error", "Liquid errors")
                ),
            );
        }
        eprintln!(
            "status {status} · {what} · {}",
            count(errors, "Liquid error", "Liquid errors")
        );
        return Ok(if args.strict && (status >= 400 || errors > 0) {
            ExitCode::FAILURE
        } else {
            ExitCode::SUCCESS
        });
    }
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
        if seen.contains(&message) {
            continue;
        }
        match args.format {
            Format::Text => eprintln!("{message}"),
            Format::Github => annotate(
                if args.strict { "error" } else { "warning" },
                error
                    .template_name
                    .as_deref()
                    .and_then(|name| source_file(theme, name)),
                error.line.map(|line| line as usize),
                &args.path,
                message.clone(),
            ),
        }
        seen.push(message);
    }
    for warning in &rendered.warnings {
        eprintln!("warning: {warning}");
    }
    eprintln!(
        "status {} · template {} · {} · {}",
        rendered.status,
        rendered.template,
        count(rendered.errors.len(), "Liquid error", "Liquid errors"),
        count(rendered.warnings.len(), "warning", "warnings")
    );
    // Strict, a path that leads to nothing fails as Liquid errors do: a snapshot of a product
    // the data no longer has must not pass.
    Ok(
        if args.strict && (!rendered.errors.is_empty() || rendered.status == 404) {
            if rendered.status == 404 && rendered.errors.is_empty() {
                let message = format!("nothing at {}: the page is the theme's 404", args.path);
                match args.format {
                    Format::Text => eprintln!("error: {message}"),
                    Format::Github => annotate("error", None, None, &args.path, message),
                }
            }
            ExitCode::FAILURE
        } else {
            ExitCode::SUCCESS
        },
    )
}
