//! Starting the game (shared contract 6, row L31): `src/main/launch.ts` and `windows-runtime.ts`. The
//! account key goes on the command line as `-AUTH_PASSWORD`, as the upstream launcher and
//! `friend-kit/play.ps1` pass it; it is never logged (`mask_args`).

use std::path::Path;
use std::sync::{Arc, Mutex};

use tokio::process::{Child, Command};

use crate::core::game_language::launch_locale;
use crate::core::invite::is_valid_host;
use crate::core::pins::EXE_NAME;
use crate::core::username::is_plausible_account_key;

pub const FIXED_ARGS: [&str; 12] = [
    "-AUTH_LOGIN=unused",
    "-AUTH_TYPE=exchangecode",
    "-epicapp=appidlol",
    "-epicenv=Prod",
    "-EpicPortal",
    "-epicusername=usernamelol",
    "-epicuserid=useridlol",
    "-epiclocale=en-US",
    "-epicsandboxid=sandboxidlol",
    "-epicdeploymentid=deploymentidlol",
    // The pinned 1.4.4 executable supports this client-mode switch; private servers do not use EAC
    "-NoEAC",
    "-RamsStore",
];

/// Windows' "start without a console window" process flag.
#[cfg(windows)]
const CREATE_NO_WINDOW: u32 = 0x0800_0000;

pub struct LaunchSettings<'a> {
    pub host: &'a str,
    pub port: u16,
    pub key: &'a str,
    pub windowed: bool,
    /// `-epiclocale`; English when left out
    pub game_locale: Option<&'a str>,
}

/// The game's arguments. The first is the server address, which UndauntedInternalServer.dll reads.
pub fn build_launch_args(settings: &LaunchSettings<'_>) -> Result<Vec<String>, &'static str> {
    if !is_valid_host(settings.host) {
        return Err("invalid host");
    }
    if settings.port == 0 {
        return Err("invalid port");
    }
    if !is_plausible_account_key(settings.key) {
        return Err("invalid key");
    }
    let locale = launch_locale(settings.game_locale);
    let mut args = vec![
        format!("{}:{}", settings.host, settings.port),
        format!("-AUTH_PASSWORD={}", settings.key),
    ];
    args.extend(FIXED_ARGS.iter().map(|arg| {
        if arg.starts_with("-epiclocale=") {
            format!("-epiclocale={locale}")
        } else {
            (*arg).to_owned()
        }
    }));
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

/// How the game runs on Linux: a compatibility tool (Proton or Wine) before the exe, with its environment.
#[derive(Debug, Clone, Default)]
pub struct LaunchRuntime {
    pub command: String,
    pub args_prefix: Vec<String>,
    pub env: Vec<(String, String)>,
}

type Listener = Box<dyn Fn(bool, Option<i32>) + Send + Sync>;

/// Starts a stand-in for the game instead of the exe (tests): (Win64 folder, arguments, runtime, extra
/// environment) to the stand-in's process id. The stand-in ends with `GameProcess::ended`.
pub type Spawn =
    Arc<dyn Fn(&Path, &[String], Option<&LaunchRuntime>, &[(&str, &str)]) -> std::io::Result<u32> + Send + Sync>;

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

    /// Starts the game from its Win64 folder; `extra_env` is added to its environment
    /// (`DR_SHOW_CONSOLE=1` keeps the DLL's log window visible).
    pub fn start(
        &self,
        win64_dir: &Path,
        args: &[String],
        runtime: Option<&LaunchRuntime>,
        extra_env: &[(&str, &str)],
    ) -> std::io::Result<()> {
        let mut pid = self.pid.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        if pid.is_some() {
            return Err(std::io::Error::new(
                std::io::ErrorKind::AlreadyExists,
                "already running",
            ));
        }
        if let Some(spawn) = &self.spawn {
            *pid = Some(spawn(win64_dir, args, runtime, extra_env)?);
            drop(pid);
            self.tell(true, None);
            return Ok(());
        }
        let exe = win64_dir.join(EXE_NAME);
        let mut command = match runtime {
            Some(runtime) => {
                let mut command = Command::new(&runtime.command);
                command
                    .args(&runtime.args_prefix)
                    .arg(&exe)
                    .args(args)
                    .envs(runtime.env.iter().map(|(k, v)| (k, v)));
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
            .stderr(std::process::Stdio::null());
        command.envs(extra_env.iter().copied());
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

/// What a non-zero exit code means for the player, signed or unsigned alike.
pub fn game_exit_detail(code: i32) -> String {
    let unsigned = code as u32;
    let hex = format!("0x{unsigned:08X}");
    match unsigned {
        0xC000_0135 => format!(
            "{hex}: A required DLL is missing. Install the Microsoft Visual C++ x64 and DirectX runtimes shown below, then retry."
        ),
        0xC000_007B => format!(
            "{hex}: Windows could not load a game dependency. Repair the Microsoft x64 runtimes and use Repair game files."
        ),
        _ => format!(
            "{hex}: The game closed unexpectedly. Use Repair game files; if it repeats, send the latest Archon game log to support."
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const KEY: &str = concat!("UUK_", "0123456789abcdef", "0123456789abcdef", "0123456789abcdef");

    fn settings(host: &str, port: u16, key: &'static str, windowed: bool) -> LaunchSettings<'static> {
        LaunchSettings {
            host: Box::leak(host.to_owned().into_boxed_str()),
            port,
            key,
            windowed,
            game_locale: None,
        }
    }

    #[test]
    fn launch_args_follow_contract_6() {
        let args = build_launch_args(&settings("100.64.0.7", 61000, KEY, false)).unwrap();
        let mut expected = vec!["100.64.0.7:61000".to_owned(), format!("-AUTH_PASSWORD={KEY}")];
        expected.extend(FIXED_ARGS.map(str::to_owned));
        assert_eq!(args, expected);
        let windowed = build_launch_args(&settings("hostpc.ts.net", 61000, KEY, true)).unwrap();
        assert_eq!(
            &windowed[windowed.len() - 3..],
            ["-windowed", "-ResX=1280", "-ResY=720"]
        );
        assert!(build_launch_args(&settings("100.64.0.7", 61000, "bad key with spaces", false)).is_err());
        assert!(build_launch_args(&settings("bad host", 61000, KEY, false)).is_err());
        assert!(build_launch_args(&settings("100.64.0.7", 0, KEY, false)).is_err());
        let locale = |game_locale| {
            let args = build_launch_args(&LaunchSettings {
                game_locale,
                ..settings("100.64.0.7", 61000, KEY, false)
            })
            .unwrap();
            args.into_iter()
                .filter(|arg| arg.starts_with("-epiclocale="))
                .collect::<Vec<_>>()
        };
        assert_eq!(locale(Some("pt-BR")), ["-epiclocale=pt-BR"]);
        assert_eq!(locale(Some("xx-XX")), ["-epiclocale=en-US"]);
    }

    #[test]
    fn the_key_is_masked_in_anything_that_can_be_logged() {
        let args = build_launch_args(&settings("127.0.0.1", 61000, KEY, true)).unwrap();
        assert_eq!(mask_args(&args)[1], "-AUTH_PASSWORD=<hidden>");
        let line = describe_launch(EXE_NAME, &args);
        assert!(!line.contains(KEY));
        assert!(
            line.starts_with("Dauntless-Win64-Shipping.exe 127.0.0.1:61000 -AUTH_PASSWORD=<hidden> -AUTH_LOGIN=unused")
        );
    }

    #[test]
    fn runtime_checks_and_exit_codes() {
        assert_eq!(
            game_exit_detail(-1_073_741_515),
            game_exit_detail(0xC000_0135_u32 as i32)
        );
        assert!(game_exit_detail(0xC000_0135_u32 as i32).starts_with("0xC0000135: A required DLL is missing"));
        assert!(game_exit_detail(1).starts_with("0x00000001:"));
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

    #[tokio::test]
    async fn the_game_process_is_tracked_and_started_once() {
        let dir = tempfile::tempdir().unwrap();
        // Any program stands in for the game: the runtime prefix runs it instead of the exe
        let (command, prefix) = if cfg!(windows) {
            (
                system_root()
                    .join("System32")
                    .join("cmd.exe")
                    .to_string_lossy()
                    .into_owned(),
                vec!["/C".to_owned(), "exit 3".to_owned(), "rem".to_owned()],
            )
        } else {
            ("sh".to_owned(), vec!["-c".to_owned(), "exit 3".to_owned()])
        };
        let runtime = LaunchRuntime {
            command,
            args_prefix: prefix,
            env: Vec::new(),
        };
        let game = GameProcess::default();
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        game.on_change(move |running, code| {
            let _ = tx.send((running, code));
        });
        game.start(dir.path(), &[], Some(&runtime), &[("DR_SHOW_CONSOLE", "1")])
            .unwrap();
        assert!(
            game.start(dir.path(), &[], Some(&runtime), &[]).is_err() || !game.running(),
            "refuses to start twice while running"
        );
        assert_eq!(rx.recv().await, Some((true, None)));
        assert_eq!(rx.recv().await, Some((false, Some(3))));
        assert!(!game.running());
    }
}
