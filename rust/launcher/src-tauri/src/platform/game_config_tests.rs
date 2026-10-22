//! `test/cases/game-config.json`, made from the TypeScript: Engine.ini, GameUserSettings.ini, the display
//! repair and the game folder rules on both platforms (DR's client mods are not brought in).

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

fn exposure(value: &Value) -> ExposureMode {
    if value == "basic" {
        ExposureMode::Basic
    } else {
        ExposureMode::Game
    }
}

#[test]
fn engine_ini_and_game_user_settings() {
    let cases = cases();
    for case in cases["systemSettings"].as_array().unwrap() {
        assert_eq!(
            system_settings_lines(graphics(&case["graphics"]), exposure(&case["exposure"])),
            strings(&case["lines"]),
            "{case}"
        );
    }
    for case in cases["engine"].as_array().unwrap() {
        let out = rewrite_engine_ini(
            &strings(&case["existing"]),
            case["host"].as_str().unwrap(),
            graphics(&case["graphics"]),
            case["port"].as_u64().unwrap() as u16,
            exposure(&case["exposure"]),
        );
        assert_eq!(out.unwrap(), strings(&case["out"]), "{case}");
    }
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
        let bytes: Vec<u8> = case["bytes"]
            .as_array()
            .unwrap()
            .iter()
            .map(|b| b.as_u64().unwrap() as u8)
            .collect();
        assert_eq!(encode_ini(&strings(&case["lines"])), bytes, "{case}");
    }
    for case in cases["decode"].as_array().unwrap() {
        let bytes: Vec<u8> = case["bytes"]
            .as_array()
            .unwrap()
            .iter()
            .map(|b| b.as_u64().unwrap() as u8)
            .collect();
        assert_eq!(decode_ini(&bytes), case["text"].as_str().unwrap(), "{case}");
    }
    assert_eq!(xmpp_lines("bad host", 61099), None);
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
        config_dir: dir.path().to_path_buf(),
        ..ApplyConfig::new("100.64.0.7", GraphicsPreset::try_from(2).unwrap())
    };
    let (engine, repaired) = apply_game_config(&options).unwrap();
    assert!(repaired);
    let text = std::fs::read_to_string(engine).unwrap();
    assert!(
        text.starts_with("[SystemSettings]\r\n")
            && text.contains("ServerAddr=\"ws://100.64.0.7\"\r\nServerPort=61099\r\n")
            && text.ends_with("[Core.System]\r\nPaths=x\r\n")
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
