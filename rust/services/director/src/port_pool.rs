//! Hunt UDP ports with an owner per held port (`src/controllers/portPool.ts`, 2026-10-10 1.14.7).
//!
//! A late or duplicate release from an old owner must not free a port a newer hunt holds:
//! - `take`: oldest free port first (FIFO), so a just-freed port is reused last
//! - `release`: frees only for the port's current owner, otherwise false and nothing changes
//! - `reclaim`: frees leases whose owner is no longer listed and whose process is gone, the fallback
//!   when the exit event never arrives

use std::collections::VecDeque;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Lease {
    pub port: u16,
    pub owner: String,
    pub pid: Option<u32>,
}

#[derive(Debug, Default)]
pub struct PortPool {
    free: VecDeque<u16>,
    // In the order taken, as a JavaScript Map iterates.
    held: Vec<Lease>,
}

impl PortPool {
    pub fn new() -> Self {
        Self::default()
    }

    fn lease(&self, port: u16) -> Option<&Lease> {
        self.held.iter().find(|l| l.port == port)
    }

    /// Ignores a port that is already free or held.
    pub fn add(&mut self, port: u16) {
        if self.lease(port).is_none() && !self.free.contains(&port) {
            self.free.push_back(port);
        }
    }

    pub fn take(&mut self, owner: &str) -> Option<u16> {
        let port = self.free.pop_front()?;
        self.held.push(Lease {
            port,
            owner: owner.to_owned(),
            pid: None,
        });
        Some(port)
    }

    pub fn attach_pid(&mut self, port: u16, owner: &str, pid: Option<u32>) -> bool {
        match self.held.iter_mut().find(|l| l.port == port) {
            Some(lease) if lease.owner == owner => {
                lease.pid = pid;
                true
            }
            _ => false,
        }
    }

    pub fn release(&mut self, port: u16, owner: &str) -> bool {
        let Some(index) = self.held.iter().position(|l| l.port == port && l.owner == owner) else {
            return false;
        };
        self.held.remove(index);
        self.free.push_back(port);
        true
    }

    /// Releases every lease whose owner is not live and whose process is gone (or never had a pid). A
    /// live pid keeps its lease even unlisted: that process may still own the socket.
    pub fn reclaim(&mut self, is_owner_live: impl Fn(&str) -> bool, is_pid_alive: impl Fn(u32) -> bool) -> Vec<Lease> {
        let mut reclaimed = Vec::new();
        for lease in self.held.clone() {
            if is_owner_live(&lease.owner) {
                continue;
            }
            if lease.pid.is_some_and(&is_pid_alive) {
                continue;
            }
            if self.release(lease.port, &lease.owner) {
                reclaimed.push(lease);
            }
        }
        reclaimed
    }

    pub fn owner_of(&self, port: u16) -> Option<&str> {
        self.lease(port).map(|l| l.owner.as_str())
    }

    pub fn pid_of(&self, port: u16) -> Option<u32> {
        self.lease(port).and_then(|l| l.pid)
    }

    pub fn held_pids(&self) -> impl Iterator<Item = u32> + '_ {
        self.held.iter().filter_map(|l| l.pid)
    }

    pub fn free_count(&self) -> usize {
        self.free.len()
    }

    pub fn held_count(&self) -> usize {
        self.held.len()
    }
}
