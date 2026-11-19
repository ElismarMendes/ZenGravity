//! Utilitários determinísticos de suporte a testes para o workspace ZenGravity.

use std::sync::atomic::{AtomicU64, Ordering};
use zengravity_core::clock::{Clock, MonotonicMs, UtcTimestampSecs};

/// Relógio determinístico avançado manualmente em testes (sem `std::thread::sleep`).
#[derive(Debug)]
pub struct FakeClock {
    mono_ms: AtomicU64,
    utc_s: AtomicU64,
}

impl FakeClock {
    pub fn new(initial_mono_ms: MonotonicMs, initial_utc_s: UtcTimestampSecs) -> Self {
        Self {
            mono_ms: AtomicU64::new(initial_mono_ms),
            utc_s: AtomicU64::new(initial_utc_s),
        }
    }

    pub fn advance_ms(&self, delta_ms: u64) {
        let prev = self.mono_ms.fetch_add(delta_ms, Ordering::SeqCst);
        let new_mono = prev + delta_ms;
        let delta_s = (new_mono / 1000) - (prev / 1000);
        if delta_s > 0 {
            self.utc_s.fetch_add(delta_s, Ordering::SeqCst);
        }
    }

    pub fn advance_secs(&self, delta_s: u64) {
        self.advance_ms(delta_s * 1000);
    }
}

impl Default for FakeClock {
    fn default() -> Self {
        Self::new(10_000, 1_795_000_000)
    }
}

impl Clock for FakeClock {
    fn monotonic_ms(&self) -> MonotonicMs {
        self.mono_ms.load(Ordering::SeqCst)
    }

    fn utc_secs(&self) -> UtcTimestampSecs {
        self.utc_s.load(Ordering::SeqCst)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fake_clock_advances_monotonically() {
        let clock = FakeClock::default();
        let m0 = clock.monotonic_ms();
        let u0 = clock.utc_secs();
        clock.advance_secs(15);
        assert_eq!(clock.monotonic_ms(), m0 + 15_000);
        assert_eq!(clock.utc_secs(), u0 + 15);
    }
}
