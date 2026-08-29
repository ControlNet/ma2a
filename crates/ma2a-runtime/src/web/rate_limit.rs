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
