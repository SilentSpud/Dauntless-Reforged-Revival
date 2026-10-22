//! Game data packs (`legacy/ReforgedHost/src/gamepack.ts`): everything `gamedata` made, in one file,
//! so a host who has generated the game data once can set up the next server (or the same one again)
//! without the SDK, CatalogExporter and the generators. `gamedata --export` writes one;
//! `gamedata --import <file>` (or the launcher's Host page) puts it back.
//!
//! A pack is gzip-compressed JSON: `{ format, version, game, created, files: [{ path, sha256, data }] }`.
//! Only the known game-data files go in and come out (the backend's and the Director's, and the
//! sources in the backend's `game-data/raw`), so a pack can never write anywhere else. Game data is
//! Phoenix Labs' data generated from the host's own game: a pack is for the host's own use, not for
//! publishing.
//!
//! The small helpers the RH4 modules share (hashes, JavaScript's dates, reading text as Node does)
//! live here too.

use std::io::{Read, Write};
use std::path::{Path, PathBuf};

use chrono::{DateTime, Utc};
use flate2::Compression;
use flate2::read::GzDecoder;
use flate2::write::GzEncoder;
use serde_json::{Map, Value, json};
use sha2::{Digest, Sha256};

use crate::error::HostError;
use crate::gamedata::{BACKEND_GAME_DATA, DIRECTOR_GAME_DATA, SUPPLIED_SOURCES};
use crate::paths::{GAME_CHANGELIST, Paths};

pub const PACK_FORMAT: &str = "dauntless-reforged-revival-gamedata";
pub const PACK_VERSION: u64 = 1;

/// `1.14.7 (647472)`.
pub fn pack_game() -> String {
    format!("1.14.7 ({GAME_CHANGELIST})")
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PackFile {
    pub path: String,
    pub sha256: String,
    pub data: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Pack {
    pub format: String,
    pub version: u64,
    pub game: String,
    pub created: String,
    pub files: Vec<PackFile>,
}

/// Pack path and file on disk, for every file a pack may hold, in the Node kit's order.
pub fn pack_entries(p: &Paths) -> Vec<(String, PathBuf)> {
    let mut entries = Vec::new();
    for name in BACKEND_GAME_DATA {
        entries.push((format!("backend/{name}"), p.backend_game_data.join(name)));
    }
    for name in DIRECTOR_GAME_DATA {
        entries.push((format!("director/{name}"), p.director_game_data.join(name)));
    }
    for (_, name) in SUPPLIED_SOURCES {
        entries.push((format!("backend/raw/{name}"), p.raw_game_data.join(name)));
    }
    entries
}

pub(crate) fn entry<'a>(entries: &'a [(String, PathBuf)], pack_path: &str) -> Option<&'a Path> {
    entries
        .iter()
        .find(|(name, _)| name == pack_path)
        .map(|(_, file)| file.as_path())
}

/// Lower-case hex SHA-256.
pub fn sha256_hex(data: impl AsRef<[u8]>) -> String {
    Sha256::digest(data.as_ref())
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

/// `Date.prototype.toISOString()`: `2026-10-15T14:52:05.055Z`.
pub fn iso_string(now: DateTime<Utc>) -> String {
    now.format("%Y-%m-%dT%H:%M:%S%.3fZ").to_string()
}

/// `toISOString().replace(/[:.]/g, "-")`: the stamp of a `.bak` file.
pub fn backup_stamp(now: DateTime<Utc>) -> String {
    iso_string(now).replace([':', '.'], "-")
}

/// `fs.readFileSync(file, "utf8")`: invalid UTF-8 becomes U+FFFD, a BOM stays.
pub fn read_text(file: &Path) -> Result<String, HostError> {
    let bytes = std::fs::read(file).map_err(|e| HostError::io(file, e))?;
    Ok(match String::from_utf8(bytes) {
        Ok(text) => text,
        Err(e) => String::from_utf8_lossy(e.as_bytes()).into_owned(),
    })
}

pub(crate) fn write_file(file: &Path, data: impl AsRef<[u8]>) -> Result<(), HostError> {
    if let Some(dir) = file.parent() {
        std::fs::create_dir_all(dir).map_err(|e| HostError::io(dir, e))?;
    }
    std::fs::write(file, data).map_err(|e| HostError::io(file, e))
}

/// `<file>.example.json` beside a `.json` file.
pub(crate) fn example_of(file: &Path) -> PathBuf {
    let text = file.to_string_lossy();
    match text.strip_suffix(".json") {
        Some(stem) => PathBuf::from(format!("{stem}.example.json")),
        None => file.to_path_buf(),
    }
}

/// Whether `data` is the text of the shipped example beside `file`.
pub(crate) fn is_example(file: &Path, data: &str) -> bool {
    let example = example_of(file);
    example.exists() && read_text(&example).is_ok_and(|text| text == data)
}

/// `.workspace/gamedata-packs/reforged-gamedata-1.14.7-<yyyymmdd-hhmm>.json.gz`.
pub fn default_pack_file(p: &Paths, now: DateTime<Utc>) -> PathBuf {
    let stamp = now.format("%Y%m%d-%H%M");
    p.workspace
        .join("gamedata-packs")
        .join(format!("reforged-gamedata-1.14.7-{stamp}.json.gz"))
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Exported {
    pub file: PathBuf,
    pub included: Vec<String>,
    pub left: Vec<String>,
}

/// The pack's JSON text, as `JSON.stringify(pack)`.
pub fn pack_json(pack: &Pack) -> String {
    let files: Vec<Value> = pack
        .files
        .iter()
        .map(|f| json!({ "path": f.path, "sha256": f.sha256, "data": f.data }))
        .collect();
    let value = json!({
        "format": pack.format,
        "version": pack.version,
        "game": pack.game,
        "created": pack.created,
        "files": files,
    });
    service_kit::jsjson::stringify(&value)
}

/// Writes the pack; the shipped examples are left out (a pack holds generated data only).
pub fn export_game_data_pack(p: &Paths, out_file: &Path, now: DateTime<Utc>) -> Result<Exported, HostError> {
    let mut files = Vec::new();
    let mut left = Vec::new();
    for (pack_path, file) in pack_entries(p) {
        if !file.exists() {
            left.push(pack_path);
            continue;
        }
        let data = read_text(&file)?;
        if is_example(&file, &data) {
            left.push(pack_path);
            continue;
        }
        files.push(PackFile {
            path: pack_path,
            sha256: sha256_hex(&data),
            data,
        });
    }
    if files.is_empty() {
        return Err(HostError::message(
            "no generated game data to pack yet (reforged-host gamedata first)",
        ));
    }
    let included = files.iter().map(|f| f.path.clone()).collect();
    let pack = Pack {
        format: PACK_FORMAT.into(),
        version: PACK_VERSION,
        game: pack_game(),
        created: iso_string(now),
        files,
    };
    let mut gz = GzEncoder::new(Vec::new(), Compression::default());
    gz.write_all(pack_json(&pack).as_bytes())
        .map_err(|e| HostError::io(out_file, e))?;
    let bytes = gz.finish().map_err(|e| HostError::io(out_file, e))?;
    write_file(out_file, bytes)?;
    Ok(Exported {
        file: out_file.to_path_buf(),
        included,
        left,
    })
}

fn not_a_pack(file: &Path) -> HostError {
    HostError::message(format!(
        "{} is not a game data pack (a .json.gz file from reforged-host gamedata --export)",
        file.display()
    ))
}

/// Reads and checks a pack: its format, its game build, and each file's name and hash.
pub fn read_game_data_pack(p: &Paths, file: &Path) -> Result<Pack, HostError> {
    let compressed = std::fs::read(file).map_err(|_| not_a_pack(file))?;
    parse_game_data_pack(p, file, &compressed)
}

/// [`read_game_data_pack`] on the file's bytes; `file` only names it in the errors.
pub fn parse_game_data_pack(p: &Paths, file: &Path, compressed: &[u8]) -> Result<Pack, HostError> {
    let mut bytes = Vec::new();
    GzDecoder::new(compressed)
        .read_to_end(&mut bytes)
        .map_err(|_| not_a_pack(file))?;
    let value: Value = service_kit::jsjson::parse(&String::from_utf8_lossy(&bytes)).map_err(|_| not_a_pack(file))?;
    let empty = Map::new();
    let object = value.as_object().unwrap_or(&empty);
    let str_field = |key: &str| object.get(key).and_then(Value::as_str);
    let Some(list) = object.get("files").and_then(Value::as_array).filter(|_| {
        str_field("format") == Some(PACK_FORMAT) && object.get("version").and_then(Value::as_u64) == Some(PACK_VERSION)
    }) else {
        return Err(HostError::message(format!(
            "{} is not a game data pack of this version",
            file.display()
        )));
    };
    let game = str_field("game");
    if game != Some(pack_game().as_str()) {
        let shown = game.map_or_else(|| "undefined".to_owned(), str::to_owned);
        return Err(HostError::message(format!(
            "{} holds game data for Dauntless {shown}; this server runs {}",
            file.display(),
            pack_game()
        )));
    }
    let entries = pack_entries(p);
    let mut files = Vec::new();
    for item in list {
        let path = item.get("path").and_then(Value::as_str);
        let data = item.get("data").and_then(Value::as_str);
        let (Some(path), Some(data)) = (path.filter(|path| entry(&entries, path).is_some()), data) else {
            let shown = match item.get("path") {
                Some(Value::String(s)) => s.clone(),
                Some(other) => service_kit::jsjson::stringify(other),
                None => "undefined".into(),
            };
            return Err(HostError::message(format!(
                "{} holds an unknown file: {shown}",
                file.display()
            )));
        };
        let sha256 = sha256_hex(data);
        if item.get("sha256").and_then(Value::as_str) != Some(sha256.as_str()) {
            return Err(HostError::message(format!(
                "{} is damaged: {path} does not match its hash",
                file.display()
            )));
        }
        if serde_json::from_str::<Value>(data).is_err() {
            return Err(HostError::message(format!(
                "{} is damaged: {path} is not JSON",
                file.display()
            )));
        }
        files.push(PackFile {
            path: path.into(),
            sha256,
            data: data.into(),
        });
    }
    Ok(Pack {
        format: PACK_FORMAT.into(),
        version: PACK_VERSION,
        game: pack_game(),
        created: str_field("created").unwrap_or_default().into(),
        files,
    })
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Imported {
    pub written: Vec<String>,
    pub unchanged: Vec<String>,
    pub backups: Vec<PathBuf>,
}

/// Puts a file in place; a file that is there and different is kept beside it as `<name>.<time>.bak`.
/// `None` when it already holds `data`.
pub(crate) fn replace_keeping_backup(
    target: &Path,
    data: &str,
    stamp: &str,
) -> Result<Option<Option<PathBuf>>, HostError> {
    let mut backup = None;
    if target.exists() {
        if read_text(target)? == data {
            return Ok(None);
        }
        let aside = PathBuf::from(format!("{}.{stamp}.bak", target.display()));
        std::fs::rename(target, &aside).map_err(|e| HostError::io(target, e))?;
        backup = Some(aside);
    }
    write_file(target, data)?;
    Ok(Some(backup))
}

/// Puts a pack's files in place. A file that is there and different is kept beside it as
/// `<name>.<time>.bak`.
pub fn import_game_data_pack(p: &Paths, file: &Path, now: DateTime<Utc>) -> Result<Imported, HostError> {
    let pack = read_game_data_pack(p, file)?;
    let entries = pack_entries(p);
    let stamp = backup_stamp(now);
    let mut out = Imported::default();
    for f in &pack.files {
        let Some(target) = entry(&entries, &f.path) else {
            continue;
        };
        match replace_keeping_backup(target, &f.data, &stamp)? {
            None => out.unchanged.push(f.path.clone()),
            Some(backup) => {
                out.backups.extend(backup);
                out.written.push(f.path.clone());
            }
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use super::*;

    #[test]
    fn writes_dates_and_hashes_as_node_does() {
        let now = DateTime::parse_from_rfc3339("2026-10-15T14:52:05.055Z")
            .unwrap()
            .to_utc();
        assert_eq!(iso_string(now), "2026-10-15T14:52:05.055Z");
        assert_eq!(backup_stamp(now), "2026-10-15T14-52-05-055Z");
        assert!(default_pack_file(&Paths::new("rr"), now).ends_with("reforged-gamedata-1.14.7-20261015-1452.json.gz"));
        assert_eq!(
            sha256_hex("{}"),
            "44136fa355b3678a1146ad16f7e8649e94fb4fc21fe77e8310c060f61caaff8a"
        );
        assert_eq!(pack_game(), "1.14.7 (647472)");
    }
}
