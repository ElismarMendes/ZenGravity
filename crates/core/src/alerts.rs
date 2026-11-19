//! Regras puras de alertas operacionais e controle de notificações (`NotifyGate`).

use crate::telemetry::eta_to_limit_secs;
use crate::ZEN_LIMIT;
use std::collections::HashMap;

/// Tipos de alerta ordenados por severidade (do mais crítico ao preventivo).
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[cfg_attr(feature = "serde", serde(rename_all = "snake_case"))]
pub enum AlertKind {
    /// Leitura desatualizada (agente sem reportar dentro de `stale_after_s`).
    Stale,
    /// Queda confirmada de conexão do jogo (DC).
    Disconnected,
    /// Personagem morto.
    Dead,
    /// Zen atingiu o limite de 1,999 bi (inventário cheio).
    Limit2B,
    /// Personagem parado (sem ganho de Zen por `stopped_after_s` ou Helper desligado).
    Stopped,
    /// Zen próximo de 2 bilhões (`ETA < near_limit_eta_s`).
    Near2B,
}

impl AlertKind {
    pub fn label_pt(self) -> &'static str {
        match self {
            AlertKind::Stale => "LEITURA VELHA",
            AlertKind::Disconnected => "DC",
            AlertKind::Dead => "MORTO",
            AlertKind::Limit2B => "LIMITE 2 BI",
            AlertKind::Stopped => "PAROU",
            AlertKind::Near2B => "QUASE 2 BI",
        }
    }

    pub fn is_critical(self) -> bool {
        !matches!(self, AlertKind::Near2B)
    }
}

#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[derive(Debug, Clone)]
pub struct AlertConfig {
    pub stale_after_s: u64,
    pub stopped_after_s: u64,
    pub limit_threshold: u64,
    pub near_limit_eta_s: u64,
    pub dead_window_s: u64,
}

impl Default for AlertConfig {
    fn default() -> Self {
        Self {
            stale_after_s: 15,
            stopped_after_s: 180,
            limit_threshold: 1_999_000_000,
            near_limit_eta_s: 30 * 60,
            dead_window_s: 10 * 60,
        }
    }
}

#[derive(Debug, Clone, Default)]
pub struct AlertEvalInput {
    pub last_report_mono_s: Option<u64>,
    pub disconnected: bool,
    pub is_paused: bool,
    pub zen: Option<u64>,
    pub zen_per_hour: f64,
    pub idle_secs: Option<u64>,
    pub is_dead: bool,
}

/// Avalia alertas para um personagem usando tempo monotônico.
/// Se a leitura estiver `Stale`, suprime falsos alertas derivados (`Stopped`, etc.).
pub fn evaluate_alerts(
    input: &AlertEvalInput,
    now_mono_s: u64,
    cfg: &AlertConfig,
) -> Vec<AlertKind> {
    let mut out = Vec::new();
    let is_stale = match input.last_report_mono_s {
        None => true,
        Some(t) => now_mono_s.saturating_sub(t) > cfg.stale_after_s,
    };
    if is_stale {
        out.push(AlertKind::Stale);
        return out;
    }
    if input.disconnected {
        out.push(AlertKind::Disconnected);
    }
    if input.is_dead {
        out.push(AlertKind::Dead);
    }
    if let Some(zen) = input.zen {
        let at_limit = zen >= cfg.limit_threshold;
        if at_limit {
            out.push(AlertKind::Limit2B);
        }
        if !input.disconnected && !input.is_paused && !at_limit {
            if let Some(idle) = input.idle_secs {
                if idle >= cfg.stopped_after_s {
                    out.push(AlertKind::Stopped);
                }
            }
        }
        if !at_limit {
            if let Some(eta) = eta_to_limit_secs(zen.min(ZEN_LIMIT), input.zen_per_hour) {
                if eta < cfg.near_limit_eta_s {
                    out.push(AlertKind::Near2B);
                }
            }
        }
    }
    out.sort();
    out.dedup();
    out
}

/// Portão de deduplicação de notificações com cooldown monotônico.
#[derive(Debug, Default, Clone)]
pub struct NotifyGate {
    active: HashMap<(String, AlertKind), u64>,
    last_sent: HashMap<(String, AlertKind), u64>,
}

impl NotifyGate {
    pub fn update(
        &mut self,
        char_key: &str,
        current: &[AlertKind],
        now_mono_s: u64,
        cooldown_s: u64,
    ) -> Vec<AlertKind> {
        let mut to_notify = Vec::new();
        for &kind in current {
            let id = (char_key.to_string(), kind);
            if self.active.contains_key(&id) {
                continue;
            }
            self.active.insert(id.clone(), now_mono_s);
            let cooldown_ok = match self.last_sent.get(&id) {
                Some(&last_t) => now_mono_s.saturating_sub(last_t) >= cooldown_s,
                None => true,
            };
            if cooldown_ok && kind.is_critical() {
                self.last_sent.insert(id, now_mono_s);
                to_notify.push(kind);
            }
        }
        self.active
            .retain(|(c, k), _| c != char_key || current.contains(k));
        to_notify
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stale_suppresses_derived_stopped_alerts() {
        let cfg = AlertConfig::default();
        let inp = AlertEvalInput {
            last_report_mono_s: Some(100),
            disconnected: false,
            is_paused: false,
            zen: Some(500_000_000),
            zen_per_hour: 100_000_000.0,
            idle_secs: Some(900),
            is_dead: false,
        };
        assert_eq!(evaluate_alerts(&inp, 120, &cfg), vec![AlertKind::Stale]);
        assert_eq!(evaluate_alerts(&inp, 105, &cfg), vec![AlertKind::Stopped]);
    }

    #[test]
    fn paused_character_does_not_trigger_false_stopped_alert() {
        let cfg = AlertConfig::default();
        let inp = AlertEvalInput {
            last_report_mono_s: Some(100),
            disconnected: false,
            is_paused: true,
            zen: Some(500_000_000),
            zen_per_hour: 0.0,
            idle_secs: Some(600),
            is_dead: false,
        };
        assert!(evaluate_alerts(&inp, 105, &cfg).is_empty());
    }
}
