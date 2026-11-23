//! A reproducible task assessment, projected only from saved task-owned records.
//! This is an evidence inventory, never a new model assessment or a safety claim.
use crate::traffic_ui::{BlobReader, body_bytes};
use crate::*;
use std::collections::BTreeMap;

fn inline(value: &str) -> String {
    value
        .chars()
        .flat_map(|c| match c {
            '\n' | '\r' | '\t' => " ".chars().collect::<Vec<_>>(),
            '<' => "&lt;".chars().collect(),
            '>' => "&gt;".chars().collect(),
            '&' => "&amp;".chars().collect(),
            '\\' | '`' | '*' | '_' | '[' | ']' | '(' | ')' | '#' | '|' | '!' => vec!['\\', c],
            c if c.is_control() => vec![],
            c => vec![c],
        })
        .collect()
}

fn value<'a>(data: &'a Value, key: &str, fallback: &'a str) -> &'a str {
    let result = text(data, key);
    if result.trim().is_empty() {
        fallback
    } else {
        result
    }
}

/// Site evidence can contain Markdown. A fence longer than every source run
/// keeps an attacker-controlled result from creating authoritative report sections.
fn quote(out: &mut String, content: &str) {
    if content.is_empty() {
        return;
    }
    let longest = content.split(|c| c != '`').map(str::len).max().unwrap_or(0);
    let fence = "`".repeat(longest.saturating_add(1).max(3));
    out.push_str(&format!("{fence}text\n{content}\n{fence}\n\n"));
}

fn body(value: &Value, reader: &BlobReader) -> Result<String> {
    // Lossy decoding would silently alter evidence. A missing/corrupt Blob must
    // fail the report query, preserving the last successfully displayed report.
    String::from_utf8(body_bytes(value, reader)?)
        .map_err(|_| Error::new("CORRUPT_STORE", "Assessment report body is not UTF-8"))
}

fn ordered(mut rows: Vec<&Record>) -> Vec<&Record> {
    rows.sort_by(|a, b| {
        a.header
            .created_at
            .cmp(&b.header.created_at)
            .then_with(|| {
                a.header
                    .commit_seq
                    .parse::<u64>()
                    .unwrap_or(0)
                    .cmp(&b.header.commit_seq.parse::<u64>().unwrap_or(0))
            })
            .then(a.header.id.cmp(&b.header.id))
    });
    rows
}

fn active(state: &str) -> bool {
    matches!(state, "queued" | "running" | "waiting")
}

fn title(data: &Value) -> &str {
    value(data, "name", value(data, "vulnclass", "이름 미기록"))
}

fn priority(data: &Value) -> (&'static str, &'static str) {
    if matches!(text(data, "status"), "fixed" | "resolved") {
        return (
            "재확인",
            "수정·해결로 저장됨. 실제 재점검 결과를 확인하세요.",
        );
    }
    if matches!(
        text(data, "status"),
        "false_positive" | "duplicate" | "ignored" | "risk_accepted"
    ) {
        return (
            "판정 검토",
            "오탐·중복·제외·위험 수용의 저장 사유를 검토하세요.",
        );
    }
    match text(data, "severity") {
        "critical" => (
            "P0",
            "최우선으로 재현 근거와 영향을 확인하고 대응을 결정하세요.",
        ),
        "high" => (
            "P1",
            "높은 위험으로 기록됐습니다. 우선 수정 계획을 수립하세요.",
        ),
        "medium" => ("P2", "영향과 노출 범위를 확인해 수정 일정을 정하세요."),
        "low" => ("P3", "낮은 위험으로 기록됐습니다. 개선 일정에 반영하세요."),
        _ => (
            "분류 필요",
            "위험도가 없거나 알려진 분류가 아닙니다. 먼저 분류하세요.",
        ),
    }
}

fn evidence_body(data: &Value, reader: &BlobReader) -> Result<String> {
    let evidence = &data["evidence"];
    body(evidence.get("poc").unwrap_or(evidence), reader)
}

struct Finding<'a> {
    id: &'a str,
    data: &'a Value,
    node: Option<&'a Record>,
    canonical: bool,
}

/// Render one live task using immutable state and verified Blob reads. No
/// timestamps, LLM calls, writes, global asset totals, or inherited-task traces
/// are introduced. The same saved generation produces exactly the same bytes.
pub fn render(task: &Record, s: &Snapshot, reader: &BlobReader) -> Result<String> {
    let exp = required(&task.data, "exploration_id")?;
    let nodes = ordered(
        s.records("exploration_nodes")
            .into_iter()
            .filter(|r| text(&r.data, "exploration_id") == exp)
            .collect(),
    );
    let node_ids: BTreeSet<_> = nodes.iter().map(|r| r.header.id.as_str()).collect();
    let runs = ordered(
        s.records("agent_runs")
            .into_iter()
            .filter(|r| r.data["task_id"] == task.header.id)
            .collect(),
    );
    let run_ids: BTreeSet<_> = runs.iter().map(|r| r.header.id.as_str()).collect();
    let tools = ordered(
        s.records("tool_invocations")
            .into_iter()
            .filter(|r| run_ids.contains(text(&r.data, "run_id")))
            .collect(),
    );
    let activity = ordered(
        s.records("activity")
            .into_iter()
            .filter(|r| text(&r.data, "exploration_id") == exp)
            .collect(),
    );
    let canonical = s
        .records("findings")
        .into_iter()
        .filter(|r| r.data["task_id"] == task.header.id)
        .collect::<Vec<_>>();
    let canonical_nodes: BTreeSet<_> = canonical.iter().map(|r| text(&r.data, "node_id")).collect();
    let mut findings = canonical
        .iter()
        .map(|r| Finding {
            id: &r.header.id,
            data: &r.data,
            node: nodes
                .iter()
                .find(|n| n.header.id == text(&r.data, "node_id"))
                .copied(),
            canonical: true,
        })
        .chain(
            nodes
                .iter()
                .filter(|n| {
                    n.data["kind"] == "finding" && !canonical_nodes.contains(n.header.id.as_str())
                })
                .map(|n| Finding {
                    id: &n.header.id,
                    data: &n.data["payload"],
                    node: Some(n),
                    canonical: false,
                }),
        )
        .collect::<Vec<_>>();
    findings.sort_by(|a, b| {
        findings_ui::severity(text(b.data, "severity"))
            .cmp(&findings_ui::severity(text(a.data, "severity")))
            .then(a.id.cmp(b.id))
    });
    let mut assets: BTreeSet<String> = s
        .records("assets")
        .into_iter()
        .filter(|r| ids(&r.data, "task_ids").contains(&task.header.id))
        .map(|r| r.header.id.clone())
        .collect();
    // Imported stores may have only the original relation table populated;
    // assets.task_ids is a compatibility projection, not the sole owner link.
    for link in s.records("task_asset_links") {
        if link.data["task_id"] == task.header.id {
            assets.insert(text(&link.data, "asset_id").to_owned());
        }
    }
    for anchor in s.records("exploration_anchors") {
        if node_ids.contains(text(&anchor.data, "node_id")) {
            assets.insert(text(&anchor.data, "asset_id").to_owned());
        }
    }
    for finding in &findings {
        assets.extend(ids(finding.data, "asset_ids"));
    }
    let active_runs = runs
        .iter()
        .filter(|r| active(text(&r.data, "state")))
        .count();
    let uncertain_runs = runs
        .iter()
        .filter(|r| matches!(text(&r.data, "state"), "interrupted" | "recovery_required"))
        .count();
    let pending_tools = tools
        .iter()
        .filter(|r| !matches!(text(&r.data, "state"), "completed" | "failed" | "cancelled"))
        .count();
    let status = text(&task.data, "status");
    let runtime_recovery = s.records("task_runtime").iter().any(|r| {
        r.data["task_id"] == task.header.id && r.data["accounting_state"] == "recovery_required"
    });
    let complete = status == "done"
        && active_runs == 0
        && pending_tools == 0
        && uncertain_runs == 0
        && !runtime_recovery
        && task.data["paused"] != true
        && task.data["queued"] != true;
    // Only the typed error flag is projected. A result may contain credentials,
    // prompts, shared task records or binary blocks; copying it wholesale would
    // cross the report's task-local evidence boundary. Reading the reference
    // still verifies that a claimed saved result is actually available.
    let mut tool_errors = BTreeMap::new();
    for tool in &tools {
        if !tool.data["result_ref"].is_null() {
            let bytes = body_bytes(&tool.data["result_ref"], reader)?;
            let result: Value = serde_json::from_slice(&bytes)
                .map_err(|_| Error::new("CORRUPT_STORE", "Saved tool result is not JSON"))?;
            let is_error = result["is_error"].as_bool().ok_or_else(|| {
                Error::new("CORRUPT_STORE", "Saved tool result has no error flag")
            })?;
            tool_errors.insert(tool.header.id.as_str(), is_error);
        }
    }
    let report_state = if complete {
        "종료 보고서"
    } else {
        "부분 보고서 · 미완료 또는 실행 정리 확인 필요"
    };
    let mut out = format!(
        "# 사이트별 종합 결과 보고서\n\n## 분석 개요\n\n- 보고서 상태: **{report_state}**\n- 작업: {}\n- 작업 ID: {}\n- 저장된 분석 상태: {}\n- 시작: {}\n- 종료: {}\n- 이 보고서의 저장 세대: {}\n- 대상 작업 최종 저장: {}\n- 활성 실행: {}개 / 미종결 도구: {}개\n\n",
        inline(value(&task.data, "description", "이름 없는 작업")),
        inline(&task.header.id),
        inline(status),
        inline(value(&task.data, "first_run_at", "시작 미기록")),
        inline(value(&task.data, "completed_at", "종료 미기록")),
        s.commit_seq,
        inline(&task.header.updated_at),
        active_runs,
        pending_tools
    );
    out.push_str("이 보고서는 저장된 기록을 집계합니다. 도구 실행 성공이나 작업 완료는 사이트가 안전하다는 뜻이 아닙니다. 외부 프로세스의 실제 종료·자원 정리는 실행 상세에서 별도로 확인해야 합니다.\n\n");
    for note in artex_core::execution_status::timeout_notes(s, task) {
        out.push_str(&format!("- {}\n", inline(&note)));
    }
    let failures = artex_core::execution_status::failure_groups(s, task);
    if !failures.is_empty() {
        out.push_str("\n### 저장된 실행 실패 원인\n\n");
        for (kind, count) in failures {
            out.push_str(&format!("- {}: {count}건\n", inline(kind.label())));
        }
        out.push_str("\n원인별 집계는 실패한 실행별 최종 오류 기록에 근거합니다. 도구별 오류 원문은 실행 상세에서 확인하세요.\n\n");
    }
    out.push_str("\n**분석 목표**\n\n");
    quote(&mut out, value(&task.data, "goal", "목표 미기록"));
    out.push_str("## 대상과 분석 범위\n\n대상은 이 작업에 연결된 자산·범위·목표만 표시합니다. 다른 작업에서 공유받은 기록은 이 작업의 실제 검사 실적으로 합산하지 않습니다. 등록된 자산은 검사 완료를 뜻하지 않습니다.\n\n");
    let scopes = ordered(
        s.records("task_scope")
            .into_iter()
            .filter(|r| r.data["task_id"] == task.header.id)
            .collect(),
    );
    for scope in &scopes {
        out.push_str(&format!(
            "- 범위 {} / 유형 {} / domain: {} / net: {} / value: {} / company ID: {}\n",
            inline(&scope.header.id),
            inline(text(&scope.data, "kind")),
            inline(text(&scope.data, "domain")),
            inline(text(&scope.data, "net")),
            inline(text(&scope.data, "value")),
            inline(text(&scope.data, "company_id"))
        ));
    }
    out.push_str(&format!("\n작업 연결 자산: {}개\n\n", assets.len()));
    for id in &assets {
        if let Some(asset) = s.get("assets", id) {
            out.push_str(&format!(
                "- {} / {} / {}\n",
                inline(id),
                inline(text(&asset.data, "type")),
                inline(&exploration_ui::asset_label(&asset.data))
            ));
        } else {
            out.push_str(&format!(
                "- {} / 연결 자산 기록 없음 · 대상 확인 필요\n",
                inline(id)
            ));
        }
    }
    for constraint in ordered(
        s.records("task_constraints")
            .into_iter()
            .filter(|r| text(&r.data, "exploration_id") == exp)
            .collect(),
    ) {
        out.push_str(&format!(
            "\n범위 제약 {} / {}\n\n",
            inline(&constraint.header.id),
            inline(text(&constraint.data, "kind"))
        ));
        quote(&mut out, text(&constraint.data, "text"));
    }
    for rule in ordered(
        s.records("task_intercept_rules")
            .into_iter()
            .filter(|r| r.data["task_id"] == task.header.id)
            .collect(),
    ) {
        out.push_str(&format!(
            "\n접근 규칙 {} / 활성 {} / {} / {}\n\n",
            inline(&rule.header.id),
            rule.data["enabled"] == true,
            inline(text(&rule.data, "action")),
            inline(text(&rule.data, "kind"))
        ));
        quote(&mut out, text(&rule.data, "pattern"));
        quote(&mut out, text(&rule.data, "note"));
    }
    out.push_str("\n## 검사 내역\n\n계획·완료 표시는 저장된 상태입니다. 계획 등록만으로 검사를 실행했다고 판단하지 않으며, 도구 완료만으로 취약점 부재나 특정 검사 항목 통과를 추론하지 않습니다.\n\n### 목표와 검사 계획\n\n");
    let plans: Vec<_> = nodes
        .iter()
        .copied()
        .filter(|n| matches!(text(&n.data, "kind"), "goal" | "intent"))
        .collect();
    if plans.is_empty() {
        out.push_str(
            "검사 항목별 계획 기록이 없습니다. 전체 보안 항목을 검사했는지 확인할 수 없습니다.\n\n",
        );
    }
    for plan in &plans {
        let linked_runs = runs
            .iter()
            .filter(|r| r.data["intent_id"] == plan.header.id)
            .count();
        out.push_str(&format!(
            "#### {} · {} · 상태 {}\n\n연결 실행 {}개 / 생성 {} / 완료 {}\n\n",
            inline(&plan.header.id),
            inline(text(&plan.data, "kind")),
            inline(text(&plan.data, "state")),
            linked_runs,
            inline(&plan.header.created_at),
            inline(text(&plan.data, "completed_at"))
        ));
        quote(
            &mut out,
            value(
                &plan.data["payload"],
                "summary",
                text(&plan.data["payload"], "text"),
            ),
        );
        for key in ["proof_reason", "stop_reason", "blocked_reason"] {
            if !text(&plan.data["payload"], key).is_empty() {
                out.push_str(&format!("{}\n\n", inline(key)));
                quote(&mut out, text(&plan.data["payload"], key));
            }
        }
        if !text(&plan.data, "blocked_reason").is_empty() {
            out.push_str("차단 사유\n\n");
            quote(&mut out, text(&plan.data, "blocked_reason"));
        }
    }
    out.push_str("### 실제 도구 실행 기록\n\n실제 저장된 호출과 상태를 열거합니다. 준비·승인 대기·취소는 성공한 검사가 아닙니다. 임의 입력·결과 원문에는 자격 증명·모델 문맥·다른 작업 자료가 섞일 수 있어 이 보고서에 복제하지 않습니다. 원문은 해당 실행 상세에서 확인하세요.\n\n");
    if tools.is_empty() {
        out.push_str("네이티브 도구 호출 기록이 없습니다. 아래 과거 활동 기록과 사실 기록을 함께 확인하세요.\n\n");
    }
    for tool in &tools {
        out.push_str(&format!(
            "- 도구 {} / 호출 {} / 실행 {} / 상태 {} / 기록 {} / 결과 본문 {} / 저장 결과 오류 {}\n",
            inline(text(&tool.data, "name")),
            inline(&tool.header.id),
            inline(text(&tool.data, "run_id")),
            inline(text(&tool.data, "state")),
            inline(&tool.header.created_at),
            if tool.data["result_ref"].is_null() {
                "없음"
            } else {
                "저장됨 · 실행 상세에서 확인"
            }, match tool_errors.get(tool.header.id.as_str()) {Some(true)=>"있음",Some(false)=>"없음",None=>"미확인"}
        ));
    }
    let tool_activities: Vec<_> = activity
        .iter()
        .copied()
        .filter(|r| matches!(text(&r.data, "kind"), "tool_use" | "tool_result"))
        .collect();
    out.push_str(&format!(
        "\n저장된 도구 활동: {}건 (호출·결과 활동이며 위 호출 수와 중복될 수 있습니다.)\n\n",
        tool_activities.len()
    ));
    for row in tool_activities {
        out.push_str(&format!(
            "- 활동 {} / {} / {} / 호출 식별자 {} / 오류 {} / {}\n",
            inline(&row.header.id),
            inline(text(&row.data, "kind")),
            inline(text(&row.data, "tool")),
            inline(text(&row.data, "tool_use_id")),
            row.data["is_error"] == true,
            inline(text(&row.data, "summary"))
        ));
    }
    out.push_str("\n### 저장된 관찰·실행 결과\n\n관찰은 저장한 작성자의 진술이며, 독립 검증이나 모든 검사 통과를 뜻하지 않습니다.\n\n");
    for fact in nodes.iter().filter(|n| n.data["kind"] == "fact") {
        out.push_str(&format!(
            "관찰 {} / 상태 {}\n\n",
            inline(&fact.header.id),
            inline(text(&fact.data, "state"))
        ));
        quote(
            &mut out,
            value(
                &fact.data["payload"],
                "summary",
                text(&fact.data["payload"], "text"),
            ),
        );
    }
    for result in activity
        .iter()
        .filter(|r| r.data["kind"] == "result" || r.data["is_error"] == true)
    {
        out.push_str(&format!(
            "- 저장 결과 {} / 오류 {} / {}\n",
            inline(&result.header.id),
            result.data["is_error"] == true,
            inline(text(&result.data, "summary"))
        ));
    }
    out.push_str("\n### 실행과 미완료 지시\n\n");
    for run in &runs {
        out.push_str(&format!(
            "- 실행 {} / 상태 {} / 목적 {} / 연결 검사 {} / 시작 {} / 종료 {}\n",
            inline(&run.header.id),
            inline(text(&run.data, "state")),
            inline(text(&run.data, "purpose")),
            inline(text(&run.data, "intent_id")),
            inline(text(&run.data, "started_at")),
            inline(text(&run.data, "finished_at"))
        ));
        for todo in run.data["todos"].as_array().into_iter().flatten() {
            out.push_str(&format!(
                "  - 할 일 상태 {}: {}\n",
                inline(text(todo, "status")),
                inline(text(todo, "content"))
            ));
        }
    }
    for hint in nodes.iter().filter(|n| n.data["kind"] == "hint") {
        out.push_str(&format!(
            "\n검증 지시·힌트 {} / 상태 {} (힌트 등록은 검사 실행이 아님)\n\n",
            inline(&hint.header.id),
            inline(text(&hint.data, "state"))
        ));
        quote(
            &mut out,
            value(
                &hint.data["payload"],
                "text",
                text(&hint.data["payload"], "summary"),
            ),
        );
    }
    out.push_str("\n## 발견 문제\n\n확인·의심의 구분은 저장된 발견 상태와 근거를 표시한 것입니다. 자동 집계가 새로운 재현·판정을 수행하지는 않습니다.\n\n");
    if findings.is_empty() {
        out.push_str("저장된 발견 사항이 없습니다. 취약점이 없거나 사이트가 안전하다는 결론을 내릴 수 없습니다.\n\n");
    }
    let mut gaps = BTreeSet::new();
    for (index, finding) in findings.iter().enumerate() {
        let data = finding.data;
        let evidence = evidence_body(data, reader)?;
        let report = if finding.canonical {
            body(&data["report"], reader)?
        } else {
            String::new()
        };
        let confirmed = finding.node.is_some_and(|n| n.data["state"] == "confirmed")
            || data["status"] == "confirmed";
        let classification = if data["status"] == "false_positive" {
            "오탐으로 기록"
        } else if matches!(text(data, "status"), "fixed" | "resolved") {
            "수정·해결로 기록 · 재점검 결과는 별도 확인"
        } else if data["status"] == "duplicate" {
            "중복으로 기록"
        } else if data["status"] == "ignored"
            || finding.node.is_some_and(|n| n.data["state"] == "dismissed")
        {
            "제외로 기록 · 판정 근거 확인 필요"
        } else if data["status"] == "risk_accepted" {
            "위험 수용으로 기록 · 해결을 뜻하지 않음"
        } else if confirmed {
            "확인으로 기록 · 독립 재검증 아님"
        } else {
            "의심·확인 필요"
        };
        out.push_str(&format!("### {}. {}\n\n- 발견 ID: {}\n- 구분: {}\n- 분류: {}\n- 기록된 위험도: {}\n- 처리 상태: {}\n- 발견 근거 버전: {} / 상세 보고서 근거 버전: {}\n\n**영향·요약 (저장 원문)**\n\n",index+1,inline(title(data)),inline(finding.id),classification,inline(text(data,"vulnclass")),inline(value(data,"severity","미분류")),inline(value(data,"status","처리 상태 미기록")),inline(text(data,"evidence_version")),inline(text(data,"report_evidence_version"))));
        quote(&mut out, value(data, "summary", "영향·요약 미기록"));
        if evidence.trim().is_empty() {
            out.push_str("재현·증거 본문이 없습니다.\n\n");
            gaps.insert(format!("발견 {}: 재현·증거 본문 미기록", finding.id));
        } else {
            out.push_str("**재현·증거 (저장 원문)**\n\n");
            quote(&mut out, &evidence);
        }
        if finding.canonical {
            for binding in ordered(
                s.records("finding_traffic_bindings")
                    .into_iter()
                    .filter(|b| b.data["finding_id"] == finding.id)
                    .collect(),
            ) {
                let sid = required(&binding.data, "snapshot_id")?;
                if let Some(snapshot) = s.get("traffic_evidence_snapshots", sid) {
                    out.push_str(&format!("- 트래픽 증거 {} / {} / {} {} / HTTP {} / 요청 SHA256 {} / 응답 SHA256 {}\n",inline(&binding.header.id),inline(text(&binding.data,"role")),inline(text(&snapshot.data,"method")),inline(text(&snapshot.data,"url")),inline(&snapshot.data["status"].to_string()),inline(text(&snapshot.data,"req_hash")),inline(text(&snapshot.data,"resp_hash"))));
                    if !text(&binding.data, "note").is_empty() {
                        quote(&mut out, text(&binding.data, "note"));
                    }
                } else {
                    gaps.insert(format!(
                        "발견 {}: 연결 트래픽 증거 {} 없음",
                        finding.id, sid
                    ));
                    out.push_str(&format!(
                        "- 연결 트래픽 증거 {} 없음 · 확인 필요\n",
                        inline(sid)
                    ));
                }
            }
        }
        if report.trim().is_empty() {
            out.push_str("\n구체적인 원인·수정 방법이 담긴 상세 보고서가 저장되지 않았습니다. 이 보고서는 임의의 기술적 수정안을 만들지 않습니다.\n\n");
            gaps.insert(format!(
                "발견 {}: 구체적 수정 방법·상세 보고서 미기록",
                finding.id
            ));
        } else {
            if data["evidence_version"] != data["report_evidence_version"] {
                out.push_str("\n**상세 보고서 작성 뒤 증거가 변경됐습니다. 원인·개선안을 최신 증거로 재검토해야 합니다.**\n\n");
                gaps.insert(format!(
                    "발견 {}: 상세 보고서와 현재 증거 버전 불일치",
                    finding.id
                ));
            }
            out.push_str("\n**원인·영향·개선 방향 (기존 상세 보고서 원문)**\n\n");
            quote(&mut out, &report);
        }
        if !finding.canonical {
            gaps.insert(format!(
                "발견 노드 {}: 정규 발견 기록 없음 · 처리 상태·상세 보고서 확인 필요",
                finding.id
            ));
        }
    }
    out.push_str("## 개선 우선순위\n\n우선순위는 저장된 위험도와 처리 상태를 기준으로 정렬한 분류입니다. 실제 영향·악용 가능성·사업 중요도를 새로 평가한 점수가 아닙니다. 구체적인 수정은 위 상세 보고서의 근거와 최신 증거를 확인한 뒤 수행하세요.\n\n| 우선순위 | 발견 | 저장 위험도 | 다음 조치 |\n| --- | --- | --- | --- |\n");
    let mut remediation = findings.iter().collect::<Vec<_>>();
    remediation.sort_by_key(|finding| match priority(finding.data).0 {
        "P0" => 0,
        "P1" => 1,
        "P2" => 2,
        "P3" => 3,
        "분류 필요" => 4,
        "재확인" => 5,
        _ => 6,
    });
    for finding in remediation {
        let (rank, action) = priority(finding.data);
        out.push_str(&format!(
            "| {} | {} ({}) | {} | {} |\n",
            rank,
            inline(title(finding.data)),
            inline(finding.id),
            inline(value(finding.data, "severity", "미분류")),
            action
        ));
    }
    if findings.is_empty() {
        out.push_str("| 검증 우선 | 발견 사항 없음 | 미판정 | 미검증 항목과 검사 범위를 먼저 확인하세요. |\n");
    }
    out.push_str("\n## 미검증·실패·차단 항목\n\n");
    if !complete {
        gaps.insert(format!(
            "작업 상태 {}: 전체 종료 보고서로 확정할 수 없음 (활성 실행 {}, 미종결 도구 {})",
            status, active_runs, pending_tools
        ));
    }
    if uncertain_runs > 0 {
        gaps.insert(format!(
            "이전 실행 {}개의 중단·복구 상태 확인 필요",
            uncertain_runs
        ));
    }
    if runtime_recovery {
        gaps.insert("작업 실행 시간 기록 복구 필요: 실행 상세를 확인해야 함".into());
    }
    if scopes.is_empty() && assets.is_empty() {
        gaps.insert(
            "구조화된 대상 범위·자산 연결 없음: 분석 목표 원문을 보고 실제 대상을 확인해야 함"
                .into(),
        );
    }
    if plans.is_empty() {
        gaps.insert("검사 항목별 목표·계획 기록 없음: 검사 범위 충족 여부를 산정할 수 없음".into());
    }
    if tools.is_empty()
        && !activity
            .iter()
            .any(|r| matches!(text(&r.data, "kind"), "tool_use" | "tool_result"))
    {
        gaps.insert("저장된 도구 실행 기록 없음: 실제 외부 검사를 수행했는지 확인 필요".into());
    }
    for plan in &plans {
        if (plan.data["kind"] == "goal" && plan.data["state"] != "met")
            || (plan.data["kind"] == "intent" && plan.data["state"] != "done")
        {
            gaps.insert(format!(
                "검사 항목 {}: 상태 {} · 미완료/차단/중지 기록과 연결 실행 확인 필요",
                plan.header.id,
                text(&plan.data, "state")
            ));
        }
    }
    for run in &runs {
        if text(&run.data, "state") != "completed" {
            gaps.insert(format!(
                "실행 {}: 상태 {} · 완료된 검사로 인정하지 않음",
                run.header.id,
                text(&run.data, "state")
            ));
        }
        for todo in run.data["todos"]
            .as_array()
            .into_iter()
            .flatten()
            .filter(|t| t["status"] != "completed")
        {
            gaps.insert(format!(
                "실행 {}의 미완료 할 일 ({}): {}",
                run.header.id,
                text(todo, "status"),
                text(todo, "content")
            ));
        }
    }
    for tool in &tools {
        if text(&tool.data, "state") != "completed" {
            gaps.insert(format!(
                "도구 {} / {}: {} · 실제 실패 사유·원문은 실행 상세 참조",
                tool.header.id,
                text(&tool.data, "name"),
                text(&tool.data, "state")
            ));
        }
        if tool.data["result_ref"].is_null() {
            gaps.insert(format!("도구 {}: 저장된 결과 본문 없음", tool.header.id));
        }
        if tool_errors.get(tool.header.id.as_str()) == Some(&true) {
            gaps.insert(format!(
                "도구 {} / {}: 저장 결과가 오류로 기록됨 · 원문은 실행 상세 참조",
                tool.header.id,
                text(&tool.data, "name")
            ));
        }
    }
    for row in activity.iter().filter(|r| r.data["is_error"] == true) {
        gaps.insert(format!(
            "오류 활동 {}: {}",
            row.header.id,
            text(&row.data, "summary")
        ));
    }
    for question in s
        .records("user_questions")
        .into_iter()
        .filter(|q| run_ids.contains(text(&q.data, "run_id")) && q.data["state"] == "pending")
    {
        gaps.insert(format!(
            "사용자 승인·질문 {}: 답변 대기",
            question.header.id
        ));
    }
    for gap in &gaps {
        out.push_str(&format!("- {}\n", inline(gap)));
    }
    out.push_str("\n저장 기록이 없는 검사 종류, 인증 수준·역할별 경로, 환경·사업 영향, 수정 후 재점검은 이 보고서만으로 확인할 수 없습니다. 미검증을 통과로 취급하지 않습니다.\n\n## 최종 판단과 후속 조치\n\n");
    let mut severity_counts = BTreeMap::<String, usize>::new();
    for finding in &findings {
        *severity_counts
            .entry(value(finding.data, "severity", "미분류").into())
            .or_default() += 1;
    }
    out.push_str(&format!("- 저장된 발견 사항: {}건 (확인·의심·오탐 등 처리 상태 포함)\n- 기록된 위험도별 건수: {}\n- 명시적으로 식별한 미완료·확인 필요 기록: {}건\n",findings.len(),inline(&severity_counts.into_iter().map(|(k,n)|format!("{k} {n}")).collect::<Vec<_>>().join(", ")),gaps.len()));
    out.push_str("- 우선순위에 따라 재현 근거와 실제 영향을 확인하고, 기록된 개선안을 검토한 뒤 수정하세요.\n- 수정 뒤 동일 조건으로 재점검하고 결과를 저장하세요. 수정·해결 표시만으로 재점검 통과를 추정하지 않습니다.\n- 이 보고서는 선택 작업의 저장 기록에 한정됩니다. 발견 건수 0, 도구 성공, 작업 완료는 전체 사이트의 안전성 보장이 아닙니다.\n");
    Ok(out)
}
