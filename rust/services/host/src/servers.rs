//! The servers built into `reforged-host`, one cargo feature each: `start`, the systemd units and the
//! containers run them as `reforged-host <server> ...`, so a release ships one binary.

use std::process::ExitCode;

use clap::Subcommand;

#[derive(Debug, Subcommand)]
pub enum Server {
    /// Run the gateway (start runs it for you)
    #[cfg(feature = "gateway")]
    Gateway(gateway::cli::Cli),
    /// Run the allowlist helper (start or the reforged-allowlist unit runs it for you)
    #[cfg(feature = "allowlist")]
    Allowlist(gateway::allowlist::cli::Cli),
    /// Run the content server (start runs it for you)
    #[cfg(feature = "content")]
    Content(content::cli::Cli),
    /// Run the admin API (start runs it for you)
    #[cfg(feature = "admin")]
    Admin(admin::cli::Cli),
    /// The Rust backend (not written yet)
    #[cfg(feature = "backend")]
    #[command(hide = true)]
    Backend,
    /// Run the Director (start runs the Node one until the cutover)
    #[cfg(feature = "director")]
    Director(director::cli::Cli),
}

impl Server {
    pub fn name(&self) -> &'static str {
        match *self {
            #[cfg(feature = "gateway")]
            Self::Gateway(_) => "gateway",
            #[cfg(feature = "allowlist")]
            Self::Allowlist(_) => "allowlist",
            #[cfg(feature = "content")]
            Self::Content(_) => "content",
            #[cfg(feature = "admin")]
            Self::Admin(_) => "admin",
            #[cfg(feature = "backend")]
            Self::Backend => "backend",
            #[cfg(feature = "director")]
            Self::Director(_) => "director",
        }
    }

    pub fn run(&self) -> ExitCode {
        // `*self`, so a build with no server features still matches its empty enum.
        match *self {
            #[cfg(feature = "gateway")]
            Self::Gateway(ref cli) => gateway::cli::run(cli),
            #[cfg(feature = "allowlist")]
            Self::Allowlist(ref cli) => gateway::allowlist::cli::run(cli),
            #[cfg(feature = "content")]
            Self::Content(ref cli) => content::cli::run(cli),
            #[cfg(feature = "admin")]
            Self::Admin(ref cli) => admin::cli::run(cli),
            #[cfg(feature = "backend")]
            Self::Backend => backend::run(),
            #[cfg(feature = "director")]
            Self::Director(ref cli) => director::cli::run(cli),
        }
    }
}

/// The servers this build has in it, by subcommand name.
pub const BUILT_IN: &[(&str, bool)] = &[
    ("gateway", cfg!(feature = "gateway")),
    ("allowlist", cfg!(feature = "allowlist")),
    ("content", cfg!(feature = "content")),
    ("admin", cfg!(feature = "admin")),
    ("backend", cfg!(feature = "backend")),
    ("director", cfg!(feature = "director")),
];

pub fn built_in(name: &str) -> bool {
    BUILT_IN.iter().any(|&(n, on)| on && n == name)
}
