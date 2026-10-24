//! Stub registered by RR1 so the workspace lists every planned member; RB5 replaces this crate.

/// The component's name, until the real crate lands.
pub const COMPONENT: &str = "backend";

/// `reforged-host backend`: refuses to run until the real crate lands.
pub fn run() -> std::process::ExitCode {
    eprintln!("backend: not implemented yet (backend is a stub)");
    std::process::ExitCode::FAILURE
}

#[cfg(test)]
mod tests {
    #[test]
    fn names_the_component() {
        assert_eq!(super::COMPONENT, "backend");
    }
}
