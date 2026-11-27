//! Actual report tab -> Service query -> display -> same workspace reopen.
//! Native picker interaction is intentionally outside this headless test.
use super::*;

fn report_model_tool(name: &str, input: Value) -> transport::Step {
    transport::Step::Reply(200, "application/json", json!({"choices":[{"message":{"content":null,
        "tool_calls":[{"id":format!("report-{name}"),"type":"function","function":{"name":name,"arguments":input.to_string()}}]},
        "finish_reason":"tool_calls"}],"usage":{"prompt_tokens":4,"completion_tokens":2}}).to_string())
}

fn report_tool_result(request: &Value, name: &str) -> String {
    let message = request["messages"].as_array().unwrap().iter().find(|m| {
        m["role"] == "tool" && m["tool_call_id"] == format!("report-{name}")
    }).unwrap_or_else(|| panic!("No actual {name} tool result: {request}"));
    message["content"].as_str().unwrap().to_owned()
}

fn export_report_to(f: &mut Fixture, destination: Option<PathBuf>) {
    // This queue replaces only the OS save dialog result. Actual egui Export
    // -> StageBody -> capability -> SaveFile/ReleaseFile is not substituted.
    TEST_EXPORT_DESTINATIONS.with(|queue| queue.borrow_mut().push_back(destination));
    f.click_visible("보고서 내보내기 (.md)");
    assert!(f.app.pending.values().any(|p| matches!(p, Purpose::ExportFile { .. })));
    f.until(|app| app.pending.is_empty());
    assert!(f.app.error.is_none(), "{:?}", f.app.error);
    TEST_EXPORT_DESTINATIONS.with(|queue| assert!(queue.borrow().is_empty()));
}

#[test]
fn completed_site_analysis_tools_report_export_cancel_and_readonly_reopen_are_connected() {
    use transport::*;
    use std::sync::Mutex;
    const OBSERVATION: &str = "LOCAL_REPORT_OBSERVATION: fixture configuration enables a demonstration-only issue. 한글 evidence.";
    const REMEDIATION: &str = "## Repair suggestions\nREPORT_REPAIR_MARKER: disable the fixture option and repeat the same local check. 한글 개선안.";
    let provider = LocalProvider::from_handler(|index, request| {
        let (name, input) = match index {
            0 => ("Read", json!({"file_path":"report-observation.txt"})),
            1 => {
                assert!(report_tool_result(request, "Read").contains(OBSERVATION));
                ("report_finding", json!({"name":"LOCAL_REPORT_FINDING","vulnclass":"fixture-only",
                    "severity":"high","summary":"REPORT_SUMMARY_MARKER","evidence":OBSERVATION}))
            }
            2 => {
                // The source-compatible tool projection has a readable first
                // line followed by JSON; use the returned IDs, never fixture IDs.
                let output = report_tool_result(request, "report_finding");
                let result: Value = serde_json::from_str(output.split_once('\n').unwrap().1).unwrap();
                ("update_finding_report", json!({"finding_node_id":result["finding_node_id"],
                    "evidence_version":result["evidence_version"],"report":REMEDIATION}))
            }
            3 => {
                assert!(report_tool_result(request, "update_finding_report").contains("report updated"));
                ("goal_met", json!({"reason":"Only the local fixture scope was checked and recorded"}))
            }
            _ => panic!("Unexpected provider request {index}: {request}"),
        };
        assert!(request["tools"].as_array().unwrap().iter().any(|t| t["function"]["name"] == name),
            "The real tool offer must include {name}");
        report_model_tool(name, input)
    });
    let secret = artex_core::new_id("secret");
    let probe = Probe::default();
    let root = Arc::new(Mutex::new(None::<PathBuf>));
    let opened = root.clone();
    let mut f = Fixture::configured(|profile| {
        let base = factory(profile, BTreeMap::from([(secret.clone(),"isolated-report-value".into())]), probe.clone());
        ServiceConfig { host_factory: Some(Arc::new(move |workspace| {
            *opened.lock().unwrap() = Some(workspace.work_root.clone()); base(workspace)
        })), wake_ui: None }
    });
    let other = source_task(&mut f, "LOCAL_OTHER_REPORT_SCOPE");
    let task = source_task(&mut f, "LOCAL_COMPLETED_REPORT_SCOPE");
    // Scope setup supplies an existing goal so resume enters the planner,
    // rather than asking the separate goals agent to invent this fixture.
    f.setup("ui.addGoal",json!({"id":task,"text":"Check only the local observation fixture"}));
    // Adding a goal intentionally revives the task. Stop it before configuring
    // the local provider so no run races the setup commands below.
    select(&mut f,Section::Tasks,Some(&task));
    f.click_visible("작업 중지·정리");
    f.until(|app| app.pending.is_empty() && app.handle.view().unwrap().generation.get("tasks",&task)
        .is_some_and(|t| t.data["paused"] == true && t.data["queued"] == false));
    // This case qualifies one configured planner. Reporter trigger fan-out is
    // a separate acceptance path; disable it through the public configuration.
    let triggers = f.app.handle.view().unwrap().generation.records("agent_triggers").into_iter()
        .filter(|t| t.data["agent_key"] == "reporter").cloned().collect::<Vec<_>>();
    for trigger in triggers {
        let mut dto = json!({"enabled":false,"tool_names":["report_finding"]});
        for key in ["interval_sec","on_finding","on_goal_met","on_task_timeout","on_tool_call","on_task_create",
            "interval_message","finding_message","goal_message","task_timeout_message","tool_call_message","task_create_message"] {
            dto[key] = trigger.data[key].clone();
        }
        f.setup("ui.updateTrigger",json!({"id":trigger.header.id,"t":dto}));
    }
    let profile = provider::profile(&mut f,"Report fixture model","openai",&provider.url,&secret,false);
    f.setup("ui.activateLLMProfile",json!({"id":profile}));
    // goal_met is deliberately unbound by default; opt in through the same
    // public editor command. Do not bypass role/binding/approval gates.
    for key in ["Read","report_finding","update_finding_report","goal_met"] {
        let tool = f.app.handle.view().unwrap().generation.records("tools").into_iter()
            .find(|t| t.data["key"] == key).unwrap().data.clone();
        let mut agents = tool["agents"].as_array().unwrap().clone();
        if !agents.iter().any(|a| a == "planner") { agents.push(json!("planner")); }
        f.setup("ui.saveTool", json!({"key":key,"patch":{"enabled":true,"agents":agents,
            "description":tool["description"],"schema":tool["schema"]}}));
    }
    let input = root.lock().unwrap().as_ref().unwrap().join("tasks").join(&task).join("report-observation.txt");
    std::fs::create_dir_all(input.parent().unwrap()).unwrap();
    std::fs::write(&input, OBSERVATION).unwrap();
    select(&mut f, Section::Tasks, Some(&task));
    f.app.drawer.open = false;
    f.click_visible("재개");
    f.prepare("ui.controlTask");
    f.until(|app| {
        let view = app.handle.view().unwrap();
        view.generation.get("tasks",&task).unwrap().data["status"] == "done"
            && view.generation.records("agent_runs").iter().any(|r| r.data["task_id"] == task && r.data["state"] == "completed")
            && !view.generation.records("agent_runs").iter().any(|r| r.data["task_id"] == task
                && matches!(r.data["state"].as_str(),Some("queued"|"running"|"waiting")))
            && app.pending.is_empty()
    });
    assert_eq!(provider.requests().len(),4,"goal_met settles the task without another model turn");
    assert_eq!(probe.attempts.lock().unwrap().iter().filter(|r| r["input"]["name"] == "Read").count(),1);
    let view = f.app.handle.view().unwrap();
    let calls = view.generation.records("tool_invocations").into_iter().filter(|call| {
        view.generation.get("agent_runs",call.data["run_id"].as_str().unwrap())
            .is_some_and(|run| run.data["task_id"] == task)
    }).collect::<Vec<_>>();
    assert_eq!(calls.len(),4);
    for name in ["Read","report_finding","update_finding_report","goal_met"] {
        assert!(calls.iter().any(|call| call.data["name"] == name && call.data["state"] == "completed"
            && call.data["result_ref"].is_object()),"Missing actual stored completion for {name}");
    }
    let finding = view.generation.records("findings").into_iter().find(|r| r.data["task_id"] == task).unwrap().clone();
    assert_eq!(finding.data["summary"],"REPORT_SUMMARY_MARKER");
    assert_eq!(finding.data["report"]["text"],REMEDIATION);
    f.click_visible("종합 보고서");
    f.until(|app| app.pending.is_empty() && app.panes.projection.get("ui.report").is_some_and(|b| b["text"].is_string()));
    let report = f.app.panes.projection["ui.report"]["text"].as_str().unwrap().to_owned();
    let report_seq: u64 = report.lines().find_map(|line| line.strip_prefix("- 이 보고서의 저장 세대: "))
        .unwrap().parse().unwrap();
    for expected in ["종료 보고서",r"LOCAL\_COMPLETED\_REPORT\_SCOPE",r"LOCAL\_REPORT\_FINDING","REPORT_SUMMARY_MARKER",
        OBSERVATION, REMEDIATION, "P1", "미검증", "Read", "사이트가 안전하다는 뜻이 아닙니다"] {
        assert!(report.contains(expected),"Missing {expected}: {report}");
    }
    assert!(!report.contains(r"LOCAL\_OTHER\_REPORT\_SCOPE"));
    let painted = Fixture::texts(&f.frame(vec![])).join("\n");
    for expected in ["종료 보고서","REPORT_SUMMARY_MARKER","REPORT_REPAIR_MARKER"] {assert!(painted.contains(expected),"Missing display {expected}");}
    let before = f.app.handle.view().unwrap().generation.snapshot.clone();
    assert!(report_seq <= before.commit_seq);
    let destination = f._dir.path().join("completed-report.md");
    export_report_to(&mut f,Some(destination.clone()));
    assert_eq!(std::fs::read(&destination).unwrap(),report.as_bytes());
    let successful_notices = f.app.notices.iter().filter(|n| n.as_str()=="내보내기를 완료했습니다.").count();
    assert!(successful_notices>0);
    export_report_to(&mut f,None);
    assert_eq!(f.app.notices.iter().filter(|n| n.as_str()=="내보내기를 완료했습니다.").count(),successful_notices);
    assert_eq!(before.collections,f.app.handle.view().unwrap().generation.snapshot.collections);
    select(&mut f,Section::Tasks,Some(&other));
    f.until(|app| app.pending.is_empty() && app.panes.projection.get("ui.report").is_some_and(|b|
        b["text"].as_str().is_some_and(|s|s.contains(r"LOCAL\_OTHER\_REPORT\_SCOPE"))));
    assert!(!f.app.panes.projection["ui.report"]["text"].as_str().unwrap().contains(r"LOCAL\_REPORT\_FINDING"));
    f.reopen_same_workspace();
    select(&mut f,Section::Tasks,Some(&task));
    f.click_visible("종합 보고서");
    f.until(|app| app.pending.is_empty() && app.panes.projection.get("ui.report").is_some_and(|b|b["text"].is_string()));
    let reopened = f.app.handle.view().unwrap().generation.snapshot.clone();
    let mut changed = Vec::new();
    for (collection, rows) in &before.collections {
        for (id, row) in rows {
            let after = reopened.get(collection,id);
            if after != Some(row) { changed.push(json!({"collection":collection,"id":id,"before":row,"after":after})); }
        }
    }
    assert!(changed.is_empty(),"Closing/reopening changed assessment objects: {changed:#?}");
    let added = reopened.collections.iter().flat_map(|(collection,rows)| rows.values()
        .filter(|row| before.get(collection,&row.header.id).is_none())).collect::<Vec<_>>();
    assert_eq!(added.len(),1,"Only the actual native retirement receipt may be added on close: {added:#?}");
    assert_eq!(added[0].header.collection,"command_receipts");
    assert_eq!(added[0].data["method"],"native.lifecycle.cleanup");
    assert_eq!(added[0].data["normalized_params"]["phase"],"workspace_retired");
    assert_eq!(added[0].data["result"]["runs"].as_array().unwrap().len(),1);
    assert_eq!(added[0].data["result"]["runs"][0]["cleanup_confirmed"],true);
    // A real NativeGateway records native.lifecycle.cleanup on close. That
    // receipt advances commit_seq even with unchanged objects; the report must
    // identify that new read generation without changing analysis/evidence.
    assert_eq!(reopened.commit_seq,before.commit_seq+1);
    let reopened_report = report.replacen(
        // A report pins its own read generation, not the later UI read view.
        // Final clock accounting can advance the view without reloading text.
        &format!("- 이 보고서의 저장 세대: {}\n",report_seq),
        &format!("- 이 보고서의 저장 세대: {}\n",reopened.commit_seq),1);
    assert_eq!(f.app.panes.projection["ui.report"]["text"],reopened_report);
    let readonly_destination = f._dir.path().join("readonly-report.md");
    export_report_to(&mut f,Some(readonly_destination));
    assert_eq!(std::fs::read(f._dir.path().join("readonly-report.md")).unwrap(),reopened_report.as_bytes());
    assert_eq!(reopened.collections,f.app.handle.view().unwrap().generation.snapshot.collections);
    assert_eq!(reopened.commit_seq,f.app.handle.view().unwrap().generation.snapshot.commit_seq);
    assert_eq!(provider.requests().len(),4,"Reports/reopen/export must not restart models");
    f.shutdown();
    assert_eq!(provider.finish().len(),4);
    println!("REPORT_ACCEPT Read -> report_finding -> update_finding_report -> goal_met -> settled report -> export/cancel -> task isolation -> readonly reopen/export");
}

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
