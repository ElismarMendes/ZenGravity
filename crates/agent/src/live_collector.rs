//! Coletor ao vivo somente-leitura (`ReadOnlyLiveCollector`).
//!
//! Executa um ciclo completo de observação passiva sobre os processos reais do MU Online
//! abertos na máquina local:
//! 1. Descobre janelas visíveis (`The Classic MU Season 2`) e faz parse do título (`Name`/`Nome`, `Level`, `Resets`).
//! 2. Captura **um único quadro** (`FrameImage`) por janela não minimizada via `PrintWindow`.
//! 3. Extrai todos os recortes em memória a partir desse único quadro:
//!    - Mapa e Coordenada `(x, y)` no canto superior esquerdo;
//!    - Estado do MU Helper (ícone Play/Pause com varredura horizontal `-12..=+40 px`);
//!    - Barra de vida do Dark Raven (`petbar::raven_in_scan`);
//!    - Campo de Zen no inventário (se o inventário estiver aberto, serve de semente/conferência para a trava de RAM);
//!    - Número de HP acima da esfera vermelha (semente/conferência para a trava de HP na RAM).
//! 4. Alimenta as máquinas de estado puras `LockMachine` (Zen `u32`) e `HpLock` (HP `u16`) + `DeathDetector`.
//! 5. Persiste e restaura endereços travados (`data/ram_locks.json`) validando `(pid, process_start_time, char_name)`.
//! 6. Lê entidades próximas (`NearbyEntity` — jogadores, monstros, NPCs, Golden Monsters) na tabela `0x598`.
//! 7. Verifica conexão TCP ativa e produz `AgentToCentral::Report` pronto para a Central e o Desktop.

use crate::image::FrameImage;
use crate::terrain_loader::ClientTerrainLoader;
use crate::win_reader::{
    capture_single_frame, discover_game_windows, has_external_tcp_connection,
    read_minimized_hero_position, read_nearby_entities, read_u16_many, read_u32, scan_u16,
    scan_u32, DiscoveredWindow, DEFAULT_WINDOW_CLASS, REGION_CORNER_MAP_COORDS, REGION_HELPER_WIDE,
    REGION_HP_ORB, REGION_RAVEN_SCAN, REGION_ZEN_INVENTORY,
};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::PathBuf;
use zengravity_core::clock::{MonotonicMs, UtcTimestampSecs};
use zengravity_core::entities::NearbyEntity;
use zengravity_core::game_title::TitleInfo;
use zengravity_core::glyphs::{
    binarize_bgra, binarize_white, helper_on, map_name, read_coords, read_number, Bits,
    COORDS_WHITE, HELPER_ICON, HELPER_WHITE,
};
use zengravity_core::hp_pixel::{binarize_bgra as binarize_hp_bgra, read_hp};
use zengravity_core::hplock::{DeathDetector, HpAction, HpLock, MAX_CANDIDATES};
use zengravity_core::lifecycle::{MetricReading, MetricSource};
use zengravity_core::map::WorldId;
use zengravity_core::petbar::raven_in_scan;
use zengravity_core::ramlock::{LockAction, LockMachine, LockState};
use zengravity_core::spots::{FallbackPolicy, SpotCoord};
use zengravity_protocol::{AgentCharReport, AgentToCentral, MachineHealth, PROTOCOL_VERSION};

const DC_THRESHOLD_CYCLES: u32 = 4;
const ZEN_CROSSCHECK_CYCLES: u64 = 12;
const HP_CROSSCHECK_CYCLES: u64 = 6;

/// Trava de RAM persistida em disco (`data/ram_locks.json`).
/// Só é reaproveitada se o processo tiver exatamente o mesmo `pid`, `process_start_time` e `char_name`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SavedProcessLock {
    pub pid: u32,
    pub process_start_time: u64,
    pub char_name: String,
    pub zen_addr: Option<u64>,
    pub hp_addr: Option<u64>,
}

/// Abstração da leitura do SO/Jogo para permitir testes unitários determinísticos do coletor
/// sem depender de uma janela real aberta durante o `cargo test`.
pub trait LiveSystemAccess {
    fn list_windows(&self) -> Vec<DiscoveredWindow>;
    fn has_tcp(&self, pid: u32) -> bool;
    fn capture_frame(&self, hwnd: isize) -> Option<FrameImage>;
    fn read_mem_u32(&self, pid: u32, addr: u64) -> Option<u32>;
    fn scan_mem_u32(&self, pid: u32, seed: u32) -> Vec<u64>;
    fn read_mem_u16_many(&self, pid: u32, addrs: &[u64]) -> Vec<Option<u16>>;
    fn scan_mem_u16(&self, pid: u32, seed: u16) -> Vec<u64>;
    fn read_objects(
        &self,
        pid: u32,
        char_name: &str,
        pos: (i32, i32),
        now_mono_ms: MonotonicMs,
    ) -> Option<Vec<NearbyEntity>>;
    fn read_hero_pos_from_ram(
        &self,
        pid: u32,
        char_name: &str,
        now_mono_ms: MonotonicMs,
    ) -> Option<(i32, i32)>;
}

/// Implementação real Win32 somente-leitura para o Windows.
pub struct Win32ReadOnlyAccess {
    pub window_class: String,
    pub allow_loopback_tcp: bool,
}

impl Default for Win32ReadOnlyAccess {
    fn default() -> Self {
        Self {
            window_class: DEFAULT_WINDOW_CLASS.to_string(),
            allow_loopback_tcp: false,
        }
    }
}

impl LiveSystemAccess for Win32ReadOnlyAccess {
    fn list_windows(&self) -> Vec<DiscoveredWindow> {
        discover_game_windows(&self.window_class)
    }

    fn has_tcp(&self, pid: u32) -> bool {
        has_external_tcp_connection(pid, self.allow_loopback_tcp)
    }

    fn capture_frame(&self, hwnd: isize) -> Option<FrameImage> {
        capture_single_frame(hwnd)
    }

    fn read_mem_u32(&self, pid: u32, addr: u64) -> Option<u32> {
        read_u32(pid, addr)
    }

    fn scan_mem_u32(&self, pid: u32, seed: u32) -> Vec<u64> {
        scan_u32(pid, seed)
    }

    fn read_mem_u16_many(&self, pid: u32, addrs: &[u64]) -> Vec<Option<u16>> {
        read_u16_many(pid, addrs)
    }

    fn scan_mem_u16(&self, pid: u32, seed: u16) -> Vec<u64> {
        scan_u16(pid, seed, MAX_CANDIDATES)
    }

    fn read_objects(
        &self,
        pid: u32,
        char_name: &str,
        pos: (i32, i32),
        now_mono_ms: MonotonicMs,
    ) -> Option<Vec<NearbyEntity>> {
        read_nearby_entities(pid, char_name, pos, now_mono_ms)
    }

    fn read_hero_pos_from_ram(
        &self,
        pid: u32,
        char_name: &str,
        now_mono_ms: MonotonicMs,
    ) -> Option<(i32, i32)> {
        read_minimized_hero_position(pid, char_name, now_mono_ms)
    }
}

#[derive(Default)]
struct PerProcessState {
    process_start_time: Option<u64>,
    char_name: Option<String>,
    zen_lock: LockMachine,
    hp_lock: HpLock,
    death_detector: DeathDetector,
    last_zen: Option<(u64, MetricSource, MonotonicMs, UtcTimestampSecs)>,
    last_hp: Option<(u32, MetricSource, MonotonicMs, UtcTimestampSecs)>,
    max_hp_seen: u32,
    last_pet_hp: Option<(u8, MonotonicMs, UtcTimestampSecs)>,
    last_position: Option<(SpotCoord, MetricSource, MonotonicMs, UtcTimestampSecs)>,
    last_helper: Option<(bool, MonotonicMs, UtcTimestampSecs)>,
    home_spot: Option<SpotCoord>,
    no_tcp_cycles: u32,
    cycles: u64,
    alive_cycle: u64,
}

pub struct ReadOnlyLiveCollector {
    pub machine_id: String,
    pub seq: u64,
    pub ram_paused: bool,
    locks_file_path: Option<PathBuf>,
    pending_saved_locks: Vec<SavedProcessLock>,
    processes: HashMap<u32, PerProcessState>,
    pub terrain_loader: ClientTerrainLoader,
}

impl ReadOnlyLiveCollector {
    pub fn new(machine_id: impl Into<String>, locks_file_path: Option<PathBuf>) -> Self {
        let pending_saved_locks = locks_file_path
            .as_ref()
            .and_then(|p| std::fs::read_to_string(p).ok())
            .and_then(|txt| serde_json::from_str::<Vec<SavedProcessLock>>(&txt).ok())
            .unwrap_or_default();

        Self {
            machine_id: machine_id.into(),
            seq: 0,
            ram_paused: false,
            locks_file_path,
            pending_saved_locks,
            processes: HashMap::new(),
            terrain_loader: ClientTerrainLoader::discover(None),
        }
    }

    pub fn relock_character(&mut self, char_name: &str) -> bool {
        let mut found = false;
        for st in self.processes.values_mut() {
            if st
                .char_name
                .as_ref()
                .is_some_and(|n| n.eq_ignore_ascii_case(char_name))
            {
                st.zen_lock.reset();
                st.hp_lock.reset();
                st.death_detector.forget();
                found = true;
            }
        }
        found
    }

    pub fn assign_home_spot(&mut self, char_name: &str, spot: SpotCoord) -> bool {
        for st in self.processes.values_mut() {
            if st
                .char_name
                .as_ref()
                .is_some_and(|n| n.eq_ignore_ascii_case(char_name))
            {
                st.home_spot = Some(spot);
                return true;
            }
        }
        false
    }

    pub fn current_saved_locks(&self) -> Vec<SavedProcessLock> {
        let mut list: Vec<SavedProcessLock> = self
            .processes
            .iter()
            .filter_map(|(&pid, st)| {
                let zen_addr = st.zen_lock.is_locked();
                let hp_addr = st.hp_lock.is_locked();
                if zen_addr.is_none() && hp_addr.is_none() {
                    return None;
                }
                Some(SavedProcessLock {
                    pid,
                    process_start_time: st.process_start_time?,
                    char_name: st.char_name.clone()?,
                    zen_addr,
                    hp_addr,
                })
            })
            .collect();
        list.sort_by_key(|s| s.pid);
        list
    }

    fn persist_locks_if_needed(&self) {
        let Some(path) = &self.locks_file_path else {
            return;
        };
        let locks = self.current_saved_locks();
        if let Ok(json) = serde_json::to_string_pretty(&locks) {
            if let Some(parent) = path.parent() {
                let _ = std::fs::create_dir_all(parent);
            }
            let _ = std::fs::write(path, json);
        }
    }

    pub fn hello_message(&self) -> AgentToCentral {
        AgentToCentral::Hello {
            protocol_version: PROTOCOL_VERSION,
            machine_id: self.machine_id.clone(),
            agent_version: "0.2.0-live-ro".into(),
            is_simulator: false,
        }
    }

    /// Executa um ciclo completo de observação passiva sobre os processos do jogo.
    pub fn collect_cycle(
        &mut self,
        access: &dyn LiveSystemAccess,
        now_mono_ms: MonotonicMs,
        now_utc_s: UtcTimestampSecs,
    ) -> AgentToCentral {
        self.seq += 1;
        let windows = access.list_windows();
        self.processes
            .retain(|pid, _| windows.iter().any(|w| w.pid == *pid));

        let mut char_reports = Vec::with_capacity(windows.len());

        for win in windows {
            if let Some(exe) = &win.exe_path {
                self.terrain_loader.set_data_dir_from_exe_path(exe);
            }

            let title = TitleInfo::parse(&win.title);
            let is_new = !self.processes.contains_key(&win.pid);
            let st = self.processes.entry(win.pid).or_default();

            if is_new {
                st.process_start_time = win.process_start_time;
                st.char_name = title.name.clone();
                st.max_hp_seen = 100;
                if let Some(idx) = self.pending_saved_locks.iter().position(|s| {
                    s.pid == win.pid
                        && Some(s.process_start_time) == st.process_start_time
                        && Some(&s.char_name) == title.name.as_ref()
                }) {
                    let saved = self.pending_saved_locks.remove(idx);
                    if let Some(z_addr) = saved.zen_addr {
                        st.zen_lock.state = LockState::Locked {
                            addr: z_addr,
                            bad_reads: 0,
                            ocr_mismatch: 0,
                        };
                    }
                    if let Some(h_addr) = saved.hp_addr {
                        st.hp_lock.restore(h_addr);
                    }
                    tracing::info!(
                        pid = win.pid,
                        char = %saved.char_name,
                        zen_addr = ?saved.zen_addr,
                        hp_addr = ?saved.hp_addr,
                        "Travas de RAM restauradas para o mesmo processo"
                    );
                }
            }

            st.cycles += 1;
            if title.name.is_some() {
                st.char_name = title.name.clone();
            }
            let display_name = st
                .char_name
                .clone()
                .unwrap_or_else(|| format!("PID-{}", win.pid));

            // 1) Captura única de quadro por ciclo (se a janela não estiver minimizada)
            let frame_opt = if win.is_minimized {
                None
            } else {
                access.capture_frame(win.hwnd)
            };

            // 2) Extração visual a partir do único FrameImage em memória
            let mut visual_zen: Option<u32> = None;
            let mut visual_hp: Option<u32> = None;
            let mut visual_pos: Option<SpotCoord> = None;
            let mut visual_helper: Option<bool> = None;
            let mut visual_raven: Option<u8> = None;

            if let Some(frame) = &frame_opt {
                // 2.a) Mapa e coordenadas no canto superior esquerdo
                if let Some(corner_crop) = frame.crop(REGION_CORNER_MAP_COORDS) {
                    let bits = binarize_white(
                        &corner_crop.bgra,
                        corner_crop.w as usize,
                        corner_crop.h as usize,
                        COORDS_WHITE,
                    );
                    if let (Some(m_name), Some((cx, cy))) = (map_name(&bits), read_coords(&bits)) {
                        if let Some(world) = WorldId::from_name(m_name) {
                            let _ = self.terrain_loader.get_or_load(world);
                            visual_pos = Some(SpotCoord::new(world, cx as i32, cy as i32));
                        }
                    }
                }

                // 2.b) Ícone do MU Helper (Play/Pause) com tolerância horizontal (-12..=+40 px)
                if let Some(helper_crop) = frame.crop(REGION_HELPER_WIDE) {
                    visual_helper = detect_helper_icon_in_crop(&helper_crop);
                }

                // 2.c) Barra do Dark Raven no topo
                if let Some(raven_crop) = frame.crop(REGION_RAVEN_SCAN) {
                    visual_raven = raven_in_scan(
                        &raven_crop.bgra,
                        raven_crop.w as usize,
                        raven_crop.h as usize,
                    );
                }

                // 2.d) Campo de Zen no inventário (se o inventário estiver aberto)
                if let Some(zen_crop) = frame.crop(REGION_ZEN_INVENTORY) {
                    let bits =
                        binarize_bgra(&zen_crop.bgra, zen_crop.w as usize, zen_crop.h as usize);
                    visual_zen = read_number(&bits).and_then(|n| u32::try_from(n).ok());
                }

                // 2.e) Número de HP acima da esfera vermelha
                if let Some(hp_crop) = frame.crop(REGION_HP_ORB) {
                    let bits =
                        binarize_hp_bgra(&hp_crop.bgra, hp_crop.w as usize, hp_crop.h as usize);
                    visual_hp = read_hp(&bits);
                }
            }

            // 3) Máquina de Trava de Zen na RAM (somente leitura)
            let zen_reading = if self.ram_paused {
                MetricReading::unavailable(now_mono_ms, now_utc_s)
            } else {
                evaluate_zen_telemetry(access, win.pid, visual_zen, st, now_mono_ms, now_utc_s)
            };

            // 4) Máquina de Trava de HP na RAM (somente leitura)
            let hp_reading = if self.ram_paused {
                MetricReading::unavailable(now_mono_ms, now_utc_s)
            } else {
                evaluate_hp_telemetry(access, win.pid, visual_hp, st, now_mono_ms, now_utc_s)
            };

            if let Some(hp_val) = hp_reading.value {
                if hp_val > st.max_hp_seen {
                    st.max_hp_seen = hp_val;
                }
                let _ = st.death_detector.on_hp(hp_val);
            }

            // 5) Posição (Tela ou fallback da lista de objetos da RAM quando minimizado)
            let pos_reading = if let Some(vp) = visual_pos {
                st.last_position =
                    Some((vp, MetricSource::ScreenPixelModel, now_mono_ms, now_utc_s));
                if st.home_spot.is_none() {
                    st.home_spot = Some(vp);
                }
                MetricReading::verified(vp, MetricSource::ScreenPixelModel, now_mono_ms, now_utc_s)
            } else if win.is_minimized && !self.ram_paused {
                if let (Some((last_spot, _, _, _)), Some((rx, ry))) = (
                    st.last_position,
                    access.read_hero_pos_from_ram(win.pid, &display_name, now_mono_ms),
                ) {
                    let ram_spot = SpotCoord::new(last_spot.world, rx, ry);
                    st.last_position =
                        Some((ram_spot, MetricSource::RamLocked, now_mono_ms, now_utc_s));
                    MetricReading::verified(
                        ram_spot,
                        MetricSource::RamLocked,
                        now_mono_ms,
                        now_utc_s,
                    )
                } else if let Some((last_spot, src, obs_mono, obs_utc)) = st.last_position {
                    MetricReading::provisional(last_spot, src, obs_mono, obs_utc)
                } else {
                    MetricReading::unavailable(now_mono_ms, now_utc_s)
                }
            } else if let Some((last_spot, src, obs_mono, obs_utc)) = st.last_position {
                MetricReading::provisional(last_spot, src, obs_mono, obs_utc)
            } else {
                MetricReading::unavailable(now_mono_ms, now_utc_s)
            };

            // 6) Helper e Dark Raven
            let helper_reading = if let Some(h_on) = visual_helper {
                st.last_helper = Some((h_on, now_mono_ms, now_utc_s));
                MetricReading::verified(
                    h_on,
                    MetricSource::ScreenPixelModel,
                    now_mono_ms,
                    now_utc_s,
                )
            } else if let Some((prev_h, obs_m, obs_u)) = st.last_helper {
                MetricReading::provisional(prev_h, MetricSource::ScreenPixelModel, obs_m, obs_u)
            } else {
                MetricReading::unavailable(now_mono_ms, now_utc_s)
            };

            let pet_reading = if let Some(r_pct) = visual_raven {
                st.last_pet_hp = Some((r_pct, now_mono_ms, now_utc_s));
                MetricReading::verified(
                    r_pct,
                    MetricSource::ScreenPixelModel,
                    now_mono_ms,
                    now_utc_s,
                )
            } else if let Some((prev_r, obs_m, obs_u)) = st.last_pet_hp {
                MetricReading::provisional(prev_r, MetricSource::ScreenPixelModel, obs_m, obs_u)
            } else {
                MetricReading::unavailable(now_mono_ms, now_utc_s)
            };

            // 7) Entidades próximas na memória (`0x598`) se soubermos a posição e nome do personagem
            let nearby_reading = if !self.ram_paused {
                if let (Some(char_name), Some(spot)) = (&title.name, pos_reading.value) {
                    if let Some(entities) =
                        access.read_objects(win.pid, char_name, (spot.x, spot.y), now_mono_ms)
                    {
                        MetricReading::verified(
                            entities,
                            MetricSource::RamLocked,
                            now_mono_ms,
                            now_utc_s,
                        )
                    } else {
                        MetricReading::unavailable(now_mono_ms, now_utc_s)
                    }
                } else {
                    MetricReading::unavailable(now_mono_ms, now_utc_s)
                }
            } else {
                MetricReading::unavailable(now_mono_ms, now_utc_s)
            };

            // 8) Verificação TCP + Vivacidade de Zen/HP para detecção segura de DC
            let tcp_ok = access.has_tcp(win.pid);
            if tcp_ok {
                st.no_tcp_cycles = 0;
                st.alive_cycle = st.cycles;
            } else {
                st.no_tcp_cycles = st.no_tcp_cycles.saturating_add(1);
            }
            let disconnected_confirmed = st.no_tcp_cycles >= DC_THRESHOLD_CYCLES
                && st.cycles.saturating_sub(st.alive_cycle) >= u64::from(DC_THRESHOLD_CYCLES);

            let diag_detail = build_diagnostic_detail(st, win.is_minimized);

            char_reports.push(AgentCharReport {
                pid: win.pid,
                title,
                zen: zen_reading,
                hp: hp_reading,
                max_hp: st.max_hp_seen.max(100),
                pet_hp_pct: pet_reading,
                position: pos_reading,
                helper_active: helper_reading,
                nearby_entities: nearby_reading,
                tcp_connected: tcp_ok,
                disconnected_confirmed,
                is_recovering: false,
                recovery_detail: Some(diag_detail),
                char_automation_enabled: false, // Fase 2 opera 100% em modo somente-leitura
                home_spot: st.home_spot,
                active_spot: st.home_spot,
                fallback_policy: FallbackPolicy::default(),
                manual_search: None,
                planned_route: Vec::new(),
            });
        }

        self.persist_locks_if_needed();

        let health = MachineHealth {
            cpu_percent: 0.3,
            ram_mb: 16.5,
            uptime_s: now_mono_ms / 1000,
            os_label: "Windows (Modo Real Somente Leitura)".into(),
            automation_enabled: false,
            emergency_stop_active: false,
            active_input_owner: None,
            last_error: None,
        };

        let catalog_spots: Vec<SpotCoord> = self
            .processes
            .values()
            .filter_map(|p| p.home_spot)
            .collect();

        AgentToCentral::Report {
            machine_id: self.machine_id.clone(),
            seq: self.seq,
            sent_at_mono_ms: now_mono_ms,
            sent_at_utc_s: now_utc_s,
            health,
            chars: char_reports,
            events: Vec::new(),
            catalog_spots,
        }
    }
}

fn detect_helper_icon_in_crop(wide_crop: &FrameImage) -> Option<bool> {
    let [_, _, w, h] = HELPER_ICON;
    if wide_crop.w < w as u32 || wide_crop.h < h as u32 {
        return None;
    }
    let big = binarize_white(
        &wide_crop.bgra,
        wide_crop.w as usize,
        wide_crop.h as usize,
        HELPER_WHITE,
    );
    let max_shift = (wide_crop.w as i32 - w).max(0);
    let mut offsets: Vec<i32> = (-12..=(max_shift - 12)).collect();
    offsets.sort_by_key(|d| d.abs());
    for d in offsets {
        let ox = (d + 12) as usize;
        if ox + w as usize > big.w {
            continue;
        }
        let mut sub = Bits::new(w as usize, h as usize);
        for yy in 0..h as usize {
            for xx in 0..w as usize {
                sub.px[yy * w as usize + xx] = big.px[yy * big.w + ox + xx];
            }
        }
        if let Some(state) = helper_on(&sub) {
            return Some(state);
        }
    }
    None
}

fn evaluate_zen_telemetry(
    access: &dyn LiveSystemAccess,
    pid: u32,
    visual_zen: Option<u32>,
    st: &mut PerProcessState,
    now_mono_ms: MonotonicMs,
    now_utc_s: UtcTimestampSecs,
) -> MetricReading<u64> {
    if let Some(addr) = st.zen_lock.is_locked() {
        let ram_val = access.read_mem_u32(pid, addr);
        if let Some(z) = st.zen_lock.on_locked_read(ram_val) {
            if st.cycles % ZEN_CROSSCHECK_CYCLES == 0 {
                if let Some(vz) = visual_zen {
                    st.zen_lock.on_crosscheck(vz, z);
                }
            }
            if st.zen_lock.is_locked().is_some() {
                if st.last_zen.is_none_or(|(prev, _, _, _)| prev != z) {
                    st.alive_cycle = st.cycles;
                }
                st.last_zen = Some((z, MetricSource::RamLocked, now_mono_ms, now_utc_s));
                return MetricReading::verified(z, MetricSource::RamLocked, now_mono_ms, now_utc_s);
            }
        }
    }

    match st.zen_lock.on_ocr(visual_zen) {
        LockAction::Nothing => {}
        LockAction::Scan(seed) => {
            let found = access.scan_mem_u32(pid, seed);
            st.zen_lock.on_scan(found);
        }
        LockAction::ReadCandidates(cands) => {
            if let Some(vz) = visual_zen {
                let reads: Vec<(u64, Option<u32>)> = cands
                    .into_iter()
                    .map(|a| (a, access.read_mem_u32(pid, a)))
                    .collect();
                st.zen_lock.on_filter_reads(vz, &reads);
            }
        }
    }

    if let Some(addr) = st.zen_lock.is_locked() {
        let ram_val = access.read_mem_u32(pid, addr);
        if let Some(z) = st.zen_lock.on_locked_read(ram_val) {
            st.last_zen = Some((z, MetricSource::RamLocked, now_mono_ms, now_utc_s));
            return MetricReading::verified(z, MetricSource::RamLocked, now_mono_ms, now_utc_s);
        }
    }

    if let Some(vz) = visual_zen {
        let z = u64::from(vz);
        st.last_zen = Some((z, MetricSource::ScreenPixelModel, now_mono_ms, now_utc_s));
        return MetricReading::provisional(
            z,
            MetricSource::ScreenPixelModel,
            now_mono_ms,
            now_utc_s,
        );
    }

    if let Some((last_z, src, obs_m, obs_u)) = st.last_zen {
        return MetricReading::provisional(last_z, src, obs_m, obs_u);
    }

    MetricReading::unavailable(now_mono_ms, now_utc_s)
}

fn evaluate_hp_telemetry(
    access: &dyn LiveSystemAccess,
    pid: u32,
    visual_hp: Option<u32>,
    st: &mut PerProcessState,
    now_mono_ms: MonotonicMs,
    now_utc_s: UtcTimestampSecs,
) -> MetricReading<u32> {
    if let Some(addr) = st.hp_lock.is_locked() {
        let before = access
            .read_mem_u16_many(pid, &[addr])
            .into_iter()
            .next()
            .flatten();
        if let Some(hp) = st.hp_lock.on_locked_read(before) {
            if st.cycles % HP_CROSSCHECK_CYCLES == 0 {
                if let Some(vhp) = visual_hp {
                    let after = access
                        .read_mem_u16_many(pid, &[addr])
                        .into_iter()
                        .next()
                        .flatten();
                    st.hp_lock.on_crosscheck(vhp, before, after);
                }
            }
            if st.hp_lock.is_locked().is_some() {
                if st.last_hp.is_none_or(|(prev, _, _, _)| prev != hp) {
                    st.alive_cycle = st.cycles;
                }
                st.last_hp = Some((hp, MetricSource::RamLocked, now_mono_ms, now_utc_s));
                return MetricReading::verified(
                    hp,
                    MetricSource::RamLocked,
                    now_mono_ms,
                    now_utc_s,
                );
            }
        }
    }

    let cands_before = st.hp_lock.candidates();
    let reads_before = if cands_before.is_empty() {
        Vec::new()
    } else {
        access.read_mem_u16_many(pid, &cands_before)
    };

    match st.hp_lock.on_screen(visual_hp) {
        HpAction::Nothing => {}
        HpAction::Scan(seed) => {
            let found = access.scan_mem_u16(pid, seed);
            st.hp_lock.on_scan(found);
        }
        HpAction::ReadCandidates(screen_hp) => {
            let reads_after = access.read_mem_u16_many(pid, &cands_before);
            let triples: Vec<(u64, Option<u16>, Option<u16>)> = cands_before
                .into_iter()
                .enumerate()
                .map(|(i, a)| {
                    (
                        a,
                        reads_before.get(i).copied().flatten(),
                        reads_after.get(i).copied().flatten(),
                    )
                })
                .collect();
            st.hp_lock.on_candidate_reads(screen_hp, &triples);
        }
    }

    if let Some(addr) = st.hp_lock.is_locked() {
        let val = access
            .read_mem_u16_many(pid, &[addr])
            .into_iter()
            .next()
            .flatten();
        if let Some(hp) = st.hp_lock.on_locked_read(val) {
            st.last_hp = Some((hp, MetricSource::RamLocked, now_mono_ms, now_utc_s));
            return MetricReading::verified(hp, MetricSource::RamLocked, now_mono_ms, now_utc_s);
        }
    }

    if let Some(vhp) = visual_hp {
        st.last_hp = Some((vhp, MetricSource::ScreenPixelModel, now_mono_ms, now_utc_s));
        return MetricReading::provisional(
            vhp,
            MetricSource::ScreenPixelModel,
            now_mono_ms,
            now_utc_s,
        );
    }

    if let Some((last_h, src, obs_m, obs_u)) = st.last_hp {
        return MetricReading::provisional(last_h, src, obs_m, obs_u);
    }

    MetricReading::unavailable(now_mono_ms, now_utc_s)
}

fn build_diagnostic_detail(st: &PerProcessState, is_minimized: bool) -> String {
    let zen_st = match &st.zen_lock.state {
        LockState::NeedSeed => "Zen: aguardando abrir inventário (V)",
        LockState::NeedScan { .. } => "Zen: varrendo RAM",
        LockState::Filtering { candidates, .. } => {
            return format!(
                "Zen: filtrando {} candidatos | {}",
                candidates.len(),
                st.hp_lock.status()
            );
        }
        LockState::Locked { addr, .. } => {
            return format!(
                "Zen RAM {addr:#x} | {} {}",
                st.hp_lock.status(),
                if is_minimized { "(Minimizado)" } else { "" }
            )
            .trim()
            .to_string();
        }
    };
    format!(
        "{zen_st} | {}{}",
        st.hp_lock.status(),
        if is_minimized { " (Minimizado)" } else { "" }
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    struct FakeLiveAccess {
        windows: Vec<DiscoveredWindow>,
        zen_mem: HashMap<u64, u32>,
        hp_mem: HashMap<u64, u16>,
    }

    impl LiveSystemAccess for FakeLiveAccess {
        fn list_windows(&self) -> Vec<DiscoveredWindow> {
            self.windows.clone()
        }
        fn has_tcp(&self, _pid: u32) -> bool {
            true
        }
        fn capture_frame(&self, _hwnd: isize) -> Option<FrameImage> {
            None
        }
        fn read_mem_u32(&self, _pid: u32, addr: u64) -> Option<u32> {
            self.zen_mem.get(&addr).copied()
        }
        fn scan_mem_u32(&self, _pid: u32, seed: u32) -> Vec<u64> {
            self.zen_mem
                .iter()
                .filter(|(_, v)| **v == seed)
                .map(|(k, _)| *k)
                .collect()
        }
        fn read_mem_u16_many(&self, _pid: u32, addrs: &[u64]) -> Vec<Option<u16>> {
            addrs.iter().map(|a| self.hp_mem.get(a).copied()).collect()
        }
        fn scan_mem_u16(&self, _pid: u32, seed: u16) -> Vec<u64> {
            self.hp_mem
                .iter()
                .filter(|(_, v)| **v == seed)
                .map(|(k, _)| *k)
                .collect()
        }
        fn read_objects(
            &self,
            _pid: u32,
            _char_name: &str,
            _pos: (i32, i32),
            _now_mono_ms: MonotonicMs,
        ) -> Option<Vec<NearbyEntity>> {
            None
        }
        fn read_hero_pos_from_ram(
            &self,
            _pid: u32,
            _char_name: &str,
            _now_mono_ms: MonotonicMs,
        ) -> Option<(i32, i32)> {
            Some((123, 233))
        }
    }

    #[test]
    fn collector_restores_saved_ram_locks_and_reads_zen_and_hp_passively() {
        let mut access = FakeLiveAccess {
            windows: vec![DiscoveredWindow {
                pid: 8860,
                hwnd: 0x10020,
                title: "TheClassic MU || Name: x9901 || Level: 400 || Resets: 1026".into(),
                is_minimized: false,
                process_start_time: Some(999_888_777),
                exe_path: None,
            }],
            zen_mem: HashMap::new(),
            hp_mem: HashMap::new(),
        };
        access.zen_mem.insert(0x4000, 913_697_884);
        access.hp_mem.insert(0x5000, 6850);

        let mut collector = ReadOnlyLiveCollector::new("PC-LOCAL", None);
        collector.pending_saved_locks.push(SavedProcessLock {
            pid: 8860,
            process_start_time: 999_888_777,
            char_name: "x9901".into(),
            zen_addr: Some(0x4000),
            hp_addr: Some(0x5000),
        });

        let msg = collector.collect_cycle(&access, 5_000, 1_795_000_005);
        let AgentToCentral::Report { chars, .. } = msg else {
            panic!("esperado Report");
        };
        assert_eq!(chars.len(), 1);
        assert_eq!(chars[0].title.name.as_deref(), Some("x9901"));
        assert_eq!(chars[0].zen.value, Some(913_697_884));
        assert_eq!(chars[0].zen.source, MetricSource::RamLocked);
        assert_eq!(chars[0].hp.value, Some(6850));
        assert_eq!(chars[0].hp.source, MetricSource::RamLocked);
    }
}
