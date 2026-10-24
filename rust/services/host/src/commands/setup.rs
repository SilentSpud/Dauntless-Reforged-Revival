//! `reforged-host setup`: the Node kit's `setup()`, `linuxSetup()`, `firewallScript()`,
//! `hostConfigFromArgs()` and the `*FromArgs` option readers in `cli.ts`.
//!
//! It builds the server's settings in setup's key order and writes them to host.toml ([`options`]), checks Mystic Paradox in `paradox/`
//! and `native/` ([`paradox`]), installs and builds the services ([`install`], RH5's seam for the
//! server release), writes the secrets ([`secrets`]), the env files and the firewall rules
//! ([`firewall`]), makes the public gateway's certificate and says what is still missing (the
//! database: [`database`], RB14's seam for Postgres).
//!
//! What it asks of this PC (Tailscale, mongod, git, npm, the certificate maker) goes through
//! [`SetupEnv`], so a test runs the whole command with those mocked. Hints name `reforged-host`
//! commands where the Node kit named `npm run` scripts; the files it writes are the Node kit's.

pub mod database;
pub mod firewall;
pub mod install;
pub mod options;
pub mod paradox;
pub mod secrets;

use std::path::{Path, PathBuf};
use std::process::ExitCode;

use crate::Outcome;
use crate::cli::SetupArgs;
use crate::error::HostError;
use crate::hostconfig::{
    HostConfig, Platform, ServerMode, build_env_files, linux_options, load_host_config, load_or_create_secrets,
    multipliers_on, save_host_config, write_env_files,
};
use crate::paths::{Paths, game_exe_relative};
use crate::preset::PresetApplied;
use options::number_text;
use paradox::Layout;

/// What setup needs from this PC beyond the checkout's files.
pub trait SetupEnv {
    fn platform(&self) -> Platform;
    /// Linux: refuses to set up as root.
    fn is_root(&self) -> Result<bool, HostError>;
    fn tailscale_ipv4(&self) -> Result<Option<String>, HostError>;
    fn find_mongod(&self) -> Result<Option<String>, HostError>;
    fn find_on_path(&self, name: &str) -> Result<Option<String>, HostError>;
    /// `git rev-parse --short=12 HEAD`, or "" outside a git checkout.
    fn source_commit(&self, repo: &Path) -> Result<String, HostError>;
    fn apply_preset(&self, paths: &Paths) -> Result<Option<PresetApplied>, HostError>;
    /// Step 2: the services' code (see [`install`]).
    fn install_services(&self, layout: &Layout, log_file: &Path, out: &mut dyn FnMut(&str)) -> Result<(), HostError>;
    /// The public gateway's self-signed certificate and key, into `tls` (`make-cert --out`).
    fn make_certificate(&self, layout: &Layout, hosts: &[String], tls: &Path, log_file: &Path)
    -> Result<(), HostError>;
    /// The program that listens on the gateway's port, for the firewall rules.
    fn gateway_program(&self) -> String;
}

/// This PC.
pub struct System;

impl SetupEnv for System {
    fn platform(&self) -> Platform {
        Platform::current()
    }

    fn is_root(&self) -> Result<bool, HostError> {
        crate::linux::is_root()
    }

    fn tailscale_ipv4(&self) -> Result<Option<String>, HostError> {
        crate::doctor::tailscale_ipv4()
    }

    fn find_mongod(&self) -> Result<Option<String>, HostError> {
        crate::mongo::find_mongod()
    }

    fn find_on_path(&self, name: &str) -> Result<Option<String>, HostError> {
        crate::wine::find_on_path(&[name])
    }

    fn source_commit(&self, repo: &Path) -> Result<String, HostError> {
        // A server release names its commit; a checkout asks git.
        if let Some(release) = crate::selfupdate::read_manifest(repo)? {
            return Ok(release.short_commit().to_owned());
        }
        Ok(crate::run::capture("git", &["rev-parse", "--short=12", "HEAD"], Some(repo))?.unwrap_or_default())
    }

    fn apply_preset(&self, paths: &Paths) -> Result<Option<PresetApplied>, HostError> {
        crate::preset::apply_preset(paths, false)
    }

    fn install_services(&self, layout: &Layout, log_file: &Path, out: &mut dyn FnMut(&str)) -> Result<(), HostError> {
        install::install_services(layout, log_file, out)
    }

    fn make_certificate(
        &self,
        layout: &Layout,
        hosts: &[String],
        tls: &Path,
        log_file: &Path,
    ) -> Result<(), HostError> {
        // The Node gateway's tool, as today (it needs the gateway's packages, which step 2 installs).
        let tool = layout.gateway_dir.join("tools").join("make-cert.js");
        let mut args = vec![tool.to_string_lossy().into_owned()];
        for host in hosts {
            args.extend(["--host".to_owned(), host.clone()]);
        }
        args.extend([
            "--out".to_owned(),
            tls.to_string_lossy().into_owned(),
            "--json".to_owned(),
        ]);
        let args: Vec<&str> = args.iter().map(String::as_str).collect();
        let r = crate::run::run(
            "node",
            &args,
            &crate::run::RunOptions {
                cwd: layout.gateway_dir.clone(),
                env: None,
                log_file: Some(log_file.to_path_buf()),
                quiet: true,
            },
        )?;
        if r.code != 0 {
            return Err(HostError::message(format!(
                "could not make the gateway certificate: {}",
                r.last_line.unwrap_or_else(|| "undefined".into())
            )));
        }
        Ok(())
    }

    fn gateway_program(&self) -> String {
        // The gateway binary next to this one (a build, or the server release, RD8).
        let name = if cfg!(windows) { "gateway.exe" } else { "gateway" };
        std::env::current_exe()
            .ok()
            .and_then(|exe| exe.parent().map(|dir| dir.join(name)))
            .map_or_else(|| name.to_owned(), |p| p.to_string_lossy().into_owned())
    }
}

/// Linux: the server runs as an ordinary user. The game servers face the internet, and Wine refuses a
/// prefix another user owns; the systemd units give the gateway and the allowlist the one privilege
/// each needs.
pub const NOT_AS_ROOT: &str = "do not run the server as root. Make a user for it (sudo adduser reforged), sign in as that user (sudo -iu reforged) and run this there; docs/hosting-linux.md shows every step";

pub fn run(paths: &Paths, args: &SetupArgs) -> Outcome {
    run_with(paths, args, &System, &mut |line| println!("{line}"))
}

fn mkdir(dir: &Path) -> Result<(), HostError> {
    std::fs::create_dir_all(dir).map_err(|e| HostError::io(dir, e))
}

fn write(file: &Path, text: &str) -> Result<(), HostError> {
    std::fs::write(file, text).map_err(|e| HostError::io(file, e))
}

fn is_unknown_word(args: &SetupArgs) -> Option<String> {
    (!args.unexpected.is_empty()).then(|| args.unexpected.join(" "))
}

/// Setup with this PC's tools behind `env`, printing through `out`.
pub fn run_with(paths: &Paths, args: &SetupArgs, env: &dyn SetupEnv, out: &mut dyn FnMut(&str)) -> Outcome {
    let p = paths;
    let layout = Layout::new(p);
    let linux = env.platform() == Platform::Linux;
    // A value with spaces that was not quoted arrives in pieces (--game D:\Games\My, then Dauntless).
    if let Some(words) = is_unknown_word(args) {
        return Err(HostError::message(format!(
            "unexpected \"{words}\": put a value with spaces in quotes, for example --game \"D:\\Games\\Dauntless 1.14.7\\Dauntless\""
        )));
    }
    if linux && env.is_root()? {
        return Err(HostError::message(NOT_AS_ROOT));
    }
    let log_file = p.logs.join("setup.log");
    mkdir(&p.logs)?;

    let existing = match load_host_config(p) {
        Ok(c) => c,
        Err(e) => {
            out(&format!("(ignoring the old settings: {e})"));
            None
        }
    };
    let config = options::host_config_from_args(args, existing.as_ref(), env)?.written_over(existing.as_ref());
    if save_host_config(p, &config)? {
        out("(host.json is now host.toml; the old file is kept as host.json.bak)");
    }
    let config = load_host_config(p)?.ok_or_else(|| HostError::message("host.toml was not written"))?;
    out(&format!(
        "\n{} server \"{}\" at {} (game servers on {})",
        if config.mode == ServerMode::Public {
            "Public"
        } else {
            "Private"
        },
        config.name,
        config.host,
        config.game_address
    ));
    if let Some(m) = config.multipliers.filter(|_| multipliers_on(&config)) {
        out(&format!(
            "Test multipliers ON: XP x{}, rewards x{} (reforged-host multipliers --off turns them off)",
            number_text(m.xp),
            number_text(m.rewards)
        ));
    }
    out(&options::performance_summary(&config));

    out("\n1. Mystic Paradox (paradox/ and native/ in this project)");
    let missing = paradox::missing_paradox(&layout);
    if !missing.is_empty() {
        return Err(HostError::message(format!(
            "this checkout has no {}: clone the project again with git (see docs/hosting.md) and move .workspace into the new clone",
            missing.join(", ")
        )));
    }
    match paradox::migrate_old_workspace(&layout, &p.repo) {
        Ok(copied) if copied.is_empty() => out("- in place"),
        Ok(copied) => out(&format!(
            "- your files from the old {} copied in: {} (that folder is no longer used; delete it once the server runs)",
            layout.old_upstream.display(),
            copied.join(", ")
        )),
        Err(e) => {
            return Err(HostError::message(format!(
                "could not copy your files from {}: {e}",
                layout.old_upstream.display()
            )));
        }
    }
    // The project's preset (preset/): the game data this server starts from, where none of its own is there yet.
    match env.apply_preset(p) {
        Ok(Some(applied)) => out(&format!(
            "- server preset \"{}\": {} game-data files taken from preset/{}",
            applied.name,
            applied.written.len(),
            if applied.kept.is_empty() {
                String::new()
            } else {
                format!(", {} of your own kept", applied.kept.len())
            }
        )),
        Ok(None) => {}
        Err(e) => out(&format!("! The server preset in preset/ was not applied: {e}")),
    }

    if args.skip_install {
        out("\n2. Skipped installing and building (--skip-install)");
    } else {
        out("\n2. Installing and building the services (a few minutes the first time)");
        env.install_services(&layout, &log_file, out)?;
    }

    out("\n3. Configuration");
    let (secrets, created) = load_or_create_secrets(&p.config, secrets::new_secrets)?;
    out(&format!(
        "- secrets: {} in {}",
        if created { "made (kept from now on)" } else { "kept" },
        p.config.join(crate::hostconfig::SECRET_FILE).display()
    ));
    for dir in [
        &p.runtime_publish,
        &p.branding,
        &p.gameserver_logs,
        &p.mongo_data,
        &layout.raw_game_data,
    ] {
        mkdir(dir)?;
    }
    if !p.news_file.exists() {
        write(&p.news_file, "{\n  \"items\": []\n}\n")?;
    }
    let source_commit = env.source_commit(&p.repo)?;
    let written = write_env_files(
        p,
        &build_env_files(&config, &secrets, p, &source_commit, env.platform()),
    )?;
    let names: Vec<String> = written
        .iter()
        .map(|f| {
            f.file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_default()
        })
        .collect();
    out(&format!("- env files: {} in {}", names.join(", "), p.config.display()));
    if linux {
        let firewall = p.config.join("firewall.sh");
        write(&firewall, &crate::linux::linux_firewall_script(&config)?)?;
        make_executable(&firewall)?;
        out(&format!("- host firewall (ufw), once: sudo sh {}", firewall.display()));
        out(&format!(
            "- game-server launcher (Wine): {}",
            crate::linux::write_gameserver_wrapper(p)?.display()
        ));
        match crate::linux::write_systemd_units(&config, p) {
            Ok(_) => out(&format!(
                "- systemd units (run the server at boot), once: sudo sh {}",
                p.systemd.join("install.sh").display()
            )),
            Err(e) => out(&format!("! systemd units not written: {e}")),
        }
    } else {
        let file = p.config.join("firewall.ps1");
        write(&file, &firewall::firewall_script(&config, &env.gateway_program()))?;
        out(&format!(
            "- firewall rules to add once (as administrator): {}",
            file.display()
        ));
    }

    if config.mode == ServerMode::Public {
        if p.cert_file.exists() {
            out(&format!(
                "- gateway certificate kept (a new one would change every invite): {}",
                p.cert_file.display()
            ));
        } else {
            let mut hosts = vec![config.host.clone()];
            if config.game_address != config.host {
                hosts.push(config.game_address.clone());
            }
            env.make_certificate(&layout, &hosts, &p.tls, &log_file)?;
            out(&format!("- gateway certificate made in {}", p.tls.display()));
        }
        out(&format!(
            "- certificate fingerprint (goes into every invite): {}",
            crate::invite::certificate_fingerprint(&p.cert_file)?
        ));
    }

    if linux {
        linux_setup(p, &config, &log_file, env, out)?;
    }

    for note in database::database_notes(&config, env.platform()) {
        out(&note);
    }
    match config.game_dir.as_deref() {
        None => out(
            "\n! No game folder yet: pass --game \"<your Dauntless 1.14.7 folder>\" (the game servers run from it).",
        ),
        Some(dir) if !Path::new(dir).join(game_exe_relative()).exists() => out(&format!(
            "\n! No {} in {dir}: pass --game \"<the Dauntless folder that holds Archon>\" (the game servers run from it).",
            game_exe_relative().display()
        )),
        Some(_) => {}
    }

    out("\nSetup done. Next:");
    if linux {
        out(
            "  1. The game runtime and the game data come from a Windows PC with your own 1.14.7 (docs/hosting-linux.md):",
        );
        out("     reforged-host publish-runtime --from <the folder with MysticParadox.dll and winmm.dll>");
        out("     reforged-host gamedata --import <the game data pack>");
        out(&format!(
            "  2. Once: sudo sh {}",
            p.config.join("firewall.sh").display()
        ));
        out(&format!(
            "  3. Run it (and at every boot): sudo sh {}   Logs: journalctl -u {} -f",
            p.systemd.join("install.sh").display(),
            crate::linux::UNIT_SERVER
        ));
        if config.mode == ServerMode::Private {
            out("     (a private server also runs in a terminal: reforged-host start)");
        }
        out("  4. reforged-host invite --for \"Alex\"");
        out("  reforged-host doctor shows what is still missing.");
        return Ok(ExitCode::SUCCESS);
    }
    out("  1. The SDK, the runtime and the game data, from your own game: docs/sdk-and-game-data.md");
    out("     (the launcher's Host tools page does most of it: Settings > Advanced > Host tools)");
    out("  2. reforged-host publish-runtime");
    out("  3. reforged-host start");
    out("  4. reforged-host invite --for \"Alex\"");
    out("  reforged-host doctor shows what is still missing.");
    Ok(ExitCode::SUCCESS)
}

#[cfg(unix)]
fn make_executable(file: &Path) -> Result<(), HostError> {
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(file, std::fs::Permissions::from_mode(0o755)).map_err(|e| HostError::io(file, e))
}

#[cfg(not(unix))]
fn make_executable(_file: &Path) -> Result<(), HostError> {
    Ok(())
}

/// Linux: what runs the Windows game servers, checked and prepared once.
fn linux_setup(
    p: &Paths,
    config: &HostConfig,
    log_file: &Path,
    env: &dyn SetupEnv,
    out: &mut dyn FnMut(&str),
) -> Result<(), HostError> {
    use crate::linux::{RuntimeKind, ensure_wine_prefix, game_runtime};
    let lx = linux_options(config);
    out("\n4. Linux: the game servers run through Wine");
    match game_runtime(&lx)? {
        None => out(
            "! Wine was not found: sudo apt install wine wine64 (docs/hosting-linux.md), then run setup again. Or pass --wine <path> or --proton <script>.",
        ),
        Some(runtime) => {
            let wine = runtime.kind == RuntimeKind::Wine;
            out(&format!("- {}: {}", if wine { "Wine" } else { "Proton" }, runtime.path));
            if wine {
                out("- Wine prefix: preparing it (the first time takes a minute or two)...");
                let r = ensure_wine_prefix(p, &lx, log_file)?;
                out(&if r.ok {
                    format!(
                        "- Wine prefix {}: {}",
                        if r.created { "made" } else { "ready" },
                        p.wine_prefix.display()
                    )
                } else {
                    format!("! Wine prefix: {}", r.detail.unwrap_or_default())
                });
            }
        }
    }
    if lx.xvfb && env.find_on_path("xvfb-run")?.is_none() {
        out("! xvfb-run was not found (the game servers need a virtual display): sudo apt install xvfb xauth");
    }
    let nft = [PathBuf::from("/usr/sbin/nft"), PathBuf::from("/sbin/nft")];
    if config.mode == ServerMode::Public && !nft.iter().any(|f| f.exists()) {
        out("! nftables was not found (the allowlist needs it): sudo apt install nftables");
    }
    Ok(())
}
