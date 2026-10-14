//! Direct-source coverage and the finding asset hierarchy share containment rules.
use crate::*;
use std::collections::BTreeMap;
use url::Url;
pub const QUERIES: &[&str] = &[
    "ui.taskCoverage",
    "ui.taskCoverageGraph",
    "ui.findingAssetTree",
];
fn host_port(a: &Value) -> (String, i64) {
    let mut host = text(a, "domain").to_owned();
    let mut port = a["port"].as_i64().unwrap_or(0);
    if host.is_empty() {
        if let Ok(url) = Url::parse(text(a, "url")) {
            host = url.host_str().unwrap_or("").into();
            if port == 0 {
                port = url.port_or_known_default().unwrap_or(0).into();
            }
        }
    }
    if host.is_empty() {
        host = text(a, "ip").into();
    }
    (host, port)
}
fn node(a: &Value, in_scope: bool, tested: bool) -> Value {
    let mut n = json!({"key":format!("a:{}",text(a,"id")),"kind":a["type"],"label":exploration_ui::asset_label(a),"asset_id":a["id"],"in_scope":in_scope,"tested":tested});
    for key in [
        "company_id",
        "domain",
        "root_domain",
        "ip",
        "url",
        "port",
        "service_type",
        "app_name",
        "page_title",
        "status_code",
    ] {
        if !a[key].is_null() && a[key] != "" && a[key] != 0 {
            n[key] = a[key].clone();
        }
    }
    n
}
pub fn hierarchy(
    s: &Snapshot,
    assets: BTreeSet<String>,
    tested: &BTreeSet<String>,
    finding_tree: bool,
) -> Value {
    let mut nodes = BTreeMap::<String, Value>::new();
    for id in &assets {
        if let Some(r) = s.get("assets", id) {
            nodes.insert(format!("a:{id}"), node(&r.data, true, tested.contains(id)));
        }
    }
    if finding_tree {
        for _ in 0..4 {
            let before = nodes.len();
            let current = nodes.values().cloned().collect::<Vec<_>>();
            for n in current {
                let Some(a) = s.get("assets", text(&n, "asset_id")) else {
                    continue;
                };
                let d = &a.data;
                let (host, _) = host_port(d);
                for p in s.records("assets") {
                    let pd = &p.data;
                    let want = match text(d, "type") {
                        "subdomain" => {
                            pd["type"] == "root_domain" && pd["domain"] == d["root_domain"]
                        }
                        "service" | "endpoint" => {
                            pd["type"] == "ip"
                                && (!text(d, "ip").is_empty() && pd["ip"] == d["ip"]
                                    || !host.is_empty() && pd["ip"] == host)
                                || pd["type"] == "subdomain"
                                    && !host.is_empty()
                                    && pd["domain"] == host
                                || pd["type"] == "root_domain"
                                    && !host.is_empty()
                                    && pd["domain"] == host
                                || d["type"] == "endpoint"
                                    && pd["type"] == "service"
                                    && !host.is_empty()
                                    && host_port(pd).0 == host
                        }
                        _ => false,
                    };
                    if want {
                        nodes
                            .entry(format!("a:{}", p.header.id))
                            .or_insert_with(|| node(pd, false, tested.contains(&p.header.id)));
                    }
                }
            }
            if before == nodes.len() {
                break;
            }
        }
    }
    let roots = nodes
        .values()
        .filter(|n| n["kind"] == "root_domain")
        .map(|n| (text(n, "domain").to_owned(), text(n, "key").to_owned()))
        .collect::<BTreeMap<_, _>>();
    let missing = nodes
        .values()
        .filter(|n| {
            n["kind"] == "subdomain"
                && !text(n, "root_domain").is_empty()
                && !roots.contains_key(text(n, "root_domain"))
        })
        .map(|n| text(n, "root_domain").to_owned())
        .collect::<BTreeSet<_>>();
    for root in missing {
        if let Some(r) = s
            .records("assets")
            .into_iter()
            .find(|a| a.data["type"] == "root_domain" && a.data["domain"] == root)
        {
            nodes
                .entry(format!("a:{}", r.header.id))
                .or_insert_with(|| node(&r.data, false, tested.contains(&r.header.id)));
        } else {
            nodes.insert(format!("r:{root}"),json!({"key":format!("r:{root}"),"kind":"root_domain","domain":root,"label":root,"in_scope":false,"tested":false}));
        }
    }
    for id in nodes
        .values()
        .filter_map(|n| n["company_id"].as_str())
        .map(str::to_owned)
        .collect::<BTreeSet<_>>()
    {
        if let Some(c) = s.get("companies", &id) {
            nodes.insert(format!("c:{id}"),json!({"key":format!("c:{id}"),"kind":"company","company_id":id,"label":c.data["name"],"in_scope":false,"tested":false}));
        }
    }
    let mut roots = BTreeMap::<String, String>::new();
    let mut subs = BTreeMap::<String, String>::new();
    let mut ips = BTreeMap::<String, String>::new();
    let mut services = BTreeMap::<String, String>::new();
    let mut svc_ports = BTreeMap::<(String, i64), String>::new();
    for (k, n) in &nodes {
        match text(n, "kind") {
            "root_domain" => {
                roots.insert(text(n, "domain").into(), k.clone());
            }
            "subdomain" => {
                subs.insert(text(n, "domain").into(), k.clone());
            }
            "ip" => {
                ips.insert(text(n, "ip").into(), k.clone());
            }
            "service" => {
                let (h, p) = host_port(n);
                if !h.is_empty() {
                    services.insert(h.clone(), k.clone());
                    svc_ports.insert((h, p), k.clone());
                }
            }
            _ => {}
        }
    }
    let mut edges = vec![];
    for (k, n) in &nodes {
        let (h, p) = host_port(n);
        let parent = match text(n, "kind") {
            "root_domain" | "ip" | "app" => n["company_id"].as_str().map(|id| format!("c:{id}")),
            "subdomain" => roots.get(text(n, "root_domain")).cloned(),
            "service" => subs
                .get(text(n, "domain"))
                .or_else(|| if finding_tree { subs.get(&h) } else { None })
                .or_else(|| ips.get(text(n, "ip")))
                .or_else(|| if finding_tree { ips.get(&h) } else { None })
                .or_else(|| roots.get(text(n, "root_domain")))
                .or_else(|| if finding_tree { roots.get(&h) } else { None })
                .cloned(),
            "endpoint" => svc_ports
                .get(&(h.clone(), p))
                .or_else(|| services.get(&h))
                .or_else(|| subs.get(&h))
                .or_else(|| subs.get(text(n, "domain")))
                .or_else(|| ips.get(&h))
                .or_else(|| ips.get(text(n, "ip")))
                .or_else(|| roots.get(text(n, "root_domain")))
                .or_else(|| if finding_tree { roots.get(&h) } else { None })
                .cloned(),
            _ => None,
        };
        if let Some(parent) = parent.filter(|p| p != k && nodes.contains_key(p)) {
            edges.push(json!({"src":k,"dst":parent}));
        }
    }
    json!({"nodes":nodes.into_values().collect::<Vec<_>>(),"edges":edges})
}
pub fn scoped_coverage(s: &Snapshot, task: &str) -> Result<(BTreeSet<String>, BTreeSet<String>)> {
    record(s, "tasks", task)?;
    let exps = visible_explorations(s, task);
    let mut target = s
        .records("assets")
        .into_iter()
        .filter(|a| assets::declared_scope(s, task, &a.data, false))
        .map(|a| a.header.id.clone())
        .collect::<BTreeSet<_>>();
    let mut tested = BTreeSet::new();
    for a in s.records("exploration_anchors") {
        if let Some(n) = s.get("exploration_nodes", text(&a.data, "node_id")) {
            if exps.contains(text(&n.data, "exploration_id")) {
                if let Some(asset) = a.data["asset_id"]
                    .as_str()
                    .filter(|id| s.get("assets", id).is_some())
                {
                    target.insert(asset.into());
                    if n.data["kind"] == "fact" {
                        tested.insert(asset.into());
                    }
                }
            }
        }
    }
    Ok((target, tested))
}
pub fn scope_asset_ids(s: &Snapshot, scope: &str) -> BTreeSet<String> {
    let all = s
        .records("assets")
        .iter()
        .map(|r| r.header.id.clone())
        .collect();
    let graph = hierarchy(s, all, &BTreeSet::new(), true);
    let mut keys = BTreeSet::from([scope.to_owned()]);
    loop {
        let before = keys.len();
        for e in graph["edges"].as_array().unwrap() {
            if keys.contains(text(e, "dst")) {
                keys.insert(text(e, "src").into());
            }
        }
        if before == keys.len() {
            break;
        }
    }
    keys.into_iter()
        .filter_map(|k| k.strip_prefix("a:").map(str::to_owned))
        .collect()
}
pub fn query(m: &str, p: &Value, s: &Snapshot, _: &Context) -> Result<Value> {
    match m {
        "ui.taskCoverage" | "ui.taskCoverageGraph" => {
            let task = required(p, "id")?;
            let t = record(s, "tasks", task)?;
            if m == "ui.taskCoverage" && t.data["coverage_enabled"] == false {
                return Ok(
                    json!({"enabled":false,"scope_rows":0,"denominator":0,"tested":0,"pct":null,"by_type":[]}),
                );
            }
            let (target, tested) = scoped_coverage(s, task)?;
            if m == "ui.taskCoverageGraph" {
                return Ok(hierarchy(s, target, &tested, false));
            }
            let mut bytype = BTreeMap::<String, (usize, usize)>::new();
            for id in &target {
                let a = record(s, "assets", id)?;
                let count = bytype.entry(text(&a.data, "type").into()).or_default();
                count.0 += 1;
                count.1 += usize::from(tested.contains(id));
            }
            let total = target.len();
            let tested = target.intersection(&tested).count();
            let scopes = direct_sources(s, task);
            Ok(
                json!({"enabled":true,"scope_rows":s.records("task_scope").iter().filter(|r|scopes.contains(text(&r.data,"task_id"))).count(),"denominator":total,"tested":tested,"pct":if total==0{Value::Null}else{json!(tested as f64/total as f64)},"by_type":bytype.into_iter().map(|(typ,(total,tested))|json!({"type":typ,"total":total,"tested":tested})).collect::<Vec<_>>()}),
            )
        }
        "ui.findingAssetTree" => {
            let mut q = p["q"].clone();
            if let Some(q) = q.as_object_mut() {
                q.remove("assetScope");
            }
            let findings = findings_ui::filtered(s, &q)?;
            let selected = findings
                .iter()
                .flat_map(|r| ids(&r.data, "asset_ids"))
                .collect();
            let graph = hierarchy(s, selected, &BTreeSet::new(), true);
            let parents = graph["edges"]
                .as_array()
                .unwrap()
                .iter()
                .map(|e| (text(e, "src").to_owned(), text(e, "dst").to_owned()))
                .collect::<BTreeMap<_, _>>();
            let mut nodes=graph["nodes"].as_array().unwrap().iter().map(|n|{let k=text(n,"key");let mut o=json!({"key":k,"kind":n["kind"],"label":n["label"],"self":0,"total":0,"critical":0,"high":0,"medium":0,"low":0,"last_found_at":"0001-01-01T00:00:00Z"});for key in ["asset_id","company_id"]{if !n[key].is_null(){o[key]=n[key].clone();}}if let Some(parent)=parents.get(k){o["parent"]=json!(parent);}(k.to_owned(),o)}).collect::<BTreeMap<_,_>>();
            nodes.insert("__none__".into(),json!({"key":"__none__","kind":"none","label":"Unassociated assets","self":0,"total":0,"critical":0,"high":0,"medium":0,"low":0,"last_found_at":"0001-01-01T00:00:00Z"}));
            for f in &findings {
                let direct = ids(&f.data, "asset_ids")
                    .into_iter()
                    .map(|id| format!("a:{id}"))
                    .filter(|k| nodes.contains_key(k))
                    .collect::<BTreeSet<_>>();
                let direct = if direct.is_empty() {
                    BTreeSet::from(["__none__".into()])
                } else {
                    direct
                };
                let mut touched = BTreeSet::new();
                for k in direct {
                    let n = nodes.get_mut(&k).unwrap();
                    n["self"] = json!(n["self"].as_u64().unwrap() + 1);
                    let mut next = Some(k);
                    while let Some(k) = next {
                        if !touched.insert(k.clone()) {
                            break;
                        }
                        next = parents.get(&k).cloned();
                    }
                }
                for k in touched {
                    let n = nodes.get_mut(&k).unwrap();
                    n["total"] = json!(n["total"].as_u64().unwrap() + 1);
                    let sev = text(&f.data, "severity");
                    if ["critical", "high", "medium", "low"].contains(&sev) {
                        n[sev] = json!(n[sev].as_u64().unwrap() + 1);
                    }
                    if text(&f.data, "created_at") > text(n, "last_found_at") {
                        n["last_found_at"] = f.data["created_at"].clone();
                    }
                }
            }
            let mut rows = nodes
                .into_values()
                .filter(|n| n["total"] != 0)
                .collect::<Vec<_>>();
            rows.sort_by(|a, b| {
                (a["kind"] == "none")
                    .cmp(&(b["kind"] == "none"))
                    .then_with(|| b["total"].as_u64().cmp(&a["total"].as_u64()))
                    .then_with(|| text(a, "label").cmp(text(b, "label")))
            });
            let mut dropped = vec![];
            for kind in ["endpoint", "service"] {
                if rows.len() <= 3000 {
                    break;
                }
                rows.retain(|n| n["kind"] != kind);
                dropped.push(kind);
            }
            let mut out = json!({"nodes":rows,"finding_total":findings.len(),"truncated":!dropped.is_empty()});
            if !dropped.is_empty() {
                out["dropped_kinds"] = json!(dropped);
            }
            Ok(out)
        }
        _ => Err(Error::new("METHOD_NOT_FOUND", m)),
    }
}
