//! Task progress pushed to the page over a `Channel`, at most about ten times a second (row L7): the
//! executable check and the runtime download report far more often than the page can draw.

use std::time::{Duration, Instant};

use super::types::TaskProgress;

pub const PROGRESS_INTERVAL: Duration = Duration::from_millis(100);

/// Decides which progress values reach the page: one per interval, plus every change the page must
/// not miss (another task, the last value of a task).
#[derive(Debug, Default)]
pub struct Throttle {
    last: Option<(Instant, TaskProgress)>,
}

impl Throttle {
    pub fn pass(&mut self, now: Instant, progress: &TaskProgress) -> bool {
        let due = match &self.last {
            None => true,
            Some((at, previous)) => {
                now.duration_since(*at) >= PROGRESS_INTERVAL
                    || previous.kind != progress.kind
                    || progress.done_bytes >= progress.total_bytes && progress.files_done >= progress.files_total
            }
        };
        if due {
            self.last = Some((now, progress.clone()));
        }
        due
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bridge::types::TaskKind;

    fn progress(kind: TaskKind, done: u64) -> TaskProgress {
        TaskProgress {
            kind,
            total_bytes: 100,
            done_bytes: done,
            files_total: 2,
            files_done: u32::from(done >= 100) * 2,
            bytes_per_second: 0.0,
            eta_seconds: None,
            current_file: None,
        }
    }

    #[test]
    fn about_ten_a_second_but_never_a_change_that_matters() {
        let start = Instant::now();
        let at = |ms| start + Duration::from_millis(ms);
        let mut throttle = Throttle::default();
        assert!(throttle.pass(at(0), &progress(TaskKind::Runtime, 1)));
        assert!(!throttle.pass(at(10), &progress(TaskKind::Runtime, 2)));
        assert!(!throttle.pass(at(99), &progress(TaskKind::Runtime, 3)));
        assert!(throttle.pass(at(100), &progress(TaskKind::Runtime, 4)));
        assert!(throttle.pass(at(102), &progress(TaskKind::Verify, 4)), "another task");
        assert!(throttle.pass(at(103), &progress(TaskKind::Verify, 100)), "the end");
        let passed = (0..1000)
            .filter(|ms| throttle.pass(at(200 + ms), &progress(TaskKind::Verify, 50)))
            .count();
        assert_eq!(passed, 10);
    }
}
