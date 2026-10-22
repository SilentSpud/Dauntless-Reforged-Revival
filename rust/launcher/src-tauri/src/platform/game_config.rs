//! The game's user config, rewritten before each launch exactly like `friend-kit/play.ps1` (row L30):
//! `src/main/engineini.ts` and `display-repair.ts`.
//!
//! - Engine.ini: `[SystemSettings]` (memory lines, the forced graphics level if one is chosen, and
//!   `r.EyeAdaptation.MethodOverride=2` for "Basic adaptive" exposure) and `[OnlineSubsystemMcp.XMPP]`
//!   (chat pointed at the host instead of Epic's live server) are replaced; every other section stays.
//!   Private mode: `ws://<host>:61099`. Public mode: `ws://127.0.0.1:<relay port>`.
//! - GameUserSettings.ini: when a graphics level is forced, its `sg.*Quality` lines follow it; impossible
//!   saved window sizes (and, when asked, every size) are reset to a safe 1280x720 window.

use std::path::{Path, PathBuf};

use crate::bridge::types::GraphicsPreset;
use crate::core::invite::is_valid_host;
use crate::core::pins::XMPP_PORT;
use crate::core::text::{is_js_space, js_number};

/// DR's exposure setting: keep the game's scene-specific exposure, or basic as an opt-in experiment. No
/// longer a bridge type (Reforged has no such setting); RA9 decides whether it goes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ExposureMode {
    Game,
    Basic,
}

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

/// The `[SystemSettings]` section. With "game" exposure it is byte for byte what play.ps1 writes.
pub fn system_settings_lines(graphics: GraphicsPreset, exposure: ExposureMode) -> Vec<String> {
    let mut lines: Vec<String> = [
        "[SystemSettings]",
        "r.Streaming.PoolSize=3000",
        "r.Streaming.LimitPoolSizeToVRAM=1",
        "gc.TimeBetweenPurgingPendingKillObjects=10",
        "s.ForceGCAfterLevelStreamedOut=1",
    ]
    .map(str::to_owned)
    .to_vec();
    let level = graphics.value();
    if level >= 0 {
        lines.extend(
            SCALABILITY_GROUPS
                .iter()
                .map(|group| format!("sg.{group}Quality={level}")),
        );
        lines.extend(
            [
                "sg.ResolutionQuality=100",
                "r.ScreenPercentage=100",
                "r.MipMapLODBias=0",
                "r.MaxAnisotropy=16",
                "r.Tonemapper.Sharpen=0.6",
            ]
            .map(str::to_owned),
        );
    }
    // "2: Auto Basic" in the pinned executable's help: adaptation stays on in dark maps
    if exposure == ExposureMode::Basic {
        lines.push("r.EyeAdaptation.MethodOverride=2".into());
    }
    lines
}

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

pub fn rewrite_engine_ini(
    existing: &[String],
    host: &str,
    graphics: GraphicsPreset,
    xmpp_port: u16,
    exposure: ExposureMode,
) -> Option<Vec<String>> {
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
    let mut out = system_settings_lines(graphics, exposure);
    out.push(String::new());
    out.extend(xmpp_lines(host, xmpp_port)?);
    out.push(String::new());
    out.extend(keep);
    Some(out)
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
    temporary.push(".dr-tmp");
    std::fs::write(&temporary, data)?;
    std::fs::rename(&temporary, file)
}

pub struct ApplyConfig<'a> {
    /// Where chat and presence (XMPP) connect
    pub host: &'a str,
    /// 61099 in private mode; the relay port in public mode
    pub xmpp_port: u16,
    pub graphics: GraphicsPreset,
    pub exposure: ExposureMode,
    pub config_dir: PathBuf,
    pub safe_window: bool,
}

impl<'a> ApplyConfig<'a> {
    #[cfg_attr(not(test), expect(dead_code, reason = "the tests use it"))]
    pub fn new(host: &'a str, graphics: GraphicsPreset) -> Self {
        Self {
            host,
            xmpp_port: XMPP_PORT,
            graphics,
            exposure: ExposureMode::Game,
            config_dir: default_config_dir(),
            safe_window: false,
        }
    }
}

/// Writes Engine.ini and, when needed, GameUserSettings.ini. (Engine.ini's path, display repaired)
pub fn apply_game_config(options: &ApplyConfig<'_>) -> std::io::Result<(PathBuf, bool)> {
    let dir = &options.config_dir;
    std::fs::create_dir_all(dir)?;
    let engine = dir.join("Engine.ini");
    let existing = read_lines(&engine)?.unwrap_or_default();
    let lines = rewrite_engine_ini(
        &existing,
        options.host,
        options.graphics,
        options.xmpp_port,
        options.exposure,
    )
    .ok_or_else(|| std::io::Error::new(std::io::ErrorKind::InvalidInput, "invalid host or port"))?;
    write_atomically(&engine, &encode_ini(&lines))?;

    let settings = dir.join("GameUserSettings.ini");
    let current = read_lines(&settings)?;
    let mut repaired = false;
    if current.is_some() || options.safe_window {
        let (lines, changed) = repair_display_settings(current.as_deref().unwrap_or_default(), options.safe_window);
        repaired = changed;
        if repaired && current.is_some() {
            let stamp = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map_or(0, |since| since.as_millis());
            let mut backup = settings.as_os_str().to_owned();
            backup.push(format!(".before-display-repair-{stamp}"));
            std::fs::copy(&settings, backup)?;
        }
        let forced = options.graphics.value() >= 0;
        let next = if forced {
            rewrite_game_user_settings(&lines, options.graphics)
        } else {
            lines
        };
        if repaired || forced {
            write_atomically(&settings, &encode_ini(&next))?;
        }
    }
    Ok((engine, repaired))
}
