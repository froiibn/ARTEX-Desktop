//! Widget-level ownership and export tests complement the real App/Service path.
use super::*;
use artex_core::Header;

fn fixture() -> (Panes, Snapshot, Record) {
    let task = Record {
        header: Header {
            id: artex_core::new_id("task"),
            collection: "tasks".into(),
            version: "1".into(),
            commit_seq: "4".into(),
            created_at: "2026-10-20T00:00:00Z".into(),
            updated_at: "2026-10-20T00:00:00Z".into(),
            legacy_key: Value::Null,
        },
        data: json!({"status":"done","name":"Local report fixture"}),
    };
    let snapshot = Snapshot {
        workspace_id: artex_core::new_id("workspace"),
        commit_seq: 4,
        collections: BTreeMap::new(),
    };
    let mut panes = Panes::default();
    panes.set_context(Section::Tasks, Some(&task.header.id));
    (panes, snapshot, task)
}
fn frame(
    p: &mut Panes,
    s: &Snapshot,
    t: &Record,
    ctx: &egui::Context,
    events: Vec<egui::Event>,
) -> (egui::FullOutput, Vec<Effect>) {
    let mut effects = vec![];
    let output = ctx.run(
        egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(1000., 1800.),
            )),
            events,
            ..Default::default()
        },
        |ctx| {
            egui::CentralPanel::default()
                .show(ctx, |ui| p.show_site_report(ui, s, t, &mut effects));
        },
    );
    (output, effects)
}
fn click(p: &mut Panes, s: &Snapshot, t: &Record, ctx: &egui::Context, label: &str) -> Vec<Effect> {
    let (out, initial) = frame(p, s, t, ctx, vec![]);
    assert!(initial.is_empty());
    let pos = out
        .shapes
        .iter()
        .find_map(|shape| match &shape.shape {
            egui::epaint::Shape::Text(text) if text.galley.text() == label => {
                Some(text.visual_bounding_rect().center())
            }
            _ => None,
        })
        .unwrap_or_else(|| panic!("Missing {label}"));
    let (_, mut effects) = frame(
        p,
        s,
        t,
        ctx,
        vec![
            egui::Event::PointerMoved(pos),
            egui::Event::PointerButton {
                pos,
                button: egui::PointerButton::Primary,
                pressed: true,
                modifiers: Default::default(),
            },
        ],
    );
    let (_, released) = frame(
        p,
        s,
        t,
        ctx,
        vec![egui::Event::PointerButton {
            pos,
            button: egui::PointerButton::Primary,
            pressed: false,
            modifiers: Default::default(),
        }],
    );
    effects.extend(released);
    effects
}
fn query_token(effects: &[Effect], task: &str) -> String {
    assert_eq!(effects.len(), 1);
    match &effects[0] {
        Effect::Query { method, params } => {
            assert_eq!(method, "ui.report");
            assert_eq!(params["task"], task);
            params["_pane_token"].as_str().unwrap().into()
        }
        _ => panic!("Report must only read stored data"),
    }
}

#[test]
fn report_widgets_export_exact_displayed_bytes_and_refresh_retire_old_task_replies() {
    let (mut p, mut s, mut t) = fixture();
    let ctx = egui::Context::default();
    crate::app::configure(&ctx);
    let (_, effects) = frame(&mut p, &s, &t, &ctx, vec![]);
    let token = query_token(&effects, &t.header.id);
    let report =
        "# 사이트 종합 결과 보고서\n\n## 검사 내역\n로컬 확인\n## 미검증 항목\n인증 범위 미검사\n";
    assert!(p.set_projection_for("ui.report", &token, artex_core::body(report), vec![]));
    assert!(
        frame(&mut p, &s, &t, &ctx, vec![]).1.is_empty(),
        "Repaint must not query repeatedly"
    );
    let exported = click(&mut p, &s, &t, &ctx, "보고서 내보내기 (.md)");
    assert!(
        matches!(&exported[..],[Effect::ExportBody{name,body}] if name.ends_with(".md") && body["text"]==report)
    );
    s.commit_seq += 1;
    assert!(
        frame(&mut p, &s, &t, &ctx, vec![]).1.is_empty(),
        "Unrelated writes must not interrupt a report read"
    );
    t.header.version = "2".into();
    t.data["status"] = json!("failed");
    let (_, effects) = frame(&mut p, &s, &t, &ctx, vec![]);
    let new = query_token(&effects, &t.header.id);
    assert!(!p.set_projection_for("ui.report", &token, artex_core::body("stale"), vec![]));
    assert!(!p.projection.contains_key("ui.report"));
    p.set_context(Section::Tasks, Some("other-task"));
    assert!(!p.set_projection_for("ui.report", &new, artex_core::body(report), vec![]));
    assert!(!p.projection.contains_key("ui.report"));
    assert!(!p.flow.append.contains_key(&new));
    assert!(!p.flow.versions.contains_key("ui.report"));
}

#[test]
fn report_error_requires_retry_and_paging_retains_whole_export_with_generation_ownership() {
    let (mut p, mut s, t) = fixture();
    let ctx = egui::Context::default();
    crate::app::configure(&ctx);
    let (_, effects) = frame(&mut p, &s, &t, &ctx, vec![]);
    let token = query_token(&effects, &t.header.id);
    p.set_projection_error("ui.report", &token, "IO_ERROR", "missing evidence");
    s.commit_seq += 1;
    let (out, effects) = frame(&mut p, &s, &t, &ctx, vec![]);
    assert!(
        effects.is_empty(),
        "A logged read failure must not cause an automatic retry loop"
    );
    assert!(out.shapes.iter().any(|s|matches!(&s.shape,egui::epaint::Shape::Text(t) if t.galley.text().contains("missing evidence"))));
    assert!(click(&mut p, &s, &t, &ctx, "보고서 내보내기 (.md)").is_empty());
    let effects = click(&mut p, &s, &t, &ctx, "보고서 다시 불러오기");
    let token = query_token(&effects, &t.header.id);
    let body = json!({"text":null,"blob":{"sha256":"a".repeat(64),"length":"90000","media_type":"text/plain"}});
    assert!(p.set_projection_for("ui.report", &token, body.clone(), vec![]));
    let (_, effects) = frame(&mut p, &s, &t, &ctx, vec![]);
    let hydration = match &effects[..] {
        [Effect::Query { method, params }] => {
            assert_eq!(method, "native.report.page");
            assert_eq!(params["offset"], 0);
            params["_pane_token"].as_str().unwrap().to_owned()
        }
        _ => panic!(),
    };
    assert!(
        frame(&mut p, &s, &t, &ctx, vec![]).1.is_empty(),
        "Body hydration must not be duplicated"
    );
    assert!(p.set_projection_for(
        "native.report.page",
        &hydration,
        json!({"text":"page one","next_offset":"8","total_length":"90000"}),
        vec![]
    ));
    let export = click(&mut p, &s, &t, &ctx, "보고서 내보내기 (.md)");
    assert!(
        matches!(&export[..],[Effect::ExportBody{body:actual,..}] if actual==&body),
        "Export must retain the full source, not only the displayed page"
    );
    let next = click(&mut p, &s, &t, &ctx, "다음 페이지");
    let next_token = match &next[..] {
        [Effect::Query { method, params }] => {
            assert_eq!(method, "native.report.page");
            assert_eq!(params["offset"], 8);
            params["_pane_token"].as_str().unwrap().to_owned()
        }
        _ => panic!(),
    };
    assert!(!p.set_projection_for(
        "native.report.page",
        &hydration,
        json!({"text":"late first page"}),
        vec![]
    ));
    p.retire_async_operations();
    assert!(!p.set_projection_for(
        "native.report.page",
        &next_token,
        json!({"text":"wrong epoch"}),
        vec![]
    ));
    assert!(!p.projection.contains_key("ui.report"));
}

#[test]
fn evidence_fences_keep_untrusted_headings_as_selectable_code() {
    let ctx = egui::Context::default();
    crate::app::configure(&ctx);
    let output = ctx.run(egui::RawInput::default(), |ctx| {
        egui::CentralPanel::default().show(ctx, |ui| {
            show_markdown(
                ui,
                "## 검사 내역\n````text\n## 거짓 결론\n```\n````\n## 미검증 항목\n남은 범위",
            );
        });
    });
    let headings = output
        .shapes
        .iter()
        .filter_map(|s| match &s.shape {
            egui::epaint::Shape::Text(t) => Some(t.galley.text().to_owned()),
            _ => None,
        })
        .collect::<Vec<_>>();
    assert!(headings.contains(&"검사 내역".into()));
    assert!(headings.contains(&"## 거짓 결론".into()));
    assert!(!headings.contains(&"거짓 결론".into()));
    assert!(headings.contains(&"미검증 항목".into()));
}

#[test]
fn task_completion_and_last_child_settlement_refresh_the_report_automatically() {
    let (mut p, mut s, mut t) = fixture();
    let ctx = egui::Context::default();
    crate::app::configure(&ctx);
    t.data["status"] = json!("running");
    let mut run = t.clone();
    run.header.id = artex_core::new_id("run");
    run.header.collection = "agent_runs".into();
    run.data = json!({"task_id":t.header.id,"state":"running"});
    s.collections
        .entry("agent_runs".into())
        .or_default()
        .insert(run.header.id.clone(), run.clone());
    let (_, effects) = frame(&mut p, &s, &t, &ctx, vec![]);
    let token = query_token(&effects, &t.header.id);
    p.set_projection_for("ui.report", &token, artex_core::body("running"), vec![]);
    t.data["status"] = json!("done");
    t.header.version = "2".into();
    s.commit_seq += 1;
    let (_, effects) = frame(&mut p, &s, &t, &ctx, vec![]);
    let token = query_token(&effects, &t.header.id);
    p.set_projection_for("ui.report", &token, artex_core::body("wrapping up"), vec![]);
    s.collections
        .get_mut("agent_runs")
        .unwrap()
        .get_mut(&run.header.id)
        .unwrap()
        .data["state"] = json!("completed");
    s.commit_seq += 1;
    let (_, effects) = frame(&mut p, &s, &t, &ctx, vec![]);
    let token = query_token(&effects, &t.header.id);
    p.set_projection_for(
        "ui.report",
        &token,
        artex_core::body("settled final report"),
        vec![],
    );
    assert!(frame(&mut p, &s, &t, &ctx, vec![]).1.is_empty());
    p.engine_epoch = artex_core::new_id("epoch");
    let (_, effects) = frame(&mut p, &s, &t, &ctx, vec![]);
    query_token(&effects, &t.header.id);
    assert!(!p.set_projection_for(
        "ui.report",
        &token,
        artex_core::body("late old epoch"),
        vec![]
    ));
}

#[test]
fn malformed_report_pages_are_explicit_errors_instead_of_empty_or_final_reports() {
    let (mut p, s, t) = fixture();
    let ctx = egui::Context::default();
    crate::app::configure(&ctx);
    for bad in [
        json!({"text":"invented"}),
        json!({"text":"a","next_offset":null,"total_length":"90000"}),
        json!({"text":"a","next_offset":"2","total_length":"90000"}),
        json!({"text":"a","next_offset":"1","total_length":"8"}),
    ] {
        p.retire_site_report();
        let (_, effects) = frame(&mut p, &s, &t, &ctx, vec![]);
        let token = query_token(&effects, &t.header.id);
        p.set_projection_for("ui.report",&token,json!({"text":null,"blob":{"sha256":"a".repeat(64),"length":"90000","media_type":"text/plain"}}),vec![]);
        let (_, effects) = frame(&mut p, &s, &t, &ctx, vec![]);
        let token = match &effects[..] {
            [Effect::Query { params, .. }] => params["_pane_token"].as_str().unwrap().to_owned(),
            _ => panic!(),
        };
        assert!(p.set_projection_for("native.report.page", &token, bad, vec![]));
        assert!(p.site_report.page.is_none());
        assert!(
            p.site_report
                .page_error
                .as_ref()
                .unwrap()
                .contains("EXTERNAL_FAILURE")
        );
        assert!(frame(&mut p, &s, &t, &ctx, vec![]).1.is_empty());
    }
}
