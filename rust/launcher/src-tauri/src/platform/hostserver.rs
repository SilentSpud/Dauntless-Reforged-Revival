//! The server a host starts and stops from the Host page, and the host kit's one-shot commands
//! (Reforged's `src/main/hostserver.ts`), for the planned layout only (RD23): `reforged-host` from the
//! workspace, never `npm` or `node dist/cli.js`, so nothing is built first.
//!
//! The server (`reforged-host start`) stays a child of the launcher. Windows has no signal one program
//! can send another to stop it as Ctrl+C does, so it runs with `REFORGED_CONTROL=stdin` and reads a
//! `stop` line (RH2): it stops every service and exits, and only when it does not within 45 s is its
//! process tree ended. Its stdin closing (the launcher quit) stops it too. The line
//! `REFORGED_SERVER_READY` means it runs (it is not shown), and something answering on 127.0.0.1:61000
//! that the launcher did not start is a server started elsewhere (`external`).

use std::collections::VecDeque;
use std::ffi::OsString;
use std::future::Future;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::pin::Pin;
use std::process::Stdio;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Duration;

use tokio::io::AsyncWriteExt;
use tokio::process::{ChildStdin, Command};
use tokio::sync::watch;

use crate::bridge::types::{HostInvite, HostServerState, HostServerView};
use crate::platform::hostkit::parse::{clean_line, failure_line};
use crate::platform::hostkit::{WorkspacePaths, find_host_binary, pump};

/// The host kit's contract (`reforged-host start`, RH2).
pub const CONTROL_ENV: &str = "REFORGED_CONTROL";
pub const SERVER_READY_LINE: &str = "REFORGED_SERVER_READY";
/// The backend's port: something answers there when a server runs on this PC.
pub const BACKEND_PORT: u16 = 61000;
pub const STOP_TIMEOUT: Duration = Duration::from_secs(45);
const KILL_GRACE: Duration = Duration::from_secs(5);
const PROBE_TIMEOUT: Duration = Duration::from_millis(700);
/// The page hears of new output at most this often; of a state change at once.
const CHANGE_EVERY: Duration = Duration::from_millis(250);
const MAX_LINES: usize = 300;
#[cfg(windows)]
const CREATE_NO_WINDOW: u32 = 0x0800_0000;

type Boxed<T> = Pin<Box<dyn Future<Output = T> + Send>>;
pub type Probe = Arc<dyn Fn() -> Boxed<bool> + Send + Sync>;
pub type Listener = Arc<dyn Fn(HostServerView) + Send + Sync>;

/// How the host kit runs: `reforged-host` in the workspace (tests: a stand-in, with arguments of its
/// own first).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HostTool {
    pub program: PathBuf,
    pub prefix: Vec<OsString>,
}

impl HostTool {
    pub fn find(workspace: &Path) -> Option<Self> {
        find_host_binary(workspace).map(|program| Self {
            program,
            prefix: Vec::new(),
        })
    }
}

/// Why the host kit could not be run.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HostKitError {
    /// No `reforged-host` in the workspace
    Missing,
    /// It would not start (the OS's reason)
    Spawn(String),
}

impl HostKitError {
    pub fn detail(&self) -> String {
        match self {
            Self::Missing => "hostkit_missing".into(),
            Self::Spawn(reason) => format!("hostkit_start:{}", crate::platform::hostkit::parse::cut(reason, 200)),
        }
    }
}

/// A one-shot command's exit code, its own lines and why it failed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommandOutput {
    pub code: i32,
    pub lines: Vec<String>,
    pub failure: Option<String>,
}

/// What a line of output is to the page and the log.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Shown {
    /// On the page and in the log
    Yes,
    /// Neither: the ready marker, or a secret (an invite's JSON line)
    No,
}

struct Running {
    pid: u32,
    stdin: Option<ChildStdin>,
    exited: watch::Receiver<bool>,
}

struct St {
    state: HostServerState,
    message: Option<String>,
    lines: VecDeque<String>,
    invite: Option<HostInvite>,
    child: Option<Running>,
    stop_requested: bool,
}

struct Inner {
    st: Mutex<St>,
    probe: Probe,
    listener: Mutex<Option<Listener>>,
    stop_timeout: Duration,
    change_pending: AtomicBool,
}

/// The server this launcher runs, and the host kit commands it starts.
#[derive(Clone)]
pub struct HostServer {
    inner: Arc<Inner>,
}

impl Default for HostServer {
    fn default() -> Self {
        Self::new(Arc::new(|| Box::pin(port_answers(BACKEND_PORT))), STOP_TIMEOUT)
    }
}

/// Is something listening on 127.0.0.1:`port`? (700 ms)
pub async fn port_answers(port: u16) -> bool {
    let connect = tokio::net::TcpStream::connect(("127.0.0.1", port));
    matches!(tokio::time::timeout(PROBE_TIMEOUT, connect).await, Ok(Ok(_)))
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// What the child gets besides this process's environment: no colours.
fn command(tool: &HostTool, args: &[String], cwd: &Path, extra: &[(&str, &str)]) -> Command {
    let mut command = Command::new(&tool.program);
    command
        .args(&tool.prefix)
        .args(args)
        .current_dir(cwd)
        .env("NO_COLOR", "1")
        .env("FORCE_COLOR", "0")
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    for (key, value) in extra {
        command.env(key, value);
    }
    #[cfg(windows)]
    command.creation_flags(CREATE_NO_WINDOW);
    command
}

/// Ends a process and everything it started.
pub async fn kill_tree(pid: u32) {
    let pid = pid.to_string();
    // On Linux start runs in a process group of its own (below): the whole group goes.
    let group = format!("-{pid}");
    let (program, args): (PathBuf, Vec<&str>) = if cfg!(windows) {
        (
            crate::platform::launch::system_root()
                .join("System32")
                .join("taskkill.exe"),
            vec!["/PID", &pid, "/T", "/F"],
        )
    } else {
        (PathBuf::from("kill"), vec!["-KILL", "--", &group])
    };
    let _ = crate::platform::launch::run_hidden(&program, &args, Duration::from_secs(20), 64 * 1024).await;
}

impl HostServer {
    pub fn new(probe: Probe, stop_timeout: Duration) -> Self {
        Self {
            inner: Arc::new(Inner {
                st: Mutex::new(St {
                    state: HostServerState::Stopped,
                    message: None,
                    lines: VecDeque::new(),
                    invite: None,
                    child: None,
                    stop_requested: false,
                }),
                probe,
                listener: Mutex::new(None),
                stop_timeout,
                change_pending: AtomicBool::new(false),
            }),
        }
    }

    /// Who hears of every change (the page's `host-server` event). The view carries the newest invite,
    /// a secret: the listener never logs it.
    pub fn set_listener(&self, listener: Listener) {
        *lock(&self.inner.listener) = Some(listener);
    }

    fn st(&self) -> MutexGuard<'_, St> {
        lock(&self.inner.st)
    }

    pub fn view(&self) -> HostServerView {
        let st = self.st();
        HostServerView {
            state: st.state,
            message: st.message.clone(),
            lines: st.lines.iter().cloned().collect(),
            invite: st.invite.clone(),
        }
    }

    pub fn state(&self) -> HostServerState {
        self.st().state
    }

    /// The launcher started a server that has not exited.
    pub fn running(&self) -> bool {
        self.st().child.is_some()
    }

    pub fn set_invite(&self, invite: Option<HostInvite>) {
        self.st().invite = invite;
        self.changed(true);
    }

    fn tell(&self) {
        let listener = lock(&self.inner.listener).clone();
        if let Some(listener) = listener {
            listener(self.view());
        }
    }

    /// A state change reaches the page at once; output at most every 250 ms.
    fn changed(&self, now: bool) {
        if now {
            self.inner.change_pending.store(false, Ordering::SeqCst);
            self.tell();
            return;
        }
        if self.inner.change_pending.swap(true, Ordering::SeqCst) {
            return;
        }
        let this = self.clone();
        let later = async move {
            tokio::time::sleep(CHANGE_EVERY).await;
            if this.inner.change_pending.swap(false, Ordering::SeqCst) {
                this.tell();
            }
        };
        match tokio::runtime::Handle::try_current() {
            Ok(runtime) => {
                runtime.spawn(later);
            }
            Err(_) => {
                self.inner.change_pending.store(false, Ordering::SeqCst);
                self.tell();
            }
        }
    }

    fn set(&self, state: HostServerState, message: Option<String>) {
        {
            let mut st = self.st();
            st.state = state;
            st.message = message;
        }
        self.changed(true);
    }

    /// A line on the page (the last 300 are kept).
    pub fn note(&self, line: &str) {
        let clean = clean_line(line);
        if clean.is_empty() {
            return;
        }
        {
            let mut st = self.st();
            st.lines.push_back(clean);
            while st.lines.len() > MAX_LINES {
                st.lines.pop_front();
            }
        }
        self.changed(false);
    }

    /// Reads a child's output line by line into the log and, unless `on_line` hides it, the page.
    /// Resolves when both pipes have ended.
    fn read_output(
        &self,
        child: &mut tokio::process::Child,
        log_file: &Path,
        header: String,
        mut on_line: impl FnMut(&str) -> Shown + Send + 'static,
    ) -> tokio::task::JoinHandle<()> {
        let mut log = log_file.parent().and_then(|dir| {
            std::fs::create_dir_all(dir).ok()?;
            std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(log_file)
                .ok()
        });
        if let Some(log) = log.as_mut() {
            let _ = log.write_all(header.as_bytes());
        }
        let (sender, mut chunks) = tokio::sync::mpsc::unbounded_channel::<Vec<u8>>();
        if let Some(out) = child.stdout.take() {
            tokio::spawn(pump(out, sender.clone()));
        }
        if let Some(err) = child.stderr.take() {
            tokio::spawn(pump(err, sender.clone()));
        }
        drop(sender);
        let this = self.clone();
        tokio::spawn(async move {
            let mut rest = Vec::<u8>::new();
            let mut take = |line: &[u8], log: &mut Option<std::fs::File>| {
                let text = String::from_utf8_lossy(line);
                let clean = clean_line(&text);
                if on_line(&clean) == Shown::Yes {
                    if let Some(log) = log.as_mut() {
                        let _ = log.write_all(text.trim_end_matches('\r').as_bytes());
                        let _ = log.write_all(b"\n");
                    }
                    this.note(&clean);
                }
            };
            while let Some(chunk) = chunks.recv().await {
                rest.extend_from_slice(&chunk);
                while let Some(end) = rest.iter().position(|&b| b == b'\n') {
                    let line: Vec<u8> = rest.drain(..=end).collect();
                    take(&line[..line.len() - 1], &mut log);
                }
                // A line with no end in sight is cut rather than kept forever.
                if rest.len() > 64 * 1024 {
                    let line: Vec<u8> = std::mem::take(&mut rest);
                    take(&line, &mut log);
                }
            }
            if !rest.is_empty() {
                take(&rest, &mut log);
            }
        })
    }

    /// A one-shot host kit command (`setup`, `invite`, `multipliers`, `update`, `gamedata`,
    /// `publish-runtime`) in the workspace, logging to `.workspace\logs\launcher-<log_name>.log`. An
    /// `invite`'s JSON line (it holds the link) goes neither to the page nor to the log.
    pub async fn command(
        &self,
        tool: &HostTool,
        workspace: &Path,
        args: &[String],
        log_name: &str,
    ) -> Result<CommandOutput, HostKitError> {
        let secret_json = args.first().is_some_and(|a| a == "invite");
        let mut cmd = command(tool, args, workspace, &[]);
        cmd.stdin(Stdio::null()).kill_on_drop(true);
        let mut child = cmd.spawn().map_err(|e| HostKitError::Spawn(e.to_string()))?;
        self.note(&format!("> {}", args.first().map_or("", String::as_str)));
        let own = Arc::new(Mutex::new(Vec::<String>::new()));
        let mine = own.clone();
        let log = WorkspacePaths::new(workspace).launcher_log(log_name);
        let reader = self.read_output(&mut child, &log, header(tool, args, workspace), move |line| {
            lock(&mine).push(line.to_owned());
            if secret_json && line.trim_start().starts_with('{') {
                Shown::No
            } else {
                Shown::Yes
            }
        });
        let code = child.wait().await.ok().and_then(|s| s.code()).unwrap_or(-1);
        let _ = tokio::time::timeout(Duration::from_secs(5), reader).await;
        append(&log, &format!("(exit code {code})\n"));
        let lines = std::mem::take(&mut *lock(&own));
        let failure = if code == 0 { None } else { failure_line(&lines) };
        Ok(CommandOutput { code, lines, failure })
    }

    /// Is a server running on this PC that the launcher did not start?
    pub async fn refresh(&self) {
        {
            let st = self.st();
            if st.child.is_some() || matches!(st.state, HostServerState::Starting | HostServerState::Stopping) {
                return;
            }
        }
        let answers = (self.inner.probe)().await;
        let state = self.state();
        if answers && state != HostServerState::External {
            self.set(HostServerState::External, None);
        } else if !answers && state == HostServerState::External {
            self.set(HostServerState::Stopped, None);
        }
    }

    /// Starts `reforged-host start`. A server already answering on port 61000 is `external` and
    /// nothing starts.
    pub async fn start(&self, tool: &HostTool, workspace: &Path) -> Result<(), HostKitError> {
        {
            let mut st = self.st();
            if st.child.is_some() || matches!(st.state, HostServerState::Starting | HostServerState::Stopping) {
                return Ok(());
            }
            // Claimed now, so a second start while the probe runs does nothing.
            st.state = HostServerState::Starting;
            st.message = None;
        }
        if (self.inner.probe)().await {
            self.set(HostServerState::External, None);
            return Ok(());
        }
        self.st().stop_requested = false;
        self.changed(true);
        let args = vec!["start".to_owned()];
        let mut cmd = command(tool, &args, workspace, &[(CONTROL_ENV, "stdin")]);
        cmd.stdin(Stdio::piped());
        #[cfg(unix)]
        cmd.process_group(0);
        let mut child = match cmd.spawn() {
            Ok(child) => child,
            Err(e) => {
                let error = HostKitError::Spawn(e.to_string());
                self.set(HostServerState::Failed, Some(error.detail()));
                return Err(error);
            }
        };
        self.note("> start");
        let own = Arc::new(Mutex::new(VecDeque::<String>::new()));
        let mine = own.clone();
        let this = self.clone();
        let log = WorkspacePaths::new(workspace).launcher_log("server");
        let reader = self.read_output(&mut child, &log, header(tool, &args, workspace), move |line| {
            if line.trim() == SERVER_READY_LINE {
                if this.state() == HostServerState::Starting {
                    this.set(HostServerState::Running, None);
                }
                return Shown::No;
            }
            let mut own = lock(&mine);
            own.push_back(line.to_owned());
            if own.len() > MAX_LINES {
                own.pop_front();
            }
            Shown::Yes
        });
        let (exited_tx, exited) = watch::channel(false);
        self.st().child = Some(Running {
            pid: child.id().unwrap_or(0),
            stdin: child.stdin.take(),
            exited,
        });
        let this = self.clone();
        tokio::spawn(async move {
            let code = child.wait().await.ok().and_then(|s| s.code());
            let _ = tokio::time::timeout(Duration::from_secs(5), reader).await;
            append(
                &log,
                &format!("(exit code {})\n", code.map_or("none".into(), |c| c.to_string())),
            );
            let lines: Vec<String> = lock(&own).iter().cloned().collect();
            let stopped = {
                let mut st = this.st();
                st.child = None;
                st.stop_requested
            };
            if stopped {
                this.set(HostServerState::Stopped, None);
            } else {
                let why = failure_line(&lines).unwrap_or_else(|| format!("exit {}", code.unwrap_or(-1)));
                this.set(HostServerState::Failed, Some(why));
            }
            let _ = exited_tx.send(true);
        });
        Ok(())
    }

    /// Asks the server to stop with a `stop` line and closes its stdin; ends its process tree when it
    /// has not exited within the stop timeout, and waits up to 5 s more.
    pub async fn stop(&self) {
        let (pid, stdin, mut exited) = {
            let mut st = self.st();
            let Some(running) = st.child.as_mut() else {
                return;
            };
            let taken = (running.pid, running.stdin.take(), running.exited.clone());
            st.stop_requested = true;
            taken
        };
        self.set(HostServerState::Stopping, None);
        if let Some(mut stdin) = stdin {
            let _ = stdin.write_all(b"stop\n").await;
            let _ = stdin.flush().await;
            drop(stdin);
        }
        let done = tokio::time::timeout(self.inner.stop_timeout, exited.wait_for(|e| *e))
            .await
            .is_ok();
        if !done && pid != 0 {
            self.note("The server did not stop in time: ending its processes.");
            kill_tree(pid).await;
            let _ = tokio::time::timeout(KILL_GRACE, exited.wait_for(|e| *e)).await;
        }
    }
}

fn header(tool: &HostTool, args: &[String], cwd: &Path) -> String {
    let name = tool
        .program
        .file_name()
        .map_or_else(String::new, |n| n.to_string_lossy().into_owned());
    format!(
        "\n> {name} {}  (in {}, {})\n",
        args.join(" "),
        cwd.display(),
        chrono::Utc::now().to_rfc3339()
    )
}

fn append(log: &Path, text: &str) {
    if let Ok(mut file) = std::fs::OpenOptions::new().append(true).open(log) {
        let _ = file.write_all(text.as_bytes());
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::platform::hostkit::fixtures;

    fn tool(mode: Option<&str>) -> HostTool {
        HostTool {
            program: fixtures::exe("fake-host-kit"),
            prefix: mode.into_iter().map(OsString::from).collect(),
        }
    }

    fn server(answers: bool) -> (HostServer, Arc<Mutex<Vec<HostServerView>>>) {
        let s = HostServer::new(
            Arc::new(move || Box::pin(async move { answers })),
            Duration::from_secs(10),
        );
        let views = Arc::new(Mutex::new(Vec::new()));
        let seen = views.clone();
        s.set_listener(Arc::new(move |view| lock(&seen).push(view)));
        (s, views)
    }

    async fn until(what: impl Fn() -> bool) {
        let end = tokio::time::Instant::now() + Duration::from_secs(15);
        while !what() {
            assert!(tokio::time::Instant::now() < end, "timed out");
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
    }

    fn args(list: &[&str]) -> Vec<String> {
        list.iter().map(|&a| a.to_owned()).collect()
    }

    // hostserver.test.ts: "the server starts, says it is up, and stops on request"
    #[tokio::test(flavor = "multi_thread")]
    async fn the_server_starts_says_it_is_up_and_stops_on_request() {
        let ws = tempfile::tempdir().unwrap();
        let (s, views) = server(false);
        s.start(&tool(None), ws.path()).await.unwrap();
        until(|| s.state() == HostServerState::Running).await;
        until(|| s.view().lines.iter().any(|l| l == "mongod   | ready")).await;
        assert!(
            !s.view().lines.iter().any(|l| l == SERVER_READY_LINE),
            "the ready marker is not shown"
        );
        assert!(lock(&views).iter().any(|v| v.state == HostServerState::Starting));
        assert!(s.running());
        s.stop().await;
        until(|| s.state() == HostServerState::Stopped).await;
        until(|| s.view().lines.iter().any(|l| l == "Stopped.")).await;
        assert!(!s.running());
        let log = std::fs::read_to_string(WorkspacePaths::new(ws.path()).launcher_log("server")).unwrap();
        assert!(
            log.contains("> fake-host-kit") && log.contains("Stopped.") && log.contains("(exit code 0)"),
            "{log}"
        );
        assert!(!log.contains(SERVER_READY_LINE));
    }

    // hostserver.test.ts: "a server that fails says why, and one started elsewhere is not started again"
    #[tokio::test(flavor = "multi_thread")]
    async fn a_server_that_fails_says_why_and_one_started_elsewhere_is_not_started_again() {
        let ws = tempfile::tempdir().unwrap();
        let (s, _) = server(false);
        s.start(&tool(Some("fail")), ws.path()).await.unwrap();
        until(|| s.state() == HostServerState::Failed).await;
        assert_eq!(s.view().message.as_deref(), Some("port 27017 is already taken"));

        let (other, _) = server(true);
        other.start(&tool(None), ws.path()).await.unwrap();
        assert_eq!(other.state(), HostServerState::External);
        assert!(!other.running());
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn refresh_sees_a_server_started_elsewhere_come_and_go() {
        let up = Arc::new(AtomicBool::new(true));
        let probe_up = up.clone();
        let s = HostServer::new(
            Arc::new(move || {
                let up = probe_up.load(Ordering::SeqCst);
                Box::pin(async move { up })
            }),
            Duration::from_secs(1),
        );
        s.refresh().await;
        assert_eq!(s.state(), HostServerState::External);
        up.store(false, Ordering::SeqCst);
        s.refresh().await;
        assert_eq!(s.state(), HostServerState::Stopped);
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn a_server_that_ignores_the_stop_line_is_ended_after_the_timeout() {
        let ws = tempfile::tempdir().unwrap();
        let s = HostServer::new(Arc::new(|| Box::pin(async { false })), Duration::from_millis(500));
        s.start(&tool(Some("deaf")), ws.path()).await.unwrap();
        until(|| s.state() == HostServerState::Running).await;
        s.stop().await;
        until(|| s.state() == HostServerState::Stopped).await;
        assert!(s.view().lines.iter().any(|l| l.contains("did not stop in time")));
    }

    // hostserver.test.ts: "one-shot commands give their lines and the error"
    #[tokio::test(flavor = "multi_thread")]
    async fn one_shot_commands_give_their_lines_and_the_error() {
        let ws = tempfile::tempdir().unwrap();
        let (s, _) = server(false);
        let invite = s
            .command(
                &tool(None),
                ws.path(),
                &args(&["invite", "--json", "--for", "Alex"]),
                "invite",
            )
            .await
            .unwrap();
        assert_eq!(invite.code, 0);
        let link = crate::platform::hostkit::parse::invite_line(&invite.lines).unwrap();
        assert_eq!(link.link, "reforged-revival://join/abc");
        assert_eq!(link.for_name.as_deref(), Some("Alex"));
        until(|| s.view().lines.iter().any(|l| l == "noise")).await;
        assert!(
            !s.view().lines.iter().any(|l| l.contains("reforged-revival://")),
            "the link stays off the page"
        );
        let log = std::fs::read_to_string(WorkspacePaths::new(ws.path()).launcher_log("invite")).unwrap();
        assert!(!log.contains("reforged-revival://"), "and out of the log: {log}");

        let failed = s
            .command(&tool(None), ws.path(), &args(&["gamedata"]), "gamedata")
            .await
            .unwrap();
        assert_eq!(failed.code, 1);
        assert_eq!(
            failed.failure.as_deref(),
            Some("game data is missing: reforged-host gamedata")
        );

        let missing = HostTool {
            program: ws.path().join("nope").join(crate::platform::hostkit::HOST_BINARY),
            prefix: Vec::new(),
        };
        assert!(matches!(
            s.command(&missing, ws.path(), &args(&["gamedata"]), "gamedata").await,
            Err(HostKitError::Spawn(_))
        ));
        assert_eq!(HostTool::find(ws.path()), None, "not a workspace");
    }

    #[test]
    fn the_contract_is_the_host_kits() {
        assert_eq!(CONTROL_ENV, "REFORGED_CONTROL");
        assert_eq!(SERVER_READY_LINE, "REFORGED_SERVER_READY");
        assert_eq!(BACKEND_PORT, 61000);
        assert_eq!(STOP_TIMEOUT, Duration::from_secs(45));
    }

    #[tokio::test]
    async fn the_probe_sees_a_listener() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        assert!(port_answers(port).await);
        drop(listener);
        assert!(!port_answers(port).await);
    }
}
