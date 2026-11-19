//! Hub reativo da Central (`zengravity-central::hub`).
//!
//! Agrega relatórios de múltiplos PCs (`AgentToCentral`), calcula taxas (`ZenTracker`),
//! determina o estado verdadeiro (`CharLifecycleState`) e os alertas (`AlertKind`),
//! coordena o diretório de spots (`SpotDirectory`) e gera `PanelSnapshot` + `PanelDelta`
//! sequenciados para o painel desktop.

use std::collections::{BTreeMap, VecDeque};
use zengravity_core::alerts::{evaluate_alerts, AlertConfig, AlertEvalInput};
use zengravity_core::chat::{ChatEvent, KillerKind, PkDeduper};
use zengravity_core::clock::{MonotonicMs, UtcTimestampSecs};
use zengravity_core::entities::{EntityKind, NearbyEntity};
use zengravity_core::lifecycle::{
    classify_character_state, CharLifecycleState, Confidence, StateEvalInput,
};
use zengravity_core::spots::{SpotCoord, SpotDirectory, SpotObservation};
use zengravity_core::telemetry::{eta_to_limit_secs, ZenTracker};
use zengravity_core::{ARRIVAL_RADIUS, OCCUPIED_RADIUS};
use zengravity_protocol::{
    AgentCharReport, AgentToCentral, Command, EventSeverity, MachineHealth, PanelCharacterView,
    PanelDelta, PanelEventLog, PanelMachineView, PanelSnapshot, PROTOCOL_VERSION,
};

const MAX_SPARK_POINTS: usize = 90;
const MAX_EVENTS: usize = 120;

struct CharacterRecord {
    tracker: ZenTracker,
    last_report: AgentCharReport,
    last_report_mono_ms: MonotonicMs,
    last_death_utc_s: Option<UtcTimestampSecs>,
    sparkline: VecDeque<(u64, u64)>,
}

struct MachineRecord {
    machine_id: String,
    agent_version: String,
    connected: bool,
    is_simulator: bool,
    last_report_mono_ms: MonotonicMs,
    last_report_utc_s: UtcTimestampSecs,
    health: MachineHealth,
    chars: BTreeMap<String, CharacterRecord>,
}

/// Efeito colateral para persistência assíncrona no SQLite.
#[derive(Debug, Clone)]
pub enum PersistenceEffect {
    ZenPoint {
        machine_id: String,
        char_name: String,
        ts_utc_s: u64,
        zen: u64,
    },
    PkEvent {
        ts_utc_s: u64,
        machine_id: String,
        killer: String,
        killer_kind: String,
        victim: String,
    },
}

pub struct CentralHub {
    pub seq: u64,
    pub simulation_mode: bool,
    pub emergency_stop_global: bool,
    pub alert_config: AlertConfig,
    machines: BTreeMap<String, MachineRecord>,
    pub spot_directory: SpotDirectory,
    events: VecDeque<PanelEventLog>,
    next_event_id: u64,
    pk_deduper: PkDeduper,
}

impl CentralHub {
    pub fn new(simulation_mode: bool, alert_config: AlertConfig) -> Self {
        Self {
            seq: 1,
            simulation_mode,
            emergency_stop_global: false,
            alert_config,
            machines: BTreeMap::new(),
            spot_directory: SpotDirectory::default(),
            events: VecDeque::new(),
            next_event_id: 1,
            pk_deduper: PkDeduper::new(120),
        }
    }

    #[allow(clippy::too_many_arguments)]
    pub fn push_event(
        &mut self,
        now_mono_ms: MonotonicMs,
        now_utc_s: UtcTimestampSecs,
        severity: EventSeverity,
        machine_id: &str,
        char_name: Option<String>,
        title: impl Into<String>,
        detail: impl Into<String>,
    ) -> PanelEventLog {
        let ev = PanelEventLog {
            id: self.next_event_id,
            timestamp_utc_s: now_utc_s,
            timestamp_mono_ms: now_mono_ms,
            severity,
            machine_id: machine_id.to_string(),
            char_name,
            title: title.into(),
            detail: detail.into(),
        };
        self.next_event_id += 1;
        self.events.push_front(ev.clone());
        self.events.truncate(MAX_EVENTS);
        ev
    }

    pub fn apply_agent_message(
        &mut self,
        msg: AgentToCentral,
        now_mono_ms: MonotonicMs,
        now_utc_s: UtcTimestampSecs,
    ) -> (Option<PanelDelta>, Vec<PersistenceEffect>) {
        let mut effects = Vec::new();
        let mut new_events = Vec::new();

        match msg {
            AgentToCentral::Hello {
                machine_id,
                agent_version,
                is_simulator,
                ..
            } => {
                if is_simulator {
                    self.simulation_mode = true;
                }
                let entry =
                    self.machines
                        .entry(machine_id.clone())
                        .or_insert_with(|| MachineRecord {
                            machine_id: machine_id.clone(),
                            agent_version: agent_version.clone(),
                            connected: true,
                            is_simulator,
                            last_report_mono_ms: now_mono_ms,
                            last_report_utc_s: now_utc_s,
                            health: MachineHealth::default(),
                            chars: BTreeMap::new(),
                        });
                entry.connected = true;
                entry.agent_version = agent_version.clone();
                entry.is_simulator = is_simulator;
                entry.last_report_mono_ms = now_mono_ms;
                entry.last_report_utc_s = now_utc_s;

                new_events.push(self.push_event(
                    now_mono_ms,
                    now_utc_s,
                    EventSeverity::Info,
                    &machine_id,
                    None,
                    format!("Agente {machine_id} conectado"),
                    format!(
                        "Versão {} ({})",
                        agent_version,
                        if is_simulator {
                            "Modo Simulação"
                        } else {
                            "Agente Real"
                        }
                    ),
                ));

                self.seq += 1;
                let machine_view = self.build_machine_view(&machine_id, now_mono_ms).unwrap();
                let delta = PanelDelta {
                    seq: self.seq,
                    central_mono_ms: now_mono_ms,
                    central_utc_s: now_utc_s,
                    emergency_stop_global: self.emergency_stop_global,
                    updated_machines: vec![machine_view],
                    updated_characters: Vec::new(),
                    spot_directory: None,
                    new_events,
                };
                (Some(delta), effects)
            }
            AgentToCentral::Report {
                machine_id,
                health,
                chars,
                events,
                catalog_spots,
                ..
            } => {
                let now_mono_s = now_mono_ms / 1000;
                for (prio, spot) in catalog_spots.iter().enumerate() {
                    self.spot_directory
                        .add_catalog_spot(*spot, prio as u32, &machine_id);
                }

                let m = self
                    .machines
                    .entry(machine_id.clone())
                    .or_insert_with(|| MachineRecord {
                        machine_id: machine_id.clone(),
                        agent_version: "0.1.0".into(),
                        connected: true,
                        is_simulator: self.simulation_mode,
                        last_report_mono_ms: now_mono_ms,
                        last_report_utc_s: now_utc_s,
                        health: health.clone(),
                        chars: BTreeMap::new(),
                    });
                m.connected = true;
                m.last_report_mono_ms = now_mono_ms;
                m.last_report_utc_s = now_utc_s;
                m.health = health;

                let mut golden_sightings: Vec<(String, NearbyEntity)> = Vec::new();

                for c_rep in chars {
                    let char_name = c_rep
                        .title
                        .name
                        .clone()
                        .unwrap_or_else(|| format!("PID-{}", c_rep.pid));

                    self.spot_directory.register_member(
                        &machine_id,
                        &char_name,
                        c_rep.home_spot,
                        c_rep.active_spot,
                    );

                    // Atualiza observação dos spots do catálogo que estão dentro do raio de visão deste personagem
                    if let (Some(pos), Some(nearby)) =
                        (c_rep.position.value, c_rep.nearby_entities.value.as_ref())
                    {
                        for entity in nearby {
                            if matches!(
                                entity.kind,
                                EntityKind::GoldenMonster | EntityKind::GoldenBoss
                            ) {
                                golden_sightings.push((char_name.clone(), entity.clone()));
                            }
                        }

                        let catalog_in_sight: Vec<SpotCoord> = self
                            .spot_directory
                            .catalog
                            .iter()
                            .map(|c| c.spot)
                            .filter(|sp| sp.is_near(pos, OCCUPIED_RADIUS))
                            .collect();

                        for sp in catalog_in_sight {
                            let foreign: Vec<String> = nearby
                                .iter()
                                .filter(|e| {
                                    e.kind == EntityKind::Player
                                        && !self.spot_directory.is_friendly(&e.name)
                                        && (e.x - sp.x).abs().max((e.y - sp.y).abs())
                                            <= OCCUPIED_RADIUS
                                })
                                .map(|e| e.name.clone())
                                .collect();
                            self.spot_directory
                                .observations
                                .retain(|o| !o.spot.is_near(sp, ARRIVAL_RADIUS));
                            self.spot_directory.observations.push(SpotObservation {
                                spot: sp,
                                observed_at_mono_s: now_mono_s,
                                observed_by: char_name.clone(),
                                foreign_players: foreign,
                            });
                        }
                    }

                    let rec = m
                        .chars
                        .entry(char_name.clone())
                        .or_insert_with(|| CharacterRecord {
                            tracker: ZenTracker::new(3600),
                            last_report: c_rep.clone(),
                            last_report_mono_ms: now_mono_ms,
                            last_death_utc_s: None,
                            sparkline: VecDeque::new(),
                        });

                    if let Some(zen_val) = c_rep.zen.value {
                        if c_rep.zen.confidence != Confidence::Unavailable {
                            rec.tracker.push(now_mono_s, zen_val);
                            let should_push_spark = rec
                                .sparkline
                                .back()
                                .is_none_or(|(last_t, _)| now_mono_s >= *last_t + 2);
                            if should_push_spark {
                                rec.sparkline.push_back((now_mono_s, zen_val));
                                if rec.sparkline.len() > MAX_SPARK_POINTS {
                                    rec.sparkline.pop_front();
                                }
                                effects.push(PersistenceEffect::ZenPoint {
                                    machine_id: machine_id.clone(),
                                    char_name: char_name.clone(),
                                    ts_utc_s: now_utc_s,
                                    zen: zen_val,
                                });
                            }
                        }
                    }

                    if c_rep.hp.value == Some(0) && rec.last_report.hp.value.unwrap_or(1) > 0 {
                        rec.last_death_utc_s = Some(now_utc_s);
                    }

                    rec.last_report = c_rep;
                    rec.last_report_mono_ms = now_mono_ms;
                }

                // Registra eventos de chat / PK / Golden
                for (observer, golden) in golden_sightings {
                    // Emite no log apenas periodicamente para não inundar a cada tick
                    if self.seq % 12 == 0 {
                        new_events.push(self.push_event(
                            now_mono_ms,
                            now_utc_s,
                            EventSeverity::Golden,
                            &machine_id,
                            Some(observer.clone()),
                            format!("{} avistado na memória!", golden.name),
                            format!(
                                "Observado por {} em ({}, {}) — classe {}",
                                observer, golden.x, golden.y, golden.class_id
                            ),
                        ));
                    }
                }

                for ev in events {
                    match ev {
                        ChatEvent::Pk {
                            killer,
                            killer_kind,
                            victim,
                        } => {
                            if self.pk_deduper.accept(&killer, &victim, now_mono_s) {
                                let kind_str = match killer_kind {
                                    KillerKind::Player => "Player",
                                    KillerKind::Monster => "Monster",
                                };
                                for mach in self.machines.values_mut() {
                                    if let Some(c) = mach.chars.get_mut(&victim) {
                                        c.last_death_utc_s = Some(now_utc_s);
                                    }
                                }
                                effects.push(PersistenceEffect::PkEvent {
                                    ts_utc_s: now_utc_s,
                                    machine_id: machine_id.clone(),
                                    killer: killer.clone(),
                                    killer_kind: kind_str.into(),
                                    victim: victim.clone(),
                                });
                                new_events.push(self.push_event(
                                    now_mono_ms,
                                    now_utc_s,
                                    EventSeverity::Critical,
                                    &machine_id,
                                    Some(victim.clone()),
                                    format!("Morte registrada: {victim}"),
                                    format!(
                                        "[PK] {killer} ({}) matou {victim}",
                                        if killer_kind == KillerKind::Player {
                                            "Jogador"
                                        } else {
                                            "Monstro"
                                        }
                                    ),
                                ));
                            }
                        }
                        ChatEvent::Pick { char_name, jewel } => {
                            new_events.push(self.push_event(
                                now_mono_ms,
                                now_utc_s,
                                EventSeverity::Info,
                                &machine_id,
                                Some(char_name.clone()),
                                format!("{char_name} coletou {:?}", jewel),
                                "Evento [PickItem] confirmado no chat",
                            ));
                        }
                        ChatEvent::ServerNotice { text } => {
                            new_events.push(self.push_event(
                                now_mono_ms,
                                now_utc_s,
                                EventSeverity::Golden,
                                &machine_id,
                                None,
                                "Aviso Global do Servidor",
                                text,
                            ));
                        }
                    }
                }

                self.seq += 1;
                let updated_machines = self
                    .build_machine_view(&machine_id, now_mono_ms)
                    .into_iter()
                    .collect();
                let updated_characters =
                    self.build_characters_for_machine(&machine_id, now_mono_ms);
                let delta = PanelDelta {
                    seq: self.seq,
                    central_mono_ms: now_mono_ms,
                    central_utc_s: now_utc_s,
                    emergency_stop_global: self.emergency_stop_global,
                    updated_machines,
                    updated_characters,
                    spot_directory: Some(self.spot_directory.clone()),
                    new_events,
                };
                (Some(delta), effects)
            }
            AgentToCentral::CommandAck {
                machine_id,
                command_id,
                accepted,
                message,
            } => {
                new_events.push(self.push_event(
                    now_mono_ms,
                    now_utc_s,
                    if accepted {
                        EventSeverity::Info
                    } else {
                        EventSeverity::Warning
                    },
                    &machine_id,
                    None,
                    format!("ACK Comando #{command_id}"),
                    message,
                ));
                self.seq += 1;
                let delta = PanelDelta {
                    seq: self.seq,
                    central_mono_ms: now_mono_ms,
                    central_utc_s: now_utc_s,
                    emergency_stop_global: self.emergency_stop_global,
                    updated_machines: Vec::new(),
                    updated_characters: Vec::new(),
                    spot_directory: None,
                    new_events,
                };
                (Some(delta), effects)
            }
            AgentToCentral::Heartbeat {
                machine_id,
                sent_at_mono_ms,
            } => {
                if let Some(m) = self.machines.get_mut(&machine_id) {
                    m.last_report_mono_ms = sent_at_mono_ms.max(now_mono_ms);
                }
                (None, effects)
            }
        }
    }

    pub fn handle_operator_command(
        &mut self,
        cmd: &Command,
        now_mono_ms: MonotonicMs,
        now_utc_s: UtcTimestampSecs,
    ) -> PanelEventLog {
        match cmd {
            Command::TriggerEmergencyStop { machine_id: None } => {
                self.emergency_stop_global = true;
            }
            Command::ClearEmergencyStop { machine_id: None } => {
                self.emergency_stop_global = false;
            }
            Command::SetCatalogSpotEnabled { spot, enabled } => {
                for entry in &mut self.spot_directory.catalog {
                    if entry.spot.is_near(*spot, ARRIVAL_RADIUS) {
                        entry.enabled = *enabled;
                    }
                }
            }
            _ => {}
        }
        self.seq += 1;
        let sev = if matches!(cmd, Command::TriggerEmergencyStop { .. }) {
            EventSeverity::Critical
        } else {
            EventSeverity::Info
        };
        self.push_event(
            now_mono_ms,
            now_utc_s,
            sev,
            "PAINEL-DESKTOP",
            None,
            "Comando do Operador",
            cmd.summary_pt(),
        )
    }

    fn build_machine_view(
        &self,
        machine_id: &str,
        now_mono_ms: MonotonicMs,
    ) -> Option<PanelMachineView> {
        let m = self.machines.get(machine_id)?;
        let stale_threshold_ms = self.alert_config.stale_after_s * 1000;
        let is_stale =
            !m.connected || now_mono_ms.saturating_sub(m.last_report_mono_ms) > stale_threshold_ms;
        Some(PanelMachineView {
            machine_id: m.machine_id.clone(),
            agent_version: m.agent_version.clone(),
            connected: m.connected,
            is_simulator: m.is_simulator,
            is_stale,
            last_report_mono_ms: m.last_report_mono_ms,
            last_report_utc_s: m.last_report_utc_s,
            health: m.health.clone(),
        })
    }

    fn build_characters_for_machine(
        &self,
        machine_id: &str,
        now_mono_ms: MonotonicMs,
    ) -> Vec<PanelCharacterView> {
        let Some(m) = self.machines.get(machine_id) else {
            return Vec::new();
        };
        let now_mono_s = now_mono_ms / 1000;
        let stale_threshold_ms = self.alert_config.stale_after_s * 1000;

        m.chars
            .iter()
            .map(|(cname, rec)| {
                let rep = &rec.last_report;
                let report_age_ms = now_mono_ms.saturating_sub(rec.last_report_mono_ms);
                let rate = rec.tracker.per_hour(now_mono_s);
                let idle_secs = rec.tracker.idle_secs(now_mono_s);
                let zen_val = rep.zen.value;
                let eta_2b = zen_val.and_then(|z| eta_to_limit_secs(z, rate));

                let is_dead = rep.hp.value == Some(0);
                let is_paused = self.emergency_stop_global
                    || m.health.emergency_stop_active
                    || !m.health.automation_enabled
                    || !rep.char_automation_enabled;

                let at_active_spot = match (rep.position.value, rep.active_spot.or(rep.home_spot)) {
                    (Some(pos), Some(target)) => pos.is_near(target, ARRIVAL_RADIUS),
                    _ => false,
                };

                let state = classify_character_state(&StateEvalInput {
                    report_age_ms,
                    stale_threshold_ms,
                    logged_in: rep.title.is_logged_in(),
                    disconnected: rep.disconnected_confirmed,
                    is_dead,
                    is_paused,
                    is_recovering: rep.is_recovering,
                    helper_on: rep.helper_active.value,
                    at_active_spot,
                    zen_idle_secs: idle_secs,
                    stopped_threshold_s: self.alert_config.stopped_after_s,
                    at_zen_limit: zen_val.is_some_and(|z| z >= self.alert_config.limit_threshold),
                });

                let alerts = evaluate_alerts(
                    &AlertEvalInput {
                        last_report_mono_s: Some(rec.last_report_mono_ms / 1000),
                        disconnected: rep.disconnected_confirmed,
                        is_paused,
                        zen: zen_val,
                        zen_per_hour: rate,
                        idle_secs,
                        is_dead,
                    },
                    now_mono_s,
                    &self.alert_config,
                );

                let status_detail = if state == CharLifecycleState::Stale {
                    format!("Sem atualização do agente há {} s", report_age_ms / 1000)
                } else if let Some(ms) = &rep.manual_search {
                    ms.status_message.clone()
                } else if let Some(detail) = &rep.recovery_detail {
                    detail.clone()
                } else {
                    match state {
                        CharLifecycleState::Farming => format!(
                            "Farmando em {} — Helper ON",
                            rep.position
                                .value
                                .map(|p| p.label())
                                .unwrap_or_else(|| "Spot".into())
                        ),
                        CharLifecycleState::Stopped => {
                            if rep.helper_active.value == Some(false) {
                                "Helper DESLIGADO no spot".into()
                            } else {
                                format!("Sem ganho de Zen há {} s", idle_secs.unwrap_or(0))
                            }
                        }
                        CharLifecycleState::Dead => "HP chegou a 0 — aguardando retorno".into(),
                        CharLifecycleState::Disconnected => {
                            "Conexão TCP caiu e contadores pararam".into()
                        }
                        CharLifecycleState::Paused => "Automação pausada pelo operador / F8".into(),
                        _ => state.label_pt().to_string(),
                    }
                };

                let mut zen_reading = rep.zen.clone();
                zen_reading.confidence =
                    zen_reading.effective_confidence(now_mono_ms, stale_threshold_ms);

                let mut hp_reading = rep.hp.clone();
                hp_reading.confidence =
                    hp_reading.effective_confidence(now_mono_ms, stale_threshold_ms);

                let mut pos_reading = rep.position.clone();
                pos_reading.confidence =
                    pos_reading.effective_confidence(now_mono_ms, stale_threshold_ms);

                PanelCharacterView {
                    machine_id: m.machine_id.clone(),
                    pid: rep.pid,
                    name: cname.clone(),
                    level: rep.title.level,
                    resets: rep.title.resets,
                    state,
                    status_detail,
                    zen: zen_reading,
                    zen_per_hour: rate,
                    zen_idle_secs: idle_secs,
                    eta_limit_2b_secs: eta_2b,
                    zen_sparkline: rec.sparkline.iter().copied().collect(),
                    hp: hp_reading,
                    max_hp: rep.max_hp,
                    pet_hp_pct: rep.pet_hp_pct.clone(),
                    position: pos_reading,
                    helper_active: rep.helper_active.clone(),
                    nearby_entities: rep.nearby_entities.clone(),
                    home_spot: rep.home_spot,
                    active_spot: rep.active_spot,
                    fallback_policy: rep.fallback_policy.clone(),
                    manual_search: rep.manual_search.clone(),
                    planned_route: rep.planned_route.clone(),
                    alerts,
                    automation_enabled: rep.char_automation_enabled && m.health.automation_enabled,
                    last_report_mono_ms: rec.last_report_mono_ms,
                    last_death_utc_s: rec.last_death_utc_s,
                }
            })
            .collect()
    }

    /// Monta um `PanelSnapshot` completo com o estado atual de todos os PCs e personagens.
    pub fn snapshot(&self, now_mono_ms: MonotonicMs, now_utc_s: UtcTimestampSecs) -> PanelSnapshot {
        let machines: Vec<PanelMachineView> = self
            .machines
            .keys()
            .filter_map(|id| self.build_machine_view(id, now_mono_ms))
            .collect();
        let characters: Vec<PanelCharacterView> = self
            .machines
            .keys()
            .flat_map(|id| self.build_characters_for_machine(id, now_mono_ms))
            .collect();

        PanelSnapshot {
            protocol_version: PROTOCOL_VERSION,
            seq: self.seq,
            simulation_mode: self.simulation_mode,
            central_mono_ms: now_mono_ms,
            central_utc_s: now_utc_s,
            emergency_stop_global: self.emergency_stop_global,
            machines,
            characters,
            spot_directory: self.spot_directory.clone(),
            recent_events: self.events.iter().cloned().collect(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use zengravity_simulator::SimulatorEngine;

    #[test]
    fn central_hub_processes_simulator_reports_and_detects_stale_machines() {
        let mut sim = SimulatorEngine::new(20261118);
        let mut hub = CentralHub::new(true, AlertConfig::default());

        for hello in sim.initial_hellos() {
            hub.apply_agent_message(hello, 1_000, 1_795_000_001);
        }

        let reports = sim.step(2_000, 1_795_000_002);
        for rep in reports {
            let (delta, _) = hub.apply_agent_message(rep, 2_000, 1_795_000_002);
            assert!(delta.is_some());
        }

        let snap = hub.snapshot(3_000, 1_795_000_003);
        assert_eq!(snap.machines.len(), 4);
        assert_eq!(snap.characters.len(), 9);
        assert!(snap.machines.iter().all(|m| !m.is_stale));

        // Se o tempo avançar 25 segundos sem novos relatórios (> stale_after_s=15s),
        // todos os personagens e PCs passam para o estado STALE verdadeiro.
        let stale_snap = hub.snapshot(28_000, 1_795_000_028);
        assert!(stale_snap.machines.iter().all(|m| m.is_stale));
        assert!(stale_snap
            .characters
            .iter()
            .all(|c| c.state == CharLifecycleState::Stale));
    }
}
