use std::path::Path;
use std::process::ExitCode;

use lsf_core::theme::Revalidate;

use crate::app::App;
use crate::server::{ServeOptions, ServerState, routes};

pub fn run(
    theme: &Path,
    data: Option<&Path>,
    previews: &[lsf_core::Preview],
) -> Result<ExitCode, String> {
    let app = App::open(theme, data, Revalidate::Never, previews)?;
    let (state, _) = ServerState::new(
        app,
        ServeOptions {
            live_reload: false,
            watch: false,
            quiet: true,
            compress: false,
            minify: false,
            throttle: Default::default(),
            timings: false,
            customer: None,
            checkout: None,
        },
    );
    for route in routes(&state) {
        println!("{route}");
    }
    Ok(ExitCode::SUCCESS)
}
