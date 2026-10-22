//! `reforged-host backup`: new in the Rust kit: a backup of the server's database (`pg_dump`
//! once Postgres is managed, RD15).
//!
//! Filled by RH3 (MIGRATION_PLAN.md section 4.4). It has no options yet; RH3 adds them through
//! the integration pass. Until then it returns a typed "not implemented" error.

use crate::Outcome;
use crate::error::not_implemented;
use crate::paths::Paths;

pub fn run(_paths: &Paths) -> Outcome {
    not_implemented("reforged-host backup", "RH3")
}

#[cfg(test)]
mod tests {
    use crate::error::HostError;

    #[test]
    fn is_a_stub_until_rh3() {
        let paths = crate::paths::Paths::new("rr");
        let error = super::run(&paths).unwrap_err();
        assert!(matches!(error, HostError::NotImplemented { row: "RH3", .. }), "{error}");
    }
}
