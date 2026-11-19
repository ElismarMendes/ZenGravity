//! Simulador reproduzível multi-PC e multicontas (`zengravity-simulator`).
//!
//! Emite mensagens reais `AgentToCentral` pelo protocolo oficial, simulando:
//! - 4 PCs (`PC1-PRINCIPAL`, `PC2-FARM-NORTE`, `PC3-ARENA-SUL`, `ELISMAR-NOTE`)
//! - 9 personagens com perfis variados:
//!   - `x9901` (Kanturu, farmando ativo, Zen subindo rápido)
//!   - `ReDMooN` (Kanturu, perto do limite de 2 bilhões — `QUASE 2 BI`)
//!   - `AMUMU` (Kanturu, recuperando e caminhando por rota A* até o spot)
//!   - `LOBOLIXO` (Kanturu, sofre ataque/morte de Genocider e recupera)
//!   - `CSRF` (Lorencia, patrulhando/observando NPCs e Golden Budge Dragon)
//!   - `0MAGIC` (Arena/Stadium, farmando no spot)
//!   - `xWizard` (Arena/Stadium, farmando a >12 tiles de distância de 0MAGIC)
//!   - `YouSee` (Dungeon, Dark Lord com Dark Raven e barra de Pet HP)
//!   - `FirstHand` (Lost Tower, simula queda de conexão / DC periódico ou leitura pausada)
//! - Permite injetar falhas sob demanda pelo painel: atraso/congelamento (`stale`),
//!   morte simulada, desconexão (DC), aparição de Golden Monster e comandos do operador.

use std::collections::BTreeSet;
use zengravity_core::chat::{ChatEvent, JewelKind, KillerKind};
use zengravity_core::clock::{MonotonicMs, UtcTimestampSecs};
use zengravity_core::entities::{EntityKind, NearbyEntity};
use zengravity_core::game_title::TitleInfo;
use zengravity_core::lifecycle::{MetricReading, MetricSource};
use zengravity_core::map::{TerrainGrid, WorldId};
use zengravity_core::spots::{FallbackPolicy, ManualSpotSearchState, SpotCoord};
use zengravity_core::ZEN_LIMIT;
use zengravity_protocol::{
    AgentCharReport, AgentToCentral, Command, MachineHealth, PROTOCOL_VERSION,
};

#[derive(Debug, Clone)]
pub struct SimCharacter {
    pub machine_id: String,
    pub pid: u32,
    pub name: String,
    pub level: u32,
    pub resets: u32,
    pub zen: u64,
    pub zen_gain_per_tick: u64,
    pub hp: u32,
    pub max_hp: u32,
    pub pet_hp_pct: Option<u8>,
    pub pos: SpotCoord,
    pub home_spot: Option<SpotCoord>,
    pub active_spot: Option<SpotCoord>,
    pub helper_on: bool,
    pub tcp_connected: bool,
    pub disconnected: bool,
    pub is_recovering: bool,
    pub recovery_detail: Option<String>,
    pub automation_enabled: bool,
    pub fallback_policy: FallbackPolicy,
    pub manual_search: Option<ManualSpotSearchState>,
    pub route: Vec<(i32, i32)>,
    pub dead_ticks_remaining: u32,
}

#[derive(Debug, Clone)]
pub struct SimMachine {
    pub machine_id: String,
    pub os_label: String,
    pub cpu_base: f32,
    pub ram_mb: f32,
    pub seq: u64,
    pub automation_enabled: bool,
    pub emergency_stop: bool,
    /// Se `true`, este PC para de enviar relatórios para testar detecção de `STALE` na Central/Desktop.
    pub silenced_for_stale_test: bool,
    pub catalog_spots: Vec<SpotCoord>,
}

/// Motor determinístico do simulador.
#[derive(Debug, Clone)]
pub struct SimulatorEngine {
    seed_state: u64,
    pub tick_count: u64,
    pub machines: Vec<SimMachine>,
    pub characters: Vec<SimCharacter>,
    silenced_machines: BTreeSet<String>,
}

impl SimulatorEngine {
    pub fn new(seed: u64) -> Self {
        let machines = vec![
            SimMachine {
                machine_id: "PC1-PRINCIPAL".into(),
                os_label: "Windows 11 Pro 23H2".into(),
                cpu_base: 0.6,
                ram_mb: 22.4,
                seq: 0,
                automation_enabled: true,
                emergency_stop: false,
                silenced_for_stale_test: false,
                catalog_spots: vec![
                    SpotCoord::new(WorldId::Kanturu, 123, 233),
                    SpotCoord::new(WorldId::Kanturu, 40, 232),
                    SpotCoord::new(WorldId::Kanturu, 90, 66),
                    SpotCoord::new(WorldId::Kanturu, 160, 197),
                ],
            },
            SimMachine {
                machine_id: "PC2-FARM-NORTE".into(),
                os_label: "Windows 10 Pro 22H2".into(),
                cpu_base: 0.5,
                ram_mb: 19.8,
                seq: 0,
                automation_enabled: true,
                emergency_stop: false,
                silenced_for_stale_test: false,
                catalog_spots: vec![
                    SpotCoord::new(WorldId::Kanturu, 211, 38),
                    SpotCoord::new(WorldId::Kanturu, 84, 236),
                    SpotCoord::new(WorldId::Lorencia, 138, 128),
                ],
            },
            SimMachine {
                machine_id: "PC3-ARENA-SUL".into(),
                os_label: "Windows 11 Pro 23H2".into(),
                cpu_base: 0.4,
                ram_mb: 17.5,
                seq: 0,
                automation_enabled: true,
                emergency_stop: false,
                silenced_for_stale_test: false,
                catalog_spots: vec![
                    SpotCoord::new(WorldId::Arena, 47, 70),
                    SpotCoord::new(WorldId::Arena, 31, 57),
                    SpotCoord::new(WorldId::Arena, 62, 85),
                ],
            },
            SimMachine {
                machine_id: "ELISMAR-NOTE".into(),
                os_label: "Windows 11 Home 23H2".into(),
                cpu_base: 0.9,
                ram_mb: 24.1,
                seq: 0,
                automation_enabled: true,
                emergency_stop: false,
                silenced_for_stale_test: false,
                catalog_spots: vec![
                    SpotCoord::new(WorldId::Dungeon, 9, 59),
                    SpotCoord::new(WorldId::Dungeon, 28, 84),
                    SpotCoord::new(WorldId::LostTower, 45, 112),
                ],
            },
        ];

        let kanturu_grid = TerrainGrid::synthetic_for(WorldId::Kanturu);
        let amumu_route = kanturu_grid
            .find_path((34, 217), (90, 66))
            .unwrap_or_else(|| vec![(40, 200), (65, 130), (90, 66)]);

        let characters = vec![
            // PC1-PRINCIPAL
            SimCharacter {
                machine_id: "PC1-PRINCIPAL".into(),
                pid: 14120,
                name: "x9901".into(),
                level: 400,
                resets: 1026,
                zen: 842_350_000,
                zen_gain_per_tick: 420_000,
                hp: 6850,
                max_hp: 6850,
                pet_hp_pct: None,
                pos: SpotCoord::new(WorldId::Kanturu, 123, 233),
                home_spot: Some(SpotCoord::new(WorldId::Kanturu, 123, 233)),
                active_spot: Some(SpotCoord::new(WorldId::Kanturu, 123, 233)),
                helper_on: true,
                tcp_connected: true,
                disconnected: false,
                is_recovering: false,
                recovery_detail: None,
                automation_enabled: true,
                fallback_policy: FallbackPolicy::default(),
                manual_search: None,
                route: Vec::new(),
                dead_ticks_remaining: 0,
            },
            SimCharacter {
                machine_id: "PC1-PRINCIPAL".into(),
                pid: 14892,
                name: "ReDMooN".into(),
                level: 400,
                resets: 915,
                zen: 1_962_400_000, // Perto de 2B -> aciona Near2B
                zen_gain_per_tick: 510_000,
                hp: 6686,
                max_hp: 6686,
                pet_hp_pct: None,
                pos: SpotCoord::new(WorldId::Kanturu, 40, 232),
                home_spot: Some(SpotCoord::new(WorldId::Kanturu, 40, 232)),
                active_spot: Some(SpotCoord::new(WorldId::Kanturu, 40, 232)),
                helper_on: true,
                tcp_connected: true,
                disconnected: false,
                is_recovering: false,
                recovery_detail: None,
                automation_enabled: true,
                fallback_policy: FallbackPolicy::default(),
                manual_search: None,
                route: Vec::new(),
                dead_ticks_remaining: 0,
            },
            SimCharacter {
                machine_id: "PC1-PRINCIPAL".into(),
                pid: 15204,
                name: "AMUMU".into(),
                level: 400,
                resets: 840,
                zen: 415_900_000,
                zen_gain_per_tick: 310_000,
                hp: 5900,
                max_hp: 5900,
                pet_hp_pct: None,
                pos: SpotCoord::new(WorldId::Kanturu, 34, 217),
                home_spot: Some(SpotCoord::new(WorldId::Kanturu, 90, 66)),
                active_spot: Some(SpotCoord::new(WorldId::Kanturu, 90, 66)),
                helper_on: false,
                tcp_connected: true,
                disconnected: false,
                is_recovering: true,
                recovery_detail: Some("Caminhando por rota A* até Kanturu (90, 66)".into()),
                automation_enabled: true,
                fallback_policy: FallbackPolicy::default(),
                manual_search: None,
                route: amumu_route,
                dead_ticks_remaining: 0,
            },
            // PC2-FARM-NORTE
            SimCharacter {
                machine_id: "PC2-FARM-NORTE".into(),
                pid: 19944,
                name: "LOBOLIXO".into(),
                level: 400,
                resets: 780,
                zen: 612_100_000,
                zen_gain_per_tick: 380_000,
                hp: 5120,
                max_hp: 5400,
                pet_hp_pct: None,
                pos: SpotCoord::new(WorldId::Kanturu, 211, 38),
                home_spot: Some(SpotCoord::new(WorldId::Kanturu, 211, 38)),
                active_spot: Some(SpotCoord::new(WorldId::Kanturu, 211, 38)),
                helper_on: true,
                tcp_connected: true,
                disconnected: false,
                is_recovering: false,
                recovery_detail: None,
                automation_enabled: true,
                fallback_policy: FallbackPolicy::default(),
                manual_search: None,
                route: Vec::new(),
                dead_ticks_remaining: 0,
            },
            SimCharacter {
                machine_id: "PC2-FARM-NORTE".into(),
                pid: 20412,
                name: "CSRF".into(),
                level: 400,
                resets: 890,
                zen: 1_120_800_000,
                zen_gain_per_tick: 190_000,
                hp: 6200,
                max_hp: 6200,
                pet_hp_pct: None,
                pos: SpotCoord::new(WorldId::Lorencia, 188, 132),
                home_spot: Some(SpotCoord::new(WorldId::Kanturu, 84, 236)),
                active_spot: Some(SpotCoord::new(WorldId::Lorencia, 188, 132)),
                helper_on: true,
                tcp_connected: true,
                disconnected: false,
                is_recovering: false,
                recovery_detail: Some("Rastreando Golden Budge Dragon em Lorencia".into()),
                automation_enabled: true,
                fallback_policy: FallbackPolicy::default(),
                manual_search: None,
                route: vec![(192, 133), (197, 133)],
                dead_ticks_remaining: 0,
            },
            // PC3-ARENA-SUL
            SimCharacter {
                machine_id: "PC3-ARENA-SUL".into(),
                pid: 8812,
                name: "0MAGIC".into(),
                level: 400,
                resets: 905,
                zen: 1_240_000_000,
                zen_gain_per_tick: 450_000,
                hp: 6400,
                max_hp: 6400,
                pet_hp_pct: None,
                pos: SpotCoord::new(WorldId::Arena, 47, 70),
                home_spot: Some(SpotCoord::new(WorldId::Arena, 47, 70)),
                active_spot: Some(SpotCoord::new(WorldId::Arena, 47, 70)),
                helper_on: true,
                tcp_connected: true,
                disconnected: false,
                is_recovering: false,
                recovery_detail: None,
                automation_enabled: true,
                fallback_policy: FallbackPolicy::default(),
                manual_search: None,
                route: Vec::new(),
                dead_ticks_remaining: 0,
            },
            SimCharacter {
                machine_id: "PC3-ARENA-SUL".into(),
                pid: 9104,
                name: "xWizard".into(),
                level: 400,
                resets: 900,
                zen: 985_400_000,
                zen_gain_per_tick: 430_000,
                hp: 6150,
                max_hp: 6150,
                pet_hp_pct: None,
                pos: SpotCoord::new(WorldId::Arena, 31, 57),
                home_spot: Some(SpotCoord::new(WorldId::Arena, 31, 57)),
                active_spot: Some(SpotCoord::new(WorldId::Arena, 31, 57)),
                helper_on: true,
                tcp_connected: true,
                disconnected: false,
                is_recovering: false,
                recovery_detail: None,
                automation_enabled: true,
                fallback_policy: FallbackPolicy::default(),
                manual_search: None,
                route: Vec::new(),
                dead_ticks_remaining: 0,
            },
            // ELISMAR-NOTE
            SimCharacter {
                machine_id: "ELISMAR-NOTE".into(),
                pid: 11240,
                name: "YouSee".into(),
                level: 400,
                resets: 870,
                zen: 730_500_000,
                zen_gain_per_tick: 360_000,
                hp: 5800,
                max_hp: 5800,
                pet_hp_pct: Some(74),
                pos: SpotCoord::new(WorldId::Dungeon, 9, 59),
                home_spot: Some(SpotCoord::new(WorldId::Dungeon, 9, 59)),
                active_spot: Some(SpotCoord::new(WorldId::Dungeon, 9, 59)),
                helper_on: true,
                tcp_connected: true,
                disconnected: false,
                is_recovering: false,
                recovery_detail: None,
                automation_enabled: true,
                fallback_policy: FallbackPolicy::default(),
                manual_search: None,
                route: Vec::new(),
                dead_ticks_remaining: 0,
            },
            SimCharacter {
                machine_id: "ELISMAR-NOTE".into(),
                pid: 11890,
                name: "FirstHand".into(),
                level: 392,
                resets: 640,
                zen: 310_200_000,
                zen_gain_per_tick: 0,
                hp: 4900,
                max_hp: 4900,
                pet_hp_pct: None,
                pos: SpotCoord::new(WorldId::LostTower, 45, 112),
                home_spot: Some(SpotCoord::new(WorldId::LostTower, 45, 112)),
                active_spot: Some(SpotCoord::new(WorldId::LostTower, 45, 112)),
                helper_on: false, // Parado (Helper desligado) para demonstrar estado PARADO
                tcp_connected: true,
                disconnected: false,
                is_recovering: false,
                recovery_detail: None,
                automation_enabled: true,
                fallback_policy: FallbackPolicy::default(),
                manual_search: None,
                route: Vec::new(),
                dead_ticks_remaining: 0,
            },
        ];

        Self {
            seed_state: seed.max(1),
            tick_count: 0,
            machines,
            characters,
            silenced_machines: BTreeSet::new(),
        }
    }

    fn next_rand(&mut self) -> u64 {
        // Xorshift64* determinístico para que a mesma semente produza a mesma sequência
        let mut x = self.seed_state;
        x ^= x >> 12;
        x ^= x << 25;
        x ^= x >> 27;
        self.seed_state = x;
        x.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }

    /// Mensagens iniciais `Hello` de todos os 4 PCs simulados.
    pub fn initial_hellos(&self) -> Vec<AgentToCentral> {
        self.machines
            .iter()
            .map(|m| AgentToCentral::Hello {
                protocol_version: PROTOCOL_VERSION,
                machine_id: m.machine_id.clone(),
                agent_version: "0.1.0-sim".into(),
                is_simulator: true,
            })
            .collect()
    }

    /// Alterna o silenciamento de um PC (ou de todos) para testar estado `STALE` na Central e no Desktop.
    pub fn set_machine_silenced(&mut self, machine_id: &str, silenced: bool) {
        if silenced {
            self.silenced_machines.insert(machine_id.to_string());
        } else {
            self.silenced_machines.remove(machine_id);
        }
        for m in &mut self.machines {
            if m.machine_id == machine_id {
                m.silenced_for_stale_test = silenced;
            }
        }
    }

    pub fn is_machine_silenced(&self, machine_id: &str) -> bool {
        self.silenced_machines.contains(machine_id)
    }

    /// Simula uma morte imediata em um personagem para verificação visual de alerta e recuperação.
    pub fn inject_death(&mut self, char_name: &str) {
        if let Some(c) = self
            .characters
            .iter_mut()
            .find(|c| c.name.eq_ignore_ascii_case(char_name))
        {
            c.hp = 0;
            c.helper_on = false;
            c.dead_ticks_remaining = 4;
            c.is_recovering = false;
        }
    }

    /// Simula desconexão TCP (DC) ou reconexão em um personagem.
    pub fn toggle_disconnect(&mut self, char_name: &str) {
        if let Some(c) = self
            .characters
            .iter_mut()
            .find(|c| c.name.eq_ignore_ascii_case(char_name))
        {
            c.disconnected = !c.disconnected;
            c.tcp_connected = !c.disconnected;
            c.helper_on = !c.disconnected;
        }
    }

    /// Aplica um comando enviado pelo Operador no Desktop sobre o estado do Simulador.
    pub fn apply_command(&mut self, cmd: &Command) -> (bool, String) {
        match cmd {
            Command::TriggerEmergencyStop { machine_id } => {
                for m in &mut self.machines {
                    if machine_id.as_ref().is_none_or(|id| id == &m.machine_id) {
                        m.emergency_stop = true;
                    }
                }
                (true, cmd.summary_pt())
            }
            Command::ClearEmergencyStop { machine_id } => {
                for m in &mut self.machines {
                    if machine_id.as_ref().is_none_or(|id| id == &m.machine_id) {
                        m.emergency_stop = false;
                    }
                }
                (true, cmd.summary_pt())
            }
            Command::SetMachineAutomation {
                machine_id,
                enabled,
            } => {
                for m in &mut self.machines {
                    if &m.machine_id == machine_id {
                        m.automation_enabled = *enabled;
                    }
                }
                (true, cmd.summary_pt())
            }
            Command::SetCharAutomation {
                char_name, enabled, ..
            } => {
                if let Some(c) = self
                    .characters
                    .iter_mut()
                    .find(|c| c.name.eq_ignore_ascii_case(char_name))
                {
                    c.automation_enabled = *enabled;
                    return (true, cmd.summary_pt());
                }
                (false, format!("Personagem {char_name} não encontrado"))
            }
            Command::StartManualSpotSearch {
                char_name,
                target_world,
                ..
            } => {
                let candidates: Vec<SpotCoord> = self
                    .machines
                    .iter()
                    .flat_map(|m| m.catalog_spots.iter().copied())
                    .filter(|s| s.world == *target_world)
                    .collect();
                if let Some(c) = self
                    .characters
                    .iter_mut()
                    .find(|c| c.name.eq_ignore_ascii_case(char_name))
                {
                    let first_target = candidates.first().copied().unwrap_or(SpotCoord::new(
                        *target_world,
                        50,
                        50,
                    ));
                    let grid = TerrainGrid::synthetic_for(*target_world);
                    c.pos = SpotCoord::new(*target_world, 34, 217);
                    c.route = grid
                        .find_path((c.pos.x, c.pos.y), (first_target.x, first_target.y))
                        .unwrap_or_else(|| vec![(first_target.x, first_target.y)]);
                    c.is_recovering = true;
                    c.recovery_detail = Some(format!(
                        "Busca manual em {}: indo verificar ({}, {})",
                        target_world.name(),
                        first_target.x,
                        first_target.y
                    ));
                    c.manual_search = Some(ManualSpotSearchState {
                        target_world: *target_world,
                        total_candidates: candidates.len().max(1) as u32,
                        examined: 0,
                        current_target: Some(first_target),
                        active: true,
                        status_message: format!(
                            "Procurando spots em {} (0/{} verificados)",
                            target_world.name(),
                            candidates.len().max(1)
                        ),
                    });
                    return (true, cmd.summary_pt());
                }
                (false, format!("Personagem {char_name} não encontrado"))
            }
            Command::CancelManualSpotSearch { char_name, .. } => {
                if let Some(c) = self
                    .characters
                    .iter_mut()
                    .find(|c| c.name.eq_ignore_ascii_case(char_name))
                {
                    c.manual_search = None;
                    c.is_recovering = false;
                    c.recovery_detail = None;
                    c.route.clear();
                    return (true, cmd.summary_pt());
                }
                (false, format!("Personagem {char_name} não encontrado"))
            }
            Command::ReturnToHomeSpot { char_name, .. } => {
                if let Some(c) = self
                    .characters
                    .iter_mut()
                    .find(|c| c.name.eq_ignore_ascii_case(char_name))
                {
                    if let Some(home) = c.home_spot {
                        let grid = TerrainGrid::synthetic_for(home.world);
                        c.pos = SpotCoord::new(home.world, 34, 217);
                        c.active_spot = Some(home);
                        c.route = grid
                            .find_path((c.pos.x, c.pos.y), (home.x, home.y))
                            .unwrap_or_else(|| vec![(home.x, home.y)]);
                        c.is_recovering = true;
                        c.helper_on = false;
                        c.recovery_detail = Some(format!("Retornando ao spot F1 {}", home.label()));
                        return (true, cmd.summary_pt());
                    }
                }
                (false, format!("Spot F1 não definido para {char_name}"))
            }
            Command::AssignHomeSpot {
                char_name, spot, ..
            } => {
                if let Some(c) = self
                    .characters
                    .iter_mut()
                    .find(|c| c.name.eq_ignore_ascii_case(char_name))
                {
                    c.home_spot = Some(*spot);
                    c.active_spot = Some(*spot);
                    return (true, cmd.summary_pt());
                }
                (false, format!("Personagem {char_name} não encontrado"))
            }
            Command::UpdateFallbackPolicy {
                char_name, policy, ..
            } => {
                if let Some(c) = self
                    .characters
                    .iter_mut()
                    .find(|c| c.name.eq_ignore_ascii_case(char_name))
                {
                    c.fallback_policy = policy.clone();
                    return (true, cmd.summary_pt());
                }
                (false, format!("Personagem {char_name} não encontrado"))
            }
            Command::SetCatalogSpotEnabled { .. } | Command::RelockTelemetry { .. } => {
                (true, cmd.summary_pt())
            }
        }
    }

    /// Avança um ciclo do simulador e produz os relatórios `AgentToCentral::Report` para cada PC não silenciado.
    pub fn step(
        &mut self,
        now_mono_ms: MonotonicMs,
        now_utc_s: UtcTimestampSecs,
    ) -> Vec<AgentToCentral> {
        self.tick_count += 1;
        let tick = self.tick_count;
        let rnd = self.next_rand();

        // Evolui cada personagem
        for idx in 0..self.characters.len() {
            let char_rnd = rnd.wrapping_add(idx as u64 * 7919);
            let c = &mut self.characters[idx];

            // Tratamento de morte temporária e renascimento
            if c.dead_ticks_remaining > 0 {
                c.dead_ticks_remaining -= 1;
                if c.dead_ticks_remaining == 0 {
                    c.hp = c.max_hp;
                    if let Some(home) = c.home_spot {
                        let grid = TerrainGrid::synthetic_for(home.world);
                        c.pos = SpotCoord::new(home.world, 34, 217);
                        c.route = grid
                            .find_path((c.pos.x, c.pos.y), (home.x, home.y))
                            .unwrap_or_else(|| vec![(home.x, home.y)]);
                        c.is_recovering = true;
                        c.recovery_detail =
                            Some(format!("Recuperando após morte rumo a {}", home.label()));
                    }
                }
                continue;
            }

            if c.disconnected {
                continue;
            }

            // Movimentação ao longo de rota A* quando em recuperação ou busca manual
            if !c.route.is_empty() {
                let steps_per_tick = 4.min(c.route.len());
                let next_pt = c.route[steps_per_tick - 1];
                c.route.drain(0..steps_per_tick);
                c.pos.x = next_pt.0;
                c.pos.y = next_pt.1;
                if c.route.is_empty() {
                    c.is_recovering = false;
                    c.helper_on = true;
                    c.recovery_detail = None;
                    if let Some(ms) = &mut c.manual_search {
                        ms.examined = ms.total_candidates;
                        ms.active = false;
                        ms.status_message =
                            format!("Vaga confirmada em ({}, {})", c.pos.x, c.pos.y);
                        c.active_spot = Some(c.pos);
                    }
                }
            }

            // Ganho de Zen e oscilação realista de HP quando Helper está ativo
            if c.helper_on && !c.is_recovering {
                let jitter = char_rnd % 90_000;
                c.zen = (c.zen + c.zen_gain_per_tick + jitter).min(ZEN_LIMIT);

                // Oscilação leve de HP em combate
                let dmg = ((char_rnd >> 8) % 240) as u32;
                c.hp = c.max_hp.saturating_sub(dmg);

                // Desgaste gradual do Dark Raven (YouSee)
                if let Some(pet) = &mut c.pet_hp_pct {
                    if tick % 9 == 0 && *pet > 18 {
                        *pet -= 1;
                    }
                }
            }
        }

        // Evento periódico a cada 18 ciclos (~36s): LOBOLIXO sofre PK/morte ou dropa joia
        let mut machine_events: std::collections::HashMap<String, Vec<ChatEvent>> =
            std::collections::HashMap::new();

        if tick == 8 {
            machine_events
                .entry("PC1-PRINCIPAL".into())
                .or_default()
                .push(ChatEvent::Pick {
                    char_name: "x9901".into(),
                    jewel: JewelKind::Soul,
                });
        } else if tick == 16 {
            self.inject_death("LOBOLIXO");
            machine_events
                .entry("PC2-FARM-NORTE".into())
                .or_default()
                .push(ChatEvent::Pk {
                    killer: "Genocider".into(),
                    killer_kind: KillerKind::Monster,
                    victim: "LOBOLIXO".into(),
                });
        } else if tick % 25 == 0 {
            machine_events
                .entry("PC2-FARM-NORTE".into())
                .or_default()
                .push(ChatEvent::ServerNotice {
                    text: "Golden Invasion in Lorencia detected!".into(),
                });
        }

        // Monta os relatórios de cada PC não silenciado
        let mut reports = Vec::new();
        for m in &mut self.machines {
            if m.silenced_for_stale_test {
                continue;
            }
            m.seq += 1;
            let chars_for_machine: Vec<AgentCharReport> = self
                .characters
                .iter()
                .filter(|c| c.machine_id == m.machine_id)
                .map(|c| {
                    let nearby = build_nearby_entities(c, tick, now_mono_ms);
                    AgentCharReport {
                        pid: c.pid,
                        title: TitleInfo {
                            name: Some(c.name.clone()),
                            level: Some(c.level),
                            resets: Some(c.resets),
                            wc: Some(0),
                            wp: Some(14_200),
                            gp_tenths: Some(85_000),
                        },
                        zen: MetricReading::verified(
                            c.zen,
                            MetricSource::Simulator,
                            now_mono_ms,
                            now_utc_s,
                        ),
                        hp: MetricReading::verified(
                            c.hp,
                            MetricSource::Simulator,
                            now_mono_ms,
                            now_utc_s,
                        ),
                        max_hp: c.max_hp,
                        pet_hp_pct: match c.pet_hp_pct {
                            Some(p) => MetricReading::verified(
                                p,
                                MetricSource::Simulator,
                                now_mono_ms,
                                now_utc_s,
                            ),
                            None => MetricReading::unavailable(now_mono_ms, now_utc_s),
                        },
                        position: MetricReading::verified(
                            c.pos,
                            MetricSource::Simulator,
                            now_mono_ms,
                            now_utc_s,
                        ),
                        helper_active: MetricReading::verified(
                            c.helper_on,
                            MetricSource::Simulator,
                            now_mono_ms,
                            now_utc_s,
                        ),
                        nearby_entities: MetricReading::verified(
                            nearby,
                            MetricSource::Simulator,
                            now_mono_ms,
                            now_utc_s,
                        ),
                        tcp_connected: c.tcp_connected,
                        disconnected_confirmed: c.disconnected,
                        is_recovering: c.is_recovering,
                        recovery_detail: c.recovery_detail.clone(),
                        char_automation_enabled: c.automation_enabled,
                        home_spot: c.home_spot,
                        active_spot: c.active_spot,
                        fallback_policy: c.fallback_policy.clone(),
                        manual_search: c.manual_search.clone(),
                        planned_route: c.route.clone(),
                    }
                })
                .collect();

            let active_owner = chars_for_machine
                .iter()
                .find(|c| c.is_recovering)
                .and_then(|c| c.title.name.clone());

            let health = MachineHealth {
                cpu_percent: m.cpu_base + ((tick % 5) as f32 * 0.1),
                ram_mb: m.ram_mb,
                uptime_s: now_mono_ms / 1000,
                os_label: m.os_label.clone(),
                automation_enabled: m.automation_enabled,
                emergency_stop_active: m.emergency_stop,
                active_input_owner: active_owner,
                last_error: None,
            };

            reports.push(AgentToCentral::Report {
                machine_id: m.machine_id.clone(),
                seq: m.seq,
                sent_at_mono_ms: now_mono_ms,
                sent_at_utc_s: now_utc_s,
                health,
                chars: chars_for_machine,
                events: machine_events.remove(&m.machine_id).unwrap_or_default(),
                catalog_spots: m.catalog_spots.clone(),
            });
        }

        reports
    }
}

fn build_nearby_entities(
    c: &SimCharacter,
    tick: u64,
    now_mono_ms: MonotonicMs,
) -> Vec<NearbyEntity> {
    let mut out = Vec::new();
    let offset = (tick % 3) as i32 - 1;
    match c.pos.world {
        WorldId::Kanturu => {
            out.push(NearbyEntity {
                kind: EntityKind::Monster,
                name: "Satyros".into(),
                class_id: 353,
                x: (c.pos.x + 2 + offset).clamp(0, 255),
                y: (c.pos.y - 1).clamp(0, 255),
                observed_by: c.name.clone(),
                observed_at_mono_ms: now_mono_ms,
            });
            out.push(NearbyEntity {
                kind: EntityKind::Monster,
                name: "Genocider".into(),
                class_id: 357,
                x: (c.pos.x - 2).clamp(0, 255),
                y: (c.pos.y + 2 + offset).clamp(0, 255),
                observed_by: c.name.clone(),
                observed_at_mono_ms: now_mono_ms,
            });
            if c.pos.x < 50 && c.pos.y > 200 {
                out.push(NearbyEntity {
                    kind: EntityKind::Npc,
                    name: "Safety Guardian".into(),
                    class_id: 240,
                    x: 34,
                    y: 217,
                    observed_by: c.name.clone(),
                    observed_at_mono_ms: now_mono_ms,
                });
            }
        }
        WorldId::Lorencia => {
            out.push(NearbyEntity {
                kind: EntityKind::GoldenMonster,
                name: "Golden Budge Dragon".into(),
                class_id: 43,
                x: (c.pos.x + 4 + offset).clamp(0, 255),
                y: (c.pos.y + 1).clamp(0, 255),
                observed_by: c.name.clone(),
                observed_at_mono_ms: now_mono_ms,
            });
            out.push(NearbyEntity {
                kind: EntityKind::Npc,
                name: "Trainer".into(),
                class_id: 226,
                x: 122,
                y: 110,
                observed_by: c.name.clone(),
                observed_at_mono_ms: now_mono_ms,
            });
            out.push(NearbyEntity {
                kind: EntityKind::Npc,
                name: "Safety Guardian".into(),
                class_id: 240,
                x: 146,
                y: 110,
                observed_by: c.name.clone(),
                observed_at_mono_ms: now_mono_ms,
            });
        }
        WorldId::Arena => {
            out.push(NearbyEntity {
                kind: EntityKind::Monster,
                name: "Death Rider".into(),
                class_id: 61,
                x: (c.pos.x + 1).clamp(0, 255),
                y: (c.pos.y - 2).clamp(0, 255),
                observed_by: c.name.clone(),
                observed_at_mono_ms: now_mono_ms,
            });
        }
        _ => {
            out.push(NearbyEntity {
                kind: EntityKind::Monster,
                name: "Dark Knight".into(),
                class_id: 10,
                x: (c.pos.x + 2).clamp(0, 255),
                y: (c.pos.y + 2).clamp(0, 255),
                observed_by: c.name.clone(),
                observed_at_mono_ms: now_mono_ms,
            });
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn simulator_is_deterministic_and_covers_four_pcs_and_nine_characters() {
        let mut s1 = SimulatorEngine::new(20261118);
        let mut s2 = SimulatorEngine::new(20261118);

        assert_eq!(s1.machines.len(), 4);
        assert_eq!(s1.characters.len(), 9);

        let r1 = s1.step(5_000, 1_795_000_005);
        let r2 = s2.step(5_000, 1_795_000_005);
        assert_eq!(r1, r2, "mesma semente deve gerar relatórios idênticos");
    }

    #[test]
    fn silencing_machine_withholds_its_reports_for_stale_testing() {
        let mut sim = SimulatorEngine::new(42);
        sim.set_machine_silenced("PC2-FARM-NORTE", true);
        let reports = sim.step(10_000, 1_795_000_010);
        assert_eq!(reports.len(), 3);
    }
}
