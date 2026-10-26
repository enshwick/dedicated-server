//! `cargo xtask dist`: the Windows archive, `enshwick-server-windows.zip`.
//!
//! The archive holds one folder, `enshwick-server/`, with the program, the
//! loader and mods it installs (`bundle/`, from the loader release that
//! `image.toml` pins, its provenance checked) and the license. The PDB goes
//! beside the archive. Off Windows the program is built through
//! `cargo xwin`. `--loader <zip>` takes a loader archive built locally.

use std::io::{Cursor, Write as _};
use std::path::{Path, PathBuf};

use crate::image::{create_dir, extract, fetch_loader, files, pins, run_tool};
use crate::{USAGE, WINDOWS_TARGET, cargo, say};

/// The archive's file name, and the folder inside it.
const ARCHIVE: &str = "enshwick-server-windows.zip";
const FOLDER: &str = "enshwick-server";

pub(crate) fn run(args: &[String]) -> Result<(), String> {
    let mut out = PathBuf::from("target/dist");
    let mut loader = None;
    let mut args = args.iter();
    while let Some(arg) = args.next() {
        let value = args.next().ok_or(USAGE)?;
        match arg.as_str() {
            "--out" => out = PathBuf::from(value),
            "--loader" => loader = Some(PathBuf::from(value)),
            _ => return Err(USAGE.to_owned()),
        }
    }
    let mut command = Vec::new();
    if !cfg!(windows) {
        command.push("xwin");
    }
    command.extend([
        "build",
        "--release",
        "--locked",
        "--package",
        "enshwick-server",
        "--target",
        WINDOWS_TARGET,
    ]);
    run_tool(
        &cargo(),
        &command
            .iter()
            .map(|word| (*word).to_owned())
            .collect::<Vec<_>>(),
    )?;
    let built = Path::new("target").join(WINDOWS_TARGET).join("release");

    let work = PathBuf::from("target/dist-work");
    create_dir(&work)?;
    let archive = match loader {
        Some(archive) => archive,
        None => fetch_loader(&pins()?.loader, &work)?.0,
    };
    let bundle = extract(&archive, &work.join("bundle"))?;

    let mut entries = vec![
        (
            format!("{FOLDER}/enshwick-server.exe"),
            read(&built.join("enshwick-server.exe"))?,
        ),
        (format!("{FOLDER}/LICENSE"), read(Path::new("LICENSE"))?),
    ];
    for file in files(&bundle)? {
        let name = file
            .components()
            .map(|part| part.as_os_str().to_string_lossy())
            .collect::<Vec<_>>()
            .join("/");
        entries.push((
            format!("{FOLDER}/bundle/{name}"),
            read(&bundle.join(&file))?,
        ));
    }
    create_dir(&out)?;
    let path = out.join(ARCHIVE);
    std::fs::write(&path, zip(&entries)?)
        .map_err(|error| format!("{}: {error}", path.display()))?;
    let pdb = out.join("enshwick-server.pdb");
    std::fs::copy(built.join("enshwick_server.pdb"), &pdb)
        .map_err(|error| format!("{}: {error}", pdb.display()))?;
    say(&format!("wrote {} and {}", path.display(), pdb.display()));
    Ok(())
}

fn read(path: &Path) -> Result<Vec<u8>, String> {
    std::fs::read(path).map_err(|error| format!("{}: {error}", path.display()))
}

/// The archive of `entries` in their order and with the fixed time of the
/// format's epoch, so the same files give the same bytes.
fn zip(entries: &[(String, Vec<u8>)]) -> Result<Vec<u8>, String> {
    let mut archive = zip::ZipWriter::new(Cursor::new(Vec::new()));
    let options = zip::write::SimpleFileOptions::default()
        .compression_method(zip::CompressionMethod::Deflated)
        .last_modified_time(zip::DateTime::default());
    for (path, content) in entries {
        archive
            .start_file(path.as_str(), options)
            .and_then(|()| archive.write_all(content).map_err(Into::into))
            .map_err(|error| format!("{path}: {error}"))?;
    }
    archive
        .finish()
        .map(Cursor::into_inner)
        .map_err(|error| error.to_string())
}
