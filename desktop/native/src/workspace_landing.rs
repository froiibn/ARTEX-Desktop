//! The closed-workspace start screen; actions are dispatched by NativeApp.
use crate::theme;
use eframe::egui::{self, RichText, Stroke};
use std::path::PathBuf;

pub(crate) enum Action {
    Create,
    Browse,
    Recent(PathBuf),
    Resume,
}

pub(crate) fn show(
    ui: &mut egui::Ui,
    name: &mut String,
    recent: &[PathBuf],
    suspended: bool,
    can_resume: bool,
    busy: bool,
) -> Option<Action> {
    let mut action = None;
    let width = ui.available_width().min(880.0);
    let top = ((ui.available_height() - 490.0) * 0.5).clamp(16.0, 72.0);
    let card_height = (ui.available_height() - top - 225.0).clamp(260.0, 330.0);
    let p = theme::palette(ui.visuals().dark_mode);
    egui::ScrollArea::vertical()
        .id_salt("workspace-start-screen")
        .auto_shrink([false, false])
        .show(ui, |ui| {
            ui.add_space(top);
            ui.vertical_centered(|ui| {
                ui.allocate_ui_with_layout(
                    egui::vec2(width, 0.0),
                    egui::Layout::top_down(egui::Align::Min),
                    |ui| {
                        ui.vertical_centered(|ui| {
                            ui.label(RichText::new("ARTEX").size(34.0).color(p.accent));
                            ui.heading("작업 공간 시작하기");
                            ui.label("새 공간을 만들거나, 기존 공간에서 작업을 이어가세요.");
                        });
                        ui.add_space(16.0);
                        ui.spacing_mut().item_spacing.x = 24.0;
                        ui.columns(2, |columns| {
                            card(&mut columns[0], card_height, |ui| {
                                ui.heading("새 작업 공간");
                                ui.weak("대화·작업·자산·증거를 함께 관리할 공간을 만듭니다.");
                                ui.add_space(24.0);
                                ui.label("작업 공간 이름");
                                ui.scope(|ui| {
                                    ui.visuals_mut().widgets.inactive.bg_stroke = Stroke::new(1.0_f32, p.border);
                                    ui.add_enabled(
                                        !busy,
                                        egui::TextEdit::singleline(name)
                                            .desired_width(ui.available_width())
                                            .margin(egui::vec2(10.0, 8.0))
                                            .hint_text("예: 내 서비스 보안 점검"),
                                    );
                                });
                                ui.add_space(14.0);
                                if ui
                                    .add_enabled(!busy, button(ui, "새 작업 공간 만들기", true))
                                    .on_hover_text("저장할 상위 폴더를 선택합니다.")
                                    .clicked()
                                {
                                    action = Some(Action::Create);
                                }
                                ui.add_space(8.0);
                                ui.weak("이름을 입력한 다음 저장할 폴더를 선택하세요.");
                            });
                            card(&mut columns[1], card_height, |ui| {
                                ui.heading("기존 작업 공간");
                                ui.weak("저장해 둔 대화와 분석 작업을 이어서 진행합니다.");
                                ui.add_space(16.0);
                                if ui
                                    .add_enabled(!busy, button(ui, "폴더에서 작업 공간 열기", false))
                                    .on_hover_text("Ctrl+O · 기존 작업 공간 폴더를 선택합니다.")
                                    .clicked()
                                {
                                    action = Some(Action::Browse);
                                }
                                ui.add_space(16.0);
                                ui.strong("최근 작업 공간");
                                egui::ScrollArea::vertical()
                                    .id_salt("recent-workspace-buttons")
                                    .max_height((card_height - 125.0).clamp(140.0, 250.0))
                                    .auto_shrink([false, true])
                                    .show(ui, |ui| {
                                        if recent.is_empty() {
                                            ui.weak("아직 열었던 작업 공간이 없습니다.");
                                        }
                                        for path in recent {
                                            let title = path
                                                .file_name()
                                                .unwrap_or_else(|| path.as_os_str())
                                                .to_string_lossy();
                                            let label = format!("{title}\n{}", path.display());
                                            if ui
                                                .add_enabled(
                                                    !busy,
                                                    button(ui, &label, false).min_size(egui::vec2(
                                                        ui.available_width(),
                                                        60.0,
                                                    )),
                                                )
                                                .on_hover_text(path.to_string_lossy())
                                                .clicked()
                                            {
                                                action = Some(Action::Recent(path.clone()));
                                            }
                                        }
                                    });
                            });
                        });
                        if suspended {
                            ui.add_space(18.0);
                            ui.colored_label(p.warning, "절전으로 실행을 중지했습니다. 저장된 공간을 다시 열어 이어갈 수 있습니다.");
                            if ui
                                .add_enabled(can_resume && !busy, button(ui, "저장된 작업 공간 다시 열기", false))
                                .clicked()
                            {
                                action = Some(Action::Resume);
                            }
                        }
                        if busy {
                            ui.add_space(16.0);
                            ui.horizontal(|ui| {
                                ui.spinner();
                                ui.label("작업 공간을 준비하고 있습니다…");
                            });
                        }
                    },
                );
            });
            ui.add_space(24.0);
        });
    action
}

fn card(ui: &mut egui::Ui, height: f32, contents: impl FnOnce(&mut egui::Ui)) {
    let width = ui.available_width();
    let p = theme::palette(ui.visuals().dark_mode);
    egui::Frame::new()
        .fill(p.panel)
        .stroke(Stroke::new(1.0_f32, p.border.gamma_multiply(0.6)))
        .corner_radius(14)
        .inner_margin(22)
        .show(ui, |ui| {
            ui.set_width((width - 44.0).max(1.0));
            ui.set_min_height(height);
            contents(ui);
        });
}

fn button(ui: &egui::Ui, label: &str, primary: bool) -> egui::Button<'static> {
    let p = theme::palette(ui.visuals().dark_mode);
    egui::Button::new(RichText::new(label.to_owned()).color(if primary {
        p.selection_text
    } else {
        p.text
    }))
    .frame(true)
    .fill(if primary { p.selection } else { p.surface })
    .stroke(Stroke::new(
        1.0_f32,
        if primary { p.accent } else { p.border },
    ))
    .corner_radius(8)
    .min_size(egui::vec2(ui.available_width(), 42.0))
    .wrap()
}
