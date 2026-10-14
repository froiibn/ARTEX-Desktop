use crate::*;
use ipnet::IpNet;
use std::collections::BTreeMap;
use std::net::IpAddr;
use url::Url;

pub fn domain_key(s: &str) -> String {
    let d = s.trim().to_lowercase();
    d.strip_suffix('.').unwrap_or(&d).to_owned()
}
pub fn company_key(name: &str) -> String {
    name.to_lowercase()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}
pub fn normalize_url(s: &str) -> Result<String> {
    // URL::parse is validation only. WHATWG reserialization changes default ports,
    // dot segments and escaping, whereas the frozen Go normalization preserves them.
    let u = Url::parse(s).map_err(|_| Error::new("INVALID_PARAMS", "invalid URL"))?;
    if !matches!(u.scheme(), "http" | "https") {
        return Err(Error::new("INVALID_PARAMS", "HTTP URL required"));
    }
    let (scheme, rest) = s
        .split_once("://")
        .ok_or_else(|| Error::new("INVALID_PARAMS", "URL requires scheme and authority"))?;
    let end = rest.find(['/', '?', '#']).unwrap_or(rest.len());
    let authority = &rest[..end];
    let suffix = &rest[end..];
    if authority.contains('@') {
        return Err(Error::new(
            "INVALID_PARAMS",
            "URL credentials require secret references",
        ));
    }
    let remove = suffix == "/";
    Ok(format!(
        "{}://{}{}",
        scheme.to_lowercase(),
        authority.to_lowercase(),
        if remove { "" } else { suffix }
    ))
}
pub fn natural_key(d: &Value) -> Result<String> {
    let typ = required(d, "type")?;
    let fields: Vec<&str> = match typ {
        "root_domain" => vec!["domain"],
        "ip" => vec!["ip"],
        "subdomain" => vec!["domain", "record_type"],
        "app" => vec![if !text(d, "bundle_id").is_empty() {
            "bundle_id"
        } else {
            "app_name"
        }],
        "service" if text(d, "service_type") == "http" => vec!["url"],
        "service" => vec!["domain", "ip", "port", "service_name"],
        "endpoint" => vec!["url", "method"],
        _ => return Err(Error::new("INVALID_PARAMS", "unknown asset type")),
    };
    Ok(format!(
        "{typ}:{}",
        fields
            .iter()
            .map(|f| if d[*f].is_null() {
                String::new()
            } else if let Some(x) = d[*f].as_str() {
                x.into()
            } else {
                d[*f].to_string()
            })
            .collect::<Vec<_>>()
            .join("\0")
    ))
}
fn normalize(p: &Value) -> Result<Value> {
    let mut d = default_data("assets")?;
    merge(&mut d, p);
    let typ = required(p, "type")?;
    if matches!(typ, "service" | "endpoint") && !text(p, "service_ip").is_empty() {
        d["ip"] = p["service_ip"].clone();
    }
    if !p["description"].is_null() {
        d["app_description"] = p["description"].clone();
    }
    if typ == "app" {
        if !text(p, "icp").is_empty() {
            d["app_icp"] = p["icp"].clone();
        }
        // The Go App DTO's icp is stored in assets.app_icp, not assets.icp.
        d["icp"] = Value::Null;
    }
    if let Some(n) = p["content_length"].as_i64() {
        d["content_length"] = json!(n.to_string());
    }
    if !text(p, "domain").is_empty() {
        d["domain"] = json!(domain_key(text(p, "domain")));
        d["root_domain"] = json!(crate::psl::root_domain(text(p, "domain")));
    }
    if !text(&d, "ip").is_empty() {
        let ip = text(&d, "ip").trim().to_owned();
        let parsed = ip
            .parse::<IpAddr>()
            .map_err(|_| Error::new("INVALID_PARAMS", "IP field must be IPv4 or IPv6"))?;
        d["ip"] = json!(ip);
        let net = IpNet::new(parsed, if parsed.is_ipv4() { 24 } else { 48 })
            .unwrap()
            .trunc();
        d["c_segment"] = json!(net.to_string());
    }
    if !text(p, "url").is_empty() {
        d["url"] = json!(normalize_url(text(p, "url"))?);
        let u = Url::parse(text(&d, "url")).unwrap();
        d["domain"] = json!(domain_key(
            u.host_str().unwrap_or("").trim_matches(['[', ']'])
        ));
        d["root_domain"] = json!(crate::psl::root_domain(text(&d, "domain")));
        d["service_type"] = json!("http");
        d["port"] = json!(u.port_or_known_default());
        d["service_name"] = json!(if u.scheme() == "https" {
            "HTTPS"
        } else {
            "HTTP"
        });
    }
    if typ == "root_domain" {
        d["root_domain"] = d["domain"].clone();
    }
    if typ == "subdomain" && matches!(text(&d, "record_type"), "A" | "AAAA") {
        if let Some(ip) = ids(&d, "record_value")
            .iter()
            .flat_map(|v| v.split(','))
            .map(str::trim)
            .find(|v| v.parse::<IpAddr>().is_ok())
        {
            let parsed = ip.parse::<IpAddr>().unwrap();
            d["ip"] = json!(ip);
            d["c_segment"] = json!(
                IpNet::new(parsed, if parsed.is_ipv4() { 24 } else { 48 })
                    .unwrap()
                    .trunc()
                    .to_string()
            );
        }
    }
    if !text(p, "method").is_empty() {
        d["method"] = json!(text(p, "method").to_uppercase());
    }
    match typ {
        "root_domain" | "subdomain" => {
            required(&d, "domain")?;
        }
        "ip" => {
            required(&d, "ip")?;
        }
        "app" => {
            required(&d, "app_name")?;
        }
        "endpoint" => {
            required(&d, "url")?;
            required(&d, "method")?;
        }
        "service" => {
            if text(&d, "url").is_empty() {
                required(&d, "service_name")?;
                if text(&d, "domain").is_empty() && text(&d, "ip").is_empty() {
                    return Err(Error::new(
                        "INVALID_PARAMS",
                        "Other service requires a domain or IP",
                    ));
                }
                d["service_name"] = json!(text(&d, "service_name").trim().to_lowercase());
                let port = d["port"].as_i64().unwrap_or(0);
                if !(1..=65535).contains(&port) {
                    return Err(Error::new("INVALID_PARAMS", "port must be 1..65535"));
                }
                d["service_type"] = json!("other");
            }
        }
        _ => return Err(Error::new("INVALID_PARAMS", "unknown asset type")),
    }
    if let Some(auth) = p["auth"].as_array() {
        for item in auth {
            if let Some(fields) = item.as_object() {
                for (key, value) in fields {
                    if ["password", "token", "api_key", "secret"].contains(&key.as_str())
                        && !value.is_null()
                        && value["secret_ref"].as_str().is_none()
                    {
                        return Err(Error::new(
                            "PERMISSION_DENIED",
                            "asset credentials require host SecretRef normalization",
                        ));
                    }
                }
            }
        }
    }
    if let Some(params) = d["params"].as_array_mut() {
        for x in params {
            if let Some(name) = x["name"].as_str().map(|n| n.trim().to_lowercase()) {
                x["name"] = json!(name);
            }
        }
    }
    Ok(d)
}
pub fn upsert(b: &mut Planner, p: &Value, task: Option<&str>) -> Result<String> {
    let mut d = normalize(p)?;
    if let Some(co) = d["company_id"].as_str() {
        record(b.state, "companies", co)?;
        d["company_source"] = json!("explicit");
    } else {
        d["company_id"] = resolve_company(b, &d);
        d["company_source"] = json!("scope");
    }
    let key = natural_key(&d)?;
    let existing = b
        .state
        .records("assets")
        .into_iter()
        .find(|r| natural_key(&r.data).ok().as_deref() == Some(&key));
    let staged = b.batch.changes.iter().find_map(|ch| {
        if let Change::Put {
            collection,
            id,
            data,
        } = ch
        {
            if collection == "assets" && natural_key(data).ok().as_deref() == Some(&key) {
                Some((id.clone(), data.clone()))
            } else {
                None
            }
        } else {
            None
        }
    });
    let prior = staged.or_else(|| existing.map(|r| (r.header.id.clone(), r.data.clone())));
    let id = if let Some((id, mut old)) = prior {
        let sticky = text(&old, "company_source") == "explicit";
        for (k, v) in d.as_object().unwrap() {
            if matches!(
                k.as_str(),
                "id" | "created_at" | "updated_at" | "last_seen" | "task_ids"
            ) {
                continue;
            }
            if sticky
                && matches!(k.as_str(), "company_id" | "company_source")
                && p["company_id"].is_null()
            {
                continue;
            }
            if k == "extra" {
                if let (Some(oldextra), Some(newextra)) = (old[k].as_object_mut(), v.as_object()) {
                    for (key, value) in newextra {
                        oldextra.insert(key.clone(), value.clone());
                    }
                    continue;
                }
            }
            if let (Some(a), Some(bb)) = (old[k].as_array_mut(), v.as_array()) {
                for i in bb {
                    if !a.contains(i) {
                        a.push(i.clone());
                    }
                }
            } else if !v.is_null()
                && (p.get(k).is_some()
                    || (text(&d, "type") == "app" && k == "app_icp" && p.get("icp").is_some())
                    || (text(&d, "type") == "subdomain"
                        && matches!(k.as_str(), "ip" | "c_segment")
                        && p.get("record_value").is_some())
                    || (matches!(k.as_str(), "ip" | "c_segment") && p.get("service_ip").is_some()))
            {
                old[k] = v.clone();
            }
        }
        old["last_seen"] = json!(artex_core::now());
        deduplicate_params(&mut old);
        if p["company_id"].as_str().is_some() {
            old["company_id"] = d["company_id"].clone();
            old["company_source"] = json!("explicit");
        }
        if !sticky && p["company_id"].is_null() {
            let resolved = resolve_company(b, &old);
            // UpsertApp uses COALESCE(new automatic owner, old owner). A full
            // company-scope recomputation can later clear an unmatched owner.
            if text(&old, "type") != "app" || !resolved.is_null() {
                old["company_id"] = resolved;
                old["company_source"] = json!("scope");
            }
        }
        b.put("assets", &id, old);
        id
    } else {
        let id = new_id("asset");
        d["id"] = json!(id);
        deduplicate_params(&mut d);
        d["last_seen"] = json!(artex_core::now());
        b.put("assets", &id, d);
        id
    };
    if let Some(task) = task {
        record(b.state, "tasks", task)?;
        let mut asset = b
            .batch
            .changes
            .iter()
            .rev()
            .find_map(|ch| match ch {
                Change::Put {
                    collection,
                    id: i,
                    data,
                } if collection == "assets" && i == &id => Some(data.clone()),
                _ => None,
            })
            .unwrap_or_else(|| b.state.get("assets", &id).unwrap().data.clone());
        let mut tasks = ids(&asset, "task_ids");
        if !tasks.iter().any(|t| t == task) {
            tasks.push(task.to_owned());
            asset["task_ids"] = json!(tasks);
            b.put("assets", &id, asset);
        }
        let linked=b.state.records("task_asset_links").iter().any(|r|text(&r.data,"task_id")==task&&text(&r.data,"asset_id")==id)||b.batch.changes.iter().any(|ch|matches!(ch,Change::Put{collection,data,..} if collection=="task_asset_links"&&text(data,"task_id")==task&&text(data,"asset_id")==id));
        if !linked {
            b.insert(
                "task_asset_links",
                "assetlink",
                json!({"task_id":task,"asset_id":id,"source":"agent","source_summary":""}),
            )?;
        }
    }
    derive_host_assets(b, p, task)?;
    Ok(id)
}
fn derive_host_assets(b: &mut Planner, input: &Value, task: Option<&str>) -> Result<()> {
    let d = normalize(input)?;
    let typ = text(&d, "type");
    if !matches!(typ, "subdomain" | "service" | "endpoint") {
        return Ok(());
    }
    let domain = text(&d, "domain");
    let root = text(&d, "root_domain");
    if !root.is_empty() {
        upsert(b, &json!({"type":"root_domain","domain":root}), task)?;
    }
    if typ != "subdomain"
        && !domain.is_empty()
        && domain != root
        && domain.trim_matches(['[', ']']).parse::<IpAddr>().is_err()
    {
        upsert(b, &json!({"type":"subdomain","domain":domain}), task)?;
    }
    if typ == "subdomain" && matches!(text(&d, "record_type"), "A" | "AAAA") {
        for value in ids(&d, "record_value") {
            for candidate in value
                .split(',')
                .map(str::trim)
                .filter(|v| v.parse::<IpAddr>().is_ok())
            {
                upsert(
                    b,
                    &json!({"type":"ip","ip":candidate,"bound_domains":[domain]}),
                    task,
                )?;
            }
        }
    } else if matches!(typ, "service" | "endpoint") && !text(&d, "ip").is_empty() {
        let mut ip = json!({"type":"ip","ip":d["ip"]});
        if typ == "service" {
            if !domain.is_empty() {
                ip["bound_domains"] = json!([domain]);
            }
            if d["port"].as_u64().is_some_and(|n| n > 0) {
                ip["open_ports"] = json!([{"port":d["port"],"service":d["service_name"]}]);
            }
        }
        upsert(b, &ip, task)?;
    }
    Ok(())
}
fn jsonb_text(v: &Value) -> String {
    match v {
        Value::Object(o) => {
            let mut keys = o.keys().collect::<Vec<_>>();
            keys.sort_by(|a, b| {
                a.len()
                    .cmp(&b.len())
                    .then_with(|| a.as_bytes().cmp(b.as_bytes()))
            });
            format!(
                "{{{}}}",
                keys.into_iter()
                    .map(|k| format!(
                        "{}: {}",
                        serde_json::to_string(k).unwrap(),
                        jsonb_text(&o[k])
                    ))
                    .collect::<Vec<_>>()
                    .join(", ")
            )
        }
        Value::Array(a) => format!(
            "[{}]",
            a.iter().map(jsonb_text).collect::<Vec<_>>().join(", ")
        ),
        _ => v.to_string(),
    }
}
fn deduplicate_params(asset: &mut Value) {
    if let Some(params) = asset["params"].as_array() {
        let mut unique = std::collections::BTreeMap::<(String, String), Value>::new();
        for p in params {
            let key = (text(p, "location").to_owned(), text(p, "name").to_owned());
            let replace = unique
                .get(&key)
                .is_none_or(|old| jsonb_text(p) > jsonb_text(old));
            if replace {
                unique.insert(key, p.clone());
            }
        }
        asset["params"] = json!(unique.into_values().collect::<Vec<_>>());
    }
}
fn projected_records(b: &Planner, collection: &str) -> BTreeMap<String, Value> {
    let mut records = b
        .state
        .records(collection)
        .into_iter()
        .map(|r| (r.header.id.clone(), r.data.clone()))
        .collect::<BTreeMap<_, _>>();
    for ch in &b.batch.changes {
        match ch {
            Change::Put {
                collection: c,
                id,
                data,
            } if c == collection => {
                records.insert(id.clone(), data.clone());
            }
            Change::Delete { collection: c, id } if c == collection => {
                records.remove(id);
            }
            _ => {}
        }
    }
    records
}
fn resolve_company(b: &Planner, asset: &Value) -> Value {
    let companies = projected_records(b, "companies");
    let rules = projected_records(b, "company_scope");
    let typ = text(asset, "type");
    let root = text(asset, "root_domain");
    let ip = text(asset, "ip").parse::<IpAddr>().ok();
    let icps = ["icp", "app_icp"].map(|field| {
        text(asset, field)
            .chars()
            .filter(|c| !c.is_whitespace())
            .collect::<String>()
            .to_lowercase()
    });
    let mut matches = vec![];
    for rule in rules.into_values() {
        if !companies.contains_key(text(&rule, "company_id")) {
            continue;
        }
        let score = match text(&rule, "kind") {
            "domain"
                if ["root_domain", "subdomain", "service", "endpoint"].contains(&typ)
                    && !root.is_empty()
                    && text(&rule, "domain") == root =>
            {
                Some(1000u16)
            }
            "ip" | "cidr" if ["ip", "subdomain", "service", "endpoint"].contains(&typ) => {
                let net = text(&rule, "net");
                let parsed = net.parse::<IpNet>().ok().or_else(|| {
                    net.parse::<IpAddr>()
                        .ok()
                        .and_then(|i| IpNet::new(i, if i.is_ipv4() { 32 } else { 128 }).ok())
                });
                parsed
                    .zip(ip)
                    .filter(|(n, i)| n.contains(i))
                    .map(|(n, _)| 500 + n.prefix_len() as u16)
            }
            "icp"
                if icps
                    .iter()
                    .any(|icp| !icp.is_empty() && text(&rule, "value") == icp) =>
            {
                Some(100)
            }
            _ => None,
        };
        if let Some(score) = score {
            matches.push((score, text(&rule, "company_id").to_owned()));
        }
    }
    matches.sort_by(|(ascore, aid), (bscore, bid)| {
        bscore
            .cmp(ascore)
            .then_with(|| {
                match (b.state.get("companies", aid), b.state.get("companies", bid)) {
                    (Some(a), Some(b)) => {
                        // Imported numeric IDs retain the source's company_id
                        // tie break; fresh records use creation time and ID.
                        match (
                            a.header
                                .legacy_key
                                .as_str()
                                .and_then(|v| v.parse::<u64>().ok()),
                            b.header
                                .legacy_key
                                .as_str()
                                .and_then(|v| v.parse::<u64>().ok()),
                        ) {
                            (Some(a), Some(b)) => a.cmp(&b),
                            _ => a.header.created_at.cmp(&b.header.created_at),
                        }
                    }
                    // Newly staged companies will receive the commit time.
                    // Missing snapshot timestamps must not sort before old rows.
                    (Some(_), None) => std::cmp::Ordering::Less,
                    (None, Some(_)) => std::cmp::Ordering::Greater,
                    (None, None) => {
                        let position = |id: &str| {
                            b.batch.changes.iter().position(|change| {
                                matches!(change, Change::Put { collection, id: i, .. }
                                if collection == "companies" && i == id)
                            })
                        };
                        position(aid).cmp(&position(bid))
                    }
                }
            })
            .then_with(|| aid.cmp(bid))
    });
    matches
        .first()
        .map(|(_, id)| json!(id))
        .unwrap_or(Value::Null)
}
pub fn recompute_attribution(b: &mut Planner) -> Result<()> {
    // Include cascade SET NULL before deciding whether explicit ownership is
    // still valid. A deleted explicit owner may fall back to a remaining scope.
    let mut assets = projected_records(b, "assets");
    for change in artex_core::expand_deletions(b.state, &b.batch.changes)? {
        match change {
            Change::Put {
                collection,
                id,
                data,
            } if collection == "assets" => {
                assets.insert(id, data);
            }
            Change::Delete { collection, id } if collection == "assets" => {
                assets.remove(&id);
            }
            _ => {}
        }
    }
    for (id, mut data) in assets {
        if text(&data, "company_source") == "explicit" && !data["company_id"].is_null() {
            continue;
        }
        let resolved = resolve_company(b, &data);
        if resolved != data["company_id"] {
            data["company_id"] = resolved;
            data["company_source"] = json!("scope");
            b.put("assets", &id, data);
        }
    }
    Ok(())
}
fn classify_scope(raw: &str) -> Result<(String, Value)> {
    let v = raw.trim();
    if v.is_empty() {
        return Err(Error::new("INVALID_PARAMS", "empty scope"));
    }
    if v.to_lowercase().contains("icp") {
        return Ok((
            "icp".into(),
            json!({"value":v.chars().filter(|c|!c.is_whitespace()).collect::<String>().to_lowercase()}),
        ));
    }
    if v.chars().any(char::is_whitespace) {
        return Ok(("keyword".into(), json!({"value":company_key(v)})));
    }
    if let Ok(net) = v.parse::<IpNet>() {
        let min = if net.addr().is_ipv4() { 16 } else { 32 };
        if net.prefix_len() < min {
            return Err(Error::new("INVALID_PARAMS", "network scope is too broad"));
        }
        return Ok(("cidr".into(), json!({"net":net.trunc().to_string()})));
    }
    if v.parse::<IpAddr>().is_ok() {
        return Ok(("ip".into(), json!({"net":v})));
    }
    let domain = Url::parse(v)
        .or_else(|_| Url::parse(&format!("http://{v}")))
        .ok()
        .and_then(|u| u.host_str().map(domain_key))
        .unwrap_or_else(|| domain_key(v));
    if let Ok(ip) = domain.parse::<IpAddr>() {
        return Ok(("ip".into(), json!({"net":ip.to_string()})));
    }
    let (suffix, icann) = crate::psl::public_suffix(&domain);
    if icann && suffix == domain {
        return Err(Error::new(
            "INVALID_PARAMS",
            "public suffix cannot be a company scope",
        ));
    }
    if domain.split('.').count() == 4
        && domain
            .split('.')
            .all(|label| label.chars().all(|c| c.is_ascii_digit()))
    {
        return Err(Error::new("INVALID_PARAMS", "invalid IP scope"));
    }
    if domain.contains('.') && !domain.contains(' ') && !domain.contains('/') {
        let (suffix, icann) = crate::psl::public_suffix(&domain);
        if icann && suffix == domain {
            return Err(Error::new(
                "INVALID_PARAMS",
                "public suffix cannot be a company scope",
            ));
        }
        return Ok(("domain".into(), json!({"domain":domain})));
    }
    Ok(("keyword".into(), json!({"value":company_key(v)})))
}
pub fn add_scope(
    b: &mut Planner,
    task: &str,
    kind: &str,
    value: &str,
    reason: &Value,
    source: &str,
) -> Result<(String, Value)> {
    let value = value.trim();
    if value.is_empty() {
        return Err(Error::new("INVALID_PARAMS", "Scope value cannot be empty"));
    }
    let bare = if value.starts_with('[') {
        value
            .split(']')
            .next()
            .map(|s| s.trim_start_matches('['))
            .unwrap_or(value)
    } else if value.matches(':').count() == 1 {
        value
            .rsplit_once(':')
            .filter(|(_, port)| port.parse::<u16>().is_ok())
            .map(|(host, _)| host)
            .unwrap_or(value)
    } else {
        value
    };
    let mut d = default_data("task_scope")?;
    merge(
        &mut d,
        &json!({"task_id":task,"kind":kind,"source":source,"reason":reason}),
    );
    match kind {
        "company" => {
            let key = company_key(value);
            let company = b
                .state
                .records("companies")
                .into_iter()
                .find(|r| r.header.id == value || company_key(text(&r.data, "name")) == key)
                .ok_or_else(|| Error::new("NOT_FOUND", "Company does not exist"))?;
            d["company_id"] = json!(company.header.id);
        }
        "root_domain" | "subdomain" => {
            let host = domain_key(bare);
            if host.is_empty() {
                return Err(Error::new("INVALID_PARAMS", "Domain cannot be empty"));
            }
            let host = if kind == "root_domain" {
                {
                    let root = crate::psl::root_domain(&host);
                    if root.is_empty() { host } else { root }
                }
            } else {
                host
            };
            d["domain"] = json!(host);
        }
        "ip" | "cidr" => {
            let net = if value.contains('/') {
                d["kind"] = json!("cidr");
                value
                    .parse::<IpNet>()
                    .map_err(|_| Error::new("INVALID_PARAMS", "Invalid CIDR"))?
                    .trunc()
                    .to_string()
            } else {
                d["kind"] = json!("ip");
                let ip = bare
                    .parse::<IpAddr>()
                    .map_err(|_| Error::new("INVALID_PARAMS", "Invalid IP"))?;
                format!("{ip}/{}", if ip.is_ipv4() { 32 } else { 128 })
            };
            d["net"] = json!(net);
        }
        "icp" => {
            let normalized = value
                .chars()
                .filter(|c| !c.is_whitespace())
                .collect::<String>()
                .to_lowercase();
            d["value"] = json!(normalized);
        }
        "keyword" => {
            d["value"] = json!(company_key(value));
        }
        _ => return Err(Error::new("INVALID_PARAMS", "Unsupported scope kind")),
    }
    let same = |old: &Value| {
        ["task_id", "kind", "company_id", "domain", "net", "value"]
            .iter()
            .all(|key| old[*key] == d[*key])
    };
    if let Some(r) = b
        .state
        .records("task_scope")
        .into_iter()
        .find(|r| same(&r.data))
    {
        return Ok((r.header.id.clone(), d));
    }
    if let Some(id) = b.batch.changes.iter().find_map(|ch| match ch {
        Change::Put {
            collection,
            id,
            data,
        } if collection == "task_scope" && same(data) => Some(id.clone()),
        _ => None,
    }) {
        return Ok((id, d));
    }
    d["created_at"] = json!(artex_core::now());
    let id = b.insert("task_scope", "scope", d.clone())?;
    Ok((id, d))
}
/// The frozen gate evaluates global blocks, task blocks, then the task whitelist.
pub fn check_asset_gate(
    b: &mut Planner,
    task: Option<&str>,
    asset: &Value,
    persisted: bool,
) -> Result<()> {
    let mut domains = ids(asset, "bound_domains");
    let mut ips = vec![];
    let mut urls = vec![];
    for key in if persisted {
        vec!["domain", "root_domain"]
    } else {
        vec!["domain"]
    } {
        if !text(asset, key).trim().is_empty() {
            domains.push(text(asset, key).trim().into());
        }
    }
    for key in ["ip", "service_ip"] {
        if !text(asset, key).trim().is_empty() {
            ips.push(text(asset, key).trim().to_owned());
        }
    }
    if !text(asset, "url").trim().is_empty() {
        urls.push(text(asset, "url").trim().to_owned());
        if let Ok(u) = Url::parse(text(asset, "url")) {
            if let Some(host) = u.host_str() {
                let host = host.trim_matches(['[', ']']);
                if host.parse::<IpAddr>().is_ok() {
                    ips.push(host.into());
                } else {
                    domains.push(host.into());
                }
            }
        }
    }
    let matches = |r: &Record| {
        let pat = text(&r.data, "pattern").trim();
        if pat.is_empty() {
            return false;
        }
        let values = match text(&r.data, "kind") {
            "exact_domain" | "fuzzy_domain" => &domains,
            "exact_ip" | "fuzzy_ip" | "cidr" => &ips,
            _ => &urls,
        };
        values.iter().any(|v| match text(&r.data, "kind") {
            "exact_domain" => v.trim().eq_ignore_ascii_case(pat),
            "exact_ip" | "exact_url" => v.trim() == pat,
            "fuzzy_domain" | "fuzzy_url" => v.to_lowercase().contains(&pat.to_lowercase()),
            "fuzzy_ip" => v.contains(pat),
            "cidr" => pat
                .parse::<IpNet>()
                .ok()
                .is_some_and(|net| v.parse::<IpAddr>().ok().is_some_and(|ip| net.contains(&ip))),
            _ => false,
        })
    };
    let mut block = b
        .state
        .records("asset_intercept_rules")
        .into_iter()
        .filter(|r| r.data["enabled"] == true)
        .collect::<Vec<_>>();
    let task_rules = b
        .state
        .records("task_intercept_rules")
        .into_iter()
        .filter(|r| task == Some(text(&r.data, "task_id")) && r.data["enabled"] == true)
        .collect::<Vec<_>>();
    block.extend(
        task_rules
            .iter()
            .copied()
            .filter(|r| r.data["action"] == "block"),
    );
    let allow = task_rules
        .iter()
        .copied()
        .filter(|r| r.data["action"] == "allow")
        .collect::<Vec<_>>();
    for r in block.iter().chain(allow.iter()) {
        b.expect(r);
    }
    if let Some(r) = block.into_iter().find(|r| matches(r)) {
        return Err(Error::new(
            "PERMISSION_DENIED",
            &format!(
                "Asset blocked by {}: {} ({})",
                text(&r.data, "kind"),
                text(&r.data, "pattern"),
                text(&r.data, "note")
            ),
        ));
    }
    if !allow.is_empty() && !allow.into_iter().any(|r| matches(r)) {
        return Err(Error::new(
            "PERMISSION_DENIED",
            "Asset is outside the task whitelist",
        ));
    }
    Ok(())
}
fn register_primary(
    b: &mut Planner,
    task: &str,
    id: &str,
    item: &Value,
    c: &Context,
) -> Result<()> {
    let asset = b.data("assets", id)?;
    let exp = exploration_id(b.state, task)?;
    let owner = c.intent_id.clone().or_else(|| {
        b.state
            .records("exploration_nodes")
            .into_iter()
            .find(|n| {
                n.data["exploration_id"] == exp
                    && n.data["kind"] == "fact"
                    && n.data["state"] == "origin"
            })
            .map(|n| n.header.id.clone())
    });
    if let Some(node) = &owner {
        own_node(b.state, task, node)?;
        if !b.state.records("exploration_anchors").iter().any(|r|r.data["node_id"]==*node&&r.data["asset_id"]==id)&&!b.batch.changes.iter().any(|ch|matches!(ch,Change::Put{collection,data,..}if collection=="exploration_anchors"&&data["node_id"]==*node&&data["asset_id"]==id)){b.insert("exploration_anchors","anchor",json!({"node_id":node,"asset_id":id}))?;}
    }
    let link = b
        .batch
        .changes
        .iter()
        .find_map(|ch| match ch {
            Change::Put {
                collection,
                id: link,
                data,
            } if collection == "task_asset_links"
                && data["task_id"] == task
                && data["asset_id"] == id =>
            {
                Some(link.clone())
            }
            _ => None,
        })
        .or_else(|| {
            b.state
                .records("task_asset_links")
                .into_iter()
                .find(|r| r.data["task_id"] == task && r.data["asset_id"] == id)
                .map(|r| r.header.id.clone())
        });
    if let Some(link) = link {
        let mut d = b.data("task_asset_links", &link)?;
        d["source"] = json!("agent");
        d["source_summary"] = json!(if let Some(node) = &owner {
            format!("Worker intent {node} registered asset using insert_assets")
        } else {
            "Agent registered asset using insert_assets".into()
        });
        d["source_node_id"] = json!(owner);
        b.put("task_asset_links", &link, d);
    }
    let typ = text(&asset, "type");
    let domain = text(&asset, "domain");
    let raw_ip = if !text(item, "service_ip").is_empty() {
        text(item, "service_ip")
    } else {
        text(item, "ip")
    };
    let (kind, value) = match typ {
        "root_domain" | "subdomain" => (typ, domain.to_owned()),
        "service" | "endpoint" if !domain.is_empty() && domain.parse::<IpAddr>().is_err() => {
            ("subdomain", domain.to_owned())
        }
        "service" | "endpoint" | "ip" => ("ip", raw_ip.to_owned()),
        _ => return Ok(()),
    };
    if !value.is_empty() {
        add_scope(b, task, kind, &value, &Value::Null, "auto")?;
    }
    Ok(())
}
pub fn plan(m: &str, p: &Value, s: &Snapshot, c: &Context) -> Result<CommandBatch> {
    let mut b = Planner::new(s, m, p);
    match m {
        "company.create" | "add_company_scope" => {
            let name = required(
                p,
                if m == "company.create" {
                    "name"
                } else {
                    "company"
                },
            )?
            .trim();
            let existing = s
                .records("companies")
                .into_iter()
                .find(|r| text(&r.data, "nkey") == company_key(name));
            let id = if let Some(r) = existing {
                if m == "company.create" {
                    return Err(Error::new(
                        "VERSION_CONFLICT",
                        "company name already exists",
                    ));
                }
                r.header.id.clone()
            } else {
                b.insert(
                    "companies",
                    "company",
                    json!({"name":name,"nkey":company_key(name),"logo":p["logo"]}),
                )?
            };
            let mut errors = vec![];
            let mut added = 0;
            let mut skipped = 0;
            if m == "add_company_scope" {
                for line in required(p, "scope")?.lines() {
                    if line.trim().is_empty() {
                        continue;
                    }
                    match classify_scope(line) {
                        Ok((kind, patch)) => {
                            if projected_records(&b, "company_scope")
                                .iter()
                                .any(|(_, data)| {
                                    text(data, "company_id") == id
                                        && text(data, "kind") == kind
                                        && ["domain", "net", "value"]
                                            .iter()
                                            .all(|k| patch.get(k).is_none_or(|v| data[k] == *v))
                                })
                            {
                                skipped += 1;
                                continue;
                            }
                            let mut d = json!({"company_id":id,"kind":kind,"raw":line.trim(),"reason":p["reason"]});
                            for (k, v) in patch.as_object().unwrap() {
                                d[k] = v.clone();
                            }
                            b.insert("company_scope", "companyscope", d)?;
                            added += 1;
                        }
                        Err(e) => errors.push(json!({"raw":line,"error":e.message})),
                    }
                }
            }
            if m == "add_company_scope" {
                recompute_attribution(&mut b)?;
            }
            let mut result =
                json!({"company_id":id,"added":added,"skipped":skipped,"invalid":errors.len()});
            if !errors.is_empty() {
                result["errors"] = json!(
                    errors
                        .iter()
                        .map(|e| format!("{}: {}", text(e, "raw"), text(e, "error")))
                        .collect::<Vec<_>>()
                );
            }
            Ok(b.done(result))
        }
        "company.update" => {
            let id = required(p, "id")?;
            let mut d = record(s, "companies", id)?.data.clone();
            merge(&mut d, &p["patch"]);
            d["nkey"] = json!(company_key(required(&d, "name")?));
            b.put("companies", id, d);
            Ok(b.done(json!({"id":id})))
        }
        "company.delete" | "asset.delete" => {
            let collection = if m == "company.delete" {
                "companies"
            } else {
                "assets"
            };
            let id = required(p, "id")?;
            record(s, collection, id)?;
            b.delete(collection, id);
            if m == "company.delete" {
                recompute_attribution(&mut b)?;
            }
            Ok(b.done(json!({"id":id})))
        }
        "asset.upsert" | "insert_assets" => {
            let task = p["task_id"].as_str().or(c.task_id.as_deref());
            let items = if m == "insert_assets" {
                p["assets"]
                    .as_array()
                    .ok_or_else(|| Error::new("INVALID_PARAMS", "assets array is required"))?
                    .clone()
            } else {
                vec![
                    p["asset"]
                        .as_object()
                        .map(|_| p["asset"].clone())
                        .unwrap_or_else(|| p.clone()),
                ]
            };
            let mut out = vec![];
            let mut errors = serde_json::Map::new();
            for (i, it) in items.into_iter().enumerate() {
                let prior = b.batch.clone();
                let outcome = (|| -> Result<String> {
                    if m == "insert_assets" {
                        check_asset_gate(&mut b, task, &it, false)?;
                    }
                    let id = upsert(&mut b, &it, task)?;
                    if m == "insert_assets" {
                        if let Some(task) = task {
                            register_primary(&mut b, task, &id, &it, c)?;
                        }
                    }
                    Ok(id)
                })();
                match outcome {
                    Ok(id) => out.push(json!(id)),
                    Err(e) => {
                        if m == "asset.upsert" {
                            return Err(e);
                        }
                        b.batch = prior;
                        out.push(Value::Null);
                        errors.insert(i.to_string(), json!({"code":e.code,"message":e.message}));
                    }
                }
            }
            Ok(b.done(json!({"ids":out,"errors":errors})))
        }
        "delete_assets_by_host" => {
            let host = domain_key(required(p, "host")?);
            let mut deleted = vec![];
            let mut counts = serde_json::Map::new();
            for r in s.records("assets") {
                if domain_key(text(&r.data, "domain")) == host
                    || domain_key(text(&r.data, "root_domain")) == host
                    || text(&r.data, "ip") == host
                {
                    deleted.push(r.header.id.clone());
                    let typ = text(&r.data, "type");
                    counts.insert(
                        typ.into(),
                        json!(counts.get(typ).and_then(Value::as_u64).unwrap_or(0) + 1),
                    );
                    b.delete("assets", &r.header.id);
                }
            }
            Ok(b.done(json!({"host":p["host"],"deleted":deleted.len(),"deleted_by_type":counts})))
        }
        "add_task_scope" => {
            let task = task_id(p, c)?;
            record(s, "tasks", task)?;
            let entries = p["entries"]
                .as_array()
                .cloned()
                .unwrap_or_else(|| vec![p.clone()]);
            let mut added = vec![];
            let mut errors = serde_json::Map::new();
            for (index, e) in entries.iter().enumerate() {
                match required(e,"kind").and_then(|k|required(e,"value").and_then(|v|add_scope(&mut b,task,k.trim(),v,&p["reason"],"agent"))){
                    Ok((_,d))=>added.push(json!({"kind":d["kind"],"domain":d["domain"].as_str().unwrap_or(""),"net":d["net"].as_str().unwrap_or(""),"value":d["value"].as_str().unwrap_or(""),"company_id":d["company_id"]})),
                    Err(e)=>{errors.insert(index.to_string(),json!(e.message));}
                }
            }
            let mut result = json!({"added":added});
            if !errors.is_empty() {
                result["errors"] = json!(errors);
            }
            Ok(b.done(result))
        }
        _ => Err(Error::new("METHOD_NOT_FOUND", m)),
    }
}

pub fn declared_scope(s: &Snapshot, task: &str, a: &Value, ip_literal_domain: bool) -> bool {
    let targets = direct_sources(s, task);
    for scope in s.records("task_scope") {
        if !targets.contains(text(&scope.data, "task_id")) {
            continue;
        }
        match text(&scope.data, "kind") {
            "company"
                if !a["company_id"].is_null() && a["company_id"] == scope.data["company_id"] =>
            {
                return true;
            }
            "root_domain" | "subdomain" => {
                let d = text(
                    a,
                    if scope.data["kind"] == "root_domain" {
                        "root_domain"
                    } else {
                        "domain"
                    },
                );
                let base = text(&scope.data, "domain");
                if d == base {
                    return true;
                }
            }
            "ip" | "cidr" => {
                let net = text(&scope.data, "net");
                if ["ip", "domain"]
                    .iter()
                    .filter(|key| **key == "ip" || ip_literal_domain)
                    .any(|key| {
                        let ip = text(a, key);
                        ip == net
                            || net
                                .parse::<IpNet>()
                                .ok()
                                .zip(ip.parse::<IpAddr>().ok())
                                .is_some_and(|(n, i)| n.contains(&i))
                    })
                {
                    return true;
                }
            }
            "icp" => {
                if ["icp", "app_icp"].iter().any(|key| {
                    text(a, key)
                        .chars()
                        .filter(|c| !c.is_whitespace())
                        .collect::<String>()
                        .to_lowercase()
                        == text(&scope.data, "value")
                }) {
                    return true;
                }
            }
            _ => {}
        }
    }
    false
}
pub fn query(m: &str, p: &Value, s: &Snapshot, c: &Context) -> Result<Value> {
    if matches!(m, "company.list" | "list_companies") {
        let q = text(p, "q").to_lowercase();
        return Ok(json!(
            s.records("companies")
                .iter()
                .filter(|r| text(&r.data, "name").to_lowercase().contains(&q))
                .map(|r| r.data.clone())
                .collect::<Vec<_>>()
        ));
    }
    let task = p["task_id"].as_str().or(c.task_id.as_deref());
    let mut chosen = ids(p, "ids");
    if !text(p, "id").is_empty() {
        chosen.push(text(p, "id").to_owned());
    }
    if m == "list_assets" && chosen.is_empty() && text(p, "dsl").is_empty() {
        return Err(Error::new(
            "INVALID_PARAMS",
            "id, ids or non-empty DSL required",
        ));
    }
    let dsl = if text(p, "dsl").is_empty() {
        None
    } else {
        Some(crate::exploration::Dsl::parse(text(p, "dsl"))?)
    };
    let mut rows = vec![];
    for r in s.records("assets") {
        let d = &r.data;
        if task.is_some_and(|id| !declared_scope(s, id, d, true)) {
            continue;
        }
        if !chosen.is_empty() && !chosen.contains(&r.header.id) {
            continue;
        }
        if !text(p, "type").is_empty() && text(d, "type") != text(p, "type") {
            continue;
        }
        if let Some(expr) = &dsl {
            if !expr.matches(d)? {
                continue;
            }
        }
        if m == "list_untested_assets" {
            let tested = s.records("exploration_anchors").iter().any(|a| {
                text(&a.data, "asset_id") == r.header.id
                    && s.get("exploration_nodes", text(&a.data, "node_id"))
                        .is_some_and(|n| {
                            text(&n.data, "kind") == "intent"
                                && matches!(text(&n.data, "state"), "done" | "running")
                        })
            });
            if tested {
                continue;
            }
        }
        let mut projected = d.clone();
        projected["task_ids"] = json!(
            s.records("task_asset_links")
                .iter()
                .filter(|l| text(&l.data, "asset_id") == r.header.id)
                .map(|l| text(&l.data, "task_id").to_owned())
                .collect::<BTreeSet<_>>()
        );
        rows.push(projected);
    }
    let total = rows.len();
    let offset = p["offset"].as_u64().unwrap_or(0) as usize;
    let limit = p["limit"].as_u64().unwrap_or(10).min(1000) as usize;
    Ok(
        json!({"assets":rows.into_iter().skip(offset).take(limit).collect::<Vec<_>>(),"total":total}),
    )
}
