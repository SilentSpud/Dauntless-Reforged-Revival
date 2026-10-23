//! `reforged-host doctor`: the Node kit's `doctor` case in `cli.ts`, with [`crate::doctor`].
//! Exit code 1 when a check fails, as today.

use std::process::ExitCode;

use crate::Outcome;
use crate::cli::DoctorArgs;
use crate::doctor::{DoctorOptions, print_checks, run_doctor};
use crate::paths::Paths;

pub fn run(paths: &Paths, args: &DoctorArgs) -> Outcome {
    let checks = run_doctor(
        paths,
        &DoctorOptions {
            hash_exe: !args.no_hash,
        },
    )?;
    let mut say = |line: &str| println!("{line}");
    let (fails, warns) = print_checks(&checks, &mut say);
    say(&summary(fails, warns));
    Ok(if fails > 0 {
        ExitCode::FAILURE
    } else {
        ExitCode::SUCCESS
    })
}

pub fn summary(fails: usize, warns: usize) -> String {
    format!(
        "\n{fails} problem{}, {warns} warning{}.",
        if fails == 1 { "" } else { "s" },
        if warns == 1 { "" } else { "s" }
    )
}

#[cfg(test)]
mod tests {
    #[test]
    fn counts_problems_and_warnings() {
        assert_eq!(super::summary(1, 0), "\n1 problem, 0 warnings.");
        assert_eq!(super::summary(2, 1), "\n2 problems, 1 warning.");
    }
}
