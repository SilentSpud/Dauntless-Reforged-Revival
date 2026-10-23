//! Running the Windows game on Linux (row L32): `src/main/linux-runtime.ts` and `linux-dependencies.ts`.
//! Proton (Steam's compatibility tools, native or Flatpak Steam, or `DAUNTLESS_REVIVED_PROTON`) or Wine
//! (`DAUNTLESS_REVIVED_WINE`, the PATH, Lutris), each with a prefix of the launcher's own under userData,
//! `dxgi=n,b` so the proxy DLL loads, and the XWayland authority GNOME keeps when `XAUTHORITY` is unset.
//! When nothing is found, Wine can be installed with the distribution's package manager through pkexec.

use std::cmp::Ordering;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::time::Duration;

use super::launch::LaunchRuntime;
use crate::platform::log;

pub type Env = BTreeMap<String, String>;

pub fn current_env() -> Env {
    std::env::vars().collect()
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RuntimeKind {
    Proton,
    Wine,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DetectedRuntime {
    pub kind: RuntimeKind,
    pub name: String,
    pub command: PathBuf,
    pub prefix_dir: PathBuf,
    pub compat_data_dir: Option<PathBuf>,
    pub env: Env,
}

#[derive(Debug, Clone)]
pub struct PreparedLaunch {
    pub runtime: LaunchRuntime,
    pub config_dir: PathBuf,
    pub runtime_name: String,
}

/// The `LinuxRuntimeMissingError` flow: the page explains how to install Proton or Wine.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RuntimeMissing;

fn executable(file: &Path) -> bool {
    let Ok(metadata) = std::fs::metadata(file) else {
        return false;
    };
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        metadata.is_file() && metadata.permissions().mode() & 0o111 != 0
    }
    #[cfg(not(unix))]
    {
        metadata.is_file()
    }
}

fn path_dirs(env: &Env) -> Vec<PathBuf> {
    env.get("PATH")
        .map(|path| {
            std::env::split_paths(path)
                .filter(|dir| !dir.as_os_str().is_empty())
                .collect()
        })
        .unwrap_or_default()
}

fn from_path(name: &str, env: &Env) -> Option<PathBuf> {
    if name.contains('/') || name.contains('\\') {
        let path = PathBuf::from(name);
        return executable(&path).then(|| std::path::absolute(&path).unwrap_or(path));
    }
    path_dirs(env)
        .into_iter()
        .map(|dir| dir.join(name))
        .find(|file| executable(file))
}

fn custom_runtime(value: Option<&String>, env: &Env, home: &Path) -> Option<PathBuf> {
    let value = value.filter(|value| !value.is_empty())?;
    match value.strip_prefix("~/") {
        Some(rest) => from_path(&home.join(rest).to_string_lossy(), env),
        None => from_path(value, env),
    }
}

fn directories(parent: &Path) -> Vec<PathBuf> {
    std::fs::read_dir(parent)
        .map(|entries| {
            entries
                .filter_map(Result::ok)
                .filter(|entry| entry.file_type().is_ok_and(|kind| kind.is_dir()))
                .map(|entry| entry.path())
                .collect()
        })
        .unwrap_or_default()
}

/// `localeCompare` with `numeric: true, sensitivity: "base"`: digit runs by value, letters ignoring case.
fn natural(a: &str, b: &str) -> Ordering {
    let (mut a, mut b) = (a.chars().peekable(), b.chars().peekable());
    loop {
        match (a.peek().copied(), b.peek().copied()) {
            (None, None) => return Ordering::Equal,
            (None, Some(_)) => return Ordering::Less,
            (Some(_), None) => return Ordering::Greater,
            (Some(x), Some(y)) if x.is_ascii_digit() && y.is_ascii_digit() => {
                let take = |chars: &mut std::iter::Peekable<std::str::Chars<'_>>| {
                    let mut digits = String::new();
                    while let Some(c) = chars.peek().copied().filter(char::is_ascii_digit) {
                        digits.push(c);
                        chars.next();
                    }
                    digits.trim_start_matches('0').to_owned()
                };
                let (x, y) = (take(&mut a), take(&mut b));
                let order = x.len().cmp(&y.len()).then_with(|| x.cmp(&y));
                if order != Ordering::Equal {
                    return order;
                }
            }
            (Some(x), Some(y)) => {
                let order = x.to_lowercase().cmp(y.to_lowercase());
                if order != Ordering::Equal {
                    return order;
                }
                a.next();
                b.next();
            }
        }
    }
}

fn newest_first(mut candidates: Vec<PathBuf>) -> Vec<PathBuf> {
    candidates.sort_by(|a, b| natural(&b.to_string_lossy(), &a.to_string_lossy()));
    candidates
}

fn proton_candidates(home: &Path) -> Vec<PathBuf> {
    let steam_roots = [
        home.join(".local").join("share").join("Steam"),
        home.join(".steam").join("root"),
        home.join(".steam").join("steam"),
        home.join(".var")
            .join("app")
            .join("com.valvesoftware.Steam")
            .join("data")
            .join("Steam"),
        home.join(".var")
            .join("app")
            .join("com.valvesoftware.Steam")
            .join(".local")
            .join("share")
            .join("Steam"),
    ];
    let mut out = Vec::new();
    for root in steam_roots {
        out.extend(
            directories(&root.join("compatibilitytools.d"))
                .into_iter()
                .map(|dir| dir.join("proton")),
        );
        for dir in directories(&root.join("steamapps").join("common")) {
            let name = dir
                .file_name()
                .map(|name| name.to_string_lossy().to_lowercase())
                .unwrap_or_default();
            if name.starts_with("proton") || name.starts_with("ge-proton") {
                out.push(dir.join("proton"));
            }
        }
    }
    newest_first(out.into_iter().filter(|file| executable(file)).collect())
}

fn lutris_wine_candidates(home: &Path) -> Vec<PathBuf> {
    let root = home
        .join(".local")
        .join("share")
        .join("lutris")
        .join("runners")
        .join("wine");
    let mut out = Vec::new();
    for dir in directories(&root) {
        for name in ["wine64", "wine"] {
            let candidate = dir.join("bin").join(name);
            if executable(&candidate) {
                out.push(candidate);
            }
        }
    }
    newest_first(out)
}

fn steam_root_for(proton: &Path) -> Option<PathBuf> {
    let mut current = proton.parent()?.to_path_buf();
    for _ in 0..8 {
        let name = current
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_default();
        if name == "compatibilitytools.d" {
            return current.parent().map(Path::to_path_buf);
        }
        if name == "common"
            && current
                .parent()
                .and_then(Path::file_name)
                .is_some_and(|parent| parent == "steamapps")
        {
            return current.parent()?.parent().map(Path::to_path_buf);
        }
        current = current.parent()?.to_path_buf();
    }
    None
}

fn dll_overrides(existing: Option<&String>) -> String {
    let mut parts: Vec<&str> = existing
        .map(String::as_str)
        .unwrap_or_default()
        .split(';')
        .map(str::trim)
        .filter(|part| !part.is_empty() && !part.to_ascii_lowercase().starts_with("dxgi="))
        .collect();
    parts.push("dxgi=n,b");
    parts.join(";")
}

fn with_xwayland_authority(mut env: Env) -> Env {
    let wayland = env
        .get("XDG_SESSION_TYPE")
        .is_some_and(|kind| kind.eq_ignore_ascii_case("wayland"));
    let Some(runtime_dir) = env
        .get("XDG_RUNTIME_DIR")
        .filter(|_| wayland && !env.contains_key("XAUTHORITY"))
        .cloned()
    else {
        return env;
    };
    let mut candidates: Vec<(std::time::SystemTime, PathBuf)> = std::fs::read_dir(&runtime_dir)
        .map(|entries| {
            entries
                .filter_map(Result::ok)
                .filter(|entry| entry.file_name().to_string_lossy().starts_with(".mutter-Xwaylandauth."))
                .filter_map(|entry| {
                    let metadata = entry.metadata().ok().filter(std::fs::Metadata::is_file)?;
                    Some((metadata.modified().ok()?, entry.path()))
                })
                .collect()
        })
        .unwrap_or_default();
    candidates.sort_by_key(|candidate| std::cmp::Reverse(candidate.0));
    if let Some((_, newest)) = candidates.into_iter().next() {
        env.insert("XAUTHORITY".into(), newest.to_string_lossy().into_owned());
    }
    env
}

fn parent_name(file: &Path) -> String {
    file.parent()
        .and_then(Path::file_name)
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_default()
}

pub fn detect_linux_runtime(user_data: &Path, env: &Env, home: &Path) -> Option<DetectedRuntime> {
    let mut env = with_xwayland_authority(env.clone());
    let explicit_proton = custom_runtime(env.get("DAUNTLESS_REVIVED_PROTON"), &env, home);
    let explicit_wine = custom_runtime(env.get("DAUNTLESS_REVIVED_WINE"), &env, home);
    let proton = explicit_proton.or_else(|| {
        if explicit_wine.is_some() {
            None
        } else {
            proton_candidates(home).into_iter().next()
        }
    });
    let overrides = dll_overrides(env.get("WINEDLLOVERRIDES"));
    if let Some(proton) = proton {
        let compat = user_data.join("compat").join("proton");
        env.insert("STEAM_COMPAT_DATA_PATH".into(), compat.to_string_lossy().into_owned());
        if let Some(root) = steam_root_for(&proton) {
            env.insert(
                "STEAM_COMPAT_CLIENT_INSTALL_PATH".into(),
                root.to_string_lossy().into_owned(),
            );
        }
        env.insert("STEAM_COMPAT_APP_ID".into(), "0".into());
        env.insert("PROTON_LOG".into(), "0".into());
        env.insert("WINEDLLOVERRIDES".into(), overrides);
        return Some(DetectedRuntime {
            kind: RuntimeKind::Proton,
            name: format!("Proton ({})", parent_name(&proton)),
            command: proton,
            prefix_dir: compat.join("pfx"),
            compat_data_dir: Some(compat),
            env,
        });
    }
    let wine = explicit_wine
        .or_else(|| from_path("wine64", &env))
        .or_else(|| from_path("wine", &env))
        .or_else(|| lutris_wine_candidates(home).into_iter().next())?;
    let prefix = user_data.join("compat").join("wine");
    env.insert("WINEPREFIX".into(), prefix.to_string_lossy().into_owned());
    env.entry("WINEDEBUG".into()).or_insert_with(|| "-all".into());
    env.insert("WINEDLLOVERRIDES".into(), overrides);
    let folder = parent_name(&wine);
    Some(DetectedRuntime {
        kind: RuntimeKind::Wine,
        name: format!("Wine ({})", if folder.is_empty() { "system" } else { &folder }),
        command: wine,
        prefix_dir: prefix,
        compat_data_dir: None,
        env,
    })
}

async fn run(command: &Path, args: &[&str], env: &Env, cwd: &Path, timeout: Duration) -> std::io::Result<()> {
    let mut process = tokio::process::Command::new(command);
    process
        .args(args)
        .env_clear()
        .envs(env)
        .current_dir(cwd)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .kill_on_drop(true);
    let status = tokio::time::timeout(timeout, process.status())
        .await
        .map_err(|_| std::io::Error::from(std::io::ErrorKind::TimedOut))??;
    if status.success() {
        Ok(())
    } else {
        Err(std::io::Error::other(format!(
            "{} exited with {status}",
            command.display()
        )))
    }
}

async fn initialize(runtime: &DetectedRuntime, home: &Path) -> std::io::Result<()> {
    if runtime.prefix_dir.join("drive_c").exists() {
        return Ok(());
    }
    let two_minutes = Duration::from_secs(120);
    if runtime.kind == RuntimeKind::Proton {
        if let Some(compat) = &runtime.compat_data_dir {
            std::fs::create_dir_all(compat)?;
        }
        return run(
            &runtime.command,
            &["run", "cmd", "/c", "exit"],
            &runtime.env,
            home,
            two_minutes,
        )
        .await;
    }
    let beside = runtime.command.with_file_name("wineboot");
    let wineboot = if executable(&beside) {
        Some(beside)
    } else {
        from_path("wineboot", &runtime.env)
    };
    match wineboot {
        Some(wineboot) => run(&wineboot, &["-u"], &runtime.env, home, two_minutes).await,
        None => run(&runtime.command, &["wineboot", "-u"], &runtime.env, home, two_minutes).await,
    }
}

fn wine_user(runtime: &DetectedRuntime, env: &Env) -> String {
    if runtime.kind == RuntimeKind::Proton {
        return "steamuser".into();
    }
    let raw = env
        .get("USER")
        .or_else(|| env.get("LOGNAME"))
        .map_or("wineuser", String::as_str);
    let user: String = raw
        .chars()
        .map(|c| if "\\/:*?\"<>|".contains(c) { '_' } else { c })
        .take(64)
        .collect();
    if user.is_empty() { "wineuser".into() } else { user }
}

pub async fn prepare_linux_game_launch(
    user_data: &Path,
    env: &Env,
    home: &Path,
    auto_install_runtime: bool,
) -> Result<PreparedLaunch, RuntimeMissing> {
    let mut detected = detect_linux_runtime(user_data, env, home);
    let auto = auto_install_runtime
        && env
            .get("DAUNTLESS_REVIVED_AUTO_INSTALL")
            .is_none_or(|value| value != "0");
    if detected.is_none()
        && auto
        && let Some(plan) = install_plan(DependencyKind::Runtime, env)
    {
        log::info(&format!(
            "no Proton/Wine runtime found; installing Wine with {}",
            plan.name
        ));
        match run_install(&plan, env, home).await {
            Ok(()) => log::info("Wine dependency install finished; detecting runtime again"),
            Err(error) => log::warn(&format!(
                "automatic Wine install failed: {}",
                log::describe_error(&error)
            )),
        }
        detected = detect_linux_runtime(user_data, env, home);
    }
    let detected = detected.ok_or(RuntimeMissing)?;
    if let Err(error) = initialize(&detected, home).await {
        log::warn(&format!(
            "preparing the {} prefix failed: {}",
            detected.name,
            log::describe_error(&error)
        ));
    }
    let user = wine_user(&detected, env);
    let config_dir = [
        "drive_c",
        "users",
        &user,
        "AppData",
        "Local",
        "Archon",
        "Saved",
        "Config",
        "WindowsClient",
    ]
    .iter()
    .fold(detected.prefix_dir.clone(), |dir, part| dir.join(part));
    Ok(PreparedLaunch {
        runtime: LaunchRuntime {
            command: detected.command.to_string_lossy().into_owned(),
            args_prefix: if detected.kind == RuntimeKind::Proton {
                vec!["run".into()]
            } else {
                Vec::new()
            },
            env: detected.env.into_iter().collect(),
        },
        config_dir,
        runtime_name: detected.name,
    })
}

// ---------------------------------------------------------------- installing what is missing

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DependencyKind {
    Runtime,
    #[cfg_attr(
        not(target_os = "linux"),
        allow(dead_code, reason = "the secure-storage bootstrap is Linux only")
    )]
    SecureStorage,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InstallPlan {
    pub name: String,
    pub command: PathBuf,
    pub args: Vec<String>,
}

fn find_executable(name: &str, env: &Env) -> Option<PathBuf> {
    path_dirs(env)
        .into_iter()
        .chain(["/usr/bin", "/usr/sbin", "/bin", "/sbin", "/usr/local/bin"].map(PathBuf::from))
        .map(|dir| dir.join(name))
        .find(|file| executable(file))
}

/// Whether the launcher runs as root (`/proc/self` belongs to the process's user).
fn is_root() -> bool {
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt as _;
        std::fs::metadata("/proc/self").is_ok_and(|metadata| metadata.uid() == 0)
    }
    #[cfg(not(unix))]
    {
        false
    }
}

fn elevated(command: PathBuf, args: &[&str], env: &Env) -> Option<InstallPlan> {
    let name = command
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_default();
    let args: Vec<String> = args.iter().map(|arg| (*arg).to_owned()).collect();
    if is_root() {
        return Some(InstallPlan { name, command, args });
    }
    let pkexec = find_executable("pkexec", env)?;
    let mut all = vec![command.to_string_lossy().into_owned()];
    all.extend(args);
    Some(InstallPlan {
        name,
        command: pkexec,
        args: all,
    })
}

/// Wine, or gnome-keyring and libsecret, through the distribution's package manager.
pub fn install_plan(kind: DependencyKind, env: &Env) -> Option<InstallPlan> {
    let runtime = kind == DependencyKind::Runtime;
    let managers: [(&str, &[&str], &[&str]); 5] = [
        (
            "apt-get",
            &["install", "-y", "wine64"],
            &["install", "-y", "gnome-keyring", "libsecret-1-0"],
        ),
        (
            "dnf",
            &["install", "-y", "wine"],
            &["install", "-y", "gnome-keyring", "libsecret"],
        ),
        (
            "zypper",
            &["--non-interactive", "install", "wine"],
            &["--non-interactive", "install", "gnome-keyring", "libsecret"],
        ),
        (
            "pacman",
            &["-S", "--needed", "--noconfirm", "wine"],
            &["-S", "--needed", "--noconfirm", "gnome-keyring", "libsecret"],
        ),
        ("xbps-install", &["-Sy", "wine"], &["-Sy", "gnome-keyring", "libsecret"]),
    ];
    managers
        .iter()
        .find_map(|(name, wine, keyring)| {
            find_executable(name, env).map(|command| (command, if runtime { *wine } else { *keyring }))
        })
        .and_then(|(command, args)| elevated(command, args, env))
}

pub async fn run_install(plan: &InstallPlan, env: &Env, cwd: &Path) -> std::io::Result<()> {
    let args: Vec<&str> = plan.args.iter().map(String::as_str).collect();
    run(&plan.command, &args, env, cwd, Duration::from_secs(15 * 60)).await
}

#[cfg(test)]
#[cfg(unix)]
mod tests {
    use super::*;

    fn executable_file(file: &Path) -> PathBuf {
        use std::os::unix::fs::PermissionsExt as _;
        std::fs::create_dir_all(file.parent().unwrap()).unwrap();
        std::fs::write(file, "#!/bin/sh\nexit 0\n").unwrap();
        std::fs::set_permissions(file, std::fs::Permissions::from_mode(0o755)).unwrap();
        file.to_path_buf()
    }

    fn env(home: &Path, extra: &[(&str, &str)]) -> Env {
        let mut env: Env = [
            ("HOME", home.to_string_lossy().as_ref()),
            ("USER", "slayer"),
            ("PATH", ""),
        ]
        .iter()
        .map(|(k, v)| ((*k).to_owned(), (*v).to_owned()))
        .collect();
        env.extend(extra.iter().map(|(k, v)| ((*k).to_owned(), (*v).to_owned())));
        env
    }

    #[test]
    fn explicit_wine_wins_over_detected_proton() {
        let home = tempfile::tempdir().unwrap();
        let wine = executable_file(&home.path().join("custom").join("wine64"));
        executable_file(
            &home
                .path()
                .join(".local/share/Steam/compatibilitytools.d/GE-Proton99/proton"),
        );
        let user_data = home.path().join("data");
        let runtime = detect_linux_runtime(
            &user_data,
            &env(home.path(), &[("DAUNTLESS_REVIVED_WINE", &wine.to_string_lossy())]),
            home.path(),
        )
        .unwrap();
        assert_eq!((runtime.kind, runtime.command.clone()), (RuntimeKind::Wine, wine));
        assert_eq!(
            runtime.env["WINEPREFIX"],
            user_data.join("compat/wine").to_string_lossy()
        );
        assert!(
            runtime.env["WINEDLLOVERRIDES"]
                .split(';')
                .any(|part| part == "dxgi=n,b")
        );
    }

    #[test]
    fn steam_compatibility_tools_native_and_flatpak() {
        let home = tempfile::tempdir().unwrap();
        let older = executable_file(
            &home
                .path()
                .join(".local/share/Steam/compatibilitytools.d/GE-Proton9-20/proton"),
        );
        let proton = executable_file(
            &home
                .path()
                .join(".local/share/Steam/compatibilitytools.d/GE-Proton10-1/proton"),
        );
        let user_data = home.path().join("data");
        let runtime = detect_linux_runtime(&user_data, &env(home.path(), &[]), home.path()).unwrap();
        assert_eq!(
            runtime.command,
            proton,
            "the newest, by number, not {}",
            older.display()
        );
        assert_eq!(runtime.prefix_dir, user_data.join("compat/proton/pfx"));
        assert_eq!(
            runtime.env["STEAM_COMPAT_DATA_PATH"],
            user_data.join("compat/proton").to_string_lossy()
        );

        let flatpak_home = tempfile::tempdir().unwrap();
        let root = flatpak_home.path().join(".var/app/com.valvesoftware.Steam/data/Steam");
        let experimental = executable_file(&root.join("steamapps/common/Proton - Experimental/proton"));
        let runtime = detect_linux_runtime(
            &flatpak_home.path().join("data"),
            &env(flatpak_home.path(), &[]),
            flatpak_home.path(),
        )
        .unwrap();
        assert_eq!(runtime.command, experimental);
        assert_eq!(runtime.env["STEAM_COMPAT_CLIENT_INSTALL_PATH"], root.to_string_lossy());
    }

    #[test]
    fn the_xwayland_authority_is_found_when_unset() {
        let home = tempfile::tempdir().unwrap();
        executable_file(
            &home
                .path()
                .join(".local/share/Steam/compatibilitytools.d/GE-Proton10-1/proton"),
        );
        let runtime_dir = home.path().join("runtime");
        std::fs::create_dir_all(&runtime_dir).unwrap();
        let authority = runtime_dir.join(".mutter-Xwaylandauth.TEST");
        std::fs::write(&authority, "cookie").unwrap();
        let wayland = [
            ("XDG_SESSION_TYPE", "wayland"),
            ("XDG_RUNTIME_DIR", runtime_dir.to_str().unwrap()),
        ];
        let runtime =
            detect_linux_runtime(&home.path().join("data"), &env(home.path(), &wayland), home.path()).unwrap();
        assert_eq!(runtime.env["XAUTHORITY"], authority.to_string_lossy());
        let mut explicit = wayland.to_vec();
        explicit.push(("XAUTHORITY", "/explicit/xauth"));
        let runtime =
            detect_linux_runtime(&home.path().join("data2"), &env(home.path(), &explicit), home.path()).unwrap();
        assert_eq!(runtime.env["XAUTHORITY"], "/explicit/xauth");
    }

    #[tokio::test]
    async fn the_proton_compat_folder_exists_before_the_first_start() {
        use std::os::unix::fs::PermissionsExt as _;
        let home = tempfile::tempdir().unwrap();
        let proton = home.path().join("custom/proton");
        std::fs::create_dir_all(proton.parent().unwrap()).unwrap();
        std::fs::write(&proton, "#!/bin/sh\n[ -d \"$STEAM_COMPAT_DATA_PATH\" ] || exit 42\n/bin/mkdir -p \"$STEAM_COMPAT_DATA_PATH/pfx/drive_c\"\n").unwrap();
        std::fs::set_permissions(&proton, std::fs::Permissions::from_mode(0o755)).unwrap();
        let user_data = home.path().join("data");
        let prepared = prepare_linux_game_launch(
            &user_data,
            &env(home.path(), &[("DAUNTLESS_REVIVED_PROTON", proton.to_str().unwrap())]),
            home.path(),
            false,
        )
        .await
        .unwrap();
        assert_eq!(prepared.runtime_name, "Proton (custom)");
        assert_eq!(
            prepared.config_dir,
            user_data.join("compat/proton/pfx/drive_c/users/steamuser/AppData/Local/Archon/Saved/Config/WindowsClient")
        );
        assert!(user_data.join("compat/proton/pfx/drive_c").is_dir());
    }

    #[test]
    fn installs_are_fixed_and_elevated() {
        let root = tempfile::tempdir().unwrap();
        let apt = executable_file(&root.path().join("apt-get"));
        let pkexec = executable_file(&root.path().join("pkexec"));
        let env: Env = [("PATH".to_owned(), root.path().to_string_lossy().into_owned())].into();
        let plan = install_plan(DependencyKind::Runtime, &env).unwrap();
        assert_eq!(plan.command, pkexec);
        assert_eq!(plan.args, [apt.to_string_lossy().as_ref(), "install", "-y", "wine64"]);
        let plan = install_plan(DependencyKind::SecureStorage, &env).unwrap();
        assert_eq!(plan.args[1..], ["install", "-y", "gnome-keyring", "libsecret-1-0"]);
    }

    #[test]
    fn newest_by_number() {
        assert_eq!(natural("GE-Proton10-1", "GE-Proton9-20"), Ordering::Greater);
        assert_eq!(natural("proton 8.0", "Proton 8.0"), Ordering::Equal);
    }
}
