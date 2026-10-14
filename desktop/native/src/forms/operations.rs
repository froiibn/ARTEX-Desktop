//! Source-backed entity actions. Each draft owns a read generation; refreshes
//! never replace its values or read-time versions. This is a structured action
//! menu, not a claim to reproduce the original bespoke web layouts.
use super::{FormSubmission, schema, widgets};
use crate::model::{Section, display_name};
use artex_core::{Error, Expected, Result, contracts};
use artex_native_core::{NativeReadView, WorkspaceScope};
use eframe::egui::{self, Ui};
use serde_json::{Value, json};
use std::{
    collections::{BTreeMap, BTreeSet},
    sync::OnceLock,
};

#[derive(Clone)]
pub struct OperationSubmission {
    pub method: String,
    pub params: Value,
    pub expected: Vec<Expected>,
    pub pinned: NativeReadView,
    pub scope: WorkspaceScope,
    pub mutation: bool,
    pub token: String,
}
#[derive(Clone)]
pub struct OperationExport {
    pub scope: WorkspaceScope,
    pub body: Value,
    pub name: String,
    pub base64: bool,
}
#[derive(Clone)]
pub struct SourceAction {
    pub key: String,
    pub method: String,
    pub label: String,
    pub section: Section,
    pub mutation: bool,
    pub destructive: bool,
    route: Option<Value>,
}

fn ui_dtos() -> &'static Value {
    static DTOS: OnceLock<Value> = OnceLock::new();
    DTOS.get_or_init(|| contracts::read("ui-dtos.v1.json"))
}
fn registered(kind: &str, method: &str) -> bool {
    // Read the actual exported facade arrays, rather than infer semantics from
    // an action's spelling. This source file is frozen into the Rust binary.
    let source = include_str!("../../../crates/rpc/src/facade.rs");
    let marker = format!("pub const {kind}:");
    source
        .split(marker.as_str())
        .nth(1)
        .map(|v| v.split("];").next().unwrap_or(v))
        .is_some_and(|array| array.contains(&format!("\"{method}\"")))
}
fn classification(method: &str) -> Option<bool> {
    let query = registered("QUERIES", method);
    let command = registered("COMMANDS", method);
    if query != command {
        return Some(command);
    }
    let operation = ui_dtos()["operations"]
        .as_array()?
        .iter()
        .find(|v| v["method"] == method)?;
    let adapter = operation["legacy_adapter"].as_str()?;
    for verb in ["get", "post", "put", "patch", "del"] {
        if adapter.contains(&format!("{verb}(")) || adapter.contains(&format!("{verb}<")) {
            return Some(verb != "get");
        }
    }
    // Explicit request bodies are not guessed from method names.
    if adapter.contains("method: \"DELETE\"") || adapter.contains("method: \"POST\"") {
        return Some(true);
    }
    None
}

/// The menus are intentionally curated by entity. Only actual registered
/// source operations are retained; login and the web updater are excluded.
pub fn catalog() -> &'static [SourceAction] {
    static CATALOG: OnceLock<Vec<SourceAction>> = OnceLock::new();
    CATALOG.get_or_init(|| {
        let mut out = Vec::new();
        macro_rules! actions {
            ($section:ident, $( $method:literal => $label:literal ),+ $(,)?) => {$({
                if let Some(mutation) = classification($method) {
                    if !schema::method($method).is_null() {
                        out.push(SourceAction {key:$method.into(),method:$method.into(),label:$label.into(),section:Section::$section,mutation,destructive:destructive($method),route:None});
                    }
                }
            })+};
        }
        actions!(Assets,
            "ui.assets"=>"전체 자산 목록", "ui.searchAssets"=>"자산 조건 검색",
            "ui.assetCounts"=>"자산 종류별 통계", "ui.taskAssets"=>"작업의 자산 목록",
            "ui.searchTaskAssets"=>"작업 자산 조건 검색", "ui.taskAssetRefs"=>"자산 참조 관계",
            "ui.taskIntentAssets"=>"의도별 자산 목록", "ui.deleteAssets"=>"선택 자산 일괄 삭제",
            "ui.attachTaskAssets"=>"작업에 자산 연결", "ui.registerTaskAssetScopes"=>"작업 자산 범위 등록",
            "ui.detachTaskAsset"=>"작업에서 자산 연결 해제", "ui.companies"=>"회사 목록과 귀속 범위",
            "ui.createCompany"=>"회사 추가", "ui.addCompanyScope"=>"회사 범위 추가",
            "ui.updateCompanyScope"=>"회사 범위 교체·삭제·순서 변경", "ui.deleteCompany"=>"회사 삭제");
        actions!(Findings,
            "ui.findingsPage"=>"발견 사항 검색·페이지", "ui.findingGroups"=>"발견 사항 그룹",
            "ui.findingAssetTree"=>"자산별 발견 사항 트리", "ui.findingStats"=>"발견 사항 통계",
            "ui.getFinding"=>"발견 사항 상세·보고서", "ui.updateFinding"=>"발견 사항 메타데이터 수정",
            "ui.setFindingStatus"=>"발견 사항 상태 변경", "ui.setFindingSeverity"=>"발견 사항 심각도 변경",
            "ui.deleteFinding"=>"발견 사항 삭제", "ui.findingLineage"=>"발견 사항 계보",
            "ui.findingTraffic"=>"연결된 트래픽 증거", "ui.bindFindingTraffic"=>"트래픽 증거 연결",
            "ui.editFindingTraffic"=>"트래픽 증거 수정", "ui.removeFindingTraffic"=>"트래픽 증거 연결 해제",
            "ui.orderFindingTraffic"=>"트래픽 증거 순서 변경", "ui.findingTrafficDetail"=>"증거 상세",
            "ui.findingTrafficBody"=>"증거 본문 페이지", "ui.downloadFindingTrafficBody"=>"증거 본문 내보내기",
            "ui.findingRetests"=>"재검증 이력", "ui.activeFindingRetests"=>"진행 중인 재검증",
            "ui.startFindingRetest"=>"재검증 시작", "ui.deepenFinding"=>"발견 사항 추가 탐색",
            "ui.exportFindings"=>"발견 사항 파일 내보내기");
        actions!(Tasks,
            "ui.task"=>"작업 상세", "ui.updateTask"=>"작업 내용 수정", "ui.renameTask"=>"작업 이름 변경",
            "ui.pinTask"=>"작업 고정 상태", "ui.deleteTask"=>"작업과 전용 파일 삭제",
            "ui.controlTasksBatch"=>"여러 작업 일괄 제어", "ui.updateTasksCategory"=>"여러 작업 분류 변경",
            "ui.updateTaskCategory"=>"작업 분류 변경", "ui.taskCategories"=>"분류 목록",
            "ui.taskTemplates"=>"작업 템플릿 목록", "ui.taskGoals"=>"작업 목표 목록",
            "ui.addGoal"=>"목표 추가", "ui.updateGoal"=>"목표 내용 수정", "ui.deleteGoal"=>"목표 삭제",
            "ui.intentsPage"=>"의도 목록·페이지", "ui.controlIntent"=>"의도 일시 중지·재개·취소",
            "ui.rerunIntent"=>"의도 다시 실행", "ui.rerunBlocked"=>"차단된 의도 다시 실행",
            "ui.sendWorkerMessage"=>"작업자에게 지시 전달", "ui.taskCoverage"=>"작업 커버리지",
            "ui.taskCoverageGraph"=>"커버리지 그래프", "ui.taskLLMResolution"=>"작업 모델 결정 내역",
            "ui.updateTaskLLMProfiles"=>"작업 모델 후보 변경", "ui.newMainSession"=>"새 메인 대화 구간",
            "ui.mainSessions"=>"메인 대화 구간 목록", "ui.report"=>"작업 보고서");
        actions!(Categories,
            "ui.taskCategories"=>"분류 목록", "ui.createTaskCategory"=>"분류 추가",
            "ui.renameTaskCategory"=>"분류 이름 변경", "ui.deleteTaskCategory"=>"분류 삭제",
            "ui.updateTasksCategory"=>"작업 일괄 분류");
        actions!(Templates,
            "ui.taskTemplates"=>"템플릿 목록", "ui.createTaskTemplate"=>"템플릿 추가",
            "ui.updateTaskTemplate"=>"템플릿 수정", "ui.deleteTaskTemplate"=>"템플릿 삭제",
            "ui.taskInterceptRules"=>"작업 전용 자산 차단 규칙",
            "ui.createTaskInterceptRule"=>"작업 전용 차단 규칙 추가",
            "ui.updateTaskInterceptRule"=>"작업 전용 차단 규칙 수정",
            "ui.deleteTaskInterceptRule"=>"작업 전용 차단 규칙 삭제",
            "ui.toggleTaskInterceptRule"=>"작업 전용 차단 규칙 사용 여부",
            "ui.explorationNodes"=>"작업 노드 조건·페이지 조회",
            "ui.explorationGraph"=>"작업과 공유 출처의 탐색 그래프 자료");
        actions!(Chat,
            "ui.updateConversation"=>"대화 이름·고정 상태 수정",
            "ui.renameConversation"=>"대화 이름 변경",
            "ui.pinConversation"=>"대화 고정 상태 변경",
            "ui.updateConversationProfile"=>"대화의 LLM 모델 변경",
            "ui.deleteConversation"=>"선택한 대화 삭제",
            "ui.deleteConversations"=>"여러 대화 삭제",
            "ui.conversationHistory"=>"대화 전체 이력·이전 페이지 조회",
            "ui.conversationMessages"=>"대화 메시지 커서 이후 조회",
            "ui.conversationMsgDetail"=>"대화의 실행·도구 메시지 상세",
            "ui.chatMentions"=>"대화에 참조할 객체 후보 조회");
        actions!(Files, "ui.workspaceDelete"=>"작업 파일·폴더 삭제");
        actions!(Tools, "ui.testCustomTool"=>"현재 사용자 도구 정의 실행 시험");
        actions!(Settings,
            "ui.detectPython"=>"Python 인터프리터 자동 탐지",
            "ui.testWebSearch"=>"현재 검색 설정으로 검색 시험");
        actions!(Agents,
            "ui.getAgent"=>"에이전트 설정·프롬프트·변수 상세",
            "ui.agentVariables"=>"프롬프트에서 사용할 변수 안내",
            "ui.previewAgentPrompt"=>"현재 프롬프트 템플릿 미리보기");
        actions!(Logs, "ui.commands"=>"작업별 도구 명령 기록 검색");
        actions!(Approvals,
            "ui.interceptGetOne"=>"대기 승인 상세", "ui.interceptExecution"=>"실행 내역",
            "ui.interceptDetail"=>"승인 상세·입력·출력", "ui.interceptHistoryPage"=>"승인 이력 페이지",
            "ui.interceptTaskPage"=>"작업별 승인 이력", "ui.interceptGetToolConfig"=>"도구 승인 정책 조회",
            "ui.interceptSetToolConfig"=>"도구 승인 정책 변경", "ui.interceptGetJudgeConfig"=>"심사 모델 설정 조회",
            "ui.interceptSetJudgeConfig"=>"심사 모델 설정 변경", "ui.interceptJudgeUsage"=>"심사 모델 사용량",
            "ui.interceptRules"=>"도구 차단 규칙 목록", "ui.createInterceptRule"=>"도구 차단 규칙 추가",
            "ui.updateInterceptRule"=>"도구 차단 규칙 수정", "ui.deleteInterceptRule"=>"도구 차단 규칙 삭제",
            "ui.toggleInterceptRule"=>"도구 차단 규칙 사용 여부", "ui.assetInterceptRules"=>"자산 차단 규칙 목록",
            "ui.createAssetInterceptRule"=>"자산 차단 규칙 추가", "ui.updateAssetInterceptRule"=>"자산 차단 규칙 수정",
            "ui.deleteAssetInterceptRule"=>"자산 차단 규칙 삭제", "ui.toggleAssetInterceptRule"=>"자산 차단 규칙 사용 여부");
        actions!(Notifications,
            "ui.notifyMeta"=>"지원 알림 채널", "ui.notifyDeliveries"=>"알림 전달·집계 이력",
            "ui.notifyRetryDelivery"=>"실패한 알림 다시 전달", "ui.notifyTestChannel"=>"알림 채널 시험 전송");
        actions!(Skills,
            "ui.skillFiles"=>"스킬 파일 목록 새로 조회", "ui.skillUsage"=>"스킬 사용 이력",
            "ui.missingSkills"=>"누락된 스킬 참조", "ui.createSkillDir"=>"스킬 하위 폴더 추가",
            "ui.deleteSkillPath"=>"스킬 파일·폴더 삭제", "ui.skillVisibility"=>"스킬 접근 권한 조회",
            "ui.toggleSkillVisibility"=>"스킬 접근 권한 변경");
        actions!(Mcp,
            "ui.mcpTools"=>"서버의 도구 목록", "ui.refreshMcpServer"=>"서버 도구 목록 갱신",
            "ui.resourceVisibility"=>"서버 접근 권한 조회", "ui.toggleVisibility"=>"서버 접근 권한 변경",
            "ui.getAgentVisibility"=>"에이전트별 서버·스킬 권한", "ui.setAgentVisibility"=>"에이전트별 서버·스킬 권한 수정");
        actions!(Archives,
            "ui.taskArchives"=>"보관된 작업 검색", "ui.taskArchive"=>"보관 작업 상세",
            "ui.archiveTasks"=>"여러 작업 보관", "ui.restoreTaskArchives"=>"여러 보관 작업 복원",
            "ui.deleteTaskArchive"=>"보관 작업 삭제", "ui.deleteTaskArchives"=>"여러 보관 작업 삭제");
        actions!(Models,
            "ui.fetchLLMModels"=>"제공자 모델 목록 조회", "ui.testLLM"=>"모델 연결 시험",
            "ui.llmPool"=>"모델 풀 상태", "ui.resetLLMPool"=>"모델 풀 실패 상태 초기화",
            "ui.llmRetryPolicy"=>"모델 재시도 정책", "ui.saveLLMRetryPolicy"=>"전역 모델 재시도 정책 저장",
            "ui.llmTasks"=>"모델 기록이 있는 작업",
            "ui.tokensByModel"=>"모델별 토큰 통계", "ui.conversationTokens"=>"대화별 토큰 통계");
        actions!(Overview,
            "ui.stats"=>"작업 실행 통계", "ui.usageStats"=>"기간별 모델 사용량",
            "ui.tokenStats"=>"작업 토큰 통계", "ui.tokenDaily"=>"일별 토큰 사용량",
            "ui.commandStats"=>"도구 실행 통계", "ui.findingStats"=>"발견 사항 통계");
        let routes = contracts::read("legacy-routes.v1.json");
        for route in routes["routes"].as_array().into_iter().flatten() {
            let handler = route["handler"].as_str().unwrap_or("");
            if !["handleSideQuestions", "sideEvents", "cancelSideRequest"].contains(&handler) {continue;}
            let verb = route["legacy_http_method"].as_str().unwrap_or("");
            let parent = match route["parent_kind"].as_str() {Some("conversation")=>"대화",Some("worker")=>"작업자",_=>"메인 작업"};
            let label = match (handler,verb) {
                ("handleSideQuestions","GET")=>format!("{parent} 보조 질문 이력·문맥 체크포인트"),
                ("handleSideQuestions","POST")=>format!("{parent} 보조 질문 보내기"),
                ("handleSideQuestions","DELETE")=>format!("{parent} 보조 질문 이력 지우기"),
                ("sideEvents",_)=>"보조 질문 응답 진행 조회".into(),
                ("cancelSideRequest",_)=>"보조 질문 중지".into(),_=>continue,
            };
            let method = route["rpc_method"].as_str().unwrap_or("");
            if schema::method(method).is_null() {continue;}
            out.push(SourceAction {key:route["route_id"].as_str().unwrap().into(),method:method.into(),label,section:Section::Sessions,mutation:verb!="GET",destructive:verb=="DELETE",route:Some(route.clone())});
        }
        out
    })
}
fn destructive(method: &str) -> bool {
    method.starts_with("ui.delete")
        || method.contains("Remove")
        || method.starts_with("ui.remove")
        || matches!(
            method,
            "ui.controlTasksBatch"
                | "ui.workspaceDelete"
                | "ui.controlIntent"
                | "ui.detachTaskAsset"
                | "ui.updateCompanyScope"
                | "ui.resetLLMPool"
        )
}

struct Draft {
    token: String,
    owner: WorkspaceScope,
    pinned: NativeReadView,
    action: SourceAction,
    collection: String,
    params: Value,
    expected: Vec<Expected>,
    result: Option<Value>,
    error: Option<String>,
    busy: bool,
    reset_required: bool,
    confirm: bool,
    secrets: BTreeMap<String, String>,
    pending_secrets: BTreeSet<String>,
    pending_reads: BTreeSet<String>,
}
#[derive(Default)]
pub struct SourceOperations {
    choices: BTreeMap<Section, String>,
    drafts: BTreeMap<String, Draft>,
    exports: Vec<OperationExport>,
    workspace_id: Option<String>,
}
impl Draft {
    fn new(
        action: &SourceAction,
        view: &NativeReadView,
        section: Section,
        selected: Option<&str>,
    ) -> Self {
        let mut params = initial(action, view, section, selected);
        let selected_record = section
            .collection()
            .and_then(|c| selected.and_then(|id| view.generation.get(c, id)));
        let expected = selected_record
            .map(|r| vec![crate::model::expected(r)])
            .unwrap_or_default();
        if action.method == "ui.resourceVisibility" || action.method == "ui.toggleVisibility" {
            params["kind"] = json!("mcp");
        }
        Self {
            token: artex_core::new_id("request"),
            owner: view.workspace.scope.clone(),
            pinned: view.clone(),
            action: action.clone(),
            collection: section.collection().unwrap_or("operations").into(),
            params,
            expected,
            result: None,
            error: None,
            busy: false,
            reset_required: false,
            confirm: false,
            secrets: BTreeMap::new(),
            pending_secrets: BTreeSet::new(),
            pending_reads: BTreeSet::new(),
        }
    }
    fn submission(&self, method: String, params: Value, mutation: bool) -> OperationSubmission {
        OperationSubmission {
            method,
            params,
            mutation,
            token: self.token.clone(),
            scope: self.owner.clone(),
            expected: self.expected.clone(),
            pinned: self.pinned.clone(),
        }
    }
    fn submit(&mut self, read_only: bool) -> Result<OperationSubmission> {
        if self.busy
            || self.reset_required
            || !self.pending_reads.is_empty()
            || !self.pending_secrets.is_empty()
        {
            return Err(Error::new(
                "BUSY",
                "입력 준비나 명시적 다시 불러오기가 필요합니다.",
            ));
        }
        if self.action.mutation && read_only {
            return Err(Error::new(
                "RECOVERY_REQUIRED",
                "읽기 전용 작업 공간에서는 변경할 수 없습니다.",
            ));
        }
        if self.action.destructive && !self.confirm {
            return Err(Error::new(
                "PERMISSION_DENIED",
                "삭제·초기화 대상 확인이 필요합니다.",
            ));
        }
        // The worker owns large Body normalization. No generic JSON value is
        // guessed to be a Body and no UI thread publishes blobs.
        let shaped = schema::shape(&self.params, &schema::method(&self.action.method));
        let validation = validation_value(&schema::method(&self.action.method), &shaped, 0);
        artex_core::validate_contract(
            &schema::method(&self.action.method),
            &schema::rpc()["$defs"],
            &validation,
        )?;
        self.busy = true;
        self.error = None;
        Ok(self.submission(self.action.method.clone(), shaped, self.action.mutation))
    }
}

impl SourceOperations {
    pub fn show(
        &mut self,
        ui: &mut Ui,
        view: &NativeReadView,
        section: Section,
        selected: Option<&str>,
    ) -> Vec<OperationSubmission> {
        self.show_mode(ui, view, section, selected, false)
    }
    /// Drawer presentation uses the same drafts and request owners as inline
    /// presentation, without a second nested management disclosure.
    pub fn show_expanded(
        &mut self,
        ui: &mut Ui,
        view: &NativeReadView,
        section: Section,
        selected: Option<&str>,
    ) -> Vec<OperationSubmission> {
        self.show_mode(ui, view, section, selected, true)
    }
    fn show_mode(
        &mut self,
        ui: &mut Ui,
        view: &NativeReadView,
        section: Section,
        selected: Option<&str>,
        expanded: bool,
    ) -> Vec<OperationSubmission> {
        if self.workspace_id.as_deref() != Some(&view.workspace.scope.workspace_id) {
            self.drafts.clear();
            self.choices.clear();
            self.exports.clear();
            self.workspace_id = Some(view.workspace.scope.workspace_id.clone());
        }
        for draft in self.drafts.values_mut() {
            if draft.owner != view.workspace.scope {
                draft.owner = view.workspace.scope.clone();
                if draft.busy
                    || !draft.pending_reads.is_empty()
                    || !draft.pending_secrets.is_empty()
                {
                    draft.reset_required = true;
                    draft.error=Some("열린 작업 공간의 실행 세대가 바뀌었습니다. 이전 결과가 불명확하므로 명시적으로 다시 불러오세요.".into());
                }
                draft.busy = false;
                draft.pending_reads.clear();
                draft.pending_secrets.clear();
            }
        }
        let actions = catalog()
            .iter()
            .filter(|a| a.section == section)
            .collect::<Vec<_>>();
        if actions.is_empty() {
            return Vec::new();
        }
        let mut submissions = Vec::new();
        let mut render = |ui: &mut Ui| {
            ui.set_width(ui.available_width());
            let choice = self
                .choices
                .entry(section)
                .or_insert_with(|| actions[0].key.clone());
            egui::ComboBox::from_id_salt((section, "management-menu"))
                .selected_text(
                    actions
                        .iter()
                        .find(|a| &a.key == choice)
                        .map(|a| a.label.as_str())
                        .unwrap_or("작업 선택"),
                )
                .show_ui(ui, |ui| {
                    for action in &actions {
                        ui.selectable_value(choice, action.key.clone(), &action.label);
                    }
                });
            let Some(action) = actions.iter().find(|a| &a.key == choice).copied() else {
                return;
            };
            let key = format!("{}:{}", action.key, selected.unwrap_or(""));
            if !self.drafts.contains_key(&key) {
                self.drafts
                    .insert(key.clone(), Draft::new(action, view, section, selected));
            }
            let draft = self.drafts.get_mut(&key).unwrap();
            ui.label(if action.mutation {
                "확인한 저장 버전을 기준으로 변경합니다."
            } else {
                "조회 결과는 아래에 표시합니다."
            });
            ui.small(format!(
                "초안 기준 저장 세대: {}",
                draft.pinned.generation.commit_seq
            ));
            if view.workspace.read_only && action.mutation {
                ui.weak("읽기 전용: 이 변경 작업을 실행할 수 없습니다.");
            }
            let mut secret_submissions = Vec::<FormSubmission>::new();
            let mut reads = Vec::new();
            let references = schema::references(&draft.pinned.generation.snapshot);
            let editable = !draft.busy
                && !draft.reset_required
                && draft.pending_reads.is_empty()
                && draft.pending_secrets.is_empty()
                && !(view.workspace.read_only && action.mutation);
            let mut invalid = false;
            ui.add_enabled_ui(editable, |ui| {
                let mut state = widgets::WidgetState {
                    token: &draft.token,
                    collection: &draft.collection,
                    selected: None,
                    references: &references,
                    secrets: &mut draft.secrets,
                    pending_secrets: &mut draft.pending_secrets,
                    pending_reads: &mut draft.pending_reads,
                    submissions: &mut secret_submissions,
                    requests: &mut reads,
                    can_secret: !view.workspace.read_only,
                    invalid_secret: false,
                };
                input(
                    ui,
                    "",
                    &mut draft.params,
                    &input_schema(action),
                    &[],
                    &mut state,
                    &draft.pinned,
                    0,
                );
                invalid = state.invalid_secret;
            });
            for submission in secret_submissions {
                submissions.push(draft.submission(submission.operation, submission.params, true));
            }
            for (method, params) in reads {
                submissions.push(draft.submission(method, params, false));
            }
            if action.destructive {
                if matches!(
                    action.method.as_str(),
                    "ui.deleteTask" | "ui.deleteConversation" | "ui.deleteConversations"
                ) {
                    ui.add(egui::Label::new("목록의 ⋯ 또는 우클릭 메뉴에서 삭제 영향·전용 파일·공유 자료를 확인할 수 있습니다. 대화 내용 비우기, 휴지통 이동과 영구 삭제는 서로 다른 동작입니다. 아래 기존 삭제 경로도 서비스의 실행 정리·버전 검증을 통과해야 합니다.").wrap());
                }
                ui.checkbox(
                    &mut draft.confirm,
                    "선택한 대상의 삭제·연결 해제·초기화를 확인했습니다.",
                );
            }
            let ready = editable && !invalid && (!action.destructive || draft.confirm);
            ui.horizontal_wrapped(|ui| {
                if ui
                    .add_enabled(
                        ready,
                        egui::Button::new(if action.mutation {
                            "변경 실행"
                        } else {
                            "조회"
                        }),
                    )
                    .clicked()
                {
                    match draft.submit(view.workspace.read_only) {
                        Ok(s) => submissions.push(s),
                        Err(e) => draft.error = Some(e.message),
                    }
                }
                if ui
                    .add_enabled(
                        !draft.busy,
                        egui::Button::new("초안을 버리고 최신 데이터 다시 불러오기"),
                    )
                    .clicked()
                {
                    *draft = Draft::new(action, view, section, selected);
                }
            });
            if draft.busy {
                ui.spinner();
            }
            if let Some(error) = &draft.error {
                ui.colored_label(ui.visuals().warn_fg_color, error);
            }
            if let Some(result) = &draft.result {
                ui.separator();
                ui.strong("조회·실행 결과");
                let mut body_requests = Vec::new();
                display_result(ui, result, &["@result".into()], &mut body_requests, 0);
                for (path, body) in body_requests {
                    let locator = serde_json::to_string(&path).unwrap();
                    if draft.pending_reads.insert(locator) {
                        submissions.push(draft.submission(
                            "native.body.read".into(),
                            json!({"body":body,"_form_token":draft.token,"_form_path":path}),
                            false,
                        ));
                    }
                }
                if let Some((body, name, base64)) = export_body(result) {
                    if ui.button("결과를 파일로 저장").clicked() {
                        self.exports.push(OperationExport {
                            scope: draft.owner.clone(),
                            body,
                            name,
                            base64,
                        });
                    }
                }
            }
        };
        if expanded {
            render(ui);
        } else {
            egui::CollapsingHeader::new("추가 관리")
                .id_salt((section, "source-actions"))
                .show(ui, |ui| render(ui));
        }
        drop(render);
        submissions
    }
    pub fn result(
        &mut self,
        scope: &WorkspaceScope,
        token: &str,
        method: &str,
        value: Value,
    ) -> bool {
        let Some(draft) = self
            .drafts
            .values_mut()
            .find(|d| d.token == token && &d.owner == scope && d.action.method == method && d.busy)
        else {
            return false;
        };
        draft.busy = false;
        draft.result = Some(value);
        draft.error = None;
        if draft.action.mutation {
            draft.reset_required = true;
            draft.error=Some("변경을 완료했습니다. 초안은 보존합니다. 다음 변경 전 최신 데이터를 명시적으로 다시 불러오세요.".into());
        }
        true
    }
    pub fn error(
        &mut self,
        scope: &WorkspaceScope,
        token: &str,
        method: &str,
        code: &str,
        message: &str,
    ) -> bool {
        let Some(draft) = self
            .drafts
            .values_mut()
            .find(|d| d.token == token && &d.owner == scope && d.action.method == method && d.busy)
        else {
            return false;
        };
        draft.busy = false;
        draft.error = Some(format!("{code}: {message}"));
        draft.reset_required |= matches!(
            code,
            "VERSION_CONFLICT" | "RECOVERY_REQUIRED" | "STALE_EPOCH"
        );
        true
    }
    pub fn body_error(
        &mut self,
        scope: &WorkspaceScope,
        token: &str,
        path: &[String],
        code: &str,
        message: &str,
    ) -> bool {
        let Some(draft) = self
            .drafts
            .values_mut()
            .find(|d| d.token == token && &d.owner == scope)
        else {
            return false;
        };
        if !draft
            .pending_reads
            .remove(&serde_json::to_string(path).unwrap())
        {
            return false;
        }
        draft.error = Some(format!("{code}: {message}"));
        draft.reset_required |= matches!(
            code,
            "VERSION_CONFLICT" | "RECOVERY_REQUIRED" | "STALE_EPOCH"
        );
        true
    }
    pub fn secret_error(
        &mut self,
        scope: &WorkspaceScope,
        target: &str,
        code: &str,
        message: &str,
    ) -> bool {
        let Ok(target) = serde_json::from_str::<Value>(target) else {
            return false;
        };
        let Some(token) = target["form_token"].as_str() else {
            return false;
        };
        let Some(path) = string_path(&target["path"]) else {
            return false;
        };
        let Some(draft) = self.drafts.values_mut().find(|d| {
            d.token == token && &d.owner == scope && target["collection"] == d.collection
        }) else {
            return false;
        };
        if !draft
            .pending_secrets
            .remove(&serde_json::to_string(&path).unwrap())
        {
            return false;
        }
        draft.error = Some(format!("{code}: {message}"));
        draft.reset_required |= matches!(
            code,
            "VERSION_CONFLICT" | "RECOVERY_REQUIRED" | "STALE_EPOCH"
        );
        true
    }
    pub fn secret_result(
        &mut self,
        scope: &WorkspaceScope,
        target: &str,
        reference: Value,
    ) -> bool {
        let Ok(target) = serde_json::from_str::<Value>(target) else {
            return false;
        };
        if artex_core::validate_contract(
            &schema::secret_schema(),
            &schema::rpc()["$defs"],
            &reference,
        )
        .is_err()
        {
            return false;
        }
        let Some(token) = target["form_token"].as_str() else {
            return false;
        };
        let Some(path) = string_path(&target["path"]) else {
            return false;
        };
        let Some(draft) = self.drafts.values_mut().find(|d| {
            d.token == token && &d.owner == scope && target["collection"] == d.collection
        }) else {
            return false;
        };
        if !draft
            .pending_secrets
            .remove(&serde_json::to_string(&path).unwrap())
        {
            return false;
        }
        super::set_path(&mut draft.params, &path, reference)
    }
    pub fn body_result(
        &mut self,
        scope: &WorkspaceScope,
        token: &str,
        path: &[String],
        text: String,
    ) -> bool {
        let Some(draft) = self
            .drafts
            .values_mut()
            .find(|d| d.token == token && &d.owner == scope)
        else {
            return false;
        };
        if !draft
            .pending_reads
            .remove(&serde_json::to_string(path).unwrap())
        {
            return false;
        }
        if path.first().is_some_and(|p| p == "@result") {
            draft.result.as_mut().is_some_and(|result| {
                super::set_path(result, &path[1..], json!({"text":text,"blob":null}))
            })
        } else {
            super::set_path(&mut draft.params, path, json!({"text":text,"blob":null}))
        }
    }
    pub fn owns_token(&self, token: &str) -> bool {
        self.drafts.values().any(|d| d.token == token)
    }
    pub fn drain_exports(&mut self) -> Vec<OperationExport> {
        std::mem::take(&mut self.exports)
    }
    pub fn retire_async_operations(&mut self) {
        self.exports.clear();
        for draft in self.drafts.values_mut() {
            if draft.busy || !draft.pending_reads.is_empty() || !draft.pending_secrets.is_empty() {
                draft.reset_required = true;
                draft.error=Some("작업 공간을 닫는 동안 요청이 중단됐습니다. 결과를 확인한 뒤 명시적으로 다시 불러오세요.".into());
            }
            draft.busy = false;
            draft.pending_reads.clear();
            draft.pending_secrets.clear();
        }
    }
}

fn string_path(value: &Value) -> Option<Vec<String>> {
    value
        .as_array()?
        .iter()
        .map(|p| p.as_str().map(str::to_owned))
        .collect()
}
fn input_schema(action: &SourceAction) -> Value {
    let mut shape = schema::method(&action.method);
    if matches!(
        action.method.as_str(),
        "ui.createCompany" | "ui.addCompanyScope" | "ui.updateCompanyScope"
    ) {
        let mut item = schema::resolve(&shape["properties"]["scope"]["items"]);
        item["properties"]["kind"] =
            json!({"type":"string","enum":["domain","ip","cidr","icp","keyword"]});
        shape["properties"]["scope"]["items"] = item;
    }
    if let Some(route) = &action.route {
        let path = if route["handler"] == "handleSideQuestions" {
            let mut fields = json!({"id":{"type":"string","pattern":if route["parent_kind"]=="conversation" {"^conversation_[0-9a-f]{32}$"}else{"^task_[0-9a-f]{32}$"}}});
            let mut required = vec!["id"];
            if route["parent_kind"] == "worker" {
                fields["iid"] = json!({"type":"string","pattern":"^node_[0-9a-f]{32}$"});
                required.push("iid");
            }
            json!({"type":"object","properties":fields,"required":required})
        } else {
            json!({"type":"object","properties":{"requestID":{"type":"string","pattern":"^sidequestion_[0-9a-f]{32}$"}},"required":["requestID"]})
        };
        shape["properties"]["path"] = path;
        shape["properties"]["query"] =
            json!({"type":"object","properties":{"before":{"type":"string"}}});
        shape["properties"]["body"] = if route["handler"] == "handleSideQuestions"
            && route["legacy_http_method"] == "POST"
        {
            json!({"type":"object","properties":{"question":{"type":"string"},"client_request_id":{"type":"string"}},"required":["question","client_request_id"]})
        } else {
            json!({"type":"object","properties":{}})
        };
    }
    shape
}
fn initial(
    action: &SourceAction,
    view: &NativeReadView,
    section: Section,
    selected: Option<&str>,
) -> Value {
    fn build(shape: &Value, selected: Option<&str>, depth: usize) -> Value {
        if depth > 12 {
            return Value::Null;
        }
        let shape = schema::actual(shape, &Value::Null);
        if let Some(pattern) = shape["pattern"].as_str() {
            if let Some(prefix) = pattern
                .strip_prefix('^')
                .and_then(|p| p.strip_suffix("_[0-9a-f]{32}$"))
            {
                if let Some(id) = selected.filter(|id| artex_core::valid_id(id, prefix)) {
                    return json!(id);
                }
            }
        }
        if shape["type"] == "object" && !schema::is_body(&shape) && !schema::is_secret(&shape) {
            let mut value = json!({});
            for key in shape["required"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(Value::as_str)
            {
                value[key] = build(&shape["properties"][key], selected, depth + 1);
            }
            value
        } else {
            schema::empty(&shape)
        }
    }
    let mut value = build(&input_schema(action), selected, 0);
    if let Some(route) = &action.route {
        value["route_id"] = route["route_id"].clone();
        if route["legacy_http_method"] == "POST" && route["handler"] == "handleSideQuestions" {
            value["body"]["client_request_id"] = json!(artex_core::new_id("request"));
        }
    }
    if section == Section::Skills {
        for key in ["name", "skill"] {
            if schema::method(&action.method)["properties"]
                .get(key)
                .is_some()
            {
                value[key] = json!(selected.unwrap_or(""));
            }
        }
    }
    if let Some(row) = section
        .collection()
        .and_then(|c| selected.and_then(|id| view.generation.get(c, id)))
    {
        if action.method == "ui.updateFinding" {
            value["fields"] = schema::shape(
                &row.data,
                &schema::method(&action.method)["properties"]["fields"],
            );
        }
        if matches!(
            action.method.as_str(),
            "ui.editFindingTraffic" | "ui.removeFindingTraffic" | "ui.orderFindingTraffic"
        ) {
            value["version"] = row.data["evidence_version"].clone();
        }
        if action.method == "ui.testLLM" {
            for (field, source) in [
                ("provider", "format"),
                ("model", "model"),
                ("base_url", "base_url"),
                ("api_key", "api_key"),
            ] {
                value[field] = row.data[source].clone();
            }
            value["profile_id"] = json!(row.header.id);
        }
        if action.method == "ui.getAgentVisibility" || action.method == "ui.setAgentVisibility" {
            value["key"] = json!(row.data["key"]);
        }
        if matches!(
            action.method.as_str(),
            "ui.getAgent" | "ui.agentVariables" | "ui.previewAgentPrompt"
        ) {
            value["key"] = row.data["key"].clone();
            if action.method == "ui.previewAgentPrompt" {
                if let Some(prompt) = row.data["current_prompt_id"]
                    .as_str()
                    .and_then(|id| view.generation.get("agent_prompts", id))
                {
                    if let Some(template) = prompt.data["template_text"]["text"].as_str() {
                        value["template"] = json!(template);
                    }
                }
            }
        }
    }
    if action.method == "ui.resourceVisibility" || action.method == "ui.toggleVisibility" {
        value["kind"] = json!("mcp");
    }
    value
}

fn validation_value(shape: &Value, value: &Value, depth: usize) -> Value {
    if depth > 32 {
        return value.clone();
    }
    let shape = schema::actual(shape, value);
    if schema::is_body(&shape) && value["text"].as_str().is_some_and(|s| s.len() > 65536) {
        return json!({"text":"","blob":null});
    }
    let mut value = value.clone();
    if let Some(properties) = shape["properties"].as_object() {
        for (key, child) in properties {
            if let Some(v) = value.get_mut(key) {
                *v = validation_value(child, v, depth + 1);
            }
        }
    }
    if let Some(items) = value.as_array_mut() {
        for item in items {
            *item = validation_value(&shape["items"], item, depth + 1);
        }
    }
    value
}

fn input(
    ui: &mut Ui,
    key: &str,
    value: &mut Value,
    shape: &Value,
    path: &[String],
    state: &mut widgets::WidgetState<'_>,
    view: &NativeReadView,
    depth: usize,
) {
    if depth > 12 {
        ui.weak("필드 구조가 깊어 기존 값을 유지합니다.");
        return;
    }
    let shape = schema::actual(shape, value);
    if key == "key" && shape["type"] == "string" {
        let current = value.as_str().unwrap_or("");
        let selected = view
            .generation
            .records("agents")
            .into_iter()
            .find(|a| a.data["key"] == current)
            .map(display_name)
            .unwrap_or_else(|| "에이전트 선택".into());
        crate::theme::field_row(ui, "에이전트", |ui| {
            egui::ComboBox::from_id_salt((state.token, path, "agent-key"))
                .width(ui.available_width())
                .selected_text(selected)
                .show_ui(ui, |ui| {
                    for row in view.generation.records("agents") {
                        ui.selectable_value(value, row.data["key"].clone(), display_name(row));
                    }
                });
        });
        return;
    }
    if let Some(prefix) = shape["pattern"]
        .as_str()
        .and_then(|p| p.strip_prefix('^'))
        .and_then(|p| p.strip_suffix("_[0-9a-f]{32}$"))
    {
        let current = value.as_str().unwrap_or("");
        let selected = view
            .generation
            .collections
            .values()
            .flat_map(|rows| rows.values())
            .find(|r| r.header.id == current)
            .map(display_name)
            .unwrap_or_else(|| "대상 선택".into());
        crate::theme::field_row(ui, schema::label(key), |ui| {
            egui::ComboBox::from_id_salt((state.token, path))
                .width(ui.available_width())
                .selected_text(selected)
                .show_ui(ui, |ui| {
                    for row in view
                        .generation
                        .collections
                        .values()
                        .flat_map(|rows| rows.values())
                        .filter(|r| {
                            artex_core::valid_id(&r.header.id, prefix)
                                && r.data["deleted_at"].is_null()
                        })
                    {
                        let label = format!(
                            "{} · {}",
                            display_name(row),
                            &row.header.id[..row.header.id.len().min(18)]
                        );
                        ui.selectable_value(value, json!(row.header.id), label);
                    }
                });
        });
        return;
    }
    if schema::is_body(&shape)
        || schema::is_secret(&shape)
        || matches!(key, "schema" | "headers" | "env")
    {
        widgets::field(ui, key, value, &shape, path, state, depth);
        return;
    }
    if shape["type"] == "object" {
        if !value.is_object() {
            *value = json!({});
        }
        if let Some(properties) = shape["properties"].as_object().filter(|p| !p.is_empty()) {
            ui.vertical(|ui| {
                if !key.is_empty() && !matches!(key, "p" | "patch" | "input") {
                    ui.add_space(6.0);
                    ui.strong(schema::label(key));
                    ui.add_space(4.0);
                }
                for (name, child) in properties {
                    if name == "route_id" {
                        continue;
                    }
                    let required = shape["required"]
                        .as_array()
                        .is_some_and(|r| r.iter().any(|v| v == name));
                    if value.get(name).is_none() {
                        if !required
                            && !ui
                                .small_button(format!("{} 설정 추가", schema::label(name)))
                                .clicked()
                        {
                            continue;
                        }
                        value[name] = schema::empty(child);
                    }
                    let mut p = path.to_vec();
                    p.push(name.clone());
                    input(
                        ui,
                        name,
                        &mut value[name],
                        child,
                        &p,
                        state,
                        view,
                        depth + 1,
                    );
                    if !required
                        && ui
                            .small_button(format!("{} 설정 제거", schema::label(name)))
                            .clicked()
                    {
                        value.as_object_mut().unwrap().remove(name);
                    }
                }
            });
        } else {
            let keys = value
                .as_object()
                .unwrap()
                .keys()
                .cloned()
                .collect::<Vec<_>>();
            for name in keys {
                let mut p = path.to_vec();
                p.push(name.clone());
                let inferred = infer_shape(&value[&name]);
                input(
                    ui,
                    &name,
                    &mut value[&name],
                    &inferred,
                    &p,
                    state,
                    view,
                    depth + 1,
                );
                if ui.small_button(format!("{name} 항목 제거")).clicked() {
                    value.as_object_mut().unwrap().remove(&name);
                }
            }
            let buffer = state
                .secrets
                .entry(format!("map-key:{path:?}"))
                .or_default();
            ui.horizontal_wrapped(|ui| {
                ui.add(egui::TextEdit::singleline(buffer).hint_text("항목 이름"));
                for (label, empty) in [
                    ("문자열 추가", json!("")),
                    ("여부 추가", json!(false)),
                    ("객체 추가", json!({})),
                    ("목록 추가", json!([])),
                ] {
                    if ui.small_button(label).clicked() && !buffer.trim().is_empty() {
                        value
                            .as_object_mut()
                            .unwrap()
                            .entry(buffer.trim().to_owned())
                            .or_insert(empty);
                        buffer.clear();
                    }
                }
            });
        }
    } else if shape["type"] == "array" {
        if !value.is_array() {
            *value = json!([]);
        }
        ui.strong(schema::label(key));
        let mut removed = None;
        for (i, item) in value.as_array_mut().unwrap().iter_mut().enumerate() {
            let mut p = path.to_vec();
            p.push(i.to_string());
            ui.group(|ui| {
                input(ui, "", item, &shape["items"], &p, state, view, depth + 1);
                if ui.small_button("항목 제거").clicked() {
                    removed = Some(i);
                }
            });
        }
        if let Some(i) = removed {
            value.as_array_mut().unwrap().remove(i);
        }
        if ui.small_button("항목 추가").clicked() {
            value
                .as_array_mut()
                .unwrap()
                .push(schema::empty(&shape["items"]));
        }
    } else {
        widgets::field(ui, key, value, &shape, path, state, depth);
    }
}
fn infer_shape(value: &Value) -> Value {
    json!({"type":match value {Value::Object(_)=>"object",Value::Array(_)=>"array",Value::Bool(_)=>"boolean",Value::Number(_)=>"number",_=>"string"}})
}
fn display_result(
    ui: &mut Ui,
    value: &Value,
    path: &[String],
    requests: &mut Vec<(Vec<String>, Value)>,
    depth: usize,
) {
    if depth > 12 {
        ui.weak("더 깊은 결과는 본문 조회로 확인하세요.");
        return;
    }
    if value.get("text").is_some() && value.get("blob").is_some() {
        if value["blob"].is_object() {
            ui.label(format!(
                "파일 본문: {} 바이트",
                value["blob"]["length"].as_str().unwrap_or("?")
            ));
            if ui.button("검증된 본문 읽기").clicked() {
                requests.push((path.to_vec(), value.clone()));
            }
        } else {
            display_text(ui, value["text"].as_str().unwrap_or(""));
        }
        return;
    }
    match value {
        Value::Object(fields) => {
            for (key, child) in fields {
                let mut p = path.to_vec();
                p.push(key.clone());
                egui::CollapsingHeader::new(schema::label(key))
                    .id_salt(&p)
                    .default_open(depth < 1)
                    .show(ui, |ui| display_result(ui, child, &p, requests, depth + 1));
            }
        }
        Value::Array(items) => {
            for (i, child) in items.iter().take(100).enumerate() {
                let mut p = path.to_vec();
                p.push(i.to_string());
                egui::CollapsingHeader::new(format!("항목 {}", i + 1))
                    .id_salt(&p)
                    .show(ui, |ui| display_result(ui, child, &p, requests, depth + 1));
            }
            if items.len() > 100 {
                ui.weak("처음 100개를 표시합니다. 다음 페이지 조건으로 나머지를 조회하세요.");
            }
        }
        Value::String(text) => {
            display_text(ui, text);
        }
        Value::Null => {
            ui.weak("없음");
        }
        value => {
            ui.add(egui::Label::new(value.to_string()).wrap().selectable(true));
        }
    }
}
fn display_text(ui: &mut Ui, text: &str) {
    ui.add(
        egui::Label::new(text.chars().take(8192).collect::<String>())
            .wrap()
            .selectable(true),
    );
    if text.chars().count() > 8192 {
        ui.horizontal_wrapped(|ui| {
            ui.weak("처음 8,192자를 표시합니다.");
            if ui.button("전체 텍스트 복사").clicked() {
                ui.ctx().copy_text(text.to_owned());
            }
        });
    }
}
fn export_body(result: &Value) -> Option<(Value, String, bool)> {
    let name = result["filename"]
        .as_str()
        .or_else(|| result["name"].as_str())
        .unwrap_or("export.bin")
        .to_owned();
    for key in ["content_base64", "base64"] {
        if let Some(value) = result.get(key) {
            let body = if value.is_string() {
                artex_core::body(value.as_str().unwrap())
            } else {
                value.clone()
            };
            if body["text"].is_string() || body["blob"].is_object() {
                return Some((body, name, true));
            }
        }
    }
    for key in ["content", "body", "report"] {
        if let Some(value) = result.get(key) {
            let body = if value.is_string() {
                artex_core::body(value.as_str().unwrap())
            } else {
                value.clone()
            };
            if body["text"].is_string() || body["blob"].is_object() {
                return Some((body, name, false));
            }
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use artex_native_core::{
        NativeAction, NativeCommand, NativeQuery, OpenMode, OpenOptions, Service, ServiceConfig,
        ServiceHandle,
    };
    use std::{
        thread,
        time::{Duration, Instant},
    };
    fn wait(
        handle: &ServiceHandle,
        ticket: artex_native_core::NativeTicket,
    ) -> artex_native_core::NativeReply {
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            for done in handle.drain_completions() {
                if done.ticket == ticket {
                    return done.result.unwrap();
                }
            }
            assert!(Instant::now() < deadline);
            thread::sleep(Duration::from_millis(3));
        }
    }
    fn fixture() -> (tempfile::TempDir, Service, ServiceHandle, NativeReadView) {
        let dir = tempfile::tempdir().unwrap();
        let service = Service::spawn(ServiceConfig::default()).unwrap();
        let handle = service.handle();
        let ticket = handle
            .try_submit(NativeAction::Open(OpenOptions {
                path: dir.path().join("workspace"),
                mode: OpenMode::Create {
                    name: "Operation drafts".into(),
                },
                read_only: false,
            }))
            .unwrap();
        wait(&handle, ticket);
        let view = handle.view().unwrap();
        (dir, service, handle, view)
    }
    fn finish(service: Service, handle: &ServiceHandle) {
        let ticket = handle.try_submit(NativeAction::Shutdown).unwrap();
        wait(handle, ticket);
        service.join().unwrap();
    }
    fn action(method: &str) -> SourceAction {
        catalog()
            .iter()
            .find(|a| a.method == method)
            .unwrap()
            .clone()
    }

    #[test]
    fn source_catalog_has_exact_routes_and_mutation_purposes_without_auth_or_transport() {
        for action in catalog() {
            assert!(
                !schema::method(&action.method).is_null(),
                "{}",
                action.method
            );
            assert!(!action.label.contains("ui.") && !action.label.contains("legacy."));
            if let Some(route) = &action.route {
                assert_eq!(action.mutation, route["legacy_http_method"] != "GET");
                assert_eq!(action.key, route["route_id"]);
            } else {
                assert_eq!(Some(action.mutation), classification(&action.method));
            }
            assert!(
                ![
                    "ui.login",
                    "ui.initPassword",
                    "ui.changePassword",
                    "ui.applyUpdate",
                    "ui.rollbackUpdate"
                ]
                .contains(&action.method.as_str())
            );
        }
        assert_eq!(catalog().iter().filter(|a| a.route.is_some()).count(), 11);
        for method in [
            "ui.findingAssetTree",
            "ui.deleteAssets",
            "ui.updateCompanyScope",
            "ui.editFindingTraffic",
            "ui.orderFindingTraffic",
            "ui.updateGoal",
            "ui.controlTasksBatch",
            "ui.notifyRetryDelivery",
            "ui.deleteSkillPath",
            "ui.mcpTools",
            "ui.restoreTaskArchives",
            "ui.taskArchive",
            "ui.testLLM",
            "ui.tokensByModel",
            "ui.renameConversation",
            "ui.pinConversation",
            "ui.updateConversationProfile",
            "ui.deleteConversations",
            "ui.conversationHistory",
            "ui.conversationMessages",
            "ui.createTaskInterceptRule",
            "ui.toggleTaskInterceptRule",
            "ui.saveLLMRetryPolicy",
            "ui.workspaceDelete",
            "ui.commands",
            "ui.explorationNodes",
            "ui.explorationGraph",
            "ui.conversationMsgDetail",
            "ui.testCustomTool",
            "ui.detectPython",
            "ui.testWebSearch",
            "ui.previewAgentPrompt",
            "ui.agentVariables",
            "ui.getAgent",
            "ui.chatMentions",
        ] {
            assert!(
                catalog().iter().any(|a| a.method == method),
                "Missing source action {method}"
            );
        }
        assert!(!action("ui.findingTrafficBody").mutation);
        assert!(action("ui.editFindingTraffic").mutation);
    }
    #[test]
    fn draft_pinned_revisions_survive_real_store_refresh_and_do_not_rebase() {
        let (_dir, service, h, view) = fixture();
        let agent = view.generation.records("agents")[0];
        let id = agent.header.id.clone();
        let key = agent.data["key"].clone();
        let mut draft = Draft::new(
            &action("ui.setAgentVisibility"),
            &view,
            Section::Agents,
            Some(&id),
        );
        let old = draft.expected[0].clone();
        draft.params = json!({"key":key,"mcp":[],"skill":[]});
        let mutation_params = json!({"key":key,"patch":{"max_turns":17}});
        let prepared = h
            .try_submit(NativeAction::Query {
                scope: view.workspace.scope.clone(),
                query: NativeQuery::Prepare {
                    method: "ui.saveAgentConfig".into(),
                    params: mutation_params.clone(),
                },
            })
            .unwrap();
        let required: Vec<Expected> =
            serde_json::from_value(wait(&h, prepared).value["expected"].clone()).unwrap();
        assert!(required.iter().any(|r| r.collection == "agent_prompts"));
        let mut mutation_expected = vec![old.clone()];
        for reference in required {
            if mutation_expected
                .iter()
                .any(|r| r.collection == reference.collection && r.id == reference.id)
            {
                continue;
            }
            // Prepare discovers dependencies; their versions still come from
            // the original immutable read, never a refreshed generation.
            let row = view
                .generation
                .get(&reference.collection, &reference.id)
                .unwrap();
            mutation_expected.push(crate::model::expected(row));
        }
        // The writer's maintenance tick may commit unrelated lifecycle data
        // between reads. Prepare must leave every planned dependency unchanged.
        let after_prepare = h.snapshot().unwrap();
        for expected in &mutation_expected {
            let original = view
                .generation
                .get(&expected.collection, &expected.id)
                .unwrap();
            let current = after_prepare
                .get(&expected.collection, &expected.id)
                .unwrap();
            assert_eq!(current.header.version, original.header.version);
            assert_eq!(current.data, original.data);
        }
        assert_ne!(agent.data["max_turns"], json!(17));
        assert_eq!(
            after_prepare.get("agents", &id).unwrap().data["max_turns"],
            agent.data["max_turns"]
        );
        let changed = h
            .try_submit(NativeAction::Command {
                scope: view.workspace.scope.clone(),
                command: NativeCommand::new(
                    "ui.saveAgentConfig",
                    mutation_params,
                    mutation_expected,
                ),
            })
            .unwrap();
        wait(&h, changed);
        assert_ne!(
            h.snapshot()
                .unwrap()
                .get("agents", &id)
                .unwrap()
                .header
                .version,
            old.version
        );
        let submitted = draft.submit(false).unwrap();
        assert_eq!(submitted.expected[0].version, old.version);
        assert_eq!(
            submitted
                .pinned
                .generation
                .get("agents", &id)
                .unwrap()
                .header
                .version,
            old.version
        );
        assert_eq!(submitted.params, json!({"key":key,"mcp":[],"skill":[]}));
        finish(service, &h);
    }
    #[test]
    fn side_question_payloads_use_real_parent_paths_and_preserve_request_identity() {
        let (_dir, service, h, view) = fixture();
        let parent = artex_core::new_id("conversation");
        let selected = catalog()
            .iter()
            .find(|a| {
                a.route.as_ref().is_some_and(|r| {
                    r["parent_kind"] == "conversation" && r["legacy_http_method"] == "POST"
                })
            })
            .unwrap();
        let mut draft = Draft::new(selected, &view, Section::Chat, Some(&parent));
        draft.params["body"]["question"] = json!("What evidence supports this result?");
        let client = draft.params["body"]["client_request_id"].clone();
        let submitted = draft.submit(false).unwrap();
        assert_eq!(submitted.params["path"]["id"], parent);
        assert_eq!(submitted.params["route_id"], selected.key);
        assert_eq!(submitted.params["body"]["client_request_id"], client);
        assert!(submitted.mutation);
        assert!(!submitted.params.to_string().contains("checkpoint.create"));
        finish(service, &h);
    }
    #[test]
    fn destructive_confirmation_and_readonly_gates_apply_before_admission() {
        let (_dir, service, h, view) = fixture();
        let mut draft = Draft::new(
            &action("ui.deleteTaskArchives"),
            &view,
            Section::Archives,
            None,
        );
        draft.params = json!({"archiveIds":[artex_core::new_id("archive")]});
        assert_eq!(draft.submit(false).err().unwrap().code, "PERMISSION_DENIED");
        draft.confirm = true;
        assert_eq!(draft.submit(true).err().unwrap().code, "RECOVERY_REQUIRED");
        let valid = draft.submit(false).unwrap();
        assert!(valid.mutation);
        assert!(!valid.params.get("confirmed").is_some());
        finish(service, &h);
    }
    #[test]
    fn scoped_secret_and_body_callbacks_require_exact_pending_paths_and_tokens() {
        let (_dir, service, h, view) = fixture();
        let mut operations = SourceOperations::default();
        let mut draft = Draft::new(&action("ui.testLLM"), &view, Section::Models, None);
        let token = draft.token.clone();
        let owner = draft.owner.clone();
        let path = vec!["api_key".to_owned()];
        draft
            .pending_secrets
            .insert(serde_json::to_string(&path).unwrap());
        draft.params = json!({"api_key":null});
        draft.result = Some(
            json!({"report":{"text":null,"blob":{"sha256":"a".repeat(64),"length":"90000","media_type":"text/plain"}}}),
        );
        let result_path = vec!["@result".into(), "report".into()];
        draft
            .pending_reads
            .insert(serde_json::to_string(&result_path).unwrap());
        operations.drafts.insert("fixture".into(), draft);
        let reference = json!({"secret_ref":artex_core::new_id("secret"),"hint":"fixture"});
        let target =
            json!({"form_token":token,"collection":"llm_profiles","path":path}).to_string();
        let mut wrong = owner.clone();
        wrong.engine_epoch = artex_core::new_id("epoch");
        assert!(!operations.secret_result(&wrong, &target, reference.clone()));
        assert!(!operations.secret_result(&owner, &target, json!("plaintext")));
        assert!(operations.secret_result(&owner, &target, reference.clone()));
        assert_eq!(operations.drafts["fixture"].params["api_key"], reference);
        assert!(!operations.secret_result(&owner, &target, reference));
        assert!(!operations.body_result(&wrong, &token, &result_path, "wrong".into()));
        assert!(operations.body_result(&owner, &token, &result_path, "verified report".into()));
        assert_eq!(
            operations.drafts["fixture"].result.as_ref().unwrap()["report"]["text"],
            "verified report"
        );
        finish(service, &h);
    }
    #[test]
    fn retire_does_not_discard_drafts_or_accept_late_mutation_completion() {
        let (_dir, service, h, view) = fixture();
        let mut operations = SourceOperations::default();
        let mut draft = Draft::new(&action("ui.createCompany"), &view, Section::Assets, None);
        draft.params = json!({"name":"Keep this unsaved company","scope":[]});
        draft.submit(false).unwrap();
        let token = draft.token.clone();
        let owner = draft.owner.clone();
        let params = draft.params.clone();
        operations.drafts.insert("company".into(), draft);
        operations.retire_async_operations();
        assert_eq!(operations.drafts["company"].params, params);
        assert!(operations.drafts["company"].reset_required);
        assert!(!operations.result(
            &owner,
            &token,
            "ui.createCompany",
            json!({"id":artex_core::new_id("company")})
        ));
        finish(service, &h);
    }
    #[test]
    fn failed_body_and_secret_requests_preserve_other_paths_and_accept_their_real_completion() {
        let (_dir, service, h, view) = fixture();
        let mut operations = SourceOperations::default();
        let mut draft = Draft::new(&action("ui.testLLM"), &view, Section::Models, None);
        let token = draft.token.clone();
        let scope = draft.owner.clone();
        let path_a = vec!["@result".into(), "a".into()];
        let path_b = vec!["@result".into(), "b".into()];
        let body_a = json!({"text":null,"blob":{"sha256":"a".repeat(64),"length":"90000","media_type":"text/plain"}});
        let body_b = json!({"text":"Verified second body","blob":null});
        draft.result = Some(json!({"a":body_a,"b":{"text":null,"blob":null}}));
        for path in [&path_a, &path_b] {
            draft
                .pending_reads
                .insert(serde_json::to_string(path).unwrap());
        }
        let secret_a = vec!["api_key".to_owned()];
        let secret_b = vec!["other_key".to_owned()];
        draft.params = json!({"api_key":null,"other_key":null});
        for path in [&secret_a, &secret_b] {
            draft
                .pending_secrets
                .insert(serde_json::to_string(path).unwrap());
        }
        operations.drafts.insert("fixture".into(), draft);
        let first = h
            .try_submit(NativeAction::Query {
                scope: scope.clone(),
                query: NativeQuery::Body { body: body_a },
            })
            .unwrap();
        let second = h
            .try_submit(NativeAction::Query {
                scope: scope.clone(),
                query: NativeQuery::Body { body: body_b },
            })
            .unwrap();
        let deadline = Instant::now() + Duration::from_secs(10);
        let mut completions = BTreeMap::new();
        while !completions.contains_key(&first) || !completions.contains_key(&second) {
            for completion in h.drain_completions() {
                completions.insert(completion.ticket, completion.result);
            }
            assert!(Instant::now() < deadline);
            thread::sleep(Duration::from_millis(3));
        }
        let failed = completions.remove(&first).unwrap().unwrap_err();
        assert!(operations.body_error(&scope, &token, &path_a, &failed.code, &failed.message));
        assert_eq!(operations.drafts["fixture"].pending_reads.len(), 1);
        assert_eq!(operations.drafts["fixture"].pending_secrets.len(), 2);
        assert!(!operations.body_error(&scope, &token, &path_a, &failed.code, &failed.message));
        assert_eq!(
            operations
                .drafts
                .get_mut("fixture")
                .unwrap()
                .submit(false)
                .err()
                .unwrap()
                .code,
            "BUSY"
        );
        let second = completions.remove(&second).unwrap().unwrap();
        assert!(operations.body_result(
            &scope,
            &token,
            &path_b,
            second.value.as_str().unwrap().into()
        ));
        assert_eq!(
            operations.drafts["fixture"].result.as_ref().unwrap()["b"]["text"],
            "Verified second body"
        );
        let target_a =
            json!({"form_token":token,"collection":"llm_profiles","path":secret_a}).to_string();
        assert!(operations.secret_error(
            &scope,
            &target_a,
            "EXTERNAL_FAILURE",
            "vault unavailable"
        ));
        assert_eq!(operations.drafts["fixture"].pending_secrets.len(), 1);
        let target_b =
            json!({"form_token":token,"collection":"llm_profiles","path":secret_b}).to_string();
        let reference = json!({"secret_ref":artex_core::new_id("secret"),"hint":"fixture"});
        assert!(operations.secret_result(&scope, &target_b, reference.clone()));
        assert_eq!(operations.drafts["fixture"].params["other_key"], reference);
        // Main action errors also leave independent helper requests intact.
        let draft = operations.drafts.get_mut("fixture").unwrap();
        draft.busy = true;
        draft
            .pending_reads
            .insert(serde_json::to_string(&path_b).unwrap());
        draft
            .pending_secrets
            .insert(serde_json::to_string(&secret_b).unwrap());
        assert!(operations.error(
            &scope,
            &token,
            "ui.testLLM",
            "EXTERNAL_FAILURE",
            "test failed"
        ));
        assert_eq!(operations.drafts["fixture"].pending_reads.len(), 1);
        assert_eq!(operations.drafts["fixture"].pending_secrets.len(), 1);
        operations.retire_async_operations();
        assert!(!operations.body_result(&scope, &token, &path_b, "late".into()));
        finish(service, &h);
    }
    #[test]
    fn inline_and_expanded_management_keep_the_same_pinned_editor_and_pending_owners() {
        let (_dir, service, h, original) = fixture();
        let params = json!({"p":{"name":"Presentation fixture","format":"openai","model":"fixture","base_url":"http://127.0.0.1:1","api_key":null}});
        let prepared = h
            .try_submit(NativeAction::Query {
                scope: original.workspace.scope.clone(),
                query: NativeQuery::Prepare {
                    method: "ui.saveLLMProfile".into(),
                    params: params.clone(),
                },
            })
            .unwrap();
        let expected =
            serde_json::from_value(wait(&h, prepared).value["expected"].clone()).unwrap();
        let created = h
            .try_submit(NativeAction::Command {
                scope: original.workspace.scope.clone(),
                command: NativeCommand::new("ui.saveLLMProfile", params, expected),
            })
            .unwrap();
        let id = wait(&h, created).value["id"].as_str().unwrap().to_owned();
        let view = h.view().unwrap();
        let chosen = action("ui.testLLM");
        let key = format!("{}:{id}", chosen.key);
        let mut operations = SourceOperations::default();
        operations.workspace_id = Some(view.workspace.scope.workspace_id.clone());
        operations
            .choices
            .insert(Section::Models, chosen.key.clone());
        let ctx = egui::Context::default();
        crate::app::configure(&ctx);
        let render = |operations: &mut SourceOperations, expanded: bool| {
            ctx.run(
                egui::RawInput {
                    screen_rect: Some(egui::Rect::from_min_size(
                        egui::Pos2::ZERO,
                        egui::vec2(900.0, 1200.0),
                    )),
                    ..Default::default()
                },
                |ctx| {
                    egui::CentralPanel::default().show(ctx, |ui| {
                        let output = if expanded {
                            operations.show_expanded(ui, &view, Section::Models, Some(&id))
                        } else {
                            operations.show(ui, &view, Section::Models, Some(&id))
                        };
                        assert!(output.is_empty());
                    });
                },
            )
        };
        let expanded = render(&mut operations, true);
        assert!(!expanded.shapes.iter().any(|shape|matches!(&shape.shape,egui::epaint::Shape::Text(text) if text.galley.text()=="추가 관리")));
        let draft = operations.drafts.get_mut(&key).unwrap();
        draft.params["model"] = json!("unsaved-model-draft");
        draft
            .secrets
            .insert("private-key-buffer".into(), "not-submitted".into());
        draft
            .pending_secrets
            .insert(serde_json::to_string(&vec!["api_key"]).unwrap());
        draft
            .pending_reads
            .insert(serde_json::to_string(&vec!["@result", "body"]).unwrap());
        let token = draft.token.clone();
        let params = draft.params.clone();
        let expected = draft.expected.clone();
        assert!(
            !expected.is_empty(),
            "Actual saved model must pin a nonempty revision"
        );
        let pinned = draft.pinned.clone();
        let secrets = draft.secrets.clone();
        let pending_secrets = draft.pending_secrets.clone();
        let pending_reads = draft.pending_reads.clone();
        for expanded in [false, true, false, true] {
            let _ = render(&mut operations, expanded);
            let draft = &operations.drafts[&key];
            assert_eq!(draft.token, token);
            assert_eq!(draft.owner, view.workspace.scope);
            assert_eq!(draft.params, params);
            assert_eq!(draft.expected, expected);
            assert!(std::sync::Arc::ptr_eq(
                &draft.pinned.generation,
                &pinned.generation
            ));
            assert_eq!(draft.secrets, secrets);
            assert_eq!(draft.pending_secrets, pending_secrets);
            assert_eq!(draft.pending_reads, pending_reads);
        }
        assert_eq!(operations.drafts.len(), 1);
        finish(service, &h);
    }
    #[test]
    fn docked_result_text_wraps_within_width_and_copy_preserves_the_complete_value() {
        let ctx = egui::Context::default();
        crate::app::configure(&ctx);
        let full = "A long stored result value ".repeat(600);
        let result = json!(full);
        let mut requests = vec![];
        let mut render = |events| {
            ctx.run(
                egui::RawInput {
                    screen_rect: Some(egui::Rect::from_min_size(
                        egui::Pos2::ZERO,
                        egui::vec2(640.0, 3400.0),
                    )),
                    events,
                    ..Default::default()
                },
                |ctx| {
                    egui::CentralPanel::default().show(ctx, |ui| {
                        display_result(ui, &result, &["@result".into()], &mut requests, 0)
                    });
                },
            )
        };
        let first = render(vec![]);
        let copy = first
            .shapes
            .iter()
            .find_map(|shape| {
                if let egui::epaint::Shape::Text(text) = &shape.shape {
                    (text.galley.text() == "전체 텍스트 복사")
                        .then(|| text.visual_bounding_rect().center())
                } else {
                    None
                }
            })
            .unwrap();
        for shape in &first.shapes {
            if let egui::epaint::Shape::Text(text) = &shape.shape {
                if text.galley.text().starts_with("A long stored result") {
                    assert!(text.visual_bounding_rect().width() <= 640.0);
                }
            }
        }
        let output = render(vec![
            egui::Event::PointerMoved(copy),
            egui::Event::PointerButton {
                pos: copy,
                button: egui::PointerButton::Primary,
                pressed: true,
                modifiers: egui::Modifiers::NONE,
            },
            egui::Event::PointerButton {
                pos: copy,
                button: egui::PointerButton::Primary,
                pressed: false,
                modifiers: egui::Modifiers::NONE,
            },
        ]);
        drop(render);
        assert!(
            output.platform_output.commands.iter().any(
                |command| matches!(command,egui::OutputCommand::CopyText(text) if text==&full)
            )
        );
        assert_eq!(result.as_str().unwrap().len(), full.len());
        assert!(requests.is_empty());
    }
    #[test]
    fn actual_egui_action_inputs_render_and_binary_exports_keep_encoding_purpose() {
        let (_dir, service, h, view) = fixture();
        let mut operations = SourceOperations::default();
        let ctx = egui::Context::default();
        // egui's public renderer-test mode sets actual CollapsingState openness
        // to 1.0, including the internal vertical child UI created by show_dyn.
        ctx.memory_mut(|memory| memory.set_everything_is_visible(true));
        for section in [
            Section::Assets,
            Section::Findings,
            Section::Tasks,
            Section::Sessions,
            Section::Approvals,
            Section::Mcp,
            Section::Models,
            Section::Archives,
        ] {
            let before = operations.drafts.len();
            let input = egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(1920.0, 1080.0),
                )),
                ..Default::default()
            };
            let _ = ctx.run(input, |ctx| {
                egui::CentralPanel::default().show(ctx, |ui| {
                    operations.show(ui, &view, section, None);
                });
            });
            assert!(
                operations.drafts.len() > before,
                "Actual input renderer did not create the {section:?} draft: before={before}, after={}",
                operations.drafts.len()
            );
        }
        assert!(
            operations.drafts.len() >= 8,
            "Expanded headers must render actual operation inputs: rendered={}",
            operations.drafts.len()
        );
        let encoded =
            json!({"filename":"evidence.bin","content_base64":{"text":"AAH/","blob":null}});
        let (body, name, base64) = export_body(&encoded).unwrap();
        assert!(base64);
        assert_eq!(name, "evidence.bin");
        assert_eq!(body["text"], "AAH/");
        assert!(
            !export_body(&json!({"body":{"text":"raw report","blob":null}}))
                .unwrap()
                .2
        );
        finish(service, &h);
    }
}
