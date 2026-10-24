//! `reforged-host backup`: new in the Rust kit: a backup of the server's database into
//! `.workspace/backups/`. Until the backend cutover (RB14) the database is MongoDB and this runs
//! `mongodump` (MongoDB Database Tools) into one gzipped archive, which `mongorestore` reads back.
//!
//! Seam for RB14: `pg_dump` once Postgres is managed (RD15) is another [`Database`] arm here. Not
//! written here. The command has no options yet; an output folder (`--out`) would go through the
//! integration pass (RH1 owns the `clap` tree).

use std::path::{Path, PathBuf};
use std::process::ExitCode;

use crate::Outcome;
use crate::commands::setup::database::{Database, database};
use crate::error::HostError;
use crate::hostconfig::{HostConfig, load_host_config, read_env_file};
use crate::mongo::{newest_installed, program_files, search_path};
use crate::paths::Paths;

/// What backup needs from this PC: the dump program, and running it.
pub trait BackupEnv {
    fn find_mongodump(&self, config: &HostConfig) -> Result<Option<PathBuf>, HostError>;
    /// Runs the program to its end, its output into `log_file`: the exit code and the last line.
    fn run(&self, program: &Path, args: &[String], log_file: &Path) -> Result<(i32, Option<String>), HostError>;
    /// The archive's time stamp: `20261022-153000`.
    fn stamp(&self) -> String;
}

pub struct System;

impl BackupEnv for System {
    fn find_mongodump(&self, config: &HostConfig) -> Result<Option<PathBuf>, HostError> {
        Ok(find_mongodump_in(
            std::env::var_os("PATH").as_deref(),
            cfg!(windows).then(program_files).as_deref(),
            config.mongo.mongod.as_deref().map(Path::new),
        ))
    }

    fn run(&self, program: &Path, args: &[String], log_file: &Path) -> Result<(i32, Option<String>), HostError> {
        let args: Vec<&str> = args.iter().map(String::as_str).collect();
        let r = crate::run::run(
            &program.to_string_lossy(),
            &args,
            &crate::run::RunOptions {
                cwd: log_file.parent().map(Path::to_path_buf).unwrap_or_default(),
                env: None,
                log_file: Some(log_file.to_path_buf()),
                quiet: true,
            },
        )?;
        Ok((r.code, r.last_line))
    }

    fn stamp(&self) -> String {
        chrono::Local::now().format("%Y%m%d-%H%M%S").to_string()
    }
}

/// mongodump on PATH, in MongoDB Database Tools' Windows folder (the newest version) or next to mongod.
pub fn find_mongodump_in(
    path: Option<&std::ffi::OsStr>,
    program_files: Option<&Path>,
    mongod: Option<&Path>,
) -> Option<PathBuf> {
    let exe = if cfg!(windows) { "mongodump.exe" } else { "mongodump" };
    search_path("mongodump", path)
        .or_else(|| newest_installed(&program_files?.join("MongoDB").join("Tools"), exe))
        .or_else(|| mongod?.parent().map(|dir| dir.join(exe)).filter(|f| f.is_file()))
}

pub fn run(paths: &Paths) -> Outcome {
    run_with(paths, &System, &mut |line| println!("{line}"))
}

pub fn run_with(paths: &Paths, env: &dyn BackupEnv, out: &mut dyn FnMut(&str)) -> Outcome {
    let config =
        load_host_config(paths)?.ok_or_else(|| HostError::message("not set up yet: run reforged-host setup first"))?;
    match database(&config) {
        Database::Mongo => mongo_backup(paths, &config, env, out),
    }
}

fn mongo_backup(paths: &Paths, config: &HostConfig, env: &dyn BackupEnv, out: &mut dyn FnMut(&str)) -> Outcome {
    let Some(mongodump) = env.find_mongodump(config)? else {
        return Err(HostError::message(
            "mongodump was not found: install MongoDB Database Tools (https://www.mongodb.com/try/download/database-tools; on Linux see docs/hosting-linux.md), then run this again",
        ));
    };
    // The URI the backend uses, as start and doctor read it.
    let backend_env = read_env_file(&paths.config.join("backend.env"))?;
    let uri = backend_env.get("MONGODB_URI").unwrap_or(&config.mongo.uri).to_owned();
    let dir = paths.workspace.join("backups");
    std::fs::create_dir_all(&dir).map_err(|e| HostError::io(&dir, e))?;
    std::fs::create_dir_all(&paths.logs).map_err(|e| HostError::io(&paths.logs, e))?;
    let file = dir.join(format!("mongodb-{}.archive.gz", env.stamp()));
    let log_file = paths.logs.join("backup.log");
    let args = vec![
        format!("--uri={uri}"),
        format!("--archive={}", file.display()),
        "--gzip".to_owned(),
    ];
    out(&format!("Backing up MongoDB with {}...", mongodump.display()));
    let (code, last_line) = env.run(&mongodump, &args, &log_file)?;
    if code != 0 || !file.exists() {
        return Err(HostError::message(format!(
            "mongodump failed: {} (full output in {})",
            last_line.unwrap_or_else(|| format!("exit {code}")),
            log_file.display()
        )));
    }
    let bytes = std::fs::metadata(&file).map_err(|e| HostError::io(&file, e))?.len();
    out(&format!("Backup written: {} ({bytes} bytes)", file.display()));
    out(&format!(
        "Restore it with the server stopped: mongorestore --uri=\"<the backend's MONGODB_URI>\" --gzip --archive=\"{}\" --drop",
        file.display()
    ));
    Ok(ExitCode::SUCCESS)
}
