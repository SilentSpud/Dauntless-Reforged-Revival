//! `reforged-host update`: the Node kit's `update()` in `cli.ts`, with [`crate::selfupdate`].
//!
//! Filled by RH5 (MIGRATION_PLAN.md section 4.4). It changes meaning (section 6): from a `git`
//! fast-forward of the checkout to the server release (RD8), and moves a Node-kit host over. Until
//! then it returns a typed "not implemented" error.

use crate::Outcome;
use crate::cli::UpdateArgs;
use crate::error::not_implemented;
use crate::paths::Paths;

pub fn run(_paths: &Paths, _args: &UpdateArgs) -> Outcome {
    not_implemented("reforged-host update", "RH5")
}

#[cfg(test)]
mod tests {
    use crate::error::HostError;

    #[test]
    fn is_a_stub_until_rh5() {
        let paths = crate::paths::Paths::new("rr");
        let error = super::run(&paths, &Default::default()).unwrap_err();
        assert!(matches!(error, HostError::NotImplemented { row: "RH5", .. }), "{error}");
    }
}
