//! Integrated native-window presentation; application shutdown remains owned
//! by NativeApp. No control here terminates the process or closes a workspace.
use crate::theme;
use eframe::egui::{self, Color32, Rect, ResizeDirection, Stroke, ViewportCommand};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Platform {
    Windows,
    Mac,
    Linux,
}
impl Platform {
    pub(crate) fn shortcut(self, key: &str) -> String {
        format!("{}+{key}", if self == Self::Mac { "⌘" } else { "Ctrl" })
    }
    pub(crate) fn current() -> Self {
        if cfg!(target_os = "macos") {
            Self::Mac
        } else if cfg!(target_os = "windows") {
            Self::Windows
        } else {
            Self::Linux
        }
    }
}

/// macOS retains a transparent native titlebar because winit 0.30.13 cannot
/// start a native resize drag there. Its native edges remain resizable.
pub(crate) fn viewport(platform: Platform, narrow: bool) -> egui::ViewportBuilder {
    let builder = egui::ViewportBuilder::default()
        .with_title("ARTEX")
        .with_inner_size(if narrow {
            [900.0, 620.0]
        } else {
            [1320.0, 860.0]
        })
        .with_min_inner_size([900.0, 620.0])
        .with_resizable(true);
    if platform == Platform::Mac {
        builder
            .with_decorations(true)
            .with_fullsize_content_view(true)
            .with_title_shown(false)
            .with_titlebar_buttons_shown(false)
            .with_titlebar_shown(false)
            .with_movable_by_background(false)
    } else {
        builder.with_decorations(false)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum WindowAction {
    Minimize,
    SetMaximized(bool),
    StartDrag,
    CloseRequested,
}
pub(crate) fn dispatch_action(ctx: &egui::Context, action: WindowAction) {
    let command = match action {
        WindowAction::Minimize => Some(ViewportCommand::Minimized(true)),
        WindowAction::SetMaximized(value) => Some(ViewportCommand::Maximized(value)),
        WindowAction::StartDrag => Some(ViewportCommand::StartDrag),
        // The caller routes this through the same deferred close/close-to-tray
        // decision as an OS close request, including checkpoint failures.
        WindowAction::CloseRequested => None,
    };
    if let Some(command) = command {
        ctx.send_viewport_cmd(command);
    }
}

pub(crate) struct TitlebarOutput {
    pub actions: Vec<WindowAction>,
    pub rect: Rect,
    pub menu_rect: Rect,
    pub drag_rect: Rect,
    /// Close, minimize, maximize/restore, independent of platform placement.
    pub controls: [Rect; 3],
}

fn control(
    ui: &mut egui::Ui,
    rect: Rect,
    platform: Platform,
    index: usize,
    maximized: bool,
) -> egui::Response {
    let label = match index {
        0 => "닫기",
        1 => "최소화",
        _ => {
            if maximized {
                "복원"
            } else {
                "최대화"
            }
        }
    };
    let response = ui.interact(
        rect,
        ui.id().with(("window-control", index)),
        egui::Sense::click(),
    );
    response.widget_info(|| {
        egui::WidgetInfo::labeled(egui::WidgetType::Button, ui.is_enabled(), label)
    });
    let p = theme::palette(ui.visuals().dark_mode);
    let painter = ui.painter();
    let center = rect.center();
    let color = if platform == Platform::Mac {
        let color = match index {
            0 => Color32::from_rgb(255, 95, 87),
            1 => Color32::from_rgb(254, 188, 46),
            _ => Color32::from_rgb(40, 200, 64),
        };
        painter.circle_filled(center, 6.0, color);
        Color32::from_rgb(38, 43, 47)
    } else {
        if response.hovered() || response.has_focus() {
            painter.rect_filled(
                rect,
                4,
                if index == 0 {
                    p.error.gamma_multiply(0.22)
                } else {
                    p.surface
                },
            );
        }
        p.text
    };
    if platform != Platform::Mac || response.hovered() || response.has_focus() {
        let half = if platform == Platform::Mac { 3.0 } else { 4.5 };
        let stroke = Stroke::new(1.3_f32, color);
        match index {
            0 => {
                painter.line_segment(
                    [
                        center - egui::vec2(half, half),
                        center + egui::vec2(half, half),
                    ],
                    stroke,
                );
                painter.line_segment(
                    [
                        center + egui::vec2(-half, half),
                        center + egui::vec2(half, -half),
                    ],
                    stroke,
                );
            }
            1 => {
                painter.line_segment(
                    [
                        center - egui::vec2(half, 0.0),
                        center + egui::vec2(half, 0.0),
                    ],
                    stroke,
                );
            }
            _ => {
                let box_rect = Rect::from_center_size(center, egui::Vec2::splat(half * 2.0));
                if maximized {
                    painter.rect_stroke(
                        box_rect.translate(egui::vec2(2.0, -2.0)),
                        0,
                        stroke,
                        egui::StrokeKind::Inside,
                    );
                }
                painter.rect_stroke(box_rect, 0, stroke, egui::StrokeKind::Inside);
            }
        }
    }
    response.on_hover_text(label)
}

/// The callback renders the existing MenuBar once. Only the blank/title area
/// is draggable; menus and window buttons never overlap it.
pub(crate) fn show_titlebar(
    ctx: &egui::Context,
    platform: Platform,
    menus: impl FnOnce(&mut egui::Ui),
) -> TitlebarOutput {
    let maximized = ctx.input(|i| i.viewport().maximized.unwrap_or(false));
    let mut output = TitlebarOutput {
        actions: vec![],
        rect: Rect::NOTHING,
        menu_rect: Rect::NOTHING,
        drag_rect: Rect::NOTHING,
        controls: [Rect::NOTHING; 3],
    };
    let panel = egui::TopBottomPanel::top("desktop_menu_bar")
        .exact_height(40.0)
        .frame(
            egui::Frame::NONE
                .fill(theme::palette(ctx.theme() == egui::Theme::Dark).panel)
                .inner_margin(egui::Margin::symmetric(8, 4)),
        )
        .show(ctx, |ui| {
            let area = ui.max_rect();
            let cell_width = if platform == Platform::Mac {
                22.0
            } else {
                38.0
            };
            let group_width = cell_width * 3.0;
            let control_left = if platform == Platform::Mac {
                area.left()
            } else {
                area.right() - group_width
            };
            // Mac: close/minimize/zoom on left. Others: minimize/maximize/close on right.
            let order = if platform == Platform::Mac {
                [0, 1, 2]
            } else {
                [1, 2, 0]
            };
            for (column, index) in order.into_iter().enumerate() {
                let rect = Rect::from_min_size(
                    egui::pos2(control_left + column as f32 * cell_width, area.top()),
                    egui::vec2(cell_width, 32.0),
                );
                output.controls[index] = rect;
                if control(ui, rect, platform, index, maximized).clicked() {
                    output.actions.push(match index {
                        0 => WindowAction::CloseRequested,
                        1 => WindowAction::Minimize,
                        _ => WindowAction::SetMaximized(!maximized),
                    });
                }
            }
            let menu_left = if platform == Platform::Mac {
                control_left + group_width + 12.0
            } else {
                area.left()
            };
            let menu_bounds =
                Rect::from_min_size(egui::pos2(menu_left, area.top()), egui::vec2(300.0, 32.0));
            ui.scope_builder(
                egui::UiBuilder::new()
                    .max_rect(menu_bounds)
                    .layout(egui::Layout::left_to_right(egui::Align::Center)),
                menus,
            );
            output.menu_rect = menu_bounds;
            let drag_right = if platform == Platform::Mac {
                area.right()
            } else {
                control_left - 8.0
            };
            let drag_rect = Rect::from_min_max(
                egui::pos2(menu_bounds.right() + 8.0, area.top()),
                egui::pos2(drag_right, area.bottom()),
            );
            output.drag_rect = drag_rect;
            if drag_rect.is_positive() {
                let drag = ui.interact(
                    drag_rect,
                    ui.id().with("title-drag"),
                    egui::Sense::click_and_drag(),
                );
                ui.painter().text(
                    drag_rect.left_center() + egui::vec2(10.0, 0.0),
                    egui::Align2::LEFT_CENTER,
                    "ARTEX",
                    egui::FontId::proportional(14.0),
                    theme::muted(ui),
                );
                if drag.double_clicked_by(egui::PointerButton::Primary) {
                    output.actions.push(WindowAction::SetMaximized(!maximized));
                } else if drag.drag_started_by(egui::PointerButton::Primary) {
                    output.actions.push(WindowAction::StartDrag);
                }
            }
        });
    output.rect = panel.response.rect;
    output
}

fn edge_regions(rect: Rect) -> [(ResizeDirection, Rect); 8] {
    let t = 4.0;
    let c = 8.0;
    let r = |x0, y0, x1, y1| Rect::from_min_max(egui::pos2(x0, y0), egui::pos2(x1, y1));
    [
        (
            ResizeDirection::NorthWest,
            r(rect.left(), rect.top(), rect.left() + c, rect.top() + c),
        ),
        (
            ResizeDirection::NorthEast,
            r(rect.right() - c, rect.top(), rect.right(), rect.top() + c),
        ),
        (
            ResizeDirection::SouthWest,
            r(
                rect.left(),
                rect.bottom() - c,
                rect.left() + c,
                rect.bottom(),
            ),
        ),
        (
            ResizeDirection::SouthEast,
            r(
                rect.right() - c,
                rect.bottom() - c,
                rect.right(),
                rect.bottom(),
            ),
        ),
        (
            ResizeDirection::North,
            r(
                rect.left() + c,
                rect.top(),
                rect.right() - c,
                rect.top() + t,
            ),
        ),
        (
            ResizeDirection::South,
            r(
                rect.left() + c,
                rect.bottom() - t,
                rect.right() - c,
                rect.bottom(),
            ),
        ),
        (
            ResizeDirection::West,
            r(
                rect.left(),
                rect.top() + c,
                rect.left() + t,
                rect.bottom() - c,
            ),
        ),
        (
            ResizeDirection::East,
            r(
                rect.right() - t,
                rect.top() + c,
                rect.right(),
                rect.bottom() - c,
            ),
        ),
    ]
}
/// Windows/X11/Wayland forward native resize gestures on the initial press.
/// macOS uses its retained native resize border instead of an unsupported call.
pub(crate) fn resize_edges(ctx: &egui::Context, platform: Platform) {
    if platform == Platform::Mac
        || ctx.input(|i| {
            i.viewport().maximized.unwrap_or(false) || i.viewport().fullscreen.unwrap_or(false)
        })
    {
        return;
    }
    for (index, (direction, rect)) in edge_regions(ctx.content_rect()).into_iter().enumerate() {
        // Area registers its own hit region before the invisible first sizing
        // pass. Using a child button instead would miss the first native press.
        let area = egui::Area::new(egui::Id::new(("native-window-edge", index)))
            .order(egui::Order::Foreground)
            .fixed_pos(rect.min)
            .default_size(rect.size())
            .movable(false)
            .constrain(false)
            .sense(egui::Sense::click_and_drag())
            .show(ctx, |ui| {
                let _ = ui.allocate_exact_size(rect.size(), egui::Sense::hover());
            });
        if area.response.hovered() {
            ctx.set_cursor_icon(match direction {
                ResizeDirection::North | ResizeDirection::South => egui::CursorIcon::ResizeVertical,
                ResizeDirection::East | ResizeDirection::West => egui::CursorIcon::ResizeHorizontal,
                ResizeDirection::NorthEast | ResizeDirection::SouthWest => {
                    egui::CursorIcon::ResizeNeSw
                }
                _ => egui::CursorIcon::ResizeNwSe,
            });
        }
        if area.response.is_pointer_button_down_on()
            && ctx.input(|i| i.pointer.button_pressed(egui::PointerButton::Primary))
        {
            ctx.send_viewport_cmd(ViewportCommand::BeginResize(direction));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn frame(
        ctx: &egui::Context,
        platform: Platform,
        events: Vec<egui::Event>,
        time: f64,
        maximized: bool,
    ) -> (TitlebarOutput, egui::FullOutput) {
        let mut raw = egui::RawInput {
            screen_rect: Some(Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(900.0, 620.0),
            )),
            events,
            time: Some(time),
            ..Default::default()
        };
        raw.viewports
            .get_mut(&egui::ViewportId::ROOT)
            .unwrap()
            .maximized = Some(maximized);
        let mut chrome = None;
        let output = ctx.run(raw, |ctx| {
            let result = show_titlebar(ctx, platform, |ui| {
                egui::MenuBar::new().ui(ui, |ui| {
                    for label in ["파일", "보기", "설정", "도움말"] {
                        let _ = ui.menu_button(label, |ui| {
                            ui.label("Action");
                        });
                    }
                });
            });
            for action in &result.actions {
                dispatch_action(ctx, *action);
            }
            chrome = Some(result);
            egui::CentralPanel::default().show(ctx, |ui| {
                ui.label("Content");
            });
            resize_edges(ctx, platform);
        });
        (chrome.unwrap(), output)
    }
    fn click(
        ctx: &egui::Context,
        platform: Platform,
        pos: egui::Pos2,
        time: f64,
        maximized: bool,
    ) -> (TitlebarOutput, egui::FullOutput) {
        let event = |pressed| egui::Event::PointerButton {
            pos,
            button: egui::PointerButton::Primary,
            pressed,
            modifiers: egui::Modifiers::NONE,
        };
        let _ = frame(
            ctx,
            platform,
            vec![egui::Event::PointerMoved(pos), event(true)],
            time,
            maximized,
        );
        frame(ctx, platform, vec![event(false)], time + 0.02, maximized)
    }
    #[test]
    fn platform_configuration_keeps_minimum_and_macos_native_resize_without_background_drag() {
        for platform in [Platform::Windows, Platform::Linux, Platform::Mac] {
            let view = viewport(platform, true);
            assert_eq!(view.min_inner_size, Some(egui::vec2(900.0, 620.0)));
            assert_eq!(view.inner_size, Some(egui::vec2(900.0, 620.0)));
            assert_eq!(view.resizable, Some(true));
            assert_eq!(view.decorations, Some(platform == Platform::Mac));
            if platform == Platform::Mac {
                assert_eq!(view.fullsize_content_view, Some(true));
                assert_eq!(view.titlebar_shown, Some(false));
                assert_eq!(view.titlebar_buttons_shown, Some(false));
                assert_eq!(view.title_shown, Some(false));
                assert_eq!(view.movable_by_window_background, Some(false));
            }
        }
    }
    #[test]
    fn production_titlebar_geometry_controls_and_menu_never_overlap_drag_area() {
        for platform in [Platform::Windows, Platform::Linux, Platform::Mac] {
            let ctx = egui::Context::default();
            theme::apply(&ctx);
            let (bar, output) = frame(&ctx, platform, vec![], 0.0, false);
            assert!(bar.rect.height() <= 40.0 + 1.0 && bar.rect.width() >= 899.0);
            assert!(!bar.menu_rect.intersects(bar.drag_rect));
            for control in bar.controls {
                assert!(bar.rect.contains_rect(control));
                assert!(!control.intersects(bar.drag_rect));
                assert!(!control.intersects(bar.menu_rect));
            }
            if platform == Platform::Mac {
                assert!(bar.controls[0].center().x < bar.menu_rect.left());
            } else {
                assert!(bar.controls[0].center().x > bar.drag_rect.right());
            }
            assert!(output.shapes.iter().any(
                |shape| matches!(&shape.shape,egui::Shape::Text(t) if t.galley.text()=="ARTEX")
            ));
        }
    }
    #[test]
    fn actual_control_clicks_minimize_restore_and_request_deferred_close_only() {
        for platform in [Platform::Windows, Platform::Linux, Platform::Mac] {
            for (index, maximized, expected) in [
                (1, false, WindowAction::Minimize),
                (2, false, WindowAction::SetMaximized(true)),
                (2, true, WindowAction::SetMaximized(false)),
                (0, false, WindowAction::CloseRequested),
            ] {
                let ctx = egui::Context::default();
                theme::apply(&ctx);
                let (bar, _) = frame(&ctx, platform, vec![], 0.0, maximized);
                let (clicked, output) =
                    click(&ctx, platform, bar.controls[index].center(), 0.2, maximized);
                assert_eq!(clicked.actions, vec![expected]);
                let commands = &output.viewport_output[&egui::ViewportId::ROOT].commands;
                assert!(!commands.contains(&ViewportCommand::StartDrag));
                if expected == WindowAction::CloseRequested {
                    assert!(!commands.contains(&ViewportCommand::Close));
                } else {
                    assert!(commands.contains(&match expected {
                        WindowAction::Minimize => ViewportCommand::Minimized(true),
                        WindowAction::SetMaximized(value) => ViewportCommand::Maximized(value),
                        _ => unreachable!(),
                    }));
                }
            }
        }
    }
    #[test]
    fn blank_drag_double_click_and_menu_click_use_distinct_actual_hit_regions() {
        let ctx = egui::Context::default();
        theme::apply(&ctx);
        let (bar, _) = frame(&ctx, Platform::Windows, vec![], 0.0, false);
        let pos = bar.drag_rect.center();
        let _ = click(&ctx, Platform::Windows, pos, 0.1, false);
        let (double, output) = click(&ctx, Platform::Windows, pos, 0.25, false);
        assert_eq!(double.actions, vec![WindowAction::SetMaximized(true)]);
        assert!(
            output.viewport_output[&egui::ViewportId::ROOT]
                .commands
                .contains(&ViewportCommand::Maximized(true))
        );
        let ctx = egui::Context::default();
        theme::apply(&ctx);
        let (bar, _) = frame(&ctx, Platform::Windows, vec![], 0.0, false);
        let pos = bar.drag_rect.center();
        let _ = frame(
            &ctx,
            Platform::Windows,
            vec![
                egui::Event::PointerMoved(pos),
                egui::Event::PointerButton {
                    pos,
                    button: egui::PointerButton::Primary,
                    pressed: true,
                    modifiers: egui::Modifiers::NONE,
                },
            ],
            0.1,
            false,
        );
        let (drag, output) = frame(
            &ctx,
            Platform::Windows,
            vec![egui::Event::PointerMoved(pos + egui::vec2(18.0, 0.0))],
            0.2,
            false,
        );
        assert_eq!(drag.actions, vec![WindowAction::StartDrag]);
        assert!(
            output.viewport_output[&egui::ViewportId::ROOT]
                .commands
                .contains(&ViewportCommand::StartDrag)
        );
        let ctx = egui::Context::default();
        theme::apply(&ctx);
        let (bar, _) = frame(&ctx, Platform::Windows, vec![], 0.0, false);
        let (menu, _) = click(
            &ctx,
            Platform::Windows,
            bar.menu_rect.left_center() + egui::vec2(25.0, 0.0),
            0.1,
            false,
        );
        assert!(menu.actions.is_empty());
    }
    #[test]
    fn edge_press_requests_native_resize_only_on_supported_platforms_and_never_when_maximized() {
        for platform in [Platform::Windows, Platform::Linux, Platform::Mac] {
            for maximized in [false, true] {
                let ctx = egui::Context::default();
                theme::apply(&ctx);
                let _ = frame(&ctx, platform, vec![], 0.0, maximized);
                let pos = egui::pos2(2.0, 310.0);
                let (_, output) = frame(
                    &ctx,
                    platform,
                    vec![
                        egui::Event::PointerMoved(pos),
                        egui::Event::PointerButton {
                            pos,
                            button: egui::PointerButton::Primary,
                            pressed: true,
                            modifiers: egui::Modifiers::NONE,
                        },
                    ],
                    0.1,
                    maximized,
                );
                let resized = output.viewport_output[&egui::ViewportId::ROOT]
                    .commands
                    .iter()
                    .any(|command| {
                        matches!(command, ViewportCommand::BeginResize(ResizeDirection::West))
                    });
                assert_eq!(resized, platform != Platform::Mac && !maximized);
            }
        }
    }

    #[test]
    fn every_edge_and_corner_handles_its_initial_press_without_resizing_menu_or_controls() {
        for platform in [Platform::Windows, Platform::Linux] {
            for (direction, region) in edge_regions(Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(900.0, 620.0),
            )) {
                let ctx = egui::Context::default();
                theme::apply(&ctx);
                let _ = frame(&ctx, platform, vec![], 0.0, false);
                let pos = region.center();
                let (_, output) = frame(
                    &ctx,
                    platform,
                    vec![
                        egui::Event::PointerMoved(pos),
                        egui::Event::PointerButton {
                            pos,
                            button: egui::PointerButton::Primary,
                            pressed: true,
                            modifiers: egui::Modifiers::NONE,
                        },
                    ],
                    0.1,
                    false,
                );
                let commands: Vec<_> = output.viewport_output[&egui::ViewportId::ROOT]
                    .commands
                    .iter()
                    .filter(|cmd| matches!(cmd, ViewportCommand::BeginResize(_)))
                    .collect();
                assert_eq!(
                    commands,
                    vec![&ViewportCommand::BeginResize(direction)],
                    "{platform:?}: initial {direction:?} press was not routed once"
                );
            }
            let points = {
                let ctx = egui::Context::default();
                theme::apply(&ctx);
                let (bar, _) = frame(&ctx, platform, vec![], 0.0, false);
                [
                    bar.controls[0].center(),
                    bar.menu_rect.center(),
                    egui::pos2(450.0, 400.0),
                ]
            };
            for pos in points {
                let ctx = egui::Context::default();
                theme::apply(&ctx);
                let _ = frame(&ctx, platform, vec![], 0.0, false);
                let (_, output) = frame(
                    &ctx,
                    platform,
                    vec![
                        egui::Event::PointerMoved(pos),
                        egui::Event::PointerButton {
                            pos,
                            button: egui::PointerButton::Primary,
                            pressed: true,
                            modifiers: egui::Modifiers::NONE,
                        },
                    ],
                    0.1,
                    false,
                );
                assert!(
                    !output.viewport_output[&egui::ViewportId::ROOT]
                        .commands
                        .iter()
                        .any(|cmd| matches!(cmd, ViewportCommand::BeginResize(_))),
                    "menu, window button or body press started resizing"
                );
            }
        }
    }
}
