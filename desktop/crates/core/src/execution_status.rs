//! Task-local failure categories shared by native status and exported reports.
//! Never copy arbitrary error bodies: they can contain keys or another task's
//! model context. Only recognized categories become public summary text.
use crate::{Record, Snapshot};
use std::collections::BTreeMap;

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum FailureKind {
    StreamBackpressure, TaskTimeout, RunDeadline, TurnBudget,
    Provider, Permission, Recovery, ReferencedBody, Other,
}
impl FailureKind {
    pub fn label(self) -> &'static str {
        match self {
            Self::StreamBackpressure => "BUSY · 내부 스트리밍 기록 지연",
            Self::TaskTimeout => "작업 실행 시간 한도 도달",
            Self::RunDeadline => "개별 에이전트 실행 시간 한도 도달",
            Self::TurnBudget => "에이전트 응답 횟수 한도 도달",
            Self::Provider => "EXTERNAL_FAILURE · 모델 또는 외부 서비스 오류",
            Self::Permission => "PERMISSION_DENIED · 실행 권한 또는 경로 제한",
            Self::Recovery => "RECOVERY_REQUIRED · 실행·저장 복구 필요",
            Self::ReferencedBody => "오류 본문이 별도 저장됨 · 실행 상세 확인",
            Self::Other => "기타 실행 오류 · 실행 상세 확인",
        }
    }
}

fn classify(row: Option<&Record>) -> FailureKind {
    let Some(row) = row else { return FailureKind::Other; };
    let detail = &row.data["detail"];
    let Some(raw) = detail["text"].as_str() else {
        return if detail["blob"].is_object() { FailureKind::ReferencedBody } else { FailureKind::Other };
    };
    match raw {
        "BUSY: Stream writer backpressure exceeded 5 seconds" | "BUSY: Aggregate stream queue exceeds 16 MiB" => FailureKind::StreamBackpressure,
        "Task timeout reached" => FailureKind::TaskTimeout,
        "Deadline reached" | "Run deadline reached" => FailureKind::RunDeadline,
        "Run and wrap-up turn budgets exhausted" => FailureKind::TurnBudget,
        _ if raw.starts_with("EXTERNAL_FAILURE:") => FailureKind::Provider,
        _ if raw.starts_with("PERMISSION_DENIED:") => FailureKind::Permission,
        _ if raw.starts_with("RECOVERY_REQUIRED:") => FailureKind::Recovery,
        _ => FailureKind::Other,
    }
}

/// Count terminal failed runs once, using their latest task-owned error result.
/// Missing/Blob-only evidence remains explicit; it never becomes success.
pub fn failure_groups(s: &Snapshot, task: &Record) -> Vec<(FailureKind, usize)> {
    let mut groups = BTreeMap::new();
    let activity = s.records("activity");
    for run in s.records("agent_runs").into_iter().filter(|r| r.data["task_id"] == task.header.id && r.data["state"] == "failed") {
        let row = activity.iter().copied().filter(|r| {
            r.data["exploration_id"] == task.data["exploration_id"]
                && r.data["kind"] == "result" && r.data["is_error"] == true
                && r.data["metadata"]["run_id"] == run.header.id
        }).max_by_key(|r| (&r.header.created_at, r.header.commit_seq.parse::<u64>().unwrap_or(0), &r.header.id));
        *groups.entry(classify(row)).or_insert(0) += 1;
    }
    groups.into_iter().collect()
}

/// Explain saved timeout policy without changing zero/disabled settings or
/// equating an inactive run count with external cleanup confirmation.
pub fn timeout_notes(s: &Snapshot, task: &Record) -> Vec<String> {
    if task.data["status"] != "timeout" { return vec![]; }
    let mut notes = vec![];
    if let Some(seconds) = task.data["timeout_seconds"].as_u64().filter(|n| *n > 0) {
        notes.push(format!("설정된 작업 실행 시간 한도: {seconds}초"));
    }
    if let Some(runtime) = s.records("task_runtime").into_iter().find(|r| r.data["task_id"] == task.header.id) {
        if let Some(ms) = runtime.data["active_elapsed_ms"].as_str().and_then(|v| v.parse::<u64>().ok()) {
            notes.push(format!("저장된 누적 활성 시간: {}초", ms / 1000));
        }
    }
    for agent in s.records("agents").into_iter().filter(|a| {
        matches!(a.data["key"].as_str(), Some("planner" | "worker"))
            && a.data["task_timeout_wrapup_max_turns"].as_u64() == Some(0)
            && s.records("agent_runs").iter().any(|r| r.data["task_id"] == task.header.id && r.data["agent_id"] == a.header.id)
    }) {
        let role = if agent.data["key"] == "planner" { "계획" } else { "실행" };
        notes.push(format!("{role} 에이전트의 시간 초과 후 마무리 실행: 비활성(0회 설정)"));
    }
    notes
}
