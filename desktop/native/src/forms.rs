//! Native settings editors. A draft owns its read-time revision vector; an
//! unrelated snapshot refresh cannot rebase an unsaved form.
pub mod operations;
mod schema;
mod spec;
mod widgets;

use artex_core::{Expected, Snapshot};
use eframe::egui::{self, Ui};
use serde_json::{Value, json};
use std::collections::{BTreeMap, BTreeSet};

#[derive(Clone, Debug)]
pub struct FormSubmission {
    pub operation: String,
    pub params: Value,
    pub expected: Vec<Expected>,
}

struct Editor {
    token: String,
    collection: String,
    selected: Option<String>,
    operation: String,
    params: Value,
    original: Value,
    source: Value,
    expected: Vec<Expected>,
    references: Vec<schema::Reference>,
    extras: Value,
    secrets: BTreeMap<String, String>,
    pending_secrets: BTreeSet<String>,
    pending_reads: BTreeSet<String>,
    requested: BTreeSet<String>,
    projections: BTreeMap<String, Value>,
    busy: bool,
    model_test_pending: bool,
    conflict: bool,
    error: Option<String>,
    delete_confirm: bool,
    file: String,
    file_token: u64,
    file_loading: bool,
    file_content: String,
    file_original: String,
    file_revision: Option<Value>,
    metadata_revision: Option<String>,
    file_target: Option<String>,
    file_create: bool,
    new_file: String,
}

impl Editor {
    fn new(token: String, collection: &str, selected: Option<&str>, s: &Snapshot) -> Option<Self> {
        let (operation, params) = spec::initial(collection, selected, s)?;
        let source = selected
            .and_then(|id| s.get(collection, id))
            .map(|r| r.data.clone())
            .unwrap_or(json!({}));
        let mut extras = json!({});
        if collection == "agents" && selected.is_some() {
            let prompt = source["current_prompt_id"]
                .as_str()
                .and_then(|id| s.get("agent_prompts", id))
                .map(|r| r.data["template_text"].clone())
                .unwrap_or(json!({"text":"","blob":null}));
            let visibility = spec::initial("agent_visibility", selected, s)
                .map(|v| v.1)
                .unwrap_or(json!({}));
            extras = json!({"prompt":prompt,"note":"","wrapup":source["wrapup_prompt"],"wrapup_turns":source["wrapup_max_turns"],"timeout_wrapup":source["task_timeout_wrapup_prompt"],"timeout_turns":source["task_timeout_wrapup_max_turns"],"visibility":visibility,"tools":s.records("tools").iter().map(|r|json!({"id":r.header.id,"key":r.data["key"],"description":r.data["description"],"enabled":r.data["enabled"],"schema":r.data["schema"],"agents":r.data["agents"]})).collect::<Vec<_>>(),"history":s.records("agent_prompts").iter().filter(|r|Some(r.data["agent_id"].as_str().unwrap_or(""))==selected).map(|r|r.data.clone()).collect::<Vec<_>>()});
        }
        if collection == "agents" && selected.is_some() {
            extras["metadata"] = json!({"key":source["key"],"name":source["name"],"description":source["description"]});
            extras["triggers"]=json!(s.records("agent_triggers").into_iter().filter(|r|r.data["agent_id"]==selected.unwrap()).map(|r|json!({"id":r.header.id,"t":schema::shape(&r.data,&schema::resolve(&schema::method("ui.updateTrigger")["properties"]["t"]))})).collect::<Vec<_>>());
        }
        Some(Self {
            token,
            collection: collection.into(),
            selected: selected.map(str::to_owned),
            operation,
            original: params.clone(),
            params,
            source,
            expected: spec::baseline(collection, selected, s),
            references: schema::references(s),
            extras,
            secrets: BTreeMap::new(),
            pending_secrets: BTreeSet::new(),
            pending_reads: BTreeSet::new(),
            requested: BTreeSet::new(),
            projections: BTreeMap::new(),
            busy: false,
            model_test_pending: false,
            conflict: false,
            error: None,
            delete_confirm: false,
            file: String::new(),
            file_token: 0,
            file_loading: false,
            file_content: String::new(),
            file_original: String::new(),
            file_revision: None,
            metadata_revision: None,
            file_target: None,
            file_create: false,
            new_file: String::new(),
        })
    }
    fn refresh_conflict(&mut self, s: &Snapshot) {
        if self.expected.iter().any(|e| {
            s.get(&e.collection, &e.id)
                .is_none_or(|r| r.header.version != e.version)
        }) {
            self.conflict = true;
        }
    }
    fn queue(&mut self, method: &str, mut p: Value, requests: &mut Vec<(String, Value)>) {
        if self.requested.insert(method.into()) {
            p["_form_token"] = json!(self.token);
            requests.push((method.into(), p));
        }
    }
    fn submit(
        &mut self,
        operation: &str,
        params: Value,
        allowed: &BTreeSet<String>,
        out: &mut Vec<FormSubmission>,
    ) {
        if !allowed.contains(operation)
            || self.conflict
            || self.busy
            || !self.pending_secrets.is_empty()
            || !self.pending_reads.is_empty()
        {
            return;
        }
        if operation == "ui.updateSkillMeta" {
            let Some(sha) = self.metadata_revision.clone() else {
                self.error = Some("메타데이터 파일을 다시 불러와 버전을 확인하세요.".into());
                return;
            };
            let shaped = schema::shape(&params, &schema::method(operation));
            if let Err(e) = artex_core::validate_contract(
                &schema::method(operation),
                &schema::rpc()["$defs"],
                &shaped,
            ) {
                self.error = Some(e.message);
                return;
            }
            self.busy = true;
            out.push(FormSubmission {
                operation: "native.skill.meta.write".into(),
                params: json!({"name":shaped["name"],"meta":shaped["meta"],"expected_sha256":sha}),
                expected: self.expected.clone(),
            });
            return;
        }
        let shaped = schema::shape(&params, &schema::method(operation));
        if let Err(e) = validate_form(operation, &shaped) {
            self.error = Some(format!("입력 확인: {}", e.message));
            return;
        }
        if let Err(e) = artex_core::validate_secret_parameters(operation, &shaped) {
            self.error = Some(format!("비밀 값 확인: {}", e.message));
            return;
        }
        self.busy = true;
        self.error = None;
        self.model_test_pending = operation == "ui.testLLM";
        out.push(FormSubmission {
            operation: operation.into(),
            params: shaped,
            expected: self.expected.clone(),
        });
    }
    fn can_activate_saved_profile(&self, s: &Snapshot, allowed: &BTreeSet<String>) -> bool {
        self.collection == "llm_profiles"
            && allowed.contains("ui.activateLLMProfile")
            && !self.busy
            && self.pending_secrets.is_empty()
            && self.pending_reads.is_empty()
            && self
                .selected
                .as_deref()
                .and_then(|id| s.get("llm_profiles", id))
                .is_some_and(|r| {
                    r.data["is_default"] != true && saved_profile_issue(&r.data).is_none()
                })
    }
    fn can_test_saved_profile(
        &self,
        s: &Snapshot,
        allowed: &BTreeSet<String>,
        read_only: bool,
    ) -> bool {
        !read_only
            && !self.busy
            && !self.conflict
            && self.pending_reads.is_empty()
            && self.pending_secrets.is_empty()
            && allowed.contains("ui.testLLM")
            && self
                .selected
                .as_deref()
                .and_then(|id| s.get("llm_profiles", id))
                .is_some_and(|row| saved_profile_issue(&row.data).is_none())
    }
    /// Activation is an immediate action on the current saved record, not a
    /// draft save. Its CAS vector is deliberately independent of the draft's
    /// pinned baseline and conflict state; none of the draft fields are sent.
    fn activate_saved_profile(
        &mut self,
        s: &Snapshot,
        allowed: &BTreeSet<String>,
        out: &mut Vec<FormSubmission>,
    ) {
        if !self.can_activate_saved_profile(s, allowed) {
            return;
        }
        let params = json!({"id":self.selected});
        if let Err(error) = validate_form("ui.activateLLMProfile", &params) {
            self.error = Some(format!("기본 모델 지정 입력 확인: {}", error.message));
            return;
        }
        self.busy = true;
        self.error = None;
        out.push(FormSubmission {
            operation: "ui.activateLLMProfile".into(),
            params,
            expected: spec::baseline("llm_profiles", self.selected.as_deref(), s),
        });
    }
}

fn saved_profile_issue(data: &Value) -> Option<&'static str> {
    if data["model"]
        .as_str()
        .is_none_or(|model| model.trim().is_empty())
    {
        return Some("저장된 모델 이름이 비어 있습니다. 모델 설정을 저장한 뒤 지정하세요.");
    }
    if !matches!(
        data["format"].as_str(),
        Some("openai" | "anthropic" | "openai-responses")
    ) {
        return Some("저장된 API 형식을 확인한 뒤 모델 설정을 저장하세요.");
    }
    if artex_core::validate_contract(
        &schema::secret_schema(),
        &schema::rpc()["$defs"],
        &data["api_key"],
    )
    .is_err()
    {
        return Some("저장된 API 키가 없습니다. 비밀 값을 저장한 뒤 모델 설정을 저장하세요.");
    }
    None
}

#[derive(Default)]
pub struct Forms {
    editors: BTreeMap<String, Editor>,
    active: Option<String>,
    next_token: u64,
    allowed: BTreeSet<String>,
    requests: Vec<(String, Value)>,
    workspace: Option<String>,
    read_only: bool,
}

impl Forms {
    pub fn all_known_operations() -> Vec<String> {
        [
            "ui.saveLLMProfile",
            "ui.deleteLLMProfile",
            "ui.activateLLMProfile",
            "ui.testLLM",
            "ui.createAgent",
            "ui.saveAgentConfig",
            "ui.deleteAgent",
            "ui.updateAgent",
            "ui.createTrigger",
            "ui.updateTrigger",
            "ui.deleteTrigger",
            "ui.saveAgentPrompt",
            "ui.resetAgentPrompt",
            "ui.saveAgentWrapup",
            "ui.resetAgentWrapup",
            "ui.saveAgentTaskTimeoutWrapup",
            "ui.resetAgentTaskTimeoutWrapup",
            "ui.setAgentVisibility",
            "ui.saveTool",
            "ui.resetTool",
            "ui.refreshMcpServer",
            "ui.createCustomTool",
            "ui.updateCustomTool",
            "ui.deleteCustomTool",
            "ui.saveMcpServer",
            "ui.deleteMcpServer",
            "ui.createSkill",
            "ui.updateSkillMeta",
            "ui.deleteSkill",
            "ui.writeSkillFile",
            "ui.deleteSkillPath",
            "ui.createSkillDir",
            "ui.createTaskCategory",
            "ui.renameTaskCategory",
            "ui.deleteTaskCategory",
            "ui.createTaskTemplate",
            "ui.updateTaskTemplate",
            "ui.deleteTaskTemplate",
            "ui.addConstraint",
            "ui.updateConstraint",
            "ui.deleteConstraint",
            "ui.createInterceptRule",
            "ui.updateInterceptRule",
            "ui.deleteInterceptRule",
            "ui.createAssetInterceptRule",
            "ui.updateAssetInterceptRule",
            "ui.deleteAssetInterceptRule",
            "ui.createTaskInterceptRule",
            "ui.updateTaskInterceptRule",
            "ui.deleteTaskInterceptRule",
            "ui.notifyCreateChannel",
            "ui.notifyUpdateChannel",
            "ui.notifyDeleteChannel",
            "ui.notifyTestChannel",
            "ui.setSettings",
            "native.secret.set",
            "native.skill.meta.write",
        ]
        .into_iter()
        .map(str::to_owned)
        .collect()
    }
    pub fn set_allowed_operations(&mut self, operations: impl IntoIterator<Item = String>) {
        self.allowed = operations.into_iter().collect();
    }
    pub fn set_read_only(&mut self, read_only: bool) {
        self.read_only = read_only;
    }
    pub fn set_model_test_result_for(&mut self, token: &str, value: &Value) {
        if let Some(e) = self
            .editors
            .values_mut()
            .find(|e| e.token == token && e.model_test_pending)
        {
            e.model_test_pending = false;
            e.busy = false;
            e.projections.insert("ui.testLLM".into(), value.clone());
        }
    }
    pub fn set_model_test_error_for(&mut self, token: &str, code: &str, message: &str) {
        if let Some(e) = self
            .editors
            .values_mut()
            .find(|e| e.token == token && e.model_test_pending)
        {
            e.model_test_pending = false;
            e.busy = false;
            e.projections.insert(
                "ui.testLLM".into(),
                json!({"ok":false,"error":format!("{code}: {message}")}),
            );
        }
    }
    pub fn active_token(&self) -> Option<&str> {
        self.active
            .as_ref()
            .and_then(|k| self.editors.get(k))
            .map(|e| e.token.as_str())
    }
    pub fn pending_projection_requests(&mut self) -> Vec<(String, Value)> {
        std::mem::take(&mut self.requests)
    }
    pub fn retire_async_operations(&mut self) {
        self.requests.clear();
        for e in self.editors.values_mut() {
            let uncertain = e.busy
                || e.file_loading
                || !e.pending_reads.is_empty()
                || !e.pending_secrets.is_empty()
                || e.requested.iter().any(|m| !e.projections.contains_key(m));
            e.busy = false;
            e.file_loading = false;
            e.model_test_pending = false;
            e.pending_reads.clear();
            e.pending_secrets.clear();
            if uncertain {
                e.conflict = true;
                e.error=Some("작업 공간 실행 세대가 종료되었습니다. 편집 내용과 이전 버전은 유지됩니다. 저장 데이터를 명시적으로 다시 불러와 결과를 확인하세요.".into());
            }
        }
    }
    pub fn set_projection(&mut self, method: &str, value: &Value) {
        if let Some(token) = self.active_token().map(str::to_owned) {
            self.set_projection_for(&token, method, value);
        }
    }
    pub fn set_projection_for(&mut self, token: &str, method: &str, value: &Value) {
        let Some(e) = self.editors.values_mut().find(|e| e.token == token) else {
            return;
        };
        let result = value.get("result").unwrap_or(value);
        if method == "native.skill.meta.read" {
            if !e.requested.contains(method)
                || e.projections.contains_key(method)
                || e.params != e.original
            {
                return;
            }
            let Some(sha) = result["sha256"]
                .as_str()
                .filter(|s| s.len() == 64 && s.bytes().all(|b| b.is_ascii_hexdigit()))
            else {
                e.error = Some("스킬 파일 버전을 확인할 수 없습니다.".into());
                return;
            };
            e.metadata_revision = Some(sha.into());
            e.params = json!({"name":e.selected,"meta":schema::shape(&result["meta"],&schema::resolve(&schema::method("ui.updateSkillMeta")["properties"]["meta"]))});
            e.original = e.params.clone();
            e.projections.insert(method.into(), result.clone());
            return;
        }
        if method == "native.body.read" {
            let Some(path) = result["_form_path"].as_array().map(|a| {
                a.iter()
                    .filter_map(|v| v.as_str().map(str::to_owned))
                    .collect::<Vec<_>>()
            }) else {
                return;
            };
            let key = serde_json::to_string(&path).unwrap();
            if !e.pending_reads.remove(&key) {
                return;
            }
            if let Some(text) = result["text"].as_str() {
                let body = json!({"text":text,"blob":null});
                if path.first().is_some_and(|v| v == "@extras") {
                    set_path(&mut e.extras, &path[1..], body);
                } else {
                    set_path(&mut e.params, &path, body);
                }
            }
            return;
        }
        if method == "ui.readSkillFile" {
            if result["_file_token"].as_u64() != Some(e.file_token) || !e.file_loading {
                return;
            }
            let content = result.get("content").unwrap_or(result);
            let text = content.as_str().or_else(|| content["text"].as_str());
            if let Some(text) = text {
                e.file_content = text.into();
                e.file_original = text.into();
                e.file_loading = false;
                e.file_revision = result.get("revision").cloned();
            } else {
                e.error = Some("파일 본문을 불러오지 못했습니다.".into());
            }
            return;
        }
        if !e.requested.contains(method) || e.projections.contains_key(method) {
            return;
        }
        e.projections.insert(method.into(), result.clone());
        if let Some(expected) = value
            .get("read_versions")
            .and_then(|v| serde_json::from_value::<Vec<Expected>>(v.clone()).ok())
        {
            for item in expected {
                if !e
                    .expected
                    .iter()
                    .any(|v| v.collection == item.collection && v.id == item.id)
                {
                    e.expected.push(item);
                }
            }
        }
        if method == "ui.settings" && e.params == e.original {
            e.params = json!({"patch":schema::shape(result,&schema::resolve(&schema::method("ui.setSettings")["properties"]["patch"]))});
            e.original = e.params.clone();
        }
        if method == "ui.skills"
            && e.collection == "skills"
            && e.params == e.original
            && e.metadata_revision.is_none()
        {
            let entries = result.as_array().or_else(|| result["skills"].as_array());
            if let Some(item) = entries
                .into_iter()
                .flatten()
                .find(|v| v["name"].as_str() == e.selected.as_deref())
            {
                e.params = json!({"name":e.selected,"meta":schema::shape(item,&schema::resolve(&schema::method("ui.updateSkillMeta")["properties"]["meta"]))});
                e.original = e.params.clone();
            }
        }
    }
    pub fn set_secret_reference(&mut self, target: &str, reference: Value) {
        let Ok(target) = serde_json::from_str::<Value>(target) else {
            return;
        };
        let Some(token) = target["form_token"].as_str() else {
            return;
        };
        if artex_core::validate_contract(
            &schema::secret_schema(),
            &schema::rpc()["$defs"],
            &reference,
        )
        .is_err()
        {
            return;
        }
        let Some(path) = target["path"].as_array().map(|a| {
            a.iter()
                .filter_map(|v| v.as_str().map(str::to_owned))
                .collect::<Vec<_>>()
        }) else {
            return;
        };
        let Some(e) = self.editors.values_mut().find(|e| {
            e.token == token && e.collection == target["collection"].as_str().unwrap_or("")
        }) else {
            return;
        };
        if !e
            .pending_secrets
            .remove(&serde_json::to_string(&path).unwrap())
        {
            return;
        }
        if path.first().is_some_and(|s| s == "@extras") {
            set_path(&mut e.extras, &path[1..], reference);
        } else {
            set_path(&mut e.params, &path, reference);
        }
    }
    pub fn set_secret_error(&mut self, target: &str, code: &str, message: &str) {
        let Ok(target) = serde_json::from_str::<Value>(target) else {
            return;
        };
        let Some(token) = target["form_token"].as_str() else {
            return;
        };
        if let Some(e) = self.editors.values_mut().find(|e| e.token == token) {
            if let Some(path) = target["path"].as_array() {
                e.pending_secrets
                    .remove(&serde_json::to_string(path).unwrap());
            }
            e.error = Some(format!("{code}: {message}"));
        }
    }
    pub fn set_error_for(&mut self, token: &str, code: &str, message: &str) {
        if let Some(e) = self.editors.values_mut().find(|e| e.token == token) {
            e.busy = false;
            e.conflict |= matches!(
                code,
                "CONFLICT" | "VERSION_CONFLICT" | "PRECONDITION_FAILED"
            );
            e.error = Some(format!("{code}: {message}"));
            if !e.busy {
                e.pending_secrets.clear();
                e.pending_reads.clear();
            }
        }
    }
    /// Called only after the writer confirms the save for this exact token.
    pub fn acknowledge_saved_for(&mut self, token: &str) {
        if let Some(key) = self
            .editors
            .iter()
            .find(|(_, e)| e.token == token)
            .map(|(k, _)| k.clone())
        {
            if self.editors[&key].selected.is_some() {
                let e = self.editors.get_mut(&key).unwrap();
                e.busy = false;
                e.conflict = true;
                e.error=Some("저장을 완료했습니다. 다른 편집 내용은 유지됩니다. 저장 데이터를 다시 불러온 뒤 다음 편집을 시작하세요.".into());
            } else {
                self.editors.remove(&key);
            }
        }
    }
    pub fn set_skill_file_saved_content(
        &mut self,
        token: &str,
        file_token: u64,
        sha256: &str,
        submitted: &str,
    ) {
        if sha256.len() != 64 || !sha256.bytes().all(|b| b.is_ascii_hexdigit()) {
            return;
        }
        if let Some(e) = self
            .editors
            .values_mut()
            .find(|e| e.token == token && e.file_token == file_token)
        {
            e.busy = false;
            e.file_revision = Some(json!(sha256));
            e.file_create = false;
            e.file_original = submitted.into();
            if e.file == "SKILL.md" {
                e.conflict = true;
                e.error=Some("SKILL.md를 저장했습니다. 메타데이터를 다시 불러오면 최신 파일 버전으로 편집할 수 있습니다.".into());
            }
        }
    }

    pub fn show(
        &mut self,
        ui: &mut Ui,
        collection: &str,
        snapshot: &Snapshot,
        selected: Option<&str>,
    ) -> Vec<FormSubmission> {
        ui.set_min_width(ui.available_width());
        if self.workspace.as_deref() != Some(&snapshot.workspace_id) {
            self.editors.clear();
            self.requests.clear();
            self.active = None;
            self.workspace = Some(snapshot.workspace_id.clone());
        }
        let collection = spec::collection_alias(collection);
        let key = editor_key(collection, selected);
        self.active = Some(key.clone());
        if !self.editors.contains_key(&key) {
            self.next_token += 1;
            let token = format!("{}:{}", snapshot.workspace_id, self.next_token);
            if let Some(editor) = Editor::new(token, collection, selected, snapshot) {
                self.editors.insert(key.clone(), editor);
            } else {
                ui.weak("이 항목에는 설정 편집기가 없거나 선택한 항목이 삭제되었습니다.");
                return vec![];
            }
        }
        let e = self.editors.get_mut(&key).unwrap();
        e.refresh_conflict(snapshot);
        let mut out = Vec::new();
        let no_mutations = BTreeSet::new();
        let allowed = if self.read_only {
            &no_mutations
        } else {
            &self.allowed
        };
        let can_edit = !self.read_only && allowed.contains(&e.operation);
        let mut reload = false;
        ui.horizontal_wrapped(|ui| {
            ui.heading(if collection == "settings" {
                "일반 설정"
            } else if collection == "llm_profiles" {
                if selected.is_some() {
                    "모델 연결 설정"
                } else {
                    "새 모델 연결"
                }
            } else if selected.is_some() {
                "설정 편집"
            } else {
                "새 항목"
            });
            if ui.button("저장 데이터 다시 불러오기").clicked() {
                reload = true;
            }
        });
        if reload {
            self.editors.remove(&key);
            return out;
        }
        if collection == "llm_profiles" {
            crate::theme::group(ui, "모델 사용 상태", |ui| {
                if let Some(current) = snapshot
                    .records("llm_profiles")
                    .into_iter()
                    .find(|r| r.data["is_default"] == true)
                {
                    ui.label(format!(
                        "현재 기본 모델: {} · {}",
                        current.data["name"].as_str().unwrap_or("이름 없음"),
                        current.data["model"].as_str().unwrap_or("모델 없음")
                    ));
                } else {
                    ui.colored_label(
                        ui.visuals().warn_fg_color,
                        "기본 모델이 지정되지 않았습니다.",
                    );
                }
                if let Some(saved) = e
                    .selected
                    .as_deref()
                    .and_then(|id| snapshot.get("llm_profiles", id))
                {
                    if saved.data["is_default"] == true {
                        ui.label("선택한 저장 모델이 현재 기본 모델입니다.");
                    } else {
                        if let Some(issue) = saved_profile_issue(&saved.data) {
                            ui.weak(issue);
                        }
                    }
                } else {
                    ui.weak("모델 설정을 저장한 뒤 목록에서 저장된 모델을 선택해 기본 모델로 지정하세요.");
                }
            });
        }
        if e.conflict {
            ui.colored_label(
                ui.visuals().warn_fg_color,
                "편집을 시작한 이후 저장 데이터가 변경되었습니다. 다시 불러온 뒤 수정하세요.",
            );
        }
        if !can_edit {
            ui.weak(if self.read_only {
                "읽기 전용 · 설정 값을 변경할 수 없습니다."
            } else {
                "이 작업을 편집할 권한이 없습니다."
            });
        }
        if let Some(error) = &e.error {
            ui.colored_label(ui.visuals().error_fg_color, error);
        }
        if e.busy {
            ui.spinner();
            ui.weak("저장 결과를 기다리는 중…");
        }
        if collection == "settings" {
            e.queue("ui.settings", json!({}), &mut self.requests);
        }
        if matches!(collection, "skills" | "agents" | "agent_visibility") {
            e.queue("ui.skills", json!({}), &mut self.requests);
        }
        if collection == "skills" && selected.is_some() {
            e.queue(
                "native.skill.meta.read",
                json!({"name":selected}),
                &mut self.requests,
            );
            e.queue(
                "ui.skillFiles",
                json!({"name":selected}),
                &mut self.requests,
            );
        }
        let loading = (collection == "settings" && !e.projections.contains_key("ui.settings"))
            || (collection == "skills"
                && selected.is_some()
                && !e.projections.contains_key("native.skill.meta.read"));
        if loading {
            ui.weak("설정 데이터를 불러오는 중…");
        }
        if collection == "llm_profiles" {
            let action_ready = can_edit
                && !e.conflict
                && !e.busy
                && !loading
                && e.pending_reads.is_empty()
                && e.pending_secrets.is_empty()
                && artex_core::validate_secret_parameters(
                    &e.operation,
                    &schema::shape(&e.params, &schema::method(&e.operation)),
                )
                .is_ok();
            crate::theme::toolbar(ui, |ui| {
                if ui
                    .add_enabled_ui(action_ready, |ui| {
                        crate::theme::primary_button(ui, "모델 설정 저장")
                    })
                    .inner
                    .clicked()
                {
                    e.submit(&e.operation.clone(), e.params.clone(), allowed, &mut out);
                }
                if let Some(saved) = e
                    .selected
                    .as_deref()
                    .and_then(|id| snapshot.get("llm_profiles", id))
                {
                    if ui
                        .add_enabled(
                            action_ready
                                && e.can_test_saved_profile(snapshot, allowed, self.read_only),
                            egui::Button::new("저장된 모델 연결 시험"),
                        )
                        .on_hover_text("미저장 편집 내용은 보내지 않고 저장된 연결로 시험합니다.")
                        .clicked()
                    {
                        e.submit(
                            "ui.testLLM",
                            saved_model_test_params(saved),
                            allowed,
                            &mut out,
                        );
                    }
                    if saved.data["is_default"] != true && ui.add_enabled(
                        !self.read_only && e.can_activate_saved_profile(snapshot, allowed),
                        egui::Button::new("기본 모델로 설정"),
                    ).on_hover_text("저장된 설정으로 기본 모델을 지정합니다. 미저장 편집 내용은 유지되며, 연결 성공을 뜻하지는 않습니다.").clicked() {
                        e.activate_saved_profile(snapshot, allowed, &mut out);
                    }
                }
            });
            if let Some(result) = e.projections.get("ui.testLLM") {
                if result["ok"] == true {
                    ui.label("연결 시험 성공");
                } else {
                    ui.colored_label(
                        ui.visuals().error_fg_color,
                        result["error"].as_str().unwrap_or("연결 시험 실패"),
                    );
                }
                if let Some(ms) = result["latency_ms"].as_f64() {
                    ui.weak(format!("응답 시간: {ms:.0} ms"));
                }
            }
        }
        let mut invalid_secret = false;
        ui.add_enabled_ui(can_edit && !e.conflict && !e.busy && !loading, |ui| {
            let mut state = widgets::WidgetState {
                token: &e.token,
                collection: &e.collection,
                selected: e.selected.as_deref(),
                references: &e.references,
                secrets: &mut e.secrets,
                pending_secrets: &mut e.pending_secrets,
                pending_reads: &mut e.pending_reads,
                submissions: &mut out,
                requests: &mut self.requests,
                can_secret: allowed.contains("native.secret.set"),
                invalid_secret: false,
            };
            widgets::field(
                ui,
                "",
                &mut e.params,
                &schema::method(&e.operation),
                &[],
                &mut state,
                0,
            );
            invalid_secret = state.invalid_secret;
        });
        if self.read_only {
            readonly_body_requests(
                ui,
                &e.params,
                &schema::method(&e.operation),
                &[],
                &e.token,
                &mut e.pending_reads,
                &mut self.requests,
            );
            ui.collapsing("설정 원문 확인 · 읽기 전용", |ui| {
                readonly_values(ui, &e.params)
            });
        }
        let ready = can_edit
            && !e.conflict
            && !e.busy
            && !loading
            && !invalid_secret
            && e.pending_reads.is_empty()
            && e.pending_secrets.is_empty()
            && e.file_content == e.file_original;
        if e.file_content != e.file_original {
            ui.weak("파일의 미저장 내용을 먼저 저장하거나 파일을 다시 불러오세요.");
        }
        crate::theme::toolbar(ui, |ui| {
            if collection != "llm_profiles"
                && ui
                    .add_enabled_ui(ready, |ui| crate::theme::primary_button(ui, "저장"))
                    .inner
                    .clicked()
            {
                e.submit(&e.operation.clone(), e.params.clone(), allowed, &mut out);
            }
            if collection == "notification_channels"
                && selected.is_some()
                && ui
                    .add_enabled(
                        ready && allowed.contains("ui.notifyTestChannel"),
                        egui::Button::new("저장된 채널로 시험 알림 전송"),
                    )
                    .clicked()
            {
                e.submit(
                    "ui.notifyTestChannel",
                    json!({"id":selected}),
                    allowed,
                    &mut out,
                );
            }
            if collection == "mcp_servers"
                && selected.is_some()
                && ui
                    .add_enabled(
                        ready && allowed.contains("ui.refreshMcpServer"),
                        egui::Button::new("저장된 서버에서 도구 다시 조회"),
                    )
                    .clicked()
            {
                e.submit(
                    "ui.refreshMcpServer",
                    json!({"id":selected}),
                    allowed,
                    &mut out,
                );
            }
            if collection == "tools"
                && e.source["system"] == true
                && ui
                    .add_enabled(
                        ready && allowed.contains("ui.resetTool"),
                        egui::Button::new("내장 도구 정의로 복원"),
                    )
                    .clicked()
            {
                e.submit(
                    "ui.resetTool",
                    json!({"key":e.source["key"]}),
                    allowed,
                    &mut out,
                );
            }
        });
        if collection == "agents" && selected.is_some() {
            agent_panels(ui, e, allowed, &mut out, &mut self.requests);
        }
        if collection == "skills" && selected.is_some() {
            skill_files(ui, e, allowed, &mut out, &mut self.requests);
        }
        let deletion = if collection == "skills" && selected.is_some() {
            Some(("ui.deleteSkill".into(), json!({"name":selected})))
        } else if collection == "task_constraints" && selected.is_some() {
            Some((
                "ui.deleteConstraint".into(),
                json!({"id":e.params["id"],"constraintId":selected}),
            ))
        } else if collection == "task_intercept_rules" && selected.is_some() {
            Some((
                "ui.deleteTaskInterceptRule".into(),
                json!({"id":e.params["id"],"ruleId":selected}),
            ))
        } else {
            spec::delete(collection, &e.source)
        };
        if let Some((operation, params)) = deletion.filter(|_| selected.is_some()) {
            ui.separator();
            crate::theme::toolbar(ui, |ui| {
                ui.add_enabled(
                    !self.read_only,
                    egui::Checkbox::new(
                        &mut e.delete_confirm,
                        "이 항목을 삭제하는 것을 확인합니다.",
                    ),
                );
                if ui
                    .add_enabled(
                        ready && e.delete_confirm && allowed.contains(&operation),
                        egui::Button::new("삭제"),
                    )
                    .clicked()
                {
                    e.submit(&operation, params, allowed, &mut out);
                }
            });
        }
        out
    }
}

fn saved_model_test_params(saved: &artex_core::Record) -> Value {
    let mut params = json!({"profile_id":saved.header.id,"provider":saved.data["format"],"model":saved.data["model"],"base_url":saved.data["base_url"].as_str().unwrap_or(""),"api_key":saved.data["api_key"]});
    for field in [
        "proxy",
        "thinking_type",
        "reasoning_effort",
        "streaming",
        "session_header_key",
    ] {
        if let Some(value) = saved.data.get(field).filter(|v| !v.is_null()) {
            params[field] = value.clone();
        }
    }
    params
}

fn editor_key(collection: &str, selected: Option<&str>) -> String {
    format!("{collection}:{}", selected.unwrap_or("@new"))
}
/// Inline large bodies are externalized by the in-process writer. This
/// validation-only copy represents their future shape without uploading or
/// putting a fabricated BlobRef into a submission.
fn validate_form(operation: &str, params: &Value) -> artex_core::Result<()> {
    fn future_body(v: &mut Value, s: &Value) {
        let s = schema::actual(s, v);
        if schema::is_body(&s)
            && v["blob"].is_null()
            && v["text"].as_str().is_some_and(|t| t.len() > 65536)
        {
            let len = v["text"].as_str().unwrap().len();
            *v = json!({"text":null,"blob":{"sha256":"0".repeat(64),"length":len.to_string(),"media_type":"text/plain"}});
            return;
        }
        if let Some(props) = s["properties"].as_object() {
            for (key, child) in props {
                if let Some(value) = v.get_mut(key) {
                    future_body(value, child);
                }
            }
        }
        if let Some(items) = v.as_array_mut() {
            for item in items {
                future_body(item, &s["items"]);
            }
        }
    }
    let s = schema::method(operation);
    let mut validated = params.clone();
    future_body(&mut validated, &s);
    artex_core::validate_contract(&s, &schema::rpc()["$defs"], &validated)
}
fn set_path(root: &mut Value, path: &[String], value: Value) -> bool {
    if path.is_empty() {
        *root = value;
        return true;
    }
    let mut cursor = root;
    for part in &path[..path.len() - 1] {
        let Some(next) = (if cursor.is_array() {
            part.parse::<usize>().ok().and_then(|i| cursor.get_mut(i))
        } else {
            cursor.get_mut(part)
        }) else {
            return false;
        };
        cursor = next;
    }
    let part = path.last().unwrap();
    if let Some(a) = cursor.as_array_mut() {
        if let Some(slot) = part.parse::<usize>().ok().and_then(|i| a.get_mut(i)) {
            *slot = value;
            return true;
        }
        return false;
    }
    if let Some(object) = cursor.as_object_mut() {
        if object.contains_key(part) {
            object.insert(part.clone(), value);
            return true;
        }
    }
    false
}

fn extra_field(
    ui: &mut Ui,
    e: &mut Editor,
    key: &str,
    s: &Value,
    out: &mut Vec<FormSubmission>,
    requests: &mut Vec<(String, Value)>,
    allowed: &BTreeSet<String>,
) {
    let mut state = widgets::WidgetState {
        token: &e.token,
        collection: &e.collection,
        selected: e.selected.as_deref(),
        references: &e.references,
        secrets: &mut e.secrets,
        pending_secrets: &mut e.pending_secrets,
        pending_reads: &mut e.pending_reads,
        submissions: out,
        requests,
        can_secret: allowed.contains("native.secret.set"),
        invalid_secret: false,
    };
    ui.add_enabled_ui(!allowed.is_empty(), |ui| {
        widgets::field(
            ui,
            key,
            &mut e.extras[key],
            s,
            &["@extras".into(), key.into()],
            &mut state,
            0,
        )
    });
    if allowed.is_empty() {
        readonly_body_requests(
            ui,
            &e.extras[key],
            s,
            &["@extras".into(), key.into()],
            &e.token,
            &mut e.pending_reads,
            requests,
        );
    }
}
fn readonly_body_requests(
    ui: &mut Ui,
    value: &Value,
    contract: &Value,
    path: &[String],
    token: &str,
    pending: &mut BTreeSet<String>,
    requests: &mut Vec<(String, Value)>,
) {
    let contract = schema::actual(contract, value);
    if schema::is_body(&contract) {
        if value["blob"].is_object() && value["text"].is_null() {
            let locator = serde_json::to_string(path).unwrap();
            if ui
                .add_enabled(
                    !pending.contains(&locator),
                    egui::Button::new(format!(
                        "{} · 저장 본문 조회",
                        schema::label(path.last().map(String::as_str).unwrap_or("내용"))
                    )),
                )
                .clicked()
            {
                pending.insert(locator);
                requests.push((
                    "native.body.read".into(),
                    json!({"reference":value["blob"],"_form_token":token,"_form_path":path}),
                ));
            }
        }
        return;
    }
    if let Some(properties) = contract["properties"].as_object() {
        for (key, child) in properties {
            if let Some(value) = value.get(key) {
                let mut next = path.to_vec();
                next.push(key.clone());
                readonly_body_requests(ui, value, child, &next, token, pending, requests);
            }
        }
    } else if let Some(items) = value.as_array() {
        for (index, value) in items.iter().enumerate() {
            let mut next = path.to_vec();
            next.push(index.to_string());
            readonly_body_requests(
                ui,
                value,
                &contract["items"],
                &next,
                token,
                pending,
                requests,
            );
        }
    }
}
fn readonly_values(ui: &mut Ui, value: &Value) {
    if let Some(fields) = value.as_object() {
        for (key, value) in fields {
            if widgets::sensitive(key) || value.get("secret_ref").is_some() {
                ui.label(format!(
                    "{}: 비밀 값은 안전한 참조로 보관됩니다.",
                    schema::label(key)
                ));
            } else if value.is_object() || value.is_array() {
                ui.collapsing(key, |ui| readonly_values(ui, value));
            } else {
                ui.horizontal_wrapped(|ui| {
                    ui.monospace(key);
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
                });
            }
        }
    } else if let Some(items) = value.as_array() {
        for value in items {
            readonly_values(ui, value);
        }
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
}
fn body_text(v: &Value) -> Option<&str> {
    v.as_str().or_else(|| v["text"].as_str())
}

fn agent_panels(
    ui: &mut Ui,
    e: &mut Editor,
    allowed: &BTreeSet<String>,
    out: &mut Vec<FormSubmission>,
    requests: &mut Vec<(String, Value)>,
) {
    let key = e.source["key"].as_str().unwrap_or("").to_owned();
    let writable = !e.conflict && !e.busy;
    ui.add_enabled_ui(writable,|ui|{
        egui::CollapsingHeader::new("에이전트 이름과 설명").show(ui,|ui|{
            extra_field(ui,e,"metadata",&schema::method("ui.updateAgent"),out,requests,allowed);
            if ui.add_enabled(allowed.contains("ui.updateAgent"),egui::Button::new("이름과 설명 저장")).clicked(){e.submit("ui.updateAgent",e.extras["metadata"].clone(),allowed,out);}
        });
        egui::CollapsingHeader::new("에이전트 프롬프트와 이력").show(ui,|ui|{
            extra_field(ui,e,"prompt",&json!({"$ref":"#/$defs/Body"}),out,requests,allowed);
            extra_field(ui,e,"note",&json!({"type":"string"}),out,requests,allowed);
            if ui.add_enabled(allowed.contains("ui.saveAgentPrompt")&&body_text(&e.extras["prompt"]).is_some(),egui::Button::new("프롬프트 저장")).clicked(){e.submit("ui.saveAgentPrompt",json!({"key":key,"template":body_text(&e.extras["prompt"]).unwrap_or(""),"note":e.extras["note"]}),allowed,out);}
            if ui.add_enabled(allowed.contains("ui.resetAgentPrompt"),egui::Button::new("내장 프롬프트로 복원")).clicked(){e.submit("ui.resetAgentPrompt",json!({"key":key}),allowed,out);}
            let mut history=e.extras["history"].as_array().cloned().unwrap_or_default().into_iter().enumerate().collect::<Vec<_>>();history.sort_by_key(|(_,v)|std::cmp::Reverse(v["version"].as_u64().unwrap_or(0)));
            for(index,item)in history{egui::CollapsingHeader::new(format!("버전 {} · {}",item["version"],item["updated_by"].as_str().unwrap_or(""))).show(ui,|ui|{
                ui.label(item["note"].as_str().unwrap_or(""));if let Some(text)=body_text(&item["template_text"]){egui::ScrollArea::vertical().max_height(220.0).show(ui,|ui|{ui.label(text);});if ui.add_enabled(allowed.contains("ui.saveAgentPrompt"),egui::Button::new("이 버전을 현재 편집기에 불러오기")).clicked(){e.extras["prompt"]=item["template_text"].clone();}}
                else{let path=vec!["@extras".to_owned(),"history".into(),index.to_string(),"template_text".into()];let locator=serde_json::to_string(&path).unwrap();if ui.add_enabled(!e.pending_reads.contains(&locator),egui::Button::new("이력 본문 불러오기")).clicked(){e.pending_reads.insert(locator);requests.push(("native.body.read".into(),json!({"reference":item["template_text"]["blob"],"_form_token":e.token,"_form_path":path})));}}
            });}
        });
        egui::CollapsingHeader::new("종료 정리 프롬프트").show(ui,|ui|{
            for(timeout,field,turns,operation,reset)in [(false,"wrapup","wrapup_turns","ui.saveAgentWrapup","ui.resetAgentWrapup"),(true,"timeout_wrapup","timeout_turns","ui.saveAgentTaskTimeoutWrapup","ui.resetAgentTaskTimeoutWrapup")]{
                if timeout&&!matches!(key.as_str(),"worker"|"planner"){continue;}
                ui.label(if timeout{"작업 시간 만료 시 정리"}else{"실행 종료 시 정리"});
                extra_field(ui,e,field,&json!({"$ref":"#/$defs/Body"}),out,requests,allowed);extra_field(ui,e,turns,&json!({"type":"integer"}),out,requests,allowed);
                if ui.add_enabled(allowed.contains(operation)&&body_text(&e.extras[field]).is_some(),egui::Button::new("정리 프롬프트 저장")).clicked(){e.submit(operation,json!({"key":key,"prompt":body_text(&e.extras[field]).unwrap_or(""),"maxTurns":e.extras[turns]}),allowed,out);}
                if ui.add_enabled(allowed.contains(reset),egui::Button::new("기본값 복원")).clicked(){e.submit(reset,json!({"key":key}),allowed,out);}
            }
        });
        egui::CollapsingHeader::new("MCP · 스킬 접근 권한").show(ui,|ui|{
            extra_field(ui,e,"visibility",&schema::method("ui.setAgentVisibility"),out,requests,allowed);
            if let Some(skills)=e.projections.get("ui.skills").and_then(Value::as_array).cloned(){for skill in skills{if let Some(name)=skill["name"].as_str(){let list=e.extras["visibility"]["skill"].as_array_mut();if let Some(list)=list{let mut enabled=list.contains(&json!(name));if ui.add_enabled(allowed.contains("ui.setAgentVisibility"),egui::Checkbox::new(&mut enabled,name)).changed(){list.retain(|v|v!=name);if enabled{list.push(json!(name));}}}}}}
            if ui.add_enabled(allowed.contains("ui.setAgentVisibility"),egui::Button::new("접근 권한 저장")).clicked(){e.submit("ui.setAgentVisibility",e.extras["visibility"].clone(),allowed,out);}
        });
        if !matches!(key.as_str(),"planner"|"worker"|"mainagent"|"goals") {
            egui::CollapsingHeader::new("자동 실행 조건").show(ui,|ui|{
                let trigger_schema=json!({"type":"array","items":{"type":"object","properties":{"id":{"type":"string"},"t":schema::resolve(&schema::method("ui.createTrigger")["properties"]["t"])},"required":["t"]}});
                extra_field(ui,e,"triggers",&trigger_schema,out,requests,allowed);
                let mut action=None;if let Some(triggers)=e.extras["triggers"].as_array(){for(index,trigger)in triggers.iter().enumerate(){ui.horizontal_wrapped(|ui|{ui.label(format!("조건 {}",index+1));if ui.add_enabled(allowed.contains("ui.updateTrigger")||allowed.contains("ui.createTrigger"),egui::Button::new("조건 저장")).clicked(){action=Some(if trigger["id"].as_str().is_some_and(|s|!s.is_empty()){("ui.updateTrigger",json!({"id":trigger["id"],"t":trigger["t"]}))}else{("ui.createTrigger",json!({"key":key,"t":trigger["t"]}))});}if trigger["id"].is_string()&&ui.add_enabled(allowed.contains("ui.deleteTrigger"),egui::Button::new("조건 삭제")).clicked(){action=Some(("ui.deleteTrigger",json!({"id":trigger["id"]})));}});}}
                if let Some((operation,p))=action{e.submit(operation,p,allowed,out);}
            });
        }
        egui::CollapsingHeader::new("도구 바인딩").show(ui,|ui|{
            let mut pending=None;if let Some(tools)=e.extras["tools"].as_array_mut(){for tool in tools{let mut enabled=tool["agents"].as_array().is_some_and(|a|a.contains(&json!(key)));ui.horizontal_wrapped(|ui|{if ui.add_enabled(allowed.contains("ui.saveTool"),egui::Checkbox::new(&mut enabled,tool["key"].as_str().unwrap_or("도구"))).changed(){let agents=tool["agents"].as_array_mut();if let Some(agents)=agents{agents.retain(|v|v!=&key);if enabled{agents.push(json!(key));}}}if ui.add_enabled(allowed.contains("ui.saveTool"),egui::Button::new("바인딩 저장")).clicked(){pending=Some(json!({"key":tool["key"],"patch":{"description":tool["description"],"enabled":tool["enabled"],"schema":tool["schema"],"agents":tool["agents"]}}));}});}}
            if let Some(p)=pending{e.submit("ui.saveTool",p,allowed,out);}
        });
    });
}

fn skill_files(
    ui: &mut Ui,
    e: &mut Editor,
    allowed: &BTreeSet<String>,
    out: &mut Vec<FormSubmission>,
    requests: &mut Vec<(String, Value)>,
) {
    egui::CollapsingHeader::new("스킬 파일").show(ui,|ui|{
        if ui.add_enabled(!e.busy&&e.projections.contains_key("ui.skillFiles"),egui::Button::new("파일 목록 다시 불러오기")).clicked(){
            e.projections.remove("ui.skillFiles");
            requests.push(("ui.skillFiles".into(),json!({"name":e.selected,"_form_token":e.token})));
        }
        let files=e.projections.get("ui.skillFiles").cloned().unwrap_or(json!([]));
        let files=files.as_array().or_else(||files["files"].as_array()).cloned().unwrap_or_default();let mut selected=None;
        for file in files {let name=file.as_str().or_else(||file["path"].as_str()).or_else(||file["name"].as_str()).unwrap_or("");if !name.is_empty()&&ui.add_enabled(!e.busy&&!e.conflict&&!e.file_loading,egui::Button::selectable(e.file==name,name)).clicked(){selected=Some(name.to_owned());}}
        if let Some(file)=selected{if e.file_content!=e.file_original{e.file_target=Some(file);}else{load_skill_file(e,file,requests);}}
        if let Some(target)=e.file_target.clone(){ui.colored_label(ui.visuals().warn_fg_color,"현재 파일에 미저장 변경 사항이 있습니다.");ui.horizontal_wrapped(|ui|{if ui.button("변경 사항을 버리고 다른 파일 열기").clicked(){e.file_target=None;load_skill_file(e,target,requests);}if ui.button("현재 파일 유지").clicked(){e.file_target=None;}});}
        ui.horizontal_wrapped(|ui|{ui.label("새 파일 경로");ui.add_enabled(allowed.contains("ui.writeSkillFile"),egui::TextEdit::singleline(&mut e.new_file));if ui.add_enabled(allowed.contains("ui.writeSkillFile")&&!e.busy&&!e.conflict&&e.file_content==e.file_original,egui::Button::new("새 파일 편집")).clicked(){let path=e.new_file.trim().replace('\\',"/");if !path.is_empty()&&!path.starts_with('/')&&!path.contains(':')&&!path.split('/').any(|p|p==".."||p.is_empty()){e.file=path;e.file_create=true;e.file_token+=1;e.file_loading=false;e.file_revision=None;e.file_content.clear();e.file_original.clear();}else{e.error=Some("스킬 안의 상대 파일 경로를 입력하세요.".into());}}});
        if e.file_loading{ui.spinner();return;}
        if !e.file.is_empty(){ui.label(&e.file);ui.add_enabled_ui(allowed.contains("ui.writeSkillFile")&&!e.busy&&!e.conflict,|ui|{ui.add(egui::TextEdit::multiline(&mut e.file_content).desired_rows(14).desired_width(f32::INFINITY));});
            // File revision is sent outside Source RPC params by the native service.
            let can_save=allowed.contains("ui.writeSkillFile")&&!e.busy&&!e.conflict&&(e.file_revision.is_some()||e.file_create);
            if e.file_revision.is_none()&&!e.file_create{ui.weak("파일 버전 확인이 필요합니다. 확인된 파일을 불러온 뒤 저장할 수 있습니다.");}
            if ui.add_enabled(can_save,egui::Button::new("파일 저장")).clicked(){let params=json!({"name":e.selected,"file":e.file,"content":{"text":e.file_content,"blob":null},"_file_revision":e.file_revision,"_file_token":e.file_token,"_file_create":e.file_create});e.busy=true;out.push(FormSubmission{operation:"ui.writeSkillFile".into(),params,expected:e.expected.clone()});}
        }
    });
}
fn load_skill_file(e: &mut Editor, file: String, requests: &mut Vec<(String, Value)>) {
    e.file = file;
    e.file_create = false;
    e.file_token += 1;
    e.file_loading = true;
    e.file_content.clear();
    e.file_original.clear();
    e.file_revision = None;
    requests.push((
        "ui.readSkillFile".into(),
        json!({"name":e.selected,"file":e.file,"_form_token":e.token,"_file_token":e.file_token}),
    ));
}

#[cfg(test)]
mod tests {
    use super::*;
    use artex_core::{Header, Record};
    fn fixture() -> (Snapshot, String) {
        let id = format!("profile_{}", "a".repeat(32));
        let mut data = artex_core::default_data("llm_profiles").unwrap();
        data["id"] = json!(id);
        data["name"] = json!("Local model");
        data["format"] = json!("openai");
        data["model"] = json!("fixture-model");
        data["api_key"] =
            json!({"secret_ref":format!("secret_{}","b".repeat(32)),"hint":"stored key"});
        let row = Record {
            header: Header {
                id: id.clone(),
                collection: "llm_profiles".into(),
                version: "1".into(),
                commit_seq: "1".into(),
                created_at: "2026-10-13T00:00:00Z".into(),
                updated_at: "2026-10-13T00:00:00Z".into(),
                legacy_key: Value::Null,
            },
            data,
        };
        let mut snapshot = Snapshot {
            workspace_id: "workspace-fixture".into(),
            commit_seq: 1,
            collections: BTreeMap::new(),
        };
        snapshot
            .collections
            .entry("llm_profiles".into())
            .or_default()
            .insert(id.clone(), row);
        (snapshot, id)
    }
    #[test]
    fn actual_readonly_model_actions_cannot_save_activate_delete_or_seal_and_preserve_drafts() {
        let (s, id) = fixture();
        let ctx = egui::Context::default();
        crate::app::configure(&ctx);
        let mut forms = Forms::default();
        forms.set_allowed_operations(Forms::all_known_operations());
        forms.set_read_only(true);
        let mut submissions = vec![];
        let render = |forms: &mut Forms, submissions: &mut Vec<FormSubmission>, events| {
            ctx.run(
                egui::RawInput {
                    screen_rect: Some(egui::Rect::from_min_size(
                        egui::Pos2::ZERO,
                        egui::vec2(900.0, 2400.0),
                    )),
                    events,
                    ..Default::default()
                },
                |ctx| {
                    egui::CentralPanel::default().show(ctx, |ui| {
                        submissions.extend(forms.show(ui, "llm_profiles", &s, Some(&id)));
                    });
                },
            )
        };
        let _ = render(&mut forms, &mut submissions, vec![]);
        let key = editor_key("llm_profiles", Some(&id));
        let e = forms.editors.get_mut(&key).unwrap();
        e.params["p"]["model"] = json!("kept-readonly-draft");
        e.secrets.insert(
            serde_json::to_string(&vec!["p", "api_key"]).unwrap(),
            "private-input-not-sent".into(),
        );
        e.delete_confirm = true;
        let params = e.params.clone();
        let versions = e.expected.clone();
        let secrets = e.secrets.clone();
        let output = render(&mut forms, &mut submissions, vec![]);
        for label in [
            "모델 설정 저장",
            "기본 모델로 설정",
            "저장된 모델 연결 시험",
            "비밀 값 저장",
            "삭제",
        ] {
            let position = output
                .shapes
                .iter()
                .find_map(|shape| {
                    if let egui::epaint::Shape::Text(text) = &shape.shape {
                        (text.galley.text() == label).then(|| text.visual_bounding_rect().center())
                    } else {
                        None
                    }
                })
                .unwrap_or_else(|| {
                    panic!("Readonly action still needs a visible disabled affordance: {label}")
                });
            let events = vec![
                egui::Event::PointerMoved(position),
                egui::Event::PointerButton {
                    pos: position,
                    button: egui::PointerButton::Primary,
                    pressed: true,
                    modifiers: egui::Modifiers::NONE,
                },
                egui::Event::PointerButton {
                    pos: position,
                    button: egui::PointerButton::Primary,
                    pressed: false,
                    modifiers: egui::Modifiers::NONE,
                },
            ];
            let _ = render(&mut forms, &mut submissions, events);
            assert!(
                submissions.is_empty(),
                "Readonly {label} must not be admitted"
            );
        }
        let e = &forms.editors[&key];
        assert_eq!(e.params, params);
        assert_eq!(e.expected, versions);
        assert_eq!(e.secrets, secrets);
        assert!(!e.busy && !e.model_test_pending && e.pending_secrets.is_empty());
    }
    #[test]
    fn readonly_body_query_remains_clickable_without_mutating_reference_or_submitting_a_command() {
        let ctx = egui::Context::default();
        crate::app::configure(&ctx);
        let body = json!({"text":null,"blob":{"sha256":"a".repeat(64),"length":"100","media_type":"text/plain"}});
        artex_core::validate_contract(
            &schema::rpc()["$defs"]["Body"],
            &schema::rpc()["$defs"],
            &body,
        )
        .unwrap();
        let original = body.clone();
        let mut pending = BTreeSet::new();
        let mut requests = vec![];
        let mut render = |events| {
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
                        readonly_body_requests(
                            ui,
                            &body,
                            &json!({"$ref":"#/$defs/Body"}),
                            &["p".into(), "prompt".into()],
                            "readonly-owner",
                            &mut pending,
                            &mut requests,
                        )
                    });
                },
            )
        };
        let output = render(vec![]);
        let position = output
            .shapes
            .iter()
            .find_map(|shape| {
                if let egui::epaint::Shape::Text(text) = &shape.shape {
                    (text.galley.text() == "prompt · 저장 본문 조회")
                        .then(|| text.visual_bounding_rect().center())
                } else {
                    None
                }
            })
            .unwrap();
        let _ = render(vec![
            egui::Event::PointerMoved(position),
            egui::Event::PointerButton {
                pos: position,
                button: egui::PointerButton::Primary,
                pressed: true,
                modifiers: egui::Modifiers::NONE,
            },
            egui::Event::PointerButton {
                pos: position,
                button: egui::PointerButton::Primary,
                pressed: false,
                modifiers: egui::Modifiers::NONE,
            },
        ]);
        drop(render);
        assert_eq!(requests.len(), 1);
        assert_eq!(requests[0].0, "native.body.read");
        assert_eq!(requests[0].1["_form_token"], "readonly-owner");
        assert_eq!(requests[0].1["_form_path"], json!(["p", "prompt"]));
        assert_eq!(body, original);
    }
    #[test]
    fn actual_model_secret_hint_localizes_configured_and_keeps_custom_hints_and_references() {
        for (hint, expected) in [
            ("configured", "비밀 값 등록됨"),
            ("사용자 지정 안내", "저장됨: 사용자 지정 안내"),
        ] {
            let (mut s, id) = fixture();
            s.collections
                .get_mut("llm_profiles")
                .unwrap()
                .get_mut(&id)
                .unwrap()
                .data["api_key"]["hint"] = json!(hint);
            let reference = s.get("llm_profiles", &id).unwrap().data["api_key"].clone();
            let ctx = egui::Context::default();
            crate::app::configure(&ctx);
            let mut forms = Forms::default();
            forms.set_allowed_operations(Forms::all_known_operations());
            let render = |forms: &mut Forms| {
                ctx.run(
                    egui::RawInput {
                        screen_rect: Some(egui::Rect::from_min_size(
                            egui::Pos2::ZERO,
                            egui::vec2(900.0, 2400.0),
                        )),
                        ..Default::default()
                    },
                    |ctx| {
                        egui::CentralPanel::default().show(ctx, |ui| {
                            assert!(forms.show(ui, "llm_profiles", &s, Some(&id)).is_empty());
                        });
                    },
                )
            };
            let _ = render(&mut forms);
            let output = render(&mut forms);
            let labels = output
                .shapes
                .iter()
                .filter_map(|shape| {
                    if let egui::epaint::Shape::Text(text) = &shape.shape {
                        Some(text.galley.text())
                    } else {
                        None
                    }
                })
                .collect::<Vec<_>>();
            assert!(
                labels.contains(&expected),
                "Stored hint must be rendered through the actual model form"
            );
            if hint == "configured" {
                assert!(!labels.iter().any(|label| label.contains("configured")));
            }
            assert_eq!(
                forms.editors[&editor_key("llm_profiles", Some(&id))].params["p"]["api_key"],
                reference
            );
        }
    }
    #[test]
    fn model_hierarchy_renders_connection_response_and_collapsed_advanced_without_changing_payload()
    {
        let (s, id) = fixture();
        let initial = Editor::new("initial".into(), "llm_profiles", Some(&id), &s).unwrap();
        let ctx = egui::Context::default();
        crate::app::configure(&ctx);
        let mut forms = Forms::default();
        forms.set_allowed_operations(Forms::all_known_operations());
        let render = |forms: &mut Forms| {
            ctx.run(
                egui::RawInput {
                    screen_rect: Some(egui::Rect::from_min_size(
                        egui::Pos2::ZERO,
                        egui::vec2(900.0, 2400.0),
                    )),
                    ..Default::default()
                },
                |ctx| {
                    egui::CentralPanel::default().show(ctx, |ui| {
                        assert!(forms.show(ui, "llm_profiles", &s, Some(&id)).is_empty());
                    });
                },
            )
        };
        let _ = render(&mut forms);
        let output = render(&mut forms);
        let labels = output
            .shapes
            .iter()
            .filter_map(|shape| {
                if let egui::epaint::Shape::Text(text) = &shape.shape {
                    Some(text.galley.text())
                } else {
                    None
                }
            })
            .collect::<Vec<_>>();
        for label in [
            "연결 설정",
            "표시 이름",
            "API 형식",
            "주소",
            "모델",
            "API 키",
            "응답 설정",
            "고급 설정",
            "모델 설정 저장",
            "저장된 모델 연결 시험",
        ] {
            assert!(
                labels.contains(&label),
                "Missing model UI input/action: {label}"
            );
        }
        assert!(
            !labels.iter().any(|label| label.contains(&id)),
            "Record identity must stay out of the primary model form"
        );
        let e = &forms.editors[&editor_key("llm_profiles", Some(&id))];
        assert_eq!(e.params, initial.params);
        assert_eq!(e.expected, initial.expected);
        let profile_schema =
            schema::resolve(&schema::method("ui.saveLLMProfile")["properties"]["p"]);
        for key in profile_schema["properties"].as_object().unwrap().keys() {
            assert!(schema::field_group("llm_profiles", key) <= 2);
        }
        assert_eq!(schema::field_group("llm_profiles", "id"), 2);
        assert_eq!(schema::field_group("llm_profiles", "api_key"), 0);
    }
    #[test]
    fn general_settings_groups_keep_boolean_and_credential_status_values_without_raw_false_labels()
    {
        let (s, _) = fixture();
        let ctx = egui::Context::default();
        crate::app::configure(&ctx);
        let mut forms = Forms::default();
        forms.set_allowed_operations(Forms::all_known_operations());
        let render = |forms: &mut Forms| {
            ctx.run(
                egui::RawInput {
                    screen_rect: Some(egui::Rect::from_min_size(
                        egui::Pos2::ZERO,
                        egui::vec2(900.0, 2400.0),
                    )),
                    ..Default::default()
                },
                |ctx| {
                    egui::CentralPanel::default().show(ctx, |ui| {
                        assert!(forms.show(ui, "settings", &s, None).is_empty());
                    });
                },
            )
        };
        let _ = render(&mut forms);
        let token = forms.active_token().unwrap().to_owned();
        forms.set_projection_for(&token, "ui.settings", &json!({"approval_bypass":false,"web_search_enabled":true,"brave_key_set":false,"workers":2,"notify_digest_interval_min":10}));
        let original = forms.editors[&editor_key("settings", None)].params.clone();
        let _ = render(&mut forms);
        let output = render(&mut forms);
        let labels = output
            .shapes
            .iter()
            .filter_map(|shape| {
                if let egui::epaint::Shape::Text(text) = &shape.shape {
                    Some(text.galley.text())
                } else {
                    None
                }
            })
            .collect::<Vec<_>>();
        for label in [
            "일반 설정",
            "일반",
            "연결",
            "에이전트",
            "고급 설정",
            "Brave 검색 키 저장 여부: 등록되지 않음",
        ] {
            assert!(
                labels.contains(&label),
                "Missing settings hierarchy/status: {label}"
            );
        }
        assert!(!labels.iter().any(|label| label.contains(": false")));
        assert_eq!(
            forms.editors[&editor_key("settings", None)].params,
            original
        );
        assert_eq!(original["patch"]["approval_bypass"], false);
        assert_eq!(original["patch"]["brave_key_set"], false);
    }
    #[test]
    fn saved_model_test_is_token_owned_and_preserves_draft_secret_and_pinned_versions() {
        let (s, id) = fixture();
        let mut e = Editor::new("model-test-owner".into(), "llm_profiles", Some(&id), &s).unwrap();
        e.params["p"]["model"] = json!("unsaved-model-must-not-be-sent");
        e.secrets
            .insert("private-buffer".into(), "not-a-saved-key".into());
        let params = e.params.clone();
        let versions = e.expected.clone();
        let secrets = e.secrets.clone();
        let saved = s.get("llm_profiles", &id).unwrap();
        let allowed = Forms::all_known_operations().into_iter().collect();
        assert!(e.can_test_saved_profile(&s, &allowed, false));
        assert!(!e.can_test_saved_profile(&s, &allowed, true));
        assert!(!e.can_test_saved_profile(&s, &BTreeSet::new(), false));
        e.pending_secrets.insert("pending-key".into());
        assert!(!e.can_test_saved_profile(&s, &allowed, false));
        e.pending_secrets.clear();
        let mut out = vec![];
        e.submit(
            "ui.testLLM",
            saved_model_test_params(saved),
            &allowed,
            &mut out,
        );
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].params["model"], saved.data["model"]);
        assert_eq!(out[0].params["api_key"], saved.data["api_key"]);
        assert_eq!(out[0].expected, versions);
        assert!(e.busy && e.model_test_pending);
        assert!(!e.can_test_saved_profile(&s, &allowed, false));
        let mut forms = Forms::default();
        forms.editors.insert("owner".into(), e);
        forms.set_model_test_result_for("wrong-token", &json!({"ok":true}));
        assert!(forms.editors["owner"].busy);
        forms.set_model_test_result_for("model-test-owner", &json!({"ok":true,"latency_ms":10}));
        let e = &forms.editors["owner"];
        assert!(!e.busy && !e.model_test_pending);
        assert_eq!(e.params, params);
        assert_eq!(e.expected, versions);
        assert_eq!(e.secrets, secrets);
        assert_eq!(e.projections["ui.testLLM"]["ok"], true);
        forms.editors.get_mut("owner").unwrap().busy = true;
        forms.set_model_test_error_for("model-test-owner", "late", "duplicate completion");
        assert!(
            forms.editors["owner"].busy,
            "A duplicate test completion must not unlock another save"
        );
        forms.editors.get_mut("owner").unwrap().model_test_pending = true;
        forms.retire_async_operations();
        forms.set_model_test_result_for("model-test-owner", &json!({"ok":false}));
        assert_eq!(forms.editors["owner"].projections["ui.testLLM"]["ok"], true);
        assert_eq!(forms.editors["owner"].params, params);
    }
    #[test]
    fn saved_model_form_keeps_label_and_input_horizontal_without_rebasing_private_draft() {
        for (viewport, central) in [(900.0, 420.0), (1320.0, 820.0)] {
            let (snapshot, id) = fixture();
            let ctx = egui::Context::default();
            crate::app::configure(&ctx);
            let mut forms = Forms::default();
            forms.set_allowed_operations(Forms::all_known_operations());
            let mut frame = || {
                ctx.run(
                    egui::RawInput {
                        screen_rect: Some(egui::Rect::from_min_size(
                            egui::Pos2::ZERO,
                            egui::vec2(viewport, 2200.0),
                        )),
                        ..Default::default()
                    },
                    |ctx| {
                        egui::CentralPanel::default().show(ctx, |ui| {
                            ui.set_width(central);
                            let out = forms.show(ui, "llm_profiles", &snapshot, Some(&id));
                            assert!(out.is_empty());
                        });
                    },
                )
            };
            let _ = frame();
            drop(frame);
            let editor = forms
                .editors
                .get_mut(&editor_key("llm_profiles", Some(&id)))
                .unwrap();
            editor.params["p"]["model"] = json!("unsaved-draft-model");
            editor.secrets.insert(
                "private-input".into(),
                "secret-ui-buffer-never-persisted".into(),
            );
            let original = editor.params.clone();
            let versions = editor.expected.clone();
            let secrets = editor.secrets.clone();
            let output = ctx.run(
                egui::RawInput {
                    screen_rect: Some(egui::Rect::from_min_size(
                        egui::Pos2::ZERO,
                        egui::vec2(viewport, 2200.0),
                    )),
                    ..Default::default()
                },
                |ctx| {
                    egui::CentralPanel::default().show(ctx, |ui| {
                        ui.set_width(central);
                        assert!(
                            forms
                                .show(ui, "llm_profiles", &snapshot, Some(&id))
                                .is_empty()
                        );
                    });
                },
            );
            let rect = |label: &str| {
                output
                    .shapes
                    .iter()
                    .find_map(|shape| {
                        if let egui::epaint::Shape::Text(text) = &shape.shape {
                            (text.galley.text() == label).then(|| text.visual_bounding_rect())
                        } else {
                            None
                        }
                    })
                    .unwrap_or_else(|| panic!("Missing {label}"))
            };
            let label = rect("모델");
            let value = rect("unsaved-draft-model");
            assert!(
                value.left() > label.right(),
                "Scalar field collapsed vertically at {viewport}px viewport"
            );
            assert!((value.center().y - label.center().y).abs() < 10.0);
            assert!(value.right() <= central + 16.0);
            assert!(!output.shapes.iter().any(|s|matches!(&s.shape,egui::epaint::Shape::Text(t) if t.galley.text().contains("secret-ui-buffer-never-persisted"))));
            let editor = &forms.editors[&editor_key("llm_profiles", Some(&id))];
            assert_eq!(editor.params, original);
            assert_eq!(editor.expected, versions);
            assert_eq!(editor.secrets, secrets);
        }
    }
    #[test]
    fn optional_profile_change_preserves_secret_and_unedited_source_fields() {
        let (s, id) = fixture();
        let mut editor = Editor::new("form1".into(), "llm_profiles", Some(&id), &s).unwrap();
        let before = editor.params.clone();
        editor.params["p"]["streaming"] = json!(false);
        let mut out = vec![];
        editor.submit(
            "ui.saveLLMProfile",
            editor.params.clone(),
            &Forms::all_known_operations().into_iter().collect(),
            &mut out,
        );
        assert_eq!(out.len(), 1, "{:?}", editor.error);
        assert_eq!(out[0].params["p"]["api_key"], before["p"]["api_key"]);
        assert_eq!(out[0].params["p"]["model"], before["p"]["model"]);
        assert_eq!(out[0].expected[0].version, "1");
    }
    #[test]
    fn snapshot_refresh_never_rebases_draft_and_conflict_requires_reload() {
        let (mut s, id) = fixture();
        let mut editor = Editor::new("form1".into(), "llm_profiles", Some(&id), &s).unwrap();
        editor.params["p"]["name"] = json!("Unsaved draft");
        s.collections
            .get_mut("llm_profiles")
            .unwrap()
            .get_mut(&id)
            .unwrap()
            .header
            .version = "2".into();
        editor.refresh_conflict(&s);
        let mut out = vec![];
        editor.submit(
            "ui.saveLLMProfile",
            editor.params.clone(),
            &Forms::all_known_operations().into_iter().collect(),
            &mut out,
        );
        assert!(out.is_empty());
        assert!(editor.conflict);
        assert_eq!(editor.params["p"]["name"], "Unsaved draft");
        assert_eq!(editor.expected[0].version, "1");
        let reloaded = Editor::new("form2".into(), "llm_profiles", Some(&id), &s).unwrap();
        assert_eq!(reloaded.expected[0].version, "2");
        assert_eq!(reloaded.params["p"]["name"], "Local model");
    }
    #[test]
    fn activation_after_save_uses_latest_saved_revision_and_preserves_conflicted_draft() {
        let (mut s, id) = fixture();
        let token = "profile-editor";
        let key = editor_key("llm_profiles", Some(&id));
        let mut forms = Forms::default();
        let mut editor = Editor::new(token.into(), "llm_profiles", Some(&id), &s).unwrap();
        editor.params["p"]["name"] = json!("Unsubmitted draft name");
        editor.params["p"]["model"] = json!("unsubmitted-draft-model");
        editor
            .secrets
            .insert("draft-key-field".into(), "unsubmitted-secret".into());
        editor.busy = true;
        forms.editors.insert(key.clone(), editor);
        forms.acknowledge_saved_for(token);
        let saved = s
            .collections
            .get_mut("llm_profiles")
            .unwrap()
            .get_mut(&id)
            .unwrap();
        saved.header.version = "2".into();
        saved.data["name"] = json!("Confirmed stored name");
        saved.data["model"] = json!("confirmed-stored-model");
        let mut previous_default = saved.clone();
        let default_id = format!("profile_{}", "c".repeat(32));
        previous_default.header.id = default_id.clone();
        previous_default.header.version = "7".into();
        previous_default.data["id"] = json!(default_id);
        previous_default.data["is_default"] = json!(true);
        s.collections
            .get_mut("llm_profiles")
            .unwrap()
            .insert(default_id.clone(), previous_default);

        let editor = forms.editors.get_mut(&key).unwrap();
        assert!(editor.conflict && !editor.busy);
        let draft = editor.params.clone();
        let draft_baseline = editor.expected.clone();
        let allowed = BTreeSet::from(["ui.activateLLMProfile".into()]);
        assert!(editor.can_activate_saved_profile(&s, &allowed));
        let mut out = vec![];
        editor.activate_saved_profile(&s, &allowed, &mut out);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].operation, "ui.activateLLMProfile");
        assert_eq!(out[0].params, json!({"id":id}));
        assert!(
            out[0]
                .expected
                .iter()
                .any(|v| v.id == id && v.version == "2")
        );
        assert!(
            out[0]
                .expected
                .iter()
                .any(|v| v.id == default_id && v.version == "7")
        );
        assert_eq!(editor.params, draft);
        assert_eq!(editor.expected, draft_baseline);
        assert_eq!(editor.secrets["draft-key-field"], "unsubmitted-secret");
        assert!(editor.conflict && editor.busy);
        editor.activate_saved_profile(&s, &allowed, &mut out);
        assert_eq!(
            out.len(),
            1,
            "activation must not duplicate an in-flight request"
        );
    }
    #[test]
    fn saved_profile_activation_requires_permission_idle_state_and_saved_configuration() {
        let (mut s, id) = fixture();
        let mut editor =
            Editor::new("profile-editor".into(), "llm_profiles", Some(&id), &s).unwrap();
        let allowed = BTreeSet::from(["ui.activateLLMProfile".into()]);
        assert!(!editor.can_activate_saved_profile(&s, &BTreeSet::new()));
        editor.busy = true;
        assert!(!editor.can_activate_saved_profile(&s, &allowed));
        editor.busy = false;
        editor.pending_secrets.insert("api_key".into());
        assert!(!editor.can_activate_saved_profile(&s, &allowed));
        editor.pending_secrets.clear();
        editor.pending_reads.insert("saved_read".into());
        assert!(!editor.can_activate_saved_profile(&s, &allowed));
        editor.pending_reads.clear();
        for (field, invalid) in [
            ("model", json!(" ")),
            ("format", json!("unsupported")),
            ("api_key", Value::Null),
            ("is_default", json!(true)),
        ] {
            let saved = s
                .collections
                .get_mut("llm_profiles")
                .unwrap()
                .get_mut(&id)
                .unwrap();
            let original = saved.data[field].clone();
            saved.data[field] = invalid;
            let mut out = vec![];
            editor.activate_saved_profile(&s, &allowed, &mut out);
            assert!(out.is_empty(), "invalid saved {field} should not activate");
            s.collections
                .get_mut("llm_profiles")
                .unwrap()
                .get_mut(&id)
                .unwrap()
                .data[field] = original;
        }
        assert!(editor.can_activate_saved_profile(&s, &allowed));
    }
    #[test]
    fn secret_callback_requires_pending_path_and_original_form_token() {
        let (s, id) = fixture();
        let mut forms = Forms::default();
        let mut editor = Editor::new("form1".into(), "llm_profiles", Some(&id), &s).unwrap();
        let path: Vec<String> = vec!["p".into(), "api_key".into()];
        editor
            .pending_secrets
            .insert(serde_json::to_string(&path).unwrap());
        editor
            .secrets
            .insert("input".into(), "private-ui-memory".into());
        let old = editor.params["p"]["api_key"].clone();
        forms.editors.insert("current".into(), editor);
        let new_ref = json!({"secret_ref":format!("secret_{}","c".repeat(32)),"hint":"new key"});
        let wrong = json!({"form_token":"form0","collection":"llm_profiles","id":id,"path":path})
            .to_string();
        forms.set_secret_reference(&wrong, new_ref.clone());
        assert_eq!(forms.editors["current"].params["p"]["api_key"], old);
        let correct = json!({"form_token":"form1","collection":"llm_profiles","id":id,"path":path})
            .to_string();
        forms.set_secret_reference(&correct, json!("plaintext"));
        assert_eq!(forms.editors["current"].params["p"]["api_key"], old);
        forms.set_secret_reference(&correct, new_ref.clone());
        assert_eq!(forms.editors["current"].params["p"]["api_key"], new_ref);
        assert!(
            !forms.editors["current"]
                .params
                .to_string()
                .contains("private-ui-memory")
        );
    }
    #[test]
    fn schema_editor_preserves_opaque_exec_headers_and_custom_parameters() {
        let params = json!({"t":{"key":"custom","description":"fixture","kind":"http","enabled":true,"schema":{"type":"object","properties":{"target":{"type":"string","default":"example.test"}},"required":["target"]},"agents":[],"exec":{"url":"https://example.test","headers":{"Authorization":{"secret_ref":format!("secret_{}","d".repeat(32)),"hint":"header"}}}}});
        let shaped = schema::shape(&params, &schema::method("ui.createCustomTool"));
        assert_eq!(shaped["t"]["schema"], params["t"]["schema"]);
        assert_eq!(shaped["t"]["exec"], params["t"]["exec"]);
    }
    #[test]
    fn new_profile_never_submits_empty_optional_typed_identity() {
        let (s, _) = fixture();
        let (_, p) = spec::initial("llm_profiles", None, &s).unwrap();
        assert!(p["p"].get("id").is_none());
    }
    #[test]
    fn large_skill_body_remains_original_until_writer_externalization() {
        let (s, _) = fixture();
        let mut e = Editor::new("form1".into(), "skills", None, &s).unwrap();
        e.params["s"]["name"] = json!("test-skill");
        e.params["s"]["description"] = json!("fixture");
        let text = "x".repeat(70_000);
        e.params["s"]["instructions"] = json!({"text":text,"blob":null});
        let mut out = vec![];
        e.submit(
            "ui.createSkill",
            e.params.clone(),
            &Forms::all_known_operations().into_iter().collect(),
            &mut out,
        );
        assert_eq!(out.len(), 1, "{:?}", e.error);
        assert_eq!(
            out[0].params["s"]["instructions"]["text"]
                .as_str()
                .unwrap()
                .len(),
            70_000
        );
        assert!(out[0].params["s"]["instructions"]["blob"].is_null());
    }
    #[test]
    fn skill_metadata_uses_pinned_filesystem_revision_and_rejects_late_read() {
        let (s, _) = fixture();
        let mut forms = Forms::default();
        let mut e = Editor::new("form1".into(), "skills", Some("fixture"), &s).unwrap();
        e.requested.insert("native.skill.meta.read".into());
        forms.editors.insert("current".into(), e);
        let reply = json!({"name":"fixture","meta":{"description":"source","license":"MIT","mcps":[]},"sha256":"a".repeat(64)});
        forms.set_projection_for("retired", "native.skill.meta.read", &reply);
        assert!(forms.editors["current"].metadata_revision.is_none());
        forms.set_projection_for("form1", "native.skill.meta.read", &reply);
        let e = forms.editors.get_mut("current").unwrap();
        e.params["meta"]["description"] = json!("draft");
        let mut out = vec![];
        e.submit(
            "ui.updateSkillMeta",
            e.params.clone(),
            &Forms::all_known_operations().into_iter().collect(),
            &mut out,
        );
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].operation, "native.skill.meta.write");
        assert_eq!(out[0].params["expected_sha256"], "a".repeat(64));
        assert_eq!(out[0].params["meta"]["license"], "MIT");
    }
}

#[cfg(test)]
mod retirement_tests {
    use super::*;
    fn editor(token: &str) -> Editor {
        let s = Snapshot {
            workspace_id: "workspace-fixture".into(),
            commit_seq: 1,
            collections: BTreeMap::new(),
        };
        let mut e = Editor::new(token.into(), "llm_profiles", None, &s).unwrap();
        e.selected = Some("profile-fixture".into());
        e.params["p"]["name"] = json!("unsaved profile");
        e.expected = vec![Expected {
            collection: "llm_profiles".into(),
            id: "profile-fixture".into(),
            version: "1".into(),
        }];
        e
    }
    #[test]
    fn saved_and_failed_hidden_editor_are_acknowledged_by_original_token() {
        let mut f = Forms::default();
        let mut a = editor("A-token");
        a.busy = true;
        let mut b = editor("B-token");
        b.busy = true;
        f.editors.insert("A".into(), a);
        f.editors.insert("B".into(), b);
        f.active = Some("B".into());
        f.acknowledge_saved_for("A-token");
        assert!(!f.editors["A"].busy);
        assert!(f.editors["B"].busy);
        assert_eq!(f.editors["A"].params["p"]["name"], "unsaved profile");
        f.editors.get_mut("A").unwrap().busy = true;
        f.set_error_for("A-token", "VERSION_CONFLICT", "changed");
        assert!(!f.editors["A"].busy);
        assert!(f.editors["A"].conflict);
        assert!(f.editors["B"].busy);
    }
    #[test]
    fn suspension_preserves_params_expected_sha_and_private_ui_buffers() {
        let mut f = Forms::default();
        let mut e = editor("A-token");
        e.busy = true;
        e.file_content = "unsaved skill".into();
        e.file_revision = Some(json!("a".repeat(64)));
        e.secrets
            .insert("draft-secret".into(), "UI-memory-only".into());
        e.pending_reads.insert("read-token".into());
        let params = e.params.clone();
        f.editors.insert("A".into(), e);
        f.retire_async_operations();
        let e = &f.editors["A"];
        assert_eq!(e.params, params);
        assert_eq!(e.expected[0].version, "1");
        assert_eq!(e.file_content, "unsaved skill");
        assert_eq!(e.file_revision, Some(json!("a".repeat(64))));
        assert_eq!(e.secrets["draft-secret"], "UI-memory-only");
        assert!(e.conflict && !e.busy && e.pending_reads.is_empty());
    }
    #[test]
    fn skill_save_acknowledges_submitted_text_without_replacing_newer_draft() {
        let mut f = Forms::default();
        let mut e = editor("A-token");
        e.file = "notes.txt".into();
        e.file_token = 3;
        e.busy = true;
        e.file_original = "old file".into();
        e.file_content = "newer unsaved text".into();
        f.editors.insert("A".into(), e);
        f.set_skill_file_saved_content("A-token", 2, &"a".repeat(64), "submitted text");
        assert!(f.editors["A"].busy);
        f.set_skill_file_saved_content("A-token", 3, &"b".repeat(64), "submitted text");
        let e = &f.editors["A"];
        assert!(!e.busy);
        assert_eq!(e.file_original, "submitted text");
        assert_eq!(e.file_content, "newer unsaved text");
        assert_ne!(e.file_original, e.file_content);
        assert_eq!(e.file_revision, Some(json!("b".repeat(64))));
    }
}
