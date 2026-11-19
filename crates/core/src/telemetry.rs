//! Telemetria pura de Zen: cálculo de ganho por hora (`Zen/h`), tempo ocioso (`idle_secs`)
//! e previsão de chegada ao limite de 2 bilhões (`ETA 2B`), usando tempo monotônico.

use crate::{MAX_FARM_STEP, ZEN_LIMIT};
use std::collections::VecDeque;

#[derive(Debug, Clone)]
pub struct ZenTracker {
    window_s: u64,
    gains: VecDeque<(u64, u64)>,
    last: Option<(u64, u64)>,
    first_seen: Option<u64>,
    last_increase_at: Option<u64>,
}

impl ZenTracker {
    pub fn new(window_s: u64) -> Self {
        Self {
            window_s: window_s.max(60),
            gains: VecDeque::new(),
            last: None,
            first_seen: None,
            last_increase_at: None,
        }
    }

    /// Registra uma leitura de Zen em segundos monotônicos (`now_mono_s`).
    /// Leituras fora de ordem são ignoradas. Saltos maiores que `MAX_FARM_STEP` (baú/trade)
    /// atualizam o patamar, mas não inflam o Zen/h de farm.
    pub fn push(&mut self, now_mono_s: u64, zen: u64) {
        if let Some((last_t, _)) = self.last {
            if now_mono_s < last_t {
                return;
            }
        }
        self.first_seen.get_or_insert(now_mono_s);
        if let Some((_, prev_zen)) = self.last {
            if zen > prev_zen {
                let step = zen - prev_zen;
                if step <= MAX_FARM_STEP {
                    self.gains.push_back((now_mono_s, step));
                    self.last_increase_at = Some(now_mono_s);
                }
            }
        }
        self.last = Some((now_mono_s, zen));
        self.prune(now_mono_s);
    }

    fn prune(&mut self, now_mono_s: u64) {
        let cut = now_mono_s.saturating_sub(self.window_s);
        while matches!(self.gains.front(), Some((t, _)) if *t <= cut) {
            self.gains.pop_front();
        }
    }

    /// Ganho de Zen por hora na janela móvel.
    pub fn per_hour(&self, now_mono_s: u64) -> f64 {
        let Some(first) = self.first_seen else {
            return 0.0;
        };
        let cut = now_mono_s.saturating_sub(self.window_s);
        let sum: u64 = self
            .gains
            .iter()
            .filter(|(t, _)| *t > cut)
            .map(|(_, g)| *g)
            .sum();
        let span = now_mono_s.saturating_sub(first).min(self.window_s).max(60);
        sum as f64 * 3600.0 / span as f64
    }

    /// Segundos monotônicos desde a última subida válida de farm.
    pub fn idle_secs(&self, now_mono_s: u64) -> Option<u64> {
        let since = self.last_increase_at.or(self.first_seen)?;
        Some(now_mono_s.saturating_sub(since))
    }

    pub fn last_zen(&self) -> Option<u64> {
        self.last.map(|(_, z)| z)
    }
}

/// Segundos estimados até atingir o limite de 2 bilhões (`ZEN_LIMIT`) no ritmo atual.
pub fn eta_to_limit_secs(zen: u64, per_hour: f64) -> Option<u64> {
    if per_hour <= 0.0 || !per_hour.is_finite() {
        return None;
    }
    let left = ZEN_LIMIT.saturating_sub(zen) as f64;
    Some((left / per_hour * 3600.0).round() as u64)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tracks_steady_farm_rate_and_ignores_vault_jumps() {
        let mut tracker = ZenTracker::new(3600);
        let mut zen = 100_000_000;
        for step in 0..=120 {
            tracker.push(step * 5, zen);
            zen += 500_000; // +500k a cada 5s = 360M/h
        }
        let rate = tracker.per_hour(600);
        assert!((rate - 360_000_000.0).abs() < 2_000_000.0, "rate={rate}");
        assert_eq!(tracker.idle_secs(600), Some(0));

        // Salto de 500M vindo do baú não conta como farm
        tracker.push(605, zen + 500_000_000);
        assert_eq!(tracker.idle_secs(620), Some(20));
    }

    #[test]
    fn calculates_eta_to_2b_limit() {
        assert_eq!(eta_to_limit_secs(1_900_000_000, 200_000_000.0), Some(1800));
        assert_eq!(eta_to_limit_secs(500_000_000, 0.0), None);
    }
}
