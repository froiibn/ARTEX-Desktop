//! Completed public tool workflow -> native report contract -> readonly reopen.
use super::*;

fn report(f: &mut Fixture) -> String {
    let scope = f.router.native_scope().unwrap();
    let reply = f
        .router
        .native_query(
            &crate::native::NativeTicket::new(),
            &scope,
            crate::native::NativeQuery::Feature {
                method: "ui.report".into(),
                params: json!({"task":f.task}),
            },
        )
        .unwrap();
    let crate::native::NativeDispatch::Complete(reply) = reply else {
        panic!("Report query must complete")
    };
    artex_core::validate_contract(
        &f.router.contract["results"]["ui.report"],
        &f.router.contract["$defs"],
        &reply.value,
    )
    .unwrap();
    String::from_utf8(
        artex_domain::traffic_ui::body_bytes(&reply.value, &|r| {
            f.router.read_blob_bytes(r, 64 * 1024 * 1024)
        })
        .unwrap(),
    )
    .unwrap()
}

#[test]
fn completed_assessment_report_preserves_real_tool_results_remediation_and_reopen() {
    let mut f = fixture();
    let remediation = "## Repair suggestions\nRestrict access to the recorded object ownership boundary. REPORT_REMEDIATION_MARKER";
    for (name, input) in [
        (
            "update_finding_report",
            json!({"finding_node_id":f.finding_node,"report":remediation,"evidence_version":"0"}),
        ),
        (
            "goal_met",
            json!({"reason":"Local fixture assessment ended"}),
        ),
    ] {
        let req = request(&f, name, input);
        let result = f.router.response_as(req, Some(&f.context));
        assert!(result["error"].is_null(), "{name}: {result}");
        assert_eq!(result["result"]["state"], "completed");
    }
    assert!(
        report(&mut f).contains("부분 보고서"),
        "A done task with active runs is not a settled report"
    );
    let runs = f
        .router
        .snapshot()
        .unwrap()
        .records("agent_runs")
        .into_iter()
        .filter(|r| {
            r.data["task_id"] == f.task
                && matches!(
                    r.data["state"].as_str(),
                    Some("queued" | "running" | "waiting")
                )
        })
        .map(|r| r.header.id.clone())
        .collect::<Vec<_>>();
    for run in runs {
        f.router
            .finish_runtime(&run, "completed", "Recorded fixture assessment ended")
            .unwrap();
    }
    let before = f.router.snapshot().unwrap();
    let first = report(&mut f);
    for expected in [
        "종료 보고서",
        "## 검사 내역",
        "## 발견 문제",
        "## 개선 우선순위",
        "## 미검증·실패·차단 항목",
        "REPORT_REMEDIATION_MARKER",
        "goal\\_met",
    ] {
        assert!(first.contains(expected), "Missing {expected}: {first}");
    }
    assert_eq!(
        before.collections,
        f.router.snapshot().unwrap().collections,
        "Report query is read-only"
    );
    f.router.store.as_mut().unwrap().checkpoint().unwrap();
    f.router.store.take();
    f.router.store = Some(Store::open_read_only(&f.dir.path().join("store")).unwrap());
    f.router.read_only = true;
    assert_eq!(
        report(&mut f),
        first,
        "Same durable assessment must reconstruct the same report"
    );
}
