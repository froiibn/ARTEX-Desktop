use crate::*;
pub const QUERIES: &[&str] = &[
    "ui.taskCategories",
    "ui.taskTemplates",
    "ui.taskConstraints",
    "ui.taskInterceptRules",
    "ui.assetInterceptRules",
];
pub const COMMANDS: &[&str] = &[
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
fn live_task<'a>(s: &'a Snapshot, id: &str) -> Result<&'a Record> {
    let r = record(s, "tasks", id)?;
    if !r.data["deleted_at"].is_null() || !r.data["archived_at"].is_null() {
        return Err(Error::new("NOT_FOUND", "Task is not live"));
    }
    Ok(r)
}
pub fn category(d: &Value, s: &Snapshot) -> Value {
    json!({"id":d["id"],"name":d["name"],"task_count":s.records("tasks").iter().filter(|t|t.data["category_id"]==d["id"]&&t.data["deleted_at"].is_null()).count(),"created_at":d["created_at"],"updated_at":d["updated_at"]})
}
fn template(d: &Value) -> Value {
    let mut d = d.clone();
    d.as_object_mut().unwrap().remove("nkey");
    d
}
fn constraint(d: &Value) -> Value {
    json!({"id":d["id"],"kind":d["kind"],"text":d["text"],"origin":d["origin"].as_str().unwrap_or(""),"ts":d["created_at"]})
}
fn rule(d: &Value, global: bool) -> Value {
    let mut d = d.clone();
    d.as_object_mut().unwrap().remove("task_id");
    if !global {
        d["builtin"] = json!(false);
    }
    d
}
fn stamp(d: &mut Value) {
    let now = artex_core::now();
    if d["created_at"] == "" {
        d["created_at"] = json!(now);
    }
    d["updated_at"] = json!(artex_core::now());
}
pub fn query(m: &str, p: &Value, s: &Snapshot, _: &Context) -> Result<Value> {
    match m {
        "ui.taskCategories" => {
            let mut rows = s.records("task_categories");
            rows.sort_by(|a, b| text(&a.data, "nkey").cmp(text(&b.data, "nkey")));
            Ok(json!(
                rows.iter()
                    .map(|r| category(&r.data, s))
                    .collect::<Vec<_>>()
            ))
        }
        "ui.taskTemplates" => {
            let mut rows = s.records("task_templates");
            rows.sort_by(|a, b| text(&a.data, "nkey").cmp(text(&b.data, "nkey")));
            Ok(json!(
                rows.iter().map(|r| template(&r.data)).collect::<Vec<_>>()
            ))
        }
        "ui.taskConstraints" => {
            let t = live_task(s, required(p, "id")?)?;
            let mut rows = s
                .records("task_constraints")
                .into_iter()
                .filter(|r| r.data["exploration_id"] == t.data["exploration_id"])
                .collect::<Vec<_>>();
            rows.sort_by(|a, b| {
                text(&a.data, "kind")
                    .cmp(text(&b.data, "kind"))
                    .then(a.header.created_at.cmp(&b.header.created_at))
            });
            Ok(json!({"constraints":rows.iter().map(|r| constraint(&r.data)).collect::<Vec<_>>()}))
        }
        "ui.taskInterceptRules" => {
            let id = required(p, "id")?;
            live_task(s, id)?;
            Ok(json!(
                s.records("task_intercept_rules")
                    .iter()
                    .filter(|r| r.data["task_id"] == id)
                    .map(|r| rule(&r.data, false))
                    .collect::<Vec<_>>()
            ))
        }
        "ui.assetInterceptRules" => Ok(json!(
            s.records("asset_intercept_rules")
                .iter()
                .map(|r| rule(&r.data, true))
                .collect::<Vec<_>>()
        )),
        _ => Err(Error::new("METHOD_NOT_FOUND", m)),
    }
}
fn valid_name(p: &Value, key: &str, limit: usize, spaces: bool) -> Result<String> {
    let value = required(p, key)?;
    let name = if spaces {
        value.split_whitespace().collect::<Vec<_>>().join(" ")
    } else {
        value.trim().into()
    };
    if name.chars().count() > limit {
        return Err(Error::new("INVALID_PARAMS", "Text exceeds character limit"));
    }
    Ok(name)
}
pub fn plan(m: &str, p: &Value, s: &Snapshot, c: &Context) -> Result<CommandBatch> {
    let mut b = Planner::new(s, m, p);
    let result = match m {
        "ui.updateTask" => {
            let t = live_task(s, required(p, "id")?)?;
            let input = &p["input"];
            if input.get("name").is_none() && input.get("pinned").is_none() {
                return Err(Error::new("INVALID_PARAMS", "Supply name or pinned"));
            }
            let mut d = t.data.clone();
            if input.get("name").is_some() {
                d["name"] = json!(valid_name(input, "name", 200, false)?);
            }
            if let Some(pin) = input["pinned"].as_bool() {
                if pin && d["pinned_at"].is_null() {
                    d["pinned_at"] = json!(artex_core::now());
                } else if !pin {
                    d["pinned_at"] = Value::Null;
                }
            }
            b.put("tasks", &t.header.id, d.clone());
            d
        }
        "ui.createTaskCategory" | "ui.renameTaskCategory" => {
            let name = valid_name(p, "name", 80, true)?;
            let nkey = name.to_lowercase();
            let existing = if m == "ui.renameTaskCategory" {
                Some(record(s, "task_categories", required(p, "id")?)?)
            } else {
                None
            };
            if s.records("task_categories").iter().any(|r| {
                r.data["nkey"] == nkey && existing.is_none_or(|old| old.header.id != r.header.id)
            }) {
                return Err(Error::new(
                    "VERSION_CONFLICT",
                    "Category name already exists",
                ));
            }
            let mut d = existing
                .map(|r| r.data.clone())
                .unwrap_or(default_data("task_categories")?);
            d["name"] = json!(name);
            d["nkey"] = json!(nkey);
            stamp(&mut d);
            let id = if let Some(r) = existing {
                b.put("task_categories", &r.header.id, d);
                r.header.id.clone()
            } else {
                b.insert("task_categories", "category", d)?
            };
            category(&b.data("task_categories", &id)?, s)
        }
        "ui.deleteTaskCategory" => {
            let id = required(p, "id")?;
            record(s, "task_categories", id)?;
            b.delete("task_categories", id);
            json!({"deleted":id})
        }
        "ui.updateTaskCategory" | "ui.updateTasksCategory" => {
            let category_id = p["categoryId"].clone();
            if let Some(id) = category_id.as_str() {
                record(s, "task_categories", id)?;
            }
            let values = if m == "ui.updateTaskCategory" {
                vec![required(p, "taskId")?.to_owned()]
            } else {
                ids(p, "taskIds")
            };
            let mut seen = BTreeSet::new();
            let values = values
                .into_iter()
                .filter(|id| seen.insert(id.clone()))
                .collect::<Vec<_>>();
            if values.is_empty() || values.len() > 100 {
                return Err(Error::new(
                    "INVALID_PARAMS",
                    "Task category batch requires 1..100 IDs",
                ));
            }
            let mut items = vec![];
            for id in values {
                match live_task(s, &id) {
                    Ok(t) => {
                        let mut d = t.data.clone();
                        d["category_id"] = category_id.clone();
                        b.put("tasks", &id, d);
                        items.push(json!({"id":id,"ok":true}));
                    }
                    Err(e) => {
                        if m == "ui.updateTaskCategory" {
                            return Err(e);
                        }
                        items.push(json!({"id":id,"ok":false,"error":e.message}));
                    }
                }
            }
            let mut cat = category_id
                .as_str()
                .map(|id| category(&record(s, "task_categories", id).unwrap().data, s))
                .unwrap_or(Value::Null);
            if !cat.is_null() {
                let staged=b.batch.changes.iter().filter(|ch|matches!(ch,Change::Put{collection,data,..}if collection=="tasks"&&data["category_id"]==category_id)).count();
                let removed=s.records("tasks").iter().filter(|t|t.data["category_id"]==category_id&&b.batch.changes.iter().any(|ch|matches!(ch,Change::Put{collection,id,..}if collection=="tasks"&&id==&t.header.id))).count();
                cat["task_count"] =
                    json!(cat["task_count"].as_u64().unwrap_or(0) + staged as u64 - removed as u64);
            }
            if m == "ui.updateTaskCategory" {
                b.data("tasks", required(p, "taskId")?)?
            } else {
                json!({"items":items,"category":cat})
            }
        }
        "ui.createTaskTemplate" | "ui.updateTaskTemplate" => {
            let existing = if m == "ui.updateTaskTemplate" {
                Some(record(s, "task_templates", required(p, "id")?)?)
            } else {
                None
            };
            let input = &p["input"];
            if existing.is_some() && input.as_object().is_none_or(|o| o.is_empty()) {
                return Err(Error::new("INVALID_PARAMS", "Template patch is empty"));
            }
            let mut d = existing
                .map(|r| r.data.clone())
                .unwrap_or(default_data("task_templates")?);
            merge(&mut d, input);
            for k in ["name", "description", "goal"] {
                d[k] = json!(valid_name(
                    &d,
                    k,
                    if k == "name" { 120 } else { 16000 },
                    k == "name"
                )?);
            }
            d["nkey"] = json!(text(&d, "name").to_lowercase());
            if s.records("task_templates").iter().any(|r| {
                r.data["nkey"] == d["nkey"]
                    && existing.is_none_or(|old| old.header.id != r.header.id)
            }) {
                return Err(Error::new(
                    "VERSION_CONFLICT",
                    "Template name already exists",
                ));
            }
            if let Some(id) = d["category_id"].as_str() {
                record(s, "task_categories", id)?;
            }
            if let Some(rules) = input["intercept_rules"].as_array() {
                d["intercept_rules"] = json!(
                    rules
                        .iter()
                        .map(|r| tasks::validate_asset_rule(r, true))
                        .collect::<Result<Vec<_>>>()?
                );
            }
            stamp(&mut d);
            let id = if let Some(r) = existing {
                b.put("task_templates", &r.header.id, d);
                r.header.id.clone()
            } else {
                b.insert("task_templates", "template", d)?
            };
            template(&b.data("task_templates", &id)?)
        }
        "ui.deleteTaskTemplate" => {
            let id = required(p, "id")?;
            record(s, "task_templates", id)?;
            b.delete("task_templates", id);
            json!({"deleted":id})
        }
        "ui.addConstraint" | "ui.updateConstraint" | "ui.deleteConstraint" => {
            let t = live_task(s, required(p, "id")?)?;
            let existing = if m != "ui.addConstraint" {
                Some(record(s, "task_constraints", required(p, "constraintId")?)?)
            } else {
                None
            };
            if existing.is_some_and(|r| r.data["exploration_id"] != t.data["exploration_id"]) {
                return Err(Error::new(
                    "NOT_FOUND",
                    "Constraint does not belong to this task",
                ));
            }
            if m == "ui.deleteConstraint" {
                b.delete("task_constraints", &existing.unwrap().header.id);
                json!({"ok":true})
            } else {
                let text = valid_name(p, "text", 16000, false)?;
                let kind = required(p, "kind")?;
                if !matches!(kind, "allow" | "deny") {
                    return Err(Error::new("INVALID_PARAMS", "Invalid constraint kind"));
                }
                let mut d = existing
                    .map(|r| r.data.clone())
                    .unwrap_or(default_data("task_constraints")?);
                d["text"] = json!(text);
                d["kind"] = json!(kind);
                d["exploration_id"] = t.data["exploration_id"].clone();
                if existing.is_none() {
                    d["origin"] = json!("human");
                }
                stamp(&mut d);
                let id = if let Some(r) = existing {
                    b.put("task_constraints", &r.header.id, d);
                    r.header.id.clone()
                } else {
                    b.insert("task_constraints", "constraint", d)?
                };
                constraint(&b.data("task_constraints", &id)?)
            }
        }
        m if m.contains("TaskInterceptRule") || m.contains("AssetInterceptRule") => {
            let global = m.contains("Asset");
            let col = if global {
                "asset_intercept_rules"
            } else {
                "task_intercept_rules"
            };
            let existing = if m.contains("create") {
                None
            } else {
                Some(record(
                    s,
                    col,
                    required(p, if global { "id" } else { "ruleId" })?,
                )?)
            };
            let tid = if global {
                None
            } else {
                Some(required(p, "id")?)
            };
            if let Some(tid) = tid {
                live_task(s, tid)?;
                if existing.is_some_and(|r| r.data["task_id"] != tid) {
                    return Err(Error::new("NOT_FOUND", "Rule does not belong to task"));
                }
            }
            if m.contains("delete") {
                let r = existing.unwrap();
                b.delete(col, &r.header.id);
                if global {
                    json!({"deleted":r.header.id})
                } else {
                    json!({"deleted":true})
                }
            } else if m.contains("toggle") {
                let r = existing.unwrap();
                let mut d = r.data.clone();
                d["enabled"] = p["enabled"].clone();
                b.put(col, &r.header.id, d);
                json!({"ok":true,"enabled":p["enabled"]})
            } else {
                let mut d = existing
                    .map(|r| r.data.clone())
                    .unwrap_or(default_data(col)?);
                merge(&mut d, &tasks::validate_asset_rule(&p["rule"], !global)?);
                if let Some(tid) = tid {
                    d["task_id"] = json!(tid);
                }
                stamp(&mut d);
                let id = if let Some(r) = existing {
                    b.put(col, &r.header.id, d);
                    r.header.id.clone()
                } else {
                    b.insert(col, if global { "assetrule" } else { "taskrule" }, d)?
                };
                rule(&b.data(col, &id)?, global)
            }
        }
        _ => return Err(Error::new("METHOD_NOT_FOUND", m)),
    };
    if let Some(Change::Put { collection, id, .. } | Change::Delete { collection, id }) =
        b.batch.changes.first().cloned()
    {
        conversation::push_event(
            &mut b.batch,
            s,
            c,
            "TaskConfigurationChanged",
            &collection,
            &id,
        )?;
    }
    let mut batch = b.done(result);
    augment_expected(&mut batch, s)?;
    Ok(batch)
}
