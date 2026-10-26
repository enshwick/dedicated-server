//! The game's dedicated server through SteamCMD, with an anonymous login.
//!
//! The server is a Windows program, so SteamCMD is told to fetch the Windows
//! depot on every system. In the image SteamCMD comes with the base image and
//! also brings the native Steam client library (`steamclient.so`) that
//! Proton's Steam bridge loads for the server; on Windows it is downloaded
//! from Valve beside the server on first use.

use std::path::Path;
#[cfg(unix)]
use std::path::PathBuf;
use std::process::Command;

use crate::say;
use crate::settings::{Layout, Settings};

/// The Steam app of Enshrouded's dedicated server.
pub(crate) const SERVER_APP: u32 = 2_278_520;

/// How often a failed update is tried again; SteamCMD's first run after its
/// own update often fails.
const ATTEMPTS: u32 = 3;

/// Valve's SteamCMD for Windows.
#[cfg(windows)]
const STEAMCMD_ZIP: &str = "https://steamcdn-a.akamaihd.net/client/installer/steamcmd.zip";

/// Updates the server when `settings.update` is on or the server is
/// missing, and makes sure what the server needs from Steam is in place.
///
/// # Errors
///
/// SteamCMD failing every attempt, or a server that is still missing.
pub(crate) fn prepare(settings: &Settings, layout: &Layout) -> Result<(), String> {
    let executable = layout.server_exe();
    if settings.update || !executable.exists() {
        update(&steamcmd(layout)?, layout)?;
    } else {
        say("ENSHWICK_UPDATE is off: starting the server as it is");
    }
    #[cfg(unix)]
    {
        if !steam_client(&layout.home()).exists() {
            // SteamCMD installs itself and the client library on its first run.
            run(&steamcmd(layout)?, layout, &["+quit".to_owned()])?;
        }
        link_steam_client(&layout.home())?;
    }
    if executable.exists() {
        Ok(())
    } else {
        Err(format!(
            "{} is missing after the update",
            executable.display()
        ))
    }
}

fn update(steamcmd: &Path, layout: &Layout) -> Result<(), String> {
    let args = update_args(&layout.server());
    let mut last = String::new();
    for attempt in 1..=ATTEMPTS {
        say(&format!(
            "updating the server through SteamCMD, attempt {attempt} of {ATTEMPTS}"
        ));
        match run(steamcmd, layout, &args) {
            Ok(()) => return Ok(()),
            // SteamCMD's exit code is not always right; Steam's own record
            // of the installed app is.
            Err(_) if installed(&layout.server()) => return Ok(()),
            Err(error) => last = error,
        }
    }
    Err(last)
}

/// Whether Steam's manifest in `dir` records the server as fully installed
/// (state 4).
fn installed(dir: &Path) -> bool {
    let manifest = dir.join(format!("steamapps/appmanifest_{SERVER_APP}.acf"));
    std::fs::read_to_string(manifest).is_ok_and(|text| fully_installed(&text))
}

/// Whether a Steam app manifest's `StateFlags` is 4, fully installed.
fn fully_installed(manifest: &str) -> bool {
    manifest.lines().any(|line| {
        let mut words = line.split('"').filter(|word| !word.trim().is_empty());
        words.next() == Some("StateFlags") && words.next() == Some("4")
    })
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

fn run(steamcmd: &Path, layout: &Layout, args: &[String]) -> Result<(), String> {
    let mut command = Command::new(steamcmd);
    command.args(args);
    #[cfg(unix)]
    command.env("HOME", layout.home());
    #[cfg(windows)]
    let _ = layout;
    let status = command
        .status()
        .map_err(|error| format!("starting {}: {error}", steamcmd.display()))?;
    if status.success() {
        Ok(())
    } else {
        Err(format!("steamcmd {}: {status}", args.join(" ")))
    }
}

/// SteamCMD: the base image's on Linux.
#[cfg(unix)]
#[expect(clippy::unnecessary_wraps, reason = "Windows can fail to download it")]
fn steamcmd(_layout: &Layout) -> Result<PathBuf, String> {
    Ok(PathBuf::from("steamcmd"))
}

/// SteamCMD beside the server, downloaded from Valve when it is missing.
#[cfg(windows)]
fn steamcmd(layout: &Layout) -> Result<std::path::PathBuf, String> {
    let dir = layout.steamcmd();
    let exe = dir.join("steamcmd.exe");
    if exe.exists() {
        return Ok(exe);
    }
    say(&format!("downloading SteamCMD from {STEAMCMD_ZIP}"));
    let archive = dir.join("steamcmd.zip");
    // curl ships with Windows 10 1803, Windows Server 2019 and newer.
    let status = Command::new("curl.exe")
        .args(["--proto", "=https", "-fsSL", "-o"])
        .arg(&archive)
        .arg(STEAMCMD_ZIP)
        .status()
        .map_err(|error| format!("starting curl.exe: {error}"))?;
    if !status.success() {
        return Err(format!("downloading {STEAMCMD_ZIP}: {status}"));
    }
    let file =
        std::fs::File::open(&archive).map_err(|error| format!("{}: {error}", archive.display()))?;
    zip::ZipArchive::new(file)
        .and_then(|mut zip| zip.extract(&dir))
        .map_err(|error| format!("{}: {error}", archive.display()))?;
    let _gone = std::fs::remove_file(&archive);
    Ok(exe)
}

/// Where Proton's Steam bridge loads the native client library from.
#[cfg(unix)]
fn steam_client(home: &Path) -> PathBuf {
    home.join(".steam/sdk64/steamclient.so")
}

/// Points `~/.steam/sdk64` at SteamCMD's 64-bit libraries when it does not
/// lead to the client library yet.
#[cfg(unix)]
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
    std::os::unix::fs::symlink(&target, &sdk).map_err(|error| format!("{}: {error}", sdk.display()))
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

    #[test]
    fn a_manifest_tells_a_full_install() {
        assert!(fully_installed(
            "\"AppState\"\n{\n\t\"appid\"\t\t\"2278520\"\n\t\"StateFlags\"\t\t\"4\"\n}"
        ));
        assert!(!fully_installed("\t\"StateFlags\"\t\t\"1026\""));
        assert!(!fully_installed(""));
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

    #[cfg(unix)]
    #[test]
    fn a_missing_client_library_is_an_error() {
        let home = tempfile::tempdir().unwrap();
        assert!(link_steam_client(home.path()).is_err());
    }
}
