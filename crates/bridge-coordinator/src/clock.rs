#[cfg(any(test, feature = "test-clock"))]
use std::sync::Mutex;
#[cfg(any(test, feature = "test-clock"))]
use std::time::Duration;
use std::time::Instant;

/// Injected time source. Covers BOTH monotonic (`now_instant`, for idle TTL) and wall-clock
/// (`now_ms`, for task rows). Keeps bridge-core's no-`Date::now` discipline at the library boundary:
/// the binary supplies `SystemClock`; tests supply an advanceable `ManualClock`.
pub trait Clock: Send + Sync {
    fn now_instant(&self) -> Instant;
    fn now_ms(&self) -> i64;
}

pub struct SystemClock;

impl Clock for SystemClock {
    fn now_instant(&self) -> Instant {
        Instant::now()
    }

    fn now_ms(&self) -> i64 {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis() as i64)
            .unwrap_or(0)
    }
}

/// An advanceable test clock (PFIX-D). Idle-TTL tests advance it, and the monotonic instant tracks
/// the same steps from a fixed base. Interior mutability lets `&self` callers use the `Clock` trait.
#[cfg(any(test, feature = "test-clock"))]
pub struct ManualClock {
    base: Instant,
    state: Mutex<(i64, Duration)>,
}

#[cfg(any(test, feature = "test-clock"))]
impl ManualClock {
    pub fn new(now_ms: i64) -> Self {
        Self {
            base: Instant::now(),
            state: Mutex::new((now_ms, Duration::ZERO)),
        }
    }

    pub fn advance(&self, by: Duration) {
        let mut state = self.state.lock().unwrap();
        state.0 += by.as_millis() as i64;
        state.1 += by;
    }
}

#[cfg(any(test, feature = "test-clock"))]
impl Clock for ManualClock {
    fn now_instant(&self) -> Instant {
        self.base + self.state.lock().unwrap().1
    }

    fn now_ms(&self) -> i64 {
        self.state.lock().unwrap().0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn system_clock_now_ms_is_positive() {
        assert!(SystemClock.now_ms() > 1_600_000_000_000);
    }

    #[test]
    fn manual_clock_is_advanceable() {
        let c = ManualClock::new(1_700_000_000_000);
        assert_eq!(c.now_ms(), 1_700_000_000_000);
        let t0 = c.now_instant();
        c.advance(std::time::Duration::from_millis(500));
        assert_eq!(c.now_ms(), 1_700_000_000_500);
        assert!(c.now_instant() >= t0 + std::time::Duration::from_millis(500));
    }
}
