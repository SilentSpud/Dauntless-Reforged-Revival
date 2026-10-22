//! Setup's step 2: the services' code, from the server release (RD8, RH5) or, in a developer's
//! checkout with no release, built from it as the Node kit did (`npm ci` and `npm run build` in
//! `paradox/backend`, `paradox/director` and the Node gateway and content server).
//!
//! A server installed from a release (`release.json` in its folder) checks every file of it against
//! `release.json`. When some are missing or changed, or the release is another platform's, it
//! downloads that release again (the same version), checks it against `SHA256SUMS.txt` and its
//! attestation, and installs it, as `update` does ([`crate::selfupdate`]). Moving to a newer release
//! is `update`'s.

use std::path::Path;

use super::paradox::Layout;
use crate::error::HostError;
use crate::run::{RunOptions, run};
use crate::selfupdate::{self, Attestation, Manifest, Source, Wanted};

/// Where setup takes the services from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ServiceSource {
    /// Built from this checkout with npm (a developer's checkout, or a Node-kit host before `update`).
    Checkout,
    /// The server release installed here (`release.json`).
    Release,
}

impl ServiceSource {
    pub fn of(root: &Path) -> Self {
        if root.join(selfupdate::RELEASE_FILE).is_file() {
            Self::Release
        } else {
            Self::Checkout
        }
    }
}

/// The server's folder.
fn root_of(l: &Layout) -> &Path {
    l.paradox.parent().unwrap_or(Path::new("."))
}

/// Step 2 for this server: from its release, or built from its checkout.
pub fn install_services(l: &Layout, log_file: &Path, out: &mut dyn FnMut(&str)) -> Result<(), HostError> {
    let root = root_of(l);
    match ServiceSource::of(root) {
        ServiceSource::Checkout => build_from_checkout(l, log_file, out),
        ServiceSource::Release => {
            let platform = selfupdate::this_platform()?;
            let installed = selfupdate::read_manifest(root)?;
            let source = Source::from_env(installed.as_ref());
            install_from_release(root, installed.as_ref(), platform, &source, log_file, out)
        }
    }
}

/// Checks the installed release and puts it back when it is incomplete.
pub fn install_from_release(
    root: &Path,
    installed: Option<&Manifest>,
    platform: &str,
    source: &Source,
    log_file: &Path,
    out: &mut dyn FnMut(&str),
) -> Result<(), HostError> {
    let Some(installed) = installed else {
        return Err(HostError::message(format!(
            "no {} in {}",
            selfupdate::RELEASE_FILE,
            root.display()
        )));
    };
    let interrupted = selfupdate::installing_marker(root).exists();
    let mut damaged = selfupdate::damaged_files(root, installed)?;
    damaged.retain(|f| f != selfupdate::RELEASE_FILE);
    if installed.platform == platform && damaged.is_empty() && !interrupted {
        out(&format!(
            "- servers {} ({platform}, {}) from the release: every file checked",
            installed.version,
            installed.short_commit()
        ));
        return Ok(());
    }
    if installed.platform != platform {
        out(&format!(
            "- this folder holds the {} release: installing servers {} for {platform}",
            installed.platform, installed.version
        ));
    } else if interrupted {
        out(&format!(
            "- the last update stopped half way: installing servers {} again",
            installed.version
        ));
    } else {
        out(&format!(
            "- {} file{} of servers {} {} missing or changed (the first: {}): installing the release again",
            damaged.len(),
            if damaged.len() == 1 { "" } else { "s" },
            installed.version,
            if damaged.len() == 1 { "is" } else { "are" },
            damaged[0]
        ));
    }
    let candidate = find_and_fetch(root, installed, platform, source, log_file, out)?;
    out(&format!("- servers {candidate} installed"));
    Ok(())
}

fn find_and_fetch(
    root: &Path,
    installed: &Manifest,
    platform: &str,
    source: &Source,
    log_file: &Path,
    out: &mut dyn FnMut(&str),
) -> Result<String, HostError> {
    let candidate = selfupdate::find_release(source, platform, Wanted::Exactly(&installed.version))?;
    let work = root.join(".workspace").join("update");
    out(&format!(
        "- downloading {} from {}",
        candidate.archive,
        source.describe()
    ));
    let archive = selfupdate::fetch(&candidate, &work)?;
    out(&format!("- SHA-256 checked against {}", selfupdate::SUMS_FILE));
    match selfupdate::verify_attestation(&archive, &candidate, log_file)? {
        Attestation::Verified => out("- build provenance: verified (gh attestation verify)"),
        Attestation::Skipped(why) => out(&format!("- build provenance: not checked ({why})")),
    }
    let staged = selfupdate::extract(&archive, &work.join("staging"))?;
    // The files the host changed are the release's own here: setup puts them back. A release for
    // another platform is installed as over a checkout.
    let previous = (installed.platform == platform).then_some(installed);
    let (manifest, _) = selfupdate::install(&staged, root, previous).or_else(|e| match e {
        selfupdate::UpdateError::Problem(selfupdate::Problem::LocalChanges(_)) => {
            selfupdate::install(&staged, root, None)
        }
        e => Err(e),
    })?;
    let _ = std::fs::remove_dir_all(work.join("staging"));
    let _ = std::fs::remove_file(&archive);
    Ok(manifest.version)
}

/// The four Node services setup builds, with the names it prints.
pub fn services(l: &Layout) -> [(&Path, &'static str); 4] {
    [
        (l.backend_dir.as_path(), "backend"),
        (l.director_dir.as_path(), "Director"),
        (l.gateway_dir.as_path(), "gateway"),
        (l.content_dir.as_path(), "content server"),
    ]
}

fn npm(dir: &Path, args: &[&str], log_file: &Path, what: &str) -> Result<(), HostError> {
    let r = run(
        "npm",
        args,
        &RunOptions {
            cwd: dir.to_path_buf(),
            env: None,
            log_file: Some(log_file.to_path_buf()),
            quiet: true,
        },
    )?;
    if r.code != 0 {
        let why = r.last_line.unwrap_or_else(|| r.code.to_string());
        return Err(HostError::message(format!(
            "{what} failed in {}: {why} (full output in {})",
            dir.display(),
            log_file.display()
        )));
    }
    Ok(())
}

/// `npm ci` and `npm run build` in each service, stopping at the first that fails.
pub fn build_from_checkout(l: &Layout, log_file: &Path, out: &mut dyn FnMut(&str)) -> Result<(), HostError> {
    for (dir, label) in services(l) {
        out(&format!("- {label}: npm ci"));
        npm(dir, &["ci", "--no-audit", "--no-fund"], log_file, "npm ci")?;
        out(&format!("- {label}: npm run build"));
        npm(dir, &["run", "build"], log_file, "npm run build")?;
    }
    Ok(())
}
