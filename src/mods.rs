//! The loader and the bundled mods, copied from the image into the server.
//!
//! The bundle is the platform's archive as it extracts into a game folder:
//! the proxy DLL, the loader under `mods/enshwick/` and one `mods/<file>.dll`
//! per mod. Every file of the bundle is copied on each start, so a new image
//! brings its loader, except the mods not chosen; a bundled mod not chosen
//! is removed from the server, and DLLs the host put into `mods/` itself
//! stay as they are.

use std::path::{Path, PathBuf};

use crate::say;

/// The prefix of our mods' file names, which a name in `ENSHWICK_MODS` may
/// leave out.
const MOD_PREFIX: &str = "enshwick-";

/// Copies the bundle into `server` with the mods named in `chosen`, and
/// removes the bundled mods not chosen.
///
/// # Errors
///
/// A chosen name the bundle lacks, listing the names it has, or a file that
/// cannot be read, copied or removed.
pub(crate) fn install(bundle: &Path, server: &Path, chosen: &[String]) -> Result<(), String> {
    let files = files(bundle)?;
    let plan = plan(&files, chosen)?;
    for file in &plan.copy {
        let to = server.join(file);
        if let Some(dir) = to.parent() {
            std::fs::create_dir_all(dir).map_err(|error| format!("{}: {error}", dir.display()))?;
        }
        std::fs::copy(bundle.join(file), &to)
            .map_err(|error| format!("{}: {error}", to.display()))?;
    }
    for file in &plan.remove {
        let path = server.join(file);
        match std::fs::remove_file(&path) {
            Ok(()) => say(&format!(
                "removed {}, which ENSHWICK_MODS no longer names",
                file.display()
            )),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(format!("{}: {error}", path.display())),
        }
    }
    say(&format!(
        "installed the loader with {}",
        if plan.mods.is_empty() {
            "no bundled mods".to_owned()
        } else {
            plan.mods.join(", ")
        }
    ));
    Ok(())
}

/// What one start copies and removes, as paths relative to the bundle and
/// the server.
#[derive(Debug, PartialEq, Eq)]
struct Plan {
    copy: Vec<PathBuf>,
    remove: Vec<PathBuf>,
    /// The chosen mods by name, for the log.
    mods: Vec<String>,
}

/// Splits the bundle's `files` into what to copy and what to remove.
fn plan(files: &[PathBuf], chosen: &[String]) -> Result<Plan, String> {
    let mut plan = Plan {
        copy: Vec::new(),
        remove: Vec::new(),
        mods: Vec::new(),
    };
    let mut available = Vec::new();
    for file in files {
        let Some(name) = mod_name(file) else {
            plan.copy.push(file.clone());
            continue;
        };
        available.push(name.to_owned());
        if chosen.iter().any(|wanted| matches(wanted, name)) {
            plan.copy.push(file.clone());
            plan.mods.push(name.to_owned());
        } else {
            plan.remove.push(file.clone());
        }
    }
    let unknown: Vec<&str> = chosen
        .iter()
        .filter(|wanted| !available.iter().any(|name| matches(wanted, name)))
        .map(String::as_str)
        .collect();
    if !unknown.is_empty() {
        available.sort();
        return Err(format!(
            "ENSHWICK_MODS names {}, which this image does not have; it has: {}",
            unknown.join(", "),
            available.join(", ")
        ));
    }
    Ok(plan)
}

/// The mod's name when `file` is a mod, `mods/<name>.dll`: the file name
/// without the prefix our mods share.
fn mod_name(file: &Path) -> Option<&str> {
    let mut parts = file.components();
    let (Some(dir), Some(name), None) = (parts.next(), parts.next(), parts.next()) else {
        return None;
    };
    if dir.as_os_str() != "mods" {
        return None;
    }
    let stem = name.as_os_str().to_str()?.strip_suffix(".dll")?;
    Some(stem.strip_prefix(MOD_PREFIX).unwrap_or(stem))
}

/// Whether `wanted`, as the host wrote it, names the mod `name`.
fn matches(wanted: &str, name: &str) -> bool {
    wanted == name || wanted.strip_prefix(MOD_PREFIX) == Some(name)
}

/// Every file under `root`, relative to it, sorted.
fn files(root: &Path) -> Result<Vec<PathBuf>, String> {
    let mut found = Vec::new();
    let mut dirs = vec![PathBuf::new()];
    while let Some(dir) = dirs.pop() {
        let entries = std::fs::read_dir(root.join(&dir))
            .map_err(|error| format!("{}: {error}", root.join(&dir).display()))?;
        for entry in entries {
            let entry = entry.map_err(|error| error.to_string())?;
            let path = dir.join(entry.file_name());
            let kind = entry.file_type().map_err(|error| error.to_string())?;
            if kind.is_dir() {
                dirs.push(path);
            } else {
                found.push(path);
            }
        }
    }
    found.sort();
    Ok(found)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bundle() -> Vec<PathBuf> {
        [
            "LICENSE",
            "mods/enshwick-game-rates.dll",
            "mods/enshwick-optimizer.dll",
            "mods/enshwick/enshwick.dll",
            "powrprof.dll",
        ]
        .into_iter()
        .map(PathBuf::from)
        .collect()
    }

    fn names(names: &[&str]) -> Vec<String> {
        names.iter().map(|name| (*name).to_owned()).collect()
    }

    fn paths(paths: &[&str]) -> Vec<PathBuf> {
        paths.iter().map(PathBuf::from).collect()
    }

    #[test]
    fn the_loader_is_always_copied_and_unchosen_mods_removed() {
        let plan = plan(&bundle(), &names(&["optimizer"])).unwrap();
        assert_eq!(
            plan.copy,
            paths(&[
                "LICENSE",
                "mods/enshwick-optimizer.dll",
                "mods/enshwick/enshwick.dll",
                "powrprof.dll"
            ])
        );
        assert_eq!(plan.remove, paths(&["mods/enshwick-game-rates.dll"]));
        assert_eq!(plan.mods, ["optimizer"]);
    }

    #[test]
    fn a_name_may_carry_the_file_prefix() {
        let plan = plan(&bundle(), &names(&["enshwick-game-rates"])).unwrap();
        assert_eq!(plan.mods, ["game-rates"]);
    }

    #[test]
    fn no_names_leave_only_the_loader() {
        let plan = plan(&bundle(), &[]).unwrap();
        assert!(plan.mods.is_empty());
        assert_eq!(plan.remove.len(), 2);
    }

    #[test]
    fn an_unknown_name_lists_every_bundled_mod() {
        let error = plan(&bundle(), &names(&["optimiser"])).unwrap_err();
        assert_eq!(
            error,
            "ENSHWICK_MODS names optimiser, which this image does not have; it has: game-rates, optimizer"
        );
    }

    #[test]
    fn install_keeps_the_hosts_own_mods() {
        let bundle_dir = tempfile::tempdir().unwrap();
        let server = tempfile::tempdir().unwrap();
        for file in bundle() {
            let path = bundle_dir.path().join(&file);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(&path, file.to_string_lossy().as_bytes()).unwrap();
        }
        std::fs::create_dir_all(server.path().join("mods")).unwrap();
        std::fs::write(server.path().join("mods/enshwick-game-rates.dll"), b"old").unwrap();
        std::fs::write(server.path().join("mods/someone-elses.dll"), b"theirs").unwrap();

        install(bundle_dir.path(), server.path(), &names(&["optimizer"])).unwrap();

        assert_eq!(
            std::fs::read(server.path().join("mods/enshwick/enshwick.dll")).unwrap(),
            b"mods/enshwick/enshwick.dll"
        );
        assert!(server.path().join("mods/enshwick-optimizer.dll").exists());
        assert!(server.path().join("powrprof.dll").exists());
        assert!(!server.path().join("mods/enshwick-game-rates.dll").exists());
        assert_eq!(
            std::fs::read(server.path().join("mods/someone-elses.dll")).unwrap(),
            b"theirs"
        );
    }
}
