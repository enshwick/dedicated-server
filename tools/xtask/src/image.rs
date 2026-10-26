//! `cargo xtask image`: the Linux image, assembled without a container
//! engine.
//!
//! Nothing runs inside the image while it is built, so no Docker daemon and
//! no privileges are needed: the launcher is built as a static binary, the
//! pinned Proton build and the platform's loader archive are checked and
//! repacked into two layers with fixed owners and times, and `crane` appends
//! them to the pinned base image and sets the entry point. The sources are
//! pinned in `image.toml`. The game itself is never in the image: the
//! launcher downloads it through SteamCMD when the container starts.
//!
//! `--tag` pushes to a registry (`crane auth login` first); `--output` writes
//! a tarball that `docker load` reads. `--digest-file` receives the pushed
//! image's digest, for provenance. `--loader` takes a loader archive built
//! locally, such as by the platform's `cargo xtask bundle`, in place of the
//! pinned release.

use std::collections::BTreeSet;
use std::fs::File;
use std::io::{BufReader, BufWriter, Read as _, Write as _};
use std::path::{Path, PathBuf};
use std::process::Command;

use serde::Deserialize;
use sha2::{Digest as _, Sha512};

use crate::{USAGE, cargo, say};

/// The launcher's target: static, so it runs on any base image.
const LAUNCHER_TARGET: &str = "x86_64-unknown-linux-musl";
/// The user the server runs as; the data volume belongs to it.
const USER: u32 = 1000;
/// The server's query port, the one port a host publishes.
const QUERY_PORT: &str = "15637/udp";
/// Where the image keeps the server, its saves and the Wine prefix.
const DATA: &str = "/data";
const SOURCE: &str = "https://github.com/enshwick/dedicated-server";

/// `image.toml`.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Pins {
    base: String,
    proton: Proton,
    pub(crate) loader: Loader,
}

/// Reads `image.toml`.
pub(crate) fn pins() -> Result<Pins, String> {
    toml::from_str(&read_text(Path::new("image.toml"))?)
        .map_err(|error| format!("image.toml: {error}"))
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct Proton {
    version: String,
    url: String,
    sha512: String,
    #[serde(default)]
    exclude: Vec<String>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
#[serde(rename_all = "kebab-case")]
pub(crate) struct Loader {
    repository: String,
    release: String,
    asset: String,
    signer_workflow: String,
}

/// Where the image goes.
#[derive(Debug, Default)]
struct Destination {
    tags: Vec<String>,
    output: Option<PathBuf>,
    digest_file: Option<PathBuf>,
    /// A loader archive built locally, in place of the pinned release.
    loader: Option<PathBuf>,
}

pub(crate) fn run(args: &[String]) -> Result<(), String> {
    let mut destination = Destination::default();
    let mut args = args.iter();
    while let Some(arg) = args.next() {
        let value = args.next().ok_or(USAGE)?;
        match arg.as_str() {
            "--tag" => destination.tags.push(value.clone()),
            "--output" => destination.output = Some(PathBuf::from(value)),
            "--digest-file" => destination.digest_file = Some(PathBuf::from(value)),
            "--loader" => destination.loader = Some(PathBuf::from(value)),
            _ => return Err(USAGE.to_owned()),
        }
    }
    if destination.tags.is_empty() && destination.output.is_none() {
        destination.output = Some(PathBuf::from("target/image/dedicated-server.tar"));
    }

    let pins = pins()?;
    let work = PathBuf::from("target/image");
    let cache = std::env::var_os("ENSHWICK_IMAGE_CACHE")
        .map_or_else(|| PathBuf::from("target/image-cache"), PathBuf::from);
    create_dir(&work)?;
    create_dir(&cache)?;
    let mtime = source_date()?;

    let launcher = build_launcher()?;
    let proton = proton_layer(&pins.proton, &cache)?;
    let (archive, loader_commit) = match &destination.loader {
        Some(archive) => (archive.clone(), "local".to_owned()),
        None => fetch_loader(&pins.loader, &work)?,
    };
    let bundle = extract(&archive, &work.join("bundle"))?;
    let app = work.join("app.tar");
    app_layer(&launcher, &bundle, &app, mtime)?;

    let revision = git(&["rev-parse", "HEAD"])?;
    let mut crane = mutate_args(&pins, &[&proton, &app], &revision, &loader_commit);
    let (first, rest) = match destination.tags.split_first() {
        Some((first, rest)) => (first.clone(), rest),
        None => ("dedicated-server:local".to_owned(), &[][..]),
    };
    crane.extend(["--tag".to_owned(), first.clone()]);
    if let Some(output) = &destination.output {
        if let Some(dir) = output.parent() {
            create_dir(dir)?;
        }
        crane.extend(["--output".to_owned(), output.display().to_string()]);
    }
    let pushed = run_tool(&crane_bin(), &crane)?;
    if let Some(output) = &destination.output {
        say(&format!("wrote {} ({first})", output.display()));
        return Ok(());
    }
    for tag in rest {
        run_tool(
            &crane_bin(),
            &["tag".to_owned(), first.clone(), tag_name(tag)],
        )?;
    }
    say(&format!("pushed {}", pushed.trim()));
    if let Some(file) = &destination.digest_file {
        let digest = run_tool(&crane_bin(), &["digest".to_owned(), first])?;
        std::fs::write(file, digest.trim())
            .map_err(|error| format!("{}: {error}", file.display()))?;
    }
    Ok(())
}

/// `crane mutate`'s arguments up to the destination: the base, the layers,
/// how the container starts and the labels.
fn mutate_args(pins: &Pins, layers: &[&Path], revision: &str, loader_commit: &str) -> Vec<String> {
    let mut crane = vec![
        "mutate".to_owned(),
        pins.base.clone(),
        "--platform".to_owned(),
        "linux/amd64".to_owned(),
    ];
    for layer in layers {
        crane.extend(["--append".to_owned(), layer.display().to_string()]);
    }
    crane.extend(
        [
            "--entrypoint",
            "/opt/enshwick/bin/enshwick-server",
            "--cmd",
            "run",
            "--user",
            &format!("{USER}:{USER}"),
            "--workdir",
            DATA,
            "--env",
            &format!("HOME={DATA}/home"),
            "--exposed-ports",
            QUERY_PORT,
        ]
        .map(str::to_owned),
    );
    for (key, value) in [
        ("org.opencontainers.image.source", SOURCE),
        ("org.opencontainers.image.revision", revision),
        (
            "org.opencontainers.image.description",
            "Enshrouded dedicated server with the Enshwick loader, under Proton",
        ),
        ("org.opencontainers.image.licenses", "MIT"),
        ("org.opencontainers.image.base.name", &pins.base),
        ("dev.enshwick.proton", &pins.proton.version),
        ("dev.enshwick.loader.revision", loader_commit),
    ] {
        crane.extend(["--label".to_owned(), format!("{key}={value}")]);
    }
    crane
}

/// The tag part of a full reference, for `crane tag`.
fn tag_name(reference: &str) -> String {
    reference
        .rsplit_once(':')
        .filter(|(_, tag)| !tag.contains('/'))
        .map_or_else(|| reference.to_owned(), |(_, tag)| tag.to_owned())
}

fn crane_bin() -> String {
    std::env::var("CRANE").unwrap_or_else(|_| "crane".to_owned())
}

/// The time every layer entry carries: `SOURCE_DATE_EPOCH`, or the commit's.
pub(crate) fn source_date() -> Result<u64, String> {
    let text = match std::env::var("SOURCE_DATE_EPOCH") {
        Ok(text) => text,
        Err(_) => git(&["log", "-1", "--format=%ct"])?,
    };
    text.trim()
        .parse()
        .map_err(|_| format!("not a time in seconds: {text:?}"))
}

fn build_launcher() -> Result<PathBuf, String> {
    let args = [
        "build",
        "--release",
        "--locked",
        "--package",
        "enshwick-server",
        "--target",
        LAUNCHER_TARGET,
    ];
    run_tool(&cargo(), &args.map(str::to_owned))?;
    Ok(Path::new("target")
        .join(LAUNCHER_TARGET)
        .join("release/enshwick-server"))
}

/// The Proton layer, made once per pin and kept in the cache. Its entries
/// carry the time 0, not the commit's, so every build of a pin gives the
/// same layer and registries keep one copy.
fn proton_layer(proton: &Proton, cache: &Path) -> Result<PathBuf, String> {
    let mut key = Sha512::new();
    key.update(proton.sha512.as_bytes());
    for path in &proton.exclude {
        key.update(path.as_bytes());
        key.update([0]);
    }
    let layer = cache.join(format!(
        "{}-{}.tar",
        proton.version,
        &hex(&key.finalize())[..16]
    ));
    if layer.exists() {
        say(&format!("reusing {}", layer.display()));
        return Ok(layer);
    }
    let archive = cache.join(format!("{}.tar.gz", proton.version));
    if !archive.exists() || sha512_file(&archive)? != proton.sha512 {
        say(&format!("downloading {}", proton.url));
        let part = archive.with_extension("part");
        run_tool(
            "curl",
            &[
                "--proto",
                "=https",
                "-fsSL",
                "-o",
                &part.display().to_string(),
                &proton.url,
            ]
            .map(str::to_owned),
        )?;
        let got = sha512_file(&part)?;
        if got != proton.sha512 {
            return Err(format!(
                "{}: SHA-512 {got}, image.toml pins {}",
                proton.url, proton.sha512
            ));
        }
        rename(&part, &archive)?;
    }
    say(&format!("repacking {} into a layer", proton.version));
    let part = layer.with_extension("part");
    repack_proton(&archive, &proton.exclude, &part, 0)?;
    rename(&part, &layer)?;
    Ok(layer)
}

/// Copies Proton's archive under `opt/proton/`, without its top folder and
/// the excluded paths, owned by root.
fn repack_proton(archive: &Path, exclude: &[String], out: &Path, mtime: u64) -> Result<(), String> {
    let input = File::open(archive).map_err(|error| format!("{}: {error}", archive.display()))?;
    let mut input = tar::Archive::new(flate2::read::GzDecoder::new(BufReader::new(input)));
    let mut layer = Layer::create(out, mtime)?;
    let entries = input.entries().map_err(|error| error.to_string())?;
    for entry in entries {
        let mut entry = entry.map_err(|error| error.to_string())?;
        let path = entry
            .path()
            .map_err(|error| error.to_string())?
            .into_owned();
        let inner: PathBuf = path.components().skip(1).collect();
        if inner.as_os_str().is_empty() || exclude.iter().any(|skip| inner.starts_with(skip)) {
            continue;
        }
        let to = Path::new("opt/proton").join(&inner);
        let mode = entry.header().mode().map_err(|error| error.to_string())?;
        match entry.header().entry_type() {
            tar::EntryType::Directory => layer.dir(&to, mode, 0)?,
            tar::EntryType::Regular => {
                let size = entry.header().size().map_err(|error| error.to_string())?;
                layer.file(&to, mode, size, &mut entry)?;
            }
            tar::EntryType::Symlink => {
                let target = entry
                    .link_name()
                    .map_err(|error| error.to_string())?
                    .ok_or_else(|| format!("{}: a link without a target", path.display()))?
                    .into_owned();
                layer.symlink(&to, &target)?;
            }
            other => return Err(format!("{}: unexpected entry {other:?}", path.display())),
        }
    }
    layer.finish()
}

/// Downloads the loader archive and checks its build provenance; returns
/// the archive and the commit it was built from.
pub(crate) fn fetch_loader(loader: &Loader, work: &Path) -> Result<(PathBuf, String), String> {
    let download = work.join("loader-download");
    let _gone = std::fs::remove_dir_all(&download);
    create_dir(&download)?;
    run_tool(
        "gh",
        &[
            "release",
            "download",
            &loader.release,
            "--repo",
            &loader.repository,
            "--pattern",
            &loader.asset,
            "--dir",
            &download.display().to_string(),
        ]
        .map(str::to_owned),
    )?;
    let archive = download.join(&loader.asset);
    run_tool(
        "gh",
        &[
            "attestation",
            "verify",
            &archive.display().to_string(),
            "--repo",
            &loader.repository,
            "--signer-workflow",
            &loader.signer_workflow,
        ]
        .map(str::to_owned),
    )?;
    let commit = run_tool(
        "gh",
        &[
            "release",
            "view",
            &loader.release,
            "--repo",
            &loader.repository,
            "--json",
            "targetCommitish",
            "--jq",
            ".targetCommitish",
        ]
        .map(str::to_owned),
    )?
    .trim()
    .to_owned();
    say(&format!("loader from {} at {commit}", loader.repository));
    Ok((archive, commit))
}

/// Extracts the loader archive into a fresh `dir`.
pub(crate) fn extract(archive: &Path, dir: &Path) -> Result<PathBuf, String> {
    let _gone = std::fs::remove_dir_all(dir);
    let file = File::open(archive).map_err(|error| format!("{}: {error}", archive.display()))?;
    zip::ZipArchive::new(file)
        .and_then(|mut zip| zip.extract(dir))
        .map_err(|error| format!("{}: {error}", archive.display()))?;
    Ok(dir.to_owned())
}

/// The launcher, the bundle and the data folder owned by the server's user.
fn app_layer(launcher: &Path, bundle: &Path, out: &Path, mtime: u64) -> Result<(), String> {
    let mut layer = Layer::create(out, mtime)?;
    let binary =
        std::fs::read(launcher).map_err(|error| format!("{}: {error}", launcher.display()))?;
    layer.file(
        Path::new("opt/enshwick/bin/enshwick-server"),
        0o755,
        u64::try_from(binary.len()).map_err(|error| error.to_string())?,
        binary.as_slice(),
    )?;
    for file in files(bundle)? {
        let content = std::fs::read(bundle.join(&file)).map_err(|error| error.to_string())?;
        layer.file(
            &Path::new("opt/enshwick/bundle").join(&file),
            0o644,
            u64::try_from(content.len()).map_err(|error| error.to_string())?,
            content.as_slice(),
        )?;
    }
    layer.dir(Path::new(DATA.trim_start_matches('/')), 0o755, USER)?;
    layer.finish()
}

/// An uncompressed layer tarball with fixed owners and times, whose parent
/// folders appear before their contents.
struct Layer {
    builder: tar::Builder<BufWriter<File>>,
    mtime: u64,
    dirs: BTreeSet<PathBuf>,
}

impl Layer {
    fn create(path: &Path, mtime: u64) -> Result<Self, String> {
        let file = File::create(path).map_err(|error| format!("{}: {error}", path.display()))?;
        Ok(Self {
            builder: tar::Builder::new(BufWriter::new(file)),
            mtime,
            dirs: BTreeSet::new(),
        })
    }

    fn header(&self, kind: tar::EntryType, mode: u32, owner: u32) -> tar::Header {
        let mut header = tar::Header::new_gnu();
        header.set_entry_type(kind);
        header.set_mode(mode);
        header.set_uid(owner.into());
        header.set_gid(owner.into());
        header.set_mtime(self.mtime);
        header.set_size(0);
        header
    }

    fn parents(&mut self, path: &Path) -> Result<(), String> {
        if let Some(parent) = path.parent().filter(|p| !p.as_os_str().is_empty()) {
            self.dir(parent, 0o755, 0)?;
        }
        Ok(())
    }

    fn dir(&mut self, path: &Path, mode: u32, owner: u32) -> Result<(), String> {
        if self.dirs.contains(path) {
            return Ok(());
        }
        self.parents(path)?;
        self.dirs.insert(path.to_owned());
        let mut header = self.header(tar::EntryType::Directory, mode, owner);
        self.builder
            .append_data(&mut header, path, std::io::empty())
            .map_err(|error| format!("{}: {error}", path.display()))
    }

    fn file(
        &mut self,
        path: &Path,
        mode: u32,
        size: u64,
        data: impl std::io::Read,
    ) -> Result<(), String> {
        self.parents(path)?;
        let mut header = self.header(tar::EntryType::Regular, mode, 0);
        header.set_size(size);
        self.builder
            .append_data(&mut header, path, data)
            .map_err(|error| format!("{}: {error}", path.display()))
    }

    fn symlink(&mut self, path: &Path, target: &Path) -> Result<(), String> {
        self.parents(path)?;
        let mut header = self.header(tar::EntryType::Symlink, 0o777, 0);
        self.builder
            .append_link(&mut header, path, target)
            .map_err(|error| format!("{}: {error}", path.display()))
    }

    fn finish(self) -> Result<(), String> {
        self.builder
            .into_inner()
            .and_then(|mut writer| writer.flush())
            .map_err(|error| error.to_string())
    }
}

/// Every file under `root`, relative to it, sorted.
pub(crate) fn files(root: &Path) -> Result<Vec<PathBuf>, String> {
    let mut found = Vec::new();
    let mut dirs = vec![PathBuf::new()];
    while let Some(dir) = dirs.pop() {
        let entries = std::fs::read_dir(root.join(&dir))
            .map_err(|error| format!("{}: {error}", root.join(&dir).display()))?;
        for entry in entries {
            let entry = entry.map_err(|error| error.to_string())?;
            let path = dir.join(entry.file_name());
            if entry
                .file_type()
                .map_err(|error| error.to_string())?
                .is_dir()
            {
                dirs.push(path);
            } else {
                found.push(path);
            }
        }
    }
    found.sort();
    Ok(found)
}

fn sha512_file(path: &Path) -> Result<String, String> {
    let mut file = File::open(path).map_err(|error| format!("{}: {error}", path.display()))?;
    let mut hasher = Sha512::new();
    let mut buffer = vec![0; 1 << 20];
    loop {
        let read = file.read(&mut buffer).map_err(|error| error.to_string())?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
    }
    Ok(hex(&hasher.finalize()))
}

fn hex(bytes: &[u8]) -> String {
    bytes
        .iter()
        .fold(String::with_capacity(bytes.len() * 2), |mut text, byte| {
            let _written = write_hex(&mut text, *byte);
            text
        })
}

fn write_hex(text: &mut String, byte: u8) -> std::fmt::Result {
    use std::fmt::Write as _;
    write!(text, "{byte:02x}")
}

pub(crate) fn read_text(path: &Path) -> Result<String, String> {
    std::fs::read_to_string(path).map_err(|error| format!("{}: {error}", path.display()))
}

pub(crate) fn create_dir(dir: &Path) -> Result<(), String> {
    std::fs::create_dir_all(dir).map_err(|error| format!("{}: {error}", dir.display()))
}

fn rename(from: &Path, to: &Path) -> Result<(), String> {
    std::fs::rename(from, to).map_err(|error| format!("{}: {error}", to.display()))
}

fn git(args: &[&str]) -> Result<String, String> {
    run_tool(
        "git",
        &args.iter().map(|arg| (*arg).to_owned()).collect::<Vec<_>>(),
    )
    .map(|out| out.trim().to_owned())
}

/// Runs `program`, showing its errors, and returns what it printed.
pub(crate) fn run_tool(program: &str, args: &[String]) -> Result<String, String> {
    let output = Command::new(program)
        .args(args)
        .stderr(std::process::Stdio::inherit())
        .output()
        .map_err(|error| format!("starting {program}: {error}"))?;
    if output.status.success() {
        Ok(String::from_utf8_lossy(&output.stdout).into_owned())
    } else {
        Err(format!("{program} {}: {}", args.join(" "), output.status))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_pins_parse() {
        let pins: Pins = toml::from_str(
            &read_text(Path::new(concat!(
                env!("CARGO_MANIFEST_DIR"),
                "/../../image.toml"
            )))
            .unwrap(),
        )
        .unwrap();
        assert!(
            pins.base.contains("@sha256:"),
            "the base is pinned by digest"
        );
        assert_eq!(pins.proton.sha512.len(), 128);
        assert!(pins.proton.url.contains(&pins.proton.version));
    }

    #[test]
    fn tag_names_come_from_full_references() {
        assert_eq!(tag_name("ghcr.io/enshwick/dedicated-server:edge"), "edge");
        assert_eq!(tag_name("localhost:5000/server:1"), "1");
        assert_eq!(tag_name("localhost:5000/server"), "localhost:5000/server");
    }

    #[test]
    fn layers_put_parents_first_and_fix_owners() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("layer.tar");
        let mut layer = Layer::create(&path, 7).unwrap();
        layer
            .file(Path::new("opt/a/b"), 0o644, 2, &b"hi"[..])
            .unwrap();
        layer.dir(Path::new("data"), 0o755, USER).unwrap();
        layer.symlink(Path::new("opt/a/c"), Path::new("b")).unwrap();
        layer.finish().unwrap();

        let mut archive = tar::Archive::new(File::open(&path).unwrap());
        let entries: Vec<(String, u64, u64)> = archive
            .entries()
            .unwrap()
            .map(|entry| {
                let entry = entry.unwrap();
                let header = entry.header();
                (
                    entry.path().unwrap().display().to_string(),
                    header.uid().unwrap(),
                    header.mtime().unwrap(),
                )
            })
            .collect();
        assert_eq!(
            entries,
            [
                ("opt".to_owned(), 0, 7),
                ("opt/a".to_owned(), 0, 7),
                ("opt/a/b".to_owned(), 0, 7),
                ("data".to_owned(), 1000, 7),
                ("opt/a/c".to_owned(), 0, 7),
            ]
        );
    }
}
