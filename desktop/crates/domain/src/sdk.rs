use crate::*;
use chrono::{Duration, Utc};
use std::collections::BTreeMap;

/// Exact MCP tool selections override a server-wide wildcard, including denial.
pub fn mcp_tool_visible(
    s: &Snapshot,
    agent_id: &str,
    server_id: &str,
    raw_name: &str,
    full_name: &str,
) -> bool {
    let rows: Vec<&Record> = s
        .records("agent_visibility")
        .into_iter()
        .filter(|row| {
            row.data["agent_id"] == agent_id
                && row.data["resource_kind"] == "mcp"
                && row.data["resource_id"] == server_id
        })
        .collect();
    let exact: Vec<&&Record> = rows
        .iter()
        .filter(|row| {
            row.data["mcp_tool_name"] == raw_name || row.data["mcp_tool_name"] == full_name
        })
        .collect();
    if !exact.is_empty() {
        return exact.iter().all(|row| row.data["enabled"] == true);
    }
    rows.iter().any(|row| {
        row.data["mcp_tool_name"].as_str().is_none_or(str::is_empty) && row.data["enabled"] == true
    })
}

/// Actual native skill metadata captured for this run, intersected with the
/// current agent's visibility. Revocation takes effect without rebuilding a run.
pub fn visible_skills(s: &Snapshot, c: &Context) -> Vec<Value> {
    let Ok(run) = current_run(s, c) else {
        return vec![];
    };
    let agent = text(&run.data, "agent_id");
    if s.get("agents", agent)
        .is_none_or(|a| a.data["enabled"] != true)
    {
        return vec![];
    }
    let mut skills = BTreeMap::new();
    for skill in run.data["context_snapshot"]["skill_catalog"]
        .as_array()
        .into_iter()
        .flatten()
    {
        let name = text(skill, "name");
        if name.is_empty()
            || !s.records("agent_skill_visibility").iter().any(|row| {
                row.data["agent_id"] == agent
                    && row.data["skill_name"] == name
                    && row.data["enabled"] == true
            })
        {
            continue;
        }
        skills
            .entry(name.to_owned())
            .or_insert_with(|| skill.clone());
    }
    skills.into_values().collect()
}

fn mcp_exact_denied(
    s: &Snapshot,
    agent_id: &str,
    server_id: &str,
    raw_name: &str,
    full_name: &str,
) -> bool {
    s.records("agent_visibility").iter().any(|row| {
        row.data["agent_id"] == agent_id
            && row.data["resource_kind"] == "mcp"
            && row.data["resource_id"] == server_id
            && (row.data["mcp_tool_name"] == raw_name || row.data["mcp_tool_name"] == full_name)
            && row.data["enabled"] != true
    })
}

fn skill_servers(skill: &Value) -> impl Iterator<Item = &str> {
    let refs = skill
        .get("mcps")
        .filter(|value| !value.is_null())
        .or_else(|| skill.get("mcp"));
    refs.and_then(Value::as_str).into_iter().chain(
        refs.and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(Value::as_str),
    )
}

fn skill_body(value: &Value, reader: &dyn Fn(&Value) -> Result<Vec<u8>>) -> Result<String> {
    if let Some(text) = value.as_str().or_else(|| value["text"].as_str()) {
        return Ok(text.to_owned());
    }
    if value["blob"].is_object() {
        return String::from_utf8(reader(&value["blob"])?)
            .map_err(|_| Error::new("CORRUPT_STORE", "Skill guidance is not UTF-8"));
    }
    Err(Error::new("NOT_FOUND", "Skill guidance body is absent"))
}

/// Skill unlock is recovered from confirmed independent guidance messages in
/// exactly the history this run may consume. A blob read failure never grants.
pub fn mcp_skill_allowed(
    s: &Snapshot,
    c: &Context,
    server_name: &str,
    reader: &dyn Fn(&Value) -> Result<Vec<u8>>,
) -> bool {
    let Ok(run) = current_run(s, c) else {
        return false;
    };
    let gated: BTreeSet<String> = visible_skills(s, c)
        .iter()
        .filter(|skill| skill_servers(skill).any(|name| name == server_name))
        .map(|skill| text(skill, "name").to_owned())
        .collect();
    if gated.is_empty() {
        return false;
    }
    let parent = if run.data["conversation_id"].is_null() {
        run.data["parent_run_id"]
            .as_str()
            .and_then(|id| s.get("agent_runs", id))
    } else {
        None
    };
    let conversation = run.data["conversation_id"]
        .as_str()
        .or_else(|| parent.and_then(|p| p.data["conversation_id"].as_str()));
    s.records("activity")
        .into_iter()
        .chain(s.records("conversation_activities"))
        .filter(|row| {
            row.data["is_error"] != true
                && if row.header.collection == "conversation_activities" {
                    conversation.is_some_and(|id| row.data["conversation_id"] == id)
                } else {
                    row.data["metadata"]["run_id"] == run.header.id
                        || parent.is_some_and(|p| {
                            p.data["conversation_id"].is_null()
                                && row.data["metadata"]["run_id"] == p.header.id
                        })
                }
        })
        .any(|row| {
            let Ok(body) = skill_body(&row.data["detail"], reader) else {
                return false;
            };
            let Ok(envelope) = serde_json::from_str::<Value>(&body) else {
                return false;
            };
            let marker = &envelope["skill_invocation"];
            if envelope["message"]["role"] != "user"
                || marker["success"] != true
                || !gated.contains(text(marker, "name"))
            {
                return false;
            }
            let Some(invocation) = marker["invocation_id"]
                .as_str()
                .and_then(|id| s.get("tool_invocations", id))
            else {
                return false;
            };
            invocation.data["name"] == "Skill"
                && invocation.data["state"] == "completed"
                && invocation.data["input"]["name"] == marker["name"]
                && invocation.data["run_id"] == envelope["run_id"]
                && s.get("agent_runs", text(&invocation.data, "run_id"))
                    .is_some_and(|owner| {
                        if row.header.collection == "conversation_activities" {
                            owner.data["conversation_id"] == row.data["conversation_id"]
                        } else {
                            owner.header.id == text(&row.data["metadata"], "run_id")
                        }
                    })
        })
}

/// Current direct grants and Skill-derived grants share exact per-tool denials.
/// Direct visibility cannot bypass a visible Skill's server gate.
pub fn mcp_tool_allowed(
    s: &Snapshot,
    c: &Context,
    server_id: &str,
    raw_name: &str,
    full_name: &str,
    reader: &dyn Fn(&Value) -> Result<Vec<u8>>,
) -> bool {
    let Ok(run) = current_run(s, c) else {
        return false;
    };
    let Some(server) = s
        .get("mcp_servers", server_id)
        .filter(|r| r.data["enabled"] == true)
    else {
        return false;
    };
    let agent = text(&run.data, "agent_id");
    if s.get("agents", agent)
        .is_none_or(|r| r.data["enabled"] != true)
        || mcp_exact_denied(s, agent, server_id, raw_name, full_name)
    {
        return false;
    }
    let server_name = text(&server.data, "name");
    if visible_skills(s, c)
        .iter()
        .any(|skill| skill_servers(skill).any(|name| name == server_name))
    {
        return mcp_skill_allowed(s, c, server_name, reader);
    }
    mcp_tool_visible(s, agent, server_id, raw_name, full_name)
}

#[cfg(test)]
mod deferred_visibility_tests {
    use super::*;
    fn snapshot(rows: &[(&str, &str, bool)]) -> Snapshot {
        let mut s = Snapshot {
            workspace_id: "workspace_test".into(),
            commit_seq: 0,
            collections: Default::default(),
        };
        for (id, name, enabled) in rows {
            s.collections.entry("agent_visibility".into()).or_default().insert((*id).into(), Record { header: artex_core::Header { id:(*id).into(),collection:"agent_visibility".into(),version:"1".into(),commit_seq:"1".into(),created_at:"2026-10-12T00:00:00Z".into(),updated_at:"2026-10-12T00:00:00Z".into(),legacy_key:Value::Null }, data:json!({"agent_id":"agent_test","resource_kind":"mcp","resource_id":"mcp_test","mcp_tool_name":name,"enabled":enabled}) });
        }
        s
    }
    #[test]
    fn exact_mcp_visibility_excludes_siblings_and_other_agents() {
        let s = snapshot(&[("selected", "inspect", true)]);
        assert!(mcp_tool_visible(
            &s,
            "agent_test",
            "mcp_test",
            "inspect",
            "mcp__fixture__inspect"
        ));
        assert!(!mcp_tool_visible(
            &s,
            "agent_test",
            "mcp_test",
            "write",
            "mcp__fixture__write"
        ));
        assert!(!mcp_tool_visible(
            &s,
            "agent_other",
            "mcp_test",
            "inspect",
            "mcp__fixture__inspect"
        ));
    }
    #[test]
    fn exact_mcp_denial_overrides_wildcard_and_full_name_alias() {
        let s = snapshot(&[("all", "", true), ("deny", "mcp__fixture__write", false)]);
        assert!(mcp_tool_visible(
            &s,
            "agent_test",
            "mcp_test",
            "inspect",
            "mcp__fixture__inspect"
        ));
        assert!(!mcp_tool_visible(
            &s,
            "agent_test",
            "mcp_test",
            "write",
            "mcp__fixture__write"
        ));
    }
}

#[cfg(test)]
mod skill_gate_tests {
    use super::*;
    fn put(s: &mut Snapshot, collection: &str, id: &str, data: Value) {
        s.collections.entry(collection.into()).or_default().insert(
            id.into(),
            Record {
                header: artex_core::Header {
                    id: id.into(),
                    collection: collection.into(),
                    version: "1".into(),
                    commit_seq: "1".into(),
                    created_at: "2026-10-12T00:00:00Z".into(),
                    updated_at: "2026-10-12T00:00:00Z".into(),
                    legacy_key: Value::Null,
                },
                data,
            },
        );
    }
    fn fixture(direct: bool) -> (Snapshot, Context) {
        let mut s = Snapshot {
            workspace_id: "workspace_test".into(),
            commit_seq: 1,
            collections: Default::default(),
        };
        put(
            &mut s,
            "agents",
            "agent_test",
            json!({"key":"worker","enabled":true}),
        );
        put(
            &mut s,
            "agent_runs",
            "run_test",
            json!({"agent_id":"agent_test","engine_epoch":"epoch_test","state":"running","conversation_id":null,"parent_run_id":null,"context_snapshot":{"skill_catalog":[{"name":"inspect","description":"Inspect a target","mcps":["fixture"]},{"name":"hidden","mcps":["other"]}]}}),
        );
        put(
            &mut s,
            "agent_skill_visibility",
            "visible",
            json!({"agent_id":"agent_test","skill_name":"inspect","enabled":true}),
        );
        put(
            &mut s,
            "mcp_servers",
            "mcp_test",
            json!({"name":"fixture","enabled":true}),
        );
        put(
            &mut s,
            "mcp_tools_cache",
            "cache",
            json!({"server_id":"mcp_test","tool_name":"read","schema":{"type":"object"}}),
        );
        if direct {
            put(
                &mut s,
                "agent_visibility",
                "direct",
                json!({"agent_id":"agent_test","resource_kind":"mcp","resource_id":"mcp_test","mcp_tool_name":"","enabled":true}),
            );
        }
        let c = Context {
            actor: artex_core::Actor {
                kind: "agent".into(),
                agent_id: Some("agent_test".into()),
                run_id: Some("run_test".into()),
                engine_epoch: "epoch_test".into(),
            },
            task_id: None,
            intent_id: None,
            conversation_id: None,
        };
        (s, c)
    }
    fn guidance(s: &mut Snapshot, run: &str, conversation: Option<&str>) -> Value {
        put(
            s,
            "tool_invocations",
            "invocation_skill",
            json!({"name":"Skill","state":"completed","run_id":run,"input":{"name":"inspect"}}),
        );
        let envelope = json!({"run_id":run,"skill_invocation":{"name":"inspect","success":true,"invocation_id":"invocation_skill"},"message":{"role":"user","content":[{"type":"text","text":"standing instructions"}]}});
        let data = json!({"is_error":false,"kind":"user","detail":artex_core::body(&envelope.to_string()),"metadata":{"run_id":run},"conversation_id":conversation});
        put(
            s,
            if conversation.is_some() {
                "conversation_activities"
            } else {
                "activity"
            },
            "guidance",
            data,
        );
        envelope
    }
    fn allowed(s: &Snapshot, c: &Context) -> bool {
        mcp_tool_allowed(s, c, "mcp_test", "read", "mcp__fixture__read", &|_| {
            Err(Error::new("NOT_FOUND", "no blob"))
        })
    }
    #[test]
    fn actual_catalog_intersects_current_agent_visibility_and_epoch() {
        let (mut s, c) = fixture(false);
        assert_eq!(visible_skills(&s, &c).len(), 1);
        assert_eq!(visible_skills(&s, &c)[0]["name"], "inspect");
        let mut stale = c.clone();
        stale.actor.engine_epoch = "old".into();
        assert!(visible_skills(&s, &stale).is_empty());
        s.collections
            .get_mut("agent_skill_visibility")
            .unwrap()
            .get_mut("visible")
            .unwrap()
            .data["enabled"] = json!(false);
        assert!(visible_skills(&s, &c).is_empty());
    }
    #[test]
    fn direct_visible_skill_server_is_locked_until_confirmed_independent_guidance() {
        let (mut s, c) = fixture(true);
        assert!(!allowed(&s, &c));
        assert!(
            query("SearchExtraTools", &json!({"query":""}), &s, &c)
                .unwrap()
                .as_array()
                .unwrap()
                .is_empty()
        );
        guidance(&mut s, "run_test", None);
        assert!(allowed(&s, &c));
        assert_eq!(
            query("SearchExtraTools", &json!({"query":""}), &s, &c).unwrap()[0]["name"],
            "mcp__fixture__read"
        );
    }
    #[test]
    fn skill_only_server_unlocks_but_exact_denial_and_revocation_win() {
        let (mut s, c) = fixture(false);
        guidance(&mut s, "run_test", None);
        assert!(allowed(&s, &c));
        put(
            &mut s,
            "agent_visibility",
            "deny",
            json!({"agent_id":"agent_test","resource_kind":"mcp","resource_id":"mcp_test","mcp_tool_name":"mcp__fixture__read","enabled":false}),
        );
        assert!(!allowed(&s, &c));
        s.collections.get_mut("agent_visibility").unwrap().clear();
        s.collections
            .get_mut("agent_skill_visibility")
            .unwrap()
            .get_mut("visible")
            .unwrap()
            .data["enabled"] = json!(false);
        assert!(!allowed(&s, &c));
    }
    #[test]
    fn failed_receipt_nonuser_guidance_and_unrelated_history_never_unlock() {
        let (mut s, c) = fixture(false);
        let envelope = guidance(&mut s, "run_test", None);
        s.collections
            .get_mut("tool_invocations")
            .unwrap()
            .get_mut("invocation_skill")
            .unwrap()
            .data["state"] = json!("failed");
        assert!(!allowed(&s, &c));
        s.collections
            .get_mut("tool_invocations")
            .unwrap()
            .get_mut("invocation_skill")
            .unwrap()
            .data["state"] = json!("completed");
        let mut wrong = envelope;
        wrong["message"]["role"] = json!("assistant");
        s.collections
            .get_mut("activity")
            .unwrap()
            .get_mut("guidance")
            .unwrap()
            .data["detail"] = artex_core::body(&wrong.to_string());
        assert!(!allowed(&s, &c));
        guidance(&mut s, "run_other", None);
        assert!(!allowed(&s, &c));
    }
    #[test]
    fn authorized_parent_conversation_replays_but_other_conversation_does_not() {
        let (mut s, c) = fixture(false);
        put(
            &mut s,
            "agent_runs",
            "run_parent",
            json!({"agent_id":"agent_test","conversation_id":"conversation_parent","state":"completed"}),
        );
        s.collections
            .get_mut("agent_runs")
            .unwrap()
            .get_mut("run_test")
            .unwrap()
            .data["parent_run_id"] = json!("run_parent");
        guidance(&mut s, "run_parent", Some("conversation_parent"));
        assert!(allowed(&s, &c));
        s.collections
            .get_mut("conversation_activities")
            .unwrap()
            .get_mut("guidance")
            .unwrap()
            .data["conversation_id"] = json!("conversation_other");
        assert!(!allowed(&s, &c));
    }
    #[test]
    fn blob_guidance_uses_real_reader_and_inline_fallback_fails_closed() {
        let (mut s, c) = fixture(false);
        let envelope = guidance(&mut s, "run_test", None);
        let reference = json!({"sha256":"fixture","length":"100","media_type":"application/json"});
        s.collections
            .get_mut("activity")
            .unwrap()
            .get_mut("guidance")
            .unwrap()
            .data["detail"] = json!({"text":null,"blob":reference});
        assert!(!allowed(&s, &c));
        let reader = |r: &Value| {
            assert_eq!(r, &reference);
            Ok(envelope.to_string().into_bytes())
        };
        assert!(mcp_tool_allowed(
            &s,
            &c,
            "mcp_test",
            "read",
            "mcp__fixture__read",
            &reader
        ));
        assert_eq!(
            query_with_blob_reader("SearchExtraTools", &json!({"query":""}), &s, &c, &reader)
                .unwrap()
                .as_array()
                .unwrap()
                .len(),
            1
        );
        assert!(
            query_with_blob_reader("SearchExtraTools", &json!({"query":""}), &s, &c, &|_| Ok(
                vec![255]
            ))
            .unwrap()
            .as_array()
            .unwrap()
            .is_empty()
        );
    }
    #[test]
    fn nongated_direct_tools_keep_original_policy_and_disabled_server_never_grants() {
        let (mut s, c) = fixture(true);
        s.collections
            .get_mut("agent_runs")
            .unwrap()
            .get_mut("run_test")
            .unwrap()
            .data["context_snapshot"]["skill_catalog"] = json!([]);
        assert!(allowed(&s, &c));
        s.collections
            .get_mut("mcp_servers")
            .unwrap()
            .get_mut("mcp_test")
            .unwrap()
            .data["enabled"] = json!(false);
        assert!(!allowed(&s, &c));
    }
    #[test]
    fn source_scalar_mcps_and_legacy_mcp_alias_have_the_same_gate() {
        for field in ["mcps", "mcp"] {
            let (mut s, c) = fixture(true);
            let mut skill = json!({"name":"inspect","description":"Inspect"});
            skill[field] = json!("fixture");
            s.collections
                .get_mut("agent_runs")
                .unwrap()
                .get_mut("run_test")
                .unwrap()
                .data["context_snapshot"]["skill_catalog"] = json!([skill]);
            assert!(!allowed(&s, &c));
            guidance(&mut s, "run_test", None);
            assert!(allowed(&s, &c));
        }
    }
    #[test]
    fn different_skill_receipt_cannot_unlock_claimed_directory() {
        let (mut s, c) = fixture(false);
        guidance(&mut s, "run_test", None);
        assert!(allowed(&s, &c));
        s.collections
            .get_mut("tool_invocations")
            .unwrap()
            .get_mut("invocation_skill")
            .unwrap()
            .data["input"]["name"] = json!("other-skill");
        assert!(!allowed(&s, &c));
        s.collections
            .get_mut("tool_invocations")
            .unwrap()
            .get_mut("invocation_skill")
            .unwrap()
            .data["input"] = json!({});
        assert!(!allowed(&s, &c));
    }
}

pub fn current_run<'a>(s: &'a Snapshot, c: &Context) -> Result<&'a Record> {
    if c.actor.kind != "agent" {
        return Err(Error::new(
            "PERMISSION_DENIED",
            "tool invocation requires an agent actor",
        ));
    }
    let run = c
        .actor
        .run_id
        .as_deref()
        .ok_or_else(|| Error::new("INVALID_PARAMS", "run context required"))?;
    let r = record(s, "agent_runs", run)?;
    if r.data["agent_id"].as_str() != c.actor.agent_id.as_deref()
        || text(&r.data, "engine_epoch") != c.actor.engine_epoch
        || !matches!(text(&r.data, "state"), "running" | "waiting")
    {
        return Err(Error::new(
            "STALE_EPOCH",
            "run owner, epoch or state is stale",
        ));
    }
    Ok(r)
}
pub fn plan(m: &str, p: &Value, s: &Snapshot, c: &Context) -> Result<CommandBatch> {
    let mut b = Planner::new(s, m, p);
    match m {
        "agent.features.update" => {
            if c.actor.kind != "user" && c.actor.kind != "engine" {
                return Err(Error::new(
                    "PERMISSION_DENIED",
                    "feature settings require user",
                ));
            }
            let agent = required(p, "agent_id")?;
            record(s, "agents", agent)?;
            let old = s
                .records("agent_features")
                .into_iter()
                .find(|r| text(&r.data, "agent_id") == agent);
            let mut d = old.map(|r| r.data.clone()).unwrap_or(
                json!({"agent_id":agent,"questions":false,"plan_mode":false,"memory":false}),
            );
            for k in ["questions", "plan_mode", "memory"] {
                if let Some(v) = p.get(k) {
                    if !v.is_boolean() {
                        return Err(Error::new(
                            "INVALID_PARAMS",
                            "feature flags must be boolean",
                        ));
                    }
                    d[k] = v.clone();
                }
            }
            let id = old
                .map(|r| r.header.id.clone())
                .unwrap_or_else(|| new_id("agentfeature"));
            b.put("agent_features", &id, d);
            Ok(b.done(json!({"agent_id":agent})))
        }
        "RecordMemory" => {
            let run = current_run(s, c)?;
            let agent = required(&run.data, "agent_id")?;
            let name = required(p, "name")?.trim();
            let content = required(p, "content")?;
            let kind = if text(p, "type").is_empty() {
                "project"
            } else {
                text(p, "type")
            };
            if !["user", "feedback", "project", "reference"].contains(&kind) {
                return Err(Error::new("INVALID_PARAMS", "invalid memory category"));
            }
            let old = s.records("memory_records").into_iter().find(|r| {
                text(&r.data, "agent_id") == agent
                    && text(&r.data, "name") == name
                    && text(&r.data, "type") == kind
            });
            let id = old
                .map(|r| r.header.id.clone())
                .unwrap_or_else(|| new_id("memory"));
            let mut d = old
                .map(|r| r.data.clone())
                .unwrap_or(default_data("memory_records")?);
            d["id"] = json!(id);
            d["agent_id"] = json!(agent);
            d["name"] = json!(name);
            d["type"] = json!(kind);
            d["description"] = json!(text(p, "description"));
            d["content"] = artex_core::body(content);
            d["version"] = json!(
                (old.map(|r| artex_core::parse_u64(text(&r.data, "version")))
                    .transpose()?
                    .unwrap_or(0)
                    + 1)
                .to_string()
            );
            b.put("memory_records", &id, d);
            Ok(b.done(json!({"id":id,"name":name})))
        }
        "TodoWrite" => {
            let run = current_run(s, c)?;
            let todos = p["todos"]
                .as_array()
                .ok_or_else(|| Error::new("INVALID_PARAMS", "todos array required"))?;
            let mut active = 0;
            for t in todos {
                required(t, "content")?;
                match required(t, "status")? {
                    "pending" | "completed" => {}
                    "in_progress" => active += 1,
                    _ => return Err(Error::new("INVALID_PARAMS", "invalid todo state")),
                }
            }
            if active > 1 {
                return Err(Error::new(
                    "INVALID_PARAMS",
                    "only one todo may be in progress",
                ));
            }
            let mut d = run.data.clone();
            d["todos"] = json!(todos);
            b.put("agent_runs", &run.header.id, d);
            Ok(b.done(json!({"todos":todos})))
        }
        "EnterPlanMode" => {
            let run = current_run(s, c)?;
            let mut d = run.data.clone();
            if text(&d, "permission_mode") == "plan" {
                return Err(Error::new("INVALID_PARAMS", "already in plan mode"));
            }
            d["pre_plan_mode"] = d["permission_mode"].clone();
            d["permission_mode"] = json!("plan");
            b.put("agent_runs", &run.header.id, d);
            Ok(b.done(json!({"permission_mode":"plan"})))
        }
        "AskUserQuestion" | "ExitPlanMode" => {
            let run = current_run(s, c)?;
            if m == "ExitPlanMode" && text(&run.data, "permission_mode") != "plan" {
                return Err(Error::new("INVALID_PARAMS", "not in plan mode"));
            }
            if s.records("user_questions").iter().any(|r| {
                text(&r.data, "run_id") == run.header.id && text(&r.data, "state") == "pending"
            }) {
                return Err(Error::new(
                    "VERSION_CONFLICT",
                    "run already has pending question",
                ));
            }
            if m == "AskUserQuestion" {
                let qs = p["questions"]
                    .as_array()
                    .ok_or_else(|| Error::new("INVALID_PARAMS", "questions array required"))?;
                if qs.is_empty() || qs.len() > 4 {
                    return Err(Error::new("INVALID_PARAMS", "1..4 questions required"));
                }
            }
            let generation = s
                .records("user_questions")
                .iter()
                .filter(|r| text(&r.data, "run_id") == run.header.id)
                .filter_map(|r| text(&r.data, "generation").parse::<u64>().ok())
                .max()
                .unwrap_or(0)
                + 1;
            let id=b.insert("user_questions","question",json!({"run_id":run.header.id,"kind":if m=="ExitPlanMode"{"plan_approval"}else{"question"},"payload":p,"state":"pending","answer":null,"deadline_at":(Utc::now()+Duration::hours(24)).to_rfc3339_opts(chrono::SecondsFormat::Millis,true),"generation":generation.to_string()}))?;
            let mut d = run.data.clone();
            d["state"] = json!("waiting");
            b.put("agent_runs", &run.header.id, d);
            Ok(b.done(
                json!({"question_id":id,"state":"waiting","generation":generation.to_string()}),
            ))
        }
        "question.answer" => {
            if c.actor.kind != "user" {
                return Err(Error::new(
                    "PERMISSION_DENIED",
                    "only user may answer or approve plan",
                ));
            }
            let id = required(p, "question_id")?;
            let q = record(s, "user_questions", id)?;
            if text(&q.data, "state") != "pending"
                || text(&q.data, "generation") != required(p, "generation")?
            {
                return Err(Error::new(
                    "STALE_EPOCH",
                    "question already closed or generation stale",
                ));
            }
            let run = record(s, "agent_runs", text(&q.data, "run_id"))?;
            if text(&run.data, "state") != "waiting"
                || text(&run.data, "engine_epoch") != c.actor.engine_epoch
            {
                return Err(Error::new("STALE_EPOCH", "waiting run changed"));
            }
            let mut d = q.data.clone();
            d["state"] = json!("answered");
            d["answer"] = p["answer"].clone();
            let mut rd = run.data.clone();
            if text(&d, "kind") == "plan_approval" {
                if p["answer"]["approved"] == true {
                    rd["permission_mode"] = rd["pre_plan_mode"].clone();
                    rd["approved_target_mode"] = rd["permission_mode"].clone();
                    rd["pre_plan_mode"] = Value::Null;
                }
            }
            rd["state"] = json!("running");
            b.put("user_questions", id, d);
            b.put("agent_runs", &run.header.id, rd);
            Ok(b.done(json!({"question_id":id,"state":"answered"})))
        }
        _ => Err(Error::new("METHOD_NOT_FOUND", m)),
    }
}
pub fn query(m: &str, p: &Value, s: &Snapshot, c: &Context) -> Result<Value> {
    query_with_blob_reader(m, p, s, c, &|_| {
        Err(Error::new("NOT_FOUND", "Blob reader unavailable"))
    })
}

pub fn query_with_blob_reader(
    m: &str,
    p: &Value,
    s: &Snapshot,
    c: &Context,
    reader: &dyn Fn(&Value) -> Result<Vec<u8>>,
) -> Result<Value> {
    match m {
        "RecallMemory" => {
            let run = current_run(s, c)?;
            let agent = text(&run.data, "agent_id");
            let q = text(p, "query").to_lowercase();
            Ok(json!(
                s.records("memory_records")
                    .iter()
                    .filter(|r| text(&r.data, "agent_id") == agent
                        && r.data.to_string().to_lowercase().contains(&q))
                    .map(|r| r.data.clone())
                    .collect::<Vec<_>>()
            ))
        }
        "SearchExtraTools" => {
            let run = current_run(s, c)?;
            let agent = record(s, "agents", text(&run.data, "agent_id"))?;
            let key = text(&agent.data, "key");
            let query = text(p, "query").trim();
            let mut candidates=s.records("tools").iter().filter(|r|r.data["enabled"]==true&&r.data["deferred"]==true&&ids(&r.data,"agents").iter().any(|a|a==key)).map(|r|json!({"name":r.data["key"],"description":r.data["description"],"schema":r.data["schema"]})).collect::<Vec<_>>();
            for cached in s.records("mcp_tools_cache") {
                let sid = text(&cached.data, "server_id");
                let Some(server) = s
                    .get("mcp_servers", sid)
                    .filter(|r| r.data["enabled"] == true)
                else {
                    continue;
                };
                let cached_name = text(&cached.data, "tool_name");
                let name = if cached_name.starts_with("mcp__") {
                    cached_name.into()
                } else {
                    format!("mcp__{}__{cached_name}", text(&server.data, "name"))
                };
                let raw_name = cached_name
                    .strip_prefix(&format!("mcp__{}__", text(&server.data, "name")))
                    .unwrap_or(cached_name);
                if !mcp_tool_allowed(s, c, sid, raw_name, &name, reader) {
                    continue;
                }
                if cached.data["schema"].is_null() {
                    continue;
                } // Never invent an MCP parameter schema.
                candidates.push(json!({"name":name,"description":cached.data["description"].as_str().unwrap_or(""),"schema":cached.data["schema"]}));
            }
            candidates.sort_by(|a, b| text(a, "name").cmp(text(b, "name")));
            let mut found = vec![];
            let mut seen = BTreeSet::new();
            if let Some(select) = query.strip_prefix("select:") {
                for token in select.split(',').map(str::trim).filter(|v| !v.is_empty()) {
                    let exact = candidates.iter().find(|c| text(c, "name") == token);
                    if let Some(c) = exact {
                        if seen.insert(text(c, "name").to_owned()) {
                            found.push(c.clone());
                        }
                        continue;
                    }
                    let lower = token.to_lowercase();
                    for c in &candidates {
                        if text(c, "name").to_lowercase().contains(&lower)
                            && seen.insert(text(c, "name").to_owned())
                        {
                            found.push(c.clone());
                        }
                    }
                }
            } else {
                let tokens = query
                    .to_lowercase()
                    .split_whitespace()
                    .map(str::to_owned)
                    .collect::<Vec<_>>();
                for c in candidates {
                    let haystack = format!("{} {}", text(&c, "name"), text(&c, "description"))
                        .to_lowercase()
                        .replace("__", " ");
                    if tokens.iter().all(|t| haystack.contains(t)) {
                        found.push(c);
                    }
                }
            }
            Ok(json!(found))
        }
        _ => Err(Error::new("METHOD_NOT_FOUND", m)),
    }
}
