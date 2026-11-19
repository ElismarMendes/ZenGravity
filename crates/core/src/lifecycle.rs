//! Estados operacionais verdadeiros e metadados de leitura de cada métrica.
//!
//! Nenhuma métrica no ZenGravity é tratada como valor absoluto sem contexto:
//! sabemos de onde veio (`MetricSource`), quando foi lida (`observed_at_mono_ms`)
//! e qual a sua confiabilidade (`Confidence`).

use crate::clock::{MonotonicMs, UtcTimestampSecs};

/// Origem da leitura de uma métrica do jogo.
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[cfg_attr(feature = "serde", serde(rename_all = "snake_case"))]
pub enum MetricSource {
    /// Endereço de memória RAM travado e validado por conferência cruzada.
    RamLocked,
    /// OCR em recorte específico da janela (ex.: Zen no inventário).
    ScreenOcr,
    /// Modelo determinístico de pixels na tela (ex.: coordenadas, mapa, ícone Helper, barra Raven).
    ScreenPixelModel,
    /// Tabela do sistema operacional (título da janela, processo, tabela TCP).
    OsProcessTable,
    /// Gerado pelo simulador determinístico (`zengravity-simulator`).
    Simulator,
    /// Fonte desconhecida ou leitura ainda não realizada.
    Unknown,
}

impl MetricSource {
    pub fn label_pt(self) -> &'static str {
        match self {
            MetricSource::RamLocked => "RAM Travada",
            MetricSource::ScreenOcr => "OCR Recortado",
            MetricSource::ScreenPixelModel => "Tela (Pixels)",
            MetricSource::OsProcessTable => "Processo/SO",
            MetricSource::Simulator => "Simulador",
            MetricSource::Unknown => "Desconhecida",
        }
    }
}

/// Grau de confiabilidade da métrica no momento atual.
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[cfg_attr(feature = "serde", serde(rename_all = "snake_case"))]
pub enum Confidence {
    /// Confirmado por fonte primária recente (ex.: RAM travada conferida com tela).
    Verified,
    /// Leitura provisória (ex.: filtrando candidatos de RAM ou apenas 1 amostra).
    Provisional,
    /// Leitura desatualizada (passou do limite de frescor `stale_after_ms`).
    Stale,
    /// Dado indisponível (janela minimizada, trava aberta ou sem observador).
    Unavailable,
}

impl Confidence {
    pub fn label_pt(self) -> &'static str {
        match self {
            Confidence::Verified => "Verificada",
            Confidence::Provisional => "Provisória",
            Confidence::Stale => "Desatualizada",
            Confidence::Unavailable => "Indisponível",
        }
    }
}

/// Envelope de leitura para qualquer métrica telemetrica.
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MetricReading<T> {
    pub value: Option<T>,
    pub source: MetricSource,
    pub observed_at_mono_ms: MonotonicMs,
    pub observed_at_utc_s: UtcTimestampSecs,
    pub confidence: Confidence,
}

impl<T> MetricReading<T> {
    pub fn verified(
        value: T,
        source: MetricSource,
        mono_ms: MonotonicMs,
        utc_s: UtcTimestampSecs,
    ) -> Self {
        Self {
            value: Some(value),
            source,
            observed_at_mono_ms: mono_ms,
            observed_at_utc_s: utc_s,
            confidence: Confidence::Verified,
        }
    }

    pub fn provisional(
        value: T,
        source: MetricSource,
        mono_ms: MonotonicMs,
        utc_s: UtcTimestampSecs,
    ) -> Self {
        Self {
            value: Some(value),
            source,
            observed_at_mono_ms: mono_ms,
            observed_at_utc_s: utc_s,
            confidence: Confidence::Provisional,
        }
    }

    pub fn unavailable(mono_ms: MonotonicMs, utc_s: UtcTimestampSecs) -> Self {
        Self {
            value: None,
            source: MetricSource::Unknown,
            observed_at_mono_ms: mono_ms,
            observed_at_utc_s: utc_s,
            confidence: Confidence::Unavailable,
        }
    }

    /// Idade da leitura em segundos em relação a um relógio monotônico atual.
    pub fn age_secs(&self, now_mono_ms: MonotonicMs) -> u64 {
        now_mono_ms.saturating_sub(self.observed_at_mono_ms) / 1000
    }

    /// Retorna a confiabilidade efetiva considerando o tempo decorrido (`max_age_ms`).
    pub fn effective_confidence(&self, now_mono_ms: MonotonicMs, max_age_ms: u64) -> Confidence {
        if self.value.is_none() || self.confidence == Confidence::Unavailable {
            return Confidence::Unavailable;
        }
        if now_mono_ms.saturating_sub(self.observed_at_mono_ms) > max_age_ms {
            Confidence::Stale
        } else {
            self.confidence
        }
    }
}

/// Estados operacionais verdadeiros de um personagem no ZenGravity.
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[cfg_attr(feature = "serde", serde(rename_all = "snake_case"))]
pub enum CharLifecycleState {
    /// Leitura desatualizada: agente sem reportar ou dados expirados.
    Stale,
    /// Queda confirmada da sessão do jogo (TCP ausente + números congelados).
    Disconnected,
    /// Personagem morto (HP = 0 confirmado ou PK recente sem renascimento).
    Dead,
    /// Automação ou leitura pausada pelo operador (F8 / Parada de Emergência / Pausa individual).
    Paused,
    /// Em rotina ativa de recuperação, retorno ao spot ou busca manual de vaga.
    Recovering,
    /// Conectado, vivo, porém sem ganho de Zen acima do limiar ou Helper desligado fora de rotina.
    Stopped,
    /// No spot ativo, Helper ligado e Zen subindo normalmente.
    Farming,
    /// Conectado e reportando normalmente (ex.: em zona segura ou sem spot definido).
    Online,
    /// Estado desconhecido (processo aberto, mas sem leituras suficientes de login/memória/tela).
    Unknown,
}

impl CharLifecycleState {
    pub fn label_pt(self) -> &'static str {
        match self {
            CharLifecycleState::Stale => "LEITURA DESATUALIZADA",
            CharLifecycleState::Disconnected => "DESCONECTADO (DC)",
            CharLifecycleState::Dead => "MORTO",
            CharLifecycleState::Paused => "PAUSADO",
            CharLifecycleState::Recovering => "RECUPERANDO",
            CharLifecycleState::Stopped => "PARADO",
            CharLifecycleState::Farming => "FARMANDO",
            CharLifecycleState::Online => "ONLINE",
            CharLifecycleState::Unknown => "DESCONHECIDO",
        }
    }
}

/// Parâmetros puros para classificar o estado verdadeiro do personagem.
#[derive(Debug, Clone, Copy)]
pub struct StateEvalInput {
    pub report_age_ms: u64,
    pub stale_threshold_ms: u64,
    pub logged_in: bool,
    pub disconnected: bool,
    pub is_dead: bool,
    pub is_paused: bool,
    pub is_recovering: bool,
    pub helper_on: Option<bool>,
    pub at_active_spot: bool,
    pub zen_idle_secs: Option<u64>,
    pub stopped_threshold_s: u64,
    pub at_zen_limit: bool,
}

/// Determina o `CharLifecycleState` sem inventar informações quando dados faltam.
pub fn classify_character_state(input: &StateEvalInput) -> CharLifecycleState {
    if input.report_age_ms > input.stale_threshold_ms {
        return CharLifecycleState::Stale;
    }
    if input.disconnected {
        return CharLifecycleState::Disconnected;
    }
    if !input.logged_in {
        return CharLifecycleState::Unknown;
    }
    if input.is_dead {
        return CharLifecycleState::Dead;
    }
    if input.is_paused {
        return CharLifecycleState::Paused;
    }
    if input.is_recovering {
        return CharLifecycleState::Recovering;
    }
    if input.at_active_spot && input.helper_on == Some(true) {
        if !input.at_zen_limit {
            if let Some(idle) = input.zen_idle_secs {
                if idle >= input.stopped_threshold_s {
                    return CharLifecycleState::Stopped;
                }
            }
        }
        return CharLifecycleState::Farming;
    }
    if input.helper_on == Some(false) && input.at_active_spot {
        return CharLifecycleState::Stopped;
    }
    if let Some(idle) = input.zen_idle_secs {
        if idle >= input.stopped_threshold_s && !input.at_zen_limit {
            return CharLifecycleState::Stopped;
        }
    }
    CharLifecycleState::Online
}

#[cfg(test)]
mod tests {
    use super::*;

    fn base_input() -> StateEvalInput {
        StateEvalInput {
            report_age_ms: 1_000,
            stale_threshold_ms: 15_000,
            logged_in: true,
            disconnected: false,
            is_dead: false,
            is_paused: false,
            is_recovering: false,
            helper_on: Some(true),
            at_active_spot: true,
            zen_idle_secs: Some(5),
            stopped_threshold_s: 180,
            at_zen_limit: false,
        }
    }

    #[test]
    fn stale_overrides_everything_else() {
        let mut inp = base_input();
        inp.report_age_ms = 20_000;
        inp.is_dead = true;
        assert_eq!(classify_character_state(&inp), CharLifecycleState::Stale);
    }

    #[test]
    fn distinguishes_all_nine_states_accurately() {
        let inp = base_input();
        assert_eq!(classify_character_state(&inp), CharLifecycleState::Farming);

        let mut stopped = inp;
        stopped.zen_idle_secs = Some(185);
        assert_eq!(
            classify_character_state(&stopped),
            CharLifecycleState::Stopped
        );

        let mut rec = inp;
        rec.is_recovering = true;
        assert_eq!(
            classify_character_state(&rec),
            CharLifecycleState::Recovering
        );

        let mut paused = rec;
        paused.is_paused = true;
        assert_eq!(
            classify_character_state(&paused),
            CharLifecycleState::Paused
        );

        let mut dead = paused;
        dead.is_dead = true;
        assert_eq!(classify_character_state(&dead), CharLifecycleState::Dead);

        let mut dc = dead;
        dc.disconnected = true;
        assert_eq!(
            classify_character_state(&dc),
            CharLifecycleState::Disconnected
        );

        let mut unk = inp;
        unk.logged_in = false;
        assert_eq!(classify_character_state(&unk), CharLifecycleState::Unknown);

        let mut online = inp;
        online.at_active_spot = false;
        online.helper_on = None;
        online.zen_idle_secs = None;
        assert_eq!(
            classify_character_state(&online),
            CharLifecycleState::Online
        );
    }
}
