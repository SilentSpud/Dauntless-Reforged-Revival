//! The checks `doctor` prints (`legacy/ReforgedHost/src/doctor.ts`).
//!
//! Filled by RH3. `start` (RH2) and `setup` (RH3) use the three helpers below.

use crate::error::{HostError, not_implemented};

/// This PC's Tailscale IPv4 (`tailscale ip -4`), or `None` when Tailscale is not running.
pub fn tailscale_ipv4() -> Result<Option<String>, HostError> {
    not_implemented("finding the Tailscale address", "RH3")
}

/// Whether something listens on the TCP port (`host`: 127.0.0.1 when `None`).
pub fn port_in_use(_port: u16, _host: Option<&str>) -> Result<bool, HostError> {
    not_implemented("checking a port", "RH3")
}

/// How to find the program on a port (`ss -tlnp ...` on Linux, `netstat -ano | findstr ...` on Windows).
pub fn port_owner_hint(_port: u16) -> Result<String, HostError> {
    not_implemented("naming a port's owner", "RH3")
}
