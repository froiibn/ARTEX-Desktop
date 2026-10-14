//! Evidence exports read immutable content through the verified store boundary.
use crate::traffic_ui::{BlobReader, body_bytes};
use crate::*;
use base64::{Engine, engine::general_purpose::STANDARD};
use std::{
    collections::BTreeMap,
    io::{Cursor, Write},
};
fn readable(v: &Value, key: &str, fallback: &str) -> String {
    let s = text(v, key).trim();
    if s.is_empty() {
        fallback.into()
    } else {
        s.into()
    }
}
fn title(f: &Record) -> String {
    readable(
        &f.data,
        "name",
        &readable(&f.data, "vulnclass", "Uncategorized"),
    )
}
fn timestamp(v: &str) -> String {
    chrono::DateTime::parse_from_rfc3339(v)
        .map(|d| d.format("%Y-%m-%d %H:%M:%S").to_string())
        .unwrap_or_else(|_| v.into())
}
fn bindings<'a>(s: &'a Snapshot, f: &Record) -> Vec<&'a Record> {
    let mut rows = s
        .records("finding_traffic_bindings")
        .into_iter()
        .filter(|r| r.data["finding_id"] == f.header.id)
        .collect::<Vec<_>>();
    rows.sort_by_key(|r| {
        (
            r.data["position"].as_i64().unwrap_or(0),
            r.header.id.clone(),
        )
    });
    rows
}
fn traffic(f: &Record, s: &Snapshot, attachments: bool) -> Result<String> {
    let rows = bindings(s, f);
    let stale = f.data["report"] != artex_core::body("")
        && f.data["evidence_version"] != f.data["report_evidence_version"];
    if rows.is_empty() && !stale {
        return Ok(String::new());
    }
    let mut out = format!(
        "\n## Correlated traffic evidence\n\nEvidence version: {}; bindings: {}.\n\n",
        text(&f.data, "evidence_version"),
        rows.len()
    );
    if stale {
        out.push_str("Evidence has changed; the detailed report needs updating.\n\n");
    }
    for (i, b) in rows.iter().enumerate() {
        let snap = record(
            s,
            "traffic_evidence_snapshots",
            required(&b.data, "snapshot_id")?,
        )?;
        out.push_str(&format!(
            "{}. **Evidence {} · {}** — `{} {}`, status {}\n",
            i + 1,
            b.header.id,
            text(&b.data, "role"),
            text(&snap.data, "method"),
            text(&snap.data, "url").replace('`', "%60"),
            snap.data["status"]
        ));
        if !text(&b.data, "note").is_empty() {
            out.push_str(&format!(
                "   {}\n",
                text(&b.data, "note").replace('\n', "\n   ")
            ));
        }
        if attachments {
            out.push_str(&format!("   [Request](evidence/{}/{}/request.http) · [Response](evidence/{}/{}/response.http)\n",f.header.id,b.header.id,f.header.id,b.header.id));
        }
    }
    Ok(out)
}
pub fn single(f: &Record, s: &Snapshot, reader: &BlobReader) -> Result<String> {
    single_with_attachments(f, s, reader, true)
}
fn single_with_attachments(
    f: &Record,
    s: &Snapshot,
    reader: &BlobReader,
    attachments: bool,
) -> Result<String> {
    let d = &f.data;
    let mut out = format!(
        "# [{}] {}\n\n- **Category**: {}\n- **Severity**: {}\n- **Status**: {}\n",
        readable(d, "severity", "info").to_uppercase(),
        title(f),
        text(d, "vulnclass"),
        readable(d, "severity", "info"),
        readable(d, "status", "pending")
    );
    if let Some(t) = d["task_id"].as_str().and_then(|id| s.get("tasks", id)) {
        out.push_str(&format!("- **Task**: {}\n", text(&t.data, "description")));
    }
    out.push_str(&format!(
        "- **Discovery time**: {}\n- **Generation time**: {}\n\n",
        timestamp(text(d, "created_at")),
        timestamp(&artex_core::now())
    ));
    if !text(d, "summary").trim().is_empty() {
        out.push_str(&format!("## Overview\n\n{}\n\n", text(d, "summary").trim()));
    }
    let evidence = body_bytes(&d["evidence"], reader)?;
    if !evidence.is_empty() {
        out.push_str(&format!(
            "## Evidence\n\n```\n{}\n```\n\n",
            String::from_utf8_lossy(&evidence)
        ));
    }
    let report = body_bytes(&d["report"], reader)?;
    if !report.is_empty() {
        out.push_str(&format!(
            "## Detailed report\n\n{}\n",
            String::from_utf8_lossy(&report)
        ));
    }
    out.push_str(&traffic(f, s, attachments)?);
    Ok(out)
}
fn summary(rows: &[&Record], s: &Snapshot, reader: &BlobReader) -> Result<String> {
    let mut out = format!(
        "# Vulnerability discovery summary report\n\n- **Generation time**: {}\n- **Total findings**: {}\n\n## Summary\n\n| Severity | Count |\n| --- | --- |\n",
        timestamp(&artex_core::now()),
        rows.len()
    );
    for level in ["critical", "high", "medium", "low"] {
        out.push_str(&format!(
            "| {} | {} |\n",
            level,
            rows.iter().filter(|r| r.data["severity"] == level).count()
        ));
    }
    if rows.is_empty() {
        out.push_str("\n_No matching vulnerabilities._\n");
    } else {
        out.push_str("\n## Vulnerability details\n\n");
        for (i, f) in rows.iter().enumerate() {
            out.push_str(&format!(
                "### {}. [{}] {}\n\n",
                i + 1,
                readable(&f.data, "severity", "info").to_uppercase(),
                title(f)
            ));
            let single = single_with_attachments(f, s, reader, false)?;
            out.push_str(single.split_once("\n\n").map(|(_, s)| s).unwrap_or(&single));
            out.push_str("\n\n---\n\n");
        }
    }
    Ok(out)
}
fn plain(v: &Value, reader: &BlobReader) -> Result<Value> {
    if v.is_object() && v.get("text").is_some() && v.get("blob").is_some() {
        return Ok(json!(String::from_utf8_lossy(&body_bytes(v, reader)?)));
    }
    match v {
        Value::Array(a) => Ok(Value::Array(
            a.iter()
                .map(|v| plain(v, reader))
                .collect::<Result<Vec<_>>>()?,
        )),
        Value::Object(o) => Ok(Value::Object(
            o.iter()
                .map(|(k, v)| Ok((k.clone(), plain(v, reader)?)))
                .collect::<Result<_>>()?,
        )),
        v => Ok(v.clone()),
    }
}
fn download(bytes: &[u8], mime: &str, ext: &str) -> Value {
    json!({"content_base64":artex_core::body(&STANDARD.encode(bytes)),"mime":mime,"filename":format!("findings-{}.{}",chrono::Utc::now().format("%Y%m%d-%H%M%S"),ext)})
}
pub fn export(p: &Value, s: &Snapshot, reader: &BlobReader) -> Result<Value> {
    let opts = &p["opts"];
    let mut rows = match required(opts, "scope")? {
        "all" => findings_ui::filtered(s, &json!({}))?,
        "filtered" => findings_ui::filtered(s, &opts["filters"])?,
        "selected" => {
            let selected = ids(opts, "ids").into_iter().collect::<BTreeSet<_>>();
            if selected.is_empty() {
                return Err(Error::new("INVALID_PARAMS", "Select at least one finding"));
            }
            s.records("findings")
                .into_iter()
                .filter(|r| selected.contains(&r.header.id))
                .collect()
        }
        _ => return Err(Error::new("INVALID_PARAMS", "Invalid export scope")),
    };
    let format = required(opts, "format")?;
    if format != "json" {
        rows.sort_by(|a, b| {
            findings_ui::severity(text(&b.data, "severity"))
                .cmp(&findings_ui::severity(text(&a.data, "severity")))
                .then_with(|| b.header.created_at.cmp(&a.header.created_at))
        });
    }
    match format {
        "json" => {
            let dto = rows
                .iter()
                .map(|r| plain(&findings_ui::finding(&r.data, s, true, None), reader))
                .collect::<Result<Vec<_>>>()?;
            Ok(download(
                &serde_json::to_vec_pretty(&dto)?,
                "application/json",
                "json",
            ))
        }
        "csv" => {
            let mut csv="\u{feff}ID,Name,Category,severity level,Status,Task,discovery time,Overview,Number of traffic evidence,traffic evidence ID\n".to_owned();
            let quote = |s: String| {
                if s.contains([',', '"', '\n', '\r']) {
                    format!("\"{}\"", s.replace('"', "\"\""))
                } else {
                    s
                }
            };
            for f in rows {
                let d = &f.data;
                let bs = bindings(s, f);
                let task = d["task_id"]
                    .as_str()
                    .and_then(|id| s.get("tasks", id))
                    .map(|r| text(&r.data, "description"))
                    .unwrap_or("");
                let fields = vec![
                    f.header.id.clone(),
                    title(f),
                    text(d, "vulnclass").into(),
                    readable(d, "severity", "info"),
                    readable(d, "status", "pending"),
                    task.into(),
                    timestamp(text(d, "created_at")),
                    text(d, "summary").trim().into(),
                    bs.len().to_string(),
                    bs.iter()
                        .map(|b| b.header.id.as_str())
                        .collect::<Vec<_>>()
                        .join(","),
                ];
                csv.push_str(&fields.into_iter().map(quote).collect::<Vec<_>>().join(","));
                csv.push('\n');
            }
            Ok(download(csv.as_bytes(), "text/csv; charset=utf-8", "csv"))
        }
        "md-single" => Ok(download(
            summary(&rows, s, reader)?.as_bytes(),
            "text/markdown; charset=utf-8",
            "md",
        )),
        "md-zip" => {
            let mut archive = zip::ZipWriter::new(Cursor::new(Vec::new()));
            for f in rows {
                let title = title(f);
                let safe = title
                    .chars()
                    .map(|c| {
                        if c.is_alphanumeric() || "._-".contains(c) {
                            c
                        } else {
                            '_'
                        }
                    })
                    .take(70)
                    .collect::<String>();
                let filename = format!(
                    "{}_{}_{}.md",
                    readable(&f.data, "severity", "info"),
                    safe.trim_matches(['.', '_']),
                    f.header.id
                );
                let mut entry = |name: String, bytes: &[u8]| -> Result<()> {
                    archive
                        .start_file(name, zip::write::SimpleFileOptions::default())
                        .map_err(|e| Error::new("IO_ERROR", &e.to_string()))?;
                    archive.write_all(bytes)?;
                    Ok(())
                };
                entry(filename, single(f, s, reader)?.as_bytes())?;
                for b in bindings(s, f) {
                    let snap = record(
                        s,
                        "traffic_evidence_snapshots",
                        required(&b.data, "snapshot_id")?,
                    )?;
                    let prefix = format!("evidence/{}/{}/", f.header.id, b.header.id);
                    entry(
                        format!("{prefix}manifest.json"),
                        &serde_json::to_vec_pretty(&plain(
                            &findings_ui::binding(&b.data, s)?,
                            reader,
                        )?)?,
                    )?;
                    for (side, k) in [("request", "req"), ("response", "resp")] {
                        let bytes = traffic_ui::evidence_body(&snap.data, side, reader)?;
                        let head = body_bytes(&snap.data[format!("{k}_head")], reader)?;
                        let mut http = String::from_utf8_lossy(&head)
                            .trim_end_matches(['\r', '\n'])
                            .as_bytes()
                            .to_vec();
                        http.extend_from_slice(b"\n\n");
                        http.extend_from_slice(&bytes);
                        entry(format!("{prefix}{side}.http"), &http)?;
                        entry(format!("{prefix}{side}.bin"), &bytes)?;
                    }
                }
            }
            let bytes = archive
                .finish()
                .map_err(|e| Error::new("IO_ERROR", &e.to_string()))?
                .into_inner();
            Ok(download(&bytes, "application/zip", "zip"))
        }
        _ => Err(Error::new("INVALID_PARAMS", "Unsupported export format")),
    }
}
pub fn task_report(p: &Value, s: &Snapshot, reader: &BlobReader) -> Result<Value> {
    let task = exploration_ui::resolve_task(s, p)?
        .ok_or_else(|| Error::new("NOT_FOUND", "No active task"))?;
    let exp = text(&task.data, "exploration_id");
    let mut findings = s
        .records("exploration_nodes")
        .into_iter()
        .filter(|r| r.data["exploration_id"] == exp && r.data["kind"] == "finding")
        .take(1000)
        .collect::<Vec<_>>();
    findings.sort_by_key(|r| {
        std::cmp::Reverse(findings_ui::severity(text(&r.data["payload"], "severity")))
    });
    let mut counts = BTreeMap::<String, usize>::new();
    for a in s.records("assets") {
        *counts.entry(text(&a.data, "type").into()).or_default() += 1;
    }
    let mut report = format!(
        "# Penetration Test Report — {}\n\n- **Mission objectives**: {}\n- **Generation time**: {}\n\n## Summary\n\n- Confirmed findings: **{}**\n- Assets: {}\n\n## Findings\n\n",
        readable(&task.data, "description", "Unnamed task"),
        readable(&task.data, "goal", "Unspecified"),
        timestamp(&artex_core::now()),
        findings.len(),
        counts
            .into_iter()
            .map(|(k, n)| format!("{k} {n}"))
            .collect::<Vec<_>>()
            .join(", ")
    );
    if findings.is_empty() {
        report.push_str("_No vulnerabilities confirmed._\n");
    }
    for (i, f) in findings.iter().enumerate() {
        let d = &f.data["payload"];
        report.push_str(&format!(
            "### {}. [{}] {}\n\n{}\n\n",
            i + 1,
            readable(d, "severity", "info").to_uppercase(),
            readable(d, "name", &readable(d, "vulnclass", "Uncategorized")),
            text(d, "summary")
        ));
        let evidence = &d["evidence"];
        let poc = if evidence.is_object() && evidence.get("poc").is_some() {
            body_bytes(&evidence["poc"], reader)?
        } else {
            body_bytes(evidence, reader)?
        };
        if !poc.is_empty() {
            report.push_str(&format!(
                "**PoC / evidence:**\n\n```\n{}\n```\n\n",
                String::from_utf8_lossy(&poc)
            ));
        }
    }
    Ok(artex_core::body(&report))
}
