//! Actual report tab -> Service query -> display -> same workspace reopen.
//! Native picker interaction is intentionally outside this headless test.
use super::*;

#[test]
fn site_report_tab_reads_only_selected_task_and_remains_available_after_readonly_reopen() {
    let mut f = Fixture::new();
    let first = source_task(&mut f, "LOCAL_SITE_FIRST_REPORT");
    let second = source_task(&mut f, "LOCAL_SITE_SECOND_REPORT");
    select(&mut f, Section::Tasks, Some(&first));
    f.app.drawer.open = false;
    f.click_visible("종합 보고서");
    f.until(|app| {
        app.pending.is_empty()
            && app
                .panes
                .projection
                .get("ui.report")
                .is_some_and(|r| r["text"].is_string())
    });
    let first_report = f.app.panes.projection["ui.report"]["text"]
        .as_str()
        .unwrap()
        .to_owned();
    assert!(first_report.contains("LOCAL_SITE_FIRST_REPORT"));
    assert!(!first_report.contains("LOCAL_SITE_SECOND_REPORT"));
    for section in [
        "검사 내역",
        "발견 문제",
        "개선 우선순위",
        "미검증·실패·차단 항목",
    ] {
        assert!(first_report.contains(section), "Missing {section}");
    }
    assert!(
        first_report.contains("부분 보고서"),
        "Stopped work must not be certified complete"
    );
    let before = f.app.handle.view().unwrap().generation.snapshot.clone();
    let output = f.frame(vec![]);
    assert!(
        Fixture::texts(&output)
            .iter()
            .any(|t| t == "사이트 종합 결과 보고서")
    );
    assert!(
        Fixture::texts(&output)
            .iter()
            .any(|t| t == "보고서 내보내기 (.md)")
    );
    for _ in 0..3 {
        f.frame(vec![]);
    }
    assert_eq!(
        before.collections,
        f.app.handle.view().unwrap().generation.snapshot.collections,
        "Reading/rendering reports must not launch work or change records"
    );

    select(&mut f, Section::Tasks, Some(&second));
    f.until(|app| {
        app.pending.is_empty()
            && app.panes.projection.get("ui.report").is_some_and(|r| {
                r["text"]
                    .as_str()
                    .is_some_and(|t| t.contains("LOCAL_SITE_SECOND_REPORT"))
            })
    });
    let second_report = f.app.panes.projection["ui.report"]["text"]
        .as_str()
        .unwrap()
        .to_owned();
    assert!(!second_report.contains("LOCAL_SITE_FIRST_REPORT"));
    f.reopen_same_workspace();
    select(&mut f, Section::Tasks, Some(&second));
    f.click_visible("종합 보고서");
    f.until(|app| {
        app.pending.is_empty()
            && app
                .panes
                .projection
                .get("ui.report")
                .is_some_and(|r| r["text"].is_string())
    });
    assert_eq!(
        f.app.panes.projection["ui.report"]["text"], second_report,
        "Saved records must reconstruct the same report in readonly mode"
    );
    f.shutdown();
}
