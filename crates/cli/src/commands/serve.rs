use std::path::Path;
use std::process::ExitCode;
use std::sync::Arc;
use std::time::Duration;

use lsf_app::admin::api::ThrottleSetting;
use lsf_core::theme::Revalidate;

use crate::app::App;
use crate::checkout::{self, AppArgs, CheckoutOptions, Variant};
use crate::output::print_diagnostics;
use crate::server::admin::AdminSettings;
use crate::server::parts::{Part, Parts};
use crate::server::polaris::Mode as PolarisMode;
use crate::server::throttle::Throttle;
use crate::server::{ServeOptions, ServerState, run as run_server};
use crate::watch::{self, Watcher};

/// The help heading of the options for a Shopify app (`--app`). The options of the theme come
/// first, without a heading.
const APP: &str = "App (with --app)";

/// The help heading of the options of the Admin API and lsf's admin.
const ADMIN: &str = "Admin API and embedded admin (with --app)";

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
    #[arg(help_heading = APP, long, conflicts_with = "static_files")]
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

    /// Run only these parts of lsf, comma-separated or repeated (`--only checkout,admin`).
    /// Without it, `serve` runs the theme, and every part with `--app`; `app dev` runs the
    /// checkout and the Admin API. `checkout` and `admin` need `--app`. A part left out answers
    /// `404` on its routes, with an `X-LSF-Reason` that says so.
    #[arg(
        long,
        env = "LSF_ONLY",
        value_enum,
        value_delimiter = ',',
        value_name = "PARTS"
    )]
    only: Vec<Part>,

    #[command(flatten, next_help_heading = APP)]
    app: AppArgs,

    /// With `--app`: serve its UI extensions and its Admin API, but run none of its Functions.
    /// For the tests of a backend, whose orders a Function that fails would block.
    #[arg(help_heading = APP, long, conflicts_with = "watch")]
    no_functions: bool,

    /// The look of the checkout of the app: `standard` (Shopify's one-page checkout) or `shop`
    /// (the Shop app and Shop Pay). A checkout URL can ask for the other with `?variant=`.
    #[arg(help_heading = APP, long, value_enum, default_value_t = Variant::Standard)]
    variant: Variant,

    /// Where the backend of the app listens (`http://localhost:3000`): the webhooks of
    /// shopify.app.toml are delivered there (a relative `uri` is joined to it, and an absolute
    /// one on the origin of `application_url` is moved to it), and the OAuth redirects and the
    /// embedded app's launch go there. Without it, `application_url`.
    #[arg(help_heading = APP, long, env = "LSF_APP_URL", value_name = "URL", value_parser = super::app_url)]
    app_url: Option<String>,

    /// The client secret webhooks, OAuth and session tokens are signed with: the backend checks
    /// them with its own. Default: `SHOPIFY_API_SECRET` of the environment, else of the app's
    /// `.env`, else a fixed local secret, printed at start.
    #[arg(
        help_heading = APP,
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
        help_heading = ADMIN,
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
    #[arg(help_heading = ADMIN, long, env = "LSF_ADMIN_VERSIONS", value_name = "VERSIONS")]
    admin_versions: Option<String>,

    /// Lets `themeCreate`, the `URL` bodies of `themeFilesUpsert` and `urlRedirectImportCreate`
    /// fetch their source from this machine or its network (the loopback, private and
    /// link-local addresses), for a theme ZIP served from localhost on purpose. Off by default:
    /// Shopify cannot reach them, and a backend could otherwise read what the machine serves.
    #[arg(help_heading = ADMIN, long, env = "LSF_ADMIN_FETCH_LOCAL")]
    admin_fetch_local: bool,

    /// A URL lsf answers itself with a file, never downloading it: `<url>=<file>`, repeatable
    /// (`--admin-source https://cdn.example.com/theme.zip=./fixtures/theme.zip`). For the URLs of
    /// your backend that `themeCreate`, the `URL` bodies of `themeFilesUpsert` and
    /// `urlRedirectImportCreate` read, when nothing serves them in a test. The file is read at
    /// each use. `PUT /__lsf/admin/sources?url=` stubs others while lsf runs.
    #[arg(help_heading = ADMIN, long = "admin-source", value_name = "URL=FILE", value_parser = super::admin_source)]
    admin_sources: Vec<(String, std::path::PathBuf)>,

    /// The plans of the app's managed pricing (Shopify App Pricing), which Shopify keeps in the
    /// Partner Dashboard: a TOML file of `[[plans]]` (`handle`, `name`, `price`, `interval`,
    /// `trial_days`, `features`, `welcome_link`), or JSON for a `.json` file. lsf's admin then
    /// shows them at `/admin/charges/<app handle>/pricing_plans`, where the merchant chooses one,
    /// and the Billing API creates no charge, as on Shopify. Read at start, and again when the
    /// file changes unless `--static`; `PUT /__lsf/admin/pricing-plans` changes them while lsf
    /// runs.
    #[arg(help_heading = ADMIN, long, env = "LSF_PRICING_PLANS", value_name = "FILE")]
    pricing_plans: Option<std::path::PathBuf>,

    /// What `/__lsf/cdn/<path>` answers, the URL an embedded app loads Polaris from in lsf's
    /// admin (`/__lsf/cdn/shopifycloud/polaris-2.js`): `online` redirects to
    /// `https://cdn.shopify.com/<path>`; `offline` serves lsf's copy of Polaris 1.x and 2.x, its
    /// icons and its fonts, for an app shown without the internet. `PUT /__lsf/polaris` changes
    /// it while lsf runs.
    #[arg(
        help_heading = ADMIN,
        long,
        env = "LSF_POLARIS",
        value_enum,
        value_name = "MODE",
        default_value_t = PolarisMode::Online
    )]
    polaris: PolarisMode,
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

/// Where the options of `serve` got their values: what tells an option that was given, on the
/// command line or in the environment, from one left to its default.
#[derive(Default)]
pub struct Given(Option<clap::ArgMatches>);

impl Given {
    /// The sources of the options parsed into `matches`: those of `serve` or `app dev`.
    pub fn of(matches: &clap::ArgMatches) -> Given {
        Given(Some(matches.clone()))
    }

    /// Where the option of id `id` (its field) was given: `None` when it was not, or is a flag
    /// turned off (`LSF_ADMIN_FETCH_LOCAL=false`).
    fn source(&self, id: &str) -> Option<clap::parser::ValueSource> {
        use clap::parser::ValueSource;
        let matches = self.0.as_ref()?;
        let source = matches.value_source(id)?;
        if source == ValueSource::DefaultValue {
            return None;
        }
        if let Ok(Some(false)) = matches.try_get_one::<bool>(id) {
            return None;
        }
        Some(source)
    }

    /// Whether the value of `id` came from its environment variable.
    fn in_env(&self, id: &str) -> bool {
        self.source(id) == Some(clap::parser::ValueSource::EnvVariable)
    }
}

/// The options that need a part, by their id: those of the Admin API, then those of the
/// checkout. `--app-url` feeds the webhooks, OAuth, the embedded app's launch and the app proxy:
/// the Admin API's alone. `--watch` builds the Functions the checkout runs.
const NEEDS: &[(&str, Part)] = &[
    ("app_url", Part::Admin),
    ("client_secret", Part::Admin),
    ("admin_throttle", Part::Admin),
    ("admin_versions", Part::Admin),
    ("admin_fetch_local", Part::Admin),
    ("admin_sources", Part::Admin),
    ("pricing_plans", Part::Admin),
    ("polaris", Part::Admin),
    ("extensions", Part::Checkout),
    ("no_functions", Part::Checkout),
    ("variant", Part::Checkout),
    ("build", Part::Checkout),
    ("watch", Part::Checkout),
];

/// How an error names the option of id `id`: `--pricing-plans`, or
/// `LSF_PRICING_PLANS (--pricing-plans)` when its value came from the environment.
fn named(given: &Given, id: &str) -> String {
    let command = <Args as clap::Args>::augment_args(clap::Command::new("serve"));
    let Some(arg) = command.get_arguments().find(|arg| arg.get_id() == id) else {
        return id.to_string();
    };
    let long = format!("--{}", arg.get_long().unwrap_or(id));
    match arg.get_env().filter(|_| given.in_env(id)) {
        Some(env) => format!("{} ({long})", env.to_string_lossy()),
        None => long,
    }
}

/// The parts a server runs: those of `--only`, else the theme for `serve`, every part with
/// `--app`, and the checkout and the Admin API for `app dev` (`standalone`). An error names
/// what is wrong: a part that needs the app without one, the theme asked of `app dev`, an option
/// of a part that is off.
fn parts_of(args: &Args, given: &Given, has_app: bool, standalone: bool) -> Result<Parts, String> {
    let command = lsf_core::util::command();
    let only = (!args.only.is_empty()).then(|| Parts::of(&args.only));
    // `--only` as it was given, for the errors.
    let only_named = |parts: Parts| {
        if given.in_env("only") {
            format!("`LSF_ONLY={}`", parts.only())
        } else {
            format!("`--only {}`", parts.only())
        }
    };
    let parts = match only {
        Some(parts) if standalone && parts.theme => {
            return Err(format!(
                "{}: `{command} app dev` serves no theme (`{command} serve --app DIR --only {}` serves it with the app)",
                only_named(parts),
                parts.only()
            ));
        }
        Some(parts) if (parts.checkout || parts.admin) && !has_app => {
            return Err(format!(
                "{} needs the app: `--app DIR` (or LSF_APP)",
                only_named(parts)
            ));
        }
        Some(parts) => parts,
        None if standalone => Parts {
            checkout: true,
            admin: true,
            ..Parts::default()
        },
        None if has_app => Parts::ALL,
        None => Parts {
            theme: true,
            ..Parts::default()
        },
    };
    for (id, part) in NEEDS {
        // A variable of the environment is said and set aside, by `ignored_variables`: one
        // exported for `app dev` must not stop a `serve` of the theme alone.
        if parts.has(*part) || given.source(id).is_none() || given.in_env(id) {
            continue;
        }
        return Err(format!(
            "{} needs the {part} part, {}",
            named(given, id),
            left_out(only.is_some(), given)
        ));
    }
    Ok(parts)
}

/// Why a part is not served: `--only` (or `LSF_ONLY`) leaves it out, or there is no app.
fn left_out(only: bool, given: &Given) -> String {
    match only {
        true if given.in_env("only") => "which LSF_ONLY leaves out".to_string(),
        true => "which --only leaves out".to_string(),
        false => "which needs the app: `--app DIR` (or LSF_APP)".to_string(),
    }
}

/// The options set by a variable of the environment for a part that is not served: each is
/// ignored, and said, where the same option on the command line is an error.
fn ignored_variables(args: &Args, given: &Given, parts: Parts) -> Vec<String> {
    NEEDS
        .iter()
        .filter(|(id, part)| !parts.has(*part) && given.in_env(id))
        .map(|(id, part)| {
            format!(
                "note: {} is set, but the {part} part is not served, {}: ignored",
                named(given, id),
                left_out(!args.only.is_empty(), given)
            )
        })
        .collect()
}

/// Serves the parts of lsf `--only` names (see [`parts_of`]): the theme, the checkout of the app
/// of `--app`, its Admin API. With `standalone`, `lsf app dev`: the app of the current folder
/// when `--app` is not given, and no theme.
pub fn run(
    theme: &Path,
    data: Option<&Path>,
    previews: &[lsf_theme::Preview],
    args: Args,
    given: &Given,
    standalone: bool,
) -> Result<ExitCode, String> {
    // The app is read before the store data and the theme, so that a wrong `--app` is the first
    // thing said. `--build` runs here too.
    // `lsf app dev` serves the app the current folder is in when `--app` is not given.
    let found = match (&args.app.app, standalone) {
        (None, true) => Some(checkout::app_of(None, "lsf app dev")?),
        _ => None,
    };
    let app_dir = args.app.app.clone().or(found);
    let parts = parts_of(&args, given, app_dir.is_some(), standalone)?;
    for line in ignored_variables(&args, given, parts) {
        eprintln!("{line}");
    }
    // What the `app:` line says of the app, when it serves a part.
    let mut summary = None;
    // How many of its Functions run: the scenarios are for them.
    let mut functions = 0;
    let mut checkout = None;
    if let Some(dir) = app_dir.as_ref().filter(|_| parts.checkout || parts.admin) {
        if parts.checkout {
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
                .retain(|diagnostic| diagnostic.message != lsf_app::config::NO_EXTENSIONS);
            checkout::print_app_diagnostics(&app, &selection);
            functions = selection.functions(&app).count();
            summary = Some(app_summary(
                Some((functions, selection.ui_extensions(&app).count())),
                app.has_extensions_directory(),
                parts.admin,
                args.no_functions,
            ));
            checkout = Some(CheckoutOptions {
                app_dir: dir.clone(),
                extensions: args.app.extensions.clone(),
                variant: args.variant,
                standalone: !parts.theme,
                no_functions: args.no_functions,
            });
        } else {
            // The Admin API alone: what is wrong with the app itself, not with its extensions.
            let mut app = lsf_app::config::load(dir)?;
            app.diagnostics
                .retain(|diagnostic| diagnostic.message != lsf_app::config::NO_EXTENSIONS);
            checkout::print_app_diagnostics(
                &app,
                &lsf_app::config::Selection::only(Vec::<String>::new()),
            );
            summary = Some(app_summary(
                None,
                app.has_extensions_directory(),
                true,
                false,
            ));
        }
    }
    let revalidate = if args.static_files {
        Revalidate::Never
    } else {
        Revalidate::Every(Duration::from_millis(200))
    };
    let throttle = Throttle::parse(args.throttle.iter().map(String::as_str))
        .map_err(|problem| format!("--throttle: {problem}"))?;
    // The settings of the Admin API, read only when it is served: a variable of the environment
    // for a part that is not served is ignored (`ignored_variables`), not read.
    let admin = if parts.admin {
        Some(AdminSettings {
            app_dir: app_dir.clone(),
            client_secret: args.client_secret.clone(),
            app_url: args.app_url.clone(),
            throttle: ThrottleSetting::parse(&args.admin_throttle)
                .map_err(|problem| format!("--admin-throttle: {problem}"))?,
            versions: match &args.admin_versions {
                Some(list) => lsf_app::admin::version::Served::parse(list)
                    .map_err(|problem| format!("--admin-versions: {problem}"))?,
                None => Default::default(),
            },
            fetch_local: args.admin_fetch_local,
            sources: args.admin_sources.clone(),
            pricing: match &args.pricing_plans {
                Some(path) => Some((
                    lsf_app::admin::domains::app::pricing::Pricing::read(path)
                        .map_err(|problem| format!("--pricing-plans: {problem}"))?,
                    path.clone(),
                )),
                None => None,
            },
        })
    } else {
        None
    };
    // The app's store data when the theme has none.
    let data = crate::app::data_with_app(theme, data, app_dir.as_deref());
    let data = data.as_deref();
    let app = if parts.theme {
        let app = App::open(theme, data, revalidate, previews).map_err(|problem| {
            // With an app, the theme may be the part that is not needed.
            if app_dir.is_some() && args.only.is_empty() {
                format!("{problem} (`--only checkout,admin` serves the app without a theme)")
            } else {
                problem
            }
        })?;
        crate::app::note_missing_previews(&app.theme, previews);
        crate::app::note_classic_accounts(&app.theme);
        // The app blocks and app embed blocks of its theme app extensions render in the theme.
        if let Some(dir) = &app_dir {
            crate::theme_extensions::attach(&app.theme, dir, revalidate);
            if let Some(line) = crate::theme_extensions::summary(&app.theme.app_extensions()) {
                match summary.as_mut() {
                    Some(summary) => summary.push_str(&format!(", {line}")),
                    None => summary = Some(line),
                }
            }
        }
        app
    } else {
        App::open_without_theme(theme, data, previews)?
    };
    let source = app.describe_source();
    let data_dir = app.data_dir();
    let (state, diagnostics) = ServerState::new(
        app,
        ServeOptions {
            polaris: args.polaris,
            admin,
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
    if parts.theme {
        crate::theme_extensions::print_findings(&state.app.theme, &state.loaded().store);
    }
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
        if parts.theme {
            eprintln!("theme:  {}", theme.display());
        }
        if let (Some(dir), Some(summary)) = (&app_dir, &summary) {
            eprintln!("app:    {} ({summary})", dir.display());
        }
        eprintln!("data:   {source}");
        if args.port.is_none() && local.port() != DEFAULT_PORT {
            eprintln!("port:   {DEFAULT_PORT} is taken, using {}", local.port());
        }
        eprintln!("serving: {}", parts.describe());
        if !throttle.is_empty() {
            eprintln!("throttle: {throttle}");
        }
        // The scenarios are carts for the Functions to run on: nothing to say without one.
        if let Some(dir) = app_dir.as_ref().filter(|_| functions > 0) {
            let scenarios = &state.loaded().store.scenarios;
            let broken = scenarios.iter().filter(|s| !s.problems.is_empty()).count();
            if scenarios.is_empty() {
                eprintln!(
                    "scenarios: none (`{} app init --app {}` writes a set in `checkouts/` of the data)",
                    lsf_core::util::command(),
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
        // What the backend's tests need comes before `ready:`, so that a tool that reads up to
        // it has the token. Nothing of it without the Admin API.
        if let Some(admin) = &state.admin {
            let origin = format!("http://{local}");
            for line in admin.banner(&origin) {
                eprintln!("{line}");
            }
            if let Some(proxy) = &state.app_proxy {
                eprintln!("{}", proxy.banner(&origin));
            }
            let lines = [
                admin.embedded_banner(&origin),
                admin.pricing_banner(&origin),
                admin.partner_banner(&origin),
                crate::server::polaris::banner(&state, &origin),
            ];
            for line in lines.into_iter().flatten() {
                eprintln!("{line}");
            }
        }
        if !parts.theme && parts.checkout {
            eprintln!(
                "ready:  http://{local}/   (extensions and checkout scenarios; checkout: http://{local}/checkout; status and control API: http://{local}/__lsf)"
            );
        } else {
            eprintln!("ready:  http://{local}/   (status and control API: http://{local}/__lsf)");
            if parts.theme && parts.checkout {
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
    app: &mut lsf_app::config::App,
    selection: lsf_app::config::Selection,
    named: &[String],
) -> Result<lsf_app::config::Selection, String> {
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

/// What the `app:` line says of the app: the Functions and UI extensions served (`None` when
/// `--only` leaves the checkout out), and what an app without an `extensions` directory is, a
/// backend.
fn app_summary(
    extensions: Option<(usize, usize)>,
    has_directory: bool,
    admin: bool,
    no_functions: bool,
) -> String {
    const BACKEND: &str = "the Admin API, webhooks and the app proxy";
    if !has_directory {
        return if admin {
            format!("no extensions: {BACKEND}")
        } else {
            "no extensions".to_string()
        };
    }
    let Some((functions, ui_extensions)) = extensions else {
        return format!("{BACKEND}; its extensions are not served: --only leaves out the checkout");
    };
    let ui_extensions = crate::output::count(ui_extensions, "UI extension", "UI extensions");
    if no_functions {
        return format!("{ui_extensions}, no Function: --no-functions");
    }
    format!(
        "{}, {ui_extensions}",
        crate::output::count(functions, "Function", "Functions")
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_app_line_says_what_is_served() {
        assert_eq!(
            app_summary(Some((2, 1)), true, true, false),
            "2 Functions, 1 UI extension"
        );
        assert_eq!(
            app_summary(Some((0, 0)), false, true, false),
            "no extensions: the Admin API, webhooks and the app proxy"
        );
        assert_eq!(
            app_summary(Some((0, 0)), false, false, false),
            "no extensions"
        );
        assert_eq!(
            app_summary(Some((0, 2)), true, true, true),
            "2 UI extensions, no Function: --no-functions"
        );
        assert_eq!(
            app_summary(None, true, true, false),
            "the Admin API, webhooks and the app proxy; its extensions are not served: --only leaves out the checkout"
        );
    }

    /// The parts `serve` runs with `words`, `app dev` with `standalone`, or its error. The
    /// environment is left out: its variables are tested where a server is started.
    fn parts(words: &[&str], standalone: bool) -> Result<Parts, String> {
        let command = <Args as clap::Args>::augment_args(clap::Command::new("serve"))
            .mut_args(|arg| arg.env(None::<&'static str>));
        let matches = command
            .try_get_matches_from(std::iter::once("serve").chain(words.iter().copied()))
            .unwrap_or_else(|error| panic!("{words:?}: {error}"));
        let args = <Args as clap::FromArgMatches>::from_arg_matches(&matches).unwrap();
        // `app dev` finds the app of the current folder.
        let has_app = args.app.app.is_some() || standalone;
        parts_of(&args, &Given::of(&matches), has_app, standalone)
    }

    const THEME: Parts = Parts {
        theme: true,
        checkout: false,
        admin: false,
    };
    const CHECKOUT_ADMIN: Parts = Parts {
        theme: false,
        checkout: true,
        admin: true,
    };

    #[test]
    fn without_only_serve_runs_the_theme_every_part_with_an_app_and_app_dev_its_app() {
        assert_eq!(parts(&[], false), Ok(THEME));
        assert_eq!(parts(&["--app", "a"], false), Ok(Parts::ALL));
        assert_eq!(parts(&[], true), Ok(CHECKOUT_ADMIN));
        assert_eq!(parts(&["--app", "a"], true), Ok(CHECKOUT_ADMIN));
    }

    #[test]
    fn only_names_the_parts_comma_separated_or_repeated() {
        let admin = Parts {
            admin: true,
            ..Parts::default()
        };
        assert_eq!(parts(&["--only", "theme"], false), Ok(THEME));
        assert_eq!(parts(&["--app", "a", "--only", "admin"], false), Ok(admin));
        assert_eq!(
            parts(&["--app", "a", "--only", "checkout,admin"], false),
            Ok(CHECKOUT_ADMIN)
        );
        assert_eq!(
            parts(
                &["--app", "a", "--only", "admin", "--only", "checkout"],
                false
            ),
            Ok(CHECKOUT_ADMIN)
        );
        assert_eq!(parts(&["--only", "admin"], true), Ok(admin));
        assert_eq!(
            parts(&["--app", "a", "--only", "theme,checkout,admin"], false),
            Ok(Parts::ALL)
        );
    }

    #[test]
    fn the_checkout_and_the_admin_api_need_the_app_and_app_dev_serves_no_theme() {
        for only in ["checkout", "admin", "theme,admin"] {
            let error = parts(&["--only", only], false).unwrap_err();
            assert_eq!(
                error,
                format!("`--only {only}` needs the app: `--app DIR` (or LSF_APP)")
            );
        }
        for only in ["theme", "theme,checkout"] {
            let error = parts(&["--only", only], true).unwrap_err();
            assert!(
                error.starts_with(&format!("`--only {only}`: `"))
                    && error.contains(" app dev` serves no theme ("),
                "{error}"
            );
        }
    }

    #[test]
    fn an_option_of_a_part_that_is_off_is_an_error_that_names_both() {
        let app = ["--app", "a"];
        let cases: &[(&[&str], &str, &str)] = &[
            (&["--client-secret", "s"], "--client-secret", "admin"),
            (
                &["--app-url", "http://localhost:3000"],
                "--app-url",
                "admin",
            ),
            (&["--admin-throttle", "off"], "--admin-throttle", "admin"),
            (
                &["--admin-versions", "2026-10"],
                "--admin-versions",
                "admin",
            ),
            (&["--admin-fetch-local"], "--admin-fetch-local", "admin"),
            (&["--pricing-plans", "p.toml"], "--pricing-plans", "admin"),
            (&["--polaris", "offline"], "--polaris", "admin"),
            (&["--extension", "banner"], "--extension", "checkout"),
            (&["--no-functions"], "--no-functions", "checkout"),
            (&["--variant", "shop"], "--variant", "checkout"),
            (&["--build"], "--build", "checkout"),
            (&["--watch"], "--watch", "checkout"),
        ];
        for (option, name, part) in cases {
            // The part is left out by `--only`.
            let other = if *part == "admin" {
                "checkout"
            } else {
                "admin"
            };
            let words = [&app[..], &["--only", other], option].concat();
            assert_eq!(
                parts(&words, false),
                Err(format!(
                    "{name} needs the {part} part, which --only leaves out"
                )),
                "{words:?}"
            );
            // A theme without an app has neither.
            assert_eq!(
                parts(option, false),
                Err(format!(
                    "{name} needs the {part} part, which needs the app: `--app DIR` (or LSF_APP)"
                )),
                "{option:?}"
            );
            // Every part: nothing left out.
            assert!(
                parts(&[&app[..], option].concat(), false).is_ok(),
                "{option:?}"
            );
        }
        // An option given its default is not given.
        assert!(parts(&["--app", "a", "--only", "checkout"], false).is_ok());
        assert!(parts(&["--app", "a", "--only", "admin"], false).is_ok());
    }

    #[test]
    fn every_option_that_needs_a_part_is_one_of_serve() {
        for (id, _) in NEEDS {
            assert!(named(&Given::default(), id).starts_with("--"), "{id}");
        }
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
