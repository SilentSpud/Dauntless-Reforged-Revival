//! `reforged-host start`: the Node kit's `start()` and `watchStopInput()` in `cli.ts`, with
//! [`crate::services`].
//!
//! Every process of the server, in order, each with its own env file and log, restarted when it
//! stops unexpectedly, each in a process tree of its own ([`crate::services::ProcessTree`]). Ctrl+C
//! (or the launcher's `stop` line) stops them all, the last started first, the Director's game
//! servers included, and nothing is left running.
//!
//! It keeps the launcher's control contract (RD23): `REFORGED_CONTROL=stdin`, the
//! `REFORGED_SERVER_READY` line once every service is up, and the backend on port 61000, which the
//! launcher probes ([`control`]).

pub mod binaries;
pub mod control;

use std::path::Path;
use std::process::ExitCode;
use std::sync::Arc;
use std::time::Duration;

use crate::Outcome;
use crate::error::HostError;
use crate::hostconfig::{HostConfig, ServerMode, gateway_port, load_host_config, read_env_file};
use crate::paths::{PORTS, Paths, game_exe_relative};
use crate::services::{Printer, Ready, ServiceSpec, Supervisor, wait_until_ready};
use binaries::{FrontService, Launch, Programs};
use control::{Control, SERVER_READY_LINE, controlled_by_stdin};

fn say(line: &str) {
    println!("{line}");
}

/// The console's `name     | line` (the name padded to 9, as the Node kit's).
pub fn console_printer() -> Printer {
    Arc::new(|name: &str, line: &str| println!("{name:<9}| {line}"))
}

/// One service `start` runs, in order.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Planned {
    pub spec: ServiceSpec,
    pub ready_timeout: Duration,
    /// The server does not run without it: when it does not come up, `start` stops.
    pub required: bool,
}

/// The env file's path, for the services' specs.
fn env_file(paths: &Paths, name: &str) -> std::path::PathBuf {
    paths.config.join(format!("{name}.env"))
}

fn spec_from(name: &str, launch: Launch, env_file: std::path::PathBuf, ready: Ready) -> ServiceSpec {
    ServiceSpec {
        name: name.into(),
        cwd: launch.cwd,
        command: launch.command,
        args: launch.args,
        env_file: (!launch.env_file_in_args).then_some(env_file),
        extra_env: Vec::new(),
        ready: Some(ready),
    }
}

/// The services after the database, in the order they start: backend, Director, content, the
/// allowlist helper (public, unless systemd runs it), the gateway last.
pub fn service_plan(paths: &Paths, config: &HostConfig, external_allowlist: bool, programs: &Programs) -> Vec<Planned> {
    let node = |name: &str, dir: &Path, script: &[&str], ready: Ready| ServiceSpec {
        name: name.into(),
        cwd: dir.to_path_buf(),
        command: programs.node.clone(),
        args: vec![script.iter().collect::<std::path::PathBuf>().display().to_string()],
        env_file: Some(env_file(paths, name)),
        extra_env: Vec::new(),
        ready: Some(ready),
    };
    let minute = Duration::from_secs(60);
    let mut plan = vec![
        Planned {
            spec: node(
                "backend",
                &paths.backend_dir,
                &["build", "server.js"],
                Ready::http(PORTS.backend, "/dauntless-status"),
            ),
            ready_timeout: Duration::from_secs(120),
            required: true,
        },
        Planned {
            spec: node(
                "director",
                &paths.director_dir,
                &["dist", "server.js"],
                Ready::http(PORTS.director, "/api/instances"),
            ),
            ready_timeout: minute,
            required: false,
        },
        Planned {
            spec: spec_from(
                "content",
                programs.launch(FrontService::Content, paths, &env_file(paths, "content")),
                env_file(paths, "content"),
                Ready::http(PORTS.content, "/content/v1/news"),
            ),
            ready_timeout: minute,
            required: false,
        },
    ];
    if config.mode == ServerMode::Public && !external_allowlist {
        plan.push(Planned {
            spec: spec_from(
                "allowlist",
                programs.launch(FrontService::Allowlist, paths, &env_file(paths, "allowlist")),
                env_file(paths, "allowlist"),
                Ready::http(PORTS.allowlist, "/status"),
            ),
            ready_timeout: minute,
            required: false,
        });
    }
    let gateway_ready = match config.mode {
        ServerMode::Public => Ready::tcp("127.0.0.1", gateway_port(config)),
        ServerMode::Private => Ready::tcp(&config.game_address, PORTS.private_gateway),
    };
    plan.push(Planned {
        spec: spec_from(
            "gateway",
            programs.launch(FrontService::Gateway, paths, &env_file(paths, "gateway")),
            env_file(paths, "gateway"),
            gateway_ready,
        ),
        ready_timeout: minute,
        required: true,
    });
    plan
}

/// How far starting got.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Started {
    /// Every service was started (and the required ones answer).
    Up,
    /// A stop was asked for while starting.
    Stopped,
    /// A required service did not come up.
    Failed { name: String },
}

/// Starts the services in order. `port_taken` says whether another program already holds a
/// service's port: readiness is "something answers on the port", so a taken port would pass it
/// while the service fails to listen (a hosting panel on 443 then answers players with its own
/// certificate, and every invite fails), and a taken port stops the start here.
pub fn start_services(
    supervisor: &Supervisor,
    control: &Control,
    plan: Vec<Planned>,
    config: Option<&HostConfig>,
    port_taken: &dyn Fn(&Ready) -> Result<bool, HostError>,
) -> Result<Started, HostError> {
    for planned in plan {
        if control.asked() {
            return Ok(Started::Stopped);
        }
        let name = planned.spec.name.clone();
        if let Some(ready) = &planned.spec.ready
            && port_taken(ready)?
        {
            let public = config.is_some_and(|c| c.mode == ServerMode::Public);
            let movable = if name == "gateway" && public {
                ", or give the gateway a free port: reforged-host setup --public-port 4443 (then new invites)"
            } else {
                ""
            };
            return Err(HostError::message(format!(
                "port {} ({name}) is already used by another program. {}, then stop that program (or a server still running from before) and start again{movable}",
                ready.port(),
                port_owner_hint(ready.port())
            )));
        }
        let ok = supervisor.start(planned.spec, planned.ready_timeout)?;
        if control.asked() {
            return Ok(Started::Stopped);
        }
        if !ok && planned.required {
            return Ok(Started::Failed { name });
        }
    }
    Ok(Started::Up)
}

fn port_owner_hint(port: u16) -> String {
    crate::doctor::port_owner_hint(port).unwrap_or_else(|_| {
        if cfg!(windows) {
            format!("See which program it is with: netstat -ano | findstr :{port}")
        } else {
            format!("See which program it is with: ss -tlnp 'sport = :{port}'")
        }
    })
}

/// After starting: says the server is up (and, for the launcher, [`SERVER_READY_LINE`]), waits for
/// a stop, then stops everything and runs `after_stop` (closing the game ports).
pub fn finish(
    started: Started,
    supervisor: &Supervisor,
    control: &Control,
    controlled: bool,
    up_line: &str,
    after_stop: impl FnOnce(),
) -> Outcome {
    match started {
        Started::Up => {
            say("");
            say(up_line);
            if controlled {
                say(SERVER_READY_LINE);
            } else {
                say("Make an invite in another terminal: reforged-host invite --for \"Alex\"");
                say("Ctrl+C stops everything.");
            }
            control.wait();
        }
        Started::Stopped => {}
        Started::Failed { name } => {
            say(&format!(
                "\n{name} did not come up. Its log: {}",
                supervisor.log_dir().join(format!("{name}.log")).display()
            ));
        }
    }
    say("\nStopping...");
    supervisor.stop_all();
    after_stop();
    say("Stopped.");
    Ok(ExitCode::SUCCESS)
}

fn is_windows_admin() -> bool {
    if !cfg!(windows) {
        return true;
    }
    let mut net = std::process::Command::new("net");
    net.arg("session")
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null());
    net.status().is_ok_and(|s| s.success())
}

const NOT_AS_ROOT: &str = "do not run the server as root. Make a user for it (sudo adduser reforged), sign in as that user (sudo -iu reforged) and run this there; docs/hosting-linux.md shows every step";

/// The checks before anything starts, as the Node kit's; returns the server's settings (host.toml).
fn preflight(paths: &Paths, external_allowlist: bool) -> Result<HostConfig, HostError> {
    let config =
        load_host_config(paths)?.ok_or_else(|| HostError::message("not set up yet: run reforged-host setup first"))?;
    for name in ["backend", "director", "content", "gateway"] {
        if !env_file(paths, name).exists() {
            return Err(HostError::message(format!(
                "{name}.env is missing: run reforged-host setup"
            )));
        }
    }
    let public = config.mode == ServerMode::Public;
    if cfg!(target_os = "linux") {
        // Linux, public: the reforged-allowlist systemd unit runs the allowlist (with CAP_NET_ADMIN), and the
        // reforged unit gives this process CAP_NET_BIND_SERVICE for a gateway port below 1024 (443).
        if crate::linux::is_root()? {
            return Err(HostError::message(NOT_AS_ROOT));
        }
        if !paths.gameserver_wrapper.exists() {
            return Err(HostError::message(
                "the game-server launcher is missing: run reforged-host setup",
            ));
        }
        let as_service = format!(
            "run it as a systemd service: sudo sh {} (then journalctl -u {} -f shows it)",
            paths.systemd.join("install.sh").display(),
            crate::linux::UNIT_SERVER
        );
        if public && !crate::linux::can_bind_port(gateway_port(&config))? {
            return Err(HostError::message(format!(
                "a public server listens on port {}, which needs a privilege: {as_service}",
                gateway_port(&config)
            )));
        }
        if public && !external_allowlist && !crate::linux::can_change_firewall()? {
            return Err(HostError::message(format!(
                "a public server's allowlist changes the firewall (nftables), which needs a privilege: {as_service}"
            )));
        }
    } else if public && !is_windows_admin() {
        return Err(HostError::message(format!(
            "a public server changes Windows Firewall rules (the allowlist helper) and listens on port {}: run this terminal as administrator",
            gateway_port(&config)
        )));
    }
    // The gateway and the game servers listen on the Tailscale address: one that changed (another
    // Tailscale account) would only fail later, with a bind error.
    if config.mode == ServerMode::Private
        && let Some(now) = crate::doctor::tailscale_ipv4()?
        && now != config.game_address
    {
        return Err(HostError::message(format!(
            "this PC's Tailscale address changed from {} to {now} (a new Tailscale account?). Run setup again (the launcher: Host > Server > Update server; a terminal: reforged-host setup), then make new invites: the old ones name the old address",
            config.game_address
        )));
    }
    use crate::gamedata::{BACKEND_GAME_DATA, DIRECTOR_GAME_DATA, GameDataKind, game_data_state};
    let game_data = [
        game_data_state(&paths.backend_game_data, &BACKEND_GAME_DATA)?,
        game_data_state(&paths.director_game_data, &DIRECTOR_GAME_DATA)?,
    ];
    if game_data.iter().any(|g| g.state == GameDataKind::Missing) {
        return Err(HostError::message(
            "game data is missing: reforged-host gamedata (or reforged-host gamedata --examples for a start-up test only)",
        ));
    }
    if game_data.iter().any(|g| g.state == GameDataKind::Synthetic) {
        say(
            "! This server runs on example game data: it starts, but it is not playable. The launcher tells players so.",
        );
    }
    if !paths.runtime_publish.join("runtime.json").exists() {
        say(
            "! No runtime published yet: players can join but not play. Build it and run reforged-host publish-runtime.",
        );
    }
    if let Some(game_dir) = &config.game_dir
        && let Some(win64) = Path::new(game_dir).join(game_exe_relative()).parent()
        && win64.exists()
    {
        let flags = crate::publish::ensure_server_flags(win64)?;
        if !flags.is_empty() {
            say(&format!(
                "Turned on for the game servers: {} (in {})",
                flags.join(", "),
                win64.display()
            ));
        }
    }
    Ok(config)
}

fn port_in_use(ready: &Ready) -> Result<bool, HostError> {
    crate::doctor::port_in_use(ready.port(), Some(ready.host()))
}

/// The managed MongoDB (until RB14): started unless one with a replica set already runs, then the
/// replica set checked and initiated when new. `Ok(false)`: a stop was asked for meanwhile.
fn start_database(
    paths: &Paths,
    config: &HostConfig,
    supervisor: &Supervisor,
    control: &Control,
) -> Result<bool, HostError> {
    use crate::mongo::{MongoCheck, MongoProblem, ReplicaSetOptions, ensure_replica_set, find_mongod, mongod_args};
    if config.mongo.manage {
        let mongod = match &config.mongo.mongod {
            Some(mongod) => mongod.clone(),
            None => find_mongod()?.ok_or_else(|| {
                HostError::message(
                    "MongoDB (mongod) was not found: install MongoDB Community Server, or run setup with --mongod <path>",
                )
            })?,
        };
        if crate::doctor::port_in_use(PORTS.mongo, None)? {
            // A mongod left running by an earlier start is a replica set and can be used. The Windows
            // service the MongoDB installer adds by default is not, and the backend needs one.
            let options = ReplicaSetOptions {
                initiate: false,
                timeout_ms: Some(4000),
                ..Default::default()
            };
            match ensure_replica_set(&config.mongo.uri, &options)? {
                MongoCheck::Ok { set_name, .. } => {
                    say(&format!(
                        "- MongoDB is already running on port {} (replica set {set_name}): using it",
                        PORTS.mongo
                    ));
                }
                MongoCheck::Failed { reason, .. } => {
                    let standalone = if reason == MongoProblem::Standalone {
                        " by a MongoDB without a replica set, usually the MongoDB Windows service"
                    } else {
                        ""
                    };
                    return Err(HostError::message(format!(
                        "port {} is already taken{standalone}. The host kit runs its own MongoDB: stop the other one first. For the Windows service, in PowerShell as administrator: Stop-Service MongoDB; Set-Service MongoDB -StartupType Disabled",
                        PORTS.mongo
                    )));
                }
            }
        } else {
            std::fs::create_dir_all(&paths.mongo_data).map_err(|e| HostError::io(&paths.mongo_data, e))?;
            let args = mongod_args(&paths.mongo_data, &paths.logs.join("mongod-server.log"), PORTS.mongo)?;
            supervisor.start(
                ServiceSpec {
                    name: "mongod".into(),
                    cwd: paths.workspace.clone(),
                    command: mongod.into(),
                    args,
                    env_file: None,
                    extra_env: Vec::new(),
                    ready: Some(Ready::tcp("127.0.0.1", PORTS.mongo)),
                },
                Duration::from_secs(60),
            )?;
            if control.asked() {
                return Ok(false);
            }
        }
    }
    let backend_env = read_env_file(&env_file(paths, "backend"))?;
    let uri = backend_env.get("MONGODB_URI").unwrap_or(&config.mongo.uri);
    let options = ReplicaSetOptions {
        initiate: true,
        timeout_ms: Some(8000),
        ..Default::default()
    };
    match ensure_replica_set(uri, &options)? {
        MongoCheck::Ok {
            set_name, initiated, ..
        } => {
            if initiated {
                say(&format!("mongod   | replica set {set_name} initiated"));
            }
            Ok(true)
        }
        MongoCheck::Failed { detail, .. } => Err(HostError::message(format!("MongoDB: {detail}"))),
    }
}

/// Runs the allowlist helper's `--close-ports` once: a hard stop never lets the helper close the game
/// ports itself, and Linux has no firewall closing them before the first start.
fn close_game_ports(paths: &Paths, programs: &Programs) -> crate::run::RunResult {
    let file = env_file(paths, "allowlist");
    let launch = programs.launch(FrontService::Allowlist, paths, &file);
    let mut env: Vec<(String, String)> = std::env::vars().collect();
    if !launch.env_file_in_args
        && let Ok(from_file) = read_env_file(&file)
    {
        for (k, v) in from_file.iter() {
            match env.iter_mut().find(|(key, _)| key == k) {
                Some(slot) => slot.1 = v.to_string(),
                None => env.push((k.to_string(), v.to_string())),
            }
        }
    }
    let mut args: Vec<&str> = launch.args.iter().map(String::as_str).collect();
    args.push("--close-ports");
    let options = crate::run::RunOptions {
        cwd: launch.cwd.clone(),
        env: Some(env),
        log_file: Some(paths.logs.join("allowlist.log")),
        quiet: true,
    };
    let command = launch.command.display().to_string();
    crate::run::run(&command, &args, &options).unwrap_or_else(|e| crate::run::RunResult {
        code: -1,
        last_line: Some(e.to_string()),
        output: String::new(),
    })
}

/// Linux: the Wine prefix, and in public mode the game ports closed before any game server starts.
/// `Ok(false)`: a stop was asked for meanwhile.
fn linux_steps(
    paths: &Paths,
    config: &HostConfig,
    programs: &Programs,
    external_allowlist: bool,
    control: &Control,
) -> Result<bool, HostError> {
    use crate::linux::{RuntimeKind, UNIT_ALLOWLIST, ensure_wine_prefix, game_runtime};
    let options = crate::hostconfig::linux_options(config);
    match game_runtime(&options)? {
        Some(runtime) if runtime.kind == RuntimeKind::Wine => {
            let prefix = ensure_wine_prefix(paths, &options, &paths.logs.join("wine.log"))?;
            if prefix.created {
                say("wine     | Wine prefix made");
            }
            if !prefix.ok {
                say(&format!("wine     | ! {}", prefix.detail.unwrap_or_default()));
            }
        }
        Some(_) => {}
        None => say("wine     | ! Wine was not found: the game servers cannot start (sudo apt install wine wine64)"),
    }
    if config.mode == ServerMode::Public {
        if external_allowlist {
            // Its table must be in place (game ports closed) before the Director starts any game server.
            let flag = control.flag();
            let cancelled = move || flag.load(std::sync::atomic::Ordering::SeqCst);
            if !wait_until_ready(
                &Ready::http(PORTS.allowlist, "/status"),
                Duration::from_secs(30),
                &cancelled,
            ) {
                if control.asked() {
                    return Ok(false);
                }
                return Err(HostError::message(format!(
                    "the allowlist ({UNIT_ALLOWLIST}) is not answering on port {}: systemctl status {UNIT_ALLOWLIST}",
                    PORTS.allowlist
                )));
            }
            say("allowlist| the reforged-allowlist service is running");
        } else {
            let closed = close_game_ports(paths, programs);
            if closed.code != 0 {
                return Err(HostError::message(format!(
                    "could not close the game ports (nftables): {}",
                    closed.last_line.unwrap_or_default()
                )));
            }
        }
    }
    Ok(true)
}

fn up_line(config: &HostConfig) -> String {
    match config.mode {
        ServerMode::Public => format!(
            "Server \"{}\" is up: https://{}:{} (pinned TLS)",
            config.name,
            config.host,
            gateway_port(config)
        ),
        ServerMode::Private => format!(
            "Server \"{}\" is up: http://{}:{} inside your tailnet",
            config.name, config.host, PORTS.private_gateway
        ),
    }
}

pub fn run(paths: &Paths) -> Outcome {
    let external_allowlist =
        cfg!(target_os = "linux") && std::env::var("REFORGED_ALLOWLIST").is_ok_and(|v| v == "external");
    let config = preflight(paths, external_allowlist)?;
    let external_allowlist = external_allowlist && config.mode == ServerMode::Public;
    let programs = Programs::find(paths)?;
    let controlled = controlled_by_stdin();
    let control = Control::begin(controlled)?;
    let supervisor = Supervisor::new(&paths.logs, console_printer()).with_cancel(control.flag());

    // MongoDB may be running by now: stop it before giving up (the supervisor stops what it
    // started when it is dropped, and each tree goes with this process in any case).
    let started = (|| {
        if !start_database(paths, &config, &supervisor, &control)? {
            return Ok(Started::Stopped);
        }
        if cfg!(target_os = "linux") && !linux_steps(paths, &config, &programs, external_allowlist, &control)? {
            return Ok(Started::Stopped);
        }
        let plan = service_plan(paths, &config, external_allowlist, &programs);
        start_services(&supervisor, &control, plan, Some(&config), &port_in_use)
    })();
    let started = match started {
        Ok(started) => started,
        Err(error) => {
            supervisor.stop_all();
            return Err(error);
        }
    };
    let close_ports = config.mode == ServerMode::Public && !external_allowlist;
    finish(started, &supervisor, &control, controlled, &up_line(&config), || {
        if close_ports {
            close_game_ports(paths, &programs);
        }
    })
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::*;
    use crate::hostconfig::{DEFAULT_MONGO_URI, FileForm, MongoConfig, Registration};

    fn config(mode: ServerMode) -> HostConfig {
        HostConfig {
            version: 1,
            mode,
            host: "100.101.102.103".into(),
            game_address: "100.101.102.103".into(),
            name: "Friday Hunts".into(),
            game_dir: None,
            registration: Registration::parse("INVITECODE").unwrap(),
            share_url: None,
            mongo: MongoConfig {
                uri: DEFAULT_MONGO_URI.into(),
                manage: true,
                mongod: None,
            },
            max_hunt_servers: 3,
            multipliers: None,
            public_port: None,
            performance: None,
            linux: None,
            file: FileForm::default(),
        }
    }

    fn programs(dir: &Path) -> Programs {
        Programs {
            node: PathBuf::from("node"),
            rust_dirs: vec![dir.join("bin")],
        }
    }

    fn names(plan: &[Planned]) -> Vec<&str> {
        plan.iter().map(|p| p.spec.name.as_str()).collect()
    }

    #[test]
    fn starts_a_private_server_backend_first_gateway_last_each_with_its_env_file() {
        let dir = tempfile::tempdir().unwrap();
        let paths = Paths::new(dir.path());
        let c = config(ServerMode::Private);
        let plan = service_plan(&paths, &c, false, &programs(dir.path()));
        assert_eq!(names(&plan), ["backend", "director", "content", "gateway"]);
        for p in &plan {
            assert_eq!(p.spec.env_file, Some(paths.config.join(format!("{}.env", p.spec.name))));
            assert_eq!(p.spec.command, PathBuf::from("node"));
        }
        let backend = &plan[0];
        assert_eq!(backend.spec.cwd, paths.backend_dir);
        assert_eq!(
            backend.spec.args,
            [Path::new("build").join("server.js").display().to_string()]
        );
        assert_eq!(backend.spec.ready, Some(Ready::http(61000, "/dauntless-status")));
        assert_eq!(backend.ready_timeout, Duration::from_secs(120));
        assert!(backend.required);
        assert_eq!(plan[1].spec.cwd, paths.director_dir);
        assert_eq!(plan[1].spec.ready, Some(Ready::http(61001, "/api/instances")));
        assert!(!plan[1].required && !plan[2].required);
        assert_eq!(plan[2].spec.ready, Some(Ready::http(61002, "/content/v1/news")));
        let gateway = &plan[3];
        assert_eq!(gateway.spec.ready, Some(Ready::tcp("100.101.102.103", 61000)));
        assert!(gateway.required);
    }

    #[test]
    fn a_public_server_adds_the_allowlist_before_the_gateway_unless_systemd_runs_it() {
        let dir = tempfile::tempdir().unwrap();
        let paths = Paths::new(dir.path());
        let mut c = config(ServerMode::Public);
        c.public_port = Some(4443);
        let plan = service_plan(&paths, &c, false, &programs(dir.path()));
        assert_eq!(names(&plan), ["backend", "director", "content", "allowlist", "gateway"]);
        assert_eq!(plan[3].spec.ready, Some(Ready::http(61005, "/status")));
        assert_eq!(plan[4].spec.ready, Some(Ready::tcp("127.0.0.1", 4443)));
        let external = service_plan(&paths, &c, true, &programs(dir.path()));
        assert_eq!(names(&external), ["backend", "director", "content", "gateway"]);
    }

    #[test]
    fn gives_a_rust_allowlist_its_env_file_as_an_argument() {
        let dir = tempfile::tempdir().unwrap();
        let paths = Paths::new(dir.path());
        std::fs::create_dir_all(dir.path().join("bin")).unwrap();
        let bin = dir
            .path()
            .join("bin")
            .join(format!("allowlist{}", std::env::consts::EXE_SUFFIX));
        std::fs::write(&bin, "").unwrap();
        let plan = service_plan(&paths, &config(ServerMode::Public), false, &programs(dir.path()));
        let allowlist = &plan[3].spec;
        assert_eq!(allowlist.command, bin);
        let file = paths.config.join("allowlist.env").display().to_string();
        assert_eq!(allowlist.args, ["--env-file".to_string(), file]);
        assert_eq!(allowlist.env_file, None, "a Rust bin reads its file itself");
    }

    #[test]
    fn says_where_the_server_is() {
        let mut c = config(ServerMode::Public);
        c.host = "play.example.org".into();
        assert_eq!(
            up_line(&c),
            "Server \"Friday Hunts\" is up: https://play.example.org:443 (pinned TLS)"
        );
        assert_eq!(
            up_line(&config(ServerMode::Private)),
            "Server \"Friday Hunts\" is up: http://100.101.102.103:61000 inside your tailnet"
        );
    }

    #[test]
    fn stops_before_a_taken_port_and_names_it() {
        let dir = tempfile::tempdir().unwrap();
        let supervisor = Supervisor::new(dir.path().join("logs"), console_printer());
        let control = Control::default();
        let paths = Paths::new(dir.path());
        let mut c = config(ServerMode::Public);
        c.public_port = Some(443);
        let plan: Vec<Planned> = service_plan(&paths, &c, false, &programs(dir.path()))
            .into_iter()
            .filter(|p| p.spec.name == "gateway")
            .collect();
        let error = start_services(&supervisor, &control, plan, Some(&c), &|_| Ok(true)).unwrap_err();
        let text = error.to_string();
        assert!(
            text.starts_with("port 443 (gateway) is already used by another program."),
            "{text}"
        );
        assert!(text.ends_with("--public-port 4443 (then new invites)"), "{text}");
    }

    #[test]
    fn starts_nothing_once_a_stop_was_asked_for() {
        let dir = tempfile::tempdir().unwrap();
        let supervisor = Supervisor::new(dir.path().join("logs"), console_printer());
        let control = Control::default();
        control.stop(control::StopReason::Input);
        let paths = Paths::new(dir.path());
        let plan = service_plan(&paths, &config(ServerMode::Private), false, &programs(dir.path()));
        let started = start_services(&supervisor, &control, plan, None, &|_| panic!("no port is checked")).unwrap();
        assert_eq!(started, Started::Stopped);
        assert!(supervisor.all_stopped());
    }
}
