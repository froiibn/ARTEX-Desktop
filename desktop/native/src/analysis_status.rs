//! Saved analysis state, deliberately distinct from a command receipt or one
//! worker finishing. This projection neither advances work nor inspects hosts.
use artex_core::{Record, Snapshot};
use eframe::egui;
use std::collections::{BTreeMap, BTreeSet};

const SUMMARY_PREVIEW_CHARS: usize = 240;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum AnalysisState {
    Queued,
    Running,
    Approval,
    Waiting,
    Paused,
    Completed,
    Failed,
    TimedOut,
    Recovery,
    Unknown,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct Counts {
    pub active_runs: usize,
    pub queued_runs: usize,
    pub waiting_runs: usize,
    pub completed_runs: usize,
    pub failed_runs: usize,
    pub running_tools: usize,
    pub approval_requests: usize,
    pub user_questions: usize,
    pub completed_tools: usize,
    pub failed_tools: usize,
    pub findings: usize,
    pub facts: usize,
    pub saved_results: usize,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct SavedResult {
    pub record_id: String,
    pub summary: String,
    pub saved_at: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct AnalysisStatus {
    pub state: AnalysisState,
    pub headline: &'static str,
    pub detail: String,
    pub counts: Counts,
    pub last_progress_at: Option<String>,
    pub latest_result: Option<SavedResult>,
    pub failures: Vec<(artex_core::execution_status::FailureKind, usize)>,
    pub timeout_notes: Vec<String>,
}

fn text<'a>(record: &'a Record, key: &str) -> &'a str {
    record.data[key].as_str().unwrap_or("")
}
fn active(state: &str) -> bool {
    matches!(state, "queued" | "running" | "waiting")
}
fn task_runs<'a>(snapshot: &'a Snapshot, task: &Record) -> Vec<&'a Record> {
    snapshot
        .records("agent_runs")
        .into_iter()
        .filter(|run| run.data["task_id"] == task.header.id)
        .collect()
}

/// Project only persisted evidence. Task completion is authoritative; no-run,
/// a worker's terminal state, or a successful tool never implies task success.
/// Epoch filtering prevents an old queued run from looking newly executing.
pub(crate) fn project(snapshot: &Snapshot, task: &Record, epoch: &str) -> AnalysisStatus {
    let runs = task_runs(snapshot, task);
    let run_ids: BTreeSet<&str> = runs.iter().map(|run| run.header.id.as_str()).collect();
    let current: Vec<_> = runs
        .iter()
        .copied()
        .filter(|run| text(run, "engine_epoch") == epoch && active(text(run, "state")))
        .collect();
    let active_ids: BTreeSet<&str> = current.iter().map(|run| run.header.id.as_str()).collect();
    let mut counts = Counts {
        active_runs: current.len(),
        queued_runs: current
            .iter()
            .filter(|run| text(run, "state") == "queued")
            .count(),
        waiting_runs: current
            .iter()
            .filter(|run| text(run, "state") == "waiting")
            .count(),
        completed_runs: runs
            .iter()
            .filter(|run| text(run, "state") == "completed")
            .count(),
        failed_runs: runs
            .iter()
            .filter(|run| text(run, "state") == "failed")
            .count(),
        ..Counts::default()
    };
    for question in snapshot.records("user_questions") {
        if text(question, "state") != "pending" || !active_ids.contains(text(question, "run_id")) {
            continue;
        }
        if text(question, "kind") == "plan_approval"
            || question.data["payload"]["type"] == "tool_approval"
        {
            counts.approval_requests += 1;
        } else {
            counts.user_questions += 1;
        }
    }
    counts.approval_requests += snapshot
        .records("intercept_pending")
        .into_iter()
        .filter(|pending| {
            text(pending, "status") == "pending"
                && pending.data["audit"]["run_id"]
                    .as_str()
                    .is_some_and(|id| active_ids.contains(id))
        })
        .count();
    let tools: Vec<_> = snapshot
        .records("tool_invocations")
        .into_iter()
        .filter(|tool| run_ids.contains(text(tool, "run_id")))
        .collect();
    for tool in &tools {
        match text(tool, "state") {
            "completed" => counts.completed_tools += 1,
            "failed" => counts.failed_tools += 1,
            "running" if active_ids.contains(text(tool, "run_id")) => counts.running_tools += 1,
            _ => {}
        }
    }
    counts.findings = snapshot
        .records("findings")
        .into_iter()
        .filter(|finding| finding.data["task_id"] == task.header.id)
        .count();
    let exploration = text(task, "exploration_id");
    counts.facts = snapshot
        .records("exploration_nodes")
        .into_iter()
        .filter(|node| {
            !exploration.is_empty()
                && text(node, "exploration_id") == exploration
                && text(node, "kind") == "fact"
        })
        .count();
    // Task activity is explicitly scoped by exploration. Never read a Blob or
    // borrow an unrelated conversation's result just to manufacture a summary.
    let activity: Vec<_> = snapshot
        .records("activity")
        .into_iter()
        .filter(|row| !exploration.is_empty() && text(row, "exploration_id") == exploration)
        .collect();
    let results: Vec<_> = activity
        .iter()
        .copied()
        .filter(|row| text(row, "kind") == "result")
        .collect();
    counts.saved_results = results.len();
    let latest_result = results
        .into_iter()
        .max_by(|a, b| {
            a.header
                .created_at
                .cmp(&b.header.created_at)
                .then(a.header.id.cmp(&b.header.id))
        })
        .map(|row| SavedResult {
            record_id: row.header.id.clone(),
            summary: text(row, "summary")
                .chars()
                .take(SUMMARY_PREVIEW_CHARS)
                .collect(),
            saved_at: row.header.created_at.clone(),
        });
    let last_progress_at = activity
        .iter()
        .map(|row| row.header.updated_at.clone())
        .chain(tools.iter().map(|tool| tool.header.updated_at.clone()))
        .max();
    let recovery = snapshot.records("task_runtime").into_iter().any(|runtime| {
        runtime.data["task_id"] == task.header.id
            && runtime.data["accounting_state"] == "recovery_required"
    }) || runs
        .iter()
        .any(|run| matches!(text(run, "state"), "interrupted" | "recovery_required"))
        || runs
            .iter()
            .any(|run| text(run, "engine_epoch") != epoch && active(text(run, "state")));
    let status = text(task, "status");
    let (state, mut headline, mut detail) = match status {
        "done" => (AnalysisState::Completed, "분석 완료", "작업의 저장 상태가 완료입니다. 발견 사항이나 개별 도구 결과의 성공 여부는 별도로 확인하세요.".into()),
        "failed" => (AnalysisState::Failed, "분석 실패", "작업의 저장 상태가 실패입니다. 저장된 실행 결과와 오류를 확인하세요.".into()),
        "timeout" => (AnalysisState::TimedOut, "분석 시간 종료", "작업의 실행 시간 한도에 도달했습니다. 취약점이 없다는 뜻이 아닙니다.".into()),
        _ if recovery => (AnalysisState::Recovery, "실행 상태 확인 필요", "이전 실행 또는 활성 시간 기록의 복구가 필요합니다. 작업 공간을 열었다는 이유로 재개하지 않습니다.".into()),
        "paused" | "stopped" | "cancelled" => (AnalysisState::Paused, "분석 일시정지·중지 상태", "새 분석 실행은 중단된 저장 상태입니다. 외부 자원 정리 여부는 실행 상세에서 별도로 확인하세요.".into()),
        _ if task.data["paused"] == true => (AnalysisState::Paused, "분석 일시정지 상태", "일시정지 설정이 저장됐습니다. 남은 실행과 외부 자원의 종료를 뜻하지는 않습니다.".into()),
        _ if task.data["queued"] == true || status == "queued" => (AnalysisState::Queued, "분석 실행 대기", "작업이 실행 순서를 기다리고 있습니다.".into()),
        "running" if counts.approval_requests > 0 => (AnalysisState::Approval, "사용자 승인 대기", format!("승인 요청 {}건이 남아 있습니다. 승인 화면에서 허용 또는 거부를 선택하세요.", counts.approval_requests)),
        "running" if counts.user_questions > 0 => (AnalysisState::Waiting, "사용자 답변 대기", format!("답변할 질문 {}건이 남아 있습니다.", counts.user_questions)),
        "running" if counts.running_tools > 0 => (AnalysisState::Running, "분석 도구 실행 중", format!("도구 {}개가 실행 중으로 저장돼 있습니다.", counts.running_tools)),
        "running" if counts.waiting_runs > 0 => (AnalysisState::Waiting, "에이전트 응답 대기", "대기 중인 실행이 있습니다. 대기 사유의 세부 단계는 현재 저장 기록만으로 확인되지 않습니다.".into()),
        "running" if counts.active_runs > counts.queued_runs => (AnalysisState::Running, "분석 실행 중", "에이전트가 실행 중입니다. 모델 응답 대기 등 세부 단계는 현재 저장 기록만으로 확인되지 않습니다.".into()),
        "running" => (AnalysisState::Queued, "다음 분석 실행 대기", "작업은 진행 상태이며 계획 또는 실행 배정을 기다립니다. 활성 실행이 없다는 이유로 완료로 판단하지 않습니다.".into()),
        "created" | "pending" => (AnalysisState::Queued, "분석 시작 대기", "작업은 저장됐으며 분석 완료는 아직 기록되지 않았습니다.".into()),
        _ => (AnalysisState::Unknown, "분석 상태 미확인", "현재 저장 상태를 알려진 실행 단계로 판정할 수 없습니다.".into()),
    };
    if matches!(
        state,
        AnalysisState::Completed
            | AnalysisState::Failed
            | AnalysisState::TimedOut
            | AnalysisState::Paused
    ) && counts.active_runs > 0
    {
        headline = match state {
            AnalysisState::Completed => "분석 완료 · 마무리 진행",
            AnalysisState::Failed => "분석 실패 · 마무리 진행",
            AnalysisState::TimedOut => "분석 시간 종료 · 마무리 진행",
            _ => "분석 일시정지·중지 · 실행 남음",
        };
        detail.push_str(&format!(" 관련 실행 {}개는 아직 대기·실행 상태입니다. 종료 요약이나 정리 진행을 실행 상세에서 확인하세요.", counts.active_runs));
    }
    AnalysisStatus {
        state,
        headline,
        detail,
        counts,
        last_progress_at,
        latest_result,
        failures: artex_core::execution_status::failure_groups(snapshot, task),
        timeout_notes: artex_core::execution_status::timeout_notes(snapshot, task),
    }
}

fn latest_conversation_root<'a>(
    snapshot: &'a Snapshot,
    conversation: &Record,
) -> Option<&'a Record> {
    snapshot
        .records("agent_runs")
        .into_iter()
        .filter(|run| {
            run.data["conversation_id"] == conversation.header.id
                && text(run, "parent_run_id").is_empty()
        })
        .max_by(|a, b| {
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
        })
}

fn conversation_children<'a>(snapshot: &'a Snapshot, root: &Record) -> Vec<&'a Record> {
    let candidates = snapshot.records("agent_runs");
    let mut ids = BTreeSet::from([root.header.id.as_str()]);
    loop {
        let before = ids.len();
        for run in &candidates {
            if run.data["conversation_id"] == root.data["conversation_id"]
                && ids.contains(text(run, "parent_run_id"))
            {
                ids.insert(run.header.id.as_str());
            }
        }
        if before == ids.len() {
            break;
        }
    }
    candidates
        .into_iter()
        .filter(|run| ids.contains(run.header.id.as_str()) && run.header.id != root.header.id)
        .collect()
}

/// A conversation completes a response, not an entire service assessment. Only
/// the latest root response owns this label; historical failures cannot mask a
/// newer success, and an active wrap-up child is not an independently new reply.
pub(crate) fn conversation_status(
    snapshot: &Snapshot,
    conversation: &Record,
    epoch: &str,
) -> (&'static str, String) {
    let Some(root) = latest_conversation_root(snapshot, conversation) else {
        return (
            "응답 대기",
            "메시지를 보내면 응답 실행 상태가 여기에 표시됩니다.".into(),
        );
    };
    let children = conversation_children(snapshot, root);
    let current_children = children
        .iter()
        .filter(|run| text(run, "engine_epoch") == epoch && active(text(run, "state")))
        .count();
    let state = text(root, "state");
    if matches!(state, "completed" | "failed" | "cancelled" | "stopped") && current_children > 0 {
        return (
            "응답 마무리 진행 중",
            format!(
                "최근 응답은 {} 상태로 저장됐으며 관련 실행 {current_children}개가 아직 대기·실행 상태입니다. 종료 요약과 정리를 확인하세요.",
                match state {
                    "completed" => "완료",
                    "failed" => "실패",
                    _ => "중지",
                }
            ),
        );
    }
    if matches!(state, "interrupted" | "recovery_required")
        || (active(state) && text(root, "engine_epoch") != epoch)
        || children
            .iter()
            .any(|run| active(text(run, "state")) && text(run, "engine_epoch") != epoch)
    {
        return (
            "이전 응답 실행 확인 필요",
            "이전 실행의 상태나 복구를 확인해야 합니다. 대화를 열기만 해서는 재개되지 않습니다."
                .into(),
        );
    }
    let relevant: BTreeSet<&str> = std::iter::once(root)
        .chain(children.iter().copied())
        .filter(|run| text(run, "engine_epoch") == epoch && active(text(run, "state")))
        .map(|run| run.header.id.as_str())
        .collect();
    let questions: Vec<_> = snapshot
        .records("user_questions")
        .into_iter()
        .filter(|question| {
            text(question, "state") == "pending" && relevant.contains(text(question, "run_id"))
        })
        .collect();
    if questions.iter().any(|question| {
        text(question, "kind") == "plan_approval"
            || question.data["payload"]["type"] == "tool_approval"
    }) || snapshot
        .records("intercept_pending")
        .into_iter()
        .any(|pending| {
            text(pending, "status") == "pending"
                && pending.data["audit"]["run_id"]
                    .as_str()
                    .is_some_and(|id| relevant.contains(id))
        })
    {
        return (
            "사용자 승인 대기",
            "최근 응답 실행에 승인 요청이 있습니다. 승인 또는 거부를 선택해야 진행할 수 있습니다."
                .into(),
        );
    }
    if !questions.is_empty() {
        return (
            "사용자 답변 대기",
            "최근 응답 실행에서 질문을 기다리고 있습니다. 답변을 제출하면 이어집니다.".into(),
        );
    }
    match state {
        "queued" => ("응답 실행 대기", "메시지는 저장됐으며 실행 배정을 기다리고 있습니다.".into()),
        "running" => {
            let tools = snapshot.records("tool_invocations").into_iter().filter(|tool| relevant.contains(text(tool,"run_id")) && text(tool,"state")=="running").count();
            ("응답 실행 중", if tools>0 {format!("도구 {tools}개가 실행 중으로 저장돼 있습니다.")} else {"에이전트가 실행 중입니다. 모델 응답 대기 등 세부 단계는 현재 저장 기록만으로 확인되지 않습니다.".into()})
        },
        "waiting" => ("응답 진행 대기", "최근 실행이 대기 상태입니다. 저장된 대기 사유와 실행 상세를 확인하세요.".into()),
        "completed" => ("응답 완료", "최근 응답 실행이 완료됐습니다. 전체 사이트 분석 완료나 취약점 없음의 판정과는 다릅니다. 저장된 답변과 도구 결과를 확인하세요.".into()),
        "failed" => ("응답 실패", "최근 응답 실행이 실패했습니다. 저장된 답변·오류와 실행 상세를 확인하세요.".into()),
        "cancelled" | "stopped" | "paused" => ("응답 중지·일시정지", "최근 응답 실행이 중단된 상태입니다. 외부 자원 정리 여부는 실행 상세에서 별도로 확인하세요.".into()),
        _ => ("응답 상태 미확인", "최근 응답의 저장 상태를 알려진 실행 단계로 판정할 수 없습니다.".into()),
    }
}

/// Render saved evidence without making a model call or interpreting an empty
/// findings list as a clean security assessment. Caller owns navigation actions.
pub(crate) fn show_card(ui: &mut egui::Ui, status: &AnalysisStatus) {
    use crate::theme::{BadgeTone, badge, surface_frame};
    let tone = match status.state {
        AnalysisState::Completed => BadgeTone::Success,
        AnalysisState::Failed => BadgeTone::Error,
        AnalysisState::Approval
        | AnalysisState::Waiting
        | AnalysisState::Paused
        | AnalysisState::TimedOut
        | AnalysisState::Recovery => BadgeTone::Warning,
        AnalysisState::Running => BadgeTone::Info,
        _ => BadgeTone::Neutral,
    };
    surface_frame(ui.style()).show(ui, |ui| {
        ui.horizontal_wrapped(|ui| {
            ui.strong("분석 상태");
            badge(ui, status.headline, tone);
        });
        ui.add(egui::Label::new(&status.detail).wrap());
        let c = &status.counts;
        ui.horizontal_wrapped(|ui| {
            ui.small(format!(
                "활성 실행 {} · 실행 대기 {} · 응답 대기 {}",
                c.active_runs, c.queued_runs, c.waiting_runs
            ));
            ui.small(format!(
                "저장된 실행 완료 {} · 실행 실패 {}",
                c.completed_runs, c.failed_runs
            ));
            ui.small(format!(
                "저장된 도구 완료 {} · 도구 실패 {}",
                c.completed_tools, c.failed_tools
            ));
        });
        ui.horizontal_wrapped(|ui| {
            ui.small(format!(
                "저장된 발견 사항 {} · 사실 {} · 실행 결과 {}",
                c.findings, c.facts, c.saved_results
            ));
            if let Some(stamp) = &status.last_progress_at {
                ui.small(format!("최근 기록 {stamp}"));
            }
        });
        if let Some(result) = &status.latest_result {
            ui.small(format!("최근 저장 결과 · {}", result.saved_at));
            ui.add(
                egui::Label::new(if result.summary.is_empty() {
                    "요약 미기록 · 실행 기록에서 본문을 확인하세요."
                } else {
                    &result.summary
                })
                .wrap(),
            );
        } else {
            ui.small("저장된 실행 결과가 아직 없습니다.");
        }
        for note in &status.timeout_notes {
            ui.add(egui::Label::new(note).wrap());
        }
        if !status.failures.is_empty() {
            ui.strong("저장된 실행 실패 원인");
            for (kind, count) in &status.failures {
                ui.add(egui::Label::new(format!("{} · {count}건", kind.label())).wrap());
            }
        }
        ui.small("활성 실행 0은 외부 자원 정리 완료를 뜻하지 않습니다. 도구별 오류 원문과 정리 확인은 실행 상세에서 확인하세요.");
    });
}

/// Observe local state transitions, not historical completion on workspace open.
/// A scope/epoch switch resets the baseline; no notification sends are involved.
#[derive(Default)]
pub(crate) struct StatusTracker {
    scope: Option<(String, String)>,
    seen: BTreeMap<String, String>,
    conversations: BTreeMap<String, (String, String)>,
    last_seq: Option<u64>,
}
impl StatusTracker {
    pub(crate) fn clear(&mut self) {
        self.scope = None;
        self.seen.clear();
        self.conversations.clear();
        self.last_seq = None;
    }

    pub(crate) fn observe(
        &mut self,
        workspace: &str,
        epoch: &str,
        snapshot: &Snapshot,
    ) -> Vec<String> {
        let scope = (workspace.to_owned(), epoch.to_owned());
        let same_scope = self.scope.as_ref() == Some(&scope);
        // The immutable generation only changes at a durable commit. Repainting
        // a frame must not repeatedly scan large task/conversation histories.
        if same_scope && self.last_seq == Some(snapshot.commit_seq) {
            return vec![];
        }
        let mut next = BTreeMap::new();
        let mut notices = vec![];
        for task in snapshot.records("tasks") {
            if !task.data["deleted_at"].is_null() || !task.data["archived_at"].is_null() {
                continue;
            }
            let status = text(task, "status");
            if same_scope
                && self.seen.get(&task.header.id).is_some_and(|before| {
                    matches!(
                        before.as_str(),
                        "running" | "queued" | "created" | "pending"
                    ) && before != status
                })
            {
                let label = match status {
                    "done" => Some("분석 완료 상태가 저장됐습니다"),
                    "failed" => Some("분석 실패 상태가 저장됐습니다"),
                    "timeout" => Some("분석 실행 시간 종료가 기록됐습니다"),
                    "paused" => Some(
                        "분석 일시정지·중지 상태가 저장됐습니다. 외부 자원 정리는 별도 확인이 필요합니다",
                    ),
                    _ => None,
                };
                if let Some(label) = label {
                    let name = task.data["name"]
                        .as_str()
                        .filter(|name| !name.is_empty())
                        .unwrap_or(&task.header.id);
                    notices.push(format!("{name}: {label}"));
                }
            }
            next.insert(task.header.id.clone(), status.to_owned());
        }
        let mut conversations = BTreeMap::new();
        for conversation in snapshot.records("conversations") {
            if !conversation.data["trashed_at"].is_null() {
                continue;
            }
            let Some(root) = latest_conversation_root(snapshot, conversation) else {
                conversations.insert(
                    conversation.header.id.clone(),
                    (String::new(), "idle".into()),
                );
                continue;
            };
            let state = text(root, "state");
            let finishing = matches!(state, "completed" | "failed" | "cancelled" | "stopped")
                && conversation_children(snapshot, root)
                    .iter()
                    .any(|run| text(run, "engine_epoch") == epoch && active(text(run, "state")));
            let phase = if finishing { "finishing" } else { state };
            let changed = self
                .conversations
                .get(&conversation.header.id)
                .is_some_and(|previous| previous != &(root.header.id.clone(), phase.into()));
            if same_scope && changed && text(root, "engine_epoch") == epoch && !finishing {
                let label = match state {
                    "completed" => Some("응답 완료"),
                    "failed" => Some("응답 실패"),
                    "cancelled" | "stopped" => Some("응답 중지"),
                    _ => None,
                };
                if let Some(label) = label {
                    let name = conversation.data["title"]
                        .as_str()
                        .filter(|name| !name.is_empty())
                        .unwrap_or(&conversation.header.id);
                    notices.push(format!(
                        "{name}: {label}. 전체 분석 완료나 외부 자원 정리의 확인과는 별개입니다."
                    ));
                }
            }
            conversations.insert(
                conversation.header.id.clone(),
                (root.header.id.clone(), phase.into()),
            );
        }
        self.scope = Some(scope);
        self.seen = next;
        self.conversations = conversations;
        self.last_seq = Some(snapshot.commit_seq);
        notices
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use artex_core::Header;
    use serde_json::{Value, json};

    fn record(collection: &str, id: &str, data: Value) -> Record {
        Record {
            header: Header {
                id: id.into(),
                collection: collection.into(),
                version: "1".into(),
                commit_seq: "1".into(),
                created_at: "2026-10-20T00:00:00Z".into(),
                updated_at: "2026-10-20T00:00:00Z".into(),
                legacy_key: Value::Null,
            },
            data,
        }
    }
    fn fixture(status: &str) -> (Snapshot, Record) {
        let task = record(
            "tasks",
            "task-own",
            json!({"name":"내 분석","status":status,"exploration_id":"exp-own"}),
        );
        let mut s = Snapshot {
            workspace_id: "workspace-own".into(),
            commit_seq: 1,
            collections: BTreeMap::new(),
        };
        add(&mut s, task.clone());
        (s, task)
    }
    fn add(s: &mut Snapshot, r: Record) {
        s.collections
            .entry(r.header.collection.clone())
            .or_default()
            .insert(r.header.id.clone(), r);
    }
    fn run(s: &mut Snapshot, id: &str, state: &str, epoch: &str) {
        add(
            s,
            record(
                "agent_runs",
                id,
                json!({"task_id":"task-own","state":state,"engine_epoch":epoch}),
            ),
        );
    }
    #[test]
    fn idle_or_finished_worker_never_completes_a_running_task() {
        let (mut s, task) = fixture("running");
        assert_eq!(project(&s, &task, "epoch").state, AnalysisState::Queued);
        run(&mut s, "run1", "completed", "epoch");
        assert_eq!(project(&s, &task, "epoch").state, AnalysisState::Queued);
        add(
            &mut s,
            record(
                "tool_invocations",
                "tool1",
                json!({"run_id":"run1","state":"failed"}),
            ),
        );
        let p = project(&s, &task, "epoch");
        assert_eq!(p.state, AnalysisState::Queued);
        assert_eq!(p.counts.failed_tools, 1);
    }
    #[test]
    fn task_terminal_state_is_authoritative_but_active_wrapup_is_disclosed() {
        for (status, expected) in [
            ("done", AnalysisState::Completed),
            ("failed", AnalysisState::Failed),
            ("timeout", AnalysisState::TimedOut),
        ] {
            let (mut s, task) = fixture(status);
            run(&mut s, "wrapup", "running", "epoch");
            let p = project(&s, &task, "epoch");
            assert_eq!(p.state, expected);
            assert!(p.detail.contains("관련 실행 1개"));
            assert_eq!(p.counts.findings, 0);
        }
    }
    #[test]
    fn pending_approval_question_and_tool_have_distinct_evidence() {
        let (mut s, task) = fixture("running");
        run(&mut s, "run1", "waiting", "epoch");
        add(
            &mut s,
            record(
                "user_questions",
                "q1",
                json!({"run_id":"run1","kind":"question","state":"pending","payload":{"type":"tool_approval"}}),
            ),
        );
        assert_eq!(project(&s, &task, "epoch").state, AnalysisState::Approval);
        s.collections
            .get_mut("user_questions")
            .unwrap()
            .get_mut("q1")
            .unwrap()
            .data["payload"] = json!({});
        assert_eq!(project(&s, &task, "epoch").state, AnalysisState::Waiting);
        s.collections.remove("user_questions");
        s.collections
            .get_mut("agent_runs")
            .unwrap()
            .get_mut("run1")
            .unwrap()
            .data["state"] = json!("running");
        add(
            &mut s,
            record(
                "tool_invocations",
                "tool1",
                json!({"run_id":"run1","state":"running"}),
            ),
        );
        assert_eq!(project(&s, &task, "epoch").headline, "분석 도구 실행 중");
    }
    #[test]
    fn old_epoch_and_recovery_do_not_look_like_fresh_execution() {
        let (mut s, task) = fixture("running");
        run(&mut s, "old", "running", "prior");
        let p = project(&s, &task, "current");
        assert_eq!(p.state, AnalysisState::Recovery);
        assert_eq!(p.counts.active_runs, 0);
        s.collections.remove("agent_runs");
        add(
            &mut s,
            record(
                "task_runtime",
                "rt",
                json!({"task_id":"task-own","accounting_state":"recovery_required"}),
            ),
        );
        assert_eq!(project(&s, &task, "current").state, AnalysisState::Recovery);
    }
    #[test]
    fn saved_summary_and_counts_exclude_other_tasks_and_do_not_read_blob_text() {
        let (mut s, task) = fixture("running");
        add(
            &mut s,
            record(
                "activity",
                "ours",
                json!({"exploration_id":"exp-own","kind":"result","summary":"확인된 일부 결과","detail":{"blob":{"sha256":"opaque"}}}),
            ),
        );
        add(
            &mut s,
            record(
                "activity",
                "foreign",
                json!({"exploration_id":"exp-other","kind":"result","summary":"다른 작업"}),
            ),
        );
        add(
            &mut s,
            record("findings", "foreign", json!({"task_id":"task-other"})),
        );
        let p = project(&s, &task, "epoch");
        assert_eq!(p.counts.saved_results, 1);
        assert_eq!(p.counts.findings, 0);
        assert_eq!(p.latest_result.unwrap().summary, "확인된 일부 결과");
        assert_eq!(p.state, AnalysisState::Queued);
    }
    #[test]
    fn transition_notices_are_once_and_scope_reopen_is_silent() {
        let (mut s, mut task) = fixture("running");
        let mut tracker = StatusTracker::default();
        assert!(tracker.observe("w", "epoch", &s).is_empty());
        task.data["status"] = json!("done");
        add(&mut s, task.clone());
        s.commit_seq += 1;
        assert_eq!(tracker.observe("w", "epoch", &s).len(), 1);
        assert!(tracker.observe("w", "epoch", &s).is_empty());
        assert!(tracker.observe("other", "epoch", &s).is_empty());
        assert!(tracker.observe("other", "new-epoch", &s).is_empty());
        tracker.clear();
        assert!(tracker.observe("w", "epoch", &s).is_empty());
    }
    fn conversation_fixture() -> (Snapshot, Record) {
        let (mut s, _) = fixture("running");
        let conversation = record("conversations", "cid", json!({"title":"사이트 점검 대화"}));
        add(&mut s, conversation.clone());
        (s, conversation)
    }
    fn conversation_run(
        s: &mut Snapshot,
        id: &str,
        state: &str,
        parent: Option<&str>,
        seq: u64,
        epoch: &str,
    ) {
        let mut row = record(
            "agent_runs",
            id,
            json!({"conversation_id":"cid","state":state,"parent_run_id":parent,"engine_epoch":epoch}),
        );
        row.header.commit_seq = seq.to_string();
        add(s, row);
        s.commit_seq += 1;
    }
    #[test]
    fn latest_conversation_root_supersedes_old_failures_without_assessment_success_claim() {
        let (mut s, c) = conversation_fixture();
        conversation_run(&mut s, "old", "failed", None, 1, "epoch");
        conversation_run(&mut s, "latest", "completed", None, 2, "epoch");
        let (title, detail) = conversation_status(&s, &c, "epoch");
        assert_eq!(title, "응답 완료");
        assert!(detail.contains("전체 사이트 분석 완료나 취약점 없음의 판정과는 다릅니다"));
        conversation_run(&mut s, "child", "completed", Some("old"), 3, "epoch");
        assert_eq!(conversation_status(&s, &c, "epoch").0, "응답 완료");
    }
    #[test]
    fn conversation_wait_reasons_and_old_epoch_are_explicit() {
        let (mut s, c) = conversation_fixture();
        conversation_run(&mut s, "latest", "queued", None, 1, "epoch");
        assert_eq!(conversation_status(&s, &c, "epoch").0, "응답 실행 대기");
        s.collections
            .get_mut("agent_runs")
            .unwrap()
            .get_mut("latest")
            .unwrap()
            .data["state"] = json!("waiting");
        add(
            &mut s,
            record(
                "user_questions",
                "q",
                json!({"run_id":"latest","state":"pending","kind":"plan_approval"}),
            ),
        );
        assert_eq!(conversation_status(&s, &c, "epoch").0, "사용자 승인 대기");
        s.collections
            .get_mut("user_questions")
            .unwrap()
            .get_mut("q")
            .unwrap()
            .data["kind"] = json!("question");
        assert_eq!(conversation_status(&s, &c, "epoch").0, "사용자 답변 대기");
        assert_eq!(
            conversation_status(&s, &c, "new-epoch").0,
            "이전 응답 실행 확인 필요"
        );
    }
    #[test]
    fn active_wrapup_delays_conversation_terminal_notice_until_child_settles() {
        let (mut s, c) = conversation_fixture();
        conversation_run(&mut s, "parent", "running", None, 1, "epoch");
        let mut tracker = StatusTracker::default();
        assert!(tracker.observe("w", "epoch", &s).is_empty());
        s.collections
            .get_mut("agent_runs")
            .unwrap()
            .get_mut("parent")
            .unwrap()
            .data["state"] = json!("failed");
        conversation_run(&mut s, "child", "running", Some("parent"), 2, "epoch");
        assert_eq!(
            conversation_status(&s, &c, "epoch").0,
            "응답 마무리 진행 중"
        );
        assert!(tracker.observe("w", "epoch", &s).is_empty());
        s.collections
            .get_mut("agent_runs")
            .unwrap()
            .get_mut("child")
            .unwrap()
            .data["state"] = json!("completed");
        s.commit_seq += 1;
        let messages = tracker.observe("w", "epoch", &s);
        assert_eq!(messages.len(), 1);
        assert!(messages[0].contains("응답 실패"));
        assert!(tracker.observe("w", "epoch", &s).is_empty());
        assert!(tracker.observe("w", "next-epoch", &s).is_empty());
    }
    #[test]
    fn tracker_same_generation_is_cached_and_initial_terminal_conversation_is_silent() {
        let (mut s, _) = conversation_fixture();
        conversation_run(&mut s, "parent", "running", None, 1, "epoch");
        let mut tracker = StatusTracker::default();
        assert!(tracker.observe("w", "epoch", &s).is_empty());
        // A fixture mutation with unchanged generation intentionally proves the
        // immutable-generation cache; actual store writes always advance seq.
        s.collections
            .get_mut("agent_runs")
            .unwrap()
            .get_mut("parent")
            .unwrap()
            .data["state"] = json!("completed");
        assert!(tracker.observe("w", "epoch", &s).is_empty());
        s.commit_seq += 1;
        assert_eq!(tracker.observe("w", "epoch", &s).len(), 1);
        tracker.clear();
        assert!(tracker.observe("w", "epoch", &s).is_empty());
    }
    #[test]
    fn card_renders_saved_status_and_no_security_success_inference() {
        let (s, task) = fixture("done");
        let p = project(&s, &task, "epoch");
        let ctx = egui::Context::default();
        let out = ctx.run(egui::RawInput::default(), |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| show_card(ui, &p));
        });
        assert!(out.shapes.iter().any(|shape|matches!(&shape.shape,egui::Shape::Text(text) if text.galley.text().contains("분석 완료"))));
        assert!(p.detail.contains("별도로 확인"));
    }
}
