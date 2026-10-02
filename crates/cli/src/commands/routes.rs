use std::path::Path;
use std::process::ExitCode;

use lsf_core::theme::Revalidate;

use crate::app::App;
use crate::server::{ServeOptions, ServerState, routes};

pub fn run(theme: &Path, data: Option<&Path>) -> Result<ExitCode, String> {
    let app = App::open(theme, data, Revalidate::Never)?;
    let (state, _) = ServerState::new(
        app,
        ServeOptions {
            live_reload: false,
            watch: false,
            quiet: true,
            throttle: Default::default(),
        },
    );
    for route in routes(&state) {
        println!("{route}");
    }
    Ok(ExitCode::SUCCESS)
}
