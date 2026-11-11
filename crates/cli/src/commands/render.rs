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
                std::fs::read_to_string(text)
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

    let target = match args.section_id.clone() {
        Some(id) => Target::Section(id),
        None => Target::Page,
    };
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
                eprintln!(
                    "error: nothing at {}: the page is the theme's 404",
                    args.path
                );
            }
            ExitCode::FAILURE
        } else {
            ExitCode::SUCCESS
        },
    )
}
