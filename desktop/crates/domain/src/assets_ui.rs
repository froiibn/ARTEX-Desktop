//! Source-compatible asset pages and atomic company/task associations.
use crate::*;
use ipnet::IpNet;
use std::{collections::BTreeMap, net::IpAddr};
use url::Url;
pub const QUERIES: &[&str] = &[
    "ui.assets",
    "ui.searchAssets",
    "ui.taskAssets",
    "ui.searchTaskAssets",
    "ui.assetCounts",
    "ui.taskIntentAssets",
    "ui.companies",
];
pub const COMMANDS: &[&str] = &[
    "ui.deleteAssets",
    "ui.attachTaskAssets",
    "ui.registerTaskAssetScopes",
    "ui.detachTaskAsset",
    "ui.createCompany",
    "ui.addCompanyScope",
    "ui.updateCompanyScope",
    "ui.deleteCompany",
];
fn live_task<'a>(s: &'a Snapshot, id: &str) -> Result<&'a Record> {
    let t = record(s, "tasks", id)?;
    if !t.data["deleted_at"].is_null() {
        return Err(Error::new("NOT_FOUND", "Task is deleted"));
    }
    Ok(t)
}
pub fn wire_asset(d: &Value, s: &Snapshot, task: Option<&str>) -> Value {
    let mut out = json!({"id":d["id"],"type":d["type"],"task_ids":s.records("task_asset_links").into_iter().filter(|l|l.data["asset_id"]==d["id"]).map(|l|l.data["task_id"].clone()).collect::<Vec<_>>(),"last_seen":d["last_seen"]});
    for key in [
        "company_id",
        "domain",
        "root_domain",
        "ip",
        "c_segment",
        "port",
        "icp",
        "bound_domains",
        "open_ports",
        "record_type",
        "record_value",
        "bundle_id",
        "app_name",
        "category",
        "app_description",
        "app_icp",
        "url",
        "service_type",
        "service_name",
        "favicon_mmh3",
        "status_code",
        "content_length",
        "page_title",
        "technologies",
        "auth",
        "method",
        "params",
        "extra",
    ] {
        let v = &d[key];
        if !v.is_null()
            && v.as_str().is_none_or(|v| !v.is_empty())
            && v.as_array().is_none_or(|v| !v.is_empty())
            && v.as_object().is_none_or(|v| !v.is_empty())
        {
            out[key] = v.clone();
        }
    }
    if let Some(task) = task {
        if let Some(l) = s
            .records("task_asset_links")
            .into_iter()
            .find(|l| l.data["task_id"] == task && l.data["asset_id"] == d["id"])
        {
            for (a, b) in [
                ("source", "task_source"),
                ("source_summary", "task_source_summary"),
                ("source_node_id", "task_source_node_id"),
            ] {
                if !l.data[a].is_null() && l.data[a] != "" {
                    out[b] = l.data[a].clone();
                }
            }
        }
    }
    out
}
fn scope_wire(r: &Record) -> Value {
    let mut d = json!({"id":r.header.id,"company_id":r.data["company_id"],"kind":r.data["kind"],"raw":r.data["raw"]});
    for key in ["domain", "net", "value", "reason"] {
        if !r.data[key].is_null() && r.data[key] != "" {
            d[key] = r.data[key].clone();
        }
    }
    d
}
pub fn query(m: &str, p: &Value, s: &Snapshot, _c: &Context) -> Result<Value> {
    match m {
        "ui.companies" => {
            let mut rows = s.records("companies");
            rows.sort_by(|a, b| {
                text(&b.data, "created_at")
                    .cmp(text(&a.data, "created_at"))
                    .then_with(|| b.header.id.cmp(&a.header.id))
            });
            Ok(json!(rows.into_iter().map(|r|{let mut d=json!({"id":r.header.id,"name":r.data["name"],"asset_count":s.records("assets").iter().filter(|a|a.data["company_id"]==r.header.id).count(),"scope":s.records("company_scope").into_iter().filter(|a|a.data["company_id"]==r.header.id).map(scope_wire).collect::<Vec<_>>()});if !r.data["logo"].is_null(){d["logo"]=r.data["logo"].clone();}d}).collect::<Vec<_>>()))
        }
        "ui.taskIntentAssets" => {
            let task = required(p, "taskId")?;
            live_task(s, task)?;
            let mut out = vec![];
            for n in exploration_ui::visible_nodes(s, task, true)?
                .into_iter()
                .filter(|n| n.data["kind"] == "intent")
            {
                let source = s
                    .records("tasks")
                    .into_iter()
                    .find(|t| t.data["exploration_id"] == n.data["exploration_id"])
                    .unwrap();
                for a in s
                    .records("exploration_anchors")
                    .into_iter()
                    .filter(|a| a.data["node_id"] == n.header.id)
                {
                    let Some(asset) = s.get("assets", text(&a.data, "asset_id")) else {
                        continue;
                    };
                    let link = s.records("task_asset_links").into_iter().find(|l| {
                        l.data["task_id"] == source.header.id
                            && l.data["asset_id"] == asset.header.id
                    });
                    let mut v = json!({"intent_id":n.header.id,"asset_id":asset.header.id,"type":asset.data["type"],"label":exploration_ui::asset_label(&asset.data),"source":link.map(|l|text(&l.data,"source")).unwrap_or("anchor"),"source_summary":link.map(|l|text(&l.data,"source_summary")).filter(|x|!x.is_empty()).unwrap_or("Intent anchors this asset in the blackboard"),"source_task_id":source.header.id,"inherited":source.header.id!=task});
                    if let Some(id) = link.and_then(|l| l.data["source_node_id"].as_str()) {
                        v["source_node_id"] = json!(id);
                    }
                    out.push(v);
                }
            }
            out.sort_by(|a, b| {
                a["inherited"]
                    .as_bool()
                    .cmp(&b["inherited"].as_bool())
                    .then_with(|| text(b, "intent_id").cmp(text(a, "intent_id")))
                    .then_with(|| text(a, "asset_id").cmp(text(b, "asset_id")))
            });
            Ok(json!(out))
        }
        "ui.assets"
        | "ui.searchAssets"
        | "ui.taskAssets"
        | "ui.searchTaskAssets"
        | "ui.assetCounts" => {
            let task = p["taskId"].as_str().filter(|t| !t.is_empty());
            if let Some(t) = task {
                live_task(s, t)?;
            }
            let mut typ = text(p, "type");
            if task.is_none()
                && text(p, "dsl").is_empty()
                && typ.is_empty()
                && m != "ui.assetCounts"
            {
                typ = "root_domain";
            }
            let dsl = if text(p, "dsl").is_empty() {
                None
            } else {
                Some(exploration::Dsl::parse(text(p, "dsl"))?)
            };
            let mut rows = vec![];
            for r in s.records("assets") {
                if task.is_some_and(|t| {
                    !s.records("task_asset_links")
                        .iter()
                        .any(|l| l.data["task_id"] == t && l.data["asset_id"] == r.header.id)
                }) || !typ.is_empty() && r.data["type"] != typ
                {
                    continue;
                }
                let wire = wire_asset(&r.data, s, task);
                if let Some(dsl) = &dsl {
                    if !dsl.matches(&wire)? {
                        continue;
                    }
                }
                rows.push(r);
            }
            if m == "ui.assetCounts" {
                let mut counts = BTreeMap::<String, usize>::new();
                for r in rows {
                    *counts.entry(text(&r.data, "type").into()).or_default() += 1;
                }
                return Ok(json!(counts));
            }
            rows.sort_by(|a, b| {
                text(&b.data, "last_seen")
                    .cmp(text(&a.data, "last_seen"))
                    .then_with(|| b.header.id.cmp(&a.header.id))
            });
            let total = rows.len();
            let limit = p["limit"]
                .as_u64()
                .filter(|n| *n > 0)
                .unwrap_or(50)
                .min(200) as usize;
            let offset = p["offset"].as_u64().unwrap_or(0) as usize;
            Ok(
                json!({"assets":rows.into_iter().skip(offset).take(limit).map(|r|wire_asset(&r.data,s,task)).collect::<Vec<_>>(),"total":total}),
            )
        }
        _ => Err(Error::new("METHOD_NOT_FOUND", m)),
    }
}

/// Mirrors ParseScopeInput: domain hosts are strict, normalized through IDNA;
/// CIDRs are canonical and bounded, explicit kinds never change classification.
pub fn parse_scope(input: &Value) -> Result<Value> {
    let raw = input
        .as_str()
        .unwrap_or_else(|| text(input, "value"))
        .trim();
    if raw.chars().count() > 1024 {
        return Err(Error::new(
            "INVALID_PARAMS",
            "Scope exceeds 1024 characters",
        ));
    }
    if raw.is_empty() {
        return Err(Error::new("INVALID_PARAMS", "Empty scope"));
    }
    let explicit = text(input, "kind").trim().to_lowercase();
    let mut kind = explicit.clone();
    let mut d = json!({"raw":raw});
    if kind.is_empty() {
        kind = if raw.parse::<IpNet>().is_ok() {
            "cidr"
        } else if raw.parse::<IpAddr>().is_ok() {
            "ip"
        } else if raw.contains("://") || raw.contains('.') && !raw.chars().any(char::is_whitespace)
        {
            "domain"
        } else if raw.to_lowercase().contains("icp") || raw.contains("\u{5907}\u{6848}") {
            "icp"
        } else {
            "keyword"
        }
        .into();
    }
    match kind.as_str() {
        "icp" => {
            d["value"] = json!(
                raw.chars()
                    .filter(|c| !c.is_whitespace())
                    .collect::<String>()
                    .to_lowercase()
            )
        }
        "keyword" => d["value"] = json!(assets::company_key(raw)),
        "ip" | "cidr" | "domain" => {
            if let Ok(net) = raw.parse::<IpNet>() {
                if !explicit.is_empty() && explicit != "cidr" {
                    return Err(Error::new(
                        "INVALID_PARAMS",
                        "Scope kind does not match CIDR",
                    ));
                }
                if net.prefix_len() < if net.addr().is_ipv4() { 16 } else { 32 } {
                    return Err(Error::new("INVALID_PARAMS", "Network scope is too broad"));
                }
                kind = "cidr".into();
                d["net"] = json!(net.trunc().to_string());
            } else {
                let candidate = if let Ok(ip) = raw.parse::<IpAddr>() {
                    format!(
                        "http://{}",
                        if ip.is_ipv6() {
                            format!("[{ip}]")
                        } else {
                            ip.to_string()
                        }
                    )
                } else if raw.starts_with("//") {
                    format!("http:{raw}")
                } else if raw.contains("://") {
                    raw.into()
                } else {
                    format!("http://{raw}")
                };
                let url = Url::parse(&candidate)
                    .map_err(|_| Error::new("INVALID_PARAMS", "Invalid scope host"))?;
                let host = url
                    .host_str()
                    .ok_or_else(|| Error::new("INVALID_PARAMS", "Scope host is missing"))?
                    .trim_matches(['[', ']'])
                    .trim_end_matches('.')
                    .to_lowercase();
                if let Ok(ip) = host.parse::<IpAddr>() {
                    if !explicit.is_empty() && explicit != "ip" {
                        return Err(Error::new("INVALID_PARAMS", "Scope kind does not match IP"));
                    }
                    kind = "ip".into();
                    d["net"] = json!(format!("{ip}/{}", if ip.is_ipv4() { 32 } else { 128 }));
                } else {
                    if !explicit.is_empty() && explicit != "domain" {
                        return Err(Error::new(
                            "INVALID_PARAMS",
                            "Scope kind does not match domain",
                        ));
                    }
                    if host.len() > 253
                        || host.split('.').count() < 2
                        || host.split('.').any(|l| {
                            l.is_empty()
                                || l.len() > 63
                                || l.starts_with('-')
                                || l.ends_with('-')
                                || l.chars().any(|c| !c.is_ascii_alphanumeric() && c != '-')
                        })
                    {
                        return Err(Error::new("INVALID_PARAMS", "Invalid domain labels"));
                    }
                    let (suf, icann) = psl::public_suffix(&host);
                    if icann && suf == host {
                        return Err(Error::new("INVALID_PARAMS", "Public suffix is not a scope"));
                    }
                    kind = "domain".into();
                    d["domain"] = json!(host);
                }
            }
        }
        _ => return Err(Error::new("INVALID_PARAMS", "Unsupported scope kind")),
    }
    d["kind"] = json!(kind);
    Ok(d)
}
fn same_scope(a: &Value, b: &Value) -> bool {
    a["kind"] == b["kind"]
        && ["domain", "net", "value"]
            .iter()
            .all(|k| a[*k].as_str().unwrap_or("") == b[*k].as_str().unwrap_or(""))
}
fn link(b: &mut Planner, task: &str, asset: &str, summary: &str) -> Result<bool> {
    let old = b
        .state
        .records("task_asset_links")
        .into_iter()
        .find(|r| r.data["task_id"] == task && r.data["asset_id"] == asset);
    let staged = b.batch.changes.iter().find_map(|ch| match ch {
        Change::Put {
            collection,
            id,
            data,
        } if collection == "task_asset_links"
            && data["task_id"] == task
            && data["asset_id"] == asset =>
        {
            Some((id.clone(), data.clone()))
        }
        _ => None,
    });
    let existed = old.is_some();
    let (id, mut d) = staged
        .or_else(|| old.map(|r| (r.header.id.clone(), r.data.clone())))
        .unwrap_or_else(|| {
            (
                new_id("assetlink"),
                default_data("task_asset_links").unwrap(),
            )
        });
    merge(
        &mut d,
        &json!({"task_id":task,"asset_id":asset,"source":"manual","source_summary":summary,"source_node_id":null}),
    );
    b.put("task_asset_links", &id, d);
    let mut a = b
        .batch
        .changes
        .iter()
        .rev()
        .find_map(|ch| match ch {
            Change::Put {
                collection,
                id,
                data,
            } if collection == "assets" && id == asset => Some(data.clone()),
            _ => None,
        })
        .or_else(|| b.state.get("assets", asset).map(|r| r.data.clone()))
        .ok_or_else(|| Error::new("NOT_FOUND", "Asset is missing"))?;
    let mut tasks = ids(&a, "task_ids");
    if !tasks.iter().any(|t| t == task) {
        tasks.push(task.into());
        a["task_ids"] = json!(tasks);
        b.put("assets", asset, a);
    }
    Ok(existed)
}
pub fn plan(m: &str, p: &Value, s: &Snapshot, c: &Context) -> Result<CommandBatch> {
    let mut b = Planner::new(s, m, p);
    let result = match m {
        "ui.deleteAssets" => {
            let selected = ids(p, "ids").into_iter().collect::<BTreeSet<_>>();
            if selected.is_empty() {
                return Err(Error::new("INVALID_PARAMS", "IDs are required"));
            }
            let mut count = 0;
            for id in selected {
                if s.get("assets", &id).is_some() {
                    b.delete("assets", &id);
                    count += 1;
                }
            }
            json!({"deleted":count})
        }
        "ui.attachTaskAssets" => {
            let task = required(p, "taskId")?;
            live_task(s, task)?;
            let summary = required(p, "sourceSummary")?.trim();
            if summary.chars().count() > 500 {
                return Err(Error::new(
                    "INVALID_PARAMS",
                    "Source summary exceeds 500 characters",
                ));
            }
            let selected = ids(p, "assetIds").into_iter().collect::<BTreeSet<_>>();
            if selected.is_empty() || selected.len() > 100 {
                return Err(Error::new("INVALID_PARAMS", "Select 1 to 100 assets"));
            }
            for id in &selected {
                record(s, "assets", id)?;
            }
            let mut existing = 0;
            for id in &selected {
                if link(&mut b, task, id, summary)? {
                    existing += 1;
                }
            }
            json!({"requested":selected.len(),"attached":selected.len()-existing,"existing":existing})
        }
        "ui.detachTaskAsset" => {
            let task = required(p, "taskId")?;
            let asset = required(p, "assetId")?;
            let links = s
                .records("task_asset_links")
                .into_iter()
                .filter(|l| l.data["task_id"] == task && l.data["asset_id"] == asset)
                .collect::<Vec<_>>();
            if !links.is_empty() {
                for l in &links {
                    b.delete("task_asset_links", &l.header.id);
                }
                if let Some(r) = s.get("assets", asset) {
                    let mut d = r.data.clone();
                    d["task_ids"] = json!(
                        ids(&d, "task_ids")
                            .into_iter()
                            .filter(|id| id != task)
                            .collect::<Vec<_>>()
                    );
                    b.put("assets", asset, d);
                }
            }
            json!({"detached":usize::from(!links.is_empty())})
        }
        "ui.registerTaskAssetScopes" => {
            let task = required(p, "taskId")?;
            live_task(s, task)?;
            let inputs = p["scope"]
                .as_array()
                .filter(|a| !a.is_empty())
                .ok_or_else(|| Error::new("INVALID_PARAMS", "Scope entries are required"))?;
            let parsed = inputs.iter().map(parse_scope).collect::<Result<Vec<_>>>()?;
            let mut linked = 0;
            let mut existing = 0;
            let mut added = 0;
            let mut old = 0;
            let mut linked_once = BTreeSet::new();
            for rule in parsed {
                let kind = text(&rule, "kind");
                let scopekind = if kind == "domain" {
                    "root_domain"
                } else {
                    kind
                };
                let value = match kind {
                    "domain" => text(&rule, "domain"),
                    "ip" | "cidr" => text(&rule, "net"),
                    _ => text(&rule, "value"),
                };
                let patch = if kind == "domain" {
                    Some(json!({"type":"root_domain","domain":value}))
                } else if kind == "ip" {
                    Some(json!({"type":"ip","ip":value.split('/').next().unwrap()}))
                } else {
                    None
                };
                if let Some(patch) = patch {
                    let id = assets::upsert(&mut b, &patch, Some(task))?;
                    let existed = link(
                        &mut b,
                        task,
                        &id,
                        "Users add manually on the test asset page",
                    )?;
                    if existed || !linked_once.insert(id) {
                        existing += 1;
                    } else {
                        linked += 1;
                    }
                }
                let before = b.batch.changes.len();
                let (id, _) = assets::add_scope(
                    &mut b,
                    task,
                    scopekind,
                    value,
                    &json!("Users add manually on the test asset page"),
                    "manual",
                )?;
                if s.get("task_scope", &id).is_some() || before == b.batch.changes.len() {
                    old += 1;
                } else {
                    added += 1;
                }
            }
            json!({"requested":inputs.len(),"assets_linked":linked,"assets_existing":existing,"scopes_added":added,"scopes_existing":old})
        }
        "ui.createCompany" | "ui.addCompanyScope" | "ui.updateCompanyScope" => {
            let inputs = p["scope"]
                .as_array()
                .ok_or_else(|| Error::new("INVALID_PARAMS", "Scope must be an array"))?;
            for input in inputs {
                if input
                    .as_str()
                    .unwrap_or_else(|| text(input, "value"))
                    .chars()
                    .count()
                    > 1024
                {
                    return Err(Error::new(
                        "INVALID_PARAMS",
                        "Scope exceeds 1024 characters",
                    ));
                }
            }
            let mut parsed = vec![];
            let mut errors = vec![];
            for input in inputs {
                match parse_scope(input) {
                    Ok(rule) => parsed.push(rule),
                    Err(e) => errors.push(format!(
                        "{}: {}",
                        input.as_str().unwrap_or_else(|| text(input, "value")),
                        e.message
                    )),
                }
            }
            let reset = m == "ui.updateCompanyScope";
            if reset && !errors.is_empty() {
                return Err(Error::new("INVALID_PARAMS", "Scope contains invalid rules"));
            }
            let id = if m == "ui.createCompany" {
                let name = required(p, "name")?.trim();
                if s.records("companies")
                    .iter()
                    .any(|r| r.data["nkey"] == assets::company_key(name))
                {
                    return Err(Error::new(
                        "VERSION_CONFLICT",
                        "Company name already exists",
                    ));
                }
                b.insert(
                    "companies",
                    "company",
                    json!({"name":name,"nkey":assets::company_key(name)}),
                )?
            } else {
                let id = required(p, "id")?;
                record(s, "companies", id)?;
                id.into()
            };
            if reset {
                for r in s
                    .records("company_scope")
                    .into_iter()
                    .filter(|r| r.data["company_id"] == id)
                {
                    b.delete("company_scope", &r.header.id);
                }
            }
            let mut added = 0;
            let mut skipped = 0;
            for mut rule in parsed {
                let original = if reset {
                    false
                } else {
                    s.records("company_scope")
                        .iter()
                        .any(|r| r.data["company_id"] == id && same_scope(&r.data, &rule))
                };
                let staged=b.batch.changes.iter().any(|ch|matches!(ch,Change::Put{collection,data,..}if collection=="company_scope"&&data["company_id"]==id&&same_scope(data,&rule)));
                if original || staged {
                    skipped += 1;
                    continue;
                }
                rule["company_id"] = json!(id);
                rule["reason"] = json!(if m == "ui.createCompany" {
                    "api"
                } else {
                    text(p, "reason")
                });
                b.insert("company_scope", "companyscope", rule)?;
                added += 1;
            }
            assets::recompute_attribution(&mut b)?;
            let mut out = if m == "ui.createCompany" {
                json!({"id":id,"created":true,"scope_added":added,"scope_skipped":skipped,"scope_invalid":errors.len()})
            } else {
                json!({"added":added,"skipped":skipped,"invalid":errors.len()})
            };
            if !errors.is_empty() {
                out[if m == "ui.createCompany" {
                    "scope_errors"
                } else {
                    "errors"
                }] = json!(errors);
            }
            out
        }
        "ui.deleteCompany" => {
            let id = required(p, "id")?;
            record(s, "companies", id)?;
            let mut deleted = 0;
            if p["deleteAssets"] == true {
                for r in s
                    .records("assets")
                    .into_iter()
                    .filter(|r| r.data["company_id"] == id)
                {
                    b.delete("assets", &r.header.id);
                    deleted += 1;
                }
            }
            b.delete("companies", id);
            assets::recompute_attribution(&mut b)?;
            json!({"deleted":1,"assets_deleted":deleted})
        }
        _ => return Err(Error::new("METHOD_NOT_FOUND", m)),
    };
    if let Some(change) = b.batch.changes.first().cloned() {
        let (collection, id) = match change {
            Change::Put { collection, id, .. } | Change::Delete { collection, id } => {
                (collection, id)
            }
        };
        conversation::push_event(&mut b.batch, s, c, "AssetsChanged", &collection, &id)?;
    }
    let mut out = b.done(result);
    augment_expected(&mut out, s)?;
    Ok(out)
}
