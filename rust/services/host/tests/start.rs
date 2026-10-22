//! `reforged-host start`'s supervisor and control contract with fake services (RH2): the ready
//! line, a `stop` line on stdin, stdin closing, Ctrl+C, and nothing left running afterwards, the
//! Director's game servers included.
//!
//! The binary is its own fake: with `HOST_TEST_ROLE` set it runs as a service, a game server, a
//! server (`start`'s supervisor and control loop with the fake services) or, on Windows, the
//! program that presses Ctrl+C in the server's console. Without it, it runs the tests.

#![allow(unsafe_code)] // Windows: OpenProcess, and Ctrl+C sent into another console
#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpListener;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, ExitCode, Stdio};
use std::sync::mpsc;
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use host::commands::start::control::{CONTROL_ENV, Control, SERVER_READY_LINE, controlled_by_stdin};
use host::commands::start::{Planned, console_printer, finish, start_services};
use host::services::{Ready, ServiceSpec, Supervisor, answers};

const ROLE: &str = "HOST_TEST_ROLE";

fn main() -> ExitCode {
    match std::env::var(ROLE).as_deref() {
        Ok("service") => fake_service(),
        Ok("gameserver") => fake_game_server(),
        Ok("server") => fake_server(),
        Ok("ctrl-c") => press_ctrl_c(),
        _ => run_tests(),
    }
}

// ------------------------------------------------------------------ roles

fn env(name: &str) -> String {
    std::env::var(name).unwrap_or_default()
}

fn write_pid(kind: &str, text: &str) {
    let dir = PathBuf::from(env("FAKE_PIDS"));
    std::fs::write(dir.join(format!("{kind}-{}.pid", std::process::id())), text).unwrap();
}

/// Answers HTTP on FAKE_PORT; with FAKE_GAMESERVER=1 starts a game server as the Director does.
fn fake_service() -> ExitCode {
    let name = env("FAKE_NAME");
    let starts = match std::env::var("FAKE_COUNTER") {
        Ok(counter) => {
            let mut f = std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(&counter)
                .unwrap();
            f.write_all(b"x").unwrap();
            std::fs::read_to_string(&counter).unwrap().len()
        }
        Err(_) => 1,
    };
    write_pid(&name, &env("GREETING"));
    if env("FAKE_GAMESERVER") == "1" {
        // Never waited for: it is the Director's game server, which only its process tree stops.
        #[allow(clippy::zombie_processes)]
        Command::new(std::env::current_exe().unwrap())
            .env(ROLE, "gameserver")
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .unwrap();
    }
    let listener = TcpListener::bind(("127.0.0.1", env("FAKE_PORT").parse::<u16>().unwrap())).unwrap();
    println!("listening {starts}");
    if starts == 1 && std::env::var("FAKE_COUNTER").is_ok() {
        thread::spawn(|| {
            thread::sleep(Duration::from_millis(300));
            std::process::exit(3);
        });
    }
    let greeting = env("GREETING");
    for socket in listener.incoming() {
        let Ok(mut socket) = socket else { continue };
        let mut buf = [0u8; 1024];
        let _ = socket.read(&mut buf);
        let reply = format!(
            "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{greeting}",
            greeting.len()
        );
        let _ = socket.write_all(reply.as_bytes());
    }
    ExitCode::SUCCESS
}

fn fake_game_server() -> ExitCode {
    write_pid("gameserver", "");
    loop {
        thread::sleep(Duration::from_secs(60));
    }
}

fn fake_spec(name: &str, port: u16, extra: &[(&str, &str)]) -> ServiceSpec {
    let mut extra_env: Vec<(String, String)> = vec![
        (ROLE.into(), "service".into()),
        ("FAKE_NAME".into(), name.into()),
        ("FAKE_PORT".into(), port.to_string()),
        ("FAKE_PIDS".into(), env("FAKE_PIDS")),
    ];
    extra_env.extend(extra.iter().map(|(k, v)| (k.to_string(), v.to_string())));
    ServiceSpec {
        name: name.into(),
        cwd: std::env::temp_dir(),
        command: std::env::current_exe().unwrap(),
        args: Vec::new(),
        env_file: None,
        extra_env,
        ready: Some(Ready::http(port, "/")),
    }
}

/// `start` after its checks: the supervisor and the control loop, with fake services.
fn fake_server() -> ExitCode {
    let ports: Vec<u16> = env("FAKE_PORTS").split(',').map(|p| p.parse().unwrap()).collect();
    let mut director = fake_spec("director", ports[1], &[("FAKE_GAMESERVER", "1")]);
    director.env_file = Some(PathBuf::from(env("FAKE_ENV_FILE")));
    let mut gateway = fake_spec("gateway", ports[2], &[]);
    gateway.ready = Some(Ready::tcp("127.0.0.1", ports[2]));
    let minute = Duration::from_secs(60);
    let plan = vec![
        Planned {
            spec: fake_spec("backend", ports[0], &[]),
            ready_timeout: minute,
            required: true,
        },
        Planned {
            spec: director,
            ready_timeout: minute,
            required: false,
        },
        Planned {
            spec: gateway,
            ready_timeout: minute,
            required: true,
        },
    ];
    let controlled = controlled_by_stdin();
    let control = Control::begin(controlled).unwrap();
    let supervisor = Supervisor::new(env("FAKE_LOGS"), console_printer()).with_cancel(control.flag());
    let taken = |ready: &Ready| Ok(answers(ready, Duration::from_millis(500)));
    let started = start_services(&supervisor, &control, plan, None, &taken).unwrap();
    let marker = PathBuf::from(env("FAKE_PIDS")).join("after-stop");
    finish(
        started,
        &supervisor,
        &control,
        controlled,
        "Server \"Test\" is up",
        || {
            std::fs::write(marker, "").unwrap();
        },
    )
    .unwrap()
}

#[cfg(windows)]
fn press_ctrl_c() -> ExitCode {
    use windows::Win32::System::Console::{
        AttachConsole, CTRL_C_EVENT, FreeConsole, GenerateConsoleCtrlEvent, SetConsoleCtrlHandler,
    };
    let pid: u32 = env("FAKE_TARGET").parse().unwrap();
    // SAFETY: plain Win32 calls on this process's own console attachment.
    unsafe {
        let _ = FreeConsole();
        if let Err(e) = AttachConsole(pid) {
            eprintln!("AttachConsole({pid}): {e}");
            return ExitCode::from(2);
        }
        let _ = SetConsoleCtrlHandler(None, true);
        if let Err(e) = GenerateConsoleCtrlEvent(CTRL_C_EVENT, 0) {
            eprintln!("GenerateConsoleCtrlEvent: {e}");
            return ExitCode::from(3);
        }
    }
    thread::sleep(Duration::from_millis(500));
    ExitCode::SUCCESS
}

#[cfg(not(windows))]
fn press_ctrl_c() -> ExitCode {
    ExitCode::FAILURE
}

// ------------------------------------------------------------------ helpers

fn free_port() -> u16 {
    TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap().port()
}

#[cfg(windows)]
fn alive(pid: u32) -> bool {
    use windows::Win32::Foundation::{CloseHandle, STILL_ACTIVE};
    use windows::Win32::System::Threading::{GetExitCodeProcess, OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION};
    // SAFETY: the handle is checked and closed here.
    unsafe {
        let Ok(handle) = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid) else {
            return false;
        };
        let mut code = 0u32;
        let ok = GetExitCodeProcess(handle, &mut code).is_ok();
        let _ = CloseHandle(handle);
        ok && code == STILL_ACTIVE.0 as u32
    }
}

#[cfg(target_os = "linux")]
fn alive(pid: u32) -> bool {
    // Gone, or a zombie waiting to be reaped (no longer running anything).
    match std::fs::read_to_string(format!("/proc/{pid}/stat")) {
        Ok(stat) => stat
            .rsplit(')')
            .next()
            .is_some_and(|rest| !rest.trim_start().starts_with('Z')),
        Err(_) => false,
    }
}

#[cfg(all(unix, not(target_os = "linux")))]
fn alive(pid: u32) -> bool {
    Command::new("kill")
        .args(["-0", &pid.to_string()])
        .status()
        .is_ok_and(|s| s.success())
}

/// The pids the fakes wrote: `(kind, pid, contents)`.
fn pids(dir: &Path) -> Vec<(String, u32, String)> {
    let mut found = Vec::new();
    for entry in std::fs::read_dir(dir).unwrap() {
        let path = entry.unwrap().path();
        let name = path.file_name().unwrap().to_string_lossy().to_string();
        let Some(stem) = name.strip_suffix(".pid") else {
            continue;
        };
        let (kind, pid) = stem.rsplit_once('-').unwrap();
        found.push((
            kind.to_string(),
            pid.parse().unwrap(),
            std::fs::read_to_string(&path).unwrap(),
        ));
    }
    found
}

fn wait_for(what: &str, within: Duration, mut check: impl FnMut() -> bool) {
    let until = Instant::now() + within;
    while !check() {
        assert!(Instant::now() < until, "timed out waiting for {what}");
        thread::sleep(Duration::from_millis(100));
    }
}

fn assert_nothing_left(dir: &Path) {
    let all = pids(dir);
    assert!(!all.is_empty());
    wait_for("every fake process to be gone", Duration::from_secs(10), || {
        all.iter().all(|(_, pid, _)| !alive(*pid))
    });
}

struct Server {
    child: Child,
    lines: mpsc::Receiver<String>,
    seen: Arc<Mutex<Vec<String>>>,
    dir: tempfile::TempDir,
}

impl Server {
    fn start(controlled: bool) -> Self {
        let dir = tempfile::tempdir().unwrap();
        let pids = dir.path().join("pids");
        std::fs::create_dir_all(&pids).unwrap();
        let env_file = dir.path().join("director.env");
        std::fs::write(&env_file, "# director\nGREETING=from-the-env-file\nREFORGED_X=1\n").unwrap();
        let ports = [free_port(), free_port(), free_port()].map(|p| p.to_string()).join(",");
        let mut command = Command::new(std::env::current_exe().unwrap());
        command
            .env(ROLE, "server")
            .env("FAKE_PIDS", &pids)
            .env("FAKE_LOGS", dir.path().join("logs"))
            .env("FAKE_PORTS", ports)
            .env("FAKE_ENV_FILE", &env_file)
            .env_remove(CONTROL_ENV)
            .stdin(if controlled { Stdio::piped() } else { Stdio::null() })
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit());
        if controlled {
            command.env(CONTROL_ENV, "stdin");
        }
        #[cfg(windows)]
        {
            // A console of its own (hidden), for Ctrl+C to be pressed in.
            use std::os::windows::process::CommandExt as _;
            command.creation_flags(0x0800_0000);
        }
        let mut child = command.spawn().unwrap();
        let (tx, lines) = mpsc::channel();
        let seen = Arc::new(Mutex::new(Vec::new()));
        let out = child.stdout.take().unwrap();
        let all = seen.clone();
        thread::spawn(move || {
            for line in BufReader::new(out).lines() {
                let Ok(line) = line else { return };
                all.lock().unwrap().push(line.clone());
                let _ = tx.send(line);
            }
        });
        Self {
            child,
            lines,
            seen,
            dir,
        }
    }

    fn pids_dir(&self) -> PathBuf {
        self.dir.path().join("pids")
    }

    fn wait_line(&self, wanted: &str) {
        let until = Instant::now() + Duration::from_secs(60);
        loop {
            let left = until.saturating_duration_since(Instant::now());
            match self.lines.recv_timeout(left) {
                Ok(line) if line == wanted => return,
                Ok(_) => {}
                Err(_) => panic!("no {wanted:?} line; output:\n{}", self.output()),
            }
        }
    }

    fn output(&self) -> String {
        self.seen.lock().unwrap().join("\n")
    }

    fn wait_exit(&mut self) -> i32 {
        let until = Instant::now() + Duration::from_secs(40);
        loop {
            if let Some(status) = self.child.try_wait().unwrap() {
                return status.code().unwrap_or(-1);
            }
            if Instant::now() > until {
                let _ = self.child.kill();
                panic!("the server did not stop; output:\n{}", self.output());
            }
            thread::sleep(Duration::from_millis(100));
        }
    }

    /// Every service and the Director's game server running, the env file read.
    fn check_running(&self) {
        let pids = pids(&self.pids_dir());
        let kinds: Vec<&str> = pids.iter().map(|(k, _, _)| k.as_str()).collect();
        for kind in ["backend", "director", "gateway"] {
            assert!(kinds.contains(&kind), "{kind} is not running: {kinds:?}");
        }
        wait_for("the Director's game server", Duration::from_secs(10), || {
            pids_of(&self.pids_dir(), "gameserver").is_some()
        });
        for (_, pid, _) in self::pids(&self.pids_dir()) {
            assert!(alive(pid));
        }
        let director = pids.iter().find(|(k, _, _)| k == "director").unwrap();
        assert_eq!(director.2, "from-the-env-file", "the Director got its env file");
    }

    fn check_stopped(&mut self) {
        assert_eq!(self.wait_exit(), 0, "{}", self.output());
        let output = self.output();
        assert!(output.contains("Stopping..."), "{output}");
        assert!(output.ends_with("Stopped."), "{output}");
        assert!(
            self.pids_dir().join("after-stop").exists(),
            "the game ports were closed after the stop"
        );
        assert_nothing_left(&self.pids_dir());
    }
}

fn pids_of(dir: &Path, kind: &str) -> Option<u32> {
    pids(dir).into_iter().find(|(k, _, _)| k == kind).map(|(_, pid, _)| pid)
}

// ------------------------------------------------------------------ tests

fn stops_on_a_stop_line_after_the_ready_line_and_leaves_nothing_running() {
    let mut server = Server::start(true);
    server.wait_line(SERVER_READY_LINE);
    server.check_running();
    let output = server.output();
    assert!(output.contains("backend  | ready on 127.0.0.1:"), "{output}");
    assert!(output.contains("Server \"Test\" is up"), "{output}");
    assert!(!output.contains("Ctrl+C stops everything."), "{output}");
    server
        .child
        .stdin
        .as_mut()
        .unwrap()
        .write_all(b"hello\nstop\n")
        .unwrap();
    server.check_stopped();
}

fn stops_when_the_launchers_pipe_closes() {
    let mut server = Server::start(true);
    server.wait_line(SERVER_READY_LINE);
    server.check_running();
    drop(server.child.stdin.take());
    server.check_stopped();
}

fn stops_on_ctrl_c_with_the_game_servers() {
    let mut server = Server::start(false);
    server.wait_line("Ctrl+C stops everything.");
    server.check_running();
    assert!(
        !server.output().contains(SERVER_READY_LINE),
        "the ready line is for the launcher only"
    );
    let pid = server.child.id();
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt as _;
        let status = Command::new(std::env::current_exe().unwrap())
            .env(ROLE, "ctrl-c")
            .env("FAKE_TARGET", pid.to_string())
            .creation_flags(0x0800_0000)
            .status()
            .unwrap();
        assert!(status.success(), "Ctrl+C could not be sent: {status}");
    }
    #[cfg(unix)]
    {
        let status = Command::new("kill").args(["-INT", &pid.to_string()]).status().unwrap();
        assert!(status.success());
    }
    server.check_stopped();
    assert!(server.output().contains("Stopping..."));
}

/// A hard kill of the server (the launcher ending its tree) closes the job objects, and with them
/// every service and game server.
#[cfg(windows)]
fn a_hard_kill_of_the_server_leaves_nothing_running() {
    let mut server = Server::start(true);
    server.wait_line(SERVER_READY_LINE);
    server.check_running();
    server.child.kill().unwrap();
    server.child.wait().unwrap();
    assert_nothing_left(&server.pids_dir());
}

/// The Node kit's supervisor test: start, wait until it answers, restart it when it dies, stop it.
fn starts_a_process_waits_until_it_answers_restarts_it_when_it_dies_and_stops_it() {
    let dir = tempfile::tempdir().unwrap();
    let pids = dir.path().join("pids");
    std::fs::create_dir_all(&pids).unwrap();
    let counter = dir.path().join("starts.txt");
    let port = free_port();
    let mut spec = fake_spec("svc", port, &[("GREETING", "hello")]);
    spec.extra_env.retain(|(k, _)| k != "FAKE_PIDS");
    spec.extra_env.push(("FAKE_PIDS".into(), pids.display().to_string()));
    spec.extra_env
        .push(("FAKE_COUNTER".into(), counter.display().to_string()));
    let lines = Arc::new(Mutex::new(Vec::<String>::new()));
    let printed = lines.clone();
    let supervisor = Supervisor::new(
        dir.path().join("logs"),
        Arc::new(move |name: &str, line: &str| printed.lock().unwrap().push(format!("{name}: {line}"))),
    );
    assert!(supervisor.start(spec, Duration::from_secs(10)).unwrap());
    // It exits once; the supervisor starts it again after its backoff.
    wait_for("the restart", Duration::from_secs(15), || {
        std::fs::read_to_string(&counter).unwrap_or_default().len() >= 2
    });
    wait_for("the restarted service", Duration::from_secs(10), || {
        answers(&Ready::http(port, "/"), Duration::from_millis(500))
    });
    supervisor.stop_all();
    assert!(supervisor.all_stopped());
    let lines = lines.lock().unwrap().join("\n");
    assert!(lines.contains("svc: exited (3); restarting in 4 s"), "{lines}");
    assert!(lines.contains("svc: listening 1"), "{lines}");
    let log = std::fs::read_to_string(dir.path().join("logs").join("svc.log")).unwrap();
    assert!(log.contains("listening 1") && log.contains("svc exited (3)"), "{log}");
    assert!(pids_of(&pids, "svc").is_some());
    assert_nothing_left(&pids);
    assert!(self::pids(&pids).iter().all(|(_, _, greeting)| greeting == "hello"));
}

fn run_tests() -> ExitCode {
    #[cfg_attr(not(windows), allow(unused_mut))]
    let mut tests: Vec<(&str, fn())> = vec![
        (
            "starts_a_process_waits_until_it_answers_restarts_it_when_it_dies_and_stops_it",
            starts_a_process_waits_until_it_answers_restarts_it_when_it_dies_and_stops_it,
        ),
        (
            "stops_on_a_stop_line_after_the_ready_line_and_leaves_nothing_running",
            stops_on_a_stop_line_after_the_ready_line_and_leaves_nothing_running,
        ),
        (
            "stops_when_the_launchers_pipe_closes",
            stops_when_the_launchers_pipe_closes,
        ),
        (
            "stops_on_ctrl_c_with_the_game_servers",
            stops_on_ctrl_c_with_the_game_servers,
        ),
    ];
    #[cfg(windows)]
    tests.push((
        "a_hard_kill_of_the_server_leaves_nothing_running",
        a_hard_kill_of_the_server_leaves_nothing_running,
    ));
    let filter: Vec<String> = std::env::args().skip(1).filter(|a| !a.starts_with('-')).collect();
    let mut failed = 0;
    let mut ran = 0;
    for (name, test) in tests {
        if !filter.is_empty() && !filter.iter().any(|f| name.contains(f.as_str())) {
            continue;
        }
        ran += 1;
        let started = Instant::now();
        match std::panic::catch_unwind(test) {
            Ok(()) => println!("test {name} ... ok ({:.1} s)", started.elapsed().as_secs_f64()),
            Err(_) => {
                failed += 1;
                println!("test {name} ... FAILED");
            }
        }
    }
    println!("\n{} passed; {failed} failed", ran - failed);
    if failed == 0 {
        ExitCode::SUCCESS
    } else {
        ExitCode::FAILURE
    }
}
