//! `reforged-host update`: the Node kit's `update()` in `cli.ts`, with [`crate::selfupdate`].
//!
//! It changed meaning (RH5, section 6): from a `git` fast-forward of the checkout to the server
//! release (RD8). It looks for the newest release for this platform, downloads it, checks it against
//! `SHA256SUMS.txt` and its build provenance (with `gh`, when it is there), installs it over this
//! folder and runs setup again with the new `reforged-host`. A Node-kit host (a git checkout with no
//! `release.json`) moves over the same way: run a release's `reforged-host` from the checkout's
//! folder once.
//!
//! `--check` only compares, `--json` ends with one JSON line in the Node kit's shape (the launcher's
//! Host page reads `behind`, then `updated`, `to`, `files` and `runtimeChanged`, or `error`), and
//! `--no-setup` stops after installing.

use std::path::{Path, PathBuf};
use std::process::ExitCode;

use serde_json::{Value, json};

use crate::Outcome;
use crate::cli::UpdateArgs;
use crate::error::HostError;
use crate::hostconfig::has_host_config;
use crate::paths::Paths;
use crate::selfupdate::{
    self, Attestation, Candidate, Manifest, Source, UpdateError, UpdateResult, Wanted, find_release, installing_marker,
    read_manifest,
};

/// What `update` asks of this PC: where releases are, the attestation check and setup.
pub trait UpdateEnv {
    fn platform(&self) -> Result<&'static str, HostError>;
    fn source(&self, installed: Option<&Manifest>) -> Source;
    fn verify_attestation(&self, archive: &Path, candidate: &Candidate, log_file: &Path) -> UpdateResult<Attestation>;
    /// `git rev-parse --short=12 HEAD` of a checkout with no release, or "".
    fn checkout_commit(&self, root: &Path) -> String;
    /// Runs `program setup` in `root`; returns its exit code.
    fn run_setup(&self, program: &Path, root: &Path) -> Result<i32, HostError>;
}

pub struct System;

impl UpdateEnv for System {
    fn platform(&self) -> Result<&'static str, HostError> {
        selfupdate::this_platform()
    }

    fn source(&self, installed: Option<&Manifest>) -> Source {
        Source::from_env(installed)
    }

    fn verify_attestation(&self, archive: &Path, candidate: &Candidate, log_file: &Path) -> UpdateResult<Attestation> {
        selfupdate::verify_attestation(archive, candidate, log_file)
    }

    fn checkout_commit(&self, root: &Path) -> String {
        if !root.join(".git").exists() {
            return String::new();
        }
        crate::run::capture("git", &["rev-parse", "--short=12", "HEAD"], Some(root))
            .ok()
            .flatten()
            .unwrap_or_default()
    }

    fn run_setup(&self, program: &Path, root: &Path) -> Result<i32, HostError> {
        let status = std::process::Command::new(program)
            .arg("setup")
            .current_dir(root)
            .status()
            .map_err(|e| HostError::io(program, e))?;
        Ok(status.code().unwrap_or(1))
    }
}

pub fn run(paths: &Paths, args: &UpdateArgs) -> Outcome {
    run_with(paths, args, &System, &mut |line| println!("{line}"))
}

/// The server's `reforged-host` once the release is installed.
pub fn release_program(root: &Path) -> PathBuf {
    root.join("bin")
        .join(format!("reforged-host{}", std::env::consts::EXE_SUFFIX))
}

fn up_to_date(installed: Option<&Manifest>, interrupted: bool, platform: &str, candidate: &Candidate) -> bool {
    let Some(installed) = installed else { return false };
    if interrupted || installed.platform != platform {
        return false;
    }
    match (
        selfupdate::Version::parse(&installed.version),
        selfupdate::Version::parse(&candidate.version),
    ) {
        (Some(have), Some(offered)) => offered <= have,
        _ => installed.version == candidate.version,
    }
}

/// How the server is described: its release, or the checkout's commit.
fn head(installed: Option<&Manifest>, commit: &str) -> String {
    match installed {
        Some(m) => m.version.clone(),
        None => commit.to_owned(),
    }
}

pub fn run_with(paths: &Paths, args: &UpdateArgs, env: &dyn UpdateEnv, out: &mut dyn FnMut(&str)) -> Outcome {
    match update(paths, args, env, out) {
        Ok(code) => Ok(code),
        Err(UpdateError::Problem(problem)) => {
            if args.json {
                out(&json!({ "error": problem.code(), "detail": problem.detail() }).to_string());
            }
            Err(HostError::message(problem.message()))
        }
        Err(UpdateError::Host(e)) => Err(e),
    }
}

fn update(paths: &Paths, args: &UpdateArgs, env: &dyn UpdateEnv, out: &mut dyn FnMut(&str)) -> UpdateResult<ExitCode> {
    let root = &paths.repo;
    let platform = env.platform()?;
    let installed = read_manifest(root)?;
    let interrupted = installing_marker(root).exists();
    let commit = if installed.is_none() {
        env.checkout_commit(root)
    } else {
        String::new()
    };
    let source = env.source(installed.as_ref());
    let current = installed.as_ref().map(|m| m.version.as_str());
    let candidate = find_release(&source, platform, Wanted::Newest(current))?;
    let fresh = up_to_date(installed.as_ref(), interrupted, platform, &candidate);
    let summary = |updated: Option<Value>| {
        let mut v = json!({
            "branch": "server release",
            "upstream": source.describe(),
            "head": head(installed.as_ref(), &commit),
            "remoteHead": candidate.version,
            "behind": if fresh { 0 } else { 1 },
            "ahead": 0,
            "platform": platform,
            "installed": installed.as_ref().map(|m| m.version.clone()),
            "available": candidate.version,
        });
        if let (Some(extra), Some(map)) = (updated, v.as_object_mut()) {
            for (k, val) in extra.as_object().into_iter().flatten() {
                map.insert(k.clone(), val.clone());
            }
        }
        v
    };
    let describe = || match (&installed, fresh) {
        (Some(m), true) => format!("Up to date: servers {} ({platform}).", m.version),
        (Some(m), false) if interrupted => format!(
            "The last update stopped before it was complete: servers {} ({platform}) install again.",
            candidate.version
        ),
        (Some(m), false) => format!(
            "Servers {} are out for {platform} ({} -> {}).",
            candidate.version, m.version, candidate.version
        ),
        (None, _) => format!(
            "This server runs from a git checkout{}: update moves it to the server release {} ({platform}).",
            if commit.is_empty() {
                String::new()
            } else {
                format!(" ({commit})")
            },
            candidate.version
        ),
    };

    if args.check {
        out(&describe());
        if args.json {
            out(&summary(None).to_string());
        }
        return Ok(ExitCode::SUCCESS);
    }

    out(&format!("Looking for the server release at {}...", source.describe()));
    if fresh {
        out(&describe());
        if args.json {
            let same = head(installed.as_ref(), &commit);
            out(&summary(Some(json!({ "updated": false, "from": same, "to": same, "files": 0, "runtimeChanged": false, "hostKitDeps": false }))).to_string());
        }
        return Ok(ExitCode::SUCCESS);
    }
    out(&describe());

    let work = paths.workspace.join("update");
    let log_file = paths.logs.join("update.log");
    out(&format!("- downloading {}", candidate.archive));
    let archive = selfupdate::fetch(&candidate, &work)?;
    out(&format!(
        "- {}: SHA-256 checked against {}",
        candidate.archive,
        selfupdate::SUMS_FILE
    ));
    match env.verify_attestation(&archive, &candidate, &log_file)? {
        Attestation::Verified => out("- build provenance: verified (gh attestation verify)"),
        Attestation::Skipped(why) => out(&format!("- build provenance: not checked ({why})")),
    }
    let staged = selfupdate::extract(&archive, &work.join("staging"))?;
    let (manifest, report) = selfupdate::install(&staged, root, installed.as_ref())?;
    let _ = std::fs::remove_dir_all(work.join("staging"));
    let _ = std::fs::remove_file(&archive);

    let from = report.from.clone().unwrap_or_else(|| head(None, &commit));
    out(&format!(
        "Updated to servers {}{} ({} file{} written{}).",
        report.to,
        if from.is_empty() {
            String::new()
        } else {
            format!(" from {from}")
        },
        report.files,
        if report.files == 1 { "" } else { "s" },
        if report.removed > 0 {
            format!(", {} removed", report.removed)
        } else {
            String::new()
        }
    ));
    match report.runtime_changed {
        Some(true) => out(
            "The game runtime changed: build MysticParadox.dll again and publish it (reforged-host publish-runtime) for the players.",
        ),
        None => out(
            "If the game runtime's source changed since your build, build MysticParadox.dll again and publish it (reforged-host publish-runtime).",
        ),
        Some(false) => {}
    }
    let program = release_program(root);
    if let Ok(me) = std::env::current_exe()
        && me.parent() != program.parent()
    {
        out(&format!(
            "From now on run {} ({} is not needed any more).",
            program.display(),
            me.display()
        ));
    }
    if args.json {
        out(&summary(Some(json!({
            "updated": true,
            "from": from,
            "to": manifest.version,
            "commit": manifest.short_commit(),
            "files": report.files,
            "removed": report.removed,
            "runtimeChanged": report.runtime_changed == Some(true),
            "hostKitDeps": false,
            "behind": 0,
            "head": manifest.version,
        })))
        .to_string());
    }
    if args.no_setup {
        return Ok(ExitCode::SUCCESS);
    }
    if !has_host_config(paths) {
        out("Now run reforged-host setup.");
        return Ok(ExitCode::SUCCESS);
    }
    out("\nRunning setup again with this server's settings (reforged-host setup)...");
    let code = env.run_setup(&program, root)?;
    if code != 0 {
        return Err(HostError::message(format!("setup failed: exit {code}")).into());
    }
    Ok(ExitCode::SUCCESS)
}
