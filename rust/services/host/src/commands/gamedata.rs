//! `reforged-host gamedata`: the Node kit's `gamedata()` in `cli.ts`, with [`crate::gamedata`],
//! [`crate::gamepack`], [`crate::progression`] and [`crate::captures`].
//!
//! Filled by RH4 (MIGRATION_PLAN.md section 4.4). Until RB11 it runs the Node generators in
//! `paradox/backend/scripts`, after it the `backend gamedata` subcommands. Until then it returns
//! a typed "not implemented" error.

use crate::Outcome;
use crate::cli::GamedataArgs;
use crate::error::not_implemented;
use crate::paths::Paths;

pub fn run(_paths: &Paths, _args: &GamedataArgs) -> Outcome {
    not_implemented("reforged-host gamedata", "RH4")
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
