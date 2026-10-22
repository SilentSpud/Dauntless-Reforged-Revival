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

/// Points the `reforged-revival:` scheme at this launcher (packaged builds only: a development build must
/// not take over the scheme).
pub fn register_scheme(exe: &std::path::Path) {
    let program = exe.to_string_lossy();
    if cfg!(windows) {
        let reg = crate::platform::launch::system_root().join("System32").join("reg.exe");
        let key = format!(r"HKCU\Software\Classes\{INVITE_SCHEME}");
        let command = format!("\"{program}\" \"%1\"");
        let command_key = format!(r"{key}\shell\open\command");
        let steps: [&[&str]; 3] = [
            &["add", &key, "/ve", "/d", "URL:Dauntless Reforged Revival invite", "/f"],
            &["add", &key, "/v", "URL Protocol", "/d", "", "/f"],
            &["add", &command_key, "/ve", "/d", &command, "/f"],
        ];
        for args in steps {
            let ok = std::process::Command::new(&reg)
                .args(args)
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .status()
                .is_ok_and(|status| status.success());
            if !ok {
                log::warn("could not register the invite link handler");
                return;
            }
        }
        return;
    }
    // The AppImage moves with its file; deb and rpm packages install their own desktop entry
    let Some(appimage) = std::env::var_os("APPIMAGE") else {
        return;
    };
    let Some(applications) = dirs::data_dir().map(|dir| dir.join("applications")) else {
        return;
    };
    let entry = format!(
        "[Desktop Entry]\nType=Application\nName=Dauntless Reforged Revival Launcher\nExec=\"{}\" %u\nMimeType=x-scheme-handler/{INVITE_SCHEME};\nNoDisplay=true\nTerminal=false\n",
        std::path::Path::new(&appimage).display()
    );
    let file = applications.join("reforged-revival-launcher-handler.desktop");
    let written = std::fs::create_dir_all(&applications).is_ok() && std::fs::write(&file, entry).is_ok();
    let registered = written
        && std::process::Command::new("xdg-mime")
            .args([
                "default",
                "reforged-revival-launcher-handler.desktop",
                &format!("x-scheme-handler/{INVITE_SCHEME}"),
            ])
            .status()
            .is_ok_and(|status| status.success());
    if !registered {
        log::warn("could not register the invite link handler");
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
}
