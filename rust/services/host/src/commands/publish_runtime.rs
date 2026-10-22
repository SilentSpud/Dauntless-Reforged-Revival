//! `reforged-host publish-runtime`: the Node kit's `publish()` in `cli.ts`, with [`crate::publish`].
//!
//! The launcher's Host page calls this command, so the version format and `runtime.json` exist
//! once (RD23). `--from <folder>`: `MysticParadox.dll` and `winmm.dll` from that folder instead of
//! this checkout's builds (a Linux server's runtime is built on a Windows PC and copied over).

use std::path::Path;
use std::process::ExitCode;

use chrono::Utc;

use crate::Outcome;
use crate::cli::PublishRuntimeArgs;
use crate::commands::gamedata::from_cwd;
use crate::error::HostError;
use crate::hostconfig::read_host_config;
use crate::paths::{Paths, RUNTIME_FILE_NAMES, game_exe_relative};
use crate::publish::{default_sources, ensure_server_flags, install_server_runtime, publish_runtime};

pub fn run(paths: &Paths, args: &PublishRuntimeArgs) -> Outcome {
    let sources = match &args.from {
        Some(None) => {
            return Err(HostError::message(
                "--from needs the folder that holds MysticParadox.dll and winmm.dll",
            ));
        }
        Some(Some(dir)) => {
            let dir = from_cwd(dir)?;
            RUNTIME_FILE_NAMES.map(|name| (name, dir.join(name)))
        }
        None => default_sources(paths),
    };
    let runtime = publish_runtime(paths, Utc::now(), &sources)?;
    println!(
        "Runtime {} published in {}:",
        runtime.version,
        paths.runtime_publish.display()
    );
    for f in &runtime.files {
        println!("  {}  {} bytes  {}", f.name, f.size, f.sha256);
    }
    println!("Players see these hashes and accept them once in the launcher.");
    let config = read_host_config(&paths.host_json)?;
    if let Some(game_dir) = config.as_ref().and_then(|c| c.game_dir.as_deref()) {
        let exe = Path::new(game_dir).join(game_exe_relative());
        let win64 = exe.parent().unwrap_or(Path::new(game_dir));
        let r = install_server_runtime(paths, win64);
        let shown = win64.display();
        if !r.installed.is_empty() {
            println!("Installed for the game servers in {shown}: {}", r.installed.join(", "));
        }
        if !r.in_use.is_empty() {
            println!(
                "Game servers still running keep the old {} until the server restarts: restart it so every world uses the new runtime.",
                r.in_use.join(", ")
            );
        }
        if let Some((_, error)) = r.failed.first() {
            let names: Vec<&str> = r.failed.iter().map(|(name, _)| name.as_str()).collect();
            println!(
                "! Could not replace {} in {shown} (another program holds it: stop the server and run this again): {error}",
                names.join(", ")
            );
        }
        ensure_server_flags(win64)?;
    }
    Ok(ExitCode::SUCCESS)
}

#[cfg(test)]
mod tests {
    use crate::error::HostError;

    #[test]
    fn needs_the_folder_after_from_and_the_builds_without_it() {
        let dir = tempfile::tempdir().unwrap_or_else(|e| panic!("{e}"));
        let paths = crate::paths::Paths::new(dir.path());
        let args = crate::cli::PublishRuntimeArgs { from: Some(None) };
        let error = super::run(&paths, &args).unwrap_err();
        assert!(
            matches!(&error, HostError::Message(m) if m.starts_with("--from needs")),
            "{error}"
        );
        let error = super::run(&paths, &Default::default()).unwrap_err();
        assert!(error.to_string().contains("is not built yet"), "{error}");
    }
}
