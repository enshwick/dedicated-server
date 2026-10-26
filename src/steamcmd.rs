//! The game's dedicated server through SteamCMD, with an anonymous login.
//!
//! The server is a Windows program, so SteamCMD is told to fetch the Windows
//! depot even on Linux. SteamCMD also brings the native Steam client library
//! (`steamclient.so`) that Proton's Steam bridge loads for the server.

use std::path::{Path, PathBuf};
use std::process::Command;

use crate::say;
use crate::settings::{Layout, Settings};

/// The Steam app of Enshrouded's dedicated server.
pub(crate) const SERVER_APP: u32 = 2_278_520;

/// How often a failed update is tried again; SteamCMD's first run after its
/// own update often fails.
const ATTEMPTS: u32 = 3;

/// Updates the server when `settings.update` is on, and makes sure the Steam
/// client library is where Proton looks for it.
///
/// # Errors
///
/// SteamCMD failing every attempt, or a server that is still missing.
pub(crate) fn prepare(settings: &Settings, layout: &Layout) -> Result<(), String> {
    let executable = layout.server.join("enshrouded_server.exe");
    let client = steam_client(&layout.home);
    if settings.update || !executable.exists() {
        update(layout)?;
    } else if !client.exists() {
        // SteamCMD installs itself and the client library on its first run.
        run(&layout.home, &["+quit".to_owned()])?;
    } else {
        say("ENSHWICK_UPDATE is off: starting the server as it is");
    }
    link_steam_client(&layout.home)?;
    if executable.exists() {
        Ok(())
    } else {
        Err(format!(
            "{} is missing after the update",
            executable.display()
        ))
    }
}

fn update(layout: &Layout) -> Result<(), String> {
    let args = update_args(&layout.server);
    let mut last = String::new();
    for attempt in 1..=ATTEMPTS {
        say(&format!(
            "updating the server through SteamCMD, attempt {attempt} of {ATTEMPTS}"
        ));
        match run(&layout.home, &args) {
            Ok(()) => return Ok(()),
            Err(error) => last = error,
        }
    }
    Err(last)
}

/// SteamCMD's arguments that install or update the Windows server in `dir`.
fn update_args(dir: &Path) -> Vec<String> {
    vec![
        "+@sSteamCmdForcePlatformType".to_owned(),
        "windows".to_owned(),
        "+force_install_dir".to_owned(),
        dir.display().to_string(),
        "+login".to_owned(),
        "anonymous".to_owned(),
        "+app_update".to_owned(),
        SERVER_APP.to_string(),
        "+quit".to_owned(),
    ]
}

fn run(home: &Path, args: &[String]) -> Result<(), String> {
    let status = Command::new("steamcmd")
        .args(args)
        .env("HOME", home)
        .status()
        .map_err(|error| format!("starting steamcmd: {error}"))?;
    if status.success() {
        Ok(())
    } else {
        Err(format!("steamcmd {}: {status}", args.join(" ")))
    }
}

/// Where Proton's Steam bridge loads the native client library from.
fn steam_client(home: &Path) -> PathBuf {
    home.join(".steam/sdk64/steamclient.so")
}

/// Points `~/.steam/sdk64` at SteamCMD's 64-bit libraries when it does not
/// lead to the client library yet.
fn link_steam_client(home: &Path) -> Result<(), String> {
    if steam_client(home).exists() {
        return Ok(());
    }
    let sdk = home.join(".steam/sdk64");
    let target = home.join(".local/share/Steam/steamcmd/linux64");
    if !target.join("steamclient.so").exists() {
        return Err(format!(
            "{} is missing: SteamCMD did not install the Steam client library",
            target.join("steamclient.so").display()
        ));
    }
    std::fs::create_dir_all(home.join(".steam")).map_err(|error| error.to_string())?;
    // A dangling link from an earlier layout goes first.
    let _gone = std::fs::remove_file(&sdk);
    symlink(&target, &sdk).map_err(|error| format!("{}: {error}", sdk.display()))
}

#[cfg(unix)]
fn symlink(target: &Path, link: &Path) -> std::io::Result<()> {
    std::os::unix::fs::symlink(target, link)
}

#[cfg(not(unix))]
fn symlink(_target: &Path, _link: &Path) -> std::io::Result<()> {
    Err(std::io::Error::other("the Steam client link is for Linux"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn update_fetches_the_windows_server_anonymously() {
        assert_eq!(
            update_args(Path::new("/data/server")).join(" "),
            "+@sSteamCmdForcePlatformType windows +force_install_dir /data/server \
             +login anonymous +app_update 2278520 +quit"
        );
    }

    #[cfg(unix)]
    #[test]
    fn the_client_library_is_linked_where_proton_looks() {
        let home = tempfile::tempdir().unwrap();
        let linux64 = home.path().join(".local/share/Steam/steamcmd/linux64");
        std::fs::create_dir_all(&linux64).unwrap();
        std::fs::write(linux64.join("steamclient.so"), b"elf").unwrap();
        link_steam_client(home.path()).unwrap();
        assert_eq!(std::fs::read(steam_client(home.path())).unwrap(), b"elf");
        // Again: nothing to do.
        link_steam_client(home.path()).unwrap();
    }

    #[test]
    fn a_missing_client_library_is_an_error() {
        let home = tempfile::tempdir().unwrap();
        assert!(link_steam_client(home.path()).is_err());
    }
}
