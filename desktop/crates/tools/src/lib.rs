#![forbid(unsafe_code)]
//! Frozen tool definitions are metadata. Coverage distinguishes actual executors from pending features.
use artex_core::{CommandBatch, Context, Error, Result, Snapshot, contracts};
use artex_domain::{ids, record, text};
use serde_json::{Value, json};
use std::sync::OnceLock;

#[derive(Debug)]
pub enum ToolDispatch {
    Query(Value),
    Command(CommandBatch),
    HostIntent {
        name: String,
        input: Value,
        context: Context,
    },
}
fn definitions() -> &'static Vec<Value> {
    static DEFS: OnceLock<Vec<Value>> = OnceLock::new();
    DEFS.get_or_init(|| {
        contracts::read("tools.v1.json")["tools"]
            .as_array()
            .expect("tools contract")
            .clone()
    })
}
pub fn definition(name: &str) -> Result<&'static Value> {
    definitions()
        .iter()
        .find(|t| text(t, "name") == name)
        .ok_or_else(|| Error::new("METHOD_NOT_FOUND", "unknown tool"))
}
pub fn list() -> Vec<Value> {
    definitions().clone()
}
/// Go DecorateTool's saved scalar defaults affect arguments, not tool identity,
/// authorization, required fields or the frozen desktop validation contract.
pub fn normalize_scalar_defaults(name: &str, input: &Value, state: &Snapshot) -> Value {
    fn inject(input: &Value, schema: &Value) -> Value {
        let Some(fields) = input.as_object() else {
            return input.clone();
        };
        let mut fields = fields.clone();
        for (key, property) in schema["properties"].as_object().into_iter().flatten() {
            if !matches!(
                property["type"].as_str(),
                Some("string" | "integer" | "number" | "boolean")
            ) {
                continue;
            }
            let Some(default) = property
                .get("default")
                .filter(|v| v.is_string() || v.is_number() || v.is_boolean())
            else {
                continue;
            };
            if fields
                .get(key)
                .is_none_or(|value| value.is_null() || value.as_str() == Some(""))
            {
                fields.insert(key.clone(), default.clone());
            }
        }
        Value::Object(fields)
    }
    let mut normalized = state
        .records("tools")
        .into_iter()
        .find(|t| t.data["key"] == name)
        .map(|tool| inject(input, &tool.data["schema"]))
        .unwrap_or_else(|| input.clone());
    if name == "ExecuteExtraTool" {
        if let Some(tool) = state.records("tools").into_iter().find(|t| {
            t.data["key"] == normalized["tool_name"]
                && t.data["system"] != true
                && t.data["deferred"] == true
        }) {
            if let Some(params) = normalized.get("params") {
                let params = inject(params, &tool.data["schema"]);
                normalized["params"] = params;
            }
        }
    }
    normalized
}
const HOST: &[&str] = &[
    "Read",
    "Write",
    "Edit",
    "MultiEdit",
    "LS",
    "Glob",
    "Grep",
    "Bash",
    "TaskList",
    "TaskOutput",
    "TaskStop",
    "Monitor",
    "shell_open",
    "shell_send",
    "shell_read",
    "shell_close",
    "shell_list",
    "WebFetch",
    "web_search",
    "sleep",
    "Skill",
    "create_skill",
    "update_skill_file",
    "traffic_blob",
    "Compress",
    "ExecuteExtraTool",
];
const PENDING: &[&str] = &[];
/// Runtime readiness is distinct from verification of the original result projection.
pub fn coverage() -> Value {
    let rows=definitions().iter().map(|d|{let name=text(d,"name");let host=HOST.contains(&name);let present=host||artex_domain::supports(name);let evidence=coverage_evidence(name);json!({"name":name,"status":if host{"host_boundary"}else if present{"domain_executor"}else{"pending"},"implementation_state":if present{"implemented"}else{"unimplemented"},"executor_present":present,"contract_handler":d["rust_handler"],"actual_executor_path":actual_executor_path(name),"effect":d["mutation"],"parity":if evidence.is_empty(){"implementation_reviewed; source equivalence not exhaustively tested"}else{"named behavior slices verified; source equivalence not exhaustively tested"},"verification_evidence":evidence,"activation":d["desktop_status"],"intentional_desktop_changes":["Typed IDs and decimal UInt64 values replace numeric database identifiers","Bodies use verified blob references above inline threshold","Actor epoch, file capabilities, CAS and uncertain external-effect recovery are enforced"],"limitations":if !present{"No executable handler; FEATURE_DISABLED"}else if name=="web_search"{"Search response normalization tested; real credential-dependent remote providers require configured credentials and connectivity"}else if host{"Native or engine executor connected; source parity is limited to the named boundary checks and platform matrix"}else{"Persistent domain handler; named regression fixtures cover listed behavior, not every possible input/state"}})}).collect::<Vec<_>>();
    json!({"source_count":definitions().len(),"tools":rows,"classification_rule":"implemented means an executable domain/native/engine handler exists; parity and evidence are separate and never imply exhaustive source equivalence", "verification_notes":{"execute_extra_actual_controller_dispatch_carries_current_config_and_prepared_identity":"The native host deliberately fails in this controller fixture. It verifies prepared identity, current configuration, dispatch payload and persisted failure, not successful native execution. Successful native custom/MCP calls are separately covered by the named host fixtures."}})
}
fn actual_executor_path(name: &str) -> &'static str {
    match name {
        "traffic_blob" => {
            "artex_rpc::Router::invoke_plan -> Router::traffic_blob_text -> Router::traffic_blob (rpc/src/runtime_tools.rs)"
        }
        "Compress" => {
            "artex_rpc::Router::invoke_plan -> Router::compress_plan (rpc/src/compression.rs)"
        }
        "ExecuteExtraTool" => {
            "artex_rpc::Router::trusted_tool_input -> artex_host::bridge::HostBridge::execute_direct -> artex_host::custom::execute or artex_host::mcp::call"
        }
        "Read" => {
            "artex_rpc::Router::compression_archive_read for blob: paths; artex_host::tools::HostRuntime::execute_tool otherwise"
        }
        name if HOST.contains(&name) => {
            "artex_host::tools::HostRuntime::execute_tool via artex_rpc::Router::next_tool and artex_host::bridge::HostBridge::execute_direct"
        }
        "traffic_search" | "traffic_get" | "get_finding_traffic" => {
            "artex_tools::dispatch_with_blob_reader -> artex_domain::traffic_ui::tool_query"
        }
        name if artex_domain::tool_projection::QUERIES.contains(&name) => {
            "artex_tools::dispatch_with_blob_reader -> artex_domain::tool_projection::query"
        }
        "RecallMemory" | "SearchExtraTools" => "artex_tools::dispatch -> artex_domain::sdk::query",
        "spawn_task" | "pause_task" => {
            "artex_tools::dispatch -> artex_domain::tasks::plan -> artex_domain::tool_projection::mutation_result"
        }
        "insert_assets" | "add_company_scope" | "add_task_scope" | "delete_assets_by_host" => {
            "artex_tools::dispatch -> artex_domain::assets::plan -> artex_domain::tool_projection::mutation_result"
        }
        "report_finding" | "update_finding_report" | "record_finding_retest_result" => {
            "artex_tools::dispatch -> artex_domain::findings::plan -> artex_domain::tool_projection::mutation_result"
        }
        "bind_finding_traffic" => {
            "artex_tools::dispatch -> artex_domain::findings::plan -> artex_domain::findings_ui::query on projected state"
        }
        "create_custom_tool" | "update_custom_tool" | "create_mcp" | "update_mcp" => {
            "artex_tools::dispatch -> artex_domain::configuration::plan -> artex_domain::tool_projection::mutation_result"
        }
        "TodoWrite" | "RecordMemory" | "AskUserQuestion" | "EnterPlanMode" | "ExitPlanMode" => {
            "artex_tools::dispatch -> artex_domain::sdk::plan"
        }
        _ => {
            "artex_tools::dispatch -> artex_domain::exploration::plan -> artex_domain::tool_projection::mutation_result"
        }
    }
}
fn coverage_evidence(name: &str) -> Vec<&'static str> {
    match name {
        "insert_assets" | "list_assets" => vec![
            "domain/tests/invariants.rs::primary_asset_registration_auto_scopes_anchors_and_gates_without_derivative_scope_expansion",
            "domain/tests/invariants.rs::source_asset_derivations_merge_hosts_ips_ports_without_returning_extra_ids",
        ],
        "add_company_scope" | "list_companies" => vec![
            "domain/tests/invariants.rs::company_natural_key_scope_precedence_and_explicit_owner_are_sticky",
            "domain/tests/invariants.rs::company_scope_add_is_partial_replace_is_atomic_and_delete_reattributes",
        ],
        "add_task_scope" | "list_untested_assets" => vec![
            "domain/tests/invariants.rs::task_scope_normalization_is_idempotent_and_sources_remain_read_only",
            "domain/tests/invariants.rs::coverage_uses_direct_source_fact_anchors_and_tree_deduplicates_ancestor_findings",
        ],
        "add_intent" | "record_fact" | "set_goals" | "set_constraints" | "add_hint"
        | "add_task_hint" => vec![
            "domain/tests/invariants.rs::goal_and_constraint_batches_follow_source_defaults_lineage_and_partial_success",
            "domain/tests/invariants.rs::partial_node_batches_and_last_goal_auto_completion_match_source",
            "domain/tests/invariants.rs::live_shared_sources_are_one_level_and_read_only",
        ],
        "graph_overview" | "get_task_graph" | "expand_digest" => vec![
            "domain/tests/invariants.rs::source_graph_summaries_keep_live_lineage_cold_digests_terminal_results_scope_and_text_budgets",
        ],
        "list_facts" | "delete_assets_by_host" | "spawn_task" => vec![
            "domain/tests/invariants.rs::canonical_spawn_delete_host_custom_config_and_fact_page_match_source_operations",
        ],
        "create_custom_tool" | "update_custom_tool" => vec![
            "domain/tests/invariants.rs::canonical_spawn_delete_host_custom_config_and_fact_page_match_source_operations",
            "domain/tests/invariants.rs::custom_tool_header_credentials_reject_plaintext_journal_and_preserve_valid_references",
        ],
        "pause_task" => vec![
            "domain/tests/invariants.rs::explicit_resume_retires_interrupted_runs_without_replaying_uncertain_host_effects",
        ],
        "prove_goal" | "goal_met" => vec![
            "domain/tests/invariants.rs::partial_node_batches_and_last_goal_auto_completion_match_source",
        ],
        "report_finding" | "update_finding_report" | "bind_finding_traffic" => vec![
            "domain/tests/invariants.rs::finding_report_requires_node_identity_and_evidence_cas",
            "domain/tests/invariants.rs::immutable_evidence_deduplicates_verified_capture_without_note_overwrite",
        ],
        "list_findings"
        | "list_task_findings"
        | "list_goals"
        | "node_detail"
        | "get_task_node_detail"
        | "get_worker_output"
        | "get_worker_trace"
        | "get_task_worker_trace"
        | "search_all_worker_traces"
        | "search_task_worker_traces" => vec![
            "domain/tests/invariants.rs::canonical_tool_nonempty_views_match_source_ids_goals_trace_output_and_overview",
            "domain/tests/invariants.rs::inherited_trace_excludes_running_intents_thinking_and_usage",
        ],
        "traffic_search" | "traffic_get" | "get_finding_traffic" => vec![
            "domain/tests/invariants.rs::canonical_traffic_search_ranges_and_evidence_export_zip_use_actual_bytes",
        ],
        "get_finding_retest_context" | "record_finding_retest_result" => vec![
            "domain/tests/invariants.rs::retest_is_conversation_bound_and_fixed_disposition_is_atomic",
        ],
        "AskUserQuestion" | "EnterPlanMode" | "ExitPlanMode" | "RecordMemory" | "RecallMemory"
        | "TodoWrite" => {
            vec![
                "domain/tests/invariants.rs::sdk_memory_unicode_names_and_plan_approval_require_user",
                "tools/src/lib.rs::tests::optional_five_are_implemented_but_default_off",
            ]
        }
        "SearchExtraTools" => vec![
            "domain/tests/invariants.rs::deferred_discovery_respects_exact_select_binding_enabled_and_mcp_visibility",
            "rpc/src/runtime_tool_tests.rs::execute_extra_requires_real_completed_same_run_discovery_and_preserves_secretref",
        ],
        "create_mcp" | "update_mcp" => vec![
            "domain/tests/invariants.rs::canonical_mcp_create_update_preserve_secret_refs_and_reject_invalid_configuration",
            "domain/tests/invariants.rs::ui_mcp_sealed_environment_preflight_commit_query_and_restart_roundtrip",
        ],
        "kill_work" | "steer_work" => vec![
            "domain/tests/invariants.rs::canonical_kill_and_steer_persist_active_run_state_without_touching_other_tasks",
        ],
        "list_tasks" | "list_llm_profiles" | "list_task_worker_traces" => vec![
            "domain/tests/invariants.rs::canonical_task_profile_and_worker_lists_match_source_nonempty_privacy_shapes",
        ],
        "Compress" => vec![
            "rpc/src/regression.rs::compression_archives_originals_and_reopens_with_stable_references",
            "rpc/src/compression_policy.rs::tests::compression_excludes_protected_pairs_skips_overlap_and_preserves_archive_bytes",
            "rpc/src/compression_policy.rs::tests::mode_is_captured_once_and_disabled_compress_never_produces_archives",
        ],
        "ExecuteExtraTool" => vec![
            "host/tests/native_boundary.rs::actual_custom_command_python_and_http_execution",
            "host/tests/native_boundary.rs::actual_mcp_stdio_roundtrip_and_cancel",
            "host/tests/native_boundary.rs::actual_mcp_http_sse_and_network_cancellation",
            "rpc/src/runtime_tool_tests.rs::execute_extra_requires_real_completed_same_run_discovery_and_preserves_secretref",
            "rpc/src/runtime_tool_tests.rs::execute_extra_rechecks_current_schema_version_binding_and_enabled_after_discovery",
            "rpc/src/runtime_tool_tests.rs::execute_extra_mcp_rechecks_live_visibility_cache_schema_and_secretref",
            "rpc/src/runtime_tool_tests.rs::execute_extra_actual_controller_dispatch_carries_current_config_and_prepared_identity",
        ],
        "traffic_blob" => vec![
            "rpc/src/runtime_tool_tests.rs::traffic_blob_actual_tool_uses_decimal_offset_8k_pages_and_source_text_result",
            "rpc/src/runtime_tool_tests.rs::traffic_blob_utf8_binary_and_legacy_negative_offset_are_bounded",
            "rpc/src/runtime_tool_tests.rs::traffic_blob_requires_a_recorded_hash_and_reads_evidence_reference_without_exchange",
        ],
        "Read" | "Write" | "Edit" | "LS" | "Bash" | "TaskOutput" | "TaskStop" | "TaskList"
        | "Monitor" | "sleep" => vec![
            "host/tests/native_boundary.rs::actual_sdk_files_processes_monitor_and_sleep",
            "host/src/execution.rs::tests::owned_execution_and_dedup",
        ],
        "MultiEdit" => vec![
            "host/src/tools.rs::tests::multi_edit_failure_leaves_original",
            "host/tests/native_boundary.rs::actual_sdk_files_processes_monitor_and_sleep",
        ],
        "Glob" | "Grep" => {
            vec!["host/src/search.rs::tests::search_filters_binary_and_supports_context"]
        }
        "shell_open" | "shell_send" | "shell_read" | "shell_close" | "shell_list" => {
            vec!["host/src/pty.rs::tests::actual_terminal_echo_screen_and_owner_cleanup"]
        }
        "Skill" | "create_skill" | "update_skill_file" => {
            vec!["host/src/skills.rs::tests::skill_yaml_and_paths"]
        }
        "WebFetch" => vec![
            "host/tests/native_boundary.rs::actual_webfetch_receives_local_http_and_rejects_assets",
        ],
        "web_search" => vec![
            "host/src/network.rs::tests::assets_and_search_normalization",
            "host/tests/native_boundary.rs::actual_search_backends_against_local_http_fixtures",
            "tools/src/lib.rs::tests::web_search_requires_global_and_agent_flags",
        ],
        _ => vec![],
    }
}
/// Resolve an agent-bound nondeferred user tool without granting arbitrary executable access.
pub fn resolved_definition(name: &str, state: &Snapshot, context: &Context) -> Result<Value> {
    if let Ok(d) = definition(name) {
        return Ok(d.clone());
    }
    let run = artex_domain::sdk::current_run(state, context)?;
    let agent = record(state, "agents", text(&run.data, "agent_id"))?;
    let tool = state
        .records("tools")
        .into_iter()
        .find(|r| r.data["key"] == name && r.data["system"] != true)
        .ok_or_else(|| Error::new("METHOD_NOT_FOUND", "Unknown tool"))?;
    if tool.data["enabled"] != true {
        return Err(Error::new("FEATURE_DISABLED", "Custom tool is disabled"));
    }
    if tool.data["deferred"] == true {
        return Err(Error::new(
            "PERMISSION_DENIED",
            "Deferred tools require discovery and ExecuteExtraTool",
        ));
    }
    if !ids(&tool.data, "agents")
        .iter()
        .any(|role| role == text(&agent.data, "key"))
    {
        return Err(Error::new(
            "PERMISSION_DENIED",
            "Custom tool is not bound to this agent",
        ));
    }
    if !["shell", "command", "python", "script", "http"].contains(&text(&tool.data, "kind")) {
        return Err(Error::new(
            "FEATURE_DISABLED",
            "Custom tool kind has no native executor",
        ));
    }
    if !tool.data["schema"].is_object() {
        return Err(Error::new(
            "INVALID_PARAMS",
            "Custom tool schema must be an object",
        ));
    }
    Ok(
        json!({"name":name,"description":tool.data["description"],"desktop_input_schema":tool.data["schema"],"input_schema":tool.data["schema"],"family":"custom","mutation":"external","kind":tool.data["kind"],"tool_id":tool.header.id,"tool_version":tool.header.version}),
    )
}

pub fn check_access(name: &str, input: &Value, state: &Snapshot, context: &Context) -> Result<()> {
    let def = resolved_definition(name, state, context)?;
    if !input.is_object() {
        return Err(Error::new("INVALID_PARAMS", "tool input must be an object"));
    }
    artex_core::validate_contract(
        &def["desktop_input_schema"],
        &Value::Object(Default::default()),
        input,
    )?;
    let run = artex_domain::sdk::current_run(state, context)?;
    if text(&run.data, "state") != "running" {
        return Err(Error::new(
            "BUSY",
            "A waiting run cannot start a new tool invocation",
        ));
    }
    let agent = record(state, "agents", text(&run.data, "agent_id"))?;
    let role = text(&agent.data, "key");
    if agent.data["enabled"] == false {
        return Err(Error::new("FEATURE_DISABLED", "agent has been disabled"));
    }
    if name == "Skill" {
        // The frozen SDK schema and native Skill handler use `name`, which is
        // the directory key, not the skill's display title or another agent's key.
        let wanted = text(input, "name");
        if !state.records("agent_skill_visibility").iter().any(|v| {
            v.data["agent_id"] == agent.header.id
                && v.data["skill_name"] == wanted
                && v.data["enabled"] == true
        }) {
            return Err(Error::new(
                "PERMISSION_DENIED",
                "Skill is not visible to this agent",
            ));
        }
    }
    if name == "web_search"
        && (agent.data["web_search"] != true
            || !artex_domain::setting_bool(state, "web_search_enabled", false))
    {
        return Err(Error::new(
            "FEATURE_DISABLED",
            "Web search requires both the global setting and the agent setting",
        ));
    }
    if name.starts_with("shell_") && agent.data["interactive_shell"] != true {
        return Err(Error::new(
            "FEATURE_DISABLED",
            "agent interactive shell is disabled",
        ));
    }
    if name == "bind_finding_traffic"
        && !artex_domain::setting_bool(state, "agent_traffic_binding", false)
    {
        return Err(Error::new(
            "FEATURE_DISABLED",
            "automatic finding evidence binding is disabled by default",
        ));
    }
    if name == "bind_finding_traffic"
        && input["traffic_refs"]
            .as_array()
            .is_none_or(|a| a.is_empty())
    {
        return Err(Error::new(
            "INVALID_PARAMS",
            "Evidence binding requires at least one verified traffic reference",
        ));
    }
    if let Some(config) = state
        .records("tools")
        .into_iter()
        .find(|r| text(&r.data, "key") == name)
    {
        if config.data["enabled"] != true {
            return Err(Error::new("FEATURE_DISABLED", "tool has been disabled"));
        }
        if def["family"] == "artex" && !ids(&config.data, "agents").iter().any(|a| a == role) {
            return Err(Error::new(
                "PERMISSION_DENIED",
                "tool is not bound to this agent",
            ));
        }
    } else if def["family"] == "artex" && !ids(&def, "fresh_seed_agents").iter().any(|a| a == role)
    {
        return Err(Error::new(
            "PERMISSION_DENIED",
            "tool is not in fresh agent bindings",
        ));
    }
    if role == "worker" && ["MultiEdit", "Glob", "Grep"].contains(&name) {
        return Err(Error::new(
            "PERMISSION_DENIED",
            "tool excluded from worker default SDK set",
        ));
    }
    let feature = match name {
        "AskUserQuestion" => Some("questions"),
        "EnterPlanMode" | "ExitPlanMode" => Some("plan_mode"),
        "RecallMemory" | "RecordMemory" => Some("memory"),
        _ => None,
    };
    if let Some(flag) = feature {
        let enabled = state
            .records("agent_features")
            .iter()
            .any(|r| text(&r.data, "agent_id") == agent.header.id && r.data[flag] == true);
        if !enabled {
            return Err(Error::new(
                "FEATURE_DISABLED",
                "optional SDK tool is disabled by default",
            ));
        }
    }
    if text(&run.data, "permission_mode") == "plan"
        && text(&def, "mutation") != "read"
        && !matches!(
            name,
            "TodoWrite" | "AskUserQuestion" | "ExitPlanMode" | "sleep"
        )
    {
        return Err(Error::new(
            "PERMISSION_DENIED",
            "plan mode forbids workspace mutations and external effects",
        ));
    }
    if let Some(request_task) = input["task_id"].as_str() {
        if !["auto", "reporter"].contains(&role) && context.task_id.as_deref() != Some(request_task)
        {
            return Err(Error::new(
                "PERMISSION_DENIED",
                "cross-task access requires orchestration agent binding",
            ));
        }
    }
    Ok(())
}
pub fn dispatch(
    name: &str,
    input: &Value,
    state: &Snapshot,
    context: &Context,
) -> Result<ToolDispatch> {
    check_access(name, input, state, context)?;
    if PENDING.contains(&name) {
        return Err(Error::new(
            "FEATURE_DISABLED",
            &format!("{name} executor is pending; registration does not imply implementation"),
        ));
    }
    if HOST.contains(&name) || definition(name).is_err() {
        return Ok(ToolDispatch::HostIntent {
            name: name.into(),
            input: input.clone(),
            context: context.clone(),
        });
    }
    let mut delegated = context.clone();
    if let Some(task) = input["task_id"].as_str() {
        delegated.task_id = Some(task.into());
    }
    if artex_domain::is_mutation(name) {
        let mut batch = artex_domain::plan(name, input, state, &delegated)?;
        if name == "bind_finding_traffic" {
            let projected = artex_domain::projected(state, &batch)?;
            let mut result = artex_domain::findings_ui::query(
                "ui.findingTraffic",
                &json!({"id":input["finding_id"],"contextTask":delegated.task_id}),
                &projected,
                &delegated,
            )?;
            for b in result["bindings"].as_array_mut().unwrap() {
                b["snapshot"]["req_head"] = json!("");
                b["snapshot"]["resp_head"] = json!("");
            }
            batch.result = result;
        } else {
            batch.result =
                artex_domain::tool_projection::mutation_result(name, input, &batch.result);
        }
        Ok(ToolDispatch::Command(batch))
    } else if artex_domain::supports(name) {
        Ok(ToolDispatch::Query(
            if artex_domain::tool_projection::QUERIES.contains(&name) {
                artex_domain::tool_projection::query(name, input, state, &delegated, &|_| {
                    Err(Error::new(
                        "FEATURE_DISABLED",
                        "Verified blob reader required",
                    ))
                })?
            } else {
                artex_domain::query(name, input, state, &delegated)?
            },
        ))
    } else {
        Err(Error::new("FEATURE_DISABLED", "executor is not available"))
    }
}

/// Blob-aware queries retain domain access checks and use the caller's verified store reader.
pub fn dispatch_with_blob_reader(
    name: &str,
    input: &Value,
    state: &Snapshot,
    context: &Context,
    reader: &dyn Fn(&Value) -> Result<Vec<u8>>,
) -> Result<ToolDispatch> {
    if ["traffic_search", "traffic_get", "get_finding_traffic"].contains(&name) {
        check_access(name, input, state, context)?;
        return Ok(ToolDispatch::Query(artex_domain::traffic_ui::tool_query(
            name, input, state, context, reader,
        )?));
    }
    if artex_domain::tool_projection::QUERIES.contains(&name) {
        check_access(name, input, state, context)?;
        let mut delegated = context.clone();
        if let Some(task) = input["task_id"].as_str() {
            delegated.task_id = Some(task.into());
        }
        return Ok(ToolDispatch::Query(artex_domain::tool_projection::query(
            name, input, state, &delegated, reader,
        )?));
    }
    dispatch(name, input, state, context)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn exact_source_count_and_no_registration_as_coverage() {
        let c = coverage();
        assert_eq!(c["source_count"], 81);
        let rows = c["tools"].as_array().unwrap();
        assert!(
            rows.iter().all(
                |r| r["implementation_state"] == "implemented" && r["executor_present"] == true
            )
        );
        assert!(rows.iter().all(|r| {
            r["parity"]
                .as_str()
                .is_some_and(|p| p.contains("not exhaustively tested"))
        }));
        assert!(rows.iter().any(|r| r["status"] == "host_boundary"));
        assert!(rows.iter().all(|r| {
            r["contract_handler"].is_string()
                && r["actual_executor_path"]
                    .as_str()
                    .is_some_and(|p| !p.is_empty())
                && !r["verification_evidence"].as_array().unwrap().is_empty()
        }));
        assert!(
            rows.iter().find(|r| r["name"] == "traffic_blob").unwrap()["actual_executor_path"]
                .as_str()
                .unwrap()
                .contains("Router::traffic_blob")
        );
        assert_eq!(
            definition("update_finding_report").unwrap()["desktop_input_schema"]["properties"]["finding_node_id"]
                ["pattern"],
            "^node_[0-9a-f]{32}$"
        );
    }
    fn fixture(role: &str) -> (Snapshot, Context) {
        use artex_core::{Actor, Header, Record, new_id};
        let agent = new_id("agent");
        let run = new_id("run");
        let epoch = new_id("epoch");
        let mut s = Snapshot {
            workspace_id: new_id("workspace"),
            commit_seq: 0,
            collections: Default::default(),
        };
        for (c, id, data) in [
            ("agents", agent.clone(), json!({"id":agent,"key":role})),
            (
                "agent_runs",
                run.clone(),
                json!({"id":run,"agent_id":agent,"state":"running","engine_epoch":epoch,"permission_mode":"default"}),
            ),
        ] {
            let r = Record {
                header: Header {
                    id: id.clone(),
                    collection: c.into(),
                    version: "1".into(),
                    commit_seq: "1".into(),
                    created_at: artex_core::now(),
                    updated_at: artex_core::now(),
                    legacy_key: Value::Null,
                },
                data,
            };
            s.collections.entry(c.into()).or_default().insert(id, r);
        }
        let context = Context {
            actor: Actor {
                kind: "agent".into(),
                agent_id: Some(agent),
                run_id: Some(run),
                engine_epoch: epoch,
            },
            task_id: None,
            intent_id: None,
            conversation_id: None,
        };
        (s, context)
    }
    #[test]
    fn optional_five_are_implemented_but_default_off() {
        let (s, c) = fixture("mainagent");
        for (name, input) in [
            (
                "AskUserQuestion",
                json!({"questions":[{"question":"choose","header":"choice","options":[{"label":"one","description":"first"},{"label":"two","description":"second"}],"multi_select":false}]}),
            ),
            ("EnterPlanMode", json!({})),
            ("ExitPlanMode", json!({"plan":"plan"})),
            ("RecallMemory", json!({"query":"x"})),
            ("RecordMemory", json!({"name":"한국어","content":"content"})),
        ] {
            assert_eq!(
                check_access(name, &input, &s, &c).unwrap_err().code,
                "FEATURE_DISABLED",
                "{name}"
            );
        }
    }
    #[test]
    fn owner_epoch_and_binding_cannot_be_bypassed() {
        let (s, mut c) = fixture("worker");
        assert_eq!(
            check_access("spawn_task", &json!({"description":"d","goal":"g"}), &s, &c)
                .unwrap_err()
                .code,
            "PERMISSION_DENIED"
        );
        assert_eq!(
            check_access("Glob", &json!({"pattern":"*"}), &s, &c)
                .unwrap_err()
                .code,
            "PERMISSION_DENIED"
        );
        c.actor.engine_epoch = artex_core::new_id("epoch");
        assert_eq!(
            check_access("Read", &json!({"file_path":"a"}), &s, &c)
                .unwrap_err()
                .code,
            "STALE_EPOCH"
        );
    }
    #[test]
    fn skill_visibility_matches_current_agent_directory_key_and_enabled_state() {
        let (mut s, c) = fixture("auto");
        let input = json!({"name":"allowed-skill","args":"run the procedure"});
        assert_eq!(
            check_access("Skill", &input, &s, &c).unwrap_err().code,
            "PERMISSION_DENIED"
        );
        let id = artex_core::new_id("skillvisibility");
        let agent = c.actor.agent_id.as_ref().unwrap();
        let mut header = s.get("agents", agent).unwrap().header.clone();
        header.id = id.clone();
        header.collection = "agent_skill_visibility".into();
        let other_id = artex_core::new_id("agent");
        let mut other = s.get("agents", agent).unwrap().clone();
        other.header.id = other_id.clone();
        other.data = json!({"id":other_id,"key":"planner","enabled":true});
        s.collections
            .get_mut("agents")
            .unwrap()
            .insert(other_id.clone(), other);
        s.collections
            .entry("agent_skill_visibility".into())
            .or_default()
            .insert(
                id.clone(),
                artex_core::Record {
                    header,
                    data: json!({"agent_id":other_id,"skill_name":"allowed-skill","enabled":true}),
                },
            );
        assert_eq!(
            check_access("Skill", &input, &s, &c).unwrap_err().code,
            "PERMISSION_DENIED"
        );
        s.collections
            .get_mut("agent_skill_visibility")
            .unwrap()
            .get_mut(&id)
            .unwrap()
            .data["agent_id"] = json!(agent);
        s.collections
            .get_mut("agent_skill_visibility")
            .unwrap()
            .get_mut(&id)
            .unwrap()
            .data["enabled"] = json!(false);
        assert_eq!(
            check_access("Skill", &input, &s, &c).unwrap_err().code,
            "PERMISSION_DENIED"
        );
        s.collections
            .get_mut("agent_skill_visibility")
            .unwrap()
            .get_mut(&id)
            .unwrap()
            .data["enabled"] = json!(true);
        assert!(matches!(
            dispatch("Skill", &input, &s, &c).unwrap(),
            ToolDispatch::HostIntent { .. }
        ));
        assert_eq!(
            check_access("Skill", &json!({"name":"Allowed-Skill"}), &s, &c)
                .unwrap_err()
                .code,
            "PERMISSION_DENIED"
        );
        assert_eq!(
            check_access("Skill", &json!({"name":"unlisted-skill"}), &s, &c)
                .unwrap_err()
                .code,
            "PERMISSION_DENIED"
        );
        assert_eq!(
            check_access("Skill", &json!({"skill":"allowed-skill"}), &s, &c)
                .unwrap_err()
                .code,
            "INVALID_PARAMS"
        );
        let mut stale = c.clone();
        stale.actor.engine_epoch = artex_core::new_id("epoch");
        assert_eq!(
            check_access("Skill", &input, &s, &stale).unwrap_err().code,
            "STALE_EPOCH"
        );
    }
    #[test]
    fn saved_scalar_defaults_fill_omitted_null_empty_only_and_keep_strict_validation() {
        let (mut s, c) = fixture("auto");
        fn tool(s: &mut Snapshot, key: &str, schema: Value) {
            let mut row = s.records("agents")[0].clone();
            row.header.id = artex_core::new_id("toolconfig");
            row.header.collection = "tools".into();
            row.data =
                json!({"key":key,"schema":schema,"enabled":true,"system":true,"agents":["auto"]});
            s.collections
                .entry("tools".into())
                .or_default()
                .insert(row.header.id.clone(), row);
        }
        tool(
            &mut s,
            "Read",
            json!({"properties":{"file_path":{"type":"string","default":"evidence.txt"}}}),
        );
        let normalized = normalize_scalar_defaults("Read", &json!({}), &s);
        assert_eq!(normalized, json!({"file_path":"evidence.txt"}));
        assert!(check_access("Read", &normalized, &s, &c).is_ok());
        for value in [Value::Null, json!(" "), json!([])] {
            if value.is_null() {
                assert_eq!(normalize_scalar_defaults("Read", &value, &s), value);
            } else {
                assert_eq!(normalize_scalar_defaults("Read", &value, &s), value);
            }
        }
        tool(
            &mut s,
            "scalar_example",
            json!({"properties":{"text":{"type":"string","default":"default text"},"count":{"type":"integer","default":7},"ratio":{"type":"number","default":1.5},"flag":{"type":"boolean","default":false},"object":{"type":"object","default":{"x":1}},"array":{"type":"array","default":[1]},"malformed":{"type":"string","default":[1]},"nil":{"type":"string","default":null}}}),
        );
        assert_eq!(
            normalize_scalar_defaults("scalar_example", &json!({"text":"","count":null}), &s),
            json!({"text":"default text","count":7,"ratio":1.5,"flag":false})
        );
        let explicit = json!({"text":" ","count":0,"ratio":2,"flag":true,"object":{},"array":[],"custom":"user input"});
        assert_eq!(
            normalize_scalar_defaults("scalar_example", &explicit, &s),
            explicit
        );
        tool(
            &mut s,
            "get_task_graph",
            json!({"properties":{"task_id":{"type":"string","default":"legacy-invalid-id"}}}),
        );
        let invalid = normalize_scalar_defaults("get_task_graph", &json!({}), &s);
        assert_eq!(
            check_access("get_task_graph", &invalid, &s, &c)
                .unwrap_err()
                .code,
            "INVALID_PARAMS"
        );
        assert_eq!(
            normalize_scalar_defaults("no_saved_row", &explicit, &s),
            explicit
        );
    }
    #[test]
    fn deferred_custom_defaults_apply_only_to_nested_scalar_params() {
        let (mut s, _) = fixture("auto");
        let mut row = s.records("agents")[0].clone();
        row.header.id = artex_core::new_id("toolconfig");
        row.header.collection = "tools".into();
        row.data = json!({"key":"deferred_probe","system":false,"deferred":true,"schema":{"properties":{"target":{"type":"string","default":"example.test"},"limit":{"type":"integer","default":5},"enabled":{"type":"boolean","default":true},"structured":{"type":"object","default":{"target":"must not merge"}}}}});
        s.collections
            .entry("tools".into())
            .or_default()
            .insert(row.header.id.clone(), row);
        let input = json!({"tool_name":"deferred_probe","params":{"target":null,"limit":"","enabled":false},"outer":"preserve"});
        assert_eq!(
            normalize_scalar_defaults("ExecuteExtraTool", &input, &s),
            json!({"tool_name":"deferred_probe","params":{"target":"example.test","limit":5,"enabled":false},"outer":"preserve"})
        );
        let mcp = json!({"tool_name":"mcp__server__probe","params":{}});
        assert_eq!(normalize_scalar_defaults("ExecuteExtraTool", &mcp, &s), mcp);
        let malformed = json!({"tool_name":"deferred_probe","params":[]});
        assert_eq!(
            normalize_scalar_defaults("ExecuteExtraTool", &malformed, &s),
            malformed
        );
    }
    #[test]
    fn plan_mode_blocks_writes_and_never_auto_approves() {
        let (mut s, c) = fixture("mainagent");
        s.collections
            .get_mut("agent_runs")
            .unwrap()
            .get_mut(c.actor.run_id.as_ref().unwrap())
            .unwrap()
            .data["permission_mode"] = json!("plan");
        assert_eq!(
            check_access("Write", &json!({"file_path":"a","content":"b"}), &s, &c)
                .unwrap_err()
                .code,
            "PERMISSION_DENIED"
        );
        assert!(check_access("Read", &json!({"file_path":"a"}), &s, &c).is_ok());
        assert!(check_access("TodoWrite", &json!({"todos":[]}), &s, &c).is_ok());
    }
    #[test]
    fn nondeferred_custom_tool_schema_binding_and_plan_gate() {
        let (mut s, c) = fixture("mainagent");
        let id = artex_core::new_id("toolconfig");
        let data = json!({"key":"inspect_custom","system":false,"kind":"command","schema":{"type":"object","properties":{"target":{"type":"string"}},"required":["target"],"additionalProperties":false},"enabled":true,"deferred":false,"agents":["mainagent"]});
        s.collections.entry("tools".into()).or_default().insert(
            id.clone(),
            artex_core::Record {
                header: artex_core::Header {
                    id: id.clone(),
                    collection: "tools".into(),
                    version: "1".into(),
                    commit_seq: "1".into(),
                    created_at: artex_core::now(),
                    updated_at: artex_core::now(),
                    legacy_key: Value::Null,
                },
                data,
            },
        );
        assert!(matches!(
            dispatch("inspect_custom", &json!({"target":"a"}), &s, &c),
            Ok(ToolDispatch::HostIntent { .. })
        ));
        assert_eq!(
            check_access("inspect_custom", &json!({"target":1}), &s, &c)
                .unwrap_err()
                .code,
            "INVALID_PARAMS"
        );
        s.collections
            .get_mut("tools")
            .unwrap()
            .get_mut(&id)
            .unwrap()
            .data["deferred"] = json!(true);
        assert_eq!(
            resolved_definition("inspect_custom", &s, &c)
                .unwrap_err()
                .code,
            "PERMISSION_DENIED"
        );
        s.collections
            .get_mut("tools")
            .unwrap()
            .get_mut(&id)
            .unwrap()
            .data["deferred"] = json!(false);
        s.collections
            .get_mut("agent_runs")
            .unwrap()
            .get_mut(c.actor.run_id.as_ref().unwrap())
            .unwrap()
            .data["permission_mode"] = json!("plan");
        assert_eq!(
            check_access("inspect_custom", &json!({"target":"a"}), &s, &c)
                .unwrap_err()
                .code,
            "PERMISSION_DENIED"
        );
    }
    #[test]
    fn web_search_requires_global_and_agent_flags() {
        let (mut s, c) = fixture("auto");
        let aid = c.actor.agent_id.as_ref().unwrap();
        s.collections
            .get_mut("agents")
            .unwrap()
            .get_mut(aid)
            .unwrap()
            .data["web_search"] = json!(true);
        let input = json!({"query":"test"});
        assert_eq!(
            check_access("web_search", &input, &s, &c).unwrap_err().code,
            "FEATURE_DISABLED"
        );
        let id = artex_core::new_id("setting");
        s.collections.entry("settings".into()).or_default().insert(
            id.clone(),
            artex_core::Record {
                header: artex_core::Header {
                    id: id.clone(),
                    collection: "settings".into(),
                    version: "1".into(),
                    commit_seq: "1".into(),
                    created_at: artex_core::now(),
                    updated_at: artex_core::now(),
                    legacy_key: Value::Null,
                },
                data: json!({"key":"web_search_enabled","value":"true"}),
            },
        );
        assert!(check_access("web_search", &input, &s, &c).is_ok());
        s.collections
            .get_mut("agents")
            .unwrap()
            .get_mut(aid)
            .unwrap()
            .data["web_search"] = json!(false);
        assert_eq!(
            check_access("web_search", &input, &s, &c).unwrap_err().code,
            "FEATURE_DISABLED"
        );
    }
    #[test]
    fn waiting_run_cannot_dispatch_new_read_write_or_workspace_tools() {
        let (mut s, c) = fixture("mainagent");
        s.collections
            .get_mut("agent_runs")
            .unwrap()
            .get_mut(c.actor.run_id.as_ref().unwrap())
            .unwrap()
            .data["state"] = json!("waiting");
        // Communication authentication permits waiting, but tool execution does not.
        assert!(artex_domain::sdk::current_run(&s, &c).is_ok());
        for (name, input) in [
            ("Read", json!({"file_path":"a"})),
            ("TodoWrite", json!({"todos":[]})),
            ("list_tasks", json!({})),
        ] {
            assert!(
                matches!(dispatch(name, &input, &s, &c), Err(e) if e.code == "BUSY"),
                "{name}"
            );
        }
    }
}
