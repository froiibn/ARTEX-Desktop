#![forbid(unsafe_code)]
pub mod models;

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::collections::{BTreeMap, BTreeSet};
use std::sync::OnceLock;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Error {
    pub code: String,
    pub message: String,
    pub retryable: bool,
    pub recovery_required: bool,
    pub details: Value,
}
impl Error {
    pub fn new(code: impl Into<String>, message: &str) -> Self {
        Self {
            code: code.into(),
            message: message.into(),
            retryable: false,
            recovery_required: false,
            details: Value::Null,
        }
    }
    pub fn io(error: std::io::Error) -> Self {
        let mut result = Self::new("IO_FAILURE", &error.to_string());
        result.details = json!({"kind": format!("{:?}", error.kind())});
        result
    }
}
impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}: {}", self.code, self.message)
    }
}
impl std::error::Error for Error {}
impl From<std::io::Error> for Error {
    fn from(e: std::io::Error) -> Self {
        Self::io(e)
    }
}
impl From<serde_json::Error> for Error {
    fn from(e: serde_json::Error) -> Self {
        Self::new("PARSE_ERROR", &e.to_string())
    }
}
pub type Result<T> = std::result::Result<T, Error>;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct Header {
    pub id: String,
    pub collection: String,
    pub version: String,
    pub commit_seq: String,
    pub created_at: String,
    pub updated_at: String,
    pub legacy_key: Value,
}
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct Record {
    pub header: Header,
    pub data: Value,
}
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Expected {
    pub collection: String,
    pub id: String,
    pub version: String,
}
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "op", rename_all = "lowercase", deny_unknown_fields)]
pub enum Change {
    Put {
        collection: String,
        id: String,
        data: Value,
    },
    Delete {
        collection: String,
        id: String,
    },
}
impl Change {
    pub fn key(&self) -> (&str, &str) {
        match self {
            Self::Put { collection, id, .. } | Self::Delete { collection, id } => (collection, id),
        }
    }
}
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct CommandBatch {
    pub command_id: String,
    pub method: String,
    pub normalized_params: Value,
    pub expected: Vec<Expected>,
    pub changes: Vec<Change>,
    pub events: Vec<Record>,
    pub deliveries: Vec<Record>,
    pub result: Value,
}
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Receipt {
    pub command_id: String,
    pub commit_seq: String,
    pub result: Value,
}
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Snapshot {
    pub workspace_id: String,
    pub commit_seq: u64,
    pub collections: BTreeMap<String, BTreeMap<String, Record>>,
}
impl Snapshot {
    /// A streaming display cursor remains resumable after an atomic replacement.
    /// The replacement event is journaled with the final transcript, so the
    /// boundary also survives a checkpoint/restart without exposing store state.
    pub fn replaced_activity_seq(&self, collection: &str, id: &str) -> Option<u64> {
        if !matches!(collection, "activity" | "conversation_activities") {
            return None;
        }
        self.records("domain_events")
            .into_iter()
            .filter(|r| {
                r.data["kind"] == "ActivityReplaced"
                    && r.data["subject_collection"] == collection
                    && r.data["subject_id"] == id
            })
            .filter_map(|r| r.header.commit_seq.parse().ok())
            .max()
    }
    pub fn get(&self, collection: &str, id: &str) -> Option<&Record> {
        self.collections.get(collection)?.get(id)
    }
    pub fn records(&self, collection: &str) -> Vec<&Record> {
        self.collections
            .get(collection)
            .map(|v| v.values().collect())
            .unwrap_or_default()
    }
}
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Actor {
    pub kind: String,
    pub agent_id: Option<String>,
    pub run_id: Option<String>,
    pub engine_epoch: String,
}
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Context {
    pub actor: Actor,
    pub task_id: Option<String>,
    pub intent_id: Option<String>,
    pub conversation_id: Option<String>,
}

pub fn random_128() -> [u8; 16] {
    let mut bytes = [0; 16];
    getrandom::fill(&mut bytes).expect("OS cryptographic randomness is unavailable");
    bytes
}
pub fn new_id(prefix: &str) -> String {
    let bytes = random_128();
    let mut id = String::with_capacity(prefix.len() + 33);
    id.push_str(prefix);
    id.push('_');
    for byte in bytes {
        use std::fmt::Write;
        write!(&mut id, "{byte:02x}").unwrap();
    }
    id
}
pub fn now() -> String {
    chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true)
}
pub fn body(text: &str) -> Value {
    json!({"text":text,"blob":null})
}
pub fn parse_u64(value: &str) -> Result<u64> {
    if value.is_empty()
        || (value.len() > 1 && value.starts_with('0'))
        || !value.bytes().all(|b| b.is_ascii_digit())
    {
        return Err(Error::new(
            "INVALID_PARAMS",
            "Expected canonical unsigned decimal string",
        ));
    }
    value
        .parse()
        .map_err(|_| Error::new("INVALID_PARAMS", "Unsigned integer overflow"))
}

pub mod contracts {
    use super::*;
    pub fn read(name: &str) -> Value {
        let source = match name {
            "data-model.v1.json" => include_str!("../../../contracts/data-model.v1.json"),
            "entities.v1.schema.json" => include_str!("../../../contracts/entities.v1.schema.json"),
            "collection-registry.v1.json" => {
                include_str!("../../../contracts/collection-registry.v1.json")
            }
            "storage.v1.schema.json" => include_str!("../../../contracts/storage.v1.schema.json"),
            "rpc.v1.schema.json" => include_str!("../../../contracts/rpc.v1.schema.json"),
            "agent-message.v1.schema.json" => {
                include_str!("../../../contracts/agent-message.v1.schema.json")
            }
            "tools.v1.json" => include_str!("../../../contracts/tools.v1.json"),
            "tools-source.v1.json" => include_str!("../../../contracts/tools-source.v1.json"),
            "ui-dtos.v1.json" => include_str!("../../../contracts/ui-dtos.v1.json"),
            "ui-wire-overrides.v1.json" => {
                include_str!("../../../contracts/ui-wire-overrides.v1.json")
            }
            "legacy-routes.v1.json" => include_str!("../../../contracts/legacy-routes.v1.json"),
            "source-manifest.v1.json" => include_str!("../../../contracts/source-manifest.v1.json"),
            "examples.v1.json" => include_str!("../../../contracts/examples.v1.json"),
            "journal-vector.v1.json" => include_str!("../../../contracts/journal-vector.v1.json"),
            _ => return Value::Null,
        };
        serde_json::from_str(source).expect("Bundled frozen contract is valid JSON")
    }
}

fn entities() -> &'static Value {
    static CONTRACT: OnceLock<Value> = OnceLock::new();
    CONTRACT.get_or_init(|| contracts::read("entities.v1.schema.json"))
}
pub fn registry() -> &'static Value {
    static CONTRACT: OnceLock<Value> = OnceLock::new();
    CONTRACT.get_or_init(|| contracts::read("collection-registry.v1.json"))
}
pub fn data_model() -> &'static Value {
    static CONTRACT: OnceLock<Value> = OnceLock::new();
    CONTRACT.get_or_init(|| contracts::read("data-model.v1.json"))
}
pub fn collection_prefix(collection: &str) -> Result<&'static str> {
    registry()["collections"]
        .as_array()
        .unwrap()
        .iter()
        .find(|c| c["collection"] == collection && c["kind"] != "read_projection")
        .and_then(|c| c["id_prefix"].as_str())
        .ok_or_else(|| Error::new("INVALID_PARAMS", "Unknown or read-only collection"))
}
pub fn valid_id(value: &str, prefix: &str) -> bool {
    value
        .strip_prefix(prefix)
        .and_then(|s| s.strip_prefix('_'))
        .is_some_and(|s| {
            s.len() == 32
                && s.bytes()
                    .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
        })
}
fn schema_name(collection: &str) -> Result<&'static str> {
    registry()["collections"]
        .as_array()
        .unwrap()
        .iter()
        .find(|c| c["collection"] == collection && c["kind"] != "read_projection")
        .and_then(|c| c["schema"].as_str())
        .ok_or_else(|| Error::new("INVALID_PARAMS", "Unknown or read-only collection"))
}
fn record_schema(collection: &str) -> Result<Value> {
    let name = schema_name(collection)?;
    let def = &entities()["$defs"][name];
    let mut root = if def["properties"].get("header").is_some() {
        def.clone()
    } else {
        json!({"type":"object","properties":{
            "header":{"type":"object","properties":{
                "id":{"type":"string","pattern":format!("^{}_[0-9a-f]{{32}}$",collection_prefix(collection)?)},
                "collection":{"const":collection},"version":{"$ref":"#/$defs/UInt64"},
                "commit_seq":{"$ref":"#/$defs/UInt64"},"created_at":{"$ref":"#/$defs/Timestamp"},
                "updated_at":{"$ref":"#/$defs/Timestamp"},"legacy_key":{"type":["null","string"]}},
                "required":["id","collection","version","commit_seq","created_at","updated_at","legacy_key"],"additionalProperties":false},
            "data":{"$ref":format!("#/$defs/{name}")}},"required":["header","data"],"additionalProperties":false})
    };
    root["$defs"] = entities()["$defs"].clone();
    root["$schema"] = json!("https://json-schema.org/draft/2020-12/schema");
    Ok(root)
}
fn validators() -> &'static BTreeMap<String, jsonschema::Validator> {
    static VALIDATORS: OnceLock<BTreeMap<String, jsonschema::Validator>> = OnceLock::new();
    VALIDATORS.get_or_init(|| {
        registry()["collections"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|c| c["kind"] != "read_projection")
            .map(|c| {
                let name = c["collection"].as_str().unwrap();
                let validator = jsonschema::options()
                    .with_draft(jsonschema::Draft::Draft202012)
                    .should_validate_formats(true)
                    .build(&record_schema(name).unwrap())
                    .expect("Frozen schema compiles");
                (name.into(), validator)
            })
            .collect()
    })
}
pub fn validate_record(record: &Record) -> Result<()> {
    let validator = validators()
        .get(&record.header.collection)
        .ok_or_else(|| Error::new("INVALID_PARAMS", "Unknown collection"))?;
    let value = serde_json::to_value(record)?;
    if let Err(e) = validator.validate(&value) {
        let mut result = Error::new("INVALID_PARAMS", &e.to_string());
        result.details = json!({"collection":record.header.collection,"id":record.header.id});
        return Err(result);
    }
    if parse_u64(&record.header.version)? == 0 || parse_u64(&record.header.commit_seq)? == 0 {
        return Err(Error::new(
            "INVALID_PARAMS",
            "Record version and commit sequence start at one",
        ));
    }
    if let Some(Value::String(id)) = record.data.get("id") {
        if valid_id(id, collection_prefix(&record.header.collection)?) && id != &record.header.id {
            return Err(Error::new(
                "INVALID_PARAMS",
                "Record header and primary ID differ",
            ));
        }
    }
    let definition = &entities()["$defs"][schema_name(&record.header.collection)?];
    if definition["properties"].get("header").is_some() {
        check_ranges(&value, definition, entities())?;
    } else {
        check_ranges(&record.data, definition, entities())?;
    }
    check_table_limits(record)?;
    validate_known_credentials(&record.header.collection, &record.data)?;
    Ok(())
}

fn credential_reference(value: &Value, path: &str) -> Result<()> {
    if value.is_null()
        || value.as_str() == Some("")
        || value.as_object().is_some_and(|m| m.is_empty())
    {
        return Ok(());
    }
    if value.as_object().is_some_and(|m| {
        m.len() == 2
            && m.get("secret_ref")
                .and_then(Value::as_str)
                .is_some_and(|s| valid_id(s, "secret"))
            && m.get("hint").is_some_and(Value::is_string)
    }) {
        return Ok(());
    }
    Err(Error::new(
        "PERMISSION_DENIED",
        &format!("Credential field {path} must be normalized to SecretRef before persistence"),
    ))
}
fn credential_env(env: &Value) -> Result<()> {
    if env.get("secret_ref").is_some() {
        return credential_reference(env, "env");
    }
    if let Some(environment) = env.as_object() {
        for (key, value) in environment {
            let normalized = key.to_ascii_uppercase();
            let sensitive = [
                "API_KEY",
                "TOKEN",
                "PASSWORD",
                "PASSWD",
                "SECRET",
                "CLIENT_SECRET",
                "ACCESS_KEY",
                "PRIVATE_KEY",
                "AUTHORIZATION",
            ]
            .iter()
            .any(|suffix| normalized == *suffix || normalized.ends_with(&format!("_{suffix}")));
            if sensitive {
                credential_reference(value, &format!("env.{key}"))?;
            } else if value.is_object() {
                credential_reference(value, &format!("env.{key}"))?;
            }
            if value.as_str().is_some_and(url_has_userinfo) {
                return Err(Error::new(
                    "PERMISSION_DENIED",
                    "Environment URL userinfo must be separated into SecretRef before persistence",
                ));
            }
        }
    }
    Ok(())
}
fn credential_headers(headers: &Value) -> Result<()> {
    if headers.get("secret_ref").is_some() {
        return credential_reference(headers, "headers");
    }
    if let Some(entries) = headers.as_object() {
        for (key, value) in entries {
            let key_upper = key.to_ascii_uppercase().replace('-', "_");
            let sensitive = matches!(
                key_upper.as_str(),
                "AUTHORIZATION"
                    | "PROXY_AUTHORIZATION"
                    | "COOKIE"
                    | "SET_COOKIE"
                    | "API_KEY"
                    | "APIKEY"
            ) || [
                "API_KEY",
                "AUTH_TOKEN",
                "ACCESS_TOKEN",
                "TOKEN",
                "SECRET",
                "PASSWORD",
            ]
            .iter()
            .any(|suffix| key_upper == *suffix || key_upper.ends_with(&format!("_{suffix}")));
            if sensitive || value.is_object() {
                credential_reference(value, &format!("headers.{key}"))?;
            }
        }
    }
    Ok(())
}
fn url_has_userinfo(value: &str) -> bool {
    value.split_once("://").is_some_and(|(_, rest)| {
        rest.split(['/', '?', '#'])
            .next()
            .is_some_and(|authority| authority.contains('@'))
    })
}
fn credential_auth(auth: &Value) -> Result<()> {
    if let Some(entries) = auth.as_array() {
        for entry in entries {
            for key in ["password", "token"] {
                if let Some(value) = entry.get(key) {
                    credential_reference(value, &format!("auth.{key}"))?;
                }
            }
        }
    }
    Ok(())
}
fn credential_notification(kind: &str, config: &Value) -> Result<()> {
    let keys: &[&str] = match kind {
        "email" => &["password"],
        "telegram" => &["bot_token"],
        "dingtalk" | "feishu" => &["webhook", "secret"],
        "wecom" => &["webhook"],
        "webhook" => &["url", "headers"],
        _ => &[],
    };
    for key in keys {
        if let Some(value) = config.get(*key) {
            credential_reference(value, &format!("config.{key}"))?;
        }
    }
    Ok(())
}
/// The known credential paths come from db.AuthItem and notify.Channel.SecretKeys.
/// Environment variable matching is limited to standard credential names/suffixes;
/// authentication headers and native sealed maps are checked separately.
/// Ordinary PATH/LANG/command arguments and opaque evidence JSON remain unchanged.
pub fn validate_known_credentials(collection: &str, data: &Value) -> Result<()> {
    match collection {
        "assets" => credential_auth(&data["auth"]),
        "mcp_servers" => {
            credential_env(&data["env"])?;
            credential_headers(&data["headers"])?;
            if matches!(data["transport"].as_str(), Some("http" | "sse")) {
                credential_headers(&data["env"])?;
            }
            if data["url"].as_str().is_some_and(url_has_userinfo) {
                return Err(Error::new(
                    "PERMISSION_DENIED",
                    "MCP URL userinfo must be separated into SecretRef",
                ));
            }
            Ok(())
        }
        "notification_channels" => {
            credential_notification(data["kind"].as_str().unwrap_or(""), &data["config"])
        }
        "tools" => {
            credential_env(&data["exec"]["env"])?;
            credential_headers(&data["exec"]["headers"])
        }
        _ => Ok(()),
    }
}
/// Prevent normalized journal inputs from retaining credentials removed from records.
pub fn validate_secret_parameters(method: &str, params: &Value) -> Result<()> {
    let method = method.to_ascii_lowercase();
    credential_auth(&params["auth"])?;
    credential_env(&params["env"])?;
    credential_env(&params["exec"]["env"])?;
    if method.contains("custom_tool") || method.contains("customtool") || method.contains("mcp") {
        credential_headers(&params["headers"])?;
        credential_headers(&params["exec"]["headers"])?;
    }
    if let Some(assets) = params["assets"].as_array() {
        for asset in assets {
            credential_auth(&asset["auth"])?;
        }
    }
    for key in ["api_key", "password", "token", "secret", "bot_token"] {
        if let Some(value) = params.get(key) {
            credential_reference(value, key)?;
        }
    }
    if method.contains("mcp") && matches!(params["transport"].as_str(), Some("http" | "sse")) {
        credential_headers(&params["env"])?;
    }
    if method.contains("notification") {
        credential_notification(params["kind"].as_str().unwrap_or(""), &params["config"])?;
        for key in ["password", "bot_token", "secret", "webhook", "headers"] {
            if let Some(value) = params["config"].get(key) {
                credential_reference(value, &format!("config.{key}"))?;
            }
        }
    }
    Ok(())
}
fn check_table_limits(record: &Record) -> Result<()> {
    let data = &record.data;
    let bound = |field: &str, low: i64, high: i64| -> Result<()> {
        if let Some(value) = data[field].as_i64() {
            if value < low || value > high {
                return Err(Error::new(
                    "INVALID_PARAMS",
                    &format!("{field} is outside its SQL CHECK range"),
                ));
            }
        }
        Ok(())
    };
    match record.header.collection.as_str() {
        "assets" => bound("port", 1, 65535)?,
        "task_archives" => bound("progress", 0, 100)?,
        "task_llm_profiles" => bound("position", 0, i32::MAX as i64)?,
        "llm_profiles" => {
            bound("max_tokens", 0, i32::MAX as i64)?;
            for field in [
                "retry_connect_attempts",
                "retry_empty_attempts",
                "retry_stream_attempts",
            ] {
                bound(field, -1, i32::MAX as i64)?;
            }
            for field in [
                "retry_connect_interval_ms",
                "retry_empty_interval_ms",
                "retry_stream_interval_ms",
            ] {
                bound(field, 0, i32::MAX as i64)?;
            }
            if !["", "max_completion_tokens"]
                .contains(&data["max_tokens_field"].as_str().unwrap_or(""))
            {
                return Err(Error::new("INVALID_PARAMS", "Unknown max_tokens_field"));
            }
        }
        "agents" => {
            static KEY: OnceLock<regex::Regex> = OnceLock::new();
            if !KEY
                .get_or_init(|| regex::Regex::new("^[a-z][a-z0-9_]*$").unwrap())
                .is_match(data["key"].as_str().unwrap_or(""))
            {
                return Err(Error::new(
                    "INVALID_PARAMS",
                    "Agent key must match its SQL CHECK",
                ));
            }
        }
        _ => (),
    }
    Ok(())
}
/// Common schema boundary for RPC/tool inputs, including ARTEX's integer annotations.
pub fn validate_contract(schema: &Value, defs: &Value, value: &Value) -> Result<()> {
    let mut root = schema.clone();
    root["$defs"] = defs.get("$defs").unwrap_or(defs).clone();
    root["$schema"] = json!("https://json-schema.org/draft/2020-12/schema");
    let validator = jsonschema::options()
        .with_draft(jsonschema::Draft::Draft202012)
        .should_validate_formats(true)
        .build(&root)
        .map_err(|e| Error::new("INVALID_PARAMS", &e.to_string()))?;
    validator
        .validate(value)
        .map_err(|e| Error::new("INVALID_PARAMS", &e.to_string()))?;
    check_ranges(value, &root, &root)
}
fn check_ranges(value: &Value, schema: &Value, root: &Value) -> Result<()> {
    if let Some(reference) = schema["$ref"].as_str() {
        if let Some(name) = reference.strip_prefix("#/$defs/") {
            if name == "Body" && value["text"].as_str().is_some_and(|s| s.len() > 65536) {
                return Err(Error::new(
                    "SIZE_LIMIT",
                    "Body text exceeds 64 KiB; use BlobRef",
                ));
            }
            return check_ranges(value, &root["$defs"][name], root);
        }
    }
    if let Some(max) = schema["x-max"].as_str() {
        let number = value
            .as_str()
            .ok_or_else(|| Error::new("INVALID_PARAMS", "Integer string required"))?;
        if parse_u64(number)? > parse_u64(max)? {
            return Err(Error::new(
                "INVALID_PARAMS",
                "Unsigned integer range exceeded",
            ));
        }
    }
    if let Some(range) = schema["x-range"].as_array() {
        if let Some(v) = value.as_str() {
            let parsed = v
                .parse::<i128>()
                .map_err(|_| Error::new("INVALID_PARAMS", "Signed integer range exceeded"))?;
            let low = range[0].as_str().unwrap().parse::<i128>().unwrap();
            let high = range[1].as_str().unwrap().parse::<i128>().unwrap();
            if parsed < low || parsed > high {
                return Err(Error::new(
                    "INVALID_PARAMS",
                    "Signed integer range exceeded",
                ));
            }
        }
    }
    if schema["x-range"] == "int64" && value.as_str().is_some_and(|s| s.parse::<i64>().is_err()) {
        return Err(Error::new(
            "INVALID_PARAMS",
            "Signed integer range exceeded",
        ));
    }
    for option in ["anyOf", "oneOf"] {
        if let Some(branches) = schema[option].as_array() {
            for branch in branches {
                if branch_shape_matches(value, branch, root) {
                    check_ranges(value, branch, root)?;
                }
            }
        }
    }
    if let Some(branches) = schema["allOf"].as_array() {
        for branch in branches {
            check_ranges(value, branch, root)?;
        }
    }
    if let Some(object) = value.as_object() {
        for (key, child) in object {
            if let Some(child_schema) = schema["properties"].get(key) {
                check_ranges(child, child_schema, root)?;
            } else if schema["additionalProperties"].is_object() {
                check_ranges(child, &schema["additionalProperties"], root)?;
            }
        }
    }
    if let Some(array) = value.as_array() {
        for child in array {
            check_ranges(child, &schema["items"], root)?;
        }
    }
    Ok(())
}

fn branch_shape_matches(value: &Value, schema: &Value, root: &Value) -> bool {
    if let Some(name) = schema["$ref"]
        .as_str()
        .and_then(|s| s.strip_prefix("#/$defs/"))
    {
        return branch_shape_matches(value, &root["$defs"][name], root);
    }
    let matches_type = |ty: &str| match ty {
        "null" => value.is_null(),
        "string" => value.is_string(),
        "object" => value.is_object(),
        "array" => value.is_array(),
        "boolean" => value.is_boolean(),
        "number" => value.is_number(),
        "integer" => value.as_i64().is_some() || value.as_u64().is_some(),
        _ => true,
    };
    if let Some(ty) = schema["type"].as_str() {
        if !matches_type(ty) {
            return false;
        }
    }
    if let Some(types) = schema["type"].as_array() {
        if !types.iter().filter_map(Value::as_str).any(matches_type) {
            return false;
        }
    }
    if let Some(constant) = schema.get("const") {
        if constant != value {
            return false;
        }
    }
    if let Some(allowed) = schema["enum"].as_array() {
        if !allowed.contains(value) {
            return false;
        }
    }
    if let (Some(pattern), Some(text)) = (schema["pattern"].as_str(), value.as_str()) {
        if !regex::Regex::new(pattern).is_ok_and(|r| r.is_match(text)) {
            return false;
        }
    }
    if let Some(required) = schema["required"].as_array() {
        if required
            .iter()
            .filter_map(Value::as_str)
            .any(|k| value.get(k).is_none())
        {
            return false;
        }
    }
    if let Some(properties) = schema["properties"].as_object() {
        for (key, child_schema) in properties {
            if let Some(child) = value.get(key) {
                if !branch_shape_matches(child, child_schema, root) {
                    return false;
                }
            }
        }
    }
    true
}
pub fn default_data(collection: &str) -> Result<Value> {
    let schema = &entities()["$defs"][schema_name(collection)?];
    let data = schema["properties"].get("data").unwrap_or(schema);
    let mut value = default_for(data);
    if let Some(table) = data_model()["tables"]
        .as_array()
        .unwrap()
        .iter()
        .find(|t| t["name"] == collection)
    {
        for column in table["columns"].as_array().unwrap() {
            let name = column["name"].as_str().unwrap();
            let decl = column["declaration"].as_str().unwrap_or("");
            if let Some(default) = sql_default(decl, &column["desktop_schema"]) {
                value[name] = default;
            }
        }
    }
    // Desktop-only optional fields leave pre-extension snapshots/WAL valid.
    // data-model.v1.json continues to describe the original SQL source.
    if collection == "conversations" {
        value["trashed_at"] = Value::Null;
        value["trash_generation"] = Value::Null;
    }
    Ok(value)
}
fn default_for(schema: &Value) -> Value {
    if let Some(name) = schema["$ref"]
        .as_str()
        .and_then(|s| s.strip_prefix("#/$defs/"))
    {
        if name == "Body" {
            return body("");
        }
        return default_for(&entities()["$defs"][name]);
    }
    if let Some(v) = schema.get("default") {
        return v.clone();
    }
    if let Some(v) = schema.get("const") {
        return v.clone();
    }
    if let Some(options) = schema["anyOf"]
        .as_array()
        .or_else(|| schema["oneOf"].as_array())
    {
        if options.iter().any(|s| s["type"] == "null") {
            return Value::Null;
        }
        return default_for(&options[0]);
    }
    if let Some(values) = schema["enum"].as_array() {
        return values[0].clone();
    }
    match schema["type"].as_str().unwrap_or("") {
        "object" => {
            let mut result = serde_json::Map::new();
            if let Some(required) = schema["required"].as_array() {
                for key in required {
                    let key = key.as_str().unwrap();
                    result.insert(key.into(), default_for(&schema["properties"][key]));
                }
            }
            Value::Object(result)
        }
        "array" => json!([]),
        "boolean" => json!(false),
        "integer" | "number" => json!(0),
        "string" => {
            if schema["format"] == "date-time" {
                json!(now())
            } else if schema["pattern"]
                .as_str()
                .is_some_and(|s| s.contains("[1-9][0-9]*"))
            {
                json!("0")
            } else {
                json!("")
            }
        }
        _ => Value::Null,
    }
}
fn sql_default(declaration: &str, schema: &Value) -> Option<Value> {
    let re =
        regex::Regex::new(r"(?i)\bDEFAULT\s+('(?:[^']|'')*'|now\(\)|true|false|-?\d+(?:\.\d+)?)")
            .unwrap();
    let capture = re.captures(declaration)?;
    let token = &capture[1];
    if token.eq_ignore_ascii_case("now()") {
        return Some(json!(now()));
    }
    let decoded = if token.starts_with('\'') {
        token[1..token.len() - 1].replace("''", "'")
    } else {
        token.into()
    };
    if schema["$ref"] == "#/$defs/Body"
        || schema["anyOf"]
            .as_array()
            .is_some_and(|v| v.iter().any(|s| s["$ref"] == "#/$defs/Body"))
    {
        return Some(body(&decoded));
    }
    if declaration.contains("[]") && decoded == "{}" {
        return Some(json!([]));
    }
    if declaration.contains("JSONB") {
        return serde_json::from_str(&decoded).ok();
    }
    if schema["type"] == "string" {
        return Some(json!(decoded));
    }
    serde_json::from_str(&decoded).ok().or(Some(json!(decoded)))
}

#[derive(Debug, Clone)]
pub struct ForeignKey {
    pub collection: String,
    pub columns: Vec<String>,
    pub target: String,
    pub target_columns: Vec<String>,
    pub on_delete: String,
}
pub fn foreign_keys() -> &'static Vec<ForeignKey> {
    static KEYS: OnceLock<Vec<ForeignKey>> = OnceLock::new();
    KEYS.get_or_init(|| {
        let mut keys=Vec::new();
        let reference=regex::Regex::new(r"(?i)REFERENCES\s+(\w+)\s*\(([^)]+)\)(?:\s+ON\s+DELETE\s+(CASCADE|SET NULL|RESTRICT|NO ACTION))?").unwrap();
        let column_re=regex::Regex::new(r"(?i)FOREIGN\s+KEY\s*\(([^)]+)\)").unwrap();
        for table in data_model()["tables"].as_array().unwrap() {
            let collection=table["name"].as_str().unwrap();
            for column in table["columns"].as_array().unwrap() {
                if let Some(c)=reference.captures(column["declaration"].as_str().unwrap_or("")) {
                    keys.push(ForeignKey { collection:collection.into(),columns:vec![column["name"].as_str().unwrap().into()],
                        target:c[1].into(), target_columns:fields(&c[2]),on_delete:c.get(3).map(|v|v.as_str()).unwrap_or("RESTRICT").into() });
                }
            }
            for constraint in table["constraints"].as_array().unwrap() {
                let text=constraint.as_str().unwrap();
                if let (Some(c),Some(cols))=(reference.captures(text),column_re.captures(text)) {
                    keys.push(ForeignKey {collection:collection.into(),columns:fields(&cols[1]),target:c[1].into(),target_columns:fields(&c[2]),on_delete:c.get(3).map(|v|v.as_str()).unwrap_or("RESTRICT").into()});
                }
            }
        }
        for constraint in data_model()["additional_constraints"].as_array().unwrap() {
            let text=constraint["declaration"].as_str().unwrap();
            if let (Some(c),Some(cols))=(reference.captures(text),column_re.captures(text)) {
                keys.push(ForeignKey {collection:constraint["collection"].as_str().unwrap().into(),columns:fields(&cols[1]),target:c[1].into(),target_columns:fields(&c[2]),on_delete:c.get(3).map(|v|v.as_str()).unwrap_or("RESTRICT").into()});
            }
        }
        // SQLite stores the relationship in its recorder code rather than a SQL FK.
        keys.push(ForeignKey {collection:"exchange_bodies".into(),columns:vec!["id".into()],target:"exchanges".into(),target_columns:vec!["id".into()],on_delete:"CASCADE".into()});
        keys.push(ForeignKey {collection:"blob_refs".into(),columns:vec!["exchange_id".into()],target:"exchanges".into(),target_columns:vec!["id".into()],on_delete:"CASCADE".into()});
        keys
    })
}
fn fields(value: &str) -> Vec<String> {
    value.split(',').map(|s| s.trim().to_owned()).collect()
}
fn same_fields(
    data: &Value,
    columns: &[String],
    target: &Value,
    target_columns: &[String],
) -> bool {
    columns
        .iter()
        .zip(target_columns)
        .all(|(a, b)| data.get(a).is_some() && data[a] == target[b])
}
pub fn expand_deletions(snapshot: &Snapshot, changes: &[Change]) -> Result<Vec<Change>> {
    let mut final_changes: BTreeMap<(String, String), Change> = changes
        .iter()
        .map(|c| {
            let (k, id) = c.key();
            ((k.into(), id.into()), c.clone())
        })
        .collect();
    loop {
        let deleted: Vec<_> = final_changes
            .values()
            .filter_map(|c| match c {
                Change::Delete { collection, id } => snapshot
                    .get(collection, id)
                    .map(|r| (collection.clone(), r.clone())),
                _ => None,
            })
            .collect();
        let mut added = false;
        for (collection, target) in deleted {
            for fk in foreign_keys().iter().filter(|k| k.target == collection) {
                for old in snapshot.records(&fk.collection) {
                    let key = (fk.collection.clone(), old.header.id.clone());
                    let effective = match final_changes.get(&key) {
                        Some(Change::Delete { .. }) => continue,
                        Some(Change::Put { data, .. }) => data,
                        None => &old.data,
                    };
                    if !same_fields(effective, &fk.columns, &target.data, &fk.target_columns) {
                        continue;
                    }
                    match fk.on_delete.as_str() {
                        "CASCADE" => {
                            final_changes.insert(
                                key,
                                Change::Delete {
                                    collection: fk.collection.clone(),
                                    id: old.header.id.clone(),
                                },
                            );
                            added = true;
                        }
                        "SET NULL" => {
                            let mut data = effective.clone();
                            for column in &fk.columns {
                                data[column] = Value::Null;
                            }
                            if fk.collection == "finding_retests"
                                && fk.columns.contains(&"conversation_id".into())
                                && ["pending", "running"]
                                    .contains(&data["status"].as_str().unwrap_or(""))
                            {
                                data["status"] = json!("stopped");
                                data["error"] = json!("Retest session deleted");
                                if data.get("finished_at").is_some() {
                                    data["finished_at"] = json!(now());
                                }
                            }
                            final_changes.insert(
                                key,
                                Change::Put {
                                    collection: fk.collection.clone(),
                                    id: old.header.id.clone(),
                                    data,
                                },
                            );
                            added = true;
                        }
                        _ => {
                            return Err(Error::new(
                                "VERSION_CONFLICT",
                                "Referenced entity cannot be deleted",
                            ));
                        }
                    }
                }
            }
        }
        if !added {
            break;
        }
    }
    Ok(final_changes.into_values().collect())
}
pub fn validate_snapshot(snapshot: &Snapshot) -> Result<()> {
    for (collection, records) in &snapshot.collections {
        collection_prefix(collection)?;
        for (id, record) in records {
            if record.header.id != *id || record.header.collection != *collection {
                return Err(Error::new(
                    "INVALID_PARAMS",
                    "Collection index and header mismatch",
                ));
            }
            validate_record(record)?;
            if parse_u64(&record.header.commit_seq)? > snapshot.commit_seq {
                return Err(Error::new(
                    "INVALID_PARAMS",
                    "Record commit sequence is in the future",
                ));
            }
        }
    }
    for fk in foreign_keys() {
        for record in snapshot.records(&fk.collection) {
            if fk
                .columns
                .iter()
                .any(|column| record.data[column].is_null())
            {
                continue;
            }
            if !snapshot.records(&fk.target).iter().any(|target| {
                same_fields(&record.data, &fk.columns, &target.data, &fk.target_columns)
            }) {
                let mut e = Error::new("VERSION_CONFLICT", "Foreign key target does not exist");
                e.details = json!({"collection":fk.collection,"id":record.header.id,"columns":fk.columns,"target":fk.target});
                return Err(e);
            }
        }
    }
    validate_unique(snapshot)?;
    validate_semantics(snapshot)
}
fn validate_unique(snapshot: &Snapshot) -> Result<()> {
    let composite = regex::Regex::new(r"(?i)(?:PRIMARY KEY|UNIQUE)\s*\(([^)]+)\)").unwrap();
    for table in data_model()["tables"].as_array().unwrap() {
        let collection = table["name"].as_str().unwrap();
        let mut keys = Vec::new();
        for column in table["columns"].as_array().unwrap() {
            let decl = column["declaration"].as_str().unwrap_or("");
            if decl.contains("UNIQUE") || decl.contains("PRIMARY KEY") {
                keys.push(vec![column["name"].as_str().unwrap().into()]);
            }
        }
        for constraint in table["constraints"].as_array().unwrap() {
            for c in composite.captures_iter(constraint.as_str().unwrap()) {
                keys.push(fields(&c[1]));
            }
        }
        for columns in keys {
            let mut seen = BTreeSet::new();
            for r in snapshot.records(collection) {
                if columns.iter().any(|c| r.data[c].is_null()) {
                    continue;
                }
                let tuple: Vec<_> = columns.iter().map(|c| r.data[c].clone()).collect();
                if !seen.insert(serde_json::to_string(&tuple)?) {
                    return Err(Error::new("VERSION_CONFLICT", "Unique key already exists"));
                }
            }
        }
    }
    // Explicit partial natural keys from db/assets.go and schema.sql.
    let mut asset_keys = BTreeSet::new();
    for r in snapshot.records("assets") {
        let d = &r.data;
        let ty = d["type"].as_str().unwrap_or("");
        let text = |k: &str| d[k].as_str().unwrap_or("").to_owned();
        let tuple = match ty {
            "root_domain" => {
                if d["domain"].is_null() {
                    continue;
                } else {
                    vec![ty.into(), text("domain")]
                }
            }
            "ip" => {
                if d["ip"].is_null() {
                    continue;
                } else {
                    vec![ty.into(), text("ip")]
                }
            }
            "subdomain" => {
                if d["domain"].is_null() {
                    continue;
                } else {
                    vec![ty.into(), text("domain"), text("record_type")]
                }
            }
            "app" => {
                if !d["bundle_id"].is_null() {
                    vec![ty.into(), format!("bundle:{}", text("bundle_id"))]
                } else if d["app_name"].is_null() {
                    continue;
                } else {
                    vec![ty.into(), format!("name:{}", text("app_name"))]
                }
            }
            "service" => {
                if text("service_type") == "http" {
                    if d["url"].is_null() {
                        continue;
                    }
                    vec![ty.into(), "http".into(), text("url")]
                } else if text("service_type") == "other" {
                    if d["port"].is_null() || d["service_name"].is_null() {
                        continue;
                    }
                    vec![
                        ty.into(),
                        "other".into(),
                        text("domain"),
                        text("ip"),
                        d["port"].to_string(),
                        text("service_name"),
                    ]
                } else {
                    continue;
                }
            }
            "endpoint" => {
                if d["url"].is_null() || d["method"].is_null() {
                    continue;
                } else {
                    vec![ty.into(), text("url"), text("method")]
                }
            }
            _ => continue,
        };
        if !asset_keys.insert(serde_json::to_string(&tuple)?) {
            return Err(Error::new(
                "VERSION_CONFLICT",
                "Asset natural key already exists",
            ));
        }
    }
    unique_filtered(snapshot, "finding_retests", &["finding_id"], |r| {
        ["pending", "running"].contains(&r.data["status"].as_str().unwrap_or(""))
    })?;
    unique_filtered(
        snapshot,
        "company_scope",
        &["company_id", "kind", "value"],
        |r| ["icp", "keyword"].contains(&r.data["kind"].as_str().unwrap_or("")),
    )?;
    unique_filtered(snapshot, "llm_profiles", &["is_default"], |r| {
        r.data["is_default"] == true
    })?;
    unique_filtered(snapshot, "side_question_requests", &["session_key"], |r| {
        r.data["status"] == "running"
    })?;
    let mut scope_keys = BTreeSet::new();
    for r in snapshot.records("task_scope") {
        let key: Vec<_> = ["task_id", "kind", "domain", "net", "company_id", "value"]
            .iter()
            .map(|k| {
                if r.data[k].is_null() {
                    json!("")
                } else {
                    r.data[k].clone()
                }
            })
            .collect();
        if !scope_keys.insert(serde_json::to_string(&key)?) {
            return Err(Error::new("VERSION_CONFLICT", "Duplicate task scope"));
        }
    }
    unique_filtered(snapshot, "agent_features", &["agent_id"], |_| true)?;
    unique_filtered(snapshot, "task_runtime", &["task_id"], |_| true)?;
    unique_filtered(
        snapshot,
        "memory_records",
        &["agent_id", "name", "type"],
        |_| true,
    )?;
    unique_filtered(snapshot, "agent_deliveries", &["activation_key"], |r| {
        !r.data["activation_key"].is_null()
    })?;
    Ok(())
}
fn unique_filtered<F: Fn(&Record) -> bool>(
    s: &Snapshot,
    c: &str,
    fields: &[&str],
    filter: F,
) -> Result<()> {
    let mut seen = BTreeSet::new();
    for r in s.records(c).into_iter().filter(|r| filter(r)) {
        let key: Vec<_> = fields.iter().map(|k| r.data[k].clone()).collect();
        if !seen.insert(serde_json::to_string(&key)?) {
            return Err(Error::new(
                "VERSION_CONFLICT",
                "Unique partial key already exists",
            ));
        }
    }
    Ok(())
}
fn validate_semantics(snapshot: &Snapshot) -> Result<()> {
    for r in snapshot.records("exploration_nodes") {
        let allowed: &[&str] = match r.data["kind"].as_str().unwrap_or("") {
            "begin" => &["open"],
            "goal" => &["open", "met", "abandoned"],
            "intent" => &[
                "open",
                "running",
                "paused",
                "done",
                "blocked",
                "exhausted",
                "stopped",
                "deleted",
            ],
            "fact" => &["origin", "confirmed", "dismissed"],
            "finding" => &["confirmed", "dismissed"],
            "hint" => &["active", "consumed"],
            "digest" => &["active", "superseded"],
            _ => &[],
        };
        if !allowed.contains(&r.data["state"].as_str().unwrap_or("")) {
            return Err(Error::new(
                "INVALID_PARAMS",
                "Node kind/state combination is invalid",
            ));
        }
    }
    for (collection, a, b) in [
        ("exploration_edges", "src_id", "dst_id"),
        ("task_relations", "task_id", "source_task_id"),
    ] {
        for r in snapshot.records(collection) {
            if r.data[a] == r.data[b] {
                return Err(Error::new(
                    "VERSION_CONFLICT",
                    "Self reference is forbidden",
                ));
            }
        }
    }
    for r in snapshot.records("agents") {
        if let Some(prompt) = r.data["current_prompt_id"].as_str() {
            if snapshot
                .get("agent_prompts", prompt)
                .is_none_or(|p| p.data["agent_id"] != r.data["id"])
            {
                return Err(Error::new(
                    "VERSION_CONFLICT",
                    "Current prompt belongs to another agent",
                ));
            }
        }
    }
    for r in snapshot.records("company_scope") {
        let kind = r.data["kind"].as_str().unwrap_or("");
        let valid = match kind {
            "domain" => {
                !r.data["domain"].is_null() && r.data["net"].is_null() && r.data["value"].is_null()
            }
            "ip" | "cidr" => {
                r.data["domain"].is_null() && !r.data["net"].is_null() && r.data["value"].is_null()
            }
            "icp" | "keyword" => {
                r.data["domain"].is_null() && r.data["net"].is_null() && !r.data["value"].is_null()
            }
            _ => false,
        };
        if !valid {
            return Err(Error::new(
                "INVALID_PARAMS",
                "Company scope payload does not match kind",
            ));
        }
    }
    for r in snapshot.records("side_question_sessions") {
        let d = &r.data;
        let valid = (!d["conversation_id"].is_null()
            && d["task_id"].is_null()
            && d["exploration_id"].is_null()
            && d["intent_id"].is_null())
            || (d["conversation_id"].is_null()
                && !d["task_id"].is_null()
                && !d["exploration_id"].is_null());
        if !valid {
            return Err(Error::new(
                "VERSION_CONFLICT",
                "Side question context is inconsistent",
            ));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn integer_format_is_exact() {
        assert_eq!(parse_u64("18446744073709551615").unwrap(), u64::MAX);
        for s in ["01", "-0", "18446744073709551616", "1.0", ""] {
            assert!(parse_u64(s).is_err());
        }
    }
    #[test]
    fn frozen_all_tables_have_complete_defaults() {
        assert_eq!(data_model()["tables"].as_array().unwrap().len(), 54);
        for table in data_model()["tables"].as_array().unwrap() {
            let mut data = default_data(table["name"].as_str().unwrap()).unwrap();
            // Native trash metadata extends the frozen SQL model; it does not
            // replace or add source database columns.
            if table["name"] == "conversations" {
                for key in ["trashed_at", "trash_generation"] {
                    assert_eq!(data.as_object_mut().unwrap().remove(key), Some(Value::Null));
                }
            }
            assert_eq!(
                data.as_object().unwrap().len(),
                table["columns"].as_array().unwrap().len(),
                "{}",
                table["name"]
            );
            for column in table["columns"].as_array().unwrap() {
                assert!(data.get(column["name"].as_str().unwrap()).is_some());
            }
        }
    }
    #[test]
    fn schema_rejects_omissions_and_ranges() {
        let id = new_id("company");
        let ts = now();
        let mut data = default_data("companies").unwrap();
        data["id"] = json!(id);
        data["name"] = json!("회사");
        data["nkey"] = json!("회사");
        let mut r = Record {
            header: Header {
                id,
                collection: "companies".into(),
                version: "1".into(),
                commit_seq: "1".into(),
                created_at: ts.clone(),
                updated_at: ts,
                legacy_key: Value::Null,
            },
            data,
        };
        validate_record(&r).unwrap();
        r.header.version = "18446744073709551616".into();
        assert!(validate_record(&r).is_err());
        r.header.version = "1".into();
        r.data.as_object_mut().unwrap().remove("logo");
        assert!(validate_record(&r).is_err());
    }
    #[test]
    fn conversation_trash_extension_keeps_old_records_valid_and_checks_new_metadata() {
        let id = new_id("conversation");
        let ts = now();
        let mut data = default_data("conversations").unwrap();
        assert!(data["trashed_at"].is_null());
        assert!(data["trash_generation"].is_null());
        data["id"] = json!(id);
        data["agent_key"] = json!("auto");
        let mut record = Record {
            header: Header {
                id,
                collection: "conversations".into(),
                version: "1".into(),
                commit_seq: "1".into(),
                created_at: ts.clone(),
                updated_at: ts.clone(),
                legacy_key: Value::Null,
            },
            data,
        };
        validate_record(&record).unwrap();
        models::typed_roundtrip(&record).unwrap();
        record.data.as_object_mut().unwrap().remove("trashed_at");
        record
            .data
            .as_object_mut()
            .unwrap()
            .remove("trash_generation");
        validate_record(&record).unwrap();
        models::typed_roundtrip(&record).unwrap();
        record.data["trashed_at"] = json!(ts);
        record.data["trash_generation"] = json!(new_id("trash"));
        validate_record(&record).unwrap();
        models::typed_roundtrip(&record).unwrap();
        record.data["trash_generation"] = json!("unfenced");
        assert!(validate_record(&record).is_err());
        record.data["trash_generation"] = Value::Null;
        record.data["trashed_at"] = json!("not a timestamp");
        assert!(validate_record(&record).is_err());
    }
    #[test]
    fn known_credentials_require_references_without_rejecting_public_metadata() {
        let secret = json!({"secret_ref":new_id("secret"),"hint":"credential"});
        validate_known_credentials("assets",&json!({"auth":[{"username":"admin","description":"Test account","password":secret,"token":""}]})).unwrap();
        let error =
            validate_known_credentials("assets", &json!({"auth":[{"password":"SECRET_PROBE_7"}]}))
                .unwrap_err();
        assert_eq!(error.code, "PERMISSION_DENIED");
        assert!(!error.message.contains("SECRET_PROBE_7"));
        validate_known_credentials("mcp_servers",&json!({"env":{"PATH":"/bin","LANG":"ko_KR.UTF-8","OPENAI_API_KEY":secret},"url":"https://example.test"})).unwrap();
        assert!(
            validate_known_credentials(
                "mcp_servers",
                &json!({"env":{"GITHUB_TOKEN":"SECRET_PROBE_7"}})
            )
            .is_err()
        );
        assert!(
            validate_known_credentials(
                "mcp_servers",
                &json!({"env":{"HTTPS_PROXY":"https://user:pass@example.test"}})
            )
            .is_err()
        );
        validate_known_credentials("notification_channels",&json!({"kind":"email","config":{"host":"smtp.example.test","username":"public-account","password":secret}})).unwrap();
        for (kind, key) in [
            ("email", "password"),
            ("telegram", "bot_token"),
            ("dingtalk", "webhook"),
            ("feishu", "secret"),
            ("wecom", "webhook"),
            ("webhook", "headers"),
        ] {
            let mut config = json!({});
            config[key] = json!("SECRET_PROBE_7");
            assert!(
                validate_known_credentials(
                    "notification_channels",
                    &json!({"kind":kind,"config":config})
                )
                .is_err(),
                "{kind}.{key}"
            );
        }
    }
    #[test]
    fn custom_tool_credential_headers_are_references_in_records_and_journal_inputs() {
        let secret = json!({"secret_ref":new_id("secret"),"hint":"configured"});
        for header in [
            "Authorization",
            "Proxy-Authorization",
            "Cookie",
            "Set-Cookie",
            "X-API-Key",
            "X-Auth-Token",
        ] {
            let mut headers = json!({"Accept":"application/json"});
            headers[header] = json!("HEADER_SECRET_PROBE");
            let input = json!({"exec":{"headers":headers}});
            for result in [
                validate_known_credentials("tools", &input),
                validate_secret_parameters("create_custom_tool", &input),
            ] {
                let error = result.unwrap_err();
                assert_eq!(error.code, "PERMISSION_DENIED");
                assert!(!error.message.contains("HEADER_SECRET_PROBE"));
            }
            headers[header] = secret.clone();
            validate_known_credentials("tools", &json!({"exec":{"headers":headers}})).unwrap();
        }
        validate_known_credentials("tools", &json!({"exec":{"headers":secret,"env":secret}}))
            .unwrap();
        assert!(
            validate_known_credentials(
                "mcp_servers",
                &json!({"transport":"http","env":{"Cookie":"HEADER_SECRET_PROBE"}})
            )
            .is_err()
        );
        assert!(
            validate_secret_parameters(
                "create_mcp",
                &json!({"transport":"sse","env":{"X-Api-Key":"HEADER_SECRET_PROBE"}})
            )
            .is_err()
        );
        validate_secret_parameters("update_custom_tool", &json!({"exec":{"headers":secret}}))
            .unwrap();
        for value in [
            json!({"secret_ref":"invalid","hint":"configured"}),
            json!({"secret_ref":new_id("secret"),"hint":"configured","Authorization":"HEADER_SECRET_PROBE"}),
        ] {
            assert!(
                validate_known_credentials("tools", &json!({"exec":{"headers":value,"env":value}}))
                    .is_err()
            );
        }
        validate_known_credentials(
            "tools",
            &json!({"exec":{"headers":{"Accept":"application/json","User-Agent":"ARTEX"}}}),
        )
        .unwrap();
        // Captured traffic and opaque evidence retain their original bytes.
        validate_known_credentials(
            "traffic_exchanges",
            &json!({"headers":{"Authorization":"HEADER_SECRET_PROBE"}}),
        )
        .unwrap();
        validate_secret_parameters(
            "traffic.capture",
            &json!({"headers":{"Authorization":"HEADER_SECRET_PROBE"}}),
        )
        .unwrap();
    }
    #[test]
    fn annotated_ranges_select_valid_union_branch_and_body_limits_are_specific() {
        let schema = json!({"anyOf":[{"type":"string","pattern":"^(0|[1-9][0-9]*)$","x-max":"18446744073709551615"},{"type":"integer"}]});
        validate_contract(&schema, &json!({}), &json!(3)).unwrap();
        assert!(validate_contract(&schema, &json!({}), &json!("18446744073709551616")).is_err());
        let defs = entities();
        let schema = json!({"$ref":"#/$defs/Body"});
        assert!(validate_contract(&schema, defs, &body(&"a".repeat(65537))).is_err());
        // Unknown extension objects are not reinterpreted as Body by their key names.
        validate_contract(
            &json!({}),
            defs,
            &json!({"text":"a".repeat(65537),"blob":null}),
        )
        .unwrap();
    }
}
