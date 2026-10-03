use std::path::Path;
use std::process::ExitCode;
use std::sync::Arc;
use std::time::Duration;

use lsf_core::theme::Revalidate;

use crate::app::App;
use crate::output::print_diagnostics;
use crate::server::throttle::Throttle;
use crate::server::{ServeOptions, ServerState, run as run_server};

#[derive(clap::Args)]
pub struct Args {
    /// The address to listen on.
    #[arg(long, default_value = "127.0.0.1")]
    host: String,

    /// The port to listen on. Use 0 to pick a free port. Without this option: 9292, or the
    /// next free port when it is taken.
    #[arg(long, short)]
    port: Option<u16>,

    /// Reload open pages when the theme or the data change. This injects a small script into
    /// the pages, so leave it off for end-to-end tests.
    #[arg(long)]
    live_reload: bool,

    /// Read the theme and the data once and never look at the files again. Fastest, for test
    /// runs against files that do not change.
    #[arg(long = "static")]
    static_files: bool,

    /// Do not log requests.
    #[arg(long, short)]
    quiet: bool,

    /// Send responses as they are. Without this option, pages, styles, scripts and JSON are
    /// compressed with Brotli or gzip for the clients that accept it, as on Shopify.
    #[arg(long)]
    no_compression: bool,

    /// Refuse to start when the store data has errors.
    #[arg(long)]
    strict: bool,

    /// Answer requests late, to see what the storefront shows while it waits. A preset
    /// (`--throttle simulated`: a Shopify storefront on a good connection; `slow`: on a slow
    /// mobile one), a duration for every request (`--throttle 300ms`), or
    /// `<kind>=<duration>` for one kind (`--throttle cart=500ms,cart-add=1s`), also to adjust
    /// a preset (`--throttle simulated,cart-add=2s`). Kinds: all, cart, page, section,
    /// cart-read, cart-add, cart-change, cart-update, cart-clear, search, recommendations,
    /// product, form, asset, image. A session can have its own through `PUT /__lsf/session`.
    #[arg(long, env = "LSF_THROTTLE", value_name = "RULES")]
    throttle: Vec<String>,

    /// Who visitors are logged in as when they arrive: the email of a customer of the data,
    /// `default` for the first one, or `none`. Without it, the store data decides
    /// (`session.customer`).
    #[arg(long, env = "LSF_CUSTOMER", value_name = "EMAIL")]
    customer: Option<String>,
}

/// The port used when `--port` is not given.
const DEFAULT_PORT: u16 = 9292;

/// How many ports are tried from the default one on, when `--port` is not given.
const PORT_ATTEMPTS: u16 = 100;

/// Listens on the first free port of `ports`.
async fn listen(
    host: &str,
    ports: std::ops::RangeInclusive<u16>,
) -> Result<tokio::net::TcpListener, String> {
    let (first, last) = (*ports.start(), *ports.end());
    let mut failure = format!("cannot listen on {host}: no port to try");
    for port in ports {
        let address = format!("{host}:{port}");
        match tokio::net::TcpListener::bind(&address).await {
            Ok(listener) => return Ok(listener),
            Err(error) if error.kind() == std::io::ErrorKind::AddrInUse && first != last => {
                failure = format!(
                    "cannot listen on {host}: the ports {first} to {last} are taken \
                     (choose one with --port)"
                );
            }
            Err(error) => return Err(format!("cannot listen on {address}: {error}")),
        }
    }
    Err(failure)
}

pub fn run(theme: &Path, data: Option<&Path>, args: Args) -> Result<ExitCode, String> {
    let revalidate = if args.static_files {
        Revalidate::Never
    } else {
        Revalidate::Every(Duration::from_millis(200))
    };
    let throttle = Throttle::parse(args.throttle.iter().map(String::as_str))
        .map_err(|problem| format!("--throttle: {problem}"))?;
    let app = App::open(theme, data, revalidate)?;
    let source = app.describe_source();
    let (state, diagnostics) = ServerState::new(
        app,
        ServeOptions {
            live_reload: args.live_reload,
            watch: !args.static_files,
            quiet: args.quiet,
            compress: !args.no_compression,
            throttle: throttle.clone(),
            customer: args.customer.clone(),
        },
    );
    print_diagnostics(&diagnostics);
    if let Some(who) = &args.customer {
        state
            .loaded()
            .store
            .customer_named(who)
            .map_err(|problem| format!("--customer: {problem}"))?;
    }
    if args.strict && diagnostics.has_errors() {
        return Err("the store data has errors (see above)".to_string());
    }

    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        // Deeply nested snippets and blocks recurse: give the render threads room.
        .thread_stack_size(16 * 1024 * 1024)
        .build()
        .map_err(|error| error.to_string())?;
    runtime.block_on(async move {
        // A port that was asked for is the one to listen on, or an error. The default one is a
        // convenience: when another server has it, the next free one does as well.
        let ports = match args.port {
            Some(port) => port..=port,
            None => DEFAULT_PORT..=DEFAULT_PORT + PORT_ATTEMPTS - 1,
        };
        let listener = listen(&args.host, ports).await?;
        let local = listener.local_addr().map_err(|error| error.to_string())?;
        eprintln!("theme:  {}", theme.display());
        eprintln!("data:   {source}");
        if args.port.is_none() && local.port() != DEFAULT_PORT {
            eprintln!("port:   {DEFAULT_PORT} is taken, using {}", local.port());
        }
        if !throttle.is_empty() {
            eprintln!("throttle: {throttle}");
        }
        eprintln!("ready:  http://{local}/   (status and control API: http://{local}/__lsf)");
        run_server(Arc::new(state), listener)
            .await
            .map_err(|error| error.to_string())
    })?;
    Ok(ExitCode::SUCCESS)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn a_taken_port_is_skipped_only_when_others_may_be_tried() {
        let taken = listen("127.0.0.1", 0..=0).await.unwrap();
        let port = taken.local_addr().unwrap().port();

        // The port that was asked for, and nothing else.
        let error = listen("127.0.0.1", port..=port).await.unwrap_err();
        assert!(
            error.starts_with(&format!("cannot listen on 127.0.0.1:{port}: ")),
            "{error}"
        );

        // Without a wish, the next free one.
        let last = port.saturating_add(50);
        let other = listen("127.0.0.1", port..=last).await.unwrap();
        let found = other.local_addr().unwrap().port();
        assert!(found > port && found <= last, "{found}");
    }
}
