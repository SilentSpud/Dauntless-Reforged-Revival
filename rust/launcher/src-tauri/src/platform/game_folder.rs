//! Finding the player's own Dauntless 1.14.7 (rows L30, RA9): Reforged's `src/main/game-folder.ts`. The
//! launcher never downloads the game: it uses the copy the player installed from Epic (with Legendary and
//! the 1.14.7 manifest, see `docs/players.md`), wherever it is.
//!
//! A pasted or picked folder is tried in these layouts only (nothing is searched): the folder itself,
//! `<folder>\Dauntless`, the folder above `Archon`, and three folders above `Archon\Binaries\Win64` (or the
//! exe itself).
//!
//! Only drive-letter paths are accepted on Windows, before any file-system call: a UNC, device or
//! root-relative path would make Windows contact another machine, possibly with the player's network
//! credentials. The root is kept as typed or picked (no realpath), at most `MAX_GAME_ROOT` characters.
//!
//! Installs are also found by asking the stores: the Epic Games Launcher's manifests, Legendary's
//! `installed.json` and Heroic's copy of it. That is Windows only here; the same on Linux (Heroic and
//! Legendary there, the Proton prefix) is RA10's.

use std::io::Read as _;
use std::path::{Path, PathBuf};

use serde_json::Value;

use crate::bridge::types::{DetectedGame, GameSource};
use crate::core::pins::{EXE_NAME, EXE_RELATIVE_PATH};
use crate::core::text::{js_length, js_trim};
use crate::platform::settings::Platform;

pub const MAX_GAME_INPUT: usize = 260;
pub const MAX_GAME_ROOT: usize = 180;

/// Dauntless 1.14.7's `Version.txt` (rel-1.14.7-Archon)
pub const GAME_VERSION_TEXT: &str = "dauntless_rel-1.14.7_Shipping_2024-04-01_14-59-21_647472";
pub const GAME_CHANGELIST: u32 = 647_472;
pub const VERSION_FILE: &str = "Version.txt";
/// Dauntless in the Epic catalog, Legendary and Heroic
pub const EPIC_APP_NAME: &str = "Jackal";

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GameFolder {
    Found(String),
    InvalidPath,
    NotFound,
    TooLong,
}

fn separator(platform: Platform) -> char {
    match platform {
        Platform::Windows => '\\',
        Platform::Linux => '/',
    }
}

/// `path.win32.resolve` / `path.posix.resolve` of an absolute path: separators made one, `.` and `..`
/// folded, no trailing separator except the root's.
fn normalize(path: &str, platform: Platform) -> String {
    let (root, rest) = match platform {
        Platform::Windows => (format!("{}\\", &path[..2]), &path[2..]),
        Platform::Linux => ("/".to_owned(), path),
    };
    let mut parts: Vec<&str> = Vec::new();
    for part in rest.split(|c| c == '/' || platform == Platform::Windows && c == '\\') {
        match part {
            "" | "." => {}
            ".." => {
                parts.pop();
            }
            _ => parts.push(part),
        }
    }
    format!("{root}{}", parts.join(&separator(platform).to_string()))
}

/// The folder as a clean absolute path, or None. Explorer's "Copy as path" adds quotes.
pub fn clean_game_folder_input(input: &str, platform: Platform) -> Option<String> {
    let mut text = js_trim(input);
    if text.len() >= 2 && text.starts_with('"') && text.ends_with('"') {
        text = js_trim(&text[1..text.len() - 1]);
    }
    if text.is_empty() || js_length(text) > MAX_GAME_INPUT || text.contains('\0') {
        return None;
    }
    let bytes = text.as_bytes();
    match platform {
        Platform::Windows
            if bytes.len() >= 3
                && bytes[0].is_ascii_alphabetic()
                && bytes[1] == b':'
                && (bytes[2] == b'\\' || bytes[2] == b'/') =>
        {
            Some(normalize(text, platform))
        }
        // Off Windows any absolute POSIX path
        Platform::Linux if text.starts_with('/') => Some(normalize(text, platform)),
        _ => None,
    }
}

/// The last part of a clean path (`path.basename`).
fn base_name(path: &str, platform: Platform) -> &str {
    path.rsplit(separator(platform)).next().unwrap_or_default()
}

pub fn game_root_candidates(folder: &str, platform: Platform) -> Vec<String> {
    let sep = separator(platform);
    let mut base = folder.to_owned();
    // The executable itself was picked or pasted
    if base_name(&base, platform).eq_ignore_ascii_case(EXE_NAME) {
        base = normalize(&format!("{base}{sep}.."), platform);
    }
    let mut out = vec![base.clone(), normalize(&format!("{base}{sep}Dauntless"), platform)];
    let name = base_name(&base, platform).to_lowercase();
    if name == "archon" {
        out.push(normalize(&format!("{base}{sep}.."), platform));
    }
    if name == "win64" {
        out.push(normalize(&format!("{base}{sep}..{sep}..{sep}.."), platform));
    }
    let mut unique: Vec<String> = Vec::with_capacity(out.len());
    for candidate in out {
        if !unique.contains(&candidate) {
            unique.push(candidate);
        }
    }
    unique
}

/// The game's exe is a file under `root`.
pub fn has_game_exe(root: &str) -> bool {
    let exe = EXE_RELATIVE_PATH
        .split('/')
        .fold(PathBuf::from(root), |path, part| path.join(part));
    std::fs::metadata(exe).is_ok_and(|metadata| metadata.is_file())
}

pub fn locate_existing_game(input: &str, platform: Platform) -> GameFolder {
    let Some(folder) = clean_game_folder_input(input, platform) else {
        return GameFolder::InvalidPath;
    };
    let mut too_long = false;
    for root in game_root_candidates(&folder, platform) {
        if !has_game_exe(&root) {
            continue;
        }
        if js_length(&root) <= MAX_GAME_ROOT {
            return GameFolder::Found(root);
        }
        too_long = true;
    }
    if too_long {
        GameFolder::TooLong
    } else {
        GameFolder::NotFound
    }
}

// ---------------------------------------------------------------- Version.txt

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VersionInfo {
    pub text: Option<String>,
    /// Names 1.14.7 and changelist 647472
    pub ok: bool,
}

/// The pinned text, or any text naming `rel-1.14.7` and the changelist as a whole number.
pub fn is_supported_version_text(text: &str) -> bool {
    if text == GAME_VERSION_TEXT {
        return true;
    }
    if !text.to_ascii_lowercase().contains("rel-1.14.7") {
        return false;
    }
    let changelist = GAME_CHANGELIST.to_string();
    text.match_indices(&changelist).any(|(at, found)| {
        let before = text[..at].chars().next_back();
        let after = text[at + found.len()..].chars().next();
        !before.is_some_and(|c| c.is_ascii_digit()) && !after.is_some_and(|c| c.is_ascii_digit())
    })
}

/// The first line of `Version.txt` (at most 256 bytes read): 1 to 128 printable ASCII characters, or none.
pub fn read_game_version(root: &Path) -> VersionInfo {
    let none = VersionInfo { text: None, ok: false };
    let Ok(file) = std::fs::File::open(root.join(VERSION_FILE)) else {
        return none;
    };
    let mut data = Vec::with_capacity(256);
    if file.take(256).read_to_end(&mut data).is_err() {
        return none;
    }
    let text = String::from_utf8_lossy(&data);
    let text = js_trim(text.strip_prefix('\u{feff}').unwrap_or(&text));
    let first = text.split('\n').next().unwrap_or_default();
    let first = js_trim(first.strip_suffix('\r').unwrap_or(first));
    let clean = (1..=128).contains(&first.len()) && first.bytes().all(|b| (0x20..=0x7e).contains(&b));
    if !clean {
        return none;
    }
    VersionInfo {
        text: Some(first.to_owned()),
        ok: is_supported_version_text(first),
    }
}

// ---------------------------------------------------------------- store detection

/// The folders the stores keep their lists in (`%ProgramData%`, `%USERPROFILE%`, `%APPDATA%`,
/// `LEGENDARY_CONFIG_PATH`).
#[derive(Debug, Clone, Default)]
pub struct StoreEnv {
    pub program_data: Option<PathBuf>,
    pub user_profile: Option<PathBuf>,
    pub app_data: Option<PathBuf>,
    pub legendary_config_path: Option<PathBuf>,
}

impl StoreEnv {
    pub fn from_system() -> Self {
        let var = |name: &str| {
            std::env::var_os(name)
                .filter(|value| !value.is_empty())
                .map(PathBuf::from)
        };
        Self {
            program_data: var("ProgramData"),
            user_profile: var("USERPROFILE"),
            app_data: var("APPDATA"),
            legendary_config_path: var("LEGENDARY_CONFIG_PATH"),
        }
    }
}

const MAX_JSON_BYTES: u64 = 4 * 1024 * 1024;
const MAX_ITEM_BYTES: u64 = 1024 * 1024;
const MAX_ITEMS: usize = 500;

/// A regular file of at most `max_bytes`, parsed as JSON (a byte order mark is allowed); else None.
fn read_json(file: &Path, max_bytes: u64) -> Option<Value> {
    let metadata = std::fs::metadata(file).ok()?;
    if !metadata.is_file() || metadata.len() > max_bytes {
        return None;
    }
    let data = std::fs::read(file).ok()?;
    let text = String::from_utf8_lossy(&data);
    serde_json::from_str(text.strip_prefix('\u{feff}').unwrap_or(&text)).ok()
}

/// `String.prototype.slice(0, 64)`: at most 64 UTF-16 units.
fn version_text(value: Option<&Value>) -> Option<String> {
    let units: Vec<u16> = value?.as_str()?.encode_utf16().take(64).collect();
    Some(String::from_utf16_lossy(&units))
}

/// A non-empty path of at most `MAX_GAME_INPUT` characters.
fn install_path(value: Option<&Value>) -> Option<String> {
    let path = value?.as_str()?;
    (!path.is_empty() && js_length(path) <= MAX_GAME_INPUT).then(|| path.to_owned())
}

/// Legendary's `installed.json` (and Heroic's copy of it): `{"Jackal": {"install_path", "version", …}}`.
pub fn parse_legendary_installed(raw: &Value, source: GameSource) -> Vec<DetectedGame> {
    let Some(entries) = raw.as_object() else {
        return Vec::new();
    };
    entries
        .iter()
        .filter_map(|(key, entry)| {
            let entry = entry.as_object()?;
            let name = entry.get("app_name").and_then(Value::as_str).unwrap_or(key);
            if name != EPIC_APP_NAME {
                return None;
            }
            Some(DetectedGame {
                root: install_path(entry.get("install_path"))?,
                source,
                version: version_text(entry.get("version")),
            })
        })
        .collect()
}

/// One Epic Games Launcher manifest (`<ProgramData>\Epic\EpicGamesLauncher\Data\Manifests\*.item`).
pub fn parse_epic_item(raw: &Value) -> Option<DetectedGame> {
    let item = raw.as_object()?;
    let is = |key: &str, value: &str| item.get(key).and_then(Value::as_str) == Some(value);
    if !(is("AppName", EPIC_APP_NAME) || is("MainGameAppName", EPIC_APP_NAME) || is("DisplayName", "Dauntless")) {
        return None;
    }
    Some(DetectedGame {
        root: install_path(item.get("InstallLocation"))?,
        source: GameSource::Epic,
        version: version_text(item.get("AppVersionString")),
    })
}

/// The installs the stores know of that are really there (the manifest of an uninstalled game can
/// linger), each folder once; the version is the store's, else `Version.txt`'s.
pub fn detect_store_installs(env: &StoreEnv, platform: Platform) -> Vec<DetectedGame> {
    if platform != Platform::Windows {
        // RA10: Heroic and Legendary on Linux
        return Vec::new();
    }
    let mut found = Vec::new();
    let mut legendary_dirs: Vec<PathBuf> = Vec::new();
    if let Some(dir) = &env.legendary_config_path {
        legendary_dirs.push(dir.clone());
    }
    if let Some(profile) = &env.user_profile {
        let dir = profile.join(".config").join("legendary");
        if !legendary_dirs.contains(&dir) {
            legendary_dirs.push(dir);
        }
    }
    for dir in legendary_dirs {
        if let Some(raw) = read_json(&dir.join("installed.json"), MAX_JSON_BYTES) {
            found.extend(parse_legendary_installed(&raw, GameSource::Legendary));
        }
    }
    if let Some(app_data) = &env.app_data {
        let file = app_data
            .join("heroic")
            .join("legendaryConfig")
            .join("legendary")
            .join("installed.json");
        if let Some(raw) = read_json(&file, MAX_JSON_BYTES) {
            found.extend(parse_legendary_installed(&raw, GameSource::Heroic));
        }
    }
    if let Some(program_data) = &env.program_data {
        let manifests = program_data
            .join("Epic")
            .join("EpicGamesLauncher")
            .join("Data")
            .join("Manifests");
        let names: Vec<PathBuf> = std::fs::read_dir(&manifests)
            .map(|entries| {
                entries
                    .filter_map(Result::ok)
                    .map(|entry| entry.file_name().to_string_lossy().into_owned())
                    .filter(|name| name.to_lowercase().ends_with(".item"))
                    .take(MAX_ITEMS)
                    .map(|name| manifests.join(name))
                    .collect()
            })
            .unwrap_or_default();
        found.extend(
            names
                .iter()
                .filter_map(|file| read_json(file, MAX_ITEM_BYTES))
                .filter_map(|raw| parse_epic_item(&raw)),
        );
    }
    let mut out: Vec<DetectedGame> = Vec::new();
    for detected in found {
        let GameFolder::Found(root) = locate_existing_game(&detected.root, platform) else {
            continue;
        };
        if out.iter().any(|known| known.root.to_lowercase() == root.to_lowercase()) {
            continue;
        }
        let version = detected.version.or_else(|| read_game_version(Path::new(&root)).text);
        out.push(DetectedGame {
            root,
            source: detected.source,
            version,
        });
    }
    out
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    /// `game.test.ts`'s `fakeGame`: a game root with the exe and, unless None, a `Version.txt` with a byte
    /// order mark and a line break.
    fn fake_game(dir: &Path, version: Option<&str>) -> PathBuf {
        let root = dir.join("Dauntless");
        let exe = EXE_RELATIVE_PATH
            .split('/')
            .fold(root.clone(), |path, part| path.join(part));
        std::fs::create_dir_all(exe.parent().unwrap()).unwrap();
        std::fs::write(&exe, b"MZ fake").unwrap();
        if let Some(version) = version {
            std::fs::write(root.join(VERSION_FILE), format!("\u{feff}{version}\r\n")).unwrap();
        }
        root
    }

    fn text(path: &Path) -> String {
        path.to_string_lossy().into_owned()
    }

    /// `game.test.ts`: "cleanGameFolderInput: drive-letter paths only on Windows, quotes from Copy as path
    /// removed"
    #[test]
    fn clean_game_folder_input_takes_drive_letter_paths_only_on_windows() {
        let win = |input| clean_game_folder_input(input, Platform::Windows);
        assert_eq!(win("\"C:\\Games\\Dauntless\"").as_deref(), Some("C:\\Games\\Dauntless"));
        assert_eq!(win("C:/Games/Dauntless").as_deref(), Some("C:\\Games\\Dauntless"));
        assert_eq!(win("\\\\server\\share\\Dauntless"), None, "UNC paths are never touched");
        assert_eq!(win("\\\\?\\C:\\Games"), None);
        assert_eq!(win("\\Games"), None);
        assert_eq!(win("Games\\Dauntless"), None);
        assert_eq!(win(""), None);
        assert_eq!(win("C:\\a\0b"), None);
        assert_eq!(
            clean_game_folder_input("/home/me/Dauntless", Platform::Linux).as_deref(),
            Some("/home/me/Dauntless")
        );
        assert_eq!(clean_game_folder_input("relative/path", Platform::Linux), None);
    }

    /// `game.test.ts`: "gameRootCandidates: the root, a Dauntless subfolder, Archon's parent, three up from
    /// Win64 or the exe"
    #[test]
    fn game_root_candidates_cover_the_known_layouts() {
        let candidates = |folder| game_root_candidates(folder, Platform::Windows);
        assert_eq!(
            candidates("C:\\Games\\Dauntless"),
            ["C:\\Games\\Dauntless", "C:\\Games\\Dauntless\\Dauntless"]
        );
        for folder in [
            "C:\\Games\\Dauntless\\Archon",
            "C:\\Games\\Dauntless\\Archon\\Binaries\\Win64",
            "C:\\Games\\Dauntless\\Archon\\Binaries\\Win64\\Dauntless-Win64-Shipping.exe",
            "C:\\Games\\Dauntless\\Archon\\Binaries\\Win64\\DAUNTLESS-WIN64-SHIPPING.EXE",
        ] {
            assert!(
                candidates(folder).contains(&"C:\\Games\\Dauntless".to_owned()),
                "{folder}"
            );
        }
        assert_eq!(
            game_root_candidates("/", Platform::Linux),
            ["/", "/Dauntless"],
            "no duplicates"
        );
    }

    /// `game.test.ts`: "locateExistingGame finds the game from any of the layouts"
    #[test]
    fn locate_existing_game_finds_the_game_from_any_layout() {
        let dir = tempfile::tempdir().unwrap();
        let root = fake_game(dir.path(), Some(GAME_VERSION_TEXT));
        let platform = Platform::current();
        let win64 = root.join("Archon").join("Binaries").join("Win64");
        for input in [
            root.clone(),
            dir.path().to_path_buf(),
            root.join("Archon"),
            win64.clone(),
            win64.join(EXE_NAME),
        ] {
            assert_eq!(
                locate_existing_game(&text(&input), platform),
                GameFolder::Found(text(&root)),
                "{input:?}"
            );
        }
        assert_eq!(
            locate_existing_game(&format!("\"{}\"", text(&root)), platform),
            GameFolder::Found(text(&root))
        );
        assert_eq!(
            locate_existing_game(&text(&root.join("nothing-here")), platform),
            GameFolder::NotFound
        );
        assert_eq!(locate_existing_game("not absolute", platform), GameFolder::InvalidPath);
    }

    #[test]
    fn a_root_past_the_limit_is_too_long() {
        let dir = tempfile::tempdir().unwrap();
        let mut deep = dir.path().to_path_buf();
        while text(&deep).len() <= MAX_GAME_ROOT {
            deep = deep.join("long");
        }
        let root = fake_game(&deep, None);
        assert_eq!(
            locate_existing_game(&text(&root), Platform::current()),
            GameFolder::TooLong
        );
    }

    /// `game.test.ts`: "Version.txt: 1.14.7 (CL 647472) is accepted, other builds are not"
    #[test]
    fn version_txt_accepts_1_14_7_only() {
        assert!(is_supported_version_text(GAME_VERSION_TEXT));
        assert!(is_supported_version_text(
            "dauntless_rel-1.14.7_Shipping_2024-04-02_10-00-00_647472"
        ));
        assert!(is_supported_version_text("DAUNTLESS_REL-1.14.7 647472"));
        assert!(!is_supported_version_text(
            "dauntless_rel-1.4.4_Shipping_2020-10-28_20-11-15_239827"
        ));
        assert!(!is_supported_version_text(
            "dauntless_rel-1.14.7_Shipping_2024-04-01_14-59-21_6474720"
        ));
        assert!(!is_supported_version_text(
            "dauntless_rel-1.14.6_Shipping_2024-03-01_647472"
        ));
        let version = |content: Option<&str>| {
            let dir = tempfile::tempdir().unwrap();
            read_game_version(&fake_game(dir.path(), content))
        };
        assert_eq!(
            version(Some(GAME_VERSION_TEXT)),
            VersionInfo {
                text: Some(GAME_VERSION_TEXT.into()),
                ok: true
            }
        );
        assert_eq!(
            version(Some("dauntless_rel-1.12.0_Shipping_x_600000")),
            VersionInfo {
                text: Some("dauntless_rel-1.12.0_Shipping_x_600000".into()),
                ok: false
            }
        );
        assert_eq!(version(None), VersionInfo { text: None, ok: false });
        assert_eq!(version(Some("bad\u{7}text")), VersionInfo { text: None, ok: false });
        assert_eq!(version(Some(&"x".repeat(129))), VersionInfo { text: None, ok: false });
    }

    /// `game.test.ts`: "store manifests: Legendary's installed.json and the Epic Games Launcher's .item
    /// files"
    #[test]
    fn store_manifests() {
        let installed = json!({
            "Jackal": { "app_name": "Jackal", "install_path": "D:\\Games\\Dauntless", "version": "1.14.7" },
            "Other": { "app_name": "Fortnite", "install_path": "D:\\Games\\Fortnite" },
            "Broken": { "app_name": "Jackal", "install_path": 7 },
        });
        assert_eq!(
            parse_legendary_installed(&installed, GameSource::Legendary),
            [DetectedGame {
                root: "D:\\Games\\Dauntless".into(),
                source: GameSource::Legendary,
                version: Some("1.14.7".into())
            }]
        );
        assert!(parse_legendary_installed(&Value::Null, GameSource::Heroic).is_empty());
        assert_eq!(
            parse_legendary_installed(
                &json!({ "Jackal": { "install_path": "D:\\D", "version": "v".repeat(70) } }),
                GameSource::Heroic
            )[0]
            .version
            .as_deref(),
            Some("v".repeat(64).as_str()),
            "the key names the app when app_name is missing; the version is cut to 64"
        );
        assert_eq!(
            parse_epic_item(&json!({
                "AppName": "Jackal",
                "InstallLocation": "C:\\Program Files\\Epic Games\\Dauntless",
                "AppVersionString": "1.14.7"
            })),
            Some(DetectedGame {
                root: "C:\\Program Files\\Epic Games\\Dauntless".into(),
                source: GameSource::Epic,
                version: Some("1.14.7".into())
            })
        );
        assert_eq!(
            parse_epic_item(&json!({ "AppName": "Fortnite", "InstallLocation": "C:\\x" })),
            None
        );
        assert_eq!(parse_epic_item(&json!({ "AppName": "Jackal" })), None);
        assert_eq!(
            parse_epic_item(&json!({ "DisplayName": "Dauntless", "InstallLocation": "C:\\x" }))
                .map(|game| game.version),
            Some(None)
        );
    }

    #[test]
    fn store_detection_keeps_installs_that_are_there_once() {
        let dir = tempfile::tempdir().unwrap();
        let base = dir.path();
        let root = fake_game(&base.join("Games"), Some(GAME_VERSION_TEXT));
        let root_text = text(&root);
        let legendary = base.join("profile").join(".config").join("legendary");
        std::fs::create_dir_all(&legendary).unwrap();
        std::fs::write(
            legendary.join("installed.json"),
            format!(
                "\u{feff}{}",
                json!({ "Jackal": { "app_name": "Jackal", "install_path": root_text, "version": "1.14.7" } })
            ),
        )
        .unwrap();
        let heroic = base
            .join("appdata")
            .join("heroic")
            .join("legendaryConfig")
            .join("legendary");
        std::fs::create_dir_all(&heroic).unwrap();
        // The same folder in another case, and one that is gone
        std::fs::write(
            heroic.join("installed.json"),
            json!({
                "Jackal": { "install_path": root_text.to_uppercase() },
                "Gone": { "app_name": "Jackal", "install_path": text(&base.join("gone")) },
            })
            .to_string(),
        )
        .unwrap();
        let manifests = base
            .join("programdata")
            .join("Epic")
            .join("EpicGamesLauncher")
            .join("Data")
            .join("Manifests");
        std::fs::create_dir_all(&manifests).unwrap();
        let epic_root = fake_game(&base.join("Epic Games"), None);
        std::fs::write(
            manifests.join("A1.ITEM"),
            json!({ "AppName": "Jackal", "InstallLocation": text(&epic_root) }).to_string(),
        )
        .unwrap();
        std::fs::write(manifests.join("big.item"), " ".repeat(1024 * 1024 + 1)).unwrap();
        std::fs::write(manifests.join("other.txt"), "{}").unwrap();
        let env = StoreEnv {
            program_data: Some(base.join("programdata")),
            user_profile: Some(base.join("profile")),
            app_data: Some(base.join("appdata")),
            legendary_config_path: None,
        };
        let platform = Platform::current();
        let found = detect_store_installs(&env, platform);
        if platform != Platform::Windows {
            assert!(found.is_empty(), "RA10 fills Linux");
            return;
        }
        assert_eq!(
            found,
            [
                DetectedGame {
                    root: root_text,
                    source: GameSource::Legendary,
                    version: Some("1.14.7".into())
                },
                DetectedGame {
                    root: text(&epic_root),
                    source: GameSource::Epic,
                    version: None
                },
            ]
        );
        assert!(detect_store_installs(&StoreEnv::default(), platform).is_empty());
    }
}
