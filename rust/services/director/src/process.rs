//! One game server process: spawned with its output appended to its own log file, its readiness
//! marker watched on stdout, and its exit reported once.
//!
//! The game thread deadlocked mid-hunt once (2026-07-19 GAME-THREAD FREEZE FIX in the Node
//! Director): the readiness reader paused stdout, the pipe filled, and the server's next log write
//! blocked forever holding the CRT's stdio lock. So both pipes are read to the end, always: the
//! readiness check only looks at what the log copy reads, and never stops it.

use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use tokio::io::{AsyncRead, AsyncReadExt, AsyncWriteExt};
use tokio::sync::{mpsc, oneshot, watch};

/// How a process ended, as Node's exit event reports it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Exit {
    pub code: Option<i32>,
    pub signal: Option<String>,
}

impl Exit {
    pub fn code_text(&self) -> String {
        self.code.map_or_else(|| "null".to_owned(), |c| c.to_string())
    }

    pub fn signal_text(&self) -> String {
        self.signal.clone().unwrap_or_else(|| "null".to_owned())
    }
}

/// A spawned process: whether it still runs, and the way to stop it.
#[derive(Debug)]
pub struct Proc {
    pub pid: Option<u32>,
    exited: AtomicBool,
    stop: mpsc::UnboundedSender<()>,
}

impl Proc {
    pub fn alive(&self) -> bool {
        !self.exited.load(Ordering::SeqCst)
    }

    /// SIGTERM (on Linux the game-server launcher passes it to Wine, the game and Xvfb);
    /// TerminateProcess on Windows.
    pub fn terminate(&self) -> Result<(), String> {
        if !self.alive() {
            return Err("the process is gone".to_owned());
        }
        self.stop.send(()).map_err(|_| "the process is gone".to_owned())
    }
}

pub struct Spec {
    pub program: String,
    pub args: Vec<String>,
    pub env: Vec<(String, String)>,
    pub windows_hide: bool,
    pub log: std::fs::File,
    pub launch_id: String,
    pub port: u16,
}

pub struct Spawned {
    pub proc: Arc<Proc>,
    /// Fires on the first matching readiness marker.
    pub ready: oneshot::Receiver<()>,
    /// `Some` once the process has exited.
    pub exit: watch::Receiver<Option<Exit>>,
    /// Lines for the log file after the process output.
    pub log: mpsc::UnboundedSender<Vec<u8>>,
}

/// `MYSTICPARADOX_GAMESERVER_READY launchId=<id> port=<port>`, which the runtime prints only after
/// Listen succeeded: `Some(true)` for this launch, `Some(false)` for another one's.
pub fn readiness(line: &str, launch_id: &str, port: u16) -> Option<bool> {
    let rest = service_kit::js::trim(line).strip_prefix("MYSTICPARADOX_GAMESERVER_READY launchId=")?;
    let (id, rest) = rest.split_once(" port=")?;
    if id.is_empty()
        || !id
            .bytes()
            .all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase() || b == b'-')
    {
        return None;
    }
    if rest.is_empty() || !rest.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    Some(id == launch_id && rest.parse::<u64>().ok() == Some(u64::from(port)))
}

/// What runs once the process has exited, with a sender for lines after its output in the log.
pub type OnExit = Box<dyn FnOnce(Exit, mpsc::UnboundedSender<Vec<u8>>) + Send>;

pub fn spawn(spec: Spec, on_exit: OnExit) -> Result<Spawned, String> {
    let os = os_spawn(&spec).map_err(|e| format!("spawn {} {e}", spec.program))?;
    let (stop_tx, mut stop_rx) = mpsc::unbounded_channel();
    let proc = Arc::new(Proc {
        pid: os.pid,
        exited: AtomicBool::new(false),
        stop: stop_tx,
    });
    let (log_tx, log_rx) = mpsc::unbounded_channel::<Vec<u8>>();
    tokio::spawn(write_log(tokio::fs::File::from_std(spec.log), log_rx));
    let (ready_tx, ready_rx) = oneshot::channel();
    let scan = Scan {
        launch_id: spec.launch_id.clone(),
        port: spec.port,
        ready: Some(ready_tx),
        buffered: Vec::new(),
    };
    tokio::spawn(drain(os.stdout, log_tx.clone(), Some(scan)));
    tokio::spawn(drain(os.stderr, log_tx.clone(), None));
    let (exit_tx, exit_rx) = watch::channel(None);
    let watched = proc.clone();
    let exit_log = log_tx.clone();
    let (mut wait, stop) = (os.wait, os.stop);
    tokio::spawn(async move {
        let exit = loop {
            tokio::select! {
                exit = &mut wait => break exit,
                Some(()) = stop_rx.recv() => stop(),
            }
        };
        watched.exited.store(true, Ordering::SeqCst);
        let _ = exit_tx.send(Some(exit.clone()));
        on_exit(exit, exit_log);
    });
    Ok(Spawned {
        proc,
        ready: ready_rx,
        exit: exit_rx,
        log: log_tx,
    })
}

/// A started process, whatever the platform.
struct OsChild {
    pid: Option<u32>,
    stdout: Box<dyn AsyncRead + Unpin + Send>,
    stderr: Box<dyn AsyncRead + Unpin + Send>,
    wait: futures_util::future::BoxFuture<'static, Exit>,
    stop: Box<dyn Fn() + Send>,
}

#[cfg(windows)]
fn os_spawn(spec: &Spec) -> std::io::Result<OsChild> {
    use futures_util::FutureExt;
    let child = crate::win_spawn::spawn(&spec.program, &spec.args, &spec.env, spec.windows_hide)?;
    let (waiter, killer) = (child.process.clone(), child.process);
    Ok(OsChild {
        pid: Some(child.pid),
        stdout: Box::new(tokio::fs::File::from_std(child.stdout)),
        stderr: Box::new(tokio::fs::File::from_std(child.stderr)),
        wait: async move {
            let code = tokio::task::spawn_blocking(move || waiter.wait())
                .await
                .ok()
                .and_then(Result::ok);
            Exit {
                code: code.map(|c| c as i32),
                signal: None,
            }
        }
        .boxed(),
        stop: Box::new(move || killer.kill()),
    })
}

#[cfg(not(windows))]
fn os_spawn(spec: &Spec) -> std::io::Result<OsChild> {
    use futures_util::FutureExt;
    use std::process::Stdio;
    let _ = spec.windows_hide;
    let mut child = tokio::process::Command::new(&spec.program)
        .args(&spec.args)
        .env_clear()
        .envs(spec.env.iter().map(|(k, v)| (k, v)))
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()?;
    let pid = child.id();
    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| std::io::Error::other("no stdout pipe"))?;
    let stderr = child
        .stderr
        .take()
        .ok_or_else(|| std::io::Error::other("no stderr pipe"))?;
    Ok(OsChild {
        pid,
        stdout: Box::new(stdout),
        stderr: Box::new(stderr),
        wait: async move {
            match child.wait().await {
                Ok(status) => exit_of(status),
                Err(_) => Exit {
                    code: None,
                    signal: None,
                },
            }
        }
        .boxed(),
        stop: Box::new(move || stop(pid)),
    })
}

#[cfg(unix)]
#[allow(unsafe_code)] // kill(2)
fn stop(pid: Option<u32>) {
    if let Some(pid) = pid.and_then(|p| i32::try_from(p).ok()) {
        // SAFETY: kill takes no pointers. The pid is our own child's; a stop is only sent while it
        // runs, though it may have exited and been reaped a moment before (then kill finds no process
        // or, rarely, a reused pid: the same race Node's child.kill has).
        unsafe {
            libc::kill(pid, libc::SIGTERM);
        }
    }
}

#[cfg(unix)]
fn exit_of(status: std::process::ExitStatus) -> Exit {
    use std::os::unix::process::ExitStatusExt;
    Exit {
        code: status.code(),
        signal: status.signal().map(signal_name),
    }
}

#[cfg(unix)]
fn signal_name(signal: i32) -> String {
    match signal {
        libc::SIGHUP => "SIGHUP".into(),
        libc::SIGINT => "SIGINT".into(),
        libc::SIGQUIT => "SIGQUIT".into(),
        libc::SIGILL => "SIGILL".into(),
        libc::SIGABRT => "SIGABRT".into(),
        libc::SIGBUS => "SIGBUS".into(),
        libc::SIGFPE => "SIGFPE".into(),
        libc::SIGKILL => "SIGKILL".into(),
        libc::SIGSEGV => "SIGSEGV".into(),
        libc::SIGPIPE => "SIGPIPE".into(),
        libc::SIGTERM => "SIGTERM".into(),
        other => format!("SIG{other}"),
    }
}

async fn write_log(mut file: tokio::fs::File, mut lines: mpsc::UnboundedReceiver<Vec<u8>>) {
    while let Some(bytes) = lines.recv().await {
        if file.write_all(&bytes).await.is_err() {
            // Keep receiving: the senders must never block or fail because the log did.
            continue;
        }
    }
    let _ = file.flush().await;
}

struct Scan {
    launch_id: String,
    port: u16,
    ready: Option<oneshot::Sender<()>>,
    buffered: Vec<u8>,
}

impl Scan {
    fn feed(&mut self, chunk: &[u8]) {
        if self.ready.is_none() {
            return;
        }
        self.buffered.extend_from_slice(chunk);
        while let Some(newline) = self.buffered.iter().position(|&b| b == b'\n') {
            let line: Vec<u8> = self.buffered.drain(..=newline).collect();
            let line = String::from_utf8_lossy(&line[..line.len() - 1]).into_owned();
            match readiness(&line, &self.launch_id, self.port) {
                Some(true) => {
                    if let Some(ready) = self.ready.take() {
                        let _ = ready.send(());
                    }
                    self.buffered.clear();
                    return;
                }
                Some(false) => crate::log::warn(&format!(
                    "Ignoring mismatched gameserver readiness marker on port {}: {line}",
                    self.port
                )),
                None => {}
            }
        }
        // A server that never writes a newline must not grow this without bound.
        if self.buffered.len() > 65536 {
            let keep = self.buffered.len() - 4096;
            self.buffered.drain(..keep);
        }
    }
}

async fn drain(mut pipe: impl AsyncRead + Unpin, log: mpsc::UnboundedSender<Vec<u8>>, mut scan: Option<Scan>) {
    let mut buffer = vec![0u8; 16 * 1024];
    loop {
        match pipe.read(&mut buffer).await {
            Ok(0) | Err(_) => return,
            Ok(n) => {
                let _ = log.send(buffer[..n].to_vec());
                if let Some(scan) = &mut scan {
                    scan.feed(&buffer[..n]);
                }
            }
        }
    }
}

/// Where a game server's log goes: `<dir>/port<port>_<label>.log`.
pub fn log_path(dir: &std::path::Path, port: u16, label: &str) -> PathBuf {
    dir.join(format!("port{port}_{label}.log"))
}
