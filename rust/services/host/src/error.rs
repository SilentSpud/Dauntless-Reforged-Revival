//! The one error type of the host kit. `main` prints it as the Node kit's `fail()` did
//! (`\nError: <message>\n` on stderr, exit code 1).

use std::path::{Path, PathBuf};

#[derive(Debug, thiserror::Error)]
pub enum HostError {
    /// A command or helper the skeleton (RH1) registered and a later row fills.
    #[error("{what} is not written yet in reforged-host (MIGRATION_PLAN.md {row})")]
    NotImplemented { what: &'static str, row: &'static str },
    /// A problem told to the host, worded as the Node kit words it.
    #[error("{0}")]
    Message(String),
    #[error("{}: {source}", path.display())]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
}

impl HostError {
    pub fn message(text: impl Into<String>) -> Self {
        Self::Message(text.into())
    }

    pub fn io(path: impl AsRef<Path>, source: std::io::Error) -> Self {
        Self::Io {
            path: path.as_ref().to_path_buf(),
            source,
        }
    }
}

/// What a stub returns: `what` names the command or helper, `row` the MIGRATION_PLAN.md row that fills it.
pub fn not_implemented<T>(what: &'static str, row: &'static str) -> Result<T, HostError> {
    Err(HostError::NotImplemented { what, row })
}
