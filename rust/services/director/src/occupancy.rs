//! How many players a hub has, and when an on-demand hub has been empty long enough to stop
//! (`src/controllers/hubOccupancy.ts`, Director patch 0033).
//!
//! The runtime writes every change of its remote-client count to its own log beside the game
//! executable, `mysticparadox_dll_port<port>.log`:
//!     [12:00:01.234] pid=292 [NetConnEdge] tick=1234 connCount 0 -> 1 connMax=... NetDriver=...
//! [`RuntimeLogOccupancy`] follows that file from where it ended when the hub was started, so only the
//! current process's lines count (under Wine the pid in the log is not the one the Director knows).
//! A count not read yet is unknown, and an unknown count never stops anything.

use std::io::{Read, Seek, SeekFrom};
use std::path::PathBuf;

// More new bytes than this between two reads: only the last part is read (the newest edge is all that counts).
const MAX_READ_BYTES: u64 = 4 * 1024 * 1024;

pub struct RuntimeLogOccupancy {
    pub file: PathBuf,
    position: u64,
    partial: String,
    count: Option<i64>,
}

impl RuntimeLogOccupancy {
    pub fn new(file: PathBuf) -> Self {
        Self {
            file,
            position: 0,
            partial: String::new(),
            count: None,
        }
    }

    /// Called just before the hub's process starts: everything the log holds now belongs to earlier runs.
    pub fn begin(&mut self) {
        self.position = self.size();
        self.partial.clear();
        self.count = None;
    }

    fn size(&self) -> u64 {
        std::fs::metadata(&self.file).map(|m| m.len()).unwrap_or(0)
    }

    /// Reads what the log gained since the last call: the newest connection count, or `None` before the
    /// first edge.
    pub fn poll(&mut self) -> Option<i64> {
        let size = self.size();
        if size < self.position {
            // Truncated or replaced: start over from its beginning.
            self.position = 0;
            self.partial.clear();
        }
        if size == self.position {
            return self.count;
        }
        let mut start = self.position;
        if size - start > MAX_READ_BYTES {
            start = size - MAX_READ_BYTES;
            self.partial.clear();
        }
        let Ok(bytes) = read_range(&self.file, start, size - start) else {
            return self.count;
        };
        self.position = start + bytes.len() as u64;
        // latin1, as the Node code reads it: one char per byte.
        let text: String = bytes.iter().map(|&b| b as char).collect();
        let joined = std::mem::take(&mut self.partial) + &text;
        let mut lines: Vec<&str> = joined.split('\n').collect();
        let last = lines.pop().unwrap_or("");
        self.partial = if last.len() > 65536 {
            String::new()
        } else {
            last.to_owned()
        };
        for line in lines {
            if let Some(after) = edge_after(line) {
                self.count = (after >= 0).then_some(after);
            }
        }
        self.count
    }
}

fn read_range(file: &PathBuf, start: u64, len: u64) -> std::io::Result<Vec<u8>> {
    let mut handle = std::fs::File::open(file)?;
    handle.seek(SeekFrom::Start(start))?;
    let mut bytes = Vec::with_capacity(len as usize);
    handle.take(len).read_to_end(&mut bytes)?;
    Ok(bytes)
}

/// `[NetConnEdge] tick=<n> connCount <before> -> <after>`: the count after the edge.
fn edge_after(line: &str) -> Option<i64> {
    let rest = &line[line.find("[NetConnEdge] tick=")? + "[NetConnEdge] tick=".len()..];
    let digits = rest.bytes().take_while(u8::is_ascii_digit).count();
    if digits == 0 {
        return None;
    }
    let rest = rest[digits..].strip_prefix(" connCount ")?;
    let (_, rest) = signed(rest)?;
    let rest = rest.strip_prefix(" -> ")?;
    signed(rest).map(|(after, _)| after)
}

fn signed(text: &str) -> Option<(i64, &str)> {
    let negative = text.starts_with('-');
    let body = if negative { &text[1..] } else { text };
    let digits = body.bytes().take_while(u8::is_ascii_digit).count();
    if digits == 0 {
        return None;
    }
    let value: i64 = body[..digits].parse().ok()?;
    Some((if negative { -value } else { value }, &body[digits..]))
}

/// When an on-demand hub has been empty long enough: no remote client for `idle_ms`, counted from the
/// latest of "the count went to 0", "the hub became ready" and "someone asked for the hub" (a player on
/// the way in has no connection yet). 0 never stops.
#[derive(Debug, Clone)]
pub struct IdleStopPolicy {
    pub idle_ms: i64,
    idle_since: Option<i64>,
}

impl IdleStopPolicy {
    pub fn new(idle_ms: i64) -> Self {
        Self {
            idle_ms,
            idle_since: None,
        }
    }

    pub fn reset(&mut self) {
        self.idle_since = None;
    }

    pub fn should_stop(&mut self, now: i64, count: Option<i64>, ready_at: i64, last_request_at: i64) -> bool {
        if self.idle_ms <= 0 {
            return false;
        }
        if count.is_none_or(|c| c > 0) {
            self.idle_since = None;
            return false;
        }
        let since = *self.idle_since.get_or_insert(now);
        now - since.max(ready_at).max(last_request_at) >= self.idle_ms
    }
}
