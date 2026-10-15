//! Soft native utility styling. Font installation remains with the app.
//! All symbols below are drawn shapes, independent of pictogram font coverage.
use eframe::egui::{
    self, Color32, CornerRadius, FontFamily, FontId, Frame, InnerResponse, Margin, Painter, Rect,
    Response, Sense, Shape, Stroke, StrokeKind, Style, TextStyle, Theme, Ui, Vec2,
};

#[derive(Clone, Copy, Debug)]
pub struct Palette {
    pub window: Color32,
    pub panel: Color32,
    pub surface: Color32,
    pub text: Color32,
    pub muted: Color32,
    pub border: Color32,
    pub accent: Color32,
    pub selection: Color32,
    pub selection_text: Color32,
    pub warning: Color32,
    pub error: Color32,
    pub success: Color32,
}

pub fn palette(dark: bool) -> Palette {
    if dark {
        Palette {
            window: Color32::from_rgb(27, 31, 38),
            panel: Color32::from_rgb(33, 38, 46),
            surface: Color32::from_rgb(42, 49, 59),
            text: Color32::from_rgb(232, 236, 242),
            muted: Color32::from_rgb(171, 183, 200),
            border: Color32::from_rgb(72, 85, 104),
            accent: Color32::from_rgb(132, 184, 246),
            selection: Color32::from_rgb(38, 92, 161),
            selection_text: Color32::WHITE,
            warning: Color32::from_rgb(237, 189, 105),
            error: Color32::from_rgb(249, 139, 139),
            success: Color32::from_rgb(122, 198, 151),
        }
    } else {
        Palette {
            window: Color32::from_rgb(245, 247, 250),
            panel: Color32::from_rgb(235, 240, 246),
            surface: Color32::from_rgb(255, 255, 255),
            text: Color32::from_rgb(32, 41, 54),
            muted: Color32::from_rgb(82, 96, 114),
            border: Color32::from_rgb(187, 198, 214),
            accent: Color32::from_rgb(31, 91, 163),
            selection: Color32::from_rgb(36, 94, 167),
            selection_text: Color32::WHITE,
            warning: Color32::from_rgb(135, 78, 10),
            error: Color32::from_rgb(179, 42, 42),
            success: Color32::from_rgb(30, 105, 58),
        }
    }
}

/// Configure both egui themes, so system-theme changes remain consistent.
/// Does not replace FontDefinitions, pixels-per-point, or renderer options.
pub fn apply(ctx: &egui::Context) {
    for theme in [Theme::Light, Theme::Dark] {
        ctx.style_mut_of(theme, |style| configure(style, theme == Theme::Dark));
    }
}

fn configure(style: &mut Style, dark: bool) {
    let p = palette(dark);
    let mut visuals = if dark {
        egui::Visuals::dark()
    } else {
        egui::Visuals::light()
    };
    visuals.override_text_color = None;
    visuals.weak_text_color = Some(p.muted);
    visuals.panel_fill = p.panel;
    visuals.window_fill = p.window;
    visuals.extreme_bg_color = if dark {
        Color32::from_rgb(23, 27, 33)
    } else {
        Color32::from_rgb(238, 242, 248)
    };
    visuals.text_edit_bg_color = Some(visuals.extreme_bg_color);
    visuals.faint_bg_color = if dark {
        Color32::from_rgb(34, 41, 51)
    } else {
        Color32::from_rgb(239, 244, 250)
    };
    visuals.code_bg_color = visuals.faint_bg_color;
    visuals.hyperlink_color = p.accent;
    visuals.warn_fg_color = p.warning;
    visuals.error_fg_color = p.error;
    visuals.window_corner_radius = CornerRadius::same(12);
    visuals.menu_corner_radius = CornerRadius::same(10);
    visuals.window_stroke = Stroke::new(1.0_f32, p.border);
    visuals.window_shadow = egui::epaint::Shadow::NONE;
    visuals.popup_shadow = egui::epaint::Shadow {
        offset: [0, 5],
        blur: 12,
        spread: 0,
        color: Color32::from_black_alpha(if dark { 72 } else { 28 }),
    };
    visuals.text_cursor.stroke = Stroke::new(1.5_f32, p.accent);
    visuals.interact_cursor = None;
    visuals.collapsing_header_frame = false;
    visuals.indent_has_left_vline = true;
    visuals.disabled_alpha = 0.48;
    let hover = if dark {
        Color32::from_rgb(53, 69, 89)
    } else {
        Color32::from_rgb(217, 232, 251)
    };
    // egui shares selection.stroke between selectable-widget text and the
    // focused TextEdit frame. A tinted selection keeps accent text readable
    // while giving every native input a visible keyboard-focus outline.
    // The dedicated primary button still uses the saturated palette.selection.
    visuals.selection.bg_fill = hover;
    visuals.selection.stroke = Stroke::new(1.5_f32, p.accent);
    for (widget, fill, border, text) in [
        (
            &mut visuals.widgets.noninteractive,
            p.panel,
            p.border,
            p.text,
        ),
        (&mut visuals.widgets.inactive, p.panel, p.border, p.text),
        (&mut visuals.widgets.hovered, hover, p.accent, p.text),
        // strong_text_color() also reads this foreground. Ordinary active
        // widgets must stay readable on neutral surfaces; selected widgets
        // obtain their tinted-blue treatment from visuals.selection instead.
        (&mut visuals.widgets.active, hover, p.accent, p.text),
        (&mut visuals.widgets.open, hover, p.accent, p.text),
    ] {
        widget.bg_fill = fill;
        widget.weak_bg_fill = fill;
        widget.bg_stroke = Stroke::new(1.0_f32, border);
        widget.fg_stroke = Stroke::new(1.0_f32, text);
        widget.corner_radius = CornerRadius::same(8);
        widget.expansion = 0.0;
    }
    // The hierarchy comes from surfaces and spacing. Only actual interaction
    // draws an accent outline; idle controls no longer repeat a box per widget.
    visuals.widgets.noninteractive.bg_stroke = Stroke::new(1.0_f32, p.border.gamma_multiply(0.4));
    visuals.widgets.inactive.bg_stroke = Stroke::NONE;
    visuals.widgets.hovered.bg_stroke = Stroke::NONE;
    visuals.widgets.active.bg_stroke = Stroke::new(1.5_f32, p.accent);
    visuals.widgets.open.bg_stroke = Stroke::new(1.0_f32, p.accent);
    style.visuals = visuals;
    style.spacing.item_spacing = egui::vec2(10.0, 9.0);
    style.spacing.button_padding = egui::vec2(12.0, 7.0);
    style.spacing.interact_size = egui::vec2(32.0, 32.0);
    style.spacing.window_margin = Margin::same(16);
    style.spacing.menu_margin = Margin::symmetric(10, 8);
    style.spacing.icon_width = 16.0;
    style.spacing.icon_width_inner = 10.0;
    style.spacing.icon_spacing = 6.0;
    style.spacing.indent = 18.0;
    style.spacing.scroll.bar_width = 10.0;
    style.spacing.scroll.floating = false;
    style
        .text_styles
        .insert(TextStyle::Body, FontId::new(14.0, FontFamily::Proportional));
    style.text_styles.insert(
        TextStyle::Button,
        FontId::new(14.0, FontFamily::Proportional),
    );
    style.text_styles.insert(
        TextStyle::Heading,
        FontId::new(20.0, FontFamily::Proportional),
    );
    style.text_styles.insert(
        TextStyle::Small,
        FontId::new(12.0, FontFamily::Proportional),
    );
    style.text_styles.insert(
        TextStyle::Monospace,
        FontId::new(13.0, FontFamily::Monospace),
    );
}

pub fn accent(ui: &Ui) -> Color32 {
    palette(ui.visuals().dark_mode).accent
}
pub fn muted(ui: &Ui) -> Color32 {
    palette(ui.visuals().dark_mode).muted
}

/// One shared label/input geometry keeps forms readable without forcing a wide window.
pub fn field_row<R>(ui: &mut Ui, label: &str, contents: impl FnOnce(&mut Ui) -> R) -> R {
    let width = ui.available_width().max(1.0);
    if width < 360.0 {
        ui.vertical(|ui| {
            ui.set_width(width);
            if !label.is_empty() {
                ui.label(label);
            }
            contents(ui)
        })
        .inner
    } else {
        let label_width = (width * 0.27).clamp(100.0, 156.0);
        ui.horizontal(|ui| {
            ui.add_sized([label_width, 28.0], egui::Label::new(label).wrap());
            ui.vertical(|ui| {
                ui.set_width((width - label_width - ui.spacing().item_spacing.x).max(1.0));
                contents(ui)
            })
            .inner
        })
        .inner
    }
}

/// Toolbars wrap naturally; unlike shrink-to-content child layouts they own the pane width.
pub fn toolbar<R>(ui: &mut Ui, contents: impl FnOnce(&mut Ui) -> R) -> InnerResponse<R> {
    toolbar_frame(ui.style()).show(ui, |ui| {
        ui.set_width(ui.available_width());
        ui.horizontal_wrapped(contents).inner
    })
}
pub fn text_field(ui: &mut Ui, label: &str, value: &mut String) -> Response {
    field_row(ui, label, |ui| {
        ui.add(egui::TextEdit::singleline(value).desired_width(f32::INFINITY))
    })
}

pub fn toolbar_frame(style: &Style) -> Frame {
    Frame::new()
        .fill(style.visuals.panel_fill)
        .inner_margin(Margin::symmetric(14, 7))
}
pub fn status_frame(style: &Style) -> Frame {
    Frame::new()
        .fill(style.visuals.panel_fill)
        .inner_margin(Margin::symmetric(14, 6))
}
pub fn surface_frame(style: &Style) -> Frame {
    Frame::new()
        .fill(palette(style.visuals.dark_mode).surface)
        .corner_radius(10)
        .inner_margin(16)
        .stroke(Stroke::NONE)
}
#[cfg(test)]
pub fn surface<R>(
    ui: &mut Ui,
    title: &str,
    contents: impl FnOnce(&mut Ui) -> R,
) -> InnerResponse<R> {
    group(ui, title, contents)
}
pub fn primary_button(ui: &mut Ui, label: &str) -> Response {
    let p = palette(ui.visuals().dark_mode);
    ui.scope(|ui| {
        let widgets = &mut ui.visuals_mut().widgets;
        for visual in [
            &mut widgets.inactive,
            &mut widgets.hovered,
            &mut widgets.active,
            &mut widgets.open,
        ] {
            visual.bg_fill = p.selection;
            visual.weak_bg_fill = p.selection;
            visual.fg_stroke = Stroke::new(1.0_f32, p.selection_text);
            visual.bg_stroke = Stroke::NONE;
        }
        widgets.hovered.bg_fill = p.selection.gamma_multiply(1.12);
        widgets.hovered.weak_bg_fill = widgets.hovered.bg_fill;
        widgets.active.bg_stroke = Stroke::new(1.5_f32, p.selection_text);
        ui.add(egui::Button::new(label))
    })
    .inner
}
pub fn secondary_button(ui: &mut Ui, label: &str) -> Response {
    let p = palette(ui.visuals().dark_mode);
    ui.add(
        egui::Button::new(label)
            .frame(true)
            .fill(p.surface)
            .stroke(Stroke::new(1.0_f32, p.border))
            .corner_radius(8),
    )
}
#[cfg(test)]
pub fn quiet_button(ui: &mut Ui, label: &str) -> Response {
    ui.scope(|ui| {
        ui.visuals_mut().widgets.inactive.weak_bg_fill = Color32::TRANSPARENT;
        ui.add(egui::Button::new(label))
    })
    .inner
}
#[derive(Clone, Copy, Debug)]
pub enum BadgeTone {
    Neutral,
    Info,
    Success,
    Warning,
    Error,
}
pub fn badge(ui: &mut Ui, label: &str, tone: BadgeTone) -> Response {
    let p = palette(ui.visuals().dark_mode);
    let color = match tone {
        BadgeTone::Neutral => p.muted,
        BadgeTone::Info => p.accent,
        BadgeTone::Success => p.success,
        BadgeTone::Warning => p.warning,
        BadgeTone::Error => p.error,
    };
    let mix = |base: u8, tint: u8| (f32::from(base) * 0.87 + f32::from(tint) * 0.13).round() as u8;
    let fill = Color32::from_rgb(
        mix(p.panel.r(), color.r()),
        mix(p.panel.g(), color.g()),
        mix(p.panel.b(), color.b()),
    );
    Frame::new()
        .fill(fill)
        .corner_radius(9)
        .inner_margin(Margin::symmetric(9, 4))
        .show(ui, |ui| {
            ui.label(egui::RichText::new(label).small().color(color))
        })
        .inner
}
pub fn group<R>(ui: &mut Ui, title: &str, contents: impl FnOnce(&mut Ui) -> R) -> InnerResponse<R> {
    surface_frame(ui.style()).show(ui, |ui| {
        if !title.is_empty() {
            ui.strong(title);
            ui.add_space(7.0);
        }
        contents(ui)
    })
}
pub fn tab(ui: &mut Ui, label: &str, selected: bool) -> Response {
    let response = ui.add(egui::Button::new(label).selected(selected).frame(false));
    if selected {
        let rect = response.rect;
        ui.painter().line_segment(
            [rect.left_bottom(), rect.right_bottom()],
            Stroke::new(2.0_f32, accent(ui)),
        );
    }
    response
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn selected_widgets_and_selected_input_text_remain_readable_in_both_themes() {
        for theme in [Theme::Light, Theme::Dark] {
            let ctx = egui::Context::default();
            ctx.set_theme(theme);
            apply(&ctx);
            let mut draft = "Retained selection".to_owned();
            let mut input_rect = Rect::NOTHING;
            let mut selected_rect = Rect::NOTHING;
            for pass in 0..2 {
                let output = ctx.run(
                    egui::RawInput {
                        screen_rect: Some(Rect::from_min_size(
                            egui::Pos2::ZERO,
                            egui::vec2(640.0, 480.0),
                        )),
                        ..Default::default()
                    },
                    |ctx| {
                        egui::CentralPanel::default().show(ctx, |ui| {
                            selected_rect = ui.selectable_label(true, "Selected item").rect;
                            let mut edit = egui::TextEdit::singleline(&mut draft)
                                .id_salt("contrast-selected-input")
                                .show(ui);
                            input_rect = edit.response.rect;
                            if pass == 0 {
                                edit.response.request_focus();
                                edit.state.cursor.set_char_range(Some(
                                    egui::text::CCursorRange::two(
                                        egui::text::CCursor::new(0),
                                        egui::text::CCursor::new(draft.chars().count()),
                                    ),
                                ));
                                edit.state.store(ctx, edit.response.id);
                            } else {
                                assert!(edit.response.has_focus());
                                assert!(edit.cursor_range.is_some_and(|range| !range.is_empty()));
                            }
                        });
                    },
                );
                if pass == 1 {
                    assert!(
                        output.shapes.iter().any(|clipped| {
                            matches!(&clipped.shape, Shape::Rect(rect)
                                if rect.fill == ctx.style().visuals.selection.bg_fill
                                    && selected_rect.intersects(rect.rect))
                        }),
                        "{theme:?}: selected widget did not paint its selection"
                    );
                    assert_rendered_text_contrast(&ctx, &output, "Selected item");
                    // TextEdit inserts selection triangles into the galley,
                    // not a separate Rect shape. Check those actual triangles
                    // and recolored glyph vertices on their real background.
                    let text = output
                        .shapes
                        .iter()
                        .find_map(|clipped| match &clipped.shape {
                            Shape::Text(text) if text.galley.text() == "Retained selection" => {
                                Some(text)
                            }
                            _ => None,
                        })
                        .expect("selected input produced no text shape");
                    let mut selected_glyphs = 0;
                    for row in &text.galley.rows {
                        let visuals = &row.row.visuals;
                        let mesh = &visuals.mesh;
                        let triangles =
                            &mesh.indices[visuals.glyph_index_start..visuals.glyph_index_start + 6];
                        assert!(
                            triangles.iter().all(|index| !visuals
                                .glyph_vertex_range
                                .contains(&(*index as usize))),
                            "selection background triangles were not painted before glyphs"
                        );
                        let fill = mesh.vertices[triangles[0] as usize].color;
                        assert!(
                            triangles
                                .iter()
                                .all(|index| mesh.vertices[*index as usize].color == fill)
                        );
                        for vertex in &mesh.vertices[visuals.glyph_vertex_range.clone()] {
                            let foreground = luminance(vertex.color);
                            let background = luminance(fill);
                            let ratio = (foreground.max(background) + 0.05)
                                / (foreground.min(background) + 0.05);
                            assert!(
                                ratio >= 4.5,
                                "{theme:?}: selected input glyph contrast {ratio:.2}"
                            );
                            selected_glyphs += 1;
                        }
                    }
                    assert!(selected_glyphs > 0);
                    assert!(input_rect.is_positive());
                }
            }
            assert_eq!(draft, "Retained selection");
        }
    }

    #[test]
    fn soft_surfaces_keep_keyboard_focus_visible_and_action_badge_text_readable() {
        for theme in [Theme::Light, Theme::Dark] {
            let ctx = egui::Context::default();
            ctx.set_theme(theme);
            apply(&ctx);
            let mut draft = "Retained input".to_owned();
            let mut input_rect = Rect::NOTHING;
            let mut focused = false;
            for pass in 0..2 {
                let output = ctx.run(
                    egui::RawInput {
                        screen_rect: Some(Rect::from_min_size(
                            egui::Pos2::ZERO,
                            egui::vec2(640.0, 480.0),
                        )),
                        ..Default::default()
                    },
                    |ctx| {
                        egui::CentralPanel::default().show(ctx, |ui| {
                            surface(ui, "Editor", |ui| {
                                let response = ui.add(
                                    egui::TextEdit::singleline(&mut draft)
                                        .id_salt("soft-focus-input"),
                                );
                                input_rect = response.rect;
                                focused = response.has_focus();
                                if pass == 0 {
                                    response.request_focus();
                                }
                                ui.horizontal(|ui| {
                                    let _ = primary_button(ui, "Send");
                                    let _ = quiet_button(ui, "Cancel");
                                    badge(ui, "Complete", BadgeTone::Success);
                                    badge(ui, "Idle", BadgeTone::Neutral);
                                    badge(ui, "Active", BadgeTone::Info);
                                    badge(ui, "Waiting", BadgeTone::Warning);
                                    badge(ui, "Failed", BadgeTone::Error);
                                });
                            });
                        });
                    },
                );
                if pass == 1 {
                    assert!(focused, "{theme:?}: keyboard focus was lost");
                    assert!(
                        output.shapes.iter().any(|clipped| {
                            if let Shape::Rect(rect) = &clipped.shape {
                                let outside = luminance(ctx.style().visuals.window_fill);
                                let ring = luminance(rect.stroke.color);
                                rect.rect.contains(input_rect.center())
                                    && rect.stroke.width >= 1.5
                                    && (outside.max(ring) + 0.05) / (outside.min(ring) + 0.05)
                                        >= 3.0
                            } else {
                                false
                            }
                        }),
                        "{theme:?}: focused input has no distinguishable focus ring"
                    );
                    for label in [
                        "Editor", "Send", "Cancel", "Complete", "Idle", "Active", "Waiting",
                        "Failed",
                    ] {
                        assert_rendered_text_contrast(&ctx, &output, label);
                    }
                    assert!(
                        Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(640.0, 480.0))
                            .contains_rect(input_rect)
                    );
                }
            }
            assert_eq!(draft, "Retained input");
        }
    }

    #[test]
    fn responsive_fields_and_wrapped_toolbar_stay_inside_the_central_pane() {
        for width in [300.0, 420.0, 820.0] {
            let ctx = egui::Context::default();
            apply(&ctx);
            let mut draft = "Preserved editor input".to_owned();
            let mut input_rect = Rect::NOTHING;
            let mut buttons = Vec::new();
            let _ = ctx.run(
                egui::RawInput {
                    screen_rect: Some(Rect::from_min_size(
                        egui::Pos2::ZERO,
                        egui::vec2(width + 16.0, 500.0),
                    )),
                    ..Default::default()
                },
                |ctx| {
                    egui::CentralPanel::default().show(ctx, |ui| {
                        // The same row geometry is shared by saved models and source-action forms.
                        field_row(ui, "Profile name", |ui| {
                            input_rect = ui
                                .add(
                                    egui::TextEdit::singleline(&mut draft)
                                        .desired_width(f32::INFINITY),
                                )
                                .rect;
                        });
                        toolbar(ui, |ui| {
                            for text in ["Save", "Reload", "Test connection"] {
                                buttons.push(ui.button(text).rect);
                            }
                        });
                        let right = ui.max_rect().right();
                        assert!(
                            input_rect.right() <= right + 1.0,
                            "Input exceeded {width}px central pane"
                        );
                        assert!(
                            buttons.iter().all(|r| r.right() <= right + 1.0),
                            "Toolbar exceeded {width}px central pane"
                        );
                    });
                },
            );
            assert_eq!(draft, "Preserved editor input");
            assert!(input_rect.width() > 120.0);
            assert!(
                (buttons[0].center().y - buttons[1].center().y).abs() < 1.0,
                "Buttons should share one row at {width}px"
            );
        }
    }

    fn luminance(color: Color32) -> f32 {
        let linear = |channel: u8| {
            let srgb = f32::from(channel) / 255.0;
            if srgb <= 0.04045 {
                srgb / 12.92
            } else {
                ((srgb + 0.055) / 1.055).powf(2.4)
            }
        };
        0.2126 * linear(color.r()) + 0.7152 * linear(color.g()) + 0.0722 * linear(color.b())
    }

    fn assert_rendered_text_contrast(ctx: &egui::Context, output: &egui::FullOutput, label: &str) {
        let (index, clipped) = output.shapes.iter().enumerate().find(|(_, clipped)| {
            matches!(&clipped.shape, Shape::Text(text) if text.galley.text() == label)
        }).unwrap_or_else(|| panic!("{label} did not produce a text shape"));
        let Shape::Text(text) = &clipped.shape else {
            unreachable!()
        };
        let background = output.shapes[..index]
            .iter()
            .rev()
            .find_map(|shape| {
                if let Shape::Rect(rect) = &shape.shape {
                    if rect.fill.a() > 0 && rect.rect.contains(text.pos + egui::vec2(2.0, 2.0)) {
                        return Some(rect.fill);
                    }
                }
                None
            })
            .expect("rendered text has no underlying opaque surface");
        // Isolating the text shape lets egui resolve explicit galley colors,
        // fallback colors, strong colors and overrides exactly as in the GPU
        // pipeline, without counting the surrounding panel's mesh colors.
        let meshes = ctx.tessellate(vec![clipped.clone()], output.pixels_per_point);
        let mut glyph_vertices = 0;
        for primitive in &meshes {
            if let egui::epaint::Primitive::Mesh(mesh) = &primitive.primitive {
                for vertex in &mesh.vertices {
                    if vertex.color.a() == 0 {
                        continue;
                    }
                    glyph_vertices += 1;
                    let foreground = luminance(vertex.color);
                    let surface = luminance(background);
                    let ratio = (foreground.max(surface) + 0.05) / (foreground.min(surface) + 0.05);
                    assert!(
                        ratio >= 4.5,
                        "{label}: rendered text contrast {ratio:.2} is unreadable on its actual surface"
                    );
                }
            }
        }
        assert!(glyph_vertices > 0, "{label} did not tessellate glyphs");
    }

    #[test]
    fn light_dark_frames_preserve_korean_font_and_tessellate_every_icon() {
        let ctx = egui::Context::default();
        // The production app supplies this same font; installing it before
        // apply verifies the styling does not replace its Korean coverage.
        let mut fonts = egui::FontDefinitions::default();
        fonts.font_data.insert(
            "native-theme-korean".into(),
            egui::FontData::from_static(include_bytes!("../assets/fonts/NotoSansKR-Regular.ttf"))
                .into(),
        );
        fonts
            .families
            .get_mut(&FontFamily::Proportional)
            .unwrap()
            .insert(0, "native-theme-korean".into());
        ctx.set_fonts(fonts);
        let all = [
            Icon::Workspace,
            Icon::Tasks,
            Icon::Chat,
            Icon::Graph,
            Icon::Files,
            Icon::Settings,
            Icon::Logs,
            Icon::Traffic,
            Icon::Findings,
            Icon::Agents,
            Icon::Tools,
            Icon::Archive,
            Icon::Refresh,
            Icon::Play,
            Icon::Stop,
            Icon::Search,
            Icon::Plus,
            Icon::Close,
            Icon::Info,
            Icon::ChevronRight,
        ];
        let input = || egui::RawInput {
            screen_rect: Some(Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(800.0, 600.0),
            )),
            ..Default::default()
        };
        let mut rendered_backgrounds = Vec::new();
        for theme in [Theme::Light, Theme::Dark, Theme::Light] {
            ctx.set_theme(theme);
            apply(&ctx);
            assert_eq!(ctx.theme(), theme);
            let output = ctx.run(input(), |ctx| {
                egui::CentralPanel::default().show(ctx, |ui| {
                    ui.strong("ARTEX");
                    toolbar_frame(ui.style()).show(ui, |ui| {
                        ui.horizontal(|ui| {
                            let _ = ui.button("작업 공간 열기");
                            ui.label("Ctrl+O");
                        });
                    });
                    group(ui, "작업 및 대화", |ui| {
                        ui.label(egui::RichText::new("128").size(24.0).strong());
                        ui.horizontal(|ui| {
                            tab(ui, "작업", true);
                            tab(ui, "대화", false);
                        });
                        ui.label("선택한 작업의 상세 정보");
                    });
                    status_frame(ui.style()).show(ui, |ui| {
                        ui.label("준비됨");
                    });
                    ctx.fonts_mut(|fonts| {
                        assert!(fonts.has_glyphs(
                            &FontId::new(14.0, FontFamily::Proportional),
                            "작업공간열기대화준비됨Ctrl+O",
                        ))
                    });
                });
            });
            assert!(output.shapes.iter().any(|shape| {
                matches!(&shape.shape, Shape::Text(text) if text.galley.text().contains("Ctrl+O"))
            }), "shortcut label did not produce a text shape");
            for label in ["ARTEX", "작업 및 대화", "128"] {
                assert_rendered_text_contrast(&ctx, &output, label);
            }
            let primitives = ctx.tessellate(output.shapes, output.pixels_per_point);
            assert!(primitives.iter().any(|primitive| {
                matches!(&primitive.primitive, egui::epaint::Primitive::Mesh(mesh) if !mesh.indices.is_empty())
            }));
            rendered_backgrounds.push(ctx.style().visuals.panel_fill);

            for kind in all {
                let mut icon_rect = Rect::NOTHING;
                let output = ctx.run(input(), |ctx| {
                    egui::CentralPanel::default().show(ctx, |ui| {
                        icon_rect = icon(ui, kind, 20.0).rect;
                    });
                });
                // Panel backgrounds do not fit this small cell; a surviving
                // geometric shape here therefore belongs to the tested icon.
                assert!(
                    output.shapes.iter().any(|shape| {
                        !matches!(shape.shape, Shape::Noop | Shape::Text(_))
                            && icon_rect
                                .expand(2.0)
                                .contains_rect(shape.shape.visual_bounding_rect())
                    }),
                    "{theme:?} {kind:?} produced no drawable icon geometry"
                );
                let primitives = ctx.tessellate(output.shapes, output.pixels_per_point);
                assert!(
                    primitives.iter().any(|primitive| {
                        if let egui::epaint::Primitive::Mesh(mesh) = &primitive.primitive {
                            mesh.vertices
                                .iter()
                                .all(|vertex| vertex.pos.x.is_finite() && vertex.pos.y.is_finite())
                                && !mesh.indices.is_empty()
                        } else {
                            false
                        }
                    }),
                    "{theme:?} {kind:?} did not tessellate"
                );
            }
        }
        assert_ne!(rendered_backgrounds[0], rendered_backgrounds[1]);
        assert_eq!(rendered_backgrounds[0], rendered_backgrounds[2]);
    }
}

#[derive(Clone, Copy, Debug)]
pub enum Icon {
    Workspace,
    Tasks,
    Chat,
    Graph,
    Files,
    Settings,
    Logs,
    Traffic,
    Findings,
    Agents,
    Tools,
    Archive,
    #[cfg(test)]
    Refresh,
    #[cfg(test)]
    Play,
    #[cfg(test)]
    Stop,
    #[cfg(test)]
    Search,
    #[cfg(test)]
    Plus,
    #[cfg(test)]
    Close,
    #[cfg(test)]
    Info,
    #[cfg(test)]
    ChevronRight,
}

/// Allocate a decorative icon; the surrounding native button owns interaction.
pub fn icon(ui: &mut Ui, kind: Icon, size: f32) -> Response {
    let (rect, response) = ui.allocate_exact_size(Vec2::splat(size.max(12.0)), Sense::hover());
    paint_icon(ui.painter(), rect, kind, ui.visuals().text_color());
    response
}

pub fn paint_icon(painter: &Painter, rect: Rect, kind: Icon, color: Color32) {
    let size = rect.width().min(rect.height()).max(0.0);
    if size == 0.0 {
        return;
    }
    let rect = Rect::from_center_size(rect.center(), Vec2::splat(size));
    let at = |x: f32, y: f32| rect.min + egui::vec2(x, y) * (size / 16.0);
    let stroke = Stroke::new((size / 16.0).clamp(1.0, 2.0), color);
    let line = |a: (f32, f32), b: (f32, f32)| {
        painter.line_segment([at(a.0, a.1), at(b.0, b.1)], stroke);
    };
    let path = |points: &[(f32, f32)]| {
        painter.add(Shape::line(
            points.iter().map(|p| at(p.0, p.1)).collect(),
            stroke,
        ));
    };
    let outline = |a: (f32, f32), b: (f32, f32)| {
        painter.rect_stroke(
            Rect::from_min_max(at(a.0, a.1), at(b.0, b.1)),
            1,
            stroke,
            StrokeKind::Inside,
        );
    };
    match kind {
        Icon::Workspace | Icon::Files => {
            path(&[
                (2.0, 13.0),
                (2.0, 3.0),
                (6.0, 3.0),
                (8.0, 5.0),
                (14.0, 5.0),
                (14.0, 13.0),
                (2.0, 13.0),
            ]);
            line((2.0, 6.0), (14.0, 6.0));
        }
        Icon::Tasks => {
            outline((2.0, 2.0), (14.0, 14.0));
            path(&[(4.0, 6.0), (5.5, 7.5), (8.0, 4.5)]);
            line((9.0, 6.0), (12.0, 6.0));
            line((4.0, 10.0), (12.0, 10.0));
        }
        Icon::Chat => {
            path(&[
                (2.0, 3.0),
                (14.0, 3.0),
                (14.0, 11.0),
                (7.0, 11.0),
                (4.0, 14.0),
                (4.0, 11.0),
                (2.0, 11.0),
                (2.0, 3.0),
            ]);
            line((5.0, 6.0), (11.0, 6.0));
            line((5.0, 8.0), (9.0, 8.0));
        }
        Icon::Graph => {
            for point in [(3.0, 8.0), (12.0, 3.0), (12.0, 13.0)] {
                painter.circle_stroke(at(point.0, point.1), size * 0.12, stroke);
            }
            line((5.0, 7.0), (10.0, 4.0));
            line((5.0, 9.0), (10.0, 12.0));
        }
        Icon::Settings => {
            for (y, x) in [(4.0, 6.0), (8.0, 11.0), (12.0, 5.0)] {
                line((2.0, y), (14.0, y));
                painter.circle_filled(at(x, y), size * 0.11, color);
            }
        }
        Icon::Logs => {
            outline((3.0, 2.0), (13.0, 14.0));
            for y in [5.0, 8.0, 11.0] {
                line((5.0, y), (11.0, y));
            }
        }
        Icon::Traffic => {
            path(&[(2.0, 5.0), (14.0, 5.0), (11.0, 2.0)]);
            path(&[(14.0, 11.0), (2.0, 11.0), (5.0, 14.0)]);
        }
        Icon::Findings => {
            path(&[
                (8.0, 1.0),
                (14.0, 4.0),
                (13.0, 10.0),
                (8.0, 15.0),
                (3.0, 10.0),
                (2.0, 4.0),
                (8.0, 1.0),
            ]);
            line((8.0, 5.0), (8.0, 9.0));
            painter.circle_filled(at(8.0, 11.0), size * 0.055, color);
        }
        Icon::Agents => {
            painter.circle_stroke(at(6.0, 5.0), size * 0.16, stroke);
            path(&[
                (2.0, 14.0),
                (2.0, 11.0),
                (4.0, 9.0),
                (8.0, 9.0),
                (10.0, 11.0),
                (10.0, 14.0),
            ]);
            path(&[(11.0, 3.0), (14.0, 4.0), (14.0, 7.0), (12.0, 8.0)]);
            path(&[(12.0, 10.0), (14.0, 11.0), (14.0, 14.0)]);
        }
        Icon::Tools => {
            outline((2.0, 6.0), (14.0, 14.0));
            path(&[(5.0, 6.0), (5.0, 3.0), (11.0, 3.0), (11.0, 6.0)]);
            line((2.0, 9.0), (14.0, 9.0));
            line((8.0, 8.0), (8.0, 11.0));
        }
        Icon::Archive => {
            outline((2.0, 2.0), (14.0, 5.0));
            outline((3.0, 5.0), (13.0, 14.0));
            line((6.0, 8.0), (10.0, 8.0));
        }
        #[cfg(test)]
        Icon::Refresh => {
            path(&[
                (13.0, 6.0),
                (11.0, 3.0),
                (7.0, 2.0),
                (3.0, 4.0),
                (2.0, 8.0),
                (4.0, 12.0),
                (8.0, 14.0),
                (12.0, 12.0),
            ]);
            path(&[(13.0, 2.0), (13.0, 6.0), (9.0, 6.0)]);
        }
        #[cfg(test)]
        Icon::Play => {
            painter.add(Shape::convex_polygon(
                vec![at(5.0, 2.0), at(14.0, 8.0), at(5.0, 14.0)],
                color,
                Stroke::NONE,
            ));
        }
        #[cfg(test)]
        Icon::Stop => {
            painter.rect_filled(Rect::from_min_max(at(3.0, 3.0), at(13.0, 13.0)), 1, color);
        }
        #[cfg(test)]
        Icon::Search => {
            painter.circle_stroke(at(6.5, 6.5), size * 0.28, stroke);
            line((10.0, 10.0), (14.0, 14.0));
        }
        #[cfg(test)]
        Icon::Plus => {
            line((2.0, 8.0), (14.0, 8.0));
            line((8.0, 2.0), (8.0, 14.0));
        }
        #[cfg(test)]
        Icon::Close => {
            line((3.0, 3.0), (13.0, 13.0));
            line((13.0, 3.0), (3.0, 13.0));
        }
        #[cfg(test)]
        Icon::Info => {
            painter.circle_stroke(at(8.0, 8.0), size * 0.4, stroke);
            painter.circle_filled(at(8.0, 4.5), size * 0.055, color);
            line((8.0, 7.0), (8.0, 12.0));
        }
        #[cfg(test)]
        Icon::ChevronRight => {
            path(&[(6.0, 3.0), (11.0, 8.0), (6.0, 13.0)]);
        }
    }
}
