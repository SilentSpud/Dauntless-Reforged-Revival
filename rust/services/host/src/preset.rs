//! The project's server preset in `preset/` (`legacy/ReforgedHost/src/preset.ts`): game data kept
//! with the project, which every server set up from it starts from. The idea is Dauntless
//! Revived's: its metagame and deploy server ship their data as `vendor/*.json` in the repository,
//! and a host's own files take precedence over the bundled ones.
//!
//! - `preset --create` extracts a preset from the game data generated from the host's own game
//!   (`--generate` runs the generators first). `preset/preset.json` records the provenance: the
//!   Dauntless build, the SHA-256 of the game executable it was read from, the tools, and the SHA-256
//!   of every file. It is written as the Node kit writes it, byte for byte.
//! - `setup` applies it ([`apply_preset`]), and so does `preset --apply`: a game-data file that is
//!   missing, or still the shipped example, is taken from the preset. A host's own generated data is
//!   never replaced unless `--force`, which keeps the old file as `.bak`.
//!
//! The files are the same set a game data pack holds ([`crate::gamepack`]), less the two stores built
//! from the live store captures ([`crate::captures`]): those carry the captures' content and no
//! license allows copying it into the project, so every server builds them itself with `gamedata`.

use std::path::{Path, PathBuf};

use chrono::{DateTime, Utc};
use serde_json::{Value, json};

use crate::error::HostError;
use crate::gamepack::{
    backup_stamp, entry, is_example, iso_string, pack_entries, pack_game, read_text, replace_keeping_backup,
    sha256_hex, write_file,
};
use crate::hostconfig::stringify_pretty;
use crate::paths::Paths;

pub const PRESET_FORMAT: &str = "dauntless-reforged-revival-preset";
pub const PRESET_MANIFEST: &str = "preset.json";
pub const PRESET_EXCLUDED: [&str; 2] = ["backend/ladyluck_store.json", "backend/platinum_store.json"];

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PresetFile {
    pub path: String,
    pub sha256: String,
    pub bytes: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PresetManifest {
    pub comment: Vec<String>,
    pub name: String,
    pub game: String,
    pub executable_sha256: Option<String>,
    pub created: String,
    pub sources: Vec<String>,
    pub files: Vec<PresetFile>,
}

impl PresetManifest {
    /// `preset.json`'s text, as `JSON.stringify(manifest, null, 2)` and a newline.
    pub fn to_json(&self) -> String {
        let files: Vec<Value> = self
            .files
            .iter()
            .map(|f| json!({ "path": f.path, "sha256": f.sha256, "bytes": f.bytes }))
            .collect();
        let value = json!({
            "_comment": self.comment,
            "format": PRESET_FORMAT,
            "version": 1,
            "name": self.name,
            "game": self.game,
            "executableSha256": self.executable_sha256,
            "created": self.created,
            "sources": self.sources,
            "files": files,
        });
        stringify_pretty(&value) + "\n"
    }
}

fn strings(value: Option<&Value>) -> Vec<String> {
    value
        .and_then(Value::as_array)
        .map(|items| items.iter().filter_map(Value::as_str).map(str::to_owned).collect())
        .unwrap_or_default()
}

/// `None` when there is no preset.
pub fn read_preset_manifest(p: &Paths) -> Result<Option<PresetManifest>, HostError> {
    let file = p.preset.join(PRESET_MANIFEST);
    if !file.exists() {
        return Ok(None);
    }
    let text = read_text(&file)?;
    let m = service_kit::jsjson::parse(text.strip_prefix('\u{feff}').unwrap_or(&text))
        .map_err(|e| HostError::message(format!("{}: {e}", file.display())))?;
    let files = m.get("files").and_then(Value::as_array).filter(|_| {
        m.get("format").and_then(Value::as_str) == Some(PRESET_FORMAT)
            && m.get("version").and_then(Value::as_i64) == Some(1)
    });
    let Some(files) = files else {
        return Err(HostError::message(format!(
            "{} is not a server preset of this version",
            file.display()
        )));
    };
    let text_of = |v: &Value, key: &str| v.get(key).and_then(Value::as_str).unwrap_or_default().to_owned();
    Ok(Some(PresetManifest {
        comment: strings(m.get("_comment")),
        name: text_of(&m, "name"),
        game: text_of(&m, "game"),
        executable_sha256: m.get("executableSha256").and_then(Value::as_str).map(str::to_owned),
        created: text_of(&m, "created"),
        sources: strings(m.get("sources")),
        files: files
            .iter()
            .map(|f| PresetFile {
                path: f.get("path").and_then(Value::as_str).unwrap_or("undefined").to_owned(),
                sha256: text_of(f, "sha256"),
                bytes: f.get("bytes").and_then(Value::as_u64).unwrap_or(0),
            })
            .collect(),
    }))
}

#[derive(Debug, Clone, Default)]
pub struct CreateOptions {
    pub name: String,
    /// The game executable, hashed when it is there.
    pub executable: Option<PathBuf>,
    /// The executable's hash when the executable is not on this machine.
    pub executable_sha256: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PresetCreated {
    pub manifest: PresetManifest,
    pub left: Vec<String>,
    pub excluded: Vec<String>,
}

fn walk(dir: &Path, out: &mut Vec<PathBuf>) -> Result<(), HostError> {
    for entry in std::fs::read_dir(dir).map_err(|e| HostError::io(dir, e))? {
        let entry = entry.map_err(|e| HostError::io(dir, e))?;
        let path = entry.path();
        if entry.file_type().map_err(|e| HostError::io(&path, e))?.is_dir() {
            walk(&path, out)?;
        } else {
            out.push(path);
        }
    }
    Ok(())
}

fn manifest_comment(name: &str, executable_sha256: Option<&str>) -> Vec<String> {
    vec![
        format!(
            "Server preset \"{name}\": the game data every server set up from this project starts from (legacy/ReforgedHost/src/preset.ts)."
        ),
        format!(
            "Read from an installed Dauntless {} client for interoperability: identifiers and tuning values (item catalogue, store, progression and hunt tables), no game assets.",
            pack_game()
        ),
        "Sources: CatalogExporter's export from the running game, converted by Mystic Paradox's game-data generators and the host kit (npm run gamedata).".into(),
        match executable_sha256 {
            Some(hash) => format!("Game executable SHA-256 {hash}."),
            None => "Game executable SHA-256 not recorded (no game folder in host.json).".into(),
        },
        "Lady Luck's Store and the platinum store are not in it: npm run gamedata builds them on each server from the live store captures (paradox/backend/store-captures.json).".into(),
        "A host's own generated game data takes precedence over this preset. Regenerate it with npm run preset -- --create; do not edit the files by hand.".into(),
    ]
}

/// Writes `preset/` from the workspace's generated game data (the examples are left out). Files of an
/// older preset that the new one does not hold are removed, so the folder is exactly the manifest.
pub fn create_preset(p: &Paths, opts: &CreateOptions, now: DateTime<Utc>) -> Result<PresetCreated, HostError> {
    let mut files = Vec::new();
    let mut left = Vec::new();
    let mut contents = Vec::new();
    for (pack_path, file) in pack_entries(p) {
        if PRESET_EXCLUDED.contains(&pack_path.as_str()) {
            continue;
        }
        if !file.exists() {
            left.push(pack_path);
            continue;
        }
        let data = read_text(&file)?;
        if is_example(&file, &data) {
            left.push(pack_path);
            continue;
        }
        // A preset only holds JSON the services can read.
        serde_json::from_str::<Value>(&data).map_err(|e| HostError::message(format!("{}: {e}", file.display())))?;
        files.push(PresetFile {
            path: pack_path.clone(),
            sha256: sha256_hex(&data),
            bytes: data.len() as u64,
        });
        contents.push((pack_path, data));
    }
    if files.is_empty() {
        return Err(HostError::message(
            "no generated game data to make a preset from yet (reforged-host gamedata first, or pass --generate)",
        ));
    }
    // The executable's hash, read from it, or given (a preset made on another machine than the game's).
    let executable_sha256 = match opts.executable.as_deref().filter(|exe| exe.exists()) {
        Some(exe) => Some(std::fs::read(exe).map(sha256_hex).map_err(|e| HostError::io(exe, e))?),
        None => opts.executable_sha256.clone(),
    };
    let manifest = PresetManifest {
        comment: manifest_comment(&opts.name, executable_sha256.as_deref()),
        name: opts.name.clone(),
        game: pack_game(),
        executable_sha256,
        created: iso_string(now),
        sources: vec![
            "CatalogExporter (native/catalog-exporter, from Mystic Paradox)".into(),
            "Mystic Paradox game-data generators".into(),
            "Dauntless Reforged Revival host kit".into(),
        ],
        files,
    };
    let keep: Vec<&str> = [PRESET_MANIFEST, "README.md"]
        .into_iter()
        .chain(manifest.files.iter().map(|f| f.path.as_str()))
        .collect();
    if p.preset.exists() {
        let mut old = Vec::new();
        walk(&p.preset, &mut old)?;
        for file in old {
            let rel = file.strip_prefix(&p.preset).unwrap_or(&file);
            let rel: Vec<String> = rel.iter().map(|c| c.to_string_lossy().into_owned()).collect();
            if !keep.contains(&rel.join("/").as_str()) {
                std::fs::remove_file(&file).map_err(|e| HostError::io(&file, e))?;
            }
        }
    }
    for (pack_path, data) in &contents {
        write_file(&preset_file(p, pack_path), data)?;
    }
    write_file(&p.preset.join(PRESET_MANIFEST), manifest.to_json())?;
    Ok(PresetCreated {
        manifest,
        left,
        excluded: PRESET_EXCLUDED.iter().map(|s| (*s).to_owned()).collect(),
    })
}

fn preset_file(p: &Paths, pack_path: &str) -> PathBuf {
    pack_path.split('/').fold(p.preset.clone(), |dir, part| dir.join(part))
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PresetApplied {
    pub name: String,
    pub written: Vec<String>,
    pub unchanged: Vec<String>,
    /// The host's own game data, left in place.
    pub kept: Vec<String>,
    pub backups: Vec<String>,
}

/// Applies the preset where the server has no game data of its own (all of it with `force`).
/// `None` when there is no preset.
pub fn apply_preset(paths: &Paths, force: bool) -> Result<Option<PresetApplied>, HostError> {
    apply_preset_at(paths, force, Utc::now())
}

/// [`apply_preset`] with the time that names the `.bak` files. Checks every file against the
/// manifest before writing anything.
pub fn apply_preset_at(p: &Paths, force: bool, now: DateTime<Utc>) -> Result<Option<PresetApplied>, HostError> {
    let Some(manifest) = read_preset_manifest(p)? else {
        return Ok(None);
    };
    if manifest.game != pack_game() {
        return Err(HostError::message(format!(
            "the preset is for Dauntless {}; this server runs {}",
            manifest.game,
            pack_game()
        )));
    }
    let entries = pack_entries(p);
    let mut data = Vec::new();
    for f in &manifest.files {
        if entry(&entries, &f.path).is_none() {
            return Err(HostError::message(format!(
                "the preset lists an unknown file: {}",
                f.path
            )));
        }
        if PRESET_EXCLUDED.contains(&f.path.as_str()) {
            return Err(HostError::message(format!(
                "the preset lists {}, which each server builds from the store captures instead",
                f.path
            )));
        }
        let file = preset_file(p, &f.path);
        if !file.exists() {
            return Err(HostError::message(format!(
                "the preset is incomplete: {} is missing",
                f.path
            )));
        }
        // Git for Windows checks the preset out with CRLF line endings (core.autocrlf); its hashes
        // are of the LF files, so the text is read back to LF first.
        let text = read_text(&file)?.replace("\r\n", "\n");
        if sha256_hex(&text) != f.sha256 {
            return Err(HostError::message(format!(
                "the preset's {} does not match its hash in preset.json (edited by hand? run reforged-host preset --create again)",
                f.path
            )));
        }
        data.push(text);
    }
    let stamp = backup_stamp(now);
    let mut out = PresetApplied {
        name: manifest.name.clone(),
        written: Vec::new(),
        unchanged: Vec::new(),
        kept: Vec::new(),
        backups: Vec::new(),
    };
    for (f, text) in manifest.files.iter().zip(&data) {
        let Some(target) = entry(&entries, &f.path) else {
            continue;
        };
        if target.exists() {
            let current = read_text(target)?;
            if current == *text {
                out.unchanged.push(f.path.clone());
                continue;
            }
            if !is_example(target, &current) && !force {
                out.kept.push(f.path.clone());
                continue;
            }
        }
        if let Some(backup) = replace_keeping_backup(target, text, &stamp)?.flatten() {
            out.backups.push(backup.display().to_string());
        }
        out.written.push(f.path.clone());
    }
    Ok(Some(out))
}
