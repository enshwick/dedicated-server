//! `enshwick-server` runs an Enshrouded dedicated server with the Enshwick
//! loader in the Linux image: it updates the server through SteamCMD, puts
//! the loader and the chosen mods beside it, applies the configuration from
//! the environment and runs the server under Proton's Wine until a stop
//! signal, which it turns into the server's own clean shutdown.

#[cfg(unix)]
mod config;
#[cfg(unix)]
mod launch;
#[cfg(unix)]
mod mods;
#[cfg(unix)]
mod settings;
#[cfg(unix)]
mod steamcmd;

use std::io::Write as _;
use std::process::ExitCode;

const USAGE: &str = "usage: enshwick-server [run | --version]";

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args
        .iter()
        .map(String::as_str)
        .collect::<Vec<_>>()
        .as_slice()
    {
        [] | ["run"] => match run() {
            Ok(code) => code,
            Err(error) => {
                say(&format!("error: {error}"));
                ExitCode::FAILURE
            }
        },
        ["--version"] => {
            say(concat!("enshwick-server ", env!("CARGO_PKG_VERSION")));
            ExitCode::SUCCESS
        }
        _ => {
            say(USAGE);
            ExitCode::FAILURE
        }
    }
}

#[cfg(unix)]
fn run() -> Result<ExitCode, String> {
    // First, so a stop during the update is not lost.
    let stop = launch::stop_flag()?;
    let settings = settings::Settings::from_env(|name| std::env::var(name).ok())?;
    let layout = settings.layout();
    layout.create()?;
    steamcmd::prepare(&settings, &layout)?;
    mods::install(&layout.bundle, &layout.server, &settings.mods)?;
    if let Some(patch) = &settings.server_config {
        config::apply(&layout.server_config(), patch)?;
    }
    launch::run(&settings, &layout, &stop)
}

#[cfg(not(unix))]
fn run() -> Result<ExitCode, String> {
    Err("`run` starts the server in the Linux image; it does not run on this system".to_owned())
}

/// Writes one line for the person reading the container's log.
fn say(text: &str) {
    let _shown = writeln!(std::io::stderr(), "enshwick-server: {text}");
}
