use crate::*;
use base64::{Engine, engine::general_purpose::STANDARD};
use std::collections::BTreeMap;
pub const QUERIES: &[&str] = &[
    "ui.traffic",
    "ui.trafficExchange",
    "ui.trafficHosts",
    "ui.findingTrafficDetail",
    "ui.findingTrafficBody",
    "ui.downloadFindingTrafficBody",
    "ui.exportFindings",
    "ui.report",
    "ui.audit",
];
pub const COMMANDS: &[&str] = &[
    "ui.trafficDeleteHost",
    "ui.trafficDeleteHosts",
    "ui.trafficDeleteAll",
];
pub type BlobReader<'a> = dyn Fn(&Value) -> Result<Vec<u8>> + 'a;
pub fn body_bytes(v: &Value, reader: &BlobReader) -> Result<Vec<u8>> {
    if v.is_null() {
        return Ok(vec![]);
    }
    if let Some(s) = v.as_str().or(v["text"].as_str()) {
        return Ok(s.as_bytes().to_vec());
    }
    if let Some(blob) = v.get("blob").filter(|b| !b.is_null()) {
        return reader(blob);
    }
    if v.get("sha256").is_some() {
        return reader(v);
    }
    Err(Error::new(
        "INVALID_PARAMS",
        "Value is not a body or blob reference",
    ))
}
fn exchange_body<'a>(s: &'a Snapshot, id: &str) -> Result<&'a Record> {
    record(s, "exchanges", id)?;
    s.records("exchange_bodies")
        .into_iter()
        .find(|r| r.data["id"] == id)
        .ok_or_else(|| Error::new("NOT_FOUND", "Capture body references not found"))
}
fn raw(s: &Snapshot, id: &str, reader: &BlobReader) -> Result<(String, String)> {
    let b = exchange_body(s, id)?;
    let side = |head: &str, body: &str| -> Result<String> {
        let head = String::from_utf8_lossy(&body_bytes(&b.data[head], reader)?).to_string();
        let body = String::from_utf8_lossy(&body_bytes(&b.data[body], reader)?).to_string();
        Ok(format!(
            "{}{}{}",
            head,
            if head.ends_with("\r\n\r\n") {
                ""
            } else {
                "\r\n\r\n"
            },
            body
        ))
    };
    Ok((
        side("req_head", "req_body")?,
        side("resp_head", "resp_body")?,
    ))
}
pub fn evidence_body(snapshot: &Value, side: &str, reader: &BlobReader) -> Result<Vec<u8>> {
    let prefix = match side {
        "request" => "req",
        "response" => "resp",
        _ => {
            return Err(Error::new(
                "INVALID_PARAMS",
                "Side must be request or response",
            ));
        }
    };
    let len = required(snapshot, &format!("{prefix}_len"))?;
    if len == "0" {
        return Ok(vec![]);
    }
    reader(
        &json!({"sha256":snapshot[format!("{prefix}_hash")],"length":len,"media_type":"application/octet-stream"}),
    )
}
fn preview(bytes: &[u8], offset: usize) -> Result<Value> {
    preview_length(bytes, offset, 8192)
}
fn preview_length(bytes: &[u8], offset: usize, length: usize) -> Result<Value> {
    if offset > bytes.len() {
        return Err(Error::new("INVALID_PARAMS", "Offset exceeds body length"));
    }
    let mut end = offset.saturating_add(length).min(bytes.len());
    if end < bytes.len() && end - offset >= 4 {
        let mut start = end - 1;
        while start > offset && bytes[start] & 0xc0 == 0x80 {
            start -= 1;
        }
        if let Err(e) = std::str::from_utf8(&bytes[start..end]) {
            if e.error_len().is_none() {
                end = start;
            }
        }
    }
    let raw = &bytes[offset..end];
    let binary = raw.contains(&0) || (offset == 0 && std::str::from_utf8(raw).is_err());
    let content = if binary {
        format!("[binary body, {} bytes; download to inspect]", bytes.len())
    } else {
        String::from_utf8_lossy(raw).to_string()
    };
    Ok(
        json!({"content":content,"offset":offset,"total":bytes.len(),"next_offset":end,"truncated":end<bytes.len(),"binary":binary}),
    )
}
fn bound<'a>(p: &Value, s: &'a Snapshot, c: &Context) -> Result<&'a Record> {
    findings_ui::query("ui.findingTraffic", p, s, c)?;
    let fid = required(p, "id")?;
    let r = record(s, "finding_traffic_bindings", required(p, "bindingId")?)?;
    if r.data["finding_id"] != fid {
        return Err(Error::new("NOT_FOUND", "Evidence binding not found"));
    }
    Ok(r)
}
fn download(bytes: &[u8], mime: &str, filename: &str) -> Value {
    json!({"content_base64":artex_core::body(&STANDARD.encode(bytes)),"mime":mime,"filename":filename})
}
fn time(v: &Value) -> String {
    let n = v.as_str().unwrap_or("").parse::<i64>().unwrap_or(0);
    chrono::DateTime::from_timestamp(n, 0)
        .map(|t| t.to_rfc3339_opts(chrono::SecondsFormat::Secs, true))
        .unwrap_or_default()
}
pub fn query(m: &str, p: &Value, s: &Snapshot, c: &Context, reader: &BlobReader) -> Result<Value> {
    match m {
        "ui.trafficHosts" => {
            let mut hosts = BTreeMap::<String, usize>::new();
            for r in s.records("exchanges") {
                *hosts.entry(text(&r.data, "host").into()).or_default() += 1;
            }
            let mut out = hosts
                .into_iter()
                .map(|(host, count)| json!({"host":host,"count":count}))
                .collect::<Vec<_>>();
            out.sort_by(|a, b| {
                b["count"]
                    .as_u64()
                    .cmp(&a["count"].as_u64())
                    .then_with(|| text(a, "host").cmp(text(b, "host")))
            });
            Ok(json!({"hosts":out}))
        }
        "ui.trafficExchange" => {
            let (req, resp) = raw(s, required(p, "id")?, reader)?;
            Ok(json!({"req":artex_core::body(&req),"resp":artex_core::body(&resp)}))
        }
        "ui.traffic" => {
            let opts = &p["opts"];
            let host = text(p, "host").trim().to_lowercase();
            let method = text(p, "method").trim().to_uppercase();
            let q = text(p, "q").trim().to_lowercase();
            let body = text(opts, "body").trim().to_lowercase();
            let path = text(opts, "path").trim().to_lowercase();
            let status = text(opts, "status").trim().to_lowercase();
            let min = text(opts, "respMin").parse::<u64>().ok();
            let max = text(opts, "respMax").parse::<u64>().ok();
            let mut rows = vec![];
            for r in s.records("exchanges") {
                let d = &r.data;
                if !text(d, "host").to_lowercase().contains(&host)
                    || !matches!(method.as_str(), "" | "ALL") && d["method"] != method
                    || !text(d, "url_template").to_lowercase().contains(&path)
                {
                    continue;
                }
                let code = d["status"].as_i64().unwrap_or(0);
                let status_match = if status.len() == 3 && status.ends_with("xx") {
                    status[..1]
                        .parse::<i64>()
                        .map(|n| code / 100 == n)
                        .unwrap_or(true)
                } else {
                    status.parse::<i64>().map(|n| code == n).unwrap_or(true)
                };
                let len = text(d, "resp_len").parse::<u64>().unwrap_or(0);
                if !status_match || min.is_some_and(|n| len < n) || max.is_some_and(|n| len > n) {
                    continue;
                }
                let meta_match = q.is_empty()
                    || [
                        "host",
                        "url",
                        "url_template",
                        "method",
                        "content_type",
                        "status",
                    ]
                    .iter()
                    .any(|k| {
                        d[*k]
                            .as_str()
                            .map(str::to_owned)
                            .unwrap_or_else(|| d[*k].to_string())
                            .to_lowercase()
                            .contains(&q)
                    });
                let requires_bodies =
                    (!meta_match && q.chars().count() >= 3) || body.chars().count() >= 3;
                if requires_bodies {
                    let (req, resp) = raw(s, &r.header.id, reader)?;
                    let corpus = format!("{req}\n{resp}").to_lowercase();
                    if !meta_match && !corpus.contains(&q)
                        || body.chars().count() >= 3 && !corpus.contains(&body)
                    {
                        continue;
                    }
                } else if !meta_match {
                    continue;
                }
                rows.push(r);
            }
            let sort = match text(opts, "sort") {
                "status" => "status",
                "resp_len" => "resp_len",
                _ => "ts",
            };
            rows.sort_by(|a, b| {
                let numeric = |r: &Record| {
                    r.data[sort]
                        .as_i64()
                        .unwrap_or_else(|| text(&r.data, sort).parse().unwrap_or(0))
                };
                numeric(a)
                    .cmp(&numeric(b))
                    .then_with(|| a.header.id.cmp(&b.header.id))
            });
            if opts["order"] != "asc" {
                rows.reverse();
            }
            let total = rows.len();
            let page = p["page"].as_u64().unwrap_or(0);
            let size = p["size"]
                .as_u64()
                .filter(|n| *n > 0 && *n <= 500)
                .unwrap_or(100);
            let out=rows.into_iter().skip(page.saturating_mul(size)as usize).take(size as usize).map(|r|json!({"id":r.header.id,"ts":time(&r.data["ts"]),"host":r.data["host"],"method":r.data["method"],"url":r.data["url"],"status":r.data["status"],"content_type":text(&r.data,"content_type"),"resp_len":r.data["resp_len"]})).collect::<Vec<_>>();
            Ok(
                json!({"enabled":setting_bool(s,"traffic_capture",true),"proxy":s.records("settings").iter().find(|r|r.data["key"]=="proxy_addr").map(|r|text(&r.data,"value")).unwrap_or(""),"count":s.records("exchanges").len(),"total":total,"page":page,"size":size,"exchanges":out}),
            )
        }
        "ui.findingTrafficDetail" | "ui.findingTrafficBody" | "ui.downloadFindingTrafficBody" => {
            let binding = bound(p, s, c)?;
            let snap = &record(
                s,
                "traffic_evidence_snapshots",
                required(&binding.data, "snapshot_id")?,
            )?
            .data;
            if m == "ui.findingTrafficDetail" {
                let req = evidence_body(snap, "request", reader)?;
                let resp = evidence_body(snap, "response", reader)?;
                Ok(
                    json!({"binding":findings_ui::binding(&binding.data,s)?,"request":preview(&req,0)?,"response":preview(&resp,0)?}),
                )
            } else {
                let side = required(p, "side")?;
                let bytes = evidence_body(snap, side, reader)?;
                if m == "ui.downloadFindingTrafficBody" {
                    Ok(download(
                        &bytes,
                        "application/octet-stream",
                        &format!("evidence-{}-{side}.bin", binding.header.id),
                    ))
                } else {
                    let offset = p["offset"].as_u64().ok_or_else(|| {
                        Error::new("INVALID_PARAMS", "Offset must be a nonnegative integer")
                    })?;
                    preview(
                        &bytes,
                        usize::try_from(offset).map_err(|_| {
                            Error::new("SIZE_LIMIT", "Offset exceeds platform size")
                        })?,
                    )
                }
            }
        }
        "ui.report" => report_export::task_report(p, s, reader),
        "ui.exportFindings" => report_export::export(p, s, reader),
        "ui.audit" => {
            let task = exploration_ui::resolve_task(s, p)?;
            let Some(task) = task else {
                return Ok(json!({"entries":[],"attributions":{}}));
            };
            let exp = text(&task.data, "exploration_id");
            let mut activities = s
                .records("activity")
                .into_iter()
                .filter(|r| r.data["exploration_id"] == exp && r.data["kind"] == "guard")
                .collect::<Vec<_>>();
            activities.sort_by(|a, b| {
                text(&a.data, "created_at")
                    .cmp(text(&b.data, "created_at"))
                    .then_with(|| a.header.id.cmp(&b.header.id))
            });
            let mut entries = vec![];
            let mut attributions = BTreeMap::<String, usize>::new();
            for row in activities {
                let detail = body_bytes(&row.data["detail"], reader)?;
                let Ok(detail) = serde_json::from_slice::<Value>(&detail) else {
                    continue;
                };
                if let Some(audit) = detail.get("guard_audit").filter(|v| v.is_object()) {
                    let mut entry =
                        json!({"ts":audit["ts"],"tool":audit["tool"],"action":audit["action"]});
                    for key in ["reason", "command"] {
                        if !text(audit, key).is_empty() {
                            entry[key] = audit[key].clone();
                        }
                    }
                    entries.push(entry);
                }
                if let Some(attr) = detail["guard_attribution"].as_str() {
                    *attributions.entry(attr.into()).or_default() += 1;
                }
            }
            let entries = entries
                .into_iter()
                .rev()
                .take(2000)
                .collect::<Vec<_>>()
                .into_iter()
                .rev()
                .collect::<Vec<_>>();
            Ok(json!({"entries":entries,"attributions":attributions}))
        }
        _ => Err(Error::new("METHOD_NOT_FOUND", m)),
    }
}
pub fn plan(m: &str, p: &Value, s: &Snapshot, c: &Context) -> Result<CommandBatch> {
    let mut b = Planner::new(s, m, p);
    let hosts = if m == "ui.trafficDeleteHost" {
        vec![required(p, "host")?.trim().to_owned()]
    } else if m == "ui.trafficDeleteHosts" {
        let h = ids(p, "hosts")
            .into_iter()
            .map(|h| h.trim().to_owned())
            .filter(|h| !h.is_empty())
            .collect::<Vec<_>>();
        if h.is_empty() {
            return Err(Error::new(
                "INVALID_PARAMS",
                "At least one host is required",
            ));
        }
        h
    } else if m == "ui.trafficDeleteAll" {
        vec![]
    } else {
        return Err(Error::new("METHOD_NOT_FOUND", m));
    };
    let mut count = 0;
    for r in s.records("exchanges") {
        let host = text(&r.data, "host");
        if m == "ui.trafficDeleteAll"
            || (m == "ui.trafficDeleteHost" && host.contains(&hosts[0]))
            || (m == "ui.trafficDeleteHosts" && hosts.iter().any(|h| h == host))
        {
            b.delete("exchanges", &r.header.id);
            count += 1;
        }
    }
    // Evidence snapshots own independent blob references; capture deletion leaves them intact.
    let result = if m == "ui.trafficDeleteAll" {
        json!({"deleted":count,"reclaimed":0})
    } else {
        json!({"deleted":count})
    };
    if let Some(Change::Delete { collection, id }) = b.batch.changes.first().cloned() {
        conversation::push_event(&mut b.batch, s, c, "TrafficDeleted", &collection, &id)?;
    }
    let mut out = b.done(result);
    augment_expected(&mut out, s)?;
    Ok(out)
}

/// Canonical agent queries use the same verified blob reader as the UI.
pub fn tool_query(
    name: &str,
    p: &Value,
    s: &Snapshot,
    c: &Context,
    reader: &BlobReader,
) -> Result<Value> {
    match name {
        "get_finding_traffic" => {
            let fid = required(p, "finding_id")?;
            let q = json!({"id":fid,"contextTask":c.task_id});
            let mut manifest = findings_ui::query("ui.findingTraffic", &q, s, c)?;
            if text(p, "binding_id").is_empty() {
                for b in manifest["bindings"].as_array_mut().unwrap() {
                    b["snapshot"]["req_head"] = json!("");
                    b["snapshot"]["resp_head"] = json!("");
                }
                return Ok(manifest);
            }
            let binding = bound(
                &json!({"id":fid,"bindingId":p["binding_id"],"contextTask":c.task_id}),
                s,
                c,
            )?;
            let snapshot = &record(
                s,
                "traffic_evidence_snapshots",
                required(&binding.data, "snapshot_id")?,
            )?
            .data;
            let side = if text(p, "side").is_empty() {
                "response"
            } else {
                required(p, "side")?
            };
            let offset = p["offset"].as_i64().unwrap_or(0);
            let length = p["length"].as_i64().unwrap_or(0);
            if offset < 0 || length < 0 {
                return Err(Error::new(
                    "INVALID_PARAMS",
                    "Offset and length cannot be negative",
                ));
            }
            let bytes = evidence_body(snapshot, side, reader)?;
            let length = if length == 0 || length > 8192 {
                8192
            } else {
                length as usize
            };
            let head = if side == "request" {
                &snapshot["req_head"]
            } else {
                &snapshot["resp_head"]
            };
            Ok(
                json!({"binding_id":binding.header.id,"head":String::from_utf8_lossy(&body_bytes(head,reader)?),"body":preview_length(&bytes,offset as usize,length)?}),
            )
        }
        "traffic_get" => {
            let id = required(p, "id")?;
            let (req, resp) = raw(s, id, reader)?;
            let stored = exchange_body(s, id)?;
            let clip = |text: &str, size: usize, body: &Value| {
                if text.len() > size {
                    let mut end = size;
                    while !text.is_char_boundary(end) {
                        end -= 1;
                    }
                    let pointer = if body["sha256"].is_string() {
                        format!(
                            "\n@blob sha256:{} (len={})",
                            body["sha256"].as_str().unwrap(),
                            body["length"].as_str().unwrap_or("0")
                        )
                    } else {
                        String::new()
                    };
                    format!(
                        "{}\n... [Truncated; total {} bytes]{pointer}",
                        &text[..end],
                        text.len()
                    )
                } else {
                    text.into()
                }
            };
            Ok(json!(format!(
                "=== REQUEST ===\n{}\n\n=== RESPONSE ===\n{}",
                clip(&req, 2500, &stored.data["req_body"]),
                clip(&resp, 4000, &stored.data["resp_body"])
            )))
        }
        "traffic_search" => {
            let raw_host = required(p, "host")?.trim();
            let parsed = if raw_host.contains("://") {
                url::Url::parse(raw_host)
            } else if matches!(
                raw_host.parse::<std::net::IpAddr>(),
                Ok(std::net::IpAddr::V6(_))
            ) {
                url::Url::parse(&format!("http://[{raw_host}]/"))
            } else {
                url::Url::parse(&format!("http://{raw_host}"))
            }
            .map_err(|_| Error::new("INVALID_PARAMS", "Invalid traffic host"))?;
            let host = parsed
                .host_str()
                .ok_or_else(|| Error::new("INVALID_PARAMS", "Traffic host is empty"))?
                .trim_matches(['[', ']'])
                .to_lowercase();
            let authority = raw_host
                .split_once("://")
                .map(|(_, a)| a)
                .unwrap_or(raw_host)
                .split(['/', '?', '#'])
                .next()
                .unwrap_or("");
            let explicit_port = authority
                .rsplit_once(':')
                .filter(|_| raw_host.parse::<std::net::IpAddr>().is_err())
                .filter(|(_, port)| {
                    !port.is_empty()
                        && port.bytes().all(|c| c.is_ascii_digit())
                        && (!authority.contains(':') || !authority.ends_with(']'))
                })
                .map(|(_, p)| p);
            let contains = text(p, "contains").to_lowercase();
            let needle = text(p, "body_contains").trim().to_lowercase();
            if !needle.is_empty() && needle.chars().count() < 3 {
                return Err(Error::new(
                    "INVALID_PARAMS",
                    "Body search needs at least three characters",
                ));
            }
            let mut rows = vec![];
            for row in s.records("exchanges") {
                let d = &row.data;
                if text(d, "host").to_lowercase() != host
                    || !["url", "url_template"]
                        .iter()
                        .any(|k| text(d, k).to_lowercase().contains(&contains))
                {
                    continue;
                }
                if let Some(port) = explicit_port {
                    let authority = text(d, "url")
                        .split_once("://")
                        .map(|(_, a)| a)
                        .unwrap_or("")
                        .split(['/', '?', '#'])
                        .next()
                        .unwrap_or("");
                    let expected = if host.contains(':') {
                        format!("[{host}]:{port}")
                    } else {
                        format!("{host}:{port}")
                    };
                    if !authority.eq_ignore_ascii_case(&expected) {
                        continue;
                    }
                }
                if !needle.is_empty() {
                    let (req, resp) = raw(s, &row.header.id, reader)?;
                    if !format!("{req}\n{resp}").to_lowercase().contains(&needle) {
                        continue;
                    }
                }
                rows.push(row);
            }
            rows.sort_by(|a, b| {
                text(&b.data, "ts")
                    .parse::<i64>()
                    .unwrap_or(0)
                    .cmp(&text(&a.data, "ts").parse().unwrap_or(0))
                    .then_with(|| b.header.id.cmp(&a.header.id))
            });
            let limit = p["limit"].as_i64().filter(|n| *n > 0).unwrap_or(3).min(10) as usize;
            let page = p["page"].as_i64().unwrap_or(0).max(0) as usize;
            let rows=rows.into_iter().skip(page.saturating_mul(limit)).take(limit).map(|r|json!({"id":r.header.id,"method":r.data["method"],"url":r.data["url"],"status":r.data["status"],"resp_len":r.data["resp_len"]})).collect::<Vec<_>>();
            if rows.is_empty() {
                Ok(json!("No matching traffic."))
            } else {
                Ok(json!(rows))
            }
        }
        _ => Err(Error::new("METHOD_NOT_FOUND", name)),
    }
}
