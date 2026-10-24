//! `setup` and `multipliers` (MIGRATION_PLAN.md RH3): the Node kit's tests for them (`kit.test.ts`,
//! `ports.test.ts`, `performance.test.ts`, `linux.test.ts`), the same host.json as the Node kit for
//! the same options (`tests/fixtures/node/rh3/rh3.json`, made by its `gen.ts` with the Node kit's own
//! code), and a fresh private and a fresh public setup into a temp folder with this PC's tools mocked.

#![allow(clippy::unwrap_used, clippy::expect_used)] // test helpers

use std::cell::RefCell;
use std::collections::HashMap;
use std::path::{Path, PathBuf};

use clap::Parser;
use serde_json::Value;

use host::cli::{Cli, Command, MultipliersArgs, SetupArgs};
use host::commands::setup::firewall::firewall_script;
use host::commands::setup::options::{
    host_config_from_args, linux_from_args, performance_from_args, performance_summary, public_port_from_args,
};
use host::commands::setup::paradox::Layout;
use host::commands::setup::{SetupEnv, run_with};
use host::error::HostError;
use host::hostconfig::{
    DEFAULT_PERFORMANCE, HostConfig, LinuxOptions, Multipliers, PerformanceOptions, Platform, ServerMode,
    host_config_text, load_host_config, read_env_file, read_host_config, write_host_config,
};
use host::paths::{Paths, game_exe_relative};
use host::preset::PresetApplied;

const MONGOD: &str = "C:\\Program Files\\MongoDB\\Server\\8.0\\bin\\mongod.exe";

/// This PC, mocked: what Tailscale, PATH and mongod say, and what setup asked of the tools.
struct Mock {
    platform: Platform,
    tailscale: Option<String>,
    mongod: Option<String>,
    on_path: HashMap<String, String>,
    installs: RefCell<usize>,
    certificates: RefCell<Vec<Vec<String>>>,
}

impl Mock {
    fn new() -> Self {
        Self {
            platform: Platform::Windows,
            tailscale: Some("100.101.102.103".into()),
            mongod: Some(MONGOD.into()),
            on_path: HashMap::new(),
            installs: RefCell::new(0),
            certificates: RefCell::new(Vec::new()),
        }
    }
}

impl SetupEnv for Mock {
    fn platform(&self) -> Platform {
        self.platform
    }
    fn is_root(&self) -> Result<bool, HostError> {
        Ok(false)
    }
    fn tailscale_ipv4(&self) -> Result<Option<String>, HostError> {
        Ok(self.tailscale.clone())
    }
    fn find_mongod(&self) -> Result<Option<String>, HostError> {
        Ok(self.mongod.clone())
    }
    fn find_on_path(&self, name: &str) -> Result<Option<String>, HostError> {
        Ok(self.on_path.get(name).cloned())
    }
    fn source_commit(&self, _repo: &Path) -> Result<String, HostError> {
        Ok("abc123def456".into())
    }
    fn apply_preset(&self, _paths: &Paths) -> Result<Option<PresetApplied>, HostError> {
        Ok(None)
    }
    fn install_services(&self, layout: &Layout, _log_file: &Path, out: &mut dyn FnMut(&str)) -> Result<(), HostError> {
        *self.installs.borrow_mut() += 1;
        for (_, label) in host::commands::setup::install::services(layout) {
            out(&format!("- {label}: npm ci (mocked)"));
        }
        Ok(())
    }
    fn make_certificate(
        &self,
        _layout: &Layout,
        hosts: &[String],
        tls: &Path,
        _log_file: &Path,
    ) -> Result<(), HostError> {
        // As the gateway's make-cert does: a self-signed certificate for the server's names.
        self.certificates.borrow_mut().push(hosts.to_vec());
        let cert = rcgen::generate_simple_self_signed(hosts.to_vec()).unwrap();
        std::fs::create_dir_all(tls).unwrap();
        std::fs::write(tls.join("gateway-cert.pem"), cert.cert.pem()).unwrap();
        std::fs::write(tls.join("gateway-key.pem"), cert.signing_key.serialize_pem()).unwrap();
        Ok(())
    }
    fn gateway_program(&self) -> String {
        "C:\\rr\\gateway.exe".into()
    }
}

fn setup_args(args: &[&str]) -> SetupArgs {
    let mut argv = vec!["reforged-host", "setup"];
    argv.extend_from_slice(args);
    match Cli::try_parse_from(argv).unwrap().command {
        Command::Setup(s) => s,
        _ => unreachable!(),
    }
}

fn from_args(args: &[&str], existing: Option<&HostConfig>, env: &Mock) -> Result<HostConfig, String> {
    host_config_from_args(&setup_args(args), existing, env).map_err(|e| e.to_string())
}

/// Written and read back, as setup does.
fn round_trip(c: &HostConfig) -> HostConfig {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("host.json");
    write_host_config(&file, c).unwrap();
    read_host_config(&file).unwrap().unwrap()
}

// ------------------------------------------------------------------ the same as the Node kit

fn fixture() -> Value {
    let file = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/node/rh3/rh3.json");
    serde_json::from_str(&std::fs::read_to_string(file).unwrap()).unwrap()
}

fn mock_for(detect: &Value) -> Mock {
    Mock {
        platform: if detect["platform"] == "linux" {
            Platform::Linux
        } else {
            Platform::Windows
        },
        tailscale: detect["tailscale"].as_str().map(str::to_owned),
        mongod: detect["mongod"].as_str().map(str::to_owned),
        ..Mock::new()
    }
}

/// The configs setup made for the fixture's cases, by name.
fn fixture_configs() -> HashMap<String, HostConfig> {
    let mut configs: HashMap<String, HostConfig> = HashMap::new();
    for case in fixture()["setups"].as_array().unwrap() {
        let name = case["name"].as_str().unwrap();
        let args: Vec<&str> = case["args"]
            .as_array()
            .unwrap()
            .iter()
            .map(|a| a.as_str().unwrap())
            .collect();
        let before = case["before"].as_str().map(|b| &configs[b]);
        let env = mock_for(&case["detect"]);
        let made = from_args(&args, before, &env)
            .and_then(|c| host_config_text(&c).map(|t| (c, t)).map_err(|e| e.to_string()));
        match (made, case.get("error").and_then(Value::as_str)) {
            (Err(e), Some(expected)) => assert_eq!(e, expected, "{name}"),
            (Err(e), None) => panic!("{name}: the Node kit takes {args:?}, this one says {e}"),
            (Ok(_), Some(expected)) => panic!("{name}: the Node kit refuses {args:?}: {expected}"),
            (Ok((c, text)), None) => {
                assert_eq!(text, case["hostJson"].as_str().unwrap(), "{name}: host.json");
                let read = round_trip(&c);
                assert_eq!(performance_summary(&read), case["summary"].as_str().unwrap(), "{name}");
                configs.insert(name.to_owned(), read);
            }
        }
    }
    configs
}

#[test]
fn writes_the_host_json_the_node_kit_writes_and_refuses_what_it_refuses() {
    assert_eq!(fixture_configs().len(), 23);
}

#[test]
fn writes_the_node_kits_firewall_rules() {
    let configs = fixture_configs();
    for case in fixture()["firewall"].as_array().unwrap() {
        let name = case["name"].as_str().unwrap();
        assert_eq!(
            firewall_script(&configs[name], "C:\\node\\node.exe"),
            case["script"].as_str().unwrap(),
            "{name}"
        );
    }
}

#[test]
fn makes_the_node_kits_invites() {
    let configs = fixture_configs();
    let fixture = fixture();
    for case in fixture["invites"].as_array().unwrap() {
        let name = case["name"].as_str().unwrap();
        let link =
            host::invite::invite_for(&configs[name], case["code"].as_str().unwrap(), case["fp"].as_str()).unwrap();
        assert_eq!(link, case["link"].as_str().unwrap(), "{name}");
    }
    let no_fp = host::invite::format_invite_parts(&host::invite::InviteParts {
        mode: ServerMode::Public,
        host: "x.example".into(),
        port: 443,
        code: "ABCD".into(),
        name: "x".into(),
        fp: None,
        share: None,
    })
    .unwrap_err();
    assert_eq!(no_fp.to_string(), fixture["noFingerprint"].as_str().unwrap());
}

// ------------------------------------------------------------------ kit.test.ts

#[test]
fn builds_host_json_from_the_arguments_the_old_host_json_and_what_it_finds_on_this_pc() {
    let env = Mock::new();
    let c = from_args(&["--mode", "private", "--name", "Friday"], None, &env).unwrap();
    assert_eq!(c.host, "100.101.102.103", "private: this PC's Tailscale address");
    assert_eq!(c.game_address, "100.101.102.103");
    assert_eq!(c.mongo.uri, host::hostconfig::DEFAULT_MONGO_URI);
    assert!(c.mongo.manage);
    assert_eq!(c.mongo.mongod.as_deref(), Some(MONGOD));
    let public = from_args(&["--mode", "public", "--host", "203.0.113.7"], Some(&c), &env).unwrap();
    assert_eq!(public.game_address, "203.0.113.7");
    assert_eq!(public.name, "Friday", "kept from the old host.json");
    assert!(
        from_args(&["--mode", "public", "--host", "hunts.example.net"], None, &env)
            .unwrap_err()
            .contains("game-address")
    );
    let dns = from_args(
        &[
            "--mode",
            "public",
            "--host",
            "hunts.example.net",
            "--game-address",
            "203.0.113.7",
        ],
        None,
        &env,
    );
    assert_eq!(dns.unwrap().game_address, "203.0.113.7");
    assert!(from_args(&[], None, &env).unwrap_err().contains("--mode"));
    let offline = Mock {
        tailscale: None,
        ..Mock::new()
    };
    assert!(
        from_args(&["--mode", "private"], None, &offline)
            .unwrap_err()
            .contains("Tailscale")
    );
    let own = from_args(
        &[
            "--mode",
            "private",
            "--mongo-uri",
            "mongodb://db.local:27017/?replicaSet=rs0",
        ],
        None,
        &env,
    );
    assert!(!own.unwrap().mongo.manage);
}

#[test]
fn takes_test_multipliers_on_a_private_server_only_keeps_them_and_off_clears_them() {
    let env = Mock::new();
    let c = from_args(
        &["--mode", "private", "--xp-multiplier", "5", "--reward-multiplier", "x3"],
        None,
        &env,
    )
    .unwrap();
    let five_three = Some(Multipliers { xp: 5.0, rewards: 3.0 });
    assert_eq!(c.multipliers, five_three);
    assert_eq!(from_args(&[], Some(&c), &env).unwrap().multipliers, five_three);
    assert_eq!(from_args(&["--off"], Some(&c), &env).unwrap().multipliers, None);
    let ones = from_args(&["--xp-multiplier", "1", "--reward-multiplier", "1"], Some(&c), &env);
    assert_eq!(ones.unwrap().multipliers, None);
    let public = from_args(&["--mode", "public", "--host", "203.0.113.7"], Some(&c), &env).unwrap();
    assert_eq!(public.multipliers, None, "not carried into a public server");
    let refused = from_args(
        &["--mode", "public", "--host", "203.0.113.7", "--xp-multiplier", "5"],
        None,
        &env,
    );
    assert!(refused.unwrap_err().contains("private"));
    let big = from_args(&["--mode", "private", "--xp-multiplier", "500"], None, &env);
    assert!(big.unwrap_err().contains("1 to 100"));
}

#[test]
fn follows_a_new_tailscale_address_and_keeps_the_old_one_only_when_tailscale_is_not_running() {
    let at = |ip: Option<&str>| Mock {
        tailscale: ip.map(str::to_owned),
        ..Mock::new()
    };
    let old = from_args(
        &["--mode", "private", "--name", "Freeza"],
        None,
        &at(Some("100.117.171.17")),
    )
    .unwrap();
    let moved = from_args(&[], Some(&old), &at(Some("100.88.12.40"))).unwrap();
    assert_eq!(moved.host, "100.88.12.40", "another Tailscale account, another address");
    assert_eq!(moved.game_address, "100.88.12.40", "the game servers move with it");
    assert_eq!(moved.name, "Freeza");
    assert_eq!(from_args(&[], Some(&old), &at(None)).unwrap().host, "100.117.171.17");
    let wins = from_args(&["--host", "100.64.0.9"], Some(&old), &Mock::new()).unwrap();
    assert_eq!(wins.host, "100.64.0.9", "--host still wins");
}

#[test]
fn wants_a_full_path_for_game() {
    let env = Mock::new();
    let posix = from_args(
        &["--mode", "private", "--game", "/games/Dauntless 1.14.7/Dauntless"],
        None,
        &env,
    )
    .unwrap();
    if cfg!(windows) {
        assert!(
            posix
                .game_dir
                .unwrap()
                .ends_with("\\games\\Dauntless 1.14.7\\Dauntless"),
            "path.resolve: this drive"
        );
    } else {
        assert_eq!(posix.game_dir.as_deref(), Some("/games/Dauntless 1.14.7/Dauntless"));
    }
    let windows = from_args(
        &["--mode", "private", "--game", "D:\\Games\\Dauntless 1.14.7\\Dauntless"],
        None,
        &env,
    );
    assert!(windows.unwrap().game_dir.is_some());
    for relative in ["Games\\Dauntless", "^D:\\Games\\Dauntless^ 1.14.7\\Dauntless^"] {
        let error = from_args(&["--mode", "private", "--game", relative], None, &env).unwrap_err();
        assert!(error.contains("full path"), "{error}");
    }
}

#[test]
fn writes_firewall_rules_tailscale_only_in_private_mode_443_and_a_closed_allowlist_rule_in_public_mode() {
    let env = Mock::new();
    let base = from_args(&["--mode", "private", "--game", "D:\\Games\\Dauntless"], None, &env).unwrap();
    let private = firewall_script(&base, "C:\\node\\node.exe");
    assert!(private.contains("100.64.0.0/10"));
    assert!(private.contains("'61000'"));
    assert!(!private.contains("'443'"));
    // path.resolve: unchanged on Windows, under the working folder elsewhere.
    let exe = Path::new(base.game_dir.as_deref().unwrap()).join(game_exe_relative());
    let public = HostConfig {
        mode: ServerMode::Public,
        host: "203.0.113.7".into(),
        game_address: "203.0.113.7".into(),
        share_url: None,
        ..base
    };
    let public = firewall_script(&public, "C:\\node\\node.exe");
    assert!(public.contains("'443'"));
    assert!(public.contains("ReforgedRevival-GamePorts-Allowlist"));
    assert!(public.contains("'False'"));
    assert!(public.contains(&format!("'{}'", exe.display())));
}

// ------------------------------------------------------------------ ports.test.ts

fn moved() -> HostConfig {
    let public = from_args(
        &["--mode", "public", "--host", "203.0.113.7", "--public-port", "4443"],
        None,
        &Mock::new(),
    );
    public.unwrap()
}

#[test]
fn the_public_port_comes_from_public_port_is_kept_when_not_given_and_443_goes_back() {
    let moved = moved();
    let port = |args: &[&str], mode, existing: Option<&HostConfig>| {
        public_port_from_args(&setup_args(args), mode, existing).map_err(|e| e.to_string())
    };
    assert_eq!(
        port(&["--public-port", "4443"], ServerMode::Public, None),
        Ok(Some(4443.0))
    );
    assert_eq!(port(&[], ServerMode::Public, Some(&moved)), Ok(Some(4443.0)));
    assert_eq!(
        port(&["--public-port", "443"], ServerMode::Public, Some(&moved)),
        Ok(None)
    );
    assert_eq!(port(&[], ServerMode::Private, Some(&moved)), Ok(None));
    assert!(
        port(&["--public-port", "4443"], ServerMode::Private, None)
            .unwrap_err()
            .contains("public servers only")
    );
    assert!(
        port(&["--public-port", "web"], ServerMode::Public, None)
            .unwrap_err()
            .contains("port number")
    );
}

#[test]
fn the_moved_port_reaches_the_invite_the_launcher_reads() {
    let link = host::invite::invite_for(&moved(), "ABCD-EFGH-IJKL", Some(&"a".repeat(64))).unwrap();
    let parsed = protocol::invite::parse_invite(&link).unwrap();
    assert_eq!(parsed.port, 4443);
}

#[test]
fn tells_how_to_find_the_program_on_a_port() {
    use host::doctor::port_owner_hint_for;
    assert!(port_owner_hint_for(443, Platform::Linux).contains("ss -tlnp 'sport = :443'"));
    assert!(port_owner_hint_for(443, Platform::Windows).contains("netstat -ano | findstr :443"));
}

// ------------------------------------------------------------------ performance.test.ts

#[test]
fn performance_defaults_keeps_what_host_json_had_and_takes_each_option() {
    let perf = |args: &[&str], existing: Option<PerformanceOptions>| {
        performance_from_args(&setup_args(args), existing).map_err(|e| e.to_string())
    };
    assert_eq!(perf(&[], None), Ok(DEFAULT_PERFORMANCE));
    let set = perf(
        &[
            "--server-fps",
            "60",
            "--idle-fps",
            "0",
            "--training-at-boot",
            "--verbose-game-logs",
            "--no-memory-guard",
            "--min-free-ram",
            "512",
            "--hunt-ram",
            "2048",
            "--training-idle-minutes",
            "15",
        ],
        None,
    )
    .unwrap();
    assert_eq!(
        set,
        PerformanceOptions {
            server_fps: 60,
            idle_fps: 0,
            training_on_demand: false,
            training_idle_minutes: 15,
            verbose_game_logs: true,
            memory_guard: false,
            min_free_memory_mb: 512,
            hunt_memory_mb: 2048
        }
    );
    assert_eq!(perf(&[], Some(set)), Ok(set), "kept on the next setup");
    assert_eq!(
        perf(&["--training-on-demand", "--quiet-game-logs"], Some(set)),
        Ok(PerformanceOptions {
            training_on_demand: true,
            verbose_game_logs: false,
            ..set
        })
    );
    for (args, name) in [
        (["--server-fps", "5"], "--server-fps"),
        (["--idle-fps", "fast"], "--idle-fps"),
        (["--hunt-ram", "100"], "--hunt-ram"),
        (["--training-idle-minutes", "soon"], "--training-idle-minutes"),
    ] {
        assert!(perf(&args, None).unwrap_err().contains(name));
    }
    assert_eq!(
        perf(&["--training-idle-minutes", "0"], None)
            .unwrap()
            .training_idle_minutes,
        0
    );
    assert!(perf(&["--memory-guard"], Some(set)).unwrap().memory_guard);
}

#[test]
fn performance_goes_into_host_json_with_setup() {
    let env = Mock::new();
    let c = from_args(
        &["--mode", "public", "--host", "203.0.113.7", "--server-fps", "45"],
        None,
        &env,
    )
    .unwrap();
    assert_eq!(c.performance.unwrap().server_fps, 45);
    assert_eq!(round_trip(&c).performance.unwrap().server_fps, 45);
    assert_eq!(
        from_args(&[], Some(&c), &env).unwrap().performance.unwrap().server_fps,
        45
    );
    let public = from_args(&["--mode", "public", "--host", "203.0.113.7"], None, &env).unwrap();
    assert!(performance_summary(&public).contains(
        "30 fps, 10 fps after 15 s without players; Training Grounds on demand, stopped after 5 min without players; quiet game logs; at most 3 hunts at once (UDP 8770-8777), a new hunt only with 2560 MB free"
    ));
    assert_eq!(
        from_args(&["--max-hunts", "8"], Some(&public), &env)
            .unwrap()
            .max_hunt_servers,
        8
    );
}

// ------------------------------------------------------------------ linux.test.ts

#[test]
fn linux_reads_wine_proton_no_xvfb_and_firewall_interface_and_keeps_the_old_ones_otherwise() {
    let find = |n: &str| Ok((n == "wine64").then(|| "/usr/bin/wine64".to_owned()));
    let lx = |args: &[&str], existing: Option<&LinuxOptions>| {
        linux_from_args(&setup_args(args), existing, &find).map_err(|e| e.to_string())
    };
    let none = LinuxOptions {
        wine: None,
        proton: None,
        xvfb: true,
        firewall_interface: None,
    };
    assert_eq!(lx(&[], None), Ok(none));
    let set = lx(&["--wine", "wine64", "--no-xvfb", "--firewall-interface", "ens3"], None).unwrap();
    assert_eq!(
        set,
        LinuxOptions {
            wine: Some("/usr/bin/wine64".into()),
            proton: None,
            xvfb: false,
            firewall_interface: Some("ens3".into())
        }
    );
    assert_eq!(lx(&[], Some(&set)), Ok(set.clone()), "kept");
    assert_eq!(
        lx(
            &[
                "--xvfb",
                "--no-firewall-interface",
                "--proton",
                "/opt/GE-Proton10/proton"
            ],
            Some(&set)
        ),
        Ok(LinuxOptions {
            wine: Some("/usr/bin/wine64".into()),
            proton: Some("/opt/GE-Proton10/proton".into()),
            xvfb: true,
            firewall_interface: None
        })
    );
    assert!(lx(&["--wine", "nowine"], None).unwrap_err().contains("not found"));
    assert!(
        lx(&["--firewall-interface", "eth0; drop"], None)
            .unwrap_err()
            .contains("interface")
    );
}

#[test]
fn linux_options_are_in_host_json_on_linux_only() {
    let args = [
        "--mode",
        "public",
        "--host",
        "203.0.113.7",
        "--game",
        "/srv/dauntless/Dauntless",
    ];
    let linux = Mock {
        platform: Platform::Linux,
        tailscale: None,
        ..Mock::new()
    };
    assert_eq!(
        from_args(&args, None, &linux).unwrap().linux,
        Some(LinuxOptions {
            wine: None,
            proton: None,
            xvfb: true,
            firewall_interface: None
        })
    );
    assert_eq!(from_args(&args, None, &Mock::new()).unwrap().linux, None);
}

// ------------------------------------------------------------------ a fresh private and public setup

/// A checkout with what setup builds from (paradox/ and native/), and a game folder.
fn checkout() -> (tempfile::TempDir, Paths, PathBuf) {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("Reforged");
    let p = Paths::new(&root);
    let l = Layout::new(&p);
    for file in [
        l.backend_dir.join("package.json"),
        l.director_dir.join("package.json"),
        l.runtime_source.join("MysticParadox.vcxproj"),
        l.loader_dir.join("Cargo.toml"),
        l.exporter_dir.join("CatalogExporter.vcxproj"),
    ] {
        std::fs::create_dir_all(file.parent().unwrap()).unwrap();
        std::fs::write(file, "{}").unwrap();
    }
    let game = dir.path().join("Games").join("Dauntless");
    let exe = game.join(game_exe_relative());
    std::fs::create_dir_all(exe.parent().unwrap()).unwrap();
    std::fs::write(exe, "not really a game").unwrap();
    (dir, p, game)
}

fn setup(p: &Paths, args: &[&str], env: &Mock) -> (Result<std::process::ExitCode, String>, Vec<String>) {
    let mut lines = Vec::new();
    let r = run_with(p, &setup_args(args), env, &mut |l| lines.push(l.to_owned())).map_err(|e| e.to_string());
    (r, lines)
}

fn read(file: &Path) -> String {
    std::fs::read_to_string(file).unwrap()
}

#[test]
fn a_fresh_private_setup_then_again() {
    let (_dir, p, game) = checkout();
    let env = Mock::new();
    let game_arg = game.to_string_lossy().into_owned();
    let (r, lines) = setup(
        &p,
        &["--mode", "private", "--name", "Friday Hunts", "--game", &game_arg],
        &env,
    );
    assert!(r.is_ok(), "{r:?}\n{}", lines.join("\n"));
    let text = lines.join("\n");
    assert!(
        text.contains("\nPrivate server \"Friday Hunts\" at 100.101.102.103 (game servers on 100.101.102.103)"),
        "{text}"
    );
    assert!(
        text.contains("\n1. Mystic Paradox (paradox/ and native/ in this project)\n- in place"),
        "{text}"
    );
    assert!(text.contains("- backend: npm ci (mocked)"));
    assert!(text.contains("- secrets: made (kept from now on) in "));
    assert!(text.contains("- env files: backend.env, director.env, content.env, gateway.env in "));
    assert!(!text.contains("! No "), "the game folder is there: {text}");
    assert!(text.ends_with("  reforged-host doctor shows what is still missing."));
    assert_eq!(*env.installs.borrow(), 1);

    let config = read_host_config(&p.host_toml).unwrap().unwrap();
    assert_eq!(config.mode, ServerMode::Private);
    assert_eq!(config.game_dir.as_deref(), Some(game_arg.as_str()));
    assert!(!p.host_json.exists(), "host.toml only");
    let toml = read(&p.host_toml);
    assert!(toml.starts_with("# This server's settings (reforged-host)."), "{toml}");
    assert!(
        toml.contains("\nversion = 1\n") && toml.contains("\nmode = \"private\"\n"),
        "{toml}"
    );
    assert!(
        toml.contains("\nhost = \"100.101.102.103\"\n") && toml.contains("\n[mongo]\n"),
        "{toml}"
    );
    let backend = read_env_file(&p.config.join("backend.env")).unwrap();
    let secrets = read_env_file(&p.config.join("secrets.env")).unwrap();
    assert_eq!(backend.get("REFORGED_ADMIN_KEY"), secrets.get("REFORGED_ADMIN_KEY"));
    assert!(!p.config.join("allowlist.env").exists(), "no allowlist in private mode");
    let firewall = read(&p.config.join("firewall.ps1"));
    assert!(firewall.contains("'C:\\rr\\gateway.exe' '100.64.0.0/10'"), "{firewall}");
    assert_eq!(read(&p.news_file), "{\n  \"items\": []\n}\n");
    for dir in [
        &p.runtime_publish,
        &p.branding,
        &p.gameserver_logs,
        &p.mongo_data,
        &Layout::new(&p).raw_game_data,
    ] {
        assert!(dir.is_dir(), "{}", dir.display());
    }
    assert!(
        env.certificates.borrow().is_empty(),
        "no certificate for a private server"
    );

    // Again, with --skip-install: the secrets are kept, the options too.
    let before = read(&p.config.join("secrets.env"));
    let (r, lines) = setup(&p, &["--skip-install"], &env);
    assert!(r.is_ok(), "{r:?}");
    assert!(lines.contains(&"\n2. Skipped installing and building (--skip-install)".to_owned()));
    assert!(lines.iter().any(|l| l.starts_with("- secrets: kept in ")));
    assert_eq!(read(&p.config.join("secrets.env")), before);
    assert_eq!(read_host_config(&p.host_toml).unwrap().unwrap().name, "Friday Hunts");
}

#[test]
fn setup_moves_a_node_kits_host_json_over_to_host_toml() {
    let (_dir, p, game) = checkout();
    let env = Mock::new();
    let game_arg = game.to_string_lossy().into_owned();
    setup(
        &p,
        &["--mode", "private", "--name", "Friday Hunts", "--game", &game_arg],
        &env,
    )
    .0
    .unwrap();
    // As a Node kit leaves it: host.json only.
    let config = read_host_config(&p.host_toml).unwrap().unwrap();
    write_host_config(&p.host_json, &config).unwrap();
    let json = read(&p.host_json);
    std::fs::remove_file(&p.host_toml).unwrap();
    assert_eq!(
        load_host_config(&p).unwrap().unwrap().name,
        "Friday Hunts",
        "read while there is no host.toml"
    );

    let (r, lines) = setup(&p, &["--skip-install"], &env);
    assert!(r.is_ok(), "{r:?}");
    assert!(
        lines.contains(&"(host.json is now host.toml; the old file is kept as host.json.bak)".to_owned()),
        "{lines:?}"
    );
    assert!(!p.host_json.exists());
    assert_eq!(read(&p.config.join("host.json.bak")), json, "kept as it was");
    let moved = read_host_config(&p.host_toml).unwrap().unwrap();
    assert_eq!(moved.name, "Friday Hunts");
    assert_eq!(moved.game_dir.as_deref(), Some(game_arg.as_str()));

    // Once moved, setup says nothing more about it.
    let (_, lines) = setup(&p, &["--skip-install"], &env);
    assert!(!lines.iter().any(|l| l.contains("host.json.bak")));
}

#[test]
fn setup_keeps_the_hosts_own_comments_and_edits_in_host_toml() {
    let (_dir, p, _game) = checkout();
    let env = Mock::new();
    setup(
        &p,
        &["--mode", "private", "--name", "Friday Hunts", "--skip-install"],
        &env,
    )
    .0
    .unwrap();
    let edited = read(&p.host_toml).replace(
        "max_hunt_servers = 3\n",
        "# two hunts are plenty on this PC\nmax_hunt_servers = 2 # was 3\n",
    );
    assert_ne!(edited, read(&p.host_toml));
    std::fs::write(&p.host_toml, &edited).unwrap();
    setup(&p, &["--name", "Saturday Hunts", "--skip-install"], &env)
        .0
        .unwrap();
    assert_eq!(
        read(&p.host_toml),
        edited.replace("name = \"Friday Hunts\"", "name = \"Saturday Hunts\"")
    );
    assert_eq!(load_host_config(&p).unwrap().unwrap().max_hunt_servers, 2);
}

#[test]
fn a_fresh_public_setup_makes_the_certificate_once() {
    let (_dir, p, _game) = checkout();
    let env = Mock {
        mongod: None,
        ..Mock::new()
    };
    let (r, lines) = setup(
        &p,
        &[
            "--mode",
            "public",
            "--host",
            "hunts.example.net",
            "--game-address",
            "203.0.113.7",
        ],
        &env,
    );
    assert!(r.is_ok(), "{r:?}\n{}", lines.join("\n"));
    assert_eq!(*env.certificates.borrow(), [["hunts.example.net", "203.0.113.7"]]);
    let fp = host::invite::certificate_fingerprint(&p.cert_file).unwrap();
    let text = lines.join("\n");
    assert!(
        text.contains(&format!("- certificate fingerprint (goes into every invite): {fp}")),
        "{text}"
    );
    assert!(
        text.contains("\n! MongoDB was not found. Install MongoDB Community Server"),
        "{text}"
    );
    assert!(text.contains("\n! No game folder yet"), "{text}");
    assert!(p.config.join("allowlist.env").exists());
    assert!(
        read(&p.config.join("firewall.ps1"))
            .contains("'ReforgedRevival-Gateway' 'Dauntless Reforged Revival gateway (TCP 443)'")
    );
    let gateway = read_env_file(&p.config.join("gateway.env")).unwrap();
    assert_eq!(gateway.get("GATEWAY_PORT"), Some("443"));

    let (r, lines) = setup(&p, &["--skip-install"], &env);
    assert!(r.is_ok(), "{r:?}");
    assert_eq!(
        env.certificates.borrow().len(),
        1,
        "kept: a new one would change every invite"
    );
    assert!(lines.iter().any(|l| l.starts_with("- gateway certificate kept")));
    assert_eq!(host::invite::certificate_fingerprint(&p.cert_file).unwrap(), fp);
}

#[test]
fn setup_refuses_an_unquoted_value_and_an_incomplete_checkout() {
    let (_dir, p, _game) = checkout();
    let (r, _) = setup(
        &p,
        &["--mode", "private", "--game", "D:\\Games\\My", "Dauntless"],
        &Mock::new(),
    );
    assert!(
        r.unwrap_err()
            .starts_with("unexpected \"Dauntless\": put a value with spaces in quotes")
    );
    std::fs::remove_file(Layout::new(&p).loader_dir.join("Cargo.toml")).unwrap();
    let (r, _) = setup(&p, &["--mode", "private"], &Mock::new());
    assert!(
        r.unwrap_err()
            .starts_with("this checkout has no native/runtime-loader: clone the project again")
    );
}

// ------------------------------------------------------------------ multipliers

fn multipliers(p: &Paths, args: &[&str]) -> (Result<std::process::ExitCode, String>, Vec<String>) {
    let mut argv = vec!["reforged-host", "multipliers"];
    argv.extend_from_slice(args);
    let Command::Multipliers(m) = Cli::try_parse_from(argv).unwrap().command else {
        unreachable!()
    };
    let mut lines = Vec::new();
    let commit = |_: &Path| Ok("abc123def456".to_owned());
    let r = host::commands::multipliers::run_with(p, &m, Platform::Windows, &commit, &mut |l| lines.push(l.to_owned()));
    (r.map_err(|e| e.to_string()), lines)
}

#[test]
fn multipliers_change_only_the_multipliers_in_host_toml_and_the_backends_env() {
    let (_dir, p, _game) = checkout();
    let (r, lines) = multipliers(&p, &[]);
    assert_eq!(
        r.unwrap_err(),
        "not set up yet: run reforged-host setup first",
        "{lines:?}"
    );
    setup(&p, &["--mode", "private", "--skip-install"], &Mock::new())
        .0
        .unwrap();
    let before = read(&p.host_toml);

    let (r, lines) = multipliers(&p, &[]);
    assert!(r.is_ok());
    assert_eq!(lines, ["Test multipliers off."], "nothing asked: only says");

    let (r, lines) = multipliers(&p, &["--xp", "5", "--rewards", "x2.5"]);
    assert!(r.is_ok(), "{r:?}");
    assert_eq!(lines[0], "Test multipliers ON: XP x5, rewards x2.5.");
    assert!(lines[1].starts_with("Restart the server"));
    let after = read(&p.host_toml);
    // A new [multipliers] table at the end; everything else stays as it was.
    assert_eq!(after, format!("{before}\n[multipliers]\nxp = 5\nrewards = 2.5\n"));
    let backend = read_env_file(&p.config.join("backend.env")).unwrap();
    assert!(
        backend.iter().any(|(k, v)| k.contains("MULTIPLIER") && v == "5"),
        "{backend:?}"
    );

    let (_, lines) = multipliers(&p, &["--xp-multiplier", "500"]);
    assert!(lines.is_empty());
    assert_eq!(
        multipliers(&p, &["--xp-multiplier", "500"]).0.unwrap_err(),
        "--xp must be a number from 1 to 100 (got 500)"
    );
    multipliers(&p, &["--off"]).0.unwrap();
    assert_eq!(read(&p.host_toml), before, "--off: the file as it was");
}

#[test]
fn multipliers_args_default_is_nothing_asked() {
    assert!(!host::commands::multipliers::asked(&MultipliersArgs::default()));
    assert!(!host::commands::multipliers::asked(&MultipliersArgs {
        no_multipliers: true,
        ..Default::default()
    }));
}
