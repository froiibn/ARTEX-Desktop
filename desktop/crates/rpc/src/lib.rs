#![forbid(unsafe_code)]
mod activation;
#[cfg(test)]
mod activity_contract_tests;
mod archives;
pub mod bridge;
mod compaction_runtime;
mod compression;
mod controller;
pub mod facade;
mod log_runtime;
mod model_records;
pub mod native;
mod native_lifecycle;
#[cfg(test)]
mod native_lifecycle_tests;
mod network_ui;
mod notifications;
mod policy_runtime;
#[cfg(test)]
mod prompt_regression;
mod prompt_tails;
#[cfg(test)]
mod regression;
#[cfg(test)]
mod result_contract_tests;
#[cfg(test)]
mod runtime_tool_tests;
mod runtime_tools;
mod scopesentry;
mod side_runtime;
mod skill_runtime;
mod stream_runtime;
mod task_delete_runtime;
mod traffic_runtime;

use artex_core::{
    Actor, CommandBatch, Context, Error, Expected, Result, Snapshot, contracts, new_id, parse_u64,
};
use artex_store::Store;
use base64::{
    Engine as _,
    engine::general_purpose::{STANDARD, URL_SAFE_NO_PAD},
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{
    collections::{BTreeMap, HashMap, HashSet},
    path::PathBuf,
    time::Instant,
};

pub const MAX_FRAME: usize = 1_048_576;
pub const MAX_CHUNK: usize = 65_536;
pub const MAX_QUEUE_BYTES: usize = 16_777_216;

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Request {
    pub jsonrpc: String,
    pub protocol: String,
    pub id: String,
    pub method: String,
    pub workspace_id: Option<String>,
    pub engine_epoch: Option<String>,
    pub command_id: Option<String>,
    pub expected: Vec<Expected>,
    pub timeout_ms: u64,
    pub params: Value,
}
/// Application operation shared by the library and the legacy wire adapter.
/// Transport versions and handshake fields never enter the application kernel.
#[derive(Clone, Debug)]
pub struct DispatchRequest {
    pub id: String,
    pub method: String,
    pub workspace_id: Option<String>,
    pub engine_epoch: Option<String>,
    pub command_id: Option<String>,
    pub expected: Vec<Expected>,
    pub timeout_ms: u64,
    pub params: Value,
}
impl From<Request> for DispatchRequest {
    fn from(r: Request) -> Self {
        Self {
            id: r.id,
            method: r.method,
            workspace_id: r.workspace_id,
            engine_epoch: r.engine_epoch,
            command_id: r.command_id,
            expected: r.expected,
            timeout_ms: r.timeout_ms,
            params: r.params,
        }
    }
}
struct Transfer {
    length: u64,
    sha256: String,
    media_type: String,
    bytes: Vec<u8>,
}
struct Subscription {
    after: u64,
    kinds: HashSet<String>,
}
#[derive(Serialize, Deserialize)]
struct Cursor {
    workspace: String,
    collection: String,
    read_seq: String,
    filter_hash: String,
    last_sort_key: String,
    epoch: String,
}

pub struct Router {
    pub engine_epoch: String,
    workspace_generation: String,
    pub shutting_down: bool,
    store: Option<Store>,
    read_only: bool,
    host_epoch: Option<String>,
    capabilities: BTreeMap<String, PathBuf>,
    contract: Value,
    validators: HashMap<String, jsonschema::Validator>,
    seen_ids: HashSet<String>,
    transfers: HashMap<String, Transfer>,
    subscriptions: BTreeMap<String, Subscription>,
    started: Instant,
    runtime: controller::Controller,
    gateway: Option<std::sync::Arc<dyn artex_runtime::HostGateway>>,
    execution_capability: Option<String>,
    resource_capability: Option<String>,
    blob_leases: HashMap<String, (Value, Instant)>,
    ephemeral_blobs: HashMap<String, (std::sync::Arc<Vec<u8>>, Instant)>,
    notification_work: Option<notifications::PendingSend>,
    archive_work: Option<archives::PendingArchive>,
    traffic: traffic_runtime::TrafficState,
    deleting_tasks: std::collections::BTreeSet<String>,
    lifecycle: native_lifecycle::LifecycleState,
    deferred_completions: std::collections::VecDeque<controller::Completion>,
    logs: log_runtime::LogState,
    side: side_runtime::SideState,
    stream_state: stream_runtime::StreamState,
    native_mode: bool,
    native_completions: std::collections::VecDeque<native::NativeCompletion>,
}
impl Router {
    pub fn new(capabilities: BTreeMap<String, PathBuf>) -> Self {
        Self {
            engine_epoch: new_id("epoch"),
            workspace_generation: new_id("open"),
            shutting_down: false,
            store: None,
            read_only: false,
            host_epoch: None,
            capabilities,
            contract: contracts::read("rpc.v1.schema.json"),
            validators: HashMap::new(),
            seen_ids: HashSet::new(),
            transfers: HashMap::new(),
            subscriptions: BTreeMap::new(),
            started: Instant::now(),
            runtime: controller::Controller::default(),
            gateway: None,
            execution_capability: None,
            resource_capability: None,
            blob_leases: HashMap::new(),
            ephemeral_blobs: HashMap::new(),
            notification_work: None,
            archive_work: None,
            traffic: Default::default(),
            deleting_tasks: Default::default(),
            lifecycle: Default::default(),
            deferred_completions: Default::default(),
            logs: Default::default(),
            side: Default::default(),
            stream_state: Default::default(),
            native_mode: false,
            native_completions: Default::default(),
        }
    }
    pub fn snapshot(&self) -> Result<std::sync::Arc<artex_store::ReadGeneration>> {
        Ok(self
            .store
            .as_ref()
            .ok_or_else(|| Error::new("WORKSPACE_NOT_OPEN", "Open a workspace first"))?
            .read_generation())
    }
    pub fn maintain(&mut self) -> Result<()> {
        self.read_only = self.effective_read_only();
        if !self.read_only && self.deleting_tasks.is_empty() && !self.native_lifecycle_busy() {
            if let Some(store) = self.store.as_mut() {
                if let Some(batch) = artex_agent::maintenance_plan(
                    store.read_generation().snapshot.as_ref(),
                    &self.engine_epoch,
                ) {
                    store.commit(batch)?;
                }
            }
        }
        self.runtime_tick()?;
        if self.native_lifecycle_busy() {
            return Ok(());
        }
        if self.store.is_some() && !self.read_only {
            self.process_archive_jobs()?;
        }
        if self.store.is_some() && !self.read_only {
            self.process_notification_jobs()?;
        }
        if self.store.is_some() && !self.read_only {
            self.process_traffic()?;
        }
        if self.store.is_some() && !self.read_only {
            self.process_logs()?;
        }
        if self.store.is_some() && !self.read_only {
            self.process_side_questions()?;
        }
        Ok(())
    }
    pub fn attach_host(&mut self, gateway: std::sync::Arc<dyn artex_runtime::HostGateway>) {
        self.gateway = Some(gateway);
    }
    pub fn set_execution_capabilities(
        &mut self,
        execution: Option<String>,
        resources: Option<String>,
    ) -> Result<()> {
        for token in execution.iter().chain(resources.iter()) {
            if !self.capabilities.contains_key(token) {
                return Err(Error::new(
                    "PERMISSION_DENIED",
                    "Unknown execution capability",
                ));
            }
        }
        self.execution_capability = execution;
        self.resource_capability = resources;
        Ok(())
    }
    pub fn response(&mut self, value: Value) -> Value {
        self.response_as(value, None)
    }
    /// Only the in-process run controller may supply a context; the host wire cannot.
    pub fn response_as(&mut self, value: Value, context: Option<&Context>) -> Value {
        self.read_only = self.effective_read_only();
        let id = value["id"].as_str().unwrap_or("").to_owned();
        let outcome = self.handle(&value, context);
        self.read_only = self.effective_read_only();
        let seq = self
            .store
            .as_ref()
            .map(|s| s.read_generation().snapshot.commit_seq)
            .unwrap_or(0)
            .to_string();
        let response = match outcome {
            Ok(result) => {
                if result["__deferred"] == true {
                    return Value::Null;
                }
                let versions = self.response_versions(
                    value["method"].as_str().unwrap_or(""),
                    &value["params"],
                    &result,
                );
                json!({"jsonrpc":"2.0","id":id,"engine_epoch":self.engine_epoch,"read_seq":seq,"read_versions":versions,"result":result})
            }
            Err(err) => self.error_response(&id, err),
        };
        if serde_json::to_vec(&response).map_or(true, |b| b.len() > MAX_FRAME) {
            self.error_response(
                &id,
                Error::new(
                    "SIZE_LIMIT",
                    "Result exceeds RPC frame; query smaller pages or command.status",
                ),
            )
        } else {
            response
        }
    }
    fn response_versions(&self, method: &str, params: &Value, result: &Value) -> Vec<Expected> {
        fn strings(v: &Value, items: &mut HashSet<String>) {
            match v {
                Value::String(s) => {
                    items.insert(s.clone());
                }
                Value::Array(a) => {
                    for v in a {
                        strings(v, items);
                    }
                }
                Value::Object(o) => {
                    for v in o.values() {
                        strings(v, items);
                    }
                }
                _ => {}
            }
        }
        let Some(store) = self.store.as_ref() else {
            return vec![];
        };
        let generation = store.read_generation();
        let snapshot = &generation.snapshot;
        let mut keys = HashSet::new();
        strings(params, &mut keys);
        strings(result, &mut keys);
        // Settings DTOs expose values rather than record IDs. Bind their editor
        // provenance to the records read, including false/numeric values.
        if matches!(method, "ui.settings" | "ui.setSettings") {
            for record in snapshot.records("settings") {
                keys.insert(record.header.id.clone());
            }
        }
        match method {
            "ui.llmRetryPolicy" | "ui.saveLLMRetryPolicy" => {
                keys.insert("llm_retry_policy".into());
            }
            "ui.notifyMeta" => {
                keys.extend(
                    [
                        "notify_enabled",
                        "notify_public_base_url",
                        "notify_digest_interval_min",
                    ]
                    .map(str::to_owned),
                );
            }
            "ui.interceptGetToolConfig" | "ui.interceptSetToolConfig" => {
                keys.insert("intercept_enabled_tools".into());
            }
            "ui.interceptGetJudgeConfig" | "ui.interceptSetJudgeConfig" => {
                keys.extend(
                    [
                        "llm_judge_enabled",
                        "llm_judge_profile_id",
                        "llm_judge_prompt",
                        "llm_judge_timeout_seconds",
                        "llm_judge_fail_action",
                        "llm_judge_ask_timeout_seconds",
                        "llm_judge_ask_timeout_action",
                    ]
                    .map(str::to_owned),
                );
            }
            _ => {}
        }
        let mut found = BTreeMap::new();
        let mut references = HashSet::new();
        for collection in snapshot.collections.keys() {
            for key in &keys {
                let direct = snapshot.get(collection, key).into_iter();
                let indexed = generation
                    .indexes
                    .ids(collection, "key", &json!(key))
                    .into_iter()
                    .flatten()
                    .filter_map(|id| snapshot.get(collection, id));
                for record in direct.chain(indexed) {
                    found.insert(
                        (collection.clone(), record.header.id.clone()),
                        record.header.version.clone(),
                    );
                    strings(&record.data, &mut references);
                }
            }
        }
        for collection in snapshot.collections.keys() {
            for id in &references {
                if let Some(record) = snapshot.get(collection, id) {
                    found.insert(
                        (collection.clone(), record.header.id.clone()),
                        record.header.version.clone(),
                    );
                }
            }
        }
        found
            .into_iter()
            .map(|((collection, id), version)| Expected {
                collection,
                id,
                version,
            })
            .collect()
    }
    fn error_response(&self, id: &str, error: Error) -> Value {
        let numeric = self.contract["error_codes"][&error.code]
            .as_i64()
            .unwrap_or(-32018);
        let mut data = json!({"code":error.code,"retryable":error.retryable,"recovery_required":error.recovery_required});
        if error.details.is_object() {
            data["details"] = error.details;
        }
        json!({"jsonrpc":"2.0","id":id,"engine_epoch":self.engine_epoch,"error":{"code":numeric,"message":error.message,"data":data}})
    }
    fn validate(&mut self, key: &str, schema: &Value, value: &Value) -> Result<()> {
        if !self.validators.contains_key(key) {
            let mut full = schema.clone();
            full["$defs"] = self.contract["$defs"].clone();
            full["$schema"] = json!("https://json-schema.org/draft/2020-12/schema");
            let validator = jsonschema::options()
                .with_draft(jsonschema::Draft::Draft202012)
                .should_validate_formats(true)
                .build(&full)
                .map_err(|_| Error::new("INVALID_REQUEST", "Invalid embedded RPC schema"))?;
            self.validators.insert(key.into(), validator);
        }
        if !self.validators[key].is_valid(value) {
            let schema_paths: Vec<String> = self.validators[key]
                .iter_errors(value)
                .take(8)
                .map(|violation| violation.schema_path.to_string())
                .collect();
            let mut error = Error::new("INVALID_PARAMS", "RPC value does not match its v1 schema");
            // Schema locations contain only embedded contract names. Never
            // format validation errors: their display includes result values.
            error.details = json!({"schema_paths":schema_paths});
            return Err(error);
        }
        check_numeric(schema, &self.contract["$defs"], value)?;
        Ok(())
    }
    fn handle(&mut self, value: &Value, provided: Option<&Context>) -> Result<Value> {
        if self.shutting_down {
            return Err(Error::new("CANCELLED", "Engine is shutting down"));
        }
        let request: Request = serde_json::from_value(value.clone())
            .map_err(|_| Error::new("INVALID_REQUEST", "RPC envelope is invalid"))?;
        if request.jsonrpc != "2.0"
            || request.protocol != "artex-rpc/1.0"
            || request.id.is_empty()
            || request.id.chars().count() > 128
            || !(1..=3_600_000).contains(&request.timeout_ms)
        {
            return Err(Error::new(
                "INVALID_REQUEST",
                "RPC envelope version, ID, or timeout is invalid",
            ));
        }
        if self.seen_ids.len() >= 1_000_000 {
            return Err(Error::new(
                "BUSY",
                "Reconnect to establish a new request epoch",
            ));
        }
        if !self.seen_ids.insert(request.id.clone()) {
            return Err(Error::new(
                "INVALID_REQUEST",
                "Request ID was reused within the connection",
            ));
        }
        let schema = self.contract["methods"][&request.method].clone();
        if schema.is_null() {
            return Err(Error::new("METHOD_NOT_FOUND", "Unknown RPC method"));
        }
        let envelope = self.contract["$defs"]["Request"].clone();
        self.validate("envelope", &envelope, value)?;
        self.validate(&request.method, &schema, &request.params)?;
        if self.native_lifecycle_busy()
            && (request.command_id.is_some()
                || matches!(
                    request.method.as_str(),
                    "workspace.close" | "system.shutdown"
                ))
        {
            return Err(Error::new(
                "BUSY",
                "Execution cleanup is in progress; wait for confirmation",
            ));
        }
        if request.method != "system.hello" && self.host_epoch.is_none() {
            return Err(Error::new(
                "VERSION_MISMATCH",
                "system.hello must be called first",
            ));
        }
        if request.method == "system.hello" {
            if self.host_epoch.is_some() {
                return Err(Error::new("INVALID_REQUEST", "Hello has already completed"));
            }
            if !request.params["versions"]
                .as_array()
                .is_some_and(|a| a.iter().any(|v| v == "artex-rpc/1.0"))
            {
                return Err(Error::new(
                    "VERSION_MISMATCH",
                    "No supported RPC protocol version",
                ));
            }
            self.host_epoch = request.params["host_epoch"].as_str().map(str::to_owned);
            return Ok(
                json!({"protocol":"artex-rpc/1.0","engine_epoch":self.engine_epoch,"engine_version":env!("CARGO_PKG_VERSION"),"store_format":1,"max_frame_bytes":MAX_FRAME,"max_blob_chunk":MAX_CHUNK,"read_only":self.read_only,"workspace_id":self.store.as_ref().map(|s|s.read_generation().snapshot.workspace_id.clone()),"commit_seq":self.store.as_ref().map(|s|s.read_generation().snapshot.commit_seq).unwrap_or(0).to_string(),"capabilities":["object_store","checkpoint","blob_transfer","domain_tools","durable_agent_messages"]}),
            );
        }
        self.dispatch_request(&request.into(), provided)
    }
    /// Shared application dispatch, entered without an RPC envelope by native.
    fn dispatch_request(
        &mut self,
        request: &DispatchRequest,
        provided: Option<&Context>,
    ) -> Result<Value> {
        if self.shutting_down {
            return Err(Error::new("CANCELLED", "Application is shutting down"));
        }
        self.read_only = self.effective_read_only();
        if self.native_lifecycle_busy()
            && (request.command_id.is_some()
                || matches!(
                    request.method.as_str(),
                    "workspace.close" | "system.shutdown"
                ))
        {
            return Err(Error::new(
                "BUSY",
                "Execution cleanup is in progress; wait for confirmation",
            ));
        }
        let schema = self.contract["methods"][&request.method].clone();
        if schema.is_null() {
            return Err(Error::new(
                "METHOD_NOT_FOUND",
                "Unknown application operation",
            ));
        }
        self.validate(&request.method, &schema, &request.params)?;
        let setup = matches!(
            request.method.as_str(),
            "workspace.create" | "workspace.open" | "system.ping" | "system.shutdown"
        );
        if !setup {
            let state = self.snapshot()?;
            if request.workspace_id.as_deref() != Some(&state.workspace_id) {
                return Err(Error::new(
                    "WORKSPACE_NOT_OPEN",
                    "Request does not match the open workspace",
                ));
            }
            if request.engine_epoch.as_deref() != Some(&self.engine_epoch) {
                return Err(Error::new("STALE_EPOCH", "Engine epoch changed"));
            }
        }
        let context = provided.cloned().unwrap_or_else(|| Context {
            actor: Actor {
                kind: "user".into(),
                agent_id: None,
                run_id: None,
                engine_epoch: self.engine_epoch.clone(),
            },
            task_id: None,
            intent_id: None,
            conversation_id: None,
        });
        if context.actor.kind != "user" {
            if context.actor.kind != "agent"
                || !matches!(
                    request.method.as_str(),
                    "tool.invoke"
                        | "agent.send"
                        | "agent.claim"
                        | "agent.acknowledge"
                        | "agent.cancel_request"
                        | "agent.inbox"
                        | "agent.features.get"
                )
            {
                return Err(Error::new(
                    "PERMISSION_DENIED",
                    "This method requires the native user session",
                ));
            }
            artex_agent::authenticate(self.snapshot()?.as_ref(), &context)?;
        }
        let p = &request.params;
        let result = match request.method.as_str() {
            "system.ping" => {
                json!({"engine_epoch":self.engine_epoch,"commit_seq":self.store.as_ref().map(|s|s.read_generation().snapshot.commit_seq).unwrap_or(0).to_string(),"uptime_ms":self.started.elapsed().as_millis().to_string()})
            }
            "system.shutdown" => {
                self.close_workspace_orderly()?;
                self.shutting_down = true;
                json!({"accepted":true})
            }
            "workspace.create" | "workspace.open" => {
                if self.store.is_some() {
                    return Err(Error::new(
                        "VERSION_CONFLICT",
                        "Close the current workspace first",
                    ));
                }
                let token = p["directory_capability"].as_str().unwrap();
                let path = self.capabilities.get(token).ok_or_else(|| {
                    Error::new("PERMISSION_DENIED", "Unknown host directory capability")
                })?;
                let mut store = if request.method == "workspace.create" {
                    Store::create(path, p["name"].as_str().unwrap())?
                } else {
                    Store::open_with_mode(path, p["read_only"] == true)?
                };
                self.read_only = store.read_only() || p["read_only"] == true;
                if !self.read_only {
                    if let Some(recovery) = artex_agent::recovery_plan(
                        store.read_generation().snapshot.as_ref(),
                        &self.engine_epoch,
                    ) {
                        store.commit(recovery)?;
                    }
                }
                self.store = Some(store);
                self.workspace_generation = new_id("open");
                if !self.read_only {
                    let recovery = artex_domain::side_questions::recovery_plan(
                        self.snapshot()?.as_ref(),
                        &context,
                    )?;
                    if !recovery.changes.is_empty() {
                        self.internal_commit(recovery)?;
                    }
                    self.recover_streams()?;
                    let seed = artex_domain::bootstrap_plan(self.snapshot()?.as_ref(), &context)?;
                    if !seed.changes.is_empty() {
                        let mut seed = seed;
                        self.externalize_batch(&mut seed)?;
                        seed.command_id = new_id("command");
                        self.store.as_mut().unwrap().commit(seed)?;
                    }
                }
                let mut info = self.store.as_ref().unwrap().info();
                info["read_only"] = json!(self.read_only);
                info
            }
            "workspace.info" => {
                let mut i = self.store.as_ref().unwrap().info();
                i["read_only"] = json!(self.effective_read_only());
                i
            }
            "workspace.close" => {
                self.close_workspace_orderly()?;
                json!({"closed":true})
            }
            "workspace.checkpoint" => {
                self.ensure_writable()?;
                self.require_command(&request)?;
                if let Some(result) = self.repeated(&request)? {
                    return Ok(result);
                }
                let result = self.store.as_mut().unwrap().checkpoint()?;
                let mut batch = CommandBatch {
                    command_id: String::new(),
                    method: request.method.clone(),
                    normalized_params: p.clone(),
                    expected: vec![],
                    changes: vec![],
                    events: vec![],
                    deliveries: vec![],
                    result,
                };
                self.commit(&request, &mut batch)?
            }
            "workspace.recover" => {
                if p["action"] == "create_recovery_copy" {
                    let token = p["directory_capability"].as_str().ok_or_else(|| {
                        Error::new("INVALID_PARAMS", "Recovery destination capability required")
                    })?;
                    let path = self.capabilities.get(token).ok_or_else(|| {
                        Error::new(
                            "PERMISSION_DENIED",
                            "Unknown recovery destination capability",
                        )
                    })?;
                    self.store.as_ref().unwrap().export_recovery_copy(path)?;
                    let mut report = self.store.as_ref().unwrap().info()["recovery"].clone();
                    report["recovery_copy_capability"] = json!(token);
                    report
                } else {
                    self.store.as_ref().unwrap().info()["recovery"].clone()
                }
            }
            "query.objects" => self.query_objects(p)?,
            "command.prepare" => {
                let method = p["method"].as_str().unwrap();
                let input_schema = self.contract["methods"][method].clone();
                if input_schema.is_null()
                    || !(method.starts_with("ui.")
                        || artex_domain::is_mutation(method)
                        || facade::COMMANDS.contains(&method)
                        || method == "user.answer")
                {
                    return Err(Error::new(
                        "INVALID_PARAMS",
                        "Only registered user commands can be prepared",
                    ));
                }
                self.validate(&format!("prepare:{method}"), &input_schema, &p["params"])?;
                let state = self.snapshot()?;
                if method == "ui.refreshMcpServer" {
                    let id = artex_domain::required(&p["params"], "id")?;
                    let mut rows = vec![artex_domain::record(&state, "mcp_servers", id)?];
                    rows.extend(
                        state
                            .records("mcp_tools_cache")
                            .into_iter()
                            .filter(|r| r.data["server_id"] == id),
                    );
                    let expected: Vec<Expected> = rows
                        .into_iter()
                        .map(|r| Expected {
                            collection: r.header.collection.clone(),
                            id: r.header.id.clone(),
                            version: r.header.version.clone(),
                        })
                        .collect();
                    json!({"expected":expected,"read_seq":state.commit_seq.to_string()})
                } else if controller::NATIVE_UI.contains(&method)
                    || matches!(method, "ui.testLLM" | "ui.fetchLLMModels")
                {
                    json!({"expected":[],"read_seq":state.commit_seq.to_string()})
                } else if facade::COMMANDS.contains(&method) {
                    let mut batch = facade::plan(method, &p["params"], &state, &context)?;
                    artex_domain::augment_expected(&mut batch, &state)?;
                    json!({"expected":batch.expected,"read_seq":state.commit_seq.to_string()})
                } else if method == "user.answer" {
                    let mut batch = artex_agent::plan(method, &p["params"], &state, &context)?;
                    artex_domain::augment_expected(&mut batch, &state)?;
                    json!({"expected":batch.expected,"read_seq":state.commit_seq.to_string()})
                } else if artex_domain::is_mutation(method) {
                    let mut batch = artex_domain::plan(method, &p["params"], &state, &context)?;
                    artex_domain::augment_expected(&mut batch, &state)?;
                    json!({"expected":batch.expected,"read_seq":state.commit_seq.to_string()})
                } else {
                    return Err(Error::new(
                        "INVALID_PARAMS",
                        "This method is not a UI command",
                    ));
                }
            }
            "command.status" => {
                let state = self.snapshot()?;
                match state
                    .records("command_receipts")
                    .into_iter()
                    .find(|r| r.data["command_id"] == p["command_id"])
                {
                    Some(r) => {
                        json!({"state":"committed","commit_seq":r.data["commit_seq"],"result":r.data["result"],"result_ref":r.data["result_ref"]})
                    }
                    None => {
                        let prepare = format!(
                            "command_{}",
                            hex::encode(Sha256::digest(
                                format!("prepare:{}", p["command_id"].as_str().unwrap()).as_bytes()
                            ))[..32]
                                .to_owned()
                        );
                        if state
                            .records("command_receipts")
                            .into_iter()
                            .any(|r| r.data["command_id"] == prepare)
                        {
                            json!({"state":"pending"})
                        } else {
                            json!({"state":"unknown"})
                        }
                    }
                }
            }
            "rpc.cancel" => self.cancel_request(p["request_id"].as_str().unwrap()),
            "events.subscribe" => {
                let seq = parse_u64(p["after_seq"].as_str().unwrap())?;
                let state = self.snapshot()?;
                if seq > state.commit_seq {
                    return Err(Error::new(
                        "RESYNC_REQUIRED",
                        "Event cursor is ahead of the store",
                    ));
                }
                let id = new_id("subscription");
                let kinds = p["kinds"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .map(|v| v.as_str().unwrap().to_owned())
                    .collect();
                self.subscriptions
                    .insert(id.clone(), Subscription { after: seq, kinds });
                json!({"subscription_id":id,"after_seq":seq.to_string(),"high_watermark":state.commit_seq.to_string()})
            }
            "events.unsubscribe" => {
                json!({"removed":self.subscriptions.remove(p["subscription_id"].as_str().unwrap()).is_some()})
            }
            "blob.begin" => {
                self.ensure_writable()?;
                let length = parse_u64(p["length"].as_str().unwrap())?;
                let allocated = self.transfers.values().map(|v| v.length).sum::<u64>();
                if self.transfers.len() >= 16
                    || length > MAX_QUEUE_BYTES as u64
                    || allocated
                        .checked_add(length)
                        .is_none_or(|n| n > MAX_QUEUE_BYTES as u64)
                {
                    return Err(Error::new(
                        "SIZE_LIMIT",
                        "Blob transfer buffer budget exceeded",
                    ));
                }
                let id = new_id("transfer");
                self.transfers.insert(
                    id.clone(),
                    Transfer {
                        length,
                        sha256: p["sha256"].as_str().unwrap().into(),
                        media_type: p["media_type"].as_str().unwrap().into(),
                        bytes: vec![],
                    },
                );
                json!({"transfer_id":id,"next_offset":"0"})
            }
            "blob.write" => {
                self.ensure_writable()?;
                let transfer = self
                    .transfers
                    .get_mut(p["transfer_id"].as_str().unwrap())
                    .ok_or_else(|| Error::new("NOT_FOUND", "Transfer not found"))?;
                let bytes = STANDARD
                    .decode(p["data_base64"].as_str().unwrap())
                    .map_err(|_| Error::new("INVALID_PARAMS", "Invalid base64"))?;
                if bytes.len() > MAX_CHUNK {
                    return Err(Error::new("SIZE_LIMIT", "Blob chunk exceeds 64 KiB"));
                }
                if parse_u64(p["offset"].as_str().unwrap())? != transfer.bytes.len() as u64 {
                    return Err(Error::new("VERSION_CONFLICT", "Blob offset changed"));
                }
                if transfer.bytes.len() as u64 + bytes.len() as u64 > transfer.length {
                    return Err(Error::new(
                        "SIZE_LIMIT",
                        "Blob data exceeds declared length",
                    ));
                }
                transfer.bytes.extend(bytes);
                json!({"next_offset":transfer.bytes.len().to_string()})
            }
            "blob.commit" => {
                self.ensure_writable()?;
                let id = p["transfer_id"].as_str().unwrap();
                let t = self
                    .transfers
                    .get(id)
                    .ok_or_else(|| Error::new("NOT_FOUND", "Transfer not found"))?;
                if t.bytes.len() as u64 != t.length
                    || hex::encode(Sha256::digest(&t.bytes)) != t.sha256
                {
                    return Err(Error::new("INVALID_PARAMS", "Blob length or hash mismatch"));
                }
                let reference = self
                    .store
                    .as_mut()
                    .unwrap()
                    .blob_put(&t.bytes, &t.media_type)?;
                self.transfers.remove(id);
                reference
            }
            "blob.read" => {
                let hash = p["sha256"].as_str().unwrap();
                self.blob_leases
                    .retain(|_, (_, at)| at.elapsed().as_secs() < 600);
                self.ephemeral_blobs
                    .retain(|_, (_, at)| at.elapsed().as_secs() < 600);
                let mut reference = self.blob_leases.get(hash).map(|(value, _)| value.clone());
                for c in self.snapshot()?.collections.values() {
                    for r in c.values() {
                        find_blob(&r.data, hash, &mut reference);
                    }
                }
                let reference = reference.ok_or_else(|| {
                    Error::new("NOT_FOUND", "Blob is not referenced by the workspace")
                })?;
                let offset = parse_u64(p["offset"].as_str().unwrap())?;
                let bytes = if let Some((bytes, _)) = self.ephemeral_blobs.get(hash) {
                    let offset = usize::try_from(offset).map_err(|_| {
                        Error::new("INVALID_PARAMS", "Blob offset exceeds address space")
                    })?;
                    if offset > bytes.len() {
                        return Err(Error::new(
                            "INVALID_PARAMS",
                            "Blob offset exceeds body length",
                        ));
                    }
                    bytes[offset
                        ..bytes
                            .len()
                            .min(offset + p["length"].as_u64().unwrap() as usize)]
                        .to_vec()
                } else {
                    self.store.as_ref().unwrap().blob_read(
                        &reference,
                        offset,
                        p["length"].as_u64().unwrap() as usize,
                    )?
                };
                let total = parse_u64(reference["length"].as_str().unwrap())?;
                json!({"offset":offset.to_string(),"length":bytes.len().to_string(),"total_length":total.to_string(),"data_base64":STANDARD.encode(&bytes),"eof":offset+bytes.len() as u64>=total})
            }
            "agent.inbox" | "agent.features.get" => {
                artex_agent::query(&request.method, p, self.snapshot()?.as_ref(), &context)?
            }
            "agent.send"
            | "agent.claim"
            | "agent.acknowledge"
            | "agent.cancel_request"
            | "agent.features.set"
            | "user.answer" => {
                self.ensure_writable()?;
                self.require_command(&request)?;
                if let Some(result) = self.repeated(&request)? {
                    return Ok(result);
                }
                let mut b =
                    artex_agent::plan(&request.method, p, self.snapshot()?.as_ref(), &context)?;
                self.commit(&request, &mut b)?
            }
            "host.update.check" => json!({"configured":false,"available":false}),
            m if m.starts_with("host.") => {
                return Err(Error::new(
                    "PERMISSION_DENIED",
                    "Host methods cannot be executed by the engine wire",
                ));
            }
            "tool.invoke" => {
                if context.actor.kind != "agent" {
                    return Err(Error::new(
                        "PERMISSION_DENIED",
                        "UI cannot invoke arbitrary run tools",
                    ));
                }
                artex_agent::authenticate(self.snapshot()?.as_ref(), &context)?;
                if context.actor.run_id.as_deref() != p["run_id"].as_str() {
                    return Err(Error::new(
                        "PERMISSION_DENIED",
                        "Tool run does not match engine context",
                    ));
                }
                self.ensure_writable()?;
                self.require_command(&request)?;
                if let Some(result) = self.repeated(&request)? {
                    return Ok(result);
                }
                let mut prepared = self.invoke_plan(p, &context)?;
                self.commit(&request, &mut prepared)?
            }
            m if m.starts_with("ui.") || m.starts_with("legacy.") => {
                if matches!(
                    m,
                    "ui.authStatus" | "ui.login" | "ui.initPassword" | "ui.changePassword"
                ) {
                    return Err(Error::new("FEATURE_DISABLED", "OS_SESSION_AUTH"));
                }
                if m == "ui.health" {
                    return Ok(
                        json!({"ok":true,"service":"ARTEX Desktop","version":env!("CARGO_PKG_VERSION")}),
                    );
                }
                if matches!(m, "ui.testLLM" | "ui.fetchLLMModels") {
                    return self.handle_llm_network(&request, &context);
                }
                if matches!(
                    m,
                    "ui.ssStatus"
                        | "ui.ssProjects"
                        | "ui.ssTasks"
                        | "ui.ssDatasource"
                        | "ui.ssSync"
                ) {
                    return self.handle_scopesentry(&request, &context);
                }
                if m == "ui.deleteTask" {
                    return self.handle_lifecycle_legacy(&request, &context);
                }
                if matches!(m, "ui.deleteConversation" | "ui.deleteConversations") {
                    return self.handle_lifecycle_legacy(&request, &context);
                }
                if matches!(m, "legacy.handleSideQuestions" | "legacy.cancelSideRequest") {
                    let mutation = m == "legacy.cancelSideRequest"
                        || artex_domain::side_questions::route(p)?["legacy_http_method"] != "GET";
                    if mutation {
                        self.ensure_writable()?;
                        self.require_command(&request)?;
                        if let Some(result) = self.repeated(&request)? {
                            return Ok(result);
                        }
                        let mut plan = artex_domain::side_questions::plan(
                            m,
                            p,
                            self.snapshot()?.as_ref(),
                            &context,
                        )?;
                        return self.commit(&request, &mut plan);
                    }
                }
                if controller::NATIVE_UI.contains(&m) {
                    return self.native_ui(&request, &context);
                }
                if m == "ui.notifyTestChannel" {
                    return self.handle_notification_test(&request, &context);
                }
                if m == "ui.gc" {
                    return self.handle_gc(&request, &context);
                }
                if [
                    "ui.taskArchives",
                    "ui.taskArchive",
                    "ui.archiveTask",
                    "ui.archiveTasks",
                    "ui.restoreTaskArchive",
                    "ui.restoreTaskArchives",
                    "ui.deleteTaskArchive",
                    "ui.deleteTaskArchives",
                ]
                .contains(&m)
                {
                    return self.handle_archive(&request, &context);
                }
                let state = self.snapshot()?;
                let query = facade::query_with_blob_reader(m, p, &state, &context, &|reference| {
                    self.read_blob_bytes(reference, 64 * 1024 * 1024)
                });
                match query {
                    Ok(v) => self.externalize_query(v)?,
                    Err(e)
                        if matches!(e.code.as_str(), "METHOD_NOT_FOUND" | "FEATURE_DISABLED") =>
                    {
                        self.ensure_writable()?;
                        self.require_command(&request)?;
                        if let Some(result) = self.repeated(&request)? {
                            return Ok(result);
                        }
                        let mut b = facade::plan(m, p, &state, &context)?;
                        self.commit(&request, &mut b)?
                    }
                    Err(e) => return Err(e),
                }
            }
            m if artex_domain::is_mutation(m) => {
                if matches!(m, "conversation.delete" | "conversation.delete_batch") {
                    return self.handle_lifecycle_legacy(&request, &context);
                }
                self.ensure_writable()?;
                self.require_command(&request)?;
                if let Some(result) = self.repeated(&request)? {
                    return Ok(result);
                }
                let mut batch = artex_domain::plan(m, p, self.snapshot()?.as_ref(), &context)?;
                self.commit(&request, &mut batch)?
            }
            m => {
                let result = artex_domain::query(m, p, self.snapshot()?.as_ref(), &context)?;
                self.externalize_query(result)?
            }
        };
        let result_schema = self.contract["results"][&request.method].clone();
        if !result_schema.is_null() {
            self.validate(
                &format!("result:{}", request.method),
                &result_schema,
                &result,
            )
            .map_err(|cause| result_contract_error(&request.method, &cause))?;
        }
        Ok(result)
    }
    fn effective_read_only(&self) -> bool {
        self.read_only || self.store.as_ref().is_some_and(Store::read_only)
    }
    fn ensure_writable(&self) -> Result<()> {
        if self.effective_read_only() {
            Err(Error::new("RECOVERY_REQUIRED", "Workspace is read-only"))
        } else {
            Ok(())
        }
    }
    fn require_command(&self, r: &DispatchRequest) -> Result<()> {
        if r.command_id.is_none() {
            return Err(Error::new(
                "INVALID_PARAMS",
                "Durable mutation requires command_id",
            ));
        }
        Ok(())
    }
    fn repeated(&self, r: &DispatchRequest) -> Result<Option<Value>> {
        let id = r.command_id.as_deref().unwrap();
        let state = self.snapshot()?;
        if let Some(old) = state
            .records("command_receipts")
            .into_iter()
            .find(|v| v.data["command_id"] == id)
        {
            if old.data["method"] != r.method
                || old.data["normalized_params"] != r.params
                || old.data["expected"]
                    != serde_json::to_value(&r.expected)
                        .map_err(|_| Error::new("INVALID_PARAMS", "Invalid expected versions"))?
            {
                return Err(Error::new(
                    "COMMAND_ID_REUSE",
                    "Command ID has different normalized input",
                ));
            }
            return self.hydrate_result(&old.data["result"]).map(Some);
        }
        Ok(None)
    }
    fn commit(&mut self, r: &DispatchRequest, b: &mut CommandBatch) -> Result<Value> {
        let supplied = r
            .expected
            .iter()
            .map(|e| (&e.collection, &e.id, &e.version))
            .collect::<HashSet<_>>();
        if supplied.len() != r.expected.len() {
            return Err(Error::new("INVALID_PARAMS", "Duplicate expected object"));
        }
        for e in &b.expected {
            if !supplied.contains(&(&e.collection, &e.id, &e.version)) {
                let mut error = Error::new(
                    "VERSION_CONFLICT",
                    "Missing expected version for a referenced object",
                );
                error.details = json!({"required":e});
                return Err(error);
            }
        }
        let result_schema = self.contract["results"][&r.method].clone();
        if !result_schema.is_null() {
            self.validate(&format!("result:{}", r.method), &result_schema, &b.result)
                .map_err(|_| {
                    Error::new(
                        "EXTERNAL_FAILURE",
                        "Mutation result does not match the v1 contract; no change committed",
                    )
                })?;
        }
        b.command_id = r.command_id.clone().unwrap();
        b.method = r.method.clone();
        b.normalized_params = r.params.clone();
        b.expected = r.expected.clone();
        self.externalize_batch(b)?;
        let value = self.store.as_mut().unwrap().commit(b.clone())?;
        let result = self.hydrate_result(&value.result)?;
        self.approval_committed(r);
        Ok(result)
    }
    fn read_json_blob(&self, reference: &Value) -> Result<Value> {
        let length = parse_u64(reference["length"].as_str().ok_or_else(|| {
            Error::new("CORRUPT_STORE", "Stored result reference has no length")
        })?)?;
        let limit = if self.native_mode {
            MAX_QUEUE_BYTES
        } else {
            MAX_FRAME
        } as u64;
        if length > limit {
            return Err(Error::new(
                "SIZE_LIMIT",
                "Stored result exceeds this operation's bounded read budget",
            ));
        }
        let bytes = self.read_blob_bytes(reference, limit)?;
        serde_json::from_slice(&bytes)
            .map_err(|_| Error::new("CORRUPT_STORE", "Stored result JSON is invalid"))
    }
    fn hydrate_result(&self, result: &Value) -> Result<Value> {
        if result
            .as_object()
            .is_some_and(|o| o.len() == 1 && o.contains_key("result_ref"))
        {
            self.read_json_blob(&result["result_ref"])
        } else {
            Ok(result.clone())
        }
    }
    fn invoke_plan(&mut self, p: &Value, context: &Context) -> Result<CommandBatch> {
        use artex_core::{Change, body};
        let state = self.snapshot()?;
        let rid = p["run_id"].as_str().unwrap();
        let run = state.get("agent_runs", rid).unwrap();
        let name = p["name"].as_str().unwrap();
        let tool_use_id = p["tool_use_id"].as_str().unwrap();
        if tool_use_id.is_empty() {
            return Err(Error::new(
                "INVALID_PARAMS",
                "tool_use_id must not be empty",
            ));
        }
        if let Some(old) = state
            .records("tool_invocations")
            .into_iter()
            .find(|r| r.data["run_id"] == rid && r.data["tool_use_id"] == tool_use_id)
        {
            if old.data["name"] != name
                || old.data["input"] != p["input"]
                || old.data["raw_input"] != p["raw_input"]
            {
                return Err(Error::new(
                    "COMMAND_ID_REUSE",
                    "Tool use ID was reused with different input",
                ));
            }
            if !matches!(old.data["state"].as_str(), Some("completed" | "failed")) {
                return Err(Error::new(
                    "RECOVERY_REQUIRED",
                    "Invocation has no confirmed terminal result; automatic repetition is forbidden",
                ));
            }
            let reference = &old.data["result_ref"];
            let result = self.read_json_blob(reference)?;
            return Ok(CommandBatch {
                command_id: String::new(),
                method: "tool.invoke".into(),
                normalized_params: p.clone(),
                expected: vec![],
                changes: vec![],
                events: vec![],
                deliveries: vec![],
                result: json!({"invocation_id":old.header.id,"state":old.data["state"],"result":result,"result_ref":reference}),
            });
        }
        if run.data["state"] != "running" {
            return Err(Error::new(
                "BUSY",
                "Run is waiting for a request or user response",
            ));
        }
        let ordinal = parse_u64(run.data["next_tool_ordinal"].as_str().unwrap())?;
        let invocation_id = new_id("invocation");
        let normalized_input = artex_tools::normalize_scalar_defaults(name, &p["input"], &state);
        let dispatch = if name == "SearchExtraTools" {
            artex_tools::check_access(name, &normalized_input, &state, context)?;
            artex_tools::ToolDispatch::Query(artex_domain::sdk::query_with_blob_reader(
                name,
                &normalized_input,
                &state,
                context,
                &|reference| self.read_blob_bytes(reference, 16 * 1024 * 1024),
            )?)
        } else if name == "Compress" {
            artex_tools::check_access(name, &normalized_input, &state, context)?;
            artex_tools::ToolDispatch::Command(self.compress_plan(
                rid,
                &normalized_input,
                context,
            )?)
        } else if name == "Read"
            && normalized_input["file_path"]
                .as_str()
                .is_some_and(|path| path.starts_with("blob:"))
        {
            artex_tools::check_access(name, &normalized_input, &state, context)?;
            artex_tools::ToolDispatch::Query(
                json!({"content":self.compression_archive_read(rid,normalized_input["file_path"].as_str().unwrap())?.ok_or_else(||Error::new("NOT_FOUND","Conversation archive was not found"))?}),
            )
        } else if name == "traffic_blob" {
            artex_tools::check_access(name, &normalized_input, &state, context)?;
            artex_tools::ToolDispatch::Query(json!(self.traffic_blob_text(&normalized_input)?))
        } else {
            artex_tools::dispatch_with_blob_reader(
                name,
                &normalized_input,
                &state,
                context,
                &|reference| self.read_blob_bytes(reference, 64 * 1024 * 1024),
            )?
        };
        let mut batch = CommandBatch {
            command_id: String::new(),
            method: "tool.invoke".into(),
            normalized_params: p.clone(),
            expected: vec![],
            changes: vec![],
            events: vec![],
            deliveries: vec![],
            result: Value::Null,
        };
        let (output, status) = match dispatch {
            artex_tools::ToolDispatch::Query(result) => (Some(result), "completed"),
            artex_tools::ToolDispatch::Command(command) => {
                batch = command;
                let result = batch.result.clone();
                (Some(result), "completed")
            }
            artex_tools::ToolDispatch::HostIntent { .. } => (None, "prepared"),
        };
        let output = output.unwrap_or(Value::Null);
        let serialized = match output.as_str() {
            Some(text) => text.to_owned(),
            None => serde_json::to_string(&output)
                .map_err(|_| Error::new("INVALID_PARAMS", "Cannot encode tool result"))?,
        };
        let text = if serialized.len() > MAX_CHUNK {
            let reference = self
                .store
                .as_mut()
                .unwrap()
                .blob_put(serialized.as_bytes(), "application/json")?;
            json!({"text":null,"blob":reference})
        } else {
            body(&serialized)
        };
        let tool_result =
            json!({"content":[{"type":"text","text":text}],"is_error":false,"extra":[]});
        let result_bytes = serde_json::to_vec(&tool_result).unwrap();
        let result_ref = self
            .store
            .as_mut()
            .unwrap()
            .blob_put(&result_bytes, "application/json")?;
        let created = artex_core::now();
        let external = status == "prepared";
        let invocation = json!({"id":invocation_id,"run_id":rid,"tool_use_id":tool_use_id,"ordinal":ordinal.to_string(),"name":name,"input":p["input"],"raw_input":p["raw_input"],"state":status,"execution_id":if external{json!(new_id("exec"))}else{Value::Null},"input_ref":null,"result_ref":if external{Value::Null}else{result_ref.clone()},"created_at":created});
        batch.changes.push(Change::Put {
            collection: "tool_invocations".into(),
            id: invocation_id.clone(),
            data: invocation,
        });
        let mut next_run = batch
            .changes
            .iter()
            .find_map(|c| match c {
                Change::Put {
                    collection,
                    id,
                    data,
                } if collection == "agent_runs" && id == rid => Some(data.clone()),
                _ => None,
            })
            .unwrap_or_else(|| run.data.clone());
        batch.changes.retain(
            |c| !matches!(c,Change::Put{collection,id,..} if collection=="agent_runs"&&id==rid),
        );
        next_run["next_tool_ordinal"] = json!(
            ordinal
                .checked_add(1)
                .ok_or_else(|| Error::new("SIZE_LIMIT", "Invocation ordinal exhausted"))?
                .to_string()
        );
        batch.changes.push(Change::Put {
            collection: "agent_runs".into(),
            id: rid.into(),
            data: next_run,
        });
        let mut dependencies = vec![run];
        if let Some(agent) = context
            .actor
            .agent_id
            .as_deref()
            .and_then(|id| state.get("agents", id))
        {
            dependencies.push(agent);
        }
        for r in state.records("tools") {
            if r.data["key"] == name {
                dependencies.push(r);
            }
        }
        for r in state.records("agent_features") {
            if r.data["agent_id"].as_str() == context.actor.agent_id.as_deref() {
                dependencies.push(r);
            }
        }
        for r in dependencies {
            if !batch
                .expected
                .iter()
                .any(|e| e.collection == r.header.collection && e.id == r.header.id)
            {
                batch.expected.push(Expected {
                    collection: r.header.collection.clone(),
                    id: r.header.id.clone(),
                    version: r.header.version.clone(),
                });
            }
        }
        batch.result = if external {
            json!({"invocation_id":invocation_id,"state":status,"result_ref":null})
        } else {
            json!({"invocation_id":invocation_id,"state":status,"result":tool_result,"result_ref":result_ref})
        };
        artex_domain::conversation::push_event(
            &mut batch,
            &state,
            context,
            "ToolInvoked",
            "tool_invocations",
            &invocation_id,
        )?;
        if !external {
            artex_domain::conversation::push_event(
                &mut batch,
                &state,
                context,
                "ToolInvocationCompleted",
                "tool_invocations",
                &invocation_id,
            )?;
        }
        Ok(batch)
    }
    fn query_objects(&self, p: &Value) -> Result<Value> {
        let state = self.snapshot()?;
        let c = p["collection"].as_str().unwrap();
        if let Some(at) = p["at_seq"].as_str() {
            if parse_u64(at)? != state.commit_seq {
                return Err(Error::new(
                    "RESYNC_REQUIRED",
                    "Requested immutable read generation is no longer held",
                ));
            }
        }
        let filter = hex::encode(Sha256::digest(serde_json::to_vec(&p["ids"]).unwrap()));
        let mut last = String::new();
        if let Some(cursor) = p["cursor"].as_str() {
            let bytes = URL_SAFE_NO_PAD
                .decode(cursor)
                .map_err(|_| Error::new("INVALID_PARAMS", "Invalid cursor"))?;
            let cursor: Cursor = serde_json::from_slice(&bytes)
                .map_err(|_| Error::new("INVALID_PARAMS", "Invalid cursor"))?;
            if cursor.workspace != state.workspace_id
                || cursor.collection != c
                || cursor.read_seq != state.commit_seq.to_string()
                || cursor.filter_hash != filter
                || cursor.epoch != self.engine_epoch
            {
                return Err(Error::new(
                    "RESYNC_REQUIRED",
                    "Cursor generation or query changed",
                ));
            }
            last = cursor.last_sort_key;
        }
        let ids = p["ids"].as_array().unwrap();
        let limit = p["limit"].as_u64().unwrap() as usize;
        let mut records = state
            .records(if c == "traffic_records" {
                "exchanges"
            } else {
                c
            })
            .into_iter()
            .filter(|r| {
                r.header.id > last && (ids.is_empty() || ids.iter().any(|id| id == &r.header.id))
            })
            .collect::<Vec<_>>();
        records.sort_by(|a, b| a.header.id.cmp(&b.header.id));
        let has_more = records.len() > limit;
        records.truncate(limit);
        let cursor = if has_more {
            let last_sort_key = records.last().unwrap().header.id.clone();
            Some(
                URL_SAFE_NO_PAD.encode(
                    serde_json::to_vec(&Cursor {
                        workspace: state.workspace_id.clone(),
                        collection: c.into(),
                        read_seq: state.commit_seq.to_string(),
                        filter_hash: filter,
                        last_sort_key,
                        epoch: self.engine_epoch.clone(),
                    })
                    .unwrap(),
                ),
            )
        } else {
            None
        };
        let items:Vec<Value>=records.into_iter().map(|r|{
            if c=="traffic_records"{
                let mut header=r.header.clone();header.collection=c.into();
                let bodies=state.records("exchange_bodies").into_iter().find(|b|b.data["id"]==r.header.id);
                json!({"header":header,"data":{"exchange":r.data,"bodies":bodies.map(|b|&b.data)}})
            }else{json!(r)}
        }).collect();
        Ok(json!({"items":items,"read_seq":state.commit_seq.to_string(),"next_cursor":cursor}))
    }
    pub fn drain_events(&mut self) -> Vec<Value> {
        let Some(store) = &self.store else {
            return vec![];
        };
        let state = store.read_generation().snapshot.clone();
        let mut messages = vec![];
        for (id, s) in &mut self.subscriptions {
            let mut events = state
                .records("domain_events")
                .into_iter()
                .filter(|r| {
                    r.header
                        .commit_seq
                        .parse::<u64>()
                        .is_ok_and(|v| v > s.after)
                })
                .collect::<Vec<_>>();
            events.sort_by(|a, b| {
                a.data["seq"]
                    .as_str()
                    .unwrap_or("0")
                    .parse::<u64>()
                    .unwrap_or(0)
                    .cmp(
                        &b.data["seq"]
                            .as_str()
                            .unwrap_or("0")
                            .parse::<u64>()
                            .unwrap_or(0),
                    )
            });
            for event in events {
                if s.kinds.is_empty() || s.kinds.contains(event.data["kind"].as_str().unwrap_or(""))
                {
                    messages.push(json!({"jsonrpc":"2.0","method":"event","params":{"subscription_id":id,"engine_epoch":self.engine_epoch,"seq":event.data["seq"],"commit_seq":event.header.commit_seq,"kind":event.data["kind"],"payload":event.data}}));
                }
            }
            s.after = state.commit_seq;
        }
        messages
    }
}
fn find_blob(value: &Value, hash: &str, found: &mut Option<Value>) {
    match value {
        Value::Object(o) => {
            if o.get("sha256").and_then(Value::as_str) == Some(hash)
                && o.contains_key("length")
                && o.contains_key("media_type")
            {
                *found = Some(value.clone());
            } else {
                for v in o.values() {
                    find_blob(v, hash, found);
                }
            }
        }
        Value::Array(a) => {
            for v in a {
                find_blob(v, hash, found)
            }
        }
        _ => {}
    }
}
fn result_contract_error(method: &str, cause: &Error) -> Error {
    let mut error = Error::new(
        "EXTERNAL_FAILURE",
        &format!("Implementation produced a result outside the v1 contract (method={method})"),
    );
    error.details = json!({
        "method":method,
        "phase":"result_validation",
        "schema_paths":cause.details.get("schema_paths").cloned().unwrap_or(json!([])),
    });
    error
}

fn check_numeric(schema: &Value, defs: &Value, value: &Value) -> Result<()> {
    if let Some(r) = schema["$ref"].as_str() {
        if let Some(name) = r.strip_prefix("#/$defs/") {
            return check_numeric(&defs[name], defs, value);
        }
    }
    if let Some(s) = value.as_str() {
        if schema.get("x-max").is_some() {
            parse_u64(s)?;
        }
        if schema["x-range"] == "int64" && s.parse::<i64>().is_err() {
            return Err(Error::new(
                "INVALID_PARAMS",
                "Signed integer is outside int64",
            ));
        }
    }
    if let Some(branches) = schema["anyOf"]
        .as_array()
        .or_else(|| schema["oneOf"].as_array())
    {
        for b in branches {
            if b.get("$ref").is_some()
                || b["type"]
                    == match value {
                        Value::Null => "null",
                        Value::String(_) => "string",
                        Value::Array(_) => "array",
                        Value::Object(_) => "object",
                        Value::Bool(_) => "boolean",
                        _ => "number",
                    }
            {
                check_numeric(b, defs, value)?;
            }
        }
    }
    if let Some(o) = value.as_object() {
        for (key, v) in o {
            if let Some(s) = schema["properties"].get(key) {
                check_numeric(s, defs, v)?;
            } else if schema["additionalProperties"].is_object() {
                check_numeric(&schema["additionalProperties"], defs, v)?;
            }
        }
    }
    if let Some(a) = value.as_array() {
        for v in a {
            check_numeric(&schema["items"], defs, v)?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn pipe_never_treats_path_as_capability() {
        let mut router = Router::new(BTreeMap::new());
        let hello = json!({"jsonrpc":"2.0","protocol":"artex-rpc/1.0","id":"1","method":"system.hello","workspace_id":null,"engine_epoch":null,"command_id":null,"expected":[],"timeout_ms":30000,"params":{"versions":["artex-rpc/1.0"],"host_epoch":new_id("epoch"),"capabilities":[]}});
        assert!(router.response(hello)["error"].is_null());
        let open = json!({"jsonrpc":"2.0","protocol":"artex-rpc/1.0","id":"2","method":"workspace.open","workspace_id":null,"engine_epoch":null,"command_id":null,"expected":[],"timeout_ms":30000,"params":{"directory_capability":"C:/Users","read_only":false}});
        assert_eq!(
            router.response(open)["error"]["data"]["code"],
            "PERMISSION_DENIED"
        );
    }
    #[test]
    fn durable_tool_result_is_paired_and_reused_without_reexecution() {
        use artex_core::{Change, default_data};
        let directory = tempfile::tempdir().unwrap();
        let mut router = Router::new(BTreeMap::new());
        router.host_epoch = Some(new_id("epoch"));
        let mut store = Store::create(directory.path(), "tools").unwrap();
        let aid = new_id("agent");
        let rid = new_id("run");
        let mut agent = default_data("agents").unwrap();
        agent["id"] = json!(aid);
        agent["key"] = json!("auto");
        agent["enabled"] = json!(true);
        let mut run = default_data("agent_runs").unwrap();
        run["id"] = json!(rid);
        run["agent_id"] = json!(aid);
        run["engine_epoch"] = json!(router.engine_epoch);
        run["state"] = json!("running");
        let cid = new_id("company");
        let mut company = default_data("companies").unwrap();
        company["id"] = json!(cid);
        company["name"] = json!("\"".repeat(20_000));
        company["nkey"] = json!("large-result-fixture");
        store
            .commit(CommandBatch {
                command_id: new_id("command"),
                method: "test.seed".into(),
                normalized_params: json!({}),
                expected: vec![],
                changes: vec![
                    Change::Put {
                        collection: "companies".into(),
                        id: cid,
                        data: company,
                    },
                    Change::Put {
                        collection: "agents".into(),
                        id: aid.clone(),
                        data: agent,
                    },
                    Change::Put {
                        collection: "agent_runs".into(),
                        id: rid.clone(),
                        data: run,
                    },
                ],
                events: vec![],
                deliveries: vec![],
                result: Value::Null,
            })
            .unwrap();
        router.store = Some(store);
        let context =
            artex_agent::context_for_run(&router.snapshot().unwrap(), &rid, &router.engine_epoch)
                .unwrap();
        let params = json!({"run_id":rid,"tool_use_id":"call-1","name":"list_companies","input":{},"raw_input":null});
        for (method, params) in [
            ("ui.createCompany", json!({"name":"forbidden","scope":[]})),
            (
                "query.objects",
                json!({"collection":"memory_records","ids":[],"cursor":null,"limit":100,"at_seq":null}),
            ),
        ] {
            let request = json!({"jsonrpc":"2.0","protocol":"artex-rpc/1.0","id":new_id("request"),"method":method,"workspace_id":router.snapshot().unwrap().workspace_id,"engine_epoch":router.engine_epoch,"command_id":new_id("command"),"expected":[],"timeout_ms":30000,"params":params});
            assert_eq!(
                router.response_as(request, Some(&context))["error"]["data"]["code"],
                "PERMISSION_DENIED"
            );
        }
        let mut prior = None;
        let mut first_request = None;
        for index in 0..3 {
            let snapshot = router.snapshot().unwrap();
            let expected = [("agents", aid.as_str()), ("agent_runs", rid.as_str())]
                .into_iter()
                .map(|(collection, id)| {
                    let r = snapshot.get(collection, id).unwrap();
                    json!({"collection":collection,"id":id,"version":r.header.version})
                })
                .collect::<Vec<_>>();
            let mut request = json!({"jsonrpc":"2.0","protocol":"artex-rpc/1.0","id":format!("tool-{index}"),"method":"tool.invoke","workspace_id":snapshot.workspace_id,"engine_epoch":router.engine_epoch,"command_id":new_id("command"),"expected":expected,"timeout_ms":30000,"params":params});
            if index == 0 {
                first_request = Some(request.clone());
            } else if index == 2 {
                request = first_request.as_ref().unwrap().clone();
                request["id"] = json!("tool-original-command-retry");
            }
            let response = router.response_as(request, Some(&context));
            assert!(response["error"].is_null(), "{response}");
            let result = &response["result"];
            assert_eq!(result["state"], "completed");
            assert_eq!(result["result"]["is_error"], false);
            assert!(result["result_ref"]["sha256"].is_string());
            assert!(
                parse_u64(result["result_ref"]["length"].as_str().unwrap()).unwrap()
                    > MAX_CHUNK as u64,
                "fixture must exercise multi-chunk result replay"
            );
            if let Some(old) = &prior {
                assert_eq!(result, old);
            } else {
                prior = Some(result.clone());
            }
        }
        let state = router.snapshot().unwrap();
        assert_eq!(state.records("tool_invocations").len(), 1);
        assert_eq!(
            state.get("agent_runs", &rid).unwrap().data["next_tool_ordinal"],
            "1"
        );
        let run = state.get("agent_runs", &rid).unwrap();
        let mut waiting = run.data.clone();
        waiting["state"] = json!("waiting");
        router
            .store
            .as_mut()
            .unwrap()
            .commit(CommandBatch {
                command_id: new_id("command"),
                method: "test.wait".into(),
                normalized_params: json!({}),
                expected: vec![Expected {
                    collection: "agent_runs".into(),
                    id: rid.clone(),
                    version: run.header.version.clone(),
                }],
                changes: vec![Change::Put {
                    collection: "agent_runs".into(),
                    id: rid.clone(),
                    data: waiting,
                }],
                events: vec![],
                deliveries: vec![],
                result: Value::Null,
            })
            .unwrap();
        for use_id in ["call-1", "call-2"] {
            let state = router.snapshot().unwrap();
            let expected=[("agents",aid.as_str()),("agent_runs",rid.as_str())].into_iter().map(|(collection,id)|json!({"collection":collection,"id":id,"version":state.get(collection,id).unwrap().header.version})).collect::<Vec<_>>();
            let mut params = params.clone();
            params["tool_use_id"] = json!(use_id);
            let request = json!({"jsonrpc":"2.0","protocol":"artex-rpc/1.0","id":format!("waiting-{use_id}"),"method":"tool.invoke","workspace_id":state.workspace_id,"engine_epoch":router.engine_epoch,"command_id":new_id("command"),"expected":expected,"timeout_ms":30000,"params":params});
            let response = router.response_as(request, Some(&context));
            if use_id == "call-1" {
                assert!(response["error"].is_null(), "{response}");
                assert_eq!(response["result"], prior.clone().unwrap());
            } else {
                assert_eq!(response["error"]["data"]["code"], "BUSY");
            }
        }
    }
}
