//! `reforged-host multipliers`: the Node kit's `multipliers()` in `cli.ts`. It reads host.json,
//! changes only `multipliers` (so the rest of the file stays byte for byte) and rewrites the env
//! files; the backend reads them when it starts.

use std::path::Path;
use std::process::ExitCode;

use crate::Outcome;
use crate::cli::MultipliersArgs;
use crate::commands::setup::options::{MultiplierOption, multipliers_from_args, number_text, text};
use crate::error::HostError;
use crate::hostconfig::{
    HostConfig, Platform, ServerMode, build_env_files, load_or_create_secrets, multipliers_on, read_host_config,
    write_env_files, write_host_config,
};
use crate::paths::Paths;

pub fn run(paths: &Paths, args: &MultipliersArgs) -> Outcome {
    let commit = |repo: &Path| -> Result<String, HostError> {
        Ok(crate::run::capture("git", &["rev-parse", "--short=12", "HEAD"], Some(repo))?.unwrap_or_default())
    };
    run_with(paths, args, Platform::current(), &commit, &mut |line| {
        println!("{line}")
    })
}

/// Whether the command was asked to change anything (else it only says how things are).
pub fn asked(args: &MultipliersArgs) -> bool {
    args.off
        || [&args.xp, &args.rewards, &args.xp_multiplier, &args.reward_multiplier]
            .iter()
            .any(|v| text(v).is_some())
}

/// The config with the multipliers the options ask for.
pub fn changed(config: &HostConfig, args: &MultipliersArgs) -> Result<HostConfig, HostError> {
    let multipliers = multipliers_from_args(
        args.off || args.no_multipliers,
        MultiplierOption {
            name: "xp",
            value: text(&args.xp).or(text(&args.xp_multiplier)),
        },
        MultiplierOption {
            name: "rewards",
            value: text(&args.rewards).or(text(&args.reward_multiplier)),
        },
        config.mode,
        config.multipliers,
    )?;
    Ok(HostConfig {
        multipliers,
        ..config.clone()
    })
}

/// The command with the platform and `git rev-parse` given.
pub fn run_with(
    paths: &Paths,
    args: &MultipliersArgs,
    platform: Platform,
    source_commit: &dyn Fn(&Path) -> Result<String, HostError>,
    out: &mut dyn FnMut(&str),
) -> Outcome {
    let mut config = read_host_config(&paths.host_json)?
        .ok_or_else(|| HostError::message("not set up yet: run reforged-host setup first"))?;
    let asked = asked(args);
    if asked {
        config = changed(&config, args)?;
        write_host_config(&paths.host_json, &config)?;
        let (secrets, _) = load_or_create_secrets(&paths.config, super::setup::secrets::new_secrets)?;
        let commit = source_commit(&paths.repo)?;
        write_env_files(paths, &build_env_files(&config, &secrets, paths, &commit, platform))?;
    }
    match config.multipliers.filter(|_| multipliers_on(&config)) {
        Some(m) => out(&format!(
            "Test multipliers ON: XP x{}, rewards x{}.",
            number_text(m.xp),
            number_text(m.rewards)
        )),
        None => out(&format!(
            "Test multipliers off.{}",
            if config.mode == ServerMode::Public {
                " (They are for private Tailscale servers only.)"
            } else {
                ""
            }
        )),
    }
    if asked {
        out(
            "Restart the server (Ctrl+C, then reforged-host start) for the backend to use them. Its log says [TestMultiplier] ON or off.",
        );
    }
    Ok(ExitCode::SUCCESS)
}
