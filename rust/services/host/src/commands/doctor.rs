//! `reforged-host doctor`: the Node kit's `doctor` case in `cli.ts`, with [`crate::doctor`].
//!
//! Filled by RH3 (MIGRATION_PLAN.md section 4.4). Exit code 1 when a check fails, as today. Until
//! then it returns a typed "not implemented" error.

use crate::Outcome;
use crate::cli::DoctorArgs;
use crate::error::not_implemented;
use crate::paths::Paths;

pub fn run(_paths: &Paths, _args: &DoctorArgs) -> Outcome {
    not_implemented("reforged-host doctor", "RH3")
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
