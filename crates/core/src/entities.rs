//! Entidades observadas na memória/tela perto de um personagem nosso.
//!
//! Regra inegociável: o cliente só conhece entidades dentro do raio de visão
//! (`OCCUPIED_RADIUS` / viewport). Jamais inferimos que o resto do mapa está vazio.

use crate::clock::MonotonicMs;

/// Tamanho de cada entrada na lista de personagens/monstros/NPCs na memória do cliente.
pub const OBJECT_STRIDE: usize = 0x598;
/// Offset do identificador de classe/monstro/NPC (`u16` little-endian) a partir do nome.
pub const OFF_CLASS: usize = 0x4c;
/// Offset da flag de entrada viva/ativa na tela (`1` = vivo/visível).
pub const OFF_LIVE: usize = 0x2d4;
/// Offset do tipo de objeto (`1` = Jogador, `2` = Monstro, `4` = NPC).
pub const OFF_KIND: usize = 0x2e8;
/// Offset da posição no mundo (`f32 x, f32 y`, onde `tile = valor / 100.0`).
pub const OFF_POS: usize = 0x390;

#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[cfg_attr(feature = "serde", serde(rename_all = "snake_case"))]
pub enum EntityKind {
    /// Outro jogador (nosso grupo ou externo).
    Player,
    /// Monstro comum de farm.
    Monster,
    /// NPC da cidade ou guarda (ex.: Safety Guardian, Lugard, Trainer, Chaos Goblin).
    Npc,
    /// Golden Monster (alvo de invasão, ex.: Golden Budge Dragon, Golden Titan).
    GoldenMonster,
    /// Chefe Dourado letal (ex.: Great Golden Dragon, classe 501 — fugir/evitar).
    GoldenBoss,
}

impl EntityKind {
    pub fn label_pt(self) -> &'static str {
        match self {
            EntityKind::Player => "Jogador",
            EntityKind::Monster => "Monstro",
            EntityKind::Npc => "NPC",
            EntityKind::GoldenMonster => "Golden (Alvo)",
            EntityKind::GoldenBoss => "Golden Boss (Perigo!)",
        }
    }
}

#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NearbyEntity {
    pub kind: EntityKind,
    pub name: String,
    pub class_id: u16,
    pub x: i32,
    pub y: i32,
    /// Quem avistou esta entidade.
    pub observed_by: String,
    /// Quando foi vista (monotônico, para calcular idade/frescor da detecção no Mapa 2D).
    pub observed_at_mono_ms: MonotonicMs,
}

impl NearbyEntity {
    pub fn classify_monster(name: &str, class_id: u16) -> EntityKind {
        let lower = name.to_ascii_lowercase();
        if class_id == 501 || lower.starts_with("great golden") {
            EntityKind::GoldenBoss
        } else if lower.contains("golden") {
            EntityKind::GoldenMonster
        } else {
            EntityKind::Monster
        }
    }

    pub fn age_secs(&self, now_mono_ms: MonotonicMs) -> u64 {
        now_mono_ms.saturating_sub(self.observed_at_mono_ms) / 1000
    }

    /// Faz o parse de uma única entrada de `OBJECT_STRIDE` (`0x598` bytes) lida da memória (somente leitura).
    pub fn parse_raw_entry(
        entry_bytes: &[u8],
        observed_by: &str,
        observed_at_mono_ms: MonotonicMs,
    ) -> Option<Self> {
        if entry_bytes.len() < OFF_POS + 8 || entry_bytes[OFF_LIVE] != 1 {
            return None;
        }
        let raw_kind = entry_bytes[OFF_KIND];
        let name_end = entry_bytes[..32].iter().position(|&c| c == 0)?;
        if name_end < 2
            || !entry_bytes[..name_end]
                .iter()
                .all(|&c| (0x20..0x7f).contains(&c))
        {
            return None;
        }
        let name = String::from_utf8_lossy(&entry_bytes[..name_end]).into_owned();
        let class_id = u16::from_le_bytes([entry_bytes[OFF_CLASS], entry_bytes[OFF_CLASS + 1]]);
        let kind = match raw_kind {
            1 => EntityKind::Player,
            2 => Self::classify_monster(&name, class_id),
            4 => EntityKind::Npc,
            _ => return None,
        };
        let read_f32 = |o: usize| {
            f32::from_le_bytes([
                entry_bytes[o],
                entry_bytes[o + 1],
                entry_bytes[o + 2],
                entry_bytes[o + 3],
            ])
        };
        let (wx, wy) = (read_f32(OFF_POS), read_f32(OFF_POS + 4));
        if !(wx.is_finite()
            && wy.is_finite()
            && (100.0..25600.0).contains(&wx)
            && (100.0..25600.0).contains(&wy))
        {
            return None;
        }
        Some(NearbyEntity {
            kind,
            name,
            class_id,
            x: (wx / 100.0) as i32,
            y: (wy / 100.0) as i32,
            observed_by: observed_by.to_string(),
            observed_at_mono_ms,
        })
    }

    /// Extrai todas as entidades vivas de um bloco de memória alinhado pela âncora `anchor`.
    pub fn parse_raw_block(
        buf: &[u8],
        anchor: usize,
        observed_by: &str,
        observed_at_mono_ms: MonotonicMs,
    ) -> Vec<Self> {
        let mut out = Vec::new();
        let mut offset = anchor % OBJECT_STRIDE;
        while offset + OBJECT_STRIDE <= buf.len() {
            if let Some(ent) = Self::parse_raw_entry(
                &buf[offset..offset + OBJECT_STRIDE],
                observed_by,
                observed_at_mono_ms,
            ) {
                out.push(ent);
            }
            offset += OBJECT_STRIDE;
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn distinguishes_golden_target_from_lethal_golden_boss() {
        assert_eq!(
            NearbyEntity::classify_monster("Golden Budge Dragon", 43),
            EntityKind::GoldenMonster
        );
        assert_eq!(
            NearbyEntity::classify_monster("Great Golden Dragon", 501),
            EntityKind::GoldenBoss
        );
        assert_eq!(
            NearbyEntity::classify_monster("Satyros", 353),
            EntityKind::Monster
        );
    }
}
