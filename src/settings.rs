//! What the host sets, and the paths that follow from it.
//!
//! Settings are `ENSHWICK_*` variables, from the environment or as
//! `NAME=value` arguments, which win. The image takes them from its
//! environment; the Windows service keeps them as its arguments.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::time::Duration;

/// Mods turned on when `ENSHWICK_MODS` is not set: the optimizer, which
/// makes the server cheaper.
const DEFAULT_MODS: &str = "optimizer";

/// The variables a setting may come from, each once.
const NAMES: [&str; 7] = [
    "ENSHWICK_DATA",
    "ENSHWICK_BUNDLE",
    "ENSHWICK_PROTON",
    "ENSHWICK_UPDATE",
    "ENSHWICK_MODS",
    "ENSHWICK_SERVER_CONFIG",
    "ENSHWICK_STOP_TIMEOUT",
];

/// The settings of one start.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Settings {
    /// Everything that persists: the server, its saves and what runs it
    /// (`ENSHWICK_DATA`).
    pub(crate) data: PathBuf,
    /// The loader and mods to install (`ENSHWICK_BUNDLE`).
    pub(crate) bundle: PathBuf,
    /// The Proton build whose Wine runs the server (`ENSHWICK_PROTON`).
    #[cfg(unix)]
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

/// Where things are when the host does not say: in the image, its fixed
/// paths; on Windows, beside the program.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Defaults {
    pub(crate) data: PathBuf,
    pub(crate) bundle: PathBuf,
}

impl Defaults {
    /// The defaults of this system.
    ///
    /// # Errors
    ///
    /// On Windows, the program's own path cannot be found.
    pub(crate) fn here() -> Result<Self, String> {
        if cfg!(windows) {
            let exe =
                std::env::current_exe().map_err(|error| format!("the program's path: {error}"))?;
            let dir = exe.parent().map_or_else(PathBuf::new, Path::to_path_buf);
            Ok(Self {
                bundle: dir.join("bundle"),
                data: dir,
            })
        } else {
            Ok(Self {
                data: PathBuf::from("/data"),
                bundle: PathBuf::from("/opt/enshwick/bundle"),
            })
        }
    }
}

impl Settings {
    /// Reads the settings from `args` (`NAME=value`), falling back to `var`,
    /// which returns an environment variable's value, then to `defaults`.
    ///
    /// # Errors
    ///
    /// An argument that is not a setting, or a value that does not parse,
    /// named with its variable.
    pub(crate) fn read(
        defaults: &Defaults,
        args: &[String],
        var: impl Fn(&str) -> Option<String>,
    ) -> Result<Self, String> {
        let given = assignments(args)?;
        let var = |name: &str| given.get(name).cloned().or_else(|| var(name));
        let path = |name: &str, default: &Path| {
            var(name).map_or_else(|| default.to_path_buf(), PathBuf::from)
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
            data: path("ENSHWICK_DATA", &defaults.data),
            bundle: path("ENSHWICK_BUNDLE", &defaults.bundle),
            #[cfg(unix)]
            proton: path("ENSHWICK_PROTON", Path::new("/opt/proton")),
            update,
            mods: names(&var("ENSHWICK_MODS").unwrap_or_else(|| DEFAULT_MODS.to_owned())),
            server_config: var("ENSHWICK_SERVER_CONFIG").filter(|text| !text.trim().is_empty()),
            stop_timeout,
        })
    }

    /// The settings as `NAME=value` arguments, which [`Settings::read`]
    /// reads back; what equals the default of `defaults` is left out.
    #[cfg(any(windows, test))]
    #[must_use]
    pub(crate) fn as_args(&self, defaults: &Defaults) -> Vec<String> {
        let mut args = Vec::new();
        if self.data != defaults.data {
            args.push(format!("ENSHWICK_DATA={}", self.data.display()));
        }
        if self.bundle != defaults.bundle {
            args.push(format!("ENSHWICK_BUNDLE={}", self.bundle.display()));
        }
        if !self.update {
            args.push("ENSHWICK_UPDATE=false".to_owned());
        }
        args.push(format!("ENSHWICK_MODS={}", self.mods.join(",")));
        if let Some(config) = &self.server_config {
            args.push(format!("ENSHWICK_SERVER_CONFIG={config}"));
        }
        args.push(format!(
            "ENSHWICK_STOP_TIMEOUT={}",
            self.stop_timeout.as_secs()
        ));
        args
    }

    /// The paths under the data directory.
    #[must_use]
    pub(crate) fn layout(&self) -> Layout {
        Layout {
            data: self.data.clone(),
            bundle: self.bundle.clone(),
        }
    }
}

/// The `NAME=value` arguments by name.
fn assignments(args: &[String]) -> Result<BTreeMap<String, String>, String> {
    args.iter()
        .map(|arg| {
            arg.split_once('=')
                .filter(|(name, _)| NAMES.contains(name))
                .map(|(name, value)| (name.to_owned(), value.to_owned()))
                .ok_or_else(|| {
                    format!(
                        "{arg:?} is not a setting; settings are NAME=value with NAME one of {}",
                        NAMES.join(", ")
                    )
                })
        })
        .collect()
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
    data: PathBuf,
    /// The loader and mods to install.
    pub(crate) bundle: PathBuf,
}

impl Layout {
    /// The game's dedicated server, as SteamCMD installs it; saves and logs
    /// stay in it, where the server puts them by default.
    #[must_use]
    pub(crate) fn server(&self) -> PathBuf {
        self.data.join("server")
    }

    /// The server's program.
    #[must_use]
    pub(crate) fn server_exe(&self) -> PathBuf {
        self.server().join("enshrouded_server.exe")
    }

    /// The server's configuration file.
    #[must_use]
    pub(crate) fn server_config(&self) -> PathBuf {
        self.server().join("enshrouded_server.json")
    }

    /// The Wine prefix.
    #[cfg(unix)]
    #[must_use]
    pub(crate) fn prefix(&self) -> PathBuf {
        self.data.join("prefix")
    }

    /// The home directory: SteamCMD and the Steam client library the server
    /// talks to.
    #[cfg(unix)]
    #[must_use]
    pub(crate) fn home(&self) -> PathBuf {
        self.data.join("home")
    }

    /// SteamCMD.
    #[cfg(windows)]
    #[must_use]
    pub(crate) fn steamcmd(&self) -> PathBuf {
        self.data.join("steamcmd")
    }

    /// The service's log.
    #[cfg(windows)]
    #[must_use]
    pub(crate) fn log(&self) -> PathBuf {
        self.data.join("enshwick-server.log")
    }

    /// Creates the directories that must exist before the first start.
    ///
    /// # Errors
    ///
    /// A directory that cannot be created, such as on a volume the
    /// container's user may not write to.
    pub(crate) fn create(&self) -> Result<(), String> {
        #[cfg(unix)]
        let dirs = [self.server(), self.prefix(), self.home()];
        #[cfg(windows)]
        let dirs = [self.server(), self.steamcmd()];
        dirs.iter().try_for_each(|dir| {
            std::fs::create_dir_all(dir).map_err(|error| {
                format!(
                    "{}: {error}; the data folder must be writable by the server's user",
                    dir.display()
                )
            })
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn defaults() -> Defaults {
        Defaults {
            data: PathBuf::from("/data"),
            bundle: PathBuf::from("/opt/enshwick/bundle"),
        }
    }

    fn settings(vars: &[(&str, &str)]) -> Result<Settings, String> {
        Settings::read(&defaults(), &[], |name| {
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
        assert_eq!(
            settings.layout().server(),
            Path::new("/data").join("server")
        );
    }

    #[test]
    fn arguments_win_over_the_environment() {
        let args = [
            "ENSHWICK_MODS=game-rates".to_owned(),
            "ENSHWICK_DATA=/srv".to_owned(),
        ];
        let settings = Settings::read(&defaults(), &args, |name| {
            (name == "ENSHWICK_MODS").then(|| "optimizer".to_owned())
        })
        .unwrap();
        assert_eq!(settings.mods, ["game-rates"]);
        assert_eq!(settings.data, PathBuf::from("/srv"));
    }

    #[test]
    fn an_argument_that_is_no_setting_is_refused() {
        let error = Settings::read(&defaults(), &["--mods".to_owned()], |_| None).unwrap_err();
        assert!(error.contains("ENSHWICK_MODS"), "{error}");
        let error = Settings::read(&defaults(), &["OTHER=1".to_owned()], |_| None).unwrap_err();
        assert!(error.starts_with("\"OTHER=1\""), "{error}");
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

    #[test]
    fn arguments_round_trip() {
        let given = [
            "ENSHWICK_MODS=optimizer,game-rates".to_owned(),
            "ENSHWICK_UPDATE=false".to_owned(),
            r#"ENSHWICK_SERVER_CONFIG={"name": "x"}"#.to_owned(),
        ];
        let settings = Settings::read(&defaults(), &given, |_| None).unwrap();
        let again = Settings::read(&defaults(), &settings.as_args(&defaults()), |_| None).unwrap();
        assert_eq!(again, settings);
    }
}
