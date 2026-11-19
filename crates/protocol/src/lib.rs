//! Contratos tipados e versionados do ZenGravity (`zengravity-protocol`).
//!
//! Define as mensagens trocadas entre:
//! - Agentes / Simulador -> Central (`AgentToCentral`, `CentralToAgent`)
//! - Central <-> Painel Desktop Nativo (`CentralToPanel`, `PanelToCentral`, `Command`)
//!
//! Suporta números de sequência (`seq`), Snapshots completos para conexão/reconexão,
//! Deltas incrementais, Heartbeats e Lista Fechada de Comandos com ACK.

use serde::{Deserialize, Serialize};
use zengravity_core::alerts::AlertKind;
use zengravity_core::chat::ChatEvent;
use zengravity_core::clock::{MonotonicMs, UtcTimestampSecs};
use zengravity_core::entities::NearbyEntity;
use zengravity_core::game_title::TitleInfo;
use zengravity_core::lifecycle::{CharLifecycleState, MetricReading};
use zengravity_core::map::WorldId;
use zengravity_core::spots::{FallbackPolicy, ManualSpotSearchState, SpotCoord, SpotDirectory};

pub const PROTOCOL_VERSION: u32 = 1;
pub const PROTOCOL_MIN: u32 = 1;

pub fn is_version_compatible(v: u32) -> bool {
    (PROTOCOL_MIN..=PROTOCOL_VERSION).contains(&v)
}

/// Saúde e telemetria do Agente em cada PC.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MachineHealth {
    pub cpu_percent: f32,
    pub ram_mb: f32,
    pub uptime_s: u64,
    pub os_label: String,
    pub automation_enabled: bool,
    pub emergency_stop_active: bool,
    pub active_input_owner: Option<String>,
    pub last_error: Option<String>,
}

impl Default for MachineHealth {
    fn default() -> Self {
        Self {
            cpu_percent: 0.4,
            ram_mb: 18.0,
            uptime_s: 0,
            os_label: "Windows 11 Pro 64-bit".into(),
            automation_enabled: true,
            emergency_stop_active: false,
            active_input_owner: None,
            last_error: None,
        }
    }
}

/// Relatório enviado pelo Agente (ou Simulador) sobre um personagem.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AgentCharReport {
    pub pid: u32,
    pub title: TitleInfo,
    pub zen: MetricReading<u64>,
    pub hp: MetricReading<u32>,
    pub max_hp: u32,
    pub pet_hp_pct: MetricReading<u8>,
    pub position: MetricReading<SpotCoord>,
    pub helper_active: MetricReading<bool>,
    pub nearby_entities: MetricReading<Vec<NearbyEntity>>,
    pub tcp_connected: bool,
    pub disconnected_confirmed: bool,
    pub is_recovering: bool,
    pub recovery_detail: Option<String>,
    pub char_automation_enabled: bool,
    pub home_spot: Option<SpotCoord>,
    pub active_spot: Option<SpotCoord>,
    pub fallback_policy: FallbackPolicy,
    pub manual_search: Option<ManualSpotSearchState>,
    pub planned_route: Vec<(i32, i32)>,
}

/// Mensagens enviadas do Agente (ou Simulador) para a Central.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum AgentToCentral {
    Hello {
        protocol_version: u32,
        machine_id: String,
        agent_version: String,
        is_simulator: bool,
    },
    Report {
        machine_id: String,
        seq: u64,
        sent_at_mono_ms: MonotonicMs,
        sent_at_utc_s: UtcTimestampSecs,
        health: MachineHealth,
        chars: Vec<AgentCharReport>,
        events: Vec<ChatEvent>,
        catalog_spots: Vec<SpotCoord>,
    },
    CommandAck {
        machine_id: String,
        command_id: u64,
        accepted: bool,
        message: String,
    },
    Heartbeat {
        machine_id: String,
        sent_at_mono_ms: MonotonicMs,
    },
}

/// Lista fechada de comandos operacionais autorizados (sem shell arbitrário).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "cmd", rename_all = "snake_case")]
pub enum Command {
    /// Aciona parada de emergência imediata em todos os PCs ou em um PC específico.
    TriggerEmergencyStop { machine_id: Option<String> },
    /// Libera a parada de emergência e retoma operação supervisionada.
    ClearEmergencyStop { machine_id: Option<String> },
    /// Pausa ou retoma a automação de um PC inteiro.
    SetMachineAutomation { machine_id: String, enabled: bool },
    /// Pausa ou retoma a automação de um personagem específico.
    SetCharAutomation {
        machine_id: String,
        char_name: String,
        enabled: bool,
    },
    /// Inicia busca manual imediata ("Procurar spot agora") no mapa escolhido.
    StartManualSpotSearch {
        machine_id: String,
        char_name: String,
        target_world: WorldId,
    },
    /// Cancela busca manual de spot em andamento.
    CancelManualSpotSearch {
        machine_id: String,
        char_name: String,
    },
    /// Ordena retorno imediato ao spot principal (F1).
    ReturnToHomeSpot {
        machine_id: String,
        char_name: String,
    },
    /// Define ou altera o spot principal (F1) do personagem.
    AssignHomeSpot {
        machine_id: String,
        char_name: String,
        spot: SpotCoord,
    },
    /// Atualiza política de retorno/fallback do personagem.
    UpdateFallbackPolicy {
        machine_id: String,
        char_name: String,
        policy: FallbackPolicy,
    },
    /// Habilita ou desabilita um spot no catálogo compartilhado.
    SetCatalogSpotEnabled { spot: SpotCoord, enabled: bool },
    /// Solicita relock de leitura de RAM (Zen/HP) do personagem.
    RelockTelemetry {
        machine_id: String,
        char_name: String,
    },
}

impl Command {
    pub fn summary_pt(&self) -> String {
        match self {
            Command::TriggerEmergencyStop { machine_id: None } => {
                "PARADA DE EMERGÊNCIA GLOBAL acionada".into()
            }
            Command::TriggerEmergencyStop {
                machine_id: Some(m),
            } => format!("Parada de emergência acionada em {m}"),
            Command::ClearEmergencyStop { .. } => "Parada de emergência liberada".into(),
            Command::SetMachineAutomation {
                machine_id,
                enabled,
            } => format!(
                "Automação do PC {machine_id}: {}",
                if *enabled { "LIGADA" } else { "PAUSADA" }
            ),
            Command::SetCharAutomation {
                char_name, enabled, ..
            } => format!(
                "Automação de {char_name}: {}",
                if *enabled { "LIGADA" } else { "PAUSADA" }
            ),
            Command::StartManualSpotSearch {
                char_name,
                target_world,
                ..
            } => format!(
                "Procurar spot agora para {char_name} em {}",
                target_world.name()
            ),
            Command::CancelManualSpotSearch { char_name, .. } => {
                format!("Busca manual cancelada para {char_name}")
            }
            Command::ReturnToHomeSpot { char_name, .. } => {
                format!("Retorno ao spot F1 solicitado para {char_name}")
            }
            Command::AssignHomeSpot {
                char_name, spot, ..
            } => format!("Spot F1 de {char_name} definido para {}", spot.label()),
            Command::UpdateFallbackPolicy { char_name, .. } => {
                format!("Política de fallback atualizada para {char_name}")
            }
            Command::SetCatalogSpotEnabled { spot, enabled } => format!(
                "Spot do catálogo {}: {}",
                spot.label(),
                if *enabled { "Ativado" } else { "Desativado" }
            ),
            Command::RelockTelemetry { char_name, .. } => {
                format!("Recalibração de leitura solicitada para {char_name}")
            }
        }
    }
}

/// Visão consolidada de um personagem calculada pela Central para exibição no Painel Desktop.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PanelCharacterView {
    pub machine_id: String,
    pub pid: u32,
    pub name: String,
    pub level: Option<u32>,
    pub resets: Option<u32>,
    pub state: CharLifecycleState,
    pub status_detail: String,
    pub zen: MetricReading<u64>,
    pub zen_per_hour: f64,
    pub zen_idle_secs: Option<u64>,
    pub eta_limit_2b_secs: Option<u64>,
    /// Série temporal recente de Zen: `(timestamp_mono_s, zen)`
    pub zen_sparkline: Vec<(u64, u64)>,
    pub hp: MetricReading<u32>,
    pub max_hp: u32,
    pub pet_hp_pct: MetricReading<u8>,
    pub position: MetricReading<SpotCoord>,
    pub helper_active: MetricReading<bool>,
    pub nearby_entities: MetricReading<Vec<NearbyEntity>>,
    pub home_spot: Option<SpotCoord>,
    pub active_spot: Option<SpotCoord>,
    pub fallback_policy: FallbackPolicy,
    pub manual_search: Option<ManualSpotSearchState>,
    pub planned_route: Vec<(i32, i32)>,
    pub alerts: Vec<AlertKind>,
    pub automation_enabled: bool,
    pub last_report_mono_ms: MonotonicMs,
    pub last_death_utc_s: Option<UtcTimestampSecs>,
}

/// Visão consolidada de um PC (Agente) para o Painel Desktop.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PanelMachineView {
    pub machine_id: String,
    pub agent_version: String,
    pub connected: bool,
    pub is_simulator: bool,
    pub is_stale: bool,
    pub last_report_mono_ms: MonotonicMs,
    pub last_report_utc_s: UtcTimestampSecs,
    pub health: MachineHealth,
}

/// Evento estruturado exibido na seção de Eventos e Auditoria do Desktop.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EventSeverity {
    Info,
    Warning,
    Critical,
    Golden,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PanelEventLog {
    pub id: u64,
    pub timestamp_utc_s: UtcTimestampSecs,
    pub timestamp_mono_ms: MonotonicMs,
    pub severity: EventSeverity,
    pub machine_id: String,
    pub char_name: Option<String>,
    pub title: String,
    pub detail: String,
}

/// Snapshot completo do estado da Central (enviado na conexão inicial ou ressincronização).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PanelSnapshot {
    pub protocol_version: u32,
    pub seq: u64,
    pub simulation_mode: bool,
    pub central_mono_ms: MonotonicMs,
    pub central_utc_s: UtcTimestampSecs,
    pub emergency_stop_global: bool,
    pub machines: Vec<PanelMachineView>,
    pub characters: Vec<PanelCharacterView>,
    pub spot_directory: SpotDirectory,
    pub recent_events: Vec<PanelEventLog>,
}

/// Delta incremental enviado pela Central ao Painel Desktop a cada ciclo/evento.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PanelDelta {
    pub seq: u64,
    pub central_mono_ms: MonotonicMs,
    pub central_utc_s: UtcTimestampSecs,
    pub emergency_stop_global: bool,
    pub updated_machines: Vec<PanelMachineView>,
    pub updated_characters: Vec<PanelCharacterView>,
    pub spot_directory: Option<SpotDirectory>,
    pub new_events: Vec<PanelEventLog>,
}

/// Mensagens da Central para o Painel Desktop.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum CentralToPanel {
    Snapshot(Box<PanelSnapshot>),
    Delta(Box<PanelDelta>),
    CommandAck {
        command_id: u64,
        accepted: bool,
        message: String,
    },
    Heartbeat {
        seq: u64,
        central_mono_ms: MonotonicMs,
        central_utc_s: UtcTimestampSecs,
    },
}

/// Mensagens do Painel Desktop para a Central.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum PanelToCentral {
    RequestSnapshot,
    IssueCommand { command_id: u64, command: Command },
    Ping { client_mono_ms: MonotonicMs },
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn command_serialization_roundtrip_and_version_check() {
        assert!(is_version_compatible(PROTOCOL_VERSION));
        assert!(!is_version_compatible(0));

        let cmd = Command::StartManualSpotSearch {
            machine_id: "PC1-PRINCIPAL".into(),
            char_name: "x9901".into(),
            target_world: WorldId::Kanturu,
        };
        let json = serde_json::to_string(&cmd).unwrap();
        let decoded: Command = serde_json::from_str(&json).unwrap();
        assert_eq!(cmd, decoded);
        assert!(cmd.summary_pt().contains("Kanturu"));
    }
}
