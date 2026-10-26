//! Repository tasks, run as `cargo xtask <task>`.
//!
//! `cargo xtask check` runs the compiling checks a change needs before it
//! merges; the git hooks run only the checks that compile nothing. Clippy,
//! the tests and the doctests run on the host; off Windows clippy also runs
//! for Windows through `cargo xwin`, beside the host checks. `--all` adds
//! formatting, rustdoc and cargo-deny.
//!
//! `cargo xtask image` builds the Linux image without a container engine
//! (see [`image`]).

#![forbid(unsafe_code)]

mod image;

use std::io::Write as _;
use std::process::{Command, ExitCode};
use std::thread;
use std::time::Instant;

const USAGE: &str = "usage: cargo xtask check [--all] [--base <git ref>] \
                     | cargo xtask image [--tag <reference>]... [--output <file>] [--digest-file <file>] [--loader <zip>]";
const WINDOWS_TARGET: &str = "x86_64-pc-windows-msvc";

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match run(&args) {
        Ok(true) => ExitCode::SUCCESS,
        Ok(false) => ExitCode::FAILURE,
        Err(error) => {
            say(&error);
            ExitCode::FAILURE
        }
    }
}

fn run(args: &[String]) -> Result<bool, String> {
    let (task, rest) = args.split_first().ok_or(USAGE)?;
    match task.as_str() {
        "image" => image::run(rest).map(|()| true),
        "check" => {
            let mut all = false;
            let mut rest = rest.iter();
            while let Some(arg) = rest.next() {
                match arg.as_str() {
                    "--all" => all = true,
                    // `--windows` is what the shared CI passes for repositories
                    // with Windows-only crates; this one has none.
                    "--windows" => {}
                    // The repository is small: every change checks all of it.
                    "--base" => {
                        rest.next().ok_or(USAGE)?;
                    }
                    _ => return Err(USAGE.to_owned()),
                }
            }
            Ok(check(all))
        }
        _ => Err(USAGE.to_owned()),
    }
}

fn cargo() -> String {
    std::env::var("CARGO").unwrap_or_else(|_| "cargo".to_owned())
}

/// One cargo command line.
#[derive(Debug)]
struct Step {
    args: Vec<String>,
    env: Vec<(&'static str, String)>,
}

impl Step {
    fn new(words: &[&str]) -> Self {
        Self {
            args: words.iter().map(|w| (*w).to_owned()).collect(),
            env: Vec::new(),
        }
    }

    fn env(mut self, name: &'static str, value: impl Into<String>) -> Self {
        self.env.push((name, value.into()));
        self
    }

    fn line(&self) -> String {
        format!("cargo {}", self.args.join(" "))
    }

    /// Runs the step with its output shown, or captured when `quiet` (for a
    /// step running beside others) and shown only when it fails.
    fn run(&self, quiet: bool) -> bool {
        let started = Instant::now();
        let mut command = Command::new(cargo());
        command
            .args(&self.args)
            .envs(self.env.iter().map(|(name, value)| (name, value)));
        let (ok, output) = if quiet {
            match command.output() {
                Ok(output) => (
                    output.status.success(),
                    [output.stdout, output.stderr].concat(),
                ),
                Err(error) => (false, error.to_string().into_bytes()),
            }
        } else {
            say(&format!("> {}", self.line()));
            match command.status() {
                Ok(status) => (status.success(), Vec::new()),
                Err(error) => (false, error.to_string().into_bytes()),
            }
        };
        let seconds = started.elapsed().as_secs_f32();
        if ok {
            say(&format!("ok in {seconds:.1} s: {}", self.line()));
        } else {
            let _shown = std::io::stderr().write_all(&output);
            say(&format!("FAILED in {seconds:.1} s: {}", self.line()));
        }
        ok
    }
}

fn say(text: &str) {
    let _shown = writeln!(std::io::stderr(), "xtask: {text}");
}

/// Runs the host steps one after another (they share the build lock) and
/// the Windows lint beside them; the first failing host step ends the host
/// checks.
fn check(all: bool) -> bool {
    let mut steps = Vec::new();
    if all {
        steps.push(Step::new(&["fmt", "--all", "--check"]));
    }
    steps.push(Step::new(&[
        "clippy",
        "--workspace",
        "--all-targets",
        "--",
        "-D",
        "warnings",
    ]));
    steps.push(Step::new(&[
        "nextest",
        "run",
        "--workspace",
        "--no-tests=pass",
    ]));
    if all {
        steps.push(
            Step::new(&[
                "doc",
                "--workspace",
                "--no-deps",
                "--document-private-items",
            ])
            .env("RUSTDOCFLAGS", "-D warnings"),
        );
        steps.push(Step::new(&["deny", "check"]));
    }
    let windows = (!cfg!(windows)).then(|| {
        Step::new(&[
            "xwin",
            "clippy",
            "--target",
            WINDOWS_TARGET,
            "--package",
            "enshwick-server",
            "--all-targets",
            "--",
            "-D",
            "warnings",
        ])
    });
    if let Some(step) = &windows {
        say(&format!("> {} (beside the host checks)", step.line()));
    }
    thread::scope(|scope| {
        let windows = windows.as_ref().map(|step| scope.spawn(|| step.run(true)));
        let host_passed = steps.iter().all(|step| step.run(false));
        let windows_passed = windows.is_none_or(|handle| handle.join().unwrap_or(false));
        host_passed && windows_passed
    })
}
