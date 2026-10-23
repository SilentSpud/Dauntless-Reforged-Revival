//! The game's user config, rewritten before each launch (rows L30, RA9): Reforged's `src/main/engineini.ts`
//! and `display-repair.ts`.
//!
//! - Engine.ini: the launcher owns two sections and replaces them on every launch; every other section is
//!   kept. `[OnlineSubsystemMcp.XMPP]` points chat and presence at the launcher's relay
//!   (`ws://127.0.0.1:<relay port>`, no TLS on this PC) instead of Epic's old server; `[SystemSettings]`
//!   holds the forced graphics level, and is left out when the game picks it.
//! - GameUserSettings.ini: when a graphics level is forced, its `sg.*Quality` lines follow it; impossible
//!   saved window sizes (and, when asked, every size) are reset to a safe 1280x720 window.
//! - Game.ini: Dauntless Revived's `[OnlineSubsystemPhoenix]` endpoints, which point 1.14.7 at 127.0.0.1,
//!   are removed (`PHOENIX_ENDPOINTS_SECTION`).
//!
//! The config folder inside a Proton prefix is RA10's.

use std::path::{Path, PathBuf};

use crate::bridge::types::GraphicsPreset;
use crate::core::invite::is_valid_host;
use crate::core::text::{is_js_space, js_number, js_trim};

pub const SCALABILITY_GROUPS: [&str; 8] = [
    "ViewDistance",
    "AntiAliasing",
    "Shadow",
    "PostProcess",
    "Texture",
    "Effects",
    "Foliage",
    "Shading",
];

pub fn default_config_dir() -> PathBuf {
    let base = std::env::var_os("LOCALAPPDATA").map(PathBuf::from).unwrap_or_else(|| {
        let profile = std::env::var_os("USERPROFILE").map_or_else(|| PathBuf::from("C:\\"), PathBuf::from);
        profile.join("AppData").join("Local")
    });
    base.join("Archon").join("Saved").join("Config").join("WindowsClient")
}

pub fn decode_ini(data: &[u8]) -> String {
    if let Some(rest) = data.strip_prefix(&[0xff, 0xfe]) {
        let units: Vec<u16> = rest
            .as_chunks::<2>()
            .0
            .iter()
            .map(|pair| u16::from_le_bytes(*pair))
            .collect();
        return String::from_utf16_lossy(&units);
    }
    String::from_utf8_lossy(data.strip_prefix(&[0xef, 0xbb, 0xbf]).unwrap_or(data)).into_owned()
}

/// Plain ASCII (what play.ps1 writes) unless the kept lines hold other characters; then UTF-16 with a
/// byte order mark, which Unreal reads, instead of turning them into "?".
pub fn encode_ini(lines: &[String]) -> Vec<u8> {
    let text: String = lines.iter().map(|line| format!("{line}\r\n")).collect();
    if text.is_ascii() {
        return text.into_bytes();
    }
    let mut out = vec![0xff, 0xfe];
    out.extend(text.encode_utf16().flat_map(u16::to_le_bytes));
    out
}

/// Get-Content splits on CRLF, LF or CR and drops the final line break.
pub fn split_lines(text: &str) -> Vec<String> {
    if text.is_empty() {
        return Vec::new();
    }
    let mut lines = Vec::new();
    let mut current = String::new();
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '\r' => {
                if chars.peek() == Some(&'\n') {
                    chars.next();
                }
                lines.push(std::mem::take(&mut current));
            }
            '\n' => lines.push(std::mem::take(&mut current)),
            _ => current.push(c),
        }
    }
    lines.push(current);
    if lines.last().is_some_and(String::is_empty) {
        lines.pop();
    }
    lines
}

/// The `[SystemSettings]` section: only when a graphics level is forced, else nothing.
pub fn system_settings_lines(graphics: GraphicsPreset) -> Vec<String> {
    let level = graphics.value();
    if level < 0 {
        return Vec::new();
    }
    let mut lines = vec!["[SystemSettings]".to_owned()];
    lines.extend(
        SCALABILITY_GROUPS
            .iter()
            .map(|group| format!("sg.{group}Quality={level}")),
    );
    lines.extend(["sg.ResolutionQuality=100", "r.ScreenPercentage=100"].map(str::to_owned));
    lines
}

/// Chat and presence through the relay; None for a host or port the game must never be given.
pub fn xmpp_lines(host: &str, port: u16) -> Option<Vec<String>> {
    if !is_valid_host(host) || port == 0 {
        return None;
    }
    Some(vec![
        "[OnlineSubsystemMcp.XMPP]".into(),
        format!("ServerAddr=\"ws://{host}\""),
        format!("ServerPort={port}"),
        "bUseSSL=false".into(),
    ])
}

fn starts_with_ignore_case(text: &str, prefix: &str) -> bool {
    text.get(..prefix.len())
        .is_some_and(|head| head.eq_ignore_ascii_case(prefix))
}

pub fn rewrite_engine_ini(existing: &[String], host: &str, port: u16, graphics: GraphicsPreset) -> Option<Vec<String>> {
    let mut keep = Vec::new();
    let mut skip = false;
    for line in existing {
        if starts_with_ignore_case(line, "[SystemSettings]")
            || starts_with_ignore_case(line, "[OnlineSubsystemMcp.XMPP]")
        {
            skip = true;
            continue;
        }
        if skip && line.starts_with('[') {
            skip = false;
        }
        if !skip {
            keep.push(line.clone());
        }
    }
    // The blank lines a removed section leaves at the top go
    let blank = keep.iter().take_while(|line| js_trim(line).is_empty()).count();
    let mut out = system_settings_lines(graphics);
    if !out.is_empty() {
        out.push(String::new());
    }
    out.extend(xmpp_lines(host, port)?);
    out.push(String::new());
    out.extend(keep.into_iter().skip(blank));
    Some(out)
}

/// Dauntless Revived's launcher, in its local-server mode, writes every Phoenix service endpoint into
/// Game.ini as `http://127.0.0.1:61000/...`. Every Dauntless version reads the same config folder, so
/// 1.14.7 would send its own login checks (`CheckCrossPlayProgressionEndpoint`, ...) to 127.0.0.1, which the
/// runtime does not redirect: the login stops at "An error occurred while communicating with the game
/// servers". 1.14.7 needs none of these keys, so the section is removed before each launch.
pub const PHOENIX_ENDPOINTS_SECTION: &str = "OnlineSubsystemPhoenix";

/// `/^\s*\[([^\]]*)\]/`: the name a header line gives, which may be empty.
fn header_name(line: &str) -> Option<&str> {
    let rest = line.trim_start_matches(is_js_space).strip_prefix('[')?;
    rest.find(']').map(|end| &rest[..end])
}

/// `/^\s*[^\s;#][^=]*=/`: a key line, not a comment or a blank.
fn is_key_line(line: &str) -> bool {
    let mut chars = line.trim_start_matches(is_js_space).chars();
    chars
        .next()
        .is_some_and(|first| first != ';' && first != '#' && chars.as_str().contains('='))
}

/// The lines without one section (its header and everything up to the next header, the name matched in
/// any case), and how many keys it held.
pub fn remove_ini_section(lines: &[String], section: &str) -> (Vec<String>, usize) {
    let mut out = Vec::new();
    let mut skip = false;
    let mut removed = 0;
    for line in lines {
        let header = header_name(line);
        if let Some(name) = header {
            skip = js_trim(name).to_lowercase() == section.to_lowercase();
        }
        if !skip {
            out.push(line.clone());
        } else if header.is_none() && is_key_line(line) {
            removed += 1;
        }
    }
    (out, removed)
}

pub fn rewrite_game_user_settings(lines: &[String], graphics: GraphicsPreset) -> Vec<String> {
    let level = graphics.value();
    if level < 0 {
        return lines.to_vec();
    }
    lines
        .iter()
        .map(|line| {
            match SCALABILITY_GROUPS
                .iter()
                .find(|group| starts_with_ignore_case(line, &format!("sg.{group}Quality=")))
            {
                Some(group) => format!("sg.{group}Quality={level}"),
                None => line.clone(),
            }
        })
        .collect()
}

// ---------------------------------------------------------------- display repair

/// `/^\s*\[([^\]]+)\]/`: the section a heading line names.
fn heading(line: &str) -> Option<&str> {
    let rest = line.trim_start_matches(is_js_space).strip_prefix('[')?;
    let end = rest.find(']')?;
    (end > 0).then(|| &rest[..end])
}

/// `/(?:^|\.)[A-Za-z]*GameUserSettings$/i`
fn is_settings_section(name: &str) -> bool {
    const SUFFIX: &str = "gameusersettings";
    if name.len() < SUFFIX.len()
        || !name.is_char_boundary(name.len() - SUFFIX.len())
        || !name[name.len() - SUFFIX.len()..].eq_ignore_ascii_case(SUFFIX)
    {
        return false;
    }
    let before = name[..name.len() - SUFFIX.len()].trim_end_matches(|c: char| c.is_ascii_alphabetic());
    before.is_empty() || before.ends_with('.')
}

/// `/^\s*([^=]+?)\s*=/`: the key, without the white space around it.
fn key_of(line: &str) -> Option<&str> {
    let (key, _) = line.trim_start_matches(is_js_space).split_once('=')?;
    let key = key.trim_end_matches(is_js_space);
    (!key.is_empty()).then_some(key)
}

/// `/^\s*([^=]+?)\s*=\s*([^;]*)/`: the value, up to a `;` comment.
fn value_of(line: &str) -> Option<&str> {
    key_of(line)?;
    let (_, value) = line.split_once('=')?;
    let value = value.trim_start_matches(is_js_space);
    Some(value.split(';').next().unwrap_or(""))
}

fn is_dimension(key: &str) -> bool {
    let key = key.to_ascii_lowercase();
    [
        "resolutionsizex",
        "resolutionsizey",
        "lastuserconfirmedresolutionsizex",
        "lastuserconfirmedresolutionsizey",
        "desiredscreenwidth",
        "desiredscreenheight",
        "lastuserconfirmeddesiredscreenwidth",
        "lastuserconfirmeddesiredscreenheight",
    ]
    .contains(&key.as_str())
}

fn is_window_mode(key: &str) -> bool {
    let key = key.to_ascii_lowercase();
    [
        "fullscreenmode",
        "lastconfirmedfullscreenmode",
        "preferredfullscreenmode",
        "windowposx",
        "windowposy",
    ]
    .contains(&key.as_str())
}

const SAFE_WINDOW: [&str; 9] = [
    "ResolutionSizeX=1280",
    "ResolutionSizeY=720",
    "LastUserConfirmedResolutionSizeX=1280",
    "LastUserConfirmedResolutionSizeY=720",
    "FullscreenMode=2",
    "LastConfirmedFullscreenMode=2",
    "PreferredFullscreenMode=2",
    "WindowPosX=-1",
    "WindowPosY=-1",
];

/// The shipped D3D11 client cannot make a texture wider or taller than 16384: only impossible saved sizes
/// are repaired (or every size, when the player asked for the safe window). (lines, repaired)
pub fn repair_display_settings(lines: &[String], force_safe_window: bool) -> (Vec<String>, bool) {
    let mut section = false;
    let bad = lines.iter().any(|line| {
        if let Some(name) = heading(line) {
            section = is_settings_section(name);
        }
        let (Some(key), Some(value)) = (key_of(line), value_of(line)) else {
            return false;
        };
        if !section || !is_dimension(key) {
            return false;
        }
        let value = js_number(value);
        !value.is_finite() || !(1.0..=16384.0).contains(&value)
    });
    if !bad && !force_safe_window {
        return (lines.to_vec(), false);
    }
    // A clean or missing file can still inherit a bad desktop or full-screen size
    let mut lines = lines.to_vec();
    if !lines.iter().any(|line| heading(line).is_some_and(is_settings_section)) {
        lines.push("[/Script/Engine.GameUserSettings]".into());
        lines.push("[/Script/Archon.ArchonGameUserSettings]".into());
    }
    let mut section = false;
    let mut out = Vec::new();
    for line in &lines {
        if let Some(name) = heading(line) {
            section = is_settings_section(name);
            out.push(line.clone());
            if section {
                out.extend(SAFE_WINDOW.map(str::to_owned));
            }
            continue;
        }
        if section && key_of(line).is_some_and(|key| is_dimension(key) || is_window_mode(key)) {
            continue;
        }
        out.push(line.clone());
    }
    let repaired = out.join("\n") != lines.join("\n");
    (out, repaired)
}

// ---------------------------------------------------------------- applying it

fn read_lines(file: &Path) -> std::io::Result<Option<Vec<String>>> {
    match std::fs::read(file) {
        Ok(data) => Ok(Some(split_lines(&decode_ini(&data)))),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error),
    }
}

fn write_atomically(file: &Path, data: &[u8]) -> std::io::Result<()> {
    let mut temporary = file.as_os_str().to_owned();
    temporary.push(".rr-tmp");
    std::fs::write(&temporary, data)?;
    std::fs::rename(&temporary, file)
}

fn with_suffix(file: &Path, suffix: &str) -> PathBuf {
    let mut name = file.as_os_str().to_owned();
    name.push(suffix);
    PathBuf::from(name)
}

pub struct ApplyConfig<'a> {
    /// Where chat and presence (XMPP) connect: the relay
    pub host: &'a str,
    /// The relay's port
    pub port: u16,
    pub graphics: GraphicsPreset,
    pub config_dir: PathBuf,
    pub safe_window: bool,
    /// Remove Dauntless Revived's `[OnlineSubsystemPhoenix]` endpoints from Game.ini
    pub clear_phoenix_endpoints: bool,
}

impl<'a> ApplyConfig<'a> {
    #[cfg_attr(not(test), expect(dead_code, reason = "the tests use it"))]
    pub fn new(host: &'a str, port: u16, graphics: GraphicsPreset) -> Self {
        Self {
            host,
            port,
            graphics,
            config_dir: default_config_dir(),
            safe_window: false,
            clear_phoenix_endpoints: true,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AppliedConfig {
    pub engine_ini: PathBuf,
    pub display_repaired: bool,
    /// The keys taken out of Game.ini (the controller logs it)
    pub phoenix_endpoints_removed: usize,
}

/// Cleans Game.ini, writes Engine.ini and, when needed, GameUserSettings.ini.
pub fn apply_game_config(options: &ApplyConfig<'_>) -> std::io::Result<AppliedConfig> {
    let dir = &options.config_dir;
    std::fs::create_dir_all(dir)?;
    let mut phoenix_endpoints_removed = 0;
    if options.clear_phoenix_endpoints {
        let game_ini = dir.join("Game.ini");
        // A missing Game.ini is left alone
        if let Some(lines) = read_lines(&game_ini)? {
            let (cleaned, removed) = remove_ini_section(&lines, PHOENIX_ENDPOINTS_SECTION);
            if cleaned.len() != lines.len() {
                std::fs::copy(&game_ini, with_suffix(&game_ini, ".before-reforged"))?;
                write_atomically(&game_ini, &encode_ini(&cleaned))?;
                phoenix_endpoints_removed = removed;
            }
        }
    }

    let engine = dir.join("Engine.ini");
    let existing = read_lines(&engine)?.unwrap_or_default();
    let lines = rewrite_engine_ini(&existing, options.host, options.port, options.graphics)
        .ok_or_else(|| std::io::Error::new(std::io::ErrorKind::InvalidInput, "invalid host or port"))?;
    write_atomically(&engine, &encode_ini(&lines))?;

    let settings = dir.join("GameUserSettings.ini");
    let current = read_lines(&settings)?;
    let mut display_repaired = false;
    if current.is_some() || options.safe_window {
        let (lines, changed) = repair_display_settings(current.as_deref().unwrap_or_default(), options.safe_window);
        display_repaired = changed;
        if display_repaired && current.is_some() {
            let stamp = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map_or(0, |since| since.as_millis());
            std::fs::copy(
                &settings,
                with_suffix(&settings, &format!(".before-display-repair-{stamp}")),
            )?;
        }
        let forced = options.graphics.value() >= 0;
        let next = if forced {
            rewrite_game_user_settings(&lines, options.graphics)
        } else {
            lines
        };
        if display_repaired || forced {
            write_atomically(&settings, &encode_ini(&next))?;
        }
    }
    Ok(AppliedConfig {
        engine_ini: engine,
        display_repaired,
        phoenix_endpoints_removed,
    })
}
