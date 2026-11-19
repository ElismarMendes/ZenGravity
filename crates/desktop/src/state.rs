//! Estado local do cliente Desktop (`zengravity-desktop::state`).
//!
//! Mantém a réplica local sincronizada por `seq` a partir de `CentralToPanel::Snapshot`
//! e `CentralToPanel::Delta`, sem bloquear a thread gráfica e detectando automaticamente
//! quando a própria conexão com a Central fica desatualizada (`central_stale`).

use std::collections::{BTreeMap, VecDeque};
use std::time::Instant;
use zengravity_core::map::WorldId;
use zengravity_core::spots::SpotDirectory;
use zengravity_protocol::{CentralToPanel, PanelCharacterView, PanelEventLog, PanelMachineView};

const MAX_LOCAL_EVENTS: usize = 150;
/// Se a Central ficar mais de 4,5 segundos sem enviar Delta nem Heartbeat, o Desktop
/// sinaliza `CENTRAL DESATUALIZADA (STALE) / RECONECTANDO`.
pub const CENTRAL_STALE_TIMEOUT_MS: u64 = 4_500;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ActiveTab {
    Overview,
    Map2D,
    EventsAndAlerts,
    MachinesHealth,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StateFilter {
    All,
    Farming,
    AttentionRequired,
    Recovering,
    StaleOrDc,
}

impl StateFilter {
    pub fn label_pt(self) -> &'static str {
        match self {
            StateFilter::All => "Todos os Estados",
            StateFilter::Farming => "Farmando",
            StateFilter::AttentionRequired => "Com Alerta / Parado / Morto",
            StateFilter::Recovering => "Recuperando",
            StateFilter::StaleOrDc => "Desatualizado / DC",
        }
    }
}

pub struct DesktopClientState {
    pub last_seq: u64,
    pub simulation_mode: bool,
    pub emergency_stop_global: bool,
    pub central_connected: bool,
    pub central_ws_url: String,
    pub last_central_packet_instant: Option<Instant>,
    pub central_mono_ms: u64,
    pub machines: BTreeMap<String, PanelMachineView>,
    pub characters: BTreeMap<String, PanelCharacterView>,
    pub spot_directory: SpotDirectory,
    pub events: VecDeque<PanelEventLog>,
    pub last_ack_message: Option<(bool, String, Instant)>,
    pub resync_needed: bool,
}

impl Default for DesktopClientState {
    fn default() -> Self {
        Self {
            last_seq: 0,
            simulation_mode: true,
            emergency_stop_global: false,
            central_connected: false,
            central_ws_url: String::new(),
            last_central_packet_instant: None,
            central_mono_ms: 0,
            machines: BTreeMap::new(),
            characters: BTreeMap::new(),
            spot_directory: SpotDirectory::default(),
            events: VecDeque::new(),
            last_ack_message: None,
            resync_needed: false,
        }
    }
}

impl DesktopClientState {
    pub fn apply_central_message(&mut self, msg: CentralToPanel, now: Instant) {
        self.central_connected = true;
        self.last_central_packet_instant = Some(now);

        match msg {
            CentralToPanel::Snapshot(snap) => {
                self.last_seq = snap.seq;
                self.simulation_mode = snap.simulation_mode;
                self.emergency_stop_global = snap.emergency_stop_global;
                self.central_mono_ms = snap.central_mono_ms;
                self.resync_needed = false;

                self.machines.clear();
                for m in snap.machines {
                    self.machines.insert(m.machine_id.clone(), m);
                }
                self.characters.clear();
                for c in snap.characters {
                    self.characters.insert(c.name.clone(), c);
                }
                self.spot_directory = snap.spot_directory;
                self.events = snap.recent_events.into_iter().collect();
            }
            CentralToPanel::Delta(delta) => {
                if self.last_seq > 0 && delta.seq > self.last_seq + 1 {
                    // Gap de sequência detectado: aplica delta mas sinaliza pedido de Snapshot
                    self.resync_needed = true;
                }
                self.last_seq = delta.seq;
                self.central_mono_ms = delta.central_mono_ms;
                self.emergency_stop_global = delta.emergency_stop_global;

                for m in delta.updated_machines {
                    self.machines.insert(m.machine_id.clone(), m);
                }
                for c in delta.updated_characters {
                    self.characters.insert(c.name.clone(), c);
                }
                if let Some(dir) = delta.spot_directory {
                    self.spot_directory = dir;
                }
                for ev in delta.new_events.into_iter().rev() {
                    if !self.events.iter().any(|e| e.id == ev.id) {
                        self.events.push_front(ev);
                    }
                }
                self.events.truncate(MAX_LOCAL_EVENTS);
            }
            CentralToPanel::CommandAck {
                accepted, message, ..
            } => {
                self.last_ack_message = Some((accepted, message, now));
            }
            CentralToPanel::Heartbeat {
                seq,
                central_mono_ms,
                ..
            } => {
                if self.last_seq > 0 && seq > self.last_seq {
                    self.resync_needed = true;
                }
                self.central_mono_ms = central_mono_ms;
            }
        }
    }

    /// Verifica se o fluxo da Central está atrasado (`stale`) em relação ao `Instant` atual.
    pub fn is_central_stream_stale(&self, now: Instant) -> bool {
        if !self.central_connected {
            return true;
        }
        match self.last_central_packet_instant {
            None => true,
            Some(last) => {
                now.saturating_duration_since(last).as_millis() as u64 > CENTRAL_STALE_TIMEOUT_MS
            }
        }
    }

    pub fn central_silence_secs(&self, now: Instant) -> u64 {
        self.last_central_packet_instant
            .map(|last| now.saturating_duration_since(last).as_secs())
            .unwrap_or(0)
    }

    pub fn total_zen(&self) -> u64 {
        self.characters.values().filter_map(|c| c.zen.value).sum()
    }

    pub fn total_zen_per_hour(&self) -> f64 {
        self.characters.values().map(|c| c.zen_per_hour).sum()
    }

    pub fn active_alerts_count(&self) -> usize {
        self.characters.values().map(|c| c.alerts.len()).sum()
    }

    pub fn characters_in_world(&self, world: WorldId) -> Vec<&PanelCharacterView> {
        self.characters
            .values()
            .filter(|c| c.position.value.is_some_and(|p| p.world == world))
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;
    use zengravity_central::hub::CentralHub;
    use zengravity_core::alerts::AlertConfig;
    use zengravity_simulator::SimulatorEngine;

    #[test]
    fn desktop_client_state_applies_snapshots_deltas_and_detects_central_stale() {
        let mut sim = SimulatorEngine::new(20261118);
        let mut hub = CentralHub::new(true, AlertConfig::default());
        for h in sim.initial_hellos() {
            hub.apply_agent_message(h, 100, 1_795_000_000);
        }
        for r in sim.step(1_000, 1_795_000_001) {
            hub.apply_agent_message(r, 1_000, 1_795_000_001);
        }

        let mut client = DesktopClientState::default();
        let t0 = Instant::now();
        assert!(client.is_central_stream_stale(t0));

        let snap = hub.snapshot(1_000, 1_795_000_001);
        client.apply_central_message(CentralToPanel::Snapshot(Box::new(snap)), t0);
        assert!(!client.is_central_stream_stale(t0));
        assert_eq!(client.machines.len(), 4);
        assert_eq!(client.characters.len(), 9);
        assert!(client.total_zen() > 5_000_000_000);

        // Após 6 segundos sem receber pacotes da central, marca stream stale
        let t_stale = t0 + Duration::from_secs(6);
        assert!(client.is_central_stream_stale(t_stale));
    }
}
