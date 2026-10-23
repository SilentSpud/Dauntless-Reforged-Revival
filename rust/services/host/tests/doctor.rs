//! `doctor` (MIGRATION_PLAN.md RH3): the Node kit's tests for its checks (`performance.test.ts`) and
//! how it prints them.

#![allow(clippy::unwrap_used, clippy::expect_used)] // test helpers

use serde_json::json;

use host::doctor::{
    Check, CpuPatches, Level, count_function_files, game_checks, hunts_that_fit, performance_checks, port_in_use,
    print_checks, runtime_cpu_patches,
};
use host::hostconfig::{DEFAULT_MONGO_URI, DEFAULT_PERFORMANCE, HostConfig, PerformanceOptions, Platform};
use host::paths::game_exe_relative;

fn public(max_hunts: u32, performance: Option<PerformanceOptions>) -> HostConfig {
    let mut c = HostConfig::from_json(&json!({
        "version": 1, "mode": "public", "host": "203.0.113.7", "gameAddress": "203.0.113.7", "name": "Freeza Hunts",
        "gameDir": "/srv/dauntless/Dauntless", "registration": "INVITECODE", "shareUrl": null,
        "mongo": { "uri": DEFAULT_MONGO_URI, "manage": true, "mongod": "/usr/bin/mongod" }, "maxHuntServers": max_hunts
    }))
    .unwrap();
    c.performance = performance;
    c
}

#[test]
fn tells_whether_the_published_runtime_has_the_game_servers_cpu_patches() {
    let dir = tempfile::tempdir().unwrap();
    let both = dir.path().join("both.dll");
    let mut bytes = b"MZ junk ".to_vec();
    bytes.extend(
        "LEGACY_EXPECTED_PLAYER_WALK.flag"
            .encode_utf16()
            .flat_map(u16::to_le_bytes),
    );
    bytes.extend_from_slice(b" REFORGED_SERVER_MAX_FPS\0");
    std::fs::write(&both, bytes).unwrap();
    let old = dir.path().join("old.dll");
    std::fs::write(&old, "MZ an older runtime").unwrap();
    let yes = CpuPatches {
        cpu_fixes: true,
        frame_cap: true,
    };
    assert_eq!(runtime_cpu_patches(&both), Some(yes));
    let no = CpuPatches {
        cpu_fixes: false,
        frame_cap: false,
    };
    assert_eq!(runtime_cpu_patches(&old), Some(no));
    assert_eq!(runtime_cpu_patches(&dir.path().join("none.dll")), None);
    let c = public(3, None);
    let warn = performance_checks(&c, Some(&old), Some(16384), Platform::Windows);
    assert_eq!(warn[0].level, Level::Info);
    assert_eq!(warn[1].level, Level::Warn);
    assert!(warn[1].text.contains("0019 (CPU fixes) and 0020 (frame cap)"));
    assert_eq!(
        performance_checks(&c, Some(&both), Some(16384), Platform::Windows)[1].level,
        Level::Ok
    );
    assert_eq!(
        performance_checks(&c, None, Some(16384), Platform::Windows).len(),
        1,
        "nothing published: only the settings"
    );
    assert_eq!(
        warn[0].text,
        "game servers at 30 fps, 10 fps after 15 s without players; Training Grounds on demand, stopped after 5 min without players; quiet game logs; at most 3 hunts (UDP 8770-8777); a new hunt only with 2560 MB free"
    );
}

#[test]
fn says_how_many_hunts_fit_in_this_machines_memory() {
    assert_eq!(
        hunts_that_fit(16 * 1024, &DEFAULT_PERFORMANCE),
        7,
        "16 GB: 7 hunts of 1.5 GB next to the hubs, the services and 1 GB free"
    );
    assert_eq!(
        hunts_that_fit(15 * 1024, &DEFAULT_PERFORMANCE),
        6,
        "a \"16 GB\" VPS often reports less"
    );
    assert_eq!(hunts_that_fit(2 * 1024, &DEFAULT_PERFORMANCE), 0);
    let eight = public(8, None);
    let guarded = performance_checks(&eight, None, Some(16 * 1024), Platform::Linux);
    assert_eq!(guarded[1].level, Level::Info);
    assert!(
        guarded[1].text.contains(
            "8 hunts at once may need more than this machine's 16.0 GB: about 7 fit; the memory guard refuses"
        )
    );
    let off = PerformanceOptions {
        memory_guard: false,
        ..DEFAULT_PERFORMANCE
    };
    let unguarded = performance_checks(&public(8, Some(off)), None, Some(16 * 1024), Platform::Linux);
    assert_eq!(unguarded[1].level, Level::Warn);
    assert_eq!(
        unguarded[1].fix.as_deref(),
        Some("reforged-host setup --memory-guard, or --max-hunts 7")
    );
    assert_eq!(
        performance_checks(&eight, None, Some(32 * 1024), Platform::Linux).len(),
        1,
        "32 GB: they fit, nothing to say"
    );
}

#[test]
fn checks_the_game_folder_its_version_and_its_executable() {
    assert_eq!(game_checks(None, true).unwrap()[0].level, Level::Fail);
    let dir = tempfile::tempdir().unwrap();
    let game = dir.path().to_string_lossy().into_owned();
    assert!(
        game_checks(Some(&game), true).unwrap()[0]
            .text
            .starts_with("no game executable at ")
    );
    let exe = dir.path().join(game_exe_relative());
    std::fs::create_dir_all(exe.parent().unwrap()).unwrap();
    std::fs::write(&exe, "not really a game").unwrap();
    std::fs::write(
        dir.path().join("Version.txt"),
        "\u{feff}dauntless_rel-1.14.7_Shipping_2024-04-01_14-59-21_647472\r\n",
    )
    .unwrap();
    let checks = game_checks(Some(&game), true).unwrap();
    assert_eq!(checks[0].level, Level::Ok, "{checks:?}");
    assert_eq!(checks[1].level, Level::Fail);
    assert!(
        checks[1].text.starts_with("the executable is another build (SHA-256 "),
        "{checks:?}"
    );
    assert_eq!(game_checks(Some(&game), false).unwrap().len(), 1, "--no-hash");
    std::fs::write(dir.path().join("Version.txt"), "").unwrap();
    assert_eq!(
        game_checks(Some(&game), false).unwrap()[0].text,
        "Version.txt says \"nothing\", not dauntless_rel-1.14.7_Shipping_2024-04-01_14-59-21_647472"
    );
}

#[test]
fn counts_the_sdks_function_files() {
    let dir = tempfile::tempdir().unwrap();
    assert_eq!(count_function_files(&dir.path().join("none")), 0);
    for f in ["A_functions.cpp", "B_functions.cpp", "A_classes.hpp"] {
        std::fs::write(dir.path().join(f), "").unwrap();
    }
    assert_eq!(count_function_files(dir.path()), 2);
}

#[test]
fn prints_checks_by_area_with_their_fix_unless_ok() {
    let c = |area, level, text: &str, fix: Option<&str>| Check {
        area,
        level,
        text: text.into(),
        fix: fix.map(str::to_owned),
    };
    let checks = [
        c("tools", Level::Ok, "git version 2.50", Some("never shown")),
        c("tools", Level::Fail, "Node.js is not installed", Some("install it")),
        c("game data", Level::Warn, "backend: example data", None),
        c("running", Level::Info, "backend is not running (127.0.0.1:61000)", None),
    ];
    let mut lines = Vec::new();
    let (fails, warns) = print_checks(&checks, &mut |l| lines.push(l.to_owned()));
    assert_eq!((fails, warns), (1, 1));
    assert_eq!(
        lines,
        [
            "\ntools",
            "  [ ok ] git version 2.50",
            "  [FAIL] Node.js is not installed",
            "         -> install it",
            "\ngame data",
            "  [warn] backend: example data",
            "\nrunning",
            "  [info] backend is not running (127.0.0.1:61000)",
        ]
    );
}

#[test]
fn a_port_is_in_use_when_something_listens() {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    assert!(port_in_use(port, None).unwrap());
    drop(listener);
    assert!(!port_in_use(port, Some("127.0.0.1")).unwrap());
}
