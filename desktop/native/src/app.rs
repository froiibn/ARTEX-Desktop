use crate::{
    forms::{
        Forms,
        operations::{OperationSubmission, SourceOperations},
    },
    graph::Graph,
    model::{NAVIGATION_GROUPS, Section, display_name, records, selected_record},
    panes::{Effect, Panes},
    tray::{NativeTray, TrayAction},
};
use artex_core::{Error, Expected, Result};
use artex_host::{LaunchListener, ProfileInstanceLock, SecretVault};
use artex_native_core::lifecycle::{NativePower, PowerNotice};
use artex_native_core::{
    NativeAction, NativeCommand, NativeFileArea, NativeQuery, NativeReadView, NativeTicket,
    OpenMode, OpenOptions, Service, ServiceConfig, ServiceHandle, ServicePhase, WorkspaceScope,
    gateway::NativeGateway,
};
use eframe::egui::{self, RichText, Vec2};
use serde_json::{Value, json};
use std::{
    collections::{BTreeMap, HashMap, VecDeque},
    path::PathBuf,
    sync::Arc,
    time::{Duration, Instant},
};

#[path = "workbench.rs"]
mod workbench;
pub use workbench::DrawerTab;
#[cfg(test)]
pub(crate) use workbench::{
    CompactToolbarPresentation, compact_toolbar, drawer_panel, drawer_scroll,
};

#[derive(Clone)]
enum Purpose {
    Prepare {
        method: String,
        params: Value,
        expected: Vec<Expected>,
        pinned: NativeReadView,
        scope: WorkspaceScope,
        then: Box<Purpose>,
    },
    Open(PathBuf),
    Command {
        method: String,
        params: Value,
        section: Section,
        selected: Option<String>,
        scope: WorkspaceScope,
    },
    FormsSave {
        token: Option<String>,
        scope: WorkspaceScope,
    },
    FormsModelTest {
        token: Option<String>,
        scope: WorkspaceScope,
    },
    Projection {
        method: String,
        params: Value,
        section: Section,
        selected: Option<String>,
        scope: WorkspaceScope,
        form_token: Option<String>,
    },
    NativeOperation {
        token: String,
        scope: WorkspaceScope,
        mutation: bool,
    },
    Secret {
        target: String,
        scope: WorkspaceScope,
    },
    Checkpoint,
    BypassToggle {
        scope: WorkspaceScope,
        enabled: bool,
    },
    Close,
    Suspend,
    Shutdown,
    FileRead {
        path: String,
        scope: WorkspaceScope,
    },
    FileWrite {
        path: String,
        scope: WorkspaceScope,
        submitted: String,
    },
    SkillFileSave {
        token: String,
        file_token: u64,
        scope: WorkspaceScope,
        submitted: String,
    },
    SourceOperation {
        method: String,
        token: String,
        scope: WorkspaceScope,
    },
    SourceBody {
        token: String,
        path: Vec<String>,
        scope: WorkspaceScope,
    },
    SourceSecret {
        token: String,
        target: String,
        scope: WorkspaceScope,
    },
    StageFile {
        purpose: String,
        scope: WorkspaceScope,
    },
    SaveFile {
        token: String,
        scope: WorkspaceScope,
    },
    ReleaseFile,
    ExportFile {
        name: String,
        scope: WorkspaceScope,
    },
}
pub struct NativeApp {
    service: Option<Service>,
    pub handle: ServiceHandle,
    power: NativePower,
    suspended: bool,
    last_workspace: Option<PathBuf>,
    last_read_only: bool,
    power_warning_recorded: bool,
    _instance: Arc<ProfileInstanceLock>,
    launch: LaunchListener,
    tray: NativeTray,
    background: bool,
    tray_warning_recorded: bool,
    incoming_workspace: Option<OpenOptions>,
    next_workspace: Option<OpenOptions>,
    pub section: Section,
    selected: BTreeMap<Section, Option<String>>,
    pub panes: Panes,
    forms: Forms,
    source_operations: SourceOperations,
    graph: Graph,
    pending: HashMap<NativeTicket, Purpose>,
    recent: Vec<PathBuf>,
    profile: PathBuf,
    workspace_name: String,
    search: String,
    error: Option<Error>,
    notices: VecDeque<String>,
    close_confirmation: bool,
    exit_after_close: bool,
    allow_exit: bool,
    last_query: Instant,
    pub screenshot: Option<PathBuf>,
    pub smoke: bool,
    pub smoke_target: Section,
    pub smoke_dock: Option<DrawerTab>,
    pub smoke_dialog: bool,
    pub smoke_trash: bool,
    pub smoke_about: bool,
    pub smoke_hold_until: Option<PathBuf>,
    smoke_hold_released: bool,
    smoke_about_started: Option<Instant>,
    smoke_about_settled: bool,
    smoke_started: Instant,
    smoke_sections: Vec<Section>,
    smoke_final_requested: bool,
    command_read: Option<NativeReadView>,
    smoke_capture: bool,
    smoke_management: Option<Value>,
    smoke_launch_requests: Vec<Value>,
    skill_catalog: Vec<Value>,
    drawer: workbench::DrawerState,
    inspector_preferences: BTreeMap<Section, bool>,
    list_preferences: workbench::ListPreferences,
    about_open: bool,
    navigation_reveal: bool,
}
impl NativeApp {
    pub fn new(
        cc: &eframe::CreationContext<'_>,
        profile: PathBuf,
        workspace: Option<OpenOptions>,
        smoke: bool,
        screenshot: Option<PathBuf>,
        instance: Arc<ProfileInstanceLock>,
    ) -> Result<Self> {
        configure(&cc.egui_ctx);
        let vault = Arc::new(SecretVault::default());
        let host_profile = profile.clone();
        let factory: artex_native_core::HostFactory = Arc::new(move |workspace| {
            Ok(Arc::new(NativeGateway::open_with_profile(
                workspace,
                vault.clone(),
                &host_profile,
            )?))
        });
        let ctx = cc.egui_ctx.clone();
        let wake: Arc<dyn Fn() + Send + Sync> = Arc::new(move || ctx.request_repaint());
        let service = Service::spawn(ServiceConfig {
            host_factory: Some(factory),
            wake_ui: Some(wake.clone()),
        })?;
        let handle = service.handle();
        let power = NativePower::start(handle.clone(), wake.clone());
        let foreground = cc.egui_ctx.clone();
        let launch = instance.start_launch_listener(Arc::new(move || {
            foreground.send_viewport_cmd(egui::ViewportCommand::Visible(true));
            foreground.send_viewport_cmd(egui::ViewportCommand::Focus);
            foreground.request_repaint();
        }))?;
        let tray = NativeTray::new(wake);
        let preferences = std::fs::read(profile.join("native-ui-preferences.json"))
            .ok()
            .and_then(|b| serde_json::from_slice::<Value>(&b).ok())
            .unwrap_or_default();
        let background = preferences["close_to_tray"] == true;
        cc.egui_ctx
            .set_theme(if preferences["dark_theme"].as_bool().unwrap_or(true) {
                egui::Theme::Dark
            } else {
                egui::Theme::Light
            });
        let recent = std::fs::read(profile.join("recent-workspaces.json"))
            .ok()
            .and_then(|b| serde_json::from_slice(&b).ok())
            .unwrap_or_default();
        let mut forms = Forms::default();
        forms.set_allowed_operations(Forms::all_known_operations());
        let mut app = Self {
            service: Some(service),
            handle,
            power,
            suspended: false,
            last_workspace: None,
            last_read_only: false,
            power_warning_recorded: false,
            _instance: instance,
            launch,
            tray,
            background,
            tray_warning_recorded: false,
            incoming_workspace: None,
            next_workspace: None,
            section: Section::Overview,
            selected: BTreeMap::new(),
            panes: Panes::default(),
            forms,
            source_operations: SourceOperations::default(),
            graph: Graph::default(),
            pending: HashMap::new(),
            recent,
            profile,
            workspace_name: "ARTEX".into(),
            search: String::new(),
            error: None,
            notices: VecDeque::new(),
            close_confirmation: false,
            exit_after_close: false,
            allow_exit: false,
            last_query: Instant::now(),
            screenshot,
            smoke,
            smoke_target: Section::Overview,
            smoke_dock: None,
            smoke_dialog: false,
            smoke_trash: false,
            smoke_about: false,
            smoke_hold_until: None,
            smoke_hold_released: false,
            smoke_about_started: None,
            smoke_about_settled: false,
            smoke_started: Instant::now(),
            smoke_sections: Vec::new(),
            smoke_final_requested: false,
            command_read: None,
            smoke_capture: false,
            smoke_management: None,
            smoke_launch_requests: Vec::new(),
            skill_catalog: Vec::new(),
            drawer: workbench::DrawerState::default(),
            inspector_preferences: BTreeMap::new(),
            list_preferences: workbench::ListPreferences::default(),
            about_open: false,
            navigation_reveal: true,
        };
        if let Some(options) = workspace {
            let path = options.path.clone();
            app.submit(NativeAction::Open(options), Purpose::Open(path));
        }
        Ok(app)
    }
    fn submit(&mut self, action: NativeAction, purpose: Purpose) {
        match self.handle.try_submit(action) {
            Ok(ticket) => {
                self.pending.insert(ticket, purpose);
            }
            Err(error) => self.failed(&purpose, error),
        }
    }
    fn command(
        &mut self,
        method: String,
        params: Value,
        expected: Vec<Expected>,
        purpose: Purpose,
    ) {
        if let Some(scope) = self.handle.scope() {
            let Some(pinned) = self.command_read.clone().or_else(|| self.handle.view()) else {
                return;
            };
            self.submit(
                NativeAction::Query {
                    scope: scope.clone(),
                    query: NativeQuery::Prepare {
                        method: method.clone(),
                        params: params.clone(),
                    },
                },
                Purpose::Prepare {
                    method,
                    params,
                    expected,
                    pinned,
                    scope,
                    then: Box::new(purpose),
                },
            );
        }
    }
    fn query(&mut self, method: String, mut params: Value) {
        // Close can be queued before the service publishes Closing. Prevent
        // timers and editor hydration from enqueueing a lookup behind it.
        if self.handle.status().phase != ServicePhase::Ready
            || self.pending.values().any(|purpose| {
                matches!(
                    purpose,
                    Purpose::Close | Purpose::Suspend | Purpose::Shutdown
                )
            })
        {
            return;
        }
        let Some(scope) = self.handle.scope() else {
            return;
        };
        let selected = self.selected.get(&self.section).cloned().flatten();
        self.panes.pin_history_request(&method, &mut params);
        let requested = params.clone();
        let form_token = params["_form_token"].as_str().map(str::to_owned);
        if let Some(object) = params.as_object_mut() {
            for key in [
                "_form_token",
                "_form_path",
                "_file_token",
                "_pane_token",
                "_pane_target",
            ] {
                object.remove(key);
            }
        }
        if self.pending.values().any(|p|matches!(p,Purpose::Projection{method:m,params:p,scope:s,..}if *m==method&&*p==requested&&*s==scope)){return;}
        let action = if method == "ui.readSkillFile" {
            NativeAction::FileRead {
                scope: scope.clone(),
                area: NativeFileArea::Resources,
                path: format!(
                    "skills/{}/{}",
                    params["name"].as_str().unwrap_or(""),
                    params["file"].as_str().unwrap_or("")
                ),
            }
        } else if method == "native.skill.meta.read" {
            NativeAction::SkillMetaRead {
                scope: scope.clone(),
                name: params["name"].as_str().unwrap_or("").into(),
            }
        } else {
            let query = if method == "native.body.read" {
                NativeQuery::Body {
                    body: params
                        .get("body")
                        .cloned()
                        .unwrap_or_else(|| json!({"text":null,"blob":params["reference"]})),
                }
            } else {
                NativeQuery::Feature {
                    method: method.clone(),
                    params: params.clone(),
                }
            };
            NativeAction::Query {
                scope: scope.clone(),
                query,
            }
        };
        self.submit(
            action,
            Purpose::Projection {
                method,
                params: requested,
                section: self.section,
                selected,
                scope,
                form_token,
            },
        );
    }
    fn poll(&mut self, ctx: &egui::Context) {
        for action in self.tray.poll() {
            match action {
                TrayAction::Show => {
                    ctx.send_viewport_cmd(egui::ViewportCommand::Visible(true));
                    ctx.send_viewport_cmd(egui::ViewportCommand::Focus);
                }
                TrayAction::Quit => {
                    ctx.send_viewport_cmd(egui::ViewportCommand::Visible(true));
                    self.request_close(true);
                }
            }
        }
        if !self.tray_warning_recorded {
            if let Some(reason) = self.tray.error() {
                self.notices.push_back(format!(
                    "트레이 상태: {reason}. 창을 닫으면 정상 종료합니다."
                ));
                self.tray_warning_recorded = true;
            }
        }
        for request in self.launch.poll() {
            self.smoke_launch_requests.push(json!({"request_id":request.request_id,"workspace":request.workspace,"read_only":request.read_only}));
            if self.smoke_launch_requests.len() > 32 {
                self.smoke_launch_requests.remove(0);
            }
            ctx.send_viewport_cmd(egui::ViewportCommand::Visible(true));
            ctx.send_viewport_cmd(egui::ViewportCommand::Focus);
            if let Some(path) = request.workspace {
                if self.handle.view().is_some_and(|v| {
                    v.workspace.root == path && v.workspace.read_only == request.read_only
                }) {
                    continue;
                }
                let options = OpenOptions {
                    path: path.clone(),
                    mode: OpenMode::Existing,
                    read_only: request.read_only,
                };
                if self.handle.status().phase == ServicePhase::Closed {
                    self.submit(NativeAction::Open(options), Purpose::Open(path));
                } else {
                    self.incoming_workspace = Some(options);
                }
            } else {
                self.notices.push_back(
                    "ARTEX 창을 활성화했습니다. 새 공간은 작업 공간 메뉴에서 만드세요.".into(),
                );
            }
        }
        self.panes.set_context(
            self.section,
            self.selected.get(&self.section).and_then(Option::as_deref),
        );
        self.power.tick();
        for notice in self.power.drain_notices() {
            match notice {
                PowerNotice::Suspend { ticket, fallback } => {
                    self.suspended = true;
                    self.pending.entry(ticket).or_insert(Purpose::Suspend);
                    self.notices.push_back(
                        if fallback {
                            "절전 시간 차이를 감지했습니다. 작업 공간을 저장·종료합니다."
                        } else {
                            "OS 절전 알림을 받았습니다. 실행을 정리하고 저장합니다."
                        }
                        .into(),
                    );
                }
                PowerNotice::Resume => {
                    self.notices.push_back(
                        "절전에서 복귀했습니다. 작업 공간을 명시적으로 다시 여세요.".into(),
                    );
                }
                PowerNotice::Failed(error) => self.error = Some(error),
            }
        }
        for completion in self.handle.drain_completions() {
            let Some(purpose) = self.pending.remove(&completion.ticket) else {
                continue;
            };
            match completion.result {
                Ok(reply) => match purpose {
                    Purpose::Prepare {
                        method,
                        params,
                        expected,
                        pinned,
                        scope,
                        then,
                    } => {
                        if self.handle.scope().as_ref() != Some(&scope) {
                            continue;
                        }
                        let result = serde_json::from_value::<Vec<Expected>>(
                            reply.value["expected"].clone(),
                        )
                        .map_err(Error::from)
                        .and_then(|required| {
                            merge_pinned_expected(expected, required, &pinned.generation.snapshot)
                        });
                        match result {
                            Ok(expected) => self.submit(
                                NativeAction::Command {
                                    scope,
                                    command: NativeCommand::new(method, params, expected),
                                },
                                *then,
                            ),
                            Err(error) => self.failed(&then, error),
                        }
                    }
                    Purpose::Open(path) => {
                        let preserve_drafts =
                            self.suspended && self.last_workspace.as_ref() == Some(&path);
                        self.last_workspace = Some(path.clone());
                        self.last_read_only =
                            self.handle.view().is_some_and(|v| v.workspace.read_only);
                        self.suspended = false;
                        self.recent.retain(|old| old != &path);
                        self.recent.insert(0, path);
                        self.recent.truncate(12);
                        // Unix may select an OS path that JSON cannot represent.
                        // Keep the previously saved list intact on encoding failure.
                        match serde_json::to_vec_pretty(&self.recent) {
                            Ok(bytes) => {
                                if std::fs::write(
                                    self.profile.join("recent-workspaces.json"),
                                    bytes,
                                ).is_err() {
                                    self.notices.push_back("최근 작업 공간 목록을 저장하지 못했습니다.".into());
                                }
                            }
                            Err(_) => self.notices.push_back("최근 작업 공간 목록을 저장하지 못했습니다. 목록 저장은 UTF-8 경로를 지원합니다.".into()),
                        }
                        self.notices.push_back("작업 공간을 열었습니다.".into());
                        if preserve_drafts {
                            self.notices.push_back("절전 전 초안을 유지했습니다. 중단된 요청 결과를 확인하고 충돌한 항목은 명시적으로 다시 불러오세요.".into());
                        } else {
                            self.forms = Forms::default();
                            self.forms
                                .set_allowed_operations(Forms::all_known_operations());
                            self.panes = Panes::default();
                            self.source_operations = SourceOperations::default();
                            self.graph = Graph::default();
                            self.selected.clear();
                        }
                        self.skill_catalog.clear();
                        self.command_read = None;
                    }
                    Purpose::Projection {
                        method,
                        params,
                        section,
                        selected,
                        scope,
                        form_token,
                    } => {
                        if self.handle.scope().as_ref() != Some(&scope) {
                            continue;
                        }
                        if method == "ui.skills" {
                            self.skill_catalog = reply
                                .value
                                .as_array()
                                .or_else(|| reply.value["skills"].as_array())
                                .cloned()
                                .unwrap_or_default();
                        }
                        if let Some(token) = &form_token {
                            let projected = if method == "native.body.read" {
                                json!({"_form_path":params["_form_path"],"text":reply.value})
                            } else if method == "ui.readSkillFile" {
                                json!({"_file_token":params["_file_token"],"content":reply.value["content"],"revision":reply.value["sha256"]})
                            } else {
                                json!({"result":reply.value,"read_versions":reply.read_versions})
                            };
                            self.forms.set_projection_for(token, &method, &projected);
                            continue;
                        }
                        if self.section != section
                            || self.selected.get(&section).cloned().flatten() != selected
                        {
                            if let Some(token) = reply.value["file_capability"].as_str() {
                                self.submit(
                                    NativeAction::ReleaseFile {
                                        scope,
                                        token: token.into(),
                                    },
                                    Purpose::ReleaseFile,
                                );
                            }
                            continue;
                        }
                        let projected = if method == "native.body.read" {
                            json!({"_form_path":params["_form_path"],"text":reply.value})
                        } else if method == "ui.readSkillFile" {
                            json!({"_file_token":params["_file_token"],"content":reply.value["content"],"revision":reply.value["sha256"]})
                        } else {
                            json!({"result":reply.value,"read_versions":reply.read_versions})
                        };
                        if let Some(token) = form_token {
                            self.forms.set_projection_for(&token, &method, &projected);
                        } else if params.get("_pane_token").is_none() {
                            self.forms.set_projection(&method, &projected);
                        }
                        if let Some(token) = params["_pane_token"].as_str() {
                            let value = if method == "native.body.read" {
                                json!({"target":params["_pane_target"],"text":reply.value})
                            } else {
                                reply.value
                            };
                            let file_capability =
                                value["file_capability"].as_str().map(str::to_owned);
                            let accepted = self.panes.set_projection_for(
                                &method,
                                token,
                                value,
                                reply.read_versions,
                            );
                            if !accepted {
                                if let Some(token) = file_capability {
                                    self.submit(
                                        NativeAction::ReleaseFile { scope, token },
                                        Purpose::ReleaseFile,
                                    );
                                }
                            }
                        } else {
                            self.panes.set_projection(&method, reply.value);
                        }
                    }
                    Purpose::NativeOperation {
                        token,
                        scope,
                        mutation,
                    } => {
                        if self.handle.scope().as_ref() == Some(&scope)
                            && reply.scope.as_ref() == Some(&scope)
                        {
                            self.panes.set_native_result(&token, &reply.value);
                            if mutation {
                                if let Some(view) = self.handle.view() {
                                    for section in
                                        [Section::Chat, Section::Tasks, Section::Archives]
                                    {
                                        let invalid = self
                                            .selected
                                            .get(&section)
                                            .and_then(Option::as_deref)
                                            .is_some_and(|id| {
                                                selected_record(
                                                    &view.generation.snapshot,
                                                    section,
                                                    Some(id),
                                                )
                                                .is_none()
                                            });
                                        if invalid {
                                            self.selected.insert(section, None);
                                        }
                                    }
                                }
                                self.notices.push_back(
                                    "요청을 처리했습니다. 실행 정리와 삭제 결과를 확인하세요."
                                        .into(),
                                );
                            }
                        }
                    }
                    Purpose::Secret { target, scope } => {
                        if self.handle.scope().as_ref() == Some(&scope) {
                            let reference =
                                reply.value.get("reference").cloned().unwrap_or(reply.value);
                            let persistent = SecretVault::persistent(&reference);
                            self.forms.set_secret_reference(&target, reference);
                            self.notices.push_back(if persistent{"자격 증명을 OS 저장소에 보관했습니다. 설정 저장으로 적용하세요."}else{"자격 증명은 이번 앱 실행 동안만 보관됩니다. 앱을 다시 열면 재입력해야 합니다."}.into());
                        }
                    }
                    Purpose::FormsSave { token, scope } => {
                        if self.handle.scope().as_ref() == Some(&scope) {
                            if let Some(token) = token {
                                self.forms.acknowledge_saved_for(&token);
                            }
                        }
                        self.notices.push_back("설정을 저장했습니다.".into());
                    }
                    Purpose::FormsModelTest { token, scope } => {
                        if self.handle.scope().as_ref() == Some(&scope) {
                            if let Some(token) = token {
                                self.forms.set_model_test_result_for(&token, &reply.value);
                            }
                        }
                    }
                    Purpose::SkillFileSave {
                        token,
                        file_token,
                        scope,
                        submitted,
                    } => {
                        if self.handle.scope().as_ref() == Some(&scope) {
                            if let Some(sha) = reply.value["sha256"].as_str() {
                                self.forms.set_skill_file_saved_content(
                                    &token, file_token, sha, &submitted,
                                );
                            }
                        }
                        self.notices.push_back("Skill 파일을 저장했습니다.".into());
                    }
                    Purpose::SourceOperation {
                        method,
                        token,
                        scope,
                    } => {
                        if self.handle.scope().as_ref() == Some(&scope) {
                            let file_capability =
                                reply.value["file_capability"].as_str().map(str::to_owned);
                            if !self
                                .source_operations
                                .result(&scope, &token, &method, reply.value)
                            {
                                if let Some(token) = file_capability {
                                    self.submit(
                                        NativeAction::ReleaseFile { scope, token },
                                        Purpose::ReleaseFile,
                                    );
                                }
                            }
                        }
                    }
                    Purpose::SourceBody { token, path, scope } => {
                        if self.handle.scope().as_ref() == Some(&scope) {
                            if let Some(text) = reply.value.as_str() {
                                self.source_operations.body_result(
                                    &scope,
                                    &token,
                                    &path,
                                    text.into(),
                                );
                            } else {
                                self.failed(
                                    &Purpose::SourceBody { token, path, scope },
                                    Error::new(
                                        "EXTERNAL_FAILURE",
                                        "본문 조회 결과가 문자열이 아닙니다.",
                                    ),
                                );
                            }
                        }
                    }
                    Purpose::SourceSecret {
                        token,
                        target,
                        scope,
                    } => {
                        if self.handle.scope().as_ref() == Some(&scope)
                            && self.source_operations.owns_token(&token)
                        {
                            let reference =
                                reply.value.get("reference").cloned().unwrap_or(reply.value);
                            let persistent = SecretVault::persistent(&reference);
                            if self
                                .source_operations
                                .secret_result(&scope, &target, reference)
                            {
                                self.notices.push_back(if persistent{"자격 증명을 OS 저장소에 보관했습니다. 변경 실행으로 적용하세요."}else{"자격 증명은 이번 앱 실행 동안만 보관됩니다. 다시 열면 재입력해야 합니다."}.into());
                            }
                        }
                    }
                    Purpose::StageFile { purpose, scope } => {
                        if self.handle.scope().as_ref() == Some(&scope) {
                            self.panes.set_staged_file(&purpose, reply.value);
                        }
                    }
                    Purpose::ExportFile { name, scope } => {
                        if self.handle.scope().as_ref() == Some(&scope) {
                            if let Some(token) = reply.value["file_capability"].as_str() {
                                self.effects(vec![Effect::SaveStagedFile {
                                    token: token.into(),
                                    name,
                                }]);
                            } else {
                                self.error = Some(Error::new(
                                    "EXTERNAL_FAILURE",
                                    "내보내기 파일 참조가 없습니다.",
                                ));
                            }
                        }
                    }
                    Purpose::SaveFile { token, scope } => {
                        self.notices.push_back("내보내기를 완료했습니다.".into());
                        if self.handle.scope().as_ref() == Some(&scope) {
                            self.submit(
                                NativeAction::ReleaseFile { scope, token },
                                Purpose::ReleaseFile,
                            );
                        }
                    }
                    Purpose::ReleaseFile => {}
                    Purpose::Command {
                        method,
                        params,
                        section,
                        selected,
                        scope,
                    } => {
                        self.notices.push_back(format!("완료: {method}"));
                        if self.handle.scope().as_ref() == Some(&scope) {
                            self.panes.set_command_result_for_params(
                                &method,
                                &params,
                                selected.as_deref(),
                                reply.value.clone(),
                            );
                        }
                        if self.handle.scope().as_ref() == Some(&scope)
                            && self.section == section
                            && self.selected.get(&section).cloned().flatten() == selected
                        {
                            self.panes.set_projection(&method, reply.value.clone());
                            if method == "ui.createConversation" {
                                if let Some(id) = reply.value["id"].as_str() {
                                    self.section = Section::Chat;
                                    self.selected.insert(Section::Chat, Some(id.into()));
                                }
                            }
                            if method == "ui.createTask" {
                                if let Some(id) = reply.value["task_id"]
                                    .as_str()
                                    .or_else(|| reply.value["id"].as_str())
                                {
                                    self.section = Section::Tasks;
                                    self.selected.insert(Section::Tasks, Some(id.into()));
                                }
                            }
                        }
                    }
                    Purpose::Checkpoint => {
                        self.notices.push_back("체크포인트를 저장했습니다.".into())
                    }
                    Purpose::BypassToggle { scope, enabled } => {
                        if self.handle.scope().as_ref() == Some(&scope) {
                            self.notices.push_back(if enabled {
                                "작업 공간 Bypass 켜짐: 앱 내부 도구 실행 승인을 생략합니다.".into()
                            } else {
                                "작업 공간 Bypass 꺼짐: 도구 실행 승인 정책을 적용합니다.".into()
                            });
                        }
                    }
                    Purpose::FileRead { path, scope } => {
                        if self.handle.scope().as_ref() == Some(&scope)
                            && self.panes.file_path == path
                        {
                            self.panes.set_projection("native.file.read", reply.value);
                        }
                    }
                    Purpose::FileWrite {
                        path,
                        scope,
                        submitted,
                    } => {
                        if self.handle.scope().as_ref() == Some(&scope)
                            && self.panes.file_path == path
                        {
                            self.panes.file_dirty = self.panes.file_text != submitted;
                            self.panes.file_revision =
                                reply.value["sha256"].as_str().map(str::to_owned);
                            self.panes.file_loaded = Some(path);
                        }
                        self.notices.push_back("파일을 저장했습니다.".into());
                    }
                    Purpose::Close => {
                        self.selected.clear();
                        self.command_read = None;
                        self.skill_catalog.clear();
                        self.forms = Forms::default();
                        self.panes = Panes::default();
                        self.source_operations = SourceOperations::default();
                        self.graph = Graph::default();
                        if self.exit_after_close {
                            self.submit(NativeAction::Shutdown, Purpose::Shutdown);
                        } else if let Some(options) = self.next_workspace.take() {
                            let path = options.path.clone();
                            self.submit(NativeAction::Open(options), Purpose::Open(path));
                        }
                    }
                    Purpose::Suspend => {
                        self.command_read = None;
                        self.forms.retire_async_operations();
                        self.panes.retire_async_operations();
                        self.source_operations.retire_async_operations();
                        self.notices.push_back(
                            "절전 전 실행 정리와 저장이 완료됐습니다. 자동 재실행은 하지 않습니다."
                                .into(),
                        );
                    }
                    Purpose::Shutdown => {
                        self.allow_exit = true;
                        if let Some(service) = self.service.take() {
                            if let Err(error) = service.join() {
                                self.error = Some(error);
                                self.allow_exit = false;
                            }
                        }
                        if !self.smoke_sections.is_empty() {
                            self.write_smoke_report(self.smoke_capture);
                        }
                        if self.allow_exit {
                            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                        }
                    }
                },
                Err(error) => {
                    let failure = if let Purpose::Prepare { then, .. } = &purpose {
                        then.as_ref()
                    } else {
                        &purpose
                    };
                    self.failed(failure, error);
                }
            }
        }
        let events = self.handle.drain_events();
        if events.resync_required {
            self.notices
                .push_back("변경 이벤트를 합쳤습니다. 최신 저장 세대를 표시합니다.".into());
        }
        for event in events.events.into_iter().rev().take(10) {
            self.notices.push_back(
                event.record.data["kind"]
                    .as_str()
                    .unwrap_or("객체 변경")
                    .into(),
            );
        }
        while self.notices.len() > 40 {
            self.notices.pop_front();
        }
    }
    fn failed(&mut self, purpose: &Purpose, error: Error) {
        if let Purpose::NativeOperation { token, scope, .. } = purpose {
            if self.handle.scope().as_ref() == Some(scope) {
                self.panes
                    .set_native_error(token, &error.code, &error.message);
            }
            return;
        }
        let failure = if let Purpose::Prepare { then, .. } = purpose {
            then.as_ref()
        } else {
            purpose
        };
        match failure {
            Purpose::SourceOperation {
                method,
                token,
                scope,
            } => {
                if self.handle.scope().as_ref() == Some(scope) {
                    self.source_operations
                        .error(scope, token, method, &error.code, &error.message);
                }
            }
            Purpose::SourceBody { token, scope, path } => {
                if self.handle.scope().as_ref() == Some(scope) {
                    self.source_operations.body_error(
                        scope,
                        token,
                        path,
                        &error.code,
                        &error.message,
                    );
                }
            }
            Purpose::SourceSecret {
                scope,
                target,
                token,
            } => {
                if self.handle.scope().as_ref() == Some(scope)
                    && self.source_operations.owns_token(token)
                {
                    self.source_operations
                        .secret_error(scope, target, &error.code, &error.message);
                }
            }
            Purpose::Command {
                method,
                params,
                selected,
                scope,
                ..
            } => {
                if self.handle.scope().as_ref() == Some(scope) {
                    self.panes.set_command_error_for_params(
                        method,
                        params,
                        selected.as_deref(),
                        &error.code,
                        &error.message,
                    );
                }
            }
            Purpose::FormsSave { token, scope } => {
                if self.handle.scope().as_ref() == Some(scope) {
                    if let Some(token) = token {
                        self.forms.set_error_for(token, &error.code, &error.message);
                    }
                }
            }
            Purpose::FormsModelTest { token, scope } => {
                if self.handle.scope().as_ref() == Some(scope) {
                    if let Some(token) = token {
                        self.forms
                            .set_model_test_error_for(token, &error.code, &error.message);
                    }
                }
            }
            Purpose::SkillFileSave { token, scope, .. } => {
                if self.handle.scope().as_ref() == Some(scope) {
                    self.forms.set_error_for(token, &error.code, &error.message);
                }
            }
            Purpose::Secret { target, scope } => {
                if self.handle.scope().as_ref() == Some(scope) {
                    self.forms
                        .set_secret_error(target, &error.code, &error.message);
                }
            }
            Purpose::StageFile { purpose, scope } => {
                if self.handle.scope().as_ref() == Some(scope) {
                    self.panes
                        .set_staged_error(purpose, &error.code, &error.message);
                }
            }
            Purpose::Projection {
                method,
                params,
                form_token,
                scope,
                ..
            } => {
                if self.handle.scope().as_ref() == Some(scope) {
                    if let Some(token) = form_token {
                        self.forms.set_error_for(token, &error.code, &error.message);
                    }
                    if let Some(token) = params["_pane_token"].as_str() {
                        self.panes
                            .set_projection_error(method, token, &error.code, &error.message);
                    }
                }
            }
            Purpose::FileWrite { path, scope, .. } => {
                if error.code == "VERSION_CONFLICT"
                    && self.handle.scope().as_ref() == Some(scope)
                    && self.panes.file_path == *path
                {
                    self.panes.file_locked = true;
                }
            }
            Purpose::Close | Purpose::Shutdown | Purpose::Suspend => self.exit_after_close = false,
            _ => {}
        }
        if !matches!(error.code.as_str(), "CANCELLED" | "STALE_EPOCH") {
            self.error = Some(error);
            self.drawer.show(DrawerTab::Activity);
        }
    }
    fn choose(&mut self, create: bool) {
        self.choose_mode(create, false);
    }
    fn choose_mode(&mut self, create: bool, read_only: bool) {
        if self.handle.status().phase != ServicePhase::Closed {
            self.error = Some(Error::new(
                "BUSY",
                "현재 작업 공간을 저장하고 닫은 뒤 다른 공간을 여세요.",
            ));
            return;
        }
        if let Some(folder) = rfd::FileDialog::new()
            .set_title(if create {
                "새 작업 공간을 만들 상위 폴더"
            } else {
                "작업 공간 열기"
            })
            .pick_folder()
        {
            let path = if create {
                folder.join(format!(
                    "ARTEX-{}",
                    &uuid::Uuid::new_v4().simple().to_string()[..8]
                ))
            } else {
                folder
            };
            self.submit(
                NativeAction::Open(OpenOptions {
                    path: path.clone(),
                    mode: if create {
                        OpenMode::Create {
                            name: self.workspace_name.clone(),
                        }
                    } else {
                        OpenMode::Existing
                    },
                    read_only,
                }),
                Purpose::Open(path),
            );
        }
    }
    fn request_close(&mut self, exit: bool) {
        self.close_confirmation = true;
        self.exit_after_close = exit;
    }
    // Native OS close requests and the app's titlebar must share the same
    // background/tray and confirmed service shutdown behavior.
    fn native_window_close(&mut self, ctx: &egui::Context) {
        if self.allow_exit {
            return;
        }
        if close_to_background(self.background, self.tray.available(), self.smoke) {
            ctx.send_viewport_cmd(egui::ViewportCommand::Visible(false));
        } else {
            self.request_close(true);
        }
    }
    fn close(&mut self) {
        self.close_confirmation = false;
        if let Some(scope) = self.handle.scope() {
            self.submit(NativeAction::Close { scope }, Purpose::Close);
        } else {
            self.submit(NativeAction::Shutdown, Purpose::Shutdown);
        }
    }
    fn effects(&mut self, effects: Vec<Effect>) {
        for effect in effects {
            match effect {
                Effect::Command {
                    method,
                    params,
                    expected,
                } => {
                    if let Some(scope) = self.handle.scope() {
                        self.command(
                            method.clone(),
                            params.clone(),
                            expected,
                            Purpose::Command {
                                method,
                                params,
                                section: self.section,
                                selected: self.selected.get(&self.section).cloned().flatten(),
                                scope,
                            },
                        );
                    }
                }
                Effect::AgentMonitor { target, token } => {
                    self.native_operation(token, false, |scope| NativeAction::Query {
                        scope,
                        query: NativeQuery::AgentMonitor { target },
                    });
                }
                Effect::DeletionPreview {
                    target,
                    options,
                    token,
                } => {
                    self.native_operation(token, false, |scope| NativeAction::Query {
                        scope,
                        query: NativeQuery::DeletionPreview { target, options },
                    });
                }
                Effect::ConversationTrash { token } => {
                    self.native_operation(token, false, |scope| NativeAction::Query {
                        scope,
                        query: NativeQuery::ConversationTrash,
                    });
                }
                Effect::RunControl {
                    target,
                    action,
                    expected,
                    token,
                } => {
                    self.native_operation(token, true, |scope| NativeAction::RunControl {
                        scope,
                        command_id: artex_core::new_id("command"),
                        expected,
                        target,
                        action,
                    });
                }
                Effect::ConversationManage {
                    ids,
                    action,
                    expected,
                    token,
                } => {
                    self.native_operation(token, true, |scope| NativeAction::ConversationManage {
                        scope,
                        command_id: artex_core::new_id("command"),
                        expected,
                        ids,
                        action,
                    });
                }
                Effect::TaskDelete {
                    ids,
                    options,
                    expected,
                    token,
                } => {
                    self.native_operation(token, true, |scope| NativeAction::TaskDelete {
                        scope,
                        command_id: artex_core::new_id("command"),
                        expected,
                        ids,
                        options,
                    });
                }
                Effect::Query { method, params } => self.query(method, params),
                Effect::Select { section, id } => {
                    self.section = section;
                    self.navigation_reveal = true;
                    self.selected.insert(section, Some(id));
                    self.search.clear();
                    self.panes.projection.clear();
                }
                Effect::Download { name, bytes } => match String::from_utf8(bytes) {
                    Ok(text) => self.export_body(name, json!({"text":text,"blob":null})),
                    Err(_) => {
                        self.error = Some(Error::new(
                            "INVALID_PARAMS",
                            "바이너리 내보내기는 Blob 참조가 필요합니다.",
                        ))
                    }
                },
                Effect::ExportBase64Body { name, body } => {
                    if let Some(scope) = self.handle.scope() {
                        self.submit(
                            NativeAction::StageBase64Body {
                                scope: scope.clone(),
                                body,
                                name: name.clone(),
                            },
                            Purpose::ExportFile { name, scope },
                        );
                    }
                }
                Effect::StageFile { purpose } => {
                    if let Some(scope) = self.handle.scope() {
                        if let Some(path) = rfd::FileDialog::new()
                            .set_title("가져올 파일 선택")
                            .pick_file()
                        {
                            self.submit(
                                NativeAction::StageFile {
                                    scope: scope.clone(),
                                    path,
                                },
                                Purpose::StageFile { purpose, scope },
                            );
                        } else {
                            self.panes.set_staged_error(
                                &purpose,
                                "CANCELLED",
                                "파일 선택을 취소했습니다.",
                            );
                        }
                    }
                }
                Effect::SaveStagedFile { token, name } => {
                    if let Some(scope) = self.handle.scope() {
                        if let Some(destination) =
                            rfd::FileDialog::new().set_file_name(&name).save_file()
                        {
                            self.submit(
                                NativeAction::SaveFile {
                                    scope: scope.clone(),
                                    token: token.clone(),
                                    destination,
                                    command_id: artex_core::new_id("command"),
                                },
                                Purpose::SaveFile { token, scope },
                            );
                        } else {
                            self.submit(
                                NativeAction::ReleaseFile { scope, token },
                                Purpose::ReleaseFile,
                            );
                        }
                    }
                }
                Effect::ReleaseStagedFile { token } => {
                    if let Some(scope) = self.handle.scope() {
                        self.submit(
                            NativeAction::ReleaseFile { scope, token },
                            Purpose::ReleaseFile,
                        );
                    }
                }
                Effect::FileRead { path } => {
                    if let Some(scope) = self.handle.scope() {
                        self.submit(
                            NativeAction::FileRead {
                                scope: scope.clone(),
                                area: NativeFileArea::Work,
                                path: path.clone(),
                            },
                            Purpose::FileRead { path, scope },
                        );
                    }
                }
                Effect::FileWrite {
                    path,
                    content,
                    expected_sha256,
                    create,
                } => {
                    if let Some(scope) = self.handle.scope() {
                        if !self.pending.values().any(|p|matches!(p,Purpose::FileWrite{path:p,scope:s,..}if *p==path&&*s==scope)){let submitted=content.clone();self.submit(NativeAction::FileWrite{scope:scope.clone(),area:NativeFileArea::Work,path:path.clone(),content,expected_sha256,create,command_id:artex_core::new_id("command")},Purpose::FileWrite{path,scope,submitted});}
                    }
                }
            }
        }
    }
    fn native_operation(
        &mut self,
        token: String,
        mutation: bool,
        action: impl FnOnce(WorkspaceScope) -> NativeAction,
    ) {
        let Some(scope) = self.handle.scope() else {
            self.panes
                .set_native_error(&token, "WORKSPACE_NOT_OPEN", "작업 공간이 닫혀 있습니다.");
            return;
        };
        self.submit(
            action(scope.clone()),
            Purpose::NativeOperation {
                token,
                scope,
                mutation,
            },
        );
    }
    fn export_body(&mut self, name: String, body: Value) {
        if let Some(scope) = self.handle.scope() {
            self.submit(
                NativeAction::StageBody {
                    scope: scope.clone(),
                    body,
                    name: name.clone(),
                },
                Purpose::ExportFile { name, scope },
            );
        }
    }
    fn forms_effects(&mut self, submissions: Vec<crate::forms::FormSubmission>) {
        for submission in submissions {
            if submission.operation == "native.secret.set" {
                if let Some(scope) = self.handle.scope() {
                    let target = if let Some(s) = submission.params["target"].as_str() {
                        s.into()
                    } else {
                        submission.params["target"].to_string()
                    };
                    let field = submission.params["field"]
                        .as_str()
                        .unwrap_or("credential")
                        .into();
                    let value = submission.params["value"].as_str().unwrap_or("").into();
                    self.submit(
                        NativeAction::SetSecret {
                            scope: scope.clone(),
                            command_id: artex_core::new_id("command"),
                            field,
                            value,
                        },
                        Purpose::Secret { target, scope },
                    );
                }
            } else if submission.operation == "ui.testLLM" {
                if let Some(scope) = self.handle.scope() {
                    self.command(
                        submission.operation,
                        submission.params,
                        submission.expected,
                        Purpose::FormsModelTest {
                            token: self.forms.active_token().map(str::to_owned),
                            scope,
                        },
                    );
                }
            } else if submission.operation == "ui.writeSkillFile" {
                if let Some(scope) = self.handle.scope() {
                    let revision = submission.params["_file_revision"]
                        .as_str()
                        .map(str::to_owned);
                    let content = submission.params["content"]
                        .as_str()
                        .or_else(|| submission.params["content"]["text"].as_str())
                        .unwrap_or("")
                        .to_owned();
                    let path = format!(
                        "skills/{}/{}",
                        submission.params["name"].as_str().unwrap_or(""),
                        submission.params["file"].as_str().unwrap_or("")
                    );
                    self.submit(
                        NativeAction::FileWrite {
                            scope: scope.clone(),
                            area: NativeFileArea::Resources,
                            path,
                            content: content.clone(),
                            expected_sha256: revision,
                            create: submission.params["_file_create"].as_bool().unwrap_or(false),
                            command_id: artex_core::new_id("command"),
                        },
                        Purpose::SkillFileSave {
                            token: self.forms.active_token().unwrap_or("").into(),
                            file_token: submission.params["_file_token"].as_u64().unwrap_or(0),
                            scope,
                            submitted: content,
                        },
                    );
                }
            } else if submission.operation == "native.skill.meta.write" {
                if let Some(scope) = self.handle.scope() {
                    self.submit(
                        NativeAction::SkillMetaWrite {
                            scope: scope.clone(),
                            name: submission.params["name"].as_str().unwrap_or("").into(),
                            meta: submission.params["meta"].clone(),
                            expected_sha256: submission.params["expected_sha256"]
                                .as_str()
                                .unwrap_or("")
                                .into(),
                            command_id: artex_core::new_id("command"),
                        },
                        Purpose::FormsSave {
                            token: self.forms.active_token().map(str::to_owned),
                            scope,
                        },
                    );
                }
            } else {
                if let Some(scope) = self.handle.scope() {
                    self.command(
                        submission.operation,
                        submission.params,
                        submission.expected,
                        Purpose::FormsSave {
                            token: self.forms.active_token().map(str::to_owned),
                            scope,
                        },
                    );
                }
            }
        }
    }
    fn source_effects(&mut self, submissions: Vec<OperationSubmission>) {
        for submission in submissions {
            let OperationSubmission {
                method,
                mut params,
                expected,
                pinned,
                scope,
                mutation,
                token,
            } = submission;
            if !self.source_operations.owns_token(&token)
                || self.handle.scope().as_ref() != Some(&scope)
            {
                continue;
            }
            if method == "native.secret.set" {
                let target = params["target"]
                    .as_str()
                    .map(str::to_owned)
                    .unwrap_or_else(|| params["target"].to_string());
                self.submit(
                    NativeAction::SetSecret {
                        scope: scope.clone(),
                        command_id: artex_core::new_id("command"),
                        field: params["field"].as_str().unwrap_or("credential").into(),
                        value: params["value"].as_str().unwrap_or("").into(),
                    },
                    Purpose::SourceSecret {
                        scope,
                        token,
                        target,
                    },
                );
            } else if method == "native.body.read" {
                let path = params["_form_path"].as_array().and_then(|a| {
                    a.iter()
                        .map(|v| v.as_str().map(str::to_owned))
                        .collect::<Option<Vec<_>>>()
                });
                let Some(path) = path else {
                    self.failed(
                        &Purpose::SourceBody {
                            scope,
                            token,
                            path: vec![],
                        },
                        Error::new("INVALID_PARAMS", "본문 필드 경로가 올바르지 않습니다."),
                    );
                    continue;
                };
                self.submit(
                    NativeAction::Query {
                        scope: scope.clone(),
                        query: NativeQuery::Body {
                            body: params
                                .get("body")
                                .cloned()
                                .unwrap_or_else(|| json!({"text":null,"blob":params["reference"]})),
                        },
                    },
                    Purpose::SourceBody { scope, token, path },
                );
            } else {
                if let Some(object) = params.as_object_mut() {
                    for key in ["_form_token", "_form_path"] {
                        object.remove(key);
                    }
                }
                let purpose = Purpose::SourceOperation {
                    method: method.clone(),
                    token,
                    scope: scope.clone(),
                };
                if mutation {
                    self.submit(
                        NativeAction::Query {
                            scope: scope.clone(),
                            query: NativeQuery::Prepare {
                                method: method.clone(),
                                params: params.clone(),
                            },
                        },
                        Purpose::Prepare {
                            method,
                            params,
                            expected,
                            pinned,
                            scope,
                            then: Box::new(purpose),
                        },
                    );
                } else {
                    self.submit(
                        NativeAction::Query {
                            scope,
                            query: NativeQuery::Feature { method, params },
                        },
                        purpose,
                    );
                }
            }
        }
        for export in self.source_operations.drain_exports() {
            if self.handle.scope().as_ref() != Some(&export.scope) {
                continue;
            }
            let purpose = Purpose::ExportFile {
                name: export.name.clone(),
                scope: export.scope.clone(),
            };
            let action = if export.base64 {
                NativeAction::StageBase64Body {
                    scope: export.scope,
                    body: export.body,
                    name: export.name,
                }
            } else {
                NativeAction::StageBody {
                    scope: export.scope,
                    body: export.body,
                    name: export.name,
                }
            };
            self.submit(action, purpose);
        }
    }
    fn workspace_screen(&mut self, ui: &mut egui::Ui) {
        ui.vertical_centered(|ui| {
            ui.add_space(80.0);
            ui.label(
                RichText::new("ARTEX")
                    .size(42.0)
                    .color(crate::theme::accent(ui)),
            );
            ui.heading("작업 공간 열기");
            ui.label("대화·작업·자산·증거를 하나의 공간에서 관리합니다.");
            if self.suspended {
                ui.colored_label(
                    ui.visuals().warn_fg_color,
                    "절전으로 실행을 중지했습니다. 불명확한 실행을 자동 반복하지 않습니다.",
                );
                if let Some(path) = self.last_workspace.clone() {
                    if ui
                        .add_enabled(
                            self.handle.status().phase == ServicePhase::Closed,
                            egui::Button::new("저장된 작업 공간 다시 열기"),
                        )
                        .clicked()
                    {
                        self.submit(
                            NativeAction::Open(OpenOptions {
                                path: path.clone(),
                                mode: OpenMode::Existing,
                                read_only: self.last_read_only,
                            }),
                            Purpose::Open(path),
                        );
                    }
                }
            }
            ui.add_space(20.0);
            ui.horizontal(|ui| {
                ui.label("새 공간 이름");
                ui.text_edit_singleline(&mut self.workspace_name);
            });
            ui.horizontal(|ui| {
                if ui.button("작업 공간 열기 · Ctrl+O").clicked() {
                    self.choose(false);
                }
                if ui.button("새 작업 공간").clicked() {
                    self.choose(true);
                }
            });
            ui.add_space(18.0);
            for path in self.recent.clone() {
                if ui.button(path.to_string_lossy()).clicked() {
                    self.submit(
                        NativeAction::Open(OpenOptions {
                            path: path.clone(),
                            mode: OpenMode::Existing,
                            read_only: false,
                        }),
                        Purpose::Open(path),
                    );
                }
            }
        });
    }
    pub fn render(&mut self, ctx: &egui::Context) {
        self.poll(ctx);
        if !self.power_warning_recorded && !self.power.available() {
            if let Some(reason) = self.power.registration_error() {
                self.notices.push_back(format!(
                    "OS 절전 알림 등록 상태: {reason}. 시간 차이 감지로 보완합니다."
                ));
                self.power_warning_recorded = true;
            }
        }
        if ctx.input(|i| i.viewport().close_requested()) && !self.allow_exit {
            ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose);
            self.native_window_close(ctx);
        }
        if ctx.input(|i| i.modifiers.command && i.key_pressed(egui::Key::O)) {
            self.choose(false);
        }
        if ctx.input(|i| i.modifiers.command && i.key_pressed(egui::Key::Q)) {
            self.request_close(true);
        }
        if ctx.input(|i| i.modifiers.command && i.key_pressed(egui::Key::J)) {
            self.drawer.toggle(DrawerTab::Activity);
        }
        if ctx.input(|i| i.key_pressed(egui::Key::F1)) {
            self.about_open = true;
        }
        if ctx.input(|i| i.modifiers.command && i.key_pressed(egui::Key::S)) {
            if let Some(scope) = self.handle.scope() {
                self.submit(
                    NativeAction::Command {
                        scope,
                        command: NativeCommand::checkpoint(),
                    },
                    Purpose::Checkpoint,
                );
            }
        }
        self.panes.set_context(
            self.section,
            self.selected.get(&self.section).and_then(Option::as_deref),
        );
        let mut source_submissions = self.draw_chrome(ctx);
        let Some(view) = self.handle.view() else {
            egui::CentralPanel::default().show(ctx, |ui| self.workspace_screen(ui));
            self.close_dialog(ctx);
            crate::window_chrome::resize_edges(ctx, crate::window_chrome::Platform::current());
            return;
        };
        let snapshot = &view.generation.snapshot;
        self.forms.set_read_only(view.workspace.read_only);
        self.panes.set_read_only(view.workspace.read_only);
        self.command_read = Some(view.clone());
        let mut effects = Vec::new();
        self.draw_navigation(ctx);
        self.draw_object_list(ctx, snapshot, view.workspace.read_only);
        let selected = self.selected.get(&self.section).cloned().flatten();
        let item = selected_record(snapshot, self.section, selected.as_deref());
        let inspector_item = item.or_else(|| {
            (self.section == Section::Chat)
                .then(|| selected.as_deref().and_then(|id| snapshot.get("tasks", id)))
                .flatten()
        });
        self.draw_inspector(ctx, snapshot, inspector_item);
        self.panes.set_context(self.section, selected.as_deref());
        let mut submissions = Vec::new();
        egui::CentralPanel::default()
            .frame(
                egui::Frame::central_panel(&ctx.style())
                    .fill(crate::theme::palette(ctx.theme() == egui::Theme::Dark).window)
                    .inner_margin(20),
            )
            .show(ctx, |ui| {
                // Conversation already has its own title and the workspace
                // toolbar always shows read-only mode. Keep this area for
                // messages and the composer, including in the minimum window.
                if self.section != Section::Chat {
                    ui.horizontal(|ui| {
                        if self.section != Section::Chat {
                            ui.strong(self.section.label());
                            if let Some(item) = item {
                                ui.separator();
                                ui.add(
                                    egui::Label::new(workbench::presentation_name(
                                        snapshot,
                                        self.section,
                                        item,
                                    ))
                                    .truncate(),
                                )
                                .on_hover_text(
                                    workbench::presentation_name(snapshot, self.section, item),
                                );
                            }
                        }
                        if view.workspace.read_only {
                            ui.with_layout(
                                egui::Layout::right_to_left(egui::Align::Center),
                                |ui| {
                                    ui.small("읽기 전용 · 조회와 내보내기 가능");
                                },
                            );
                        }
                    });
                    ui.add_space(16.0);
                }
                if self.section == Section::Graph {
                    let selection = self.selected.entry(Section::Graph).or_default();
                    self.graph.show(ui, snapshot, selection);
                    return;
                }
                // A short window with both bottom panels expanded falls back to
                // whole-pane scrolling so the composer remains reachable.
                let chat_pane = self.section == Section::Chat && ui.available_height() >= 420.0;
                let mut render_pane = |ui: &mut egui::Ui| {
                    if self.section.configuration() {
                        if self.section == Section::Skills {
                            self.panes
                                .skill_import(ui, selected.as_deref(), &mut effects);
                        }
                        let collection = self.section.collection().unwrap_or("skills");
                        submissions =
                            self.forms
                                .show(ui, collection, snapshot, selected.as_deref());
                    } else {
                        match self.section {
                            Section::Overview => self.panes.overview(ui, snapshot, &mut effects),
                            Section::Tasks => self.panes.tasks(ui, snapshot, item, &mut effects),
                            Section::Chat => {
                                let task = selected
                                    .as_deref()
                                    .filter(|id| snapshot.get("tasks", id).is_some());
                                self.panes.chat(ui, snapshot, item, task, &mut effects);
                            }
                            Section::Assets => self.panes.assets(ui, snapshot, item, &mut effects),
                            Section::Findings => {
                                self.panes.finding(ui, snapshot, item, &mut effects)
                            }
                            Section::Graph => {
                                let selection = self.selected.entry(Section::Graph).or_default();
                                self.graph.show(ui, snapshot, selection);
                            }
                            Section::Files => self.panes.files(ui, &mut effects),
                            Section::Approvals => self.panes.approvals(ui, item, &mut effects),
                            Section::Archives => {
                                self.panes.archive(ui, snapshot, item, &mut effects)
                            }
                            Section::Traffic => {
                                self.panes.traffic(ui, snapshot, item, &mut effects)
                            }
                            Section::Sessions => {
                                self.panes.sessions(ui, snapshot, item, &mut effects)
                            }
                            Section::Sync => self.panes.sync(ui, snapshot, item, &mut effects),
                            Section::Logs => self.panes.logs(ui, snapshot, item, &mut effects),
                            _ => self.panes.generic(ui, self.section, item),
                        }
                    }
                    if self.section != Section::Chat {
                        ui.separator();
                        source_submissions.extend(self.source_operations.show(
                            ui,
                            &view,
                            self.section,
                            selected.as_deref(),
                        ));
                    }
                };
                if chat_pane {
                    render_pane(ui);
                } else {
                    egui::ScrollArea::vertical()
                        .id_salt("active_pane")
                        .auto_shrink([false, false])
                        .show(ui, |ui| {
                            ui.set_width(ui.available_width());
                            render_pane(ui);
                        });
                }
            });
        effects.extend(self.panes.show_operation_dialog(ctx, snapshot));
        effects.extend(self.panes.drain_effects());
        self.forms_effects(submissions);
        self.source_effects(source_submissions);
        self.effects(effects);
        for (method, params) in self.forms.pending_projection_requests() {
            self.query(method, params);
        }
        if self.section == Section::Chat && self.last_query.elapsed() > Duration::from_secs(1) {
            if let Some(id) = selected.filter(|id| snapshot.get("conversations", id).is_some()) {
                self.query(
                    "ui.conversationHistory".into(),
                    json!({"id":id,"limit":100}),
                );
            }
            self.last_query = Instant::now();
        }
        if self.section == Section::Skills && self.last_query.elapsed() > Duration::from_secs(1) {
            self.query("ui.skills".into(), json!({}));
            self.last_query = Instant::now();
        }
        self.close_dialog(ctx);
        crate::window_chrome::resize_edges(ctx, crate::window_chrome::Platform::current());
        ctx.request_repaint_after(Duration::from_millis(200));
    }
    fn close_dialog(&mut self, ctx: &egui::Context) {
        if let Some(options) = self.incoming_workspace.clone() {
            egui::Window::new("작업 공간 열기 요청")
                .collapsible(false)
                .resizable(false)
                .anchor(egui::Align2::CENTER_CENTER, Vec2::ZERO)
                .show(ctx, |ui| {
                    ui.label(options.path.to_string_lossy());
                    if options.read_only {
                        ui.label("요청 모드: 읽기 전용");
                    }
                    ui.label(
                        "현재 공간을 저장하고 실행을 정리한 뒤 전환합니다. 미저장 초안은 버립니다.",
                    );
                    ui.horizontal(|ui| {
                        if ui
                            .add_enabled(
                                matches!(
                                    self.handle.status().phase,
                                    ServicePhase::Ready | ServicePhase::Closed
                                ),
                                egui::Button::new("저장 후 전환"),
                            )
                            .clicked()
                        {
                            self.incoming_workspace = None;
                            self.exit_after_close = false;
                            if self.handle.status().phase == ServicePhase::Closed {
                                let path = options.path.clone();
                                self.submit(
                                    NativeAction::Open(options.clone()),
                                    Purpose::Open(path),
                                );
                            } else {
                                self.next_workspace = Some(options.clone());
                                self.close();
                            }
                        }
                        if ui.button("현재 공간 유지").clicked() {
                            self.incoming_workspace = None;
                        }
                    });
                });
        }
        if self.close_confirmation {
            egui::Window::new("작업 공간 저장·종료")
                .collapsible(false)
                .resizable(false)
                .anchor(egui::Align2::CENTER_CENTER, Vec2::ZERO)
                .show(ctx, |ui| {
                    ui.label("실행을 정리하고 변경 사항과 체크포인트를 저장합니다.");
                    ui.label("저장하지 않은 편집 내용은 버립니다.");
                    ui.horizontal(|ui| {
                        if ui.button("저장 후 닫기").clicked() {
                            self.close();
                        }
                        if ui.button("계속 작업").clicked() {
                            self.close_confirmation = false;
                            self.exit_after_close = false;
                        }
                    });
                });
        }
    }
}
impl NativeApp {
    fn smoke_tick(&mut self, ctx: &egui::Context) {
        // Check before every early-return path (including About settling and
        // external verifier hold), so diagnostics can never stall indefinitely.
        if smoke_deadline_expired(self.smoke_started) {
            self.error = Some(Error::new("TIMEOUT", "Native window smoke deadline"));
            self.write_smoke_report(false);
            self.exit_after_close = true;
            self.close();
            return;
        }
        if self.handle.status().phase == ServicePhase::Ready && !self.smoke_final_requested {
            if self.smoke_sections.len() < Section::ALL.len() {
                let section = Section::ALL[self.smoke_sections.len()];
                self.section = section;
                self.smoke_sections.push(section);
                ctx.request_repaint();
            } else {
                self.section = self.smoke_target;
                self.about_open = self.smoke_about;
                if self.smoke_about {
                    let opened = self.smoke_about_started.get_or_insert_with(Instant::now);
                    let settle =
                        Duration::from_secs_f32(ctx.style().animation_time.max(0.25) + 0.1);
                    if opened.elapsed() < settle {
                        ctx.request_repaint_after(Duration::from_millis(16));
                        return;
                    }
                    self.smoke_about_settled = true;
                }
                self.navigation_reveal = true;
                if let Some(tab) = self.smoke_dock {
                    self.drawer.show(tab);
                }
                if let Some(view) = self.handle.view() {
                    if self.section != Section::Settings
                        && let Some(record) =
                            records(&view.generation.snapshot, self.section, "").first()
                    {
                        self.selected
                            .insert(self.section, Some(record.header.id.clone()));
                    }
                    if self.smoke_trash {
                        self.panes.open_smoke_trash();
                    }
                    if self.smoke_dialog {
                        self.panes.open_smoke_deletion_preview(
                            &view.generation.snapshot,
                            self.section,
                            self.selected.get(&self.section).and_then(Option::as_deref),
                        );
                    }
                }
                self.smoke_final_requested = true;
                ctx.request_repaint();
            }
        } else if self.smoke_final_requested
            && self.handle.status().phase == ServicePhase::Ready
            && self.pending.is_empty()
        {
            if !smoke_release_ready(
                self.smoke_hold_until.as_deref(),
                &mut self.smoke_hold_released,
            ) {
                ctx.request_repaint_after(Duration::from_millis(16));
            } else if self.screenshot.is_some() {
                // Retain the rendered frame's evidence: orderly close retires
                // pane observations before writing the final shutdown report.
                self.smoke_management = Some(self.panes.native_management_diagnostic());
                ctx.send_viewport_cmd(egui::ViewportCommand::Screenshot(egui::UserData::default()));
                self.smoke_final_requested = false;
                self.smoke = false;
            } else {
                self.write_smoke_report(false);
                self.exit_after_close = true;
                self.close();
            }
        }
    }
    fn screenshot_events(&mut self, ctx: &egui::Context) {
        let captures = ctx.input(|i| {
            i.events
                .iter()
                .filter_map(|e| {
                    if let egui::Event::Screenshot { image, .. } = e {
                        Some(image.clone())
                    } else {
                        None
                    }
                })
                .collect::<Vec<_>>()
        });
        for capture in captures {
            if let Some(path) = self.screenshot.take() {
                let bytes: Vec<u8> = capture.pixels.iter().flat_map(|p| p.to_array()).collect();
                if let Some(parent) = path.parent() {
                    let _ = std::fs::create_dir_all(parent);
                }
                let result = image::save_buffer(
                    &path,
                    &bytes,
                    capture.size[0] as u32,
                    capture.size[1] as u32,
                    image::ColorType::Rgba8,
                );
                self.smoke_capture = result.is_ok();
                if let Err(error) = result {
                    self.error = Some(Error::new("IO_FAILURE", &error.to_string()));
                }
                self.write_smoke_report(self.smoke_capture);
                self.exit_after_close = true;
                self.close();
            }
        }
    }
    fn write_smoke_report(&self, captured: bool) {
        let root = self.profile.join("native-window-smoke.json");
        let actions = crate::forms::operations::catalog().iter().map(|action| {
            json!({"key":action.key,"method":action.method,"section":action.section.label(),"mutation":action.mutation})
        }).collect::<Vec<_>>();
        let mut value = json!({"pid":std::process::id(),"rendered_sections":self.smoke_sections.iter().map(|s|s.label()).collect::<Vec<_>>(),"source_management_actions":actions,"rust_ui":true,"single_application_process_design":true,"webview":false,"engine_sidecar":false,"screenshot_received":captured,"orderly_shutdown":self.allow_exit&&self.service.is_none(),"error":self.error.as_ref().map(|e|json!({"code":e.code,"message":e.message})),"scope":self.handle.scope().map(|s|s.workspace_id),"received_launch_requests":self.smoke_launch_requests});
        value["smoke_hold_handshake"] = json!({"requested":self.smoke_hold_until.is_some(),"released":self.smoke_hold_released});
        value["about_dialog"] = json!({"open":self.about_open,"requested_in_smoke":self.smoke_about,"settled_before_capture":self.smoke_about_settled});
        value["drawer"] = json!({"open":self.drawer.open,"tab":match self.drawer.tab { DrawerTab::Activity=>"activity",DrawerTab::Management=>"management" },"logical_rect":self.drawer.last_rect.map(|rect|json!({"x":rect.left(),"y":rect.top(),"width":rect.width(),"height":rect.height()}))});
        value["drawer"]["body_rect"] = json!(self.drawer.body_rect.map(|rect|json!({"x":rect.left(),"y":rect.top(),"width":rect.width(),"height":rect.height()})));
        value["drawer"]["visible_body_height"] =
            json!(self.drawer.body_rect.map(|rect| rect.height()));
        let platform = crate::window_chrome::Platform::current();
        let viewport = crate::window_chrome::viewport(platform, false);
        value["window_chrome"] = json!({
            "integrated_titlebar": true,
            "platform": format!("{platform:?}"),
            "requested_native_decorations": viewport.decorations,
            "macos_native_resize_retained": platform == crate::window_chrome::Platform::Mac,
            "close_uses_application_lifecycle": true,
        });
        value["native_management"] = self
            .smoke_management
            .clone()
            .unwrap_or_else(|| self.panes.native_management_diagnostic());
        value["native_management_evidence"] = json!(if self.smoke_management.is_some() {
            "screenshot_requested_frame"
        } else {
            "current_state"
        });
        let _ = std::fs::write(root, serde_json::to_vec_pretty(&value).unwrap_or_default());
    }
}
impl eframe::App for NativeApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        self.render(ctx);
        if self.smoke {
            self.smoke_tick(ctx);
        }
        self.screenshot_events(ctx);
    }
    fn on_exit(&mut self, _gl: Option<&eframe::glow::Context>) {
        if let Some(service) = self.service.take() {
            let _ = self.handle.try_submit(NativeAction::Shutdown);
            let deadline = Instant::now() + Duration::from_secs(5);
            while self.handle.status().phase != ServicePhase::Shutdown && Instant::now() < deadline
            {
                self.handle.drain_completions();
                std::thread::sleep(Duration::from_millis(10));
            }
            if self.handle.status().phase == ServicePhase::Shutdown {
                let _ = service.join();
            }
        }
    }
}
fn purpose_label(purpose: &Purpose) -> &str {
    match purpose {
        Purpose::Prepare { .. } => "참조 버전 검사",
        Purpose::Open(_) => "작업 공간 열기",
        Purpose::Command { method, .. } => method,
        Purpose::Projection { method, .. } => method,
        Purpose::NativeOperation { mutation, .. } => {
            if *mutation {
                "실행 제어·기록 관리"
            } else {
                "실행 현황·삭제 영향 조회"
            }
        }
        Purpose::FormsSave { .. } | Purpose::SkillFileSave { .. } => "설정 저장",
        Purpose::FormsModelTest { .. } => "저장된 모델 연결 시험",
        Purpose::Secret { .. } => "자격 증명 저장",
        Purpose::SourceOperation { method, .. } => method,
        Purpose::SourceBody { .. } => "추가 관리 본문 조회",
        Purpose::SourceSecret { .. } => "추가 관리 자격 증명 저장",
        Purpose::Checkpoint => "체크포인트",
        Purpose::BypassToggle { .. } => "작업 공간 Bypass 변경",
        Purpose::Close => "작업 공간 정리",
        Purpose::Suspend => "절전·저장",
        Purpose::Shutdown => "앱 종료",
        Purpose::FileRead { .. } => "파일 읽기",
        Purpose::FileWrite { .. } => "파일 저장",
        Purpose::StageFile { .. } => "파일 가져오기 준비",
        Purpose::SaveFile { .. } => "파일 내보내기",
        Purpose::ExportFile { .. } => "내보내기 자료 준비",
        Purpose::ReleaseFile => "파일 토큰 해제",
    }
}

fn merge_pinned_expected(
    mut draft: Vec<Expected>,
    required: Vec<Expected>,
    pinned: &artex_core::Snapshot,
) -> Result<Vec<Expected>> {
    for reference in required {
        if draft
            .iter()
            .any(|old| old.collection == reference.collection && old.id == reference.id)
        {
            continue;
        }
        let row = pinned
            .get(&reference.collection, &reference.id)
            .ok_or_else(|| {
                Error::new(
                    "VERSION_CONFLICT",
                    "참조 객체를 표시할 때 읽은 버전이 없습니다. 내용을 다시 불러오세요.",
                )
            })?;
        draft.push(Expected {
            collection: reference.collection,
            id: reference.id,
            version: row.header.version.clone(),
        });
    }
    Ok(draft)
}
fn smoke_deadline_expired(started: Instant) -> bool {
    started.elapsed() > Duration::from_secs(90)
}
fn smoke_release_ready(path: Option<&std::path::Path>, released: &mut bool) -> bool {
    let Some(path) = path else {
        return true;
    };
    if *released {
        return true;
    }
    let Ok(metadata) = std::fs::symlink_metadata(path) else {
        return false;
    };
    #[cfg(windows)]
    let reparse = {
        use std::os::windows::fs::MetadataExt;
        metadata.file_attributes() & 0x400 != 0
    };
    #[cfg(not(windows))]
    let reparse = false;
    if metadata.is_file() && !metadata.file_type().is_symlink() && !reparse {
        *released = true;
    }
    *released
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn smoke_hold_waits_for_regular_release_and_remembers_proof_through_shutdown() {
        let dir = tempfile::tempdir().unwrap();
        let release = dir.path().join("release");
        let mut confirmed = false;
        assert!(!smoke_release_ready(Some(&release), &mut confirmed));
        std::fs::create_dir(&release).unwrap();
        assert!(!smoke_release_ready(Some(&release), &mut confirmed));
        std::fs::remove_dir(&release).unwrap();
        std::fs::write(&release, b"release").unwrap();
        assert!(smoke_release_ready(Some(&release), &mut confirmed));
        std::fs::remove_file(&release).unwrap();
        assert!(smoke_release_ready(Some(&release), &mut confirmed));
        assert!(confirmed);
        let mut generic = false;
        assert!(smoke_release_ready(None, &mut generic));
        assert!(!generic);
        assert!(
            smoke_deadline_expired(Instant::now() - Duration::from_secs(91)),
            "hold must not skip the bounded smoke timeout"
        );
    }
    #[test]
    fn reference_preflight_never_rebases_an_existing_draft_or_a_displayed_reference() {
        let row:artex_core::Record=serde_json::from_value(json!({"header":{"id":"company_00000000000000000000000000000001","collection":"companies","version":"4","commit_seq":"4","created_at":"2026-10-13T00:00:00Z","updated_at":"2026-10-13T00:00:00Z","legacy_key":null},"data":{}})).unwrap();
        let company = Expected {
            collection: "companies".into(),
            id: row.header.id.clone(),
            version: "9".into(),
        };
        let task = Expected {
            collection: "tasks".into(),
            id: "task_00000000000000000000000000000001".into(),
            version: "2".into(),
        };
        let snapshot = artex_core::Snapshot {
            workspace_id: "workspace_00000000000000000000000000000001".into(),
            commit_seq: 4,
            collections: BTreeMap::from([(
                "companies".into(),
                BTreeMap::from([(row.header.id.clone(), row)]),
            )]),
        };
        let merged = merge_pinned_expected(
            vec![task.clone()],
            vec![
                Expected {
                    version: "7".into(),
                    ..task.clone()
                },
                company.clone(),
            ],
            &snapshot,
        )
        .unwrap();
        assert_eq!(merged[0].version, "2");
        assert_eq!(merged[1].version, "4");
        let unknown = Expected {
            collection: "assets".into(),
            id: "asset_00000000000000000000000000000001".into(),
            version: "1".into(),
        };
        assert_eq!(
            merge_pinned_expected(vec![], vec![unknown], &snapshot)
                .unwrap_err()
                .code,
            "VERSION_CONFLICT"
        );
    }
    #[test]
    fn attachment_identity_is_checked_against_its_selected_context() {
        let full = artex_core::contracts::read("rpc.v1.schema.json");
        let schema = full["methods"]["ui.chatUpload"].clone();
        let task = "task_00000000000000000000000000000001";
        let conversation = "conversation_00000000000000000000000000000001";
        assert!(
            artex_core::validate_contract(
                &schema,
                &full,
                &json!({"scope":"session","id":conversation,"files":[]})
            )
            .is_ok()
        );
        assert!(
            artex_core::validate_contract(
                &schema,
                &full,
                &json!({"scope":"task","id":task,"files":[]})
            )
            .is_ok()
        );
        assert!(
            artex_core::validate_contract(
                &schema,
                &full,
                &json!({"scope":"session","id":task,"files":[]})
            )
            .is_err()
        );
        assert!(
            artex_core::validate_contract(
                &schema,
                &full,
                &json!({"scope":"task","id":conversation,"files":[]})
            )
            .is_err()
        );
    }
}
fn close_to_background(background: bool, tray_available: bool, smoke: bool) -> bool {
    background && tray_available && !smoke
}

pub fn configure(ctx: &egui::Context) {
    ctx.set_theme(egui::Theme::Dark);
    let mut fonts = egui::FontDefinitions::default();
    fonts.font_data.insert(
        "korean".into(),
        egui::FontData::from_static(include_bytes!("../assets/fonts/NotoSansKR-Regular.ttf"))
            .into(),
    );
    fonts
        .families
        .entry(egui::FontFamily::Proportional)
        .or_default()
        .insert(0, "korean".into());
    fonts
        .families
        .entry(egui::FontFamily::Monospace)
        .or_default()
        .push("korean".into());
    ctx.set_fonts(fonts);
    crate::theme::apply(ctx);
}
