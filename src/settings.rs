//! What the host sets through the environment, and the paths that follow
//! from it.

use std::path::{Path, PathBuf};
use std::time::Duration;

/// Mods turned on when `ENSHWICK_MODS` is not set: the optimizer, which
/// makes the server cheaper under Wine.
const DEFAULT_MODS: &str = "optimizer";

/// The settings of one start.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Settings {
    /// Everything that persists: the server, its saves and the Wine prefix
    /// (`ENSHWICK_DATA`, `/data`).
    pub(crate) data: PathBuf,
    /// The loader and mods shipped in the image (`ENSHWICK_BUNDLE`).
    pub(crate) bundle: PathBuf,
    /// The Proton build whose Wine runs the server (`ENSHWICK_PROTON`).
    pub(crate) proton: PathBuf,
    /// Whether SteamCMD updates the server before it starts
    /// (`ENSHWICK_UPDATE`, on by default).
    pub(crate) update: bool,
    /// The bundled mods to turn on, by name (`ENSHWICK_MODS`).
    pub(crate) mods: Vec<String>,
    /// A JSON object merged into the server's `enshrouded_server.json`
    /// before every start (`ENSHWICK_SERVER_CONFIG`).
    pub(crate) server_config: Option<String>,
    /// How long the server may take to save and stop before it is killed
    /// (`ENSHWICK_STOP_TIMEOUT`, in seconds).
    pub(crate) stop_timeout: Duration,
}

impl Settings {
    /// Reads the settings through `var`, which returns an environment
    /// variable's value.
    ///
    /// # Errors
    ///
    /// A value that does not parse, named with its variable.
    pub(crate) fn from_env(var: impl Fn(&str) -> Option<String>) -> Result<Self, String> {
        let path = |name: &str, default: &str| {
            PathBuf::from(var(name).unwrap_or_else(|| default.to_owned()))
        };
        let update = match var("ENSHWICK_UPDATE").as_deref().map(str::trim) {
            None | Some("" | "1" | "true" | "yes" | "on") => true,
            Some("0" | "false" | "no" | "off") => false,
            Some(other) => {
                return Err(format!(
                    "ENSHWICK_UPDATE: expected true or false, got {other:?}"
                ));
            }
        };
        let stop_timeout = match var("ENSHWICK_STOP_TIMEOUT") {
            None => Duration::from_secs(60),
            Some(text) => Duration::from_secs(text.trim().parse().map_err(|_| {
                format!("ENSHWICK_STOP_TIMEOUT: expected whole seconds, got {text:?}")
            })?),
        };
        Ok(Self {
            data: path("ENSHWICK_DATA", "/data"),
            bundle: path("ENSHWICK_BUNDLE", "/opt/enshwick/bundle"),
            proton: path("ENSHWICK_PROTON", "/opt/proton"),
            update,
            mods: names(&var("ENSHWICK_MODS").unwrap_or_else(|| DEFAULT_MODS.to_owned())),
            server_config: var("ENSHWICK_SERVER_CONFIG").filter(|text| !text.trim().is_empty()),
            stop_timeout,
        })
    }

    /// The paths under the data directory.
    #[must_use]
    pub(crate) fn layout(&self) -> Layout {
        Layout {
            server: self.data.join("server"),
            prefix: self.data.join("prefix"),
            home: self.data.join("home"),
            bundle: self.bundle.clone(),
        }
    }
}

/// The names in a list separated by commas or whitespace.
fn names(text: &str) -> Vec<String> {
    text.split(|c: char| c == ',' || c.is_whitespace())
        .filter(|name| !name.is_empty())
        .map(str::to_owned)
        .collect()
}

/// Where things live in the data directory.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Layout {
    /// The game's dedicated server, as SteamCMD installs it; saves and logs
    /// stay in it, where the server puts them by default.
    pub(crate) server: PathBuf,
    /// The Wine prefix.
    pub(crate) prefix: PathBuf,
    /// The home directory: SteamCMD and the Steam client library the server
    /// talks to.
    pub(crate) home: PathBuf,
    /// The loader and mods shipped in the image.
    pub(crate) bundle: PathBuf,
}

impl Layout {
    /// The server's configuration file.
    #[must_use]
    pub(crate) fn server_config(&self) -> PathBuf {
        self.server.join("enshrouded_server.json")
    }

    /// Creates the directories that must exist before the first start.
    ///
    /// # Errors
    ///
    /// A directory that cannot be created, such as on a volume the
    /// container's user may not write to.
    pub(crate) fn create(&self) -> Result<(), String> {
        [&self.server, &self.prefix, &self.home]
            .into_iter()
            .try_for_each(|dir| create_dir(dir))
    }
}

fn create_dir(dir: &Path) -> Result<(), String> {
    std::fs::create_dir_all(dir).map_err(|error| {
        format!(
            "{}: {error}; the data volume must be writable by the container's user",
            dir.display()
        )
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn settings(vars: &[(&str, &str)]) -> Result<Settings, String> {
        Settings::from_env(|name| {
            vars.iter()
                .find(|(key, _)| *key == name)
                .map(|(_, value)| (*value).to_owned())
        })
    }

    #[test]
    fn defaults_fit_the_image() {
        let settings = settings(&[]).unwrap();
        assert_eq!(settings.data, PathBuf::from("/data"));
        assert!(settings.update);
        assert_eq!(settings.mods, ["optimizer"]);
        assert_eq!(settings.server_config, None);
        assert_eq!(settings.stop_timeout, Duration::from_secs(60));
        assert_eq!(settings.layout().server, PathBuf::from("/data/server"));
    }

    #[test]
    fn mods_split_on_commas_and_spaces() {
        let settings = settings(&[("ENSHWICK_MODS", "optimizer, game-rates  safes")]).unwrap();
        assert_eq!(settings.mods, ["optimizer", "game-rates", "safes"]);
    }

    #[test]
    fn empty_mods_turn_every_bundled_mod_off() {
        assert!(settings(&[("ENSHWICK_MODS", "")]).unwrap().mods.is_empty());
    }

    #[test]
    fn update_reads_common_spellings() {
        assert!(!settings(&[("ENSHWICK_UPDATE", "false")]).unwrap().update);
        assert!(!settings(&[("ENSHWICK_UPDATE", "0")]).unwrap().update);
        assert!(settings(&[("ENSHWICK_UPDATE", "yes")]).unwrap().update);
        let error = settings(&[("ENSHWICK_UPDATE", "maybe")]).unwrap_err();
        assert!(error.starts_with("ENSHWICK_UPDATE"), "{error}");
    }

    #[test]
    fn a_bad_stop_timeout_names_its_variable() {
        let error = settings(&[("ENSHWICK_STOP_TIMEOUT", "soon")]).unwrap_err();
        assert!(error.starts_with("ENSHWICK_STOP_TIMEOUT"), "{error}");
        let settings = settings(&[("ENSHWICK_STOP_TIMEOUT", "90")]).unwrap();
        assert_eq!(settings.stop_timeout, Duration::from_secs(90));
    }

    #[test]
    fn a_blank_server_config_is_none() {
        assert_eq!(
            settings(&[("ENSHWICK_SERVER_CONFIG", "  ")])
                .unwrap()
                .server_config,
            None
        );
    }
}
