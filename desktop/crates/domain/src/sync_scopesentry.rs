//! Source ScopeSentry synchronization: native MCP I/O is outside this pure planner.
use crate::*;
use std::collections::BTreeMap;
pub const QUERIES: &[&str] = &["ui.ssStatus", "ui.ssProjects", "ui.ssTasks"];
pub const COMMANDS: &[&str] = &["ui.ssDatasource", "ui.ssSync"];
pub fn server(s: &Snapshot) -> Option<&Record> {
    s.records("mcp_servers")
        .into_iter()
        .find(|r| r.data["name"] == "ScopeSentry")
}
pub fn configured(row: &Record) -> bool {
    !text(&row.data, "url").is_empty()
        && row.data["env"].as_object().is_some_and(|o| {
            o.values().any(|v| {
                v["secret_ref"].is_string() || v.as_str().is_some_and(|s| !s.trim().is_empty())
            })
        })
}
pub fn status(s: &Snapshot) -> Value {
    let mut out =
        json!({"exists":false,"configured":false,"enabled":false,"reachable":false,"tools":[]});
    if let Some(r) = server(s) {
        out["exists"] = json!(true);
        out["configured"] = json!(configured(r));
        out["enabled"] = r.data["enabled"].clone();
        out["url"] = r.data["url"].clone();
        out["tools"] = json!(
            s.records("mcp_tools_cache")
                .into_iter()
                .filter(|t| t.data["server_id"] == r.header.id)
                .filter_map(|t| t.data["tool_name"].as_str())
                .collect::<Vec<_>>()
        );
    }
    out
}
pub fn query(m: &str, _p: &Value, s: &Snapshot, c: &Context) -> Result<Value> {
    if c.actor.kind != "user" {
        return Err(Error::new(
            "PERMISSION_DENIED",
            "ScopeSentry management requires native user",
        ));
    }
    match m {
        "ui.ssStatus" => Ok(status(s)),
        "ui.ssProjects" | "ui.ssTasks" => Err(Error::new(
            "FEATURE_DISABLED",
            "ScopeSentry listings require the native MCP worker",
        )),
        _ => Err(Error::new("METHOD_NOT_FOUND", m)),
    }
}
pub fn validate_sync(p: &Value) -> Result<()> {
    let p = &p["body"];
    if !["project", "task"].contains(&text(p, "dimension")) {
        return Err(Error::new(
            "INVALID_PARAMS",
            "dimension must be project or task",
        ));
    }
    let targets = p["targets"]
        .as_array()
        .ok_or_else(|| Error::new("INVALID_PARAMS", "targets must be an array"))?;
    if targets.is_empty()
        || targets.len() > 100
        || targets.iter().any(|t| {
            t.as_str()
                .is_none_or(|s| s.trim().is_empty() || s.len() > 4096)
        })
    {
        return Err(Error::new(
            "INVALID_PARAMS",
            "Sync requires 1..100 bounded targets",
        ));
    }
    if p["asset_types"]
        .as_array()
        .is_none_or(|a| a.len() > 32 || a.iter().any(|v| !v.is_string()))
    {
        return Err(Error::new("INVALID_PARAMS", "Invalid asset types"));
    }
    Ok(())
}
fn sync_expected(b: &mut Planner, s: &Snapshot) {
    for c in ["mcp_servers", "assets", "companies", "company_scope"] {
        for r in s.records(c) {
            b.expect(r);
        }
    }
}
pub fn plan(m: &str, p: &Value, s: &Snapshot, c: &Context) -> Result<CommandBatch> {
    if c.actor.kind != "user" {
        return Err(Error::new(
            "PERMISSION_DENIED",
            "ScopeSentry management requires native user",
        ));
    }
    let mut b = Planner::new(s, m, p);
    let result = match m {
        "ui.ssDatasource" => {
            let old = server(s);
            let id = old
                .map(|r| r.header.id.clone())
                .unwrap_or_else(|| new_id(artex_core::collection_prefix("mcp_servers").unwrap()));
            let mut d = old
                .map(|r| r.data.clone())
                .unwrap_or(default_data("mcp_servers")?);
            d["id"] = json!(id);
            d["name"] = json!("ScopeSentry");
            d["transport"] = json!("http");
            if old.is_none() {
                d["env"] = json!({"X-API-Key":""});
            }
            let body = &p["body"];
            if let Some(url) = body["url"].as_str().filter(|u| !u.trim().is_empty()) {
                let u = url::Url::parse(url.trim())
                    .map_err(|_| Error::new("INVALID_PARAMS", "Datasource URL is invalid"))?;
                if !["http", "https"].contains(&u.scheme())
                    || !u.username().is_empty()
                    || u.password().is_some()
                {
                    return Err(Error::new(
                        "INVALID_PARAMS",
                        "Datasource requires HTTP(S) URL without credentials",
                    ));
                }
                d["url"] = json!(url.trim());
            }
            if !body["api_key"].is_null() {
                artex_core::validate_contract(
                    &json!({"$ref":"#/$defs/SecretRef"}),
                    &artex_core::contracts::read("entities.v1.schema.json")["$defs"],
                    &body["api_key"],
                )?;
                d["env"] = json!({"X-API-Key":body["api_key"]});
            }
            let enabled = !text(&d, "url").is_empty()
                && d["env"].as_object().is_some_and(|o| {
                    o.values().any(|v| {
                        v["secret_ref"].is_string()
                            || v.as_str().is_some_and(|s| !s.trim().is_empty())
                    })
                });
            d["enabled"] = json!(enabled);
            b.put("mcp_servers", &id, d);
            json!({"id":id,"enabled":enabled})
        }
        "ui.ssSync" => {
            validate_sync(p)?;
            let row = server(s)
                .filter(|r| configured(r) && r.data["enabled"] == true)
                .ok_or_else(|| {
                    Error::new(
                        "INVALID_PARAMS",
                        "ScopeSentry data source is not enabled and configured",
                    )
                })?;
            b.expect(row);
            sync_expected(&mut b, s);
            json!({"synced":{"subdomain":0,"app":0,"service":0,"ip":0},"companies":null,"warnings":null,"errors":null})
        }
        _ => return Err(Error::new("METHOD_NOT_FOUND", m)),
    };
    let mut result = b.done(result);
    augment_expected(&mut result, s)?;
    Ok(result)
}
fn absorb(b: &mut Planner, other: CommandBatch) {
    for change in other.changes {
        match change {
            Change::Put {
                collection,
                id,
                data,
            } => b.put(&collection, &id, data),
            Change::Delete { collection, id } => b.delete(&collection, &id),
        }
    }
}
/// Each raw item is normalized through the same asset service as local tools.
/// All company scopes precede asset assignment and the result is one CAS commit.
pub fn sync_plan(p: &Value, raw: &Value, s: &Snapshot, c: &Context) -> Result<CommandBatch> {
    let mut b = Planner::new(s, "ui.ssSync", p);
    validate_sync(p)?;
    sync_expected(&mut b, s);
    let mut companies = Vec::new();
    let mut warnings = raw["warnings"].as_array().cloned().unwrap_or_default();
    let mut errors = raw["errors"].as_array().cloned().unwrap_or_default();
    let mut counts = BTreeMap::from([("subdomain", 0u64), ("app", 0), ("service", 0), ("ip", 0)]);
    for metadata in raw["projects"].as_array().into_iter().flatten() {
        let name = text(metadata, "name");
        if name.trim().is_empty() {
            continue;
        }
        let projected = projected(s, &b.batch)?;
        let child = assets::plan(
            "add_company_scope",
            &json!({"company":name,"scope":text(metadata,"target"),"reason":format!("scopesentry:{name}")}),
            &projected,
            c,
        )?;
        for e in child.result["errors"].as_array().into_iter().flatten() {
            let _ = e;
            warnings.push(json!("An invalid project scope was excluded"));
        }
        absorb(&mut b, child);
        companies.push(json!(name));
    }
    for group in raw["groups"].as_array().into_iter().flatten() {
        let kind = text(group, "kind");
        let items = group["items"]
            .as_array()
            .ok_or_else(|| Error::new("PARSE_ERROR", "ScopeSentry items are not an array"))?;
        if items.len() > 5000 {
            return Err(Error::new(
                "SIZE_LIMIT",
                "ScopeSentry group exceeds 5000 assets",
            ));
        }
        for item in items {
            let asset = match kind {
                "subdomain" => {
                    if text(item, "host").is_empty() {
                        continue;
                    }
                    json!({"type":"subdomain","domain":item["host"],"record_type":item["type"].as_str().unwrap_or(""),"record_value":item["value"].as_array().cloned().unwrap_or_default()})
                }
                "app" => {
                    if text(item, "name").is_empty() {
                        continue;
                    }
                    json!({"type":"app","app_name":item["name"],"category":item["category"].as_str().unwrap_or(""),"app_description":item["description"].as_str().unwrap_or(""),"app_icp":item["icp"].as_str().unwrap_or("")})
                }
                "service" => {
                    if item["service"] == "http" || !text(item, "url").is_empty() {
                        if text(item, "url").is_empty() {
                            continue;
                        }
                        json!({"type":"service","service_type":"http","url":item["url"],"technologies":item["products"].as_array().cloned().unwrap_or_default(),"status_code":item["status"],"page_title":item["title"].as_str().unwrap_or(""),"favicon_mmh3":item["icon"].as_str().unwrap_or(""),"ip":item["ip"].as_str().unwrap_or("")})
                    } else {
                        let port = item["port"]
                            .as_u64()
                            .or_else(|| item["port"].as_str().and_then(|p| p.parse().ok()))
                            .unwrap_or(0);
                        json!({"type":"service","service_type":"other","domain":item["domain"].as_str().unwrap_or(""),"ip":item["ip"].as_str().unwrap_or(""),"port":port,"service_name":item["service"].as_str().unwrap_or("")})
                    }
                }
                _ => {
                    warnings.push(json!("Unknown asset type was excluded"));
                    continue;
                }
            };
            // A failed item must not leave any staged partial asset mutations.
            let saved = b.batch.clone();
            match assets::upsert(&mut b, &asset, None) {
                Ok(_) => {
                    *counts.get_mut(kind).unwrap() += 1;
                }
                Err(e) => {
                    b.batch = saved;
                    errors.push(json!(format!("{kind} import failed ({})", e.code)));
                    continue;
                }
            }
            if kind == "subdomain" {
                for ip in item["ip"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .filter_map(Value::as_str)
                    .filter(|s| !s.is_empty())
                {
                    let saved = b.batch.clone();
                    match assets::upsert(
                        &mut b,
                        &json!({"type":"ip","ip":ip,"bound_domains":[item["host"]]}),
                        None,
                    ) {
                        Ok(_) => {
                            *counts.get_mut("ip").unwrap() += 1;
                        }
                        Err(_) => {
                            b.batch = saved;
                        }
                    }
                }
            }
        }
    }
    if !companies.is_empty() {
        assets::recompute_attribution(&mut b)?;
    }
    let result = json!({"synced":counts,"companies":if companies.is_empty(){Value::Null}else{json!(companies)},"warnings":if warnings.is_empty(){Value::Null}else{json!(warnings)},"errors":if errors.is_empty(){Value::Null}else{json!(errors)}});
    let mut out = b.done(result);
    augment_expected(&mut out, s)?;
    Ok(out)
}
#[cfg(test)]
mod tests {
    use super::*;
    use artex_store::Store;
    fn context() -> Context {
        Context {
            actor: artex_core::Actor {
                kind: "user".into(),
                agent_id: None,
                run_id: None,
                engine_epoch: "fixture".into(),
            },
            task_id: None,
            intent_id: None,
            conversation_id: None,
        }
    }
    fn commit(store: &mut Store, mut b: CommandBatch) {
        b.command_id = new_id("command");
        store.commit(b).unwrap();
    }
    #[test]
    fn datasource_secretref_and_sync_use_existing_asset_normalization_and_company_scope() {
        let dir = tempfile::tempdir().unwrap();
        let mut store = Store::create(&dir.path().join("w"), "SS").unwrap();
        let c = context();
        let p = json!({"body":{"url":"https://ss.example.test/mcp","api_key":{"secret_ref":"secret_00000000000000000000000000000001","hint":"configured"}}});
        let b = plan("ui.ssDatasource", &p, &store.snapshot(), &c).unwrap();
        assert_eq!(b.result["enabled"], true);
        commit(&mut store, b);
        let p = json!({"body":{"dimension":"project","targets":["opaque-project"],"asset_types":[],"create_company":true}});
        let raw = json!({"projects":[{"name":"Fixture Company","target":"example.test"}],"groups":[{"kind":"subdomain","items":[{"host":"www.example.test","type":"A","value":["203.0.113.8"],"ip":["203.0.113.8"]}]},{"kind":"app","items":[{"name":"Fixture App","icp":"fixture-icp"}]},{"kind":"service","items":[{"url":"https://www.example.test","service":"http","title":"fixture","products":["nginx"],"status":200}]}]});
        let b = sync_plan(&p, &raw, &store.snapshot(), &c).unwrap();
        assert_eq!(
            b.result["synced"],
            json!({"subdomain":1,"ip":1,"app":1,"service":1})
        );
        commit(&mut store, b);
        let s = store.snapshot();
        let company = &s.records("companies")[0].header.id;
        assert_eq!(s.records("assets").len(), 6);
        assert!(
            s.records("assets")
                .iter()
                .filter(|r| r.data["type"] == "subdomain" || r.data["type"] == "service")
                .all(|r| r.data["company_id"] == *company)
        );
        assert_eq!(
            s.records("assets")
                .iter()
                .find(|r| r.data["type"] == "app")
                .unwrap()
                .data["app_icp"],
            "fixture-icp"
        );
        assert_eq!(status(&s)["tools"], json!([]));
    }
    #[test]
    fn bad_asset_is_excluded_without_any_staged_partial_mutation() {
        let dir = tempfile::tempdir().unwrap();
        let mut store = Store::create(&dir.path().join("w"), "SS errors").unwrap();
        let c = context();
        let p = json!({"body":{"dimension":"task","targets":["opaque"],"asset_types":["service"]}});
        let b=sync_plan(&p,&json!({"groups":[{"kind":"service","items":[{"ip":"not-ip","port":"80","service":"ssh"}]}]}),&store.snapshot(),&c).unwrap();
        assert!(b.changes.is_empty());
        assert_eq!(b.result["errors"].as_array().unwrap().len(), 1);
        commit(&mut store, b);
        assert!(store.snapshot().records("assets").is_empty());
    }
}
