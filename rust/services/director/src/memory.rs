//! Memory admission for hunt servers (`src/controllers/memoryAdmission.ts`, Director patch 0022).
//!
//! With GAMESERVER_MEMORY_GUARD=1 a new hunt starts only when the memory available now, minus what the
//! hunts started in the last GAMESERVER_MEMORY_RESERVATION_SECONDS still have to allocate, covers one more
//! hunt (GAMESERVER_HUNT_MEMORY_MB) and leaves GAMESERVER_MIN_FREE_MB free. Otherwise the request is
//! refused like a full hunt cap. "Available" is Linux's MemAvailable, elsewhere the free memory.

use std::path::Path;
use std::sync::Mutex;

use service_kit::env::Env;

const MB: u64 = 1024 * 1024;

type Available = Box<dyn Fn() -> Option<u64> + Send + Sync>;
type Clock = Box<dyn Fn() -> i64 + Send + Sync>;
/// `(id, until, bytes)`
type Held = Vec<(u64, i64, u64)>;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Status {
    pub enabled: bool,
    pub available_mb: Option<i64>,
    pub reserved_mb: i64,
    pub needed_mb: i64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HuntMemoryUnavailable(pub Status);

impl std::fmt::Display for HuntMemoryUnavailable {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let available = self.0.available_mb.map_or_else(|| "?".to_owned(), |mb| mb.to_string());
        write!(
            f,
            "Not enough free memory for another hunt server ({available} MB available, {} MB reserved for hunts still starting, {} MB needed)",
            self.0.reserved_mb, self.0.needed_mb
        )
    }
}

#[derive(Debug, Clone)]
pub struct Options {
    pub enabled: bool,
    pub min_free_mb: i64,
    pub hunt_mb: i64,
    pub hold_ms: i64,
}

pub struct MemoryAdmission {
    options: Options,
    available: Available,
    now: Clock,
    /// The last id given, and the reservations held.
    reservations: Mutex<(u64, Held)>,
}

/// One hunt's reservation: [`MemoryAdmission::give_back`] returns it early (a start that failed).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Reservation(Option<u64>);

impl MemoryAdmission {
    pub fn new(options: Options) -> Self {
        Self::with(
            options,
            Box::new(|| read_available_bytes(Path::new("/proc/meminfo"), cfg!(target_os = "linux"))),
            Box::new(|| chrono::Utc::now().timestamp_millis()),
        )
    }

    pub fn with(options: Options, available: Available, now: Clock) -> Self {
        Self {
            options,
            available,
            now,
            reservations: Mutex::new((0, Vec::new())),
        }
    }

    pub fn enabled(&self) -> bool {
        self.options.enabled
    }

    pub fn status(&self) -> Status {
        let now = (self.now)();
        let mut guard = self
            .reservations
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        guard.1.retain(|&(_, until, _)| until > now);
        let reserved: u64 = guard.1.iter().map(|&(_, _, bytes)| bytes).sum();
        drop(guard);
        let available = if self.options.enabled { (self.available)() } else { None };
        Status {
            enabled: self.options.enabled,
            available_mb: available.map(|b| (b / MB) as i64),
            reserved_mb: (reserved as f64 / MB as f64).round() as i64,
            needed_mb: self.options.hunt_mb + self.options.min_free_mb,
        }
    }

    /// Reserves one hunt's memory, or refuses. Unknown available memory admits the hunt: the guard
    /// never blocks a server it cannot measure.
    pub fn reserve(&self) -> Result<Reservation, HuntMemoryUnavailable> {
        if !self.options.enabled {
            return Ok(Reservation(None));
        }
        let status = self.status();
        if let Some(available) = status.available_mb
            && available - status.reserved_mb < status.needed_mb
        {
            return Err(HuntMemoryUnavailable(status));
        }
        let until = (self.now)() + self.options.hold_ms;
        let bytes = self.options.hunt_mb.max(0) as u64 * MB;
        let mut guard = self
            .reservations
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        guard.0 += 1;
        let id = guard.0;
        guard.1.push((id, until, bytes));
        Ok(Reservation(Some(id)))
    }

    pub fn give_back(&self, reservation: Reservation) {
        if let Reservation(Some(id)) = reservation {
            let mut guard = self
                .reservations
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            guard.1.retain(|&(r, _, _)| r != id);
        }
    }
}

/// MemAvailable from `/proc/meminfo` on Linux; the free memory elsewhere (or when it cannot be read).
pub fn read_available_bytes(meminfo: &Path, linux: bool) -> Option<u64> {
    if linux && let Ok(text) = std::fs::read_to_string(meminfo) {
        for line in text.lines() {
            if let Some(rest) = line.strip_prefix("MemAvailable:")
                && let Some(kb) = rest.trim().strip_suffix("kB")
                && let Ok(kb) = kb.trim().parse::<u64>()
            {
                return Some(kb * 1024);
            }
        }
    }
    free_memory()
}

#[cfg(windows)]
#[allow(unsafe_code)] // GlobalMemoryStatusEx
fn free_memory() -> Option<u64> {
    use windows::Win32::System::SystemInformation::{GlobalMemoryStatusEx, MEMORYSTATUSEX};
    let mut status = MEMORYSTATUSEX {
        dwLength: std::mem::size_of::<MEMORYSTATUSEX>() as u32,
        ..Default::default()
    };
    // SAFETY: `status` is a valid MEMORYSTATUSEX with its length set, as the call requires.
    unsafe { GlobalMemoryStatusEx(&mut status) }.ok()?;
    Some(status.ullAvailPhys).filter(|&b| b > 0)
}

#[cfg(target_os = "linux")]
#[allow(unsafe_code)] // sysconf(3)
fn free_memory() -> Option<u64> {
    // SAFETY: sysconf takes no pointers and only reads system configuration.
    let (pages, size) = unsafe { (libc::sysconf(libc::_SC_AVPHYS_PAGES), libc::sysconf(libc::_SC_PAGESIZE)) };
    (pages > 0 && size > 0).then(|| pages as u64 * size as u64)
}

#[cfg(not(any(target_os = "linux", windows)))]
fn free_memory() -> Option<u64> {
    None
}

/// The settings from the environment. A value that is not a whole number in range keeps its default
/// and is reported in the problems (logged by the caller), so a typo never stops the Director.
pub fn options_from_env(env: &Env) -> (Options, Vec<String>) {
    let mut problems = Vec::new();
    let mut read = |name: &str, fallback: i64, min: i64, max: i64| -> i64 {
        let raw = env.raw(name).map(service_kit::js::trim).unwrap_or("");
        if raw.is_empty() {
            return fallback;
        }
        match raw.parse::<i64>() {
            Ok(value) if raw.bytes().all(|b| b.is_ascii_digit()) && (min..=max).contains(&value) => value,
            _ => {
                problems.push(format!(
                    "{name}='{raw}' is not a whole number from {min} to {max}; using {fallback}"
                ));
                fallback
            }
        }
    };
    let options = Options {
        enabled: crate::config::enabled(env.raw("GAMESERVER_MEMORY_GUARD")),
        min_free_mb: read("GAMESERVER_MIN_FREE_MB", 1024, 0, 1_048_576),
        hunt_mb: read("GAMESERVER_HUNT_MEMORY_MB", 1536, 1, 1_048_576),
        hold_ms: read("GAMESERVER_MEMORY_RESERVATION_SECONDS", 120, 0, 3600) * 1000,
    };
    (options, problems)
}
