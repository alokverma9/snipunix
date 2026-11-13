//! Pause-aware elapsed-time accounting for recordings.

use std::time::{Duration, Instant};

/// Tracks how long a recording has actually been running.
///
/// Time spent paused is excluded. `Instant` values are injectable so the
/// arithmetic can be unit-tested without sleeping.
#[derive(Debug, Clone, Copy)]
pub struct ElapsedTracker {
    started: Instant,
    paused_at: Option<Instant>,
    paused_total: Duration,
}

impl ElapsedTracker {
    #[must_use]
    pub fn new(started: Instant) -> Self {
        Self {
            started,
            paused_at: None,
            paused_total: Duration::ZERO,
        }
    }

    /// True while the tracker is paused.
    #[must_use]
    pub fn is_paused(&self) -> bool {
        self.paused_at.is_some()
    }

    /// Starts a pause. Calling this while already paused is a no-op.
    pub fn pause(&mut self, now: Instant) {
        if self.paused_at.is_none() {
            self.paused_at = Some(now);
        }
    }

    /// Ends a pause. Calling this while running is a no-op.
    pub fn resume(&mut self, now: Instant) {
        if let Some(paused_at) = self.paused_at.take() {
            self.paused_total += now.saturating_duration_since(paused_at);
        }
    }

    /// Elapsed running time at `now`.
    #[must_use]
    pub fn elapsed(&self, now: Instant) -> Duration {
        let reference = self.paused_at.unwrap_or(now);
        reference
            .saturating_duration_since(self.started)
            .saturating_sub(self.paused_total)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn elapsed_is_the_wall_time_while_running() {
        let start = Instant::now();
        let tracker = ElapsedTracker::new(start);
        assert_eq!(tracker.elapsed(start), Duration::ZERO);
        assert_eq!(
            tracker.elapsed(start + Duration::from_secs(5)),
            Duration::from_secs(5)
        );
    }

    #[test]
    fn paused_time_is_excluded() {
        let start = Instant::now();
        let mut tracker = ElapsedTracker::new(start);
        tracker.pause(start + Duration::from_secs(2));
        assert!(tracker.is_paused());
        assert_eq!(
            tracker.elapsed(start + Duration::from_secs(9)),
            Duration::from_secs(2)
        );
        tracker.resume(start + Duration::from_secs(9));
        assert!(!tracker.is_paused());
        assert_eq!(
            tracker.elapsed(start + Duration::from_secs(12)),
            Duration::from_secs(5)
        );
    }

    #[test]
    fn repeated_pause_and_resume_calls_do_not_double_count() {
        let start = Instant::now();
        let mut tracker = ElapsedTracker::new(start);
        tracker.pause(start + Duration::from_secs(1));
        tracker.pause(start + Duration::from_secs(3));
        tracker.resume(start + Duration::from_secs(4));
        tracker.resume(start + Duration::from_secs(8));
        assert_eq!(
            tracker.elapsed(start + Duration::from_secs(10)),
            Duration::from_secs(7)
        );
    }

    #[test]
    fn elapsed_never_goes_backwards() {
        let start = Instant::now();
        let mut tracker = ElapsedTracker::new(start);
        tracker.pause(start + Duration::from_secs(2));
        tracker.resume(start + Duration::from_secs(1));
        assert_eq!(
            tracker.elapsed(start + Duration::from_secs(5)),
            Duration::from_secs(5)
        );
    }
}
