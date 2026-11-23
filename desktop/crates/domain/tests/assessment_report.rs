use artex_core::{Actor, Context, Error, Header, Record, Snapshot, body, new_id};
use artex_domain::{assessment_report, report_export};
use artex_store::Store;
use serde_json::{Value, json};
use std::collections::BTreeMap;

fn put(snapshot: &mut Snapshot, collection: &str, id: &str, data: Value) {
    snapshot
        .collections
        .entry(collection.into())
        .or_default()
        .insert(
            id.into(),
            Record {
                header: Header {
                    id: id.into(),
                    collection: collection.into(),
                    version: "1".into(),
                    commit_seq: "17".into(),
                    created_at: "2026-10-20T03:00:00.000Z".into(),
                    updated_at: "2026-10-20T04:00:00.000Z".into(),
                    legacy_key: Value::Null,
                },
                data,
            },
        );
}

fn fixture() -> Snapshot {
    let mut snapshot = Snapshot {
        workspace_id: "workspace-fixture".into(),
        commit_seq: 17,
        collections: BTreeMap::new(),
    };
    put(
        &mut snapshot,
        "tasks",
        "task-owned",
        json!({"description":"Owned example.test assessment","goal":"Check the authorized site only","exploration_id":"exploration-owned","status":"done","first_run_at":"2026-10-20T02:00:00.000Z","completed_at":"2026-10-20T04:00:00.000Z","deleted_at":null,"archived_at":null}),
    );
    snapshot
}

fn render(snapshot: &Snapshot) -> String {
    assessment_report::render(
        snapshot.get("tasks", "task-owned").unwrap(),
        snapshot,
        &|_| Err(Error::new("NOT_FOUND", "Unexpected Blob read")),
    )
    .unwrap()
}

#[test]
fn report_joins_owned_checks_findings_priorities_and_gaps_without_shared_trace_leaks() {
    let mut snapshot = fixture();
    put(
        &mut snapshot,
        "tasks",
        "task-other",
        json!({"exploration_id":"exploration-other"}),
    );
    put(
        &mut snapshot,
        "task_relations",
        "shared",
        json!({"task_id":"task-owned","source_task_id":"task-other"}),
    );
    put(
        &mut snapshot,
        "assets",
        "asset-owned",
        json!({"type":"url","url":"https://example.test/","task_ids":[]}),
    );
    put(
        &mut snapshot,
        "task_asset_links",
        "owned-link",
        json!({"task_id":"task-owned","asset_id":"asset-owned"}),
    );
    put(
        &mut snapshot,
        "assets",
        "asset-other",
        json!({"type":"url","url":"https://UNRELATED-PRIVATE.invalid/","task_ids":["task-other"]}),
    );
    put(
        &mut snapshot,
        "exploration_nodes",
        "plan-owned",
        json!({"exploration_id":"exploration-owned","kind":"intent","state":"blocked","blocked_reason":"Missing authorized login account","payload":{"summary":"Check authenticated access control"}}),
    );
    put(
        &mut snapshot,
        "exploration_nodes",
        "plan-other",
        json!({"exploration_id":"exploration-other","kind":"intent","payload":{"summary":"UNRELATED-PLAN"}}),
    );
    put(
        &mut snapshot,
        "exploration_nodes",
        "hint-owned",
        json!({"exploration_id":"exploration-owned","kind":"hint","state":"active","payload":{"text":"Compare the editor role after credentials are supplied"}}),
    );
    put(
        &mut snapshot,
        "exploration_nodes",
        "finding-node",
        json!({"exploration_id":"exploration-owned","kind":"finding","state":"confirmed","payload":{"summary":"OBSOLETE-NODE-SUMMARY"}}),
    );
    put(
        &mut snapshot,
        "findings",
        "finding-owned",
        json!({"task_id":"task-owned","node_id":"finding-node","name":"Recorded security issue","severity":"high","status":"pending","summary":"The saved response disclosed an internal path","evidence":body("Observed response sample"),"report":body("Saved remediation: suppress debug paths in production responses"),"evidence_version":"3","report_evidence_version":"2","asset_ids":["asset-owned"]}),
    );
    put(
        &mut snapshot,
        "findings",
        "finding-other",
        json!({"task_id":"task-other","name":"UNRELATED-FINDING","evidence":body("UNRELATED-EVIDENCE")}),
    );
    put(
        &mut snapshot,
        "agent_runs",
        "run-owned",
        json!({"task_id":"task-owned","intent_id":"plan-owned","state":"failed","context_snapshot":{"prompt":"DO-NOT-EXPORT-PROMPT"},"todos":[{"content":"Check remaining role","status":"pending"}]}),
    );
    put(
        &mut snapshot,
        "agent_runs",
        "run-other",
        json!({"task_id":"task-other","state":"running"}),
    );
    put(
        &mut snapshot,
        "tool_invocations",
        "call-owned",
        json!({"run_id":"run-owned","name":"WebFetch","state":"completed","input":{"Authorization":"DO-NOT-EXPORT-INPUT"},"result_ref":{"sha256":"result-owned"}}),
    );
    put(
        &mut snapshot,
        "tool_invocations",
        "call-other",
        json!({"run_id":"run-other","name":"UNRELATED-TOOL","state":"running"}),
    );
    put(
        &mut snapshot,
        "activity",
        "error-owned",
        json!({"exploration_id":"exploration-owned","kind":"tool_result","tool":"WebFetch","is_error":true,"summary":"Request failed: connection refused","detail":body("DO-NOT-EXPORT-RAW-ACTIVITY")}),
    );
    put(
        &mut snapshot,
        "activity",
        "error-other",
        json!({"exploration_id":"exploration-other","kind":"result","summary":"UNRELATED-ACTIVITY"}),
    );
    let before = snapshot.clone();
    let reads = std::cell::Cell::new(0);
    let report = assessment_report::render(snapshot.get("tasks","task-owned").unwrap(),&snapshot,&|reference| {
        assert_eq!(reference["sha256"],"result-owned"); reads.set(reads.get()+1);
        Ok(serde_json::to_vec(&json!({"is_error":true,"content":[{"type":"text","text":"DO-NOT-EXPORT-ARBITRARY-RESULT"}]})).unwrap())
    }).unwrap();
    assert_eq!(reads.get(), 1);
    for expected in [
        "## 검사 내역",
        "## 발견 문제",
        "## 개선 우선순위",
        "## 미검증·실패·차단 항목",
        "Check authenticated access control",
        "Missing authorized login account",
        "Recorded security issue",
        "Observed response sample",
        "Saved remediation: suppress debug paths",
        "상세 보고서 작성 뒤 증거가 변경",
        "| P1 |",
        "저장 결과 오류 있음",
        "Request failed: connection refused",
        "Check remaining role",
        "Compare the editor role",
        "작업 연결 자산: 1개",
        "발견 사항: 1건",
    ] {
        assert!(report.contains(expected), "Missing {expected}");
    }
    for forbidden in [
        "UNRELATED-PRIVATE",
        "UNRELATED-PLAN",
        "UNRELATED-FINDING",
        "UNRELATED-EVIDENCE",
        "UNRELATED-TOOL",
        "UNRELATED-ACTIVITY",
        "OBSOLETE-NODE-SUMMARY",
        "DO-NOT-EXPORT-PROMPT",
        "DO-NOT-EXPORT-INPUT",
        "DO-NOT-EXPORT-ARBITRARY-RESULT",
        "DO-NOT-EXPORT-RAW-ACTIVITY",
    ] {
        assert!(!report.contains(forbidden), "Leaked {forbidden}");
    }
    assert_eq!(snapshot, before);
}

#[test]
fn terminal_failure_pause_running_and_unfinished_children_remain_partial_without_false_safe_claim()
{
    for state in ["failed", "timeout", "paused", "running", "created"] {
        let mut snapshot = fixture();
        snapshot
            .collections
            .get_mut("tasks")
            .unwrap()
            .get_mut("task-owned")
            .unwrap()
            .data["status"] = json!(state);
        let report = render(&snapshot);
        assert!(report.contains("부분 보고서"), "{state}");
        assert!(report.contains("저장된 발견 사항이 없습니다. 취약점이 없거나 사이트가 안전하다는 결론을 내릴 수 없습니다."));
        assert!(report.contains("전체 종료 보고서로 확정할 수 없음"));
    }
    for state in [
        "running",
        "waiting",
        "queued",
        "interrupted",
        "recovery_required",
    ] {
        let mut snapshot = fixture();
        put(
            &mut snapshot,
            "agent_runs",
            "unfinished",
            json!({"task_id":"task-owned","state":state}),
        );
        assert!(
            render(&snapshot).contains("부분 보고서"),
            "done task with {state}"
        );
    }
    let mut snapshot = fixture();
    put(
        &mut snapshot,
        "agent_runs",
        "finished",
        json!({"task_id":"task-owned","state":"completed"}),
    );
    put(
        &mut snapshot,
        "tool_invocations",
        "unfinished",
        json!({"run_id":"finished","state":"awaiting_approval","result_ref":null}),
    );
    assert!(render(&snapshot).contains("부분 보고서"));
    assert!(render(&fixture()).contains("**종료 보고서**"));
    for flag in ["paused", "queued"] {
        let mut snapshot = fixture();
        snapshot
            .collections
            .get_mut("tasks")
            .unwrap()
            .get_mut("task-owned")
            .unwrap()
            .data[flag] = json!(true);
        assert!(
            render(&snapshot).contains("부분 보고서"),
            "done task with {flag}"
        );
    }
    let mut snapshot = fixture();
    put(
        &mut snapshot,
        "task_runtime",
        "recovery",
        json!({"task_id":"task-owned","accounting_state":"recovery_required"}),
    );
    assert!(render(&snapshot).contains("부분 보고서"));
    assert!(render(&snapshot).contains("작업 실행 시간 기록 복구 필요"));
}

#[test]
fn treatment_state_overrides_old_confirmed_node_and_actionable_priorities_come_first() {
    let mut snapshot = fixture();
    for (id, severity, status) in [
        ("critical-fixed", "critical", "fixed"),
        ("critical-false", "critical", "false_positive"),
        ("active-high", "high", "pending"),
    ] {
        put(
            &mut snapshot,
            "exploration_nodes",
            &format!("node-{id}"),
            json!({"exploration_id":"exploration-owned","kind":"finding","state":"confirmed","payload":{}}),
        );
        put(
            &mut snapshot,
            "findings",
            id,
            json!({"task_id":"task-owned","node_id":format!("node-{id}"),"name":id,"severity":severity,"status":status,"evidence":body("Recorded evidence"),"report":body("Recorded remediation") }),
        );
    }
    let report = render(&snapshot);
    assert!(report.contains("수정·해결로 기록 · 재점검 결과는 별도 확인"));
    assert!(report.contains("오탐으로 기록"));
    let priority = report.split("## 개선 우선순위").nth(1).unwrap();
    let active = priority.find("| P1 |").unwrap();
    assert!(active < priority.find("| 재확인 |").unwrap());
    assert!(active < priority.find("| 판정 검토 |").unwrap());
}

#[test]
fn inline_markup_and_embedded_fences_cannot_replace_authoritative_report_sections() {
    let mut snapshot = fixture();
    snapshot
        .collections
        .get_mut("tasks")
        .unwrap()
        .get_mut("task-owned")
        .unwrap()
        .data["description"] =
        json!("site\n## FORGED-HEADING <script> [click](https://wrong.invalid)");
    put(
        &mut snapshot,
        "findings",
        "malicious",
        json!({"task_id":"task-owned","name":"Issue | forged\n## FAKE","severity":"high","summary":"````\n## UNSAFE-SOURCE-HEADING\n<script>arbitrary</script>\n````","evidence":body("Observed proof with ``` inside"),"report":body(""),"status":"pending"}),
    );
    let report = render(&snapshot);
    assert!(!report.contains("\n## FORGED-HEADING"));
    assert!(!report.contains("\n## FAKE"));
    assert!(report.contains("&lt;script&gt;"));
    assert!(report.contains(
        "`````text\n````\n## UNSAFE-SOURCE-HEADING\n<script>arbitrary</script>\n````\n`````"
    ));
    assert!(report.contains("Issue \\| forged \\#\\# FAKE"));
}

#[test]
fn complete_blob_evidence_and_saved_remediation_are_retained_without_inline_size_truncation() {
    let mut snapshot = fixture();
    let evidence = format!("EVIDENCE-BEGIN{}EVIDENCE-END", "e".repeat(1_100_000));
    let report_body = format!("REMEDIATION-BEGIN{}REMEDIATION-END", "r".repeat(1_200_000));
    put(
        &mut snapshot,
        "findings",
        "large",
        json!({"task_id":"task-owned","name":"Large evidence","severity":"medium","evidence":{"text":null,"blob":{"sha256":"evidence"}},"report":{"text":null,"blob":{"sha256":"report"}},"evidence_version":"2","report_evidence_version":"2"}),
    );
    let report = assessment_report::render(
        snapshot.get("tasks", "task-owned").unwrap(),
        &snapshot,
        &|r| match r["sha256"].as_str() {
            Some("evidence") => Ok(evidence.as_bytes().to_vec()),
            Some("report") => Ok(report_body.as_bytes().to_vec()),
            _ => panic!("unexpected reference"),
        },
    )
    .unwrap();
    assert!(report.contains(&evidence));
    assert!(report.contains(&report_body));
    assert!(!report.contains("상세 보고서 작성 뒤 증거가 변경"));
}

#[test]
fn unavailable_corrupt_or_non_utf8_bodies_fail_instead_of_manufacturing_a_success_report() {
    let mut snapshot = fixture();
    put(
        &mut snapshot,
        "findings",
        "unreadable",
        json!({"task_id":"task-owned","evidence":{"text":null,"blob":{"sha256":"missing"}},"report":body("")}),
    );
    let task = snapshot.get("tasks", "task-owned").unwrap();
    let error = assessment_report::render(task, &snapshot, &|_| {
        Err(Error::new("HASH_MISMATCH", "Fixture hash failed"))
    })
    .unwrap_err();
    assert_eq!(error.code, "HASH_MISMATCH");
    let error = assessment_report::render(task, &snapshot, &|_| Ok(vec![0xff])).unwrap_err();
    assert_eq!(error.code, "CORRUPT_STORE");
    let mut snapshot = fixture();
    put(
        &mut snapshot,
        "agent_runs",
        "run-owned",
        json!({"task_id":"task-owned","state":"completed"}),
    );
    put(
        &mut snapshot,
        "tool_invocations",
        "bad-result",
        json!({"run_id":"run-owned","state":"completed","result_ref":{"sha256":"result"}}),
    );
    assert_eq!(
        assessment_report::render(
            snapshot.get("tasks", "task-owned").unwrap(),
            &snapshot,
            &|_| Ok(b"not-json".to_vec())
        )
        .unwrap_err()
        .code,
        "CORRUPT_STORE"
    );
}

#[test]
fn imported_node_only_findings_are_explicit_and_never_silently_capped_at_one_thousand() {
    let mut snapshot = fixture();
    for index in 0..1003 {
        put(
            &mut snapshot,
            "exploration_nodes",
            &format!("node-{index:04}"),
            json!({"exploration_id":"exploration-owned","kind":"finding","state":"confirmed","payload":{"name":format!("Legacy finding {index:04}"),"severity":"low","evidence":{"poc":body("Imported saved evidence")}}}),
        );
    }
    let report = render(&snapshot);
    assert!(report.contains("발견 사항: 1003건"));
    assert!(report.contains("Legacy finding 1002"));
    assert!(report.contains("정규 발견 기록 없음"));
    assert_eq!(report.matches("**재현·증거 (저장 원문)**").count(), 1003);
}

fn commit(store: &mut Store, context: &Context, method: &str, params: Value) -> Value {
    let mut plan = artex_domain::plan(method, &params, &store.snapshot(), context).unwrap();
    plan.command_id = new_id("command");
    store.commit(plan).unwrap().result
}

#[test]
fn real_domain_writes_and_reopen_produce_identical_contract_valid_report_without_new_writes() {
    let dir = tempfile::tempdir().unwrap();
    let mut store = Store::create(dir.path(), "Persisted site report fixture").unwrap();
    let mut context = Context {
        actor: Actor {
            kind: "user".into(),
            agent_id: None,
            run_id: None,
            engine_epoch: new_id("epoch"),
        },
        task_id: None,
        intent_id: None,
        conversation_id: None,
    };
    let created = commit(
        &mut store,
        &context,
        "task.create",
        json!({"description":"example.test saved assessment","goal":"Inspect authorized responses"}),
    );
    let task = created["task_id"].as_str().unwrap().to_owned();
    context.task_id = Some(task.clone());
    commit(
        &mut store,
        &context,
        "set_goals",
        json!({"text":"Check response error disclosures"}),
    );
    let found = commit(
        &mut store,
        &context,
        "report_finding",
        json!({"name":"Error disclosure","vulnclass":"Information disclosure","severity":"high","summary":"A response disclosed an internal path","evidence":"Captured response body from the fixture"}),
    );
    let node = found["finding_node_id"].clone();
    commit(
        &mut store,
        &context,
        "update_finding_report",
        json!({"finding_node_id":node,"evidence_version":"0","report":"Disable debug responses and verify that the path no longer appears."}),
    );
    commit(
        &mut store,
        &context,
        "goal_met",
        json!({"reason":"Authorized checks concluded"}),
    );
    let before = store.snapshot();
    let first = report_export::task_report(&json!({"task":task}), &before, &|r| {
        let mut bytes = Vec::new();
        store.copy_blob_to(r, &mut bytes)?;
        Ok(bytes)
    })
    .unwrap();
    assert_eq!(store.snapshot(), before);
    let contract = artex_core::contracts::read("rpc.v1.schema.json");
    artex_core::validate_contract(&contract["results"]["ui.report"], &contract, &first).unwrap();
    assert!(
        first["text"]
            .as_str()
            .unwrap()
            .contains("Disable debug responses")
    );
    assert!(first["text"].as_str().unwrap().contains("**종료 보고서**"));
    drop(store);
    let reopened = Store::open_read_only(dir.path()).unwrap();
    let second = report_export::task_report(&json!({"task":task}), &reopened.snapshot(), &|r| {
        let mut bytes = Vec::new();
        reopened.copy_blob_to(r, &mut bytes)?;
        Ok(bytes)
    })
    .unwrap();
    assert_eq!(first, second);
}
