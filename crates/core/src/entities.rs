//! Entidades observadas na memória/tela perto de um personagem nosso.
//!
//! Regra inegociável: o cliente só conhece entidades dentro do raio de visão
//! (`OCCUPIED_RADIUS` / viewport). Jamais inferimos que o resto do mapa está vazio.

use crate::clock::MonotonicMs;

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
