//! Ctrl+C for the server on Windows, which saves and quits on it.
//!
//! Windows delivers Ctrl+C only to the processes of one console. The
//! foreground `run` shares its console with the server, so the host's
//! Ctrl+C reaches the server by itself and the launcher only ignores it. The
//! service has no console: it joins the server's for a moment to send the
//! event there.

use windows_sys::Win32::System::Console::{
    AttachConsole, CTRL_C_EVENT, FreeConsole, GenerateConsoleCtrlEvent, SetConsoleCtrlHandler,
};

/// Makes this process ignore Ctrl+C. Child processes started afterwards
/// inherit that, so it is called after the server has started.
pub(crate) fn ignore_ctrl_c() -> Result<(), String> {
    // SAFETY: a null handler with TRUE only sets this process's flag to
    // ignore Ctrl+C; no pointer is passed.
    let ignored = unsafe { SetConsoleCtrlHandler(None, 1) };
    if ignored == 0 {
        Err(format!(
            "ignoring Ctrl+C: {}",
            std::io::Error::last_os_error()
        ))
    } else {
        Ok(())
    }
}

/// Sends Ctrl+C to the console of process `pid`, from a process without a
/// console of its own (the service).
pub(crate) fn send_ctrl_c(pid: u32) -> Result<(), String> {
    // SAFETY: plain call with a process id; it fails when the process is
    // gone or has no console, which the result reports.
    let attached = unsafe { AttachConsole(pid) };
    if attached == 0 {
        return Err(format!(
            "joining the server's console: {}",
            std::io::Error::last_os_error()
        ));
    }
    // The event reaches every process of the console, this one too.
    let sent = ignore_ctrl_c().and_then(|()| {
        // SAFETY: plain call; group 0 is every process of the console just
        // joined.
        let sent = unsafe { GenerateConsoleCtrlEvent(CTRL_C_EVENT, 0) };
        if sent == 0 {
            Err(format!(
                "sending Ctrl+C: {}",
                std::io::Error::last_os_error()
            ))
        } else {
            Ok(())
        }
    });
    // SAFETY: plain call; leaves the console joined above.
    let _left = unsafe { FreeConsole() };
    sent
}
