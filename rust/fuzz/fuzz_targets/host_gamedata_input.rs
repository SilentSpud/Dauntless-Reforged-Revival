//! The host kit's game-data input (MIGRATION_PLAN.md RH4, RX3; docs/security/host-gamedata.md): a game
//! data pack (gzip-compressed JSON a host imports, perhaps made elsewhere), the CatalogExporter
//! progression export (JSON lines read from the game) and the store-capture pin. None of it may
//! panic; a pack the checks accept names only known game-data files, each matching its hash; a pin
//! the checks accept names only plain relative paths.
#![no_main]

use std::io::Write;
use std::path::Path;

use host::captures::parse_capture_pin;
use host::gamepack::{pack_entries, parse_game_data_pack, sha256_hex};
use host::paths::Paths;
use host::progression::{ACTIVE_HUNT_PASS, build_from_rows, parse_jsonl};
use libfuzzer_sys::fuzz_target;

fn gzip(data: &[u8]) -> Vec<u8> {
    let mut gz = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::fast());
    let _ = gz.write_all(data);
    gz.finish().unwrap_or_default()
}

fuzz_target!(|data: &[u8]| {
    let Some((&selector, rest)) = data.split_first() else {
        return;
    };
    let p = Paths::new("rr");
    match selector % 4 {
        // The pack's bytes as they are, and its JSON compressed (to reach the checks).
        0 | 1 => {
            let bytes = if selector % 4 == 0 { rest.to_vec() } else { gzip(rest) };
            if let Ok(pack) = parse_game_data_pack(&p, Path::new("pack.json.gz"), &bytes) {
                let entries = pack_entries(&p);
                for f in &pack.files {
                    assert!(entries.iter().any(|(name, _)| *name == f.path), "{}", f.path);
                    assert_eq!(sha256_hex(&f.data), f.sha256);
                }
            }
        }
        // The progression export: the rows split over the track files and the season table.
        2 => {
            let text = String::from_utf8_lossy(rest);
            let rows = parse_jsonl(&text);
            let (seasons, tracks) = rows.split_at(rows.len() / 5);
            let tracks: Vec<_> = tracks
                .chunks(tracks.len().div_ceil(4).max(1))
                .map(<[_]>::to_vec)
                .collect();
            if let Ok(built) = build_from_rows(&tracks, seasons, ACTIVE_HUNT_PASS) {
                assert!(built.tracks.iter().any(|t| t == "ExperienceTrack_PlayerLevel"));
                assert!(built.tracks.iter().any(|t| t == ACTIVE_HUNT_PASS));
            }
        }
        _ => {
            let text = String::from_utf8_lossy(rest);
            if let Ok(pin) = parse_capture_pin(Path::new("store-captures.json"), &text) {
                for (name, hash) in &pin.files {
                    assert!(!name.contains("..") && !Path::new(name).is_absolute(), "{name}");
                    let plain = Path::new(name)
                        .components()
                        .all(|c| matches!(c, std::path::Component::Normal(_)));
                    assert!(plain, "{name}");
                    assert_eq!(hash.len(), 64);
                }
            }
        }
    }
});
