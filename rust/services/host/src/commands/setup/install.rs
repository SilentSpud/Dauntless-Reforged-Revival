//! Setup's step 2: the services' code. Today, as in the Node kit, each one is installed and built
//! from this checkout: `npm ci` and `npm run build` in `paradox/backend`, `paradox/director` and
//! the Node gateway and content server (`npmInstallAndBuild()` in `cli.ts`).
//!
//! Seam for RH5: setup's install step from the server release (RD8) goes here as another
//! [`ServiceSource`]; it downloads and checks the release archive instead of building. Not written
//! here.

use std::path::Path;

use super::paradox::Layout;
use crate::error::HostError;
use crate::run::{RunOptions, run};

/// Where setup takes the services from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ServiceSource {
    /// Built from this checkout with npm (the Node kit's way, until RH5).
    Checkout,
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
