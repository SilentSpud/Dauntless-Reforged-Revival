//! `reforged-host start`: the Node kit's `start()` and `watchStopInput()` in `cli.ts`, with
//! [`crate::services`].
//!
//! Filled by RH2 (MIGRATION_PLAN.md section 4.4). It keeps the launcher's control contract:
//! `REFORGED_CONTROL=stdin`, the `REFORGED_SERVER_READY` line and the port 61000 probe (RD23). Until
//! then it returns a typed "not implemented" error.

use crate::Outcome;
use crate::error::not_implemented;
use crate::paths::Paths;

pub fn run(_paths: &Paths) -> Outcome {
    not_implemented("reforged-host start", "RH2")
}

#[cfg(test)]
mod tests {
    use crate::error::HostError;

    #[test]
    fn is_a_stub_until_rh2() {
        let paths = crate::paths::Paths::new("rr");
        let error = super::run(&paths).unwrap_err();
        assert!(matches!(error, HostError::NotImplemented { row: "RH2", .. }), "{error}");
    }
}
