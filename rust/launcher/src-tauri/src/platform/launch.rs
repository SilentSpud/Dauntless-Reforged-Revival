//! Starting the game (rows L31, RA9): Reforged's `src/main/launch.ts`. The command line is the one
//! Mystic Paradox's launcher uses for 1.14.7, with the account key as the "exchange code": a Reforged
//! backend runs `AUTH_MODE=APIKEY` and trades the key for the game's session token. The key is never
//! logged (`mask_args`).
//!
//! The runtime learns where the backend is from the environment: `REFORGED_BACKEND_URL` names the
//! launcher's local relay, `MYSTICPARADOX_RUNTIME_PROFILE` picks the runtime's log profile.
//!
//! On Linux the game runs through DR's `LaunchRuntime` (Proton or Wine before the exe) and the running
//! clients are found with `pgrep`; their values (the prefix, `WINEDLLOVERRIDES` for the runtime's
//! `winmm.dll`) are RA10's.

use std::ffi::OsString;
use std::path::Path;
use std::sync::{Arc, Mutex};

use tokio::process::{Child, Command};

use crate::bridge::types::RuntimeProfile;
use crate::core::game_language::launch_locale;
use crate::core::pins::EXE_NAME;
use crate::core::username::is_plausible_account_key;

/// Windows' "start without a console window" process flag.
#[cfg(windows)]
const CREATE_NO_WINDOW: u32 = 0x0800_0000;

pub struct LaunchSettings<'a> {
    pub key: &'a str,
    /// From GetUserInfo
    pub user_id: &'a str,
    pub username: &'a str,
    pub windowed: bool,
    /// `-epiclocale`; English when left out or unknown
    pub game_locale: Option<&'a str>,
}

/// `^[A-Za-z0-9_-]{1,128}$`: the identity values go on the command line, so only plain characters pass.
fn is_plain_id(text: &str) -> bool {
    (1..=128).contains(&text.len())
        && text
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
}

/// `^[A-Za-z0-9_]{1,32}$`
fn is_plain_username(text: &str) -> bool {
    (1..=32).contains(&text.len()) && text.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_')
}

/// The 1.14.7 command line.
pub fn build_launch_args(settings: &LaunchSettings<'_>) -> Result<Vec<String>, &'static str> {
    if !is_plausible_account_key(settings.key) {
        return Err("invalid key");
    }
    if !is_plain_id(settings.user_id) {
        return Err("invalid user id");
    }
    if !is_plain_username(settings.username) {
        return Err("invalid username");
    }
    let (user_id, username) = (settings.user_id, settings.username);
    let locale = launch_locale(settings.game_locale);
    let mut args = vec![
        "-DisableEOS".to_owned(),
        "-EpicPortal".to_owned(),
        "-NoEAC".to_owned(),
        "-AUTH_TYPE=exchangecode".to_owned(),
        format!("-AUTH_LOGIN={user_id}"),
        format!("-AUTH_PASSWORD={}", settings.key),
        "-epicapp=Archon".to_owned(),
        "-epicenv=Prod".to_owned(),
        format!("-epicusername={username}"),
        format!("-epicuserid={user_id}"),
        format!("-epicaccountid={user_id}"),
        "-epicsandboxid=jackal".to_owned(),
        "-epicdeploymentid=53565ba467df4edbb6f5a3d939a8b4f2".to_owned(),
        format!("-epiclocale={locale}"),
    ];
    if settings.windowed {
        args.extend(["-windowed", "-ResX=1280", "-ResY=720"].map(str::to_owned));
    }
    Ok(args)
}

pub fn mask_args(args: &[String]) -> Vec<String> {
    args.iter()
        .map(|arg| {
            if arg
                .get(..15)
                .is_some_and(|head| head.eq_ignore_ascii_case("-AUTH_PASSWORD="))
            {
                "-AUTH_PASSWORD=<hidden>".to_owned()
            } else {
                arg.clone()
            }
        })
        .collect()
}

pub fn describe_launch(exe: &str, args: &[String]) -> String {
    format!("{exe} {}", mask_args(args).join(" "))
}

/// `^http://127\.0\.0\.1:[1-9][0-9]{0,4}$`: only the local relay.
fn is_relay_url(url: &str) -> bool {
    url.strip_prefix("http://127.0.0.1:").is_some_and(|port| {
        (1..=5).contains(&port.len()) && !port.starts_with('0') && port.bytes().all(|b| b.is_ascii_digit())
    })
}

pub fn runtime_profile_name(profile: RuntimeProfile) -> &'static str {
    match profile {
        RuntimeProfile::Production => "production",
        RuntimeProfile::Development => "development",
    }
}

/// The game's whole environment: `base` without anything a value left from elsewhere could redirect the
/// game with (every `REFORGED_` and `MYSTICPARADOX_` variable, in any case), then the relay and the
/// runtime's profile.
pub fn launch_environment(
    base: impl IntoIterator<Item = (OsString, OsString)>,
    backend_url: &str,
    profile: RuntimeProfile,
) -> Result<Vec<(OsString, OsString)>, &'static str> {
    if !is_relay_url(backend_url) {
        return Err("the backend URL must be the local relay");
    }
    let owned = |name: &OsString| {
        let name = name.to_string_lossy().to_ascii_uppercase();
        name.starts_with("REFORGED_") || name.starts_with("MYSTICPARADOX_")
    };
    let mut env: Vec<(OsString, OsString)> = base.into_iter().filter(|(name, _)| !owned(name)).collect();
    env.push(("REFORGED_BACKEND_URL".into(), backend_url.into()));
    env.push((
        "MYSTICPARADOX_RUNTIME_PROFILE".into(),
        runtime_profile_name(profile).into(),
    ));
    Ok(env)
}

/// How the game runs on Linux: a compatibility tool (Proton or Wine) before the exe, with its environment.
#[derive(Debug, Clone, Default)]
pub struct LaunchRuntime {
    pub command: String,
    pub args_prefix: Vec<String>,
    pub env: Vec<(String, String)>,
}

type Listener = Box<dyn Fn(bool, Option<i32>) + Send + Sync>;

/// Starts a stand-in for the game instead of the exe (tests): (Win64 folder, arguments, runtime, the whole
/// environment) to the stand-in's process id. The stand-in ends with `GameProcess::ended`.
pub type Spawn = Arc<
    dyn Fn(&Path, &[String], Option<&LaunchRuntime>, &[(OsString, OsString)]) -> std::io::Result<u32> + Send + Sync,
>;

/// The one game process this launcher started.
#[derive(Clone, Default)]
pub struct GameProcess {
    pid: Arc<Mutex<Option<u32>>>,
    listeners: Arc<Mutex<Vec<Listener>>>,
    spawn: Option<Spawn>,
}

impl GameProcess {
    pub fn with_spawn(spawn: Option<Spawn>) -> Self {
        Self {
            spawn,
            ..Self::default()
        }
    }

    /// The game a `Spawn` started has exited.
    #[cfg_attr(not(test), expect(dead_code, reason = "the tests use it"))]
    pub fn ended(&self, code: Option<i32>) {
        *self.pid.lock().unwrap_or_else(|poisoned| poisoned.into_inner()) = None;
        self.tell(false, code);
    }

    pub fn running(&self) -> bool {
        self.pid().is_some()
    }

    pub fn pid(&self) -> Option<u32> {
        *self.pid.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    pub fn on_change(&self, listener: impl Fn(bool, Option<i32>) + Send + Sync + 'static) {
        self.listeners
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .push(Box::new(listener));
    }

    fn tell(&self, running: bool, code: Option<i32>) {
        for listener in self
            .listeners
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .iter()
        {
            listener(running, code);
        }
    }

    /// Starts the game from its Win64 folder with exactly `env` (`launch_environment`), plus a Linux
    /// runtime's own variables.
    pub fn start(
        &self,
        win64_dir: &Path,
        args: &[String],
        runtime: Option<&LaunchRuntime>,
        env: &[(OsString, OsString)],
    ) -> std::io::Result<()> {
        let mut pid = self.pid.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        if pid.is_some() {
            return Err(std::io::Error::new(
                std::io::ErrorKind::AlreadyExists,
                "already running",
            ));
        }
        if let Some(spawn) = &self.spawn {
            *pid = Some(spawn(win64_dir, args, runtime, env)?);
            drop(pid);
            self.tell(true, None);
            return Ok(());
        }
        let exe = win64_dir.join(EXE_NAME);
        let mut command = match runtime {
            Some(runtime) => {
                let mut command = Command::new(&runtime.command);
                command.args(&runtime.args_prefix).arg(&exe).args(args);
                command
            }
            None => {
                let mut command = Command::new(&exe);
                command.args(args);
                command
            }
        };
        command
            .current_dir(win64_dir)
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .env_clear()
            .envs(env.iter().map(|(name, value)| (name, value)));
        if let Some(runtime) = runtime {
            command.envs(runtime.env.iter().map(|(name, value)| (name, value)));
        }
        let mut child: Child = command.spawn()?;
        *pid = child.id();
        drop(pid);
        self.tell(true, None);
        let this = self.clone();
        tokio::spawn(async move {
            let code = child.wait().await.ok().and_then(|status| status.code());
            *this.pid.lock().unwrap_or_else(|poisoned| poisoned.into_inner()) = None;
            this.tell(false, code);
        });
        Ok(())
    }
}

/// A hidden helper process (PowerShell, tasklist, pgrep): its output, or None on error or timeout.
pub async fn run_hidden(
    program: &Path,
    args: &[&str],
    timeout: std::time::Duration,
    max_bytes: usize,
) -> Option<String> {
    let mut command = Command::new(program);
    command
        .args(args)
        .stdin(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .kill_on_drop(true);
    #[cfg(windows)]
    command.creation_flags(CREATE_NO_WINDOW);
    let output = tokio::time::timeout(timeout, command.output()).await.ok()?.ok()?;
    if !output.status.success() || output.stdout.len() > max_bytes {
        return None;
    }
    String::from_utf8(output.stdout).ok()
}

fn ids(text: &str) -> Vec<u32> {
    text.lines()
        .map(str::trim)
        .filter(|line| !line.is_empty() && line.bytes().all(|b| b.is_ascii_digit()))
        .filter_map(|line| line.parse().ok())
        .collect()
}

pub fn system_root() -> std::path::PathBuf {
    std::env::var_os("SystemRoot")
        .or_else(|| std::env::var_os("WINDIR"))
        .map_or_else(|| "C:\\Windows".into(), std::path::PathBuf::from)
}

/// Running Dauntless clients. Game servers (the same exe started with `-server`, as on the host's own
/// PC) do not count. Only process ids come back: other processes' command lines hold their keys and
/// never leave PowerShell.
pub async fn find_running_clients() -> Vec<u32> {
    let ten = std::time::Duration::from_secs(10);
    if cfg!(target_os = "linux") {
        return run_hidden(Path::new("pgrep"), &["-f", EXE_NAME], ten, 64 * 1024)
            .await
            .map(|out| ids(&out))
            .unwrap_or_default();
    }
    if !cfg!(windows) {
        return Vec::new();
    }
    let root = system_root();
    let filter = format!("IMAGENAME eq {EXE_NAME}");
    let listed = run_hidden(
        &root.join("System32").join("tasklist.exe"),
        &["/FI", &filter, "/FO", "CSV", "/NH"],
        ten,
        1024 * 1024,
    )
    .await;
    if !listed.is_some_and(|out| out.to_lowercase().contains(&EXE_NAME.to_lowercase())) {
        return Vec::new();
    }
    let script = format!(
        "Get-CimInstance Win32_Process -Filter \"Name='{EXE_NAME}'\" | ForEach-Object {{ if ($_.CommandLine -notmatch '(^|\\s)-server(\\s|$)') {{ $_.ProcessId }} }}"
    );
    let powershell = root
        .join("System32")
        .join("WindowsPowerShell")
        .join("v1.0")
        .join("powershell.exe");
    let args = [
        "-NoProfile",
        "-NonInteractive",
        "-ExecutionPolicy",
        "Bypass",
        "-Command",
        &script,
    ];
    run_hidden(&powershell, &args, std::time::Duration::from_secs(15), 64 * 1024)
        .await
        .map(|out| ids(&out))
        .unwrap_or_default()
}

pub const DIRECTX_FILES: [&str; 3] = ["XINPUT1_3.dll", "X3DAudio1_7.dll", "XAPOFX1_5.dll"];

fn missing_in_system32(files: &[&'static str], exists: impl Fn(&Path) -> bool) -> Vec<&'static str> {
    if !cfg!(windows) {
        return Vec::new();
    }
    let system32 = system_root().join("System32");
    files
        .iter()
        .copied()
        .filter(|file| !exists(&system32.join(file)))
        .collect()
}

/// The legacy DirectX files the game needs, independent of modern DirectX.
pub fn missing_direct_x(exists: impl Fn(&Path) -> bool) -> Vec<&'static str> {
    missing_in_system32(&DIRECTX_FILES, exists)
}

/// The server DLL needs the Visual C++ 2015-2022 runtime (x64).
pub fn missing_vc_runtime(exists: impl Fn(&Path) -> bool) -> Vec<&'static str> {
    missing_in_system32(&crate::core::pins::VC_RUNTIME_FILES, exists)
}

/// The game's console was closed or sent Ctrl+C (Windows ends the process with this status): the player
/// closed it, not a failure.
pub const STATUS_CONTROL_C_EXIT: u32 = 0xC000_013A;

/// An exit code as `0x` and eight uppercase hex digits, signed or unsigned alike.
pub fn game_exit_detail(code: i32) -> String {
    format!("0x{:08X}", code as u32)
}

#[cfg(test)]
mod tests {
    use super::*;

    const KEY: &str = concat!("RRK_", "cccccccccccccccc", "cccccccccccccccc", "cccccccccccccccc");
    const USER_ID: &str = "0b7c2f6e-1d2a-4c55-9d6f-7a1e2b3c4d5e";

    fn settings<'a>(key: &'a str, user_id: &'a str, username: &'a str, windowed: bool) -> LaunchSettings<'a> {
        LaunchSettings {
            key,
            user_id,
            username,
            windowed,
            game_locale: None,
        }
    }

    /// `launch.test.ts`: "launch arguments: the 1.14.7 command line with the key as the exchange code"
    #[test]
    fn the_1_14_7_command_line_with_the_key_as_the_exchange_code() {
        let args = build_launch_args(&LaunchSettings {
            game_locale: Some("pt-BR"),
            ..settings(KEY, USER_ID, "Slayer_42", false)
        })
        .unwrap();
        assert_eq!(
            args,
            [
                "-DisableEOS".to_owned(),
                "-EpicPortal".to_owned(),
                "-NoEAC".to_owned(),
                "-AUTH_TYPE=exchangecode".to_owned(),
                format!("-AUTH_LOGIN={USER_ID}"),
                format!("-AUTH_PASSWORD={KEY}"),
                "-epicapp=Archon".to_owned(),
                "-epicenv=Prod".to_owned(),
                "-epicusername=Slayer_42".to_owned(),
                format!("-epicuserid={USER_ID}"),
                format!("-epicaccountid={USER_ID}"),
                "-epicsandboxid=jackal".to_owned(),
                "-epicdeploymentid=53565ba467df4edbb6f5a3d939a8b4f2".to_owned(),
                "-epiclocale=pt-BR".to_owned(),
            ]
        );
        let windowed = build_launch_args(&LaunchSettings {
            game_locale: Some("xx-YY"),
            ..settings(KEY, "u1", "abc", true)
        })
        .unwrap();
        assert_eq!(
            &windowed[windowed.len() - 3..],
            ["-windowed", "-ResX=1280", "-ResY=720"]
        );
        assert!(
            windowed.iter().any(|arg| arg == "-epiclocale=en-US"),
            "an unknown locale falls back to English"
        );
        assert!(build_launch_args(&settings("nope", "u1", "abc", false)).is_err());
        assert!(build_launch_args(&settings(KEY, "a b", "abc", false)).is_err());
        assert!(build_launch_args(&settings(KEY, "", "abc", false)).is_err());
        assert!(build_launch_args(&settings(KEY, &"u".repeat(129), "abc", false)).is_err());
        assert!(build_launch_args(&settings(KEY, "u1", "a\"b", false)).is_err());
        assert!(build_launch_args(&settings(KEY, "u1", "a-b", false)).is_err());
        assert!(build_launch_args(&settings(KEY, "u1", &"a".repeat(33), false)).is_err());
    }

    /// `launch.test.ts`: "the key never reaches a log line"
    #[test]
    fn the_key_never_reaches_a_log_line() {
        let args = build_launch_args(&settings(KEY, "u1", "abc", false)).unwrap();
        assert!(mask_args(&args).iter().any(|arg| arg == "-AUTH_PASSWORD=<hidden>"));
        let line = describe_launch("game.exe", &args);
        assert!(!line.contains(KEY));
        assert!(line.starts_with(
            "game.exe -DisableEOS -EpicPortal -NoEAC -AUTH_TYPE=exchangecode -AUTH_LOGIN=u1 -AUTH_PASSWORD=<hidden>"
        ));
        assert_eq!(mask_args(&["-auth_password=x".into()]), ["-AUTH_PASSWORD=<hidden>"]);
    }

    fn env(pairs: &[(&str, &str)]) -> Vec<(OsString, OsString)> {
        pairs
            .iter()
            .map(|(name, value)| (OsString::from(name), OsString::from(value)))
            .collect()
    }

    fn get<'a>(env: &'a [(OsString, OsString)], name: &str) -> Option<&'a str> {
        env.iter()
            .find(|(key, _)| key == name)
            .and_then(|(_, value)| value.to_str())
    }

    /// `launch.test.ts`: "launchEnvironment: the runtime is pointed at the local relay and nothing else"
    #[test]
    fn the_runtime_is_pointed_at_the_local_relay_and_nothing_else() {
        let out = launch_environment(
            env(&[
                ("PATH", "x"),
                ("REFORGED_BACKEND_URL", "https://evil.example"),
                ("MYSTICPARADOX_LOG", "1"),
                ("reforged_other", "y"),
            ]),
            "http://127.0.0.1:61100",
            RuntimeProfile::Development,
        )
        .unwrap();
        assert_eq!(get(&out, "REFORGED_BACKEND_URL"), Some("http://127.0.0.1:61100"));
        assert_eq!(get(&out, "MYSTICPARADOX_RUNTIME_PROFILE"), Some("development"));
        assert_eq!(get(&out, "PATH"), Some("x"));
        assert_eq!(get(&out, "MYSTICPARADOX_LOG"), None);
        assert_eq!(get(&out, "reforged_other"), None);
        assert_eq!(out.len(), 3, "the inherited REFORGED_BACKEND_URL is gone, not doubled");
        for bad in [
            "http://192.168.1.2:61100",
            "https://127.0.0.1:61100",
            "http://127.0.0.1:0",
            "http://127.0.0.1:061100",
            "http://127.0.0.1:611000",
            "http://127.0.0.1:61100/",
            "http://127.0.0.1",
        ] {
            assert!(
                launch_environment(env(&[]), bad, RuntimeProfile::Production).is_err(),
                "{bad}"
            );
        }
        assert_eq!(
            get(
                &launch_environment(env(&[]), "http://127.0.0.1:1", RuntimeProfile::Production).unwrap(),
                "MYSTICPARADOX_RUNTIME_PROFILE"
            ),
            Some("production")
        );
    }

    #[test]
    fn exit_codes() {
        assert_eq!(game_exit_detail(-1_073_741_515), "0xC0000135");
        assert_eq!(game_exit_detail(STATUS_CONTROL_C_EXIT as i32), "0xC000013A");
        assert_eq!(game_exit_detail(1), "0x00000001");
        if cfg!(windows) {
            assert_eq!(
                missing_direct_x(|path| path.to_string_lossy().contains("XINPUT1_3")),
                ["X3DAudio1_7.dll", "XAPOFX1_5.dll"]
            );
            assert!(missing_direct_x(|_| true).is_empty());
        } else {
            assert!(missing_direct_x(|_| false).is_empty());
        }
    }

    #[test]
    fn a_stand_in_gets_the_arguments_and_the_whole_environment() {
        let seen = Arc::new(Mutex::new(None));
        let record = seen.clone();
        let spawn: Spawn = Arc::new(move |dir, args, runtime, env| {
            *record.lock().unwrap() = Some((dir.to_path_buf(), args.to_vec(), runtime.is_some(), env.to_vec()));
            Ok(42)
        });
        let game = GameProcess::with_spawn(Some(spawn));
        let environment = launch_environment(
            env(&[("PATH", "x")]),
            "http://127.0.0.1:61100",
            RuntimeProfile::Production,
        )
        .unwrap();
        game.start(Path::new("win64"), &["-NoEAC".into()], None, &environment)
            .unwrap();
        assert_eq!(game.pid(), Some(42));
        assert!(
            game.start(Path::new("win64"), &[], None, &environment).is_err(),
            "one game at a time"
        );
        let (dir, args, runtime, env) = seen.lock().unwrap().take().unwrap();
        assert_eq!(
            (dir.as_path(), args.as_slice(), runtime),
            (Path::new("win64"), ["-NoEAC".to_owned()].as_slice(), false)
        );
        assert_eq!(env, environment);
        game.ended(Some(0));
        assert!(!game.running());
    }

    #[tokio::test]
    async fn the_game_process_is_tracked_and_started_once() {
        let dir = tempfile::tempdir().unwrap();
        // Any program stands in for the game: the runtime prefix runs it instead of the exe. It exits with
        // the code the launch environment gives it
        let (command, prefix) = if cfg!(windows) {
            (
                system_root()
                    .join("System32")
                    .join("cmd.exe")
                    .to_string_lossy()
                    .into_owned(),
                vec!["/C".to_owned(), "exit %RR_TEST_CODE%".to_owned(), "rem".to_owned()],
            )
        } else {
            ("sh".to_owned(), vec!["-c".to_owned(), "exit $RR_TEST_CODE".to_owned()])
        };
        let runtime = LaunchRuntime {
            command,
            args_prefix: prefix,
            env: Vec::new(),
        };
        let mut environment = launch_environment(
            std::env::vars_os(),
            "http://127.0.0.1:61100",
            RuntimeProfile::Production,
        )
        .unwrap();
        environment.push(("RR_TEST_CODE".into(), "3".into()));
        let game = GameProcess::default();
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        game.on_change(move |running, code| {
            let _ = tx.send((running, code));
        });
        game.start(dir.path(), &[], Some(&runtime), &environment).unwrap();
        assert!(
            game.start(dir.path(), &[], Some(&runtime), &environment).is_err() || !game.running(),
            "refuses to start twice while running"
        );
        assert_eq!(rx.recv().await, Some((true, None)));
        assert_eq!(rx.recv().await, Some((false, Some(3))));
        assert!(!game.running());
    }
}
