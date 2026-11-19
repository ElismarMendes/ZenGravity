//! Núcleo puro de domínio do ZenGravity.
//!
//! Regra de ouro: este crate não faz I/O, não acessa rede, banco de dados,
//! APIs do Windows nem relógio global do sistema. Todo tempo entra por
//! parâmetros explícitos (`MonotonicMs` para lógica/cooldown/stale e
//! `UtcTimestampSecs` para auditoria), tornando 100% das regras determinísticas.

pub mod alerts;
pub mod chat;
pub mod clock;
pub mod entities;
pub mod game_title;
pub mod lifecycle;
pub mod map;
pub mod spots;
pub mod telemetry;

/// Limite máximo de Zen no inventário do personagem no MU Online (2 bilhões).
pub const ZEN_LIMIT: u64 = 2_000_000_000;

/// Subida de Zen num único ciclo maior que 30 milhões indica retirada de baú ou trade, não farm.
pub const MAX_FARM_STEP: u64 = 30_000_000;

/// Raio máximo (em tiles) para considerar o personagem posicionado no spot de farm.
pub const ARRIVAL_RADIUS: i32 = 2;

/// Espaçamento mínimo (em tiles) entre spots atribuídos a personagens do nosso próprio grupo
/// (o MU Helper tem alcance 6; dois personagens a menos de 8 tiles disputam mobs).
pub const OWN_SPACING: i32 = 8;

/// Raio de ocupação e de visão confiável de objetos próximos (em tiles).
pub const OCCUPIED_RADIUS: i32 = 12;

/// Remove caracteres de controle e limita o comprimento máximo de textos vindos de fora.
pub fn sanitize_text(input: &str, max_chars: usize) -> String {
    input
        .chars()
        .filter(|c| !c.is_control())
        .take(max_chars)
        .collect::<String>()
        .trim()
        .to_string()
}

/// Formata um valor de Zen com separadores de milhar brasileiros (ex.: `1.450.200.000`).
pub fn format_zen(zen: u64) -> String {
    let s = zen.to_string();
    let mut out = String::with_capacity(s.len() + s.len() / 3);
    for (idx, ch) in s.chars().rev().enumerate() {
        if idx > 0 && idx % 3 == 0 {
            out.push('.');
        }
        out.push(ch);
    }
    out.chars().rev().collect()
}

/// Formata Zen de maneira compacta para badges e gráficos (ex.: `1,45B`, `320,5M`, `850K`).
pub fn format_zen_compact(zen: f64) -> String {
    if !zen.is_finite() || zen <= 0.0 {
        return "0".to_string();
    }
    if zen >= 1_000_000_000.0 {
        format!("{:.2}B", zen / 1_000_000_000.0).replace('.', ",")
    } else if zen >= 1_000_000.0 {
        format!("{:.1}M", zen / 1_000_000.0).replace('.', ",")
    } else if zen >= 1_000.0 {
        format!("{:.0}K", zen / 1_000.0)
    } else {
        format!("{:.0}", zen)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sanitize_removes_control_chars_and_truncates() {
        assert_eq!(sanitize_text("  ReDMooN\u{0}\n ", 16), "ReDMooN");
        assert_eq!(sanitize_text("PersonagemMuitoLongo", 6), "Person");
    }

    #[test]
    fn formats_zen_human_readable() {
        assert_eq!(format_zen(0), "0");
        assert_eq!(format_zen(999), "999");
        assert_eq!(format_zen(1_234_567_890), "1.234.567.890");
        assert_eq!(format_zen_compact(1_450_000_000.0), "1,45B");
        assert_eq!(format_zen_compact(245_600_000.0), "245,6M");
    }
}
