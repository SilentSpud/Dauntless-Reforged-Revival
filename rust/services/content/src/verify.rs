//! `content verify [--quick] [--game-dir <folder>] [--manifest <file>]` (`src/verify.ts`, `npm run
//! verify`): hashes every file of the game folder (`CONTENT_GAME_DIR` unless `--game-dir`) against the
//! manifest and prints what is missing, the wrong size or the wrong hash. `--quick` compares sizes
//! only. Read-only. Exit code 0 only if every file matches. Run it after copying the game onto a new
//! host, and before restarting the content server after anything touched the game folder.

use std::collections::HashSet;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::time::Instant;

use sha2::{Digest, Sha256};

use crate::manifest::{self, on_disk_path};

fn hash_file(file: &Path) -> std::io::Result<String> {
    let mut hasher = Sha256::new();
    let mut reader = std::fs::File::open(file)?;
    let mut buffer = vec![0u8; 4 * 1024 * 1024];
    loop {
        let read = reader.read(&mut buffer)?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
    }
    Ok(hasher.finalize().iter().map(|b| format!("{b:02x}")).collect())
}

fn list_files(dir: &Path, prefix: &str, out: &mut Vec<String>) -> std::io::Result<()> {
    for entry in std::fs::read_dir(dir)? {
        let entry = entry?;
        let name = entry.file_name().to_string_lossy().into_owned();
        let relative = if prefix.is_empty() {
            name.clone()
        } else {
            format!("{prefix}/{name}")
        };
        if entry.file_type()?.is_dir() {
            list_files(&entry.path(), &relative, out)?;
        } else {
            out.push(relative);
        }
    }
    Ok(())
}

/// Runs the check; the process exit code.
pub fn run(game_dir: Option<PathBuf>, manifest_path: Option<PathBuf>, quick: bool) -> i32 {
    let Some(game_dir) = game_dir.or_else(|| {
        std::env::var("CONTENT_GAME_DIR")
            .ok()
            .filter(|d| !d.trim().is_empty())
            .map(PathBuf::from)
    }) else {
        eprintln!("Set CONTENT_GAME_DIR in .env or pass --game-dir <folder that contains Archon\\>");
        return 2;
    };
    let game_dir = std::path::absolute(&game_dir).unwrap_or(game_dir);
    let manifest_path = manifest_path.or_else(|| {
        std::env::var("CONTENT_MANIFEST")
            .ok()
            .filter(|m| !m.trim().is_empty())
            .map(PathBuf::from)
    });
    let manifest = match manifest::load(manifest_path.as_deref()) {
        Ok(manifest) => manifest,
        Err(error) => {
            eprintln!("{error}");
            return 2;
        }
    };
    println!(
        "Manifest:  {}",
        manifest_path
            .as_ref()
            .map_or_else(|| "built-in".to_owned(), |p| p.display().to_string())
    );
    println!("Build:     {}", manifest.build);
    println!("Game dir:  {}", game_dir.display());
    println!(
        "Mode:      {}",
        if quick {
            "sizes only (--quick)"
        } else {
            "full SHA-256 of every file"
        }
    );
    println!("Files:     {}, {} bytes\n", manifest.files.len(), manifest.total_bytes);
    let mut problems = Vec::new();
    let (mut ok, mut done_bytes) = (0usize, 0u64);
    let started = Instant::now();
    let mut last_report = Instant::now();
    for file in &manifest.files {
        let on_disk = on_disk_path(&game_dir, file);
        let Ok(meta) = std::fs::symlink_metadata(&on_disk) else {
            problems.push(format!("MISSING   {}", file.path));
            continue;
        };
        if !meta.is_file() {
            problems.push(format!("NOT FILE  {}", file.path));
            continue;
        }
        if meta.len() != file.size {
            problems.push(format!(
                "SIZE      {} ({} bytes, expected {})",
                file.path,
                meta.len(),
                file.size
            ));
            continue;
        }
        if !quick && hash_file(&on_disk).ok().as_deref() != Some(file.sha256.as_str()) {
            problems.push(format!("HASH      {}", file.path));
            continue;
        }
        ok += 1;
        done_bytes += file.size;
        if last_report.elapsed().as_secs() > 5 {
            last_report = Instant::now();
            println!(
                "  {}/{} files, {:.1}%, {:.0} s",
                ok + problems.len(),
                manifest.files.len(),
                done_bytes as f64 / manifest.total_bytes as f64 * 100.0,
                started.elapsed().as_secs_f64()
            );
        }
    }
    // Extra files are fine (the two DLLs installed, logs), but worth seeing
    let known: HashSet<String> = manifest.files.iter().map(|f| f.path.to_lowercase()).collect();
    let mut all = Vec::new();
    let extras: Vec<String> = if list_files(&game_dir, "", &mut all).is_ok() {
        all.into_iter().filter(|r| !known.contains(&r.to_lowercase())).collect()
    } else {
        Vec::new()
    };
    for problem in &problems {
        println!("{problem}");
    }
    println!(
        "\nok: {ok}/{}  problems: {}  ({:.1} s)",
        manifest.files.len(),
        problems.len(),
        started.elapsed().as_secs_f64()
    );
    if !extras.is_empty() {
        println!(
            "Not in the manifest (never served): {} file(s): {}{}",
            extras.len(),
            extras.iter().take(10).cloned().collect::<Vec<_>>().join(", "),
            if extras.len() > 10 { ", ..." } else { "" }
        );
    }
    if problems.is_empty() { 0 } else { 1 }
}
