//! Aplicativo Desktop Nativo do Windows (`zengravity-desktop`).
//!
//! Construído com `egui`/`eframe` + `wgpu`, tema escuro profissional e
//! comunicação assíncrona reativa com a Central (`zengravity-central`) via
//! protocolo tipado (`zengravity-protocol`).

mod map_view;
mod state;

use crate::map_view::Map2DViewport;
use crate::state::{ActiveTab, DesktopClientState, StateFilter};
use eframe::egui::{self, Color32, CornerRadius, FontId, Margin, Pos2, RichText, Stroke, Ui, Vec2};
use futures_util::{SinkExt, StreamExt};
use std::collections::BTreeSet;
use std::time::{Duration, Instant};
use tokio::sync::mpsc;
use zengravity_central::{spawn_central_with_simulator, SimControlAction};
use zengravity_core::alerts::AlertKind;
use zengravity_core::lifecycle::{CharLifecycleState, Confidence};
use zengravity_core::map::WorldId;
use zengravity_core::{format_zen, format_zen_compact};
use zengravity_protocol::{
    CentralToPanel, Command, EventSeverity, PanelCharacterView, PanelToCentral,
};

fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "info,zengravity=debug".into()),
        )
        .init();

    let args: Vec<String> = std::env::args().collect();
    let external_ws = args
        .windows(2)
        .find(|w| w[0] == "--connect")
        .map(|w| w[1].clone());

    let rt = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()?;
    let _guard = rt.enter();

    let (incoming_tx, incoming_rx) = mpsc::channel::<CentralToPanel>(256);
    let (sim_ctrl_tx, mut sim_ctrl_rx) = mpsc::channel::<SimControlAction>(64);

    let ws_url = if let Some(url) = external_ws {
        url
    } else {
        let handle = rt.block_on(spawn_central_with_simulator(
            "127.0.0.1:0",
            "data/zengravity.db",
            20261118,
        ))?;
        let central_ctrl = handle.control_tx.clone();
        rt.spawn(async move {
            while let Some(act) = sim_ctrl_rx.recv().await {
                let _ = central_ctrl.send(act).await;
            }
        });
        handle.ws_url
    };

    // Task de conexão WebSocket resiliente com reconexão automática exponencial
    let (ws_cmd_tx, mut ws_cmd_rx) = mpsc::channel::<PanelToCentral>(64);
    let ws_target = ws_url.clone();
    rt.spawn(async move {
        let mut backoff_ms = 500u64;
        loop {
            match tokio_tungstenite::connect_async(&ws_target).await {
                Ok((ws_stream, _)) => {
                    backoff_ms = 500;
                    let (mut write, mut read) = ws_stream.split();
                    loop {
                        tokio::select! {
                            msg_opt = read.next() => {
                                match msg_opt {
                                    Some(Ok(tokio_tungstenite::tungstenite::Message::Text(txt))) => {
                                        if let Ok(parsed) = serde_json::from_str::<CentralToPanel>(&txt) {
                                            let _ = incoming_tx.send(parsed).await;
                                        }
                                    }
                                    Some(Ok(_)) => {}
                                    _ => break,
                                }
                            }
                            Some(out_msg) = ws_cmd_rx.recv() => {
                                if let Ok(json) = serde_json::to_string(&out_msg) {
                                    if write
                                        .send(tokio_tungstenite::tungstenite::Message::Text(json.into()))
                                        .await
                                        .is_err()
                                    {
                                        break;
                                    }
                                }
                            }
                        }
                    }
                }
                Err(e) => {
                    tracing::warn!(erro = %e, url = %ws_target, "aguardando reconexao com a Central...");
                }
            }
            tokio::time::sleep(Duration::from_millis(backoff_ms)).await;
            backoff_ms = (backoff_ms * 2).min(5000);
        }
    });

    let native_options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("ZenGravity Desktop — Monitoramento Multi-PC & Multicontas (MU Online)")
            .with_inner_size([1480.0, 900.0])
            .with_min_inner_size([1024.0, 680.0]),
        ..Default::default()
    };

    eframe::run_native(
        "ZenGravity Desktop",
        native_options,
        Box::new(move |cc| {
            configure_dark_theme(&cc.egui_ctx);
            Ok(Box::new(ZenGravityDesktopApp::new(
                ws_url,
                incoming_rx,
                ws_cmd_tx,
                sim_ctrl_tx,
            )))
        }),
    )
    .map_err(|e| anyhow::anyhow!("Falha ao iniciar interface nativa eframe/wgpu: {e}"))?;

    Ok(())
}

fn configure_dark_theme(ctx: &egui::Context) {
    let mut visuals = egui::Visuals::dark();
    visuals.window_fill = Color32::from_rgb(13, 17, 23);
    visuals.panel_fill = Color32::from_rgb(15, 20, 28);
    visuals.faint_bg_color = Color32::from_rgb(20, 26, 36);
    visuals.extreme_bg_color = Color32::from_rgb(9, 12, 17);
    visuals.widgets.noninteractive.bg_fill = Color32::from_rgb(22, 29, 40);
    visuals.widgets.inactive.bg_fill = Color32::from_rgb(28, 37, 52);
    visuals.widgets.hovered.bg_fill = Color32::from_rgb(38, 52, 74);
    visuals.widgets.active.bg_fill = Color32::from_rgb(48, 82, 130);
    visuals.selection.bg_fill = Color32::from_rgb(35, 95, 175);
    ctx.set_visuals(visuals);
}

struct ZenGravityDesktopApp {
    state: DesktopClientState,
    incoming_rx: mpsc::Receiver<CentralToPanel>,
    ws_cmd_tx: mpsc::Sender<PanelToCentral>,
    sim_ctrl_tx: mpsc::Sender<SimControlAction>,
    next_command_id: u64,

    active_tab: ActiveTab,
    search_query: String,
    selected_machine_filter: Option<String>,
    selected_world_filter: Option<WorldId>,
    state_filter: StateFilter,
    selected_char: Option<String>,
    manual_search_world: WorldId,
    map_viewport: Map2DViewport,

    // Controles de demonstração do Simulador no topo
    sim_stream_paused: bool,
    silenced_machines: BTreeSet<String>,
}

impl ZenGravityDesktopApp {
    fn new(
        ws_url: String,
        incoming_rx: mpsc::Receiver<CentralToPanel>,
        ws_cmd_tx: mpsc::Sender<PanelToCentral>,
        sim_ctrl_tx: mpsc::Sender<SimControlAction>,
    ) -> Self {
        let state = DesktopClientState {
            central_ws_url: ws_url,
            ..Default::default()
        };
        Self {
            state,
            incoming_rx,
            ws_cmd_tx,
            sim_ctrl_tx,
            next_command_id: 1,
            active_tab: ActiveTab::Overview,
            search_query: String::new(),
            selected_machine_filter: None,
            selected_world_filter: None,
            state_filter: StateFilter::All,
            selected_char: Some("x9901".into()),
            manual_search_world: WorldId::Kanturu,
            map_viewport: Map2DViewport::default(),
            sim_stream_paused: false,
            silenced_machines: BTreeSet::new(),
        }
    }

    fn dispatch_command(&mut self, command: Command) {
        let command_id = self.next_command_id;
        self.next_command_id += 1;
        let _ = self.ws_cmd_tx.try_send(PanelToCentral::IssueCommand {
            command_id,
            command: command.clone(),
        });
        let _ = self
            .sim_ctrl_tx
            .try_send(SimControlAction::OperatorCommand {
                command_id,
                command,
            });
    }

    fn drain_incoming_messages(&mut self) {
        let now = Instant::now();
        while let Ok(msg) = self.incoming_rx.try_recv() {
            self.state.apply_central_message(msg, now);
        }
        if self.state.resync_needed {
            self.state.resync_needed = false;
            let _ = self.ws_cmd_tx.try_send(PanelToCentral::RequestSnapshot);
        }
    }

    fn render_top_header(&mut self, ui: &mut Ui, now: Instant) {
        let central_stale = self.state.is_central_stream_stale(now);

        ui.horizontal_wrapped(|ui| {
            // Marca e Título
            ui.label(
                RichText::new("◆ ZENGRAVITY")
                    .font(FontId::proportional(20.0))
                    .strong()
                    .color(Color32::from_rgb(95, 185, 255)),
            );
            ui.label(
                RichText::new("v0.1.0")
                    .monospace()
                    .color(Color32::from_rgb(130, 145, 165)),
            );

            ui.separator();

            // Badge obrigatório de Modo Operacional (Simulador vs Modo Real Somente Leitura)
            if self.state.simulation_mode {
                egui::Frame::NONE
                    .fill(Color32::from_rgb(80, 52, 12))
                    .stroke(Stroke::new(1.5_f32, Color32::from_rgb(245, 175, 45)))
                    .corner_radius(CornerRadius::same(5))
                    .inner_margin(Margin::symmetric(10, 4))
                    .show(ui, |ui| {
                        ui.label(
                            RichText::new("⚡ MODO SIMULAÇÃO (4 PCs / 9 Personagens)")
                                .strong()
                                .color(Color32::from_rgb(255, 210, 95)),
                        );
                    });
            } else {
                egui::Frame::NONE
                    .fill(Color32::from_rgb(16, 62, 55))
                    .stroke(Stroke::new(1.5_f32, Color32::from_rgb(65, 225, 195)))
                    .corner_radius(CornerRadius::same(5))
                    .inner_margin(Margin::symmetric(10, 4))
                    .show(ui, |ui| {
                        ui.label(
                            RichText::new("🛡 MODO REAL — SOMENTE LEITURA (MainMU.exe)")
                                .strong()
                                .color(Color32::from_rgb(125, 245, 220)),
                        );
                    });
            }

            // Status da Central
            let (status_bg, status_fg, status_text) = if central_stale {
                (
                    Color32::from_rgb(90, 28, 28),
                    Color32::from_rgb(255, 130, 130),
                    format!(
                        "● CENTRAL DESATUALIZADA (STALE há {}s) — Reconectando...",
                        self.state.central_silence_secs(now)
                    ),
                )
            } else {
                (
                    Color32::from_rgb(18, 65, 42),
                    Color32::from_rgb(90, 235, 155),
                    format!(
                        "● CENTRAL CONECTADA (seq #{} • {})",
                        self.state.last_seq, self.state.central_ws_url
                    ),
                )
            };

            egui::Frame::NONE
                .fill(status_bg)
                .stroke(Stroke::new(1.0_f32, status_fg))
                .corner_radius(CornerRadius::same(5))
                .inner_margin(Margin::symmetric(10, 4))
                .show(ui, |ui| {
                    ui.label(RichText::new(status_text).strong().color(status_fg));
                });

            ui.separator();

            // Métricas globais resumidas
            ui.label(
                RichText::new(format!(
                    "PCs: {}  |  Chars: {}  |  Zen Total: {}  |  Taxa Global: {}/h  |  Alertas: {}",
                    self.state.machines.len(),
                    self.state.characters.len(),
                    format_zen_compact(self.state.total_zen() as f64),
                    format_zen_compact(self.state.total_zen_per_hour()),
                    self.state.active_alerts_count()
                ))
                .strong()
                .color(Color32::from_rgb(210, 220, 235)),
            );

            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if self.state.emergency_stop_global {
                    let btn = egui::Button::new(
                        RichText::new("▶ LIBERAR PARADA DE EMERGÊNCIA")
                            .strong()
                            .color(Color32::WHITE),
                    )
                    .fill(Color32::from_rgb(35, 140, 80));
                    if ui.add(btn).clicked() {
                        self.dispatch_command(Command::ClearEmergencyStop { machine_id: None });
                    }
                } else {
                    let btn = egui::Button::new(
                        RichText::new("🛑 PARADA DE EMERGÊNCIA (F8)")
                            .strong()
                            .color(Color32::WHITE),
                    )
                    .fill(Color32::from_rgb(185, 35, 35));
                    if ui.add(btn).clicked() {
                        self.dispatch_command(Command::TriggerEmergencyStop { machine_id: None });
                    }
                }
            });
        });

        // Faixa de Controles do Simulador (permite ao proprietário testar Stale, DC, Morte e Reconexão ao vivo)
        ui.add_space(4.0);
        egui::Frame::NONE
            .fill(Color32::from_rgb(20, 26, 36))
            .stroke(Stroke::new(1.0_f32, Color32::from_rgb(45, 58, 78)))
            .corner_radius(CornerRadius::same(4))
            .inner_margin(Margin::symmetric(8, 4))
            .show(ui, |ui| {
                ui.horizontal_wrapped(|ui| {
                    ui.label(
                        RichText::new("Fonte de Telemetria:")
                            .strong()
                            .color(Color32::from_rgb(160, 185, 220)),
                    );

                    let switch_label = if self.state.simulation_mode {
                        "🛡 Mudar para MODO REAL (Ler MainMU.exe Somente Leitura)"
                    } else {
                        "⚡ Mudar para MODO SIMULAÇÃO (4 PCs / 9 Personagens)"
                    };
                    if ui.button(switch_label).clicked() {
                        let want_live = self.state.simulation_mode;
                        let _ =
                            self.sim_ctrl_tx
                                .try_send(SimControlAction::SetUseLiveWindowsSource {
                                    use_live: want_live,
                                });
                    }

                    ui.separator();

                    let pause_label = if self.sim_stream_paused {
                        "🔌 Reconectar Fluxo da Central (Sair de STALE)"
                    } else {
                        "⚡ Simular Queda/Atraso da Central (Testar STALE)"
                    };
                    if ui.button(pause_label).clicked() {
                        self.sim_stream_paused = !self.sim_stream_paused;
                        let _ =
                            self.sim_ctrl_tx
                                .try_send(SimControlAction::TogglePauseCentralStream {
                                    paused: self.sim_stream_paused,
                                });
                    }

                    let pc2_silenced = self.silenced_machines.contains("PC2-FARM-NORTE");
                    let pc2_label = if pc2_silenced {
                        "📡 Retomar Agente PC2-FARM-NORTE"
                    } else {
                        "🔇 Silenciar Agente PC2-FARM-NORTE (Stale 15s)"
                    };
                    if ui.button(pc2_label).clicked() {
                        let new_state = !pc2_silenced;
                        if new_state {
                            self.silenced_machines.insert("PC2-FARM-NORTE".into());
                        } else {
                            self.silenced_machines.remove("PC2-FARM-NORTE");
                        }
                        let _ = self
                            .sim_ctrl_tx
                            .try_send(SimControlAction::ToggleSilenceMachine {
                                machine_id: "PC2-FARM-NORTE".into(),
                                silenced: new_state,
                            });
                    }

                    if let Some(sel) = self.selected_char.clone() {
                        ui.separator();
                        ui.label(
                            RichText::new(format!("Alvo ({sel}):"))
                                .color(Color32::from_rgb(180, 195, 215)),
                        );
                        if ui.button("💀 Simular Morte (HP=0)").clicked() {
                            let _ = self.sim_ctrl_tx.try_send(SimControlAction::InjectDeath {
                                char_name: sel.clone(),
                            });
                        }
                        if ui.button("🔌 Alternar DC (Queda TCP)").clicked() {
                            let _ = self
                                .sim_ctrl_tx
                                .try_send(SimControlAction::ToggleDisconnect { char_name: sel });
                        }
                    }

                    if let Some((ok, msg, ts)) = &self.state.last_ack_message {
                        if now.saturating_duration_since(*ts).as_secs() < 12 {
                            ui.separator();
                            let col = if *ok {
                                Color32::from_rgb(95, 230, 150)
                            } else {
                                Color32::from_rgb(255, 120, 120)
                            };
                            ui.label(RichText::new(format!("ACK: {msg}")).color(col));
                        }
                    }
                });
            });

        // Banner de aviso crítico quando a Central está Stale ou Parada de Emergência está ativa
        if central_stale {
            ui.add_space(4.0);
            egui::Frame::NONE
                .fill(Color32::from_rgb(105, 32, 22))
                .stroke(Stroke::new(1.5_f32, Color32::from_rgb(255, 140, 110)))
                .corner_radius(CornerRadius::same(4))
                .inner_margin(Margin::symmetric(10, 6))
                .show(ui, |ui| {
                    ui.label(
                        RichText::new(
                            "⚠ LEITURA DESATUALIZADA (STALE): A Central não enviou atualizações recentes. \
                             Os valores exibidos abaixo correspondem à última observação válida e nenhuma decisão cega é tomada.",
                        )
                        .strong()
                        .color(Color32::from_rgb(255, 220, 200)),
                    );
                });
        } else if self.state.emergency_stop_global {
            ui.add_space(4.0);
            egui::Frame::NONE
                .fill(Color32::from_rgb(120, 20, 20))
                .stroke(Stroke::new(1.5_f32, Color32::from_rgb(255, 110, 110)))
                .corner_radius(CornerRadius::same(4))
                .inner_margin(Margin::symmetric(10, 6))
                .show(ui, |ui| {
                    ui.label(
                        RichText::new(
                            "🛑 PARADA DE EMERGÊNCIA GLOBAL ATIVA: Todas as sessões de teclado e mouse em todos os PCs estão bloqueadas.",
                        )
                        .strong()
                        .color(Color32::WHITE),
                    );
                });
        }
    }

    fn render_left_sidebar(&mut self, ui: &mut Ui) {
        ui.add_space(4.0);
        ui.label(
            RichText::new("NAVEGAÇÃO")
                .small()
                .strong()
                .color(Color32::from_rgb(135, 150, 175)),
        );
        ui.add_space(4.0);

        for (tab, label) in [
            (ActiveTab::Overview, "📊 Visão Geral (PCs & Chars)"),
            (ActiveTab::Map2D, "🗺 Mapa 2D & Spots"),
            (ActiveTab::EventsAndAlerts, "🔔 Eventos & Alertas"),
            (ActiveTab::MachinesHealth, "🖥 Saúde dos PCs"),
        ] {
            let selected = self.active_tab == tab;
            let btn = egui::Button::new(RichText::new(label).strong().color(if selected {
                Color32::WHITE
            } else {
                Color32::from_rgb(190, 200, 215)
            }))
            .fill(if selected {
                Color32::from_rgb(38, 92, 165)
            } else {
                Color32::from_rgb(24, 32, 44)
            })
            .min_size(Vec2::new(ui.available_width(), 32.0));
            if ui.add(btn).clicked() {
                self.active_tab = tab;
            }
            ui.add_space(3.0);
        }

        ui.separator();
        ui.label(
            RichText::new("FILTROS RÁPIDOS")
                .small()
                .strong()
                .color(Color32::from_rgb(135, 150, 175)),
        );
        ui.add_space(4.0);

        ui.horizontal(|ui| {
            ui.label("Busca:");
            ui.text_edit_singleline(&mut self.search_query);
            if !self.search_query.is_empty() && ui.small_button("✖").clicked() {
                self.search_query.clear();
            }
        });

        ui.add_space(6.0);
        ui.label(RichText::new("Computador (PC):").small());
        if ui
            .selectable_label(self.selected_machine_filter.is_none(), "Todos os PCs")
            .clicked()
        {
            self.selected_machine_filter = None;
        }
        let machine_ids: Vec<(String, bool, usize)> = self
            .state
            .machines
            .values()
            .map(|m| {
                let count = self
                    .state
                    .characters
                    .values()
                    .filter(|c| c.machine_id == m.machine_id)
                    .count();
                (m.machine_id.clone(), m.is_stale, count)
            })
            .collect();
        for (m_id, is_stale, count) in machine_ids {
            let tag = if is_stale {
                format!("⚠ {m_id} ({count}) [STALE]")
            } else {
                format!("● {m_id} ({count})")
            };
            let is_sel = self.selected_machine_filter.as_deref() == Some(m_id.as_str());
            if ui.selectable_label(is_sel, tag).clicked() {
                self.selected_machine_filter = Some(m_id);
            }
        }

        ui.add_space(6.0);
        ui.label(RichText::new("Estado Operacional:").small());
        for filter in [
            StateFilter::All,
            StateFilter::Farming,
            StateFilter::AttentionRequired,
            StateFilter::Recovering,
            StateFilter::StaleOrDc,
        ] {
            if ui
                .selectable_label(self.state_filter == filter, filter.label_pt())
                .clicked()
            {
                self.state_filter = filter;
            }
        }

        ui.add_space(6.0);
        ui.label(RichText::new("Mapa Atual:").small());
        if ui
            .selectable_label(self.selected_world_filter.is_none(), "Todos os Mapas")
            .clicked()
        {
            self.selected_world_filter = None;
        }
        for &w in &[
            WorldId::Kanturu,
            WorldId::Lorencia,
            WorldId::Arena,
            WorldId::Dungeon,
            WorldId::LostTower,
        ] {
            if ui
                .selectable_label(self.selected_world_filter == Some(w), w.name())
                .clicked()
            {
                self.selected_world_filter = Some(w);
            }
        }

        ui.separator();
        // Zona de Alertas Ativos na barra lateral
        ui.label(
            RichText::new("ZONA DE ALERTAS ATIVOS")
                .small()
                .strong()
                .color(Color32::from_rgb(255, 165, 95)),
        );
        ui.add_space(4.0);

        let mut any_alert = false;
        egui::ScrollArea::vertical()
            .id_salt("sidebar_alerts_scroll")
            .max_height(210.0)
            .show(ui, |ui| {
                for ch in self.state.characters.values() {
                    for &alert in &ch.alerts {
                        any_alert = true;
                        let (bg, fg) = alert_colors(alert);
                        egui::Frame::NONE
                            .fill(bg)
                            .stroke(Stroke::new(1.0_f32, fg))
                            .corner_radius(CornerRadius::same(4))
                            .inner_margin(Margin::symmetric(6, 4))
                            .show(ui, |ui| {
                                ui.horizontal(|ui| {
                                    ui.label(
                                        RichText::new(format!(
                                            "[{}] {}",
                                            alert.label_pt(),
                                            ch.name
                                        ))
                                        .strong()
                                        .color(fg),
                                    );
                                    ui.with_layout(
                                        egui::Layout::right_to_left(egui::Align::Center),
                                        |ui| {
                                            if ui.small_button("Ver").clicked() {
                                                self.selected_char = Some(ch.name.clone());
                                            }
                                        },
                                    );
                                });
                                ui.label(
                                    RichText::new(format!(
                                        "{} — {}",
                                        ch.machine_id, ch.status_detail
                                    ))
                                    .small()
                                    .color(Color32::from_rgb(215, 220, 230)),
                                );
                            });
                        ui.add_space(3.0);
                    }
                }
                if !any_alert {
                    ui.label(
                        RichText::new("✓ Nenhum alerta ativo no momento.")
                            .color(Color32::from_rgb(95, 210, 135)),
                    );
                }
            });
    }

    fn render_right_details_panel(&mut self, ui: &mut Ui) {
        let Some(sel_name) = self.selected_char.clone() else {
            ui.vertical_centered(|ui| {
                ui.add_space(60.0);
                ui.label(
                    RichText::new("Selecione um personagem para inspecionar métricas, confiabilidade e controles.")
                        .color(Color32::from_rgb(150, 165, 185)),
                );
            });
            return;
        };

        let Some(ch) = self.state.characters.get(&sel_name).cloned() else {
            ui.label("Personagem selecionado não está mais listado.");
            return;
        };

        let now_mono = self.state.central_mono_ms;
        let (state_bg, state_fg) = lifecycle_colors(ch.state);

        egui::ScrollArea::vertical()
            .id_salt("char_details_scroll")
            .show(ui, |ui| {
                // Cabeçalho do Personagem
                ui.horizontal(|ui| {
                    ui.label(
                        RichText::new(&ch.name)
                            .font(FontId::proportional(20.0))
                            .strong()
                            .color(Color32::WHITE),
                    );
                    egui::Frame::NONE
                        .fill(state_bg)
                        .stroke(Stroke::new(1.0_f32, state_fg))
                        .corner_radius(CornerRadius::same(4))
                        .inner_margin(Margin::symmetric(8, 3))
                        .show(ui, |ui| {
                            ui.label(
                                RichText::new(ch.state.label_pt())
                                    .strong()
                                    .small()
                                    .color(state_fg),
                            );
                        });
                });

                ui.label(
                    RichText::new(format!(
                        "PC: {}  |  PID: {}  |  Lv. {}  |  Resets: {}",
                        ch.machine_id,
                        ch.pid,
                        ch.level.unwrap_or(0),
                        ch.resets.unwrap_or(0)
                    ))
                    .color(Color32::from_rgb(170, 185, 205)),
                );
                ui.label(
                    RichText::new(&ch.status_detail)
                        .italics()
                        .color(Color32::from_rgb(210, 220, 235)),
                );

                ui.separator();

                // Seção 1: Telemetria com Origem, Momento e Confiabilidade
                ui.label(
                    RichText::new("TELEMETRIA E CONFIABILIDADE DA LEITURA")
                        .small()
                        .strong()
                        .color(Color32::from_rgb(130, 185, 245)),
                );
                ui.add_space(4.0);

                egui::Grid::new("telemetry_provenance_grid")
                    .num_columns(4)
                    .striped(true)
                    .spacing([10.0, 6.0])
                    .show(ui, |ui| {
                        ui.strong("Métrica");
                        ui.strong("Valor");
                        ui.strong("Origem");
                        ui.strong("Idade / Confiança");
                        ui.end_row();

                        // Zen
                        ui.label("Zen Inventário");
                        ui.label(
                            RichText::new(
                                ch.zen
                                    .value
                                    .map(format_zen)
                                    .unwrap_or_else(|| "—".into()),
                            )
                            .monospace()
                            .strong()
                            .color(Color32::from_rgb(255, 215, 90)),
                        );
                        ui.label(ch.zen.source.label_pt());
                        render_confidence_cell(
                            ui,
                            ch.zen.confidence,
                            ch.zen.age_secs(now_mono),
                        );
                        ui.end_row();

                        // Zen/h e ETA
                        ui.label("Ganho (Zen/h)");
                        ui.label(
                            RichText::new(format!(
                                "{}/h",
                                format_zen_compact(ch.zen_per_hour)
                            ))
                            .monospace()
                            .color(Color32::from_rgb(100, 230, 155)),
                        );
                        ui.label("ZenTracker (1h)");
                        ui.label(
                            ch.eta_limit_2b_secs
                                .map(|s| format!("ETA 2B: {}m", s / 60))
                                .unwrap_or_else(|| "ETA: —".into()),
                        );
                        ui.end_row();

                        // HP
                        ui.label("Vida (HP)");
                        ui.label(
                            RichText::new(
                                ch.hp
                                    .value
                                    .map(|v| format!("{v} / {}", ch.max_hp))
                                    .unwrap_or_else(|| "—".into()),
                            )
                            .monospace(),
                        );
                        ui.label(ch.hp.source.label_pt());
                        render_confidence_cell(ui, ch.hp.confidence, ch.hp.age_secs(now_mono));
                        ui.end_row();

                        // Dark Raven
                        ui.label("Dark Raven");
                        ui.label(
                            ch.pet_hp_pct
                                .value
                                .map(|p| format!("{p}%"))
                                .unwrap_or_else(|| "Sem Raven".into()),
                        );
                        ui.label(ch.pet_hp_pct.source.label_pt());
                        render_confidence_cell(
                            ui,
                            ch.pet_hp_pct.confidence,
                            ch.pet_hp_pct.age_secs(now_mono),
                        );
                        ui.end_row();

                        // Posição
                        ui.label("Mapa / Tile");
                        ui.label(
                            RichText::new(
                                ch.position
                                    .value
                                    .map(|p| p.label())
                                    .unwrap_or_else(|| "—".into()),
                            )
                            .monospace(),
                        );
                        ui.label(ch.position.source.label_pt());
                        render_confidence_cell(
                            ui,
                            ch.position.confidence,
                            ch.position.age_secs(now_mono),
                        );
                        ui.end_row();

                        // MU Helper
                        ui.label("MU Helper");
                        ui.label(match ch.helper_active.value {
                            Some(true) => "LIGADO (Play)",
                            Some(false) => "DESLIGADO",
                            None => "Não lido",
                        });
                        ui.label(ch.helper_active.source.label_pt());
                        render_confidence_cell(
                            ui,
                            ch.helper_active.confidence,
                            ch.helper_active.age_secs(now_mono),
                        );
                        ui.end_row();
                    });

                ui.add_space(8.0);

                // Seção 2: Gráfico de Série Temporal de Zen (Sparkline)
                ui.label(
                    RichText::new("SÉRIE TEMPORAL DE ZEN (EVOLUÇÃO RECENTE)")
                        .small()
                        .strong()
                        .color(Color32::from_rgb(130, 185, 245)),
                );
                render_zen_sparkline(ui, &ch.zen_sparkline);

                ui.add_space(8.0);
                ui.separator();

                // Seção 3: Spots, Busca Manual e Fallback
                ui.label(
                    RichText::new("GESTÃO DE SPOTS, BUSCA MANUAL E FALLBACK")
                        .small()
                        .strong()
                        .color(Color32::from_rgb(130, 185, 245)),
                );
                ui.add_space(4.0);

                ui.label(format!(
                    "Spot Principal (F1): {}",
                    ch.home_spot
                        .map(|s| s.label())
                        .unwrap_or_else(|| "Não definido".into())
                ));
                ui.label(format!(
                    "Spot Ativo Atual: {}",
                    ch.active_spot
                        .map(|s| s.label())
                        .unwrap_or_else(|| "Nenhum".into())
                ));
                ui.label(format!(
                    "Fallback: {} | Secundários: {} | Reavaliar F1 após: {} min",
                    if ch.fallback_policy.enabled {
                        "Ativo"
                    } else {
                        "Desligado"
                    },
                    ch.fallback_policy
                        .secondary_worlds
                        .iter()
                        .map(|w| w.name())
                        .collect::<Vec<_>>()
                        .join(" → "),
                    ch.fallback_policy.reevaluate_original_after_min
                ));

                ui.add_space(4.0);
                ui.horizontal_wrapped(|ui| {
                    egui::ComboBox::from_id_salt("manual_search_world_combo")
                        .selected_text(self.manual_search_world.name())
                        .show_ui(ui, |ui| {
                            for &w in &[
                                WorldId::Kanturu,
                                WorldId::Arena,
                                WorldId::Dungeon,
                                WorldId::LostTower,
                                WorldId::Aida,
                            ] {
                                ui.selectable_value(&mut self.manual_search_world, w, w.name());
                            }
                        });

                    if ui
                        .button(
                            RichText::new("🔍 Procurar spot agora")
                                .strong()
                                .color(Color32::from_rgb(110, 220, 255)),
                        )
                        .clicked()
                    {
                        self.dispatch_command(Command::StartManualSpotSearch {
                            machine_id: ch.machine_id.clone(),
                            char_name: ch.name.clone(),
                            target_world: self.manual_search_world,
                        });
                    }

                    if ch.manual_search.as_ref().is_some_and(|m| m.active)
                        && ui.button("✖ Cancelar busca").clicked()
                    {
                        self.dispatch_command(Command::CancelManualSpotSearch {
                            machine_id: ch.machine_id.clone(),
                            char_name: ch.name.clone(),
                        });
                    }
                });

                ui.add_space(4.0);
                ui.horizontal_wrapped(|ui| {
                    if ui.button("🏠 Voltar ao Spot F1").clicked() {
                        self.dispatch_command(Command::ReturnToHomeSpot {
                            machine_id: ch.machine_id.clone(),
                            char_name: ch.name.clone(),
                        });
                    }

                    let toggle_label = if ch.automation_enabled {
                        "⏸ Pausar Automação deste Char"
                    } else {
                        "▶ Retomar Automação deste Char"
                    };
                    if ui.button(toggle_label).clicked() {
                        self.dispatch_command(Command::SetCharAutomation {
                            machine_id: ch.machine_id.clone(),
                            char_name: ch.name.clone(),
                            enabled: !ch.automation_enabled,
                        });
                    }

                    if let Some(pos) = ch.position.value {
                        if ui.button("🗺 Focar no Mapa 2D").clicked() {
                            self.map_viewport.focus_on_spot(pos);
                            self.active_tab = ActiveTab::Map2D;
                        }
                    }

                    if ui.button("🔄 Recalibrar Trava RAM").clicked() {
                        self.dispatch_command(Command::RelockTelemetry {
                            machine_id: ch.machine_id.clone(),
                            char_name: ch.name.clone(),
                        });
                    }
                });

                ui.add_space(8.0);
                ui.separator();

                // Seção 4: Entidades Próximas Observadas (Raio de 12 tiles)
                ui.label(
                    RichText::new("ENTIDADES OBSERVADAS NO RAIO PRÓXIMO (12 TILES)")
                        .small()
                        .strong()
                        .color(Color32::from_rgb(130, 185, 245)),
                );
                let obs_age = ch.nearby_entities.age_secs(now_mono);
                if let Some(entities) = &ch.nearby_entities.value {
                    if entities.is_empty() {
                        ui.label("Nenhuma entidade dentro do raio de 12 tiles neste ciclo.");
                    } else {
                        for ent in entities {
                            ui.horizontal(|ui| {
                                let col = match ent.kind {
                                    zengravity_core::entities::EntityKind::GoldenMonster => {
                                        Color32::from_rgb(255, 215, 50)
                                    }
                                    zengravity_core::entities::EntityKind::GoldenBoss => {
                                        Color32::from_rgb(255, 90, 210)
                                    }
                                    zengravity_core::entities::EntityKind::Npc => {
                                        Color32::from_rgb(100, 230, 200)
                                    }
                                    zengravity_core::entities::EntityKind::Player => {
                                        Color32::from_rgb(255, 165, 90)
                                    }
                                    zengravity_core::entities::EntityKind::Monster => {
                                        Color32::from_rgb(220, 130, 130)
                                    }
                                };
                                ui.colored_label(
                                    col,
                                    format!(
                                        "• [{}] {} em ({}, {}) — lido há {}s",
                                        ent.kind.label_pt(),
                                        ent.name,
                                        ent.x,
                                        ent.y,
                                        obs_age
                                    ),
                                );
                            });
                        }
                    }
                } else {
                    ui.colored_label(
                        Color32::from_rgb(160, 165, 175),
                        "Leitura de entidades próximas não disponível (ausência de leitura != vazio).",
                    );
                }
            });
    }

    fn render_overview_tab(&mut self, ui: &mut Ui) {
        // 1) Faixa de Cards de PCs
        ui.label(
            RichText::new("COMPUTADORES CONECTADOS (AGENTES)")
                .small()
                .strong()
                .color(Color32::from_rgb(140, 165, 200)),
        );
        ui.add_space(4.0);

        ui.horizontal_wrapped(|ui| {
            let machines: Vec<_> = self.state.machines.values().cloned().collect();
            for m in machines {
                let char_count = self
                    .state
                    .characters
                    .values()
                    .filter(|c| c.machine_id == m.machine_id)
                    .count();
                let border_col = if m.is_stale {
                    Color32::from_rgb(235, 155, 50)
                } else if m.health.emergency_stop_active {
                    Color32::from_rgb(235, 70, 70)
                } else {
                    Color32::from_rgb(50, 75, 110)
                };

                egui::Frame::NONE
                    .fill(Color32::from_rgb(21, 28, 39))
                    .stroke(Stroke::new(1.2_f32, border_col))
                    .corner_radius(CornerRadius::same(6))
                    .inner_margin(Margin::symmetric(10, 8))
                    .show(ui, |ui| {
                        ui.set_min_width(215.0);
                        ui.horizontal(|ui| {
                            ui.label(RichText::new(&m.machine_id).strong().color(Color32::WHITE));
                            let badge = if m.is_stale {
                                ("STALE", Color32::from_rgb(255, 180, 70))
                            } else if m.health.emergency_stop_active {
                                ("PARADA F8", Color32::from_rgb(255, 100, 100))
                            } else {
                                ("ONLINE", Color32::from_rgb(80, 225, 145))
                            };
                            ui.colored_label(badge.1, format!("● {}", badge.0));
                        });
                        ui.label(
                            RichText::new(format!(
                                "{} chars | CPU {:.1}% | RAM {:.0} MB",
                                char_count, m.health.cpu_percent, m.health.ram_mb
                            ))
                            .small()
                            .color(Color32::from_rgb(175, 185, 200)),
                        );
                        ui.label(
                            RichText::new(format!(
                                "Sessão Input: {}",
                                m.health
                                    .active_input_owner
                                    .as_deref()
                                    .unwrap_or("Livre (0/1)")
                            ))
                            .small()
                            .color(Color32::from_rgb(135, 190, 245)),
                        );
                    });
            }
        });

        ui.add_space(10.0);

        // 2) Cards dos Personagens Filtrados
        let filtered_chars: Vec<PanelCharacterView> = self
            .state
            .characters
            .values()
            .filter(|c| self.matches_filters(c))
            .cloned()
            .collect();

        ui.horizontal(|ui| {
            ui.label(
                RichText::new(format!(
                    "PERSONAGENS MONITORADOS ({}/{})",
                    filtered_chars.len(),
                    self.state.characters.len()
                ))
                .small()
                .strong()
                .color(Color32::from_rgb(140, 165, 200)),
            );
        });
        ui.add_space(4.0);

        if filtered_chars.is_empty() {
            egui::Frame::NONE
                .fill(Color32::from_rgb(20, 26, 36))
                .corner_radius(CornerRadius::same(6))
                .inner_margin(Margin::same(24))
                .show(ui, |ui| {
                    ui.label(
                        RichText::new("Nenhum personagem corresponde aos filtros selecionados.")
                            .color(Color32::from_rgb(165, 175, 190)),
                    );
                });
            return;
        }

        egui::ScrollArea::vertical()
            .id_salt("overview_chars_scroll")
            .show(ui, |ui| {
                ui.horizontal_wrapped(|ui| {
                    for ch in &filtered_chars {
                        self.render_character_card(ui, ch);
                    }
                });

                ui.add_space(14.0);
                ui.separator();
                ui.label(
                    RichText::new("ÚLTIMOS EVENTOS & AUDITORIA")
                        .small()
                        .strong()
                        .color(Color32::from_rgb(140, 165, 200)),
                );
                ui.add_space(4.0);
                self.render_events_list(ui, 8);
            });
    }

    fn render_character_card(&mut self, ui: &mut Ui, ch: &PanelCharacterView) {
        let is_selected = self.selected_char.as_deref() == Some(ch.name.as_str());
        let (state_bg, state_fg) = lifecycle_colors(ch.state);
        let border = if is_selected {
            Stroke::new(2.0_f32, Color32::from_rgb(95, 190, 255))
        } else {
            Stroke::new(1.0_f32, state_fg.gamma_multiply(0.55))
        };

        let resp = egui::Frame::NONE
            .fill(if is_selected {
                Color32::from_rgb(26, 36, 52)
            } else {
                Color32::from_rgb(19, 25, 35)
            })
            .stroke(border)
            .corner_radius(CornerRadius::same(7))
            .inner_margin(Margin::symmetric(12, 10))
            .show(ui, |ui| {
                ui.set_width(280.0);

                // Linha 1: Nome, PC e Badge de Estado Verdadeiro
                ui.horizontal(|ui| {
                    ui.label(
                        RichText::new(&ch.name)
                            .font(FontId::proportional(16.5))
                            .strong()
                            .color(Color32::WHITE),
                    );
                    ui.label(
                        RichText::new(format!("({})", ch.machine_id))
                            .small()
                            .color(Color32::from_rgb(145, 160, 180)),
                    );
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        egui::Frame::NONE
                            .fill(state_bg)
                            .stroke(Stroke::new(1.0_f32, state_fg))
                            .corner_radius(CornerRadius::same(4))
                            .inner_margin(Margin::symmetric(6, 2))
                            .show(ui, |ui| {
                                ui.label(
                                    RichText::new(ch.state.label_pt())
                                        .small()
                                        .strong()
                                        .color(state_fg),
                                );
                            });
                    });
                });

                ui.add_space(4.0);

                // Linha 2: Zen, Zen/h e ETA 2B
                ui.horizontal(|ui| {
                    let zen_str = ch.zen.value.map(format_zen).unwrap_or_else(|| "—".into());
                    ui.label(
                        RichText::new(format!("Zen: {zen_str}"))
                            .monospace()
                            .strong()
                            .color(Color32::from_rgb(255, 215, 95)),
                    );
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        ui.label(
                            RichText::new(format!("+{}/h", format_zen_compact(ch.zen_per_hour)))
                                .monospace()
                                .strong()
                                .color(Color32::from_rgb(90, 225, 150)),
                        );
                    });
                });

                // Linha 3: Barra de HP e Pet HP (Dark Raven)
                let hp_val = ch.hp.value.unwrap_or(0);
                let hp_ratio = if ch.max_hp > 0 {
                    (hp_val as f32 / ch.max_hp as f32).clamp(0.0, 1.0)
                } else {
                    0.0
                };
                ui.horizontal(|ui| {
                    ui.label(
                        RichText::new(format!("HP: {hp_val}/{}", ch.max_hp))
                            .small()
                            .monospace(),
                    );
                    let bar = egui::ProgressBar::new(hp_ratio).desired_width(95.0).fill(
                        if hp_ratio > 0.35 {
                            Color32::from_rgb(210, 55, 55)
                        } else {
                            Color32::from_rgb(255, 120, 40)
                        },
                    );
                    ui.add(bar);

                    if let Some(pet) = ch.pet_hp_pct.value {
                        ui.label(
                            RichText::new(format!("Raven: {pet}%"))
                                .small()
                                .color(Color32::from_rgb(90, 220, 210)),
                        );
                    }
                });

                // Linha 4: Posição, Spot F1 e Helper
                let pos_str = ch
                    .position
                    .value
                    .map(|p| p.label())
                    .unwrap_or_else(|| "Posição desconhecida".into());
                let helper_str = match ch.helper_active.value {
                    Some(true) => "Helper ON",
                    Some(false) => "Helper OFF",
                    None => "Helper ?",
                };
                ui.label(
                    RichText::new(format!("📍 {pos_str}  |  {helper_str}"))
                        .small()
                        .color(Color32::from_rgb(185, 200, 220)),
                );

                // Linha 5: Origem, idade e confiança da leitura
                let age_s = ch.zen.age_secs(self.state.central_mono_ms);
                ui.horizontal(|ui| {
                    ui.label(
                        RichText::new(format!(
                            "Fonte: {} • há {}s • {}",
                            ch.zen.source.label_pt(),
                            age_s,
                            ch.zen.confidence.label_pt()
                        ))
                        .small()
                        .color(Color32::from_rgb(135, 150, 170)),
                    );
                });

                // Alertas do card
                if !ch.alerts.is_empty() {
                    ui.add_space(3.0);
                    ui.horizontal_wrapped(|ui| {
                        for &al in &ch.alerts {
                            let (abg, afg) = alert_colors(al);
                            egui::Frame::NONE
                                .fill(abg)
                                .stroke(Stroke::new(1.0_f32, afg))
                                .corner_radius(CornerRadius::same(3))
                                .inner_margin(Margin::symmetric(5, 1))
                                .show(ui, |ui| {
                                    ui.label(
                                        RichText::new(al.label_pt()).small().strong().color(afg),
                                    );
                                });
                        }
                    });
                }
            });

        if resp.response.interact(egui::Sense::click()).clicked() {
            self.selected_char = Some(ch.name.clone());
        }
    }

    fn render_events_list(&self, ui: &mut Ui, limit: usize) {
        for ev in self.state.events.iter().take(limit) {
            let (sev_col, icon) = match ev.severity {
                EventSeverity::Info => (Color32::from_rgb(100, 190, 255), "ℹ"),
                EventSeverity::Warning => (Color32::from_rgb(245, 185, 65), "⚠"),
                EventSeverity::Critical => (Color32::from_rgb(255, 95, 95), "🚨"),
                EventSeverity::Golden => (Color32::from_rgb(255, 215, 45), "★"),
            };
            egui::Frame::NONE
                .fill(Color32::from_rgb(19, 25, 35))
                .stroke(Stroke::new(1.0_f32, Color32::from_rgb(38, 48, 65)))
                .corner_radius(CornerRadius::same(4))
                .inner_margin(Margin::symmetric(8, 5))
                .show(ui, |ui| {
                    ui.horizontal_wrapped(|ui| {
                        ui.colored_label(sev_col, format!("{icon} [{}]", ev.machine_id));
                        if let Some(c) = &ev.char_name {
                            ui.label(
                                RichText::new(format!("({c})"))
                                    .strong()
                                    .color(Color32::WHITE),
                            );
                        }
                        ui.label(RichText::new(&ev.title).strong().color(sev_col));
                        ui.label("—");
                        ui.label(RichText::new(&ev.detail).color(Color32::from_rgb(190, 200, 215)));
                    });
                });
            ui.add_space(3.0);
        }
    }

    fn render_events_and_alerts_tab(&self, ui: &mut Ui) {
        ui.heading("Console de Eventos, PKs, Avistamentos Golden e Auditoria");
        ui.add_space(6.0);
        egui::ScrollArea::vertical()
            .id_salt("full_events_scroll")
            .show(ui, |ui| {
                self.render_events_list(ui, 100);
            });
    }

    fn render_machines_health_tab(&mut self, ui: &mut Ui) {
        ui.heading("Saúde dos Agentes por Computador e Agendador de Entrada");
        ui.label(
            RichText::new(
                "Regra de hardware: cada PC possui apenas 1 sessão de teclado/mouse por vez. \
                 O agendador garante exclusão mútua e parada imediata via F8.",
            )
            .color(Color32::from_rgb(175, 190, 210)),
        );
        ui.add_space(8.0);

        let machines: Vec<_> = self.state.machines.values().cloned().collect();
        for m in machines {
            egui::Frame::NONE
                .fill(Color32::from_rgb(20, 27, 38))
                .stroke(Stroke::new(1.0_f32, Color32::from_rgb(55, 75, 105)))
                .corner_radius(CornerRadius::same(6))
                .inner_margin(Margin::same(12))
                .show(ui, |ui| {
                    ui.horizontal(|ui| {
                        ui.label(
                            RichText::new(&m.machine_id)
                                .font(FontId::proportional(17.0))
                                .strong()
                                .color(Color32::WHITE),
                        );
                        ui.label(format!("Versão: {} | SO: {}", m.agent_version, m.health.os_label));
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            let toggle_text = if m.health.automation_enabled {
                                "⏸ Pausar Automação deste PC"
                            } else {
                                "▶ Ativar Automação deste PC"
                            };
                            if ui.button(toggle_text).clicked() {
                                self.dispatch_command(Command::SetMachineAutomation {
                                    machine_id: m.machine_id.clone(),
                                    enabled: !m.health.automation_enabled,
                                });
                            }
                        });
                    });
                    ui.add_space(4.0);
                    ui.label(format!(
                        "CPU do Agente: {:.1}%  |  Memória RAM: {:.1} MB  |  Uptime: {}s  |  Dono atual do Mouse/Teclado: {}",
                        m.health.cpu_percent,
                        m.health.ram_mb,
                        m.health.uptime_s,
                        m.health.active_input_owner.as_deref().unwrap_or("Nenhum (Livre)")
                    ));
                });
            ui.add_space(8.0);
        }
    }

    fn matches_filters(&self, ch: &PanelCharacterView) -> bool {
        if let Some(m_filter) = &self.selected_machine_filter {
            if &ch.machine_id != m_filter {
                return false;
            }
        }
        if let Some(w_filter) = self.selected_world_filter {
            if ch.position.value.map(|p| p.world) != Some(w_filter) {
                return false;
            }
        }
        match self.state_filter {
            StateFilter::All => {}
            StateFilter::Farming => {
                if ch.state != CharLifecycleState::Farming {
                    return false;
                }
            }
            StateFilter::AttentionRequired => {
                if ch.alerts.is_empty()
                    && !matches!(
                        ch.state,
                        CharLifecycleState::Stopped
                            | CharLifecycleState::Dead
                            | CharLifecycleState::Disconnected
                            | CharLifecycleState::Stale
                    )
                {
                    return false;
                }
            }
            StateFilter::Recovering => {
                if ch.state != CharLifecycleState::Recovering {
                    return false;
                }
            }
            StateFilter::StaleOrDc => {
                if !matches!(
                    ch.state,
                    CharLifecycleState::Stale | CharLifecycleState::Disconnected
                ) {
                    return false;
                }
            }
        }
        if !self.search_query.trim().is_empty() {
            let q = self.search_query.trim().to_ascii_lowercase();
            let matches_name = ch.name.to_ascii_lowercase().contains(&q);
            let matches_pc = ch.machine_id.to_ascii_lowercase().contains(&q);
            let matches_map = ch
                .position
                .value
                .is_some_and(|p| p.world.name().to_ascii_lowercase().contains(&q));
            if !(matches_name || matches_pc || matches_map) {
                return false;
            }
        }
        true
    }
}

impl eframe::App for ZenGravityDesktopApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        self.drain_incoming_messages();
        let now = Instant::now();

        // Solicita repaint controlado a cada 500ms para atualizar contadores de idade ("há Xs")
        // e detectar prontamente transição para STALE sem gastar GPU à toa.
        ctx.request_repaint_after(Duration::from_millis(500));

        egui::TopBottomPanel::top("top_header_panel")
            .frame(
                egui::Frame::NONE
                    .fill(Color32::from_rgb(13, 17, 24))
                    .inner_margin(Margin::symmetric(12, 8)),
            )
            .show(ctx, |ui| {
                self.render_top_header(ui, now);
            });

        egui::SidePanel::left("left_nav_sidebar")
            .resizable(true)
            .default_width(235.0)
            .min_width(195.0)
            .frame(
                egui::Frame::NONE
                    .fill(Color32::from_rgb(15, 20, 28))
                    .inner_margin(Margin::symmetric(10, 8)),
            )
            .show(ctx, |ui| {
                self.render_left_sidebar(ui);
            });

        egui::SidePanel::right("right_char_inspector")
            .resizable(true)
            .default_width(390.0)
            .min_width(320.0)
            .frame(
                egui::Frame::NONE
                    .fill(Color32::from_rgb(16, 21, 30))
                    .inner_margin(Margin::symmetric(12, 10)),
            )
            .show(ctx, |ui| {
                self.render_right_details_panel(ui);
            });

        egui::CentralPanel::default()
            .frame(
                egui::Frame::NONE
                    .fill(Color32::from_rgb(12, 16, 22))
                    .inner_margin(Margin::symmetric(14, 10)),
            )
            .show(ctx, |ui| match self.active_tab {
                ActiveTab::Overview => self.render_overview_tab(ui),
                ActiveTab::Map2D => {
                    self.map_viewport
                        .ui(ui, &self.state, &mut self.selected_char);
                }
                ActiveTab::EventsAndAlerts => self.render_events_and_alerts_tab(ui),
                ActiveTab::MachinesHealth => self.render_machines_health_tab(ui),
            });
    }
}

fn lifecycle_colors(state: CharLifecycleState) -> (Color32, Color32) {
    match state {
        CharLifecycleState::Farming => (
            Color32::from_rgb(16, 62, 38),
            Color32::from_rgb(75, 235, 145),
        ),
        CharLifecycleState::Recovering => (
            Color32::from_rgb(75, 55, 14),
            Color32::from_rgb(255, 200, 70),
        ),
        CharLifecycleState::Stopped => (
            Color32::from_rgb(80, 42, 16),
            Color32::from_rgb(255, 155, 65),
        ),
        CharLifecycleState::Dead => (
            Color32::from_rgb(92, 20, 24),
            Color32::from_rgb(255, 105, 105),
        ),
        CharLifecycleState::Disconnected => (
            Color32::from_rgb(85, 22, 55),
            Color32::from_rgb(255, 115, 185),
        ),
        CharLifecycleState::Stale => (
            Color32::from_rgb(78, 52, 18),
            Color32::from_rgb(245, 185, 85),
        ),
        CharLifecycleState::Paused => (
            Color32::from_rgb(38, 45, 58),
            Color32::from_rgb(170, 185, 210),
        ),
        CharLifecycleState::Online => (
            Color32::from_rgb(20, 52, 82),
            Color32::from_rgb(100, 195, 255),
        ),
        CharLifecycleState::Unknown => (
            Color32::from_rgb(40, 40, 48),
            Color32::from_rgb(155, 155, 170),
        ),
    }
}

fn alert_colors(alert: AlertKind) -> (Color32, Color32) {
    match alert {
        AlertKind::Stale => (
            Color32::from_rgb(78, 52, 18),
            Color32::from_rgb(250, 190, 80),
        ),
        AlertKind::Disconnected | AlertKind::Dead => (
            Color32::from_rgb(90, 22, 26),
            Color32::from_rgb(255, 115, 115),
        ),
        AlertKind::Limit2B | AlertKind::Stopped => (
            Color32::from_rgb(82, 42, 16),
            Color32::from_rgb(255, 160, 75),
        ),
        AlertKind::Near2B => (
            Color32::from_rgb(68, 58, 16),
            Color32::from_rgb(255, 215, 85),
        ),
    }
}

fn render_confidence_cell(ui: &mut Ui, conf: Confidence, age_s: u64) {
    let col = match conf {
        Confidence::Verified => Color32::from_rgb(85, 225, 145),
        Confidence::Provisional => Color32::from_rgb(245, 200, 80),
        Confidence::Stale => Color32::from_rgb(255, 140, 70),
        Confidence::Unavailable => Color32::from_rgb(140, 150, 165),
    };
    ui.colored_label(col, format!("{} (há {}s)", conf.label_pt(), age_s));
}

fn render_zen_sparkline(ui: &mut Ui, points: &[(u64, u64)]) {
    let desired = Vec2::new(ui.available_width(), 74.0);
    let (rect, _) = ui.allocate_exact_size(desired, egui::Sense::hover());
    let painter = ui.painter_at(rect);

    painter.rect_filled(rect, 4.0, Color32::from_rgb(11, 15, 22));
    painter.rect_stroke(
        rect,
        4.0,
        Stroke::new(1.0_f32, Color32::from_rgb(38, 50, 70)),
        egui::StrokeKind::Middle,
    );

    if points.len() < 2 {
        painter.text(
            rect.center(),
            egui::Align2::CENTER_CENTER,
            "Coletando amostras de Zen...",
            FontId::proportional(11.0),
            Color32::from_rgb(140, 150, 165),
        );
        return;
    }

    let min_z = points.iter().map(|(_, z)| *z).min().unwrap_or(0);
    let max_z = points
        .iter()
        .map(|(_, z)| *z)
        .max()
        .unwrap_or(1)
        .max(min_z + 1000);

    let pad = 8.0;
    let inner = rect.shrink(pad);
    let n = (points.len() - 1) as f32;

    let mut screen_pts = Vec::with_capacity(points.len());
    for (i, &(_, z)) in points.iter().enumerate() {
        let t = i as f32 / n;
        let x = inner.left() + t * inner.width();
        let norm_y = (z.saturating_sub(min_z)) as f32 / (max_z - min_z) as f32;
        let y = inner.bottom() - norm_y * inner.height();
        screen_pts.push(Pos2::new(x, y));
    }

    for pair in screen_pts.windows(2) {
        painter.line_segment(
            [pair[0], pair[1]],
            Stroke::new(2.0_f32, Color32::from_rgb(75, 215, 145)),
        );
    }
    if let Some(&last_pt) = screen_pts.last() {
        painter.circle_filled(last_pt, 3.5, Color32::from_rgb(120, 255, 180));
    }
}
