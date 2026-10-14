//! Source-shaped configuration services; UI input wrappers remain explicit.
use crate::*;
pub const QUERIES: &[&str] = &[
    "ui.agents",
    "ui.getAgent",
    "ui.agentPromptVersions",
    "ui.agentVariables",
    "ui.previewAgentPrompt",
    "ui.getAgentVisibility",
    "ui.tools",
    "ui.mcpServers",
    "ui.mcpTools",
    "ui.resourceVisibility",
    "ui.skillVisibility",
    "ui.agentTriggers",
    "ui.llmProfiles",
    "ui.llmRetryPolicy",
    "ui.settings",
];
pub const COMMANDS: &[&str] = &[
    "ui.createAgent",
    "ui.updateAgent",
    "ui.deleteAgent",
    "ui.saveAgentConfig",
    "ui.saveAgentPrompt",
    "ui.resetAgentPrompt",
    "ui.saveAgentWrapup",
    "ui.resetAgentWrapup",
    "ui.saveAgentTaskTimeoutWrapup",
    "ui.resetAgentTaskTimeoutWrapup",
    "ui.setAgentVisibility",
    "ui.toggleVisibility",
    "ui.toggleSkillVisibility",
    "ui.saveTool",
    "ui.resetTool",
    "ui.createCustomTool",
    "ui.updateCustomTool",
    "ui.deleteCustomTool",
    "ui.saveMcpServer",
    "ui.deleteMcpServer",
    "ui.createTrigger",
    "ui.updateTrigger",
    "ui.deleteTrigger",
    "ui.saveLLMProfile",
    "ui.deleteLLMProfile",
    "ui.activateLLMProfile",
    "ui.saveLLMRetryPolicy",
    "ui.setSettings",
];
fn keyed<'a>(s: &'a Snapshot, col: &str, key: &str) -> Result<&'a Record> {
    s.records(col)
        .into_iter()
        .find(|r| r.data["key"] == key)
        .ok_or_else(|| Error::new("NOT_FOUND", "Configuration does not exist"))
}
pub fn wire_agent(a: &Record, s: &Snapshot) -> Value {
    let mut d = a.data.clone();
    let out = d.as_object_mut().unwrap();
    for k in [
        "current_prompt_id",
        "wrapup_prompt",
        "wrapup_max_turns",
        "task_timeout_wrapup_prompt",
        "task_timeout_wrapup_max_turns",
        "created_at",
        "updated_at",
    ] {
        out.remove(k);
    }
    if d["description"].is_null() {
        d["description"] = json!("");
    }
    d["mcp_count"] = json!(
        s.records("agent_visibility")
            .iter()
            .filter(|r| r.data["agent_id"] == a.header.id
                && r.data["enabled"] == true
                && r.data["resource_kind"] == "mcp")
            .count()
    );
    d["skill_count"] = json!(
        s.records("agent_skill_visibility")
            .iter()
            .filter(|r| r.data["agent_id"] == a.header.id && r.data["enabled"] == true)
            .count()
    );
    d["tool_count"] = json!(
        s.records("tools")
            .iter()
            .filter(|r| r.data["agents"]
                .as_array()
                .is_some_and(|v| v.contains(&a.data["key"])))
            .count()
    );
    d
}
fn visibility(s: &Snapshot, id: &str) -> Value {
    json!({"mcp":s.records("agent_visibility").iter().filter(|r|r.data["agent_id"]==id&&r.data["resource_kind"]=="mcp"&&r.data["enabled"]==true).map(|r|r.data["resource_id"].clone()).collect::<Vec<_>>(),"skill":s.records("agent_skill_visibility").iter().filter(|r|r.data["agent_id"]==id&&r.data["enabled"]==true).map(|r|r.data["skill_name"].clone()).collect::<Vec<_>>()})
}
fn variables(s: &Snapshot, id: &str) -> Vec<Value> {
    let mut rows=s.records("agent_prompt_vars").into_iter().filter(|r|r.data["agent_id"]==id&&!matches!(text(&r.data,"var_name"),"Now"|"DataDir")).map(|r|json!({"name":r.data["var_name"],"description":r.data["description"],"example":r.data["example"],"source":r.data["source"]})).collect::<Vec<_>>();
    rows.extend([json!({"name":"Now","description":"Current time, refreshed for every run","example":"2026-08-11 14:30:00 UTC","source":"runtime"}),json!({"name":"DataDir","description":"Workspace data root directory","example":"/workspace/data","source":"runtime"})]);
    rows
}
pub fn render_prompt(template: &str, vars: &[Value], sample: &Value) -> Result<String> {
    crate::prompt_template::render(template, vars, sample)
}
pub fn render_agent_prompt(
    s: &Snapshot,
    key: &str,
    template: &str,
    sample: &Value,
) -> Result<String> {
    let agent = keyed(s, "agents", key)?;
    render_prompt(template, &variables(s, &agent.header.id), sample)
}
/// Runtime fields mirror the source role structs. Preview examples and editable
/// variable catalogs cannot become live scope, targets or findings.
pub fn render_agent_prompt_runtime(key: &str, template: &str, sample: &Value) -> Result<String> {
    let role_fields: &[&str] = match key {
        "planner" => &["Goal", "Scope", "AssetSummary"],
        "worker" => &["ProxyAddr", "WorkerName"],
        "mainagent" => &["Goal", "AssetSummary", "FindingsSummary"],
        "goals" => &["EngagementDescription"],
        _ => &[],
    };
    let mut vars = Vec::new();
    let mut values = serde_json::Map::new();
    for name in role_fields.iter().copied().chain(["DataDir", "Now"]) {
        let value = match sample.get(name) {
            Some(Value::String(value)) => value.clone(),
            Some(Value::Null) | None => String::new(),
            _ => {
                return Err(Error::new(
                    "INVALID_PARAMS",
                    "Runtime prompt fields must be strings",
                ));
            }
        };
        vars.push(json!({"name":name,"example":""}));
        values.insert(name.into(), json!(value));
    }
    render_prompt(template, &vars, &Value::Object(values))
}
fn versions(s: &Snapshot, id: &str) -> Vec<Value> {
    let mut rows = s
        .records("agent_prompts")
        .into_iter()
        .filter(|r| r.data["agent_id"] == id)
        .collect::<Vec<_>>();
    rows.sort_by_key(|r| std::cmp::Reverse(r.data["version"].as_i64().unwrap_or(0)));
    rows.iter().map(|r|json!({"version":r.data["version"].as_i64().unwrap_or(0).to_string(),"template_text":r.data["template_text"],"note":r.data["note"].as_str().unwrap_or(""),"ts":r.header.created_at})).collect()
}
pub fn wrapup_default(key: &str, timeout: bool) -> &'static str {
    static CATALOG: std::sync::OnceLock<Value> = std::sync::OnceLock::new();
    let catalog = CATALOG.get_or_init(|| {
        serde_json::from_str(include_str!("../resources/builtin-wrapup.v1.json"))
            .expect("wrapup catalog")
    });
    let name = if timeout {
        match key {
            "worker" => "worker_timeout",
            "planner" => "planner_timeout",
            _ => return "",
        }
    } else {
        match key {
            "worker" | "planner" | "mainagent" => key,
            _ => "generic",
        }
    };
    catalog[name].as_str().unwrap()
}
fn profile_wire(r: &Record) -> Value {
    let mut d = r.data.clone();
    for key in ["api_key", "created_at", "updated_at"] {
        d.as_object_mut().unwrap().remove(key);
    }
    let mut retry = json!({});
    for layer in ["connect", "empty", "stream"] {
        retry[layer] = json!({"attempts":d[format!("retry_{layer}_attempts")],"interval_ms":d[format!("retry_{layer}_interval_ms")]});
        d.as_object_mut()
            .unwrap()
            .remove(&format!("retry_{layer}_attempts"));
        d.as_object_mut()
            .unwrap()
            .remove(&format!("retry_{layer}_interval_ms"));
    }
    d["retry"] = retry;
    for k in ["base_url", "proxy", "api_key_hint"] {
        if d[k].is_null() || d[k] == "" {
            d.as_object_mut().unwrap().remove(k);
        }
    }
    d
}
fn retry_policy(p: &Value) -> Value {
    let mut out = json!({});
    for layer in ["connect", "empty", "stream", "breaker", "intent"] {
        out[layer] = json!({"attempts":p[layer]["attempts"].as_i64().unwrap_or(0).clamp(-1,20),"interval_ms":p[layer]["interval_ms"].as_i64().unwrap_or(0).clamp(0,3600000)});
    }
    out
}
pub fn setting(s: &Snapshot, key: &str) -> String {
    s.records("settings")
        .into_iter()
        .find(|r| r.data["key"] == key)
        .map(|r| text(&r.data, "value").into())
        .unwrap_or_default()
}
fn settings_payload(s: &Snapshot) -> Value {
    let mut p = json!({"traffic_capture":false,"agent_traffic_binding":false,"llm_record":false,"web_search_enabled":false,"web_search_backend":"ddgs","brave_key_set":false,"tavily_key_set":false,"web_search_proxy":"","global_proxy":"","python_interpreter":"","workers":3,"task_concurrency_enabled":false,"task_concurrency_limit":5,"llm_pool_enabled":false,"llm_pool_bind_fallback":false,"constraints_inject_planner":true,"constraints_inject_worker":true,"noa_compaction":false,"notify_enabled":true,"notify_public_base_url":"","notify_digest_interval_min":30});
    p["approval_bypass"] = json!(false);
    for (k, v) in p.as_object_mut().unwrap() {
        let raw = setting(s, k);
        if !raw.is_empty() {
            *v = if v.is_boolean() {
                json!(raw == "true")
            } else if v.is_number() {
                json!(raw.parse::<i64>().unwrap_or(v.as_i64().unwrap_or(0)))
            } else {
                json!(raw)
            };
        }
    }
    p["brave_key_set"] = json!(!setting(s, "brave_search_api_key").is_empty());
    p["tavily_key_set"] = json!(!setting(s, "tavily_search_api_key").is_empty());
    p
}
pub fn query(m: &str, p: &Value, s: &Snapshot, _: &Context) -> Result<Value> {
    match m {
        "ui.agents" => {
            let mut rows = s.records("agents");
            rows.sort_by(|a, b| {
                a.header
                    .created_at
                    .cmp(&b.header.created_at)
                    .then(a.header.id.cmp(&b.header.id))
            });
            Ok(json!(
                rows.iter().map(|r| wire_agent(r, s)).collect::<Vec<_>>()
            ))
        }
        "ui.getAgent" => {
            let a = keyed(s, "agents", required(p, "key")?)?;
            let prompt = s
                .get("agent_prompts", text(&a.data, "current_prompt_id"))
                .map(|r| r.data["template_text"].clone())
                .unwrap_or(artex_core::body(""));
            let profiles=s.records("llm_profiles").iter().map(|r|json!({"id":r.header.id,"name":r.data["name"],"model":r.data["model"],"is_default":r.data["is_default"]})).collect::<Vec<_>>();
            let key = text(&a.data, "key");
            Ok(
                json!({"agent":wire_agent(a,s),"prompt":prompt,"variables":variables(s,&a.header.id),"versions":versions(s,&a.header.id),"visibility":visibility(s,&a.header.id),"llm_profiles":profiles,"wrapup_prompt":a.data["wrapup_prompt"],"wrapup_default":wrapup_default(key,false),"wrapup_max_turns":a.data["wrapup_max_turns"],"wrapup_max_turns_default":10,"task_timeout_wrapup_supported":matches!(key,"worker"|"planner"),"task_timeout_wrapup_prompt":a.data["task_timeout_wrapup_prompt"],"task_timeout_wrapup_default":wrapup_default(key,true),"task_timeout_wrapup_max_turns":a.data["task_timeout_wrapup_max_turns"],"task_timeout_wrapup_max_turns_default":10}),
            )
        }
        "ui.agentPromptVersions" => Ok(json!(versions(
            s,
            &keyed(s, "agents", required(p, "key")?)?.header.id
        ))),
        "ui.agentVariables" => Ok(json!(variables(
            s,
            &keyed(s, "agents", required(p, "key")?)?.header.id
        ))),
        "ui.previewAgentPrompt" => {
            let vars = variables(s, &keyed(s, "agents", required(p, "key")?)?.header.id);
            Ok(
                match render_prompt(text(p, "template"), &vars, &p["sample"]) {
                    Ok(rendered) => json!({"rendered":rendered}),
                    Err(e) => json!({"rendered":"","error":e.message}),
                },
            )
        }
        "ui.getAgentVisibility" => Ok(visibility(
            s,
            &keyed(s, "agents", required(p, "key")?)?.header.id,
        )),
        "ui.tools" => {
            let mut rows = s.records("tools");
            rows.sort_by(|a, b| text(&a.data, "key").cmp(text(&b.data, "key")));
            Ok(json!(
                rows.iter()
                    .map(|r| {
                        let mut d = r.data.clone();
                        d.as_object_mut().unwrap().remove("updated_at");
                        if d["exec"].is_null() {
                            d.as_object_mut().unwrap().remove("exec");
                        }
                        d["calls"] = json!(
                            s.records("tool_usage")
                                .iter()
                                .filter(|u| u.data["tool_key"] == r.data["key"])
                                .count()
                        );
                        d
                    })
                    .collect::<Vec<_>>()
            ))
        }
        "ui.mcpServers" => Ok(json!(
            s.records("mcp_servers")
                .iter()
                .map(|r| {
                    let mut d = r.data.clone();
                    d["tools"] = json!(
                        s.records("mcp_tools_cache")
                            .iter()
                            .filter(|t| t.data["server_id"] == r.header.id)
                            .map(|t| t.data["tool_name"].clone())
                            .collect::<Vec<_>>()
                    );
                    for key in ["created_at", "updated_at"] {
                        d.as_object_mut().unwrap().remove(key);
                    }
                    for key in ["command", "url"] {
                        if d[key].is_null() {
                            d.as_object_mut().unwrap().remove(key);
                        }
                    }
                    d
                })
                .collect::<Vec<_>>()
        )),
        "ui.mcpTools" => {
            let id = required(p, "id")?;
            record(s, "mcp_servers", id)?;
            Ok(json!(s.records("mcp_tools_cache").iter().filter(|t|t.data["server_id"]==id).map(|t|json!({"name":t.data["tool_name"],"description":t.data["description"],"schema":t.data["schema"]})).collect::<Vec<_>>()))
        }
        "ui.resourceVisibility" => {
            if p["kind"] != "mcp" {
                return Err(Error::new(
                    "INVALID_PARAMS",
                    "Only MCP resources use resource visibility",
                ));
            }
            let id = required(p, "id")?;
            record(s, "mcp_servers", id)?;
            Ok(json!(
                s.records("agent_visibility")
                    .iter()
                    .filter(|r| r.data["resource_kind"] == "mcp"
                        && r.data["resource_id"] == id
                        && r.data["enabled"] == true)
                    .map(|r| r.data["agent_id"].clone())
                    .collect::<Vec<_>>()
            ))
        }
        "ui.skillVisibility" => Ok(json!(
            s.records("agent_skill_visibility")
                .iter()
                .filter(|r| r.data["skill_name"] == p["name"] && r.data["enabled"] == true)
                .map(|r| r.data["agent_id"].clone())
                .collect::<Vec<_>>()
        )),
        "ui.agentTriggers" => {
            keyed(s, "agents", required(p, "key")?)?;
            Ok(json!(
                s.records("agent_triggers")
                    .iter()
                    .filter(|r| r.data["agent_key"] == p["key"])
                    .map(|r| {
                        let mut d = r.data.clone();
                        for key in ["created_at", "updated_at"] {
                            d.as_object_mut().unwrap().remove(key);
                        }
                        if d["last_fire"].is_null() {
                            d.as_object_mut().unwrap().remove("last_fire");
                        }
                        d["tool_names"] = serde_json::from_str::<Value>(text(&d, "tool_names"))
                            .unwrap_or(json!([]));
                        d
                    })
                    .collect::<Vec<_>>()
            ))
        }
        "ui.llmProfiles" => {
            let mut rows = s.records("llm_profiles");
            rows.sort_by(|a, b| {
                a.header
                    .created_at
                    .cmp(&b.header.created_at)
                    .then(a.header.id.cmp(&b.header.id))
            });
            Ok(json!(
                rows.iter().map(|r| profile_wire(r)).collect::<Vec<_>>()
            ))
        }
        "ui.llmRetryPolicy" => Ok(retry_policy(
            &serde_json::from_str::<Value>(&setting(s, "llm_retry_policy")).unwrap_or(Value::Null),
        )),
        "ui.settings" => Ok(settings_payload(s)),
        _ => Err(Error::new("METHOD_NOT_FOUND", m)),
    }
}
fn save_prompt(b: &mut Planner<'_>, id: &str, template: &str, note: &str, by: &str) -> Result<i64> {
    let next = b
        .state
        .records("agent_prompts")
        .iter()
        .filter(|r| r.data["agent_id"] == id)
        .filter_map(|r| r.data["version"].as_i64())
        .max()
        .unwrap_or(0)
        + 1;
    let pid=b.insert("agent_prompts","prompt",json!({"agent_id":id,"version":next,"template_text":artex_core::body(template),"note":note,"updated_by":by}))?;
    let mut a = b.data("agents", id)?;
    a["current_prompt_id"] = json!(pid);
    b.put("agents", id, a);
    Ok(next)
}
fn set_setting(b: &mut Planner<'_>, key: &str, value: &str) -> Result<()> {
    if let Some(r) = b
        .state
        .records("settings")
        .into_iter()
        .find(|r| r.data["key"] == key)
    {
        let mut d = r.data.clone();
        d["value"] = json!(value);
        b.put("settings", &r.header.id, d);
    } else {
        b.insert("settings", "setting", json!({"key":key,"value":value}))?;
    }
    Ok(())
}
fn put_visibility(
    b: &mut Planner<'_>,
    agent: &str,
    kind: &str,
    key: &str,
    enabled: bool,
) -> Result<()> {
    let (col, field, prefix) = if kind == "mcp" {
        ("agent_visibility", "resource_id", "visibility")
    } else {
        ("agent_skill_visibility", "skill_name", "skillvisibility")
    };
    let row = b.state.records(col).into_iter().find(|r| {
        r.data["agent_id"] == agent
            && r.data[field] == key
            && (kind != "mcp" || r.data["resource_kind"] == "mcp")
    });
    let mut d = row.map(|r| r.data.clone()).unwrap_or(default_data(col)?);
    d["agent_id"] = json!(agent);
    d[field] = json!(key);
    d["enabled"] = json!(enabled);
    if kind == "mcp" {
        d["resource_kind"] = json!("mcp");
    }
    if let Some(r) = row {
        b.put(col, &r.header.id, d);
    } else {
        b.insert(col, prefix, d)?;
    }
    Ok(())
}
pub fn plan(m: &str, p: &Value, s: &Snapshot, c: &Context) -> Result<CommandBatch> {
    let mut b = Planner::new(s, m, p);
    let result = match m {
        "ui.createAgent" => {
            let key = required(p, "key")?.trim();
            if !key.starts_with(|x: char| x.is_ascii_lowercase())
                || !key
                    .chars()
                    .all(|x| x.is_ascii_lowercase() || x.is_ascii_digit() || x == '_')
            {
                return Err(Error::new(
                    "INVALID_PARAMS",
                    "Agent key must contain lowercase letters, digits, or underscores",
                ));
            }
            if keyed(s, "agents", key).is_ok() {
                return Err(Error::new("VERSION_CONFLICT", "Agent key already exists"));
            }
            let name = required(p, "name")?.trim();
            let id=b.insert("agents","agent",json!({"key":key,"name":name,"description":text(p,"description"),"role":"assistant","builtin":false,"enabled":true}))?;
            save_prompt(
                &mut b,
                &id,
                text(&bootstrap::prompts(), "assistant"),
                "Starter prompt",
                "system",
            )?;
            let mut wire = b.data("agents", &id)?;
            for k in [
                "current_prompt_id",
                "wrapup_prompt",
                "wrapup_max_turns",
                "task_timeout_wrapup_prompt",
                "task_timeout_wrapup_max_turns",
                "created_at",
                "updated_at",
            ] {
                wire.as_object_mut().unwrap().remove(k);
            }
            for k in ["mcp_count", "skill_count", "tool_count"] {
                wire[k] = json!(0);
            }
            wire
        }
        "ui.updateAgent" | "ui.deleteAgent" => {
            let a = keyed(s, "agents", required(p, "key")?)?;
            if a.data["builtin"] == true {
                return Err(Error::new(
                    "INVALID_PARAMS",
                    "Built-in agent metadata cannot be changed or deleted",
                ));
            }
            if m == "ui.updateAgent" {
                let mut d = a.data.clone();
                d["name"] = json!(required(p, "name")?.trim());
                d["description"] = json!(text(p, "description"));
                b.put("agents", &a.header.id, d);
                json!({"ok":true})
            } else {
                for t in s.records("tools") {
                    let mut d = t.data.clone();
                    if let Some(values) = d["agents"].as_array_mut() {
                        let old = values.len();
                        values.retain(|v| v != &p["key"]);
                        if values.len() != old {
                            b.put("tools", &t.header.id, d);
                        }
                    }
                }
                for t in s.records("agent_triggers") {
                    if t.data["agent_key"] == p["key"] {
                        b.delete("agent_triggers", &t.header.id);
                    }
                }
                b.delete("agents", &a.header.id);
                json!({"deleted":p["key"]})
            }
        }
        "ui.saveAgentConfig" => {
            let a = keyed(s, "agents", required(p, "key")?)?;
            let mut d = a.data.clone();
            let patch = &p["patch"];
            if let Some(id) = patch["llm_profile_id"].as_str() {
                let profile = record(s, "llm_profiles", id)?;
                if profile.data["api_key"].is_null() || text(&profile.data, "model").is_empty() {
                    return Err(Error::new(
                        "INVALID_PARAMS",
                        "Bound profile is not configured",
                    ));
                }
            }
            merge(&mut d, patch);
            for key in ["max_turns", "run_seconds", "trigger_max_parallel"] {
                if let Some(value) = patch[key].as_i64() {
                    d[key] = json!(value.max(0));
                }
            }
            if !matches!(text(&d, "trigger_run_mode"), "serial" | "parallel")
                || !matches!(text(&d, "trigger_merge_mode"), "by_task" | "all" | "none")
            {
                return Err(Error::new("INVALID_PARAMS", "Invalid trigger policy"));
            }
            b.put("agents", &a.header.id, d);
            json!({"ok":true})
        }
        "ui.saveAgentPrompt" | "ui.resetAgentPrompt" => {
            let a = keyed(s, "agents", required(p, "key")?)?;
            let catalog = bootstrap::prompts();
            let template = if m == "ui.resetAgentPrompt" {
                catalog[text(&a.data, "key")].as_str().ok_or_else(|| {
                    Error::new("INVALID_PARAMS", "Custom agent has no built-in prompt")
                })?
            } else {
                render_prompt(text(p, "template"), &variables(s, &a.header.id), &json!({}))?;
                text(p, "template")
            };
            let version = save_prompt(
                &mut b,
                &a.header.id,
                template,
                if m == "ui.resetAgentPrompt" {
                    "Revert to built-in default"
                } else {
                    text(p, "note")
                },
                if m == "ui.resetAgentPrompt" {
                    "system"
                } else {
                    "ui"
                },
            )?;
            json!({"version":version.to_string()})
        }
        "ui.saveAgentWrapup"
        | "ui.resetAgentWrapup"
        | "ui.saveAgentTaskTimeoutWrapup"
        | "ui.resetAgentTaskTimeoutWrapup" => {
            let a = keyed(s, "agents", required(p, "key")?)?;
            let timeout = m.contains("TaskTimeout");
            if timeout && !matches!(text(&a.data, "key"), "worker" | "planner") {
                return Err(Error::new(
                    "INVALID_PARAMS",
                    "Only workers and planners have task timeout wrapup",
                ));
            }
            let field = if timeout {
                "task_timeout_wrapup_prompt"
            } else {
                "wrapup_prompt"
            };
            let turns = if timeout {
                "task_timeout_wrapup_max_turns"
            } else {
                "wrapup_max_turns"
            };
            let reset = m.contains("reset");
            let mut d = a.data.clone();
            d[field] = artex_core::body(if reset { "" } else { text(p, "prompt") });
            if reset {
                d[turns] = json!(0);
            } else if let Some(n) = p["maxTurns"].as_i64() {
                d[turns] = json!(n.max(0));
            }
            b.put("agents", &a.header.id, d);
            let mut out = json!({"ok":true});
            if reset {
                out[if timeout {
                    "task_timeout_wrapup_default"
                } else {
                    "wrapup_default"
                }] = json!(wrapup_default(text(&a.data, "key"), timeout));
                out[if timeout {
                    "task_timeout_wrapup_max_turns_default"
                } else {
                    "wrapup_max_turns_default"
                }] = json!(10);
            }
            out
        }
        "ui.setAgentVisibility" => {
            let a = keyed(s, "agents", required(p, "key")?)?;
            let mcp = ids(p, "mcp");
            let skills = ids(p, "skill");
            for id in &mcp {
                record(s, "mcp_servers", id)?;
            }
            for r in s.records("agent_visibility") {
                if r.data["agent_id"] == a.header.id && r.data["resource_kind"] == "mcp" {
                    put_visibility(
                        &mut b,
                        &a.header.id,
                        "mcp",
                        text(&r.data, "resource_id"),
                        false,
                    )?;
                }
            }
            for r in s.records("agent_skill_visibility") {
                if r.data["agent_id"] == a.header.id {
                    put_visibility(
                        &mut b,
                        &a.header.id,
                        "skill",
                        text(&r.data, "skill_name"),
                        false,
                    )?;
                }
            }
            for id in mcp {
                put_visibility(&mut b, &a.header.id, "mcp", &id, true)?;
            }
            for name in skills {
                put_visibility(&mut b, &a.header.id, "skill", &name, true)?;
            }
            json!({"ok":true})
        }
        "ui.toggleVisibility" | "ui.toggleSkillVisibility" => {
            let id = required(p, "agentId")?;
            record(s, "agents", id)?;
            let kind = if m == "ui.toggleVisibility" {
                if p["kind"] != "mcp" {
                    return Err(Error::new("INVALID_PARAMS", "Invalid visibility kind"));
                }
                "mcp"
            } else {
                "skill"
            };
            let key = required(
                p,
                if kind == "mcp" {
                    "resourceId"
                } else {
                    "skillName"
                },
            )?;
            if kind == "mcp" {
                record(s, "mcp_servers", key)?;
            }
            put_visibility(&mut b, id, kind, key, p["visible"] == true)?;
            json!({"ok":true})
        }
        "ui.saveTool" | "ui.resetTool" => {
            let r = keyed(s, "tools", required(p, "key")?)?;
            let mut d = r.data.clone();
            if m == "ui.resetTool" {
                let def = artex_core::contracts::read("tools.v1.json")["tools"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .find(|t| t["name"] == p["key"])
                    .cloned()
                    .ok_or_else(|| Error::new("NOT_FOUND", "No built-in tool default"))?;
                merge(&mut d, &bootstrap::tool_seed(&def));
            } else {
                merge(&mut d, &p["patch"]);
            }
            b.put("tools", &r.header.id, d);
            json!({"ok":true})
        }
        "ui.createCustomTool" | "ui.updateCustomTool" => {
            let mut input = p["t"].clone();
            if m == "ui.updateCustomTool" {
                input["key"] = p["key"].clone();
            }
            let mut batch = configuration::plan(
                if m == "ui.createCustomTool" {
                    "create_custom_tool"
                } else {
                    "update_custom_tool"
                },
                &input,
                s,
                c,
            )?;
            batch.method = m.into();
            batch.normalized_params = p.clone();
            batch.result = if m == "ui.createCustomTool" {
                json!({"key":batch.result["key"]})
            } else {
                json!({"ok":true})
            };
            augment_expected(&mut batch, s)?;
            return Ok(batch);
        }
        "ui.deleteCustomTool" => {
            let r = keyed(s, "tools", required(p, "key")?)?;
            if r.data["system"] == true {
                return Err(Error::new(
                    "INVALID_PARAMS",
                    "Built-in tools cannot be deleted",
                ));
            }
            b.delete("tools", &r.header.id);
            json!({"deleted":p["key"]})
        }
        "ui.saveMcpServer" => {
            let mcp = &p["m"];
            let mut batch = configuration::plan(
                if mcp["id"].is_string() {
                    "update_mcp"
                } else {
                    "create_mcp"
                },
                mcp,
                s,
                c,
            )?;
            batch.method = m.into();
            batch.normalized_params = p.clone();
            batch.result = json!({"id":batch.result["id"]});
            augment_expected(&mut batch, s)?;
            return Ok(batch);
        }
        "ui.deleteMcpServer" => {
            let id = required(p, "id")?;
            record(s, "mcp_servers", id)?;
            b.delete("mcp_servers", id);
            json!({"deleted":id})
        }
        "ui.createTrigger" | "ui.updateTrigger" => {
            let existing = if m == "ui.updateTrigger" {
                Some(record(s, "agent_triggers", required(p, "id")?)?)
            } else {
                None
            };
            let key = existing
                .map(|r| text(&r.data, "agent_key"))
                .unwrap_or(text(p, "key"));
            let a = keyed(s, "agents", key)?;
            if a.data["builtin"] == true {
                return Err(Error::new(
                    "INVALID_PARAMS",
                    "Built-in orchestration agents do not support custom triggers",
                ));
            }
            let mut d = existing
                .map(|r| r.data.clone())
                .unwrap_or(default_data("agent_triggers")?);
            merge(&mut d, &p["t"]);
            d["agent_key"] = json!(key);
            let names = ids(&p["t"], "tool_names");
            if d["on_tool_call"] == true && names.is_empty() {
                return Err(Error::new(
                    "INVALID_PARAMS",
                    "Tool-call trigger requires selected tool names",
                ));
            }
            d["tool_names"] = json!(if names.is_empty() {
                String::new()
            } else {
                serde_json::to_string(&names)?
            });
            if d["interval_sec"].as_i64().unwrap_or(0) < 0 {
                return Err(Error::new(
                    "INVALID_PARAMS",
                    "Trigger interval cannot be negative",
                ));
            }
            if let Some(r) = existing {
                b.put("agent_triggers", &r.header.id, d);
                json!({"ok":true})
            } else {
                let id = b.insert("agent_triggers", "trigger", d)?;
                let mut out = b.data("agent_triggers", &id)?;
                out["tool_names"] = json!(names);
                for k in ["last_fire", "created_at", "updated_at"] {
                    out.as_object_mut().unwrap().remove(k);
                }
                out
            }
        }
        "ui.deleteTrigger" => {
            let id = required(p, "id")?;
            record(s, "agent_triggers", id)?;
            b.delete("agent_triggers", id);
            json!({"deleted":id})
        }
        "ui.saveLLMProfile" => {
            let input = &p["p"];
            let existing = input["id"]
                .as_str()
                .map(|id| record(s, "llm_profiles", id))
                .transpose()?;
            let mut d = default_data("llm_profiles")?;
            merge(&mut d, input);
            if let Some(r) = existing {
                d["id"] = json!(r.header.id);
                d["is_default"] = r.data["is_default"].clone();
                d["api_key"] = r.data["api_key"].clone();
                d["api_key_hint"] = r.data["api_key_hint"].clone();
            }
            let key = &input["api_key"];
            if key.is_object() {
                if key["secret_ref"].as_str().is_none() {
                    return Err(Error::new("INVALID_PARAMS", "Invalid API SecretRef"));
                }
                d["api_key"] = key.clone();
                if let Some(hint) = input.get("api_key_hint") {
                    d["api_key_hint"] = hint.clone();
                }
            } else if key.as_str().is_some_and(|v| !v.is_empty()) {
                return Err(Error::new(
                    "PERMISSION_DENIED",
                    "Host must normalize API keys into SecretRef",
                ));
            } else if existing.is_none() {
                d["api_key"] = Value::Null;
            }
            d["streaming"] = json!(input["streaming"].as_bool().unwrap_or(true));
            d["max_tokens"] = json!(input["max_tokens"].as_i64().unwrap_or(0).max(0));
            if d["format"] != "openai" || d["max_tokens_field"] != "max_completion_tokens" {
                d["max_tokens_field"] = json!("");
            }
            for layer in ["connect", "empty", "stream"] {
                d[format!("retry_{layer}_attempts")] = json!(
                    input["retry"][layer]["attempts"]
                        .as_i64()
                        .unwrap_or(0)
                        .clamp(-1, 20)
                );
                d[format!("retry_{layer}_interval_ms")] = json!(
                    input["retry"][layer]["interval_ms"]
                        .as_i64()
                        .unwrap_or(0)
                        .clamp(0, 3600000)
                );
            }
            required(&d, "name")?;
            required(&d, "model")?;
            if !matches!(
                text(&d, "format"),
                "openai" | "anthropic" | "openai-responses"
            ) {
                return Err(Error::new("INVALID_PARAMS", "Unknown LLM format"));
            }
            let id = if let Some(r) = existing {
                b.put("llm_profiles", &r.header.id, d);
                r.header.id.clone()
            } else {
                b.insert("llm_profiles", "profile", d)?
            };
            json!({"id":id})
        }
        "ui.deleteLLMProfile" => {
            let id = required(p, "id")?;
            let r = record(s, "llm_profiles", id)?;
            if r.data["is_default"] == true {
                return Err(Error::new(
                    "VERSION_CONFLICT",
                    "Activate another profile before deleting the active profile",
                ));
            }
            b.delete("llm_profiles", id);
            json!({"deleted":id})
        }
        "ui.activateLLMProfile" => {
            let id = required(p, "id")?;
            let r = record(s, "llm_profiles", id)?;
            if r.data["api_key"].is_null() || text(&r.data, "model").is_empty() {
                return Err(Error::new("INVALID_PARAMS", "Profile is not configured"));
            }
            for r in s.records("llm_profiles") {
                if r.data["is_default"] == true || r.header.id == id {
                    let mut d = r.data.clone();
                    d["is_default"] = json!(r.header.id == id);
                    b.put("llm_profiles", &r.header.id, d);
                }
            }
            json!({"ok":true})
        }
        "ui.saveLLMRetryPolicy" => {
            let policy = retry_policy(&p["p"]);
            set_setting(&mut b, "llm_retry_policy", &serde_json::to_string(&policy)?)?;
            policy
        }
        "ui.setSettings" => {
            let patch = p["patch"]
                .as_object()
                .ok_or_else(|| Error::new("INVALID_PARAMS", "Settings patch must be an object"))?;
            let mut out = settings_payload(s);
            if let Some(value) = patch.get("approval_bypass") {
                if c.actor.kind != "user" {
                    return Err(Error::new(
                        "PERMISSION_DENIED",
                        "Only a user may change workspace approval bypass",
                    ));
                }
                if !value.is_boolean() {
                    return Err(Error::new(
                        "INVALID_PARAMS",
                        "Approval bypass must be boolean",
                    ));
                }
            }
            for (k, v) in patch {
                if v.is_null() {
                    continue;
                }
                if k == "brave_search_api_key" || k == "tavily_search_api_key" {
                    let value = if v == "" {
                        String::new()
                    } else if v["secret_ref"].is_string() {
                        serde_json::to_string(v)?
                    } else {
                        return Err(Error::new(
                            "PERMISSION_DENIED",
                            "Host must normalize search keys into SecretRef",
                        ));
                    };
                    set_setting(&mut b, k, &value)?;
                    out[if k.starts_with("brave") {
                        "brave_key_set"
                    } else {
                        "tavily_key_set"
                    }] = json!(!value.is_empty());
                    continue;
                }
                if !out.as_object().unwrap().contains_key(k) || k.ends_with("_key_set") {
                    continue;
                }
                let value = if k == "notify_public_base_url" {
                    let base = v.as_str().unwrap_or("").trim().trim_end_matches('/');
                    if !base.is_empty()
                        && !base.starts_with("https://")
                        && !base.starts_with("http://")
                    {
                        return Err(Error::new(
                            "INVALID_PARAMS",
                            "Notification URL requires HTTP or HTTPS",
                        ));
                    }
                    json!(base)
                } else {
                    v.clone()
                };
                if matches!(k.as_str(), "workers" | "task_concurrency_limit")
                    && value.as_i64().unwrap_or(0) < 1
                {
                    return Err(Error::new(
                        "INVALID_PARAMS",
                        "Concurrency count must be positive",
                    ));
                }
                if k == "notify_digest_interval_min"
                    && !(1..=1440).contains(&value.as_i64().unwrap_or(0))
                {
                    return Err(Error::new(
                        "INVALID_PARAMS",
                        "Digest interval must be within 1..1440 minutes",
                    ));
                }
                if k == "web_search_backend"
                    && !matches!(
                        value.as_str(),
                        Some("ddgs" | "brave-free" | "tavily" | "deepseek")
                    )
                {
                    return Err(Error::new("INVALID_PARAMS", "Invalid search backend"));
                }
                let raw = value
                    .as_str()
                    .map(str::to_owned)
                    .unwrap_or(value.to_string());
                set_setting(&mut b, k, &raw)?;
                out[k] = value;
            }
            if patch.get("approval_bypass") == Some(&json!(true)) {
                crate::approval_bypass::settle_pending(&mut b, c)?;
            }
            out
        }
        _ => return Err(Error::new("METHOD_NOT_FOUND", m)),
    };
    if let Some(Change::Put { collection, id, .. } | Change::Delete { collection, id }) =
        b.batch.changes.first().cloned()
    {
        conversation::push_event(&mut b.batch, s, c, "ConfigurationChanged", &collection, &id)?;
    }
    let mut batch = b.done(result);
    augment_expected(&mut batch, s)?;
    Ok(batch)
}
