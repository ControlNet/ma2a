use std::{collections::VecDeque, sync::Mutex};

#[derive(Debug)]
pub(super) struct LoginRateLimit {
    attempts: Mutex<VecDeque<i64>>,
    limit: usize,
    window_ms: i64,
}

impl LoginRateLimit {
    pub(super) fn new(limit: usize, window_ms: i64) -> Self {
        Self {
            attempts: Mutex::new(VecDeque::with_capacity(limit)),
            limit,
            window_ms,
        }
    }

    pub(super) fn allow(&self, now_ms: i64) -> bool {
        let Ok(mut attempts) = self.attempts.lock() else {
            return false;
        };
        for attempt in &mut *attempts {
            *attempt = (*attempt).min(now_ms);
        }
        while attempts
            .front()
            .is_some_and(|attempt| now_ms.saturating_sub(*attempt) >= self.window_ms)
        {
            attempts.pop_front();
        }
        if attempts.len() >= self.limit {
            return false;
        }
        attempts.push_back(now_ms);
        true
    }
}

#[cfg(test)]
mod tests {
    use super::LoginRateLimit;

    #[test]
    fn wall_clock_rollback_recovers_within_window() {
        let limit = LoginRateLimit::new(5, 60_000);
        for _ in 0..5 {
            assert!(limit.allow(10_000_000));
        }
        assert!(!limit.allow(10_000_000));
        assert!(!limit.allow(5_000_000));
        assert!(!limit.allow(5_059_999));
        assert!(limit.allow(5_060_000));
    }

    #[test]
    fn partial_attempts_before_rollback_do_not_pin_eviction() {
        let limit = LoginRateLimit::new(5, 60_000);
        assert!(limit.allow(10_000_000));
        for _ in 0..4 {
            assert!(limit.allow(5_000_000));
        }
        assert!(!limit.allow(5_000_000));
        assert!(limit.allow(5_060_000));
    }

    #[test]
    fn rollback_without_intermediate_attempts_recovers_within_one_window() {
        let limit = LoginRateLimit::new(5, 60_000);
        for _ in 0..5 {
            assert!(limit.allow(10_000_000));
        }
        assert!(!limit.allow(5_060_000));
        assert!(limit.allow(5_120_000));
    }
}
