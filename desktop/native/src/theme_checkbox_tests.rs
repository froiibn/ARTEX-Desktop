//! Render the real egui Checkbox, including idle shapes and disabled input.
use super::*;

struct Fixture {
    ctx: egui::Context,
    checked: bool,
    enabled: bool,
    time: f64,
}
struct Rendered {
    output: egui::FullOutput,
    response: Response,
    panel: Color32,
    icon_width: f32,
}
impl Fixture {
    fn new(theme: Theme, checked: bool, enabled: bool) -> Self {
        let ctx = egui::Context::default();
        ctx.set_theme(theme);
        apply(&ctx);
        ctx.style_mut(|style| style.animation_time = 0.0);
        Self {
            ctx,
            checked,
            enabled,
            time: 0.0,
        }
    }
    fn frame(&mut self, events: Vec<egui::Event>) -> Rendered {
        self.time += 1.0 / 60.0;
        let mut response = None;
        let mut icon_width = 0.0;
        let panel = palette(self.ctx.theme() == Theme::Dark).panel;
        let output = self.ctx.run(
            egui::RawInput {
                time: Some(self.time),
                screen_rect: Some(Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(320.0, 160.0),
                )),
                events,
                ..Default::default()
            },
            |ctx| {
                egui::CentralPanel::default()
                    .frame(Frame::new().fill(panel))
                    .show(ctx, |ui| {
                        icon_width = ui.spacing().icon_width;
                        let before = ui.visuals().widgets.inactive;
                        response = Some(
                            ui.add_enabled_ui(self.enabled, |ui| {
                                list_checkbox(ui, &mut self.checked)
                            })
                            .inner,
                        );
                        // The checkbox's border must not leak back to ordinary idle
                        // buttons or revive the discarded global box-per-widget style.
                        assert_eq!(ui.visuals().widgets.inactive, before);
                        assert_eq!(ui.visuals().widgets.inactive.bg_stroke, Stroke::NONE);
                        let _ = ui.button("Unchanged ordinary control");
                    });
            },
        );
        Rendered {
            output,
            response: response.unwrap(),
            panel,
            icon_width,
        }
    }
    fn click(&mut self, pos: egui::Pos2) -> Rendered {
        self.frame(vec![
            egui::Event::PointerMoved(pos),
            egui::Event::PointerButton {
                pos,
                button: egui::PointerButton::Primary,
                pressed: true,
                modifiers: Default::default(),
            },
        ]);
        self.frame(vec![egui::Event::PointerButton {
            pos,
            button: egui::PointerButton::Primary,
            pressed: false,
            modifiers: Default::default(),
        }])
    }
}
fn checkbox_rect(rendered: &Rendered) -> egui::epaint::RectShape {
    // Checkbox first paints AtomLayout's transparent response-sized Frame,
    // then its icon_width square. Select by that geometry, not by the expected
    // fill/stroke: an invisible or wrongly styled icon must still fail below.
    let icons: Vec<_> = rendered
        .output
        .shapes
        .iter()
        .filter_map(|clipped| match &clipped.shape {
            Shape::Rect(rect)
                if rendered.response.rect.contains_rect(rect.rect)
                    && (rect.rect.width() - rendered.icon_width).abs() < 0.01
                    && (rect.rect.height() - rendered.icon_width).abs() < 0.01
                    && clipped.clip_rect.contains_rect(rect.rect) =>
            {
                Some(rect.clone())
            }
            _ => None,
        })
        .collect();
    assert_eq!(
        icons.len(),
        1,
        "The actual checkbox must paint exactly one icon inside the visible response: {:?}",
        rendered.output.shapes
    );
    icons.into_iter().next().unwrap()
}
fn checkmark(rendered: &Rendered) -> bool {
    rendered
        .output
        .shapes
        .iter()
        .any(|clipped| match &clipped.shape {
            Shape::Path(path) => {
                !path.closed
                    && path.points.len() == 3
                    && path.stroke.width > 0.0
                    && path
                        .points
                        .iter()
                        .all(|pos| rendered.response.rect.contains(*pos))
            }
            _ => false,
        })
}
fn composite(color: Color32, background: Color32) -> [f32; 3] {
    // egui Color32 channels are premultiplied; compare what is actually painted
    // over the list surface, including disabled-opacity transformation.
    let a = color.a() as f32 / 255.0;
    [0, 1, 2].map(|i| color[i] as f32 / 255.0 + background[i] as f32 / 255.0 * (1.0 - a))
}
fn luminance(rgb: [f32; 3]) -> f32 {
    let linear = rgb.map(|v| {
        if v <= 0.04045 {
            v / 12.92
        } else {
            ((v + 0.055) / 1.055).powf(2.4)
        }
    });
    linear[0] * 0.2126 + linear[1] * 0.7152 + linear[2] * 0.0722
}
fn contrast(a: [f32; 3], b: [f32; 3]) -> f32 {
    let a = luminance(a);
    let b = luminance(b);
    (a.max(b) + 0.05) / (a.min(b) + 0.05)
}

#[test]
fn list_checkbox_idle_hover_checked_and_disabled_paint_in_both_themes() {
    for theme in [Theme::Light, Theme::Dark] {
        for checked in [false, true] {
            for enabled in [false, true] {
                let mut f = Fixture::new(theme, checked, enabled);
                let idle = f.frame(vec![egui::Event::PointerGone]);
                assert!(!idle.response.hovered());
                let idle_rect = checkbox_rect(&idle);
                assert!(idle_rect.stroke.width >= 1.0 && idle_rect.stroke.color.a() > 0);
                let outline = composite(idle_rect.stroke.color, idle.panel);
                let background = composite(Color32::TRANSPARENT, idle.panel);
                assert!(
                    contrast(outline, background) > 1.5,
                    "{theme:?} {checked} {enabled}: idle outline disappeared"
                );
                assert_eq!(checkmark(&idle), checked);
                if enabled {
                    assert_eq!(
                        idle_rect.fill,
                        if checked {
                            palette(theme == Theme::Dark).selection
                        } else {
                            palette(theme == Theme::Dark).surface
                        }
                    );
                }
                let hover = f.frame(vec![egui::Event::PointerMoved(idle.response.rect.center())]);
                let hover_rect = checkbox_rect(&hover);
                assert_eq!(hover.response.hovered(), enabled);
                assert_eq!(checkmark(&hover), checked);
                if enabled {
                    assert_eq!(
                        hover_rect.stroke.color,
                        palette(theme == Theme::Dark).accent
                    );
                    assert_ne!(hover_rect.stroke.color, idle_rect.stroke.color);
                } else {
                    assert_eq!(hover_rect.stroke, idle_rect.stroke);
                }
            }
        }
    }
}

#[test]
fn list_checkbox_real_click_toggles_once_and_disabled_click_does_not_select() {
    for theme in [Theme::Light, Theme::Dark] {
        let mut f = Fixture::new(theme, false, true);
        let idle = f.frame(vec![egui::Event::PointerGone]);
        let clicked = f.click(idle.response.rect.center());
        assert!(clicked.response.changed() && f.checked);
        let selected = f.frame(vec![egui::Event::PointerGone]);
        assert!(checkmark(&selected));
        assert_eq!(
            checkbox_rect(&selected).fill,
            palette(theme == Theme::Dark).selection
        );
        let clicked = f.click(selected.response.rect.center());
        assert!(clicked.response.changed() && !f.checked);
        assert!(!checkmark(&f.frame(vec![egui::Event::PointerGone])));
        for checked in [false, true] {
            f.enabled = false;
            f.checked = checked;
            let idle = f.frame(vec![egui::Event::PointerGone]);
            let clicked = f.click(idle.response.rect.center());
            assert!(!clicked.response.changed());
            assert_eq!(f.checked, checked);
        }
    }
}
