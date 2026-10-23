//! Running the Windows game servers through Wine or Proton (`legacy/ReforgedHost/src/wine.ts` and
//! `gameserver.ts`): what the Director starts for each game server on Linux, through
//! `.workspace/bin/reforged-gameserver` and the hidden `reforged-host gameserver` command.
//!
//! The game servers are still `Dauntless-Win64-Shipping.exe`, a Windows program. The Director starts
//! the wrapper instead of the executable (`GAMESERVER_BINARY_PATH`), and it runs the executable
//! through Wine (or Proton) on a virtual X display (`xvfb-run`): the game opens a window even with
//! `-nullrhi`. The Director's contract does not change: the same arguments, the same environment
//! (the runtime reads `MYSTICPARADOX_GAMESERVER_LAUNCH_ID` and `REFORGED_BACKEND_URL` from it), and
//! the readiness line the runtime prints on stdout, which Wine passes through.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use crate::error::HostError;

/// What the Director's env file tells the launcher (written by setup, see `hostconfig`).
pub const ENV_EXE: &str = "REFORGED_GAME_EXE";
pub const ENV_WINE: &str = "REFORGED_WINE";
pub const ENV_PROTON: &str = "REFORGED_PROTON";
pub const ENV_PREFIX: &str = "REFORGED_WINEPREFIX";
pub const ENV_PROTON_DATA: &str = "REFORGED_PROTON_DATA";
pub const ENV_XVFB: &str = "REFORGED_XVFB";
pub const ENV_XVFB_RUN: &str = "REFORGED_XVFB_RUN";

#[cfg(unix)]
pub fn is_executable(file: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt;
    std::fs::metadata(file).is_ok_and(|m| m.is_file() && m.permissions().mode() & 0o111 != 0)
}

#[cfg(not(unix))]
pub fn is_executable(file: &Path) -> bool {
    file.is_file()
}

/// The first of `names` found on `path_var` (or a path given as is), as an absolute path.
pub fn find_on_path_in(names: &[&str], path_var: &str) -> Option<String> {
    for name in names {
        if name.contains('/') || name.contains(std::path::MAIN_SEPARATOR) {
            let file = Path::new(name);
            if is_executable(file) {
                return Some(
                    std::path::absolute(file)
                        .unwrap_or_else(|_| file.to_path_buf())
                        .to_string_lossy()
                        .into_owned(),
                );
            }
            continue;
        }
        for dir in std::env::split_paths(path_var) {
            if dir.as_os_str().is_empty() {
                continue;
            }
            for candidate in candidates(&dir, name) {
                if is_executable(&candidate) {
                    return Some(candidate.to_string_lossy().into_owned());
                }
            }
        }
    }
    None
}

/// `name`, and on Windows `name.exe` and `name.cmd` too.
fn candidates(dir: &Path, name: &str) -> Vec<PathBuf> {
    let mut list = vec![dir.join(name)];
    if cfg!(windows) && Path::new(name).extension().is_none() {
        list.push(dir.join(format!("{name}.exe")));
        list.push(dir.join(format!("{name}.cmd")));
    }
    list
}

/// The first of the programs found on PATH, as a full path.
pub fn find_on_path(names: &[&str]) -> Result<Option<String>, HostError> {
    Ok(find_on_path_in(names, &std::env::var("PATH").unwrap_or_default()))
}

pub fn find_wine(path_var: &str) -> Result<Option<String>, HostError> {
    Ok(find_on_path_in(&["wine", "wine64"], path_var))
}

/// `winmm=n,b`: the game must load Mystic Paradox's `winmm.dll` (the runtime loader beside the
/// executable), not Wine's own. `mscoree` and `mshtml` off: no Mono or Gecko installer prompt, which
/// would wait forever on the virtual display.
pub fn dll_overrides(existing: Option<&str>) -> String {
    let ours = ["winmm=n,b", "mscoree=d", "mshtml=d"];
    let names: Vec<&str> = ours.iter().filter_map(|o| o.split('=').next()).collect();
    let mut parts: Vec<String> = existing
        .unwrap_or_default()
        .split(';')
        .map(str::trim)
        .filter(|p| !p.is_empty())
        .filter(|p| {
            let name = p.split('=').next().unwrap_or_default().to_lowercase();
            !names.contains(&name.as_str())
        })
        .map(str::to_owned)
        .collect();
    parts.extend(ours.iter().map(|o| (*o).to_owned()));
    parts.join(";")
}

/// How one game server is started.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LaunchSpec {
    pub command: String,
    pub args: Vec<String>,
    pub cwd: PathBuf,
    pub env: BTreeMap<String, String>,
    pub runtime: String,
}

/// Where Steam keeps a Proton build, for `STEAM_COMPAT_CLIENT_INSTALL_PATH`.
fn steam_root_for(proton: &Path) -> Option<PathBuf> {
    let mut current = proton.parent()?;
    for _ in 0..8 {
        let name = current.file_name().map(|n| n.to_string_lossy().into_owned());
        if name.as_deref() == Some("compatibilitytools.d") {
            return current.parent().map(Path::to_path_buf);
        }
        if name.as_deref() == Some("common")
            && current
                .parent()
                .and_then(Path::file_name)
                .is_some_and(|n| n == "steamapps")
        {
            return current.parent().and_then(Path::parent).map(Path::to_path_buf);
        }
        current = current.parent()?;
    }
    None
}

fn setting(env: &BTreeMap<String, String>, name: &str) -> Option<String> {
    env.get(name).map(|v| v.trim().to_owned()).filter(|v| !v.is_empty())
}

fn mkdir(dir: &Path) -> Result<(), HostError> {
    std::fs::create_dir_all(dir).map_err(|e| HostError::io(dir, e))
}

/// The program, arguments and environment for one game server with the Director's `game_args`.
pub fn build_launch(game_args: &[String], env: &BTreeMap<String, String>) -> Result<LaunchSpec, HostError> {
    let exe = env.get(ENV_EXE).cloned().unwrap_or_default();
    let exe_path = Path::new(&exe);
    if exe.is_empty() || !exe_path.is_absolute() || !exe_path.exists() {
        return Err(HostError::message(format!(
            "{ENV_EXE} must be the full path of an existing Dauntless-Win64-Shipping.exe (got {}); run reforged-host setup --game <folder>",
            serde_json::Value::String(exe.clone())
        )));
    }
    let cwd = exe_path.parent().map(Path::to_path_buf).unwrap_or_default();
    let mut run_env = env.clone();
    let (command, mut args, runtime) = if let Some(proton) = setting(env, ENV_PROTON) {
        if !is_executable(Path::new(&proton)) {
            return Err(HostError::message(format!(
                "{ENV_PROTON} is not an executable Proton script: {proton}"
            )));
        }
        let data = setting(env, ENV_PROTON_DATA).filter(|d| Path::new(d).is_absolute());
        let Some(data) = data else {
            return Err(HostError::message(format!(
                "{ENV_PROTON_DATA} must be a full path (Proton's compat-data folder)"
            )));
        };
        mkdir(Path::new(&data))?;
        run_env.insert("STEAM_COMPAT_DATA_PATH".into(), data);
        if let Some(root) = steam_root_for(Path::new(&proton)) {
            run_env.insert(
                "STEAM_COMPAT_CLIENT_INSTALL_PATH".into(),
                root.to_string_lossy().into_owned(),
            );
        }
        run_env.insert("STEAM_COMPAT_APP_ID".into(), "0".into());
        run_env.insert("PROTON_LOG".into(), "0".into());
        run_env.insert(
            "WINEDLLOVERRIDES".into(),
            dll_overrides(env.get("WINEDLLOVERRIDES").map(String::as_str)),
        );
        let build = Path::new(&proton)
            .parent()
            .and_then(Path::file_name)
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        let mut args = vec!["run".to_owned(), exe.clone()];
        args.extend(game_args.iter().cloned());
        (proton, args, format!("Proton ({build})"))
    } else {
        let wine_setting = setting(env, ENV_WINE);
        let wine = match &wine_setting {
            Some(w) => is_executable(Path::new(w)).then(|| w.clone()),
            None => find_wine(env.get("PATH").map(String::as_str).unwrap_or_default())?,
        };
        let Some(wine) = wine else {
            return Err(HostError::message(match wine_setting {
                Some(w) => format!("{ENV_WINE} is not executable: {w}"),
                None => "Wine was not found: apt install wine (or run setup with --wine <path>)".to_owned(),
            }));
        };
        let prefix = setting(env, ENV_PREFIX).filter(|p| Path::new(p).is_absolute());
        let Some(prefix) = prefix else {
            return Err(HostError::message(format!(
                "{ENV_PREFIX} must be a full path (the Wine prefix)"
            )));
        };
        mkdir(Path::new(&prefix))?;
        run_env.insert("WINEPREFIX".into(), prefix);
        if !env.contains_key("WINEDEBUG") {
            run_env.insert("WINEDEBUG".into(), "-all".into());
        }
        run_env.insert(
            "WINEDLLOVERRIDES".into(),
            dll_overrides(env.get("WINEDLLOVERRIDES").map(String::as_str)),
        );
        let mut args = vec![exe.clone()];
        args.extend(game_args.iter().cloned());
        (wine, args, "Wine".to_owned())
    };
    let no_display = run_env.get("DISPLAY").is_none_or(|d| d.trim().is_empty());
    if env.get(ENV_XVFB).map(String::as_str) != Some("0") && no_display {
        let xvfb_setting = setting(env, ENV_XVFB_RUN);
        let xvfb = match &xvfb_setting {
            Some(x) => is_executable(Path::new(x)).then(|| x.clone()),
            None => find_on_path_in(&["xvfb-run"], env.get("PATH").map(String::as_str).unwrap_or_default()),
        };
        let Some(xvfb) = xvfb else {
            return Err(HostError::message(
                "there is no display and xvfb-run was not found: apt install xvfb xauth (or set a DISPLAY)",
            ));
        };
        let mut wrapped = vec![
            "-a".to_owned(),
            "-s".to_owned(),
            "-screen 0 1024x768x24".to_owned(),
            command,
        ];
        wrapped.append(&mut args);
        return Ok(LaunchSpec {
            command: xvfb,
            args: wrapped,
            cwd,
            env: run_env,
            runtime: format!("{runtime} on xvfb-run"),
        });
    }
    Ok(LaunchSpec {
        command,
        args,
        cwd,
        env: run_env,
        runtime,
    })
}

/// A Wine prefix is ready once wineboot has made its registry.
pub fn wine_prefix_ready(prefix: &Path) -> bool {
    prefix.join("system.reg").exists()
}

/// Runs one game server with the Director's arguments; returns its exit code. stdio is inherited:
/// the Director reads the runtime's readiness line from it and writes it all to the game server's
/// log.
pub fn run_game_server(args: &[String]) -> Result<i32, HostError> {
    let env: BTreeMap<String, String> = std::env::vars().collect();
    let spec = match build_launch(args, &env) {
        Ok(spec) => spec,
        Err(e) => {
            eprintln!("reforged-gameserver: {e}");
            return Ok(127);
        }
    };
    group::run(&spec)
}

#[cfg(unix)]
#[allow(unsafe_code)] // kill(2) on the game server's process group
mod group {
    use std::os::unix::process::{CommandExt, ExitStatusExt};
    use std::process::Command;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicI32, Ordering};

    use super::LaunchSpec;
    use crate::error::HostError;

    fn signal_group(pid: i32, signal: i32) {
        if pid <= 0 {
            return;
        }
        // SAFETY: kill(2) has no memory effects; a stale pid only fails.
        if unsafe { libc::kill(-pid, signal) } != 0 {
            let _ = unsafe { libc::kill(pid, signal) };
        }
    }

    /// The launched runtime leads its own process group (xvfb-run, Xvfb, wine and the game): a
    /// signal for the game server goes to all of them, so no Xvfb or game process outlives it.
    pub fn run(spec: &LaunchSpec) -> Result<i32, HostError> {
        let mut child = match Command::new(&spec.command)
            .args(&spec.args)
            .current_dir(&spec.cwd)
            .env_clear()
            .envs(&spec.env)
            .process_group(0)
            .spawn()
        {
            Ok(child) => child,
            Err(e) => {
                eprintln!("reforged-gameserver: could not start {}: {e}", spec.command);
                return Ok(127);
            }
        };
        let pid = i32::try_from(child.id()).unwrap_or(0);
        let shared = Arc::new(AtomicI32::new(pid));
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .map_err(|e| HostError::message(format!("could not start the signal handler: {e}")))?;
        let forward = Arc::clone(&shared);
        let handler = std::thread::spawn(move || {
            runtime.block_on(async move {
                use tokio::signal::unix::{SignalKind, signal};
                let (Ok(mut term), Ok(mut int), Ok(mut hup)) = (
                    signal(SignalKind::terminate()),
                    signal(SignalKind::interrupt()),
                    signal(SignalKind::hangup()),
                ) else {
                    return;
                };
                loop {
                    let sig = tokio::select! {
                        _ = term.recv() => libc::SIGTERM,
                        _ = int.recv() => libc::SIGINT,
                        _ = hup.recv() => libc::SIGHUP,
                    };
                    signal_group(forward.load(Ordering::SeqCst), sig);
                }
            });
        });
        drop(handler);
        eprintln!(
            "reforged-gameserver: game server started with {} (pid {pid})",
            spec.runtime
        );
        let status = child
            .wait()
            .map_err(|e| HostError::message(format!("waiting for the game server: {e}")))?;
        // Whatever is left of the group (Xvfb) goes with it.
        signal_group(pid, libc::SIGTERM);
        if let Some(sig) = status.signal() {
            eprintln!("reforged-gameserver: {} ended on signal {sig}", spec.runtime);
        }
        Ok(status.code().unwrap_or(1))
    }
}

#[cfg(not(unix))]
mod group {
    use super::LaunchSpec;
    use crate::error::HostError;

    /// Linux only: Windows runs the game servers itself.
    pub fn run(_spec: &LaunchSpec) -> Result<i32, HostError> {
        Err(HostError::message(
            "reforged-host gameserver runs the game servers through Wine on Linux; on Windows the Director starts them itself",
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keeps_other_overrides_and_puts_ours_last() {
        assert_eq!(dll_overrides(None), "winmm=n,b;mscoree=d;mshtml=d");
        assert_eq!(
            dll_overrides(Some("d3d11=n; WINMM=b ;;dxgi=n")),
            "d3d11=n;dxgi=n;winmm=n,b;mscoree=d;mshtml=d"
        );
    }

    #[test]
    fn refuses_a_missing_executable() {
        let env = BTreeMap::from([(ENV_EXE.to_owned(), "relative/game.exe".to_owned())]);
        let error = build_launch(&[], &env).unwrap_err().to_string();
        assert!(error.starts_with("REFORGED_GAME_EXE must be the full path"), "{error}");
    }

    #[cfg(unix)]
    #[test]
    fn runs_wine_on_xvfb_without_a_display() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let exe = dir.path().join("Dauntless-Win64-Shipping.exe");
        std::fs::write(&exe, "").unwrap();
        let bin = dir.path().join("bin");
        std::fs::create_dir(&bin).unwrap();
        for name in ["wine", "xvfb-run"] {
            let f = bin.join(name);
            std::fs::write(&f, "#!/bin/sh\n").unwrap();
            std::fs::set_permissions(&f, std::fs::Permissions::from_mode(0o755)).unwrap();
        }
        let prefix = dir.path().join("prefix");
        let env = BTreeMap::from([
            (ENV_EXE.to_owned(), exe.to_string_lossy().into_owned()),
            (ENV_PREFIX.to_owned(), prefix.to_string_lossy().into_owned()),
            ("PATH".to_owned(), bin.to_string_lossy().into_owned()),
        ]);
        let spec = build_launch(&["-port=8777".to_owned()], &env).unwrap();
        assert_eq!(spec.command, bin.join("xvfb-run").to_string_lossy());
        assert_eq!(spec.args[..3], ["-a", "-s", "-screen 0 1024x768x24"]);
        assert_eq!(spec.args[3], bin.join("wine").to_string_lossy());
        assert_eq!(
            spec.args[4..],
            [exe.to_string_lossy().into_owned(), "-port=8777".to_owned()]
        );
        assert_eq!(spec.runtime, "Wine on xvfb-run");
        assert_eq!(spec.env["WINEDEBUG"], "-all");
        assert!(prefix.is_dir());
    }

    #[test]
    fn finds_steams_root_for_a_proton_build() {
        let p = Path::new("/home/u/.steam/root/compatibilitytools.d/GE-Proton9/proton");
        assert_eq!(steam_root_for(p), Some(PathBuf::from("/home/u/.steam/root")));
        let p = Path::new("/home/u/.local/share/Steam/steamapps/common/Proton 9.0/proton");
        assert_eq!(steam_root_for(p), Some(PathBuf::from("/home/u/.local/share/Steam")));
    }
}
