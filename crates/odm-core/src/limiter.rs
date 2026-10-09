//! Async bandwidth limiter. Clones share one budget. A rate of 0 means
//! unlimited.
//!
//! The limiter keeps a "theoretical arrival time": every acquired byte
//! pushes it forward by `1 / rate` seconds and a caller waits until the
//! time it was handed (minus a small burst allowance) has come. Debt is
//! therefore never forgiven, so many connections sharing one limiter can't
//! exceed the rate, and every wait is paid in full.

use parking_lot::Mutex;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;
use tokio::time::Instant;
use tokio_util::sync::CancellationToken;

/// Bytes worth this much time may pass without waiting (burst allowance).
const BURST: Duration = Duration::from_millis(250);
/// Sleep in slices so rate changes take effect promptly.
const SLICE: Duration = Duration::from_millis(250);

#[derive(Clone)]
pub struct Limiter {
    inner: Arc<Inner>,
}

struct Inner {
    rate: AtomicU64,
    /// bumped whenever the rate changes; pending waits end early
    generation: AtomicU64,
    tat: Mutex<Instant>,
}

impl Limiter {
    pub fn new(rate_bps: u64) -> Self {
        Self {
            inner: Arc::new(Inner {
                rate: AtomicU64::new(rate_bps),
                generation: AtomicU64::new(0),
                tat: Mutex::new(Instant::now()),
            }),
        }
    }

    pub fn unlimited() -> Self {
        Self::new(0)
    }

    pub fn set_rate(&self, bps: u64) {
        let old = self.inner.rate.swap(bps, Ordering::SeqCst);
        if old != bps {
            *self.inner.tat.lock() = Instant::now();
            self.inner.generation.fetch_add(1, Ordering::SeqCst);
        }
    }

    pub fn rate(&self) -> u64 {
        self.inner.rate.load(Ordering::SeqCst)
    }

    /// Wait until `bytes` may be consumed.
    pub async fn acquire(&self, bytes: u64) {
        self.wait(bytes, None).await;
    }

    /// Like [`acquire`](Self::acquire) but returns `false` as soon as
    /// `cancel` fires.
    pub async fn acquire_cancellable(&self, bytes: u64, cancel: &CancellationToken) -> bool {
        self.wait(bytes, Some(cancel)).await
    }

    async fn wait(&self, bytes: u64, cancel: Option<&CancellationToken>) -> bool {
        let rate = self.rate();
        if rate == 0 || bytes == 0 {
            return true;
        }
        let generation = self.inner.generation.load(Ordering::SeqCst);
        let deadline = {
            let mut tat = self.inner.tat.lock();
            let now = Instant::now();
            if *tat < now {
                *tat = now;
            }
            *tat += Duration::from_secs_f64(bytes as f64 / rate as f64);
            tat.checked_sub(BURST).unwrap_or(now)
        };
        loop {
            let now = Instant::now();
            if deadline <= now {
                return true;
            }
            if self.inner.generation.load(Ordering::SeqCst) != generation {
                // the rate changed (or the limit was lifted): start over
                return true;
            }
            let step = (deadline - now).min(SLICE);
            match cancel {
                Some(c) => {
                    tokio::select! {
                        _ = c.cancelled() => return false,
                        _ = tokio::time::sleep(step) => {}
                    }
                }
                None => tokio::time::sleep(step).await,
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn many_callers_share_the_rate() {
        let l = Limiter::new(1_000_000);
        let t = Instant::now();
        let mut set = tokio::task::JoinSet::new();
        for _ in 0..16 {
            let l = l.clone();
            set.spawn(async move {
                for _ in 0..10 {
                    l.acquire(25_000).await; // 16 * 10 * 25 KB = 4 MB
                }
            });
        }
        while set.join_next().await.is_some() {}
        let secs = t.elapsed().as_secs_f64();
        assert!(secs > 3.5, "4 MB at 1 MB/s took only {secs:.2}s");
        assert!(secs < 5.5, "too slow: {secs:.2}s");
    }

    #[tokio::test(start_paused = true)]
    async fn debt_is_paid_in_full() {
        let l = Limiter::new(10_000);
        let t = Instant::now();
        l.acquire(100_000).await; // 10 s worth, minus the burst allowance
        l.acquire(10_000).await; // 1 s more
        let secs = t.elapsed().as_secs_f64();
        assert!(secs >= 10.5, "only waited {secs:.2}s for 11 s worth of data");
        assert!(secs < 11.5, "waited {secs:.2}s");
    }

    #[tokio::test]
    async fn cancellation_ends_a_wait() {
        let l = Limiter::new(1000);
        let c = CancellationToken::new();
        let c2 = c.clone();
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(100)).await;
            c2.cancel();
        });
        let t = Instant::now();
        // 60 s worth of bytes
        assert!(!l.acquire_cancellable(60_000, &c).await);
        assert!(t.elapsed() < Duration::from_secs(2));
    }

    #[tokio::test]
    async fn lifting_the_limit_ends_waits() {
        let l = Limiter::new(1000);
        let l2 = l.clone();
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(100)).await;
            l2.set_rate(0);
        });
        let t = Instant::now();
        l.acquire(60_000).await;
        assert!(t.elapsed() < Duration::from_secs(2));
    }
}
