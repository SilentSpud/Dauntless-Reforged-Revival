//! Invite links and the single running launcher (row L34): `inviteFromArgv`, `requestSingleInstanceLock`
//! and `setAsDefaultProtocolClient` in `src/main.ts`, without plugins (tauri-plugin-single-instance
//! brings zbus 5 on Linux, tauri-plugin-deep-link a CC0 crate the licence list does not take).
//!
//! The first launcher listens on a per-user channel (a named pipe on Windows, a socket in
//! `$XDG_RUNTIME_DIR` on Linux); a second one sends it its arguments and exits, and the first focuses its
//! window and takes the invite link from them. In a packaged build the `reforged-revival:` scheme points
//! at this launcher (the current user's registry on Windows, a desktop entry for the AppImage on Linux;
//! deb and rpm install their own).

use std::future::Future;
use std::path::Path;
use std::pin::Pin;
use std::sync::Arc;

use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

use crate::core::invite::parse_invite;
use crate::core::text::js_trim;
use crate::platform::log;

pub const INVITE_SCHEME: &str = "reforged-revival";

/// The invite link among a launcher's arguments, if it is a valid one.
pub fn invite_from_args<S: AsRef<str>>(args: &[S]) -> Option<String> {
    let link = args.iter().map(AsRef::as_ref).find(|arg| {
        arg.get(..INVITE_SCHEME.len() + 1)
            .is_some_and(|head| head.eq_ignore_ascii_case(&format!("{INVITE_SCHEME}:")))
    })?;
    parse_invite(link).ok().map(|_| js_trim(link).to_owned())
}

pub type OnArgs = Arc<dyn Fn(Vec<String>) -> Pin<Box<dyn Future<Output = ()> + Send>> + Send + Sync>;

/// The channel's name for this user.
pub fn channel_name() -> String {
    let id = "io.github.freezamotoqueiro.dauntlessreforgedrevival";
    if cfg!(windows) {
        let user: String = std::env::var("USERNAME")
            .unwrap_or_default()
            .chars()
            .filter(char::is_ascii_alphanumeric)
            .collect();
        format!(r"\\.\pipe\{id}-{user}")
    } else {
        let runtime = std::env::var_os("XDG_RUNTIME_DIR")
            .map(std::path::PathBuf::from)
            .filter(|dir| dir.is_dir());
        let user = std::env::var("USER").unwrap_or_default();
        runtime
            .map_or_else(
                || std::env::temp_dir().join(format!("{id}-{user}.sock")),
                |dir| dir.join(format!("{id}.sock")),
            )
            .to_string_lossy()
            .into_owned()
    }
}

#[derive(Debug, PartialEq, Eq)]
pub enum Claim {
    /// This launcher is the only one; it now listens for others
    First,
    /// Another launcher took the arguments; this one should exit
    Handed,
}

fn encode(args: &[String]) -> String {
    format!("{}\n", serde_json::to_string(args).unwrap_or_else(|_| "[]".into()))
}

fn decode(line: &str) -> Vec<String> {
    serde_json::from_str(line.trim()).unwrap_or_default()
}

#[cfg(windows)]
mod channel {
    use std::time::Duration;

    use tokio::net::windows::named_pipe::{ClientOptions, ServerOptions};

    use super::*;

    async fn send(name: &str, args: &[String]) -> bool {
        for _ in 0..20 {
            match ClientOptions::new().open(name) {
                Ok(mut pipe) => return pipe.write_all(encode(args).as_bytes()).await.is_ok(),
                // All instances busy: the first launcher is between connections
                Err(error) if error.raw_os_error() == Some(231) => tokio::time::sleep(Duration::from_millis(50)).await,
                Err(_) => return false,
            }
        }
        false
    }

    pub async fn claim(name: &str, args: &[String], on_args: OnArgs) -> Claim {
        if send(name, args).await {
            return Claim::Handed;
        }
        let Ok(mut server) = ServerOptions::new().first_pipe_instance(true).create(name) else {
            // Someone else just became the first launcher
            return if send(name, args).await {
                Claim::Handed
            } else {
                Claim::First
            };
        };
        let name = name.to_owned();
        tokio::spawn(async move {
            loop {
                if server.connect().await.is_err() {
                    continue;
                }
                let connected = server;
                server = match ServerOptions::new().create(&name) {
                    Ok(next) => next,
                    Err(error) => {
                        log::warn(&format!("single launcher channel stopped: {error}"));
                        return;
                    }
                };
                let on_args = on_args.clone();
                tokio::spawn(async move {
                    let mut line = String::new();
                    if BufReader::new(connected).read_line(&mut line).await.is_ok() {
                        on_args(decode(&line)).await;
                    }
                });
            }
        });
        Claim::First
    }
}

#[cfg(unix)]
mod channel {
    use tokio::net::{UnixListener, UnixStream};

    use super::*;

    async fn send(name: &str, args: &[String]) -> bool {
        match UnixStream::connect(name).await {
            Ok(mut stream) => stream.write_all(encode(args).as_bytes()).await.is_ok(),
            Err(_) => false,
        }
    }

    pub async fn claim(name: &str, args: &[String], on_args: OnArgs) -> Claim {
        if send(name, args).await {
            return Claim::Handed;
        }
        // Nobody answers: a socket file left by a launcher that stopped
        let _ = std::fs::remove_file(name);
        let Ok(listener) = UnixListener::bind(name) else {
            return if send(name, args).await {
                Claim::Handed
            } else {
                Claim::First
            };
        };
        {
            use std::os::unix::fs::PermissionsExt as _;
            let _ = std::fs::set_permissions(name, std::fs::Permissions::from_mode(0o600));
        }
        tokio::spawn(async move {
            loop {
                let Ok((stream, _)) = listener.accept().await else {
                    continue;
                };
                let on_args = on_args.clone();
                tokio::spawn(async move {
                    let mut line = String::new();
                    if BufReader::new(stream).read_line(&mut line).await.is_ok() {
                        on_args(decode(&line)).await;
                    }
                });
            }
        });
        Claim::First
    }
}

/// Becomes the first launcher on `name`, or hands `args` to the one already running.
pub async fn claim(name: &str, args: &[String], on_args: OnArgs) -> Claim {
    channel::claim(name, args, on_args).await
}

// ---------------------------------------------------------------- the scheme

/// What the current user's registry gets on Windows, as `setAsDefaultProtocolClient` writes it:
/// `HKCU\Software\Classes\reforged-revival` and its open command. (key, value name or None for the
/// default value, data)
pub fn registry_entries(program: &Path) -> Vec<(String, Option<&'static str>, String)> {
    let key = format!(r"Software\Classes\{INVITE_SCHEME}");
    vec![
        (key.clone(), None, "URL:Dauntless Reforged Revival invite".to_owned()),
        (key.clone(), Some("URL Protocol"), String::new()),
        (
            format!(r"{key}\shell\open\command"),
            None,
            format!("\"{}\" \"%1\"", program.display()),
        ),
    ]
}

/// The current user's registry (`HKEY_CURRENT_USER`), as far as registering the scheme needs it.
pub trait Registry {
    /// Sets a string value under `key` (relative to `HKEY_CURRENT_USER`), creating the key.
    fn set_string(&mut self, key: &str, name: Option<&str>, data: &str) -> std::io::Result<()>;
}

/// `reg.exe` from System32, so no registry crate or feature comes in.
pub struct RegExe;

impl Registry for RegExe {
    fn set_string(&mut self, key: &str, name: Option<&str>, data: &str) -> std::io::Result<()> {
        let reg = crate::platform::launch::system_root().join("System32").join("reg.exe");
        let key = format!(r"HKCU\{key}");
        let mut args = vec!["add", key.as_str()];
        match name {
            Some(name) => args.extend(["/v", name]),
            None => args.push("/ve"),
        }
        args.extend(["/t", "REG_SZ", "/d", data, "/f"]);
        let status = std::process::Command::new(reg)
            .args(args)
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()?;
        if status.success() {
            Ok(())
        } else {
            Err(std::io::Error::other(format!("reg.exe failed for {key}")))
        }
    }
}

/// Windows: the scheme's keys, stopping at the first that fails.
pub fn register_windows(registry: &mut dyn Registry, program: &Path) -> std::io::Result<()> {
    for (key, name, data) in registry_entries(program) {
        registry.set_string(&key, name, &data)?;
    }
    Ok(())
}

pub const DESKTOP_FILE: &str = "reforged-revival-launcher-handler.desktop";

/// An `Exec` argument: quoted, with `"`, `` ` ``, `$` and `\` escaped, then every `\` doubled for the
/// string value and every `%` doubled for the field codes (the Desktop Entry Specification's rules).
fn exec_argument(path: &str) -> String {
    let mut quoted = String::from("\"");
    for c in path.chars() {
        if matches!(c, '"' | '`' | '$' | '\\') {
            quoted.push('\\');
        }
        quoted.push(c);
    }
    quoted.push('"');
    quoted.replace('\\', "\\\\").replace('%', "%%")
}

/// The AppImage's handler entry, or None for a path a desktop entry cannot hold.
pub fn desktop_entry(appimage: &Path) -> Option<String> {
    let path = appimage.to_str()?;
    if !path.starts_with('/') || path.chars().any(char::is_control) {
        return None;
    }
    Some(format!(
        "[Desktop Entry]\nType=Application\nName=Dauntless Reforged Revival Launcher\nExec={} %u\nMimeType=x-scheme-handler/{INVITE_SCHEME};\nNoDisplay=true\nTerminal=false\n",
        exec_argument(path)
    ))
}

/// Linux (the AppImage): the handler entry in `applications`, then `set_default(desktop file, MIME
/// type)`, which is `xdg-mime default` outside the tests.
pub fn register_linux(
    applications: &Path,
    appimage: &Path,
    set_default: &mut dyn FnMut(&str, &str) -> bool,
) -> std::io::Result<()> {
    let entry = desktop_entry(appimage).ok_or_else(|| {
        std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "the AppImage's path cannot go in a desktop entry",
        )
    })?;
    std::fs::create_dir_all(applications)?;
    std::fs::write(applications.join(DESKTOP_FILE), entry)?;
    if set_default(DESKTOP_FILE, &format!("x-scheme-handler/{INVITE_SCHEME}")) {
        Ok(())
    } else {
        Err(std::io::Error::other("xdg-mime failed"))
    }
}

fn xdg_mime_default(desktop_file: &str, mime_type: &str) -> bool {
    std::process::Command::new("xdg-mime")
        .args(["default", desktop_file, mime_type])
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .is_ok_and(|status| status.success())
}

/// Points the `reforged-revival:` scheme at this launcher (packaged builds only: a development build must
/// not take over the scheme). On Linux only the AppImage registers itself, since it moves with its file;
/// deb and rpm packages install their own desktop entry.
pub fn register_scheme(exe: &Path) {
    let result = if cfg!(windows) {
        register_windows(&mut RegExe, exe)
    } else {
        let Some(appimage) = std::env::var_os("APPIMAGE") else {
            return;
        };
        let Some(applications) = dirs::data_dir().map(|dir| dir.join("applications")) else {
            return;
        };
        register_linux(&applications, Path::new(&appimage), &mut xdg_mime_default)
    };
    if let Err(error) = result {
        log::warn(&format!("could not register the invite link handler: {error}"));
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::*;

    const LINK: &str = "reforged-revival://join?v=1&host=100.64.0.1&port=61000&code=abcd&name=x";

    #[test]
    fn the_invite_link_among_the_arguments() {
        assert_eq!(invite_from_args(&["launcher.exe", LINK]).as_deref(), Some(LINK));
        assert_eq!(
            invite_from_args(&[
                "launcher.exe",
                "REFORGED-REVIVAL://join?v=1&host=100.64.0.1&port=61000&code=abcd&name=x "
            ])
            .as_deref(),
            Some("REFORGED-REVIVAL://join?v=1&host=100.64.0.1&port=61000&code=abcd&name=x")
        );
        assert_eq!(
            invite_from_args(&["launcher.exe", "reforged-revival://join?v=9"]),
            None,
            "the first link counts, and it must be valid"
        );
        assert_eq!(invite_from_args(&["launcher.exe", "--flag"]), None);
    }

    #[tokio::test]
    async fn a_second_launcher_hands_its_arguments_to_the_first() {
        let name = if cfg!(windows) {
            format!(r"\\.\pipe\rr-launcher-test-{}", std::process::id())
        } else {
            std::env::temp_dir()
                .join(format!("rr-launcher-test-{}.sock", std::process::id()))
                .to_string_lossy()
                .into_owned()
        };
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        let on_args: OnArgs = Arc::new(move |args| {
            let tx = tx.clone();
            Box::pin(async move {
                let _ = tx.send(args);
            })
        });
        assert_eq!(claim(&name, &["first".into()], on_args.clone()).await, Claim::First);
        for _ in 0..2 {
            assert_eq!(
                claim(&name, &["second".into(), LINK.into()], on_args.clone()).await,
                Claim::Handed
            );
            let got = tokio::time::timeout(Duration::from_secs(5), rx.recv())
                .await
                .unwrap()
                .unwrap();
            assert_eq!(invite_from_args(&got).as_deref(), Some(LINK));
        }
    }

    /// Records what would be written, failing at one key when asked to.
    #[derive(Default)]
    struct FakeRegistry {
        values: Vec<(String, Option<String>, String)>,
        fail_at: Option<usize>,
    }

    impl Registry for FakeRegistry {
        fn set_string(&mut self, key: &str, name: Option<&str>, data: &str) -> std::io::Result<()> {
            if self.fail_at == Some(self.values.len()) {
                return Err(std::io::Error::other("denied"));
            }
            self.values
                .push((key.to_owned(), name.map(str::to_owned), data.to_owned()));
            Ok(())
        }
    }

    #[test]
    fn windows_registers_the_scheme_for_the_current_user() {
        let mut registry = FakeRegistry::default();
        let exe = Path::new(r"C:\Users\me\AppData\Local\Dauntless Reforged Revival Launcher\launcher.exe");
        register_windows(&mut registry, exe).unwrap();
        let key = r"Software\Classes\reforged-revival";
        assert_eq!(
            registry.values,
            [
                (key.to_owned(), None, "URL:Dauntless Reforged Revival invite".to_owned()),
                (key.to_owned(), Some("URL Protocol".to_owned()), String::new()),
                (
                    format!(r"{key}\shell\open\command"),
                    None,
                    format!("\"{}\" \"%1\"", exe.display())
                ),
            ]
        );
        let mut failing = FakeRegistry {
            fail_at: Some(1),
            ..FakeRegistry::default()
        };
        assert!(register_windows(&mut failing, exe).is_err());
        assert_eq!(failing.values.len(), 1, "stops at the first key that fails");
    }

    #[test]
    fn linux_writes_a_handler_entry_for_the_appimage() {
        let dir = tempfile::tempdir().unwrap();
        let applications = dir.path().join("share").join("applications");
        let mut calls = Vec::new();
        let mut set_default = |file: &str, mime: &str| {
            calls.push((file.to_owned(), mime.to_owned()));
            true
        };
        register_linux(
            &applications,
            Path::new("/home/me/Apps/Reforged Launcher.AppImage"),
            &mut set_default,
        )
        .unwrap();
        assert_eq!(
            calls,
            [(DESKTOP_FILE.to_owned(), "x-scheme-handler/reforged-revival".to_owned())]
        );
        assert_eq!(
            std::fs::read_to_string(applications.join(DESKTOP_FILE)).unwrap(),
            "[Desktop Entry]\nType=Application\nName=Dauntless Reforged Revival Launcher\nExec=\"/home/me/Apps/Reforged Launcher.AppImage\" %u\nMimeType=x-scheme-handler/reforged-revival;\nNoDisplay=true\nTerminal=false\n"
        );
        let mut refuse = |_: &str, _: &str| false;
        assert!(register_linux(&applications, Path::new("/x.AppImage"), &mut refuse).is_err());
        let mut never = |_: &str, _: &str| -> bool { unreachable!() };
        assert!(register_linux(&applications, Path::new("relative.AppImage"), &mut never).is_err());
    }

    #[test]
    fn desktop_entries_escape_the_path() {
        let exec = |path: &str| {
            desktop_entry(Path::new(path)).map(|entry| {
                entry
                    .lines()
                    .find_map(|line| line.strip_prefix("Exec="))
                    .unwrap()
                    .to_owned()
            })
        };
        assert_eq!(
            exec(r#"/a/$b`c"d\e%f.AppImage"#).as_deref(),
            Some(r#""/a/\\$b\\`c\\"d\\\\e%%f.AppImage" %u"#)
        );
        assert_eq!(exec("/a/b\nExec=evil"), None);
        assert_eq!(exec("relative.AppImage"), None);
    }
}
