//! Stub registered by RR1 so the workspace lists every planned member; the Director skeleton row replaces this crate.

/// The component's name, until the real crate lands.
pub const COMPONENT: &str = "director";

#[cfg(test)]
mod tests {
    #[test]
    fn names_the_component() {
        assert_eq!(super::COMPONENT, "director");
    }
}
