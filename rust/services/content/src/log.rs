//! The content server's log: JSON lines (`service_kit::linelog`).

pub use service_kit::linelog::*;

#[cfg(test)]
pub mod tests {
    use std::sync::{Arc, Mutex};

    /// The log is one per process: tests that read it take turns.
    static CAPTURE: Mutex<()> = Mutex::new(());

    /// Runs `f` and returns what it logged (other tests' lines may be among them).
    pub fn captured<T>(f: impl FnOnce() -> T) -> (T, Vec<String>) {
        let _turn = CAPTURE.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        let lines = Arc::new(Mutex::new(Vec::new()));
        let sink = lines.clone();
        super::set_sink(Some(Box::new(move |line, _| {
            sink.lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .push(line.to_owned());
        })));
        let result = f();
        super::set_sink(None);
        let lines = lines.lock().unwrap_or_else(std::sync::PoisonError::into_inner).clone();
        (result, lines)
    }
}
