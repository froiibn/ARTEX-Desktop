use super::schema;
use artex_core::{Expected, Snapshot, default_data};
use serde_json::{Value, json};

pub(super) fn collection_alias(name: &str) -> &str {
    match name {
        "profiles" => "llm_profiles",
        "categories" => "task_categories",
        "templates" => "task_templates",
        "constraints" => "task_constraints",
        "notifications" => "notification_channels",
        "intercept" => "intercept_rules",
        _ => name,
    }
}
pub(super) fn initial(
    collection: &str,
    selected: Option<&str>,
    snapshot: &Snapshot,
) -> Option<(String, Value)> {
    let row = selected.and_then(|id| snapshot.get(collection, id));
    if selected.is_some()
        && row.is_none()
        && !matches!(collection, "skills" | "settings" | "agent_visibility")
    {
        return None;
    }
    let mut data = row
        .map(|r| r.data.clone())
        .unwrap_or_else(|| default_data(collection).unwrap_or(json!({})));
    if let Some(row) = row {
        data["id"] = json!(row.header.id);
    }
    let existing = row.is_some();
    if !existing {
        data.as_object_mut().map(|o| o.remove("id"));
    }
    let task = row.and_then(|r| {
        snapshot
            .records("tasks")
            .into_iter()
            .find(|t| t.data["exploration_id"] == r.data["exploration_id"])
            .map(|r| r.header.id.clone())
    });
    let (operation, params) = match collection {
        "llm_profiles" => {
            let mut p = data.clone();
            let mut retry = json!({});
            for layer in ["connect", "empty", "stream"] {
                retry[layer] = json!({"attempts":data[format!("retry_{layer}_attempts")],"interval_ms":data[format!("retry_{layer}_interval_ms")]});
            }
            p["retry"] = retry;
            ("ui.saveLLMProfile", json!({"p":p}))
        }
        "agents" if existing => (
            "ui.saveAgentConfig",
            json!({"key":data["key"],"patch":data}),
        ),
        "agents" => (
            "ui.createAgent",
            json!({"key":"","name":"","description":""}),
        ),
        "tools" if data["system"] == true => {
            ("ui.saveTool", json!({"key":data["key"],"patch":data}))
        }
        "tools" if existing => ("ui.updateCustomTool", json!({"key":data["key"],"t":data})),
        "tools" => {
            data["kind"] = json!("command");
            data["schema"] = json!({"type":"object","properties":{}});
            ("ui.createCustomTool", json!({"t":data}))
        }
        "mcp_servers" => {
            if !existing {
                data["name"] = json!("");
                data["transport"] = json!("stdio");
                data["enabled"] = json!(true);
            }
            ("ui.saveMcpServer", json!({"m":data}))
        }
        "task_categories" if existing => (
            "ui.renameTaskCategory",
            json!({"id":data["id"],"name":data["name"]}),
        ),
        "task_categories" => ("ui.createTaskCategory", json!({"name":""})),
        "task_templates" if existing => (
            "ui.updateTaskTemplate",
            json!({"id":data["id"],"input":data}),
        ),
        "task_templates" => ("ui.createTaskTemplate", json!({"input":data})),
        "task_constraints" if existing => (
            "ui.updateConstraint",
            json!({"id":task,"constraintId":data["id"],"text":data["text"],"kind":data["kind"]}),
        ),
        "task_constraints" => ("ui.addConstraint", json!({"id":"","text":"","kind":"deny"})),
        "intercept_rules" if existing => (
            "ui.updateInterceptRule",
            json!({"id":data["id"],"rule":data}),
        ),
        "intercept_rules" => ("ui.createInterceptRule", json!({"rule":data})),
        "asset_intercept_rules" if existing => (
            "ui.updateAssetInterceptRule",
            json!({"id":data["id"],"rule":data}),
        ),
        "asset_intercept_rules" => ("ui.createAssetInterceptRule", json!({"rule":data})),
        "task_intercept_rules" if existing => (
            "ui.updateTaskInterceptRule",
            json!({"id":data["task_id"],"ruleId":data["id"],"rule":data}),
        ),
        "task_intercept_rules" => ("ui.createTaskInterceptRule", json!({"id":"","rule":data})),
        "notification_channels" if existing => (
            "ui.notifyUpdateChannel",
            json!({"id":data["id"],"payload":data}),
        ),
        "notification_channels" => {
            data["kind"] = json!("email");
            data["mode"] = json!("realtime");
            data["enabled"] = json!(true);
            ("ui.notifyCreateChannel", json!({"payload":data}))
        }
        "settings" => ("ui.setSettings", json!({"patch":{}})),
        "skills" if selected.is_some() => {
            ("ui.updateSkillMeta", json!({"name":selected,"meta":{}}))
        }
        "skills" => (
            "ui.createSkill",
            json!({"s":{"name":"","description":"","license":"","compatibility":"","mcps":[],"instructions":{"text":"","blob":null}}}),
        ),
        "agent_visibility" => {
            let agent = selected.and_then(|id| {
                snapshot.get("agents", id).or_else(|| {
                    snapshot.get("agent_visibility", id).and_then(|v| {
                        v.data["agent_id"]
                            .as_str()
                            .and_then(|id| snapshot.get("agents", id))
                    })
                })
            });
            let key = agent.map(|a| a.data["key"].clone()).unwrap_or(json!(""));
            let aid = agent.map(|a| a.header.id.as_str()).unwrap_or("");
            (
                "ui.setAgentVisibility",
                json!({"key":key,"mcp":snapshot.records("agent_visibility").iter().filter(|v|v.data["agent_id"]==aid&&v.data["resource_kind"]=="mcp"&&v.data["enabled"]==true).map(|v|v.data["resource_id"].clone()).collect::<Vec<_>>(),"skill":snapshot.records("agent_skill_visibility").iter().filter(|v|v.data["agent_id"]==aid&&v.data["enabled"]==true).map(|v|v.data["skill_name"].clone()).collect::<Vec<_>>()}),
            )
        }
        _ => return None,
    };
    let params = schema::shape(&params, &schema::method(operation));
    Some((operation.into(), params))
}
pub(super) fn baseline(
    collection: &str,
    selected: Option<&str>,
    snapshot: &Snapshot,
) -> Vec<Expected> {
    let tables: &[&str] = match collection {
        "agents" => &[
            "agents",
            "agent_prompts",
            "agent_triggers",
            "agent_visibility",
            "agent_skill_visibility",
            "tools",
            "llm_profiles",
            "mcp_servers",
        ],
        "llm_profiles" => &["llm_profiles"],
        "tools" => &["tools", "agents"],
        "mcp_servers" | "agent_visibility" => &[
            "mcp_servers",
            "agents",
            "agent_visibility",
            "agent_skill_visibility",
        ],
        "task_templates" => &["task_templates", "task_categories"],
        "task_categories" => &["task_categories", "tasks"],
        "task_constraints" => &["task_constraints", "tasks", "explorations"],
        "task_intercept_rules" => &["task_intercept_rules", "tasks"],
        "notification_channels" => &["notification_channels"],
        "settings" => &["settings"],
        "intercept_rules" => &["intercept_rules"],
        "asset_intercept_rules" => &["asset_intercept_rules"],
        _ => &[],
    };
    tables
        .iter()
        .flat_map(|table| snapshot.records(table))
        .filter(|row| match row.header.collection.as_str() {
            "agent_prompts" | "agent_triggers" | "agent_visibility" | "agent_skill_visibility"
                if collection == "agents" =>
            {
                selected.is_none_or(|id| row.data["agent_id"] == id)
            }
            c if c == collection && selected.is_some() && collection != "llm_profiles" => {
                selected == Some(row.header.id.as_str())
            }
            "agents" if collection == "agents" => selected == Some(row.header.id.as_str()),
            _ => true,
        })
        .map(|r| Expected {
            collection: r.header.collection.clone(),
            id: r.header.id.clone(),
            version: r.header.version.clone(),
        })
        .collect()
}
pub(super) fn delete(collection: &str, data: &Value) -> Option<(String, Value)> {
    let id = data["id"].clone();
    let (op, p) = match collection {
        "llm_profiles" => ("ui.deleteLLMProfile", json!({"id":id})),
        "agents" if data["builtin"] != true => ("ui.deleteAgent", json!({"key":data["key"]})),
        "tools" if data["system"] != true => ("ui.deleteCustomTool", json!({"key":data["key"]})),
        "mcp_servers" => ("ui.deleteMcpServer", json!({"id":id})),
        "task_categories" => ("ui.deleteTaskCategory", json!({"id":id})),
        "task_templates" => ("ui.deleteTaskTemplate", json!({"id":id})),
        "intercept_rules" => ("ui.deleteInterceptRule", json!({"id":id})),
        "asset_intercept_rules" => ("ui.deleteAssetInterceptRule", json!({"id":id})),
        "notification_channels" => ("ui.notifyDeleteChannel", json!({"id":id})),
        _ => return None,
    };
    Some((op.into(), p))
}
