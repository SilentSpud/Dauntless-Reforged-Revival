//! Mystic Paradox is part of this repository (RP2-RP4, `legacy/ReforgedHost/src/paradox.ts`): its
//! backend and Director in `paradox/`, the game runtime, its loader and the CatalogExporter in
//! `native/`. Nothing is downloaded from Paradox's repository any more.
//!
//! Hosts set up before that had it in `.workspace/Mystic-Paradox`, a patched checkout that also held
//! their own files: the SDK from their game, game data, exports and builds.
//! [`migrate_old_workspace`] copies those into the new places once, where nothing of the host's is
//! there yet, and never deletes the old folder.
//!
//! [`Layout`] holds the code side of the Node kit's `paths()` since RP4. RH1's [`Paths`] still has
//! the layout from before the fork (`.workspace/Mystic-Paradox/ParadoxBackend` ...); once the
//! integration pass moves these fields into `paths`, this struct goes away.

use std::path::{Path, PathBuf};

use crate::error::HostError;
use crate::paths::Paths;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Layout {
    pub paradox: PathBuf,
    /// Where setup took the host's own files from before Mystic Paradox was part of the repository.
    pub old_upstream: PathBuf,
    pub backend_dir: PathBuf,
    pub director_dir: PathBuf,
    pub gateway_dir: PathBuf,
    pub content_dir: PathBuf,
    pub runtime_source: PathBuf,
    pub sdk_header: PathBuf,
    pub sdk_dir: PathBuf,
    pub runtime_dll: PathBuf,
    pub loader_dir: PathBuf,
    pub loader_dll: PathBuf,
    pub exporter_dir: PathBuf,
    pub exporter_dll: PathBuf,
    pub backend_game_data: PathBuf,
    pub director_game_data: PathBuf,
    pub raw_game_data: PathBuf,
    pub exports: PathBuf,
    pub store_captures: PathBuf,
}

impl Layout {
    pub fn new(p: &Paths) -> Self {
        let paradox = p.repo.join("paradox");
        let native = p.repo.join("native");
        let runtime_source = native.join("paradox-runtime");
        let loader_dir = native.join("runtime-loader");
        let exporter_dir = native.join("catalog-exporter");
        let backend_dir = paradox.join("backend");
        let director_dir = paradox.join("director");
        Self {
            old_upstream: p.workspace.join("Mystic-Paradox"),
            gateway_dir: p.gateway_dir.clone(),
            content_dir: p.content_dir.clone(),
            sdk_header: runtime_source.join("SDK.hpp"),
            sdk_dir: runtime_source.join("SDK"),
            runtime_dll: runtime_source.join("x64").join("Release").join("MysticParadox.dll"),
            loader_dll: loader_dir.join("target").join("release").join("winmm.dll"),
            exporter_dll: exporter_dir.join("x64").join("Release").join("CatalogExporter.dll"),
            backend_game_data: backend_dir.join("game-data"),
            director_game_data: director_dir.join("game-data"),
            raw_game_data: backend_dir.join("game-data").join("raw"),
            // Mystic Paradox's generators read these from its root (paradox/).
            exports: paradox.join("Items_Analysis"),
            store_captures: paradox.join("DauntlessEndpointDocumentation"),
            backend_dir,
            director_dir,
            runtime_source,
            loader_dir,
            exporter_dir,
            paradox,
        }
    }
}

/// The folders the host kit builds from, relative to the repository. A checkout without one of them
/// is incomplete (a sparse or damaged clone).
pub fn missing_paradox(l: &Layout) -> Vec<&'static str> {
    let needed: [(&str, PathBuf); 5] = [
        ("paradox/backend", l.backend_dir.join("package.json")),
        ("paradox/director", l.director_dir.join("package.json")),
        ("native/paradox-runtime", l.runtime_source.join("MysticParadox.vcxproj")),
        ("native/runtime-loader", l.loader_dir.join("Cargo.toml")),
        (
            "native/catalog-exporter",
            l.exporter_dir.join("CatalogExporter.vcxproj"),
        ),
    ];
    needed
        .into_iter()
        .filter(|(_, file)| !file.exists())
        .map(|(name, _)| name)
        .collect()
}

/// The Dumper-7 SDK files next to SDK.hpp (the SDK folder is copied as a whole).
pub const SDK_FILES: [&str; 6] = [
    "SDK.hpp",
    "Assertions.inl",
    "NameCollisions.inl",
    "PropertyFixup.hpp",
    "UnrealContainers.hpp",
    "UtfN.hpp",
];

/// What the old `.workspace/Mystic-Paradox` held of the host's own, as (old path, new path) pairs.
/// Game data is listed file by file, so a file the preset or the host already put in the new place
/// is kept.
pub fn old_workspace_items(l: &Layout) -> Vec<(PathBuf, PathBuf)> {
    let old = &l.old_upstream;
    let runtime = old.join("ParadoxRuntime");
    let mut items: Vec<(PathBuf, PathBuf)> = SDK_FILES
        .iter()
        .chain(&["SDK"])
        .map(|f| (runtime.join(f), l.runtime_source.join(f)))
        .collect();
    items.push((
        runtime.join("x64").join("Release").join("MysticParadox.dll"),
        l.runtime_dll.clone(),
    ));
    items.push((
        join(old, &["tools", "RuntimeLoader", "target", "release", "winmm.dll"]),
        l.loader_dll.clone(),
    ));
    items.push((
        join(
            old,
            &["tools", "CatalogExporter", "x64", "Release", "CatalogExporter.dll"],
        ),
        l.exporter_dll.clone(),
    ));
    items.push((old.join("Items_Analysis"), l.exports.clone()));
    items.push((old.join("DauntlessEndpointDocumentation"), l.store_captures.clone()));
    for (from, to) in [
        (join(old, &["ParadoxBackend", "game-data"]), &l.backend_game_data),
        (join(old, &["ParadoxDirector", "game-data"]), &l.director_game_data),
    ] {
        let Ok(entries) = std::fs::read_dir(&from) else {
            continue;
        };
        let mut entries: Vec<_> = entries.filter_map(Result::ok).collect();
        entries.sort_by_key(|e| e.file_name());
        for entry in entries {
            let name = entry.file_name().to_string_lossy().into_owned();
            let Ok(kind) = entry.file_type() else { continue };
            // The committed examples are already in place; raw/ is the host's own sources.
            if (kind.is_file() && name.ends_with(".json") && !name.ends_with(".example.json"))
                || (kind.is_dir() && name == "raw")
            {
                items.push((from.join(&name), to.join(&name)));
            }
        }
    }
    items
}

fn join(base: &Path, parts: &[&str]) -> PathBuf {
    parts.iter().fold(base.to_path_buf(), |p, part| p.join(part))
}

fn is_empty_dir(path: &Path) -> bool {
    std::fs::read_dir(path).is_ok_and(|mut d| d.next().is_none())
}

/// `fs.cpSync(from, to, { recursive: true, force: false })`: files already at `to` are kept.
fn copy_tree(from: &Path, to: &Path) -> Result<(), HostError> {
    if from.is_dir() {
        std::fs::create_dir_all(to).map_err(|e| HostError::io(to, e))?;
        for entry in std::fs::read_dir(from).map_err(|e| HostError::io(from, e))? {
            let entry = entry.map_err(|e| HostError::io(from, e))?;
            copy_tree(&entry.path(), &to.join(entry.file_name()))?;
        }
        Ok(())
    } else if to.exists() {
        Ok(())
    } else {
        std::fs::copy(from, to).map(|_| ()).map_err(|e| HostError::io(to, e))
    }
}

/// Copies the host's own files from the old `.workspace/Mystic-Paradox`. Returns what was copied,
/// relative to the repository with `/`. Anything already in the new place is left as it is.
pub fn migrate_old_workspace(l: &Layout, repo: &Path) -> Result<Vec<String>, HostError> {
    if !l.old_upstream.exists() {
        return Ok(Vec::new());
    }
    let mut copied = Vec::new();
    for (from, to) in old_workspace_items(l) {
        if !from.exists() {
            continue;
        }
        if to.exists() && !(to.is_dir() && is_empty_dir(&to)) {
            continue;
        }
        if let Some(dir) = to.parent() {
            std::fs::create_dir_all(dir).map_err(|e| HostError::io(dir, e))?;
        }
        copy_tree(&from, &to)?;
        let relative = to.strip_prefix(repo).unwrap_or(&to);
        copied.push(
            relative
                .components()
                .map(|c| c.as_os_str().to_string_lossy())
                .collect::<Vec<_>>()
                .join("/"),
        );
    }
    Ok(copied)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write(file: &Path, text: &str) {
        std::fs::create_dir_all(file.parent().unwrap()).unwrap();
        std::fs::write(file, text).unwrap();
    }

    fn repo() -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR")).join("..").join("..").join("..")
    }

    #[test]
    fn this_checkout_has_everything_the_host_kit_builds_from() {
        assert_eq!(missing_paradox(&Layout::new(&Paths::new(repo()))), Vec::<&str>::new());
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(
            missing_paradox(&Layout::new(&Paths::new(dir.path()))),
            [
                "paradox/backend",
                "paradox/director",
                "native/paradox-runtime",
                "native/runtime-loader",
                "native/catalog-exporter"
            ]
        );
    }

    #[test]
    fn the_hosts_own_files_move_over_from_the_old_workspace_once() {
        let dir = tempfile::tempdir().unwrap();
        let p = Paths::new(dir.path());
        let l = Layout::new(&p);
        let old = &l.old_upstream;
        assert!(
            migrate_old_workspace(&l, &p.repo).unwrap().is_empty(),
            "no old folder: nothing to do"
        );

        write(
            &join(old, &["ParadoxRuntime", "SDK.hpp"]),
            "sdk
",
        );
        write(
            &join(old, &["ParadoxRuntime", "SDK", "Basic_functions.cpp"]),
            "fn
",
        );
        write(
            &join(old, &["ParadoxRuntime", "x64", "Release", "MysticParadox.dll"]),
            "dll",
        );
        write(
            &join(old, &["tools", "RuntimeLoader", "target", "release", "winmm.dll"]),
            "loader",
        );
        write(
            &join(old, &["Items_Analysis", "catalog_1_14_7.jsonl"]),
            "{}
",
        );
        write(
            &join(old, &["ParadoxBackend", "game-data", "progression.json"]),
            "{\"mine\":1}
",
        );
        write(
            &join(old, &["ParadoxBackend", "game-data", "progression.example.json"]),
            "{}
",
        );
        write(
            &join(
                old,
                &["ParadoxBackend", "game-data", "raw", "ladyluck_store.source.json"],
            ),
            "[]
",
        );
        write(
            &join(old, &["ParadoxDirector", "game-data", "hunts.json"]),
            "{\"old\":1}
",
        );
        // Already there (from the preset, say): kept.
        write(
            &l.director_game_data.join("hunts.json"),
            "{\"new\":1}
",
        );
        std::fs::create_dir_all(&l.raw_game_data).unwrap();

        let mut copied = migrate_old_workspace(&l, &p.repo).unwrap();
        copied.sort();
        assert_eq!(
            copied,
            [
                "native/paradox-runtime/SDK",
                "native/paradox-runtime/SDK.hpp",
                "native/paradox-runtime/x64/Release/MysticParadox.dll",
                "native/runtime-loader/target/release/winmm.dll",
                "paradox/Items_Analysis",
                "paradox/backend/game-data/progression.json",
                "paradox/backend/game-data/raw",
            ]
        );
        let read = |f: PathBuf| std::fs::read_to_string(f).unwrap();
        assert_eq!(
            read(l.sdk_dir.join("Basic_functions.cpp")),
            "fn
"
        );
        assert_eq!(
            read(l.backend_game_data.join("progression.json")),
            "{\"mine\":1}
"
        );
        assert!(
            !l.backend_game_data.join("progression.example.json").exists(),
            "the committed examples are not copied"
        );
        assert_eq!(
            read(l.director_game_data.join("hunts.json")),
            "{\"new\":1}
"
        );
        assert!(
            old.join("ParadoxRuntime").join("SDK.hpp").exists(),
            "the old folder is left alone"
        );
        assert!(
            migrate_old_workspace(&l, &p.repo).unwrap().is_empty(),
            "the second time there is nothing left to copy"
        );
    }
}
