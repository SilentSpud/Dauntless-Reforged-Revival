//! Running other programs (`legacy/ReforgedHost/src/run.ts`): always with an argument list (never a
//! shell string), output into a log file and, unless quiet, onto the console.
//!
//! Filled by RH2. Setup (RH3), game data (RH4) and update (RH5) call these signatures; a row that
//! needs them changed goes through the integration pass.

use std::fs::{self, OpenOptions};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use crate::error::HostError;

#[derive(Debug, Clone, Default)]
pub struct RunOptions {
    pub cwd: PathBuf,
    /// The child's whole environment; `None`: this process's.
    pub env: Option<Vec<(String, String)>>,
    pub log_file: Option<PathBuf>,
    pub quiet: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RunResult {
    /// -1 when the program could not start.
    pub code: i32,
    /// The line that names a failure, else the last line of output.
    pub last_line: Option<String>,
    /// The last 64 KiB of output.
    pub output: String,
}

const OUTPUT_KEPT: usize = 64 * 1024;

/// npm and npx are `.cmd` files on Windows, which Rust starts through cmd.exe with its arguments
/// escaped; the arguments here are always fixed words, never user input.
fn resolve_command(command: &str) -> String {
    if cfg!(windows) && (command == "npm" || command == "npx") {
        format!("{command}.cmd")
    } else {
        command.to_string()
    }
}

#[cfg(windows)]
fn hide_window(command: &mut Command) {
    use std::os::windows::process::CommandExt as _;
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    command.creation_flags(CREATE_NO_WINDOW);
}

#[cfg(not(windows))]
fn hide_window(_command: &mut Command) {}

/// The line that names a failure: the thrown error (`Error: Catalog item not found: x`,
/// `TypeError: ...`), which comes before its stack and Node's `Node.js v24.x` trailer
/// (`/^(Uncaught )?\w*Error\b/`).
pub fn is_error_line(line: &str) -> bool {
    let rest = line.strip_prefix("Uncaught ").unwrap_or(line);
    let word: String = rest
        .chars()
        .take_while(|c| c.is_ascii_alphanumeric() || *c == '_')
        .collect();
    word.ends_with("Error")
}

fn is_node_trailer(line: &str) -> bool {
    line.strip_prefix("Node.js v")
        .is_some_and(|rest| rest.starts_with(|c: char| c.is_ascii_digit()))
}

fn clip(line: &str, max: usize) -> String {
    line.chars().take(max).collect()
}

#[derive(Default)]
struct Collected {
    output: String,
    last_line: Option<String>,
    error_line: Option<String>,
}

impl Collected {
    fn add(&mut self, text: &str) {
        self.output.push_str(text);
        if self.output.len() > OUTPUT_KEPT {
            let mut cut = self.output.len() - OUTPUT_KEPT;
            while !self.output.is_char_boundary(cut) {
                cut += 1;
            }
            self.output.drain(..cut);
        }
        let lines: Vec<&str> = text
            .split(['\n', '\r'])
            .map(str::trim)
            .filter(|l| !l.is_empty() && !is_node_trailer(l))
            .collect();
        if let Some(last) = lines.last() {
            self.last_line = Some(clip(last, 300));
        }
        if self.error_line.is_none() {
            self.error_line = lines.iter().find(|l| is_error_line(l)).map(|l| clip(l, 300));
        }
    }
}

/// Runs a program to its end.
pub fn run(command: &str, args: &[&str], options: &RunOptions) -> Result<RunResult, HostError> {
    let mut log = match &options.log_file {
        Some(file) => {
            if let Some(dir) = file.parent() {
                fs::create_dir_all(dir).map_err(|e| HostError::io(dir, e))?;
            }
            let mut log = OpenOptions::new()
                .create(true)
                .append(true)
                .open(file)
                .map_err(|e| HostError::io(file, e))?;
            let _ = write!(
                log,
                "\n$ {command} {}  (in {})\n",
                args.join(" "),
                options.cwd.display()
            );
            Some(log)
        }
        None => None,
    };
    let mut process = Command::new(resolve_command(command));
    process
        .args(args)
        .current_dir(&options.cwd)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    if let Some(env) = &options.env {
        process.env_clear().envs(env.iter().map(|(k, v)| (k, v)));
    }
    hide_window(&mut process);
    let mut child = match process.spawn() {
        Ok(child) => child,
        Err(e) => {
            return Ok(RunResult {
                code: -1,
                last_line: Some(e.to_string()),
                output: String::new(),
            });
        }
    };
    let collected = Arc::new(Mutex::new((Collected::default(), log.take())));
    let mut pumps = Vec::new();
    let pipes: [Option<Box<dyn Read + Send>>; 2] = [
        child.stdout.take().map(|p| Box::new(p) as Box<dyn Read + Send>),
        child.stderr.take().map(|p| Box::new(p) as Box<dyn Read + Send>),
    ];
    for mut pipe in pipes.into_iter().flatten() {
        let collected = collected.clone();
        let quiet = options.quiet;
        pumps.push(thread::spawn(move || {
            let mut buf = [0u8; 8192];
            loop {
                let n = match pipe.read(&mut buf) {
                    Ok(0) | Err(_) => return,
                    Ok(n) => n,
                };
                let text = String::from_utf8_lossy(&buf[..n]).to_string();
                let mut guard = collected.lock().unwrap_or_else(|p| p.into_inner());
                if let Some(log) = guard.1.as_mut() {
                    let _ = log.write_all(&buf[..n]);
                }
                if !quiet {
                    let mut out = std::io::stdout().lock();
                    let _ = out.write_all(text.as_bytes());
                    let _ = out.flush();
                }
                guard.0.add(&text);
            }
        }));
    }
    let status = child
        .wait()
        .map_err(|e| HostError::message(format!("{command}: {e}")))?;
    for pump in pumps {
        let _ = pump.join();
    }
    let guard = collected.lock().unwrap_or_else(|p| p.into_inner());
    let collected = &guard.0;
    let code = status.code().unwrap_or(-1);
    let last_line = if code != 0 && collected.error_line.is_some() {
        collected.error_line.clone()
    } else {
        collected.last_line.clone()
    };
    Ok(RunResult {
        code,
        last_line,
        output: collected.output.clone(),
    })
}

/// A quick call for checks (`git --version`, `tailscale ip -4`): its trimmed output, or `None` when
/// it fails or takes more than 20 s.
pub fn capture(command: &str, args: &[&str], cwd: Option<&Path>) -> Result<Option<String>, HostError> {
    let mut process = Command::new(resolve_command(command));
    process
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null());
    if let Some(cwd) = cwd {
        process.current_dir(cwd);
    }
    hide_window(&mut process);
    let Ok(mut child) = process.spawn() else {
        return Ok(None);
    };
    let mut stdout = child.stdout.take();
    let reader = thread::spawn(move || {
        let mut out = Vec::new();
        if let Some(pipe) = stdout.as_mut() {
            let _ = pipe.read_to_end(&mut out);
        }
        out
    });
    let until = Instant::now() + Duration::from_secs(20);
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break Some(status),
            Ok(None) if Instant::now() < until => thread::sleep(Duration::from_millis(20)),
            _ => {
                let _ = child.kill();
                let _ = child.wait();
                break None;
            }
        }
    };
    let out = reader.join().unwrap_or_default();
    match status {
        Some(status) if status.success() => match String::from_utf8(out) {
            Ok(text) => Ok(Some(text.trim().to_string())),
            Err(_) => Ok(None),
        },
        _ => Ok(None),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_the_thrown_error_not_nodes_trailer() {
        assert!(is_error_line("Error: Catalog item not found: x"));
        assert!(is_error_line("TypeError: x is not a function"));
        assert!(is_error_line("Uncaught RangeError: too far"));
        assert!(!is_error_line("    at Object.<anonymous> (file.js:1:1)"));
        assert!(!is_error_line("Errors were found"));
        assert!(!is_error_line("Node.js v24.1.0"));
        let mut c = Collected::default();
        c.add("working\nError: Catalog item not found: x\n    at f (a.js:1)\n\nNode.js v24.1.0\n");
        assert_eq!(c.error_line.as_deref(), Some("Error: Catalog item not found: x"));
        assert_eq!(c.last_line.as_deref(), Some("at f (a.js:1)"));
    }

    #[test]
    fn keeps_the_last_64_kib_of_output() {
        let mut c = Collected::default();
        c.add(&"é".repeat(40 * 1024));
        c.add(&"x".repeat(1024));
        assert!(c.output.len() <= OUTPUT_KEPT);
        assert!(c.output.ends_with('x'));
    }

    fn shell(script: &str) -> (&'static str, Vec<String>) {
        if cfg!(windows) {
            ("cmd", vec!["/d".into(), "/c".into(), script.into()])
        } else {
            ("sh", vec!["-c".into(), script.into()])
        }
    }

    #[test]
    fn runs_a_program_to_its_end_with_its_code_and_last_line_and_logs_it() {
        let dir = tempfile::tempdir().unwrap();
        let log = dir.path().join("logs").join("run.log");
        let (program, args) = shell("echo one&& echo Error: broken&& echo two&& exit 3");
        let args: Vec<&str> = args.iter().map(String::as_str).collect();
        let options = RunOptions {
            cwd: dir.path().to_path_buf(),
            log_file: Some(log.clone()),
            quiet: true,
            ..Default::default()
        };
        let result = run(program, &args, &options).unwrap();
        assert_eq!(result.code, 3);
        assert_eq!(result.last_line.as_deref(), Some("Error: broken"), "{result:?}");
        assert!(fs::read_to_string(&log).unwrap().contains("two"));
        let missing = run("reforged-no-such-program", &[], &options).unwrap();
        assert_eq!(missing.code, -1);
        assert!(missing.last_line.is_some());
    }

    #[test]
    fn captures_a_checks_output_or_nothing() {
        let (program, args) = shell("echo  hello ");
        let args: Vec<&str> = args.iter().map(String::as_str).collect();
        assert_eq!(capture(program, &args, None).unwrap().as_deref(), Some("hello"));
        let (program, args) = shell("exit 1");
        let args: Vec<&str> = args.iter().map(String::as_str).collect();
        assert_eq!(capture(program, &args, None).unwrap(), None);
        assert_eq!(capture("reforged-no-such-program", &[], None).unwrap(), None);
    }
}
