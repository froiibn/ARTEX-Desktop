use artex_core::{
    Change, CommandBatch, Error, Expected, Header, Record, Result, Snapshot, data_model,
    default_data, new_id, now, registry,
};
use artex_store::{ImportMetadata, Store};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    path::Path,
};
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct LegacyExport {
    pub source_instance: String,
    pub format_version: u32,
    pub schema: BTreeMap<String, Vec<String>>,
    pub tables: BTreeMap<String, Vec<Value>>,
    pub blobs: BTreeMap<String, Vec<u8>>,
}
impl LegacyExport {
    pub fn merge(&mut self, other: Self) -> Result<()> {
        if self.source_instance != other.source_instance {
            return Err(Error::new(
                "INVALID_PARAMS",
                "Export source identities differ",
            ));
        }
        for (c, rows) in other.tables {
            if self.tables.contains_key(&c) {
                return Err(Error::new(
                    "VERSION_CONFLICT",
                    "Duplicate export collection",
                ));
            }
            self.tables.insert(c, rows);
        }
        self.schema.extend(other.schema);
        for (h, bytes) in other.blobs {
            if self.blobs.get(&h).is_some_and(|b| *b != bytes) {
                return Err(Error::new("CORRUPT_STORE", "Conflicting export blob"));
            }
            self.blobs.insert(h, bytes);
        }
        Ok(())
    }
}
#[derive(Debug, Clone)]
pub struct LegacyOptions {
    pub max_batch_bytes: usize,
    pub allow_missing_optional: bool,
    /// (collection, canonical JSON primary key) -> existing typed header ID.
    pub id_bindings: BTreeMap<(String, String), String>,
    /// Existing bound dependency records that must keep their current content.
    pub keep_existing: BTreeSet<(String, String)>,
}
impl Default for LegacyOptions {
    fn default() -> Self {
        Self {
            max_batch_bytes: 8 * 1024 * 1024,
            allow_missing_optional: true,
            id_bindings: BTreeMap::new(),
            keep_existing: BTreeSet::new(),
        }
    }
}
fn validate_options(source: &LegacyExport, options: &LegacyOptions) -> Result<()> {
    if options.max_batch_bytes == 0 || options.max_batch_bytes > 8 * 1024 * 1024 {
        return Err(Error::new(
            "INVALID_PARAMS",
            "Migration batch boundary must be 1..8MiB",
        ));
    }
    if !options.allow_missing_optional
        && ["llm_records", "llm_usage"]
            .iter()
            .any(|c| !source.tables.contains_key(*c))
    {
        return Err(Error::new(
            "INVALID_PARAMS",
            "Optional source tables missing under strict migration policy",
        ));
    }
    Ok(())
}
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ImportReport {
    pub source_instance: String,
    pub source_format_version: u32,
    pub counts: BTreeMap<String, usize>,
    pub missing_collections: Vec<String>,
    pub missing_columns: BTreeMap<String, Vec<String>>,
    pub extra_columns: BTreeMap<String, Vec<String>>,
    pub extensions: BTreeMap<String, Value>,
    pub pending_secret_refs: Vec<String>,
    pub warnings: Vec<String>,
    pub command_ids: Vec<String>,
    pub complete: bool,
}
pub struct SecretOutcome {
    pub reference: Value,
    pub available: bool,
}
/// Implementations must reuse a durable secret for the same stable key. Errors
/// must never contain the submitted plaintext.
pub trait SecretSink {
    fn import_secret(&mut self, stable_key: &str, plaintext: &str) -> Result<SecretOutcome>;
    /// Archive references carry no credential material. The native host can
    /// confirm availability in its vault; offline migration conservatively
    /// keeps them pending without persisting a plaintext fallback.
    fn reference_available(&mut self, _reference: &Value) -> Result<bool> {
        Ok(false)
    }
}
pub struct PendingSecrets;
fn verified_blob<'a>(source: &'a LegacyExport, hash: &str) -> Result<&'a [u8]> {
    let bytes = source
        .blobs
        .get(hash)
        .ok_or_else(|| Error::new("NOT_FOUND", "Legacy binary blob is missing"))?;
    if hex::encode(Sha256::digest(bytes)) != hash {
        return Err(Error::new("CORRUPT_STORE", "Legacy blob checksum mismatch"));
    }
    Ok(bytes)
}
/// Preparation has immutable snapshot reads and content-addressed blob publication
/// only. A background worker cannot commit or replace a graph generation.
pub trait ImportPreparation {
    fn snapshot(&self) -> Snapshot;
    fn blob_put(&mut self, bytes: &[u8], media_type: &str) -> Result<Value>;
}
impl ImportPreparation for Store {
    fn snapshot(&self) -> Snapshot {
        Store::snapshot(self)
    }
    fn blob_put(&mut self, b: &[u8], m: &str) -> Result<Value> {
        Store::blob_put(self, b, m)
    }
}
impl ImportPreparation for artex_store::StoreReadView {
    fn snapshot(&self) -> Snapshot {
        self.snapshot()
    }
    fn blob_put(&mut self, b: &[u8], m: &str) -> Result<Value> {
        self.blob_put(b, m)
    }
}
fn install_body_blob(
    store: &mut impl ImportPreparation,
    source: &LegacyExport,
    body: &Value,
) -> Result<()> {
    if let Some(hash) = body["blob"]["sha256"].as_str() {
        let bytes = verified_blob(source, hash)?;
        let reference = store.blob_put(
            bytes,
            body["blob"]["media_type"]
                .as_str()
                .unwrap_or("application/octet-stream"),
        )?;
        if reference["length"] != body["blob"]["length"] {
            return Err(Error::new(
                "CORRUPT_STORE",
                "Portable body blob length mismatch",
            ));
        }
    }
    Ok(())
}
impl SecretSink for PendingSecrets {
    fn import_secret(&mut self, key: &str, _: &str) -> Result<SecretOutcome> {
        let h = hex::encode(Sha256::digest(key.as_bytes()));
        Ok(SecretOutcome {
            reference: json!({"secret_ref":format!("secret_{}",&h[..32]),"hint":"Import credential pending host storage"}),
            available: false,
        })
    }
}
#[derive(Clone, Serialize, Deserialize)]
pub(crate) struct Mapping {
    collection: String,
    legacy_primary_key: Value,
    new_id: String,
}
pub(crate) fn canonical_key(v: &Value) -> Value {
    match v {
        Value::Number(n) => json!(n.to_string()),
        Value::Array(a) => json!(a.iter().map(canonical_key).collect::<Vec<_>>()),
        _ => v.clone(),
    }
}
fn key(v: &Value) -> String {
    canonical_key(v).to_string()
}
fn pk(table: &Value, row: &Value) -> Result<Value> {
    let columns = table["columns"].as_array().unwrap();
    if let Some(c) = columns.iter().find(|c| {
        c["declaration"]
            .as_str()
            .unwrap()
            .to_uppercase()
            .contains("PRIMARY KEY")
    }) {
        return row
            .get(c["name"].as_str().unwrap())
            .cloned()
            .map(|v| canonical_key(&v))
            .ok_or_else(|| Error::new("INVALID_PARAMS", "Legacy primary key missing"));
    }
    let declaration = table["constraints"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(Value::as_str)
        .find(|s| s.to_uppercase().starts_with("PRIMARY KEY"));
    if let Some(d) = declaration {
        let names = d
            .split_once('(')
            .unwrap()
            .1
            .trim_end_matches(')')
            .split(',')
            .map(str::trim);
        let mut values = Vec::new();
        for n in names {
            values.push(canonical_key(row.get(n).ok_or_else(|| {
                Error::new("INVALID_PARAMS", "Legacy composite key missing")
            })?));
        }
        return Ok(json!(values));
    }
    if let Some(id) = row.get("id") {
        return Ok(canonical_key(id));
    }
    Err(Error::new(
        "INVALID_PARAMS",
        "Legacy table has no primary key",
    ))
}
fn table<'a>(name: &str, model: &'a Value) -> Result<&'a Value> {
    model["tables"]
        .as_array()
        .unwrap()
        .iter()
        .find(|t| t["name"] == name)
        .ok_or_else(|| Error::new("INVALID_PARAMS", "Unknown legacy table"))
}
/// Canonical primary-key value used by durable import mappings and explicit bindings.
pub fn source_primary_key(collection: &str, row: &Value) -> Result<Value> {
    pk(table(collection, data_model())?, row)
}
fn map_id(m: &BTreeMap<(String, String), String>, collection: &str, v: &Value) -> Result<Value> {
    if v.is_null() {
        return Ok(Value::Null);
    }
    m.get(&(collection.into(), key(v)))
        .map(|id| json!(id))
        .ok_or_else(|| Error::new("VERSION_CONFLICT", "Legacy referenced ID has no mapping"))
}
fn prefix_target(schema: &Value) -> Option<String> {
    if let Some(p) = schema["pattern"].as_str() {
        for c in registry()["collections"].as_array().unwrap() {
            if p == format!("^{}_[0-9a-f]{{32}}$", c["id_prefix"].as_str().unwrap_or("")) {
                return c["collection"].as_str().map(str::to_owned);
            }
        }
    }
    for k in ["anyOf", "oneOf"] {
        if let Some(a) = schema[k].as_array() {
            for s in a {
                if let Some(c) = prefix_target(s) {
                    return Some(c);
                }
            }
        }
    }
    None
}
fn secret_fields(
    collection: &str,
    row: &mut Value,
    sink: &mut dyn SecretSink,
    base: &str,
    report: &mut ImportReport,
) -> Result<bool> {
    fn walk(
        v: &mut Value,
        path: &str,
        sink: &mut dyn SecretSink,
        report: &mut ImportReport,
        known: bool,
    ) -> Result<bool> {
        if v.get("secret_ref").and_then(Value::as_str).is_some() {
            return Ok(false);
        }
        let mut pending = false;
        match v {
            Value::Object(o) => {
                for (k, v) in o.iter_mut() {
                    let p = format!("{path}/{k}");
                    let u = k.to_uppercase();
                    let leaf = known
                        && [
                            "PASSWORD",
                            "TOKEN",
                            "API_KEY",
                            "APIKEY",
                            "SECRET",
                            "BOT_TOKEN",
                            "WEBHOOK",
                            "AUTHORIZATION",
                            "CLIENT_SECRET",
                            "ACCESS_KEY",
                            "PRIVATE_KEY",
                        ]
                        .iter()
                        .any(|s| u == *s || u.ends_with(&format!("_{s}")));
                    if leaf && v.as_str() == Some("") {
                        *v = Value::Null;
                    } else if leaf && v.as_str().is_some_and(|s| !s.is_empty()) {
                        let out = sink.import_secret(&p, v.as_str().unwrap()).map_err(|_| {
                            Error::new("EXTERNAL_FAILURE", "Host credential transfer failed")
                        })?;
                        artex_core::validate_contract(
                            &json!({"$ref":"#/$defs/SecretRef"}),
                            &artex_core::contracts::read("entities.v1.schema.json")["$defs"],
                            &out.reference,
                        )?;
                        if !out.available {
                            report
                                .pending_secret_refs
                                .push(out.reference["secret_ref"].as_str().unwrap().into());
                            pending = true;
                        }
                        *v = out.reference;
                    } else {
                        pending |= walk(v, &p, sink, report, known)?;
                    }
                }
            }
            Value::Array(a) => {
                for (i, v) in a.iter_mut().enumerate() {
                    pending |= walk(v, &format!("{path}/{i}"), sink, report, known)?;
                }
            }
            _ => {}
        }
        Ok(pending)
    }
    let known = [
        "agents",
        "llm_profiles",
        "assets",
        "mcp_servers",
        "tools",
        "notification_channels",
    ]
    .contains(&collection);
    fn existing_refs(v: &Value, out: &mut Vec<Value>) {
        if let Some(r) = v["secret_ref"].as_str() {
            let _ = r;
            out.push(v.clone());
            return;
        }
        match v {
            Value::Object(o) => {
                for v in o.values() {
                    existing_refs(v, out);
                }
            }
            Value::Array(a) => {
                for v in a {
                    existing_refs(v, out);
                }
            }
            _ => {}
        }
    }
    let mut references = Vec::new();
    existing_refs(row, &mut references);
    let mut had_existing = false;
    for reference in references {
        if !sink.reference_available(&reference)? {
            had_existing = true;
            report
                .pending_secret_refs
                .push(reference["secret_ref"].as_str().unwrap().into());
        }
    }
    fn transfer(
        value: &mut Value,
        key: &str,
        sink: &mut dyn SecretSink,
        report: &mut ImportReport,
    ) -> Result<bool> {
        let plaintext = value
            .as_str()
            .map(str::to_owned)
            .unwrap_or_else(|| value.to_string());
        let out = sink
            .import_secret(key, &plaintext)
            .map_err(|_| Error::new("EXTERNAL_FAILURE", "Host credential transfer failed"))?;
        artex_core::validate_contract(
            &json!({"$ref":"#/$defs/SecretRef"}),
            &artex_core::contracts::read("entities.v1.schema.json")["$defs"],
            &out.reference,
        )?;
        let pending = !out.available;
        if pending {
            report
                .pending_secret_refs
                .push(out.reference["secret_ref"].as_str().unwrap().into());
        }
        *value = out.reference;
        Ok(pending)
    }
    fn seal_headers(
        value: &mut Value,
        key: &str,
        sink: &mut dyn SecretSink,
        report: &mut ImportReport,
    ) -> Result<bool> {
        if value.is_null()
            || value.as_str() == Some("")
            || value.as_object().is_some_and(|o| o.is_empty())
            || value.get("secret_ref").is_some()
        {
            return Ok(false);
        }
        let mut existing = Vec::new();
        existing_refs(value, &mut existing);
        // Raw legacy maps match the native form's one-value vault representation.
        if existing.is_empty() {
            return transfer(value, key, sink, report);
        }
        // Portable maps may mix already sealed leaves and raw sensitive fields.
        // Keep references intact: this boundary cannot resolve them to plaintext.
        let mut pending = false;
        if let Some(headers) = value.as_object_mut() {
            for (name, value) in headers {
                let mut probe = json!({"exec":{"headers":{}}});
                probe["exec"]["headers"][name] = value.clone();
                if artex_core::validate_known_credentials("tools", &probe).is_err() {
                    if !value.is_string() {
                        return Err(Error::new(
                            "INVALID_PARAMS",
                            "Legacy credential header must be text or SecretRef",
                        ));
                    }
                    pending |= transfer(value, &format!("{key}/{name}"), sink, report)?;
                }
            }
        }
        Ok(pending)
    }
    // Seal configuration headers before generic traversal, including HTTP/SSE
    // MCP env maps which the source uses as HTTP headers, not process variables.
    let mut header_pending = false;
    if collection == "tools" {
        if let Some(value) = row.get_mut("exec").and_then(|exec| exec.get_mut("headers")) {
            header_pending |= seal_headers(value, &format!("{base}/exec/headers"), sink, report)?;
        }
    }
    if collection == "mcp_servers" && matches!(row["transport"].as_str(), Some("http" | "sse")) {
        if let Some(value) = row.get_mut("env") {
            header_pending |= seal_headers(value, &format!("{base}/env"), sink, report)?;
        }
    }
    // Notification providers define their credential paths explicitly. A
    // webhook's arbitrary headers are one vault value, never partial plaintext.
    if collection == "notification_channels" {
        let keys: &[&str] = match row["kind"].as_str().unwrap_or("") {
            "email" => &["password"],
            "telegram" => &["bot_token"],
            "dingtalk" | "feishu" => &["webhook", "secret"],
            "wecom" => &["webhook"],
            "webhook" => &["url", "headers"],
            _ => &[],
        };
        for key in keys {
            let value = &mut row["config"][*key];
            if value.is_null()
                || value.as_str() == Some("")
                || value
                    .as_object()
                    .is_some_and(|o| o.is_empty() || o.contains_key("secret_ref"))
            {
                continue;
            }
            let plaintext = value
                .as_str()
                .map(str::to_owned)
                .unwrap_or_else(|| value.to_string());
            let out = sink
                .import_secret(&format!("{base}/config/{key}"), &plaintext)
                .map_err(|_| Error::new("EXTERNAL_FAILURE", "Host credential transfer failed"))?;
            if !out.available {
                report.pending_secret_refs.push(
                    out.reference["secret_ref"]
                        .as_str()
                        .ok_or_else(|| {
                            Error::new("INVALID_PARAMS", "Host returned invalid secret reference")
                        })?
                        .into(),
                );
                row["enabled"] = json!(false);
            }
            row["config"][*key] = out.reference;
        }
    }
    let pending = walk(row, base, sink, report, known)? || had_existing || header_pending;
    if pending {
        if let Some(o) = row.as_object_mut() {
            if o.contains_key("enabled") {
                o.insert("enabled".into(), json!(false));
            }
            if collection == "llm_profiles" {
                o.insert("pool_exclude".into(), json!(true));
                o.insert("is_default".into(), json!(false));
            }
        }
    }
    Ok(pending)
}
fn rewrite_json(v: &mut Value, m: &BTreeMap<(String, String), String>) -> Result<()> {
    if let Some(o) = v.as_object_mut() {
        for (k, v) in o {
            let target = match k.as_str() {
                "node_id" | "intent_id" | "goal_id" | "evidence_id" | "finding_node_id"
                | "proof_node_id" => Some("exploration_nodes"),
                "task_id" | "source_task_id" => Some("tasks"),
                "finding_id" => Some("findings"),
                "asset_id" => Some("assets"),
                "traffic_id" | "source_traffic_id" => Some("exchanges"),
                "snapshot_id" => Some("traffic_evidence_snapshots"),
                _ => None,
            };
            if let Some(t) = target {
                if !v.is_null() {
                    *v = map_id(m, t, v)?;
                }
            } else if let Some(t) = match k.as_str() {
                "asset_ids" => Some("assets"),
                "source_parent_ids" | "member_ids" | "node_ids" => Some("exploration_nodes"),
                "source_task_ids" | "task_ids" => Some("tasks"),
                _ => None,
            } {
                if let Some(a) = v.as_array_mut() {
                    for item in a {
                        *item = map_id(m, t, item)?;
                    }
                }
            } else {
                rewrite_json(v, m)?;
            }
        }
    } else if let Some(a) = v.as_array_mut() {
        for v in a {
            rewrite_json(v, m)?;
        }
    }
    Ok(())
}
fn stamp(v: &Value) -> Result<String> {
    let Some(s) = v.as_str() else {
        return Ok(now());
    };
    if let Ok(d) = chrono::DateTime::parse_from_rfc3339(s) {
        return Ok(d
            .with_timezone(&chrono::Utc)
            .to_rfc3339_opts(chrono::SecondsFormat::Millis, true));
    }
    if let Ok(d) = chrono::NaiveDateTime::parse_from_str(s, "%Y-%m-%d %H:%M:%S%.f") {
        return Ok(d
            .and_utc()
            .to_rfc3339_opts(chrono::SecondsFormat::Millis, true));
    }
    Err(Error::new("INVALID_PARAMS", "Legacy timestamp is invalid"))
}
pub(crate) struct Prepared {
    pub rows: Vec<(String, String, Value, ImportMetadata)>,
    pub mappings: Vec<Mapping>,
    pub report: ImportReport,
}
pub(crate) fn prepare(
    store: &mut impl ImportPreparation,
    source: &LegacyExport,
    sink: &mut dyn SecretSink,
    options: &LegacyOptions,
) -> Result<Prepared> {
    if source.source_instance.trim().is_empty() {
        return Err(Error::new("INVALID_PARAMS", "Source identity is required"));
    }
    let model = data_model();
    let mut report = ImportReport {
        source_instance: source.source_instance.clone(),
        source_format_version: source.format_version,
        ..Default::default()
    };
    let mut map = BTreeMap::new();
    for r in store.snapshot().records("command_receipts") {
        if r.data["normalized_params"]["source_instance"] == source.source_instance {
            if let Some(a) = r.data["normalized_params"]["mappings"].as_array() {
                for e in a {
                    let e: Mapping = serde_json::from_value(e.clone())?;
                    map.insert((e.collection, key(&e.legacy_primary_key)), e.new_id);
                }
            }
        }
    }
    let mut mappings = Vec::new();
    for ((collection, legacy), id) in &options.id_bindings {
        if !artex_core::valid_id(id, artex_core::collection_prefix(collection)?)
            || store.snapshot().get(collection, id).is_none()
        {
            return Err(Error::new(
                "INVALID_PARAMS",
                "Import identity binding target missing",
            ));
        }
        map.insert((collection.clone(), legacy.clone()), id.clone());
    }
    for (c, rows) in &source.tables {
        let t = table(c, model)?;
        let known = t["columns"]
            .as_array()
            .unwrap()
            .iter()
            .map(|v| v["name"].as_str().unwrap().to_owned())
            .collect::<BTreeSet<_>>();
        let actual = source
            .schema
            .get(c)
            .cloned()
            .unwrap_or_else(|| {
                rows.first()
                    .and_then(Value::as_object)
                    .map(|o| o.keys().cloned().collect())
                    .unwrap_or_default()
            })
            .into_iter()
            .collect::<BTreeSet<_>>();
        report
            .missing_columns
            .insert(c.clone(), known.difference(&actual).cloned().collect());
        report
            .extra_columns
            .insert(c.clone(), actual.difference(&known).cloned().collect());
        report.counts.insert(c.clone(), rows.len());
        for row in rows {
            let legacy = pk(t, row)?;
            let id = map
                .entry((c.clone(), key(&legacy)))
                .or_insert_with(|| new_id(t["id_prefix"].as_str().unwrap()))
                .clone();
            mappings.push(Mapping {
                collection: c.clone(),
                legacy_primary_key: legacy,
                new_id: id,
            });
        }
    }
    for t in model["tables"].as_array().unwrap() {
        let c = t["name"].as_str().unwrap();
        if !source.tables.contains_key(c) {
            report.missing_collections.push(c.into());
        }
    }
    let mut rows_out = Vec::new();
    for (c, rows) in &source.tables {
        let t = table(c, model)?;
        for input in rows {
            let legacy = pk(t, input)?;
            let id = map[&(c.clone(), key(&legacy))].clone();
            if options.keep_existing.contains(&(c.clone(), id.clone())) {
                continue;
            }
            let mut input = input.clone();
            secret_fields(
                c,
                &mut input,
                sink,
                &format!("{}/{}/{}", source.source_instance, c, key(&legacy)),
                &mut report,
            )?;
            let mut data = default_data(c)?;
            for column in t["columns"].as_array().unwrap() {
                let field = column["name"].as_str().unwrap();
                let Some(mut value) = input.get(field).cloned() else {
                    continue;
                };
                if value.is_null() {
                    data[field] = value;
                    continue;
                }
                let typ = column["desktop_type"].as_str().unwrap();
                if field == "id" && prefix_target(&column["desktop_schema"]).as_deref() == Some(c) {
                    value = json!(id);
                } else if let Some(target) = prefix_target(&column["desktop_schema"]) {
                    value = match map_id(&map, &target, &value) {
                        Ok(v) => v,
                        Err(_)
                            if column.get("reference").is_none() && column["nullable"] == true =>
                        {
                            report
                                .extensions
                                .entry(format!("{c}/{id}"))
                                .or_insert(json!({}))["unresolved_provenance"][field] =
                                value.clone();
                            report.warnings.push(format!(
                                "Unresolved historical provenance preserved: {c}.{field}"
                            ));
                            Value::Null
                        }
                        Err(e) => return Err(e),
                    };
                } else if c == "assets" && field == "task_ids" {
                    // Source denormalized memberships are migrated separately and may contain stale tasks.
                    value = json!([]);
                } else if typ == "typed_id[]" {
                    let target = if field == "asset_ids" {
                        "assets"
                    } else if field == "task_ids" || field == "source_task_ids" {
                        "tasks"
                    } else {
                        "exploration_nodes"
                    };
                    value = json!(
                        value
                            .as_array()
                            .ok_or_else(|| Error::new(
                                "INVALID_PARAMS",
                                "Legacy ID array is invalid"
                            ))?
                            .iter()
                            .map(|v| map_id(&map, target, v))
                            .collect::<Result<Vec<_>>>()?
                    );
                } else if typ == "utc_timestamp" {
                    value = json!(stamp(&value)?);
                } else if typ == "decimal_string" {
                    if value.is_number() {
                        value = json!(value.to_string());
                    }
                } else if typ == "blob_ref" {
                    if let Some(bytes) = value["legacy_bytes"].as_str() {
                        use base64::Engine;
                        let bytes = base64::engine::general_purpose::STANDARD
                            .decode(bytes)
                            .map_err(|_| {
                                Error::new("INVALID_PARAMS", "Legacy binary encoding invalid")
                            })?;
                        value = store.blob_put(&bytes, "application/octet-stream")?;
                    } else if let Some(hash) = value.as_str().or_else(|| value["sha256"].as_str()) {
                        let bytes = verified_blob(source, hash)?;
                        value = store.blob_put(
                            bytes,
                            value["media_type"]
                                .as_str()
                                .unwrap_or("application/octet-stream"),
                        )?;
                    }
                } else if typ == "string"
                    && column["desktop_schema"]
                        .to_string()
                        .contains("#/$defs/Body")
                {
                    if value.is_object() {
                        install_body_blob(store, source, &value)?;
                    } else {
                        let text = value.as_str().ok_or_else(|| {
                            Error::new("INVALID_PARAMS", "Legacy body text invalid")
                        })?;
                        value = if text.len() > 65536 {
                            json!({"text":null,"blob":store.blob_put(text.as_bytes(),"text/plain; charset=utf-8")?})
                        } else {
                            artex_core::body(text)
                        };
                    }
                } else if ["payload", "snapshot", "traffic_refs"].contains(&field) {
                    rewrite_json(&mut value, &map)?;
                }
                data[field] = value;
            }
            if c == "assets" {
                data["task_ids"] = json!([]);
            }
            let unknown = input
                .as_object()
                .unwrap()
                .iter()
                .filter(|(k, _)| {
                    t["columns"]
                        .as_array()
                        .unwrap()
                        .iter()
                        .all(|col| col["name"] != k.as_str())
                })
                .map(|(k, v)| (k.clone(), v.clone()))
                .collect::<serde_json::Map<_, _>>();
            if !unknown.is_empty() {
                let entry = report
                    .extensions
                    .entry(format!("{c}/{id}"))
                    .or_insert(json!({}));
                entry.as_object_mut().unwrap().extend(unknown);
            }
            let created = stamp(&input["created_at"])?;
            let updated = if input["updated_at"].is_null() {
                created.clone()
            } else {
                stamp(&input["updated_at"])?
            };
            let legacy_key = legacy
                .as_str()
                .map(str::to_owned)
                .unwrap_or_else(|| legacy.to_string());
            let header = Header {
                id: id.clone(),
                collection: c.clone(),
                version: "1".into(),
                commit_seq: "1".into(),
                created_at: created.clone(),
                updated_at: updated.clone(),
                legacy_key: json!(legacy_key),
            };
            artex_core::validate_record(&Record {
                header,
                data: data.clone(),
            })?;
            rows_out.push((
                c.clone(),
                id,
                data,
                ImportMetadata {
                    legacy_key: json!(legacy_key),
                    created_at: created,
                    updated_at: updated,
                },
            ));
        }
    }
    // Arrays in old assets are a redundant source of task links. Preserve live
    // references missed by the original link table, without reviving dangling IDs.
    if let Some(assets) = source.tables.get("assets") {
        for a in assets {
            let aid = map_id(&map, "assets", &a["id"])?;
            for task in a["task_ids"].as_array().into_iter().flatten() {
                if let Ok(tid) = map_id(&map, "tasks", task) {
                    if !rows_out.iter().any(|(c, _, d, _)| {
                        c == "task_asset_links" && d["task_id"] == tid && d["asset_id"] == aid
                    }) {
                        let mut d = default_data("task_asset_links")?;
                        d["task_id"] = tid;
                        d["asset_id"] = aid.clone();
                        d["source"] = json!("legacy");
                        let id = new_id(artex_core::collection_prefix("task_asset_links")?);
                        rows_out.push((
                            "task_asset_links".into(),
                            id,
                            d,
                            ImportMetadata {
                                legacy_key: Value::Null,
                                created_at: now(),
                                updated_at: now(),
                            },
                        ));
                    }
                } else {
                    report
                        .warnings
                        .push("Dangling legacy asset task link was excluded".into());
                }
            }
        }
    }
    Ok(Prepared {
        rows: rows_out,
        mappings,
        report,
    })
}
fn batch(
    source: &str,
    rows: &[(String, String, Value, ImportMetadata)],
    mappings: &[Mapping],
    snapshot: &Snapshot,
    index: usize,
) -> Result<(CommandBatch, BTreeMap<(String, String), ImportMetadata>)> {
    let digest = hex::encode(Sha256::digest(serde_json::to_vec(
        &rows
            .iter()
            .map(|(c, id, d, _)| (c, id, d))
            .collect::<Vec<_>>(),
    )?));
    let mut b = CommandBatch {
        command_id: new_id("command"),
        method: "migration.import".into(),
        normalized_params: json!({"source_instance":source,"mapping_version":1,"batch_index":index,"content_digest":digest,"mappings":mappings}),
        expected: Vec::new(),
        changes: Vec::new(),
        events: Vec::new(),
        deliveries: Vec::new(),
        result: json!({"imported":rows.len(),"complete":false}),
    };
    let mut meta = BTreeMap::new();
    for (c, id, d, m) in rows {
        if let Some(old) = snapshot.get(c, id) {
            b.expected.push(Expected {
                collection: c.clone(),
                id: id.clone(),
                version: old.header.version.clone(),
            });
        }
        b.changes.push(Change::Put {
            collection: c.clone(),
            id: id.clone(),
            data: d.clone(),
        });
        meta.insert((c.clone(), id.clone()), m.clone());
    }
    Ok((b, meta))
}
pub fn import_into(
    store: &mut Store,
    source: &LegacyExport,
    sink: &mut dyn SecretSink,
    options: LegacyOptions,
) -> Result<ImportReport> {
    validate_options(source, &options)?;
    let mut p = prepare(store, source, sink, &options)?;
    let (mut b, meta) = batch(
        &source.source_instance,
        &p.rows,
        &p.mappings,
        &store.snapshot(),
        0,
    )?;
    let report_blob = store.blob_put(
        &serde_json::to_vec(&p.report)?,
        "application/vnd.artex.import-report+json",
    )?;
    b.result["report_blob"] = report_blob;
    let receipt = store.commit_import(b, meta)?;
    p.report.command_ids.push(receipt.command_id);
    p.report.complete = true;
    Ok(p.report)
}
/// Resume-safe bounded import into an existing writer. Each FK-connected batch is
/// durable; callers retain their workflow marker until complete. On failure the
/// partial graph and its ID mappings remain available for an explicit retry.
pub fn import_into_bounded(
    store: &mut Store,
    source: &LegacyExport,
    sink: &mut dyn SecretSink,
    options: LegacyOptions,
) -> Result<ImportReport> {
    let mut session = ImportSession::prepare(store, source, sink, options)?;
    while !session.step(store)? {}
    Ok(session.report().clone())
}
/// An owned import plan executes at most one bounded, FK-valid transaction per
/// step. The caller keeps its workflow marker until the final report receipt.
pub struct ImportSession {
    source_instance: String,
    prepared: Prepared,
    pending: Vec<(String, String, Value, ImportMetadata)>,
    options: LegacyOptions,
    index: usize,
    complete: bool,
    observed_versions: BTreeMap<(String, String), Option<String>>,
}
impl ImportSession {
    pub fn prepare(
        store: &mut impl ImportPreparation,
        source: &LegacyExport,
        sink: &mut dyn SecretSink,
        options: LegacyOptions,
    ) -> Result<Self> {
        validate_options(source, &options)?;
        let mut prepared = prepare(store, source, sink, &options)?;
        let pending = std::mem::take(&mut prepared.rows);
        let snapshot = store.snapshot();
        let observed_versions = pending
            .iter()
            .map(|(c, id, _, _)| {
                (
                    (c.clone(), id.clone()),
                    snapshot.get(c, id).map(|r| r.header.version.clone()),
                )
            })
            .collect();
        Ok(Self {
            source_instance: source.source_instance.clone(),
            prepared,
            pending,
            options,
            index: 0,
            complete: false,
            observed_versions,
        })
    }
    pub fn report(&self) -> &ImportReport {
        &self.prepared.report
    }
    pub fn remaining(&self) -> usize {
        self.pending.len()
    }
    pub fn step(&mut self, store: &mut Store) -> Result<bool> {
        if self.complete {
            return Ok(true);
        }
        if self.pending.is_empty() {
            artex_core::validate_snapshot(&store.snapshot())?;
            self.prepared.report.complete = true;
            let report_blob = store.blob_put(
                &serde_json::to_vec(&self.prepared.report)?,
                "application/vnd.artex.import-report+json",
            )?;
            let (mut b, meta) = batch(
                &self.source_instance,
                &[],
                &self.prepared.mappings,
                &store.snapshot(),
                self.index,
            )?;
            b.result = json!({"complete":true,"report_blob":report_blob});
            self.prepared
                .report
                .command_ids
                .push(store.commit_import(b, meta)?.command_id);
            self.complete = true;
            return Ok(true);
        }
        let snapshot = store.snapshot();
        for (c, id, _, _) in &self.pending {
            let actual = snapshot.get(c, id).map(|r| r.header.version.clone());
            if self.observed_versions.get(&(c.clone(), id.clone())) != Some(&actual) {
                return Err(Error::new(
                    "VERSION_CONFLICT",
                    "A migration target changed after preparation",
                ));
            }
        }
        commit_one_prepared_batch(
            store,
            &self.source_instance,
            &mut self.prepared,
            &self.options,
            &mut self.pending,
            self.index,
        )?;
        self.index += 1;
        Ok(false)
    }
}
fn commit_prepared_batches(
    store: &mut Store,
    source: &LegacyExport,
    p: &mut Prepared,
    options: &LegacyOptions,
) -> Result<()> {
    let mut pending = std::mem::take(&mut p.rows);
    let mut index = 0;
    while !pending.is_empty() {
        commit_one_prepared_batch(
            store,
            &source.source_instance,
            p,
            options,
            &mut pending,
            index,
        )?;
        index += 1;
    }
    artex_core::validate_snapshot(&store.snapshot())?;
    Ok(())
}
fn commit_one_prepared_batch(
    store: &mut Store,
    source_instance: &str,
    p: &mut Prepared,
    options: &LegacyOptions,
    pending: &mut Vec<(String, String, Value, ImportMetadata)>,
    index: usize,
) -> Result<()> {
    let snapshot = store.snapshot();
    let mut selected = Vec::new();
    let mut selected_ids = BTreeSet::new();
    let mut bytes = 0;
    loop {
        let mut progressed = false;
        let mut i = 0;
        while i < pending.len() {
            let (c, _id, d, _) = &pending[i];
            let dependencies = artex_core::foreign_keys()
                .iter()
                .filter(|fk| fk.collection == *c && fk.columns.iter().all(|col| !d[col].is_null()))
                .all(|fk| {
                    snapshot.records(&fk.target).iter().any(|r| {
                        fk.columns
                            .iter()
                            .zip(&fk.target_columns)
                            .all(|(a, b)| d[a] == r.data[b])
                    }) || selected.iter().any(
                        |(col, _, target, _): &(String, String, Value, ImportMetadata)| {
                            col == &fk.target
                                && fk
                                    .columns
                                    .iter()
                                    .zip(&fk.target_columns)
                                    .all(|(a, b)| d[a] == target[b])
                        },
                    ) || fk.target == *c
                        && fk
                            .columns
                            .iter()
                            .zip(&fk.target_columns)
                            .all(|(a, b)| d[a] == d[b])
                });
            let n = serde_json::to_vec(d)?.len() + 512;
            if dependencies && bytes + n <= options.max_batch_bytes {
                let row = pending.remove(i);
                bytes += n;
                selected_ids.insert((row.0.clone(), row.1.clone()));
                selected.push(row);
                progressed = true;
            } else {
                i += 1;
            }
        }
        if !progressed || bytes >= options.max_batch_bytes {
            break;
        }
    }
    // Cyclic FK components must stay atomic. If no acyclic row is ready, try the
    // remaining graph as one bounded transaction rather than inserting dangling FK.
    if selected.is_empty() {
        let n = serde_json::to_vec(
            &pending
                .iter()
                .map(|(c, id, d, _)| (c, id, d))
                .collect::<Vec<_>>(),
        )?
        .len();
        if n > options.max_batch_bytes {
            return Err(Error::new(
                "SIZE_LIMIT",
                "Cyclic migration component exceeds batch limit; staging preserved",
            ));
        }
        selected = std::mem::take(pending);
        selected_ids = selected
            .iter()
            .map(|(c, id, _, _)| (c.clone(), id.clone()))
            .collect();
    }
    let maps = p
        .mappings
        .iter()
        .filter(|m| selected_ids.contains(&(m.collection.clone(), m.new_id.clone())))
        .cloned()
        .collect::<Vec<_>>();
    let (b, meta) = batch(source_instance, &selected, &maps, &snapshot, index)?;
    let receipt = store.commit_import(b, meta)?;
    p.report.command_ids.push(receipt.command_id);
    Ok(())
}
/// Publish a whole migration only after the private staging writer has imported
/// every FK-connected batch and verified two complete checkpoint generations.
pub fn migrate_new(
    destination: &Path,
    source: &LegacyExport,
    sink: &mut dyn SecretSink,
    options: LegacyOptions,
) -> Result<ImportReport> {
    validate_options(source, &options)?;
    if destination.exists() {
        return Err(Error::new(
            "VERSION_CONFLICT",
            "Migration destination exists",
        ));
    }
    let parent = destination
        .parent()
        .ok_or_else(|| Error::new("INVALID_PARAMS", "Missing destination parent"))?;
    fs::create_dir_all(parent)?;
    let label = hex::encode(Sha256::digest(
        format!("{}:{}", destination.display(), source.source_instance).as_bytes(),
    ));
    let stage = parent.join(format!(".artex-import-{}", &label[..24]));
    let mut store = if stage.exists() {
        Store::open(&stage)?
    } else {
        Store::create(&stage, "Imported ARTEX workspace")?
    };
    let mut p = prepare(&mut store, source, sink, &options)?;
    commit_prepared_batches(&mut store, source, &mut p, &options)?;
    artex_core::validate_snapshot(&store.snapshot())?;
    store.checkpoint()?;
    store.checkpoint()?;
    p.report.complete = true;
    let report = serde_json::to_vec(&p.report)?;
    let report_path = stage.join("backups/migration-report.json");
    fs::write(&report_path, report)?;
    fs::OpenOptions::new()
        .write(true)
        .open(&report_path)?
        .sync_all()?;
    artex_platform::sync_directory(report_path.parent().unwrap())?;
    drop(store);
    artex_platform::install_new(&stage, destination)?;
    Ok(p.report)
}
