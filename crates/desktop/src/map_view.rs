//! Canvas nativo de Mapa 2D (`zengravity-desktop::map_view`).
//!
//! Renderiza usando `egui::Painter` acelerado via `wgpu`:
//! - Grid 256×256 do MU Online com Zoom (roda do mouse ou botões) e Pan (arrastar);
//! - Distinção explícita entre:
//!   - **Névoa / Não Observado**: áreas fora do raio de visão (`OCCUPIED_RADIUS = 12` tiles)
//!     de qualquer personagem nosso online naquele mapa. Nunca prometemos cobertura integral!
//!   - **Raio de Visão Ativo**: círculo/quadrado iluminado em torno de cada personagem online.
//! - Zonas seguras (`Safezone`), obstáculos/paredes conhecidas (`TerrainGrid`);
//! - Spots catalogados (`F1`/`F2`) com cor indicando status (`Nosso`, `Livre Confirmado`,
//!   `Ocupado por Externo`, `Não Observado`);
//! - Rotas planejadas A* (`planned_route`) com linhas tracejadas/direcionais;
//! - Entidades próximas (`Player`, `Monster`, `Npc`, `GoldenMonster`, `GoldenBoss`)
//!   com indicação visual distinta e idade da detecção.

use crate::state::DesktopClientState;
use eframe::egui::{self, Align2, Color32, FontId, Pos2, Rect, Response, Sense, Stroke, Ui, Vec2};
use zengravity_agent::terrain_loader::ClientTerrainLoader;
use zengravity_core::entities::EntityKind;
use zengravity_core::lifecycle::CharLifecycleState;
use zengravity_core::map::{WorldId, MAP_SIZE};
use zengravity_core::spots::{SpotCoord, SpotOccupancyStatus};
use zengravity_core::OCCUPIED_RADIUS;
use zengravity_protocol::Command;

pub struct Map2DViewport {
    pub selected_world: WorldId,
    /// Escala em pixels por tile (`1.5..=14.0`).
    pub zoom_px_per_tile: f32,
    /// Centro do mapa em coordenadas de tile `(0..=255, 0..=255)`.
    pub center_tile: (f32, f32),
    pub show_grid: bool,
    pub show_terrain_walls: bool,
    pub show_vision_radius: bool,
    pub show_routes: bool,
    pub show_entities: bool,
    pub hovered_tile: Option<(i32, i32)>,
    pub clicked_tile: Option<(i32, i32)>,
    pub new_friendly_input: String,
    pub terrain_loader: ClientTerrainLoader,
}

impl Default for Map2DViewport {
    fn default() -> Self {
        Self {
            selected_world: WorldId::Kanturu,
            zoom_px_per_tile: 2.6,
            center_tile: (128.0, 128.0),
            show_grid: true,
            show_terrain_walls: true,
            show_vision_radius: true,
            show_routes: true,
            show_entities: true,
            hovered_tile: None,
            clicked_tile: Some((123, 233)),
            new_friendly_input: String::new(),
            terrain_loader: ClientTerrainLoader::discover(None),
        }
    }
}

impl Map2DViewport {
    pub fn reset_view(&mut self) {
        self.zoom_px_per_tile = 2.6;
        self.center_tile = (128.0, 128.0);
    }

    pub fn focus_on_spot(&mut self, spot: SpotCoord) {
        self.selected_world = spot.world;
        self.center_tile = (spot.x as f32, spot.y as f32);
        self.zoom_px_per_tile = self.zoom_px_per_tile.max(4.2);
    }

    fn tile_to_screen(&self, rect: Rect, tile_x: f32, tile_y: f32) -> Pos2 {
        let center = rect.center();
        let dx = (tile_x - self.center_tile.0) * self.zoom_px_per_tile;
        let dy = (tile_y - self.center_tile.1) * self.zoom_px_per_tile;
        Pos2::new(center.x + dx, center.y + dy)
    }

    fn screen_to_tile(&self, rect: Rect, pos: Pos2) -> (i32, i32) {
        let center = rect.center();
        let tx = ((pos.x - center.x) / self.zoom_px_per_tile + self.center_tile.0).round() as i32;
        let ty = ((pos.y - center.y) / self.zoom_px_per_tile + self.center_tile.1).round() as i32;
        (tx, ty)
    }

    pub fn ui(
        &mut self,
        ui: &mut Ui,
        state: &DesktopClientState,
        selected_char: &mut Option<String>,
    ) -> Vec<Command> {
        let mut commands = Vec::new();
        // Barra superior de controles do Mapa 2D
        ui.horizontal_wrapped(|ui| {
            ui.label(
                egui::RichText::new("Mapa:")
                    .strong()
                    .color(Color32::from_rgb(200, 210, 225)),
            );
            for &world in &[
                WorldId::Kanturu,
                WorldId::Lorencia,
                WorldId::Arena,
                WorldId::Dungeon,
                WorldId::LostTower,
                WorldId::Aida,
                WorldId::Devias,
            ] {
                let count = state.characters_in_world(world).len();
                let label = if count > 0 {
                    format!("{} ({})", world.name(), count)
                } else {
                    world.name().to_string()
                };
                if ui
                    .selectable_label(self.selected_world == world, label)
                    .clicked()
                {
                    self.selected_world = world;
                }
            }

            ui.separator();
            if ui.button("Zoom +").clicked() {
                self.zoom_px_per_tile = (self.zoom_px_per_tile * 1.25).clamp(1.4, 16.0);
            }
            if ui.button("Zoom -").clicked() {
                self.zoom_px_per_tile = (self.zoom_px_per_tile / 1.25).clamp(1.4, 16.0);
            }
            if ui.button("Centralizar (128,128)").clicked() {
                self.reset_view();
            }

            ui.separator();
            ui.checkbox(&mut self.show_grid, "Grid");
            ui.checkbox(&mut self.show_terrain_walls, "Terreno (.att)");
            ui.checkbox(&mut self.show_vision_radius, "Raio de Visão Real (12t)");
            ui.checkbox(&mut self.show_routes, "Rotas A*");
            ui.checkbox(&mut self.show_entities, "Mobs/NPCs/Goldens");
        });

        // Aviso inegociável de honestidade epistêmica
        ui.add_space(4.0);
        egui::Frame::NONE
            .fill(Color32::from_rgb(24, 30, 42))
            .stroke(Stroke::new(1.0_f32, Color32::from_rgb(55, 75, 105)))
            .inner_margin(6.0)
            .corner_radius(4.0)
            .show(ui, |ui| {
                ui.horizontal_wrapped(|ui| {
                    ui.label(
                        egui::RichText::new("COBERTURA REAL DE OBSERVAÇÃO:")
                            .strong()
                            .color(Color32::from_rgb(110, 190, 255)),
                    );
                    ui.label(
                        egui::RichText::new(
                            "O cliente do jogo só expõe entidades próximas (raio de 12 tiles) de cada personagem online. \
                             Áreas fora dos círculos de visão aparecem como NÃO OBSERVADAS (nunca assumidas como vazias).",
                        )
                        .color(Color32::from_rgb(185, 195, 210)),
                    );
                });
            });
        ui.add_space(6.0);

        let avail = ui.available_size();
        let canvas_height = (avail.y - 235.0).max(240.0);
        let (response, painter) =
            ui.allocate_painter(Vec2::new(avail.x, canvas_height), Sense::click_and_drag());
        let rect = response.rect;

        self.handle_canvas_interaction(ui, &response, rect);

        // Fundo do canvas
        painter.rect_filled(rect, 6.0, Color32::from_rgb(11, 14, 20));

        // Borda do mundo 0..=255
        let p_min = self.tile_to_screen(rect, 0.0, 0.0);
        let p_max = self.tile_to_screen(rect, MAP_SIZE as f32, MAP_SIZE as f32);
        let world_rect = Rect::from_min_max(p_min, p_max).intersect(rect);

        // Área do mapa em névoa ("Não Observado" por padrão)
        painter.rect_filled(world_rect, 2.0, Color32::from_rgb(17, 21, 30));

        // Grid a cada 16 ou 32 tiles
        if self.show_grid {
            let step = if self.zoom_px_per_tile >= 4.0 { 16 } else { 32 };
            for t in (0..=256).step_by(step) {
                let px = self.tile_to_screen(rect, t as f32, 0.0).x;
                if px >= rect.left() && px <= rect.right() {
                    painter.line_segment(
                        [
                            Pos2::new(px, world_rect.top()),
                            Pos2::new(px, world_rect.bottom()),
                        ],
                        Stroke::new(1.0_f32, Color32::from_rgba_unmultiplied(60, 75, 95, 45)),
                    );
                }
                let py = self.tile_to_screen(rect, 0.0, t as f32).y;
                if py >= rect.top() && py <= rect.bottom() {
                    painter.line_segment(
                        [
                            Pos2::new(world_rect.left(), py),
                            Pos2::new(world_rect.right(), py),
                        ],
                        Stroke::new(1.0_f32, Color32::from_rgba_unmultiplied(60, 75, 95, 45)),
                    );
                }
            }
        }

        // Renderiza o terreno real do arquivo Data\WorldN\EncTerrainN.att (ou sintético se ausente)
        let terrain = self.terrain_loader.get_or_load(self.selected_world).clone();
        if self.show_terrain_walls {
            let step = if self.zoom_px_per_tile >= 3.0 { 1 } else { 2 };
            let tile_px = (self.zoom_px_per_tile * step as f32).max(1.0);
            for ty in (0..256).step_by(step) {
                for tx in (0..256).step_by(step) {
                    let pos = self.tile_to_screen(rect, tx as f32, ty as f32);
                    if !rect.contains(pos) {
                        continue;
                    }
                    let tile_rect = Rect::from_min_size(pos, Vec2::new(tile_px, tile_px));
                    if terrain.is_safezone(tx, ty) {
                        painter.rect_filled(
                            tile_rect,
                            0.0,
                            Color32::from_rgba_unmultiplied(35, 130, 85, 65),
                        );
                    } else if terrain.is_walkable(tx, ty) {
                        painter.rect_filled(
                            tile_rect,
                            0.0,
                            Color32::from_rgba_unmultiplied(34, 46, 66, 95),
                        );
                    }
                }
            }
        }

        if let Some((sx0, sy0, _, _)) = self.selected_world.safezone_bounds() {
            let s_pos = self.tile_to_screen(rect, sx0 as f32, sy0 as f32);
            if rect.contains(s_pos) {
                painter.text(
                    Pos2::new(s_pos.x + 2.0, s_pos.y + 2.0),
                    Align2::LEFT_TOP,
                    "SAFE ZONE",
                    FontId::proportional(10.5),
                    Color32::from_rgb(100, 235, 160),
                );
            }
        }

        let now_mono_s = state.central_mono_ms / 1000;

        // 1) Raio de visão de cada personagem nosso presente neste mapa
        let chars_here = state.characters_in_world(self.selected_world);
        if self.show_vision_radius {
            for ch in &chars_here {
                if let Some(pos) = ch.position.value {
                    let center = self.tile_to_screen(rect, pos.x as f32, pos.y as f32);
                    let radius_px = OCCUPIED_RADIUS as f32 * self.zoom_px_per_tile;
                    let fill_col = if ch.state == CharLifecycleState::Stale {
                        Color32::from_rgba_unmultiplied(160, 120, 40, 22)
                    } else {
                        Color32::from_rgba_unmultiplied(50, 145, 240, 28)
                    };
                    let stroke_col = if ch.state == CharLifecycleState::Stale {
                        Color32::from_rgba_unmultiplied(210, 160, 50, 110)
                    } else {
                        Color32::from_rgba_unmultiplied(75, 170, 255, 120)
                    };
                    painter.circle_filled(center, radius_px, fill_col);
                    painter.circle_stroke(center, radius_px, Stroke::new(1.0_f32, stroke_col));
                }
            }
        }

        // 2) Spots do catálogo neste mundo
        for entry in state
            .spot_directory
            .catalog
            .iter()
            .filter(|c| c.spot.world == self.selected_world)
        {
            let sp = entry.spot;
            let pos = self.tile_to_screen(rect, sp.x as f32, sp.y as f32);
            if !rect.contains(pos) {
                continue;
            }
            let occ = state.spot_directory.occupancy_status(sp, now_mono_s, 20);
            let (color, tag) = match occ {
                SpotOccupancyStatus::OwnedByUs { char_name, .. } => (
                    Color32::from_rgb(65, 195, 255),
                    format!("Spot ({},{}) [{}]", sp.x, sp.y, char_name),
                ),
                SpotOccupancyStatus::ConfirmedFree { .. } => (
                    Color32::from_rgb(55, 215, 125),
                    format!("Spot ({},{}) [LIVRE]", sp.x, sp.y),
                ),
                SpotOccupancyStatus::OccupiedByForeign { players, .. } => (
                    Color32::from_rgb(245, 85, 85),
                    format!("Spot ({},{}) [OCUPADO: {}]", sp.x, sp.y, players.join(",")),
                ),
                SpotOccupancyStatus::Unobserved => (
                    Color32::from_rgb(125, 135, 150),
                    format!("Spot ({},{}) [NÃO OBSERVADO]", sp.x, sp.y),
                ),
            };

            let r = Rect::from_center_size(pos, Vec2::splat(9.0));
            painter.rect_stroke(
                r,
                2.0,
                Stroke::new(1.6_f32, color),
                egui::StrokeKind::Middle,
            );
            painter.text(
                Pos2::new(pos.x + 7.0, pos.y - 7.0),
                Align2::LEFT_BOTTOM,
                tag,
                FontId::proportional(10.5),
                color,
            );
        }

        // 3) Rotas A* planejadas
        if self.show_routes {
            for ch in &chars_here {
                if ch.planned_route.is_empty() {
                    continue;
                }
                if let Some(start) = ch.position.value {
                    let mut prev = self.tile_to_screen(rect, start.x as f32, start.y as f32);
                    for &(rx, ry) in &ch.planned_route {
                        let next = self.tile_to_screen(rect, rx as f32, ry as f32);
                        painter.line_segment(
                            [prev, next],
                            Stroke::new(2.0_f32, Color32::from_rgb(250, 190, 60)),
                        );
                        prev = next;
                    }
                    painter.circle_filled(prev, 4.0, Color32::from_rgb(255, 210, 80));
                }
            }
        }

        // 4) Entidades próximas (Mobs, NPCs, Jogadores, Goldens) com idade da leitura
        if self.show_entities {
            for ch in &chars_here {
                let age_s = ch.nearby_entities.age_secs(state.central_mono_ms);
                if let Some(entities) = &ch.nearby_entities.value {
                    for ent in entities {
                        let p = self.tile_to_screen(rect, ent.x as f32, ent.y as f32);
                        if !rect.contains(p) {
                            continue;
                        }
                        match ent.kind {
                            EntityKind::Monster => {
                                painter.circle_filled(p, 3.2, Color32::from_rgb(225, 95, 95));
                                if self.zoom_px_per_tile >= 3.2 {
                                    painter.text(
                                        Pos2::new(p.x + 5.0, p.y),
                                        Align2::LEFT_CENTER,
                                        format!("{} ({}s)", ent.name, age_s),
                                        FontId::proportional(9.5),
                                        Color32::from_rgb(215, 140, 140),
                                    );
                                }
                            }
                            EntityKind::Npc => {
                                painter.rect_filled(
                                    Rect::from_center_size(p, Vec2::splat(6.5)),
                                    1.5,
                                    Color32::from_rgb(90, 220, 190),
                                );
                                painter.text(
                                    Pos2::new(p.x + 6.0, p.y),
                                    Align2::LEFT_CENTER,
                                    format!("NPC: {}", ent.name),
                                    FontId::proportional(10.0),
                                    Color32::from_rgb(120, 235, 210),
                                );
                            }
                            EntityKind::GoldenMonster | EntityKind::GoldenBoss => {
                                let gold_col = if ent.kind == EntityKind::GoldenBoss {
                                    Color32::from_rgb(255, 70, 200)
                                } else {
                                    Color32::from_rgb(255, 215, 40)
                                };
                                painter.circle_filled(p, 6.0, gold_col);
                                painter.circle_stroke(
                                    p,
                                    9.0,
                                    Stroke::new(1.5_f32, Color32::from_rgb(255, 240, 140)),
                                );
                                painter.text(
                                    Pos2::new(p.x + 11.0, p.y),
                                    Align2::LEFT_CENTER,
                                    format!("★ {} [há {}s]", ent.name, age_s),
                                    FontId::proportional(11.5),
                                    gold_col,
                                );
                            }
                            EntityKind::Player => {
                                painter.circle_filled(p, 4.5, Color32::from_rgb(255, 145, 65));
                                painter.text(
                                    Pos2::new(p.x + 6.0, p.y),
                                    Align2::LEFT_CENTER,
                                    &ent.name,
                                    FontId::proportional(10.5),
                                    Color32::from_rgb(255, 175, 110),
                                );
                            }
                        }
                    }
                }
            }
        }

        // 5) Personagens do nosso grupo presentes neste mapa
        for ch in &chars_here {
            if let Some(pos) = ch.position.value {
                let p = self.tile_to_screen(rect, pos.x as f32, pos.y as f32);
                let is_sel = selected_char.as_deref() == Some(ch.name.as_str());
                let fill = match ch.state {
                    CharLifecycleState::Farming => Color32::from_rgb(45, 215, 125),
                    CharLifecycleState::Recovering => Color32::from_rgb(245, 185, 55),
                    CharLifecycleState::Dead | CharLifecycleState::Disconnected => {
                        Color32::from_rgb(245, 75, 75)
                    }
                    CharLifecycleState::Stale => Color32::from_rgb(215, 150, 60),
                    _ => Color32::from_rgb(90, 175, 250),
                };

                if is_sel {
                    painter.circle_stroke(p, 10.0, Stroke::new(2.0_f32, Color32::WHITE));
                }
                painter.circle_filled(p, 6.0, fill);
                painter.circle_stroke(p, 6.0, Stroke::new(1.5_f32, Color32::BLACK));
                painter.text(
                    Pos2::new(p.x, p.y - 10.0),
                    Align2::CENTER_BOTTOM,
                    format!("{} ({},{})", ch.name, pos.x, pos.y),
                    FontId::proportional(12.0),
                    Color32::WHITE,
                );

                // Clique sobre um personagem no mapa seleciona sua ficha
                if response.clicked() {
                    if let Some(click_pos) = response.interact_pointer_pos() {
                        if click_pos.distance(p) <= 14.0 {
                            *selected_char = Some(ch.name.clone());
                        }
                    }
                }
            }
        }

        // 6) Mira do tile selecionado com clique no mapa
        if let Some((cx, cy)) = self.clicked_tile {
            let cp = self.tile_to_screen(rect, cx as f32, cy as f32);
            if rect.contains(cp) {
                painter.circle_stroke(
                    cp,
                    8.0,
                    Stroke::new(1.8_f32, Color32::from_rgb(255, 230, 80)),
                );
                painter.line_segment(
                    [Pos2::new(cp.x - 11.0, cp.y), Pos2::new(cp.x + 11.0, cp.y)],
                    Stroke::new(1.4_f32, Color32::from_rgb(255, 230, 80)),
                );
                painter.line_segment(
                    [Pos2::new(cp.x, cp.y - 11.0), Pos2::new(cp.x, cp.y + 11.0)],
                    Stroke::new(1.4_f32, Color32::from_rgb(255, 230, 80)),
                );
            }
        }

        // Contorno do mundo 0..=255
        painter.rect_stroke(
            world_rect,
            2.0,
            Stroke::new(1.2_f32, Color32::from_rgb(75, 95, 130)),
            egui::StrokeKind::Middle,
        );

        // Rodapé do Mapa 2D com coordenadas sob o cursor, mira clicada e ações rápidas F1/F2
        ui.add_space(4.0);
        ui.horizontal_wrapped(|ui| {
            if let Some((tx, ty)) = self.hovered_tile {
                ui.label(
                    egui::RichText::new(format!(
                        "Cursor: {} ({}, {})",
                        self.selected_world.name(),
                        tx.clamp(0, 255),
                        ty.clamp(0, 255)
                    ))
                    .monospace()
                    .color(Color32::from_rgb(130, 210, 255)),
                );
            } else {
                ui.label(
                    egui::RichText::new("Arraste para mover | Roda do mouse: Zoom | Clique num tile para selecionar")
                        .color(Color32::from_rgb(150, 160, 175)),
                );
            }

            if let Some((cx, cy)) = self.clicked_tile {
                let target_spot = SpotCoord::new(self.selected_world, cx, cy);
                let walk_steps = terrain.walking_distance_from_entry(cx, cy);
                ui.separator();
                ui.label(
                    egui::RichText::new(format!(
                        "🎯 Tile: {} ({cx}, {cy}) [{}]",
                        self.selected_world.name(),
                        walk_steps
                            .map(|d| format!("{d} passos da Safe"))
                            .unwrap_or_else(|| "Bloqueado".into())
                    ))
                    .strong()
                    .color(Color32::from_rgb(255, 225, 95)),
                );

                if ui.button("📌 Catalogar Spot (F2)").clicked() {
                    commands.push(Command::AddCatalogSpot { spot: target_spot });
                }

                if let Some(sel_name) = selected_char.as_ref() {
                    if let Some(ch_view) = state.characters.get(sel_name) {
                        if ui
                            .button(format!("🏠 Definir como F1 de {sel_name}"))
                            .clicked()
                        {
                            commands.push(Command::AssignHomeSpot {
                                machine_id: ch_view.machine_id.clone(),
                                char_name: ch_view.name.clone(),
                                spot: target_spot,
                            });
                        }
                    }
                }
            }
        });

        ui.add_space(6.0);
        ui.separator();

        // Painel Inferior Fase 3: Catálogo Ordenado por Distância da Safe Zone + Friendly Chars
        ui.columns(2, |cols| {
            // Coluna 1: Catálogo de Spots deste Mapa com distância BFS real do EncTerrainN.att
            cols[0].label(
                egui::RichText::new(format!(
                    "CATÁLOGO DE SPOTS EM {} (ORDEM DE BUSCA PELA DISTÂNCIA DA SAFE ZONE)",
                    self.selected_world.name().to_uppercase()
                ))
                .small()
                .strong()
                .color(Color32::from_rgb(130, 185, 245)),
            );
            cols[0].add_space(3.0);

            let mut world_entries: Vec<_> = state
                .spot_directory
                .catalog
                .iter()
                .filter(|c| c.spot.world == self.selected_world)
                .map(|c| {
                    let dist = terrain.walking_distance_from_entry(c.spot.x, c.spot.y);
                    (c.clone(), dist)
                })
                .collect();
            world_entries.sort_by_key(|(c, d)| (d.unwrap_or(u32::MAX), c.priority, c.spot.x, c.spot.y));

            egui::ScrollArea::vertical()
                .id_salt("map_catalog_table_scroll")
                .max_height(135.0)
                .show(&mut cols[0], |ui| {
                    if world_entries.is_empty() {
                        ui.label("Nenhum spot catalogado neste mapa. Clique num tile acima e aperte 'Catalogar Spot (F2)'.");
                    } else {
                        egui::Grid::new("map_catalog_grid")
                            .num_columns(5)
                            .striped(true)
                            .spacing([10.0, 4.0])
                            .show(ui, |ui| {
                                ui.strong("#");
                                ui.strong("Coordenada");
                                ui.strong("Passos da Safe");
                                ui.strong("Ocupação / Dono");
                                ui.strong("Ações");
                                ui.end_row();

                                for (idx, (entry, dist_opt)) in world_entries.iter().enumerate() {
                                    ui.label(format!("#{}", idx + 1));
                                    ui.monospace(format!("({}, {})", entry.spot.x, entry.spot.y));
                                    ui.label(
                                        dist_opt
                                            .map(|d| format!("{d} tiles"))
                                            .unwrap_or_else(|| "Sem rota".into()),
                                    );

                                    let occ = state.spot_directory.occupancy_status(
                                        entry.spot,
                                        now_mono_s,
                                        20,
                                    );
                                    match occ {
                                        SpotOccupancyStatus::OwnedByUs { char_name, machine } => {
                                            ui.colored_label(
                                                Color32::from_rgb(80, 200, 255),
                                                format!("Nosso: {char_name} ({machine})"),
                                            );
                                        }
                                        SpotOccupancyStatus::ConfirmedFree { observed_by, age_s } => {
                                            ui.colored_label(
                                                Color32::from_rgb(75, 225, 135),
                                                format!("Livre (visto por {observed_by} há {age_s}s)"),
                                            );
                                        }
                                        SpotOccupancyStatus::OccupiedByForeign { players, .. } => {
                                            ui.colored_label(
                                                Color32::from_rgb(250, 95, 95),
                                                format!("Ocupado: {}", players.join(", ")),
                                            );
                                        }
                                        SpotOccupancyStatus::Unobserved => {
                                            ui.colored_label(
                                                Color32::from_rgb(145, 155, 170),
                                                "Não Observado",
                                            );
                                        }
                                    }

                                    ui.horizontal(|ui| {
                                        let toggle_lbl = if entry.enabled { "Ativo" } else { "Pulado" };
                                        if ui.small_button(toggle_lbl).clicked() {
                                            commands.push(Command::SetCatalogSpotEnabled {
                                                spot: entry.spot,
                                                enabled: !entry.enabled,
                                            });
                                        }
                                        if ui.small_button("Focar").clicked() {
                                            self.clicked_tile = Some((entry.spot.x, entry.spot.y));
                                            self.focus_on_spot(entry.spot);
                                        }
                                        if ui.small_button("🗑").clicked() {
                                            commands.push(Command::RemoveCatalogSpot {
                                                spot: entry.spot,
                                            });
                                        }
                                    });
                                    ui.end_row();
                                }
                            });
                    }
                });

            // Coluna 2: Personagens Aliados (`friendly_chars`) e Regras de Espaçamento
            cols[1].label(
                egui::RichText::new("PERSONAGENS ALIADOS (FRIENDLY CHARS) & ESPAÇAMENTO")
                    .small()
                    .strong()
                    .color(Color32::from_rgb(130, 185, 245)),
            );
            cols[1].add_space(3.0);
            cols[1].label(
                egui::RichText::new(
                    "Personagens de todas as máquinas conectadas já são reconhecidos como aliados automaticamente. \
                     Adicione abaixo amigos/contas extras para que não sejam contados como invasores no raio de 12 tiles.",
                )
                .small()
                .color(Color32::from_rgb(175, 185, 200)),
            );
            cols[1].add_space(4.0);

            cols[1].horizontal(|ui| {
                ui.label("Novo Aliado:");
                ui.text_edit_singleline(&mut self.new_friendly_input);
                if ui.button("+ Adicionar").clicked() && !self.new_friendly_input.trim().is_empty() {
                    commands.push(Command::AddFriendlyChar {
                        char_name: self.new_friendly_input.trim().to_string(),
                    });
                    self.new_friendly_input.clear();
                }
            });

            cols[1].add_space(4.0);
            cols[1].horizontal_wrapped(|ui| {
                if state.spot_directory.friendly_chars.is_empty() {
                    ui.colored_label(
                        Color32::from_rgb(145, 155, 170),
                        "Nenhum aliado externo extra cadastrado (apenas os personagens do grupo).",
                    );
                } else {
                    for friend in &state.spot_directory.friendly_chars {
                        egui::Frame::NONE
                            .fill(Color32::from_rgb(24, 34, 48))
                            .stroke(Stroke::new(1.0_f32, Color32::from_rgb(65, 115, 175)))
                            .corner_radius(3.0)
                            .inner_margin(egui::Margin::symmetric(6, 2))
                            .show(ui, |ui| {
                                ui.horizontal(|ui| {
                                    ui.label(
                                        egui::RichText::new(friend)
                                            .strong()
                                            .color(Color32::from_rgb(140, 215, 255)),
                                    );
                                    if ui.small_button("✖").clicked() {
                                        commands.push(Command::RemoveFriendlyChar {
                                            char_name: friend.clone(),
                                        });
                                    }
                                });
                            });
                    }
                }
            });
        });

        commands
    }

    fn handle_canvas_interaction(&mut self, ui: &Ui, response: &Response, rect: Rect) {
        if response.dragged() {
            let delta = response.drag_delta();
            self.center_tile.0 =
                (self.center_tile.0 - delta.x / self.zoom_px_per_tile).clamp(0.0, 255.0);
            self.center_tile.1 =
                (self.center_tile.1 - delta.y / self.zoom_px_per_tile).clamp(0.0, 255.0);
        }

        if response.clicked() {
            if let Some(click_pos) = response.interact_pointer_pos() {
                let (tx, ty) = self.screen_to_tile(rect, click_pos);
                if (0..=255).contains(&tx) && (0..=255).contains(&ty) {
                    self.clicked_tile = Some((tx, ty));
                }
            }
        }

        if let Some(hover_pos) = response.hover_pos() {
            self.hovered_tile = Some(self.screen_to_tile(rect, hover_pos));
            let scroll = ui.input(|i| i.smooth_scroll_delta.y);
            if scroll.abs() > 0.1 {
                let factor = if scroll > 0.0 { 1.12 } else { 1.0 / 1.12 };
                self.zoom_px_per_tile = (self.zoom_px_per_tile * factor).clamp(1.4, 16.0);
            }
        } else {
            self.hovered_tile = None;
        }
    }
}
