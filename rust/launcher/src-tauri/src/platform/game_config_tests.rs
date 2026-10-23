//! `test/cases/game-config.json`, made from the TypeScript (GameUserSettings.ini, the encodings, the display
//! repair and the game folder rules on both platforms), and Reforged's `launch.test.ts` (Engine.ini and
//! Game.ini) and `display-repair.test.ts` cases, with the ini files in temporary folders.

use serde_json::Value;

use super::game_config::*;
use super::game_folder::{clean_game_folder_input, game_root_candidates};
use super::settings::Platform;
use crate::bridge::types::GraphicsPreset;

fn cases() -> Value {
    let path = concat!(env!("CARGO_MANIFEST_DIR"), "/../test/cases/game-config.json");
    serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap()
}

fn strings(value: &Value) -> Vec<String> {
    value
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v.as_str().unwrap().to_owned())
        .collect()
}

fn graphics(value: &Value) -> GraphicsPreset {
    GraphicsPreset::try_from(value.as_i64().unwrap() as i8).unwrap()
}

fn lines(list: &[&str]) -> Vec<String> {
    list.iter().map(|line| (*line).to_owned()).collect()
}

fn preset(level: i8) -> GraphicsPreset {
    GraphicsPreset::try_from(level).unwrap()
}

fn bytes(value: &Value) -> Vec<u8> {
    value
        .as_array()
        .unwrap()
        .iter()
        .map(|b| b.as_u64().unwrap() as u8)
        .collect()
}

#[test]
fn game_user_settings_and_encodings() {
    let cases = cases();
    for case in cases["gus"].as_array().unwrap() {
        assert_eq!(
            rewrite_game_user_settings(&strings(&case["lines"]), graphics(&case["graphics"])),
            strings(&case["out"]),
            "{case}"
        );
    }
    for case in cases["split"].as_array().unwrap() {
        assert_eq!(
            split_lines(case["text"].as_str().unwrap()),
            strings(&case["lines"]),
            "{case}"
        );
    }
    for case in cases["encode"].as_array().unwrap() {
        assert_eq!(encode_ini(&strings(&case["lines"])), bytes(&case["bytes"]), "{case}");
    }
    for case in cases["decode"].as_array().unwrap() {
        assert_eq!(
            decode_ini(&bytes(&case["bytes"])),
            case["text"].as_str().unwrap(),
            "{case}"
        );
    }
}

#[test]
fn display_repair() {
    for case in cases()["display"].as_array().unwrap() {
        let (lines, repaired) = repair_display_settings(&strings(&case["lines"]), case["force"].as_bool().unwrap());
        assert_eq!(
            (lines, repaired),
            (
                strings(&case["out"]["lines"]),
                case["out"]["repaired"].as_bool().unwrap()
            ),
            "{case}"
        );
    }
}

#[test]
fn game_folder_rules_on_both_platforms() {
    for case in cases()["folders"].as_array().unwrap() {
        let platform = if case["platform"] == "win32" {
            Platform::Windows
        } else {
            Platform::Linux
        };
        let clean = clean_game_folder_input(case["input"].as_str().unwrap(), platform);
        assert_eq!(clean.as_deref(), case["clean"].as_str(), "{case}");
        if let Some(clean) = clean {
            assert_eq!(
                game_root_candidates(&clean, platform),
                strings(&case["candidates"]),
                "{case}"
            );
        }
    }
}

/// `launch.test.ts`: "Engine.ini: chat goes to the relay, the owned sections are replaced and everything
/// else is kept"
#[test]
fn engine_ini_points_chat_at_the_relay_and_keeps_the_rest() {
    let existing = lines(&[
        "[Core.Log]",
        "LogOnline=Verbose",
        "",
        "[OnlineSubsystemMcp.XMPP]",
        "ServerAddr=\"wss://xmpp-service-prod.ol.epicgames.com\"",
        "ServerPort=443",
        "bUseSSL=true",
        "",
        "[SystemSettings]",
        "sg.ShadowQuality=0",
        "[/Script/Engine.Engine]",
        "bSmoothFrameRate=true",
    ]);
    let out = rewrite_engine_ini(&existing, "127.0.0.1", 61100, GraphicsPreset::GAME_MENU).unwrap();
    assert_eq!(
        out[..4],
        lines(&[
            "[OnlineSubsystemMcp.XMPP]",
            "ServerAddr=\"ws://127.0.0.1\"",
            "ServerPort=61100",
            "bUseSSL=false"
        ])
    );
    assert!(!out.iter().any(|line| line.contains("epicgames")));
    assert!(
        !out.iter().any(|line| line == "[SystemSettings]"),
        "graphics left to the game"
    );
    assert!(out.iter().any(|line| line == "LogOnline=Verbose"));
    assert!(out.iter().any(|line| line == "bSmoothFrameRate=true"));
    let forced = rewrite_engine_ini(&existing, "127.0.0.1", 61100, preset(2)).unwrap();
    assert_eq!(forced[0], "[SystemSettings]");
    assert!(forced.iter().any(|line| line == "sg.ShadowQuality=2"));
    assert_eq!(xmpp_lines("bad host!", 1), None);
    assert_eq!(xmpp_lines("127.0.0.1", 0), None);
    assert_eq!(
        rewrite_game_user_settings(&lines(&["sg.TextureQuality=0", "Other=1"]), preset(3)),
        lines(&["sg.TextureQuality=3", "Other=1"])
    );
}

#[test]
fn system_settings_hold_the_forced_level_only() {
    assert!(system_settings_lines(GraphicsPreset::GAME_MENU).is_empty());
    assert_eq!(
        system_settings_lines(preset(4)),
        lines(&[
            "[SystemSettings]",
            "sg.ViewDistanceQuality=4",
            "sg.AntiAliasingQuality=4",
            "sg.ShadowQuality=4",
            "sg.PostProcessQuality=4",
            "sg.TextureQuality=4",
            "sg.EffectsQuality=4",
            "sg.FoliageQuality=4",
            "sg.ShadingQuality=4",
            "sg.ResolutionQuality=100",
            "r.ScreenPercentage=100",
        ])
    );
    // A file with only owned sections on top: no blank lines are left above what is kept
    let out = rewrite_engine_ini(
        &lines(&["[SystemSettings]", "x=1", "", "", "[Kept]", "y=2"]),
        "127.0.0.1",
        61100,
        GraphicsPreset::GAME_MENU,
    )
    .unwrap();
    assert_eq!(
        out,
        lines(&[
            "[OnlineSubsystemMcp.XMPP]",
            "ServerAddr=\"ws://127.0.0.1\"",
            "ServerPort=61100",
            "bUseSSL=false",
            "",
            "[Kept]",
            "y=2",
        ])
    );
}

fn config(dir: &std::path::Path) -> ApplyConfig<'static> {
    ApplyConfig {
        config_dir: dir.to_path_buf(),
        ..ApplyConfig::new("127.0.0.1", 61100, GraphicsPreset::GAME_MENU)
    }
}

fn read(path: &std::path::Path) -> String {
    decode_ini(&std::fs::read(path).unwrap())
}

/// `launch.test.ts`: "applyGameConfig writes Engine.ini and keeps a UTF-16 file readable"
#[test]
fn apply_keeps_a_utf16_file_readable() {
    let dir = tempfile::tempdir().unwrap();
    let mut data = vec![0xff, 0xfe];
    data.extend("[Custom]\r\nName=Jörð\r\n".encode_utf16().flat_map(u16::to_le_bytes));
    std::fs::write(dir.path().join("Engine.ini"), data).unwrap();
    let applied = apply_game_config(&config(dir.path())).unwrap();
    let text = read(&applied.engine_ini);
    assert!(text.contains("ServerPort=61100"));
    assert!(text.contains("Name=Jörð"));
    assert_eq!(applied.phoenix_endpoints_removed, 0);
}

/// What Dauntless Revived's launcher leaves in Game.ini (local-server mode), between player sections.
const REVIVED_GAME_INI: [&str; 11] = [
    "[/Script/Archon.ArchonGameSettings]",
    "bShowHints=False",
    "",
    "[OnlineSubsystemPhoenix]",
    "AuthEndpoint=\"http://127.0.0.1:61000/auth\"",
    "CheckCrossPlayProgressionEndpoint=\"http://127.0.0.1:61000/features/platform/{platform}\"",
    "; a comment",
    "PhoenixStatusMessageEndpoint=\"http://127.0.0.1:61000/dauntless-status\"",
    "",
    "[Custom]",
    "Name=Kept",
];

/// `launch.test.ts`: "removeIniSection drops one section (any case), counts its keys and keeps the rest"
#[test]
fn remove_ini_section_drops_one_section() {
    let (out, removed) = remove_ini_section(&lines(&REVIVED_GAME_INI), "onlinesubsystemphoenix");
    assert_eq!(removed, 3);
    assert_eq!(
        out,
        lines(&[
            "[/Script/Archon.ArchonGameSettings]",
            "bShowHints=False",
            "",
            "[Custom]",
            "Name=Kept"
        ])
    );
    assert_eq!(
        remove_ini_section(&lines(&["[A]", "x=1"]), PHOENIX_ENDPOINTS_SECTION),
        (lines(&["[A]", "x=1"]), 0)
    );
    assert_eq!(
        remove_ini_section(
            &lines(&["  [ OnlineSubsystemPhoenix ]  ", "#x=1", "  y = 2", "[B]"]),
            PHOENIX_ENDPOINTS_SECTION
        ),
        (lines(&["[B]"]), 1)
    );
}

/// `launch.test.ts`: "applyGameConfig (1.14.7): Revived's 127.0.0.1 endpoints leave Game.ini, with a copy;
/// nothing else changes"
#[test]
fn apply_removes_revived_endpoints_from_game_ini() {
    let dir = tempfile::tempdir().unwrap();
    let game_ini = dir.path().join("Game.ini");
    let original: String = REVIVED_GAME_INI.iter().map(|line| format!("{line}\r\n")).collect();
    std::fs::write(&game_ini, &original).unwrap();
    let kept = apply_game_config(&ApplyConfig {
        clear_phoenix_endpoints: false,
        ..config(dir.path())
    })
    .unwrap();
    assert_eq!(kept.phoenix_endpoints_removed, 0);
    assert_eq!(read(&game_ini), original, "untouched when not asked");

    let applied = apply_game_config(&config(dir.path())).unwrap();
    assert_eq!(applied.phoenix_endpoints_removed, 3);
    let text = read(&game_ini);
    assert!(!text.contains("127.0.0.1"), "no endpoint left");
    assert!(text.contains("bShowHints=False") && text.contains("Name=Kept"));
    assert!(read(&dir.path().join("Game.ini.before-reforged")).contains("CheckCrossPlayProgressionEndpoint"));

    // Already clean: not written again, and no Game.ini is made where there is none
    let again = apply_game_config(&config(dir.path())).unwrap();
    assert_eq!(again.phoenix_endpoints_removed, 0);
    let empty = tempfile::tempdir().unwrap();
    apply_game_config(&config(empty.path())).unwrap();
    assert!(!empty.path().join("Game.ini").exists());
    assert!(!dir.path().join("Engine.ini.rr-tmp").exists());
}

#[test]
fn apply_writes_both_files_and_keeps_a_backup_of_a_repaired_one() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(
        dir.path().join("Engine.ini"),
        "[Core.System]\r\nPaths=x\r\n[SystemSettings]\r\nold=1\r\n",
    )
    .unwrap();
    std::fs::write(
        dir.path().join("GameUserSettings.ini"),
        "[/Script/Engine.GameUserSettings]\r\nResolutionSizeX=40000\r\nsg.ShadowQuality=1\r\n",
    )
    .unwrap();
    let options = ApplyConfig {
        graphics: preset(2),
        ..config(dir.path())
    };
    let applied = apply_game_config(&options).unwrap();
    assert!(applied.display_repaired);
    let text = std::fs::read_to_string(applied.engine_ini).unwrap();
    assert!(
        text.starts_with("[SystemSettings]\r\n")
            && text.contains("ServerAddr=\"ws://127.0.0.1\"\r\nServerPort=61100\r\n")
            && text.ends_with("[Core.System]\r\nPaths=x\r\n"),
        "{text}"
    );
    assert!(!text.contains("old=1"));
    let settings = std::fs::read_to_string(dir.path().join("GameUserSettings.ini")).unwrap();
    assert!(
        settings.contains("ResolutionSizeX=1280") && settings.contains("sg.ShadowQuality=2"),
        "{settings}"
    );
    let backups = std::fs::read_dir(dir.path())
        .unwrap()
        .filter(|e| {
            e.as_ref()
                .unwrap()
                .file_name()
                .to_string_lossy()
                .contains("before-display-repair")
        })
        .count();
    assert_eq!(backups, 1);
}

/// `display-repair.test.ts`: "FuturePoet oversized viewport recovers to windowed 720p and preserves
/// unrelated preferences"
#[test]
fn an_oversized_viewport_recovers_to_a_720p_window() {
    let before = lines(&[
        "[/Script/Archon.ArchonGameUserSettings]",
        "ResolutionSizeX=33424",
        "ResolutionSizeY=828",
        "FullscreenMode=1",
        "Volume=0.7",
        "[Other]",
        "ResolutionSizeX=99999",
    ]);
    let (fixed, repaired) = repair_display_settings(&before, false);
    assert!(repaired);
    for line in [
        "ResolutionSizeX=1280",
        "FullscreenMode=2",
        "Volume=0.7",
        "ResolutionSizeX=99999",
    ] {
        assert!(fixed.iter().any(|l| l == line), "{line}");
    }
    assert!(!repair_display_settings(&fixed, false).1);
}

/// `display-repair.test.ts`: "valid widescreen preferences are unchanged"
#[test]
fn valid_widescreen_preferences_are_unchanged() {
    let before = lines(&[
        "[/Script/Engine.GameUserSettings]",
        "ResolutionSizeX=5120",
        "ResolutionSizeY=1440",
    ]);
    assert_eq!(repair_display_settings(&before, false), (before, false));
}

/// `display-repair.test.ts`: "Poet fix overrides valid or missing saved settings and stays idempotent"
#[test]
fn the_safe_window_overrides_any_saved_size_once() {
    for before in [
        Vec::new(),
        lines(&[
            "[/Script/Archon.ArchonGameUserSettings]",
            "ResolutionSizeX=1920",
            "ResolutionSizeY=1080",
            "FullscreenMode=1",
            "WindowPosX=33423",
            "Volume=0.7",
        ]),
    ] {
        let (fixed, repaired) = repair_display_settings(&before, true);
        assert!(repaired);
        for line in ["ResolutionSizeX=1280", "FullscreenMode=2", "WindowPosX=-1"] {
            assert!(fixed.iter().any(|l| l == line), "{line}");
        }
        if before.iter().any(|l| l == "Volume=0.7") {
            assert!(fixed.iter().any(|l| l == "Volume=0.7"));
        }
        assert_eq!(repair_display_settings(&fixed, true), (fixed.clone(), false));
    }
}
