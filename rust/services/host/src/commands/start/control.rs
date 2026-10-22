//! What stops a running server: Ctrl+C (and Ctrl+Break, closing the console, SIGTERM, SIGHUP) and,
//! for the launcher's Host page, its stdin (the Node kit's `watchStopInput()`).
//!
//! The Host page starts the server with `REFORGED_CONTROL=stdin` and its stdin a pipe: a `stop`
//! line, or the pipe closing (the launcher quit), stops the server as Ctrl+C does. Windows has no
//! signal a program can send another for that. The launcher also reads [`SERVER_READY_LINE`] from
//! the output, and probes the backend's port 61000 to see a server running (RD23).

use std::io::BufRead;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::thread;

use crate::error::HostError;

pub const CONTROL_ENV: &str = "REFORGED_CONTROL";
pub const SERVER_READY_LINE: &str = "REFORGED_SERVER_READY";

/// Whether the launcher controls this run through stdin.
pub fn controlled_by_stdin() -> bool {
    std::env::var(CONTROL_ENV).is_ok_and(|v| v == "stdin")
}

/// Reads `input` until a whole `stop` line (any case, spaces around it allowed) or its end, then
/// calls `on_stop`, once.
pub fn watch_stop_input(input: impl BufRead, on_stop: impl FnOnce()) {
    for line in input.lines() {
        match line {
            Ok(line) if line.trim().eq_ignore_ascii_case("stop") => break,
            Ok(_) => {}
            Err(_) => break,
        }
    }
    on_stop();
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StopReason {
    /// Ctrl+C, Ctrl+Break, the console closing, SIGINT, SIGTERM or SIGHUP.
    Signal(&'static str),
    /// A `stop` line on stdin, or stdin closing.
    Input,
}

/// A stop asked for, from any thread; the first reason is kept.
#[derive(Clone, Default)]
pub struct Control {
    asked: Arc<AtomicBool>,
    reason: Arc<(Mutex<Option<StopReason>>, Condvar)>,
}

impl Control {
    /// Listens for the stop signals and, when `stdin` is true, for the launcher's stop line.
    pub fn begin(stdin: bool) -> Result<Self, HostError> {
        let control = Self::default();
        control.listen_for_signals()?;
        if stdin {
            let on_input = control.clone();
            thread::Builder::new()
                .name("stop input".into())
                .spawn(move || watch_stop_input(std::io::stdin().lock(), || on_input.stop(StopReason::Input)))
                .map_err(|e| HostError::message(format!("could not watch stdin: {e}")))?;
        }
        Ok(control)
    }

    pub fn stop(&self, reason: StopReason) {
        let (slot, wake) = &*self.reason;
        let mut slot = slot.lock().unwrap_or_else(|p| p.into_inner());
        if slot.is_none() {
            *slot = Some(reason);
        }
        self.asked.store(true, Ordering::SeqCst);
        wake.notify_all();
    }

    pub fn asked(&self) -> bool {
        self.asked.load(Ordering::SeqCst)
    }

    /// Set once a stop is asked for.
    pub fn flag(&self) -> Arc<AtomicBool> {
        self.asked.clone()
    }

    /// Blocks until a stop is asked for.
    pub fn wait(&self) -> StopReason {
        let (slot, wake) = &*self.reason;
        let mut slot = slot.lock().unwrap_or_else(|p| p.into_inner());
        loop {
            if let Some(reason) = *slot {
                return reason;
            }
            slot = wake.wait(slot).unwrap_or_else(|p| p.into_inner());
        }
    }

    /// A thread with a small runtime waits for the signals; this returns once they are registered,
    /// so a Ctrl+C from here on stops the server in order instead of ending this process.
    fn listen_for_signals(&self) -> Result<(), HostError> {
        let control = self.clone();
        let (ready_tx, ready_rx) = std::sync::mpsc::channel::<Result<(), String>>();
        thread::Builder::new()
            .name("stop signals".into())
            .spawn(move || {
                let runtime = match tokio::runtime::Builder::new_current_thread().enable_all().build() {
                    Ok(runtime) => runtime,
                    Err(e) => {
                        let _ = ready_tx.send(Err(e.to_string()));
                        return;
                    }
                };
                runtime.block_on(signals(control, ready_tx));
            })
            .map_err(|e| HostError::message(format!("could not start a thread: {e}")))?;
        match ready_rx.recv() {
            Ok(Ok(())) => Ok(()),
            Ok(Err(e)) => Err(HostError::message(format!("could not listen for Ctrl+C: {e}"))),
            Err(_) => Err(HostError::message("could not listen for Ctrl+C")),
        }
    }
}

/// A program started in a new process group (as CI runners and some launchers start theirs) inherits
/// "ignore Ctrl+C", and a Ctrl+C would then never reach the handlers below: turn it back on.
#[cfg(windows)]
#[allow(unsafe_code)] // SetConsoleCtrlHandler
fn accept_ctrl_c() {
    // SAFETY: no handler routine; only clears this process's "ignore Ctrl+C" flag.
    let _ = unsafe { windows::Win32::System::Console::SetConsoleCtrlHandler(None, false) };
}

#[cfg(windows)]
async fn signals(control: Control, ready: std::sync::mpsc::Sender<Result<(), String>>) {
    use tokio::signal::windows;
    accept_ctrl_c();
    let registered =
        (|| Ok::<_, std::io::Error>((windows::ctrl_c()?, windows::ctrl_break()?, windows::ctrl_close()?)))();
    let (mut ctrl_c, mut ctrl_break, mut close) = match registered {
        Ok(signals) => signals,
        Err(e) => {
            let _ = ready.send(Err(e.to_string()));
            return;
        }
    };
    let _ = ready.send(Ok(()));
    loop {
        let name = tokio::select! {
            _ = ctrl_c.recv() => "Ctrl+C",
            _ = ctrl_break.recv() => "Ctrl+Break",
            _ = close.recv() => "the console closing",
        };
        control.stop(StopReason::Signal(name));
    }
}

#[cfg(unix)]
async fn signals(control: Control, ready: std::sync::mpsc::Sender<Result<(), String>>) {
    use tokio::signal::unix::{SignalKind, signal};
    let registered = (|| {
        Ok::<_, std::io::Error>((
            signal(SignalKind::interrupt())?,
            signal(SignalKind::terminate())?,
            signal(SignalKind::hangup())?,
        ))
    })();
    let (mut int, mut term, mut hup) = match registered {
        Ok(signals) => signals,
        Err(e) => {
            let _ = ready.send(Err(e.to_string()));
            return;
        }
    };
    let _ = ready.send(Ok(()));
    loop {
        let name = tokio::select! {
            _ = int.recv() => "SIGINT",
            _ = term.recv() => "SIGTERM",
            _ = hup.recv() => "SIGHUP",
        };
        control.stop(StopReason::Signal(name));
    }
}

#[cfg(test)]
mod tests {
    use std::io::{BufReader, Read};
    use std::sync::mpsc;

    use super::*;

    /// A pipe written in pieces, as the launcher writes it.
    struct Chunks(mpsc::Receiver<Vec<u8>>, Vec<u8>);

    impl Read for Chunks {
        fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
            if self.1.is_empty() {
                match self.0.recv() {
                    Ok(chunk) => self.1 = chunk,
                    Err(_) => return Ok(0),
                }
            }
            let n = buf.len().min(self.1.len());
            buf[..n].copy_from_slice(&self.1[..n]);
            self.1.drain(..n);
            Ok(n)
        }
    }

    #[test]
    fn stops_on_a_stop_line_or_when_the_launchers_pipe_closes_once() {
        let (tx, rx) = mpsc::channel();
        let (stopped_tx, stopped_rx) = mpsc::channel();
        let watcher = thread::spawn(move || {
            watch_stop_input(BufReader::new(Chunks(rx, Vec::new())), || stopped_tx.send(()).unwrap());
        });
        tx.send(b"hello\nSTO".to_vec()).unwrap();
        assert!(
            stopped_rx.recv_timeout(std::time::Duration::from_millis(200)).is_err(),
            "only a whole stop line"
        );
        tx.send(b"P\r\n".to_vec()).unwrap();
        stopped_rx.recv_timeout(std::time::Duration::from_secs(5)).unwrap();
        drop(tx);
        watcher.join().unwrap();
        assert!(
            stopped_rx.try_recv().is_err(),
            "stopped once, not again when the pipe closed"
        );

        let mut closed = 0;
        watch_stop_input(&b""[..], || closed += 1);
        assert_eq!(closed, 1, "the launcher quit: the pipe closed");
        let mut other = 0;
        watch_stop_input(&b"  Stop  \nmore\n"[..], || other += 1);
        assert_eq!(other, 1);
    }

    #[test]
    fn keeps_the_first_reason_and_wakes_the_waiter() {
        let control = Control::default();
        assert!(!control.asked());
        let waiter = {
            let control = control.clone();
            thread::spawn(move || control.wait())
        };
        control.stop(StopReason::Input);
        control.stop(StopReason::Signal("Ctrl+C"));
        assert_eq!(waiter.join().unwrap(), StopReason::Input);
        assert!(control.flag().load(Ordering::SeqCst));
    }

    #[test]
    fn keeps_the_launchers_contract() {
        assert_eq!(CONTROL_ENV, "REFORGED_CONTROL");
        assert_eq!(SERVER_READY_LINE, "REFORGED_SERVER_READY");
    }
}
