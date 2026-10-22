//! Running other programs (`legacy/ReforgedHost/src/run.ts`): always with an argument list (never a
//! shell string), output into a log file and, unless quiet, onto the console.
//!
//! Filled by RH2. Setup (RH3), game data (RH4) and update (RH5) call these signatures; a row that
//! needs them changed goes through the integration pass.

use std::path::{Path, PathBuf};

use crate::error::{HostError, not_implemented};

#[derive(Debug, Clone, Default)]
pub struct RunOptions {
    pub cwd: PathBuf,
    /// The child's whole environment; `None`: this process's.
    pub env: Option<Vec<(String, String)>>,
    pub log_file: Option<PathBuf>,
    pub quiet: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RunResult {
    /// -1 when the program could not start.
    pub code: i32,
    /// The line that names a failure, else the last line of output.
    pub last_line: Option<String>,
    /// The last 64 KiB of output.
    pub output: String,
}

/// Runs a program to its end.
pub fn run(_command: &str, _args: &[&str], _options: &RunOptions) -> Result<RunResult, HostError> {
    not_implemented("running a program", "RH2")
}

/// A quick call for checks (`git --version`, `tailscale ip -4`): its trimmed output, or `None` when
/// it fails or takes more than 20 s.
pub fn capture(_command: &str, _args: &[&str], _cwd: Option<&Path>) -> Result<Option<String>, HostError> {
    not_implemented("capturing a program's output", "RH2")
}
