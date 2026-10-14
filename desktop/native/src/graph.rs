use crate::model::display_name;
use artex_core::{Record, Snapshot};
use eframe::egui::{self, Pos2, Rect, Sense, Stroke, StrokeKind, Vec2};
use std::collections::{BTreeMap, BTreeSet};

pub struct Graph {
    positions: BTreeMap<String, Pos2>,
    offset: Vec2,
    zoom: f32,
    filter: String,
    page: usize,
}
impl Default for Graph {
    fn default() -> Self {
        Self {
            positions: BTreeMap::new(),
            offset: Vec2::new(90.0, 70.0),
            zoom: 1.0,
            filter: String::new(),
            page: 0,
        }
    }
}
impl Graph {
    pub fn show(&mut self, ui: &mut egui::Ui, snapshot: &Snapshot, selected: &mut Option<String>) {
        ui.horizontal(|ui| {
            ui.label("관계 탐색");
            if ui.text_edit_singleline(&mut self.filter).changed() {
                self.page = 0;
            }
            if ui.button("뷰 초기화").clicked() {
                self.zoom = 1.0;
                self.offset = Vec2::new(90.0, 70.0);
            }
            ui.small("드래그: 노드 이동 · 빈 영역: 화면 이동 · 휠: 확대");
        });
        let filter = self.filter.to_lowercase();
        let matching: Vec<&Record> = [
            "companies",
            "tasks",
            "explorations",
            "exploration_nodes",
            "assets",
            "findings",
            "agents",
            "agent_runs",
        ]
        .into_iter()
        .flat_map(|c| snapshot.records(c))
        .filter(|r| {
            filter.is_empty()
                || display_name(r).to_lowercase().contains(&filter)
                || r.header.id.to_lowercase().contains(&filter)
        })
        .collect();
        let pages = matching.len().div_ceil(250).max(1);
        self.page = self.page.min(pages - 1);
        ui.horizontal(|ui| {
            if ui
                .add_enabled(self.page > 0, egui::Button::new("이전 객체 묶음"))
                .clicked()
            {
                self.page -= 1;
            }
            ui.label(format!(
                "{}개 객체 · {}/{} · 묶음당 최대 250개",
                matching.len(),
                self.page + 1,
                pages
            ));
            if ui
                .add_enabled(self.page + 1 < pages, egui::Button::new("다음 객체 묶음"))
                .clicked()
            {
                self.page += 1;
            }
            ui.small(
                "표시된 객체 사이의 연결을 그립니다. 이름 또는 ID로 전체를 검색할 수 있습니다.",
            );
        });
        let nodes: Vec<&Record> = matching
            .into_iter()
            .skip(self.page * 250)
            .take(250)
            .collect();
        if nodes.is_empty() {
            ui.label("작업·자산·에이전트를 등록하면 연결된 객체가 표시됩니다.");
            return;
        }
        let (response, painter) = ui.allocate_painter(
            ui.available_size().max(Vec2::new(200.0, 200.0)),
            Sense::drag(),
        );
        let colors = crate::theme::palette(ui.visuals().dark_mode);
        painter.rect_filled(response.rect, 0.0_f32, colors.surface);
        if response.hovered() {
            let scroll = ui.input(|i| i.smooth_scroll_delta.y);
            if scroll.abs() > 0.0 {
                self.zoom = (self.zoom * (1.0 + scroll * 0.002)).clamp(0.25, 2.5);
            }
        }
        let mut ids = BTreeSet::new();
        for (index, node) in nodes.iter().enumerate() {
            ids.insert(node.header.id.as_str());
            self.positions
                .entry(node.header.id.clone())
                .or_insert(Pos2::new(
                    (index % 4) as f32 * 260.0,
                    (index / 4) as f32 * 105.0,
                ));
        }
        let center = |id: &str, positions: &BTreeMap<String, Pos2>, offset: Vec2, zoom: f32| {
            positions
                .get(id)
                .map(|p| response.rect.min + offset + p.to_vec2() * zoom)
        };
        for edge in snapshot.records("exploration_edges") {
            if let (Some(from), Some(to)) =
                (edge.data["src_id"].as_str(), edge.data["dst_id"].as_str())
            {
                if ids.contains(from) && ids.contains(to) {
                    if let (Some(a), Some(b)) = (
                        center(from, &self.positions, self.offset, self.zoom),
                        center(to, &self.positions, self.offset, self.zoom),
                    ) {
                        painter.arrow(a, b - a, Stroke::new(1.5_f32, colors.accent));
                    }
                }
            }
        }
        for node in &nodes {
            for value in node.data.as_object().into_iter().flat_map(|o| o.values()) {
                for target in value.as_str().into_iter().chain(
                    value
                        .as_array()
                        .into_iter()
                        .flatten()
                        .filter_map(serde_json::Value::as_str),
                ) {
                    if !ids.contains(target) {
                        continue;
                    }
                    if let (Some(a), Some(b)) = (
                        center(&node.header.id, &self.positions, self.offset, self.zoom),
                        center(target, &self.positions, self.offset, self.zoom),
                    ) {
                        painter.line_segment([a, b], Stroke::new(1.0_f32, colors.muted));
                    }
                }
            }
        }
        let mut dragging_node = false;
        for node in &nodes {
            let point = center(&node.header.id, &self.positions, self.offset, self.zoom).unwrap();
            let rect = Rect::from_center_size(point, Vec2::new(218.0, 72.0) * self.zoom);
            let interaction =
                ui.interact(rect, ui.id().with(&node.header.id), Sense::click_and_drag());
            if interaction.clicked() {
                *selected = Some(node.header.id.clone());
            }
            if interaction.dragged() {
                dragging_node = true;
                if let Some(position) = self.positions.get_mut(&node.header.id) {
                    *position += ui.input(|i| i.pointer.delta()) / self.zoom;
                }
            }
            let active = selected.as_deref() == Some(&node.header.id);
            let tint = if active {
                colors.selection
            } else {
                colors.window
            };
            painter.rect_filled(rect, 3.0_f32, tint);
            painter.rect_stroke(
                rect,
                3.0_f32,
                Stroke::new(
                    if active { 2.0_f32 } else { 1.0_f32 },
                    if active { colors.accent } else { colors.border },
                ),
                StrokeKind::Inside,
            );
            painter.text(
                rect.left_top() + Vec2::new(10.0, 10.0) * self.zoom,
                egui::Align2::LEFT_TOP,
                display_name(node).chars().take(22).collect::<String>(),
                egui::FontId::proportional(13.0 * self.zoom),
                if active {
                    colors.selection_text
                } else {
                    colors.text
                },
            );
            painter.text(
                rect.left_bottom() + Vec2::new(10.0, -12.0) * self.zoom,
                egui::Align2::LEFT_BOTTOM,
                &node.header.collection,
                egui::FontId::proportional(10.0 * self.zoom),
                if active {
                    colors.selection_text
                } else {
                    colors.muted
                },
            );
        }
        if response.dragged() && !dragging_node {
            self.offset += ui.input(|i| i.pointer.delta());
        }
    }
}
