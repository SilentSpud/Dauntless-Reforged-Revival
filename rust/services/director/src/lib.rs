//! Stub registered by RR1 so the workspace lists every planned member; the Director skeleton row replaces this crate.

/// The component's name, until the real crate lands.
pub const COMPONENT: &str = "director";

/// `reforged-host director`: refuses to run until the real crate lands.
pub fn run() -> std::process::ExitCode {
    eprintln!("director: not implemented yet (director is a stub)");
    std::process::ExitCode::FAILURE
}

#[cfg(test)]
mod tests {
    #[test]
    fn names_the_component() {
        assert_eq!(super::COMPONENT, "director");
    }
}
