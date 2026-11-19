//! Biblioteca e servidor da Central (`zengravity-central`).
//!
//! Suporta duas fontes de telemetria intercambiáveis em tempo real:
//! 1. **Modo Real (Somente Leitura Windows)**: usa `ReadOnlyLiveCollector` + `Win32ReadOnlyAccess`
//!    para observar os processos `MainMU.exe` (`The Classic MU Season 2`) abertos na máquina local.
//! 2. **Modo Simulação Reproduzível**: usa `SimulatorEngine` (4 PCs / 9 personagens).

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
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Instant, SystemTime, UNIX_EPOCH};
use tokio::sync::{broadcast, mpsc, Mutex};
use zengravity_agent::live_collector::{ReadOnlyLiveCollector, Win32ReadOnlyAccess};
use zengravity_agent::win_reader::{discover_game_windows, DEFAULT_WINDOW_CLASS};
use zengravity_core::alerts::AlertConfig;
use zengravity_protocol::{CentralToPanel, Command, EventSeverity, PanelToCentral};
use zengravity_simulator::SimulatorEngine;

/// Comandos de controle da Central (troca de fonte Real vs Simulador, injeção de falhas no simulador e comandos do operador).
#[derive(Debug, Clone)]
pub enum SimControlAction {
    OperatorCommand { command_id: u64, command: Command },
    SetUseLiveWindowsSource { use_live: bool },
    ToggleSilenceMachine { machine_id: String, silenced: bool },
    TogglePauseCentralStream { paused: bool },
    InjectDeath { char_name: String },
    ToggleDisconnect { char_name: String },
}

#[derive(Clone)]
pub struct CentralRuntimeHandle {
    pub ws_url: String,
    pub initial_live_mode: bool,
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

/// Inicia a Central detectando automaticamente se há janelas reais do cliente MU Online abertas
/// (ou iniciando no Simulador e permitindo alternar em tempo real com 1 clique no painel).
pub async fn spawn_central_with_simulator(
    bind_addr: &str,
    db_path: &str,
    sim_seed: u64,
) -> anyhow::Result<CentralRuntimeHandle> {
    let db = LocalDatabase::open(db_path).await?;
    let start_instant = Instant::now();
    let now_utc = CentralRuntimeHandle::now_utc_s();

    let machine_name = std::env::var("COMPUTERNAME").unwrap_or_else(|_| "PC-LOCAL".into());
    let live_windows_found = !discover_game_windows(DEFAULT_WINDOW_CLASS).is_empty();
    let mut use_live = live_windows_found;

    let mut sim = SimulatorEngine::new(sim_seed);
    let mut live_collector =
        ReadOnlyLiveCollector::new(&machine_name, Some(PathBuf::from("data/ram_locks.json")));
    let win_access = Win32ReadOnlyAccess::default();

    let mut hub = CentralHub::new(!use_live, AlertConfig::default());

    if use_live {
        hub.apply_agent_message(live_collector.hello_message(), 100, now_utc);
        let rep = live_collector.collect_cycle(&win_access, 500, now_utc);
        hub.apply_agent_message(rep, 500, now_utc);
    } else {
        for hello in sim.initial_hellos() {
            hub.apply_agent_message(hello, 100, now_utc);
        }
        for rep in sim.step(500, now_utc) {
            hub.apply_agent_message(rep, 500, now_utc);
        }
    }

    let shared_hub = Arc::new(Mutex::new(hub));
    let (panel_tx, _) = broadcast::channel::<CentralToPanel>(128);
    let (control_tx, mut control_rx) = mpsc::channel::<SimControlAction>(64);

    let listener = tokio::net::TcpListener::bind(bind_addr).await?;
    let local_addr = listener.local_addr()?;
    let ws_url = format!("ws://{local_addr}/ws/panel");

    let handle = CentralRuntimeHandle {
        ws_url: ws_url.clone(),
        initial_live_mode: use_live,
        control_tx: control_tx.clone(),
        panel_broadcast: panel_tx.clone(),
        shared_hub: shared_hub.clone(),
        start_instant,
    };

    let app_state = handle.clone();
    let app = Router::new()
        .route("/ws/panel", get(ws_panel_handler))
        .with_state(app_state);

    tokio::spawn(async move {
        if let Err(e) = axum::serve(listener, app).await {
            tracing::error!(erro = %e, "erro no servidor WebSocket da Central");
        }
    });

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

                    let reports = if use_live {
                        vec![live_collector.collect_cycle(&win_access, mono_ms, utc_s)]
                    } else {
                        sim.step(mono_ms, utc_s)
                    };

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
                        SimControlAction::SetUseLiveWindowsSource { use_live: want_live } => {
                            use_live = want_live;
                            let mut hub_guard = loop_hub.lock().await;
                            hub_guard.reset_for_source(!use_live);
                            if use_live {
                                hub_guard.apply_agent_message(live_collector.hello_message(), mono_ms, utc_s);
                                let rep = live_collector.collect_cycle(&win_access, mono_ms, utc_s);
                                hub_guard.apply_agent_message(rep, mono_ms, utc_s);
                                let _ = hub_guard.push_event(
                                    mono_ms,
                                    utc_s,
                                    EventSeverity::Info,
                                    &live_collector.machine_id.clone(),
                                    None,
                                    "Fonte alterada para MODO REAL (SOMENTE LEITURA)",
                                    "Lendo janelas MainMU.exe locais com captura única PrintWindow e ReadProcessMemory passivo",
                                );
                            } else {
                                for hello in sim.initial_hellos() {
                                    hub_guard.apply_agent_message(hello, mono_ms, utc_s);
                                }
                                for rep in sim.step(mono_ms, utc_s) {
                                    hub_guard.apply_agent_message(rep, mono_ms, utc_s);
                                }
                                let _ = hub_guard.push_event(
                                    mono_ms,
                                    utc_s,
                                    EventSeverity::Info,
                                    "SIMULADOR",
                                    None,
                                    "Fonte alterada para MODO SIMULAÇÃO",
                                    "Simulando 4 PCs e 9 personagens determinísticos",
                                );
                            }
                            let snap = hub_guard.snapshot(mono_ms, utc_s);
                            if !stream_paused {
                                let _ = loop_tx.send(CentralToPanel::Snapshot(Box::new(snap)));
                            }
                        }
                        SimControlAction::OperatorCommand { command_id, command } => {
                            let (ok, msg) = if use_live {
                                match &command {
                                    Command::RelockTelemetry { char_name, .. } => {
                                        let found = live_collector.relock_character(char_name);
                                        (found, format!("Travas de RAM resetadas para {char_name} (abra o inventário V para nova semente)"))
                                    }
                                    Command::AssignHomeSpot { char_name, spot, .. } => {
                                        let found = live_collector.assign_home_spot(char_name, *spot);
                                        (found, command.summary_pt())
                                    }
                                    _ => (true, format!("{} (Modo Somente Leitura ativo)", command.summary_pt())),
                                }
                            } else {
                                sim.apply_command(&command)
                            };
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
                                if silenced { EventSeverity::Warning } else { EventSeverity::Info },
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
