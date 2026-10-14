//! In-process application entrypoints. No framing, handshake or wire envelopes.
use super::*;
use std::{path::Path, sync::Arc, time::Duration};

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct NativeTicket(pub String);
impl NativeTicket {
    pub fn new() -> Self {
        Self(new_id("request"))
    }
}
impl Default for NativeTicket {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WorkspaceScope {
    pub workspace_id: String,
    pub engine_epoch: String,
    pub open_generation: String,
}
#[derive(Clone, Debug)]
pub enum OpenMode {
    Existing,
    Create { name: String },
}
#[derive(Clone, Debug)]
pub struct OpenOptions {
    pub path: PathBuf,
    pub mode: OpenMode,
    pub read_only: bool,
}
#[derive(Clone, Debug)]
pub struct NativeWorkspace {
    pub scope: WorkspaceScope,
    pub root: PathBuf,
    pub work_root: PathBuf,
    pub resources_root: PathBuf,
    pub execution_capability: String,
    pub resource_capability: String,
    pub read_only: bool,
}
#[derive(Clone, Debug)]
pub struct ObjectsQuery {
    pub collection: String,
    pub ids: Vec<String>,
    pub limit: usize,
    pub cursor: Option<String>,
    pub at_seq: Option<u64>,
}
#[derive(Clone, Debug)]
pub enum MonitorTarget {
    All,
    Page { offset: usize, limit: usize },
    Run { run_id: String },
    Task { task_id: String },
    Conversation { conversation_id: String },
}
#[derive(Clone, Debug)]
pub enum DeletionTarget {
    Conversations { ids: Vec<String> },
    Tasks { ids: Vec<String> },
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ConversationAction {
    ClearHistory,
    Trash,
    Restore,
    DeletePermanently,
}
#[derive(Clone, Debug, Default)]
pub struct TaskDeleteOptions {
    pub delete_assets: bool,
    pub delete_traffic: bool,
    pub delete_files: bool,
    pub delete_findings: bool,
    pub delete_llm_records: bool,
}
#[derive(Clone, Debug)]
pub enum DeletionOptions {
    Conversations { action: ConversationAction },
    Tasks { options: TaskDeleteOptions },
}
#[derive(Clone, Debug)]
pub enum RunControlTarget {
    Run { run_id: String },
    Task { task_id: String },
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RunControlAction {
    Stop,
    ForceStop,
}
#[derive(Clone, Debug)]
pub enum NativeQuery {
    AgentMonitor {
        target: MonitorTarget,
    },
    DeletionPreview {
        target: DeletionTarget,
        options: DeletionOptions,
    },
    ConversationTrash,
    WorkspaceInfo,
    Feature {
        method: String,
        params: Value,
    },
    Prepare {
        method: String,
        params: Value,
    },
    CommandStatus {
        command_id: String,
    },
    Objects(ObjectsQuery),
    Blob {
        sha256: String,
        offset: u64,
        length: usize,
    },
    Body {
        body: Value,
    },
}
#[derive(Clone, Debug)]
pub struct NativeCommand {
    pub id: String,
    pub expected: Vec<Expected>,
    pub method: String,
    pub params: Value,
    pub timeout: Duration,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NativeFileArea {
    Work,
    Resources,
}
impl NativeCommand {
    pub fn new(method: impl Into<String>, params: Value, expected: Vec<Expected>) -> Self {
        Self {
            id: new_id("command"),
            expected,
            method: method.into(),
            params,
            timeout: Duration::from_secs(30),
        }
    }
    pub fn checkpoint() -> Self {
        Self::new("workspace.checkpoint", json!({}), vec![])
    }
}
#[derive(Clone)]
pub enum NativeAction {
    RunControl {
        scope: WorkspaceScope,
        command_id: String,
        expected: Vec<Expected>,
        target: RunControlTarget,
        action: RunControlAction,
    },
    ConversationManage {
        scope: WorkspaceScope,
        command_id: String,
        expected: Vec<Expected>,
        ids: Vec<String>,
        action: ConversationAction,
    },
    TaskDelete {
        scope: WorkspaceScope,
        command_id: String,
        expected: Vec<Expected>,
        ids: Vec<String>,
        options: TaskDeleteOptions,
    },
    Open(OpenOptions),
    Query {
        scope: WorkspaceScope,
        query: NativeQuery,
    },
    Command {
        scope: WorkspaceScope,
        command: NativeCommand,
    },
    Cancel {
        scope: WorkspaceScope,
        ticket: NativeTicket,
    },
    Close {
        scope: WorkspaceScope,
    },
    SetSecret {
        scope: WorkspaceScope,
        command_id: String,
        field: String,
        value: String,
    },
    DeleteSecret {
        scope: WorkspaceScope,
        reference: String,
    },
    FileRead {
        scope: WorkspaceScope,
        area: NativeFileArea,
        path: String,
    },
    FileWrite {
        scope: WorkspaceScope,
        area: NativeFileArea,
        path: String,
        content: String,
        expected_sha256: Option<String>,
        create: bool,
        command_id: String,
    },
    StageFile {
        scope: WorkspaceScope,
        path: PathBuf,
    },
    StageBody {
        scope: WorkspaceScope,
        body: Value,
        name: String,
    },
    StageBase64Body {
        scope: WorkspaceScope,
        body: Value,
        name: String,
    },
    ReleaseFile {
        scope: WorkspaceScope,
        token: String,
    },
    SaveFile {
        scope: WorkspaceScope,
        token: String,
        destination: PathBuf,
        command_id: String,
    },
    SkillMetaRead {
        scope: WorkspaceScope,
        name: String,
    },
    SkillMetaWrite {
        scope: WorkspaceScope,
        name: String,
        meta: Value,
        expected_sha256: String,
        command_id: String,
    },
    Shutdown,
    Suspend,
}
impl std::fmt::Debug for NativeAction {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::RunControl { .. } => "NativeAction::RunControl",
            Self::ConversationManage { .. } => "NativeAction::ConversationManage",
            Self::TaskDelete { .. } => "NativeAction::TaskDelete",
            Self::Open(_) => "NativeAction::Open",
            Self::Query { .. } => "NativeAction::Query",
            Self::Command { .. } => "NativeAction::Command",
            Self::Cancel { .. } => "NativeAction::Cancel",
            Self::Close { .. } => "NativeAction::Close",
            Self::SetSecret { .. } => "NativeAction::SetSecret(<redacted>)",
            Self::DeleteSecret { .. } => "NativeAction::DeleteSecret",
            Self::FileRead { .. } => "NativeAction::FileRead",
            Self::FileWrite { .. } => "NativeAction::FileWrite",
            Self::StageFile { .. } => "NativeAction::StageFile",
            Self::StageBody { .. } => "NativeAction::StageBody",
            Self::StageBase64Body { .. } => "NativeAction::StageBase64Body",
            Self::ReleaseFile { .. } => "NativeAction::ReleaseFile",
            Self::SaveFile { .. } => "NativeAction::SaveFile",
            Self::SkillMetaRead { .. } => "NativeAction::SkillMetaRead",
            Self::SkillMetaWrite { .. } => "NativeAction::SkillMetaWrite",
            Self::Shutdown => "NativeAction::Shutdown",
            Self::Suspend => "NativeAction::Suspend",
        })
    }
}
#[derive(Clone, Debug)]
pub struct NativeReply {
    pub value: Value,
    pub scope: Option<WorkspaceScope>,
    pub read_seq: u64,
    pub read_versions: Vec<Expected>,
}
#[derive(Clone, Debug)]
pub struct NativeCompletion {
    pub ticket: NativeTicket,
    pub result: Result<NativeReply>,
}
pub enum NativeDispatch {
    Complete(NativeReply),
    Deferred,
}
#[derive(Clone, Debug)]
pub struct NativeReadView {
    pub workspace: NativeWorkspace,
    pub generation: Arc<artex_store::ReadGeneration>,
}

/// A verified export source. The pinned store view holds blob generations and
/// the native workspace lock until the reader has finished or cancelled.
pub enum NativeStageSource {
    Inline(Vec<u8>),
    Ephemeral(std::sync::Arc<Vec<u8>>),
    Blob {
        view: artex_store::StoreReadView,
        reference: Value,
    },
}
impl NativeStageSource {
    pub fn copy_to(&self, writer: &mut impl std::io::Write) -> Result<u64> {
        match self {
            Self::Inline(bytes) => {
                for chunk in bytes.chunks(MAX_CHUNK) {
                    writer.write_all(chunk)?;
                }
                Ok(bytes.len() as u64)
            }
            Self::Ephemeral(bytes) => {
                for chunk in bytes.chunks(MAX_CHUNK) {
                    writer.write_all(chunk)?;
                }
                Ok(bytes.len() as u64)
            }
            Self::Blob { view, reference } => view.copy_blob_to(reference, writer),
        }
    }
}

impl Router {
    pub fn native_stage_source(
        &self,
        scope: &WorkspaceScope,
        body: Value,
    ) -> Result<NativeStageSource> {
        self.check_native_scope(scope)?;
        if let Some(text) = body["text"].as_str().filter(|_| body["blob"].is_null()) {
            if body.as_object().is_none_or(|object| {
                object.len() != 2 || !object.contains_key("text") || !object.contains_key("blob")
            }) {
                return Err(Error::new(
                    "INVALID_PARAMS",
                    "StageBody requires an exact Body value",
                ));
            }
            if text.len() > MAX_QUEUE_BYTES {
                return Err(Error::new("SIZE_LIMIT", "Inline export exceeds 16 MiB"));
            }
            return Ok(NativeStageSource::Inline(text.as_bytes().to_vec()));
        }
        artex_core::validate_contract(
            &self.contract["$defs"]["Body"],
            &self.contract["$defs"],
            &body,
        )?;
        let reference = body["blob"].clone();
        if reference.is_null() {
            return Err(Error::new(
                "INVALID_PARAMS",
                "An export Body must contain text or a blob",
            ));
        }
        if reference["length"]
            .as_str()
            .and_then(|length| length.parse::<u64>().ok())
            .is_none_or(|length| length > 512 * 1024 * 1024)
        {
            return Err(Error::new("SIZE_LIMIT", "Blob export exceeds 512 MiB"));
        }
        if let Some(bytes) = self.ephemeral_blob_bytes(&reference, 512 * 1024 * 1024)? {
            return Ok(NativeStageSource::Ephemeral(bytes));
        }
        let view = self
            .store
            .as_ref()
            .ok_or_else(|| Error::new("WORKSPACE_NOT_OPEN", "Workspace is closed"))?
            .read_view();
        Ok(NativeStageSource::Blob { view, reference })
    }
    /// Switch output to Rust values before any deferred operation is started.
    pub fn enable_native(&mut self) {
        self.native_mode = true;
    }
    pub fn native_scope(&self) -> Option<WorkspaceScope> {
        self.store.as_ref().map(|store| WorkspaceScope {
            workspace_id: store.read_generation().workspace_id.clone(),
            engine_epoch: self.engine_epoch.clone(),
            open_generation: self.workspace_generation.clone(),
        })
    }
    pub fn native_read_only(&self) -> bool {
        self.effective_read_only()
    }
    pub fn native_attach_host(&mut self, gateway: Arc<dyn artex_runtime::HostGateway>) {
        self.attach_host(gateway);
    }
    pub fn native_detach_host(&mut self) {
        self.gateway = None;
    }
    pub(super) fn check_native_scope(&self, scope: &WorkspaceScope) -> Result<()> {
        if self.native_scope().as_ref() != Some(scope) {
            return Err(Error::new("STALE_EPOCH", "Workspace lifetime has changed"));
        }
        Ok(())
    }
    pub(super) fn native_reply(&self, method: &str, params: &Value, value: Value) -> NativeReply {
        NativeReply {
            read_versions: self.response_versions(method, params, &value),
            value,
            scope: self.native_scope(),
            read_seq: self.snapshot().map(|s| s.commit_seq).unwrap_or(0),
        }
    }
    pub(super) fn push_native_completion(
        &mut self,
        id: &str,
        method: &str,
        params: &Value,
        result: Result<Value>,
    ) {
        let result = result.map(|v| self.native_reply(method, params, v));
        self.native_completions.push_back(NativeCompletion {
            ticket: NativeTicket(id.into()),
            result,
        });
    }
    pub fn drain_native_completions(&mut self) -> Vec<NativeCompletion> {
        self.native_completions.drain(..).collect()
    }
    /// Feature names identify library handlers; transport-only control methods
    /// cannot be smuggled through a feature operation.
    fn native_feature(&self, method: &str) -> Result<()> {
        if [
            "system.",
            "workspace.",
            "events.",
            "blob.",
            "rpc.",
            "tool.",
            "host.",
            "command.",
        ]
        .iter()
        .any(|p| method.starts_with(p))
        {
            return Err(Error::new(
                "PERMISSION_DENIED",
                "Use the typed application control operation",
            ));
        }
        if self.contract["methods"][method].is_null() {
            return Err(Error::new(
                "METHOD_NOT_FOUND",
                "Unknown application feature",
            ));
        }
        Ok(())
    }
    fn native_call(
        &mut self,
        ticket: &NativeTicket,
        scope: &WorkspaceScope,
        method: &str,
        params: Value,
        command: Option<&NativeCommand>,
    ) -> Result<NativeDispatch> {
        self.check_native_scope(scope)?;
        let timeout = command
            .map(|c| c.timeout)
            .unwrap_or(Duration::from_secs(30));
        let timeout_ms = u64::try_from(timeout.as_millis())
            .map_err(|_| Error::new("INVALID_PARAMS", "Operation timeout overflows"))?;
        if !(1..=3_600_000).contains(&timeout_ms) {
            return Err(Error::new(
                "INVALID_PARAMS",
                "Operation timeout must be 1..3600000 ms",
            ));
        }
        if let Some(command) = command {
            if !artex_core::valid_id(&command.id, "command") {
                return Err(Error::new("INVALID_PARAMS", "Typed command ID required"));
            }
            let expected_schema = self.contract["$defs"]["Expected"].clone();
            if !expected_schema.is_null() {
                for row in &command.expected {
                    self.validate(
                        "native:expected",
                        &expected_schema,
                        &serde_json::to_value(row)?,
                    )?;
                }
            }
        }
        let request = DispatchRequest {
            id: ticket.0.clone(),
            method: method.into(),
            workspace_id: Some(scope.workspace_id.clone()),
            engine_epoch: Some(scope.engine_epoch.clone()),
            command_id: command.map(|c| c.id.clone()),
            expected: command.map(|c| c.expected.clone()).unwrap_or_default(),
            timeout_ms,
            params: params.clone(),
        };
        let value = self.dispatch_request(&request, None)?;
        if value["__deferred"] == true {
            Ok(NativeDispatch::Deferred)
        } else {
            Ok(NativeDispatch::Complete(
                self.native_reply(method, &params, value),
            ))
        }
    }
    pub fn native_open(&mut self, options: &OpenOptions) -> Result<(NativeWorkspace, NativeReply)> {
        self.enable_native();
        if self.store.is_some() {
            return Err(Error::new(
                "VERSION_CONFLICT",
                "Close the current workspace first",
            ));
        }
        if self.shutting_down {
            return Err(Error::new("CANCELLED", "Application has shut down"));
        }
        if options.read_only && matches!(options.mode, OpenMode::Create { .. }) {
            return Err(Error::new(
                "INVALID_PARAMS",
                "A new workspace cannot be created read-only",
            ));
        }
        let root = std::path::absolute(&options.path).map_err(Error::io)?;
        // Every opening gets a fresh controller epoch, even in one process.
        // Results from a retired folder cannot authenticate against a reopened
        // copy that happens to contain the same record identifiers.
        self.engine_epoch = new_id("epoch");
        let directory = new_id("capability");
        self.capabilities.insert(directory.clone(), root.clone());
        let method = match options.mode {
            OpenMode::Existing => "workspace.open",
            OpenMode::Create { .. } => "workspace.create",
        };
        let params = match &options.mode {
            OpenMode::Existing => {
                json!({"directory_capability":directory,"read_only":options.read_only})
            }
            OpenMode::Create { name } => json!({"directory_capability":directory,"name":name}),
        };
        let request = DispatchRequest {
            id: new_id("request"),
            method: method.into(),
            workspace_id: None,
            engine_epoch: None,
            command_id: None,
            expected: vec![],
            timeout_ms: 30000,
            params: params.clone(),
        };
        let value = match self.dispatch_request(&request, None) {
            Ok(value) => value,
            Err(error) => {
                self.capabilities.remove(&directory);
                return Err(error);
            }
        };
        let root = root.canonicalize().map_err(Error::io)?;
        let work_root = root.join("work");
        let resources_root = root.join("resources");
        let read_only = self.effective_read_only();
        if !read_only {
            std::fs::create_dir_all(&work_root)
                .and_then(|_| std::fs::create_dir_all(&resources_root))
                .map_err(Error::io)?;
        }
        let execution_capability = new_id("capability");
        let resource_capability = new_id("capability");
        self.capabilities
            .insert(execution_capability.clone(), work_root.clone());
        self.capabilities
            .insert(resource_capability.clone(), resources_root.clone());
        self.set_execution_capabilities(
            Some(execution_capability.clone()),
            Some(resource_capability.clone()),
        )?;
        let workspace = NativeWorkspace {
            scope: self.native_scope().unwrap(),
            root,
            work_root,
            resources_root,
            execution_capability,
            resource_capability,
            read_only,
        };
        Ok((workspace, self.native_reply(method, &params, value)))
    }
    pub fn native_query(
        &mut self,
        ticket: &NativeTicket,
        scope: &WorkspaceScope,
        query: NativeQuery,
    ) -> Result<NativeDispatch> {
        self.check_native_scope(scope)?;
        match query {
            NativeQuery::AgentMonitor { target } => {
                return self.native_agent_monitor(ticket, scope, target);
            }
            NativeQuery::DeletionPreview { target, options } => {
                return Ok(NativeDispatch::Complete(self.native_reply(
                    "native.lifecycle.preview",
                    &json!({}),
                    self.lifecycle_preview(target, options)?,
                )));
            }
            NativeQuery::ConversationTrash => {
                return Ok(NativeDispatch::Complete(self.native_reply(
                    "native.lifecycle.trash",
                    &json!({}),
                    self.lifecycle_trash()?,
                )));
            }
            _ => {}
        }
        if let NativeQuery::Body { body } = &query {
            self.check_native_scope(scope)?;
            let schema = self.contract["$defs"]["Body"].clone();
            self.validate("native:body", &schema, body)?;
            let value = Value::String(self.body_text(body)?);
            return Ok(NativeDispatch::Complete(NativeReply {
                value,
                scope: self.native_scope(),
                read_seq: self.snapshot()?.commit_seq,
                read_versions: vec![],
            }));
        }
        let (method, params) = match query {
            NativeQuery::WorkspaceInfo => ("workspace.info".into(), json!({})),
            NativeQuery::Feature { method, params } => {
                self.native_feature(&method)?;
                (method, params)
            }
            NativeQuery::Prepare { method, mut params } => {
                self.native_feature(&method)?;
                self.check_native_scope(scope)?;
                let writable = !self.effective_read_only();
                if let Some(store) = self.store.as_mut() {
                    normalize_body_fields(
                        &self.contract["methods"][&method],
                        &self.contract["$defs"],
                        &mut params,
                        store,
                        writable,
                        0,
                        &mut std::collections::BTreeSet::new(),
                    )?;
                }
                (
                    "command.prepare".into(),
                    json!({"method":method,"params":params}),
                )
            }
            NativeQuery::CommandStatus { command_id } => {
                ("command.status".into(), json!({"command_id":command_id}))
            }
            NativeQuery::Objects(q) => (
                "query.objects".into(),
                json!({"collection":q.collection,"ids":q.ids,"limit":q.limit,"cursor":q.cursor,"at_seq":q.at_seq.map(|n|n.to_string())}),
            ),
            NativeQuery::Blob {
                sha256,
                offset,
                length,
            } => (
                "blob.read".into(),
                json!({"sha256":sha256,"offset":offset.to_string(),"length":length}),
            ),
            NativeQuery::Body { .. } => unreachable!("Body handled above"),
            NativeQuery::AgentMonitor { .. }
            | NativeQuery::DeletionPreview { .. }
            | NativeQuery::ConversationTrash => unreachable!("Native lifecycle handled above"),
        };
        self.native_call(ticket, scope, &method, params, None)
    }
    pub fn native_command(
        &mut self,
        ticket: &NativeTicket,
        scope: &WorkspaceScope,
        command: &NativeCommand,
    ) -> Result<NativeDispatch> {
        if command.method != "workspace.checkpoint" {
            self.native_feature(&command.method)?;
        }
        self.check_native_scope(scope)?;
        self.ensure_writable()?;
        let mut normalized = command.clone();
        if let Some(store) = self.store.as_mut() {
            let schema = &self.contract["methods"][&command.method];
            normalize_body_fields(
                schema,
                &self.contract["$defs"],
                &mut normalized.params,
                store,
                true,
                0,
                &mut std::collections::BTreeSet::new(),
            )?;
        }
        self.native_call(
            ticket,
            scope,
            &normalized.method,
            normalized.params.clone(),
            Some(&normalized),
        )
    }
    pub fn native_cancel(
        &mut self,
        scope: &WorkspaceScope,
        ticket: &NativeTicket,
    ) -> Result<NativeReply> {
        self.check_native_scope(scope)?;
        let value = self.cancel_request(&ticket.0);
        Ok(self.native_reply("", &Value::Null, value))
    }
    pub fn native_close(&mut self, scope: &WorkspaceScope) -> Result<NativeReply> {
        self.check_native_scope(scope)?;
        self.close_workspace_orderly()?;
        self.execution_capability = None;
        self.resource_capability = None;
        self.capabilities.clear();
        Ok(self.native_reply("", &Value::Null, json!({"closed":true})))
    }
    pub fn native_shutdown(&mut self) -> Result<NativeReply> {
        self.close_workspace_orderly()?;
        self.shutting_down = true;
        Ok(self.native_reply("", &Value::Null, json!({"accepted":true})))
    }
    pub fn native_root_is_open(&self, root: &Path) -> bool {
        self.store.is_some() && self.capabilities.values().any(|path| path == root)
    }
}

/// Follow declared schema paths only. In particular arbitrary executable JSON
/// and tool parameters that happen to look like Body are never rewritten.
fn normalize_body_fields(
    schema: &Value,
    defs: &Value,
    value: &mut Value,
    store: &mut Store,
    writable: bool,
    depth: usize,
    refs: &mut std::collections::BTreeSet<String>,
) -> Result<()> {
    if depth > 128 {
        return Err(Error::new(
            "SIZE_LIMIT",
            "Native command schema/value nesting exceeds 128",
        ));
    }
    if let Some(reference) = schema["$ref"].as_str() {
        if reference == "#/$defs/Body" {
            if let Some(object) = value.as_object() {
                if object.len() == 2
                    && object.contains_key("text")
                    && object.get("blob").is_some_and(Value::is_null)
                {
                    if let Some(text) = object["text"]
                        .as_str()
                        .filter(|text| text.len() > MAX_CHUNK)
                    {
                        if !writable {
                            return Err(Error::new(
                                "RECOVERY_REQUIRED",
                                "Large mutation Body preflight requires a writable workspace",
                            ));
                        }
                        if text.len() > MAX_QUEUE_BYTES {
                            return Err(Error::new("SIZE_LIMIT", "Inline Body exceeds 16 MiB"));
                        }
                        let reference =
                            store.blob_put(text.as_bytes(), "text/plain; charset=utf-8")?;
                        *value = json!({"text":null,"blob":reference});
                    }
                }
            }
            return Ok(());
        }
        if let Some(name) = reference.strip_prefix("#/$defs/") {
            if refs.insert(name.into()) {
                normalize_body_fields(&defs[name], defs, value, store, writable, depth + 1, refs)?;
                refs.remove(name);
            }
        }
    }
    for keyword in ["anyOf", "oneOf", "allOf"] {
        if let Some(branches) = schema[keyword].as_array() {
            for branch in branches {
                normalize_body_fields(branch, defs, value, store, writable, depth + 1, refs)?;
            }
        }
    }
    if let (Some(properties), Some(object)) =
        (schema["properties"].as_object(), value.as_object_mut())
    {
        for (name, field) in properties {
            if let Some(child) = object.get_mut(name) {
                normalize_body_fields(
                    field,
                    defs,
                    child,
                    store,
                    writable,
                    depth + 1,
                    &mut std::collections::BTreeSet::new(),
                )?;
            }
        }
    }
    if schema["items"].is_object() {
        if let Some(items) = value.as_array_mut() {
            for child in items {
                normalize_body_fields(
                    &schema["items"],
                    defs,
                    child,
                    store,
                    writable,
                    depth + 1,
                    &mut std::collections::BTreeSet::new(),
                )?;
            }
        }
    }
    if schema["additionalProperties"].is_object() {
        if let Some(object) = value.as_object_mut() {
            for (name, child) in object {
                if schema["properties"].get(name).is_none() {
                    normalize_body_fields(
                        &schema["additionalProperties"],
                        defs,
                        child,
                        store,
                        writable,
                        depth + 1,
                        &mut std::collections::BTreeSet::new(),
                    )?;
                }
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn body_normalization_follows_declared_refs_and_never_guesses_custom_json() {
        let dir = tempfile::tempdir().unwrap();
        let mut store = Store::create(&dir.path().join("w"), "Body boundaries").unwrap();
        let text = "N".repeat(70000);
        let schema = json!({"type":"object","properties":{"declared":{"$ref":"#/$defs/Alias"},"custom":{"type":"object"},"mapped":{"type":"object","additionalProperties":{"$ref":"#/$defs/Body"}}}});
        let defs = json!({"Alias":{"anyOf":[{"type":"null"},{"$ref":"#/$defs/Body"}]}});
        let mut value = json!({"declared":artex_core::body(&text),"custom":artex_core::body(&text),"mapped":{"one":artex_core::body(&text)}});
        normalize_body_fields(
            &schema,
            &defs,
            &mut value,
            &mut store,
            true,
            0,
            &mut Default::default(),
        )
        .unwrap();
        assert!(value["declared"]["text"].is_null());
        assert!(value["declared"]["blob"].is_object());
        assert_eq!(value["custom"]["text"], text);
        assert_eq!(value["mapped"]["one"], value["declared"]);
        let mut again = json!({"declared":artex_core::body(&text),"custom":artex_core::body(&text),"mapped":{"one":artex_core::body(&text)}});
        normalize_body_fields(
            &schema,
            &defs,
            &mut again,
            &mut store,
            true,
            0,
            &mut Default::default(),
        )
        .unwrap();
        assert_eq!(again, value);
        assert_eq!(
            store.read_generation().commit_seq,
            0,
            "Blob normalization writes no journal command or object change"
        );
        let mut readonly = json!({"declared":artex_core::body(&text)});
        assert_eq!(
            normalize_body_fields(
                &schema,
                &defs,
                &mut readonly,
                &mut store,
                false,
                0,
                &mut Default::default()
            )
            .unwrap_err()
            .code,
            "RECOVERY_REQUIRED"
        );
    }
    #[test]
    fn direct_native_open_and_query_never_negotiate_a_wire_handshake() {
        let dir = tempfile::tempdir().unwrap();
        let mut router = Router::new(Default::default());
        let (workspace, _) = router
            .native_open(&OpenOptions {
                path: dir.path().join("w"),
                mode: OpenMode::Create {
                    name: "Direct application".into(),
                },
                read_only: false,
            })
            .unwrap();
        assert!(router.host_epoch.is_none());
        assert!(router.seen_ids.is_empty());
        let result = router
            .native_query(
                &NativeTicket::new(),
                &workspace.scope,
                NativeQuery::WorkspaceInfo,
            )
            .unwrap();
        let NativeDispatch::Complete(result) = result else {
            panic!("Workspace info must be synchronous")
        };
        assert!(result.value.get("jsonrpc").is_none());
        assert_eq!(result.scope, Some(workspace.scope.clone()));
        router.native_close(&workspace.scope).unwrap();
    }
}
