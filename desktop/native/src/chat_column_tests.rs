//! Geometry and input checks use the actual pane, not a replacement chat widget.
use super::*;

#[test]
fn chat_reading_column_centers_messages_composer_and_creation_without_narrow_overflow() {
    for width in [460.0, 1000.0, 1800.0] {
        let ctx = egui::Context::default();
        crate::app::configure(&ctx);
        let cid = artex_core::new_id("conversation");
        let conversation: Record = serde_json::from_value(json!({
            "header":{"id":cid,"collection":"conversations","version":"1","commit_seq":"1",
                "created_at":"2026-10-20T00:00:00Z","updated_at":"2026-10-20T00:00:00Z","legacy_key":null},
            "data":{"id":cid,"title":"Column fixture","agent_key":"auto"}
        })).unwrap();
        let snapshot = Snapshot {
            workspace_id: artex_core::new_id("workspace"),
            commit_seq: 1,
            collections: BTreeMap::new(),
        };
        let mut panes = Panes::default();
        panes.flow.history.insert(cid.clone(),json!({"items":[{
            "seq":"fixture-history","kind":"assistant","summary":"CENTERED_TRANSCRIPT_MARKER",
            "detail":{"text":"CENTERED_TRANSCRIPT_MARKER","blob":null},"ts":"2026-10-20T00:00:00Z"
        }]}));
        for creation in [true, false] {
            let mut output = None;
            let mut area = egui::Rect::NOTHING;
            for _ in 0..6 {
                let mut effects = vec![];
                output = Some(ctx.run(
                    egui::RawInput {
                        screen_rect: Some(egui::Rect::from_min_size(
                            egui::Pos2::ZERO,
                            egui::vec2(width, 760.0),
                        )),
                        events: vec![egui::Event::PointerGone],
                        ..Default::default()
                    },
                    |ctx| {
                        egui::CentralPanel::default().show(ctx, |ui| {
                            area = ui.available_rect_before_wrap();
                            panes.chat(
                                ui,
                                &snapshot,
                                if creation { None } else { Some(&conversation) },
                                None,
                                &mut effects,
                            );
                        });
                    },
                ));
                assert!(
                    effects.is_empty(),
                    "A layout repaint must not send a command"
                );
            }
            let output = output.unwrap();
            let left = area.center().x - area.width().min(860.0) / 2.0;
            let marker = if creation {
                "새 대화"
            } else {
                "Column fixture"
            };
            let text = output
                .shapes
                .iter()
                .find_map(|s| match &s.shape {
                    egui::Shape::Text(t) if t.galley.text() == marker => Some(t),
                    _ => None,
                })
                .expect("Actual heading must render");
            if creation {
                assert!(
                    (text.pos.x - left).abs() < 2.0,
                    "Creation form not in reading column at {width}"
                );
            } else {
                // The selected title is centered inside its original sized
                // label. Its glyph position is not the column's left edge.
                assert!(text.pos.x >= left && text.pos.x < left + 260.0);
            }
            if !creation {
                let composer = output
                    .shapes
                    .iter()
                    .find_map(|s| match &s.shape {
                        egui::Shape::Rect(r)
                            if r.fill
                                == crate::theme::palette(ctx.style().visuals.dark_mode).surface
                                && r.rect.width() > 200.0 =>
                        {
                            Some(r.rect)
                        }
                        _ => None,
                    })
                    .expect("Actual composer surface must render");
                assert!(
                    (composer.center().x - area.center().x).abs() < 2.0,
                    "Composer center differs at width {width}"
                );
                assert!(
                    composer.width() <= 860.5 && area.contains_rect(composer),
                    "Composer overflows at {width}: {composer:?} vs {area:?}"
                );
                let text = output
                    .shapes
                    .iter()
                    .find_map(|s| match &s.shape {
                        egui::Shape::Text(t) if t.galley.text() == "CENTERED_TRANSCRIPT_MARKER" => {
                            Some((t, s.clip_rect))
                        }
                        _ => None,
                    })
                    .expect("Actual history must remain visible");
                assert!(text.1.contains_rect(text.0.visual_bounding_rect()));
                assert!(
                    (text.0.pos.x - left - 10.0).abs() < 3.0,
                    "Transcript must share reading column"
                );
            }
        }
    }
}
