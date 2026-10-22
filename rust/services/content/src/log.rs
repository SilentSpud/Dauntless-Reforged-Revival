//! The content server's log: JSON lines (`service_kit::linelog`).

pub use service_kit::linelog::*;

#[cfg(test)]
pub mod tests {
    use std::future::Future;
    use std::sync::{Arc, Mutex};

    /// The log is one per process: tests that read it take turns.
    static CAPTURE: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

    type Lines = Arc<Mutex<Vec<String>>>;

    fn start() -> Lines {
        let lines = Lines::default();
        let sink = lines.clone();
        super::set_sink(Some(Box::new(move |line, _| {
            sink.lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .push(line.to_owned());
        })));
        lines
    }

    fn finish(lines: &Lines) -> Vec<String> {
        super::set_sink(None);
        lines.lock().unwrap_or_else(std::sync::PoisonError::into_inner).clone()
    }

    /// Runs `f` and returns what it logged (other tests' lines may be among them).
    pub fn captured<T>(f: impl FnOnce() -> T) -> (T, Vec<String>) {
        let _turn = CAPTURE.blocking_lock();
        let lines = start();
        let result = f();
        (result, finish(&lines))
    }

    /// [`captured`] for a future.
    pub async fn captured_async<T>(f: impl Future<Output = T>) -> (T, Vec<String>) {
        let _turn = CAPTURE.lock().await;
        let lines = start();
        let result = f.await;
        (result, finish(&lines))
    }
}
