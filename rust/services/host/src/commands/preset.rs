//! `reforged-host preset`: the Node kit's `preset()` in `cli.ts`, with [`crate::preset`].
//!
//! Without options it shows the preset; `--create [--generate] [--name <name>]` makes it from this
//! server's game data; `--apply [--force]` applies it now.

use std::process::ExitCode;

use chrono::Utc;

use crate::Outcome;
use crate::cli::PresetArgs;
use crate::commands::gamedata::{log_file, not_set_up};
use crate::error::HostError;
use crate::gamedata::{GenerateOptions, generate_game_data};
use crate::hostconfig::load_host_config;
use crate::paths::{Paths, game_exe_relative};
use crate::preset::{CreateOptions, apply_preset, create_preset, read_preset_manifest};

const DEFAULT_NAME: &str = "Dauntless Reforged Revival";

fn create(paths: &Paths, args: &PresetArgs) -> Outcome {
    not_set_up(paths)?;
    if args.generate {
        println!("Generating the game data from your export (Items_Analysis) first:");
        let opts = GenerateOptions {
            examples_for_missing: false,
            log_file: log_file(paths),
            only: None,
        };
        let r = generate_game_data(paths, &opts, &mut |line| println!("{line}"))?;
        if !r.ok {
            println!(
                "! Not every generator ran ({}); the preset takes the files that are there.",
                r.failed.join(", ")
            );
        }
    }
    let config = load_host_config(paths)?;
    let executable = config
        .as_ref()
        .and_then(|c| c.game_dir.as_deref())
        .map(|dir| std::path::Path::new(dir).join(game_exe_relative()));
    let name = match args.name.as_deref().map(str::trim).filter(|n| !n.is_empty()) {
        Some(name) => name.to_owned(),
        None => config
            .as_ref()
            .map_or_else(|| DEFAULT_NAME.to_owned(), |c| c.name.clone()),
    };
    let opts = CreateOptions {
        name: name.clone(),
        executable,
        executable_sha256: None,
    };
    let r = create_preset(paths, &opts, Utc::now())?;
    println!(
        "Server preset \"{name}\" written to {}: {} files",
        paths.preset.display(),
        r.manifest.files.len()
    );
    for f in &r.manifest.files {
        println!("  {}  {} bytes", f.path, f.bytes);
    }
    if !r.left.is_empty() {
        println!("  not in it (missing, or still the example): {}", r.left.join(", "));
    }
    println!(
        "  never in a preset (built from the store captures on each server): {}",
        r.excluded.join(", ")
    );
    match &r.manifest.executable_sha256 {
        Some(hash) => println!("  read from the game executable {hash}"),
        None => println!("  ! no game folder in host.toml, so the executable's SHA-256 was not recorded"),
    }
    println!(
        "Every server set up from this checkout now starts from it (reforged-host setup applies it). To ship it with the project, commit preset/."
    );
    println!(
        "It is data read from Phoenix Labs' game: committing it to a public repository publishes it (see docs/sdk-and-game-data.md)."
    );
    Ok(ExitCode::SUCCESS)
}

fn apply(paths: &Paths, args: &PresetArgs) -> Outcome {
    not_set_up(paths)?;
    let Some(r) = apply_preset(paths, args.force)? else {
        return Err(HostError::message(format!(
            "no server preset in {} (make one with reforged-host preset --create)",
            paths.preset.display()
        )));
    };
    let backups = if r.backups.is_empty() {
        String::new()
    } else {
        format!(" ({} replaced files kept as .bak)", r.backups.len())
    };
    println!(
        "Server preset \"{}\": {} files written, {} already the same{backups}",
        r.name,
        r.written.len(),
        r.unchanged.len()
    );
    if !r.kept.is_empty() {
        println!(
            "  your own game data kept (reforged-host preset --apply --force replaces it): {}",
            r.kept.join(", ")
        );
    }
    Ok(ExitCode::SUCCESS)
}

pub fn run(paths: &Paths, args: &PresetArgs) -> Outcome {
    if args.create {
        return create(paths, args);
    }
    if args.apply {
        return apply(paths, args);
    }
    let Some(m) = read_preset_manifest(paths)? else {
        println!(
            "No server preset in {} yet. Make one from your game data: reforged-host preset --create [--generate]",
            paths.preset.display()
        );
        return Ok(ExitCode::SUCCESS);
    };
    let from = m
        .executable_sha256
        .as_deref()
        .map(|hash| format!(", from the executable {}...", hash.get(..16).unwrap_or(hash)))
        .unwrap_or_default();
    println!(
        "Server preset \"{}\" for Dauntless {}, made {}{from}:",
        m.name, m.game, m.created
    );
    for f in &m.files {
        println!("  {}  {} bytes", f.path, f.bytes);
    }
    println!("reforged-host setup applies it; reforged-host preset --apply [--force] applies it now.");
    Ok(ExitCode::SUCCESS)
}

#[cfg(test)]
mod tests {
    #[test]
    fn shows_that_there_is_no_preset() {
        let dir = tempfile::tempdir().unwrap_or_else(|e| panic!("{e}"));
        let paths = crate::paths::Paths::new(dir.path());
        assert!(super::run(&paths, &Default::default()).is_ok());
    }
}
