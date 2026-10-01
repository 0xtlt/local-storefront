use std::path::Path;
use std::process::ExitCode;
use std::sync::Arc;
use std::time::Duration;

use slt_core::theme::Revalidate;

use crate::app::App;
use crate::output::print_diagnostics;
use crate::server::{ServeOptions, ServerState, run as run_server};

#[derive(clap::Args)]
pub struct Args {
    /// The address to listen on.
    #[arg(long, default_value = "127.0.0.1")]
    host: String,

    /// The port to listen on. Use 0 to pick a free port.
    #[arg(long, short, default_value_t = 9292)]
    port: u16,

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

    /// Refuse to start when the store data has errors.
    #[arg(long)]
    strict: bool,
}

pub fn run(theme: &Path, data: Option<&Path>, args: Args) -> Result<ExitCode, String> {
    let revalidate = if args.static_files {
        Revalidate::Never
    } else {
        Revalidate::Every(Duration::from_millis(200))
    };
    let app = App::open(theme, data, revalidate)?;
    let source = app.describe_source();
    let (state, diagnostics) = ServerState::new(
        app,
        ServeOptions {
            live_reload: args.live_reload,
            watch: !args.static_files,
            quiet: args.quiet,
        },
    );
    print_diagnostics(&diagnostics);
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
        let address = format!("{}:{}", args.host, args.port);
        let listener = tokio::net::TcpListener::bind(&address)
            .await
            .map_err(|error| format!("cannot listen on {address}: {error}"))?;
        let local = listener.local_addr().map_err(|error| error.to_string())?;
        eprintln!("theme:  {}", theme.display());
        eprintln!("data:   {source}");
        eprintln!("ready:  http://{local}/   (status and control API: http://{local}/__slt)");
        run_server(Arc::new(state), listener)
            .await
            .map_err(|error| error.to_string())
    })?;
    Ok(ExitCode::SUCCESS)
}
