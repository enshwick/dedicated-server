//! The server under Proton's Wine, supervised as the container's first
//! process.
//!
//! The server saves and quits cleanly on `SIGINT`, which Wine turns into a
//! console Ctrl+C, so a stop request (`SIGTERM` from Docker or Kubernetes, or
//! `SIGINT`) is passed on as `SIGINT`; a server still running after the stop
//! timeout is killed. As process 1 the launcher also reaps the processes Wine
//! leaves behind.

use std::process::{Command, ExitCode};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use rustix::io::Errno;
use rustix::process::{Pid, Signal, WaitOptions, kill_process, wait};
use signal_hook::consts::{SIGINT, SIGTERM};

use crate::say;
use crate::settings::{Layout, Settings};

/// How often the launcher looks at the server and the stop flag.
const TICK: Duration = Duration::from_millis(200);

/// DLL overrides every start needs: the proxy DLL from the server's folder
/// before the system's, and no .NET or HTML engine, which the server does
/// not use and the image does not ship.
const DLL_OVERRIDES: &str = "mscoree=;mshtml=;powrprof=n,b";

/// A flag set by `SIGTERM` or `SIGINT`, registered before anything else
/// starts.
///
/// # Errors
///
/// The handlers cannot be registered.
pub(crate) fn stop_flag() -> Result<Arc<AtomicBool>, String> {
    let stop = Arc::new(AtomicBool::new(false));
    for signal in [SIGTERM, SIGINT] {
        signal_hook::flag::register(signal, Arc::clone(&stop))
            .map_err(|error| format!("registering for signal {signal}: {error}"))?;
    }
    Ok(stop)
}

/// Runs the server until it exits, and returns its exit code.
///
/// # Errors
///
/// Wine cannot start, or waiting for the server fails.
pub(crate) fn run(
    settings: &Settings,
    layout: &Layout,
    stop: &AtomicBool,
) -> Result<ExitCode, String> {
    if stop.load(Ordering::Relaxed) {
        say("stop requested before the server started");
        return Ok(ExitCode::SUCCESS);
    }
    let wine = settings.proton.join("files/bin/wine");
    let child = Command::new(&wine)
        .arg("enshrouded_server.exe")
        .current_dir(layout.server())
        .envs(wine_env(settings, layout, |name| std::env::var(name).ok()))
        .spawn()
        .map_err(|error| format!("starting {}: {error}", wine.display()))?;
    let server = reaped_by_us(child);
    say(&format!(
        "started the server, process {}",
        server.as_raw_nonzero()
    ));
    supervise(server, settings.stop_timeout, stop)
}

/// The process of `child`, which [`supervise`] reaps with `wait(2)` rather
/// than through [`std::process::Child`], since it reaps every orphan too.
fn reaped_by_us(child: std::process::Child) -> Pid {
    let pid = Pid::from_child(&child);
    // Dropping a `Child` neither waits for nor kills the process.
    drop(child);
    pid
}

/// Reaps every child until `server` exits, passing a stop request on.
fn supervise(server: Pid, timeout: Duration, stop: &AtomicBool) -> Result<ExitCode, String> {
    let mut asked: Option<Instant> = None;
    let mut killed = false;
    loop {
        match wait(WaitOptions::NOHANG) {
            Ok(Some((pid, status))) if pid == server => {
                let (code, signal) = (status.exit_status(), status.terminating_signal());
                say(&match (code, signal) {
                    (Some(code), _) => format!("the server exited with code {code}"),
                    (None, Some(signal)) => format!("the server was ended by signal {signal}"),
                    (None, None) => "the server is gone".to_owned(),
                });
                return Ok(exit_code(code, signal));
            }
            // A process Wine left behind, or a wait a signal cut short.
            Ok(Some(_)) | Err(Errno::INTR) => continue,
            Ok(None) => {}
            Err(Errno::CHILD) => return Err("the server process is gone".to_owned()),
            Err(error) => return Err(format!("waiting for the server: {error}")),
        }
        if asked.is_none() && stop.load(Ordering::Relaxed) {
            say(&format!(
                "stopping: the server saves and quits, or is killed in {} s",
                timeout.as_secs()
            ));
            send(server, Signal::INT);
            asked = Some(Instant::now());
        }
        if !killed && asked.is_some_and(|at| at.elapsed() >= timeout) {
            say("the server did not stop in time: killing it");
            send(server, Signal::KILL);
            killed = true;
        }
        std::thread::sleep(TICK);
    }
}

fn send(pid: Pid, signal: Signal) {
    if let Err(error) = kill_process(pid, signal) {
        say(&format!("signalling the server: {error}"));
    }
}

/// The container's exit code: the server's own, or 128 plus the signal
/// that ended it, as shells report it.
fn exit_code(code: Option<i32>, signal: Option<i32>) -> ExitCode {
    let value = code
        .or_else(|| signal.map(|signal| 128 + signal))
        .unwrap_or(1);
    ExitCode::from(u8::try_from(value & 0xff).unwrap_or(1))
}

/// The environment Wine runs the server with. Variables the host set win,
/// except the DLL overrides, where the host's come after the ones the
/// loader needs.
fn wine_env(
    settings: &Settings,
    layout: &Layout,
    var: impl Fn(&str) -> Option<String>,
) -> Vec<(String, String)> {
    let default = |name: &str, value: &str| {
        (
            name.to_owned(),
            var(name).unwrap_or_else(|| value.to_owned()),
        )
    };
    let libraries = settings.proton.join("files/lib/x86_64-linux-gnu");
    let library_path = match var("LD_LIBRARY_PATH").filter(|path| !path.is_empty()) {
        Some(path) => format!("{}:{path}", libraries.display()),
        None => libraries.display().to_string(),
    };
    let overrides = match var("WINEDLLOVERRIDES").filter(|value| !value.is_empty()) {
        Some(value) => format!("{DLL_OVERRIDES};{value}"),
        None => DLL_OVERRIDES.to_owned(),
    };
    vec![
        ("HOME".to_owned(), layout.home().display().to_string()),
        (
            "WINEPREFIX".to_owned(),
            layout.prefix().display().to_string(),
        ),
        // 64-bit only: the server needs no 32-bit Linux libraries.
        ("WINEARCH".to_owned(), "wow64".to_owned()),
        ("WINEDLLOVERRIDES".to_owned(), overrides),
        ("LD_LIBRARY_PATH".to_owned(), library_path),
        default("WINEDEBUG", "-all"),
        // Fast synchronisation: ntsync where the host passes /dev/ntsync in,
        // fsync otherwise; Wine falls back on its own when neither works.
        default("WINENTSYNC", "1"),
        default("WINEFSYNC", "1"),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    fn env(vars: &[(&str, &str)]) -> Vec<(String, String)> {
        let lookup = |name: &str| {
            vars.iter()
                .find(|(key, _)| *key == name)
                .map(|(_, value)| (*value).to_owned())
        };
        let defaults = crate::settings::Defaults {
            data: "/data".into(),
            bundle: "/opt/enshwick/bundle".into(),
        };
        let settings = Settings::read(&defaults, &[], lookup).unwrap();
        wine_env(&settings, &settings.layout(), lookup)
    }

    fn value<'a>(env: &'a [(String, String)], name: &str) -> &'a str {
        env.iter()
            .find(|(key, _)| key == name)
            .map(|(_, value)| value.as_str())
            .unwrap()
    }

    #[test]
    fn the_loader_is_loaded_before_the_system_dll() {
        let env = env(&[]);
        assert_eq!(
            value(&env, "WINEDLLOVERRIDES"),
            "mscoree=;mshtml=;powrprof=n,b"
        );
        assert_eq!(value(&env, "WINEPREFIX"), "/data/prefix");
        assert_eq!(value(&env, "HOME"), "/data/home");
        assert_eq!(
            value(&env, "LD_LIBRARY_PATH"),
            "/opt/proton/files/lib/x86_64-linux-gnu"
        );
        assert_eq!(value(&env, "WINEDEBUG"), "-all");
        assert_eq!(value(&env, "WINENTSYNC"), "1");
    }

    #[test]
    fn the_hosts_variables_win() {
        let env = env(&[
            ("WINEDEBUG", "+timestamp"),
            ("WINEFSYNC", "0"),
            ("WINEDLLOVERRIDES", "winhttp=n,b"),
            ("LD_LIBRARY_PATH", "/usr/local/lib"),
        ]);
        assert_eq!(value(&env, "WINEDEBUG"), "+timestamp");
        assert_eq!(value(&env, "WINEFSYNC"), "0");
        assert_eq!(
            value(&env, "WINEDLLOVERRIDES"),
            "mscoree=;mshtml=;powrprof=n,b;winhttp=n,b"
        );
        assert_eq!(
            value(&env, "LD_LIBRARY_PATH"),
            "/opt/proton/files/lib/x86_64-linux-gnu:/usr/local/lib"
        );
    }

    #[test]
    fn exit_codes_follow_the_shell() {
        assert_eq!(exit_code(Some(0), None), ExitCode::SUCCESS);
        assert_eq!(exit_code(Some(3), None), ExitCode::from(3));
        assert_eq!(exit_code(None, Some(9)), ExitCode::from(137));
        assert_eq!(exit_code(None, None), ExitCode::from(1));
    }

    #[test]
    fn a_stop_request_ends_a_server_that_quits_on_sigint() {
        // `sleep` ends on SIGINT, as the server does.
        let child = Command::new("sleep").arg("30").spawn().unwrap();
        let stop = AtomicBool::new(true);
        let started = Instant::now();
        let code = supervise(reaped_by_us(child), Duration::from_secs(10), &stop).unwrap();
        assert_eq!(code, ExitCode::from(128 + 2));
        assert!(started.elapsed() < Duration::from_secs(5));
    }

    #[test]
    fn a_server_ignoring_the_stop_is_killed_after_the_timeout() {
        let dir = tempfile::tempdir().unwrap();
        let ready = dir.path().join("ready");
        let child = Command::new("sh")
            .args(["-c", "trap '' INT; touch \"$1\"; exec sleep 30", "sh"])
            .arg(&ready)
            .spawn()
            .unwrap();
        // The stop must come after the trap is set.
        while !ready.exists() {
            std::thread::sleep(Duration::from_millis(10));
        }
        let stop = AtomicBool::new(true);
        let code = supervise(reaped_by_us(child), Duration::from_millis(500), &stop).unwrap();
        assert_eq!(code, ExitCode::from(128 + 9));
    }
}
