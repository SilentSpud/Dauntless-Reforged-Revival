//! `reforged-host multipliers`: the Node kit's `multipliers()` in `cli.ts`.
//!
//! Filled by RH3 (MIGRATION_PLAN.md section 4.4). It reads host.json, changes only `multipliers`
//! (so the rest of the file stays byte for byte) and rewrites the env files. Until then it returns
//! a typed "not implemented" error.

use crate::Outcome;
use crate::cli::MultipliersArgs;
use crate::error::not_implemented;
use crate::paths::Paths;

pub fn run(_paths: &Paths, _args: &MultipliersArgs) -> Outcome {
    not_implemented("reforged-host multipliers", "RH3")
}

#[cfg(test)]
mod tests {
    use crate::error::HostError;

    #[test]
    fn is_a_stub_until_rh3() {
        let paths = crate::paths::Paths::new("rr");
        let error = super::run(&paths, &Default::default()).unwrap_err();
        assert!(matches!(error, HostError::NotImplemented { row: "RH3", .. }), "{error}");
    }
}
