//! `reforged-host publish-runtime`: the Node kit's `publish()` in `cli.ts`, with [`crate::publish`].
//!
//! Filled by RH4 (MIGRATION_PLAN.md section 4.4). The launcher's Host page calls this command,
//! so the version format and `runtime.json` exist once (RD23). Until then it returns a typed
//! "not implemented" error.

use crate::Outcome;
use crate::cli::PublishRuntimeArgs;
use crate::error::not_implemented;
use crate::paths::Paths;

pub fn run(_paths: &Paths, _args: &PublishRuntimeArgs) -> Outcome {
    not_implemented("reforged-host publish-runtime", "RH4")
}

#[cfg(test)]
mod tests {
    use crate::error::HostError;

    #[test]
    fn is_a_stub_until_rh4() {
        let paths = crate::paths::Paths::new("rr");
        let error = super::run(&paths, &Default::default()).unwrap_err();
        assert!(matches!(error, HostError::NotImplemented { row: "RH4", .. }), "{error}");
    }
}
