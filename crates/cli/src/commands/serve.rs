use std::path::Path;
use std::process::ExitCode;
use std::sync::Arc;
use std::time::Duration;

use lsf_core::theme::Revalidate;

use crate::app::App;
use crate::checkout::{self, AppArgs, CheckoutOptions, Variant};
use crate::output::print_diagnostics;
use crate::server::admin::AdminSettings;
use crate::server::throttle::Throttle;
use crate::server::{ServeOptions, ServerState, run as run_server};
use crate::watch::{self, Watcher};
use lsf_admin::api::ThrottleSetting;

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

    /// With `--app`: serve its UI extensions and its Admin API, but run none of its Functions.
    /// For the tests of a backend, whose orders a Function that fails would block.
    #[arg(long, conflicts_with = "watch")]
    no_functions: bool,

    /// The look of the checkout of the app: `standard` (Shopify's one-page checkout) or `shop`
    /// (the Shop app and Shop Pay). A checkout URL can ask for the other with `?variant=`.
    #[arg(long, value_enum, default_value_t = Variant::Standard)]
    variant: Variant,

    /// Where the backend of the app listens (`http://localhost:3000`): the webhooks of
    /// shopify.app.toml are delivered there (a relative `uri` is joined to it, and an absolute
    /// one on the origin of `application_url` is moved to it), and the OAuth redirects and the
    /// embedded app's launch go there. Without it, `application_url`.
    #[arg(long, env = "LSF_APP_URL", value_name = "URL", value_parser = super::app_url)]
    app_url: Option<String>,

    /// The client secret webhooks, OAuth and session tokens are signed with: the backend checks
    /// them with its own. Default: `SHOPIFY_API_SECRET` of the environment, else of the app's
    /// `.env`, else a fixed local secret, printed at start.
    #[arg(
        long,
        env = "LSF_CLIENT_SECRET",
        value_name = "SECRET",
        hide_env_values = true
    )]
    client_secret: Option<String>,

    /// The cost bucket of the Admin API, per shop and app: `store` (the plan of the store data:
    /// Plus for a store with companies, Standard otherwise), a plan (`standard`: 2000 points
    /// restored at 100 a second, `advanced`, `plus`, `enterprise`, `legacy`: 1000 at 50), or
    /// `off` (nothing is throttled; `extensions.cost` is still reported).
    #[arg(
        long,
        env = "LSF_ADMIN_THROTTLE",
        value_name = "PLAN",
        default_value = "store"
    )]
    admin_throttle: String,

    /// The versions of the Admin API served, comma-separated (`2026-10,2027-01`), among those
    /// this build knows: the default one alone without it. A request for another answers 404;
    /// one Shopify no longer supports falls forward to the oldest stable version served, as
    /// Shopify does.
    #[arg(long, env = "LSF_ADMIN_VERSIONS", value_name = "VERSIONS")]
    admin_versions: Option<String>,

    /// Lets `themeCreate`, the `URL` bodies of `themeFilesUpsert` and `urlRedirectImportCreate`
    /// fetch their source from this machine or its network (the loopback, private and
    /// link-local addresses), for a theme ZIP served from localhost on purpose. Off by default:
    /// Shopify cannot reach them, and a backend could otherwise read what the machine serves.
    #[arg(long, env = "LSF_ADMIN_FETCH_LOCAL")]
    admin_fetch_local: bool,

    /// A URL lsf answers itself with a file, never downloading it: `<url>=<file>`, repeatable
    /// (`--admin-source https://cdn.example.com/theme.zip=./fixtures/theme.zip`). For the URLs of
    /// your backend that `themeCreate`, the `URL` bodies of `themeFilesUpsert` and
    /// `urlRedirectImportCreate` read, when nothing serves them in a test. The file is read at
    /// each use. `PUT /__lsf/admin/sources?url=` stubs others while lsf runs.
    #[arg(long = "admin-source", value_name = "URL=FILE", value_parser = super::admin_source)]
    admin_sources: Vec<(String, std::path::PathBuf)>,
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
            // The watcher builds the Functions itself, without stopping at one that fails. With
            // `--no-functions`, the UI extensions alone are built, below.
            let opening = AppArgs {
                build: args.app.build && !args.watch && !args.no_functions,
                ..args.app.clone()
            };
            let (mut app, mut selection) = checkout::open(&opening, dir)?;
            if args.no_functions {
                selection = without_functions(&mut app, selection, &args.app.extensions)?;
                if args.app.build {
                    checkout::build_ui_extensions(&app, &selection)?;
                }
            }
            // An app without extensions is a backend: the `app:` line says so.
            app.diagnostics
                .retain(|diagnostic| diagnostic.message != lsf_checkout::app::NO_EXTENSIONS);
            checkout::print_app_diagnostics(&app, &selection);
            Some((
                CheckoutOptions {
                    app_dir: dir.clone(),
                    extensions: args.app.extensions.clone(),
                    variant: args.variant,
                    standalone,
                    no_functions: args.no_functions,
                },
                (
                    selection.functions(&app).count(),
                    selection.ui_extensions(&app).count(),
                    app.has_extensions_directory(),
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
    let admin = AdminSettings {
        client_secret: args.client_secret.clone(),
        app_url: args.app_url.clone(),
        throttle: ThrottleSetting::parse(&args.admin_throttle)
            .map_err(|problem| format!("--admin-throttle: {problem}"))?,
        versions: match &args.admin_versions {
            Some(list) => lsf_admin::version::Served::parse(list)
                .map_err(|problem| format!("--admin-versions: {problem}"))?,
            None => Default::default(),
        },
        fetch_local: args.admin_fetch_local,
        sources: args.admin_sources.clone(),
    };
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
            admin: Some(admin),
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
    let served = runtime.block_on(async move {
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
        if let (Some(dir), Some((functions, ui_extensions, has_directory))) =
            (&app_dir, extensions)
        {
            eprintln!(
                "app:    {} ({})",
                dir.display(),
                app_summary(functions, ui_extensions, has_directory, args.no_functions)
            );
        }
        eprintln!("data:   {source}");
        if args.port.is_none() && local.port() != DEFAULT_PORT {
            eprintln!("port:   {DEFAULT_PORT} is taken, using {}", local.port());
        }
        if !throttle.is_empty() {
            eprintln!("throttle: {throttle}");
        }
        // The scenarios are carts for the Functions to run on: nothing to say without one.
        let functions = extensions.is_some_and(|(functions, _, _)| functions > 0);
        if let Some(dir) = app_dir.as_ref().filter(|_| functions) {
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
        // With an app, what its backend's tests need comes before `ready:`, so that a tool that
        // reads up to it has the token. Without one, a theme has no use for them: the Admin API
        // is served all the same, and `GET /__lsf/admin` has its token.
        if app_dir.is_some() {
            if let Some(admin) = &state.admin {
                for line in admin.banner(&format!("http://{local}")) {
                    eprintln!("{line}");
                }
            }
            if let Some(proxy) = &state.app_proxy {
                eprintln!("{}", proxy.banner(&format!("http://{local}")));
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
    });
    // A request abandoned at the shutdown deadline (a render that does not end) runs on a
    // blocking thread, which dropping the runtime would wait for, for ever.
    runtime.shutdown_timeout(Duration::from_millis(200));
    served?;
    // Interrupted: the builds that run stop too.
    if let Some(watcher) = watcher {
        watcher.stop();
    }
    builds.close();
    Ok(ExitCode::SUCCESS)
}

/// `--no-functions`: the UI extensions `selection` has, and no Function; `app` leaves them out
/// too. A Function named with `--extension` is an error: it would not run.
fn without_functions(
    app: &mut lsf_checkout::app::App,
    selection: lsf_checkout::app::Selection,
    named: &[String],
) -> Result<lsf_checkout::app::Selection, String> {
    if let Some(function) = named.iter().find(|handle| app.function(handle).is_some()) {
        return Err(format!(
            "--extension {function} is a Function, and --no-functions runs none"
        ));
    }
    let ui_extensions: Vec<String> = selection
        .ui_extensions(app)
        .map(|extension| extension.handle.clone())
        .collect();
    app.without_functions();
    Ok(selection.narrowed_to(&ui_extensions))
}

/// What the `app:` line says of the extensions served: an app without an `extensions` directory
/// is a backend, served without them.
fn app_summary(
    functions: usize,
    ui_extensions: usize,
    has_directory: bool,
    no_functions: bool,
) -> String {
    if !has_directory {
        return "no extensions: the Admin API, webhooks and the app proxy".to_string();
    }
    if no_functions {
        return format!("{ui_extensions} UI extensions, no Function: --no-functions");
    }
    format!("{functions} Functions, {ui_extensions} UI extensions")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_app_line_says_what_is_served() {
        assert_eq!(
            app_summary(2, 1, true, false),
            "2 Functions, 1 UI extensions"
        );
        assert_eq!(
            app_summary(0, 0, false, false),
            "no extensions: the Admin API, webhooks and the app proxy"
        );
        assert_eq!(
            app_summary(0, 1, true, true),
            "1 UI extensions, no Function: --no-functions"
        );
    }

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
