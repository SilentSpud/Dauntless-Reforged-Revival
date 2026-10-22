//! `reforged-host upstream`: the Node kit's `upstream()` in `cli.ts`, with [`crate::upstream`].
//!
//! Filled by RH5 (MIGRATION_PLAN.md section 4.4). RH5 removes it with `npm run upstream` (RP4): one
//! of the two command changes section 6 allows. Until then it returns a typed "not implemented" error.

use crate::Outcome;
use crate::error::not_implemented;
use crate::paths::Paths;

pub fn run(_paths: &Paths) -> Outcome {
    not_implemented("reforged-host upstream", "RH5")
}

#[cfg(test)]
mod tests {
    use crate::error::HostError;

    #[test]
    fn is_a_stub_until_rh5() {
        let paths = crate::paths::Paths::new("rr");
        let error = super::run(&paths).unwrap_err();
        assert!(matches!(error, HostError::NotImplemented { row: "RH5", .. }), "{error}");
    }
}
