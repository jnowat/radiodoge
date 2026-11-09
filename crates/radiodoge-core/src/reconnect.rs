//! Auto-reconnect backoff schedule for the desktop serial watchdog.
//!
//! The GUI sleeps 2 s between connection checks, then waits
//! [`RECONNECT_BACKOFF_INITIAL_SECS`] after a failed reconnect attempt and
//! doubles that wait up to [`RECONNECT_BACKOFF_MAX_SECS`] (5 → 10 → 20 → 40 → 60).
//! Keeping the math here lets us unit-test the schedule without spinning Tokio.

/// First wait after a failed reconnect attempt, in seconds.
pub const RECONNECT_BACKOFF_INITIAL_SECS: u64 = 5;

/// Cap for the exponential reconnect wait, in seconds.
pub const RECONNECT_BACKOFF_MAX_SECS: u64 = 60;

/// Next backoff after a failed reconnect (`current` is the wait that just elapsed).
pub fn next_reconnect_backoff_secs(current: u64) -> u64 {
    current.saturating_mul(2).min(RECONNECT_BACKOFF_MAX_SECS)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn doubles_until_the_sixty_second_cap() {
        assert_eq!(next_reconnect_backoff_secs(5), 10);
        assert_eq!(next_reconnect_backoff_secs(10), 20);
        assert_eq!(next_reconnect_backoff_secs(20), 40);
        assert_eq!(next_reconnect_backoff_secs(40), 60);
        assert_eq!(next_reconnect_backoff_secs(60), 60);
        assert_eq!(next_reconnect_backoff_secs(100), 60);
    }

    #[test]
    fn initial_constant_matches_documented_schedule() {
        assert_eq!(RECONNECT_BACKOFF_INITIAL_SECS, 5);
        assert_eq!(
            next_reconnect_backoff_secs(RECONNECT_BACKOFF_INITIAL_SECS),
            10
        );
    }
}
