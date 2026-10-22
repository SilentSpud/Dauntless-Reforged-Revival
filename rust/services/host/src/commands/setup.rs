//! `reforged-host setup`: the Node kit's `setup()`, `linuxSetup()`, `firewallScript()`,
//! `hostConfigFromArgs()` and the `*FromArgs` option readers in `cli.ts`.
//!
//! Filled by RH3 (MIGRATION_PLAN.md section 4.4). It builds a new [`crate::hostconfig::HostConfig`]
//! with `file: FileForm::default()`, as the Node kit builds a new object, so the keys come in
//! setup's order; it makes the secrets with [`crate::hostconfig::new_secrets`]; RH5 changes its
//! install step to the server release. Until then it returns a typed "not implemented" error.

use crate::Outcome;
use crate::cli::SetupArgs;
use crate::error::not_implemented;
use crate::paths::Paths;

pub fn run(_paths: &Paths, _args: &SetupArgs) -> Outcome {
    not_implemented("reforged-host setup", "RH3")
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
