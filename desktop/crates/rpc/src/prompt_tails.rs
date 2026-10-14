//! Runtime tails owned by code, appended after the editable agent template.
//! Source: agent/{worker,chat,constraints,goals,finding_workflow,deferred}.go.
use artex_core::{Context, Error, Record, Result, Snapshot};
use serde_json::{Value, json};
use std::collections::BTreeSet;

fn has(tools: &[Value], name: &str) -> bool {
    tools.iter().any(|tool| tool["name"] == name)
}

/// `work_dir` is the actual native execution CWD, already validated by the caller.
/// This function performs no file or network access and never grants a tool.
#[cfg(test)]
pub(crate) fn append_code_owned(
    system: &mut String,
    s: &Snapshot,
    run: &Record,
    context: &Context,
    tools: &[Value],
    work_dir: &str,
    traffic_enabled: bool,
) -> Result<()> {
    append_code_owned_with_reader(
        system,
        s,
        run,
        context,
        tools,
        work_dir,
        traffic_enabled,
        &|_| Err(Error::new("NOT_FOUND", "Blob reader is not available")),
    )
}

pub(crate) fn append_code_owned_with_reader(
    system: &mut String,
    s: &Snapshot,
    run: &Record,
    context: &Context,
    tools: &[Value],
    work_dir: &str,
    traffic_enabled: bool,
    reader: &dyn Fn(&Value) -> Result<Vec<u8>>,
) -> Result<()> {
    if work_dir.trim().is_empty() || work_dir.contains(['\r', '\n', '\0']) {
        return Err(Error::new(
            "INVALID_PARAMS",
            "Runtime working directory is invalid",
        ));
    }
    let agent = run.data["agent_id"]
        .as_str()
        .and_then(|id| s.get("agents", id))
        .ok_or_else(|| Error::new("NOT_FOUND", "Runtime agent does not exist"))?;
    let role = agent.data["key"].as_str().unwrap_or("");
    system.push_str("\n\n<code-owned-runtime-guidance>\nThe following runtime rules are supplied by the application and remain applicable regardless of edits to the agent template.");
    if role == "worker" {
        system.push_str(&format!("\n\n**Intermediate artifact output protocol**: Write scripts, payloads, captured response bodies and temporary data only in the task-scoped working directory: {work_dir}. This is the actual default CWD; use relative paths for file tools. Do not write to /tmp or other absolute locations. Workers share the task directory: use this run's ID {} or assigned intent ID in unique relative file names to avoid collisions, and do not overwrite another worker's artifacts.", run.header.id));
        system.push_str("\n\n**Assigned intent and situational awareness**: Perform only the assigned intent, produce facts, and stop when it is complete. Any global graph overview, other workers' discoveries and historical coverage are read-only context for avoiding duplication and understanding relationships; they do not assign another job or authorize chasing uncovered areas. Do not execute another worker's intent or expand your own scope. Think deeply about relationships, but hand valuable cross-asset or exploit-chain clues back to the planner rather than acting on them.");
        if has(tools, "record_fact") {
            system.push_str(" Preserve these useful clues as facts using record_fact, with the assigned intent identity.");
        }
    } else if matches!(role, "planner" | "mainagent") {
        system.push_str(&format!("\n\n**Intermediate artifact output protocol**: Write scripts, payloads, captured response bodies and temporary data only in this task's working directory: {work_dir}. This is the actual default CWD; use relative paths for file tools. Do not write to /tmp or other absolute locations."));
    } else {
        system.push_str(&format!("\n\n**File output convention**: Write files only in the working directory {work_dir}. This is the actual default CWD; use relative paths for file tools. Do not write to /tmp or other absolute locations."));
    }
    // Source workerTrafficBlock follows capture, not the separate global proxy.
    if role == "worker" && traffic_enabled {
        let names: Vec<&str> = ["traffic_search", "traffic_get", "traffic_blob"]
            .into_iter()
            .filter(|name| has(tools, name))
            .collect();
        if !names.is_empty() {
            system.push_str(&format!("\n\n**Traffic tools**: {} are available. Review captured traffic before repeating a request to the same URL; do not repeat curl merely to retrieve an existing response.", names.join(" / ")));
            if has(tools, "traffic_search") {
                system.push_str(" traffic_search requires host. Its default limit is 3 lightweight index rows (id/method/url/status/resp_len), without response bodies; increase limit explicitly when needed. body_contains searches captured request and response bodies using a substring of at least 3 characters, including Korean or English text.");
            }
            if has(tools, "traffic_get") {
                system.push_str(" Read a selected exchange with traffic_get(id). Large body text may be returned as @blob sha256:<hash>.");
            }
            if has(tools, "traffic_blob") {
                system.push_str(" Use traffic_blob(hash) to read that complete body in sections.");
            }
        }
    }
    if matches!(role, "planner" | "worker")
        && artex_domain::setting_bool(s, &format!("constraints_inject_{role}"), true)
    {
        append_constraints(system, s, context)?;
    }
    if role == "goals" && context.task_id.is_some() && has(tools, "add_task_scope") {
        system.push_str("\n\n**Register the explicit test asset scope**: Extract only assets explicitly authorized in the user's goal or task description and register them with add_task_scope. This defines the task's authorization boundary and coverage denominator. Follow minimum scope: never infer unmentioned hosts/IPs or broaden the user's target. For a URL or a hostname with subdomains, register the complete hostname as kind=subdomain (https://a1b2c3.lab.example.net/path means a1b2c3.lab.example.net, never example.net). Use kind=root_domain only for an explicitly supplied bare root domain or explicit authorization for all subdomains/the entire domain. Use kind=ip or cidr for an explicit IP or network. Do not register company scope during initial goal decomposition; company scope belongs to the subsequent planning stage. Include a reason citing the user's statement. If no explicit asset range exists, do not call add_task_scope.");
        if has(tools, "set_goals") {
            system.push_str(" Register the explicit scope first, then submit the decomposed objectives using set_goals.");
        }
    }
    append_findings(
        system,
        tools,
        artex_domain::setting_bool(s, "agent_traffic_binding", false),
    );
    system.push_str("\n</code-owned-runtime-guidance>");
    append_deferred(system, s, context, tools, reader)?;
    Ok(())
}

fn append_constraints(system: &mut String, s: &Snapshot, context: &Context) -> Result<()> {
    let Some(task) = context.task_id.as_deref().and_then(|id| s.get("tasks", id)) else {
        return Ok(());
    };
    let Some(exploration) = task.data["exploration_id"].as_str() else {
        return Ok(());
    };
    let mut allowed = Vec::new();
    let mut denied = Vec::new();
    for row in s
        .records("task_constraints")
        .into_iter()
        .filter(|row| row.data["exploration_id"] == exploration)
    {
        let text = row.data["text"]
            .as_str()
            .ok_or_else(|| {
                Error::new(
                    "INVALID_PARAMS",
                    "Constraint text cannot be omitted from the runtime prompt",
                )
            })?
            .trim();
        if text.is_empty() {
            continue;
        }
        if row.data["kind"] == "allow" {
            allowed.push(text)
        } else {
            denied.push(text)
        }
    }
    if allowed.is_empty() && denied.is_empty() {
        return Ok(());
    }
    system.push_str("\n\n**Operational constraints — highest task priority**: These authorization constraints take precedence over exploration and expansion heuristics. Before creating each intent and before performing each action, check these boundaries and do not proceed if an action violates them.");
    for (label, rows) in [
        ("Allowed operations", allowed),
        ("Prohibited operations", denied),
    ] {
        if !rows.is_empty() {
            system.push_str(&format!("\n{label}:"));
            for row in rows {
                system.push_str(&format!("\n- {row}"));
            }
        }
    }
    system.push_str("\nA newly discovered goal, port or host does not grant authorization. Unless it falls within the allowed boundary, record it only as an out-of-scope fact and skip it; do not derive an intent or action from it.");
    Ok(())
}

fn append_findings(system: &mut String, tools: &[Value], binding: bool) {
    if binding
        && ["report_finding", "add_hint", "add_task_hint"]
            .iter()
            .any(|name| has(tools, name))
    {
        system.push_str("\n\n**Optional traffic evidence handover**: Automatic traffic binding is performed by the reporting agent after a finding is stored and before its report is written. Preserve verification commands, key output, existing real traffic IDs and their purposes, and the relevant intent_id in the finding's evidence. Do not issue additional probes solely to collect packets for binding. When reporting on another executor's behalf, preserve the executor's verified references. Never invent a traffic ID; unverified candidates are not evidence.");
        if has(tools, "add_hint") || has(tools, "add_task_hint") {
            let names: Vec<&str> = ["add_hint", "add_task_hint"]
                .into_iter()
                .filter(|name| has(tools, name))
                .collect();
            system.push_str(&format!(" Use {} with traffic_refs for handover: preserve verified IDs, role, note and replay order for the specific finding; batch hints must retain their corresponding references.", names.join(" / ")));
        }
        if has(tools, "report_finding") {
            system.push_str(" report_finding also supports explicit traffic_refs or evidence_hint_id from a task-local verified hint. Any invalid reference causes the whole report call to fail. Findings with TCP-only evidence or no captured packet can be registered normally without these optional fields.");
        }
        if has(tools, "add_task_hint") && !has(tools, "add_hint") {
            system.push_str(" In a platform conversation without task context, do not report a task finding directly. Hand evidence to the appropriate existing task using add_task_hint for its task agent to register.");
            if has(tools, "list_task_findings") {
                system.push_str(" Check the resulting finding using list_task_findings.");
            }
        }
        if has(tools, "prove_goal") || has(tools, "goal_met") {
            system.push_str(" Complete existing evidence registration/handover before declaring the goal complete. Do not finish the task or cancel a worker merely because a textual finding was registered while evidence handover remains pending. No-packet evidence does not require waiting for or forcing packet capture.");
        }
    }
    if binding
        && [
            "update_finding_report",
            "bind_finding_traffic",
            "get_finding_traffic",
        ]
        .iter()
        .all(|name| has(tools, name))
    {
        system.push_str("\n\n**Automatic traffic correlation before reporting is enabled**: Operate only on the finding that triggered this report. Obtain its explicit finding_id and finding_node_id from the tool result; read finding details, the corresponding intent execution record and existing evidence. Prefer verified real IDs handed over by the executor. Do not recreate a finding or probe the target again.");
        if has(tools, "traffic_search") && has(tools, "traffic_get") {
            system.push_str(" For HTTP evidence, use traffic_search to locate candidates and traffic_get to verify each request/response supports this specific finding. Hostname and time are filters, not proof of ownership.");
        }
        system.push_str(" Bind confirmed exchanges using bind_finding_traffic(finding_id, traffic_refs) in replay order with baseline/proof/verification/supporting roles and explanatory notes. Do not duplicate existing bindings. After a successful binding, call get_finding_traffic again and pass the evidence_version actually read to update_finding_report. Use that tool's finding_node_id parameter for the exploration node identity. For TCP-only evidence, capture disabled, unavailable traffic tools or no exact match, skip binding and explain why in the normal textual/command evidence report. Do not guess references, declare failed binding successful or discard existing evidence.");
    }
    if [
        "report_finding",
        "add_hint",
        "add_task_hint",
        "get_finding_traffic",
        "update_finding_report",
    ]
    .iter()
    .any(|name| has(tools, name))
    {
        system.push_str("\n\n**Finding identity convention**: finding_id is an independent finding record ID; finding_node_id identifies its exploration node. In list_findings/list_task_findings/node_detail/get_task_node_detail, id is the exploration node identity; use the explicit finding_id field for get_finding_traffic and bind_finding_traffic. update_finding_report takes finding_node_id in the desktop schema, never the independent finding ID. Do not use an unrelated first result row or guess another ID after an identity error.");
    }
    if has(tools, "update_finding_report") {
        system.push_str("\nEvidence versions are compare-and-set preconditions: use the evidence_version actually read for this finding. Never guess it, silently rebase stale evidence, or overwrite evidence changed by another run.");
        if has(tools, "get_finding_traffic") {
            system.push_str(" Read the current evidence with get_finding_traffic before submitting an evidence_version-dependent report update.");
        }
        if !binding {
            system.push_str(" Automatic traffic binding is disabled. Preserve existing evidence and do not request a new traffic binding while updating the report.");
        }
    }
}

fn append_deferred(
    system: &mut String,
    s: &Snapshot,
    context: &Context,
    tools: &[Value],
    reader: &dyn Fn(&Value) -> Result<Vec<u8>>,
) -> Result<()> {
    if !has(tools, "SearchExtraTools") || !has(tools, "ExecuteExtraTool") {
        return Ok(());
    }
    // Use the same pure resolver as discovery; do not infer names of disabled or
    // invisible resources and do not disclose schemas or unlock a callable tool.
    let available = artex_domain::sdk::query_with_blob_reader(
        "SearchExtraTools",
        &json!({"query":""}),
        s,
        context,
        reader,
    )?;
    let names: BTreeSet<&str> = available
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|item| item["name"].as_str())
        .filter(|name| deferred_name_visible(s, context, name, reader))
        .collect();
    if names.is_empty() {
        return Ok(());
    }
    system.push_str("\n\n<available-deferred-tools>\n");
    for name in names {
        system.push_str(name);
        system.push('\n');
    }
    system.push_str("</available-deferred-tools>\nThe listed tools are deferred, not direct entries in your current tool list. Before using one, discover its schema with SearchExtraTools, then call it through ExecuteExtraTool. Prefer the available core tools for core tasks; use a deferred tool only when the task needs its specific capability. This list does not grant permission or bypass visibility, skill gating or the execution allowlist.");
    Ok(())
}

fn deferred_name_visible(
    s: &Snapshot,
    context: &Context,
    name: &str,
    reader: &dyn Fn(&Value) -> Result<Vec<u8>>,
) -> bool {
    for cached in s.records("mcp_tools_cache") {
        let Some(server_id) = cached.data["server_id"].as_str() else {
            continue;
        };
        let Some(server) = s.get("mcp_servers", server_id) else {
            continue;
        };
        let tool_name = cached.data["tool_name"].as_str().unwrap_or("");
        let full_name = if tool_name.starts_with("mcp__") {
            tool_name.to_string()
        } else {
            format!(
                "mcp__{}__{tool_name}",
                server.data["name"].as_str().unwrap_or("")
            )
        };
        if full_name != name {
            continue;
        }
        if server.data["enabled"] != true || cached.data["schema"].is_null() {
            return false;
        }
        let prefix = format!("mcp__{}__", server.data["name"].as_str().unwrap_or(""));
        let raw_name = tool_name.strip_prefix(&prefix).unwrap_or(tool_name);
        return artex_domain::sdk::mcp_tool_allowed(
            s, context, server_id, raw_name, &full_name, reader,
        );
    }
    // Non-MCP deferred tools already passed the actual tool-binding resolver.
    true
}

#[cfg(test)]
mod tests {
    use super::*;
    use artex_core::{Actor, Header};
    fn record(collection: &str, id: &str, data: Value) -> Record {
        Record {
            header: Header {
                id: id.into(),
                collection: collection.into(),
                version: "1".into(),
                commit_seq: "1".into(),
                created_at: "2026-10-12T00:00:00Z".into(),
                updated_at: "2026-10-12T00:00:00Z".into(),
                legacy_key: Value::Null,
            },
            data,
        }
    }
    fn fixture(role: &str) -> (Snapshot, Record, Context) {
        let mut s = Snapshot {
            workspace_id: "workspace_test".into(),
            commit_seq: 1,
            collections: Default::default(),
        };
        let run = record(
            "agent_runs",
            "run_test",
            json!({"agent_id":"agent_test","task_id":"task_test","engine_epoch":"epoch_test","state":"running"}),
        );
        for item in [
            record("agents", "agent_test", json!({"key":role,"enabled":true})),
            record(
                "tasks",
                "task_test",
                json!({"exploration_id":"exploration_test"}),
            ),
            run.clone(),
        ] {
            s.collections
                .entry(item.header.collection.clone())
                .or_default()
                .insert(item.header.id.clone(), item);
        }
        let c = Context {
            actor: Actor {
                kind: "agent".into(),
                agent_id: Some("agent_test".into()),
                run_id: Some("run_test".into()),
                engine_epoch: "epoch_test".into(),
            },
            task_id: Some("task_test".into()),
            intent_id: None,
            conversation_id: None,
        };
        (s, run, c)
    }
    fn tools(names: &[&str]) -> Vec<Value> {
        names.iter().map(|name| json!({"name":name})).collect()
    }
    #[test]
    fn edited_empty_template_retains_worker_artifacts_and_capture_gated_tools() {
        let (s, r, c) = fixture("worker");
        let mut sys = String::new();
        append_code_owned(
            &mut sys,
            &s,
            &r,
            &c,
            &tools(&[
                "traffic_search",
                "traffic_get",
                "traffic_blob",
                "record_fact",
            ]),
            "work/tasks/task_test",
            true,
        )
        .unwrap();
        assert!(
            sys.contains("task-scoped working directory")
                && sys.contains("Workers share the task directory")
                && !sys.contains("exclusive")
                && sys.contains("body_contains")
                && sys.contains("at least 3")
                && sys.contains("traffic_blob(hash)")
        );
        assert!(sys.contains("do not assign another job") && sys.contains("using record_fact"));
        let mut off = String::new();
        append_code_owned(
            &mut off,
            &s,
            &r,
            &c,
            &tools(&["traffic_search"]),
            "work/tasks/task_test",
            false,
        )
        .unwrap();
        assert!(!off.contains("Traffic tools"));
        let mut absent = String::new();
        append_code_owned(&mut absent, &s, &r, &c, &[], "work/tasks/task_test", true).unwrap();
        assert!(!absent.contains("traffic_search"));
    }
    #[test]
    fn constraints_are_task_local_high_priority_and_empty_rows_do_not_render() {
        let (mut s, r, c) = fixture("planner");
        for item in [
            record(
                "task_constraints",
                "constraint_a",
                json!({"exploration_id":"exploration_test","kind":"allow","text":"Only lab.example.net"}),
            ),
            record(
                "task_constraints",
                "constraint_b",
                json!({"exploration_id":"exploration_test","kind":"deny","text":"No destructive writes"}),
            ),
            record(
                "task_constraints",
                "constraint_other",
                json!({"exploration_id":"exploration_other","kind":"deny","text":"OTHER-TASK"}),
            ),
        ] {
            s.collections
                .entry("task_constraints".into())
                .or_default()
                .insert(item.header.id.clone(), item);
        }
        let mut sys = "Editable instructions".into();
        append_code_owned(&mut sys, &s, &r, &c, &[], "work/tasks/task_test", false).unwrap();
        assert!(
            sys.contains("highest task priority")
                && sys.contains("Only lab.example.net")
                && sys.contains("No destructive writes")
                && !sys.contains("OTHER-TASK")
        );
        s.collections.entry("settings".into()).or_default().insert(
            "setting_constraints".into(),
            record(
                "settings",
                "setting_constraints",
                json!({"key":"constraints_inject_planner","value":"false"}),
            ),
        );
        let mut off = String::new();
        append_code_owned(&mut off, &s, &r, &c, &[], "work/tasks/task_test", false).unwrap();
        assert!(!off.contains("highest task priority"));
    }
    #[test]
    fn goals_scope_tail_requires_the_real_scope_tool() {
        let (s, r, c) = fixture("goals");
        let mut sys = String::new();
        append_code_owned(
            &mut sys,
            &s,
            &r,
            &c,
            &tools(&["set_goals"]),
            "work/tasks/task_test",
            false,
        )
        .unwrap();
        assert!(!sys.contains("Register the explicit"));
        append_code_owned(
            &mut sys,
            &s,
            &r,
            &c,
            &tools(&["set_goals", "add_task_scope"]),
            "work/tasks/task_test",
            false,
        )
        .unwrap();
        assert!(
            sys.contains("a1b2c3.lab.example.net, never example.net")
                && sys.contains("then submit")
        );
    }
    #[test]
    fn finding_guidance_obeys_setting_and_resolved_tool_branches() {
        let (mut s, r, c) = fixture("reporter");
        let toolset = tools(&[
            "update_finding_report",
            "bind_finding_traffic",
            "get_finding_traffic",
            "traffic_search",
            "traffic_get",
        ]);
        let mut off = String::new();
        append_code_owned(&mut off, &s, &r, &c, &toolset, "work/tasks/task_test", true).unwrap();
        assert!(
            !off.contains("Automatic traffic correlation")
                && off.contains("finding_id is an independent")
                && off.contains("evidence_version actually read")
                && off.contains("Automatic traffic binding is disabled")
        );
        s.collections.entry("settings".into()).or_default().insert(
            "setting_binding".into(),
            record(
                "settings",
                "setting_binding",
                json!({"key":"agent_traffic_binding","value":"true"}),
            ),
        );
        let mut on = String::new();
        append_code_owned(&mut on, &s, &r, &c, &toolset, "work/tasks/task_test", true).unwrap();
        assert!(
            on.contains("evidence_version actually read")
                && on.contains("finding_node_id parameter")
                && on.contains("Hostname and time are filters")
        );
        let mut partial = String::new();
        append_code_owned(
            &mut partial,
            &s,
            &r,
            &c,
            &tools(&["add_task_hint", "list_task_findings"]),
            "work/tasks/task_test",
            false,
        )
        .unwrap();
        assert!(
            partial.contains("platform conversation without task context")
                && !partial.contains("Automatic traffic correlation")
        );
    }
    #[test]
    fn conversation_uses_neutral_file_convention() {
        let (s, r, c) = fixture("assistant");
        let mut sys = String::new();
        append_code_owned(
            &mut sys,
            &s,
            &r,
            &c,
            &[],
            "work/conversations/conversation_test",
            false,
        )
        .unwrap();
        assert!(sys.contains("File output convention") && !sys.contains("payload"));
    }
    #[test]
    fn deferred_names_follow_discovery_without_granting_tool_access() {
        let (mut s, r, c) = fixture("mainagent");
        for (id, name, enabled) in [
            ("tool_z", "z_extra", true),
            ("tool_a", "a_extra", true),
            ("tool_hidden", "hidden", false),
        ] {
            let item = record(
                "tools",
                id,
                json!({"key":name,"deferred":true,"enabled":enabled,"agents":["mainagent"],"description":"test","schema":{"type":"object"}}),
            );
            s.collections
                .entry("tools".into())
                .or_default()
                .insert(id.into(), item);
        }
        let mut sys = String::new();
        append_code_owned(
            &mut sys,
            &s,
            &r,
            &c,
            &tools(&["SearchExtraTools", "ExecuteExtraTool"]),
            "work/tasks/task_test",
            false,
        )
        .unwrap();
        assert!(
            sys.contains("<available-deferred-tools>\na_extra\nz_extra\n")
                && !sys.contains("\nhidden\n")
                && sys.contains("does not grant permission")
        );
        let mut missing = String::new();
        append_code_owned(
            &mut missing,
            &s,
            &r,
            &c,
            &tools(&["SearchExtraTools"]),
            "work/tasks/task_test",
            false,
        )
        .unwrap();
        assert!(!missing.contains("available-deferred-tools"));
    }
    #[test]
    fn deferred_mcp_exact_selection_does_not_disclose_sibling_tools() {
        let (mut s, r, c) = fixture("mainagent");
        for item in [
            record(
                "mcp_servers",
                "mcp_test",
                json!({"name":"fixture","enabled":true}),
            ),
            record(
                "mcp_tools_cache",
                "cache_selected",
                json!({"server_id":"mcp_test","tool_name":"selected","schema":{"type":"object"}}),
            ),
            record(
                "mcp_tools_cache",
                "cache_other",
                json!({"server_id":"mcp_test","tool_name":"other","schema":{"type":"object"}}),
            ),
            record(
                "agent_visibility",
                "visibility_selected",
                json!({"agent_id":"agent_test","resource_kind":"mcp","resource_id":"mcp_test","mcp_tool_name":"selected","enabled":true}),
            ),
        ] {
            s.collections
                .entry(item.header.collection.clone())
                .or_default()
                .insert(item.header.id.clone(), item);
        }
        let mut sys = String::new();
        append_code_owned(
            &mut sys,
            &s,
            &r,
            &c,
            &tools(&["SearchExtraTools", "ExecuteExtraTool"]),
            "work/tasks/task_test",
            false,
        )
        .unwrap();
        assert!(
            sys.contains("\nmcp__fixture__selected\n") && !sys.contains("\nmcp__fixture__other\n")
        );
        let deny = record(
            "agent_visibility",
            "visibility_deny",
            json!({"agent_id":"agent_test","resource_kind":"mcp","resource_id":"mcp_test","mcp_tool_name":"selected","enabled":false}),
        );
        s.collections
            .get_mut("agent_visibility")
            .unwrap()
            .remove("visibility_selected");
        s.collections
            .get_mut("agent_visibility")
            .unwrap()
            .insert(deny.header.id.clone(), deny);
        let mut hidden = String::new();
        append_code_owned(
            &mut hidden,
            &s,
            &r,
            &c,
            &tools(&["SearchExtraTools", "ExecuteExtraTool"]),
            "work/tasks/task_test",
            false,
        )
        .unwrap();
        assert!(!hidden.contains("available-deferred-tools"));
    }
}
