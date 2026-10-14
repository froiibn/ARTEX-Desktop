#![forbid(unsafe_code)]
//! Domain changes are plans against an immutable snapshot; only Store commits them.
pub mod asset_graph_ui;
pub mod assets;
pub mod assets_ui;
pub mod bootstrap;
pub mod tool_projection;
pub use bootstrap::bootstrap_plan;
pub mod approval_bypass;
pub mod auxiliary_ui;
pub mod configuration;
pub mod configuration_ui;
pub mod conversation;
pub mod exploration;
pub mod exploration_ui;
pub mod findings;
pub mod findings_ui;
pub mod llm_runtime;
pub mod llm_ui;
pub mod logs_ui;
pub mod mentions_ui;
pub mod native_lifecycle;
pub mod policy;
pub mod policy_ui;
pub mod prompt_template;
pub mod psl;
pub mod report_export;
pub mod sdk;
pub mod side_questions;
pub mod skills_ui;
pub mod sync_scopesentry;
pub mod task_control_ui;
pub mod task_delete_ui;
pub mod task_ui;
pub mod tasks;
pub mod traffic_ui;
pub mod transcript_ui;
pub mod trigger_merge;

use artex_core::{
    Change, CommandBatch, Context, Error, Expected, Record, Result, Snapshot, default_data, new_id,
};
use serde_json::{Value, json};
use std::collections::BTreeSet;

pub fn text<'a>(v: &'a Value, key: &str) -> &'a str {
    v[key].as_str().unwrap_or("")
}
pub fn required<'a>(v: &'a Value, key: &str) -> Result<&'a str> {
    let s = text(v, key);
    if s.trim().is_empty() {
        Err(Error::new("INVALID_PARAMS", &format!("{key} is required")))
    } else {
        Ok(s)
    }
}
pub fn ids(v: &Value, key: &str) -> Vec<String> {
    v[key]
        .as_array()
        .map(|a| {
            a.iter()
                .filter_map(|i| i.as_str().map(String::from))
                .collect()
        })
        .unwrap_or_default()
}
pub fn record<'a>(s: &'a Snapshot, c: &str, id: &str) -> Result<&'a Record> {
    s.get(c, id)
        .ok_or_else(|| Error::new("NOT_FOUND", &format!("{c}/{id} not found")))
}
pub fn task_id<'a>(p: &'a Value, ctx: &'a Context) -> Result<&'a str> {
    p["task_id"]
        .as_str()
        .or(ctx.task_id.as_deref())
        .ok_or_else(|| Error::new("INVALID_PARAMS", "task context is required"))
}
pub fn exploration_id(s: &Snapshot, id: &str) -> Result<String> {
    Ok(required(&record(s, "tasks", id)?.data, "exploration_id")?.to_owned())
}
pub fn direct_sources(s: &Snapshot, task: &str) -> BTreeSet<String> {
    let mut set = BTreeSet::from([task.to_owned()]);
    for r in s.records("task_relations") {
        if text(&r.data, "task_id") == task {
            let source = text(&r.data, "source_task_id");
            if s.get("tasks", source)
                .is_some_and(|t| t.data["deleted_at"].is_null() && t.data["archived_at"].is_null())
            {
                set.insert(source.to_owned());
            }
        }
    }
    set
}
pub fn visible_explorations(s: &Snapshot, task: &str) -> BTreeSet<String> {
    direct_sources(s, task)
        .iter()
        .filter_map(|t| {
            s.get("tasks", t)
                .and_then(|r| r.data["exploration_id"].as_str())
                .map(String::from)
        })
        .collect()
}
pub fn own_node<'a>(s: &'a Snapshot, task: &str, id: &str) -> Result<&'a Record> {
    let n = record(s, "exploration_nodes", id)?;
    if text(&n.data, "exploration_id") != exploration_id(s, task)? {
        return Err(Error::new(
            "PERMISSION_DENIED",
            "source tasks are read only",
        ));
    }
    Ok(n)
}
pub fn visible_node<'a>(s: &'a Snapshot, task: &str, id: &str) -> Result<&'a Record> {
    let n = record(s, "exploration_nodes", id)?;
    if !visible_explorations(s, task).contains(text(&n.data, "exploration_id")) {
        return Err(Error::new(
            "PERMISSION_DENIED",
            "node is outside direct shared context",
        ));
    }
    Ok(n)
}
pub fn merge(data: &mut Value, patch: &Value) {
    if let (Some(d), Some(p)) = (data.as_object_mut(), patch.as_object()) {
        for (k, v) in p {
            if d.contains_key(k) {
                d.insert(k.clone(), v.clone());
            }
        }
    }
}
pub fn setting_bool(state: &Snapshot, key: &str, default: bool) -> bool {
    state
        .records("settings")
        .into_iter()
        .find(|r| text(&r.data, "key") == key)
        .map(|r| matches!(text(&r.data, "value"), "true" | "1"))
        .unwrap_or(default)
}
pub fn empty_batch(method: &str, p: &Value) -> CommandBatch {
    CommandBatch {
        command_id: String::new(),
        method: method.to_owned(),
        normalized_params: p.clone(),
        expected: vec![],
        changes: vec![],
        events: vec![],
        deliveries: vec![],
        result: Value::Null,
    }
}
/// Project a pure plan for its response; the Store remains the only writer.
pub fn projected(s: &Snapshot, b: &CommandBatch) -> Result<Snapshot> {
    let mut next = s.clone();
    let changes = artex_core::expand_deletions(s, &b.changes)?;
    for change in changes {
        match change {
            Change::Delete { collection, id } => {
                if let Some(rows) = next.collections.get_mut(&collection) {
                    rows.remove(&id);
                }
            }
            Change::Put {
                collection,
                id,
                mut data,
            } => {
                let old = s.get(&collection, &id);
                let stamp = artex_core::now();
                if data.get("created_at").is_some() && data["created_at"] == "" {
                    data["created_at"] = json!(
                        old.map(|r| r.header.created_at.clone())
                            .unwrap_or(stamp.clone())
                    );
                }
                if data.get("updated_at").is_some() {
                    data["updated_at"] = json!(stamp);
                }
                let version = old
                    .map(|r| r.header.version.parse::<u64>().unwrap_or(0) + 1)
                    .unwrap_or(1);
                let row = Record {
                    header: artex_core::Header {
                        id: id.clone(),
                        collection: collection.clone(),
                        version: version.to_string(),
                        commit_seq: (s.commit_seq + 1).to_string(),
                        created_at: old
                            .map(|r| r.header.created_at.clone())
                            .unwrap_or(stamp.clone()),
                        updated_at: stamp,
                        legacy_key: old
                            .map(|r| r.header.legacy_key.clone())
                            .unwrap_or(Value::Null),
                    },
                    data,
                };
                next.collections
                    .entry(collection)
                    .or_default()
                    .insert(id, row);
            }
        }
    }
    Ok(next)
}
pub struct Planner<'a> {
    pub state: &'a Snapshot,
    pub batch: CommandBatch,
}
impl<'a> Planner<'a> {
    pub fn new(state: &'a Snapshot, method: &str, p: &Value) -> Self {
        Self {
            state,
            batch: empty_batch(method, p),
        }
    }
    pub fn expect(&mut self, row: &Record) {
        if !self
            .batch
            .expected
            .iter()
            .any(|e| e.collection == row.header.collection && e.id == row.header.id)
        {
            self.batch.expected.push(Expected {
                collection: row.header.collection.clone(),
                id: row.header.id.clone(),
                version: row.header.version.clone(),
            });
        }
    }
    pub fn insert(&mut self, c: &str, prefix: &str, patch: Value) -> Result<String> {
        let id = new_id(prefix);
        let mut d = default_data(c)?;
        merge(&mut d, &patch);
        if d.get("id").is_some() {
            d["id"] = json!(id);
        }
        self.put(c, &id, d);
        Ok(id)
    }
    pub fn put(&mut self, c: &str, id: &str, data: Value) {
        self.batch.changes.retain(|ch|!matches!(ch,Change::Put{collection,id:i,..}|Change::Delete{collection,id:i} if collection==c&&i==id));
        if let Some(r) = self.state.get(c, id) {
            if !self
                .batch
                .expected
                .iter()
                .any(|e| e.collection == c && e.id == id)
            {
                self.batch.expected.push(Expected {
                    collection: c.to_owned(),
                    id: id.to_owned(),
                    version: r.header.version.clone(),
                });
            }
        }
        self.batch.changes.push(Change::Put {
            collection: c.to_owned(),
            id: id.to_owned(),
            data,
        });
    }
    pub fn delete(&mut self, c: &str, id: &str) {
        self.batch.changes.push(Change::Delete {
            collection: c.into(),
            id: id.into(),
        });
        if let Some(r) = self.state.get(c, id) {
            self.batch.expected.push(Expected {
                collection: c.into(),
                id: id.into(),
                version: r.header.version.clone(),
            });
        }
    }
    pub fn data(&self, c: &str, id: &str) -> Result<Value> {
        for ch in self.batch.changes.iter().rev() {
            if let Change::Put {
                collection,
                id: i,
                data,
            } = ch
            {
                if collection == c && i == id {
                    return Ok(data.clone());
                }
            }
        }
        Ok(record(self.state, c, id)?.data.clone())
    }
    pub fn done(mut self, result: Value) -> CommandBatch {
        // Include optimistic versions of objects used to decide this plan, including
        // referenced source tasks/captures and permission/scope configuration.
        fn strings(v: &Value, out: &mut BTreeSet<String>) {
            match v {
                Value::String(s) => {
                    out.insert(s.clone());
                }
                Value::Array(a) => {
                    for v in a {
                        strings(v, out);
                    }
                }
                Value::Object(o) => {
                    for v in o.values() {
                        strings(v, out);
                    }
                }
                _ => {}
            }
        }
        let mut refs = BTreeSet::new();
        strings(&self.batch.normalized_params, &mut refs);
        for ch in &self.batch.changes {
            if let Change::Put { data, .. } = ch {
                strings(data, &mut refs);
            }
        }
        for (collection, records) in &self.state.collections {
            for r in records.values() {
                let legacy = text(&r.data, "id");
                if refs.contains(&r.header.id) || (!legacy.is_empty() && refs.contains(legacy)) {
                    if !self
                        .batch
                        .expected
                        .iter()
                        .any(|e| e.collection == *collection && e.id == r.header.id)
                    {
                        self.batch.expected.push(Expected {
                            collection: collection.clone(),
                            id: r.header.id.clone(),
                            version: r.header.version.clone(),
                        });
                    }
                }
            }
        }
        self.batch.result = result;
        self.batch
    }
}

pub fn is_mutation(m: &str) -> bool {
    conversation::COMMANDS.contains(&m)
        || matches!(
            m,
            "task.create"
                | "task.pause"
                | "task.resume"
                | "task.recover_budget"
                | "task.sources.replace"
                | "company.create"
                | "company.update"
                | "company.delete"
                | "asset.upsert"
                | "asset.delete"
                | "finding.retest.create"
                | "finding.update"
                | "finding.delete"
                | "question.answer"
                | "agent.features.update"
                | "spawn_task"
                | "pause_task"
                | "add_task_hint"
                | "insert_assets"
                | "add_company_scope"
                | "add_task_scope"
                | "delete_assets_by_host"
                | "add_intent"
                | "set_goals"
                | "set_constraints"
                | "record_fact"
                | "report_finding"
                | "add_hint"
                | "prove_goal"
                | "goal_met"
                | "kill_work"
                | "steer_work"
                | "bind_finding_traffic"
                | "update_finding_report"
                | "record_finding_retest_result"
                | "create_custom_tool"
                | "update_custom_tool"
                | "create_mcp"
                | "update_mcp"
                | "TodoWrite"
                | "RecordMemory"
                | "EnterPlanMode"
                | "ExitPlanMode"
                | "AskUserQuestion"
        )
}
pub fn supports(m: &str) -> bool {
    is_mutation(m)
        || conversation::QUERIES.contains(&m)
        || matches!(
            m,
            "task.list"
                | "task.get"
                | "company.list"
                | "asset.list"
                | "finding.list"
                | "finding.get"
                | "list_tasks"
                | "list_llm_profiles"
                | "list_companies"
                | "list_assets"
                | "list_untested_assets"
                | "graph_overview"
                | "get_task_graph"
                | "node_detail"
                | "get_task_node_detail"
                | "list_facts"
                | "list_findings"
                | "list_task_findings"
                | "list_goals"
                | "expand_digest"
                | "get_worker_trace"
                | "get_task_worker_trace"
                | "list_task_worker_traces"
                | "get_worker_output"
                | "search_all_worker_traces"
                | "search_task_worker_traces"
                | "get_finding_traffic"
                | "get_finding_retest_context"
                | "traffic_search"
                | "traffic_get"
                | "RecallMemory"
                | "SearchExtraTools"
        )
}
pub fn plan(m: &str, p: &Value, s: &Snapshot, c: &Context) -> Result<CommandBatch> {
    if !p.is_object() {
        return Err(Error::new("INVALID_PARAMS", "parameters must be an object"));
    }
    let mut batch = match m {
        m if conversation::COMMANDS.contains(&m) => conversation::plan(m, p, s, c),
        "task.create"
        | "spawn_task"
        | "task.pause"
        | "pause_task"
        | "task.resume"
        | "task.recover_budget"
        | "task.sources.replace" => tasks::plan(m, p, s, c),
        "company.create"
        | "company.update"
        | "company.delete"
        | "asset.upsert"
        | "asset.delete"
        | "insert_assets"
        | "add_company_scope"
        | "add_task_scope"
        | "delete_assets_by_host" => assets::plan(m, p, s, c),
        "report_finding"
        | "bind_finding_traffic"
        | "update_finding_report"
        | "finding.retest.create"
        | "record_finding_retest_result"
        | "finding.update"
        | "finding.delete" => findings::plan(m, p, s, c),
        "create_custom_tool" | "update_custom_tool" | "create_mcp" | "update_mcp" => {
            configuration::plan(m, p, s, c)
        }
        "TodoWrite"
        | "RecordMemory"
        | "EnterPlanMode"
        | "ExitPlanMode"
        | "AskUserQuestion"
        | "question.answer"
        | "agent.features.update" => sdk::plan(m, p, s, c),
        _ => exploration::plan(m, p, s, c),
    }?;
    let mut reads = BTreeSet::new();
    if let Some(t) = c.task_id.as_deref() {
        for task in direct_sources(s, t) {
            reads.insert(("tasks".to_owned(), task));
        }
    }
    for (collection, id) in [
        ("agents", c.actor.agent_id.as_deref()),
        ("agent_runs", c.actor.run_id.as_deref()),
    ] {
        if let Some(id) = id {
            reads.insert((collection.to_owned(), id.to_owned()));
        }
    }
    for collection in ["agent_features", "tools", "task_scope", "task_relations"] {
        for r in s.records(collection) {
            let relevant = match collection {
                "agent_features" => r.data["agent_id"].as_str() == c.actor.agent_id.as_deref(),
                "tools" => text(&r.data, "key") == m,
                "task_scope" | "task_relations" => c
                    .task_id
                    .as_deref()
                    .is_some_and(|t| direct_sources(s, t).contains(text(&r.data, "task_id"))),
                _ => false,
            };
            if relevant {
                reads.insert((collection.to_owned(), r.header.id.clone()));
            }
        }
    }
    for (collection, id) in reads {
        if let Some(r) = s.get(&collection, &id) {
            if !batch
                .expected
                .iter()
                .any(|e| e.collection == collection && e.id == id)
            {
                batch.expected.push(Expected {
                    collection,
                    id,
                    version: r.header.version.clone(),
                });
            }
        }
    }
    augment_expected(&mut batch, s)?;
    Ok(batch)
}
pub fn augment_expected(batch: &mut CommandBatch, state: &Snapshot) -> Result<()> {
    for change in artex_core::expand_deletions(state, &batch.changes)? {
        let (c, id) = change.key();
        if let Some(r) = state.get(c, id) {
            if !batch
                .expected
                .iter()
                .any(|e| e.collection == c && e.id == id)
            {
                batch.expected.push(Expected {
                    collection: c.into(),
                    id: id.into(),
                    version: r.header.version.clone(),
                });
            }
        }
    }
    Ok(())
}
pub fn query(m: &str, p: &Value, s: &Snapshot, c: &Context) -> Result<Value> {
    match m {
        m if conversation::QUERIES.contains(&m) => conversation::query(m, p, s, c),
        "task.list" | "task.get" | "list_tasks" | "list_llm_profiles" => tasks::query(m, p, s, c),
        "company.list"
        | "asset.list"
        | "list_companies"
        | "list_assets"
        | "list_untested_assets" => assets::query(m, p, s, c),
        "finding.list"
        | "finding.get"
        | "list_findings"
        | "list_task_findings"
        | "get_finding_traffic"
        | "get_finding_retest_context"
        | "traffic_search"
        | "traffic_get" => findings::query(m, p, s, c),
        "RecallMemory" | "SearchExtraTools" => sdk::query(m, p, s, c),
        _ => exploration::query(m, p, s, c),
    }
}
