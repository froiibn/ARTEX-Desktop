//! Explicit UI facade adapters. Unsupported options fail before domain planning.
use artex_core::{Change, CommandBatch, Context, Error, Result, Snapshot};
use serde_json::{Value, json};
mod auxiliary;
mod chat;
mod configuration;

pub const QUERIES: &[&str] = &[
    "legacy.handleSideQuestions",
    "legacy.sideEvents",
    "ui.ssStatus",
    "ui.ssProjects",
    "ui.ssTasks",
    "legacy.getLogs",
    "legacy.getLogsHistory",
    "ui.chatMentions",
    "ui.getLLM",
    "ui.llmPool",
    "ui.taskLLMResolution",
    "ui.llmRecords",
    "ui.llmRecordDetail",
    "ui.llmTasks",
    "ui.tokensByModel",
    "ui.commands",
    "ui.interceptRules",
    "ui.interceptPending",
    "ui.interceptGetOne",
    "ui.interceptExecution",
    "ui.interceptDetail",
    "ui.interceptHistory",
    "ui.interceptHistoryPage",
    "ui.interceptTask",
    "ui.interceptTaskPage",
    "ui.interceptGetToolConfig",
    "ui.interceptGetJudgeConfig",
    "ui.interceptJudgeUsage",
    "ui.taskCoverage",
    "ui.taskCoverageGraph",
    "ui.findingAssetTree",
    "ui.assets",
    "ui.searchAssets",
    "ui.taskAssets",
    "ui.searchTaskAssets",
    "ui.assetCounts",
    "ui.taskIntentAssets",
    "ui.traffic",
    "ui.trafficExchange",
    "ui.trafficHosts",
    "ui.findingTrafficDetail",
    "ui.findingTrafficBody",
    "ui.downloadFindingTrafficBody",
    "ui.exportFindings",
    "ui.report",
    "ui.audit",
    "ui.findings",
    "ui.findingsPage",
    "ui.findingGroups",
    "ui.getFinding",
    "ui.findingTraffic",
    "ui.findingLineage",
    "ui.findingRetests",
    "ui.activeFindingRetests",
    "ui.frontier",
    "ui.intents",
    "ui.intentsPage",
    "ui.explorationGraph",
    "ui.explorationNodes",
    "ui.activity",
    "ui.activityDetail",
    "ui.activityHistory",
    "ui.mainSessions",
    "ui.taskAssetRefs",
    "ui.taskArchives",
    "ui.taskArchive",
    "ui.stats",
    "ui.usageStats",
    "ui.findingStats",
    "ui.tokenStats",
    "ui.tokenDaily",
    "ui.commandStats",
    "ui.notifyMeta",
    "ui.notifyChannels",
    "ui.notifyDeliveries",
    "ui.tasks",
    "ui.conversationTokens",
    "ui.skillUsage",
    "ui.missingSkills",
    "ui.taskGoals",
    "ui.taskScope",
    "ui.task",
    "ui.companies",
    "ui.conversations",
    "ui.conversationMessages",
    "ui.conversationHistory",
    "ui.conversationMsgDetail",
    "ui.chatStatus",
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
    "ui.taskCategories",
    "ui.taskTemplates",
    "ui.taskConstraints",
    "ui.taskInterceptRules",
    "ui.assetInterceptRules",
];
pub const COMMANDS: &[&str] = &[
    "ui.recoverTaskBudget",
    "legacy.handleSideQuestions",
    "legacy.cancelSideRequest",
    "ui.ssDatasource",
    "ui.ssSync",
    "ui.deleteTask",
    "ui.setLLM",
    "ui.resetLLMPool",
    "ui.llmRecordsDeleteTask",
    "ui.createInterceptRule",
    "ui.updateInterceptRule",
    "ui.deleteInterceptRule",
    "ui.toggleInterceptRule",
    "ui.interceptDecide",
    "ui.interceptSetToolConfig",
    "ui.interceptSetJudgeConfig",
    "ui.deleteAssets",
    "ui.attachTaskAssets",
    "ui.registerTaskAssetScopes",
    "ui.detachTaskAsset",
    "ui.addCompanyScope",
    "ui.updateCompanyScope",
    "ui.deleteCompany",
    "ui.trafficDeleteHost",
    "ui.trafficDeleteHosts",
    "ui.trafficDeleteAll",
    "ui.setFindingStatus",
    "ui.setFindingSeverity",
    "ui.updateFinding",
    "ui.deleteFinding",
    "ui.bindFindingTraffic",
    "ui.editFindingTraffic",
    "ui.removeFindingTraffic",
    "ui.orderFindingTraffic",
    "ui.startFindingRetest",
    "ui.deepenFinding",
    "ui.newMainSession",
    "ui.archiveTask",
    "ui.archiveTasks",
    "ui.restoreTaskArchive",
    "ui.restoreTaskArchives",
    "ui.deleteTaskArchive",
    "ui.deleteTaskArchives",
    "ui.notifyCreateChannel",
    "ui.notifyUpdateChannel",
    "ui.notifyDeleteChannel",
    "ui.notifyRetryDelivery",
    "ui.createTask",
    "ui.renameTask",
    "ui.pinTask",
    "ui.addGoal",
    "ui.updateGoal",
    "ui.deleteGoal",
    "ui.controlTasksBatch",
    "ui.controlIntent",
    "ui.rerunIntent",
    "ui.rerunBlocked",
    "ui.sendWorkerMessage",
    "ui.updateTaskLLMProfiles",
    "ui.addTaskScope",
    "ui.deleteTaskScope",
    "ui.setActive",
    "ui.createCompany",
    "ui.controlTask",
    "ui.createConversation",
    "ui.updateConversation",
    "ui.renameConversation",
    "ui.pinConversation",
    "ui.updateConversationProfile",
    "ui.deleteConversation",
    "ui.deleteConversations",
    "ui.sendConversationMessage",
    "ui.stopConversation",
    "ui.chat",
    "ui.stopChat",
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
    "ui.updateTask",
    "ui.createTaskCategory",
    "ui.renameTaskCategory",
    "ui.deleteTaskCategory",
    "ui.updateTaskCategory",
    "ui.updateTasksCategory",
    "ui.createTaskTemplate",
    "ui.updateTaskTemplate",
    "ui.deleteTaskTemplate",
    "ui.addConstraint",
    "ui.updateConstraint",
    "ui.deleteConstraint",
    "ui.createTaskInterceptRule",
    "ui.updateTaskInterceptRule",
    "ui.deleteTaskInterceptRule",
    "ui.toggleTaskInterceptRule",
    "ui.createAssetInterceptRule",
    "ui.updateAssetInterceptRule",
    "ui.deleteAssetInterceptRule",
    "ui.toggleAssetInterceptRule",
];
fn wire_task(data: &Value, state: &Snapshot) -> Value {
    let mut out = json!({"id":data["id"],"name":data["name"],"description":data["description"],"goal":data["goal"],"status":data["status"],"created_at":data["created_at"],"paused":data["paused"],"queued":data["queued"],"coverage_enabled":data["coverage_enabled"]});
    for key in [
        "category_id",
        "llm_profile_id",
        "active_llm_profile_id",
        "completed_at",
        "pinned_at",
    ] {
        if !data[key].is_null() {
            out[key] = data[key].clone();
        }
    }
    if let Some(runtime) = state
        .records("task_runtime")
        .into_iter()
        .find(|r| r.data["task_id"] == data["id"])
    {
        for field in [
            "accounting_state",
            "active_elapsed_ms",
            "remaining_timeout_ms",
        ] {
            out[field] = runtime.data[field].clone();
        }
    }
    out["source_task_ids"] = json!(
        state
            .records("task_relations")
            .into_iter()
            .filter(|r| r.data["task_id"] == data["id"])
            .map(|r| r.data["source_task_id"].clone())
            .collect::<Vec<_>>()
    );
    out
}
pub fn query_with_blob_reader(
    method: &str,
    p: &Value,
    s: &Snapshot,
    c: &Context,
    reader: &dyn Fn(&Value) -> Result<Vec<u8>>,
) -> Result<Value> {
    if artex_domain::llm_ui::QUERIES.contains(&method) {
        return artex_domain::llm_ui::query(method, p, s, c, reader);
    }
    if artex_domain::traffic_ui::QUERIES.contains(&method) {
        return artex_domain::traffic_ui::query(method, p, s, c, reader);
    }
    artex_domain::transcript_ui::project_with_snapshot(method, query(method, p, s, c)?, s, reader)
}
pub fn query(method: &str, p: &Value, s: &Snapshot, c: &Context) -> Result<Value> {
    match method {
        m if artex_domain::side_questions::QUERIES.contains(&m) => {
            artex_domain::side_questions::query(m, p, s, c)
        }
        m if artex_domain::sync_scopesentry::QUERIES.contains(&m) => {
            artex_domain::sync_scopesentry::query(m, p, s, c)
        }
        m if artex_domain::logs_ui::QUERIES.contains(&m) => {
            artex_domain::logs_ui::query(m, p, s, c)
        }
        m if artex_domain::mentions_ui::QUERIES.contains(&m) => {
            artex_domain::mentions_ui::query(m, p, s, c)
        }
        m if artex_domain::llm_ui::QUERIES.contains(&m) => {
            artex_domain::llm_ui::query(m, p, s, c, &|_| {
                Err(Error::new("FEATURE_DISABLED", "Blob reader is required"))
            })
        }
        m if artex_domain::asset_graph_ui::QUERIES.contains(&m) => {
            artex_domain::asset_graph_ui::query(m, p, s, c)
        }
        m if artex_domain::policy_ui::QUERIES.contains(&m) => {
            artex_domain::policy_ui::query(m, p, s, c)
        }
        m if artex_domain::assets_ui::QUERIES.contains(&m) => {
            artex_domain::assets_ui::query(m, p, s, c)
        }
        m if artex_domain::task_control_ui::QUERIES.contains(&m) => {
            artex_domain::task_control_ui::query(m, p, s, c)
        }
        m if artex_domain::auxiliary_ui::QUERIES.contains(&m) => auxiliary::query(m, p, s, c),
        m if artex_domain::skills_ui::QUERIES.contains(&m) => {
            artex_domain::skills_ui::query(m, p, s, c)
        }
        m if artex_domain::exploration_ui::QUERIES.contains(&m) => {
            artex_domain::exploration_ui::query(m, p, s, c)
        }
        m if artex_domain::findings_ui::QUERIES.contains(&m) => {
            artex_domain::findings_ui::query(m, p, s, c)
        }
        m if artex_domain::traffic_ui::QUERIES.contains(&m) => {
            artex_domain::traffic_ui::query(m, p, s, c, &|_| {
                Err(Error::new(
                    "FEATURE_DISABLED",
                    "Blob reader is required for this query",
                ))
            })
        }
        m if chat::QUERIES.contains(&m) => chat::query(m, p, s, c),
        m if artex_domain::task_ui::QUERIES.contains(&m) => {
            artex_domain::task_ui::query(m, p, s, c)
        }
        m if artex_domain::configuration_ui::QUERIES.contains(&m) => {
            configuration::query(m, p, s, c)
        }
        "ui.tasks" => {
            let tasks = artex_domain::query("task.list", &json!({}), s, c)?;
            Ok(
                json!({"tasks":tasks.as_array().unwrap().iter().map(|t|wire_task(t,s)).collect::<Vec<_>>(),"active":""}),
            )
        }
        "ui.task" => {
            let d = artex_domain::query("task.get", &json!({"task_id":p["id"]}), s, c)?;
            Ok(wire_task(&d, s))
        }
        "ui.companies" => {
            let rows = artex_domain::query("company.list", &json!({}), s, c)?;
            Ok(json!(rows.as_array().unwrap().iter().map(|co|{
            let mut v=json!({"id":co["id"],"name":co["name"],"asset_count":s.records("assets").into_iter().filter(|a|a.data["company_id"]==co["id"]).count()});
            if !co["logo"].is_null(){v["logo"]=co["logo"].clone();}v
        }).collect::<Vec<_>>()))
        }
        _ => Err(Error::new("METHOD_NOT_FOUND", "Unknown UI query")),
    }
}
pub fn plan(method: &str, p: &Value, s: &Snapshot, c: &Context) -> Result<CommandBatch> {
    match method {
        m if artex_domain::side_questions::COMMANDS.contains(&m) => {
            artex_domain::side_questions::plan(m, p, s, c)
        }
        m if artex_domain::sync_scopesentry::COMMANDS.contains(&m) => {
            artex_domain::sync_scopesentry::plan(m, p, s, c)
        }
        m if artex_domain::task_delete_ui::COMMANDS.contains(&m) => {
            artex_domain::task_delete_ui::plan(m, p, s, c)
        }
        m if artex_domain::llm_ui::COMMANDS.contains(&m) => artex_domain::llm_ui::plan(m, p, s, c),
        m if artex_domain::policy_ui::COMMANDS.contains(&m) => {
            artex_domain::policy_ui::plan(m, p, s, c)
        }
        m if artex_domain::assets_ui::COMMANDS.contains(&m) => {
            artex_domain::assets_ui::plan(m, p, s, c)
        }
        m if artex_domain::task_control_ui::COMMANDS.contains(&m) => {
            let mut b = artex_domain::task_control_ui::plan(m, p, s, c)?;
            if matches!(m, "ui.renameTask" | "ui.pinTask") {
                b.result = wire_task(&b.result, s);
            }
            Ok(b)
        }
        m if artex_domain::auxiliary_ui::COMMANDS.contains(&m) => auxiliary::plan(m, p, s, c),
        m if artex_domain::exploration_ui::COMMANDS.contains(&m) => {
            artex_domain::exploration_ui::plan(m, p, s, c)
        }
        m if artex_domain::findings_ui::COMMANDS.contains(&m) => {
            artex_domain::findings_ui::plan(m, p, s, c)
        }
        m if artex_domain::traffic_ui::COMMANDS.contains(&m) => {
            artex_domain::traffic_ui::plan(m, p, s, c)
        }
        m if chat::COMMANDS.contains(&m) => chat::plan(m, p, s, c),
        m if artex_domain::task_ui::COMMANDS.contains(&m) => {
            let mut b = artex_domain::task_ui::plan(m, p, s, c)?;
            if m == "ui.updateTask" || m == "ui.updateTaskCategory" {
                b.result = wire_task(&b.result, s);
            }
            Ok(b)
        }
        m if artex_domain::configuration_ui::COMMANDS.contains(&m) => {
            configuration::plan(m, p, s, c)
        }
        "ui.createTask" => {
            let input = &p["input"];
            let mut parameters = json!({"description":input["description"],"goal":input["goal"]});
            for (ui, domain) in [
                ("name", "name"),
                ("categoryId", "category_id"),
                ("sourceTaskIds", "source_task_ids"),
                ("timeoutSeconds", "timeout_seconds"),
                ("seedFirstIntent", "seed_first_intent"),
                ("planHeartbeatSeconds", "plan_heartbeat_seconds"),
                ("coverageEnabled", "coverage_enabled"),
                ("llmProfileIds", "llm_profile_ids"),
                ("companyIds", "company_ids"),
                ("interceptRules", "intercept_rules"),
            ] {
                if let Some(v) = input.get(ui) {
                    parameters[domain] = v.clone();
                }
            }
            let mut b = artex_domain::plan("task.create", &parameters, s, c)?;
            let id = b.result["task_id"]
                .as_str()
                .ok_or_else(|| Error::new("EXTERNAL_FAILURE", "Task planner omitted its ID"))?
                .to_owned();
            let mut task = b
                .changes
                .iter()
                .find_map(|change| match change {
                    Change::Put {
                        collection,
                        id: i,
                        data,
                    } if collection == "tasks" && i == &id => Some(data.clone()),
                    _ => None,
                })
                .ok_or_else(|| Error::new("EXTERNAL_FAILURE", "Task planner omitted its record"))?;
            if let Some(name) = input["name"].as_str() {
                task["name"] = json!(name.trim());
            }
            if let Some(coverage) = input["coverageEnabled"].as_bool() {
                task["coverage_enabled"] = json!(coverage);
            }
            let limit = artex_domain::configuration_ui::setting(s, "task_concurrency_limit")
                .parse::<usize>()
                .unwrap_or(5)
                .max(1);
            let running = s
                .records("tasks")
                .iter()
                .filter(|t| {
                    t.data["status"] == "running"
                        && t.data["deleted_at"].is_null()
                        && t.data["archived_at"].is_null()
                })
                .count();
            let queued = artex_domain::setting_bool(s, "task_concurrency_enabled", false)
                && running >= limit;
            task["paused"] = json!(false);
            task["queued"] = json!(queued);
            task["status"] = json!(if queued { "queued" } else { "running" });
            task["queue_mode"] = json!("bootstrap");
            task["queued_at"] = if queued {
                json!(artex_core::now())
            } else {
                Value::Null
            };
            for change in &mut b.changes {
                if let Change::Put {
                    collection,
                    id: i,
                    data,
                } = change
                {
                    if collection == "tasks" && i == &id {
                        *data = task.clone();
                    }
                }
            }
            b.result = wire_task(&task, s);
            b.result["source_task_ids"] = parameters
                .get("source_task_ids")
                .cloned()
                .unwrap_or(json!([]));
            artex_domain::conversation::push_event(&mut b, s, c, "TaskCreated", "tasks", &id)?;
            for row in s.records("settings") {
                if matches!(
                    row.data["key"].as_str(),
                    Some("task_concurrency_limit" | "task_concurrency_enabled")
                ) {
                    b.expected.push(artex_core::Expected {
                        collection: "settings".into(),
                        id: row.header.id.clone(),
                        version: row.header.version.clone(),
                    });
                }
            }
            Ok(b)
        }
        "ui.createCompany" => artex_domain::assets_ui::plan(method, p, s, c),
        "ui.controlTask" => {
            if p["action"] != "pause" && p["action"] != "resume" {
                return Err(Error::new(
                    "INVALID_PARAMS",
                    "Task action must be pause or resume",
                ));
            }
            let method = if p["action"] == "pause" {
                "task.pause"
            } else {
                "task.resume"
            };
            let mut b = artex_domain::plan(method, &json!({"task_id":p["id"]}), s, c)?;
            let task = b
                .changes
                .iter()
                .find_map(|ch| match ch {
                    Change::Put {
                        collection,
                        id,
                        data,
                    } if collection == "tasks" && Some(id.as_str()) == p["id"].as_str() => {
                        Some(data.clone())
                    }
                    _ => None,
                })
                .ok_or_else(|| Error::new("EXTERNAL_FAILURE", "Task control omitted its record"))?;
            b.result = json!({"id":p["id"],"paused":task["paused"],"queued":task["queued"],"status":task["status"]});
            Ok(b)
        }
        _ => Err(Error::new("METHOD_NOT_FOUND", "Unknown UI command")),
    }
}
