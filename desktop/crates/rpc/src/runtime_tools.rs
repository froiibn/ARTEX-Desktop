//! Resolve executable configurations inside the trusted engine boundary.
use super::*;
use artex_domain::{ids, record, text};
impl Router {
    pub(super) fn run_attachment_paths(&self, rid: &str) -> Result<Vec<String>> {
        let s = self.snapshot()?;
        let run = record(&s, "agent_runs", rid)?;
        let mut paths = std::collections::BTreeSet::new();
        for row in s
            .records("activity")
            .into_iter()
            .chain(s.records("conversation_activities"))
        {
            if row.data["kind"] != "user" {
                continue;
            }
            let Ok(admission) = self.read_json_blob(&row.data["detail"]) else {
                continue;
            };
            let own = admission["run_id"] == rid
                || (!run.data["conversation_id"].is_null()
                    && row.data["conversation_id"] == run.data["conversation_id"]);
            if !own {
                continue;
            }
            for attachment in admission["attachments"].as_array().into_iter().flatten() {
                let path = text(attachment, "path");
                let Some(filename) = path.strip_prefix("attachments/") else {
                    continue;
                };
                let Some((id, name)) = filename.split_once('-') else {
                    continue;
                };
                if artex_core::valid_id(id, "attachment")
                    && !name.is_empty()
                    && !name.contains(['/', '\\'])
                    && !name.contains("..")
                {
                    paths.insert(path.to_owned());
                }
            }
        }
        if paths.len() > 100 {
            return Err(Error::new(
                "SIZE_LIMIT",
                "Run attachment grant count exceeds 100",
            ));
        }
        Ok(paths.into_iter().collect())
    }
    pub(super) fn guard_audit(
        &mut self,
        rid: &str,
        call: &artex_runtime::ToolCall,
        action: &str,
        reason: &str,
    ) -> Result<()> {
        let s = self.snapshot()?;
        let marker = format!("guard:{rid}:{}:{action}", call.id);
        if s.records("activity")
            .into_iter()
            .chain(s.records("conversation_activities"))
            .any(|a| a.data["summary"] == marker)
        {
            return Ok(());
        }
        let context = self.stored_run_context_for_guard(&s, rid)?;
        let command = match call.name.as_str() {
            "Bash" | "shell_open" => text(&call.input, "command"),
            "shell_send" => text(&call.input, "text"),
            _ => "",
        };
        let detail = json!({"run_id":rid,"guard_audit":{"ts":chrono::Utc::now().timestamp(),"tool":call.name,"action":action,"reason":reason,"command":command}});
        let batch = artex_domain::conversation::append_activity(
            &s,
            &context,
            rid,
            "guard",
            &marker,
            &artex_core::body(&detail.to_string()),
            None,
        )?;
        self.internal_commit(batch)?;
        Ok(())
    }
    fn stored_run_context_for_guard(&self, s: &Snapshot, rid: &str) -> Result<Context> {
        let run = record(s, "agent_runs", rid)?;
        Ok(Context {
            actor: Actor {
                kind: "agent".into(),
                agent_id: run.data["agent_id"].as_str().map(str::to_owned),
                run_id: Some(rid.into()),
                engine_epoch: self.engine_epoch.clone(),
            },
            task_id: run.data["task_id"].as_str().map(str::to_owned),
            intent_id: run.data["intent_id"].as_str().map(str::to_owned),
            conversation_id: run.data["conversation_id"].as_str().map(str::to_owned),
        })
    }
    pub(super) fn guard_attribution(
        &mut self,
        rid: &str,
        name: &str,
        output: &str,
        is_error: bool,
    ) -> Result<()> {
        if name != "Bash" {
            return Ok(());
        }
        static BLOCKED: std::sync::OnceLock<regex::Regex> = std::sync::OnceLock::new();
        let blocked = BLOCKED.get_or_init(|| {
            regex::Regex::new(r"(?i)\b(403|forbidden|waf|blocked|rate.?limit|429|captcha|denied)\b")
                .unwrap()
        });
        let class = if blocked.is_match(output) {
            "blocked"
        } else if is_error {
            "error"
        } else {
            "ok"
        };
        let s = self.snapshot()?;
        let context = self.stored_run_context_for_guard(&s, rid)?;
        let batch = artex_domain::conversation::append_activity(
            &s,
            &context,
            rid,
            "guard",
            "guard_attribution",
            &artex_core::body(&json!({"run_id":rid,"guard_attribution":class}).to_string()),
            None,
        )?;
        self.internal_commit(batch)?;
        Ok(())
    }
    pub(super) fn read_blob_bytes(&self, reference: &Value, max: u64) -> Result<Vec<u8>> {
        let length = parse_u64(text(reference, "length"))?;
        if length > max {
            return Err(Error::new(
                "SIZE_LIMIT",
                "Body exceeds this operation's memory limit",
            ));
        }
        if let Some(bytes) = self.ephemeral_blob_bytes(reference, max)? {
            return Ok(bytes.as_ref().clone());
        }
        let mut bytes = Vec::with_capacity(
            usize::try_from(length)
                .map_err(|_| Error::new("SIZE_LIMIT", "Body exceeds platform address space"))?,
        );
        self.store
            .as_ref()
            .ok_or_else(|| Error::new("WORKSPACE_NOT_OPEN", "Workspace closed"))?
            .copy_blob_to(reference, &mut bytes)?;
        Ok(bytes)
    }
    /// Cached query bodies are scoped to the current open generation: close
    /// clears both maps, and a matching live lease is required before reading.
    pub(super) fn ephemeral_blob_bytes(
        &self,
        reference: &Value,
        max: u64,
    ) -> Result<Option<std::sync::Arc<Vec<u8>>>> {
        let hash = text(reference, "sha256");
        let Some((bytes, created)) = self.ephemeral_blobs.get(hash) else {
            return Ok(None);
        };
        if self.store.is_none() || created.elapsed().as_secs() >= 600 {
            return Err(Error::new(
                "NOT_FOUND",
                "Generated export reference has expired",
            ));
        }
        let Some((lease, at)) = self
            .blob_leases
            .get(hash)
            .filter(|(_, at)| at.elapsed().as_secs() < 600)
        else {
            return Err(Error::new(
                "NOT_FOUND",
                "Generated export reference has no current workspace lease",
            ));
        };
        let _ = at;
        let length = parse_u64(text(reference, "length"))?;
        if length > max {
            return Err(Error::new(
                "SIZE_LIMIT",
                "Generated Body exceeds this operation's memory limit",
            ));
        }
        if length != bytes.len() as u64
            || lease["length"] != reference["length"]
            || hex::encode(Sha256::digest(bytes.as_slice())) != hash
        {
            return Err(Error::new(
                "CORRUPT_STORE",
                "Generated export reference integrity mismatch",
            ));
        }
        Ok(Some(bytes.clone()))
    }
    pub(super) fn externalize_query(&mut self, value: Value) -> Result<Value> {
        if self.read_only {
            self.ephemeral_blobs
                .retain(|_, (_, at)| at.elapsed().as_secs() < 600);
            self.blob_leases
                .retain(|_, (_, at)| at.elapsed().as_secs() < 600);
            fn stage(
                v: &mut Value,
                bytes: &mut HashMap<String, (std::sync::Arc<Vec<u8>>, Instant)>,
                leases: &mut HashMap<String, (Value, Instant)>,
            ) -> Result<()> {
                if v.get("text").is_some() && v.get("blob").is_some() {
                    if let Some(text) = v["text"].as_str().filter(|s| s.len() > MAX_CHUNK) {
                        let data = text.as_bytes().to_vec();
                        let hash = hex::encode(Sha256::digest(&data));
                        if !bytes.contains_key(&hash)
                            && (bytes.len() >= 256
                                || bytes.values().map(|(b, _)| b.len()).sum::<usize>() + data.len()
                                    > 64 * 1024 * 1024)
                        {
                            return Err(Error::new(
                                "BUSY",
                                "Read-only export leases exceed their 64 MiB budget",
                            ));
                        }
                        let reference = json!({"sha256":hash,"length":data.len().to_string(),"media_type":"text/plain; charset=utf-8"});
                        bytes.insert(hash.clone(), (std::sync::Arc::new(data), Instant::now()));
                        leases.insert(hash, (reference.clone(), Instant::now()));
                        *v = json!({"text":null,"blob":reference});
                        return Ok(());
                    }
                }
                match v {
                    Value::Object(o) => {
                        for v in o.values_mut() {
                            stage(v, bytes, leases)?;
                        }
                    }
                    Value::Array(a) => {
                        for v in a {
                            stage(v, bytes, leases)?;
                        }
                    }
                    _ => {}
                }
                Ok(())
            }
            let mut result = value;
            stage(
                &mut result,
                &mut self.ephemeral_blobs,
                &mut self.blob_leases,
            )?;
            return Ok(result);
        }
        let mut batch = artex_domain::empty_batch("query.externalize", &json!({}));
        batch.result = value;
        self.externalize_batch(&mut batch)?;
        fn collect(v: &Value, leases: &mut HashMap<String, (Value, Instant)>) {
            if let Some(hash) = v["sha256"].as_str().filter(|h| h.len() == 64) {
                if v["length"].is_string() {
                    leases.insert(hash.into(), (v.clone(), Instant::now()));
                }
            }
            match v {
                Value::Array(a) => {
                    for v in a {
                        collect(v, leases);
                    }
                }
                Value::Object(o) => {
                    for v in o.values() {
                        collect(v, leases);
                    }
                }
                _ => {}
            }
        }
        self.blob_leases
            .retain(|_, (_, at)| at.elapsed().as_secs() < 600);
        if self.blob_leases.len() > 256 {
            return Err(Error::new(
                "BUSY",
                "Too many active export bodies; wait for download leases",
            ));
        }
        collect(&batch.result, &mut self.blob_leases);
        Ok(batch.result)
    }
    pub(super) fn traffic_blob(&self, input: &Value) -> Result<Value> {
        let hash = text(input, "hash");
        if hash.len() != 64 || !hash.bytes().all(|b| b.is_ascii_hexdigit()) {
            return Err(Error::new(
                "INVALID_PARAMS",
                "Expected a 64-digit SHA-256 hash",
            ));
        }
        let s = self.snapshot()?;
        fn find(v: &Value, hash: &str) -> Option<Value> {
            if v["sha256"] == hash && v["length"].is_string() && v["media_type"].is_string() {
                return Some(v.clone());
            }
            for side in ["req", "resp"] {
                if v[format!("{side}_hash")] == hash && v[format!("{side}_len")].is_string() {
                    return Some(
                        json!({"sha256":hash,"length":v[format!("{side}_len")],"media_type":"application/octet-stream"}),
                    );
                }
            }
            match v {
                Value::Object(m) => m.values().find_map(|v| find(v, hash)),
                Value::Array(a) => a.iter().find_map(|v| find(v, hash)),
                _ => None,
            }
        }
        let reference = ["exchange_bodies", "traffic_evidence_snapshots"]
            .into_iter()
            .flat_map(|c| s.records(c))
            .find_map(|r| find(&r.data, hash))
            .ok_or_else(|| {
                Error::new("NOT_FOUND", "No recorded traffic body references this hash")
            })?;
        let requested = input.get("offset").cloned().unwrap_or(json!(0));
        let offset = match &requested {
            Value::String(value) => parse_u64(value)?,
            Value::Number(value) => value
                .as_i64()
                .map(|n| n.max(0) as u64)
                .or_else(|| value.as_u64())
                .ok_or_else(|| Error::new("INVALID_PARAMS", "Invalid traffic body offset"))?,
            Value::Null => 0,
            _ => return Err(Error::new("INVALID_PARAMS", "Invalid traffic body offset")),
        };
        let length = input["length"]
            .as_i64()
            .filter(|n| *n > 0 && *n <= 8192)
            .unwrap_or(8192) as usize;
        let total = parse_u64(text(&reference, "length"))?;
        if offset >= total {
            return Ok(
                json!({"offset":requested,"length":0,"total_length":total.to_string(),"content":format!("offset {offset} exceeds total length {total} bytes")}),
            );
        }
        let bytes = self
            .store
            .as_ref()
            .unwrap()
            .blob_read(&reference, offset, length)?;
        let binary = bytes
            .iter()
            .take(512)
            .any(|b| *b == 0 || (*b < 0x09) || (*b > 0x0d && *b < 0x20));
        let content = if binary {
            format!(
                "Binary content; first 512 bytes (hex): {}",
                hex::encode(&bytes[..bytes.len().min(512)])
            )
        } else {
            let mut end = bytes.len();
            while end > 0 && std::str::from_utf8(&bytes[..end]).is_err() {
                end -= 1;
            }
            if end == 0 && !bytes.is_empty() {
                String::from_utf8_lossy(&bytes).into_owned()
            } else {
                std::str::from_utf8(&bytes[..end]).unwrap().into()
            }
        };
        Ok(
            json!({"offset":requested,"length":bytes.len(),"total_length":total.to_string(),"content":content,"binary":binary}),
        )
    }
    pub(super) fn traffic_blob_text(&self, input: &Value) -> Result<String> {
        let result = self.traffic_blob(input)?;
        let offset = result["offset"]
            .as_str()
            .map(str::to_owned)
            .unwrap_or_else(|| result["offset"].to_string());
        let length = result["length"].as_u64().unwrap_or(0);
        let total = text(&result, "total_length");
        if length == 0 {
            return Ok(format!(
                "offset {offset} exceeds total length {total} bytes"
            ));
        }
        Ok(format!(
            "[offset={offset} bytes={length} total_length={total}]\n{}",
            text(&result, "content")
        ))
    }
    pub(super) fn search_network_config(&self, backend: Option<&str>) -> Result<Value> {
        let s = self.snapshot()?;
        let setting = |key: &str| {
            let raw = artex_domain::configuration_ui::setting(&s, key);
            serde_json::from_str::<Value>(&raw).unwrap_or(json!(raw))
        };
        let selected = setting("web_search_backend");
        let backend = backend
            .filter(|s| !s.is_empty())
            .or_else(|| selected.as_str().filter(|s| !s.is_empty()))
            .unwrap_or("ddgs");
        let mut result = json!({"backend":backend,"proxy":setting("web_search_proxy"),"api_key":setting(if backend=="tavily"{"tavily_search_api_key"}else{"brave_search_api_key"})});
        if backend == "deepseek" {
            let profile = s
                .records("llm_profiles")
                .into_iter()
                .find(|p| p.data["is_default"] == true)
                .ok_or_else(|| {
                    Error::new(
                        "FEATURE_DISABLED",
                        "DeepSeek search requires an active Anthropic-format model profile",
                    )
                })?;
            result["base_url"] = profile.data["base_url"].clone();
            result["model"] = profile.data["model"].clone();
            result["api_key"] = profile.data["api_key"].clone();
        }
        Ok(result)
    }
    pub(super) fn trusted_tool_input(&self, rid: &str, name: &str, input: &Value) -> Result<Value> {
        let s = self.snapshot()?;
        let mut input = artex_tools::normalize_scalar_defaults(name, input, &s);
        let setting = |key: &str| -> Value {
            s.records("settings")
                .into_iter()
                .find(|r| r.data["key"] == key)
                .map(|r| {
                    serde_json::from_str(text(&r.data, "value"))
                        .unwrap_or_else(|_| r.data["value"].clone())
                })
                .unwrap_or(Value::Null)
        };
        if artex_tools::definition(name).is_err() {
            let c = artex_agent::context_for_run(&s, rid, &self.engine_epoch)?;
            artex_tools::check_access(name, &input, &s, &c)?;
            let config = s
                .records("tools")
                .into_iter()
                .find(|t| t.data["key"] == name)
                .ok_or_else(|| Error::new("NOT_FOUND", "Custom tool configuration unavailable"))?;
            input = json!({"tool_name":name,"params":input,"resolved_tool":{"kind":config.data["kind"],"exec":config.data["exec"]},"python_interpreter":setting("python_interpreter")});
        }
        if name == "web_search" || name == "WebFetch" {
            input["network_config"] = if name == "web_search" {
                self.search_network_config(None)?
            } else {
                json!({"proxy":setting("global_proxy")})
            };
        }
        if name == "ExecuteExtraTool" {
            let wanted = text(&input, "tool_name");
            let disclosed = s
                .records("tool_invocations")
                .into_iter()
                .filter(|r| {
                    r.data["run_id"] == rid
                        && r.data["name"] == "SearchExtraTools"
                        && r.data["state"] == "completed"
                })
                .any(|r| {
                    self.read_json_blob(&r.data["result_ref"])
                        .ok()
                        .and_then(|v| self.tool_result_text(&v).ok())
                        .and_then(|v| serde_json::from_str::<Value>(&v).ok())
                        .is_some_and(|v| {
                            v.as_array()
                                .is_some_and(|a| a.iter().any(|t| t["name"] == wanted))
                        })
                });
            if !disclosed {
                return Err(Error::new(
                    "PERMISSION_DENIED",
                    "Discover this deferred tool with SearchExtraTools first",
                ));
            }
            let run = record(&s, "agent_runs", rid)?;
            let agent = record(&s, "agents", text(&run.data, "agent_id"))?;
            if wanted.starts_with("mcp__") {
                let cache = s
                    .records("mcp_tools_cache")
                    .into_iter()
                    .find(|t| {
                        t.data["tool_name"] == wanted
                            || s.get("mcp_servers", text(&t.data, "server_id"))
                                .is_some_and(|server| {
                                    format!(
                                        "mcp__{}__{}",
                                        text(&server.data, "name"),
                                        text(&t.data, "tool_name")
                                    ) == wanted
                                })
                    })
                    .ok_or_else(|| Error::new("NOT_FOUND", "MCP tool schema no longer exists"))?;
                let server = record(&s, "mcp_servers", text(&cache.data, "server_id"))?;
                let native_name = text(&cache.data, "tool_name")
                    .strip_prefix(&format!("mcp__{}__", text(&server.data, "name")))
                    .unwrap_or(text(&cache.data, "tool_name"));
                let c = artex_agent::context_for_run(&s, rid, &self.engine_epoch)?;
                let visible = artex_domain::sdk::mcp_tool_allowed(
                    &s,
                    &c,
                    &server.header.id,
                    native_name,
                    wanted,
                    &|reference| self.read_blob_bytes(reference, 16 * 1024 * 1024),
                );
                if server.data["enabled"] != true || !visible {
                    return Err(Error::new(
                        "PERMISSION_DENIED",
                        "MCP server or tool is disabled for this agent",
                    ));
                }
                let params = input.get("params").cloned().unwrap_or_else(|| json!({}));
                artex_core::validate_contract(&cache.data["schema"], &json!({}), &params)?;
                input["params"] = params;
                let native_name = text(&cache.data, "tool_name")
                    .strip_prefix(&format!("mcp__{}__", text(&server.data, "name")))
                    .unwrap_or(text(&cache.data, "tool_name"));
                input["resolved_tool"] =
                    json!({"kind":"mcp","server":server.data,"name":native_name});
                return Ok(input);
            }
            let config = s
                .records("tools")
                .into_iter()
                .find(|t| t.data["key"] == wanted)
                .ok_or_else(|| {
                    Error::new("NOT_FOUND", "Deferred tool definition no longer exists")
                })?;
            if config.data["enabled"] != true
                || config.data["deferred"] != true
                || !ids(&config.data, "agents")
                    .iter()
                    .any(|a| a == text(&agent.data, "key") || a == &agent.header.id)
            {
                return Err(Error::new(
                    "PERMISSION_DENIED",
                    "Deferred tool is disabled or not bound to this agent",
                ));
            }
            let params = input.get("params").cloned().unwrap_or_else(|| json!({}));
            artex_core::validate_contract(&config.data["schema"], &json!({}), &params)?;
            input["params"] = params;
            input["resolved_tool"] = json!({"kind":config.data["kind"],"exec":config.data["exec"]});
            input["python_interpreter"] = setting("python_interpreter");
        }
        Ok(input)
    }
}
