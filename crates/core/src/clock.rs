//! Abstração de tempo monotônico e tempo UTC de auditoria.
//!
//! No legado (`ZenMonitor`), mudanças no relógio do Windows causaram paradas reais
//! de 46 minutos porque decisões de espera usavam hora de parede. No ZenGravity:
//! - `MonotonicMs`: usado para todo timeout, cooldown, cálculo de taxa e detecção de `stale`.
//! - `UtcTimestampSecs`: usado exclusivamente para registro de auditoria, banco e exibição.

/// Tempo monotônico em milissegundos desde o início do processo/sessão.
pub type MonotonicMs = u64;

/// Timestamp Unix UTC em segundos (apenas para auditoria e histórico).
pub type UtcTimestampSecs = u64;

/// Fonte de tempo injetável para permitir testes 100% determinísticos sem `sleep`.
pub trait Clock: Send + Sync {
    /// Milissegundos monotônicos (nunca anda para trás).
    fn monotonic_ms(&self) -> MonotonicMs;

    /// Segundos monotônicos convenientes (`monotonic_ms() / 1000`).
    fn monotonic_secs(&self) -> u64 {
        self.monotonic_ms() / 1000
    }

    /// Timestamp Unix UTC em segundos para carimbo de auditoria.
    fn utc_secs(&self) -> UtcTimestampSecs;
}
