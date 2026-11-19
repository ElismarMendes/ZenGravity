//! Biblioteca e servidor da Central (`zengravity-central`).
//!
//! Pode rodar como servidor standalone (`zengravity-central.exe`) ou embutida
//! em background pelo aplicativo desktop (`zengravity-desktop.exe`) para operação
//! imediata com um único clique no Windows.

pub mod db;
pub mod hub;

use crate::db::LocalDatabase;
use crate::hub::{CentralHub, PersistenceEffect};
use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::extract::State;
use axum::response::IntoResponse;
use axum::routing::get;
use axum::Router;
use futures_util::{SinkExt, StreamExt};
use std::sync::Arc;
use std::time::{Instant, SystemTime, UNIX_EPOCH};
use tokio::sync::{broadcast, mpsc, Mutex};
use zengravity_core::alerts::AlertConfig;
use zengravity_protocol::{CentralToPanel, Command, PanelToCentral};
use zengravity_simulator::SimulatorEngine;

/// Comandos especiais de controle do Simulador enviados pela barra de simulação do Desktop.
#[derive(Debug, Clone)]
pub enum SimControlAction {
    OperatorCommand { command_id: u64, command: Command },
    ToggleSilenceMachine { machine_id: String, silenced: bool },
    TogglePauseCentralStream { paused: bool },
    InjectDeath { char_name: String },
    ToggleDisconnect { char_name: String },
}

#[derive(Clone)]
pub struct CentralRuntimeHandle {
    pub ws_url: String,
    pub control_tx: mpsc::Sender<SimControlAction>,
    pub panel_broadcast: broadcast::Sender<CentralToPanel>,
    pub shared_hub: Arc<Mutex<CentralHub>>,
    start_instant: Instant,
}

impl CentralRuntimeHandle {
    pub fn now_mono_ms(&self) -> u64 {
        self.start_instant.elapsed().as_millis() as u64
    }

    pub fn now_utc_s() -> u64 {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(1_795_000_000)
    }
}

/// Inicia a Central + Simulador Reproduzível em background e expõe tanto WebSocket local
/// (`/ws/panel`) quanto canais diretos para o `zengravity-desktop`.
pub async fn spawn_central_with_simulator(
    bind_addr: &str,
    db_path: &str,
    sim_seed: u64,
) -> anyhow::Result<CentralRuntimeHandle> {
    let db = LocalDatabase::open(db_path).await?;
    let start_instant = Instant::now();
    let now_utc = CentralRuntimeHandle::now_utc_s();

    let mut sim = SimulatorEngine::new(sim_seed);
    let mut hub = CentralHub::new(true, AlertConfig::default());

    for hello in sim.initial_hellos() {
        hub.apply_agent_message(hello, 100, now_utc);
    }
    for rep in sim.step(500, now_utc) {
        hub.apply_agent_message(rep, 500, now_utc);
    }

    let shared_hub = Arc::new(Mutex::new(hub));
    let (panel_tx, _) = broadcast::channel::<CentralToPanel>(128);
    let (control_tx, mut control_rx) = mpsc::channel::<SimControlAction>(64);

    let listener = tokio::net::TcpListener::bind(bind_addr).await?;
    let local_addr = listener.local_addr()?;
    let ws_url = format!("ws://{local_addr}/ws/panel");

    let handle = CentralRuntimeHandle {
        ws_url: ws_url.clone(),
        control_tx: control_tx.clone(),
        panel_broadcast: panel_tx.clone(),
        shared_hub: shared_hub.clone(),
        start_instant,
    };

    // Servidor WebSocket Axum para conexões de painel externo ou reconexão WS
    let app_state = handle.clone();
    let app = Router::new()
        .route("/ws/panel", get(ws_panel_handler))
        .with_state(app_state);

    tokio::spawn(async move {
        if let Err(e) = axum::serve(listener, app).await {
            tracing::error!(erro = %e, "erro no servidor WebSocket da Central");
        }
    });

    // Laço reativo do Simulador -> Central -> SQLite -> Broadcast para Painel Desktop
    let loop_hub = shared_hub.clone();
    let loop_tx = panel_tx.clone();
    tokio::spawn(async move {
        let mut tick_interval = tokio::time::interval(std::time::Duration::from_millis(1500));
        let mut stream_paused = false;

        loop {
            tokio::select! {
                _ = tick_interval.tick() => {
                    let mono_ms = start_instant.elapsed().as_millis() as u64;
                    let utc_s = CentralRuntimeHandle::now_utc_s();
                    let reports = sim.step(mono_ms, utc_s);

                    let mut effects_to_save = Vec::new();
                    {
                        let mut hub_guard = loop_hub.lock().await;
                        for rep in reports {
                            let (delta_opt, effects) = hub_guard.apply_agent_message(rep, mono_ms, utc_s);
                            effects_to_save.extend(effects);
                            if !stream_paused {
                                if let Some(delta) = delta_opt {
                                    let _ = loop_tx.send(CentralToPanel::Delta(Box::new(delta)));
                                }
                            }
                        }
                        if !stream_paused {
                            let _ = loop_tx.send(CentralToPanel::Heartbeat {
                                seq: hub_guard.seq,
                                central_mono_ms: mono_ms,
                                central_utc_s: utc_s,
                            });
                        }
                    }

                    for eff in effects_to_save {
                        match eff {
                            PersistenceEffect::ZenPoint { machine_id, char_name, ts_utc_s, zen } => {
                                let _ = db.record_zen_point(&machine_id, &char_name, ts_utc_s, zen).await;
                            }
                            PersistenceEffect::PkEvent { ts_utc_s, machine_id, killer, killer_kind, victim } => {
                                let _ = db.record_pk_event(ts_utc_s, &machine_id, &killer, &killer_kind, &victim).await;
                            }
                        }
                    }
                }
                Some(ctrl) = control_rx.recv() => {
                    let mono_ms = start_instant.elapsed().as_millis() as u64;
                    let utc_s = CentralRuntimeHandle::now_utc_s();
                    match ctrl {
                        SimControlAction::OperatorCommand { command_id, command } => {
                            let (ok, msg) = sim.apply_command(&command);
                            let _ = db.record_audit(utc_s, "desktop-operator", &command.summary_pt()).await;
                            let mut hub_guard = loop_hub.lock().await;
                            let _ = hub_guard.handle_operator_command(&command, mono_ms, utc_s);
                            let _ = loop_tx.send(CentralToPanel::CommandAck {
                                command_id,
                                accepted: ok,
                                message: msg,
                            });
                            let snap = hub_guard.snapshot(mono_ms, utc_s);
                            if !stream_paused {
                                let _ = loop_tx.send(CentralToPanel::Snapshot(Box::new(snap)));
                            }
                        }
                        SimControlAction::ToggleSilenceMachine { machine_id, silenced } => {
                            sim.set_machine_silenced(&machine_id, silenced);
                            let mut hub_guard = loop_hub.lock().await;
                            let _ = hub_guard.push_event(
                                mono_ms,
                                utc_s,
                                if silenced { zengravity_protocol::EventSeverity::Warning } else { zengravity_protocol::EventSeverity::Info },
                                &machine_id,
                                None,
                                if silenced { format!("Simulando atraso (STALE) em {machine_id}") } else { format!("Sinal retomado em {machine_id}") },
                                if silenced {
                                    "Agente parou de reportar; após 15s a central marcará LEITURA DESATUALIZADA"
                                } else {
                                    "Relatórios em tempo real restabelecidos"
                                },
                            );
                            let snap = hub_guard.snapshot(mono_ms, utc_s);
                            if !stream_paused {
                                let _ = loop_tx.send(CentralToPanel::Snapshot(Box::new(snap)));
                            }
                        }
                        SimControlAction::TogglePauseCentralStream { paused } => {
                            stream_paused = paused;
                            if !stream_paused {
                                // Ao retomar a conexão, envia imediatamente um Snapshot fresco de ressincronização
                                let hub_guard = loop_hub.lock().await;
                                let snap = hub_guard.snapshot(mono_ms, utc_s);
                                let _ = loop_tx.send(CentralToPanel::Snapshot(Box::new(snap)));
                            }
                        }
                        SimControlAction::InjectDeath { char_name } => {
                            sim.inject_death(&char_name);
                        }
                        SimControlAction::ToggleDisconnect { char_name } => {
                            sim.toggle_disconnect(&char_name);
                        }
                    }
                }
            }
        }
    });

    Ok(handle)
}

async fn ws_panel_handler(
    ws: WebSocketUpgrade,
    State(state): State<CentralRuntimeHandle>,
) -> impl IntoResponse {
    ws.on_upgrade(move |socket| handle_ws_client(socket, state))
}

async fn handle_ws_client(socket: WebSocket, state: CentralRuntimeHandle) {
    let (mut sender, mut receiver) = socket.split();
    let mut rx = state.panel_broadcast.subscribe();

    // Envia Snapshot inicial assim que o cliente conecta
    {
        let mono_ms = state.now_mono_ms();
        let utc_s = CentralRuntimeHandle::now_utc_s();
        let hub = state.shared_hub.lock().await;
        let snap = CentralToPanel::Snapshot(Box::new(hub.snapshot(mono_ms, utc_s)));
        if let Ok(json) = serde_json::to_string(&snap) {
            let _ = sender.send(Message::Text(json.into())).await;
        }
    }

    let send_task = tokio::spawn(async move {
        while let Ok(msg) = rx.recv().await {
            if let Ok(json) = serde_json::to_string(&msg) {
                if sender.send(Message::Text(json.into())).await.is_err() {
                    break;
                }
            }
        }
    });

    while let Some(Ok(Message::Text(text))) = receiver.next().await {
        if let Ok(panel_msg) = serde_json::from_str::<PanelToCentral>(&text) {
            match panel_msg {
                PanelToCentral::IssueCommand {
                    command_id,
                    command,
                } => {
                    let _ = state
                        .control_tx
                        .send(SimControlAction::OperatorCommand {
                            command_id,
                            command,
                        })
                        .await;
                }
                PanelToCentral::RequestSnapshot => {
                    let mono_ms = state.now_mono_ms();
                    let utc_s = CentralRuntimeHandle::now_utc_s();
                    let hub = state.shared_hub.lock().await;
                    let snap = CentralToPanel::Snapshot(Box::new(hub.snapshot(mono_ms, utc_s)));
                    let _ = state.panel_broadcast.send(snap);
                }
                PanelToCentral::Ping { .. } => {}
            }
        }
    }

    send_task.abort();
}
