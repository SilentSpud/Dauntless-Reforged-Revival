//! Stub registered by RR1 so the workspace lists every planned member; RA1 replaces this crate.

/// The component's name, until the real crate lands.
pub const COMPONENT: &str = "launcher";

#[cfg(test)]
mod tests {
    #[test]
    fn names_the_component() {
        assert_eq!(super::COMPONENT, "launcher");
    }
}
