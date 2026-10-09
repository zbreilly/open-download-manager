//! Async token-bucket bandwidth limiter. Clones share one bucket.
//! A rate of 0 means unlimited.

use parking_lot::Mutex;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

#[derive(Clone)]
pub struct Limiter {
    inner: Arc<Inner>,
}

struct Inner {
    rate: AtomicU64,
    state: Mutex<(Instant, f64)>,
}

impl Limiter {
    pub fn new(rate_bps: u64) -> Self {
        Self {
            inner: Arc::new(Inner {
                rate: AtomicU64::new(rate_bps),
                state: Mutex::new((Instant::now(), 0.0)),
            }),
        }
    }

    pub fn unlimited() -> Self {
        Self::new(0)
    }

    pub fn set_rate(&self, bps: u64) {
        let old = self.inner.rate.swap(bps, Ordering::SeqCst);
        if old != bps {
            let mut st = self.inner.state.lock();
            *st = (Instant::now(), 0.0);
        }
    }

    pub fn rate(&self) -> u64 {
        self.inner.rate.load(Ordering::SeqCst)
    }

    /// Wait until `bytes` may be consumed. The bucket can go into debt so
    /// large chunks never deadlock; the debt is paid back by sleeping.
    pub async fn acquire(&self, bytes: u64) {
        let rate = self.rate();
        if rate == 0 || bytes == 0 {
            return;
        }
        let wait = {
            let mut st = self.inner.state.lock();
            let now = Instant::now();
            let dt = now.duration_since(st.0).as_secs_f64();
            st.0 = now;
            // burst cap: 250 ms worth of tokens
            st.1 = (st.1 + rate as f64 * dt).min(rate as f64 * 0.25);
            st.1 -= bytes as f64;
            if st.1 < 0.0 {
                Duration::from_secs_f64((-st.1) / rate as f64)
            } else {
                Duration::ZERO
            }
        };
        if wait > Duration::ZERO {
            tokio::time::sleep(wait.min(Duration::from_secs(5))).await;
        }
    }
}
