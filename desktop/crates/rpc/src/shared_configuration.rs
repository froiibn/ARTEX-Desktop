//! Portable configuration uses semantic keys; workspace object IDs and histories stay local.
//! Bodies remain immutable references. The catalog owner must copy their verified blobs
//! before persisting this DTO or committing its projection into another workspace.
use artex_core::{Change, Error, Record, Result, Snapshot, default_data, new_id};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::collections::{BTreeMap, BTreeSet};

const MARKER_KEY: &str = "native.shared_configuration.v1";
const AGENT_FIELDS: &[&str] = &[
    "key",
    "name",
    "description",
    "role",
    "builtin",
    "enabled",
    "llm_profile_id",
    "max_turns",
    "run_seconds",
    "web_search",
    "interactive_shell",
    "wrapup_prompt",
    "wrapup_max_turns",
    "task_timeout_wrapup_prompt",
    "task_timeout_wrapup_max_turns",
    "trigger_run_mode",
    "trigger_merge_mode",
    "trigger_max_parallel",
];
const TOOL_FIELDS: &[&str] = &[
    "key",
    "system",
    "description",
    "schema",
    "agents",
    "enabled",
    "kind",
    "exec",
    "deferred",
];
const MCP_FIELDS: &[&str] = &[
    "name",
    "transport",
    "command",
    "args",
    "env",
    "url",
    "enabled",
    "insecure",
];
const NOTIFICATION_FIELDS: &[&str] = &[
    "name",
    "kind",
    "enabled",
    "config",
    "mode",
    "filter",
    "rate_per_min",
];
const INTERCEPT_FIELDS: &[&str] = &[
    "name",
    "enabled",
    "priority",
    "match_target",
    "match_type",
    "pattern",
    "action",
    "message",
    "timeout_enabled",
    "timeout_seconds",
    "timeout_action",
];
const TRIGGER_FIELDS: &[&str] = &[
    "agent_key",
    "enabled",
    "interval_sec",
    "on_finding",
    "on_goal_met",
    "on_task_timeout",
    "on_tool_call",
    "on_task_create",
    "interval_message",
    "finding_message",
    "goal_message",
    "task_timeout_message",
    "tool_call_message",
    "task_create_message",
    "tool_names",
];

#[derive(Debug, Clone, Default, Deserialize, Serialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub(super) struct SharedConfiguration {
    pub(super) agents: BTreeMap<String, AgentConfiguration>,
    pub(super) tools: BTreeMap<String, Value>,
    pub(super) mcps: BTreeMap<String, Value>,
    /// Channels already have a portable native identity. Their delivery records,
    /// attempts, rate windows, and network actions never enter configuration.
    #[serde(default)]
    pub(super) notification_channels: BTreeMap<String, Value>,
    #[serde(default)]
    pub(super) intercept_rules: BTreeMap<String, InterceptConfiguration>,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub(super) struct InterceptConfiguration {
    data: Value,
    /// Equal-priority allow/deny evaluation uses the original header creation
    /// time. Replacing it with the projection commit time changes policy meaning.
    created_at: String,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub(super) struct AgentConfiguration {
    data: Value,
    prompt: Option<Value>,
    variables: BTreeMap<String, Value>,
    visibility: Vec<McpVisibility>,
    skills: BTreeMap<String, bool>,
    features: Option<Value>,
    triggers: BTreeMap<String, Value>,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq, PartialOrd, Ord)]
#[serde(deny_unknown_fields)]
struct McpVisibility {
    server: String,
    tool: String,
    enabled: bool,
}

/// Trigger activation keys contain local IDs. Aliases prevent imported default
/// triggers from duplicating an equivalent local trigger or invalidating history.
#[derive(Default, Deserialize, Serialize, PartialEq)]
#[serde(deny_unknown_fields)]
struct Marker {
    trigger_ids: BTreeMap<String, String>,
}

fn corrupt(message: &str) -> Error {
    Error::new("CORRUPT_STORE", message)
}

fn fields(data: &Value, names: &[&str]) -> Result<Value> {
    let mut out = serde_json::Map::new();
    for name in names {
        out.insert(
            (*name).into(),
            data.get(*name).cloned().ok_or_else(|| {
                corrupt("Shared configuration is missing a required configuration field")
            })?,
        );
    }
    Ok(Value::Object(out))
}

fn string<'a>(data: &'a Value, name: &str) -> Result<&'a str> {
    data[name]
        .as_str()
        .ok_or_else(|| corrupt("Invalid shared configuration identity"))
}

fn marker(s: &Snapshot) -> Result<Marker> {
    s.records("settings")
        .into_iter()
        .find(|r| r.data["key"] == MARKER_KEY)
        .map(|r| {
            serde_json::from_str(
                r.data["value"]
                    .as_str()
                    .ok_or_else(|| corrupt("Invalid shared configuration aliases"))?,
            )
            .map_err(Error::from)
        })
        .transpose()
        .map(|v| v.unwrap_or_default())
}

fn keyed<'a>(s: &'a Snapshot, col: &str, field: &str) -> Result<BTreeMap<String, &'a Record>> {
    let mut out = BTreeMap::new();
    for r in s.records(col) {
        if out.insert(string(&r.data, field)?.to_owned(), r).is_some() {
            return Err(corrupt("Duplicate semantic configuration identity"));
        }
    }
    Ok(out)
}

impl SharedConfiguration {
    pub(super) fn is_empty(&self) -> bool {
        self.agents.is_empty()
            && self.tools.is_empty()
            && self.mcps.is_empty()
            && self.notification_channels.is_empty()
            && self.intercept_rules.is_empty()
    }

    pub(super) fn root_keys(&self) -> BTreeMap<String, BTreeSet<String>> {
        BTreeMap::from([
            ("agents".into(), self.agents.keys().cloned().collect()),
            ("tools".into(), self.tools.keys().cloned().collect()),
            ("mcps".into(), self.mcps.keys().cloned().collect()),
            (
                "notification_channels".into(),
                self.notification_channels.keys().cloned().collect(),
            ),
            (
                "intercept_rules".into(),
                self.intercept_rules.keys().cloned().collect(),
            ),
        ])
    }

    /// Tombstones affect only imports. Explicit creation is handled by the catalog
    /// transaction, which clears the tombstone before applying this filter.
    pub(super) fn without_deleted(&mut self, deleted: &BTreeMap<String, BTreeSet<String>>) {
        let contains = |kind: &str, key: &str| deleted.get(kind).is_some_and(|s| s.contains(key));
        self.agents.retain(|key, _| !contains("agents", key));
        self.tools.retain(|key, _| !contains("tools", key));
        self.mcps.retain(|key, _| !contains("mcps", key));
        self.notification_channels
            .retain(|id, _| !contains("notification_channels", id));
        self.intercept_rules
            .retain(|id, _| !contains("intercept_rules", id));
        for a in self.agents.values_mut() {
            a.visibility.retain(|v| !contains("mcps", &v.server));
        }
        for tool in self.tools.values_mut() {
            if let Some(agents) = tool["agents"].as_array_mut() {
                agents.retain(|v| !v.as_str().is_some_and(|key| contains("agents", key)));
            }
        }
    }

    pub(super) fn retain_profiles(&mut self, profiles: &BTreeSet<String>) {
        for a in self.agents.values_mut() {
            if a.data["llm_profile_id"]
                .as_str()
                .is_some_and(|id| !profiles.contains(id))
            {
                a.data["llm_profile_id"] = Value::Null;
            }
        }
    }

    pub(super) fn from_snapshot(s: &Snapshot) -> Result<Self> {
        let aliases = marker(s)?;
        let mut reverse = BTreeMap::new();
        for (global, local) in &aliases.trigger_ids {
            if !artex_core::valid_id(global, "trigger")
                || !artex_core::valid_id(local, "trigger")
                || reverse.insert(local.as_str(), global.as_str()).is_some()
            {
                return Err(corrupt("Invalid or duplicate trigger aliases"));
            }
        }
        let servers = keyed(s, "mcp_servers", "name")?;
        let server_names: BTreeMap<_, _> = servers
            .iter()
            .map(|(name, r)| (r.header.id.as_str(), name.as_str()))
            .collect();
        let mut agents = BTreeMap::new();
        for (key, a) in keyed(s, "agents", "key")? {
            let id = &a.header.id;
            let prompt = match a.data["current_prompt_id"].as_str() {
                Some(pid) => {
                    let p = s
                        .get("agent_prompts", pid)
                        .ok_or_else(|| corrupt("Current shared agent prompt is missing"))?;
                    if p.data["agent_id"] != *id {
                        return Err(corrupt("Current shared agent prompt has another owner"));
                    }
                    Some(fields(&p.data, &["template_text", "note", "updated_by"])?)
                }
                None if a.data["current_prompt_id"].is_null() => None,
                None => return Err(corrupt("Invalid current agent prompt reference")),
            };
            let mut variables = BTreeMap::new();
            for v in s
                .records("agent_prompt_vars")
                .into_iter()
                .filter(|r| r.data["agent_id"] == *id)
            {
                variables.insert(
                    string(&v.data, "var_name")?.into(),
                    fields(&v.data, &["description", "example", "source"])?,
                );
            }
            let mut visibility = Vec::new();
            for v in s
                .records("agent_visibility")
                .into_iter()
                .filter(|r| r.data["agent_id"] == *id)
            {
                let server_id = string(&v.data, "resource_id")?;
                // Older native snapshots can contain orphaned polymorphic grants.
                // An absent server grants nothing; do not resurrect legacy damage.
                let Some(server) = server_names.get(server_id) else {
                    continue;
                };
                visibility.push(McpVisibility {
                    server: (*server).into(),
                    tool: string(&v.data, "mcp_tool_name")?.into(),
                    enabled: v.data["enabled"]
                        .as_bool()
                        .ok_or_else(|| corrupt("Invalid MCP visibility"))?,
                });
            }
            visibility.sort();
            let mut skills = BTreeMap::new();
            for v in s
                .records("agent_skill_visibility")
                .into_iter()
                .filter(|r| r.data["agent_id"] == *id)
            {
                skills.insert(
                    string(&v.data, "skill_name")?.into(),
                    v.data["enabled"]
                        .as_bool()
                        .ok_or_else(|| corrupt("Invalid skill visibility"))?,
                );
            }
            let features = s
                .records("agent_features")
                .into_iter()
                .find(|r| r.data["agent_id"] == *id)
                .map(|r| fields(&r.data, &["questions", "plan_mode", "memory"]))
                .transpose()?;
            let mut triggers = BTreeMap::new();
            for t in s
                .records("agent_triggers")
                .into_iter()
                .filter(|r| r.data["agent_key"] == key)
            {
                let canonical = reverse
                    .get(t.header.id.as_str())
                    .copied()
                    .unwrap_or(&t.header.id);
                triggers.insert(canonical.into(), fields(&t.data, TRIGGER_FIELDS)?);
            }
            agents.insert(
                key,
                AgentConfiguration {
                    data: fields(&a.data, AGENT_FIELDS)?,
                    prompt,
                    variables,
                    visibility,
                    skills,
                    features,
                    triggers,
                },
            );
        }
        Ok(Self {
            agents,
            tools: keyed(s, "tools", "key")?
                .into_iter()
                .map(|(k, r)| Ok((k, fields(&r.data, TOOL_FIELDS)?)))
                .collect::<Result<_>>()?,
            mcps: servers
                .into_iter()
                .map(|(k, r)| Ok((k, fields(&r.data, MCP_FIELDS)?)))
                .collect::<Result<_>>()?,
            notification_channels: s
                .records("notification_channels")
                .into_iter()
                .map(|r| Ok((r.header.id.clone(), fields(&r.data, NOTIFICATION_FIELDS)?)))
                .collect::<Result<_>>()?,
            intercept_rules: s
                .records("intercept_rules")
                .into_iter()
                .map(|r| {
                    Ok((
                        r.header.id.clone(),
                        InterceptConfiguration {
                            data: fields(&r.data, INTERCEPT_FIELDS)?,
                            created_at: r.header.created_at.clone(),
                        },
                    ))
                })
                .collect::<Result<_>>()?,
        })
    }

    /// Conflicting old workspaces cannot overwrite the common configuration.
    /// A whole agent (including its triggers) is one import unit, avoiding duplicate
    /// one-time reporter triggers independently seeded in different workspaces.
    pub(super) fn merge_missing(&mut self, incoming: &Self) -> bool {
        let mut changed = false;
        for (key, value) in &incoming.agents {
            if !self.agents.contains_key(key) {
                self.agents.insert(key.clone(), value.clone());
                changed = true;
            }
        }
        for (target, source) in [
            (&mut self.tools, &incoming.tools),
            (&mut self.mcps, &incoming.mcps),
            (
                &mut self.notification_channels,
                &incoming.notification_channels,
            ),
        ] {
            for (key, value) in source {
                if !target.contains_key(key) {
                    target.insert(key.clone(), value.clone());
                    changed = true;
                }
            }
        }
        for (id, rule) in &incoming.intercept_rules {
            if !self.intercept_rules.contains_key(id) {
                self.intercept_rules.insert(id.clone(), rule.clone());
                changed = true;
            }
        }
        changed
    }

    /// The ordinary Store writer preserves existing headers. New projected
    /// intercept rows must use this import metadata so priority ties retain their
    /// source ordering. The readonly view applies the same metadata in memory.
    pub(super) fn import_metadata(
        &self,
        s: &Snapshot,
    ) -> BTreeMap<(String, String), artex_store::ImportMetadata> {
        self.intercept_rules
            .iter()
            .filter(|(id, _)| s.get("intercept_rules", id).is_none())
            .map(|(id, rule)| {
                (
                    ("intercept_rules".into(), id.clone()),
                    artex_store::ImportMetadata {
                        legacy_key: Value::Null,
                        created_at: rule.created_at.clone(),
                        updated_at: rule.created_at.clone(),
                    },
                )
            })
            .collect()
    }

    /// Only actual Body-bearing configuration fields participate in blob transfer.
    /// Opaque custom schemas/exec JSON may contain arbitrary `blob` keys.
    pub(super) fn body_references(&self) -> Vec<Value> {
        let mut refs = Vec::new();
        for a in self.agents.values() {
            for body in [
                &a.data["wrapup_prompt"],
                &a.data["task_timeout_wrapup_prompt"],
            ]
            .into_iter()
            .chain(a.prompt.iter().map(|p| &p["template_text"]))
            {
                if body["blob"].is_object() && !refs.contains(&body["blob"]) {
                    refs.push(body["blob"].clone());
                }
            }
        }
        refs
    }

    /// Produces ordinary writer changes without touching task data, live execution,
    /// credentials, or discovery. Caller supplies CAS expansion and blob ownership.
    pub(super) fn projection(&self, s: &Snapshot) -> Result<Vec<Change>> {
        let local_agents = keyed(s, "agents", "key")?;
        let local_mcps = keyed(s, "mcp_servers", "name")?;
        let aliases = marker(s)?;
        let mut next_aliases = Marker::default();
        let mut changes = Vec::new();
        let mut server_ids = BTreeMap::new();
        for (name, config) in &self.mcps {
            if config["name"] != *name || fields(config, MCP_FIELDS)? != *config {
                return Err(corrupt("Invalid shared MCP definition"));
            }
            let old = local_mcps.get(name).copied();
            let id = old
                .map(|r| r.header.id.clone())
                .unwrap_or_else(|| new_id("mcp"));
            put_config(s, &mut changes, "mcp_servers", &id, config)?;
            if old.is_none_or(|r| fields(&r.data, MCP_FIELDS).ok().as_ref() != Some(config)) {
                for cache in s
                    .records("mcp_tools_cache")
                    .into_iter()
                    .filter(|r| r.data["server_id"] == id)
                {
                    delete(&mut changes, "mcp_tools_cache", &cache.header.id);
                }
            }
            server_ids.insert(name.clone(), id);
        }
        for (name, old) in &local_mcps {
            if !self.mcps.contains_key(name) {
                delete(&mut changes, "mcp_servers", &old.header.id);
                for cache in s
                    .records("mcp_tools_cache")
                    .into_iter()
                    .filter(|r| r.data["server_id"] == old.header.id)
                {
                    delete(&mut changes, "mcp_tools_cache", &cache.header.id);
                }
            }
        }
        let mut vars = Vec::new();
        let mut visibility = Vec::new();
        let mut skills = Vec::new();
        let mut features = Vec::new();
        let mut used_triggers = BTreeSet::new();
        for (key, config) in &self.agents {
            if config.data["key"] != *key || fields(&config.data, AGENT_FIELDS)? != config.data {
                return Err(corrupt("Invalid shared agent definition"));
            }
            let old = local_agents.get(key).copied();
            let id = old
                .map(|a| a.header.id.clone())
                .unwrap_or_else(|| new_id("agent"));
            let mut data = old
                .map(|r| r.data.clone())
                .unwrap_or(default_data("agents")?);
            overlay(&mut data, &config.data)?;
            data["id"] = json!(id);
            let current = old
                .and_then(|a| a.data["current_prompt_id"].as_str())
                .and_then(|pid| s.get("agent_prompts", pid));
            data["current_prompt_id"] = match &config.prompt {
                None => Value::Null,
                Some(prompt) => {
                    if fields(prompt, &["template_text", "note", "updated_by"])? != *prompt {
                        return Err(corrupt("Invalid shared prompt definition"));
                    }
                    if let Some(current) = current.filter(|p| {
                        p.data["agent_id"] == id
                            && fields(&p.data, &["template_text", "note", "updated_by"])
                                .ok()
                                .as_ref()
                                == Some(prompt)
                    }) {
                        json!(current.header.id)
                    } else {
                        // Append in the destination's version sequence. Source IDs
                        // and version numbers must never replace local prompt history.
                        let version = s
                            .records("agent_prompts")
                            .into_iter()
                            .filter(|r| r.data["agent_id"] == id)
                            .filter_map(|r| r.data["version"].as_i64())
                            .max()
                            .unwrap_or(0)
                            .checked_add(1)
                            .filter(|v| *v <= i32::MAX as i64)
                            .ok_or_else(|| Error::new("SIZE_LIMIT", "Prompt version exhausted"))?;
                        let pid = new_id("prompt");
                        let mut p = prompt.clone();
                        p["agent_id"] = json!(id);
                        p["version"] = json!(version);
                        put_config(s, &mut changes, "agent_prompts", &pid, &p)?;
                        json!(pid)
                    }
                }
            };
            push_put(s, &mut changes, "agents", &id, data);
            for (name, v) in &config.variables {
                let mut v = fields(v, &["description", "example", "source"])?;
                v["agent_id"] = json!(id);
                v["var_name"] = json!(name);
                vars.push(v);
            }
            for v in &config.visibility {
                let server = server_ids
                    .get(&v.server)
                    .ok_or_else(|| corrupt("Shared visibility references a missing MCP server"))?;
                visibility.push(
                    json!({"agent_id":id,"resource_kind":"mcp","resource_id":server,
                    "mcp_tool_name":v.tool,"enabled":v.enabled}),
                );
            }
            for (name, enabled) in &config.skills {
                skills.push(json!({"agent_id":id,"skill_name":name,"enabled":enabled}));
            }
            if let Some(f) = &config.features {
                let mut f = fields(f, &["questions", "plan_mode", "memory"])?;
                f["agent_id"] = json!(id);
                features.push(f);
            }
            for (canonical, trigger) in &config.triggers {
                if !artex_core::valid_id(canonical, "trigger")
                    || trigger["agent_key"] != *key
                    || fields(trigger, TRIGGER_FIELDS)? != *trigger
                {
                    return Err(corrupt("Invalid shared trigger definition"));
                }
                let eligible = |r: &&Record| {
                    r.data["agent_key"] == *key && !used_triggers.contains(&r.header.id)
                };
                let old = aliases
                    .trigger_ids
                    .get(canonical)
                    .and_then(|local| s.get("agent_triggers", local))
                    .filter(eligible)
                    .or_else(|| s.get("agent_triggers", canonical).filter(eligible))
                    .or_else(|| {
                        s.records("agent_triggers")
                            .into_iter()
                            .filter(eligible)
                            .find(|r| {
                                fields(&r.data, TRIGGER_FIELDS).ok().as_ref() == Some(trigger)
                            })
                    });
                let local = old
                    .map(|r| r.header.id.clone())
                    .unwrap_or_else(|| new_id("trigger"));
                used_triggers.insert(local.clone());
                next_aliases
                    .trigger_ids
                    .insert(canonical.clone(), local.clone());
                // put_config preserves last_fire for matched rows; a new row starts
                // with the original null default and does not inherit another timer.
                put_config(s, &mut changes, "agent_triggers", &local, trigger)?;
            }
        }
        for (key, old) in &local_agents {
            if !self.agents.contains_key(key) {
                delete(&mut changes, "agents", &old.header.id);
                // These rows have actual agent ownership; run/conversation/memory
                // history has no cascading FK and is intentionally left untouched.
                for p in s
                    .records("agent_prompts")
                    .into_iter()
                    .filter(|p| p.data["agent_id"] == old.header.id)
                {
                    delete(&mut changes, "agent_prompts", &p.header.id);
                }
            }
        }
        reconcile(
            s,
            &mut changes,
            "agent_prompt_vars",
            "promptvar",
            &["agent_id", "var_name"],
            vars,
        )?;
        reconcile(
            s,
            &mut changes,
            "agent_visibility",
            "visibility",
            &["agent_id", "resource_kind", "resource_id", "mcp_tool_name"],
            visibility,
        )?;
        reconcile(
            s,
            &mut changes,
            "agent_skill_visibility",
            "skillvisibility",
            &["agent_id", "skill_name"],
            skills,
        )?;
        reconcile(
            s,
            &mut changes,
            "agent_features",
            "agentfeature",
            &["agent_id"],
            features,
        )?;
        for old in s.records("agent_triggers") {
            if !used_triggers.contains(&old.header.id) {
                delete(&mut changes, "agent_triggers", &old.header.id);
            }
        }
        let local_tools = keyed(s, "tools", "key")?;
        for (key, value) in &self.tools {
            if value["key"] != *key || fields(value, TOOL_FIELDS)? != *value {
                return Err(corrupt("Invalid shared tool definition"));
            }
            let id = local_tools
                .get(key)
                .map(|r| r.header.id.clone())
                .unwrap_or_else(|| new_id("toolconfig"));
            put_config(s, &mut changes, "tools", &id, value)?;
        }
        for (key, old) in local_tools {
            if !self.tools.contains_key(&key) {
                delete(&mut changes, "tools", &old.header.id);
            }
        }
        // Preserve native IDs for local delivery FK references. Deletion is a
        // normal channel deletion; the caller expands the existing CASCADE rules.
        // Projection itself never generates or sends a notification.
        for (id, value) in &self.notification_channels {
            if !artex_core::valid_id(id, "notifychannel")
                || fields(value, NOTIFICATION_FIELDS)? != *value
            {
                return Err(corrupt("Invalid shared notification channel definition"));
            }
            put_config(s, &mut changes, "notification_channels", id, value)?;
        }
        for old in s.records("notification_channels") {
            if !self.notification_channels.contains_key(&old.header.id) {
                delete(&mut changes, "notification_channels", &old.header.id);
            }
        }
        for (id, rule) in &self.intercept_rules {
            if !artex_core::valid_id(id, "interceptrule")
                || fields(&rule.data, INTERCEPT_FIELDS)? != rule.data
            {
                return Err(corrupt("Invalid shared tool intercept definition"));
            }
            put_config(s, &mut changes, "intercept_rules", id, &rule.data)?;
        }
        for old in s.records("intercept_rules") {
            if !self.intercept_rules.contains_key(&old.header.id) {
                delete(&mut changes, "intercept_rules", &old.header.id);
            }
        }
        if aliases != next_aliases {
            let saved = s
                .records("settings")
                .into_iter()
                .find(|r| r.data["key"] == MARKER_KEY);
            let id = saved
                .map(|r| r.header.id.clone())
                .unwrap_or_else(|| new_id("setting"));
            put_config(
                s,
                &mut changes,
                "settings",
                &id,
                &json!({"key":MARKER_KEY,"value":serde_json::to_string(&next_aliases)?}),
            )?;
        }
        Ok(changes)
    }
}

fn overlay(target: &mut Value, patch: &Value) -> Result<()> {
    let patch = patch
        .as_object()
        .ok_or_else(|| corrupt("Configuration must be an object"))?;
    for (key, value) in patch {
        target[key] = value.clone();
    }
    Ok(())
}

fn push_put(s: &Snapshot, changes: &mut Vec<Change>, col: &str, id: &str, data: Value) {
    if s.get(col, id).is_none_or(|r| r.data != data) {
        changes.push(Change::Put {
            collection: col.into(),
            id: id.into(),
            data,
        });
    }
}

fn put_config(
    s: &Snapshot,
    changes: &mut Vec<Change>,
    col: &str,
    id: &str,
    patch: &Value,
) -> Result<()> {
    let mut data = s
        .get(col, id)
        .map(|r| r.data.clone())
        .unwrap_or(default_data(col)?);
    overlay(&mut data, patch)?;
    if data.get("id").is_some() {
        data["id"] = json!(id);
    }
    push_put(s, changes, col, id, data);
    Ok(())
}

fn delete(changes: &mut Vec<Change>, col: &str, id: &str) {
    changes.push(Change::Delete {
        collection: col.into(),
        id: id.into(),
    });
}

fn reconcile(
    s: &Snapshot,
    changes: &mut Vec<Change>,
    col: &str,
    prefix: &str,
    keys: &[&str],
    desired: Vec<Value>,
) -> Result<()> {
    let mut used = BTreeSet::new();
    for data in desired {
        let old = s
            .records(col)
            .into_iter()
            .find(|r| !used.contains(&r.header.id) && keys.iter().all(|k| r.data[*k] == data[*k]));
        let id = old
            .map(|r| r.header.id.clone())
            .unwrap_or_else(|| new_id(prefix));
        used.insert(id.clone());
        put_config(s, changes, col, &id, &data)?;
    }
    for old in s.records(col) {
        if !used.contains(&old.header.id) {
            delete(changes, col, &old.header.id);
        }
    }
    Ok(())
}

#[cfg(test)]
#[path = "shared_configuration_tests.rs"]
mod tests;
