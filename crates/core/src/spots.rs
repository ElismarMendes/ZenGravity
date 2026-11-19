//! Modelo unificado de Spots (`SpotCoord`), Catálogo compartilhado, Leases,
//! Regras de Fallback e Busca Manual.
//!
//! Regra inegociável: ausência de observador próximo nunca significa spot livre.

use crate::entities::{EntityKind, NearbyEntity};
use crate::map::WorldId;
use crate::{ARRIVAL_RADIUS, OCCUPIED_RADIUS, OWN_SPACING};

/// Coordenada única e fortemente tipada de um Spot em um mapa específico.
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct SpotCoord {
    pub world: WorldId,
    pub x: i32,
    pub y: i32,
}

impl SpotCoord {
    pub fn new(world: WorldId, x: i32, y: i32) -> Self {
        Self { world, x, y }
    }

    pub fn is_valid(self) -> bool {
        (0..=255).contains(&self.x) && (0..=255).contains(&self.y)
    }

    /// Distância de Chebyshev (máximo entre |dx| e |dy|) no mesmo mundo.
    pub fn distance_chebyshev(self, other: Self) -> Option<i32> {
        if self.world != other.world {
            return None;
        }
        Some((self.x - other.x).abs().max((self.y - other.y).abs()))
    }

    pub fn is_near(self, other: Self, radius: i32) -> bool {
        self.distance_chebyshev(other).is_some_and(|d| d <= radius)
    }

    pub fn label(self) -> String {
        format!("{} ({}, {})", self.world.name(), self.x, self.y)
    }
}

/// Entrada catalogada (F2) no diretório de spots.
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CatalogEntry {
    pub spot: SpotCoord,
    pub priority: u32,
    pub enabled: bool,
    pub source_machine: String,
}

/// Reserva temporária em trânsito (`Lease`) para evitar que dois PCs caminhem para o mesmo spot.
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SpotLease {
    pub machine: String,
    pub char_name: String,
    pub spot: SpotCoord,
    pub expires_at_mono_s: u64,
}

/// Observação recente de um spot por um personagem nosso a até `OCCUPIED_RADIUS` tiles.
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SpotObservation {
    pub spot: SpotCoord,
    pub observed_at_mono_s: u64,
    pub observed_by: String,
    pub foreign_players: Vec<String>,
}

/// Estado de ocupação verificado de um spot do catálogo.
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[derive(Debug, Clone, PartialEq, Eq)]
#[cfg_attr(feature = "serde", serde(tag = "status", rename_all = "snake_case"))]
pub enum SpotOccupancyStatus {
    /// Ocupado ou reservado por um personagem nosso.
    OwnedByUs { char_name: String, machine: String },
    /// Ocupado por jogador externo observado recentemente.
    OccupiedByForeign {
        players: Vec<String>,
        observed_by: String,
        age_s: u64,
    },
    /// Observado recentemente por um personagem nosso próximo e confirmado sem invasores.
    ConfirmedFree { observed_by: String, age_s: u64 },
    /// Nenhum personagem nosso está perto (ou observação expirou). NUNCA tratamos como livre sem ir lá!
    Unobserved,
}

/// Regras de retorno e fallback configuráveis por personagem.
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FallbackPolicy {
    pub enabled: bool,
    /// Mapas alternativos em ordem de prioridade (padrão: Arena/Stadium primeiro).
    pub secondary_worlds: Vec<WorldId>,
    /// Minutos no mapa alternativo antes de reavaliar o mapa original (padrão: 60 min / 1 hora).
    pub reevaluate_original_after_min: u64,
    pub return_on_death: bool,
    pub spot_locked: bool,
}

impl Default for FallbackPolicy {
    fn default() -> Self {
        Self {
            enabled: true,
            secondary_worlds: vec![WorldId::Arena, WorldId::LostTower, WorldId::Dungeon],
            reevaluate_original_after_min: 60,
            return_on_death: true,
            spot_locked: false,
        }
    }
}

/// Estado de uma busca manual ("Procurar spot agora" com seletor de mapa).
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ManualSpotSearchState {
    pub target_world: WorldId,
    pub total_candidates: u32,
    pub examined: u32,
    pub current_target: Option<SpotCoord>,
    pub active: bool,
    pub status_message: String,
}

/// Diretório coordenado de spots entre todos os PCs.
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SpotDirectory {
    pub catalog: Vec<CatalogEntry>,
    pub leases: Vec<SpotLease>,
    pub observations: Vec<SpotObservation>,
    pub friendly_chars: Vec<String>,
    /// `(machine, char_name, home_spot, active_spot)`
    pub members: Vec<(String, String, Option<SpotCoord>, Option<SpotCoord>)>,
}

impl SpotDirectory {
    pub fn register_member(
        &mut self,
        machine: &str,
        char_name: &str,
        home: Option<SpotCoord>,
        active: Option<SpotCoord>,
    ) {
        if let Some(entry) = self
            .members
            .iter_mut()
            .find(|(_, c, _, _)| c.eq_ignore_ascii_case(char_name))
        {
            entry.0 = machine.to_string();
            entry.2 = home;
            entry.3 = active;
        } else {
            self.members
                .push((machine.to_string(), char_name.to_string(), home, active));
        }
    }

    pub fn is_friendly(&self, player_name: &str) -> bool {
        self.members
            .iter()
            .any(|(_, c, _, _)| c.eq_ignore_ascii_case(player_name))
            || self
                .friendly_chars
                .iter()
                .any(|f| f.eq_ignore_ascii_case(player_name))
    }

    pub fn add_catalog_spot(&mut self, spot: SpotCoord, priority: u32, machine: &str) {
        let _ = self.try_add_catalog_spot(spot, priority, machine);
    }

    /// Tenta catalogar um spot (F2 ou clique no Mapa 2D), garantindo que:
    /// 1. Não é em Lorencia (cidade sem spots de farm);
    /// 2. Se já existir a até `ARRIVAL_RADIUS` (2 tiles), atualiza/mantém a entrada;
    /// 3. Se estiver entre `3..=OWN_SPACING` (8 tiles) de outro spot catalogado no mesmo mapa,
    ///    recusa para evitar KS mútuo entre helpers do nosso grupo.
    pub fn try_add_catalog_spot(
        &mut self,
        spot: SpotCoord,
        priority: u32,
        machine: &str,
    ) -> Result<usize, String> {
        if !spot.is_valid() {
            return Err("Coordenada fora dos limites 0..=255".into());
        }
        if spot.world == WorldId::Lorencia {
            return Err(
                "Lorencia é cidade: spots de farm só são permitidos em mapas de caça".into(),
            );
        }
        if let Some((idx, existing)) = self
            .catalog
            .iter_mut()
            .enumerate()
            .find(|(_, c)| c.spot.is_near(spot, ARRIVAL_RADIUS))
        {
            existing.priority = existing.priority.min(priority);
            return Ok(idx + 1);
        }
        if let Some(conflict) = self
            .catalog
            .iter()
            .find(|c| c.spot.is_near(spot, OWN_SPACING))
        {
            return Err(format!(
                "Spot {} muito próximo de {} (mínimo de {} tiles entre spots para não disputar mobs)",
                spot.label(),
                conflict.spot.label(),
                OWN_SPACING + 1
            ));
        }
        self.catalog.push(CatalogEntry {
            spot,
            priority,
            enabled: true,
            source_machine: machine.to_string(),
        });
        self.catalog
            .sort_by_key(|c| (c.spot.world.code(), c.priority, c.spot.x, c.spot.y));
        Ok(self.catalog.len())
    }

    pub fn remove_catalog_spot(&mut self, spot: SpotCoord) -> bool {
        let before = self.catalog.len();
        self.catalog
            .retain(|c| !c.spot.is_near(spot, ARRIVAL_RADIUS));
        self.catalog.len() < before
    }

    pub fn add_friendly_char(&mut self, name: &str) -> Result<(), String> {
        let cleaned = crate::sanitize_text(name, 16);
        if cleaned.is_empty() || cleaned.contains(' ') {
            return Err("Nome de personagem aliado inválido".into());
        }
        if !self
            .friendly_chars
            .iter()
            .any(|f| f.eq_ignore_ascii_case(&cleaned))
        {
            self.friendly_chars.push(cleaned);
            self.friendly_chars.sort();
        }
        Ok(())
    }

    pub fn remove_friendly_char(&mut self, name: &str) -> bool {
        let before = self.friendly_chars.len();
        self.friendly_chars
            .retain(|f| !f.eq_ignore_ascii_case(name.trim()));
        self.friendly_chars.len() < before
    }

    /// Verifica se algum outro personagem nosso já é dono ou reservou um spot a menos de `OWN_SPACING` (8 tiles).
    pub fn owner_near(
        &self,
        spot: SpotCoord,
        except_char: &str,
        now_mono_s: u64,
    ) -> Option<(String, String)> {
        for (machine, name, home, active) in &self.members {
            if name.eq_ignore_ascii_case(except_char) {
                continue;
            }
            if [*home, *active]
                .into_iter()
                .flatten()
                .any(|s| s.is_near(spot, OWN_SPACING))
            {
                return Some((machine.clone(), name.clone()));
            }
        }
        for lease in &self.leases {
            if lease.expires_at_mono_s > now_mono_s
                && !lease.char_name.eq_ignore_ascii_case(except_char)
                && lease.spot.is_near(spot, OWN_SPACING)
            {
                return Some((lease.machine.clone(), lease.char_name.clone()));
            }
        }
        None
    }

    /// Tenta reservar exclusivamente um spot por 90 segundos.
    pub fn try_reserve(
        &mut self,
        machine: &str,
        char_name: &str,
        spot: SpotCoord,
        now_mono_s: u64,
    ) -> Result<(), String> {
        self.leases.retain(|l| l.expires_at_mono_s > now_mono_s);
        if !spot.is_valid() {
            return Err("Coordenada de spot inválida".into());
        }
        if let Some((_, owner)) = self.owner_near(spot, char_name, now_mono_s) {
            return Err(format!(
                "Spot {} já pertence ou está reservado próximo a {}",
                spot.label(),
                owner
            ));
        }
        self.leases
            .retain(|l| !l.char_name.eq_ignore_ascii_case(char_name));
        self.leases.push(SpotLease {
            machine: machine.to_string(),
            char_name: char_name.to_string(),
            spot,
            expires_at_mono_s: now_mono_s + 90,
        });
        Ok(())
    }

    /// Avalia o status verdadeiro de ocupação de um spot.
    /// Se não houver observação recente (`<= max_obs_age_s`), retorna `Unobserved` (nunca `ConfirmedFree`).
    pub fn occupancy_status(
        &self,
        spot: SpotCoord,
        now_mono_s: u64,
        max_obs_age_s: u64,
    ) -> SpotOccupancyStatus {
        if let Some((machine, char_name)) = self.owner_near(spot, "", now_mono_s) {
            return SpotOccupancyStatus::OwnedByUs { char_name, machine };
        }
        if let Some(obs) = self.observations.iter().find(|o| {
            o.spot.is_near(spot, ARRIVAL_RADIUS)
                && now_mono_s.saturating_sub(o.observed_at_mono_s) <= max_obs_age_s
        }) {
            let age_s = now_mono_s.saturating_sub(obs.observed_at_mono_s);
            if obs.foreign_players.is_empty() {
                return SpotOccupancyStatus::ConfirmedFree {
                    observed_by: obs.observed_by.clone(),
                    age_s,
                };
            } else {
                return SpotOccupancyStatus::OccupiedByForeign {
                    players: obs.foreign_players.clone(),
                    observed_by: obs.observed_by.clone(),
                    age_s,
                };
            }
        }
        SpotOccupancyStatus::Unobserved
    }

    /// Calcula a ordem de busca de spots:
    /// 1. `home` (F1) e demais spots habilitados do mapa original;
    /// 2. `secondary_worlds` (ex.: Arena/Stadium e demais mapas permitidos).
    pub fn candidate_search_order(
        &self,
        home: SpotCoord,
        policy: &FallbackPolicy,
    ) -> Vec<SpotCoord> {
        let mut order = vec![home];
        if policy.spot_locked {
            return order;
        }
        let mut worlds = vec![home.world];
        if policy.enabled {
            for &w in &policy.secondary_worlds {
                if !worlds.contains(&w) && w != WorldId::Lorencia {
                    worlds.push(w);
                }
            }
        }
        for w in worlds {
            for entry in self
                .catalog
                .iter()
                .filter(|c| c.enabled && c.spot.world == w)
            {
                if !order.iter().any(|s| s.is_near(entry.spot, ARRIVAL_RADIUS)) {
                    order.push(entry.spot);
                }
            }
        }
        order
    }

    /// Calcula a ordem de busca de spots priorizando os mais próximos a pé da Safe Zone / entrada do mapa
    /// (usando `walking_dist(spot)` calculado no `TerrainGrid` real `EncTerrainN.att`),
    /// seguidos pelo spot alternativo ativo atual (`current_alt`) e pelos mapas secundários configurados.
    pub fn ranked_search_order(
        &self,
        home: SpotCoord,
        current_alt: Option<SpotCoord>,
        policy: &FallbackPolicy,
        walking_dist: &dyn Fn(SpotCoord) -> Option<u32>,
    ) -> Vec<SpotCoord> {
        let mut order = vec![home];
        if policy.spot_locked {
            return order;
        }
        let mut primary_candidates: Vec<&CatalogEntry> = self
            .catalog
            .iter()
            .filter(|c| c.enabled && c.spot.world == home.world)
            .collect();
        primary_candidates.sort_by_key(|c| {
            (
                walking_dist(c.spot).unwrap_or(u32::MAX),
                c.priority,
                c.spot.x,
                c.spot.y,
            )
        });
        for c in primary_candidates {
            if !order.iter().any(|s| s.is_near(c.spot, ARRIVAL_RADIUS)) {
                order.push(c.spot);
            }
        }

        if policy.enabled {
            if let Some(alt) =
                current_alt.filter(|a| a.world != home.world && a.world != WorldId::Lorencia)
            {
                if !order.iter().any(|s| s.is_near(alt, ARRIVAL_RADIUS)) {
                    order.push(alt);
                }
            }
            for &w in &policy.secondary_worlds {
                if w == home.world || w == WorldId::Lorencia {
                    continue;
                }
                let mut sec: Vec<&CatalogEntry> = self
                    .catalog
                    .iter()
                    .filter(|c| c.enabled && c.spot.world == w)
                    .collect();
                sec.sort_by_key(|c| {
                    (
                        walking_dist(c.spot).unwrap_or(u32::MAX),
                        c.priority,
                        c.spot.x,
                        c.spot.y,
                    )
                });
                for c in sec {
                    if !order.iter().any(|s| s.is_near(c.spot, ARRIVAL_RADIUS)) {
                        order.push(c.spot);
                    }
                }
            }
        }
        order
    }
}

/// Verifica jogadores externos próximos a `spot` a partir de uma leitura real de entidades.
/// Se `entities` for `None` (leitura indisponível), retorna `Err` — ausência de leitura nunca é spot livre!
pub fn inspect_foreign_occupants<'a>(
    entities: Option<&'a [NearbyEntity]>,
    spot: SpotCoord,
    self_char: &str,
    is_friendly: impl Fn(&str) -> bool,
) -> Result<Vec<&'a str>, &'static str> {
    let list = entities.ok_or("Leitura de jogadores próximos indisponível")?;
    Ok(list
        .iter()
        .filter(|e| {
            e.kind == EntityKind::Player
                && !e.name.eq_ignore_ascii_case(self_char)
                && !is_friendly(&e.name)
                && (e.x - spot.x).abs().max((e.y - spot.y).abs()) <= OCCUPIED_RADIUS
        })
        .map(|e| e.name.as_str())
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn enforces_own_spacing_and_never_treats_unobserved_as_free() {
        let mut dir = SpotDirectory::default();
        let s1 = SpotCoord::new(WorldId::Arena, 48, 57);
        let s2_too_close = SpotCoord::new(WorldId::Arena, 53, 57); // 5 tiles (< OWN_SPACING=8)
        let s3_far = SpotCoord::new(WorldId::Arena, 47, 70); // 13 tiles

        dir.register_member("PC1", "xWizard", None, Some(s1));
        assert!(dir.try_reserve("PC2", "0MAGIC", s2_too_close, 100).is_err());
        assert!(dir.try_reserve("PC2", "0MAGIC", s3_far, 100).is_ok());

        let unobserved = SpotCoord::new(WorldId::Kanturu, 200, 200);
        assert_eq!(
            dir.occupancy_status(unobserved, 100, 30),
            SpotOccupancyStatus::Unobserved
        );
        assert!(inspect_foreign_occupants(None, unobserved, "x9901", |_| false).is_err());
    }

    #[test]
    fn catalog_rejects_lorencia_and_close_spots_and_ranks_by_walking_distance() {
        let mut dir = SpotDirectory::default();
        // Lorencia é cidade: proibido catalogar spot de farm
        assert!(dir
            .try_add_catalog_spot(SpotCoord::new(WorldId::Lorencia, 130, 125), 0, "PC1")
            .is_err());

        // Primeiro spot em Kanturu (90, 66)
        assert_eq!(
            dir.try_add_catalog_spot(SpotCoord::new(WorldId::Kanturu, 90, 66), 0, "PC1")
                .unwrap(),
            1
        );
        // Spot a 1 tile (91, 66) é considerado o mesmo spot (ARRIVAL_RADIUS = 2): atualiza/mantém #1
        assert_eq!(
            dir.try_add_catalog_spot(SpotCoord::new(WorldId::Kanturu, 91, 66), 1, "PC1")
                .unwrap(),
            1
        );
        // Spot a 5 tiles (95, 66) está dentro de OWN_SPACING (8 tiles): recusado para evitar KS
        assert!(dir
            .try_add_catalog_spot(SpotCoord::new(WorldId::Kanturu, 95, 66), 2, "PC1")
            .is_err());

        // Outros spots bem espaçados em Kanturu e Arena
        dir.try_add_catalog_spot(SpotCoord::new(WorldId::Kanturu, 40, 232), 5, "PC1")
            .unwrap();
        dir.try_add_catalog_spot(SpotCoord::new(WorldId::Arena, 47, 70), 0, "PC2")
            .unwrap();

        let home = SpotCoord::new(WorldId::Kanturu, 160, 197);
        let policy = FallbackPolicy::default();
        // Distância simulada: (40,232) está a 13 passos da safe; (90,66) está a 433 passos
        let order = dir.ranked_search_order(home, None, &policy, &|sp| match (sp.x, sp.y) {
            (40, 232) => Some(13),
            (90, 66) => Some(433),
            _ => Some(50),
        });
        assert_eq!(
            order,
            vec![
                home,
                SpotCoord::new(WorldId::Kanturu, 40, 232),
                SpotCoord::new(WorldId::Kanturu, 90, 66),
                SpotCoord::new(WorldId::Arena, 47, 70),
            ]
        );

        dir.add_friendly_char("Amigo1").unwrap();
        assert!(dir.is_friendly("amigo1"));
        assert!(dir.remove_friendly_char("AMIGO1"));
        assert!(!dir.is_friendly("Amigo1"));
    }
}
