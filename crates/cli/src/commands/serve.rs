use std::path::Path;
use std::process::ExitCode;
use std::sync::Arc;
use std::time::Duration;

use lsf_core::theme::Revalidate;

use crate::app::App;
use crate::checkout::{self, AppArgs, CheckoutOptions, Variant};
use crate::output::print_diagnostics;
use crate::server::throttle::Throttle;
use crate::server::{ServeOptions, ServerState, run as run_server};
use crate::watch::{self, Watcher};

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

    /// Develop the app of `--app`, as `shopify app dev` does: build a Function again when its
    /// files change, and reload the open pages (`--live-reload`) when the builds are over or
    /// another file of the app or the data changes. At start, only the Functions that have no
    /// module or whose files changed since the last watch are built (all with `--build`). The
    /// checkout runs a copy of the last module that built: a build that fails keeps it, and the
    /// pages show its error.
    #[arg(long, conflicts_with = "static_files")]
    watch: bool,

    /// Do not log requests.
    #[arg(long, short)]
    quiet: bool,

    /// Send responses as they are. Without this option, pages, styles, scripts and JSON are
    /// compressed with Brotli or gzip for the clients that accept it, as on Shopify.
    #[arg(long)]
    no_compression: bool,

    /// Serve the stylesheets and the scripts of the theme as they are written. Without this
    /// option they are minified, with a source map, as on Shopify.
    #[arg(long)]
    no_minify: bool,

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

    /// Name every section and every theme block in the `Server-Timing` header of the pages,
    /// with how long it took to render. Without this option the header says how long the
    /// page, its template and its layout took. Either way `GET /__lsf/timings` adds up what
    /// was rendered so far.
    #[arg(long)]
    timings: bool,

    /// Who visitors are logged in as when they arrive: the email of a customer of the data,
    /// `default` for the first one, or `none`. Without it, the store data decides
    /// (`session.customer`).
    #[arg(long, env = "LSF_CUSTOMER", value_name = "EMAIL")]
    customer: Option<String>,

    #[command(flatten)]
    app: AppArgs,

    /// The look of the checkout of the app: `standard` (Shopify's one-page checkout) or `shop`
    /// (the Shop app and Shop Pay). A checkout URL can ask for the other with `?variant=`.
    #[arg(long, value_enum, default_value_t = Variant::Standard)]
    variant: Variant,
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

/// Serves the theme. With `standalone`, serves the app of `--app` alone, its checkout and its
/// customer account: `lsf app dev`.
pub fn run(
    theme: &Path,
    data: Option<&Path>,
    previews: &[lsf_core::Preview],
    args: Args,
    standalone: bool,
) -> Result<ExitCode, String> {
    // The app is read before the store data and the theme, so that a wrong `--app` is the first
    // thing said. `--build` runs here too.
    // `lsf app dev` serves the app the current folder is in when `--app` is not given.
    let found = match (&args.app.app, standalone) {
        (None, true) => Some(checkout::app_of(None, "lsf app dev")?),
        _ => None,
    };
    if args.watch && args.app.app.is_none() && found.is_none() {
        return Err("--watch needs the app: `--app DIR` (or LSF_APP)".to_string());
    }
    let app_dir = args.app.app.clone().or(found);
    let checkout = match &app_dir {
        Some(dir) => {
            // The watcher builds the Functions itself, without stopping at one that fails.
            let opening = AppArgs {
                build: args.app.build && !args.watch,
                ..args.app.clone()
            };
            let (app, selection) = checkout::open(&opening, dir)?;
            checkout::print_app_diagnostics(&app, &selection);
            Some((
                CheckoutOptions {
                    app_dir: dir.clone(),
                    extensions: args.app.extensions.clone(),
                    variant: args.variant,
                    standalone,
                },
                (
                    selection.functions(&app).count(),
                    selection.ui_extensions(&app).count(),
                ),
            ))
        }
        None if standalone => {
            return Err("`lsf app dev` needs the app: `--app DIR` (or LSF_APP)".to_string());
        }
        None => None,
    };
    let (checkout, extensions) = match checkout {
        Some((options, counts)) => (Some(options), Some(counts)),
        None => (None, None),
    };
    let revalidate = if args.static_files {
        Revalidate::Never
    } else {
        Revalidate::Every(Duration::from_millis(200))
    };
    let throttle = Throttle::parse(args.throttle.iter().map(String::as_str))
        .map_err(|problem| format!("--throttle: {problem}"))?;
    // The app's store data when the theme has none.
    let data = crate::app::data_with_app(theme, data, app_dir.as_deref());
    let data = data.as_deref();
    let app = if standalone {
        App::open_without_theme(theme, data, previews)?
    } else {
        let app = App::open(theme, data, revalidate, previews)?;
        crate::app::note_missing_previews(&app.theme, previews);
        crate::app::note_classic_accounts(&app.theme);
        app
    };
    let source = app.describe_source();
    let data_dir = app.data_dir();
    let (state, diagnostics) = ServerState::new(
        app,
        ServeOptions {
            live_reload: args.live_reload || args.watch,
            watch: !args.static_files,
            quiet: args.quiet,
            compress: !args.no_compression,
            minify: !args.no_minify,
            throttle: throttle.clone(),
            timings: args.timings,
            customer: args.customer.clone(),
            checkout,
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

    let mut state = state;
    if args.watch {
        // The modules the checkout runs: copies, out of the app, of the last that built.
        let modules = watch::Modules::create()
            .map_err(|error| format!("--watch: cannot create a folder for the modules: {error}"))?;
        state.builds = Arc::new(watch::Builds::publishing(modules));
    }
    let builds = state.builds.clone();
    let state = Arc::new(state);
    let watcher = match (&app_dir, args.watch) {
        (Some(dir), true) => {
            let changed = state.clone();
            let mut options = watch::Options::new(args.app.extensions.clone(), args.app.build);
            options.data = data_dir;
            options.record = watch::record_dir(dir);
            Some(Watcher::start(
                dir,
                state.builds.clone(),
                options,
                // What the home page found when it ran the Functions is for the old modules.
                Box::new(move || changed.home_check.clear()),
                Arc::new(|line: &str| eprintln!("{line}")),
            ))
        }
        _ => None,
    };

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
        if !standalone {
            eprintln!("theme:  {}", theme.display());
        }
        if let (Some(dir), Some((functions, ui_extensions))) = (&app_dir, extensions) {
            eprintln!(
                "app:    {} ({functions} Functions, {ui_extensions} UI extensions)",
                dir.display()
            );
        }
        eprintln!("data:   {source}");
        if args.port.is_none() && local.port() != DEFAULT_PORT {
            eprintln!("port:   {DEFAULT_PORT} is taken, using {}", local.port());
        }
        if !throttle.is_empty() {
            eprintln!("throttle: {throttle}");
        }
        if let Some(dir) = &app_dir {
            let scenarios = &state.loaded().store.scenarios;
            let broken = scenarios.iter().filter(|s| !s.problems.is_empty()).count();
            if scenarios.is_empty() {
                eprintln!(
                    "scenarios: none (`lsf app init --app {}` writes a set in `checkouts/` of the data)",
                    dir.display()
                );
            } else if broken > 0 {
                eprintln!(
                    "scenarios: {} ({broken} with errors, listed on the home page)",
                    scenarios.len()
                );
            } else {
                eprintln!("scenarios: {}", scenarios.len());
            }
        }
        if standalone {
            eprintln!(
                "ready:  http://{local}/   (extensions and checkout scenarios; checkout: http://{local}/checkout; status and control API: http://{local}/__lsf)"
            );
        } else {
            eprintln!("ready:  http://{local}/   (status and control API: http://{local}/__lsf)");
            if app_dir.is_some() {
                eprintln!(
                    "home:   http://{local}/__lsf/checkouts   (extensions and checkout scenarios of the app)"
                );
            }
        }
        if args.watch {
            eprintln!("watch:  the Functions build again when their files change, and the pages reload (Ctrl-C to stop)");
        }
        run_server(state, listener)
            .await
            .map_err(|error| error.to_string())
    })?;
    // Interrupted: the builds that run stop too.
    if let Some(watcher) = watcher {
        watcher.stop();
    }
    builds.close();
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
