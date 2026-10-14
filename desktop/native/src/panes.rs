use crate::model::{Section, display_name, expected, text};
use artex_core::{Expected, Record, Snapshot};
use artex_native_core::{
    ConversationAction, DeletionOptions, DeletionTarget, MonitorTarget, RunControlAction,
    RunControlTarget, TaskDeleteOptions,
};
use eframe::egui::{self, RichText};
use serde_json::{Value, json};
use std::collections::{BTreeMap, BTreeSet};

pub enum Effect {
    AgentMonitor {
        target: MonitorTarget,
        token: String,
    },
    DeletionPreview {
        target: DeletionTarget,
        options: DeletionOptions,
        token: String,
    },
    ConversationTrash {
        token: String,
    },
    RunControl {
        target: RunControlTarget,
        action: RunControlAction,
        expected: Vec<Expected>,
        token: String,
    },
    ConversationManage {
        ids: Vec<String>,
        action: ConversationAction,
        expected: Vec<Expected>,
        token: String,
    },
    TaskDelete {
        ids: Vec<String>,
        options: TaskDeleteOptions,
        expected: Vec<Expected>,
        token: String,
    },
    Command {
        method: String,
        params: Value,
        expected: Vec<Expected>,
    },
    Query {
        method: String,
        params: Value,
    },
    Select {
        section: Section,
        id: String,
    },
    Download {
        name: String,
        bytes: Vec<u8>,
    },
    FileRead {
        path: String,
    },
    FileWrite {
        path: String,
        content: String,
        expected_sha256: Option<String>,
        create: bool,
    },
    StageFile {
        purpose: String,
    },
    SaveStagedFile {
        token: String,
        name: String,
    },
    ReleaseStagedFile {
        token: String,
    },
    ExportBase64Body {
        name: String,
        body: Value,
    },
}
fn command(effects: &mut Vec<Effect>, method: &str, params: Value, versions: Vec<Expected>) {
    effects.push(Effect::Command {
        method: method.into(),
        params,
        expected: versions,
    });
}
fn query(effects: &mut Vec<Effect>, method: &str, params: Value) {
    effects.push(Effect::Query {
        method: method.into(),
        params,
    });
}
fn approval_key(method: &str, params: &Value) -> Option<(String, String)> {
    let field = match method {
        "user.answer" => "question_id",
        "ui.interceptDecide" => "id",
        _ => return None,
    };
    params[field]
        .as_str()
        .filter(|id| !id.is_empty())
        .map(|id| (method.into(), id.into()))
}
fn body_text(value: &Value) -> String {
    if let Some(s) = value.as_str().or_else(|| value["text"].as_str()) {
        return s.into();
    }
    if value["blob"].is_object() {
        return "큰 본문은 상세 조회를 통해 불러옵니다.".into();
    }
    String::new()
}
fn reference_token(collection: &str, id: &str, label: &str) -> Option<String> {
    let (prefix, kind) = match collection {
        "findings" => ("finding", "취약점"),
        "companies" => ("company", "기업"),
        "assets" => ("asset", "자산"),
        _ => return None,
    };
    if !artex_core::valid_id(id, prefix) {
        return None;
    }
    let label = label
        .chars()
        .map(|c| match c {
            '[' => '(',
            ']' => ')',
            c if c.is_control() || c.is_whitespace() => ' ',
            c => c,
        })
        .collect::<String>();
    let label = label
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .chars()
        .take(100)
        .collect::<String>();
    let label = if label.is_empty() { id } else { label.as_str() };
    Some(format!("@[{kind}#{id} {label}]"))
}
fn reference_label(record: &Record) -> String {
    if record.header.collection == "assets" {
        for field in [
            "url",
            "domain",
            "ip",
            "app_name",
            "bundle_id",
            "service_name",
            "root_domain",
        ] {
            let label = text(&record.data, field);
            if !label.is_empty() {
                return label.chars().take(100).collect();
            }
        }
    }
    display_name(record)
}
pub fn multiline(ui: &mut egui::Ui, label: &str, value: &mut String, rows: usize) {
    crate::theme::group(ui, label, |ui| {
        ui.add(
            egui::TextEdit::multiline(value)
                .desired_rows(rows)
                .desired_width(f32::INFINITY),
        );
    });
}

pub struct Panes {
    pub projection: BTreeMap<String, Value>,
    pub title: String,
    pub description: String,
    pub goal: String,
    pub draft_message: String,
    pub agent_key: String,
    pub profile_id: Option<String>,
    pub selected_profiles: Vec<String>,
    pub selected_sources: Vec<String>,
    pub task_tab: usize,
    pub scope_kind: String,
    pub scope_value: String,
    pub scope_reason: String,
    pub company_name: String,
    pub finding_note: String,
    pub selected_status: String,
    pub selected_severity: String,
    pub file_path: String,
    pub file_text: String,
    pub file_dirty: bool,
    pub file_loaded: Option<String>,
    pub file_revision: Option<String>,
    pub file_locked: bool,
    pub new_directory: String,
    pub sync_search: String,
    read_only: bool,
    management: ManagementState,
    flow: FlowState,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ManagementAction {
    ClearHistory,
    Trash,
    Restore,
    Purge,
    TaskDelete,
    ForceStop,
    ArchiveRestore,
}
impl ManagementAction {
    fn label(self) -> &'static str {
        match self {
            Self::ClearHistory => "대화 내용 비우기",
            Self::Trash => "대화 삭제 · 휴지통으로 이동",
            Self::Restore => "대화 복구",
            Self::Purge => "휴지통에서 영구 삭제",
            Self::TaskDelete => "작업 삭제",
            Self::ForceStop => "소유 외부 프로세스 강제 종료",
            Self::ArchiveRestore => "보관 작업 복원",
        }
    }
    fn collection(self) -> &'static str {
        match self {
            Self::TaskDelete => "tasks",
            Self::ForceStop => "agent_runs",
            Self::ArchiveRestore => "task_archives",
            _ => "conversations",
        }
    }
}
#[derive(Default, Clone)]
struct DeleteSelection {
    assets: bool,
    findings: bool,
    traffic: bool,
    llm_records: bool,
    files: bool,
}
struct ManagementDialog {
    nonce: String,
    action: ManagementAction,
    ids: Vec<String>,
    names: Vec<String>,
    baseline: Vec<Expected>,
    versions: Vec<Expected>,
    preview: Option<Value>,
    preview_token: Option<String>,
    action_token: Option<String>,
    options: DeleteSelection,
    confirmed: bool,
    error: Option<String>,
    reload_required: bool,
    source_allowed: bool,
}
enum ManagementRequest {
    Preview(String),
    TaskStopPreview {
        task_id: String,
        baseline: Expected,
    },
    Mutation {
        nonce: Option<String>,
        ids: Vec<String>,
    },
    Monitor(String),
    Trash,
}
#[derive(Default)]
struct ManagementState {
    batches: BTreeMap<Section, BTreeSet<String>>,
    dialog: Option<ManagementDialog>,
    requests: BTreeMap<String, ManagementRequest>,
    monitor: Option<Value>,
    monitor_details: BTreeMap<String, Value>,
    monitor_detail_tokens: BTreeMap<String, String>,
    monitor_latest: Option<String>,
    monitor_last_poll: Option<std::time::Instant>,
    monitor_offset: usize,
    monitor_error: Option<String>,
    trash: Option<Value>,
    trash_token: Option<String>,
    trash_open: bool,
    feedback: BTreeMap<String, String>,
    archive_tab: usize,
}
struct BodyTarget {
    method: String,
    path: Vec<String>,
    parent_token: Option<String>,
}
struct StagedTarget {
    section: Option<Section>,
    selected: Option<String>,
    path: Option<String>,
    operation: String,
    params: Value,
}
struct ComposerMeasurement {
    scope: String,
    width: f32,
    compact: bool,
    dense: bool,
    header_height: f32,
    height: f32,
}
struct FlowState {
    serial: u64,
    latest: BTreeMap<String, String>,
    bodies: BTreeMap<String, BodyTarget>,
    versions: BTreeMap<String, Vec<Expected>>,
    history: BTreeMap<String, Value>,
    history_requests: BTreeMap<String, String>,
    history_latest: BTreeMap<String, String>,
    queued: Vec<Effect>,
    error: Option<String>,
    company_kind: String,
    traffic_host: String,
    traffic_method: String,
    traffic_query: String,
    traffic_body: String,
    traffic_path: String,
    traffic_status: String,
    traffic_min: String,
    traffic_max: String,
    traffic_sort: String,
    traffic_order: String,
    traffic_page: u64,
    traffic_size: u64,
    traffic_selected: String,
    traffic_delete_confirm: bool,
    session_task: String,
    session_selected_run: String,
    session_kind: String,
    session_tab: usize,
    session_message: String,
    session_reason: String,
    question_drafts: BTreeMap<String, Value>,
    question_versions: BTreeMap<String, Vec<Expected>>,
    sync_dimension: String,
    sync_page: u64,
    sync_size: u64,
    sync_targets: Vec<String>,
    sync_types: Vec<String>,
    sync_company: bool,
    sync_url: String,
    sync_read_versions: Option<Vec<Expected>>,
    export_format: String,
    context_section: Option<Section>,
    context_selected: Option<String>,
    stages: BTreeMap<String, StagedTarget>,
    append: BTreeMap<String, bool>,
    logs_tab: usize,
    logs_model: String,
    logs_session: String,
    logs_task: String,
    logs_page: u64,
    logs_size: u64,
    logs_confirm: bool,
    budget_override: String,
    attachments: BTreeMap<String, Vec<Value>>,
    pending_messages: BTreeMap<String, (String, Vec<Value>)>,
    pending_approvals: BTreeMap<(String, String), Option<String>>,
    approval_feedback: BTreeMap<(String, String), (bool, String)>,
    message_drafts: BTreeMap<String, String>,
    composer_measurement: Option<ComposerMeasurement>,
    evidence_binding: String,
    evidence_side: String,
    evidence_finding: String,
    skill_overwrite: bool,
    mention_query: String,
}
impl Default for FlowState {
    fn default() -> Self {
        Self {
            serial: 0,
            latest: BTreeMap::new(),
            bodies: BTreeMap::new(),
            versions: BTreeMap::new(),
            history: BTreeMap::new(),
            history_requests: BTreeMap::new(),
            history_latest: BTreeMap::new(),
            queued: vec![],
            error: None,
            company_kind: "domain".into(),
            traffic_host: String::new(),
            traffic_method: String::new(),
            traffic_query: String::new(),
            traffic_body: String::new(),
            traffic_path: String::new(),
            traffic_status: String::new(),
            traffic_min: String::new(),
            traffic_max: String::new(),
            traffic_sort: "ts".into(),
            traffic_order: "desc".into(),
            traffic_page: 0,
            traffic_size: 100,
            traffic_selected: String::new(),
            traffic_delete_confirm: false,
            session_task: String::new(),
            session_selected_run: String::new(),
            session_kind: "main".into(),
            session_tab: 0,
            session_message: String::new(),
            session_reason: String::new(),
            question_drafts: BTreeMap::new(),
            question_versions: BTreeMap::new(),
            sync_dimension: "project".into(),
            sync_page: 1,
            sync_size: 50,
            sync_targets: vec![],
            sync_types: vec!["subdomain".into(), "service".into(), "app".into()],
            sync_company: true,
            sync_url: String::new(),
            sync_read_versions: None,
            export_format: "json".into(),
            context_section: None,
            context_selected: None,
            stages: BTreeMap::new(),
            append: BTreeMap::new(),
            logs_tab: 0,
            logs_model: String::new(),
            logs_session: String::new(),
            logs_task: String::new(),
            logs_page: 0,
            logs_size: 50,
            logs_confirm: false,
            budget_override: String::new(),
            attachments: BTreeMap::new(),
            pending_messages: BTreeMap::new(),
            pending_approvals: BTreeMap::new(),
            approval_feedback: BTreeMap::new(),
            message_drafts: BTreeMap::new(),
            composer_measurement: None,
            evidence_binding: String::new(),
            evidence_side: "request".into(),
            evidence_finding: String::new(),
            skill_overwrite: false,
            mention_query: String::new(),
        }
    }
}
impl Default for Panes {
    fn default() -> Self {
        Self {
            projection: BTreeMap::new(),
            title: String::new(),
            description: String::new(),
            goal: String::new(),
            draft_message: String::new(),
            agent_key: "auto".into(),
            profile_id: None,
            selected_profiles: Vec::new(),
            selected_sources: Vec::new(),
            task_tab: 0,
            scope_kind: "subdomain".into(),
            scope_value: String::new(),
            scope_reason: String::new(),
            company_name: String::new(),
            finding_note: String::new(),
            selected_status: "confirmed".into(),
            selected_severity: "medium".into(),
            file_path: String::new(),
            file_text: String::new(),
            file_dirty: false,
            file_loaded: None,
            file_revision: None,
            file_locked: false,
            new_directory: String::new(),
            sync_search: String::new(),
            read_only: false,
            management: ManagementState::default(),
            flow: FlowState::default(),
        }
    }
}
impl Panes {
    pub(crate) fn chat_minimum_height(&self) -> f32 {
        self.flow
            .composer_measurement
            .as_ref()
            .filter(|m| {
                self.flow
                    .context_selected
                    .as_ref()
                    .is_some_and(|id| m.scope.ends_with(&format!(":{id}")))
            })
            .map(|m| m.height + m.header_height + 88.0)
            .unwrap_or(310.0)
    }
    pub fn set_read_only(&mut self, read_only: bool) {
        self.read_only = read_only;
    }
    pub fn set_command_result_for_params(
        &mut self,
        method: &str,
        params: &Value,
        selected: Option<&str>,
        value: Value,
    ) {
        self.complete_archive_management(method, params, Some(&value), None);
        if let Some(key) = approval_key(method, params) {
            self.flow.pending_approvals.remove(&key);
            self.flow.approval_feedback.insert(
                key,
                (
                    true,
                    "처리가 완료되었습니다. 저장된 상태가 곧 갱신됩니다.".into(),
                ),
            );
        }
        self.set_command_result(method, selected, value);
    }
    pub fn set_command_error_for_params(
        &mut self,
        method: &str,
        params: &Value,
        selected: Option<&str>,
        code: &str,
        message: &str,
    ) {
        self.complete_archive_management(method, params, None, Some(format!("{code}: {message}")));
        if let Some(key) = approval_key(method, params) {
            self.flow.pending_approvals.remove(&key);
            self.flow
                .approval_feedback
                .insert(key, (false, format!("{code}: {message}")));
        }
        self.set_command_error_for(method, selected, code, message);
    }
    fn submit_approval(
        &mut self,
        effects: &mut Vec<Effect>,
        method: &str,
        params: Value,
        versions: Vec<Expected>,
    ) {
        let Some(key) = approval_key(method, &params) else {
            return;
        };
        if self.flow.pending_approvals.contains_key(&key)
            || self
                .flow
                .approval_feedback
                .get(&key)
                .is_some_and(|(done, _)| *done)
        {
            return;
        }
        self.flow.approval_feedback.remove(&key);
        self.flow
            .pending_approvals
            .insert(key, self.flow.context_selected.clone());
        command(effects, method, params, versions);
    }
    fn approval_busy(&self, method: &str, id: &str) -> bool {
        let key = (method.to_owned(), id.to_owned());
        self.flow.pending_approvals.contains_key(&key)
            || self
                .flow
                .approval_feedback
                .get(&key)
                .is_some_and(|(done, _)| *done)
    }
    fn approval_status(&self, ui: &mut egui::Ui, method: &str, id: &str) {
        let key = (method.to_owned(), id.to_owned());
        if self.flow.pending_approvals.contains_key(&key) {
            ui.horizontal_wrapped(|ui| {
                ui.spinner();
                ui.strong("처리 중… 중복 요청은 보내지 않습니다.");
            });
        } else if let Some((done, message)) = self.flow.approval_feedback.get(&key) {
            if *done {
                ui.label(message);
            } else {
                ui.colored_label(ui.visuals().error_fg_color, message);
            }
        }
    }
    pub fn set_command_result(&mut self, method: &str, selected: Option<&str>, value: Value) {
        let Some(id) = selected else {
            return;
        };
        if method == "ui.chatUpload" {
            if let Some(items) = value["attachments"].as_array() {
                self.flow
                    .attachments
                    .entry(id.into())
                    .or_default()
                    .extend(items.iter().cloned());
            }
        }
        if matches!(method, "ui.sendConversationMessage" | "ui.chat") {
            if let Some((message, attachments)) = self.flow.pending_messages.remove(id) {
                if self
                    .flow
                    .message_drafts
                    .get(id)
                    .is_some_and(|v| v == &message)
                {
                    self.flow.message_drafts.insert(id.into(), String::new());
                }
                if self.flow.context_section == Some(Section::Chat)
                    && self.flow.context_selected.as_deref() == Some(id)
                    && self.draft_message == message
                {
                    self.draft_message.clear();
                }
                if let Some(current) = self.flow.attachments.get_mut(id) {
                    current.retain(|v| !attachments.contains(v));
                }
            }
        }
    }
    pub fn set_command_error_for(
        &mut self,
        method: &str,
        selected: Option<&str>,
        code: &str,
        message: &str,
    ) {
        if matches!(method, "ui.chat" | "ui.sendConversationMessage") {
            if let Some(id) = selected {
                self.flow.pending_messages.remove(id);
            }
        }
        if self.flow.context_selected.as_deref() == selected {
            self.flow.error = Some(format!("{code}: {message}"));
        }
    }
    pub fn retire_async_operations(&mut self) {
        self.read_only = true;
        self.management.requests.clear();
        self.management.monitor_latest = None;
        self.management.monitor = None;
        self.management.monitor_detail_tokens.clear();
        self.management.monitor_details.clear();
        self.management.monitor_last_poll = None;
        self.management.trash_token = None;
        self.management.trash = None;
        self.management.feedback.clear();
        self.management.monitor_offset = 0;
        if let Some(dialog) = self.management.dialog.as_mut() {
            dialog.preview_token = None;
            dialog.action_token = None;
            dialog.reload_required = true;
            dialog.error =
                Some("실행 세대가 종료되었습니다. 현재 상태를 다시 확인해야 합니다.".into());
        }
        if self.flow.context_section == Some(Section::Chat) {
            self.flow.message_drafts.insert(
                self.flow.context_selected.clone().unwrap_or("@new".into()),
                self.draft_message.clone(),
            );
        }
        self.flow.pending_messages.clear();
        self.flow.composer_measurement = None;
        self.flow.pending_approvals.clear();
        self.flow.approval_feedback.clear();
        self.flow.stages.clear();
        self.flow.bodies.clear();
        self.flow.latest.clear();
        self.flow.history.clear();
        self.flow.history_requests.clear();
        self.flow.history_latest.clear();
        self.flow.append.clear();
        self.flow.queued.clear();
        if self.file_dirty || self.file_loaded.is_some() {
            self.file_locked = true;
        }
        self.flow.error=Some("실행 세대가 종료되었습니다. 미저장 초안과 읽은 버전은 유지됩니다. 파일·결과를 명시적으로 다시 불러와 확인하세요.".into());
    }
    pub fn set_projection_error(&mut self, method: &str, token: &str, code: &str, message: &str) {
        if self.flow.latest.get(method).is_some_and(|v| v == token)
            || self.flow.bodies.remove(token).is_some()
        {
            self.flow.error = Some(format!("{code}: {message}"));
        }
    }
    pub fn set_staged_error(&mut self, purpose: &str, code: &str, message: &str) {
        self.flow.stages.remove(purpose);
        if code != "CANCELLED" {
            self.flow.error = Some(format!("{code}: {message}"));
        }
    }
    pub fn sessions(
        &mut self,
        ui: &mut egui::Ui,
        snapshot: &Snapshot,
        item: Option<&Record>,
        effects: &mut Vec<Effect>,
    ) {
        ui.heading("에이전트 실행과 대화");
        self.agent_monitor(ui, snapshot, item, effects);
        if let Some(run) = item {
            let tid = text(&run.data, "task_id");
            let iid = text(&run.data, "intent_id");
            if self.flow.session_selected_run != run.header.id && !tid.is_empty() {
                self.flow.session_selected_run = run.header.id.clone();
                self.flow.session_task = tid.into();
                if !iid.is_empty() {
                    self.flow.session_kind = format!("intent:{iid}");
                }
            }
            ui.horizontal_wrapped(|ui| {
                state_badge(ui, text(&run.data, "state"));
                if let Some(agent) = snapshot.get("agents", text(&run.data, "agent_id")) {
                    ui.strong(display_name(agent));
                }
                ui.add(egui::Label::new(RichText::new("실행 기록").small()).truncate())
                    .on_hover_text(&run.header.id);
            });
        }
        ui.horizontal_wrapped(|ui| {
            for (tab, label) in [(0, "실행 개요·승인"), (1, "활동 이력")] {
                if crate::theme::tab(ui, label, self.flow.session_tab == tab).clicked() {
                    self.flow.session_tab = tab;
                }
            }
            if let Some(run) = item {
                let questions = snapshot
                    .records("user_questions")
                    .into_iter()
                    .filter(|q| q.data["run_id"] == run.header.id && q.data["state"] == "pending")
                    .count();
                if questions > 0 {
                    ui.colored_label(
                        ui.visuals().warn_fg_color,
                        format!("답변·승인 대기 {questions}"),
                    );
                }
            }
        });
        ui.add_space(8.0);
        if self.flow.session_tab == 0 {
            if let Some(run) = item {
                for (field, label) in [
                    ("created_at", "생성 시각"),
                    ("started_at", "시작 시각"),
                    ("finished_at", "종료 시각"),
                    ("deadline_at", "실행 기한"),
                    ("failure_code", "실패 원인"),
                    ("turn_count", "진행한 대화 턴"),
                ] {
                    if !run.data[field].is_null() {
                        ui.horizontal_wrapped(|ui| {
                            ui.weak(label);
                            ui.label(
                                run.data[field]
                                    .as_str()
                                    .map(|v| {
                                        if field.ends_with("_at") {
                                            presentation_time(v)
                                        } else {
                                            v.to_owned()
                                        }
                                    })
                                    .unwrap_or_else(|| run.data[field].to_string()),
                            );
                        });
                    }
                }
                let tid = text(&run.data, "task_id");
                let iid = text(&run.data, "intent_id");
                let cid = text(&run.data, "conversation_id");
                if !cid.is_empty() {
                    if ui.button("실행의 대화 열기").clicked() {
                        effects.push(Effect::Select {
                            section: Section::Chat,
                            id: cid.into(),
                        });
                    }
                }
                if let Some(task) = snapshot.get("tasks", tid) {
                    crate::theme::text_field(ui, "제어 사유", &mut self.flow.session_reason);
                    ui.horizontal_wrapped(|ui| {
                        if !iid.is_empty() {
                            let intent=snapshot.get("exploration_nodes",iid);
                            for (label,action) in [("작업자 일시정지","pause"),("작업자 재개","resume"),("작업자 취소","cancel")] {
                                if ui.add_enabled(!self.read_only && intent_action_allowed(task,intent,action,&self.flow.session_reason),egui::Button::new(label)).clicked() {
                                    let mut versions=owned_run_versions(snapshot,run); versions.push(expected(task));
                                    if let Some(intent)=intent { versions.push(expected(intent)); }
                                    command(effects,"ui.controlIntent",json!({"taskId":tid,"intentId":iid,"action":action,"mode":"hard","reason":self.flow.session_reason}),versions);
                                }
                            }
                        } else if ui.add_enabled(!self.read_only && task_action_allowed(snapshot,task,"pause"),egui::Button::new("전체 작업 일시정지")).clicked() {
                            let mut versions=owned_run_versions(snapshot,run); versions.push(expected(task));
                            command(effects,"ui.controlTask",json!({"id":tid,"action":"pause"}),versions);
                        }
                    });
                    multiline(
                        ui,
                        "작업 담당에게 전달할 지침",
                        &mut self.flow.session_message,
                        3,
                    );
                    if ui
                        .add_enabled(
                            !self.flow.session_message.trim().is_empty(),
                            egui::Button::new("작업 대화에 지침 전달"),
                        )
                        .clicked()
                    {
                        command(
                            effects,
                            "ui.chat",
                            json!({"task":tid,"message":self.flow.session_message}),
                            vec![expected(task)],
                        );
                    }
                }
                self.pending_questions(ui, snapshot, run, effects);
            } else {
                ui.label("실행을 선택하거나 작업의 대화 이력을 조회하세요.");
            }
        }
        if self.flow.session_tab == 0 && item.is_some() {
            return;
        }
        ui.separator();
        ui.horizontal_wrapped(|ui| {
            ui.label("작업");
            egui::ComboBox::from_id_salt("session-task")
                .selected_text(
                    snapshot
                        .get("tasks", &self.flow.session_task)
                        .map(display_name)
                        .unwrap_or_else(|| "작업 선택".into()),
                )
                .show_ui(ui, |ui| {
                    for task in snapshot.records("tasks").into_iter().filter(|t| {
                        t.data["deleted_at"].is_null() && t.data["archived_at"].is_null()
                    }) {
                        if ui
                            .selectable_value(
                                &mut self.flow.session_task,
                                task.header.id.clone(),
                                display_name(task),
                            )
                            .changed()
                        {
                            self.projection.remove("ui.activityHistory");
                            self.flow.session_kind = "main".into();
                        }
                    }
                });
            ui.label("대화");
            egui::ComboBox::from_id_salt("session-kind")
                .selected_text(&self.flow.session_kind)
                .show_ui(ui, |ui| {
                    ui.selectable_value(
                        &mut self.flow.session_kind,
                        "main".into(),
                        "담당 에이전트",
                    );
                    ui.selectable_value(
                        &mut self.flow.session_kind,
                        "plan".into(),
                        "계획 에이전트",
                    );
                    if let Some(task) = snapshot.get("tasks", &self.flow.session_task) {
                        for node in snapshot
                            .records("exploration_nodes")
                            .into_iter()
                            .filter(|n| {
                                n.data["exploration_id"] == task.data["exploration_id"]
                                    && n.data["kind"] == "intent"
                            })
                        {
                            ui.selectable_value(
                                &mut self.flow.session_kind,
                                format!("intent:{}", node.header.id),
                                display_name(node),
                            );
                        }
                    }
                });
        });
        if ui
            .add_enabled(
                !self.flow.session_task.is_empty(),
                egui::Button::new("대화 이력 조회"),
            )
            .clicked()
        {
            self.request(effects,"ui.activityHistory",json!({"task":self.flow.session_task,"session":self.flow.session_kind,"before":null,"limit":200}),false);
        }
        if let Some(result) = self.projection.get("ui.activityHistory").cloned() {
            if result["has_more"] == true && ui.button("이전 이력 더 불러오기").clicked() {
                self.request(effects,"ui.activityHistory",json!({"task":self.flow.session_task,"session":self.flow.session_kind,"before":result["earliest_cursor"],"limit":200}),true);
            }
            for row in result["items"].as_array().into_iter().flatten() {
                ui.group(|ui| {
                    ui.small(format!(
                        "{} · {} · {}",
                        text(row, "ts"),
                        text(row, "worker"),
                        text(row, "kind")
                    ));
                    ui.label(text(row, "summary"));
                    if row["has_detail"] == true && ui.small_button("전체 내용 조회").clicked()
                    {
                        self.request(
                            effects,
                            "ui.activityDetail",
                            json!({"id":row["seq"],"task":self.flow.session_task}),
                            false,
                        );
                    }
                });
            }
        }
        if self.projection.contains_key("ui.activityDetail") {
            self.projection_body(
                ui,
                effects,
                "ui.activityDetail",
                vec!["detail".into()],
                "활동 상세",
            );
        }
        if let Some(error) = &self.flow.error {
            ui.colored_label(ui.visuals().warn_fg_color, error);
        }
    }
    fn pending_questions(
        &mut self,
        ui: &mut egui::Ui,
        s: &Snapshot,
        run: &Record,
        effects: &mut Vec<Effect>,
    ) {
        for question in s
            .records("user_questions")
            .into_iter()
            .filter(|q| q.data["run_id"] == run.header.id && q.data["state"] == "pending")
        {
            let qid = question.header.id.clone();
            let mut initial = owned_run_versions(s, run);
            if !initial
                .iter()
                .any(|e| e.collection == question.header.collection && e.id == question.header.id)
            {
                initial.push(expected(question));
            }
            self.flow
                .question_versions
                .entry(qid.clone())
                .or_insert(initial);
            let baseline = self.flow.question_versions[&qid].clone();
            let conflict = baseline.iter().any(|e| {
                s.get(&e.collection, &e.id)
                    .is_none_or(|r| r.header.version != e.version)
            });
            ui.push_id(&qid, |ui| {
                crate::theme::group(ui, "사용자 답변 필요", |ui| {
                    self.approval_status(ui, "user.answer", &qid);
                    if conflict {
                        ui.colored_label(ui.visuals().warn_fg_color, "질문 또는 실행 상태가 변경되었습니다. 질문을 다시 불러오세요.");
                        if ui.button("질문 다시 불러오기").clicked() {
                            self.flow.question_versions.remove(&qid);
                            // Keep the user's answer draft when explicitly refreshing its baseline.
                        }
                        return;
                    }
                    let payload = &question.data["payload"];
                    let busy = self.approval_busy("user.answer", &qid);
                    let mut submitted = None;
                    if question.data["kind"] == "plan_approval" || payload["type"] == "tool_approval" {
                        ui.label(if question.data["kind"] == "plan_approval" { "제안한 계획 이후 실행을 승인하시겠습니까?" } else { "이 도구 실행을 승인하시겠습니까?" });
                        ui.strong(text(payload, "name"));
                        if !payload["input"].is_null() { ui.collapsing("실행 입력 확인", |ui| { read_value(ui, &payload["input"]); }); }
                        crate::theme::toolbar(ui, |ui| {
                            for (label, approved) in [("승인", true), ("거부", false)] {
                                if ui.add_enabled(!busy, egui::Button::new(label)).clicked() { submitted = Some(json!({"approved":approved})); }
                            }
                        });
                    } else {
                        let answer = self.flow.question_drafts.entry(qid.clone()).or_insert(json!({"answers":{}}));
                        ui.add_enabled_ui(!busy, |ui| {
                            for (index, q) in payload["questions"].as_array().into_iter().flatten().enumerate() {
                                let prompt = text(q, "question");
                                ui.label(prompt);
                                let key = if prompt.is_empty() { index.to_string() } else { prompt.into() };
                                if q["multiSelect"] == true {
                                    if !answer["answers"][&key].is_array() { answer["answers"][&key] = json!([]); }
                                    ui.horizontal_wrapped(|ui| {
                                        for option in q["options"].as_array().into_iter().flatten() {
                                            let label = text(option, "label");
                                            let mut on = answer["answers"][&key].as_array().unwrap().contains(&json!(label));
                                            if ui.checkbox(&mut on, label).changed() {
                                                let list = answer["answers"][&key].as_array_mut().unwrap(); list.retain(|v| v != label); if on { list.push(json!(label)); }
                                            }
                                        }
                                    });
                                } else {
                                    if !answer["answers"][&key].is_string() { answer["answers"][&key] = json!(""); }
                                    let mut entered = text(&answer["answers"], &key).to_owned();
                                    ui.horizontal_wrapped(|ui| {
                                        for option in q["options"].as_array().into_iter().flatten() { ui.radio_value(&mut entered, text(option,"label").to_owned(), format!("{} — {}", text(option,"label"), text(option,"description"))); }
                                    });
                                    ui.add(egui::TextEdit::singleline(&mut entered).desired_width(ui.available_width()));
                                    answer["answers"][&key] = json!(entered);
                                }
                            }
                        });
                        if ui.add_enabled(!busy, egui::Button::new("답변 제출")).clicked() { submitted = Some(answer.clone()); }
                    }
                    if let Some(answer) = submitted {
                        self.submit_approval(effects, "user.answer", json!({"question_id":qid,"generation":question.data["generation"],"answer":answer}), baseline.clone());
                    }
                });
            });
        }
    }
}
fn owned_run_versions(s: &Snapshot, run: &Record) -> Vec<Expected> {
    let mut out = vec![expected(run)];
    for table in [
        "user_questions",
        "message_deliveries",
        "conversation_requests",
        "finding_retests",
    ] {
        for r in s.records(table) {
            if r.data["run_id"] == run.header.id
                || r.data["owner_run_id"] == run.header.id
                || r.data["sender_run_id"] == run.header.id
                || r.data["recipient_run_id"] == run.header.id
            {
                out.push(expected(r));
            }
        }
    }
    out
}
impl Panes {
    pub fn set_projection(&mut self, method: &str, value: Value) {
        // History needs its original conversation/read token. An unscoped
        // reply cannot establish whether it predates a destructive command.
        if method == "ui.conversationHistory" {
            return;
        }
        if method == "native.file.read" && !self.file_dirty {
            self.file_locked = value["binary"] == true || value["too_large"] == true;
            let content = value.get("content").unwrap_or(&value);
            self.file_text = body_text(content);
            self.file_loaded = Some(self.file_path.clone());
            self.file_revision = value["sha256"].as_str().map(str::to_owned);
        }
        self.projection.insert(method.into(), value);
    }
    pub fn overview(&mut self, ui: &mut egui::Ui, snapshot: &Snapshot, effects: &mut Vec<Effect>) {
        ui.heading("대시보드");
        ui.small(format!(
            "현재 작업 공간 · 저장 세대 {}",
            snapshot.commit_seq
        ));
        ui.add_space(12.0);
        let tasks = snapshot
            .records("tasks")
            .into_iter()
            .filter(|r| live_task(r))
            .count();
        let runs = snapshot
            .records("agent_runs")
            .into_iter()
            .filter(|r| active_run(r))
            .count();
        let approvals = snapshot
            .records("intercept_pending")
            .into_iter()
            .filter(|r| pending_record(r))
            .count()
            + snapshot
                .records("user_questions")
                .into_iter()
                .filter(|r| pending_record(r))
                .count();
        ui.horizontal_wrapped(|ui| {
            for (label, count) in [
                ("작업", tasks),
                ("실행 중", runs),
                ("실행 승인·질문", approvals),
                ("발견 사항", snapshot.records("findings").len()),
                ("자산", snapshot.records("assets").len()),
            ] {
                egui::Frame::group(ui.style()).show(ui, |ui| {
                    ui.set_min_width(104.0);
                    ui.small(label);
                    ui.label(RichText::new(count.to_string()).size(26.0).strong());
                });
            }
        });
        ui.add_space(14.0);
        if ui.available_width() > 740.0 {
            ui.columns(2, |columns| {
                dashboard_tasks(&mut columns[0], snapshot, effects);
                dashboard_approvals(&mut columns[1], snapshot, effects);
            });
        } else {
            dashboard_tasks(ui, snapshot, effects);
            ui.add_space(10.0);
            dashboard_approvals(ui, snapshot, effects);
        }
        ui.add_space(12.0);
        egui::Frame::group(ui.style()).show(ui, |ui| {
            ui.strong("최근 활동");
            let mut entries = snapshot.records("activity");
            entries.extend(snapshot.records("conversation_activities"));
            entries.retain(|r| visible_activity(text(&r.data, "kind")));
            recent_first(&mut entries);
            if entries.is_empty() {
                ui.weak("아직 기록된 활동이 없습니다.");
            }
            for entry in entries.into_iter().take(8) {
                ui.push_id(&entry.header.id, |ui| {
                    ui.horizontal_wrapped(|ui| {
                        ui.small(compact_time(&entry.header.created_at));
                        ui.strong(if text(&entry.data, "worker").is_empty() {
                            "시스템"
                        } else {
                            text(&entry.data, "worker")
                        });
                        ui.label(display_name(entry));
                    });
                });
                ui.separator();
            }
        });
        let mut running = snapshot
            .records("agent_runs")
            .into_iter()
            .filter(|r| active_run(r))
            .collect::<Vec<_>>();
        recent_first(&mut running);
        egui::CollapsingHeader::new(format!("현재 에이전트 실행 · {}", running.len()))
            .id_salt("dashboard-runs")
            .show(ui, |ui| {
                if running.is_empty() {
                    ui.weak("현재 실행 중인 에이전트가 없습니다.");
                }
                for run in running.into_iter().take(20) {
                    run_row(ui, snapshot, run, effects);
                }
            });
    }
    pub fn tasks(
        &mut self,
        ui: &mut egui::Ui,
        snapshot: &Snapshot,
        selected: Option<&Record>,
        effects: &mut Vec<Effect>,
    ) {
        let Some(task) = selected else {
            ui.heading("새 작업");
            crate::theme::field_row(ui, "이름", |ui| {
                ui.add(egui::TextEdit::singleline(&mut self.title).desired_width(f32::INFINITY));
            });
            multiline(ui, "설명", &mut self.description, 3);
            multiline(ui, "목표와 허용 범위", &mut self.goal, 4);
            ui.strong("모델 실행 순서");
            for profile in snapshot.records("llm_profiles") {
                let mut checked = self.selected_profiles.contains(&profile.header.id);
                if ui.checkbox(&mut checked, display_name(profile)).changed() {
                    if checked {
                        self.selected_profiles.push(profile.header.id.clone());
                    } else {
                        self.selected_profiles.retain(|p| p != &profile.header.id);
                    }
                }
            }
            ui.collapsing("정보를 공유할 직접 원본 작업", |ui| {
                for source in snapshot.records("tasks") {
                    let mut checked = self.selected_sources.contains(&source.header.id);
                    if ui.checkbox(&mut checked, display_name(source)).changed() {
                        if checked && self.selected_sources.len() < 8 {
                            self.selected_sources.push(source.header.id.clone());
                        } else {
                            self.selected_sources.retain(|id| id != &source.header.id);
                        }
                    }
                }
            });
            if ui
                .add_enabled(
                    !self.description.trim().is_empty() && !self.goal.trim().is_empty(),
                    egui::Button::new("작업 만들기"),
                )
                .clicked()
            {
                command(
                    effects,
                    "ui.createTask",
                    json!({"input":{"name":self.title,"description":self.description,"goal":self.goal,"llmProfileIds":self.selected_profiles,"sourceTaskIds":self.selected_sources}}),
                    vec![],
                );
            }
            return;
        };
        ui.horizontal_wrapped(|ui| {
            ui.heading(display_name(task));
            let _ = ui.menu_button("⋯", |ui| {
                self.object_actions(ui, snapshot, Section::Tasks, task)
            });
        });
        self.request_agent_monitor(false);
        let task_run = self
            .management
            .monitor
            .as_ref()
            .and_then(|v| v["runs"].as_array())
            .and_then(|runs| {
                runs.iter()
                    .find(|r| r["task_id"] == task.header.id && monitor_can_stop(r))
            })
            .cloned();
        if let Some(row) = task_run {
            if ui
                .add_enabled(
                    !self.read_only && !self.native_target_busy(&task.header.id),
                    egui::Button::new("작업의 모든 실행 중지"),
                )
                .clicked()
            {
                self.stop_native_run(&row, true, snapshot);
            }
        }
        if let Some(feedback) = self.management.feedback.get(&task.header.id) {
            ui.add(egui::Label::new(feedback).wrap());
        }
        if let Some(runtime) = snapshot.records("task_runtime").into_iter().find(|r| {
            r.data["task_id"] == task.header.id && r.data["accounting_state"] == "recovery_required"
        }) {
            ui.colored_label(ui.visuals().warn_fg_color,"비정상 종료로 활성 실행 시간 확인이 필요합니다. 명시적으로 복구한 뒤 작업을 재개하세요.");
            ui.label(format!(
                "기록된 활성 시간: {} ms",
                runtime.data["elapsed_active_ms"]
            ));
            if ui.button("기록된 활성 시간을 수용하여 복구").clicked() {
                command(
                    effects,
                    "ui.recoverTaskBudget",
                    json!({"id":task.header.id,"mode":"use_recorded","elapsedActiveMs":null}),
                    vec![expected(task), expected(runtime)],
                );
            }
            ui.horizontal_wrapped(|ui|{crate::theme::text_field(ui, "확인한 전체 활성 시간 (ms)", &mut self.flow.budget_override);if ui.add_enabled(self.flow.budget_override.parse::<u64>().is_ok(),egui::Button::new("입력한 시간으로 복구")).clicked(){command(effects,"ui.recoverTaskBudget",json!({"id":task.header.id,"mode":"override","elapsedActiveMs":self.flow.budget_override}),vec![expected(task),expected(runtime)]);}});
        }
        ui.horizontal_wrapped(|ui| {
            state_badge(ui, text(&task.data, "status"));
            ui.separator();
            ui.label(format!("버전 {}", task.header.version));
            ui.small(format!(
                "최근 변경 {}",
                compact_time(&task.header.updated_at)
            ));
        });
        ui.horizontal_wrapped(|ui| {
            for (label, action) in [("일시정지", "pause"), ("재개", "resume")] {
                if ui
                    .add_enabled(
                        !self.read_only && task_action_allowed(snapshot, task, action),
                        egui::Button::new(label),
                    )
                    .clicked()
                {
                    command(
                        effects,
                        "ui.controlTask",
                        json!({"id":task.header.id,"action":action}),
                        vec![expected(task)],
                    );
                }
            }
            if ui.button("보관").clicked() {
                command(
                    effects,
                    "ui.archiveTask",
                    json!({"id":task.header.id}),
                    vec![expected(task)],
                );
            }
            if ui.button("대화 열기").clicked() {
                effects.push(Effect::Select {
                    section: Section::Chat,
                    id: task.header.id.clone(),
                });
            }
        });
        ui.separator();
        ui.horizontal_wrapped(|ui| {
            for (i, label) in [(0, "개요"), (5, "에이전트"), (3, "발견 사항"), (4, "증거")]
            {
                if crate::theme::tab(
                    ui,
                    label,
                    self.task_tab == i || (i == 0 && matches!(self.task_tab, 1 | 2)),
                )
                .clicked()
                {
                    self.task_tab = i;
                }
            }
        });
        ui.separator();
        if matches!(self.task_tab, 0 | 1 | 2) {
            ui.horizontal_wrapped(|ui| {
                for (index, label) in [(0, "설명·설정"), (1, "목표·허용 범위"), (2, "탐색 노드")]
                {
                    if crate::theme::tab(ui, label, self.task_tab == index).clicked() {
                        self.task_tab = index;
                    }
                }
            });
            ui.add_space(8.0);
        }
        let exploration = text(&task.data, "exploration_id");
        match self.task_tab {
            0 => {
                ui.strong("설명");
                ui.label(body_text(&task.data["description"]));
                ui.strong("목표");
                ui.label(body_text(&task.data["goal"]));
                ui.collapsing("작업 설정", |ui| {
                    for field in [
                        "timeout_seconds",
                        "plan_heartbeat_seconds",
                        "coverage_enabled",
                        "total_work_ms",
                        "paused",
                    ] {
                        ui.horizontal_wrapped(|ui| {
                            ui.label(field);
                            ui.label(task.data[field].to_string());
                        });
                    }
                });
            }
            1 => {
                ui.strong("명시적 허용 범위");
                for scope in snapshot
                    .records("task_scope")
                    .into_iter()
                    .filter(|r| r.data["task_id"] == task.header.id)
                {
                    ui.horizontal_wrapped(|ui| {
                        ui.label(format!(
                            "{} · {}",
                            text(&scope.data, "kind"),
                            text(&scope.data, "value")
                        ));
                        if ui.small_button("삭제").clicked() {
                            command(
                                effects,
                                "ui.deleteTaskScope",
                                json!({"id":task.header.id,"scopeId":scope.header.id}),
                                vec![expected(task), expected(scope)],
                            );
                        }
                    });
                }
                egui::ComboBox::from_id_salt("scope_kind")
                    .selected_text(&self.scope_kind)
                    .show_ui(ui, |ui| {
                        for kind in ["subdomain", "root_domain", "ip", "cidr", "url"] {
                            ui.selectable_value(&mut self.scope_kind, kind.into(), kind);
                        }
                    });
                crate::theme::text_field(ui, "범위 값", &mut self.scope_value);
                crate::theme::text_field(ui, "추가 사유", &mut self.scope_reason);
                if ui
                    .add_enabled(!self.scope_value.is_empty(), egui::Button::new("범위 추가"))
                    .clicked()
                {
                    command(
                        effects,
                        "ui.addTaskScope",
                        json!({"id":task.header.id,"kind":self.scope_kind,"value":self.scope_value,"reason":self.scope_reason}),
                        vec![expected(task)],
                    );
                }
                ui.separator();
                for node in snapshot
                    .records("exploration_nodes")
                    .into_iter()
                    .filter(|r| r.data["exploration_id"] == exploration && r.data["kind"] == "goal")
                {
                    ui.label(format!(
                        "{} · {}",
                        text(&node.data, "state"),
                        display_name(node)
                    ));
                }
            }
            2 => {
                for node in snapshot
                    .records("exploration_nodes")
                    .into_iter()
                    .filter(|r| r.data["exploration_id"] == exploration)
                {
                    ui.horizontal_wrapped(|ui| {
                        ui.label(text(&node.data, "kind"));
                        ui.label(text(&node.data, "state"));
                        ui.label(display_name(node));
                    });
                }
            }
            3 => {
                ui.strong("발견 사항");
                let mut total = 0;
                for finding in snapshot.records("findings").into_iter().filter(|r| {
                    r.data["task_id"] == task.header.id || r.data["exploration_id"] == exploration
                }) {
                    total += 1;
                    ui.push_id(&finding.header.id, |ui| {
                        ui.horizontal_wrapped(|ui| {
                            state_badge(ui, text(&finding.data, "status"));
                            ui.small(text(&finding.data, "severity"));
                            if ui.link(display_name(finding)).clicked() {
                                effects.push(Effect::Select {
                                    section: Section::Findings,
                                    id: finding.header.id.clone(),
                                });
                            }
                        });
                    });
                }
                if total == 0 {
                    ui.weak("아직 이 작업에 기록된 발견 사항이 없습니다.");
                }
            }
            4 => {
                ui.strong("연결된 자산");
                for link in snapshot
                    .records("task_asset_links")
                    .into_iter()
                    .filter(|r| r.data["task_id"] == task.header.id)
                {
                    if let Some(asset) = snapshot.get("assets", text(&link.data, "asset_id")) {
                        if ui.link(display_name(asset)).clicked() {
                            effects.push(Effect::Select {
                                section: Section::Assets,
                                id: asset.header.id.clone(),
                            });
                        }
                    }
                }
                ui.add_space(10.0);
                ui.strong("발견 사항의 트래픽 증거");
                let findings = snapshot
                    .records("findings")
                    .into_iter()
                    .filter(|r| {
                        r.data["task_id"] == task.header.id
                            || r.data["exploration_id"] == exploration
                    })
                    .map(|r| r.header.id.clone())
                    .collect::<Vec<_>>();
                let mut total = 0;
                for evidence in snapshot
                    .records("finding_traffic_bindings")
                    .into_iter()
                    .filter(|r| findings.iter().any(|id| r.data["finding_id"] == *id))
                {
                    total += 1;
                    ui.push_id(&evidence.header.id, |ui| {
                        let fid = text(&evidence.data, "finding_id");
                        let label = snapshot
                            .get("findings", fid)
                            .map(display_name)
                            .unwrap_or_else(|| fid.into());
                        if ui
                            .link(format!("{label} · {}", display_name(evidence)))
                            .clicked()
                        {
                            effects.push(Effect::Select {
                                section: Section::Findings,
                                id: fid.into(),
                            });
                        }
                    });
                }
                if total == 0 {
                    ui.weak("아직 연결된 트래픽 증거가 없습니다. 발견 사항 상세에서 증거를 연결할 수 있습니다.");
                }
            }
            _ => {
                ui.strong("에이전트 실행");
                let mut runs = snapshot
                    .records("agent_runs")
                    .into_iter()
                    .filter(|r| r.data["task_id"] == task.header.id)
                    .collect::<Vec<_>>();
                recent_first(&mut runs);
                if runs.is_empty() {
                    ui.weak("이 작업에는 아직 실행 기록이 없습니다.");
                }
                for run in runs {
                    run_row(ui, snapshot, run, effects);
                }
                egui::CollapsingHeader::new("작업 활동 기록")
                    .id_salt(("task-activities", &task.header.id))
                    .show(ui, |ui| self.activities(ui, snapshot, Some(exploration)));
            }
        }
    }
    pub fn chat(
        &mut self,
        ui: &mut egui::Ui,
        snapshot: &Snapshot,
        selected: Option<&Record>,
        task: Option<&str>,
        effects: &mut Vec<Effect>,
    ) {
        if selected.is_none() && task.is_none() {
            ui.heading("새 대화");
            crate::theme::field_row(ui, "에이전트", |ui| {
                egui::ComboBox::from_id_salt("chat_agent")
                    .width(ui.available_width())
                    .selected_text(&self.agent_key)
                    .show_ui(ui, |ui| {
                        for agent in snapshot
                            .records("agents")
                            .into_iter()
                            .filter(|a| a.data["enabled"] == true)
                        {
                            let key = text(&agent.data, "key");
                            ui.selectable_value(
                                &mut self.agent_key,
                                key.into(),
                                display_name(agent),
                            );
                        }
                    });
            });
            crate::theme::field_row(ui, "제목", |ui| {
                ui.add(egui::TextEdit::singleline(&mut self.title).desired_width(f32::INFINITY));
            });
            profile_selector(ui, snapshot, &mut self.profile_id);
            if ui.button("대화 만들기").clicked() {
                command(
                    effects,
                    "ui.createConversation",
                    json!({"agent_key":self.agent_key,"title":self.title,"llm_profile_id":self.profile_id}),
                    vec![],
                );
            }
            return;
        }
        let header_top = ui.cursor().top();
        ui.horizontal_wrapped(|ui| {
            let title = selected
                .map(display_name)
                .unwrap_or_else(|| "작업 대화".into());
            ui.add_sized(
                [(ui.available_width() - 265.0).clamp(100.0, 260.0), 28.0],
                egui::Label::new(RichText::new(&title).heading()).truncate(),
            )
            .on_hover_text(title);
            if ui
                .add_enabled(
                    !self.read_only
                        && chat_can_stop(snapshot, selected.map(|r| r.header.id.as_str()), task),
                    egui::Button::new("응답 중지"),
                )
                .on_disabled_hover_text(if self.read_only {
                    "읽기 전용 작업 공간에서는 실행을 중지할 수 없습니다."
                } else {
                    "중단할 실행이나 응답 대기가 없습니다."
                })
                .clicked()
            {
                if let Some(conversation) = selected {
                    command(
                        effects,
                        "ui.stopConversation",
                        json!({"id":conversation.header.id}),
                        vec![expected(conversation)],
                    );
                } else if let Some(id) = task {
                    command(effects, "ui.stopChat", json!({"taskId":id}), vec![]);
                }
            }
            if let Some(conversation) = selected {
                let _ = ui.menu_button("⋯", |ui| {
                    self.object_actions(ui, snapshot, Section::Chat, conversation)
                });
                if ui.button("기록 새로고침").clicked() {
                    query(
                        effects,
                        "ui.conversationHistory",
                        json!({"id":conversation.header.id,"limit":100}),
                    );
                }
            }
        });
        let related = snapshot
            .records("agent_runs")
            .into_iter()
            .filter(|r| {
                selected.is_some_and(|c| r.data["conversation_id"] == c.header.id)
                    || task.is_some_and(|id| r.data["task_id"] == id)
            })
            .collect::<Vec<_>>();
        let running = related
            .iter()
            .filter(|r| matches!(text(&r.data, "state"), "running" | "queued"))
            .count();
        let waiting = related
            .iter()
            .filter(|r| text(&r.data, "state") == "waiting")
            .count();
        ui.horizontal_wrapped(|ui| {
            ui.small(format!("관련 실행 {running} · 응답 대기 {waiting}"));
            if let Some(conversation) = selected {
                ui.small(text(&conversation.data, "agent_key"));
            }
        });
        let header_height = (ui.cursor().top() - header_top).max(0.0);
        let compact_composer = ui.available_height() < 460.0 || ui.available_width() < 500.0;
        let dense_composer = ui.available_height() < 300.0;
        let send_shortcut = if ui.ctx().os() == egui::os::OperatingSystem::Mac {
            "⌘+Enter"
        } else {
            "Ctrl+Enter"
        };
        let send_label = format!("보내기 · {send_shortcut}");
        let attachment_scope = selected
            .map(|c| c.header.id.as_str())
            .or(task)
            .unwrap_or("")
            .to_owned();
        let geometry_scope = format!("{}:{}", snapshot.workspace_id, attachment_scope);
        let width = ui.available_width();
        let measured = self
            .flow
            .composer_measurement
            .as_ref()
            .filter(|m| m.scope == geometry_scope)
            .map(|m| m.height);
        let changed_geometry = self.flow.composer_measurement.as_ref().is_none_or(|m| {
            m.scope != geometry_scope
                || (m.width - width).abs() >= 1.0
                || m.compact != compact_composer
                || m.dense != dense_composer
                || (m.header_height - header_height).abs() > 0.5
        });
        // A scrollbar changing available width must not reset this owner's
        // already measured height and repeatedly recreate first-frame overflow.
        let composer_height = measured.unwrap_or(if compact_composer { 270.0 } else { 290.0 });
        let mut body_rect = ui.available_rect_before_wrap();
        body_rect.set_bottom(
            body_rect
                .bottom()
                .max(body_rect.top() + composer_height + 88.0),
        );
        let composer_rect = egui::Rect::from_min_max(
            egui::pos2(body_rect.left(), body_rect.bottom() - composer_height),
            body_rect.right_bottom(),
        );
        let transcript_rect = egui::Rect::from_min_max(
            body_rect.left_top(),
            egui::pos2(body_rect.right(), composer_rect.top() - 8.0),
        );
        let mut transcript_ui = ui.new_child(
            egui::UiBuilder::new()
                .id_salt(("chat-messages-pane", &attachment_scope))
                .max_rect(transcript_rect)
                .layout(egui::Layout::top_down(egui::Align::Min)),
        );
        egui::ScrollArea::vertical()
            .id_salt((
                "chat-transcript",
                selected.map(|r| r.header.id.as_str()).or(task),
            ))
            .stick_to_bottom(true)
            .auto_shrink([false, false])
            .max_height(transcript_rect.height())
            .show(&mut transcript_ui, |ui| {
                let projection = selected.and_then(|c| self.flow.history.get(&c.header.id));
                if let Some(items) = projection.and_then(|v| v["items"].as_array()) {
                    for item in items {
                        let id = text(item, "seq");
                        if let Some(entry) = snapshot.get("conversation_activities", id) {
                            transcript(ui, entry);
                        } else {
                            message_ui(ui, id, item, text(item, "ts"));
                        }
                    }
                } else {
                    let mut entries: Vec<_> = if let Some(conversation) = selected {
                        snapshot
                            .records("conversation_activities")
                            .into_iter()
                            .filter(|r| r.data["conversation_id"] == conversation.header.id)
                            .collect()
                    } else {
                        snapshot
                            .records("activity")
                            .into_iter()
                            .filter(|r| {
                                task.is_some_and(|id| {
                                    snapshot.get("tasks", id).is_some_and(|t| {
                                        r.data["exploration_id"] == t.data["exploration_id"]
                                    })
                                })
                            })
                            .collect()
                    };
                    entries.sort_by(|a, b| {
                        a.header
                            .created_at
                            .cmp(&b.header.created_at)
                            .then(a.header.commit_seq.cmp(&b.header.commit_seq))
                    });
                    for entry in entries {
                        transcript(ui, entry);
                    }
                }
            });
        let mut composer_ui = ui.new_child(
            egui::UiBuilder::new()
                .id_salt(("chat-composer-pane", &attachment_scope))
                .max_rect(composer_rect)
                .layout(egui::Layout::top_down(egui::Align::Min)),
        );
        // One real render measures the editor; normal repaint adjusts its position
        // next frame without replaying commands, hotkeys or attachment actions.
        let composer = crate::theme::surface_frame(ui.style()).inner_margin(if dense_composer {6}else{10}).show(&mut composer_ui, |ui| {
        ui.set_width(ui.available_width());
        if dense_composer { ui.spacing_mut().item_spacing.y = 6.0; }
        ui.horizontal_wrapped(|ui| {
            if dense_composer { ui.small(RichText::new("메시지").weak()); } else { ui.weak("메시지"); }
            if self.flow.pending_messages.contains_key(&attachment_scope) {
                ui.spinner();
                ui.small("저장·실행 요청 확인 중");
            }
        });
        ui.add(
            egui::TextEdit::multiline(&mut self.draft_message)
                .id_salt(("chat-input", &attachment_scope))
                .frame(false)
                .desired_rows(if dense_composer {1}else if compact_composer {2}else{3})
                .desired_width(f32::INFINITY)
                .hint_text(format!("메시지를 입력하세요. {send_shortcut}로 보냅니다.")),
        );
        if let Some(items) = self.flow.attachments.get_mut(&attachment_scope) {
            let mut remove = None;
            egui::ScrollArea::vertical()
                .id_salt(("chat-attachments", &attachment_scope))
                // The default 64px floor would exceed the dense 32px viewport
                // and push the footer outside a small window with an open drawer.
                .min_scrolled_height(0.0)
                .max_height(if dense_composer {32.0}else if compact_composer {48.0}else{72.0})
                .show(ui, |ui| {
                    for (index, item) in items.iter().enumerate() {
                        ui.horizontal_wrapped(|ui| {
                            ui.label(format!("첨부: {} ({})", text(item, "name"), item["size"]));
                            if ui.small_button("메시지에서 제외").clicked() {
                                remove = Some(index);
                            }
                        });
                    }
                });
            if let Some(index) = remove {
                items.remove(index);
            }
        }
        let clicked = ui.horizontal_wrapped(|ui| {
            if ui.button("파일 첨부 준비").clicked() {
                self.staged_request(effects, &format!("chat-upload:{attachment_scope}"), "ui.chatUpload", json!({"scope":if task.is_some(){"task"}else{"session"},"id":attachment_scope}), None);
            }
            self.reference_picker(ui, snapshot);
            let can_send = (!self.draft_message.trim().is_empty()
                    || self
                        .flow
                        .attachments
                        .get(&attachment_scope)
                        .is_some_and(|a| !a.is_empty()))
                    && !self.flow.pending_messages.contains_key(&attachment_scope);
            ui.add_enabled_ui(can_send, |ui| crate::theme::primary_button(ui, &send_label)).inner.clicked()
        }).inner;
        clicked || ui.input(|i| (i.modifiers.ctrl || i.modifiers.command) && i.key_pressed(egui::Key::Enter))
        });
        let send = composer.inner;
        let actual_height = composer.response.rect.height();
        if changed_geometry || (actual_height - composer_height).abs() > 0.5 {
            ui.ctx().request_repaint();
        }
        self.flow.composer_measurement = Some(ComposerMeasurement {
            scope: geometry_scope,
            width,
            compact: compact_composer,
            dense: dense_composer,
            header_height,
            height: actual_height,
        });
        ui.advance_cursor_after_rect(body_rect.union(composer.response.rect));
        if send
            && (!self.draft_message.trim().is_empty()
                || self
                    .flow
                    .attachments
                    .get(&attachment_scope)
                    .is_some_and(|a| !a.is_empty()))
            && !self.flow.pending_messages.contains_key(&attachment_scope)
        {
            self.flow.pending_messages.insert(
                attachment_scope.clone(),
                (
                    self.draft_message.clone(),
                    self.flow
                        .attachments
                        .get(&attachment_scope)
                        .cloned()
                        .unwrap_or_default(),
                ),
            );
            if let Some(conversation) = selected {
                command(
                    effects,
                    "ui.sendConversationMessage",
                    json!({"id":conversation.header.id,"message":self.draft_message,"attachments":self.flow.attachments.get(&attachment_scope).cloned().unwrap_or_default()}),
                    vec![expected(conversation)],
                );
            } else if let Some(id) = task {
                command(
                    effects,
                    "ui.chat",
                    json!({"task":id,"message":self.draft_message,"attachments":self.flow.attachments.get(&attachment_scope).cloned().unwrap_or_default()}),
                    vec![],
                );
            }
        }
    }
    fn reference_picker(&mut self, ui: &mut egui::Ui, snapshot: &Snapshot) {
        ui.menu_button("객체 참조 삽입", |ui| {
            crate::theme::text_field(ui, "이름 또는 ID 검색", &mut self.flow.mention_query);
            let query = self.flow.mention_query.trim().to_lowercase();
            let mut candidates = Vec::new();
            let mut matches = 0;
            for collection in ["findings", "companies", "assets"] {
                for record in snapshot.records(collection) {
                    let label = reference_label(record);
                    if !query.is_empty()
                        && !label.to_lowercase().contains(&query)
                        && !record.header.id.to_lowercase().contains(&query)
                    {
                        continue;
                    }
                    let Some(token) = reference_token(collection, &record.header.id, &label) else {
                        continue;
                    };
                    matches += 1;
                    if candidates.len() < 30 {
                        candidates.push((record, label, token));
                    }
                }
            }
            ui.small(format!(
                "검색 결과 {matches}개 · 최대 30개 표시. 이름이나 ID로 검색 범위를 좁히세요."
            ));
            egui::ScrollArea::vertical()
                .id_salt("chat-mention-candidates")
                .max_height(150.0)
                .show(ui, |ui| {
                    for (record, label, token) in candidates {
                        ui.push_id(&record.header.id, |ui| {
                            ui.horizontal_wrapped(|ui| {
                                let kind = match record.header.collection.as_str() {
                                    "findings" => "취약점",
                                    "companies" => "기업",
                                    _ => "자산",
                                };
                                if ui.button(format!("{kind} · {label}")).clicked() {
                                    if !self.draft_message.is_empty()
                                        && !self.draft_message.ends_with(char::is_whitespace)
                                    {
                                        self.draft_message.push(' ');
                                    }
                                    self.draft_message.push_str(&token);
                                    self.draft_message.push(' ');
                                    ui.close();
                                }
                                ui.small(&record.header.id);
                            });
                        });
                    }
                });
        });
    }
    pub fn assets(
        &mut self,
        ui: &mut egui::Ui,
        snapshot: &Snapshot,
        selected: Option<&Record>,
        effects: &mut Vec<Effect>,
    ) {
        if let Some(asset) = selected {
            ui.heading(display_name(asset));
            details(ui, asset);
            ui.strong("객체 관계");
            for link in snapshot
                .records("task_asset_links")
                .into_iter()
                .filter(|r| r.data["asset_id"] == asset.header.id)
            {
                if let Some(task) = snapshot.get("tasks", text(&link.data, "task_id")) {
                    if ui.link(display_name(task)).clicked() {
                        effects.push(Effect::Select {
                            section: Section::Tasks,
                            id: task.header.id.clone(),
                        });
                    }
                }
            }
        } else {
            ui.heading("회사와 자산 범위");
            crate::theme::text_field(ui, "회사 이름", &mut self.company_name);
            if ui
                .add_enabled(
                    !self.company_name.is_empty(),
                    egui::Button::new("회사 만들기"),
                )
                .clicked()
            {
                command(
                    effects,
                    "ui.createCompany",
                    json!({"name":self.company_name,"scope":[]}),
                    vec![],
                );
            }
            choice(
                ui,
                "company-scope-kind",
                "회사 범위 종류",
                &mut self.flow.company_kind,
                &["domain", "ip", "cidr", "icp", "keyword"],
            );
            for company in snapshot.records("companies") {
                ui.collapsing(display_name(company),|ui|{for scope in snapshot.records("company_scope").into_iter().filter(|s|s.data["company_id"]==company.header.id){ui.label(format!("{} · {}",text(&scope.data,"kind"),text(&scope.data,"value")));}ui.text_edit_singleline(&mut self.scope_value);if ui.add_enabled(!self.scope_value.is_empty(),egui::Button::new("자산 범위 추가")).clicked(){command(effects,"ui.addCompanyScope",json!({"id":company.header.id,"scope":[{"kind":self.flow.company_kind,"value":self.scope_value}],"reason":self.scope_reason}),vec![expected(company)]);}});
            }
        }
    }
    pub fn finding(
        &mut self,
        ui: &mut egui::Ui,
        snapshot: &Snapshot,
        finding: Option<&Record>,
        effects: &mut Vec<Effect>,
    ) {
        let Some(f) = finding else {
            ui.label("목록에서 발견 사항을 선택하세요.");
            return;
        };
        ui.heading(display_name(f));
        details(ui, f);
        ui.horizontal_wrapped(|ui| {
            egui::ComboBox::from_id_salt("finding_status")
                .selected_text(&self.selected_status)
                .show_ui(ui, |ui| {
                    for status in [
                        "pending",
                        "in_progress",
                        "confirmed",
                        "resolved",
                        "fixed",
                        "false_positive",
                        "ignored",
                        "duplicate",
                        "risk_accepted",
                    ] {
                        ui.selectable_value(&mut self.selected_status, status.into(), status);
                    }
                });
            if ui.button("상태 변경").clicked() {
                command(
                    effects,
                    "ui.setFindingStatus",
                    json!({"id":f.header.id,"status":self.selected_status}),
                    finding_versions(snapshot, f),
                );
            }
        });
        ui.horizontal_wrapped(|ui| {
            egui::ComboBox::from_id_salt("finding_severity")
                .selected_text(&self.selected_severity)
                .show_ui(ui, |ui| {
                    for severity in ["critical", "high", "medium", "low"] {
                        ui.selectable_value(&mut self.selected_severity, severity.into(), severity);
                    }
                });
            if ui.button("위험도 변경").clicked() {
                command(
                    effects,
                    "ui.setFindingSeverity",
                    json!({"id":f.header.id,"severity":self.selected_severity}),
                    finding_versions(snapshot, f),
                );
            }
        });
        multiline(ui, "재검증·심화 지침", &mut self.finding_note, 3);
        ui.horizontal_wrapped(|ui| {
            if ui.button("재검증 시작").clicked() {
                command(
                    effects,
                    "ui.startFindingRetest",
                    json!({"id":f.header.id,"notes":self.finding_note}),
                    finding_versions(snapshot, f),
                );
            }
            if ui
                .add_enabled(
                    !self.finding_note.is_empty(),
                    egui::Button::new("심화 작업 만들기"),
                )
                .clicked()
            {
                command(
                    effects,
                    "ui.deepenFinding",
                    json!({"id":f.header.id,"description":self.finding_note}),
                    finding_versions(snapshot, f),
                );
            }
            if ui.button("증거 조회").clicked() {
                query(effects, "ui.findingTraffic", json!({"id":f.header.id}));
            }
        });
        self.finding_evidence(ui, &f.header.id, effects);
    }
    pub fn approvals(
        &mut self,
        ui: &mut egui::Ui,
        pending: Option<&Record>,
        effects: &mut Vec<Effect>,
    ) {
        if let Some(p) = pending {
            ui.heading("실행 승인");
            self.approval_status(ui, "ui.interceptDecide", &p.header.id);
            ui.horizontal_wrapped(|ui| {
                ui.strong(text(&p.data, "tool_name"));
                ui.label(format!("상태: {}", text(&p.data, "status")));
                if !text(&p.data, "reason").is_empty() {
                    ui.label(text(&p.data, "reason"));
                }
            });
            if text(&p.data, "state") == "pending" || text(&p.data, "status") == "pending" {
                let busy = self.approval_busy("ui.interceptDecide", &p.header.id);
                crate::theme::toolbar(ui, |ui| {
                    for (label, decision) in [("허용", "allowed"), ("거부", "denied")] {
                        if ui.add_enabled(!busy, egui::Button::new(label)).clicked() {
                            self.submit_approval(
                                effects,
                                "ui.interceptDecide",
                                json!({"id":p.header.id,"decision":decision}),
                                vec![expected(p)],
                            );
                        }
                    }
                });
            }
            crate::theme::group(ui, "실행 요청", |ui| {
                if !p.data["tool_input"].is_null() {
                    read_value(ui, &p.data["tool_input"]);
                } else if !p.data["input"].is_null() {
                    read_value(ui, &p.data["input"]);
                }
            });
            ui.collapsing("전체 승인 기록", |ui| {
                details(ui, p);
            });
        } else {
            ui.label("승인이 필요한 실행을 선택하세요.");
        }
    }
    pub fn files(&mut self, ui: &mut egui::Ui, effects: &mut Vec<Effect>) {
        ui.heading("작업 공간 파일");
        crate::theme::toolbar(ui, |ui| {
            if ui.button("선택한 로컬 파일을 이 폴더에 복사").clicked() {
                self.staged_request(
                    effects,
                    &format!("workspace-upload:{}", self.file_path),
                    "ui.workspaceUpload",
                    json!({"dir":self.file_path}),
                    Some(self.file_path.clone()),
                );
            }
            if ui.button("선택 경로 파일 내보내기 준비").clicked() {
                self.request(
                    effects,
                    "ui.workspaceDownload",
                    json!({"path":self.file_path}),
                    false,
                );
            }
        });
        self.download_projection(ui, effects, "ui.workspaceDownload");
        ui.horizontal_wrapped(|ui| {
            crate::theme::text_field(ui, "상대 경로", &mut self.file_path);
            if ui.button("폴더 목록").clicked() {
                query(effects, "ui.workspaceList", json!({"path":self.file_path}));
            }
            if ui
                .add_enabled(!self.file_dirty, egui::Button::new("파일 읽기"))
                .clicked()
            {
                effects.push(Effect::FileRead {
                    path: self.file_path.clone(),
                });
            }
        });
        if let Some(list) = self.projection.get("ui.workspaceList") {
            let rows = list["entries"]
                .as_array()
                .or_else(|| list.as_array())
                .or_else(|| list["items"].as_array());
            if let Some(rows) = rows {
                for item in rows {
                    let name = text(item, "name");
                    if ui
                        .selectable_label(
                            false,
                            if name.is_empty() {
                                text(item, "path")
                            } else {
                                name
                            },
                        )
                        .clicked()
                    {
                        self.file_path = text(item, "path").into();
                    }
                }
            }
        }
        self.file_dirty |= ui
            .add_enabled(
                !self.file_locked,
                egui::TextEdit::multiline(&mut self.file_text)
                    .code_editor()
                    .desired_width(f32::INFINITY)
                    .desired_rows(16),
            )
            .changed();
        if self.file_locked {
            ui.colored_label(ui.visuals().warn_fg_color,"바이너리·큰 파일 또는 충돌한 파일은 편집할 수 없습니다. 다시 읽기로 상태를 확인하세요.");
        }
        ui.horizontal_wrapped(|ui| {
            if ui
                .add_enabled(
                    !self.file_locked
                        && self.file_loaded.as_deref() == Some(&self.file_path)
                        && self.file_revision.is_some(),
                    egui::Button::new("변경 저장"),
                )
                .clicked()
            {
                effects.push(Effect::FileWrite {
                    path: self.file_path.clone(),
                    content: self.file_text.clone(),
                    expected_sha256: self.file_revision.clone(),
                    create: false,
                });
            }
            if ui
                .add_enabled(
                    !self.file_locked && !self.file_path.is_empty(),
                    egui::Button::new("새 파일 만들기"),
                )
                .clicked()
            {
                effects.push(Effect::FileWrite {
                    path: self.file_path.clone(),
                    content: self.file_text.clone(),
                    expected_sha256: None,
                    create: true,
                });
            }
            if ui.button("편집 버리고 다시 읽기").clicked() {
                self.file_dirty = false;
                self.file_locked = false;
                effects.push(Effect::FileRead {
                    path: self.file_path.clone(),
                });
            }
        });
        ui.separator();
        crate::theme::text_field(ui, "새 폴더 경로", &mut self.new_directory);
        if ui
            .add_enabled(
                !self.new_directory.is_empty(),
                egui::Button::new("폴더 만들기"),
            )
            .clicked()
        {
            command(
                effects,
                "ui.workspaceMkdir",
                json!({"path":self.new_directory}),
                vec![],
            );
        }
    }
    pub fn activities(&self, ui: &mut egui::Ui, snapshot: &Snapshot, exploration: Option<&str>) {
        let mut list: Vec<_> = snapshot
            .records("activity")
            .into_iter()
            .filter(|r| exploration.is_none_or(|id| r.data["exploration_id"] == id))
            .collect();
        list.sort_by(|a, b| b.header.created_at.cmp(&a.header.created_at));
        for item in list.into_iter().take(150) {
            ui.collapsing(
                format!(
                    "{} · {}",
                    presentation_time(&item.header.created_at),
                    display_name(item)
                ),
                |ui| {
                    transcript(ui, item);
                },
            );
        }
    }
    pub fn archive(
        &mut self,
        ui: &mut egui::Ui,
        snapshot: &Snapshot,
        archive: Option<&Record>,
        effects: &mut Vec<Effect>,
    ) {
        ui.horizontal_wrapped(|ui| {
            for (tab, label) in [(0, "보관 작업"), (1, "대화 휴지통")] {
                if crate::theme::tab(ui, label, self.management.archive_tab == tab).clicked() {
                    self.management.archive_tab = tab;
                    if tab == 1 {
                        self.request_conversation_trash();
                    }
                }
            }
        });
        if self.management.archive_tab == 1 {
            self.trash_contents(ui, snapshot);
            return;
        }
        if let Some(a) = archive {
            ui.heading(display_name(a));
            details(ui, a);
            if ui
                .add_enabled(
                    !self.read_only && matches!(text(&a.data, "state"), "ready" | "restore_failed"),
                    egui::Button::new("보관 작업 복구"),
                )
                .clicked()
            {
                command(
                    effects,
                    "ui.restoreTaskArchive",
                    json!({"id":a.header.id}),
                    vec![expected(a)],
                );
            }
        } else {
            ui.label("보관된 작업을 선택하세요.");
        }
    }
    pub fn generic(&self, ui: &mut egui::Ui, section: Section, item: Option<&Record>) {
        ui.heading(section.label());
        if let Some(item) = item {
            details(ui, item);
        } else {
            ui.label("목록에서 객체를 선택하세요.");
        }
    }
}
fn live_task(r: &Record) -> bool {
    r.data["deleted_at"].is_null() && r.data["archived_at"].is_null()
}
fn active_run(r: &Record) -> bool {
    matches!(text(&r.data, "state"), "queued" | "running" | "waiting")
}
fn pending_record(r: &Record) -> bool {
    text(&r.data, "state") == "pending" || text(&r.data, "status") == "pending"
}
fn visible_activity(kind: &str) -> bool {
    !matches!(kind, "context_config" | "context_nudge" | "compression")
}
fn recent_first(rows: &mut Vec<&Record>) {
    rows.sort_by(|a, b| {
        b.header
            .updated_at
            .cmp(&a.header.updated_at)
            .then_with(|| {
                b.header
                    .commit_seq
                    .parse::<u64>()
                    .unwrap_or(0)
                    .cmp(&a.header.commit_seq.parse::<u64>().unwrap_or(0))
            })
            .then(b.header.id.cmp(&a.header.id))
    });
}
fn compact_time(value: &str) -> String {
    presentation_time(value)
}
pub(crate) fn presentation_time(value: &str) -> String {
    chrono::DateTime::parse_from_rfc3339(value)
        .map(|stamp| timestamp_label(stamp.with_timezone(&chrono::Local).fixed_offset()))
        .unwrap_or_else(|_| value.to_owned())
}
pub(crate) fn presentation_short_time(value: &str) -> String {
    chrono::DateTime::parse_from_rfc3339(value)
        .map(|stamp| {
            stamp
                .with_timezone(&chrono::Local)
                .format("%H:%M:%S · 현지")
                .to_string()
        })
        .unwrap_or_else(|_| "시각 미기록".into())
}
fn timestamp_label(stamp: chrono::DateTime<chrono::FixedOffset>) -> String {
    format!("{} · 현지", stamp.format("%Y-%m-%d %H:%M:%S UTC%:z"))
}
fn cancellable_run(run: &Record) -> bool {
    matches!(
        text(&run.data, "state"),
        "queued" | "running" | "waiting" | "paused"
    )
}
fn chat_can_stop(s: &Snapshot, conversation: Option<&str>, task: Option<&str>) -> bool {
    s.records("agent_runs").into_iter().any(|run| {
        cancellable_run(run)
            && if let Some(id) = conversation {
                run.data["conversation_id"] == id
            } else if let Some(id) = task {
                run.data["task_id"] == id
                    && run.data["intent_id"].is_null()
                    && s.get("agents", text(&run.data, "agent_id"))
                        .is_some_and(|agent| agent.data["key"] == "mainagent")
            } else {
                false
            }
    })
}
fn task_action_allowed(s: &Snapshot, task: &Record, action: &str) -> bool {
    if !live_task(task) {
        return false;
    }
    match action {
        "pause" => {
            matches!(text(&task.data, "status"), "running" | "queued")
                && task.data["paused"] != true
        }
        "resume" => {
            matches!(text(&task.data, "status"), "paused" | "created")
                && s.records("task_runtime")
                    .into_iter()
                    .filter(|r| r.data["task_id"] == task.header.id)
                    .all(|r| {
                        r.data["accounting_state"] != "recovery_required"
                            && r.data["remaining_timeout_ms"] != "0"
                            && (r.data["timeout_policy"] != "legacy_absolute"
                                || r.data["legacy_deadline_at"].as_str().is_none_or(|v| {
                                    chrono::DateTime::parse_from_rfc3339(v)
                                        .is_ok_and(|deadline| deadline > chrono::Utc::now())
                                }))
                    })
        }
        _ => false,
    }
}
fn intent_action_allowed(
    task: &Record,
    intent: Option<&Record>,
    action: &str,
    reason: &str,
) -> bool {
    if !live_task(task) {
        return false;
    }
    intent.is_some_and(|intent| {
        intent.data["kind"] == "intent"
            && match action {
                "pause" => intent.data["state"] == "running",
                "resume" => intent.data["state"] == "paused",
                "cancel" => {
                    matches!(text(&intent.data, "state"), "running" | "paused" | "open")
                        && !reason.trim().is_empty()
                }
                _ => false,
            }
    })
}
fn state_label(state: &str) -> &str {
    match state {
        "running" => "실행 중",
        "waiting" => "응답 대기",
        "queued" => "실행 대기",
        "created" => "준비",
        "paused" => "일시정지",
        "done" | "completed" | "succeeded" => "완료",
        "failed" => "실패",
        "cancelled" | "stopped" => "중지",
        "pending" => "승인 대기",
        "blocked" => "차단됨",
        "open" => "대기",
        "confirmed" => "확인됨",
        "interrupted" => "중단됨",
        "" => "미설정",
        _ => state,
    }
}
fn state_badge(ui: &mut egui::Ui, state: &str) {
    let color = match state {
        "failed" | "blocked" => ui.visuals().error_fg_color,
        "pending" | "waiting" | "paused" | "interrupted" => ui.visuals().warn_fg_color,
        "running" | "queued" => ui.visuals().selection.stroke.color,
        _ => ui.visuals().weak_text_color(),
    };
    ui.colored_label(color, RichText::new(state_label(state)).small());
}
fn run_row(ui: &mut egui::Ui, s: &Snapshot, run: &Record, effects: &mut Vec<Effect>) {
    ui.push_id(&run.header.id, |ui| {
        ui.horizontal_wrapped(|ui| {
            state_badge(ui, text(&run.data, "state"));
            let label = s
                .get("agents", text(&run.data, "agent_id"))
                .map(display_name)
                .unwrap_or_else(|| "에이전트".into());
            if ui.link(label).clicked() {
                effects.push(Effect::Select {
                    section: Section::Sessions,
                    id: run.header.id.clone(),
                });
            }
            if let Some(intent) = s.get("exploration_nodes", text(&run.data, "intent_id")) {
                ui.label(display_name(intent));
            }
            ui.small(compact_time(&run.header.updated_at));
        });
    });
}
fn dashboard_tasks(ui: &mut egui::Ui, s: &Snapshot, effects: &mut Vec<Effect>) {
    egui::Frame::group(ui.style()).show(ui, |ui| {
        ui.strong("최근 작업");
        let mut tasks = s
            .records("tasks")
            .into_iter()
            .filter(|r| live_task(r))
            .collect::<Vec<_>>();
        recent_first(&mut tasks);
        if tasks.is_empty() {
            ui.weak("탐색의 작업 메뉴에서 새 작업을 만드세요.");
        }
        for task in tasks.into_iter().take(6) {
            ui.push_id(&task.header.id, |ui| {
                ui.horizontal_wrapped(|ui| {
                    state_badge(ui, text(&task.data, "status"));
                    if ui
                        .selectable_label(false, RichText::new(display_name(task)).strong())
                        .clicked()
                    {
                        effects.push(Effect::Select {
                            section: Section::Tasks,
                            id: task.header.id.clone(),
                        });
                    }
                });
                let exploration = text(&task.data, "exploration_id");
                let runs = s
                    .records("agent_runs")
                    .into_iter()
                    .filter(|r| r.data["task_id"] == task.header.id && active_run(r))
                    .count();
                let findings = s
                    .records("findings")
                    .into_iter()
                    .filter(|r| {
                        r.data["task_id"] == task.header.id
                            || r.data["exploration_id"] == exploration
                    })
                    .count();
                ui.small(format!(
                    "실행 {runs} · 발견 사항 {findings} · {}",
                    compact_time(&task.header.updated_at)
                ));
            });
            ui.separator();
        }
    });
}
fn dashboard_approvals(ui: &mut egui::Ui, s: &Snapshot, effects: &mut Vec<Effect>) {
    egui::Frame::group(ui.style()).show(ui, |ui| {
        ui.strong("실행 승인·사용자 질문");
        let mut pending = s.records("intercept_pending");
        pending.extend(s.records("user_questions"));
        pending.retain(|r| pending_record(r));
        recent_first(&mut pending);
        if pending.is_empty() {
            ui.weak("현재 확인이 필요한 요청이 없습니다.");
        }
        for item in pending.into_iter().take(6) {
            ui.push_id(&item.header.id, |ui| {
                let question = item.header.collection == "user_questions";
                ui.horizontal_wrapped(|ui| {
                    state_badge(ui, "pending");
                    ui.strong(if question {
                        "사용자 질문"
                    } else {
                        "도구 실행 승인"
                    });
                    if ui.button("확인").clicked() {
                        effects.push(Effect::Select {
                            section: if question {
                                Section::Sessions
                            } else {
                                Section::Approvals
                            },
                            id: if question {
                                text(&item.data, "run_id").into()
                            } else {
                                item.header.id.clone()
                            },
                        });
                    }
                });
                let description = if question {
                    item.data["payload"]["questions"][0]["question"]
                        .as_str()
                        .unwrap_or(text(&item.data, "kind"))
                } else {
                    text(&item.data, "tool_name")
                };
                ui.label(description.chars().take(120).collect::<String>());
                ui.small(compact_time(&item.header.created_at));
            });
            ui.separator();
        }
    });
}
fn profile_selector(ui: &mut egui::Ui, snapshot: &Snapshot, id: &mut Option<String>) {
    egui::ComboBox::from_id_salt("chat_profile")
        .selected_text(
            id.as_deref()
                .and_then(|id| snapshot.get("llm_profiles", id))
                .map(display_name)
                .unwrap_or_else(|| "에이전트 기본 모델".into()),
        )
        .show_ui(ui, |ui| {
            ui.selectable_value(id, None, "에이전트 기본 모델");
            for profile in snapshot.records("llm_profiles") {
                ui.selectable_value(id, Some(profile.header.id.clone()), display_name(profile));
            }
        });
}
fn finding_versions(snapshot: &Snapshot, f: &Record) -> Vec<Expected> {
    let mut out = vec![expected(f)];
    if let Some(node) = snapshot.get("exploration_nodes", text(&f.data, "node_id")) {
        out.push(expected(node));
    }
    out
}
pub fn details(ui: &mut egui::Ui, record: &Record) {
    ui.small(&record.header.id);
    egui::Grid::new(ui.id().with(&record.header.id))
        .num_columns(2)
        .striped(true)
        .show(ui, |ui| {
            for (key, value) in record.data.as_object().into_iter().flat_map(|o| o.iter()) {
                if value.is_null() || key == "id" {
                    continue;
                }
                ui.label(key);
                if key.contains("password") || key == "api_key" {
                    ui.label(if value["secret_ref"].is_string() {
                        "OS 저장소 참조"
                    } else {
                        "미설정"
                    });
                } else if let Some(text) = value.as_str() {
                    ui.add(egui::Label::new(text).wrap());
                } else if value["text"].is_string() {
                    ui.add(egui::Label::new(body_text(value)).wrap());
                } else if value.is_object() || value.is_array() {
                    ui.collapsing("상세", |ui| read_value(ui, value));
                } else {
                    ui.label(value.to_string());
                }
                ui.end_row();
            }
        });
}
pub fn read_value(ui: &mut egui::Ui, value: &Value) {
    match value {
        Value::Array(items) => {
            for (i, item) in items.iter().enumerate().take(200) {
                ui.collapsing(format!("항목 {}", i + 1), |ui| read_value(ui, item));
            }
        }
        Value::Object(fields) => {
            for (key, value) in fields {
                ui.horizontal_wrapped(|ui| {
                    ui.add(
                        egui::Label::new(RichText::new(key).strong())
                            .wrap()
                            .selectable(true),
                    );
                    if value.is_object() || value.is_array() {
                        ui.collapsing("펼치기", |ui| read_value(ui, value));
                    } else {
                        ui.add(
                            egui::Label::new(
                                value
                                    .as_str()
                                    .map(str::to_owned)
                                    .unwrap_or_else(|| value.to_string()),
                            )
                            .wrap()
                            .selectable(true),
                        );
                    }
                });
            }
        }
        _ => {
            ui.add(
                egui::Label::new(
                    value
                        .as_str()
                        .map(str::to_owned)
                        .unwrap_or_else(|| value.to_string()),
                )
                .wrap()
                .selectable(true),
            );
        }
    }
}
#[derive(Debug, PartialEq)]
enum TranscriptPart {
    Text(String),
    Thinking(String),
    Audit {
        title: String,
        value: Value,
    },
    Tool {
        name: String,
        value: Value,
        result: bool,
        error: bool,
    },
}
fn content_text(value: &Value) -> Option<String> {
    if let Some(text) = value.as_str() {
        return Some(text.into());
    }
    if let Some(items) = value.as_array() {
        return Some(
            items
                .iter()
                .map(content_text)
                .collect::<Option<Vec<_>>>()?
                .join("\n"),
        );
    }
    if let Some(fields) = value.as_object() {
        let text_block =
            fields.len() == 2 && value["type"] == "text" && fields.contains_key("text");
        let inline_body = fields.len() == 2
            && fields.contains_key("text")
            && fields.contains_key("blob")
            && value["blob"].is_null();
        if text_block || inline_body {
            return value["text"].as_str().map(str::to_owned);
        }
    }
    None
}
fn transcript_parts(data: &Value) -> Vec<TranscriptPart> {
    let kind = text(data, "kind");
    if !visible_activity(kind) {
        return vec![];
    }
    let raw = body_text(&data["detail"]);
    let envelope = serde_json::from_str::<Value>(&raw).ok();
    let mut parts = vec![];
    if let Some(audit) = envelope.as_ref().filter(|e| e["guard_audit"].is_object()) {
        let tool = text(&audit["guard_audit"], "tool");
        let action = &audit["guard_audit"]["action"];
        let status = match action.as_str() {
            Some("allow") => "실행 허용".into(),
            Some("block" | "deny") => "실행 차단".into(),
            Some("ask" | "review") => "실행 검토".into(),
            _ => format!(
                "실행 상태: {}",
                action
                    .as_str()
                    .map(str::to_owned)
                    .unwrap_or_else(|| action.to_string())
            ),
        };
        return vec![TranscriptPart::Audit {
            title: format!("({}) {status}", if tool.is_empty() { "도구" } else { tool }),
            value: audit.clone(),
        }];
    }
    if let Some(blocks) = envelope
        .as_ref()
        .and_then(|e| e["message"]["content"].as_array())
    {
        for block in blocks {
            match text(block, "type") {
                "text" => parts.push(TranscriptPart::Text(text(block, "text").into())),
                "thinking" => parts.push(TranscriptPart::Thinking(text(block, "thinking").into())),
                "tool_use" => parts.push(TranscriptPart::Tool {
                    name: text(block, "name").into(),
                    value: block["input"].clone(),
                    result: false,
                    error: false,
                }),
                "tool_result" => parts.push(TranscriptPart::Tool {
                    name: text(data, "tool").into(),
                    value: block["content"].clone(),
                    result: true,
                    error: block["is_error"] == true || data["is_error"] == true,
                }),
                _ => {}
            }
        }
        return parts;
    }
    let visible = envelope
        .as_ref()
        .and_then(|e| e["text"].as_str())
        .map(str::to_owned)
        .unwrap_or_else(|| {
            if raw.is_empty() {
                text(data, "summary").into()
            } else {
                raw.clone()
            }
        });
    if matches!(kind, "tool_use" | "tool_result")
        || (kind == "result" && !text(data, "tool").is_empty())
    {
        let value = envelope.unwrap_or_else(|| json!(visible));
        parts.push(TranscriptPart::Tool {
            name: text(data, "tool").into(),
            value,
            result: kind != "tool_use",
            error: data["is_error"] == true,
        });
    } else if kind == "thinking" {
        parts.push(TranscriptPart::Thinking(visible));
    } else if !visible.is_empty() {
        parts.push(TranscriptPart::Text(visible));
    }
    parts
}
fn message_ui(ui: &mut egui::Ui, id: &str, data: &Value, ts: &str) {
    let parts = transcript_parts(data);
    if parts.is_empty() {
        return;
    }
    let kind = text(data, "kind");
    let who = if parts
        .iter()
        .any(|p| matches!(p, TranscriptPart::Audit { .. }))
    {
        "실행 승인"
    } else if kind == "user" {
        "사용자"
    } else if matches!(kind, "tool_use" | "tool_result") {
        "도구"
    } else if text(data, "worker").is_empty() {
        "에이전트"
    } else {
        text(data, "worker")
    };
    ui.push_id(id, |ui| {
        let available = ui.available_width();
        // Child scroll areas need a nonzero layout budget when disclosures open.
        // allocate_ui_with_layout only consumes the height actually used.
        let message_height = ui.available_height().clamp(240.0, 10000.0);
        let user_message = kind == "user";
        let message_width = if user_message {
            (available * 0.86).max(200.0).min(available)
        } else {
            available
        };
        ui.horizontal(|ui| {
            if user_message {
                ui.add_space((available - message_width - ui.spacing().item_spacing.x).max(0.0));
            }
            ui.allocate_ui_with_layout(
                egui::vec2(message_width, message_height),
                egui::Layout::top_down(egui::Align::Min),
                |ui| {
                    egui::Frame::new()
                        .inner_margin(egui::Margin::symmetric(10, 8))
                        .corner_radius(8)
                        .fill(if kind == "user" {
                            ui.visuals().faint_bg_color
                        } else {
                            egui::Color32::TRANSPARENT
                        })
                        .show(ui, |ui| {
                            ui.horizontal_wrapped(|ui| {
                                ui.strong(who);
                                ui.small(compact_time(ts));
                                if data["is_error"] == true {
                                    state_badge(ui, "failed");
                                }
                                let raw = body_text(&data["detail"]);
                                let partial = data["stream"]["pending"] == true
                                    || serde_json::from_str::<Value>(&raw).is_ok_and(|v| {
                                        v["stream"]["pending"] == true
                                            || v["stream"]["partial"] == true
                                    });
                                if partial {
                                    ui.small("부분 기록");
                                }
                            });
                            for (index, part) in parts.iter().enumerate() {
                                match part {
                                    TranscriptPart::Audit { title, value } => {
                                        ui.strong(title);
                                        egui::CollapsingHeader::new("실행 승인 기록")
                                            .id_salt((id, index, "audit"))
                                            .show(ui, |ui| {
                                                disclosure_body(
                                                    ui,
                                                    (id, index, "audit-body"),
                                                    value,
                                                    |ui| read_value(ui, value),
                                                );
                                            });
                                    }
                                    TranscriptPart::Text(body) => {
                                        ui.add(egui::Label::new(body).wrap());
                                    }
                                    TranscriptPart::Thinking(body) => {
                                        egui::CollapsingHeader::new("추론 기록")
                                            .id_salt((id, index, "thinking"))
                                            .show(ui, |ui| {
                                                ui.add(egui::Label::new(body).wrap());
                                            });
                                    }
                                    TranscriptPart::Tool {
                                        name,
                                        value,
                                        result,
                                        error,
                                    } => {
                                        let label = format!(
                                            "{} · {}{}",
                                            if name.is_empty() { "도구" } else { name },
                                            if *result {
                                                "실행 결과"
                                            } else {
                                                "실행 입력"
                                            },
                                            if *error { " · 오류" } else { "" }
                                        );
                                        egui::CollapsingHeader::new(label)
                                            .id_salt((id, index, "tool"))
                                            .default_open(*error)
                                            .show(ui, |ui| {
                                                disclosure_body(
                                                    ui,
                                                    (id, index, "tool-body"),
                                                    value,
                                                    |ui| {
                                                        if let Some(body) = result
                                                            .then(|| content_text(value))
                                                            .flatten()
                                                        {
                                                            ui.add(
                                                                egui::Label::new(
                                                                    RichText::new(body).monospace(),
                                                                )
                                                                .wrap(),
                                                            );
                                                        } else {
                                                            read_value(ui, value);
                                                        }
                                                    },
                                                );
                                            });
                                    }
                                }
                            }
                        });
                },
            );
        });
    });
    ui.add_space(6.0);
}
fn transcript(ui: &mut egui::Ui, entry: &Record) {
    message_ui(ui, &entry.header.id, &entry.data, &entry.header.created_at);
}
fn disclosure_body(
    ui: &mut egui::Ui,
    id: impl std::hash::Hash,
    value: &Value,
    contents: impl FnOnce(&mut egui::Ui),
) {
    // Small structured records use the outer transcript's scrolling; this keeps
    // approval provenance and mixed tool metadata visible together when opened.
    // Large bodies retain their own bounded viewport.
    if value.to_string().len() <= 8192 {
        contents(ui);
    } else {
        egui::ScrollArea::vertical()
            .id_salt(id)
            .max_height(240.0)
            .show(ui, contents);
    }
}
impl Panes {
    pub fn set_context(&mut self, section: Section, selected: Option<&str>) {
        let next = selected.map(str::to_owned);
        if self.flow.context_section != Some(section) || self.flow.context_selected != next {
            if self.flow.context_section == Some(Section::Chat) {
                let key = self
                    .flow
                    .context_selected
                    .as_deref()
                    .unwrap_or("@new")
                    .to_owned();
                self.flow
                    .message_drafts
                    .insert(key, self.draft_message.clone());
            }
            if section == Section::Chat {
                let key = selected.unwrap_or("@new");
                self.draft_message = self
                    .flow
                    .message_drafts
                    .get(key)
                    .cloned()
                    .unwrap_or_default();
            }
        }
        self.flow.context_section = Some(section);
        self.flow.context_selected = next;
    }
    fn request(
        &mut self,
        effects: &mut Vec<Effect>,
        method: &str,
        mut params: Value,
        append: bool,
    ) {
        self.flow.serial += 1;
        let token = format!("pane-{}", self.flow.serial);
        self.flow.latest.insert(method.into(), token.clone());
        self.flow.append.insert(token.clone(), append);
        params["_pane_token"] = json!(token);
        self.pin_history_request(method, &mut params);
        query(effects, method, params);
    }
    pub fn pin_history_request(&mut self, method: &str, params: &mut Value) {
        if method != "ui.conversationHistory" {
            return;
        }
        let Some(id) = params["id"]
            .as_str()
            .filter(|id| !id.is_empty())
            .map(str::to_owned)
        else {
            return;
        };
        let token = if let Some(token) = params["_pane_token"].as_str() {
            if self.flow.history_requests.get(token) == Some(&id) {
                return;
            }
            token.to_owned()
        } else {
            self.flow.serial += 1;
            format!("pane-history-{}", self.flow.serial)
        };
        self.flow.history_requests.retain(|_, owner| owner != &id);
        self.flow.history_requests.insert(token.clone(), id.clone());
        self.flow.history_latest.insert(id, token.clone());
        params["_pane_token"] = json!(token);
    }
    fn invalidate_history(&mut self, ids: &[String]) {
        for id in ids {
            // An empty authoritative projection also prevents the old snapshot
            // from rendering removed activities while its refresh is pending.
            self.flow.history.insert(id.clone(), json!({"items":[]}));
            self.flow.history_latest.remove(id);
        }
        let retired = self
            .flow
            .history_requests
            .iter()
            .filter(|(_, id)| ids.contains(id))
            .map(|(token, _)| token.clone())
            .collect::<Vec<_>>();
        for token in retired {
            self.flow.history_requests.remove(&token);
            self.flow.append.remove(&token);
            if self.flow.latest.get("ui.conversationHistory") == Some(&token) {
                self.flow.latest.remove("ui.conversationHistory");
                self.flow.versions.remove("ui.conversationHistory");
                self.projection.remove("ui.conversationHistory");
                self.flow
                    .bodies
                    .retain(|_, body| body.parent_token.as_ref() != Some(&token));
            }
        }
    }
    pub fn set_projection_for(
        &mut self,
        method: &str,
        token: &str,
        value: Value,
        read_versions: Vec<Expected>,
    ) -> bool {
        if method == "ui.conversationHistory" {
            let Some(id) = self.flow.history_requests.remove(token) else {
                return false;
            };
            if self
                .flow
                .history_latest
                .get(&id)
                .is_none_or(|latest| latest != token)
            {
                return false;
            }
            let result = value.get("result").unwrap_or(&value).clone();
            self.flow.append.remove(token);
            self.flow.history.insert(id, result);
            return true;
        }
        if method == "native.body.read" {
            let Some(target) = self.flow.bodies.remove(token) else {
                return false;
            };
            if self.flow.latest.get(&target.method) != target.parent_token.as_ref() {
                return false;
            }
            let result = value.get("result").unwrap_or(&value);
            let Some(text) = result.as_str().or_else(|| result["text"].as_str()) else {
                return false;
            };
            if let Some(parent) = self.projection.get_mut(&target.method) {
                return put_value(parent, &target.path, json!({"text":text,"blob":null}));
            }
            return false;
        }
        if self.flow.latest.get(method).is_none_or(|v| v != token) {
            return false;
        }
        let mut result = value.get("result").unwrap_or(&value).clone();
        if self.flow.append.remove(token) == Some(true) {
            if let (Some(previous), Some(incoming)) = (
                self.projection
                    .get(method)
                    .and_then(|v| v["items"].as_array()),
                result["items"].as_array(),
            ) {
                let mut merged = incoming.clone();
                for old in previous {
                    if !merged
                        .iter()
                        .any(|v| match (projection_id(v), projection_id(old)) {
                            (Some(a), Some(b)) => a == b,
                            _ => v == old,
                        })
                    {
                        merged.push(old.clone());
                    }
                }
                result["items"] = json!(merged);
            }
        }
        self.flow.versions.insert(method.into(), read_versions);
        self.set_projection(method, result);
        true
    }
    fn load_body(
        &mut self,
        effects: &mut Vec<Effect>,
        method: &str,
        path: Vec<String>,
        body: Value,
    ) {
        let parent_token = self.flow.latest.get(method).cloned();
        if self
            .flow
            .bodies
            .values()
            .any(|v| v.method == method && v.path == path && v.parent_token == parent_token)
        {
            return;
        }
        self.flow.serial += 1;
        let token = format!("pane-body-{}", self.flow.serial);
        let target = json!({"method":method,"path":path}).to_string();
        self.flow.bodies.insert(
            token.clone(),
            BodyTarget {
                method: method.into(),
                path,
                parent_token,
            },
        );
        query(
            effects,
            "native.body.read",
            json!({"body":body,"_pane_token":token,"_pane_target":target}),
        );
    }
    fn staged_request(
        &mut self,
        effects: &mut Vec<Effect>,
        purpose: &str,
        operation: &str,
        params: Value,
        path: Option<String>,
    ) {
        self.flow.serial += 1;
        let purpose = format!("{purpose}:{}", self.flow.serial);
        self.flow.stages.insert(
            purpose.clone(),
            StagedTarget {
                section: self.flow.context_section,
                selected: self.flow.context_selected.clone(),
                path,
                operation: operation.into(),
                params,
            },
        );
        effects.push(Effect::StageFile { purpose });
    }
    pub fn set_staged_file(&mut self, purpose: &str, value: Value) {
        let Some(token) = value
            .get("file_capability")
            .or_else(|| value.get("token"))
            .or_else(|| value.get("file"))
            .and_then(Value::as_str)
            .map(str::to_owned)
        else {
            self.flow.error = Some("준비된 파일 참조가 없습니다.".into());
            return;
        };
        let Some(mut target) = self.flow.stages.remove(purpose) else {
            self.flow.queued.push(Effect::ReleaseStagedFile { token });
            return;
        };
        if target.section != self.flow.context_section
            || target.selected != self.flow.context_selected
            || target.path.as_ref().is_some_and(|v| v != &self.file_path)
        {
            self.flow.queued.push(Effect::ReleaseStagedFile { token });
            return;
        }
        if target.operation == "ui.uploadSkill" {
            target.params["file"] = json!(token);
        } else {
            target.params["files"] = json!([token]);
        }
        self.flow.queued.push(Effect::Command {
            method: target.operation,
            params: target.params,
            expected: vec![],
        });
    }
    pub fn drain_effects(&mut self) -> Vec<Effect> {
        std::mem::take(&mut self.flow.queued)
    }
    fn projection_body(
        &mut self,
        ui: &mut egui::Ui,
        effects: &mut Vec<Effect>,
        method: &str,
        path: Vec<String>,
        label: &str,
    ) {
        let body = self
            .projection
            .get(method)
            .and_then(|v| get_value(v, &path))
            .cloned()
            .unwrap_or(Value::Null);
        ui.strong(label);
        if body["blob"].is_object() {
            if ui.button(format!("{label} 본문 불러오기")).clicked() {
                self.load_body(effects, method, path, body);
            }
        } else {
            let mut text = body_text(&body);
            ui.add(
                egui::TextEdit::multiline(&mut text)
                    .code_editor()
                    .interactive(false)
                    .desired_rows(10)
                    .desired_width(f32::INFINITY),
            );
        }
    }
    fn download_projection(&mut self, ui: &mut egui::Ui, effects: &mut Vec<Effect>, method: &str) {
        let Some(result) = self.projection.get(method).cloned() else {
            return;
        };
        if let Some(token) = result["file_capability"]
            .as_str()
            .or_else(|| result["file"].as_str())
            .or_else(|| result["token"].as_str())
        {
            if ui.button("파일로 저장").clicked() {
                effects.push(Effect::SaveStagedFile {
                    token: token.into(),
                    name: result["filename"].as_str().unwrap_or("export.bin").into(),
                });
            }
            return;
        }
        let key = if result.get("content_base64").is_some() {
            "content_base64"
        } else {
            "base64"
        };
        let data = &result[key];
        if (data.is_string() || data["text"].is_string() || data["blob"].is_object())
            && ui.button("파일로 저장").clicked()
        {
            effects.push(Effect::ExportBase64Body {
                name: result["filename"].as_str().unwrap_or("export.bin").into(),
                body: if data.is_string() {
                    json!({"text":data,"blob":null})
                } else {
                    data.clone()
                },
            });
        }
    }
    pub fn traffic(
        &mut self,
        ui: &mut egui::Ui,
        snapshot: &Snapshot,
        selected: Option<&Record>,
        effects: &mut Vec<Effect>,
    ) {
        ui.heading("트래픽 · 증거");
        crate::theme::text_field(ui, "호스트", &mut self.flow.traffic_host);
        crate::theme::text_field(ui, "검색", &mut self.flow.traffic_query);
        ui.horizontal_wrapped(|ui| {
            choice(
                ui,
                "traffic-method",
                "메서드",
                &mut self.flow.traffic_method,
                &[
                    "", "GET", "POST", "PUT", "PATCH", "DELETE", "HEAD", "OPTIONS", "CONNECT",
                ],
            );
            choice(
                ui,
                "traffic-status",
                "응답 코드",
                &mut self.flow.traffic_status,
                &["", "2xx", "3xx", "4xx", "5xx", "200", "404", "500"],
            );
            choice(
                ui,
                "traffic-sort",
                "정렬",
                &mut self.flow.traffic_sort,
                &["ts", "status", "resp_len"],
            );
            choice(
                ui,
                "traffic-order",
                "방향",
                &mut self.flow.traffic_order,
                &["desc", "asc"],
            );
        });
        ui.collapsing("본문·경로·크기 필터", |ui| {
            crate::theme::text_field(ui, "본문 검색 (3글자 이상)", &mut self.flow.traffic_body);
            crate::theme::text_field(ui, "URL 경로", &mut self.flow.traffic_path);
            crate::theme::text_field(ui, "응답 최소 bytes", &mut self.flow.traffic_min);
            crate::theme::text_field(ui, "최대 bytes", &mut self.flow.traffic_max);
        });
        let mut load = false;
        ui.horizontal_wrapped(|ui| {
            ui.label(format!("페이지 {}", self.flow.traffic_page + 1));
            if ui
                .add_enabled(self.flow.traffic_page > 0, egui::Button::new("이전"))
                .clicked()
            {
                self.flow.traffic_page -= 1;
                load = true;
            }
            if ui.button("다음").clicked() {
                self.flow.traffic_page += 1;
                load = true;
            }
            ui.label("페이지당");
            ui.add(egui::DragValue::new(&mut self.flow.traffic_size).range(1..=500));
            if ui.button("조회").clicked() {
                self.flow.traffic_page = 0;
                load = true;
            }
            if ui.button("호스트 목록").clicked() {
                self.request(effects, "ui.trafficHosts", json!({}), false);
            }
        });
        if load
            || (!self.projection.contains_key("ui.traffic")
                && !self.flow.latest.contains_key("ui.traffic"))
        {
            let p = json!({"page":self.flow.traffic_page,"size":self.flow.traffic_size,"host":self.flow.traffic_host,"method":self.flow.traffic_method,"q":self.flow.traffic_query,"opts":{"body":self.flow.traffic_body,"path":self.flow.traffic_path,"status":self.flow.traffic_status,"respMin":self.flow.traffic_min,"respMax":self.flow.traffic_max,"sort":self.flow.traffic_sort,"order":self.flow.traffic_order}});
            self.request(effects, "ui.traffic", p, false);
        }
        if let Some(result) = self.projection.get("ui.traffic").cloned() {
            ui.label(format!(
                "검색 {}건 / 전체 {}건 · 캡처 {} · 프록시 {}",
                result["total"],
                result["count"],
                if result["enabled"] == true {
                    "활성"
                } else {
                    "비활성"
                },
                text(&result, "proxy")
            ));
            if let Some(rows) = result["exchanges"].as_array() {
                egui::Grid::new("traffic-results")
                    .striped(true)
                    .show(ui, |ui| {
                        for title in ["시각", "메서드", "상태", "URL", "응답 bytes"] {
                            ui.strong(title);
                        }
                        ui.end_row();
                        for row in rows {
                            ui.label(presentation_time(text(row, "ts")));
                            ui.label(text(row, "method"));
                            ui.label(row["status"].to_string());
                            if ui.link(text(row, "url")).clicked() {
                                effects.push(Effect::Select {
                                    section: Section::Traffic,
                                    id: text(row, "id").into(),
                                });
                            }
                            ui.label(body_text(&row["resp_len"]));
                            ui.end_row();
                        }
                    });
            }
        }
        if let Some(hosts) = self
            .projection
            .get("ui.trafficHosts")
            .and_then(|v| v["hosts"].as_array())
            .cloned()
        {
            ui.collapsing("캡처 호스트", |ui| {
                for host in hosts {
                    if ui
                        .link(format!("{} ({})", text(&host, "host"), host["count"]))
                        .clicked()
                    {
                        self.flow.traffic_host = text(&host, "host").into();
                        self.flow.traffic_page = 0;
                    }
                }
            });
        }
        if let Some(row) = selected {
            if self.flow.traffic_selected != row.header.id {
                self.flow.traffic_selected = row.header.id.clone();
                self.projection.remove("ui.trafficExchange");
                self.request(
                    effects,
                    "ui.trafficExchange",
                    json!({"id":row.header.id}),
                    false,
                );
            }
            ui.separator();
            ui.heading(display_name(row));
            if let Some(capture) = snapshot
                .records("domain_events")
                .into_iter()
                .find(|e| {
                    e.data["kind"] == "TrafficCaptured" && e.data["subject_id"] == row.header.id
                })
                .and_then(|e| e.data.get("capture"))
            {
                ui.label(format!(
                    "캡처 {} · 요청 {} bytes · 응답 {} bytes",
                    text(capture, "state"),
                    capture["request_captured_bytes"],
                    capture["response_captured_bytes"]
                ));
                if let Some(error) = capture["error"].as_str() {
                    ui.colored_label(ui.visuals().warn_fg_color, error);
                }
            }
            self.projection_body(
                ui,
                effects,
                "ui.trafficExchange",
                vec!["req".into()],
                "요청",
            );
            self.projection_body(
                ui,
                effects,
                "ui.trafficExchange",
                vec!["resp".into()],
                "응답",
            );
            if ui.button("확인한 요청·응답을 텍스트로 저장").clicked() {
                if let Some(v) = self.projection.get("ui.trafficExchange") {
                    if v["req"]["blob"].is_null() && v["resp"]["blob"].is_null() {
                        effects.push(Effect::Download {
                            name: format!("{}.http.txt", row.header.id),
                            bytes: format!("{}\n\n{}", body_text(&v["req"]), body_text(&v["resp"]))
                                .into_bytes(),
                        });
                    } else {
                        self.flow.error = Some("먼저 요청과 응답 본문을 불러오세요.".into());
                    }
                }
            }
        }
        ui.separator();
        ui.checkbox(
            &mut self.flow.traffic_delete_confirm,
            "캡처 삭제를 확인합니다 (발견 사항에 고정된 증거는 유지)",
        );
        let pinned = traffic_versions(snapshot, &self.flow.traffic_host);
        ui.horizontal_wrapped(|ui| {
            if ui
                .add_enabled(
                    self.flow.traffic_delete_confirm && !self.flow.traffic_host.is_empty(),
                    egui::Button::new("선택 호스트 캡처 삭제"),
                )
                .clicked()
            {
                command(
                    effects,
                    "ui.trafficDeleteHost",
                    json!({"host":self.flow.traffic_host}),
                    pinned.clone(),
                );
            }
            if ui
                .add_enabled(
                    self.flow.traffic_delete_confirm,
                    egui::Button::new("전체 캡처 삭제"),
                )
                .clicked()
            {
                command(
                    effects,
                    "ui.trafficDeleteAll",
                    json!({}),
                    traffic_versions(snapshot, ""),
                );
            }
        });
        if let Some(error) = &self.flow.error {
            ui.colored_label(ui.visuals().warn_fg_color, error);
        }
    }
}

fn choice(ui: &mut egui::Ui, id: &str, label: &str, value: &mut String, items: &[&str]) {
    ui.label(label);
    egui::ComboBox::from_id_salt(id)
        .selected_text(if value.is_empty() {
            "전체 / 기본값"
        } else {
            value.as_str()
        })
        .show_ui(ui, |ui| {
            for item in items {
                ui.selectable_value(
                    value,
                    (*item).into(),
                    if item.is_empty() {
                        "전체 / 기본값"
                    } else {
                        item
                    },
                );
            }
        });
}
fn get_value<'a>(root: &'a Value, path: &[String]) -> Option<&'a Value> {
    let mut value = root;
    for key in path {
        value = if value.is_array() {
            value.get(key.parse::<usize>().ok()?)?
        } else {
            value.get(key)?
        };
    }
    Some(value)
}
fn put_value(root: &mut Value, path: &[String], value: Value) -> bool {
    if path.is_empty() {
        *root = value;
        return true;
    }
    let mut target = root;
    for key in &path[..path.len() - 1] {
        let next = if target.is_array() {
            key.parse::<usize>().ok().and_then(|i| target.get_mut(i))
        } else {
            target.get_mut(key)
        };
        let Some(next) = next else {
            return false;
        };
        target = next;
    }
    let key = path.last().unwrap();
    if target.is_array() {
        if let Some(slot) = key.parse::<usize>().ok().and_then(|i| target.get_mut(i)) {
            *slot = value;
            return true;
        }
    } else if let Some(slot) = target.get_mut(key) {
        *slot = value;
        return true;
    }
    false
}
fn traffic_versions(s: &Snapshot, host: &str) -> Vec<Expected> {
    let ids = s
        .records("exchanges")
        .into_iter()
        .filter(|r| host.is_empty() || r.data["host"] == host)
        .map(|r| r.header.id.clone())
        .collect::<Vec<_>>();
    s.records("exchanges")
        .into_iter()
        .chain(s.records("exchange_bodies"))
        .filter(|r| {
            ids.contains(&r.header.id)
                || r.data["id"]
                    .as_str()
                    .is_some_and(|id| ids.iter().any(|v| v == id))
        })
        .map(expected)
        .collect()
}
impl Panes {
    pub fn skill_import(
        &mut self,
        ui: &mut egui::Ui,
        selected: Option<&str>,
        effects: &mut Vec<Effect>,
    ) {
        ui.horizontal_wrapped(|ui| {
            ui.checkbox(
                &mut self.flow.skill_overwrite,
                "ZIP에 포함된 기존 스킬 덮어쓰기 허용",
            );
            if ui.button("스킬 ZIP 가져오기").clicked() {
                self.staged_request(
                    effects,
                    "skill-import",
                    "ui.uploadSkill",
                    json!({"overwrite":self.flow.skill_overwrite}),
                    None,
                );
            }
        });
        if let Some(name) = selected {
            ui.weak(format!(
                "현재 선택: {name}. ZIP 안의 스킬 이름으로 가져옵니다."
            ));
        }
        ui.weak("ZIP 루트 또는 하나의 최상위 폴더 안에 SKILL.md가 있어야 합니다.");
        if let Some(result) = self.projection.get("ui.uploadSkill") {
            ui.label(format!("가져온 스킬: {}", text(result, "name")));
            ui.weak("최신 목록과 메타데이터는 저장 데이터 다시 불러오기로 확인하세요.");
        }
    }
    pub fn sync(
        &mut self,
        ui: &mut egui::Ui,
        snapshot: &Snapshot,
        _item: Option<&Record>,
        effects: &mut Vec<Effect>,
    ) {
        ui.heading("ScopeSentry 자산 동기화");
        if self.flow.sync_read_versions.is_none() {
            self.flow.sync_read_versions = Some(config_sync_versions(snapshot));
            if let Some(server) = snapshot
                .records("mcp_servers")
                .into_iter()
                .find(|m| m.data["name"] == "ScopeSentry")
            {
                self.flow.sync_url = text(&server.data, "url").into();
            }
        }
        let baseline = self.flow.sync_read_versions.clone().unwrap_or_default();
        let stale = baseline.iter().any(|e| {
            snapshot
                .get(&e.collection, &e.id)
                .is_none_or(|r| r.header.version != e.version)
        });
        ui.horizontal_wrapped(|ui| {
            if ui.button("연결 상태 조회").clicked() {
                self.request(effects, "ui.ssStatus", json!({}), false);
            }
            if ui.button("설정·귀속 버전 다시 불러오기").clicked() {
                self.flow.sync_read_versions = None;
                self.projection.remove("ui.ssStatus");
            }
        });
        if let Some(status) = self.projection.get("ui.ssStatus") {
            ui.label(format!(
                "설정 {} · 활성 {} · 연결 {}",
                status["configured"], status["enabled"], status["reachable"]
            ));
            if let Some(error) = status["error"].as_str() {
                ui.colored_label(ui.visuals().warn_fg_color, error);
            }
        }
        ui.collapsing("연결 설정",|ui|{crate::theme::text_field(ui, "MCP HTTP 주소", &mut self.flow.sync_url);ui.label("API 키는 MCP 설정의 ScopeSentry 환경 변수 X-API-Key에 OS 비밀 보관소 참조로 저장하세요.");if ui.add_enabled(!stale&&!self.flow.sync_url.trim().is_empty(),egui::Button::new("연결 주소 저장 / 데이터 소스 만들기")).clicked(){command(effects,"ui.ssDatasource",json!({"body":{"url":self.flow.sync_url}}),baseline.clone());}if let Some(server)=snapshot.records("mcp_servers").into_iter().find(|m|m.data["name"]=="ScopeSentry"){if ui.button("MCP 연결·비밀 값 설정 열기").clicked(){effects.push(Effect::Select{section:Section::Mcp,id:server.header.id.clone()});}}});
        if stale {
            ui.colored_label(ui.visuals().warn_fg_color,"설정이나 자산 귀속이 변경되었습니다. 버전을 명시적으로 다시 불러온 뒤 동기화하세요.");
        }
        let old_dimension = self.flow.sync_dimension.clone();
        ui.horizontal_wrapped(|ui| {
            choice(
                ui,
                "sync-dimension",
                "대상 종류",
                &mut self.flow.sync_dimension,
                &["project", "task"],
            );
            crate::theme::text_field(ui, "검색", &mut self.sync_search);
        });
        if old_dimension != self.flow.sync_dimension {
            self.flow.sync_targets.clear();
            self.flow.sync_page = 1;
        }
        let method = if self.flow.sync_dimension == "project" {
            "ui.ssProjects"
        } else {
            "ui.ssTasks"
        };
        let mut fetch = false;
        ui.horizontal_wrapped(|ui| {
            if ui
                .add_enabled(self.flow.sync_page > 1, egui::Button::new("이전"))
                .clicked()
            {
                self.flow.sync_page -= 1;
                fetch = true;
            }
            ui.label(format!("페이지 {}", self.flow.sync_page));
            if ui.button("다음").clicked() {
                self.flow.sync_page += 1;
                fetch = true;
            }
            ui.add(egui::DragValue::new(&mut self.flow.sync_size).range(1..=200));
            if ui.button("대상 조회").clicked() {
                self.flow.sync_page = 1;
                fetch = true;
            }
        });
        if fetch {
            self.request(effects,method,json!({"page":self.flow.sync_page,"size":self.flow.sync_size,"search":self.sync_search}),false);
        }
        if let Some(result) = self.projection.get(method).cloned() {
            let rows = result.as_array().or_else(|| {
                result[if method == "ui.ssProjects" {
                    "projects"
                } else {
                    "tasks"
                }]
                .as_array()
            });
            for row in rows.into_iter().flatten() {
                let id = text(row, "id");
                let mut selected = self.flow.sync_targets.iter().any(|v| v == id);
                if ui
                    .checkbox(&mut selected, format!("{} · {}", text(row, "name"), id))
                    .changed()
                {
                    self.flow.sync_targets.retain(|v| v != id);
                    if selected && self.flow.sync_targets.len() < 100 {
                        self.flow.sync_targets.push(id.into());
                    }
                }
            }
        }
        ui.label(format!("선택한 대상 {}개", self.flow.sync_targets.len()));
        ui.horizontal_wrapped(|ui| {
            for (kind, label) in [
                ("subdomain", "하위 도메인"),
                ("service", "서비스"),
                ("app", "앱"),
            ] {
                let mut enabled = self.flow.sync_types.iter().any(|v| v == kind);
                if ui.checkbox(&mut enabled, label).changed() {
                    self.flow.sync_types.retain(|v| v != kind);
                    if enabled {
                        self.flow.sync_types.push(kind.into());
                    }
                }
            }
            ui.checkbox(&mut self.flow.sync_company, "프로젝트 회사를 생성");
        });
        if ui
            .add_enabled(
                !stale && !self.flow.sync_targets.is_empty() && !self.flow.sync_types.is_empty(),
                egui::Button::new("선택한 대상의 실제 자산 동기화 실행"),
            )
            .clicked()
        {
            command(
                effects,
                "ui.ssSync",
                json!({"body":{"dimension":self.flow.sync_dimension,"targets":self.flow.sync_targets,"asset_types":self.flow.sync_types,"create_company":self.flow.sync_company,"page_size":self.flow.sync_size}}),
                baseline,
            );
        }
        if let Some(result) = self.projection.get("ui.ssSync") {
            ui.strong("동기화 결과");
            for (kind, count) in result["synced"].as_object().into_iter().flatten() {
                ui.label(format!("{kind}: {count}"));
            }
            for (name, title) in [
                ("companies", "생성된 회사"),
                ("warnings", "주의"),
                ("errors", "오류"),
            ] {
                for value in result[name].as_array().into_iter().flatten() {
                    ui.label(format!("{title}: {}", value.as_str().unwrap_or("")));
                }
            }
        }
        ui.separator();
        ui.strong("자료 가져오기와 내보내기");
        if ui
            .button("스킬 ZIP 파일 가져오기 (기존 항목 덮어쓰기 안 함)")
            .clicked()
        {
            self.staged_request(
                effects,
                "skill-import",
                "ui.uploadSkill",
                json!({"overwrite":false}),
                None,
            );
        }
        choice(
            ui,
            "export-findings-format",
            "발견 사항 파일 형식",
            &mut self.flow.export_format,
            &["json", "csv", "md-single", "md-zip"],
        );
        if ui.button("전체 발견 사항 내보내기 준비").clicked() {
            self.request(
                effects,
                "ui.exportFindings",
                json!({"opts":{"format":self.flow.export_format,"scope":"all"}}),
                false,
            );
        }
        self.download_projection(ui, effects, "ui.exportFindings");
        if let Some(error) = &self.flow.error {
            ui.colored_label(ui.visuals().warn_fg_color, error);
        }
    }
    pub fn logs(
        &mut self,
        ui: &mut egui::Ui,
        snapshot: &Snapshot,
        item: Option<&Record>,
        effects: &mut Vec<Effect>,
    ) {
        ui.heading("활동 · 진단 · LLM 기록");
        ui.horizontal_wrapped(|ui| {
            for (index, label) in ["작업 활동", "앱 진단", "LLM 요청·응답"].iter().enumerate()
            {
                ui.selectable_value(&mut self.flow.logs_tab, index, *label);
            }
        });
        match self.flow.logs_tab {
            0 => {
                self.activities(ui, snapshot, None);
                if let Some(activity) = item {
                    if ui.button("선택 활동 전체 내용 조회").clicked() {
                        self.request(
                            effects,
                            "ui.activityDetail",
                            json!({"id":activity.header.id}),
                            false,
                        );
                    }
                }
                if self.projection.contains_key("ui.activityDetail") {
                    self.projection_body(
                        ui,
                        effects,
                        "ui.activityDetail",
                        vec!["detail".into()],
                        "활동 내용",
                    );
                }
            }
            1 => {
                if ui.button("진단 이력 조회").clicked() {
                    self.request(
                        effects,
                        "legacy.getLogsHistory",
                        legacy_parameters("legacy.getLogsHistory", json!({"limit":200})),
                        false,
                    );
                }
                if let Some(result) = self.projection.get("legacy.getLogsHistory").cloned() {
                    let rows = result["items"].as_array();
                    if result["has_more"] == true && ui.button("이전 진단 더 불러오기").clicked()
                    {
                        if let Some(first) = rows.and_then(|v| v.first()) {
                            self.request(
                                effects,
                                "legacy.getLogsHistory",
                                legacy_parameters(
                                    "legacy.getLogsHistory",
                                    json!({"before":first["db_id"],"limit":200}),
                                ),
                                true,
                            );
                        }
                    }
                    for row in rows.into_iter().flatten() {
                        ui.horizontal_wrapped(|ui| {
                            ui.small(presentation_time(text(row, "ts")));
                            ui.strong(text(row, "level"));
                            ui.label(text(row, "tag"));
                            ui.label(text(row, "text"));
                        });
                    }
                } else {
                    for row in snapshot.records("server_logs").into_iter().rev().take(200) {
                        ui.label(format!(
                            "{} [{}] {}",
                            presentation_time(text(&row.data, "created_at")),
                            text(&row.data, "level"),
                            text(&row.data, "text")
                        ));
                    }
                }
            }
            _ => {
                ui.horizontal_wrapped(|ui| {
                    crate::theme::text_field(ui, "모델", &mut self.flow.logs_model);
                    crate::theme::text_field(ui, "세션", &mut self.flow.logs_session);
                    crate::theme::text_field(ui, "작업 ID", &mut self.flow.logs_task);
                });
                let mut load = false;
                ui.horizontal_wrapped(|ui| {
                    if ui
                        .add_enabled(self.flow.logs_page > 0, egui::Button::new("이전"))
                        .clicked()
                    {
                        self.flow.logs_page -= 1;
                        load = true;
                    }
                    ui.label(format!("페이지 {}", self.flow.logs_page + 1));
                    if ui.button("다음").clicked() {
                        self.flow.logs_page += 1;
                        load = true;
                    }
                    ui.add(egui::DragValue::new(&mut self.flow.logs_size).range(1..=200));
                    if ui.button("LLM 기록 조회").clicked() {
                        self.flow.logs_page = 0;
                        load = true;
                    }
                });
                if load {
                    self.request(effects,"ui.llmRecords",json!({"params":{"model":self.flow.logs_model,"session":self.flow.logs_session,"task":self.flow.logs_task,"page":self.flow.logs_page,"size":self.flow.logs_size}}),false);
                }
                if let Some(result) = self.projection.get("ui.llmRecords").cloned() {
                    ui.label(format!("{}개 기록", result["total"]));
                    for row in result["records"].as_array().into_iter().flatten() {
                        ui.horizontal_wrapped(|ui| {
                            ui.label(text(row, "model"));
                            ui.label(format!(
                                "입력 {} / 출력 {}",
                                row["input_tokens"], row["output_tokens"]
                            ));
                            ui.label(text(row, "task_id"));
                            if ui.button("요청·응답 상세").clicked() {
                                self.request(
                                    effects,
                                    "ui.llmRecordDetail",
                                    json!({"id":row["id"]}),
                                    false,
                                );
                            }
                        });
                    }
                }
                if let Some(detail) = self.projection.get("ui.llmRecordDetail").cloned() {
                    for (name, label) in [
                        ("request_body", "요청 본문"),
                        ("response_body", "응답 본문"),
                        ("raw_request", "원본 요청"),
                        ("raw_response", "원본 응답"),
                        ("error", "오류"),
                    ] {
                        if detail.get(name).is_some() {
                            self.projection_body(
                                ui,
                                effects,
                                "ui.llmRecordDetail",
                                vec![name.into()],
                                label,
                            );
                        }
                    }
                }
                ui.separator();
                ui.checkbox(
                    &mut self.flow.logs_confirm,
                    "입력한 작업 ID에 속한 LLM 기록 삭제를 확인합니다",
                );
                if ui
                    .add_enabled(
                        self.flow.logs_confirm && !self.flow.logs_task.trim().is_empty(),
                        egui::Button::new("작업 LLM 기록 삭제"),
                    )
                    .clicked()
                {
                    let task = &self.flow.logs_task;
                    let versions = snapshot
                        .records("llm_records")
                        .into_iter()
                        .filter(|r| r.data["task_id"] == *task)
                        .map(expected)
                        .collect();
                    command(
                        effects,
                        "ui.llmRecordsDeleteTask",
                        json!({"task":task}),
                        versions,
                    );
                }
            }
        }
        if let Some(error) = &self.flow.error {
            ui.colored_label(ui.visuals().warn_fg_color, error);
        }
    }
}
fn config_sync_versions(s: &Snapshot) -> Vec<Expected> {
    [
        "mcp_servers",
        "mcp_tools_cache",
        "assets",
        "companies",
        "company_scope",
    ]
    .iter()
    .flat_map(|c| s.records(c))
    .map(expected)
    .collect()
}
fn projection_id(value: &Value) -> Option<&str> {
    value["seq"]
        .as_str()
        .filter(|v| !v.is_empty() && *v != "0")
        .or_else(|| value["db_id"].as_str())
        .or_else(|| value["id"].as_str())
}
fn legacy_parameters(method: &str, query: Value) -> Value {
    static RPC: std::sync::OnceLock<Value> = std::sync::OnceLock::new();
    let rpc = RPC.get_or_init(|| artex_core::contracts::read("rpc.v1.schema.json"));
    json!({"route_id":rpc["methods"][method]["properties"]["route_id"]["enum"][0],"path":{},"query":query,"body":null})
}
impl Panes {
    fn finding_evidence(&mut self, ui: &mut egui::Ui, finding: &str, effects: &mut Vec<Effect>) {
        if self.flow.evidence_finding != finding {
            self.flow.evidence_finding = finding.into();
            self.projection.remove("ui.findingTraffic");
            self.projection.remove("ui.findingTrafficBody");
            self.projection.remove("ui.downloadFindingTrafficBody");
        }
        if let Some(result) = self.projection.get("ui.findingTraffic").cloned() {
            ui.strong(format!(
                "증거 버전 {} · 보고서 버전 {}",
                result["version"], result["report_version"]
            ));
            for binding in result["bindings"].as_array().into_iter().flatten() {
                ui.group(|ui| {
                    ui.label(format!(
                        "{} {}",
                        text(binding, "title"),
                        text(binding, "note")
                    ));
                    let id = text(binding, "id");
                    ui.horizontal_wrapped(|ui| {
                        for (side, label) in [("request", "요청"), ("response", "응답")] {
                            if ui.button(format!("{label} 미리보기")).clicked() {
                                self.flow.evidence_binding = id.into();
                                self.flow.evidence_side = side.into();
                                self.request(
                                    effects,
                                    "ui.findingTrafficBody",
                                    json!({"id":finding,"bindingId":id,"side":side,"offset":0}),
                                    false,
                                );
                            }
                            if ui.button(format!("{label} 전체 내보내기 준비")).clicked() {
                                self.request(
                                    effects,
                                    "ui.downloadFindingTrafficBody",
                                    json!({"id":finding,"bindingId":id,"side":side}),
                                    false,
                                );
                            }
                        }
                    });
                });
            }
        }
        if let Some(preview) = self.projection.get("ui.findingTrafficBody").cloned() {
            ui.label(format!(
                "{} · {} / {} bytes{}",
                self.flow.evidence_side,
                preview["next_offset"],
                preview["total"],
                if preview["binary"] == true {
                    " (바이너리: 전체 파일로 내보내세요)"
                } else {
                    ""
                }
            ));
            let mut text = body_text(&preview["content"]);
            ui.add(
                egui::TextEdit::multiline(&mut text)
                    .interactive(false)
                    .code_editor()
                    .desired_rows(12)
                    .desired_width(f32::INFINITY),
            );
            if preview["truncated"] == true && ui.button("다음 8KiB 미리보기").clicked() {
                self.request(effects,"ui.findingTrafficBody",json!({"id":finding,"bindingId":self.flow.evidence_binding,"side":self.flow.evidence_side,"offset":preview["next_offset"]}),false);
            }
        }
        self.download_projection(ui, effects, "ui.downloadFindingTrafficBody");
        choice(
            ui,
            "finding-export-format",
            "발견 사항 파일 형식",
            &mut self.flow.export_format,
            &["json", "csv", "md-single", "md-zip"],
        );
        if ui.button("이 발견 사항 내보내기 준비").clicked() {
            self.request(effects,"ui.exportFindings",json!({"opts":{"format":self.flow.export_format,"scope":"selected","ids":[finding]}}),false);
        }
        self.download_projection(ui, effects, "ui.exportFindings");
    }
}

#[cfg(test)]
fn expected_send_button(ctx: &egui::Context) -> &'static str {
    if ctx.os() == egui::os::OperatingSystem::Mac {
        "보내기 · ⌘+Enter"
    } else {
        "보내기 · Ctrl+Enter"
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn snapshot() -> Snapshot {
        Snapshot {
            workspace_id: "workspace-fixture".into(),
            commit_seq: 1,
            collections: BTreeMap::new(),
        }
    }
    fn row(table: &str, prefix: &str, data: Value) -> Record {
        Record {
            header: artex_core::Header {
                id: format!("{prefix}_{}", "a".repeat(32)),
                collection: table.into(),
                version: "1".into(),
                commit_seq: "1".into(),
                created_at: "2026-10-13T00:00:00Z".into(),
                updated_at: "2026-10-13T00:00:00Z".into(),
                legacy_key: Value::Null,
            },
            data,
        }
    }
    #[test]
    fn reversed_filter_callbacks_do_not_replace_newer_result_or_revision() {
        let mut p = Panes::default();
        let mut effects = vec![];
        p.request(&mut effects, "ui.traffic", json!({"host":"old"}), false);
        let old = p.flow.latest["ui.traffic"].clone();
        p.request(&mut effects, "ui.traffic", json!({"host":"new"}), false);
        let new = p.flow.latest["ui.traffic"].clone();
        assert!(p.set_projection_for("ui.traffic", &new, json!({"total":2}), vec![]));
        assert!(!p.set_projection_for("ui.traffic", &old, json!({"total":1}), vec![]));
        assert_eq!(p.projection["ui.traffic"]["total"], 2);
    }
    #[test]
    fn cancelled_or_stale_picker_releases_capability_and_never_queues_upload() {
        let mut p = Panes::default();
        p.set_context(Section::Files, None);
        p.file_path = "original".into();
        let mut effects = vec![];
        p.staged_request(
            &mut effects,
            "workspace-upload:original",
            "ui.workspaceUpload",
            json!({"dir":"original"}),
            Some("original".into()),
        );
        let purpose = match &effects[0] {
            Effect::StageFile { purpose } => purpose.clone(),
            _ => panic!(),
        };
        p.file_path = "new-target".into();
        p.set_staged_file(&purpose, json!({"file_capability":"private-file-cap"}));
        let queued = p.drain_effects();
        assert!(matches!(&queued[0],Effect::ReleaseStagedFile{token}if token=="private-file-cap"));
        assert!(!queued.iter().any(|e| matches!(e, Effect::Command { .. })));
    }
    #[test]
    fn active_picker_uses_opaque_file_reference_without_original_absolute_path() {
        let mut p = Panes::default();
        p.set_context(Section::Sync, None);
        let mut effects = vec![];
        p.staged_request(
            &mut effects,
            "skill-import",
            "ui.uploadSkill",
            json!({"overwrite":false}),
            None,
        );
        let purpose = match &effects[0] {
            Effect::StageFile { purpose } => purpose.clone(),
            _ => panic!(),
        };
        p.set_staged_file(
            &purpose,
            json!({"file_capability":"opaque-file-cap","name":"skill.zip"}),
        );
        let queued = p.drain_effects();
        match &queued[0] {
            Effect::Command { method, params, .. } => {
                assert_eq!(method, "ui.uploadSkill");
                assert_eq!(params["file"], "opaque-file-cap");
                assert_eq!(params["overwrite"], false);
            }
            _ => panic!(),
        }
    }
    #[test]
    fn legacy_history_merge_uses_persistent_log_ids_when_source_seq_is_zero() {
        let mut p = Panes::default();
        p.projection.insert(
            "legacy.getLogsHistory".into(),
            json!({"items":[{"seq":"0","db_id":"log_new","text":"new"}]}),
        );
        let mut effects = vec![];
        p.request(
            &mut effects,
            "legacy.getLogsHistory",
            legacy_parameters("legacy.getLogsHistory", json!({"before":"log_new"})),
            true,
        );
        let token = p.flow.latest["legacy.getLogsHistory"].clone();
        p.set_projection_for(
            "legacy.getLogsHistory",
            &token,
            json!({"items":[{"seq":"0","db_id":"log_old","text":"old"}]}),
            vec![],
        );
        assert_eq!(
            p.projection["legacy.getLogsHistory"]["items"]
                .as_array()
                .unwrap()
                .len(),
            2
        );
    }
    #[test]
    fn company_scope_default_is_independent_of_task_subdomain_scope() {
        let mut p = Panes::default();
        p.scope_kind = "subdomain".into();
        assert_eq!(p.flow.company_kind, "domain");
    }
    #[test]
    fn nonempty_waiting_question_session_renders_without_mutating_snapshot() {
        let mut s = snapshot();
        let run = row(
            "agent_runs",
            "run",
            json!({"state":"waiting","task_id":null,"conversation_id":null,"intent_id":null}),
        );
        let question = row(
            "user_questions",
            "question",
            json!({"run_id":run.header.id,"state":"pending","kind":"question","generation":"1","payload":{"questions":[{"question":"Continue?","options":[{"label":"Yes","description":"Continue work"}],"multiSelect":false}]}}),
        );
        s.collections
            .entry("agent_runs".into())
            .or_default()
            .insert(run.header.id.clone(), run.clone());
        s.collections
            .entry("user_questions".into())
            .or_default()
            .insert(question.header.id.clone(), question);
        let original = s.clone();
        let mut p = Panes::default();
        let ctx = egui::Context::default();
        let mut effects = vec![];
        let _ = ctx.run(egui::RawInput::default(), |ctx| {
            egui::CentralPanel::default()
                .show(ctx, |ui| p.sessions(ui, &s, Some(&run), &mut effects));
        });
        assert_eq!(s, original);
        assert_eq!(p.flow.question_drafts.len(), 1);
        assert!(effects.is_empty());
    }

    #[test]
    fn session_tabs_keep_question_draft_and_read_versions_across_actual_selection() {
        let mut s = snapshot();
        let run = row("agent_runs", "run", json!({"state":"waiting"}));
        let question = row(
            "user_questions",
            "question",
            json!({"run_id":run.header.id,"state":"pending","generation":"1","kind":"question","payload":{"questions":[{"question":"Scope","options":[]}]}}),
        );
        for r in [run.clone(), question.clone()] {
            s.collections
                .entry(r.header.collection.clone())
                .or_default()
                .insert(r.header.id.clone(), r);
        }
        let original = s.clone();
        let ctx = egui::Context::default();
        crate::app::configure(&ctx);
        let mut panes = Panes::default();
        let mut effects = vec![];
        panes.flow.question_drafts.insert(
            question.header.id.clone(),
            json!({"answers":{"Scope":"Keep my typed answer"}}),
        );
        let mut render = |events| {
            ctx.run(
                egui::RawInput {
                    screen_rect: Some(egui::Rect::from_min_size(
                        egui::Pos2::ZERO,
                        egui::vec2(680.0, 800.0),
                    )),
                    events,
                    ..Default::default()
                },
                |ctx| {
                    egui::CentralPanel::default()
                        .show(ctx, |ui| panes.sessions(ui, &s, Some(&run), &mut effects));
                },
            )
        };
        let first = render(vec![]);
        let position = |o: &egui::FullOutput, label: &str| {
            o.shapes
                .iter()
                .find_map(|sh| {
                    if let egui::epaint::Shape::Text(t) = &sh.shape {
                        (t.galley.text() == label).then(|| t.visual_bounding_rect().center())
                    } else {
                        None
                    }
                })
                .unwrap_or_else(|| panic!("Missing {label}"))
        };
        let click = |pos| {
            vec![
                egui::Event::PointerMoved(pos),
                egui::Event::PointerButton {
                    pos,
                    button: egui::PointerButton::Primary,
                    pressed: true,
                    modifiers: egui::Modifiers::NONE,
                },
                egui::Event::PointerButton {
                    pos,
                    button: egui::PointerButton::Primary,
                    pressed: false,
                    modifiers: egui::Modifiers::NONE,
                },
            ]
        };
        let history = render(click(position(&first, "활동 이력")));
        assert!(
            history.shapes.iter().any(
                |s| matches!(&s.shape,egui::epaint::Shape::Text(t) if t.galley.text()=="작업")
            ),
            "Existing history controls must remain accessible"
        );
        let back = render(click(position(&history, "실행 개요·승인")));
        assert!(back.shapes.iter().any(|s|matches!(&s.shape,egui::epaint::Shape::Text(t) if t.galley.text()=="Keep my typed answer")));
        drop(render);
        assert_eq!(panes.flow.session_tab, 0);
        assert_eq!(
            panes.flow.question_drafts[&question.header.id]["answers"]["Scope"],
            "Keep my typed answer"
        );
        assert!(
            panes.flow.question_versions[&question.header.id]
                .iter()
                .any(|e| e.id == question.header.id && e.version == "1")
        );
        assert!(effects.is_empty());
        assert_eq!(s, original);
    }
}

#[cfg(test)]
mod draft_lifecycle_tests {
    use super::*;
    #[test]
    fn integrated_composer_keeps_hotkey_attachments_cas_and_other_conversation_draft() {
        let conversation_id = artex_core::new_id("conversation");
        let mut data = artex_core::default_data("conversations").unwrap();
        data["id"] = json!(conversation_id);
        data["title"] = json!("Composer source");
        data["agent_key"] = json!("auto");
        let conversation:Record=serde_json::from_value(json!({"header":{"id":conversation_id,"collection":"conversations","version":"9","commit_seq":"11","created_at":"2026-10-13T00:00:00Z","updated_at":"2026-10-13T00:00:00Z","legacy_key":null},"data":data})).unwrap();
        let mut snapshot = Snapshot {
            workspace_id: artex_core::new_id("workspace"),
            commit_seq: 11,
            collections: BTreeMap::new(),
        };
        snapshot
            .collections
            .entry("conversations".into())
            .or_default()
            .insert(conversation_id.clone(), conversation.clone());
        let original = snapshot.clone();
        let attachment = json!({"id":artex_core::new_id("attachment"),"name":"evidence.txt","path":"attachments/evidence.txt","size":17});
        let mut panes = Panes::default();
        panes.set_context(Section::Chat, Some(&conversation_id));
        panes.draft_message = "Preserved A message".into();
        panes
            .flow
            .attachments
            .insert(conversation_id.clone(), vec![attachment.clone()]);
        let ctx = egui::Context::default();
        crate::app::configure(&ctx);
        let mut effects = vec![];
        let modifiers = egui::Modifiers {
            ctrl: true,
            ..Default::default()
        };
        let _ = ctx.run(
            egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(680.0, 560.0),
                )),
                modifiers,
                events: vec![egui::Event::Key {
                    key: egui::Key::Enter,
                    physical_key: None,
                    pressed: true,
                    repeat: false,
                    modifiers,
                }],
                ..Default::default()
            },
            |ctx| {
                egui::CentralPanel::default().show(ctx, |ui| {
                    panes.chat(ui, &snapshot, Some(&conversation), None, &mut effects)
                });
            },
        );
        let (params, versions) = effects
            .iter()
            .find_map(|e| match e {
                Effect::Command {
                    method,
                    params,
                    expected,
                } if method == "ui.sendConversationMessage" => {
                    Some((params.clone(), expected.clone()))
                }
                _ => None,
            })
            .expect("Ctrl+Enter must submit the existing source command");
        assert_eq!(params["message"], "Preserved A message");
        assert_eq!(params["attachments"], json!([attachment.clone()]));
        assert_eq!(versions, vec![expected(&conversation)]);
        assert_eq!(
            panes.flow.pending_messages[&conversation_id].0,
            "Preserved A message"
        );
        panes.draft_message = "New A input while saving".into();
        panes.set_context(Section::Chat, Some("conversation-B"));
        panes.draft_message = "Untouched B input".into();
        panes.set_command_result_for_params(
            "ui.sendConversationMessage",
            &params,
            Some(&conversation_id),
            json!({"accepted":true}),
        );
        assert_eq!(panes.draft_message, "Untouched B input");
        panes.set_context(Section::Chat, Some(&conversation_id));
        assert_eq!(panes.draft_message, "New A input while saving");
        assert!(panes.flow.attachments[&conversation_id].is_empty());
        assert_eq!(snapshot, original);
    }
    #[test]
    fn actual_approval_click_has_horizontal_actions_and_pending_feedback_for_both_methods() {
        fn record(collection: &str, prefix: &str, patch: Value) -> Record {
            let id = artex_core::new_id(prefix);
            let mut data = artex_core::default_data(collection).unwrap();
            for (key, value) in patch.as_object().unwrap() {
                data[key] = value.clone();
            }
            if data.get("id").is_some() {
                data["id"] = json!(id);
            }
            serde_json::from_value(json!({"header":{"id":id,"collection":collection,"version":"4","commit_seq":"7","created_at":"2026-10-13T00:00:00Z","updated_at":"2026-10-13T00:00:00Z","legacy_key":null},"data":data})).unwrap()
        }
        for method in ["ui.interceptDecide", "user.answer"] {
            for central in [420.0, 820.0] {
                let ctx = egui::Context::default();
                crate::app::configure(&ctx);
                let mut panes = Panes::default();
                let mut effects = vec![];
                let run = record("agent_runs", "run", json!({"state":"waiting"}));
                let pending = if method == "user.answer" {
                    record(
                        "user_questions",
                        "question",
                        json!({"run_id":run.header.id,"kind":"question","state":"pending","generation":"1","payload":{"type":"tool_approval","name":"Read","input":{"file_path":"evidence.txt"}}}),
                    )
                } else {
                    record(
                        "intercept_pending",
                        "approval",
                        json!({"status":"pending","tool_name":"Read","tool_input":{"file_path":"evidence.txt","mode":"strict"}}),
                    )
                };
                let mut snapshot = Snapshot {
                    workspace_id: artex_core::new_id("workspace"),
                    commit_seq: 7,
                    collections: BTreeMap::new(),
                };
                for row in [run.clone(), pending.clone()] {
                    snapshot
                        .collections
                        .entry(row.header.collection.clone())
                        .or_default()
                        .insert(row.header.id.clone(), row);
                }
                let original = snapshot.clone();
                let label = if method == "user.answer" {
                    "승인"
                } else {
                    "허용"
                };
                let mut render = |events: Vec<egui::Event>| {
                    ctx.run(
                        egui::RawInput {
                            screen_rect: Some(egui::Rect::from_min_size(
                                egui::Pos2::ZERO,
                                egui::vec2(central + 16.0, 720.0),
                            )),
                            events,
                            ..Default::default()
                        },
                        |ctx| {
                            egui::CentralPanel::default().show(ctx, |ui| {
                                if method == "user.answer" {
                                    panes.pending_questions(ui, &snapshot, &run, &mut effects);
                                } else {
                                    panes.approvals(ui, Some(&pending), &mut effects);
                                }
                            });
                        },
                    )
                };
                let initial = render(vec![]);
                let rect = |output: &egui::FullOutput, label: &str| {
                    output
                        .shapes
                        .iter()
                        .find_map(|s| {
                            if let egui::epaint::Shape::Text(t) = &s.shape {
                                (t.galley.text() == label).then(|| t.visual_bounding_rect())
                            } else {
                                None
                            }
                        })
                        .unwrap_or_else(|| panic!("Missing {label}"))
                };
                let yes = rect(&initial, label);
                let no = rect(&initial, "거부");
                assert!(
                    (yes.center().y - no.center().y).abs() < 1.0,
                    "Approval actions stacked at {central}px"
                );
                assert!(no.right() < central + 16.0);
                let pos = yes.center();
                let events = || {
                    vec![
                        egui::Event::PointerMoved(pos),
                        egui::Event::PointerButton {
                            pos,
                            button: egui::PointerButton::Primary,
                            pressed: true,
                            modifiers: egui::Modifiers::NONE,
                        },
                        egui::Event::PointerButton {
                            pos,
                            button: egui::PointerButton::Primary,
                            pressed: false,
                            modifiers: egui::Modifiers::NONE,
                        },
                    ]
                };
                let _ = render(events());
                let pending_output = render(events());
                drop(render);
                assert_eq!(
                    effects
                        .iter()
                        .filter(|e| matches!(e, Effect::Command { .. }))
                        .count(),
                    1,
                    "Repeated approval click must be disabled"
                );
                assert!(panes.approval_busy(method, &pending.header.id));
                assert!(pending_output.shapes.iter().any(|s|matches!(&s.shape,egui::epaint::Shape::Text(t) if t.galley.text().contains("처리 중"))));
                assert_eq!(snapshot, original);
            }
        }
    }
    #[test]
    fn approval_submissions_are_immediate_deduplicated_and_original_params_release_only_their_owner()
     {
        let mut panes = Panes::default();
        panes.set_context(Section::Sessions, Some("run-A"));
        panes.flow.question_drafts.insert(
            "question-A".into(),
            json!({"answers":{"Target":"Retained answer"}}),
        );
        let baseline = vec![Expected {
            collection: "user_questions".into(),
            id: "question-A".into(),
            version: "5".into(),
        }];
        let params =
            json!({"question_id":"question-A","generation":"1","answer":{"approved":true}});
        let mut effects = vec![];
        panes.submit_approval(
            &mut effects,
            "user.answer",
            params.clone(),
            baseline.clone(),
        );
        panes.submit_approval(
            &mut effects,
            "user.answer",
            params.clone(),
            baseline.clone(),
        );
        assert_eq!(effects.len(), 1);
        assert!(panes.approval_busy("user.answer", "question-A"));
        assert!(
            matches!(&effects[0],Effect::Command{method,params:p,expected} if method=="user.answer"&&p==&params&&expected==&baseline)
        );
        let other = json!({"id":"approval-B","decision":"allowed"});
        panes.submit_approval(&mut effects, "ui.interceptDecide", other.clone(), vec![]);
        panes.set_context(Section::Approvals, Some("approval-B"));
        panes.set_command_error_for_params(
            "user.answer",
            &params,
            Some("run-A"),
            "VERSION_CONFLICT",
            "Reload the question baseline",
        );
        assert!(!panes.approval_busy("user.answer", "question-A"));
        assert!(panes.approval_busy("ui.interceptDecide", "approval-B"));
        assert_eq!(
            panes.flow.question_drafts["question-A"]["answers"]["Target"],
            "Retained answer"
        );
        assert!(
            panes.flow.error.is_none(),
            "An old owner's error must not replace the current pane message"
        );
        panes.set_command_result_for_params(
            "ui.interceptDecide",
            &other,
            Some("approval-B"),
            json!({"saved":true}),
        );
        let before = effects.len();
        panes.submit_approval(&mut effects, "ui.interceptDecide", other, vec![]);
        assert_eq!(
            effects.len(),
            before,
            "Successful receipt keeps old pending snapshot from resubmitting"
        );
        panes.retire_async_operations();
        assert!(panes.flow.pending_approvals.is_empty());
        assert_eq!(
            panes.flow.question_drafts["question-A"]["answers"]["Target"],
            "Retained answer"
        );
    }
    #[test]
    fn late_a_success_cleans_only_captured_message_and_preserves_new_drafts() {
        let mut p = Panes::default();
        p.set_context(Section::Chat, Some("conversation-A"));
        p.draft_message = "sent A".into();
        p.flow
            .pending_messages
            .insert("conversation-A".into(), ("sent A".into(), vec![]));
        p.draft_message = "new A draft".into();
        p.set_context(Section::Chat, Some("conversation-B"));
        p.draft_message = "B draft".into();
        p.set_command_result(
            "ui.sendConversationMessage",
            Some("conversation-A"),
            json!({"accepted":true}),
        );
        assert_eq!(p.draft_message, "B draft");
        assert!(!p.flow.pending_messages.contains_key("conversation-A"));
        p.set_context(Section::Chat, Some("conversation-A"));
        assert_eq!(p.draft_message, "new A draft");
    }
    #[test]
    fn late_a_failure_unlocks_a_without_changing_b_input() {
        let mut p = Panes::default();
        p.set_context(Section::Chat, Some("conversation-A"));
        p.draft_message = "A retry input".into();
        p.flow
            .pending_messages
            .insert("conversation-A".into(), ("A retry input".into(), vec![]));
        p.set_context(Section::Chat, Some("conversation-B"));
        p.draft_message = "B input".into();
        p.set_command_error_for(
            "ui.sendConversationMessage",
            Some("conversation-A"),
            "BUSY",
            "still running",
        );
        assert_eq!(p.draft_message, "B input");
        assert!(!p.flow.pending_messages.contains_key("conversation-A"));
        p.set_context(Section::Chat, Some("conversation-A"));
        assert_eq!(p.draft_message, "A retry input");
    }
    #[test]
    fn suspension_retires_async_flags_preserves_conversation_file_and_sha_drafts() {
        let mut p = Panes::default();
        p.set_context(Section::Chat, Some("conversation-A"));
        p.draft_message = "unsaved message".into();
        p.file_text = "unsaved file".into();
        p.file_dirty = true;
        p.file_revision = Some("a".repeat(64));
        p.flow
            .pending_messages
            .insert("conversation-A".into(), ("sent old".into(), vec![]));
        p.retire_async_operations();
        assert_eq!(p.draft_message, "unsaved message");
        assert_eq!(p.file_text, "unsaved file");
        assert_eq!(p.file_revision, Some("a".repeat(64)));
        assert!(p.file_dirty && p.file_locked);
        assert!(p.flow.pending_messages.is_empty());
        p.set_context(Section::Chat, Some("conversation-B"));
        p.set_context(Section::Chat, Some("conversation-A"));
        assert_eq!(p.draft_message, "unsaved message");
    }
}

#[cfg(test)]
mod reference_tests {
    use super::*;
    #[test]
    fn reference_tokens_require_the_matching_typed_namespace() {
        let finding = artex_core::new_id("finding");
        let company = artex_core::new_id("company");
        let asset = artex_core::new_id("asset");
        for (collection, id, kind) in [
            ("findings", finding.as_str(), "취약점"),
            ("companies", company.as_str(), "기업"),
            ("assets", asset.as_str(), "자산"),
        ] {
            assert_eq!(
                reference_token(collection, id, "label"),
                Some(format!("@[{kind}#{id} label]"))
            );
        }
        assert!(reference_token("assets", &company, "label").is_none());
        assert!(reference_token("findings", "finding_1", "label").is_none());
        assert!(reference_token("tasks", &artex_core::new_id("task"), "label").is_none());
    }
    #[test]
    fn reference_label_cannot_close_the_token_or_inject_another_reference() {
        let id = artex_core::new_id("asset");
        let source = format!(
            "  hello]\r\n@[기업#{} fake]\t\0 {}",
            artex_core::new_id("company"),
            "한".repeat(140)
        );
        let token = reference_token("assets", &id, &source).unwrap();
        assert_eq!(token.matches('[').count(), 1);
        assert_eq!(token.matches(']').count(), 1);
        assert!(
            !token.contains('\r')
                && !token.contains('\n')
                && !token.contains('\0')
                && !token.contains('\t')
        );
        let label = token.split_once(' ').unwrap().1.strip_suffix(']').unwrap();
        assert_eq!(label.chars().count(), 100);
        assert!(!label.contains("  "));
        assert_eq!(
            reference_token("assets", &id, " \r\n\t"),
            Some(format!("@[자산#{id} {id}]"))
        );
    }
}

#[cfg(test)]
mod workbench_tests {
    use super::*;
    #[test]
    fn outer_scrollbar_width_changes_do_not_restart_composer_reservation_or_leave_footer_gap() {
        let cid = artex_core::new_id("conversation");
        let conversation:Record=serde_json::from_value(json!({"header":{"id":cid,"collection":"conversations","version":"1","commit_seq":"1","created_at":"2026-10-13T00:00:00Z","updated_at":"2026-10-13T00:00:00Z","legacy_key":null},"data":{"id":cid,"title":"Outer composer","agent_key":"auto"}})).unwrap();
        let mut snapshot = Snapshot {
            workspace_id: artex_core::new_id("workspace"),
            commit_seq: 1,
            collections: BTreeMap::new(),
        };
        snapshot
            .collections
            .entry("conversations".into())
            .or_default()
            .insert(cid.clone(), conversation.clone());
        let original = snapshot.clone();
        let ctx = egui::Context::default();
        crate::app::configure(&ctx);
        let mut panes = Panes::default();
        panes.set_context(Section::Chat, Some(&cid));
        panes.draft_message = "Outer draft".into();
        let mut effects = vec![];
        let mut time = 0.0;
        let mut widths = vec![];
        for (size, viewport_height, attached) in [
            (egui::vec2(440.0, 390.0), 350.0, 0),
            (egui::vec2(680.0, 510.0), 450.0, 3),
            (egui::vec2(440.0, 390.0), 350.0, 0),
        ] {
            if attached == 0 {
                panes.flow.attachments.remove(&cid);
            } else {
                panes.flow.attachments.insert(
                    cid.clone(),
                    (0..attached)
                        .map(|i| json!({"name":format!("evidence-{i}.txt"),"size":17}))
                        .collect(),
                );
            }
            for frame_index in 0..12 {
                time += 0.05;
                let mut viewport_bottom = 0.0;
                let output = ctx.run(
                    egui::RawInput {
                        screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, size)),
                        time: Some(time),
                        ..Default::default()
                    },
                    |ctx| {
                        egui::CentralPanel::default().show(ctx, |ui| {
                            // This is the app's short-window wrapper, including its
                            // real scrollbar animation and width allocation.
                            let scroll = egui::ScrollArea::vertical()
                                .id_salt("active_pane")
                                .auto_shrink([false, false])
                                .max_height(viewport_height)
                                .show(ui, |ui| {
                                    ui.set_width(ui.available_width());
                                    panes.chat(
                                        ui,
                                        &snapshot,
                                        Some(&conversation),
                                        None,
                                        &mut effects,
                                    );
                                });
                            viewport_bottom = scroll.inner_rect.bottom();
                        });
                    },
                );
                widths.push(panes.flow.composer_measurement.as_ref().unwrap().width);
                if frame_index >= 10 {
                    let text = |label: &str| {
                        output
                            .shapes
                            .iter()
                            .find_map(|s| {
                                if let egui::epaint::Shape::Text(t) = &s.shape {
                                    (t.galley.text() == label).then(|| t.visual_bounding_rect())
                                } else {
                                    None
                                }
                            })
                            .unwrap_or_else(|| panic!("Missing {label}"))
                    };
                    let draft = text("Outer draft");
                    let send = text(expected_send_button(&ctx));
                    let composer = output
                        .shapes
                        .iter()
                        .find_map(|s| {
                            if let egui::epaint::Shape::Rect(r) = &s.shape {
                                (r.corner_radius.nw >= 8
                                    && r.rect.contains(draft.center())
                                    && r.rect.contains(send.center()))
                                .then_some(r.rect)
                            } else {
                                None
                            }
                        })
                        .expect("Missing shared composer surface");
                    assert!(
                        (composer.bottom() - viewport_bottom).abs() <= 1.0,
                        "Outer-scroll composer reservation did not converge at {size:?}: {} vs {viewport_bottom}",
                        composer.bottom()
                    );
                    assert!(send.bottom() < viewport_bottom && draft.bottom() < viewport_bottom);
                }
            }
        }
        assert!(
            widths[..12]
                .iter()
                .copied()
                .fold(f32::NEG_INFINITY, f32::max)
                - widths[..12].iter().copied().fold(f32::INFINITY, f32::min)
                > 1.0,
            "Fixture must exercise a real first-frame overflow scrollbar width change"
        );
        assert_eq!(panes.draft_message, "Outer draft");
        assert_eq!(snapshot, original);
        assert!(effects.is_empty());
    }
    #[test]
    fn measured_composer_tracks_pane_bottom_with_empty_history_attachments_and_resize() {
        let cid = artex_core::new_id("conversation");
        let conversation:Record=serde_json::from_value(json!({"header":{"id":cid,"collection":"conversations","version":"3","commit_seq":"3","created_at":"2026-10-13T00:00:00Z","updated_at":"2026-10-13T00:00:00Z","legacy_key":null},"data":{"id":cid,"title":"Measured composer","agent_key":"auto"}})).unwrap();
        let activity_id = artex_core::new_id("conversationactivity");
        let activity:Record=serde_json::from_value(json!({"header":{"id":activity_id,"collection":"conversation_activities","version":"1","commit_seq":"4","created_at":"2026-10-13T00:00:00Z","updated_at":"2026-10-13T00:00:00Z","legacy_key":null},"data":{"id":activity_id,"conversation_id":cid,"kind":"text","worker":"auto","detail":{"text":"Stored message in transcript","blob":null}}})).unwrap();
        let mut snapshot = Snapshot {
            workspace_id: artex_core::new_id("workspace"),
            commit_seq: 4,
            collections: BTreeMap::new(),
        };
        snapshot
            .collections
            .entry("conversations".into())
            .or_default()
            .insert(cid.clone(), conversation.clone());
        let mut panes = Panes::default();
        panes.set_context(Section::Chat, Some(&cid));
        panes.draft_message = "Geometry draft".into();
        let ctx = egui::Context::default();
        crate::app::configure(&ctx);
        let mut effects = vec![];
        for (size, attached, populated) in [
            (egui::vec2(680.0, 560.0), 0, false),
            (egui::vec2(680.0, 560.0), 3, true),
            (egui::vec2(440.0, 430.0), 5, true),
            (egui::vec2(1150.0, 700.0), 5, true),
            (egui::vec2(440.0, 430.0), 0, false),
        ] {
            if populated {
                snapshot
                    .collections
                    .entry("conversation_activities".into())
                    .or_default()
                    .insert(activity_id.clone(), activity.clone());
            } else {
                snapshot.collections.remove("conversation_activities");
            }
            let attachments = (0..attached)
                .map(|i| json!({"name":format!("evidence-{i}.txt"),"size":17}))
                .collect::<Vec<_>>();
            panes
                .flow
                .attachments
                .insert(cid.clone(), attachments.clone());
            let before = snapshot.clone();
            let mut last = None;
            let mut pane_bottom = 0.0;
            // Each frame runs the real controls exactly once. Layout convergence
            // uses repaint, not a second hidden render with duplicate effects.
            for _ in 0..3 {
                last = Some(ctx.run(
                    egui::RawInput {
                        screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, size)),
                        ..Default::default()
                    },
                    |ctx| {
                        egui::CentralPanel::default().show(ctx, |ui| {
                            pane_bottom = ui.available_rect_before_wrap().bottom();
                            panes.chat(ui, &snapshot, Some(&conversation), None, &mut effects);
                        });
                    },
                ));
            }
            let output = last.unwrap();
            let text_rect = |label: &str| {
                output
                    .shapes
                    .iter()
                    .find_map(|s| {
                        if let egui::epaint::Shape::Text(t) = &s.shape {
                            (t.galley.text() == label).then(|| t.visual_bounding_rect())
                        } else {
                            None
                        }
                    })
                    .unwrap_or_else(|| panic!("Missing {label}"))
            };
            let draft = text_rect("Geometry draft");
            let send = text_rect(expected_send_button(&ctx));
            let frame = output
                .shapes
                .iter()
                .find_map(|s| {
                    if let egui::epaint::Shape::Rect(r) = &s.shape {
                        (r.corner_radius.nw >= 8
                            && r.rect.contains(draft.center())
                            && r.rect.contains(send.center()))
                        .then_some(r.rect)
                    } else {
                        None
                    }
                })
                .expect("Composer controls must share one actual surface");
            assert!(
                (frame.bottom() - pane_bottom).abs() <= 1.0,
                "Composer left a footer gap at {size:?}: bottom {} vs pane {pane_bottom}",
                frame.bottom()
            );
            assert!(
                send.bottom() < pane_bottom && draft.bottom() < pane_bottom,
                "Composer controls clipped at {size:?}"
            );
            assert_eq!(panes.draft_message, "Geometry draft");
            assert_eq!(panes.flow.attachments[&cid], attachments);
            assert_eq!(snapshot, before);
            assert!(
                effects.is_empty(),
                "Layout measurement must not emit source effects"
            );
        }
    }
    #[test]
    fn readonly_long_request_paths_wrap_inside_the_card_without_losing_values() {
        let path = format!(
            "work/{}evidence.txt",
            "long-directory-without-spaces/".repeat(20)
        );
        let input = json!({"file_path":path,"mode":"strict","count":17});
        let original = input.clone();
        for width in [420.0, 820.0] {
            let ctx = egui::Context::default();
            crate::app::configure(&ctx);
            let mut right = 0.0;
            let output = ctx.run(
                egui::RawInput {
                    screen_rect: Some(egui::Rect::from_min_size(
                        egui::Pos2::ZERO,
                        egui::vec2(width, 1000.0),
                    )),
                    ..Default::default()
                },
                |ctx| {
                    egui::CentralPanel::default().show(ctx, |ui| {
                        right = ui.available_rect_before_wrap().right();
                        crate::theme::group(ui, "실행 요청", |ui| read_value(ui, &input));
                    });
                },
            );
            for value in [path.as_str(), "strict", "17"] {
                let shape=output.shapes.iter().find(|s|matches!(&s.shape,egui::epaint::Shape::Text(t) if t.galley.text()==value)).unwrap_or_else(||panic!("Missing complete request value {value}"));
                assert!(
                    shape.shape.visual_bounding_rect().right() <= right + 1.0,
                    "Request text crossed its {width}px pane boundary"
                );
                assert!(
                    shape
                        .clip_rect
                        .contains_rect(shape.shape.visual_bounding_rect()),
                    "Request value was clipped instead of wrapped"
                );
            }
            assert_eq!(input, original);
        }
    }
    #[test]
    fn audit_messages_show_human_state_and_keep_all_fields_in_expandable_details() {
        let audit = json!({"run_id":"run-audit","guard_audit":{"action":"allow","tool":"Read","command":"cat evidence.txt","reason":"Policy permits read","ts":1700000000,"custom_metadata":"preserved audit metadata"}});
        let data =
            json!({"kind":"result","tool":"Read","detail":{"text":audit.to_string(),"blob":null}});
        assert!(
            matches!(&transcript_parts(&data)[0],TranscriptPart::Audit{value,..} if value==&audit)
        );
        for (expanded, kind) in [(false, "result"), (true, "text")] {
            let ctx = egui::Context::default();
            crate::app::configure(&ctx);
            ctx.memory_mut(|m| m.set_everything_is_visible(expanded));
            let mut record = data.clone();
            record["kind"] = json!(kind);
            let output = ctx.run(
                egui::RawInput {
                    screen_rect: Some(egui::Rect::from_min_size(
                        egui::Pos2::ZERO,
                        egui::vec2(1000.0, 900.0),
                    )),
                    ..Default::default()
                },
                |ctx| {
                    egui::CentralPanel::default().show(ctx, |ui| {
                        message_ui(ui, "audit", &record, "2026-10-13T00:00:00Z")
                    });
                },
            );
            let has = |label: &str| {
                output.shapes.iter().any(
                    |s| matches!(&s.shape,egui::epaint::Shape::Text(t) if t.galley.text()==label),
                )
            };
            assert!(has("(Read) 실행 허용"));
            assert!(has("실행 승인 기록"));
            for label in [
                "run-audit",
                "cat evidence.txt",
                "Policy permits read",
                "1700000000",
                "preserved audit metadata",
            ] {
                assert_eq!(
                    has(label),
                    expanded,
                    "Incorrect audit detail visibility for {label}"
                );
            }
            assert!(!has(&audit.to_string()));
        }
        let mut unknown = audit.clone();
        unknown["guard_audit"]["action"] = json!("hold-for-owner");
        let parts = transcript_parts(
            &json!({"kind":"text","detail":{"text":unknown.to_string(),"blob":null}}),
        );
        assert!(
            matches!(&parts[0],TranscriptPart::Audit{title,value} if title=="(Read) 실행 상태: hold-for-owner"&&value==&unknown)
        );
    }
    #[test]
    fn structured_messages_preserve_text_and_tool_result_blocks_without_internal_config() {
        let input = json!({"command":"printf result"});
        let result = json!([{"type":"text","text":"first"},{"type":"text","text":"second"}]);
        let envelope = json!({"message":{"role":"assistant","content":[{"type":"text","text":"Actual answer"},{"type":"thinking","thinking":"private reasoning"},{"type":"tool_use","name":"Bash","input":input},{"type":"tool_result","content":result,"is_error":true}]}});
        let data =
            json!({"kind":"text","tool":"Bash","detail":{"text":envelope.to_string(),"blob":null}});
        let parts = transcript_parts(&data);
        assert_eq!(parts[0], TranscriptPart::Text("Actual answer".into()));
        assert_eq!(
            parts[1],
            TranscriptPart::Thinking("private reasoning".into())
        );
        assert_eq!(
            parts[2],
            TranscriptPart::Tool {
                name: "Bash".into(),
                value: input,
                result: false,
                error: false
            }
        );
        assert_eq!(
            parts[3],
            TranscriptPart::Tool {
                name: "Bash".into(),
                value: result.clone(),
                result: true,
                error: true
            }
        );
        assert_eq!(content_text(&result), Some("first\nsecond".into()));
        let mut internal = data.clone();
        internal["kind"] = json!("context_config");
        assert!(transcript_parts(&internal).is_empty());
        let partial = json!({"kind":"text","summary":"short","detail":{"text":json!({"stream":{"partial":true},"text":"received partial text"}).to_string(),"blob":null}});
        assert_eq!(
            transcript_parts(&partial),
            vec![TranscriptPart::Text("received partial text".into())]
        );
    }
    #[test]
    fn expanded_tool_views_render_complete_inputs_mixed_resources_and_standalone_metadata() {
        let input = json!({"text":"hello","mode":"strict","target":"file"});
        let result = json!([{"type":"text","text":"ok"},{"type":"resource","resource":{"uri":"resource://evidence/test","mimeType":"text/plain"}}]);
        let message = json!({"message":{"content":[{"type":"tool_use","name":"Custom","input":input},{"type":"tool_result","content":result}]}});
        let nested = json!({"kind":"text","worker":"worker","tool":"Custom","detail":{"text":message.to_string(),"blob":null}});
        let standalone = json!({"kind":"tool_use","tool":"Custom","detail":{"text":input.to_string(),"blob":null}});
        assert!(
            matches!(&transcript_parts(&standalone)[0],TranscriptPart::Tool{value,..} if value==&input)
        );
        assert_eq!(content_text(&result), None);
        assert_eq!(
            content_text(&json!({"type":"text","text":"ok","annotations":{"audience":["user"]}})),
            None
        );
        let ctx = egui::Context::default();
        crate::app::configure(&ctx);
        ctx.memory_mut(|m| m.set_everything_is_visible(true));
        let output = ctx.run(
            egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(1000.0, 900.0),
                )),
                ..Default::default()
            },
            |ctx| {
                egui::CentralPanel::default().show(ctx, |ui| {
                    message_ui(ui, "nested", &nested, "2026-10-13T00:00:00Z");
                    message_ui(ui, "standalone", &standalone, "2026-10-13T00:00:00Z");
                });
            },
        );
        for label in ["strict", "file", "resource://evidence/test", "text/plain"] {
            assert!(
                output.shapes.iter().any(
                    |s| matches!(&s.shape,egui::epaint::Shape::Text(t) if t.galley.text()==label)
                ),
                "Missing structured tool metadata {label}"
            );
        }
        for label in ["strict", "file"] {
            assert!(output.shapes.iter().filter(|s|matches!(&s.shape,egui::epaint::Shape::Text(t) if t.galley.text()==label)).count()>=2,"Standalone tool input lost {label}");
        }
    }
    #[test]
    fn recent_records_use_numeric_commit_ties_and_live_records_exclude_archives() {
        let record = |seq: &str, archived: Value| -> Record {
            serde_json::from_value(json!({"header":{"id":artex_core::new_id("task"),"collection":"tasks","version":seq,"commit_seq":seq,"created_at":"2026-10-13T00:00:00Z","updated_at":"2026-10-13T00:00:00Z","legacy_key":null},"data":{"deleted_at":null,"archived_at":archived}})).unwrap()
        };
        let old = record("9", Value::Null);
        let newer = record("10", Value::Null);
        let archived = record("11", json!("2026-10-13T00:00:01Z"));
        let mut rows = vec![&old, &archived, &newer]
            .into_iter()
            .filter(|r| live_task(r))
            .collect();
        recent_first(&mut rows);
        assert_eq!(
            rows.iter()
                .map(|r| r.header.commit_seq.as_str())
                .collect::<Vec<_>>(),
            vec!["10", "9"]
        );
    }
    #[test]
    fn populated_task_tabs_and_chat_keep_controls_visible_in_small_and_large_native_frames() {
        let make = |collection: &str, patch: Value| -> Record {
            let id = artex_core::new_id(artex_core::collection_prefix(collection).unwrap());
            let mut data = artex_core::default_data(collection).unwrap();
            for (key, value) in patch.as_object().unwrap() {
                data[key] = value.clone();
            }
            if data.get("id").is_some() {
                data["id"] = json!(id);
            }
            serde_json::from_value(json!({"header":{"id":id,"collection":collection,"version":"1","commit_seq":"1","created_at":"2026-10-13T00:00:00Z","updated_at":"2026-10-13T00:00:00Z","legacy_key":null},"data":data})).unwrap()
        };
        let exploration = artex_core::new_id("exploration");
        let task = make(
            "tasks",
            json!({"name":"Stored task","status":"running","description":"Stored engagement","goal":"Stored goal","exploration_id":exploration}),
        );
        let agent = make("agents", json!({"key":"worker","name":"Stored agent"}));
        let conversation = make(
            "conversations",
            json!({"title":"Stored conversation","agent_key":"worker"}),
        );
        let run = make(
            "agent_runs",
            json!({"state":"running","task_id":task.header.id,"agent_id":agent.header.id,"conversation_id":conversation.header.id}),
        );
        let finding = make(
            "findings",
            json!({"name":"Stored finding","task_id":task.header.id,"exploration_id":exploration,"status":"confirmed","severity":"medium"}),
        );
        let asset = make(
            "assets",
            json!({"type":"subdomain","domain":"example.test"}),
        );
        let link = make(
            "task_asset_links",
            json!({"task_id":task.header.id,"asset_id":asset.header.id}),
        );
        let evidence = make(
            "finding_traffic_bindings",
            json!({"finding_id":finding.header.id,"label":"Stored evidence"}),
        );
        let content = json!({"message":{"role":"assistant","content":[{"type":"text","text":"Actual stored answer"},{"type":"tool_use","name":"Read","input":{"file_path":"notes.txt"}},{"type":"tool_result","content":[{"type":"text","text":"Actual stored output"}]}]}});
        let activity = make(
            "conversation_activities",
            json!({"conversation_id":conversation.header.id,"kind":"text","worker":"worker","tool":"Read","summary":"Actual stored answer","detail":{"text":content.to_string(),"blob":null}}),
        );
        let mut snapshot = Snapshot {
            workspace_id: artex_core::new_id("workspace"),
            commit_seq: 1,
            collections: BTreeMap::new(),
        };
        for record in [
            task.clone(),
            agent,
            conversation.clone(),
            run,
            finding,
            asset,
            link,
            evidence,
            activity,
        ] {
            snapshot
                .collections
                .entry(record.header.collection.clone())
                .or_default()
                .insert(record.header.id.clone(), record);
        }
        for index in 0..29 {
            let candidate = make(
                "assets",
                json!({"type":"subdomain","domain":format!("reference-{index}.test")}),
            );
            snapshot
                .collections
                .entry("assets".into())
                .or_default()
                .insert(candidate.header.id.clone(), candidate);
        }
        let original = serde_json::to_value(&snapshot).unwrap();
        let visible = |output: &egui::FullOutput, label: &str| {
            output.shapes.iter().any(|shape| {
                matches!(&shape.shape,egui::epaint::Shape::Text(t) if t.galley.text()==label)
                    && shape
                        .clip_rect
                        .expand(2.0)
                        .contains_rect(shape.shape.visual_bounding_rect())
            })
        };
        for size in [
            egui::vec2(440.0, 430.0),
            egui::vec2(680.0, 560.0),
            egui::vec2(1150.0, 700.0),
        ] {
            for tab in [0, 5, 3, 4] {
                let ctx = egui::Context::default();
                crate::app::configure(&ctx);
                let mut panes = Panes::default();
                panes.task_tab = tab;
                let mut effects = vec![];
                let output = ctx.run(
                    egui::RawInput {
                        screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, size)),
                        ..Default::default()
                    },
                    |ctx| {
                        egui::CentralPanel::default().show(ctx, |ui| {
                            egui::ScrollArea::vertical().show(ui, |ui| {
                                panes.tasks(ui, &snapshot, Some(&task), &mut effects)
                            });
                        });
                    },
                );
                for label in ["개요", "에이전트", "발견 사항", "증거"] {
                    assert!(
                        visible(&output, label),
                        "Missing/clipped task tab {label} at {size:?}"
                    );
                }
                assert!(effects.is_empty());
            }
            let ctx = egui::Context::default();
            crate::app::configure(&ctx);
            let mut panes = Panes::default();
            panes.set_context(Section::Chat, Some(&conversation.header.id));
            panes.draft_message = "Keep this unsaved draft".into();
            panes.flow.attachments.insert(
                conversation.header.id.clone(),
                (0..5)
                    .map(|i| json!({"name":format!("attachment-{i}.txt"),"size":10}))
                    .collect(),
            );
            let mut effects = vec![];
            let mut reference_position = None;
            for frame in 0..5 {
                let events = if matches!(frame, 1 | 2) {
                    let pos = reference_position.expect("Reference popup button must be visible");
                    vec![
                        egui::Event::PointerMoved(pos),
                        egui::Event::PointerButton {
                            pos,
                            button: egui::PointerButton::Primary,
                            pressed: frame == 1,
                            modifiers: egui::Modifiers::default(),
                        },
                    ]
                } else {
                    vec![]
                };
                let output = ctx.run(
                    egui::RawInput {
                        screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, size)),
                        events,
                        ..Default::default()
                    },
                    |ctx| {
                        egui::CentralPanel::default().show(ctx, |ui| {
                            panes.chat(ui, &snapshot, Some(&conversation), None, &mut effects)
                        });
                    },
                );
                if frame == 0 {
                    reference_position = output.shapes.iter().find_map(|s| match &s.shape {
                        egui::epaint::Shape::Text(t)
                            if t.galley.text().contains("객체 참조 삽입") =>
                        {
                            Some(s.shape.visual_bounding_rect().center())
                        }
                        _ => None,
                    });
                }
                for label in ["메시지", expected_send_button(&ctx)] {
                    assert!(
                        visible(&output, label),
                        "Missing/clipped chat control {label} at {size:?}"
                    );
                }
                if frame == 4 {
                    assert!(
                        visible(&output, "Read · 실행 결과"),
                        "Collapsed tool result is missing/clipped at {size:?}"
                    );
                    assert!(output.shapes.iter().any(|s|matches!(&s.shape,egui::epaint::Shape::Text(t) if t.galley.text().contains("최대 30개 표시"))),"Reference popup did not actually open at {size:?}");
                }
            }
            assert_eq!(panes.draft_message, "Keep this unsaved draft");
            assert!(effects.is_empty());
            assert_eq!(panes.flow.attachments[&conversation.header.id].len(), 5);
        }
        assert_eq!(serde_json::to_value(&snapshot).unwrap(), original);
    }
}

#[cfg(test)]
mod lifecycle_presentation_tests {
    use super::*;

    fn record(collection: &str, patch: Value) -> Record {
        let id = artex_core::new_id(artex_core::collection_prefix(collection).unwrap());
        let mut data = artex_core::default_data(collection).unwrap();
        for (key, value) in patch.as_object().unwrap() {
            data[key] = value.clone();
        }
        if data.get("id").is_some() {
            data["id"] = json!(id);
        }
        serde_json::from_value(json!({"header":{"id":id,"collection":collection,"version":"7","commit_seq":"9","created_at":"2026-10-13T00:00:00Z","updated_at":"2026-10-13T00:00:00Z","legacy_key":null},"data":data})).unwrap()
    }
    fn insert(snapshot: &mut Snapshot, record: Record) {
        snapshot
            .collections
            .entry(record.header.collection.clone())
            .or_default()
            .insert(record.header.id.clone(), record);
    }
    fn render_chat(
        ctx: &egui::Context,
        panes: &mut Panes,
        snapshot: &Snapshot,
        conversation: &Record,
        events: Vec<egui::Event>,
        readonly: bool,
    ) -> (egui::FullOutput, Vec<Effect>) {
        panes.set_read_only(readonly);
        let mut effects = vec![];
        let output = ctx.run(
            egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(680.0, 560.0),
                )),
                events,
                ..Default::default()
            },
            |ctx| {
                egui::CentralPanel::default().show(ctx, |ui| {
                    panes.chat(ui, snapshot, Some(conversation), None, &mut effects);
                });
            },
        );
        (output, effects)
    }
    #[test]
    fn actual_chat_stop_tracks_completed_waiting_readonly_and_conversation_ownership() {
        let conversation = record(
            "conversations",
            json!({"title":"Owned conversation","agent_key":"auto"}),
        );
        let run = record(
            "agent_runs",
            json!({"state":"running","conversation_id":conversation.header.id}),
        );
        let foreign = record(
            "agent_runs",
            json!({"state":"running","conversation_id":artex_core::new_id("conversation")}),
        );
        let mut snapshot = Snapshot {
            workspace_id: artex_core::new_id("workspace"),
            commit_seq: 9,
            collections: BTreeMap::new(),
        };
        for row in [conversation.clone(), run.clone(), foreign] {
            insert(&mut snapshot, row);
        }
        let ctx = egui::Context::default();
        crate::app::configure(&ctx);
        let mut panes = Panes::default();
        panes.set_context(Section::Chat, Some(&conversation.header.id));
        panes.draft_message = "Keep an unsent user draft".into();
        let attachment = json!({"name":"retained.txt","size":12});
        panes
            .flow
            .attachments
            .insert(conversation.header.id.clone(), vec![attachment.clone()]);
        for (state, readonly, should_stop) in [
            ("running", false, true),
            ("completed", false, false),
            ("waiting", false, true),
            ("waiting", true, false),
            ("paused", false, true),
            ("failed", false, false),
            ("queued", false, true),
        ] {
            snapshot
                .collections
                .get_mut("agent_runs")
                .unwrap()
                .get_mut(&run.header.id)
                .unwrap()
                .data["state"] = json!(state);
            if state == "waiting" {
                insert(
                    &mut snapshot,
                    record(
                        "user_questions",
                        json!({"run_id":run.header.id,"state":"pending","kind":"question","payload":{"questions":[{"question":"Confirm the target"}]}}),
                    ),
                );
            }
            let before = snapshot.clone();
            let (output, _) =
                render_chat(&ctx, &mut panes, &snapshot, &conversation, vec![], readonly);
            let stop = output
                .shapes
                .iter()
                .find_map(|shape| match &shape.shape {
                    egui::epaint::Shape::Text(t) if t.galley.text() == "응답 중지" => {
                        assert!(
                            shape
                                .clip_rect
                                .expand(1.0)
                                .contains_rect(t.visual_bounding_rect())
                        );
                        Some(t.visual_bounding_rect().center())
                    }
                    _ => None,
                })
                .expect("The stop action must remain visible in the header");
            let pointer = |pressed| {
                vec![
                    egui::Event::PointerMoved(stop),
                    egui::Event::PointerButton {
                        pos: stop,
                        button: egui::PointerButton::Primary,
                        pressed,
                        modifiers: egui::Modifiers::NONE,
                    },
                ]
            };
            let (_, mut effects) = render_chat(
                &ctx,
                &mut panes,
                &snapshot,
                &conversation,
                pointer(true),
                readonly,
            );
            effects.extend(
                render_chat(
                    &ctx,
                    &mut panes,
                    &snapshot,
                    &conversation,
                    pointer(false),
                    readonly,
                )
                .1,
            );
            let commands = effects
                .iter()
                .filter_map(|effect| match effect {
                    Effect::Command {
                        method,
                        params,
                        expected,
                    } => Some((method, params, expected)),
                    _ => None,
                })
                .collect::<Vec<_>>();
            assert_eq!(
                commands.len(),
                usize::from(should_stop),
                "Invalid Stop interaction in {state}, readonly={readonly}"
            );
            if let Some((method, params, versions)) = commands.first() {
                assert_eq!(method.as_str(), "ui.stopConversation");
                assert_eq!(*params, &json!({"id":conversation.header.id}));
                assert_eq!(*versions, &vec![expected(&conversation)]);
            }
            assert_eq!(panes.draft_message, "Keep an unsent user draft");
            assert_eq!(
                panes.flow.attachments[&conversation.header.id],
                vec![attachment.clone()]
            );
            assert_eq!(
                snapshot, before,
                "Rendering or clicking must not mutate the read snapshot"
            );
        }
        let (output, _) = render_chat(&ctx, &mut panes, &snapshot, &conversation, vec![], true);
        let refresh = output
            .shapes
            .iter()
            .find_map(|shape| match &shape.shape {
                egui::epaint::Shape::Text(text) if text.galley.text() == "기록 새로고침" => {
                    Some(text.visual_bounding_rect().center())
                }
                _ => None,
            })
            .expect("Read-only mode must retain the history query action");
        let pointer = |pressed| {
            vec![
                egui::Event::PointerMoved(refresh),
                egui::Event::PointerButton {
                    pos: refresh,
                    button: egui::PointerButton::Primary,
                    pressed,
                    modifiers: egui::Modifiers::NONE,
                },
            ]
        };
        let _ = render_chat(
            &ctx,
            &mut panes,
            &snapshot,
            &conversation,
            pointer(true),
            true,
        );
        let (_, effects) = render_chat(
            &ctx,
            &mut panes,
            &snapshot,
            &conversation,
            pointer(false),
            true,
        );
        assert!(
            effects
                .iter()
                .any(|effect| matches!(effect, Effect::Query { method, params }
            if method == "ui.conversationHistory" && params["id"] == conversation.header.id))
        );
        assert!(
            !effects
                .iter()
                .any(|effect| matches!(effect, Effect::Command { .. }))
        );
        panes.set_read_only(false);
        panes.retire_async_operations();
        assert!(
            panes.read_only,
            "Retired owners remain blocked until the app injects the next scope's mode"
        );
        panes.set_read_only(false);
        assert!(!panes.read_only);
        assert_eq!(panes.draft_message, "Keep an unsent user draft");
    }
    #[test]
    fn task_controls_observe_budget_recovery_and_worker_intent_state() {
        let mut task = record("tasks", json!({"status":"running","paused":false}));
        let mut snapshot = Snapshot {
            workspace_id: artex_core::new_id("workspace"),
            commit_seq: 9,
            collections: BTreeMap::new(),
        };
        assert!(task_action_allowed(&snapshot, &task, "pause"));
        assert!(!task_action_allowed(&snapshot, &task, "resume"));
        task.data["status"] = json!("paused");
        task.data["paused"] = json!(true);
        assert!(!task_action_allowed(&snapshot, &task, "pause"));
        assert!(task_action_allowed(&snapshot, &task, "resume"));
        let mut runtime = record(
            "task_runtime",
            json!({"task_id":task.header.id,"accounting_state":"recovery_required","remaining_timeout_ms":"1000"}),
        );
        insert(&mut snapshot, runtime.clone());
        assert!(!task_action_allowed(&snapshot, &task, "resume"));
        runtime.data["accounting_state"] = json!("known");
        runtime.data["remaining_timeout_ms"] = json!("0");
        insert(&mut snapshot, runtime);
        assert!(!task_action_allowed(&snapshot, &task, "resume"));
        let mut intent = record(
            "exploration_nodes",
            json!({"kind":"intent","state":"running"}),
        );
        assert!(intent_action_allowed(&task, Some(&intent), "pause", ""));
        assert!(!intent_action_allowed(&task, Some(&intent), "resume", ""));
        assert!(!intent_action_allowed(&task, Some(&intent), "cancel", "  "));
        assert!(intent_action_allowed(
            &task,
            Some(&intent),
            "cancel",
            "User requested a stop"
        ));
        intent.data["state"] = json!("paused");
        assert!(intent_action_allowed(&task, Some(&intent), "resume", ""));
        task.data["archived_at"] = json!("2026-10-13T00:00:00Z");
        assert!(!intent_action_allowed(&task, Some(&intent), "resume", ""));
        assert!(!task_action_allowed(&snapshot, &task, "resume"));
    }
    #[test]
    fn task_chat_stop_does_not_cancel_a_worker_or_foreign_task_run() {
        let task = artex_core::new_id("task");
        let agent = record("agents", json!({"key":"worker"}));
        let mut run = record(
            "agent_runs",
            json!({"state":"waiting","task_id":task,"agent_id":agent.header.id,"intent_id":artex_core::new_id("node")}),
        );
        let mut snapshot = Snapshot {
            workspace_id: artex_core::new_id("workspace"),
            commit_seq: 9,
            collections: BTreeMap::new(),
        };
        insert(&mut snapshot, agent.clone());
        insert(&mut snapshot, run.clone());
        assert!(!chat_can_stop(&snapshot, None, Some(&task)));
        let mut main = agent;
        main.data["key"] = json!("mainagent");
        insert(&mut snapshot, main);
        assert!(
            !chat_can_stop(&snapshot, None, Some(&task)),
            "An intent-owned worker must use its own control action"
        );
        run.data["intent_id"] = Value::Null;
        insert(&mut snapshot, run.clone());
        assert!(chat_can_stop(&snapshot, None, Some(&task)));
        assert!(!chat_can_stop(
            &snapshot,
            None,
            Some(&artex_core::new_id("task"))
        ));
        run.data["state"] = json!("completed");
        insert(&mut snapshot, run);
        assert!(!chat_can_stop(&snapshot, None, Some(&task)));
    }
    #[test]
    fn actual_mac_command_enter_preserves_pending_dedup_cas_and_late_draft_ownership() {
        let conversation = record(
            "conversations",
            json!({"title":"Mac shortcut","agent_key":"auto"}),
        );
        let mut snapshot = Snapshot {
            workspace_id: artex_core::new_id("workspace"),
            commit_seq: 9,
            collections: BTreeMap::new(),
        };
        insert(&mut snapshot, conversation.clone());
        let original = snapshot.clone();
        let ctx = egui::Context::default();
        crate::app::configure(&ctx);
        ctx.set_os(egui::os::OperatingSystem::Mac);
        let mut panes = Panes::default();
        panes.set_context(Section::Chat, Some(&conversation.header.id));
        let attachment =
            json!({"id":artex_core::new_id("attachment"),"name":"mac-evidence.txt","size":17});
        panes
            .flow
            .attachments
            .insert(conversation.header.id.clone(), vec![attachment.clone()]);
        let command_only = egui::Modifiers {
            command: true,
            mac_cmd: true,
            ctrl: false,
            ..Default::default()
        };
        let render = |panes: &mut Panes, pressed: bool| {
            let mut effects = vec![];
            let output = ctx.run(
                egui::RawInput {
                    screen_rect: Some(egui::Rect::from_min_size(
                        egui::Pos2::ZERO,
                        egui::vec2(680.0, 560.0),
                    )),
                    modifiers: if pressed {
                        command_only
                    } else {
                        egui::Modifiers::NONE
                    },
                    events: if pressed {
                        vec![egui::Event::Key {
                            key: egui::Key::Enter,
                            physical_key: None,
                            pressed: true,
                            repeat: false,
                            modifiers: command_only,
                        }]
                    } else {
                        vec![]
                    },
                    ..Default::default()
                },
                |ctx| {
                    egui::CentralPanel::default().show(ctx, |ui| {
                        panes.chat(ui, &snapshot, Some(&conversation), None, &mut effects)
                    });
                },
            );
            (output, effects)
        };
        let (initial, initial_effects) = render(&mut panes, false);
        assert!(initial_effects.is_empty());
        assert!(initial.shapes.iter().any(|s|matches!(&s.shape,egui::epaint::Shape::Text(t) if t.galley.text()=="보내기 · ⌘+Enter")));
        assert!(initial.shapes.iter().any(|s|matches!(&s.shape,egui::epaint::Shape::Text(t) if t.galley.text()=="메시지를 입력하세요. ⌘+Enter로 보냅니다.")));
        panes.draft_message = "Mac submitted draft".into();
        let (_, effects) = render(&mut panes, true);
        let (params, versions) = effects
            .iter()
            .find_map(|e| match e {
                Effect::Command {
                    method,
                    params,
                    expected,
                } if method == "ui.sendConversationMessage" => {
                    Some((params.clone(), expected.clone()))
                }
                _ => None,
            })
            .expect("Command-only Enter must submit the real source operation");
        assert_eq!(params["message"], "Mac submitted draft");
        assert_eq!(params["attachments"], json!([attachment]));
        assert_eq!(versions, vec![expected(&conversation)]);
        panes.draft_message = "New unsent A draft".into();
        assert!(
            !render(&mut panes, true)
                .1
                .iter()
                .any(|e| matches!(e, Effect::Command { .. })),
            "A pending request must reject repeated Command+Enter"
        );
        panes.set_context(Section::Chat, Some("conversation-B"));
        panes.draft_message = "Keep B draft".into();
        panes.set_command_result_for_params(
            "ui.sendConversationMessage",
            &params,
            Some(&conversation.header.id),
            json!({"accepted":true}),
        );
        assert_eq!(panes.draft_message, "Keep B draft");
        panes.set_context(Section::Chat, Some(&conversation.header.id));
        assert_eq!(panes.draft_message, "New unsent A draft");
        assert!(panes.flow.attachments[&conversation.header.id].is_empty());
        assert_eq!(snapshot, original);
    }
    #[test]
    fn local_timestamp_preserves_offset_and_date_boundaries_without_changing_raw_values() {
        let utc = chrono::DateTime::parse_from_rfc3339("2026-10-13T23:30:45Z").unwrap();
        let east = chrono::FixedOffset::east_opt(9 * 3600).unwrap();
        let west = chrono::FixedOffset::west_opt(7 * 3600).unwrap();
        assert_eq!(
            timestamp_label(utc.with_timezone(&east)),
            "2026-10-14 08:30:45 UTC+09:00 · 현지"
        );
        let early = chrono::DateTime::parse_from_rfc3339("2026-10-13T00:30:45Z").unwrap();
        assert_eq!(
            timestamp_label(early.with_timezone(&west)),
            "2026-10-12 17:30:45 UTC-07:00 · 현지"
        );
        assert_eq!(
            presentation_time("2026-10-13T23:30:45Z"),
            presentation_time("2026-10-14T08:30:45+09:00")
        );
        assert_eq!(presentation_time("not a timestamp"), "not a timestamp");
        assert_eq!(presentation_time(""), "");
        assert_eq!(presentation_short_time("not a timestamp"), "시각 미기록");
        assert_eq!(
            presentation_short_time("2026-10-13T23:30:45Z"),
            utc.with_timezone(&chrono::Local)
                .format("%H:%M:%S · 현지")
                .to_string()
        );
        assert_eq!(
            presentation_time("2026-10-13T23:30:45Z"),
            timestamp_label(utc.with_timezone(&chrono::Local).fixed_offset())
        );
    }
}

#[cfg(test)]
mod drawer_composer_tests {
    use super::*;
    #[test]
    fn bounded_drawer_resize_keeps_composer_actions_visible_with_attachments_and_pending_requests()
    {
        let cid = artex_core::new_id("conversation");
        let conversation:Record=serde_json::from_value(json!({"header":{"id":cid,"collection":"conversations","version":"3","commit_seq":"3","created_at":"2026-10-13T00:00:00Z","updated_at":"2026-10-13T00:00:00Z","legacy_key":null},"data":{"id":cid,"title":"Drawer conversation","agent_key":"auto"}})).unwrap();
        let aid = artex_core::new_id("activity");
        let activity:Record=serde_json::from_value(json!({"header":{"id":aid,"collection":"conversation_activities","version":"1","commit_seq":"3","created_at":"2026-10-13T00:00:00Z","updated_at":"2026-10-13T00:00:00Z","legacy_key":null},"data":{"id":aid,"conversation_id":cid,"kind":"text","worker":"auto","summary":"Persisted answer","detail":{"text":"Persisted answer","blob":null}}})).unwrap();
        let mut snapshot = Snapshot {
            workspace_id: artex_core::new_id("workspace"),
            commit_seq: 3,
            collections: BTreeMap::new(),
        };
        for row in [conversation.clone(), activity] {
            snapshot
                .collections
                .entry(row.header.collection.clone())
                .or_default()
                .insert(row.header.id.clone(), row);
        }
        let original = snapshot.clone();
        let ctx = egui::Context::default();
        crate::app::configure(&ctx);
        let mut panes = Panes::default();
        panes.set_context(Section::Chat, Some(&cid));
        panes.draft_message = "Retain this drawer draft".into();
        let mut effects = vec![];
        let mut time = 0.0;
        for (width, drawer_requested, attached, pending) in [
            (900.0_f32, 220.0_f32, 0_usize, false),
            (900.0, 260.0, 5, true),
            (1320.0, 210.0, 5, false),
            (900.0, 260.0, 0, true),
            (900.0, 148.0, 5, false),
        ] {
            let attachments = (0..attached)
                .map(|i| json!({"name":format!("evidence-{i}.txt"),"size":17}))
                .collect::<Vec<_>>();
            panes
                .flow
                .attachments
                .insert(cid.clone(), attachments.clone());
            if pending {
                panes.flow.pending_messages.insert(
                    cid.clone(),
                    ("Already submitted message".into(), attachments.clone()),
                );
            } else {
                panes.flow.pending_messages.remove(&cid);
            }
            for frame in 0..12 {
                time += 0.05;
                let mut viewport = egui::Rect::NOTHING;
                let output = ctx.run(
                    egui::RawInput {
                        screen_rect: Some(egui::Rect::from_min_size(
                            egui::Pos2::ZERO,
                            egui::vec2(width, 620.0),
                        )),
                        time: Some(time),
                        ..Default::default()
                    },
                    |ctx| {
                        egui::TopBottomPanel::top("drawer-fixture-chrome")
                            .exact_height(60.0)
                            .show(ctx, |ui| {
                                ui.label("Workspace");
                            });
                        let remaining = ctx.available_rect().height();
                        // The app's shared drawer uses the real previous composer measurement,
                        // leaving central chrome margins in addition to the pane's own budget.
                        let drawer_height = drawer_requested
                            .min((remaining - panes.chat_minimum_height() - 40.0).max(38.0));
                        egui::TopBottomPanel::bottom("drawer-fixture")
                            .exact_height(drawer_height)
                            .show(ctx, |ui| {
                                ui.horizontal(|ui| {
                                    ui.label("실행 기록");
                                    ui.label("추가 관리");
                                });
                            });
                        egui::SidePanel::left("drawer-fixture-sidebar")
                            .exact_width(432.0)
                            .show(ctx, |ui| {
                                ui.label("Navigation and objects");
                            });
                        egui::CentralPanel::default().show(ctx, |ui| {
                            let scroll = egui::ScrollArea::vertical()
                                .id_salt("active_pane")
                                .auto_shrink([false, false])
                                .show(ui, |ui| {
                                    ui.set_width(ui.available_width());
                                    panes.chat(
                                        ui,
                                        &snapshot,
                                        Some(&conversation),
                                        None,
                                        &mut effects,
                                    );
                                });
                            viewport = scroll.inner_rect;
                        });
                    },
                );
                if frame >= 10 {
                    for label in [
                        "Retain this drawer draft",
                        "파일 첨부 준비",
                        "객체 참조 삽입",
                        expected_send_button(&ctx),
                    ] {
                        let shape=output.shapes.iter().find(|s| matches!(&s.shape,egui::epaint::Shape::Text(t) if t.galley.text()==label)).unwrap_or_else(||panic!("Missing {label}"));
                        let rect = shape.shape.visual_bounding_rect();
                        assert!(
                            shape.clip_rect.expand(1.0).contains_rect(rect),
                            "Clipped {label}: width={width}, attachments={attached}, pending={pending}"
                        );
                        assert!(
                            viewport.expand(1.0).contains_rect(rect),
                            "{label} left the bounded drawer viewport"
                        );
                    }
                    if pending {
                        assert!(output.shapes.iter().any(|s|matches!(&s.shape,egui::epaint::Shape::Text(t) if t.galley.text()=="저장·실행 요청 확인 중")));
                    }
                }
            }
            assert_eq!(panes.flow.attachments[&cid], attachments);
            assert_eq!(panes.flow.pending_messages.contains_key(&cid), pending);
            assert_eq!(panes.draft_message, "Retain this drawer draft");
        }
        assert_eq!(snapshot, original);
        assert!(
            effects.is_empty(),
            "Measuring and repainting must never submit user actions"
        );
    }
}

#[cfg(test)]
mod actual_drawer_composer_tests {
    use super::*;
    #[test]
    fn production_compact_toolbar_and_drawer_show_sixty_pixel_body_and_chat_controls_at_minimum_window()
     {
        let cid = artex_core::new_id("conversation");
        let conversation:Record=serde_json::from_value(json!({"header":{"id":cid,"collection":"conversations","version":"2","commit_seq":"2","created_at":"2026-10-13T00:00:00Z","updated_at":"2026-10-13T00:00:00Z","legacy_key":null},"data":{"id":cid,"title":"Actual drawer","agent_key":"auto"}})).unwrap();
        let mut snapshot = Snapshot {
            workspace_id: artex_core::new_id("workspace"),
            commit_seq: 2,
            collections: BTreeMap::new(),
        };
        snapshot
            .collections
            .entry("conversations".into())
            .or_default()
            .insert(cid.clone(), conversation.clone());
        let original = snapshot.clone();
        let ctx = egui::Context::default();
        crate::app::configure(&ctx);
        let mut panes = Panes::default();
        panes.set_context(Section::Chat, Some(&cid));
        panes.draft_message = "Unsubmitted draft".into();
        let mut effects = vec![];
        let mut time = 0.0;
        for (width, attached, pending, tab) in [
            (900.0, 0, false, crate::app::DrawerTab::Activity),
            (900.0, 5, true, crate::app::DrawerTab::Management),
            (1320.0, 5, false, crate::app::DrawerTab::Activity),
            (900.0, 5, false, crate::app::DrawerTab::Management),
        ] {
            let attachments = (0..attached)
                .map(|i| json!({"name":format!("evidence-{i}.txt"),"size":17}))
                .collect::<Vec<_>>();
            panes
                .flow
                .attachments
                .insert(cid.clone(), attachments.clone());
            if pending {
                panes.flow.pending_messages.insert(
                    cid.clone(),
                    ("Captured request".into(), attachments.clone()),
                );
            } else {
                panes.flow.pending_messages.remove(&cid);
            }
            for frame in 0..12 {
                time += 0.05;
                let mut body = egui::Rect::NOTHING;
                let mut center = egui::Rect::NOTHING;
                let output = ctx.run(
                    egui::RawInput {
                        screen_rect: Some(egui::Rect::from_min_size(
                            egui::Pos2::ZERO,
                            egui::vec2(width, 620.0),
                        )),
                        time: Some(time),
                        ..Default::default()
                    },
                    |ctx| {
                        let _ = crate::window_chrome::show_titlebar(
                            ctx,
                            crate::window_chrome::Platform::current(),
                            |ui| {
                                egui::MenuBar::new().ui(ui, |ui| {
                                    for label in ["파일", "보기", "작업 공간", "도움말"]
                                    {
                                        let _ = ui.menu_button(label, |_| {});
                                    }
                                });
                            },
                        );
                        egui::TopBottomPanel::top("actual-toolbar")
                            .frame(crate::theme::toolbar_frame(&ctx.style()))
                            .show(ctx, |ui| {
                                let actions = crate::app::compact_toolbar(
                                    ui,
                                    &crate::app::CompactToolbarPresentation {
                                        workspace_name: Some("Fixture"),
                                        workspace_path: Some("fixture-only"),
                                        bypass: Some((false, false, true)),
                                        list_open: Some(false),
                                        management_open: Some(
                                            tab == crate::app::DrawerTab::Management,
                                        ),
                                        ..Default::default()
                                    },
                                );
                                assert!(actions.is_empty());
                            });
                        egui::TopBottomPanel::bottom("actual-status")
                            .frame(crate::theme::status_frame(&ctx.style()))
                            .show(ctx, |ui| {
                                ui.horizontal(|ui| {
                                    ui.small("준비됨");
                                    ui.small("Rust 네이티브");
                                    ui.with_layout(
                                        egui::Layout::right_to_left(egui::Align::Center),
                                        |ui| {
                                            let _ = ui.selectable_label(true, "실행·변경 기록");
                                        },
                                    );
                                });
                            });
                        crate::app::drawer_panel(ctx, panes.chat_minimum_height() + 40.0, |ui| {
                            ui.horizontal_wrapped(|ui| {
                                let _ = ui.selectable_label(
                                    tab == crate::app::DrawerTab::Activity,
                                    "실행·변경 기록",
                                );
                                let _ = ui.selectable_label(
                                    tab == crate::app::DrawerTab::Management,
                                    "추가 관리",
                                );
                                ui.small("대기 0");
                                let _ = ui.small_button("접기");
                            });
                            ui.separator();
                            let scroll = crate::app::drawer_scroll(ui, tab, |ui| {
                                for record in snapshot.records("conversations") {
                                    ui.label(format!("{} · 기록 조회 가능", display_name(record)));
                                }
                                for n in 0..12 {
                                    ui.label(format!("조회·설정 동작 {n}"));
                                }
                            });
                            body = scroll.inner_rect;
                        });
                        egui::SidePanel::left("actual-sidebar")
                            .exact_width(432.0)
                            .frame(crate::theme::toolbar_frame(&ctx.style()))
                            .show(ctx, |ui| {
                                ui.label("탐색·목록");
                            });
                        egui::CentralPanel::default()
                            .frame(egui::Frame::central_panel(&ctx.style()).inner_margin(20))
                            .show(ctx, |ui| {
                                let scroll = egui::ScrollArea::vertical()
                                    .id_salt("active_pane")
                                    .auto_shrink([false, false])
                                    .show(ui, |ui| {
                                        ui.set_width(ui.available_width());
                                        panes.chat(
                                            ui,
                                            &snapshot,
                                            Some(&conversation),
                                            None,
                                            &mut effects,
                                        );
                                    });
                                center = scroll.inner_rect;
                            });
                    },
                );
                if frame >= 10 {
                    assert!(
                        body.height() >= 60.0,
                        "Production drawer body is only {}px: width={width}, attached={attached}, pending={pending}",
                        body.height()
                    );
                    for label in [
                        "메시지",
                        "Unsubmitted draft",
                        "파일 첨부 준비",
                        "객체 참조 삽입",
                        expected_send_button(&ctx),
                    ] {
                        let shape=output.shapes.iter().find(|s|matches!(&s.shape,egui::epaint::Shape::Text(t) if t.galley.text()==label)).unwrap_or_else(||panic!("Missing {label}"));
                        let rect = shape.shape.visual_bounding_rect();
                        assert!(
                            shape.clip_rect.expand(1.0).contains_rect(rect)
                                && center.expand(1.0).contains_rect(rect),
                            "Clipped {label} with production chrome: width={width}, attached={attached}, pending={pending}, rect={rect:?}, clip={:?}, center={center:?}",
                            shape.clip_rect
                        );
                    }
                    if pending {
                        let feedback=output.shapes.iter().find(|s| matches!(&s.shape,egui::epaint::Shape::Text(t) if t.galley.text()=="저장·실행 요청 확인 중")).expect("Pending feedback must remain visible");
                        let rect = feedback.shape.visual_bounding_rect();
                        assert!(
                            feedback.clip_rect.expand(1.0).contains_rect(rect)
                                && center.expand(1.0).contains_rect(rect)
                        );
                    }
                }
            }
            assert_eq!(panes.draft_message, "Unsubmitted draft");
            assert_eq!(panes.flow.attachments[&cid], attachments);
            assert_eq!(panes.flow.pending_messages.contains_key(&cid), pending);
        }
        assert_eq!(snapshot, original);
        assert!(effects.is_empty());
    }
}

impl Panes {
    fn management_token(&mut self) -> String {
        self.flow.serial += 1;
        format!("native-panes-{}", self.flow.serial)
    }
    fn open_management(&mut self, snapshot: &Snapshot, action: ManagementAction, ids: Vec<String>) {
        if ids.is_empty()
            || self
                .management
                .dialog
                .as_ref()
                .is_some_and(|d| d.action_token.is_some())
        {
            return;
        }
        let mut seen = BTreeSet::new();
        let rows = ids
            .iter()
            .filter(|id| seen.insert((*id).clone()))
            .filter_map(|id| snapshot.get(action.collection(), id))
            .collect::<Vec<_>>();
        if rows.is_empty() {
            return;
        }
        let nonce = self.management_token();
        let ids = rows.iter().map(|r| r.header.id.clone()).collect();
        let baseline = rows.iter().map(|r| expected(r)).collect::<Vec<_>>();
        self.management.dialog = Some(ManagementDialog {
            nonce,
            action,
            ids,
            names: rows.iter().map(|r| display_name(r)).collect(),
            baseline: baseline.clone(),
            versions: baseline,
            preview: None,
            preview_token: None,
            action_token: None,
            options: DeleteSelection::default(),
            confirmed: false,
            error: None,
            reload_required: false,
            source_allowed: action != ManagementAction::ArchiveRestore
                || rows
                    .iter()
                    .all(|r| matches!(text(&r.data, "state"), "ready" | "restore_failed")),
        });
        self.request_management_preview();
    }
    pub fn object_batch_checkbox(&mut self, ui: &mut egui::Ui, section: Section, row: &Record) {
        if !matches!(section, Section::Chat | Section::Tasks) {
            return;
        }
        let batch = self.management.batches.entry(section).or_default();
        let mut checked = batch.contains(&row.header.id);
        if ui
            .checkbox(&mut checked, "")
            .on_hover_text("여러 항목 선택")
            .changed()
        {
            if checked {
                batch.insert(row.header.id.clone());
            } else {
                batch.remove(&row.header.id);
            }
        }
    }
    pub fn object_actions(
        &mut self,
        ui: &mut egui::Ui,
        snapshot: &Snapshot,
        section: Section,
        row: &Record,
    ) {
        for (label, action) in match section {
            Section::Chat => vec![
                ("대화 내용 비우기", ManagementAction::ClearHistory),
                ("대화 삭제", ManagementAction::Trash),
            ],
            Section::Tasks => vec![("작업 삭제…", ManagementAction::TaskDelete)],
            Section::Archives => vec![("보관 작업 복원…", ManagementAction::ArchiveRestore)],
            _ => vec![],
        } {
            if ui
                .add_enabled(
                    self.management
                        .dialog
                        .as_ref()
                        .is_none_or(|d| d.action_token.is_none()),
                    egui::Button::new(label),
                )
                .clicked()
            {
                self.open_management(snapshot, action, vec![row.header.id.clone()]);
                ui.close();
            }
        }
    }
    pub fn object_batch_toolbar(
        &mut self,
        ui: &mut egui::Ui,
        snapshot: &Snapshot,
        section: Section,
    ) {
        if !matches!(section, Section::Chat | Section::Tasks | Section::Archives) {
            return;
        }
        let ids = self
            .management
            .batches
            .get(&section)
            .map(|b| b.iter().cloned().collect::<Vec<_>>())
            .unwrap_or_default();
        ui.horizontal_wrapped(|ui| {
            if matches!(section, Section::Chat | Section::Tasks) {
                ui.small(format!("{}개 선택", ids.len()));
                if ui
                    .add_enabled(!ids.is_empty(), egui::Button::new("선택 해제"))
                    .clicked()
                {
                    self.management.batches.remove(&section);
                }
                if ui
                    .add_enabled(
                        !ids.is_empty(),
                        egui::Button::new(if self.read_only {
                            "선택 영향 보기…"
                        } else {
                            "선택 삭제…"
                        }),
                    )
                    .clicked()
                {
                    self.open_management(
                        snapshot,
                        if section == Section::Chat {
                            ManagementAction::Trash
                        } else {
                            ManagementAction::TaskDelete
                        },
                        ids.clone(),
                    );
                }
            }
            if section == Section::Chat && ui.button("휴지통·복구").clicked() {
                self.management.trash_open = true;
                self.request_conversation_trash();
            }
        });
        if let Some(message) = self.management.feedback.get("global") {
            ui.add(egui::Label::new(message).wrap());
        }
    }
    fn request_conversation_trash(&mut self) {
        let token = self.management_token();
        self.management.trash_token = Some(token.clone());
        self.management
            .requests
            .insert(token.clone(), ManagementRequest::Trash);
        self.flow.queued.push(Effect::ConversationTrash { token });
    }
}

fn native_expected(value: &Value) -> Vec<Expected> {
    serde_json::from_value(value.clone()).unwrap_or_default()
}
impl DeleteSelection {
    fn native(&self) -> TaskDeleteOptions {
        TaskDeleteOptions {
            delete_assets: self.assets,
            delete_findings: self.findings,
            delete_traffic: self.traffic,
            delete_llm_records: self.llm_records,
            delete_files: self.files,
        }
    }
}
impl Panes {
    pub fn open_smoke_deletion_preview(
        &mut self,
        snapshot: &Snapshot,
        section: Section,
        selected: Option<&str>,
    ) {
        let action = match section {
            Section::Tasks => ManagementAction::TaskDelete,
            Section::Chat => ManagementAction::Trash,
            Section::Archives => ManagementAction::ArchiveRestore,
            _ => return,
        };
        let id = selected
            .and_then(|id| snapshot.get(action.collection(), id))
            .or_else(|| {
                snapshot
                    .records(action.collection())
                    .into_iter()
                    .find(|r| r.data["deleted_at"].is_null() && r.data["trashed_at"].is_null())
            })
            .map(|r| r.header.id.clone());
        if let Some(id) = id {
            self.open_management(snapshot, action, vec![id]);
        }
    }
    fn request_management_preview(&mut self) {
        let Some(dialog) = self.management.dialog.as_ref() else {
            return;
        };
        if dialog.action_token.is_some() {
            return;
        }
        let action = dialog.action;
        let ids = dialog.ids.clone();
        let nonce = dialog.nonce.clone();
        let flags = dialog.options.native();
        if matches!(action, ManagementAction::ForceStop) {
            let row = self
                .management
                .monitor
                .as_ref()
                .and_then(|v| v["runs"].as_array())
                .and_then(|runs| runs.iter().find(|r| r["run_id"] == ids[0]))
                .cloned();
            let Some(row) = row else {
                if let Some(d) = self.management.dialog.as_mut() {
                    d.error = Some("실행 모니터 자료를 먼저 새로고침하세요.".into());
                    d.reload_required = true;
                }
                return;
            };
            let versions = native_expected(&row["expected"]);
            let allowed = if action == ManagementAction::ForceStop {
                monitor_can_force_stop(&row)
            } else {
                row["can_stop"] == true
            };
            if let Some(d) = self.management.dialog.as_mut() {
                d.reload_required = !allowed
                    || d.baseline.iter().any(|base| {
                        !versions.iter().any(|v| {
                            v.collection == base.collection
                                && v.id == base.id
                                && v.version == base.version
                        })
                    });
                d.error = if d.reload_required {
                    Some("실행 상태 또는 권한이 바뀌었습니다. 다시 확인하세요.".into())
                } else {
                    None
                };
                d.versions = versions;
                d.preview = Some(
                    json!({"can_apply":allowed,"cleanup_confirmed":row["cleanup_confirmed"],"affected":{"runs":1},"executions":row["executions"],"cleanup_state":row["cleanup_state"]}),
                );
            }
            return;
        }
        if action == ManagementAction::ArchiveRestore {
            if let Some(d) = self.management.dialog.as_mut() {
                d.preview = Some(
                    json!({"can_apply":d.source_allowed,"targets":d.names,"affected":{"archives":d.ids.len()}}),
                );
            }
            return;
        }
        let (target, options) = if matches!(action, ManagementAction::TaskDelete) {
            (
                DeletionTarget::Tasks { ids },
                DeletionOptions::Tasks { options: flags },
            )
        } else {
            let action = match action {
                ManagementAction::ClearHistory => ConversationAction::ClearHistory,
                ManagementAction::Restore => ConversationAction::Restore,
                ManagementAction::Purge => ConversationAction::DeletePermanently,
                _ => ConversationAction::Trash,
            };
            (
                DeletionTarget::Conversations { ids },
                DeletionOptions::Conversations { action },
            )
        };
        let token = self.management_token();
        self.management
            .requests
            .insert(token.clone(), ManagementRequest::Preview(nonce));
        if let Some(d) = self.management.dialog.as_mut() {
            d.preview = None;
            d.preview_token = Some(token.clone());
            d.confirmed = false;
            d.error = None;
        }
        self.flow.queued.push(Effect::DeletionPreview {
            target,
            options,
            token,
        });
    }
    pub fn set_native_result(&mut self, token: &str, value: &Value) -> bool {
        let Some(request) = self.management.requests.remove(token) else {
            return false;
        };
        match request {
            ManagementRequest::TaskStopPreview { task_id, baseline } => {
                let versions = native_expected(&value["task_control_expected"]);
                if self.read_only
                    || versions.is_empty()
                    || !versions.iter().any(|v| {
                        v.collection == baseline.collection
                            && v.id == baseline.id
                            && v.version == baseline.version
                    })
                {
                    self.management.feedback.insert(task_id,"VERSION_CONFLICT: 작업 상태가 바뀌었거나 제어 의존성이 없습니다. 다시 조회한 뒤 중지를 요청하세요.".into());
                    return true;
                }
                let action_token = self.management_token();
                self.management.requests.insert(
                    action_token.clone(),
                    ManagementRequest::Mutation {
                        nonce: None,
                        ids: vec![task_id.clone()],
                    },
                );
                self.management.feedback.insert(
                    task_id.clone(),
                    "중지 요청 중 · 외부 자원 정리 확인 대기".into(),
                );
                self.flow.queued.push(Effect::RunControl {
                    target: RunControlTarget::Task { task_id },
                    action: RunControlAction::Stop,
                    expected: versions,
                    token: action_token,
                });
            }
            ManagementRequest::Monitor(key) => {
                if let Some(id) = key.strip_prefix("detail:") {
                    if self
                        .management
                        .monitor_detail_tokens
                        .get(id)
                        .is_none_or(|active| active != token)
                    {
                        return false;
                    }
                    self.management.monitor_detail_tokens.remove(id);
                    self.management
                        .monitor_details
                        .insert(id.into(), value.clone());
                    return true;
                }
                if self.management.monitor_latest.as_deref() != Some(token) {
                    return false;
                }
                self.management.monitor = Some(value.clone());
                self.management.monitor_error = None;
                self.management.monitor_latest = None;
            }
            ManagementRequest::Trash => {
                if self.management.trash_token.as_deref() != Some(token) {
                    return false;
                }
                self.management.trash = Some(value.clone());
                self.management.trash_token = None;
            }
            ManagementRequest::Preview(nonce) => {
                let Some(d) = self
                    .management
                    .dialog
                    .as_mut()
                    .filter(|d| d.nonce == nonce && d.preview_token.as_deref() == Some(token))
                else {
                    return false;
                };
                d.preview_token = None;
                let versions = native_expected(&value["expected"]);
                let stale = d.baseline.iter().any(|base| {
                    !versions.iter().any(|v| {
                        v.collection == base.collection
                            && v.id == base.id
                            && v.version == base.version
                    })
                });
                d.reload_required = stale;
                d.error = if stale {
                    Some("VERSION_CONFLICT: 처음 확인한 항목의 버전이 바뀌었습니다. 명시적으로 다시 확인하세요.".into())
                } else {
                    None
                };
                d.versions = versions;
                d.preview = Some(value.clone());
            }
            ManagementRequest::Mutation { nonce, ids } => {
                let invalidated = self
                    .management
                    .dialog
                    .as_ref()
                    .filter(|d| {
                        nonce.as_ref() == Some(&d.nonce) && d.action_token.as_deref() == Some(token)
                    })
                    .filter(|d| {
                        matches!(
                            d.action,
                            ManagementAction::ClearHistory
                                | ManagementAction::Trash
                                | ManagementAction::Purge
                        )
                    })
                    .and_then(|d| {
                        value["results"].as_array().map(|rows| {
                            rows.iter()
                                .filter(|r| r["ok"] == true)
                                .filter_map(|r| r["id"].as_str())
                                .filter(|id| d.ids.iter().any(|target| target == id))
                                .map(str::to_owned)
                                .collect::<Vec<_>>()
                        })
                    })
                    .unwrap_or_default();
                self.invalidate_history(&invalidated);
                let feedback = if value["cleanup_confirmed"] == true {
                    "중지·정리 결과 확인됨"
                } else {
                    "요청 결과 수신 · 외부 자원 정리 미확인"
                };
                for id in ids {
                    self.management.feedback.insert(id, feedback.into());
                }
                self.management.monitor_last_poll = None;
                self.management.monitor_latest = None;
                self.management.monitor_detail_tokens.clear();
                self.management.monitor_details.clear();
                self.management
                    .requests
                    .retain(|_, r| !matches!(r, ManagementRequest::Monitor(_)));
                let Some(nonce) = nonce else { return true };
                let Some(d) = self
                    .management
                    .dialog
                    .as_mut()
                    .filter(|d| d.nonce == nonce && d.action_token.as_deref() == Some(token))
                else {
                    return false;
                };
                d.action_token = None;
                let succeeded = value["results"]
                    .as_array()
                    .map(|rows| rows.iter().filter(|r| r["ok"] == true).count())
                    .unwrap_or(0);
                let failed = value["results"]
                    .as_array()
                    .map(|rows| rows.iter().filter(|r| r["ok"] == false).count())
                    .unwrap_or(0);
                let confirmed = value["cleanup_confirmed"] == true;
                let message = format!(
                    "성공 {succeeded} · 실패 {failed} · {}",
                    if confirmed {
                        "외부 자원 정리 확인됨"
                    } else {
                        "외부 자원 정리 미확인"
                    }
                );
                self.management
                    .feedback
                    .insert("global".into(), message.clone());
                if let Some(rows) = value["results"].as_array() {
                    for row in rows.iter().filter(|r| r["ok"] == true) {
                        if let Some(id) = row["id"].as_str() {
                            for batch in self.management.batches.values_mut() {
                                batch.remove(id);
                            }
                        }
                    }
                }
                if failed > 0 || succeeded == 0 {
                    if let Some(rows) = value["results"].as_array() {
                        let successful = rows
                            .iter()
                            .filter(|r| r["ok"] == true)
                            .filter_map(|r| r["id"].as_str())
                            .collect::<BTreeSet<_>>();
                        let retained = d
                            .ids
                            .iter()
                            .cloned()
                            .zip(d.names.iter().cloned())
                            .filter(|(id, _)| !successful.contains(id.as_str()))
                            .collect::<Vec<_>>();
                        d.ids = retained.iter().map(|(id, _)| id.clone()).collect();
                        d.names = retained.into_iter().map(|(_, name)| name).collect();
                        d.baseline.retain(|v| !successful.contains(v.id.as_str()));
                    }
                    d.error = Some(message);
                    d.reload_required = true;
                    d.preview = Some(value.clone());
                } else {
                    self.management.dialog = None;
                }
                self.management.monitor_last_poll = None;
                self.management.trash = None;
                self.management.trash_token = None;
                self.management
                    .requests
                    .retain(|_, r| !matches!(r, ManagementRequest::Trash));
            }
        }
        true
    }
    pub fn set_native_error(&mut self, token: &str, code: &str, message: &str) -> bool {
        let Some(request) = self.management.requests.remove(token) else {
            return false;
        };
        let text = format!("{code}: {message}");
        match request {
            ManagementRequest::TaskStopPreview { task_id, .. } => {
                self.management.feedback.insert(task_id, text);
            }
            ManagementRequest::Monitor(key) => {
                if let Some(id) = key.strip_prefix("detail:") {
                    if self
                        .management
                        .monitor_detail_tokens
                        .get(id)
                        .is_none_or(|active| active != token)
                    {
                        return false;
                    }
                    self.management.monitor_detail_tokens.remove(id);
                    self.management.feedback.insert(id.into(), text);
                    return true;
                }
                if self.management.monitor_latest.as_deref() == Some(token) {
                    self.management.monitor_latest = None;
                    self.management.monitor_error = Some(text);
                } else {
                    return false;
                }
            }
            ManagementRequest::Trash => {
                if self.management.trash_token.as_deref() == Some(token) {
                    self.management.trash_token = None;
                    self.management.feedback.insert("global".into(), text);
                } else {
                    return false;
                }
            }
            ManagementRequest::Mutation { nonce, ids } => {
                for id in ids {
                    self.management.feedback.insert(id, text.clone());
                }
                self.management
                    .feedback
                    .insert("global".into(), text.clone());
                if let Some(nonce) = nonce {
                    if let Some(d) =
                        self.management.dialog.as_mut().filter(|d| {
                            d.nonce == nonce && d.action_token.as_deref() == Some(token)
                        })
                    {
                        d.action_token = None;
                        d.error = Some(text);
                        d.reload_required = true;
                    }
                }
            }
            ManagementRequest::Preview(nonce) => {
                let Some(d) = self.management.dialog.as_mut().filter(|d| d.nonce == nonce) else {
                    return false;
                };
                if d.preview_token.as_deref() == Some(token) {
                    d.preview_token = None;
                } else if d.action_token.as_deref() == Some(token) {
                    d.action_token = None;
                } else {
                    return false;
                }
                d.error = Some(text);
                d.reload_required = true;
            }
        }
        true
    }
    fn apply_management(&mut self) {
        let Some(d) = self.management.dialog.as_ref() else {
            return;
        };
        if self.read_only
            || !d.confirmed
            || d.reload_required
            || d.preview_token.is_some()
            || d.action_token.is_some()
            || d.preview.as_ref().is_none_or(|p| p["can_apply"] != true)
        {
            return;
        }
        let action = d.action;
        let ids = d.ids.clone();
        let expected = d.versions.clone();
        let options = d.options.native();
        let nonce = d.nonce.clone();
        if action == ManagementAction::ArchiveRestore {
            let method = if ids.len() == 1 {
                "ui.restoreTaskArchive"
            } else {
                "ui.restoreTaskArchives"
            };
            let params = if ids.len() == 1 {
                json!({"id":ids[0]})
            } else {
                json!({"archiveIds":ids})
            };
            self.flow.queued.push(Effect::Command {
                method: method.into(),
                params,
                expected,
            });
            if let Some(d) = self.management.dialog.as_mut() {
                d.action_token = Some(format!("source:{method}"));
            }
            return;
        }
        let token = self.management_token();
        self.management.requests.insert(
            token.clone(),
            ManagementRequest::Mutation {
                nonce: Some(nonce),
                ids: ids.clone(),
            },
        );
        if let Some(d) = self.management.dialog.as_mut() {
            d.action_token = Some(token.clone());
        }
        self.flow.queued.push(match action {
            ManagementAction::TaskDelete => Effect::TaskDelete {
                ids,
                options,
                expected,
                token,
            },
            ManagementAction::ForceStop => Effect::RunControl {
                target: RunControlTarget::Run {
                    run_id: ids[0].clone(),
                },
                action: RunControlAction::ForceStop,
                expected,
                token,
            },
            _ => Effect::ConversationManage {
                ids,
                action: match action {
                    ManagementAction::ClearHistory => ConversationAction::ClearHistory,
                    ManagementAction::Restore => ConversationAction::Restore,
                    ManagementAction::Purge => ConversationAction::DeletePermanently,
                    _ => ConversationAction::Trash,
                },
                expected,
                token,
            },
        });
    }
    pub fn show_operation_dialog(
        &mut self,
        ctx: &egui::Context,
        snapshot: &Snapshot,
    ) -> Vec<Effect> {
        if self.management.trash_open {
            self.show_conversation_trash(ctx, snapshot);
        }
        let Some(mut d) = self.management.dialog.take() else {
            return vec![];
        };
        let mut close = false;
        let mut reload = false;
        let mut apply = false;
        let mut options_changed = false;
        let safe_rect = ctx.content_rect().shrink(12.0);
        egui::Window::new(d.action.label())
            .id(egui::Id::new(("native-management-confirm",&d.nonce)))
            .collapsible(false).resizable(true)
            .constrain_to(safe_rect)
            .default_width(520.0_f32.min(safe_rect.width()))
            .max_width(safe_rect.width())
            .default_height((safe_rect.height()-60.0).max(180.0))
            .max_height((safe_rect.height()-48.0).max(180.0))
            .show(ctx,|ui|{
            let busy=d.preview_token.is_some()||d.action_token.is_some();
            let body_height=(ui.available_height()-100.0).min(safe_rect.height()-180.0).max(80.0);
            egui::ScrollArea::vertical()
                .id_salt(("management-impact",&d.nonce))
                .min_scrolled_height(0.0)
                .max_height(body_height)
                .auto_shrink([false,true])
                .show(ui,|ui| {
            ui.strong(format!("{}개 항목",d.ids.len()));
            for name in d.names.iter().take(10){ui.add(egui::Label::new(name).wrap());}
            if d.names.len()>10{ui.small(format!("외 {}개",d.names.len()-10));}
            if d.action==ManagementAction::TaskDelete {
                ui.label("함께 정리할 자료를 선택하세요. 다른 작업이 참조하는 공유 자료는 보존합니다.");
                ui.add_enabled_ui(d.action_token.is_none()&&d.preview_token.is_none(),|ui|{
                    for (value,label) in [(&mut d.options.assets,"전용 자산"),(&mut d.options.findings,"발견 사항"),(&mut d.options.traffic,"전용 트래픽"),(&mut d.options.llm_records,"전용 모델 호출 기록"),(&mut d.options.files,"작업 전용 파일")]{options_changed|=ui.checkbox(value,label).changed();}
                });
            }
            if let Some(preview)=&d.preview {
                if let Some(counts)=preview["affected"].as_object(){ui.horizontal_wrapped(|ui|{for (key,value)in counts{let label=match key.as_str(){"activities"=>"활동","runs"=>"실행","questions"=>"질문","assets"=>"자산","traffic"=>"트래픽","findings"=>"발견사항","llm_records"=>"모델 기록",_=>key};ui.label(format!("{label} {value}"));}});}
                if let Some(files)=preview["owned_attachments"].as_array(){ui.label(format!("소유 범위의 디렉터리 후보 {}개",files.len()));ui.collapsing("대상 디렉터리 후보",|ui|{for file in files{ui.add(egui::Label::new(text(file,"relative_path")).wrap());}});}
                if preview["shared_preserved"].is_object(){ui.collapsing("보존되는 공유 자료",|ui|read_value(ui,&preview["shared_preserved"]));}
                if let Some(runs)=preview["requires_stop"].as_array(){if !runs.is_empty(){ui.colored_label(ui.visuals().warn_fg_color,format!("{}개 실행의 중지와 자원 정리 확인이 필요합니다.",runs.len()));}}
                if let Some(reason)=preview["blocked_reason"].as_str(){ui.add(egui::Label::new(reason).wrap());}
                if d.action==ManagementAction::ForceStop {
                    ui.small("이 앱이 소유한 외부 실행만 대상으로 합니다. 모델 요청은 취소 후 응답 반영을 차단하며 원격 모델 서버를 종료하지 않습니다.");
                }else{
                    ui.small("전용 파일은 작업 영역에서 격리되며 복구용 사본이 남을 수 있습니다. 공유 자료와 감사 기록은 보존합니다. 보안 삭제를 보장하지 않습니다.");
                }
                if d.action==ManagementAction::ForceStop{ui.collapsing("소유 실행과 정리 상태",|ui|read_value(ui,preview));}
            }
            if let Some(error)=&d.error{ui.add(egui::Label::new(RichText::new(error).color(ui.visuals().error_fg_color)).wrap());}
            if busy{ui.horizontal(|ui|{ui.spinner();ui.label(if d.action_token.is_some(){"처리 중 · 중복 요청은 보내지 않습니다."}else{"영향과 현재 버전을 확인 중…"});});}
            });
            ui.add_space(4.0);
            ui.add_enabled_ui(!self.read_only&&!busy&&!d.reload_required,|ui|{ui.checkbox(&mut d.confirmed,"표시한 범위와 자료 영향을 확인했습니다.");});
            ui.horizontal_wrapped(|ui|{
                if ui.add_enabled(!busy,egui::Button::new("닫기")).clicked(){close=true;}
                if ui.add_enabled(!busy,egui::Button::new("다시 확인")).clicked(){reload=true;}
                if ui.add_enabled(!self.read_only&&!busy&&d.confirmed&&!d.reload_required&&d.preview.as_ref().is_some_and(|p|p["can_apply"]==true),egui::Button::new(d.action.label())).clicked(){apply=true;}
            });
        });
        if !close {
            if reload {
                d.source_allowed = d.action != ManagementAction::ArchiveRestore
                    || d.ids.iter().all(|id| {
                        snapshot.get("task_archives", id).is_some_and(|r| {
                            matches!(text(&r.data, "state"), "ready" | "restore_failed")
                        })
                    });
                d.baseline = d
                    .ids
                    .iter()
                    .filter_map(|id| snapshot.get(d.action.collection(), id))
                    .map(expected)
                    .collect();
                d.confirmed = false;
                d.reload_required = false;
                d.error = None;
            }
            self.management.dialog = Some(d);
            if reload || options_changed {
                self.request_management_preview();
            }
            if apply {
                self.apply_management();
            }
        }
        vec![]
    }
    fn show_conversation_trash(&mut self, ctx: &egui::Context, snapshot: &Snapshot) {
        let mut open = true;
        let mut action = None;
        egui::Window::new("대화 휴지통")
            .id(egui::Id::new("native-conversation-trash"))
            .open(&mut open)
            .default_width(560.0)
            .show(ctx, |ui| {
                if ui.button("새로고침").clicked() {
                    self.request_conversation_trash();
                }
                if self.management.trash_token.is_some() {
                    ui.spinner();
                }
                let rows = self
                    .management
                    .trash
                    .as_ref()
                    .and_then(|v| v["items"].as_array().or_else(|| v.as_array()))
                    .cloned()
                    .unwrap_or_default();
                if self.management.trash.is_some() && rows.is_empty() {
                    ui.label("휴지통에 대화가 없습니다.");
                }
                egui::ScrollArea::vertical()
                    .max_height(300.0)
                    .show(ui, |ui| {
                        for row in rows {
                            let id = text(&row, "id");
                            ui.push_id(id, |ui| {
                                ui.horizontal_wrapped(|ui| {
                                    ui.label(text(&row, "title"));
                                    ui.small(presentation_time(text(&row, "trashed_at")));
                                    if ui
                                        .add_enabled(!self.read_only, egui::Button::new("복구…"))
                                        .clicked()
                                    {
                                        action = Some((ManagementAction::Restore, id.to_owned()));
                                    }
                                    if ui
                                        .add_enabled(
                                            !self.read_only,
                                            egui::Button::new("영구 삭제…"),
                                        )
                                        .clicked()
                                    {
                                        action = Some((ManagementAction::Purge, id.to_owned()));
                                    }
                                });
                            });
                        }
                    });
            });
        self.management.trash_open = open;
        if let Some((action, id)) = action {
            self.open_management(snapshot, action, vec![id]);
        }
    }
}

impl Panes {
    pub fn native_management_diagnostic(&self) -> Value {
        let d = self.management.dialog.as_ref();
        json!({"dialog_open":d.is_some(),"preview_received":d.is_some_and(|d|d.preview.is_some()),"preview_pending":d.is_some_and(|d|d.preview_token.is_some()),"action_pending":d.is_some_and(|d|d.action_token.is_some()),"error":d.and_then(|d|d.error.as_deref()),"action":d.map(|d|d.action.label()),"monitor_received":self.management.monitor.is_some(),"trash_received":self.management.trash.is_some()})
    }
    fn trash_contents(&mut self, ui: &mut egui::Ui, snapshot: &Snapshot) {
        if ui.button("휴지통 새로고침").clicked() {
            self.request_conversation_trash();
        }
        if self.management.trash.is_none() && self.management.trash_token.is_none() {
            self.request_conversation_trash();
        }
        if self.management.trash_token.is_some() {
            ui.horizontal(|ui| {
                ui.spinner();
                ui.label("휴지통 조회 중…");
            });
        }
        let rows = self
            .management
            .trash
            .as_ref()
            .and_then(|v| v["items"].as_array().or_else(|| v.as_array()))
            .cloned();
        if let Some(rows) = rows {
            if rows.is_empty() {
                ui.label("휴지통에 대화가 없습니다.");
            }
            for row in rows {
                let id = text(&row, "id");
                ui.push_id(id, |ui| {
                    ui.horizontal_wrapped(|ui| {
                        ui.strong(text(&row, "title"));
                        ui.small(presentation_time(text(&row, "trashed_at")));
                        if ui
                            .add_enabled(!self.read_only, egui::Button::new("복구…"))
                            .clicked()
                        {
                            self.open_management(
                                snapshot,
                                ManagementAction::Restore,
                                vec![id.into()],
                            );
                        }
                        if ui
                            .add_enabled(!self.read_only, egui::Button::new("영구 삭제…"))
                            .clicked()
                        {
                            self.open_management(
                                snapshot,
                                ManagementAction::Purge,
                                vec![id.into()],
                            );
                        }
                    });
                    ui.separator();
                });
            }
        }
    }
    fn request_agent_monitor(&mut self, force: bool) {
        if self.management.monitor_latest.is_some() {
            return;
        }
        if !force
            && self
                .management
                .monitor_last_poll
                .is_some_and(|stamp| stamp.elapsed() < std::time::Duration::from_secs(1))
        {
            return;
        }
        let token = self.management_token();
        self.management.monitor_latest = Some(token.clone());
        self.management.monitor_last_poll = Some(std::time::Instant::now());
        self.management.requests.insert(
            token.clone(),
            ManagementRequest::Monitor(format!("page:{}", self.management.monitor_offset)),
        );
        self.flow.queued.push(Effect::AgentMonitor {
            target: if self.management.monitor_offset == 0 {
                MonitorTarget::All
            } else {
                MonitorTarget::Page {
                    offset: self.management.monitor_offset,
                    limit: 200,
                }
            },
            token,
        });
    }
    fn native_target_busy(&self, id: &str) -> bool {
        self.management.requests.values().any(|r| {
            matches!(r,ManagementRequest::Mutation{ids,..} if ids.iter().any(|target|target==id))
                || matches!(r,ManagementRequest::TaskStopPreview{task_id,..} if task_id==id)
        })
    }
    fn stop_native_run(&mut self, row: &Value, task: bool, snapshot: &Snapshot) {
        let id = text(row, if task { "task_id" } else { "run_id" });
        if self.read_only || id.is_empty() || !monitor_can_stop(row) || self.native_target_busy(id)
        {
            return;
        }
        if task {
            let Some(parent) = snapshot.get("tasks", id) else {
                return;
            };
            let token = self.management_token();
            self.management.requests.insert(
                token.clone(),
                ManagementRequest::TaskStopPreview {
                    task_id: id.into(),
                    baseline: expected(parent),
                },
            );
            self.management.feedback.insert(
                id.into(),
                "중지 준비 중 · 전체 실행과 읽은 작업 버전 확인".into(),
            );
            self.flow.queued.push(Effect::AgentMonitor {
                target: MonitorTarget::Task { task_id: id.into() },
                token,
            });
            return;
        }
        let versions = native_expected(&row["expected"]);
        if versions.is_empty() {
            return;
        }
        let token = self.management_token();
        self.management.requests.insert(
            token.clone(),
            ManagementRequest::Mutation {
                nonce: None,
                ids: vec![id.into()],
            },
        );
        self.management.feedback.insert(
            id.into(),
            "중지 요청 중 · 외부 자원 정리는 아직 확인되지 않았습니다.".into(),
        );
        self.flow.queued.push(Effect::RunControl {
            target: if task {
                RunControlTarget::Task { task_id: id.into() }
            } else {
                RunControlTarget::Run { run_id: id.into() }
            },
            action: RunControlAction::Stop,
            expected: versions,
            token,
        });
    }
    fn agent_monitor(
        &mut self,
        ui: &mut egui::Ui,
        snapshot: &Snapshot,
        selected: Option<&Record>,
        effects: &mut Vec<Effect>,
    ) {
        self.request_agent_monitor(false);
        ui.ctx()
            .request_repaint_after(std::time::Duration::from_secs(1));
        ui.horizontal_wrapped(|ui| {
            ui.strong("에이전트 모니터");
            if ui.button("실행 상태 새로고침").clicked() {
                self.request_agent_monitor(true);
            }
            if self.management.monitor_latest.is_some() {
                ui.spinner();
            }
            let summary=self.management.monitor.clone();
            if let Some(value)=&summary {ui.small(format!("총 {} · 현재 {}개",value["total"],value["runs"].as_array().map_or(0,Vec::len)));}
            let mut offset=None;
            if ui.add_enabled(self.management.monitor_offset>0,egui::Button::new("이전 실행")).clicked(){offset=Some(self.management.monitor_offset.saturating_sub(200));}
            if ui.add_enabled(summary.as_ref().is_some_and(|v|v["has_more"]==true),egui::Button::new("다음 실행")).clicked(){offset=summary.as_ref().and_then(|v|v["next_offset"].as_u64()).and_then(|v|usize::try_from(v).ok());}
            if let Some(offset)=offset {self.management.monitor_offset=offset;self.management.monitor_latest=None;self.management.monitor_last_poll=None;self.management.monitor=None;self.management.requests.retain(|_,r|!matches!(r,ManagementRequest::Monitor(key) if !key.starts_with("detail:")));self.request_agent_monitor(true);}
        });
        if let Some(error) = &self.management.monitor_error {
            ui.add(
                egui::Label::new(RichText::new(error).color(ui.visuals().error_fg_color)).wrap(),
            );
        }
        let rows = self
            .management
            .monitor
            .as_ref()
            .and_then(|v| v["runs"].as_array())
            .cloned();
        let Some(rows) = rows else {
            ui.label("실행의 진행·소유 도구·정리 상태를 확인 중입니다.");
            return;
        };
        if rows.is_empty() {
            ui.label("기록된 에이전트 실행이 없습니다.");
        }
        for row in &rows {
            let rid = text(row, "run_id");
            let tid = text(row, "task_id");
            ui.push_id(("monitor-run", rid), |ui| {
                egui::Frame::new()
                    .fill(ui.visuals().faint_bg_color)
                    .inner_margin(6.0)
                    .corner_radius(6.0)
                    .show(ui, |ui| {
                        ui.set_width(ui.available_width());
                        ui.spacing_mut().item_spacing.y = 3.0;
                        ui.spacing_mut().interact_size.y = 22.0;
                        ui.spacing_mut().button_padding.y = 3.0;
                        let agent = snapshot.get("agents", text(row, "agent_id"))
                            .map(display_name).unwrap_or_else(|| "에이전트 정보 없음".into());
                        ui.horizontal_wrapped(|ui| {
                            if ui.add(egui::Button::selectable(
                                selected.is_some_and(|r| r.header.id == rid), &agent,
                            ).wrap_mode(egui::TextWrapMode::Truncate))
                                .on_hover_text(format!("{agent}\n{rid}")).clicked()
                            {
                                effects.push(Effect::Select { section: Section::Sessions, id: rid.into() });
                            }
                            let state = text(row, "state");
                            ui.small(if matches!(state,"cancelled"|"stopped") && row["cleanup_confirmed"] != true {
                                "중지됨 · 정리 미확인"
                            } else { state_label(state) });
                            ui.small(monitor_stage(text(row,"stage")));
                        });
                        if let Some(task) = snapshot.get("tasks", tid) {
                            let name = display_name(task);
                            ui.add(egui::Label::new(format!("작업 · {name}")).truncate()).on_hover_text(name);
                        }
                        if let Some(reason)=row["wait_reason"].as_str().filter(|s|!s.is_empty()) {
                            ui.add(egui::Label::new(reason).truncate()).on_hover_text(reason);
                        }
                        ui.horizontal_wrapped(|ui| {
                            for (key,label) in [("last_progress_at","진행"),("heartbeat_at","하트비트")] {
                                if let Some(stamp)=row[key].as_str() {
                                    ui.small(format!("{label} {}",presentation_short_time(stamp)))
                                        .on_hover_text(presentation_time(stamp));
                                } else { ui.small(format!("{label} 미기록")); }
                            }
                            if let Some(ms)=row["elapsed_ms"].as_u64().or_else(||row["elapsed_ms"].as_str().and_then(|v|v.parse().ok())) {
                                ui.small(format!("경과 {}초",ms/1000));
                            }
                        });
                        ui.horizontal_wrapped(|ui| {
                            if let Some(tool) = snapshot.records("tool_invocations").into_iter()
                                .filter(|t|t.data["run_id"]==rid)
                                .max_by(|a,b|a.header.updated_at.cmp(&b.header.updated_at))
                            {
                                ui.small(format!("도구 {} · {}",text(&tool.data,"name"),state_label(text(&tool.data,"state"))));
                            }
                            ui.small(if row["cleanup_confirmed"]==true { "정리 확인됨" } else { "정리 미확인" });
                            ui.small(monitor_cleanup(text(row,"cleanup_state")));
                            let busy=self.native_target_busy(rid)||(!tid.is_empty()&&self.native_target_busy(tid));
                            if busy { ui.spinner(); ui.small("중지 요청 중…"); }
                        });
                        let busy=self.native_target_busy(rid)||(!tid.is_empty()&&self.native_target_busy(tid));
                        let force_label=if ui.available_width()<280.0 {"강제 종료…"} else {"외부 프로세스 강제 종료…"};
                        ui.horizontal_wrapped(|ui| {
                            if ui.add_enabled(!self.read_only&&!busy&&monitor_can_stop(row),
                                egui::Button::new("실행 중지").wrap_mode(egui::TextWrapMode::Extend)).clicked()
                            { self.stop_native_run(row,false,snapshot); }
                            if ui.add_enabled(!self.read_only&&!busy&&monitor_can_force_stop(row),
                                egui::Button::new(force_label).wrap_mode(egui::TextWrapMode::Extend))
                                .on_hover_text("이 앱이 소유한 외부 프로세스만 강제 종료합니다. 영향을 먼저 확인합니다.").clicked()
                            { self.open_management(snapshot,ManagementAction::ForceStop,vec![rid.into()]); }
                        });
                    });
            });
        }
        if let Some(run) = selected {
            ui.horizontal_wrapped(|ui| {
                if ui.button("선택 실행 상세 새로고침").clicked() {
                    self.request_monitor_detail(&run.header.id, true);
                }
                if self
                    .management
                    .monitor_detail_tokens
                    .contains_key(&run.header.id)
                {
                    ui.spinner();
                }
            });
            let detail = self
                .management
                .monitor_details
                .get(&run.header.id)
                .and_then(|v| v["runs"].as_array())
                .and_then(|runs| runs.iter().find(|r| r["run_id"] == run.header.id))
                .cloned();
            let row =
                detail.or_else(|| rows.iter().find(|r| r["run_id"] == run.header.id).cloned());
            if row.is_none() {
                self.request_monitor_detail(&run.header.id, false);
            }
            if let Some(row) = row.as_ref() {
                ui.horizontal_wrapped(|ui| {
                    let tid = text(row, "task_id");
                    if !tid.is_empty()
                        && ui
                            .add_enabled(
                                !self.read_only
                                    && !self.native_target_busy(tid)
                                    && monitor_can_stop(row),
                                egui::Button::new("이 작업의 모든 실행 중지"),
                            )
                            .clicked()
                    {
                        self.stop_native_run(row, true, snapshot);
                    }
                });
                if let Some(feedback) = self.management.feedback.get(&run.header.id) {
                    ui.add(egui::Label::new(feedback).wrap());
                }
                for tool in snapshot
                    .records("tool_invocations")
                    .into_iter()
                    .filter(|t| t.data["run_id"] == run.header.id && t.data["state"] == "completed")
                    .take(20)
                {
                    ui.push_id((&run.header.id, &tool.header.id), |ui| {
                    ui.collapsing(format!("저장된 도구 결과 · {}",text(&tool.data,"name")),|ui|{
                        let result=&tool.data["result"];
                        let output=body_text(&result["text"]);
                        if output.is_empty(){read_value(ui,result);}else{ui.add(egui::Label::new(output.chars().take(4000).collect::<String>()).wrap().selectable(true));if output.chars().count()>4000{ui.small("일부만 미리보기로 표시합니다. 대화 이력에서 전체 저장 결과를 확인하세요.");}}
                    });
                    });
                }
                let cid = text(&run.data, "conversation_id");
                if !cid.is_empty() && ui.button("대화에서 도구 결과·출력 확인").clicked()
                {
                    effects.push(Effect::Select {
                        section: Section::Chat,
                        id: cid.into(),
                    });
                }
                ui.small("저장된 결과는 대화 이력·실행 기록에서 확인하세요. 실행 중 출력의 실시간 표시는 지원하지 않습니다.");
                if let Some(executions) = row["executions"].as_array() {
                    for execution in executions {
                        execution_disclosure(ui, &run.header.id, execution);
                    }
                }
            }
        }
        ui.separator();
    }
}
fn monitor_stage(stage: &str) -> &str {
    match stage {
        "queued" => "실행 대기",
        "model" | "llm" | "llm_running" | "model_request" => "모델 응답 대기",
        "tool" | "tool_running" => "도구 실행",
        "waiting" | "waiting_user" => "사용자 응답 대기",
        "terminal" | "finished" => "모델 실행 종료",
        "recovering" => "재시작 상태 확인",
        "controller" => "대화 제어",
        "stopping" => "중지·정리 확인 중",
        "paused" => "일시정지",
        _ => stage,
    }
}
fn monitor_cleanup(state: &str) -> &str {
    match state {
        "running" => "실행 중",
        "stopping" | "cancel_requested" => "중지 요청됨",
        "confirmed" | "cleanup_confirmed" => "정리 확인됨",
        "stopped" => "프로세스 중지 상태",
        "unknown" | "uncertain" | "unconfirmed" => "정리 여부 미확인",
        "completed" | "exited" => "프로세스 종료",
        _ => state,
    }
}

impl Panes {
    fn complete_archive_management(
        &mut self,
        method: &str,
        params: &Value,
        value: Option<&Value>,
        error: Option<String>,
    ) {
        if !matches!(method, "ui.restoreTaskArchive" | "ui.restoreTaskArchives") {
            return;
        }
        let ids = if let Some(id) = params["id"].as_str() {
            vec![id.to_owned()]
        } else {
            params["archiveIds"]
                .as_array()
                .map(|ids| {
                    ids.iter()
                        .filter_map(Value::as_str)
                        .map(str::to_owned)
                        .collect()
                })
                .unwrap_or_default()
        };
        let source_token = format!("source:{method}");
        let Some(d) = self.management.dialog.as_mut().filter(|d| {
            d.action == ManagementAction::ArchiveRestore
                && d.ids == ids
                && d.action_token.as_deref() == Some(source_token.as_str())
        }) else {
            return;
        };
        d.action_token = None;
        if let Some(error) = error {
            d.error = Some(error);
            d.reload_required = true;
        } else {
            let failed = value
                .and_then(|v| v["items"].as_array())
                .map(|items| items.iter().filter(|i| i["ok"] == false).count())
                .unwrap_or(0);
            if failed > 0 {
                d.error = Some(format!(
                    "복원 요청 {failed}개가 거부되었습니다. 보관 작업 상태를 다시 확인하세요."
                ));
                d.reload_required = true;
            } else {
                self.management.feedback.insert("global".into(),"보관 작업 복원 요청을 접수했습니다. 실제 파일 복원 상태는 보관 작업에서 확인하세요.".into());
                self.management.dialog = None;
            }
        }
    }
}

impl Panes {
    fn request_monitor_detail(&mut self, id: &str, force: bool) {
        if self.management.monitor_detail_tokens.contains_key(id)
            || (!force && self.management.monitor_details.contains_key(id))
        {
            return;
        }
        let token = self.management_token();
        self.management
            .monitor_detail_tokens
            .insert(id.into(), token.clone());
        self.management.requests.insert(
            token.clone(),
            ManagementRequest::Monitor(format!("detail:{id}")),
        );
        self.flow.queued.push(Effect::AgentMonitor {
            target: MonitorTarget::Run { run_id: id.into() },
            token,
        });
    }
}

#[cfg(test)]
mod native_management_tests {
    use super::*;
    pub(super) fn record(collection: &str, patch: Value) -> Record {
        let id = artex_core::new_id(artex_core::collection_prefix(collection).unwrap());
        let mut data = artex_core::default_data(collection).unwrap();
        for (key, value) in patch.as_object().unwrap() {
            data[key] = value.clone();
        }
        if data.get("id").is_some() {
            data["id"] = json!(id);
        }
        serde_json::from_value(json!({"header":{"id":id,"collection":collection,"version":"7","commit_seq":"9","created_at":"2026-10-14T00:00:00Z","updated_at":"2026-10-14T00:00:00Z","legacy_key":null},"data":data})).unwrap()
    }
    pub(super) fn snapshot(rows: &[Record]) -> Snapshot {
        let mut s = Snapshot {
            workspace_id: artex_core::new_id("workspace"),
            commit_seq: 9,
            collections: BTreeMap::new(),
        };
        for r in rows {
            s.collections
                .entry(r.header.collection.clone())
                .or_default()
                .insert(r.header.id.clone(), r.clone());
        }
        s
    }
    fn preview(ids: &[&Record], extra: &[Expected]) -> Value {
        let mut versions = ids.iter().map(|r| expected(r)).collect::<Vec<_>>();
        versions.extend_from_slice(extra);
        json!({"expected":versions,"targets":ids.iter().map(|r|json!({"id":r.header.id,"name":display_name(r)})).collect::<Vec<_>>(),"affected":{"activities":3,"runs":1,"questions":1},"owned_attachments":[{"owner_id":ids[0].header.id,"relative_path":"attachments/evidence.txt"}],"shared_preserved":{"assets":2,"traffic":1,"tool_audit":4},"requires_stop":[],"can_apply":true,"cleanup_confirmed":true,"secure_erase":false})
    }
    fn dialog_frame(
        ctx: &egui::Context,
        panes: &mut Panes,
        snapshot: &Snapshot,
        events: Vec<egui::Event>,
    ) -> egui::FullOutput {
        ctx.run(
            egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(900.0, 620.0),
                )),
                events,
                ..Default::default()
            },
            |ctx| {
                let effects = panes.show_operation_dialog(ctx, snapshot);
                assert!(effects.is_empty());
            },
        )
    }
    fn last_text(output: &egui::FullOutput, label: &str) -> egui::Pos2 {
        output
            .shapes
            .iter()
            .filter_map(|s| match &s.shape {
                egui::epaint::Shape::Text(t) if t.galley.text() == label => {
                    Some(t.visual_bounding_rect().center())
                }
                _ => None,
            })
            .max_by(|a, b| a.y.total_cmp(&b.y))
            .unwrap_or_else(|| panic!("Missing {label}"))
    }
    fn click_dialog(ctx: &egui::Context, panes: &mut Panes, snapshot: &Snapshot, label: &str) {
        let _ = dialog_frame(ctx, panes, snapshot, vec![]);
        let output = dialog_frame(ctx, panes, snapshot, vec![]);
        let pos = last_text(&output, label);
        for pressed in [true, false] {
            let _ = dialog_frame(
                ctx,
                panes,
                snapshot,
                vec![
                    egui::Event::PointerMoved(pos),
                    egui::Event::PointerButton {
                        pos,
                        button: egui::PointerButton::Primary,
                        pressed,
                        modifiers: egui::Modifiers::NONE,
                    },
                ],
            );
        }
    }
    #[test]
    fn actual_delete_confirmation_preserves_preview_versions_readonly_and_late_other_chat_draft() {
        let a = record("conversations", json!({"title":"Delete target A"}));
        let b = record("conversations", json!({"title":"Keep B"}));
        let asset = record("assets", json!({"type":"domain","domain":"shared.test"}));
        let s = snapshot(&[a.clone(), b.clone(), asset.clone()]);
        let original = s.clone();
        let ctx = egui::Context::default();
        crate::app::configure(&ctx);
        let mut panes = Panes::default();
        panes.set_context(Section::Chat, Some(&a.header.id));
        panes.draft_message = "Keep A draft".into();
        panes.open_management(&s, ManagementAction::Trash, vec![a.header.id.clone()]);
        let effects = panes.drain_effects();
        let token = match &effects[0] {
            Effect::DeletionPreview {
                target: DeletionTarget::Conversations { ids },
                options:
                    DeletionOptions::Conversations {
                        action: ConversationAction::Trash,
                    },
                token,
            } => {
                assert_eq!(ids, &vec![a.header.id.clone()]);
                token.clone()
            }
            _ => panic!("Menu confirmation must only request the real native preview"),
        };
        assert_eq!(effects.len(), 1);
        let pinned = preview(&[&a], &[expected(&asset)]);
        assert!(panes.set_native_result(&token, &pinned));
        click_dialog(
            &ctx,
            &mut panes,
            &s,
            "표시한 범위와 자료 영향을 확인했습니다.",
        );
        panes.set_read_only(true);
        click_dialog(&ctx, &mut panes, &s, ManagementAction::Trash.label());
        assert!(panes.drain_effects().is_empty());
        panes.set_read_only(false);
        click_dialog(&ctx, &mut panes, &s, ManagementAction::Trash.label());
        let sent = panes.drain_effects();
        let token = match &sent[0] {
            Effect::ConversationManage {
                ids,
                action: ConversationAction::Trash,
                expected,
                token,
            } => {
                assert_eq!(ids, &vec![a.header.id.clone()]);
                assert_eq!(expected, &native_expected(&pinned["expected"]));
                token.clone()
            }
            _ => panic!("The confirmed operation must retain its exact preview CAS vector"),
        };
        assert_eq!(sent.len(), 1);
        panes.apply_management();
        assert!(panes.drain_effects().is_empty());
        panes.set_context(Section::Chat, Some(&b.header.id));
        panes.draft_message = "Unsent B draft".into();
        assert!(panes.set_native_result(&token,&json!({"accepted":true,"cleanup_confirmed":true,"results":[{"id":a.header.id,"ok":true,"trashed":true}]})));
        assert_eq!(panes.draft_message, "Unsent B draft");
        assert!(panes.management.dialog.is_none());
        panes.set_context(Section::Chat, Some(&a.header.id));
        assert_eq!(panes.draft_message, "Keep A draft");
        assert_eq!(s, original);
    }
    #[test]
    fn stale_preview_and_mutation_error_never_rebase_or_retry_and_retired_replies_are_rejected() {
        let a = record("conversations", json!({"title":"Pinned target"}));
        let s = snapshot(&[a.clone()]);
        let mut panes = Panes::default();
        panes.open_management(
            &s,
            ManagementAction::ClearHistory,
            vec![a.header.id.clone()],
        );
        let token = match panes.drain_effects().remove(0) {
            Effect::DeletionPreview { token, .. } => token,
            _ => panic!(),
        };
        let mut changed = preview(&[&a], &[]);
        changed["expected"][0]["version"] = json!("8");
        panes.set_native_result(&token, &changed);
        assert!(panes.management.dialog.as_ref().unwrap().reload_required);
        assert_eq!(
            panes.management.dialog.as_ref().unwrap().baseline,
            vec![expected(&a)]
        );
        panes.management.dialog.as_mut().unwrap().confirmed = true;
        panes.apply_management();
        assert!(panes.drain_effects().is_empty());
        panes.open_management(&s, ManagementAction::Trash, vec![a.header.id.clone()]);
        let token = match panes.drain_effects().remove(0) {
            Effect::DeletionPreview { token, .. } => token,
            _ => panic!(),
        };
        panes.set_native_result(&token, &preview(&[&a], &[]));
        panes.management.dialog.as_mut().unwrap().confirmed = true;
        panes.apply_management();
        let token = match panes.drain_effects().remove(0) {
            Effect::ConversationManage { token, .. } => token,
            _ => panic!(),
        };
        panes.set_native_error(&token, "VERSION_CONFLICT", "The read version changed");
        assert!(panes.management.dialog.as_ref().unwrap().reload_required);
        panes.apply_management();
        assert!(panes.drain_effects().is_empty());
        panes.request_conversation_trash();
        let token = match panes.drain_effects().remove(0) {
            Effect::ConversationTrash { token } => token,
            _ => panic!(),
        };
        panes.retire_async_operations();
        assert!(!panes.set_native_result(&token, &json!({"items":[]})));
    }
    #[test]
    fn batch_partial_success_retains_failed_selection_and_clear_history_has_its_own_action() {
        let a = record("conversations", json!({"title":"A"}));
        let b = record("conversations", json!({"title":"B"}));
        let s = snapshot(&[a.clone(), b.clone()]);
        let mut panes = Panes::default();
        panes.management.batches.insert(
            Section::Chat,
            BTreeSet::from([a.header.id.clone(), b.header.id.clone()]),
        );
        panes.open_management(
            &s,
            ManagementAction::Trash,
            vec![a.header.id.clone(), b.header.id.clone()],
        );
        let token = match panes.drain_effects().remove(0) {
            Effect::DeletionPreview { token, .. } => token,
            _ => panic!(),
        };
        panes.set_native_result(&token, &preview(&[&a, &b], &[]));
        panes.management.dialog.as_mut().unwrap().confirmed = true;
        panes.apply_management();
        let token = match panes.drain_effects().remove(0) {
            Effect::ConversationManage { token, .. } => token,
            _ => panic!(),
        };
        panes.set_native_result(&token,&json!({"accepted":true,"cleanup_confirmed":false,"results":[{"id":a.header.id,"ok":true},{"id":b.header.id,"ok":false,"error":"cleanup unknown"}]}));
        assert_eq!(
            panes.management.batches[&Section::Chat],
            BTreeSet::from([b.header.id.clone()])
        );
        assert_eq!(
            panes.management.dialog.as_ref().unwrap().ids,
            vec![b.header.id.clone()]
        );
        assert!(panes.management.dialog.as_ref().unwrap().reload_required);
        panes.open_management(
            &s,
            ManagementAction::ClearHistory,
            vec![b.header.id.clone()],
        );
        let token = match panes.drain_effects().remove(0) {
            Effect::DeletionPreview {
                token,
                options:
                    DeletionOptions::Conversations {
                        action: ConversationAction::ClearHistory,
                    },
                ..
            } => token,
            _ => panic!(),
        };
        panes.set_native_result(&token, &preview(&[&b], &[]));
        panes.management.dialog.as_mut().unwrap().confirmed = true;
        panes.apply_management();
        assert!(matches!(
            panes.drain_effects().remove(0),
            Effect::ConversationManage {
                action: ConversationAction::ClearHistory,
                ..
            }
        ));
    }
    #[test]
    fn immediate_stop_captures_runtime_cas_disables_duplicates_and_does_not_claim_cleanup() {
        let task = record("tasks", json!({"name":"Owned task","status":"running"}));
        let runtime = record(
            "task_runtime",
            json!({"task_id":task.header.id,"remaining_timeout_ms":"10000"}),
        );
        let run = record(
            "agent_runs",
            json!({"state":"running","task_id":task.header.id}),
        );
        let s = snapshot(&[task.clone(), runtime.clone(), run.clone()]);
        let row = json!({"state":"running","run_id":run.header.id,"task_id":task.header.id,"can_stop":true,"can_force_stop":false,"cleanup_confirmed":false,"expected":[expected(&run),expected(&task)]});
        let mut panes = Panes::default();
        panes.management.monitor = Some(json!({"runs":[row.clone()]}));
        panes.stop_native_run(&row, true, &s);
        let preview_token = match panes.drain_effects().remove(0) {
            Effect::AgentMonitor {
                target: MonitorTarget::Task { task_id },
                token,
            } => {
                assert_eq!(task_id, task.header.id);
                token
            }
            _ => panic!(),
        };
        assert!(panes.native_target_busy(&task.header.id));
        assert!(panes.management.dialog.is_none());
        panes.stop_native_run(&row, true, &s);
        assert!(panes.drain_effects().is_empty());
        panes.set_native_result(
            &preview_token,
            &json!({"task_control_expected":[expected(&run),expected(&task),expected(&runtime)]}),
        );
        let effect = panes.drain_effects().remove(0);
        let token = match effect {
            Effect::RunControl {
                target: RunControlTarget::Task { task_id },
                action: RunControlAction::Stop,
                expected: versions,
                token,
            } => {
                assert_eq!(task_id, task.header.id);
                assert!(versions.contains(&expected(&runtime)));
                token
            }
            _ => panic!(),
        };
        assert!(panes.native_target_busy(&task.header.id));
        panes.stop_native_run(&row, true, &s);
        assert!(panes.drain_effects().is_empty());
        panes.set_native_result(&token, &json!({"accepted":true,"cleanup_confirmed":false}));
        assert!(!panes.native_target_busy(&task.header.id));
        assert!(panes.management.feedback[&task.header.id].contains("정리 미확인"));
        panes.set_read_only(true);
        panes.stop_native_run(&row, false, &s);
        assert!(panes.drain_effects().is_empty());
    }
}

impl Panes {
    pub fn open_smoke_trash(&mut self) {
        self.management.archive_tab = 1;
        self.request_conversation_trash();
    }
}

fn monitor_can_stop(row: &Value) -> bool {
    row["can_stop"] == true
        && (matches!(
            text(row, "state"),
            "queued" | "running" | "waiting" | "paused" | "interrupted"
        ) || row["executions"]
            .as_array()
            .is_some_and(|items| items.iter().any(|e| e["cleanup_confirmed"] != true)))
}
fn monitor_can_force_stop(row: &Value) -> bool {
    row["can_force_stop"] == true
        && row["executions"].as_array().is_some_and(|items| {
            items
                .iter()
                .any(|e| e["cleanup_confirmed"] != true && !text(e, "execution_id").is_empty())
        })
}

#[cfg(test)]
mod monitor_widget_tests {
    use super::native_management_tests::{record, snapshot};
    use super::*;
    #[test]
    fn actual_monitor_click_is_immediate_owned_pending_and_force_requires_owned_external_confirmation()
     {
        let agent = record("agents", json!({"key":"worker","name":"Actual worker"}));
        let task = record("tasks", json!({"name":"Actual task","status":"running"}));
        let run = record(
            "agent_runs",
            json!({"state":"running","agent_id":agent.header.id,"task_id":task.header.id}),
        );
        let tool = record(
            "tool_invocations",
            json!({"run_id":run.header.id,"name":"TaskOutput","state":"completed","result":{"text":{"text":"Stored tool output","blob":null}}}),
        );
        let s = snapshot(&[agent, task, run.clone(), tool]);
        let original = s.clone();
        let row = json!({"run_id":run.header.id,"agent_id":run.data["agent_id"],"task_id":run.data["task_id"],"state":"running","stage":"model","heartbeat_at":null,"last_progress_at":"2026-10-14T00:00:00Z","cleanup_confirmed":true,"cleanup_state":"confirmed","can_stop":true,"can_force_stop":true,"executions":[],"expected":[expected(&run)]});
        let ctx = egui::Context::default();
        crate::app::configure(&ctx);
        let mut panes = Panes::default();
        panes.set_context(Section::Sessions, Some(&run.header.id));
        panes.management.monitor = Some(json!({"runs":[row],"total":1,"has_more":false}));
        let render = |panes: &mut Panes, events: Vec<egui::Event>| {
            panes.management.monitor_last_poll = Some(std::time::Instant::now());
            let mut effects = vec![];
            let output = ctx.run(
                egui::RawInput {
                    screen_rect: Some(egui::Rect::from_min_size(
                        egui::Pos2::ZERO,
                        egui::vec2(900.0, 620.0),
                    )),
                    events,
                    ..Default::default()
                },
                |ctx| {
                    egui::CentralPanel::default().show(ctx, |ui| {
                        egui::ScrollArea::vertical().show(ui, |ui| {
                            panes.agent_monitor(ui, &s, Some(&run), &mut effects)
                        });
                    });
                },
            );
            assert!(effects.is_empty());
            output
        };
        let locate = |out: &egui::FullOutput, label: &str| {
            out.shapes
                .iter()
                .find_map(|shape| match &shape.shape {
                    egui::epaint::Shape::Text(t) if t.galley.text() == label => {
                        Some(t.visual_bounding_rect().center())
                    }
                    _ => None,
                })
                .unwrap_or_else(|| panic!("Missing {label}"))
        };
        let initial = render(&mut panes, vec![]);
        assert!(initial.shapes.iter().any(|s|matches!(&s.shape,egui::epaint::Shape::Text(t) if t.galley.text()=="하트비트 미기록")));
        let click = |panes: &mut Panes, label: &str| {
            let out = render(panes, vec![]);
            let pos = locate(&out, label);
            for pressed in [true, false] {
                let _ = render(
                    panes,
                    vec![
                        egui::Event::PointerMoved(pos),
                        egui::Event::PointerButton {
                            pos,
                            button: egui::PointerButton::Primary,
                            pressed,
                            modifiers: egui::Modifiers::NONE,
                        },
                    ],
                );
            }
        };
        click(&mut panes, "외부 프로세스 강제 종료…");
        assert!(
            panes.management.dialog.is_none(),
            "A model request without an owned process cannot expose force termination"
        );
        click(&mut panes, "실행 중지");
        let token = match panes.drain_effects().remove(0) {
            Effect::RunControl {
                target: RunControlTarget::Run { run_id },
                action: RunControlAction::Stop,
                expected: versions,
                token,
            } => {
                assert_eq!(run_id, run.header.id);
                assert_eq!(versions, vec![expected(&run)]);
                token
            }
            _ => panic!(),
        };
        assert!(panes.management.dialog.is_none());
        assert!(panes.native_target_busy(&run.header.id));
        click(&mut panes, "실행 중지");
        assert!(panes.drain_effects().is_empty());
        panes.set_native_result(&token, &json!({"accepted":true,"cleanup_confirmed":false}));
        panes.set_read_only(true);
        click(&mut panes, "실행 중지");
        assert!(panes.drain_effects().is_empty());
        panes.set_read_only(false);
        panes.management.monitor.as_mut().unwrap()["runs"][0]["executions"] = json!([{"execution_id":"exec_owned","state":"running","pid":123,"cleanup_confirmed":false}]);
        panes.management.monitor.as_mut().unwrap()["runs"][0]["cleanup_confirmed"] = json!(false);
        click(&mut panes, "외부 프로세스 강제 종료…");
        assert_eq!(
            panes.management.dialog.as_ref().unwrap().action,
            ManagementAction::ForceStop
        );
        assert!(
            panes.drain_effects().is_empty(),
            "Force termination requires separate impact confirmation"
        );
        assert_eq!(s, original);
    }
    #[test]
    fn task_stop_prepare_rejects_a_changed_task_version_instead_of_rebasing_the_click() {
        let task = record("tasks", json!({"name":"Pinned task","status":"running"}));
        let run = record(
            "agent_runs",
            json!({"state":"running","task_id":task.header.id}),
        );
        let s = snapshot(&[task.clone(), run.clone()]);
        let row = json!({"run_id":run.header.id,"task_id":task.header.id,"state":"running","can_stop":true,"expected":[expected(&run)]});
        let mut panes = Panes::default();
        panes.stop_native_run(&row, true, &s);
        let token = match panes.drain_effects().remove(0) {
            Effect::AgentMonitor { token, .. } => token,
            _ => panic!(),
        };
        let mut changed = expected(&task);
        changed.version = "8".into();
        panes.set_native_result(
            &token,
            &json!({"task_control_expected":[changed,expected(&run)]}),
        );
        assert!(panes.drain_effects().is_empty());
        assert!(!panes.native_target_busy(&task.header.id));
        assert!(panes.management.feedback[&task.header.id].contains("VERSION_CONFLICT"));
    }
    #[test]
    fn actual_two_run_monitor_cards_keep_controls_and_selected_detail_readable_at_615_and_320_widths()
     {
        for width in [615.0, 320.0] {
            let agent_a = record(
                "agents",
                json!({"key":"alpha","name":"Monitor worker alpha"}),
            );
            let agent_b = record("agents", json!({"key":"beta","name":"Monitor worker beta"}));
            let task = record("tasks", json!({"name":"Observed task","status":"running"}));
            let run_a = record(
                "agent_runs",
                json!({"agent_id":agent_a.header.id,"task_id":task.header.id,"state":"running"}),
            );
            let run_b = record(
                "agent_runs",
                json!({"agent_id":agent_b.header.id,"task_id":task.header.id,"state":"waiting"}),
            );
            let tool = record(
                "tool_invocations",
                json!({"run_id":run_a.header.id,"name":"Bash","state":"running"}),
            );
            let s = snapshot(&[
                agent_a.clone(),
                agent_b.clone(),
                task,
                run_a.clone(),
                run_b.clone(),
                tool,
            ]);
            let original = s.clone();
            let rows=[&run_a,&run_b].map(|run|json!({"run_id":run.header.id,"agent_id":run.data["agent_id"],"task_id":run.data["task_id"],"state":run.data["state"],"stage":"tool","last_progress_at":"2026-10-14T00:00:00Z","heartbeat_at":null,"cleanup_confirmed":false,"cleanup_state":"unconfirmed","elapsed_ms":12000,"can_stop":true,"can_force_stop":true,"expected":[expected(run)],"executions":[{"execution_id":format!("exec_{}",run.header.id),"state":"running","cleanup_confirmed":false}]}));
            let ctx = egui::Context::default();
            crate::app::configure(&ctx);
            let mut panes = Panes::default();
            panes.set_context(Section::Sessions, Some(&run_a.header.id));
            panes.draft_message = "Unrelated unsent draft".into();
            panes.management.monitor = Some(json!({"runs":rows,"total":2,"has_more":false}));
            panes.management.monitor_last_poll = Some(std::time::Instant::now());
            let mut output = None;
            for _ in 0..2 {
                let mut effects = vec![];
                output = Some(ctx.run(
                    egui::RawInput {
                        screen_rect: Some(egui::Rect::from_min_size(
                            egui::Pos2::ZERO,
                            egui::vec2(width + 40.0, 620.0),
                        )),
                        ..Default::default()
                    },
                    |ctx| {
                        egui::CentralPanel::default().show(ctx, |ui| {
                            ui.set_width(width);
                            egui::ScrollArea::vertical()
                                .auto_shrink([false, false])
                                .show(ui, |ui| {
                                    ui.set_width(width);
                                    panes.agent_monitor(ui, &s, Some(&run_a), &mut effects);
                                });
                        });
                    },
                ));
                assert!(effects.is_empty());
            }
            let output = output.unwrap();
            let labels = |label: &str| {
                output
                    .shapes
                    .iter()
                    .filter_map(|shape| match &shape.shape {
                        egui::epaint::Shape::Text(text) if text.galley.text() == label => {
                            Some((text.visual_bounding_rect(), shape.clip_rect))
                        }
                        _ => None,
                    })
                    .collect::<Vec<_>>()
            };
            let force_label = if width - 12.0 < 280.0 {
                "강제 종료…"
            } else {
                "외부 프로세스 강제 종료…"
            };
            for label in ["실행 중지", force_label, "하트비트 미기록", "정리 미확인"]
            {
                let found = labels(label);
                assert_eq!(
                    found.len(),
                    2,
                    "Both runs must expose {label} at width {width}"
                );
                for (bounds, clip) in found {
                    assert!(
                        bounds.height() < 24.0,
                        "Single-character wrapping: {label} {bounds:?}"
                    );
                    assert!(
                        bounds.left() >= clip.left() - 1.0 && bounds.right() <= clip.right() + 1.0,
                        "Clipped {label} at width {width}: {bounds:?} vs {clip:?}"
                    );
                    assert!(
                        bounds.bottom() <= clip.bottom() + 1.0,
                        "Controls must be accessible before the fold: {label}"
                    );
                }
            }
            let first = labels("Monitor worker alpha")[0].0;
            let second = labels("Monitor worker beta")[0].0;
            assert!(
                second.top() > first.bottom(),
                "Run cards may not concatenate horizontally"
            );
            if width == 615.0 {
                assert!(
                    second.top() - first.top() < 155.0,
                    "A normal-width run card must stay compact"
                );
            }
            let detail = labels("선택 실행 상세 새로고침");
            assert_eq!(detail.len(), 1);
            assert!(
                detail[0].0.bottom() <= detail[0].1.bottom() + 1.0,
                "Selected detail must remain accessible"
            );
            assert_eq!(panes.draft_message, "Unrelated unsent draft");
            assert!(panes.drain_effects().is_empty());
            assert_eq!(s, original);
        }
    }
}

fn execution_title(execution: &Value) -> &str {
    for key in ["tool_name", "executable", "kind", "execution_id"] {
        if let Some(value) = execution[key]
            .as_str()
            .filter(|value| !value.trim().is_empty())
        {
            return value;
        }
    }
    "실행 정보"
}
fn execution_disclosure(ui: &mut egui::Ui, run_id: &str, execution: &Value) {
    ui.push_id((run_id, text(execution, "execution_id")), |ui| {
        ui.collapsing(
            format!(
                "{} · {}",
                execution_title(execution),
                monitor_cleanup(text(execution, "state"))
            ),
            |ui| {
                for (key, label) in [
                    ("execution_id", "실행 식별자"),
                    ("kind", "종류"),
                    ("state", "현재 상태"),
                    ("exit_code", "종료 코드"),
                    ("output", "실행 출력"),
                    ("error", "실패 이유"),
                    ("cleanup_error", "자원 정리 오류"),
                ] {
                    if !execution[key].is_null() {
                        ui.strong(label);
                        read_value(ui, &execution[key]);
                    }
                }
                ui.collapsing("전체 실행 메타데이터", |ui| {
                    read_value(ui, execution)
                });
            },
        );
    });
}
#[cfg(test)]
mod management_retirement_tests {
    use super::native_management_tests::{record, snapshot};
    use super::*;
    #[test]
    fn retirement_discards_observations_but_preserves_same_workspace_draft_and_cas_selection() {
        let conversation = record("conversations", json!({"title":"Retained draft"}));
        let s = snapshot(&[conversation.clone()]);
        let mut panes = Panes::default();
        panes.set_context(Section::Chat, Some(&conversation.header.id));
        panes.draft_message = "Retain unsaved message".into();
        panes.management.batches.insert(
            Section::Chat,
            BTreeSet::from([conversation.header.id.clone()]),
        );
        panes.open_management(
            &s,
            ManagementAction::Trash,
            vec![conversation.header.id.clone()],
        );
        let token = match panes.drain_effects().remove(0) {
            Effect::DeletionPreview { token, .. } => token,
            _ => panic!(),
        };
        let baseline = panes.management.dialog.as_ref().unwrap().baseline.clone();
        panes.management.monitor = Some(json!({"runs":[{"run_id":"old observation"}]}));
        panes.management.trash = Some(json!({"items":[{"id":"old observation"}]}));
        panes
            .management
            .feedback
            .insert("global".into(), "Old scope feedback".into());
        panes.retire_async_operations();
        assert!(panes.management.monitor.is_none());
        assert!(panes.management.trash.is_none());
        assert!(panes.management.feedback.is_empty());
        assert!(!panes.set_native_result(&token, &json!({"expected":baseline})));
        assert_eq!(panes.draft_message, "Retain unsaved message");
        assert_eq!(panes.management.dialog.as_ref().unwrap().baseline, baseline);
        assert!(panes.management.dialog.as_ref().unwrap().reload_required);
        assert_eq!(
            panes.management.batches[&Section::Chat],
            BTreeSet::from([conversation.header.id.clone()])
        );
        let replacement = Panes::default();
        assert!(replacement.management.batches.is_empty());
        assert!(replacement.management.monitor.is_none());
    }
    #[test]
    fn execution_disclosure_uses_real_nonempty_names_and_keeps_unknown_titles_honest() {
        assert_eq!(
            execution_title(
                &json!({"tool_name":"","executable":"cmd.exe","kind":"process","execution_id":"exec_own"})
            ),
            "cmd.exe"
        );
        assert_eq!(
            execution_title(&json!({"tool_name":"Bash","executable":"cmd.exe"})),
            "Bash"
        );
        assert_eq!(
            execution_title(&json!({"kind":"terminal","execution_id":"terminal_own"})),
            "terminal"
        );
        assert_eq!(
            execution_title(&json!({"execution_id":"exec_own"})),
            "exec_own"
        );
        assert_eq!(execution_title(&json!({})), "실행 정보");
    }
    #[test]
    fn identical_executables_have_independent_actual_disclosure_state() {
        let ctx = egui::Context::default();
        crate::app::configure(&ctx);
        ctx.style_mut(|style| style.animation_time = 0.0);
        let executions = [
            json!({"execution_id":"exec_first","executable":"cmd.exe","state":"running","cleanup_error":"First process cleanup detail"}),
            json!({"execution_id":"exec_second","executable":"cmd.exe","state":"running","cleanup_error":"Second process cleanup detail"}),
        ];
        let render = |events| {
            ctx.run(
                egui::RawInput {
                    screen_rect: Some(egui::Rect::from_min_size(
                        egui::Pos2::ZERO,
                        egui::vec2(900.0, 620.0),
                    )),
                    events,
                    ..Default::default()
                },
                |ctx| {
                    egui::CentralPanel::default().show(ctx, |ui| {
                        for execution in &executions {
                            execution_disclosure(ui, "run_actual", execution);
                        }
                    });
                },
            )
        };
        let initial = render(vec![]);
        let headers: Vec<_> = initial
            .shapes
            .iter()
            .filter_map(|shape| match &shape.shape {
                egui::epaint::Shape::Text(text) if text.galley.text() == "cmd.exe · 실행 중" => {
                    Some(text.visual_bounding_rect().center())
                }
                _ => None,
            })
            .collect();
        assert_eq!(headers.len(), 2);
        let pos = headers[0];
        for pressed in [true, false] {
            let _ = render(vec![
                egui::Event::PointerMoved(pos),
                egui::Event::PointerButton {
                    pos,
                    button: egui::PointerButton::Primary,
                    pressed,
                    modifiers: egui::Modifiers::NONE,
                },
            ]);
        }
        let expanded = render(vec![]);
        let visible = |value: &str| {
            expanded.shapes.iter().any(|shape| {
                matches!(
                    &shape.shape, egui::epaint::Shape::Text(text) if text.galley.text() == value
                )
            })
        };
        assert!(visible("exec_first"));
        assert!(visible("First process cleanup detail"));
        assert!(!visible("exec_second"));
        assert!(!visible("Second process cleanup detail"));
    }
}

#[cfg(test)]
mod history_cache_tests {
    use super::native_management_tests::{record, snapshot};
    use super::*;

    fn pin(panes: &mut Panes, id: &str) -> String {
        let mut params = json!({"id":id,"limit":100});
        panes.pin_history_request("ui.conversationHistory", &mut params);
        let token = params["_pane_token"].as_str().unwrap().to_owned();
        panes.pin_history_request("ui.conversationHistory", &mut params);
        assert_eq!(
            params["_pane_token"], token,
            "Submitting an already pinned read must preserve its ownership token"
        );
        token
    }
    fn history(value: &str) -> Value {
        json!({"items":[{"seq":artex_core::new_id("convactivity"),"kind":"text","summary":value,"detail":{"text":value,"blob":null},"ts":"2026-10-14T00:00:00Z"}]})
    }
    fn complete(
        panes: &mut Panes,
        s: &Snapshot,
        action: ManagementAction,
        rows: &[&Record],
        results: Value,
    ) {
        panes.open_management(
            s,
            action,
            rows.iter().map(|r| r.header.id.clone()).collect(),
        );
        let token = match panes.drain_effects().remove(0) {
            Effect::DeletionPreview { token, .. } => token,
            _ => panic!("Expected impact preview before a destructive action"),
        };
        panes.set_native_result(&token, &json!({"expected":rows.iter().map(|r|expected(r)).collect::<Vec<_>>(),"can_apply":true}));
        panes.management.dialog.as_mut().unwrap().confirmed = true;
        panes.apply_management();
        let token = match panes.drain_effects().remove(0) {
            Effect::ConversationManage { token, .. } => token,
            _ => panic!("Expected the typed conversation action"),
        };
        assert!(
            panes.set_native_result(&token, &json!({"cleanup_confirmed":true,"results":results}))
        );
    }
    fn rendered(
        panes: &mut Panes,
        ctx: &egui::Context,
        s: &Snapshot,
        conversation: &Record,
    ) -> Vec<String> {
        let mut output = None;
        for _ in 0..2 {
            let mut effects = vec![];
            output = Some(ctx.run(
                egui::RawInput {
                    screen_rect: Some(egui::Rect::from_min_size(
                        egui::Pos2::ZERO,
                        egui::vec2(900.0, 620.0),
                    )),
                    ..Default::default()
                },
                |ctx| {
                    egui::CentralPanel::default().show(ctx, |ui| {
                        panes.chat(ui, s, Some(conversation), None, &mut effects)
                    });
                },
            ));
            assert!(
                effects.is_empty(),
                "Rendering history cannot resubmit a deletion or send a message"
            );
        }
        output
            .unwrap()
            .shapes
            .into_iter()
            .filter_map(|shape| match shape.shape {
                egui::epaint::Shape::Text(text) => Some(text.galley.text().to_owned()),
                _ => None,
            })
            .collect()
    }
    #[test]
    fn clear_history_removes_actual_cached_and_old_snapshot_messages_rejects_late_read_and_preserves_other_chat()
     {
        let a = record("conversations", json!({"title":"Conversation A"}));
        let b = record("conversations", json!({"title":"Conversation B"}));
        let old = record(
            "conversation_activities",
            json!({"conversation_id":a.header.id,"kind":"text","summary":"Old snapshot A","detail":{"text":"Old snapshot A","blob":null}}),
        );
        let s = snapshot(&[a.clone(), b.clone(), old]);
        let original = s.clone();
        let ctx = egui::Context::default();
        crate::app::configure(&ctx);
        let mut panes = Panes::default();
        panes.set_context(Section::Chat, Some(&a.header.id));
        panes.draft_message = "Retain A draft".into();
        let cached = pin(&mut panes, &a.header.id);
        assert!(panes.set_projection_for(
            "ui.conversationHistory",
            &cached,
            history("Cached A history"),
            vec![]
        ));
        assert!(
            rendered(&mut panes, &ctx, &s, &a)
                .iter()
                .any(|t| t == "Cached A history")
        );
        let late_a = pin(&mut panes, &a.header.id);
        panes.set_context(Section::Chat, Some(&b.header.id));
        panes.draft_message = "Retain B draft".into();
        let cached = pin(&mut panes, &b.header.id);
        assert!(panes.set_projection_for(
            "ui.conversationHistory",
            &cached,
            history("Cached B history"),
            vec![]
        ));
        let late_b = pin(&mut panes, &b.header.id);
        complete(
            &mut panes,
            &s,
            ManagementAction::ClearHistory,
            &[&a],
            json!([{"id":a.header.id,"ok":true,"cleared":true}]),
        );
        assert!(!panes.set_projection_for(
            "ui.conversationHistory",
            &late_a,
            history("Late stale A"),
            vec![]
        ));
        assert!(panes.set_projection_for(
            "ui.conversationHistory",
            &late_b,
            history("Fresh B history"),
            vec![]
        ));
        assert_eq!(panes.draft_message, "Retain B draft");
        assert!(
            rendered(&mut panes, &ctx, &s, &b)
                .iter()
                .any(|t| t == "Fresh B history")
        );
        panes.set_context(Section::Chat, Some(&a.header.id));
        assert_eq!(panes.draft_message, "Retain A draft");
        panes.set_projection("ui.conversationHistory", history("Unfenced stale A"));
        let texts = rendered(&mut panes, &ctx, &s, &a);
        for stale in [
            "Cached A history",
            "Old snapshot A",
            "Late stale A",
            "Unfenced stale A",
            "Fresh B history",
        ] {
            assert!(
                !texts.iter().any(|t| t == stale),
                "Deleted or foreign history rendered: {stale}"
            );
        }
        let fresh = pin(&mut panes, &a.header.id);
        assert!(panes.set_projection_for(
            "ui.conversationHistory",
            &fresh,
            history("New A history"),
            vec![]
        ));
        assert!(
            rendered(&mut panes, &ctx, &s, &a)
                .iter()
                .any(|t| t == "New A history")
        );
        assert_eq!(s, original);
    }
    #[test]
    fn trash_and_purge_invalidate_only_successful_targets_while_run_stop_preserves_history() {
        for action in [ManagementAction::Trash, ManagementAction::Purge] {
            let a = record("conversations", json!({"title":"A"}));
            let b = record("conversations", json!({"title":"B"}));
            let s = snapshot(&[a.clone(), b.clone()]);
            let mut panes = Panes::default();
            let read_a = pin(&mut panes, &a.header.id);
            panes.set_projection_for(
                "ui.conversationHistory",
                &read_a,
                history("Preserved before stop"),
                vec![],
            );
            let before_stop = panes.flow.history[&a.header.id].clone();
            let stop = panes.management_token();
            panes.management.requests.insert(
                stop.clone(),
                ManagementRequest::Mutation {
                    nonce: None,
                    ids: vec![a.header.id.clone()],
                },
            );
            panes.set_native_result(
                &stop,
                &json!({"cleanup_confirmed":true,"results":[{"id":a.header.id,"ok":true}]}),
            );
            assert_eq!(panes.flow.history[&a.header.id], before_stop);
            let late_a = pin(&mut panes, &a.header.id);
            let late_b = pin(&mut panes, &b.header.id);
            complete(
                &mut panes,
                &s,
                action,
                &[&a, &b],
                json!([{"id":a.header.id,"ok":true},{"id":b.header.id,"ok":false,"error":"cleanup unconfirmed"}]),
            );
            assert!(!panes.set_projection_for(
                "ui.conversationHistory",
                &late_a,
                history("Stale A"),
                vec![]
            ));
            assert!(panes.set_projection_for(
                "ui.conversationHistory",
                &late_b,
                history("Retained B"),
                vec![]
            ));
            assert_eq!(panes.flow.history[&a.header.id]["items"], json!([]));
            assert_eq!(
                panes.flow.history[&b.header.id]["items"][0]["summary"],
                "Retained B"
            );
        }
    }
}

#[cfg(test)]
mod management_geometry_tests {
    use super::native_management_tests::{record, snapshot};
    use super::*;

    #[test]
    fn actual_task_delete_window_scrolls_long_impact_but_keeps_confirmation_and_footer_inside_900x620()
     {
        let tasks=(0..16).map(|i|record("tasks",json!({"name":format!("Task {i} · A long actual task name requiring more space for deletion impact review"),"status":"paused"}))).collect::<Vec<_>>();
        let s = snapshot(&tasks);
        let original = s.clone();
        let mut panes = Panes::default();
        panes.set_read_only(true);
        panes.open_management(
            &s,
            ManagementAction::TaskDelete,
            tasks.iter().map(|task| task.header.id.clone()).collect(),
        );
        let token = match panes.drain_effects().remove(0) {
            Effect::DeletionPreview { token, .. } => token,
            _ => panic!(),
        };
        panes.set_native_result(&token,&json!({"expected":tasks.iter().map(expected).collect::<Vec<_>>(),"can_apply":true,"cleanup_confirmed":true,"affected":{"activities":100,"runs":16,"questions":8,"assets":20,"traffic":15,"findings":4,"llm_records":12},"owned_attachments":[{"owner_id":tasks[0].header.id,"relative_path":"task-directories/owned-candidate"}],"shared_preserved":{"assets":10,"traffic":20,"tool_audit":100},"requires_stop":[]}));
        panes.management.dialog.as_mut().unwrap().confirmed = true;
        let ctx = egui::Context::default();
        crate::app::configure(&ctx);
        let render = |panes: &mut Panes, events| {
            ctx.run(
                egui::RawInput {
                    screen_rect: Some(egui::Rect::from_min_size(
                        egui::Pos2::ZERO,
                        egui::vec2(900.0, 620.0),
                    )),
                    events,
                    ..Default::default()
                },
                |ctx| {
                    assert!(panes.show_operation_dialog(ctx, &s).is_empty());
                },
            )
        };
        let mut out = render(&mut panes, vec![]);
        for _ in 0..3 {
            out = render(&mut panes, vec![]);
        }
        let check_footer = |out: &egui::FullOutput| {
            for label in [
                "표시한 범위와 자료 영향을 확인했습니다.",
                "닫기",
                "다시 확인",
                "작업 삭제",
            ] {
                let matches = out
                    .shapes
                    .iter()
                    .filter_map(|shape| match &shape.shape {
                        egui::epaint::Shape::Text(text) if text.galley.text() == label => {
                            Some((text.visual_bounding_rect(), shape.clip_rect))
                        }
                        _ => None,
                    })
                    .collect::<Vec<_>>();
                assert!(
                    !matches.is_empty(),
                    "Missing actual deletion footer: {label}"
                );
                assert!(
                    matches
                        .iter()
                        .any(|(bounds, clip)| bounds.top() >= clip.top() - 1.0
                            && bounds.bottom() <= clip.bottom() + 1.0
                            && bounds.left() >= clip.left() - 1.0
                            && bounds.right() <= clip.right() + 1.0
                            && bounds.top() >= 0.0
                            && bounds.bottom() <= 620.0),
                    "Clipped deletion footer {label}: {matches:?}"
                );
            }
        };
        check_footer(&out);
        let body_pos = out
            .shapes
            .iter()
            .find_map(|shape| match &shape.shape {
                egui::epaint::Shape::Text(text) if text.galley.text() == "16개 항목" => {
                    Some(text.visual_bounding_rect().center())
                }
                _ => None,
            })
            .expect("The actual long impact body must render before scrolling");
        let scrolled = render(
            &mut panes,
            vec![
                egui::Event::PointerMoved(body_pos),
                egui::Event::MouseWheel {
                    unit: egui::MouseWheelUnit::Point,
                    delta: egui::vec2(0.0, -900.0),
                    modifiers: egui::Modifiers::NONE,
                },
            ],
        );
        check_footer(&scrolled);
        let _ = render(&mut panes, vec![]);
        assert!(panes.drain_effects().is_empty());
        assert!(
            panes
                .management
                .dialog
                .as_ref()
                .unwrap()
                .action_token
                .is_none()
        );
        assert_eq!(s, original);
    }
}
