//! The service supervisor (`legacy/ReforgedHost/src/services.ts`): starts each service with its env
//! file, waits until it answers, restarts it when it dies and stops the whole process tree (a
//! Windows job object, a Linux process group; RD3).
//!
//! Filled by RH2, for `start` only.
//!
//!   mongod (if managed)  127.0.0.1:27017
//!   backend              127.0.0.1:61000   paradox/backend   (API, chat; HTTP only)
//!   director             127.0.0.1:61001   paradox/director  (starts Ramsgate, the Training Grounds, hunts on UDP 8770-8777)
//!   content              127.0.0.1:61002   the content server (the runtime, news, art)
//!   allowlist (public)   127.0.0.1:61005   the allowlist helper (the firewall, administrator)
//!   gateway              0.0.0.0:443 (public, TLS) or <Tailscale IP>:61000 (private, HTTP)
//!
//! Each service runs in a process tree of its own ([`ProcessTree`]): on Windows a job object that
//! is ended when the service stops and closed (ending whatever is left in it) when this process
//! ends in any way; on Linux and macOS a process group that is sent SIGTERM, then SIGKILL. The
//! Director's game servers are its children, so they are in its tree and stop with it.

mod tree;

use std::fs::{self, File, OpenOptions};
use std::io::{BufRead, BufReader, Read, Write};
use std::net::{SocketAddr, TcpStream, ToSocketAddrs};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, ExitStatus, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::thread;
use std::time::{Duration, Instant};

pub use tree::ProcessTree;

use crate::error::HostError;
use crate::hostconfig::read_env_file;

/// How long a service has to stop after it was asked to (Node's 8 s), before its tree is killed.
pub const STOP_GRACE: Duration = Duration::from_secs(8);

/// When a service counts as started.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Ready {
    /// An HTTP GET of `path` gets an answer, whatever its status.
    Http { host: String, port: u16, path: String },
    /// A TCP connection is accepted.
    Tcp { host: String, port: u16 },
}

impl Ready {
    pub fn http(port: u16, path: &str) -> Self {
        Self::Http {
            host: "127.0.0.1".into(),
            port,
            path: path.into(),
        }
    }

    pub fn tcp(host: &str, port: u16) -> Self {
        Self::Tcp {
            host: host.into(),
            port,
        }
    }

    pub fn host(&self) -> &str {
        match self {
            Self::Http { host, .. } | Self::Tcp { host, .. } => host,
        }
    }

    pub fn port(&self) -> u16 {
        match self {
            Self::Http { port, .. } | Self::Tcp { port, .. } => *port,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ServiceSpec {
    pub name: String,
    pub cwd: PathBuf,
    pub command: PathBuf,
    pub args: Vec<String>,
    /// Read and given to the process as its environment (the Node services). A Rust service is
    /// given its file with `--env-file` in `args` instead (DR X7) and this is `None`.
    pub env_file: Option<PathBuf>,
    pub extra_env: Vec<(String, String)>,
    /// Considered started once this answers, or as soon as the process is up when `None`.
    pub ready: Option<Ready>,
}

/// Variables the env files decide: one left over in the shell (an old MYSTICPARADOX_ or REFORGED_
/// setting) never reaches a service.
const SERVER_PREFIXES: [&str; 12] = [
    "REFORGED_",
    "MYSTICPARADOX_",
    "MYSTPAX_",
    "GATEWAY_",
    "ALLOWLIST_",
    "CONTENT_",
    "MONGODB_",
    "AUTH_",
    "GAMESERVER_",
    "METAGAME_",
    "QOS_",
    "REALTIME_",
];
const SERVER_NAMES: [&str; 10] = [
    "NODE_ENV",
    "PORT",
    "BIND_HOST",
    "API_KEY_HASH_SECRET",
    "MY_IP",
    "LOG_LEVEL",
    "REGISTRATION_MODE",
    "DEPLOYSERVER_URL",
    "MATCHMAKING_MODE",
    "BACKEND_URL",
];

fn is_server_setting(key: &str) -> bool {
    let upper = key.to_ascii_uppercase();
    SERVER_PREFIXES.iter().any(|p| upper.starts_with(p)) || SERVER_NAMES.contains(&upper.as_str())
}

/// What a service gets: the system's own variables from `base`, then the env file's, then `extra`
/// (a later one wins).
pub fn service_environment<'a>(
    base: impl IntoIterator<Item = (String, String)>,
    from_file: impl IntoIterator<Item = (&'a str, &'a str)>,
    extra: &[(String, String)],
) -> Vec<(String, String)> {
    let mut env: Vec<(String, String)> = Vec::new();
    let mut set = |key: &str, value: &str| match env.iter_mut().find(|(k, _)| k == key) {
        Some(slot) => slot.1 = value.to_string(),
        None => env.push((key.to_string(), value.to_string())),
    };
    for (k, v) in base {
        if !is_server_setting(&k) {
            set(&k, &v);
        }
    }
    for (k, v) in from_file {
        set(k, v);
    }
    for (k, v) in extra {
        set(k, v);
    }
    env
}

/// Prints one line of a service's output, or a line about it: `(name, line)`.
pub type Printer = Arc<dyn Fn(&str, &str) + Send + Sync>;

/// `2026-10-22T13:38:00.000Z`, as JavaScript's `toISOString()`.
fn iso_now() -> String {
    chrono::Utc::now().format("%Y-%m-%dT%H:%M:%S%.3fZ").to_string()
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    // A thread that panicked holding it left plain data behind; carry on with it.
    mutex.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
}

struct Live {
    child: Child,
    tree: ProcessTree,
}

struct State {
    live: Option<Live>,
    restarts: u32,
}

struct Entry {
    spec: ServiceSpec,
    log_file: PathBuf,
    state: Mutex<State>,
}

/// Starts services, restarts the ones that die and stops them all, the last started first.
pub struct Supervisor {
    log_dir: PathBuf,
    print: Printer,
    stopping: Arc<AtomicBool>,
    cancel: Option<Arc<AtomicBool>>,
    entries: Mutex<Vec<Arc<Entry>>>,
    monitors: Mutex<Vec<thread::JoinHandle<()>>>,
}

impl Supervisor {
    pub fn new(log_dir: impl Into<PathBuf>, print: Printer) -> Self {
        Self {
            log_dir: log_dir.into(),
            print,
            stopping: Arc::new(AtomicBool::new(false)),
            cancel: None,
            entries: Mutex::new(Vec::new()),
            monitors: Mutex::new(Vec::new()),
        }
    }

    /// Waiting for a service to answer ends early once `cancel` is set (a stop asked for while the
    /// server is starting).
    pub fn with_cancel(mut self, cancel: Arc<AtomicBool>) -> Self {
        self.cancel = Some(cancel);
        self
    }

    pub fn log_dir(&self) -> &Path {
        &self.log_dir
    }

    fn cancelled(&self) -> bool {
        self.stopping.load(Ordering::SeqCst) || self.cancel.as_ref().is_some_and(|c| c.load(Ordering::SeqCst))
    }

    /// Starts the service and waits up to `ready_timeout` for it to answer. `Ok(false)`: it did not
    /// (or a stop was asked for meanwhile); it keeps running and being restarted all the same.
    pub fn start(&self, spec: ServiceSpec, ready_timeout: Duration) -> Result<bool, HostError> {
        fs::create_dir_all(&self.log_dir).map_err(|e| HostError::io(&self.log_dir, e))?;
        let (name, ready) = (spec.name.clone(), spec.ready.clone());
        let entry = Arc::new(Entry {
            log_file: self.log_dir.join(format!("{}.log", spec.name)),
            spec,
            state: Mutex::new(State {
                live: None,
                restarts: 0,
            }),
        });
        lock(&self.entries).push(entry.clone());
        let (print, stopping) = (self.print.clone(), self.stopping.clone());
        let (launched_tx, launched_rx) = std::sync::mpsc::channel();
        let monitor = thread::Builder::new()
            .name(format!("service {}", entry.spec.name))
            .spawn(move || monitor(&entry, &print, &stopping, launched_tx))
            .map_err(|e| HostError::message(format!("could not start a thread: {e}")))?;
        lock(&self.monitors).push(monitor);
        // The first launch has been tried (it may have failed, which the monitor printed).
        let _ = launched_rx.recv();
        let Some(ready) = ready else { return Ok(true) };
        let ok = wait_until_ready(&ready, ready_timeout, &|| self.cancelled());
        if !self.cancelled() {
            let line = if ok {
                format!("ready on {}:{}", ready.host(), ready.port())
            } else {
                format!(
                    "did not answer on port {} within {} s (see its log in {})",
                    ready.port(),
                    ready_timeout.as_secs_f64().round(),
                    self.log_dir.display()
                )
            };
            (self.print)(&name, &line);
        }
        Ok(ok)
    }

    /// Stops every service, the last started first (the gateway, so no player request reaches a
    /// service that is going away), each with its whole process tree. Returns once all are gone.
    pub fn stop_all(&self) {
        self.stopping.store(true, Ordering::SeqCst);
        let entries: Vec<Arc<Entry>> = lock(&self.entries).iter().rev().cloned().collect();
        for entry in entries {
            stop_entry(&entry);
        }
        for monitor in lock(&self.monitors).drain(..) {
            let _ = monitor.join();
        }
    }

    /// Whether every service's process tree is gone.
    pub fn all_stopped(&self) -> bool {
        lock(&self.entries).iter().all(|e| lock(&e.state).live.is_none())
    }
}

impl Drop for Supervisor {
    fn drop(&mut self) {
        if !self.all_stopped() || !lock(&self.monitors).is_empty() {
            self.stop_all();
        }
    }
}

fn stop_entry(entry: &Entry) {
    {
        let state = lock(&entry.state);
        match &state.live {
            Some(live) => live.tree.terminate(),
            None => return,
        }
    }
    if wait_gone(entry, STOP_GRACE) {
        return;
    }
    if let Some(live) = &lock(&entry.state).live {
        live.tree.kill();
    }
    wait_gone(entry, Duration::from_secs(5));
}

fn wait_gone(entry: &Entry, within: Duration) -> bool {
    let until = Instant::now() + within;
    loop {
        if lock(&entry.state).live.is_none() {
            return true;
        }
        if Instant::now() >= until {
            return false;
        }
        thread::sleep(Duration::from_millis(50));
    }
}

/// One service's life: launch it, wait for it to end, restart it with a backoff unless the
/// supervisor is stopping. A service that keeps dying right after it starts is given up on.
fn monitor(entry: &Entry, print: &Printer, stopping: &AtomicBool, launched: std::sync::mpsc::Sender<()>) {
    let name = entry.spec.name.as_str();
    let mut launched = Some(launched);
    loop {
        let started = Instant::now();
        let status = match launch(entry, print) {
            Ok(()) => {
                if let Some(tx) = launched.take() {
                    let _ = tx.send(());
                }
                wait_for_exit(entry)
            }
            Err(message) => {
                if let Some(tx) = launched.take() {
                    let _ = tx.send(());
                }
                print(name, &format!("could not start: {message}"));
                None
            }
        };
        let described = status.map_or_else(|| "not started".to_string(), describe_status);
        append_log(
            &entry.log_file,
            &format!("===== {} {name} exited ({described})\n", iso_now()),
        );
        if stopping.load(Ordering::SeqCst) {
            return;
        }
        let restarts = {
            let mut state = lock(&entry.state);
            let quick = started.elapsed() < Duration::from_secs(30);
            state.restarts = if quick { state.restarts + 1 } else { 0 };
            state.restarts
        };
        if restarts > 5 {
            print(
                name,
                &format!(
                    "stopped {restarts} times in a row right after starting; not restarting it. Check {} and run reforged-host doctor.",
                    entry.log_file.display()
                ),
            );
            return;
        }
        let delay = Duration::from_millis((2000u64 << restarts).min(30000));
        print(
            name,
            &format!(
                "exited ({described}); restarting in {} s",
                delay.as_millis().div_ceil(1000)
            ),
        );
        let until = Instant::now() + delay;
        while Instant::now() < until {
            if stopping.load(Ordering::SeqCst) {
                return;
            }
            thread::sleep(Duration::from_millis(50));
        }
        if stopping.load(Ordering::SeqCst) {
            return;
        }
    }
}

fn describe_status(status: ExitStatus) -> String {
    if let Some(code) = status.code() {
        return code.to_string();
    }
    #[cfg(unix)]
    {
        use std::os::unix::process::ExitStatusExt as _;
        if let Some(signal) = status.signal() {
            return tree::signal_name(signal);
        }
    }
    status.to_string()
}

fn append_log(file: &Path, text: &str) {
    if let Ok(mut log) = OpenOptions::new().create(true).append(true).open(file) {
        let _ = log.write_all(text.as_bytes());
    }
}

/// Starts the process in its own tree, its output into its log and onto the console.
fn launch(entry: &Entry, print: &Printer) -> Result<(), String> {
    let spec = &entry.spec;
    let mut log = OpenOptions::new()
        .create(true)
        .append(true)
        .open(&entry.log_file)
        .map_err(|e| format!("{}: {e}", entry.log_file.display()))?;
    let _ = log.write_all(format!("\n===== {} starting {}\n", iso_now(), spec.name).as_bytes());
    let from_file = match &spec.env_file {
        Some(file) => read_env_file(file).map_err(|e| e.to_string())?,
        None => Default::default(),
    };
    let env = service_environment(std::env::vars(), from_file.iter(), &spec.extra_env);
    let mut command = Command::new(&spec.command);
    command
        .args(&spec.args)
        .current_dir(&spec.cwd)
        .env_clear()
        .envs(env)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    tree::configure(&mut command);
    let mut child = command.spawn().map_err(|e| e.to_string())?;
    let tree = match ProcessTree::adopt(&child) {
        Ok(tree) => tree,
        Err(e) => {
            let _ = child.kill();
            let _ = child.wait();
            return Err(format!("could not put it in its own process tree: {e}"));
        }
    };
    let log = Arc::new(Mutex::new(log));
    if let Some(out) = child.stdout.take() {
        pump(out, &spec.name, log.clone(), print.clone());
    }
    if let Some(err) = child.stderr.take() {
        pump(err, &spec.name, log, print.clone());
    }
    lock(&entry.state).live = Some(Live { child, tree });
    Ok(())
}

/// Copies a pipe into the log and its non-blank lines (at most 400 characters) onto the console.
fn pump(pipe: impl Read + Send + 'static, name: &str, log: Arc<Mutex<File>>, print: Printer) {
    let name = name.to_string();
    let _ = thread::Builder::new().name(format!("{name} output")).spawn(move || {
        let mut reader = BufReader::new(pipe);
        let mut line = Vec::new();
        loop {
            line.clear();
            match reader.read_until(b'\n', &mut line) {
                Ok(0) | Err(_) => return,
                Ok(_) => {}
            }
            let _ = lock(&log).write_all(&line);
            let text = String::from_utf8_lossy(&line);
            let text = text.trim_end_matches(['\n', '\r']);
            if !text.trim().is_empty() {
                let shown: String = text.chars().take(400).collect();
                print(&name, &shown);
            }
        }
    });
}

/// Waits until the process ends; its exit status, and its tree dropped (on Windows: whatever is
/// left in its job is ended with it).
fn wait_for_exit(entry: &Entry) -> Option<ExitStatus> {
    loop {
        {
            let mut state = lock(&entry.state);
            let live = state.live.as_mut()?;
            match live.child.try_wait() {
                Ok(Some(status)) => {
                    state.live = None;
                    return Some(status);
                }
                Ok(None) => {}
                Err(_) => {
                    state.live = None;
                    return None;
                }
            }
        }
        thread::sleep(Duration::from_millis(100));
    }
}

fn resolve(host: &str, port: u16) -> Option<SocketAddr> {
    (host, port).to_socket_addrs().ok()?.next()
}

/// Whether something answers: an HTTP reply to a GET of the path, or for TCP a connection.
pub fn answers(ready: &Ready, timeout: Duration) -> bool {
    let Some(addr) = resolve(ready.host(), ready.port()) else {
        return false;
    };
    let Ok(mut stream) = TcpStream::connect_timeout(&addr, timeout) else {
        return false;
    };
    // Linux can connect a socket to itself when nothing listens on a loopback port in the ephemeral
    // range (a TCP simultaneous open): that is no service.
    if stream.local_addr().ok() == Some(addr) {
        return false;
    }
    match ready {
        Ready::Tcp { .. } => true,
        Ready::Http { host, path, port } => {
            let _ = stream.set_read_timeout(Some(timeout));
            let _ = stream.set_write_timeout(Some(timeout));
            let request = format!("GET {path} HTTP/1.1\r\nHost: {host}:{port}\r\nConnection: close\r\n\r\n");
            if stream.write_all(request.as_bytes()).is_err() {
                return false;
            }
            let mut first = [0u8; 5];
            matches!(stream.read(&mut first), Ok(n) if n > 0) && first.starts_with(b"HTTP/")
        }
    }
}

/// Tries every half second until it answers, `timeout` passes or `cancelled` says to give up.
pub fn wait_until_ready(ready: &Ready, timeout: Duration, cancelled: &dyn Fn() -> bool) -> bool {
    let until = Instant::now() + timeout;
    loop {
        if cancelled() {
            return false;
        }
        if answers(ready, Duration::from_secs(2)) {
            return true;
        }
        if Instant::now() >= until {
            return false;
        }
        let pause = Instant::now() + Duration::from_millis(500);
        while Instant::now() < pause {
            if cancelled() {
                return false;
            }
            thread::sleep(Duration::from_millis(50));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pairs(list: &[(&str, &str)]) -> Vec<(String, String)> {
        list.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect()
    }

    #[test]
    fn get_only_the_systems_variables_from_the_shell_never_an_old_server_setting() {
        let base = pairs(&[
            ("PATH", "/bin"),
            ("SystemRoot", "C:\\Windows"),
            ("REFORGED_BACKEND_URL", "http://evil"),
            ("MYSTPAX_X", "1"),
            ("MONGODB_URI", "mongodb://elsewhere"),
            ("NODE_ENV", "development"),
            ("GATEWAY_SECRET", "old"),
        ]);
        let file = [
            ("MONGODB_URI", "mongodb://127.0.0.1:27017/?replicaSet=rs0"),
            ("NODE_ENV", "production"),
        ];
        let env = service_environment(base, file, &[]);
        assert_eq!(
            env,
            pairs(&[
                ("PATH", "/bin"),
                ("SystemRoot", "C:\\Windows"),
                ("MONGODB_URI", "mongodb://127.0.0.1:27017/?replicaSet=rs0"),
                ("NODE_ENV", "production"),
            ])
        );
    }

    #[test]
    fn matches_the_node_patterns_case_insensitively_and_exact_names_only_whole() {
        assert!(is_server_setting("reforged_x"));
        assert!(is_server_setting("Port"));
        assert!(!is_server_setting("PORTS"), "PORT$ is a whole name");
        assert!(!is_server_setting("MY_IPS"));
        assert!(is_server_setting("QOS_ANY"));
        assert!(!is_server_setting("ProgramFiles"));
        let env = service_environment(pairs(&[("A", "1")]), [("A", "2")], &pairs(&[("A", "3")]));
        assert_eq!(env, pairs(&[("A", "3")]), "the env file, then the extra variables, win");
    }

    #[test]
    fn a_closed_port_does_not_answer_and_an_open_one_does() {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        assert!(answers(&Ready::tcp("127.0.0.1", port), Duration::from_secs(1)));
        drop(listener);
        assert!(!answers(&Ready::tcp("127.0.0.1", port), Duration::from_secs(1)));
        assert!(!wait_until_ready(
            &Ready::http(port, "/"),
            Duration::from_secs(5),
            &|| true
        ));
    }

    #[test]
    fn an_http_check_needs_an_http_reply() {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let server = thread::spawn(move || {
            let (mut socket, _) = listener.accept().unwrap();
            let mut buf = [0u8; 512];
            let n = socket.read(&mut buf).unwrap();
            let request = String::from_utf8_lossy(&buf[..n]).to_string();
            socket
                .write_all(b"HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\n\r\n")
                .unwrap();
            request
        });
        assert!(answers(&Ready::http(port, "/dauntless-status"), Duration::from_secs(2)));
        let request = server.join().unwrap();
        assert!(request.starts_with("GET /dauntless-status HTTP/1.1\r\n"), "{request}");
    }
}
