//! Arquitetura base e Leitores Somente-Leitura do Agente Windows (`zengravity-agent`).
//!
//! Implementa:
//! - `image`: quadro único `FrameImage` capturado por ciclo e recortado em memória.
//! - `terrain_loader`: leitura somente-leitura de `Data\WorldN\EncTerrainN.att` do cliente instalado.
//! - `win_reader` (Windows): enumeração de janelas (`The Classic MU Season 2`), `PrintWindow`,
//!   `ReadProcessMemory` somente-leitura (`PROCESS_VM_READ`), lista de objetos `0x598` e tabela TCP.
//! - `live_collector`: ciclo de telemetria passiva unificando captura única, modelos de pixel e
//!   travas de RAM (`LockMachine` + `HpLock` + `DeathDetector`).
//! - `InputSessionScheduler`: garantia de 1 única sessão física de teclado/mouse por PC e F8.

pub mod image;
pub mod live_collector;
pub mod terrain_loader;
#[cfg(windows)]
pub mod win_reader;

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use thiserror::Error;

/// Prioridade de execução de uma rotina que precisa do teclado/mouse do PC.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum RoutinePriority {
    /// Conveniência (Fortuna do Imperador, Depósito de Zen, Restauração de Raven preventiva).
    Convenience = 1,
    /// Retorno automático padrão após desconexão ou deslocamento.
    StandardRecovery = 2,
    /// Busca manual solicitada explicitamente pelo operador ("Procurar spot agora").
    ManualOperatorAction = 3,
    /// Sobrevivência imediata (reativar Helper após morte/risco no spot).
    Survival = 4,
}

#[derive(Debug, Error, PartialEq, Eq)]
pub enum InputScheduleError {
    #[error("Parada de emergência ativa (F8 ou botão de emergência no painel)")]
    EmergencyStopActive,
    #[error("Automação deste PC está desabilitada pelo operador")]
    AutomationDisabled,
    #[error("Sessão de entrada já ocupada pelo personagem '{active_char}' ({routine})")]
    SessionBusy {
        active_char: String,
        routine: String,
    },
    #[error("Sessão de entrada expirada ou cancelada por época de emergência")]
    SessionCancelled,
}

#[derive(Debug, Clone)]
pub struct ActiveSessionInfo {
    pub char_name: String,
    pub routine_name: String,
    pub priority: RoutinePriority,
    pub epoch: u64,
}

/// Coordenador seguro de sessão única de entrada física por PC.
#[derive(Debug, Clone)]
pub struct InputSessionScheduler {
    emergency_stop: Arc<AtomicBool>,
    automation_enabled: Arc<AtomicBool>,
    cancel_epoch: Arc<AtomicU64>,
    active_session: Arc<Mutex<Option<ActiveSessionInfo>>>,
}

impl Default for InputSessionScheduler {
    fn default() -> Self {
        Self::new(true)
    }
}

impl InputSessionScheduler {
    pub fn new(automation_enabled: bool) -> Self {
        Self {
            emergency_stop: Arc::new(AtomicBool::new(false)),
            automation_enabled: Arc::new(AtomicBool::new(automation_enabled)),
            cancel_epoch: Arc::new(AtomicU64::new(1)),
            active_session: Arc::new(Mutex::new(None)),
        }
    }

    /// Aciona imediatamente a parada de emergência (equivalente ao F8 ou botão de pânico).
    /// Incrementa a época de cancelamento e derruba qualquer sessão ativa.
    pub fn trigger_emergency_stop(&self) {
        self.emergency_stop.store(true, Ordering::SeqCst);
        self.cancel_epoch.fetch_add(1, Ordering::SeqCst);
        if let Ok(mut guard) = self.active_session.lock() {
            *guard = None;
        }
    }

    pub fn clear_emergency_stop(&self) {
        self.emergency_stop.store(false, Ordering::SeqCst);
    }

    pub fn is_emergency_stopped(&self) -> bool {
        self.emergency_stop.load(Ordering::SeqCst)
    }

    pub fn set_automation_enabled(&self, enabled: bool) {
        self.automation_enabled.store(enabled, Ordering::SeqCst);
        if !enabled {
            self.cancel_epoch.fetch_add(1, Ordering::SeqCst);
            if let Ok(mut guard) = self.active_session.lock() {
                *guard = None;
            }
        }
    }

    pub fn is_automation_enabled(&self) -> bool {
        self.automation_enabled.load(Ordering::SeqCst)
    }

    pub fn active_owner(&self) -> Option<ActiveSessionInfo> {
        self.active_session.lock().ok().and_then(|g| g.clone())
    }

    /// Adquire uma guarda RAII exclusiva para operar teclado/mouse em nome de `char_name`.
    /// Libera automaticamente mesmo se a rotina sofrer erro ou pânico.
    pub fn try_acquire(
        &self,
        char_name: &str,
        routine_name: &str,
        priority: RoutinePriority,
    ) -> Result<InputSessionGuard, InputScheduleError> {
        if self.is_emergency_stopped() {
            return Err(InputScheduleError::EmergencyStopActive);
        }
        if !self.is_automation_enabled() {
            return Err(InputScheduleError::AutomationDisabled);
        }
        let mut slot = self
            .active_session
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if let Some(current) = slot.as_ref() {
            return Err(InputScheduleError::SessionBusy {
                active_char: current.char_name.clone(),
                routine: current.routine_name.clone(),
            });
        }
        let epoch = self.cancel_epoch.load(Ordering::SeqCst);
        *slot = Some(ActiveSessionInfo {
            char_name: char_name.to_string(),
            routine_name: routine_name.to_string(),
            priority,
            epoch,
        });
        Ok(InputSessionGuard {
            scheduler: self.clone(),
            epoch,
        })
    }
}

/// Guarda RAII que garante que o teclado/mouse do PC é liberado ao término da rotina.
#[derive(Debug)]
pub struct InputSessionGuard {
    scheduler: InputSessionScheduler,
    epoch: u64,
}

impl InputSessionGuard {
    /// Verifica antes de cada passo de clique/tecla se a sessão continua válida e não sofreu F8.
    pub fn verify_before_step(&self) -> Result<(), InputScheduleError> {
        if self.scheduler.is_emergency_stopped() {
            return Err(InputScheduleError::EmergencyStopActive);
        }
        if !self.scheduler.is_automation_enabled() {
            return Err(InputScheduleError::AutomationDisabled);
        }
        if self.scheduler.cancel_epoch.load(Ordering::SeqCst) != self.epoch {
            return Err(InputScheduleError::SessionCancelled);
        }
        Ok(())
    }
}

impl Drop for InputSessionGuard {
    fn drop(&mut self) {
        if let Ok(mut slot) = self.scheduler.active_session.lock() {
            if slot.as_ref().is_some_and(|s| s.epoch == self.epoch) {
                *slot = None;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn enforces_single_input_session_per_pc_and_immediate_emergency_stop() {
        let sched = InputSessionScheduler::new(true);
        let g1 = sched
            .try_acquire("x9901", "Volta ao Spot", RoutinePriority::StandardRecovery)
            .expect("primeira sessão deve adquirir");
        assert!(g1.verify_before_step().is_ok());

        // Segundo personagem no mesmo PC não pode operar mouse/teclado simultaneamente
        let err = sched
            .try_acquire("ReDMooN", "Fortuna", RoutinePriority::Convenience)
            .unwrap_err();
        assert!(matches!(err, InputScheduleError::SessionBusy { .. }));

        // Acionar parada de emergência invalida imediatamente a sessão em andamento
        sched.trigger_emergency_stop();
        assert_eq!(
            g1.verify_before_step(),
            Err(InputScheduleError::EmergencyStopActive)
        );
        drop(g1);

        // Enquanto emergência estiver ativa, novas sessões são recusadas
        assert_eq!(
            sched
                .try_acquire("ReDMooN", "Volta ao Spot", RoutinePriority::Survival)
                .unwrap_err(),
            InputScheduleError::EmergencyStopActive
        );

        // Após liberar emergência, o PC volta a aceitar sessões
        sched.clear_emergency_stop();
        let g2 = sched
            .try_acquire("ReDMooN", "Volta ao Spot", RoutinePriority::Survival)
            .expect("deve adquirir após limpar emergência");
        assert!(g2.verify_before_step().is_ok());
    }
}
