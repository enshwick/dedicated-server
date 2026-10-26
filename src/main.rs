//! `enshwick-server` runs an Enshrouded dedicated server with the Enshwick
//! loader. On every start it updates the server through SteamCMD, puts the
//! loader and the chosen mods beside it and applies the configuration from
//! the settings; then it runs the server until a stop, which it turns into
//! the server's own clean shutdown.
//!
//! In the Linux image it is the entry point and runs the server under
//! Proton's Wine. On Windows, `install` registers it as a service, and `run`
//! runs the server in the console.

#![deny(unsafe_code)]

mod config;
#[cfg(windows)]
#[expect(
    unsafe_code,
    reason = "Ctrl+C for another process needs the console API"
)]
mod console;
#[cfg(windows)]
mod host;
#[cfg(unix)]
mod launch;
mod mods;
#[cfg(windows)]
mod service;
mod settings;
mod steamcmd;

use std::fs::File;
use std::io::Write as _;
use std::path::Path;
use std::process::ExitCode;
use std::sync::{Mutex, OnceLock};

use settings::{Defaults, Layout, Settings};

#[cfg(unix)]
const USAGE: &str = "usage: enshwick-server [run] [NAME=value]... | --version";
#[cfg(windows)]
const USAGE: &str =
    "usage: enshwick-server (run | install) [NAME=value]... | uninstall | --version";

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let (command, rest) = match args.split_first() {
        Some((command, rest)) if !command.contains('=') => (command.as_str(), rest),
        _ => ("run", args.as_slice()),
    };
    let result = match command {
        "run" => run(rest),
        "--version" => {
            say(concat!("enshwick-server ", env!("CARGO_PKG_VERSION")));
            Ok(ExitCode::SUCCESS)
        }
        #[cfg(windows)]
        "install" => service::install(rest),
        #[cfg(windows)]
        "uninstall" if rest.is_empty() => service::uninstall(),
        #[cfg(windows)]
        "service" => service::dispatch(rest),
        _ => Err(USAGE.to_owned()),
    };
    result.unwrap_or_else(|error| {
        say(&format!("error: {error}"));
        ExitCode::FAILURE
    })
}

/// Reads the settings from `args` and the environment.
fn settings(args: &[String]) -> Result<Settings, String> {
    Settings::read(&Defaults::here()?, args, |name| std::env::var(name).ok())
}

/// Updates the server and installs the loader, the mods and the
/// configuration: everything before the server starts.
///
/// # Errors
///
/// The step that failed.
fn prepare(settings: &Settings) -> Result<Layout, String> {
    let layout = settings.layout();
    layout.create()?;
    steamcmd::prepare(settings, &layout)?;
    mods::install(&layout.bundle, &layout.server(), &settings.mods)?;
    if let Some(patch) = &settings.server_config {
        config::apply(&layout.server_config(), patch)?;
    }
    Ok(layout)
}

#[cfg(unix)]
fn run(args: &[String]) -> Result<ExitCode, String> {
    // First, so a stop during the update is not lost.
    let stop = launch::stop_flag()?;
    let settings = settings(args)?;
    let layout = prepare(&settings)?;
    launch::run(&settings, &layout, &stop)
}

#[cfg(windows)]
fn run(args: &[String]) -> Result<ExitCode, String> {
    let layout = prepare(&settings(args)?)?;
    host::run_foreground(&layout)
}

/// Where [`say`] also writes, such as the Windows service's log.
static LOG: OnceLock<Mutex<File>> = OnceLock::new();

/// Makes [`say`] append to `path` as well.
#[cfg_attr(
    unix,
    expect(dead_code, reason = "only the Windows service has no console")
)]
fn log_to_file(path: &Path) {
    if let Ok(file) = File::options().create(true).append(true).open(path) {
        let _first = LOG.set(Mutex::new(file));
    }
}

/// Writes one line for the person reading the log.
fn say(text: &str) {
    let line = format!("enshwick-server: {text}\n");
    let _shown = std::io::stderr().write_all(line.as_bytes());
    if let Some(Ok(mut file)) = LOG.get().map(Mutex::lock) {
        let _logged = file.write_all(line.as_bytes());
    }
}
