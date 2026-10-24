//! The Node kit's tests of host.json, the secrets and the service env files
//! (`legacy/ReforgedHost/test/config.test.ts`), with the host.json and env-file cases of
//! `ports.test.ts`, `performance.test.ts` and `linux.test.ts`. The cases of those files that test
//! setup's option readers, invites, the firewall scripts, doctor and MongoDB belong to the rows that
//! port that code (RH3, RH5).

#![allow(clippy::unwrap_used, clippy::expect_used)] // test helpers

use std::path::Path;

use host::envfile::parse_env;
use host::hostconfig::{
    DEFAULT_MONGO_URI, DEFAULT_PERFORMANCE, EnvEntries, HostConfig, Platform, QUIET_GAME_LOG_CMDS, Secrets,
    build_env_files, game_ports, gateway_port, load_or_create_secrets, read_host_config, validate, write_env_files,
    write_host_config,
};
use host::paths::{Paths, game_exe_relative};
use serde_json::{Value, json};

fn private() -> Value {
    json!({
        "version": 1,
        "mode": "private",
        "host": "100.101.102.103",
        "gameAddress": "100.101.102.103",
        "name": "Friday Hunts",
        "gameDir": "D:\\Games\\Dauntless",
        "registration": "INVITECODE",
        "shareUrl": null,
        "mongo": { "uri": DEFAULT_MONGO_URI, "manage": true, "mongod": null },
        "maxHuntServers": 3,
    })
}

fn public() -> Value {
    with(
        private(),
        json!({ "mode": "public", "host": "hunts.example.net", "gameAddress": "203.0.113.7" }),
    )
}

/// `{ ...base, ...changes }`
fn with(mut base: Value, changes: Value) -> Value {
    if let (Some(b), Some(c)) = (base.as_object_mut(), changes.as_object()) {
        for (k, v) in c {
            b.insert(k.clone(), v.clone());
        }
    }
    base
}

fn config(value: Value) -> HostConfig {
    HostConfig::from_json(&value).unwrap()
}

fn error(value: Value) -> String {
    validate(&value).unwrap_err().to_string()
}

fn get<'a>(entries: &'a EnvEntries, key: &str) -> Option<&'a str> {
    entries.iter().find(|(k, _)| k == key).map(|(_, v)| v.as_str())
}

fn default_performance() -> Value {
    let d = DEFAULT_PERFORMANCE;
    json!({
        "serverFps": d.server_fps, "idleFps": d.idle_fps, "trainingOnDemand": d.training_on_demand,
        "trainingIdleMinutes": d.training_idle_minutes, "verboseGameLogs": d.verbose_game_logs,
        "memoryGuard": d.memory_guard, "minFreeMemoryMb": d.min_free_memory_mb, "huntMemoryMb": d.hunt_memory_mb,
    })
}

fn fast_secrets() -> Secrets {
    use std::sync::atomic::{AtomicU32, Ordering};
    static N: AtomicU32 = AtomicU32::new(0);
    let n = N.fetch_add(1, Ordering::Relaxed);
    let hex = |i: u32| format!("{:064x}", u64::from(n) * 1000 + u64::from(i));
    Secrets {
        gateway_secret: hex(1),
        allowlist_secret: hex(2),
        admin_key: hex(3),
        gameserver_key: hex(4),
        api_key_hash_secret: hex(5),
        metrics_token: hex(6),
        jwt_private_key_b64: hex(7),
        jwt_public_key_b64: hex(8),
    }
}

// ------------------------------------------------------------------ host.json

#[test]
fn accepts_a_private_server_on_a_tailscale_address_and_a_public_one_on_a_public_address_or_name() {
    assert_eq!(validate(&private()).unwrap()["host"], "100.101.102.103");
    assert_eq!(
        validate(&with(private(), json!({ "host": "hostpc.tail1234.ts.net" }))).unwrap()["host"],
        "hostpc.tail1234.ts.net"
    );
    assert_eq!(validate(&public()).unwrap()["host"], "hunts.example.net");
    assert_eq!(
        validate(&with(public(), json!({ "host": "203.0.113.7" }))).unwrap()["mode"],
        "public"
    );
}

#[test]
fn refuses_addresses_that_cannot_work_for_the_mode_and_other_bad_values() {
    assert!(error(with(private(), json!({ "host": "192.168.1.20" }))).contains("Tailscale"));
    assert!(error(with(private(), json!({ "gameAddress": "203.0.113.7" }))).contains("Tailscale"));
    assert!(error(with(public(), json!({ "host": "192.168.1.20" }))).contains("public"));
    assert!(error(with(public(), json!({ "host": "100.101.102.103" }))).contains("public"));
    assert!(error(with(public(), json!({ "gameAddress": "hunts.example.net" }))).contains("IPv4"));
    assert!(error(with(private(), json!({ "name": "  " }))).contains("name"));
    assert!(error(with(private(), json!({ "maxHuntServers": 11 }))).contains("maxHuntServers"));
    assert!(error(with(private(), json!({ "maxHuntServers": 0 }))).contains("maxHuntServers"));
    assert_eq!(
        validate(&with(private(), json!({ "maxHuntServers": 10 }))).unwrap()["maxHuntServers"],
        10
    );
    assert!(error(with(private(), json!({ "shareUrl": "https://evil.example/" }))).contains("shareUrl"));
    assert!(
        error(with(
            private(),
            json!({ "mongo": { "uri": "http://x", "manage": true, "mongod": null } })
        ))
        .contains("mongo")
    );
}

#[test]
fn is_written_and_read_back() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("host.json");
    let mut c = config(private());
    c.name = "  Friday\u{7} Hunts ".into();
    write_host_config(&file, &c).unwrap();
    assert_eq!(read_host_config(&file).unwrap().unwrap().name, "Friday Hunts");
    assert!(read_host_config(&dir.path().join("none.json")).unwrap().is_none());
}

// ------------------------------------------------------------------ host.toml

/// A config written to host.toml and read back.
fn through_toml(c: &HostConfig) -> (HostConfig, String) {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("host.toml");
    write_host_config(&file, c).unwrap();
    let text = std::fs::read_to_string(&file).unwrap();
    (read_host_config(&file).unwrap().unwrap(), text)
}

#[test]
fn host_toml_holds_every_field_of_host_json() {
    let full = with(
        public(),
        json!({
            "publicPort": 4443,
            "shareUrl": null,
            "mongo": { "uri": DEFAULT_MONGO_URI, "manage": true, "mongod": "/usr/bin/mongod" },
            "performance": {
                "serverFps": 0, "idleFps": 1, "trainingOnDemand": false, "trainingIdleMinutes": 0,
                "verboseGameLogs": true, "memoryGuard": false, "minFreeMemoryMb": 0, "huntMemoryMb": 256,
            },
            "linux": { "wine": "/usr/bin/wine", "proton": null, "xvfb": false, "firewallInterface": "eth0" },
        }),
    );
    let tests = [
        config(full),
        config(with(
            private(),
            json!({ "multipliers": { "xp": 5, "rewards": 2.5 }, "gameDir": null }),
        )),
    ];
    for c in tests {
        let (back, text) = through_toml(&c);
        assert_eq!(
            (
                back.mode,
                &back.host,
                &back.game_address,
                &back.name,
                &back.game_dir,
                back.registration
            ),
            (c.mode, &c.host, &c.game_address, &c.name, &c.game_dir, c.registration),
            "{text}"
        );
        assert_eq!(
            (&back.share_url, &back.mongo, back.max_hunt_servers),
            (&c.share_url, &c.mongo, c.max_hunt_servers)
        );
        assert_eq!(
            (back.multipliers, back.public_port),
            (c.multipliers, c.public_port),
            "{text}"
        );
        assert_eq!((back.performance, &back.linux), (c.performance, &c.linux), "{text}");
        assert!(
            !text.contains("null") && !text.contains("Fps"),
            "no null, snake_case keys: {text}"
        );
    }
}

#[test]
fn host_toml_errors_name_its_own_keys() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("host.toml");
    let (_, text) = through_toml(&config(private()));
    let error = |text: &str| {
        std::fs::write(&file, text).unwrap();
        read_host_config(&file).unwrap_err().to_string()
    };
    let e = error(&text.replace("max_hunt_servers = 3", "max_hunt_servers = 99"));
    assert!(e.contains("max_hunt_servers must be 1 to 10"), "{e}");
    let e = error(&format!("{text}\n[performance]\nserver_fps = 5\n"));
    assert!(
        e.contains("performance.server_fps must be 0 (no cap) or 10 to 240"),
        "{e}"
    );
    let e = error(&text.replace("game_address", "gameAddress"));
    assert!(e.contains("did you mean game_address?"), "{e}");
    assert!(e.contains("host.toml"), "names the file: {e}");
}

// ------------------------------------------------------------------ secrets

#[test]
fn secrets_are_made_once_and_kept_and_a_missing_one_is_added_without_touching_the_others() {
    let dir = tempfile::tempdir().unwrap();
    let mut made = 0;
    let mut make = || {
        made += 1;
        Ok(fast_secrets())
    };
    let (first, created) = load_or_create_secrets(dir.path(), &mut make).unwrap();
    assert!(created);
    let (again, created) = load_or_create_secrets(dir.path(), &mut make).unwrap();
    assert!(!created);
    assert_eq!(again, first);
    let file = dir.path().join("secrets.env");
    let text = std::fs::read_to_string(&file).unwrap();
    let without: String = text
        .split_inclusive('\n')
        .map(|line| {
            if line.starts_with("REFORGED_ADMIN_KEY=") {
                "\n"
            } else {
                line
            }
        })
        .collect();
    std::fs::write(&file, without).unwrap();
    let (third, created) = load_or_create_secrets(dir.path(), &mut make).unwrap();
    assert!(created);
    assert_ne!(third.admin_key, first.admin_key);
    assert_eq!(third.gateway_secret, first.gateway_secret);
    assert_eq!(third.jwt_private_key_b64, first.jwt_private_key_b64);
    assert_eq!(made, 2);
}

// ------------------------------------------------------------------ the service env files

fn win_paths() -> Paths {
    Paths::new("C:\\rr")
}

#[test]
fn private_mode_the_backend_behind_the_gateway_on_the_tailscale_address_in_plain_http_no_allowlist() {
    let s = fast_secrets();
    let p = win_paths();
    let f = build_env_files(&config(private()), &s, &p, "abc123", Platform::Windows);
    assert_eq!(get(&f.backend, "AUTH_MODE"), Some("APIKEY"));
    assert_eq!(get(&f.backend, "NODE_ENV"), Some("production"));
    assert_eq!(get(&f.backend, "MYSTICPARADOX_SERVICE_ROLE"), Some("api"));
    assert_eq!(get(&f.backend, "BIND_HOST"), Some("127.0.0.1"));
    assert_eq!(get(&f.backend, "REFORGED_HTTP_ONLY"), Some("true"));
    assert_eq!(get(&f.backend, "QOS_TARGET_URL"), Some("http://127.0.0.1:61100/QoS"));
    assert_eq!(get(&f.backend, "REALTIME_XMPP_DEV_CITY_MUC"), Some("false"));
    assert_eq!(
        get(&f.backend, "HUNT_GO_NOW_MIN_DELAY_MS"),
        Some("1"),
        "no 10-20 s wait (with the build locked) after the hunt server is ready"
    );
    assert_eq!(get(&f.backend, "HUNT_GO_NOW_MAX_DELAY_MS"), Some("1000"));
    assert_eq!(get(&f.backend, "REALTIME_XMPP_ALLOWED_HOSTS"), Some("100.101.102.103"));
    assert_eq!(get(&f.backend, "GAMESERVER_API_KEYS"), Some(s.gameserver_key.as_str()));
    assert_eq!(
        get(&f.director, "METAGAME_API_KEY"),
        Some(s.gameserver_key.as_str()),
        "the Director's game servers use the backend's game-server key"
    );
    assert_eq!(get(&f.director, "MY_IP"), Some("100.101.102.103"));
    assert_eq!(get(&f.director, "REFORGED_BACKEND_URL"), Some("http://127.0.0.1:61000"));
    assert_eq!(get(&f.director, "PORT_RANGE_BEGIN"), Some("8770"));
    assert_eq!(get(&f.director, "PORT_RANGE_END"), Some("8777"));
    assert!(
        get(&f.director, "GAMESERVER_BINARY_PATH")
            .unwrap()
            .ends_with("Dauntless-Win64-Shipping.exe")
    );
    assert_eq!(get(&f.gateway, "GATEWAY_TLS"), Some("0"));
    assert_eq!(get(&f.gateway, "GATEWAY_BIND"), Some("100.101.102.103"));
    assert_eq!(get(&f.gateway, "GATEWAY_PORT"), Some("61000"));
    assert_eq!(get(&f.gateway, "GATEWAY_ALLOWLIST"), Some("0"));
    assert_eq!(f.allowlist, None);
    assert_eq!(
        get(&f.content, "CONTENT_RUNTIME_DIR"),
        Some(p.runtime_publish.to_str().unwrap())
    );
}

#[test]
fn public_mode_tls_on_443_with_the_certificate_and_the_allowlist_helper_for_real_on_windows_and_linux() {
    let s = fast_secrets();
    let p = win_paths();
    let c = config(public());
    let f = build_env_files(&c, &s, &p, "", Platform::Windows);
    let allowlist = f.allowlist.clone().unwrap();
    assert_eq!(get(&f.gateway, "GATEWAY_TLS"), Some("1"));
    assert_eq!(get(&f.gateway, "GATEWAY_BIND"), Some("0.0.0.0"));
    assert_eq!(get(&f.gateway, "GATEWAY_PORT"), Some("443"));
    assert_eq!(get(&f.gateway, "GATEWAY_CERT"), Some(p.cert_file.to_str().unwrap()));
    assert_eq!(get(&f.gateway, "ALLOWLIST_SECRET"), Some(s.allowlist_secret.as_str()));
    assert_eq!(get(&allowlist, "ALLOWLIST_SECRET"), Some(s.allowlist_secret.as_str()));
    assert_eq!(get(&allowlist, "ALLOWLIST_DRY_RUN"), Some("0"));
    assert_eq!(get(&allowlist, "ALLOWLIST_FIREWALL"), Some("windows"));
    let linux = build_env_files(&c, &s, &p, "", Platform::Linux).allowlist.unwrap();
    assert_eq!(get(&linux, "ALLOWLIST_DRY_RUN"), Some("0"));
    assert_eq!(get(&linux, "ALLOWLIST_FIREWALL"), Some("nftables"));
    let other = build_env_files(&c, &s, &p, "", Platform::Other).allowlist.unwrap();
    assert_eq!(get(&other, "ALLOWLIST_DRY_RUN"), Some("1"));
    assert_eq!(get(&f.director, "MY_IP"), Some("203.0.113.7"));
    assert_eq!(
        get(&f.backend, "REALTIME_XMPP_ALLOWED_HOSTS"),
        Some("hunts.example.net")
    );
}

#[test]
fn test_multipliers_only_a_private_servers_backend_gets_them_always_with_its_mode_and_host() {
    let s = fast_secrets();
    let p = win_paths();
    let on = build_env_files(
        &config(with(private(), json!({ "multipliers": { "xp": 5, "rewards": 3 } }))),
        &s,
        &p,
        "",
        Platform::Windows,
    );
    assert_eq!(get(&on.backend, "REFORGED_SERVER_MODE"), Some("private"));
    assert_eq!(get(&on.backend, "REFORGED_SERVER_HOST"), Some("100.101.102.103"));
    assert_eq!(get(&on.backend, "REFORGED_XP_MULTIPLIER"), Some("5"));
    assert_eq!(get(&on.backend, "REFORGED_REWARD_MULTIPLIER"), Some("3"));
    let off = build_env_files(&config(private()), &s, &p, "", Platform::Windows);
    assert_eq!(get(&off.backend, "REFORGED_XP_MULTIPLIER"), None);
    let public_files = build_env_files(&config(public()), &s, &p, "", Platform::Windows);
    assert_eq!(get(&public_files.backend, "REFORGED_SERVER_MODE"), Some("public"));
    assert_eq!(get(&public_files.backend, "REFORGED_XP_MULTIPLIER"), None);
    assert!(error(with(public(), json!({ "multipliers": { "xp": 5, "rewards": 1 } }))).contains("private"));
    assert!(error(with(private(), json!({ "multipliers": { "xp": 101, "rewards": 1 } }))).contains("1 to 100"));
    assert!(error(with(private(), json!({ "multipliers": { "xp": 0.5, "rewards": 1 } }))).contains("1 to 100"));
    assert_eq!(
        validate(&with(public(), json!({ "multipliers": { "xp": 1, "rewards": 1 } }))).unwrap()["mode"],
        "public"
    );
}

#[test]
fn are_written_owner_only_and_the_allowlist_file_goes_away_in_private_mode() {
    let s = fast_secrets();
    let root = tempfile::tempdir().unwrap();
    let lp = Paths::new(root.path());
    write_env_files(&lp, &build_env_files(&config(public()), &s, &lp, "", Platform::Linux)).unwrap();
    assert!(lp.config.join("allowlist.env").exists());
    write_env_files(&lp, &build_env_files(&config(private()), &s, &lp, "", Platform::Linux)).unwrap();
    assert!(!lp.config.join("allowlist.env").exists());
    let backend = parse_env(&std::fs::read_to_string(lp.config.join("backend.env")).unwrap());
    assert_eq!(backend.get("REFORGED_ADMIN_KEY"), Some(s.admin_key.as_str()));
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = std::fs::metadata(lp.config.join("backend.env"))
            .unwrap()
            .permissions()
            .mode();
        assert_eq!(mode & 0o077, 0);
    }
}

// ------------------------------------------------------------------ ports.test.ts

fn moved() -> Value {
    with(public_on_ip(), json!({ "publicPort": 4443 }))
}

fn public_on_ip() -> Value {
    with(
        public(),
        json!({ "host": "203.0.113.7", "gameDir": "/srv/dauntless/Dauntless", "mongo": { "uri": DEFAULT_MONGO_URI, "manage": true, "mongod": "/usr/bin/mongod" } }),
    )
}

#[test]
fn the_public_gateways_port_is_443_unless_moved_and_61000_for_a_private_server() {
    assert_eq!(gateway_port(&config(public_on_ip())), 443);
    assert_eq!(gateway_port(&config(moved())), 4443);
    assert_eq!(gateway_port(&config(private())), 61000);
}

#[test]
fn the_public_port_is_checked_a_real_port_not_one_the_server_uses_itself_public_servers_only() {
    assert_eq!(validate(&moved()).unwrap()["publicPort"], 4443);
    for bad in [0, 70000, 61000, 61005, 27017, 8777, 8766, 61100] {
        assert!(
            error(with(public_on_ip(), json!({ "publicPort": bad }))).contains("publicPort"),
            "{bad}"
        );
    }
    assert!(
        error(with(
            public_on_ip(),
            json!({ "mode": "private", "host": "100.101.102.103", "gameAddress": "100.101.102.103", "publicPort": 4443 })
        ))
        .contains("public servers only")
    );
}

#[test]
fn the_public_port_reaches_the_gateways_env_file() {
    let p = Paths::new("/home/reforged/Reforged");
    let f = build_env_files(&config(moved()), &fast_secrets(), &p, "", Platform::Linux);
    assert_eq!(get(&f.gateway, "GATEWAY_PORT"), Some("4443"));
}

// ------------------------------------------------------------------ performance.test.ts

fn perf_public() -> Value {
    with(
        public_on_ip(),
        json!({ "name": "Friday Hunts", "gameDir": "/home/reforged/games/Dauntless" }),
    )
}

fn perf_with(changes: Value) -> Value {
    with(
        perf_public(),
        json!({ "performance": with(default_performance(), changes) }),
    )
}

#[test]
fn caps_the_game_servers_at_30_fps_starts_training_on_demand_and_keeps_the_game_logs_quiet() {
    let s = fast_secrets();
    let p = Paths::new("/home/reforged/Reforged");
    for platform in [Platform::Linux, Platform::Windows] {
        let f = build_env_files(&config(perf_public()), &s, &p, "", platform);
        assert_eq!(get(&f.director, "REFORGED_SERVER_MAX_FPS"), Some("30"));
        assert_eq!(get(&f.director, "REFORGED_SERVER_IDLE_FPS"), Some("10"));
        assert_eq!(get(&f.director, "TRAINING_ON_DEMAND"), Some("1"));
        assert_eq!(get(&f.director, "GAMESERVER_LOG_CMDS"), Some(QUIET_GAME_LOG_CMDS));
        assert_eq!(get(&f.director, "GAMESERVER_MEMORY_GUARD"), Some("1"));
        assert_eq!(
            get(&f.director, "TRAINING_IDLE_STOP_MINUTES"),
            Some("5"),
            "on demand, and stopped again after 5 min empty"
        );
        let exe = Path::new("/home/reforged/games/Dauntless").join(game_exe_relative());
        assert_eq!(
            get(&f.director, "GAMESERVER_RUNTIME_LOG_DIR"),
            exe.parent().and_then(Path::to_str)
        );
        assert_eq!(get(&f.director, "GAMESERVER_MIN_FREE_MB"), Some("1024"));
        assert_eq!(get(&f.director, "GAMESERVER_HUNT_MEMORY_MB"), Some("1536"));
    }
}

#[test]
fn keeps_udp_8770_8777_up_to_6_hunts_and_reaches_one_port_further_down_per_hunt_above_that() {
    let s = fast_secrets();
    let p = Paths::new("/home/reforged/Reforged");
    let hunts = |n: u32| config(with(perf_public(), json!({ "maxHuntServers": n })));
    assert_eq!(game_ports(&hunts(3)), (8770, 8777));
    assert_eq!(game_ports(&hunts(6)), (8770, 8777));
    assert_eq!(game_ports(&hunts(8)), (8768, 8777));
    assert_eq!(game_ports(&hunts(10)), (8766, 8777));
    let f = build_env_files(&hunts(8), &s, &p, "", Platform::Linux);
    assert_eq!(
        get(&f.director, "PORT_RANGE_BEGIN"),
        Some("8768"),
        "Ramsgate 8777, Training 8776, eight hunts 8768-8775"
    );
    assert_eq!(get(&f.director, "PORT_RANGE_END"), Some("8777"));
    assert_eq!(get(&f.director, "MAX_CONCURRENT_HUNT_SERVERS"), Some("8"));
    assert_eq!(get(f.allowlist.as_ref().unwrap(), "ALLOWLIST_PORTS"), Some("8768-8777"));
    let three = build_env_files(&config(perf_public()), &s, &p, "", Platform::Linux);
    assert_eq!(
        get(three.allowlist.as_ref().unwrap(), "ALLOWLIST_PORTS"),
        Some("8770-8777")
    );
}

#[test]
fn follows_host_json_no_cap_training_at_boot_upstreams_verbose_logs() {
    let s = fast_secrets();
    let p = Paths::new("/home/reforged/Reforged");
    let c = config(perf_with(json!({
        "serverFps": 0, "idleFps": 0, "trainingOnDemand": false, "verboseGameLogs": true, "memoryGuard": false,
    })));
    let f = build_env_files(&c, &s, &p, "", Platform::Linux);
    assert_eq!(get(&f.director, "REFORGED_SERVER_MAX_FPS"), Some("0"));
    assert_eq!(get(&f.director, "REFORGED_SERVER_IDLE_FPS"), Some("0"));
    assert_eq!(get(&f.director, "TRAINING_ON_DEMAND"), Some("0"));
    assert_eq!(
        get(&f.director, "GAMESERVER_LOG_CMDS"),
        None,
        "the Director keeps its own default"
    );
    assert_eq!(get(&f.director, "GAMESERVER_MEMORY_GUARD"), Some("0"));
    assert_eq!(
        get(&f.director, "TRAINING_IDLE_STOP_MINUTES"),
        Some("0"),
        "at boot: never stopped"
    );
    let no_game = build_env_files(
        &config(with(perf_public(), json!({ "gameDir": null }))),
        &s,
        &p,
        "",
        Platform::Linux,
    );
    assert_eq!(
        get(&no_game.director, "GAMESERVER_RUNTIME_LOG_DIR"),
        None,
        "no game folder yet"
    );
}

#[test]
fn writes_only_values_the_director_accepts() {
    // The Director's check for GAMESERVER_LOG_CMDS (patch 0021).
    assert!(QUIET_GAME_LOG_CMDS.len() <= 512);
    assert!(
        QUIET_GAME_LOG_CMDS
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || "_, ".contains(c))
    );
}

#[test]
fn refuses_frame_rates_and_switches_the_runtime_would_not_take() {
    for (changes, name) in [
        (json!({ "serverFps": 5 }), "serverFps"),
        (json!({ "serverFps": 241 }), "serverFps"),
        (json!({ "serverFps": 29.5 }), "serverFps"),
        (json!({ "idleFps": -1 }), "idleFps"),
        (json!({ "idleFps": 300 }), "idleFps"),
        (json!({ "trainingOnDemand": "yes" }), "trainingOnDemand"),
        (json!({ "huntMemoryMb": 100 }), "huntMemoryMb"),
        (json!({ "minFreeMemoryMb": -1 }), "minFreeMemoryMb"),
        (json!({ "trainingIdleMinutes": 1441 }), "trainingIdleMinutes"),
    ] {
        let message = error(perf_with(changes.clone()));
        assert!(message.contains(name), "{changes}: {message}");
    }
    // A host.json written before the memory guard existed: the defaults fill the new fields.
    let older = with(
        perf_public(),
        json!({ "performance": { "serverFps": 30, "idleFps": 10, "trainingOnDemand": true, "verboseGameLogs": false } }),
    );
    let f = build_env_files(
        &config(older),
        &fast_secrets(),
        &Paths::new("/home/reforged/Reforged"),
        "",
        Platform::Linux,
    );
    assert_eq!(get(&f.director, "GAMESERVER_MEMORY_GUARD"), Some("1"));
    assert_eq!(
        validate(&perf_with(json!({ "idleFps": 1 }))).unwrap()["performance"]["idleFps"],
        1
    );
}

// ------------------------------------------------------------------ linux.test.ts

fn linux_public() -> Value {
    with(
        public_on_ip(),
        json!({
            "name": "Freeza Hunts",
            "linux": { "wine": null, "proton": null, "xvfb": true, "firewallInterface": "eth0" },
        }),
    )
}

#[test]
fn linux_options_are_checked() {
    assert_eq!(validate(&linux_public()).unwrap()["linux"]["firewallInterface"], "eth0");
    let lx = |changes: Value| {
        with(
            linux_public(),
            json!({ "linux": with(json!({ "wine": null, "proton": null, "xvfb": true, "firewallInterface": "eth0" }), changes) }),
        )
    };
    assert!(error(lx(json!({ "wine": "wine" }))).contains("linux.wine"));
    assert!(error(lx(json!({ "firewallInterface": "a b" }))).contains("firewallInterface"));
}

#[test]
fn linux_the_director_starts_the_wine_launcher_with_the_executable_the_prefix_and_a_longer_timeout() {
    let s = fast_secrets();
    let p = Paths::new("/home/reforged/Reforged");
    let f = build_env_files(&config(linux_public()), &s, &p, "", Platform::Linux);
    let exe = Path::new("/srv/dauntless/Dauntless").join(game_exe_relative());
    assert_eq!(
        get(&f.director, "GAMESERVER_BINARY_PATH"),
        p.gameserver_wrapper.to_str()
    );
    assert_eq!(get(&f.director, "REFORGED_GAME_EXE"), exe.to_str());
    assert_eq!(get(&f.director, "REFORGED_WINEPREFIX"), p.wine_prefix.to_str());
    assert_eq!(get(&f.director, "REFORGED_XVFB"), Some("1"));
    assert_eq!(get(&f.director, "REFORGED_WINE"), None, "found on PATH when not set");
    assert_eq!(get(&f.director, "GAMESERVER_READY_TIMEOUT_MS"), Some("90000"));
    let allowlist = f.allowlist.unwrap();
    assert_eq!(get(&allowlist, "ALLOWLIST_DRY_RUN"), Some("0"));
    assert_eq!(get(&allowlist, "ALLOWLIST_FIREWALL"), Some("nftables"));
    assert_eq!(get(&allowlist, "ALLOWLIST_NFT_TABLE"), Some("reforged_revival"));
    assert_eq!(get(&allowlist, "ALLOWLIST_INTERFACE"), Some("eth0"));
    let wine = config(with(
        linux_public(),
        json!({ "linux": { "wine": "/usr/bin/wine64", "proton": null, "xvfb": false, "firewallInterface": "eth0" } }),
    ));
    let f = build_env_files(&wine, &s, &p, "", Platform::Linux);
    assert_eq!(get(&f.director, "REFORGED_WINE"), Some("/usr/bin/wine64"));
    assert_eq!(get(&f.director, "REFORGED_XVFB"), Some("0"));
}

#[test]
fn windows_keeps_starting_the_executable_itself() {
    let p = Paths::new("/home/reforged/Reforged");
    let f = build_env_files(&config(linux_public()), &fast_secrets(), &p, "", Platform::Windows);
    let exe = Path::new("/srv/dauntless/Dauntless").join(game_exe_relative());
    assert_eq!(get(&f.director, "GAMESERVER_BINARY_PATH"), exe.to_str());
    assert_eq!(get(&f.director, "REFORGED_GAME_EXE"), None);
    assert_eq!(get(&f.director, "GAMESERVER_READY_TIMEOUT_MS"), Some("30000"));
    assert_eq!(get(f.allowlist.as_ref().unwrap(), "ALLOWLIST_INTERFACE"), None);
}
