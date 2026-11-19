//! Parser e deduplicador de eventos de chat (`[PK]` e `[PickItem]`).

use crate::sanitize_text;
use std::collections::HashMap;

#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[cfg_attr(feature = "serde", serde(rename_all = "snake_case"))]
pub enum KillerKind {
    Player,
    Monster,
}

#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[cfg_attr(feature = "serde", serde(rename_all = "snake_case"))]
pub enum JewelKind {
    Bless,
    Soul,
    Chaos,
    Life,
    Creation,
    Guardian,
    EmperorsFortune,
}

#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[derive(Debug, Clone, PartialEq, Eq)]
#[cfg_attr(feature = "serde", serde(tag = "kind", rename_all = "snake_case"))]
pub enum ChatEvent {
    Pk {
        killer: String,
        killer_kind: KillerKind,
        victim: String,
    },
    Pick {
        char_name: String,
        jewel: JewelKind,
    },
    ServerNotice {
        text: String,
    },
}

const ONE_WORD_MONSTERS: &[&str] = &[
    "Spider",
    "Lich",
    "Balrog",
    "Hydra",
    "Yeti",
    "Gorgon",
    "Medusa",
    "Kundun",
    "Valkyrie",
    "Worm",
    "Bahamut",
    "Genocider",
];

pub fn is_valid_player_name(s: &str) -> bool {
    let n = s.chars().count();
    (1..=16).contains(&n)
        && !s
            .chars()
            .any(|c| c.is_whitespace() || c == '[' || c == ']' || c == ':')
}

pub fn parse_chat_line(line: &str) -> Option<ChatEvent> {
    let line: String = line.chars().take(160).filter(|c| !c.is_control()).collect();
    let line = line.trim();

    if let Some(rest) = line.strip_prefix("[PK]") {
        let rest = rest.trim_start().strip_prefix(':')?.trim();
        let (killer, victim) = rest.rsplit_once(" killed ")?;
        let killer = sanitize_text(killer, 32);
        let victim = victim.trim();
        if killer.is_empty() || !is_valid_player_name(victim) {
            return None;
        }
        let killer_kind = if killer.contains(' ')
            || ONE_WORD_MONSTERS
                .iter()
                .any(|m| m.eq_ignore_ascii_case(&killer))
        {
            KillerKind::Monster
        } else {
            KillerKind::Player
        };
        return Some(ChatEvent::Pk {
            killer,
            killer_kind,
            victim: victim.to_string(),
        });
    }

    let (name, rest) = line.split_once(" : ")?;
    let item = rest.trim().strip_prefix("[PickItem]")?.trim();
    let name = name.trim();
    if !is_valid_player_name(name) || item.to_ascii_lowercase().starts_with("enchanted ") {
        return None;
    }
    let jewel = match item.to_ascii_lowercase().as_str() {
        "jewel of bless" => JewelKind::Bless,
        "jewel of soul" => JewelKind::Soul,
        "jewel of chaos" => JewelKind::Chaos,
        "jewel of life" => JewelKind::Life,
        "jewel of creation" => JewelKind::Creation,
        "jewel of guardian" => JewelKind::Guardian,
        "fortuna do imperador" | "emperor's fortune" => JewelKind::EmperorsFortune,
        _ => return None,
    };
    Some(ChatEvent::Pick {
        char_name: name.to_string(),
        jewel,
    })
}

/// Deduplicador de eventos de PK vistos por múltiplos PCs/clientes ao mesmo tempo.
#[derive(Debug, Clone)]
pub struct PkDeduper {
    window_s: u64,
    seen: HashMap<(String, String), u64>,
}

impl PkDeduper {
    pub fn new(window_s: u64) -> Self {
        Self {
            window_s,
            seen: HashMap::new(),
        }
    }

    pub fn accept(&mut self, killer: &str, victim: &str, now_mono_s: u64) -> bool {
        self.seen
            .retain(|_, &mut t| now_mono_s.saturating_sub(t) < self.window_s);
        let key = (killer.to_ascii_lowercase(), victim.to_ascii_lowercase());
        if let std::collections::hash_map::Entry::Vacant(e) = self.seen.entry(key) {
            e.insert(now_mono_s);
            true
        } else {
            false
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_pk_and_deduplicates_within_window() {
        let ev = parse_chat_line("[PK] : Genocider killed LOBOLIXO").unwrap();
        assert_eq!(
            ev,
            ChatEvent::Pk {
                killer: "Genocider".into(),
                killer_kind: KillerKind::Monster,
                victim: "LOBOLIXO".into(),
            }
        );

        let mut dedup = PkDeduper::new(60);
        assert!(dedup.accept("KceTinHoO", "ReDMooN", 100));
        assert!(!dedup.accept("kcetinhoo", "redmoon", 120));
        assert!(dedup.accept("KceTinHoO", "ReDMooN", 170));
    }
}
