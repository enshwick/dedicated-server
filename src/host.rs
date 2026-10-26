//! The server as a Windows process: started beside its files, stopped with
//! Ctrl+C so it saves, killed when it does not stop in time.

use std::process::{Child, Command, ExitCode, ExitStatus};
use std::sync::mpsc::{Receiver, RecvTimeoutError};
use std::time::{Duration, Instant};

use crate::console;
use crate::say;
use crate::settings::Layout;

/// How often the stop request and the server are looked at.
const TICK: Duration = Duration::from_millis(200);

/// Starts the server.
///
/// # Errors
///
/// The server's program cannot start.
pub(crate) fn start(layout: &Layout) -> Result<Child, String> {
    let exe = layout.server_exe();
    let child = Command::new(&exe)
        .current_dir(layout.server())
        .spawn()
        .map_err(|error| format!("starting {}: {error}", exe.display()))?;
    say(&format!("started the server, process {}", child.id()));
    Ok(child)
}

/// Runs the server in this console until it exits; Ctrl+C reaches it
/// directly, and it saves and quits.
///
/// # Errors
///
/// The server cannot start, or waiting for it fails.
pub(crate) fn run_foreground(layout: &Layout) -> Result<ExitCode, String> {
    let mut child = start(layout)?;
    console::ignore_ctrl_c()?;
    let status = child
        .wait()
        .map_err(|error| format!("waiting for the server: {error}"))?;
    Ok(exit_code(status))
}

/// Waits for the server to exit or for a stop request on `stop`; a stop
/// sends Ctrl+C, calls `stopping` and kills the server after `timeout`.
///
/// # Errors
///
/// Waiting for the server fails.
pub(crate) fn supervise(
    child: &mut Child,
    stop: &Receiver<()>,
    timeout: Duration,
    stopping: impl FnOnce(),
) -> Result<ExitStatus, String> {
    loop {
        if let Some(status) = try_wait(child)? {
            return Ok(status);
        }
        match stop.recv_timeout(TICK) {
            Ok(()) | Err(RecvTimeoutError::Disconnected) => break,
            Err(RecvTimeoutError::Timeout) => {}
        }
    }
    stopping();
    say(&format!(
        "stopping: the server saves and quits, or is killed in {} s",
        timeout.as_secs()
    ));
    if let Err(error) = console::send_ctrl_c(child.id()) {
        say(&error);
    }
    let asked = Instant::now();
    while asked.elapsed() < timeout {
        if let Some(status) = try_wait(child)? {
            return Ok(status);
        }
        std::thread::sleep(TICK);
    }
    say("the server did not stop in time: killing it");
    let _killed = child.kill();
    child
        .wait()
        .map_err(|error| format!("waiting for the server: {error}"))
}

fn try_wait(child: &mut Child) -> Result<Option<ExitStatus>, String> {
    let status = child
        .try_wait()
        .map_err(|error| format!("waiting for the server: {error}"))?;
    if let Some(status) = status {
        say(&format!("the server exited: {status}"));
    }
    Ok(status)
}

/// The server's exit code as this program's; Windows codes past 255 become
/// a plain failure.
fn exit_code(status: ExitStatus) -> ExitCode {
    status
        .code()
        .and_then(|code| u8::try_from(code).ok())
        .map_or(ExitCode::FAILURE, ExitCode::from)
}
