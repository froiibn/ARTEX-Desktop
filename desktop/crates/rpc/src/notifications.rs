//! Durable notification outbox with bounded asynchronous host delivery.
use crate::*;
use artex_domain::{Planner, record, text};
use chrono::{DateTime, Utc};
use std::{
    sync::mpsc::{self, Receiver, TryRecvError},
    time::Duration,
};

pub(super) struct PendingSend {
    pub(super) ids: Vec<String>,
    workspace: String,
    epoch: String,
    receiver: Receiver<Result<Value>>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, Mutex};
    struct Gateway {
        calls: Mutex<Vec<Value>>,
    }
    impl artex_runtime::HostGateway for Gateway {
        fn request(
            &self,
            _: &str,
            c: &Context,
            operation: &str,
            p: Value,
            _: Duration,
        ) -> Result<Value> {
            assert_eq!(c.actor.kind, "user");
            assert_eq!(operation, "notification.send");
            self.calls.lock().unwrap().push(p);
            Ok(json!({"ok":true,"latency_ms":1,"kept":1}))
        }
    }
    fn seeded() -> (tempfile::TempDir, Router, String) {
        let dir = tempfile::tempdir().unwrap();
        let mut router = Router::new(BTreeMap::new());
        router.store = Some(Store::create(&dir.path().join("w"), "notify").unwrap());
        let s = router.snapshot().unwrap();
        let mut p = Planner::new(&s, "fixture.notify", &json!({}));
        let exp = p.insert("explorations", "exploration", json!({})).unwrap();
        let task = p
            .insert("tasks", "task", json!({"exploration_id":exp}))
            .unwrap();
        let finding=p.insert("findings","finding",json!({"task_id":task,"name":"proof","summary":"evidence","severity":"high","status":"pending"})).unwrap();
        p.insert("notification_channels",artex_core::collection_prefix("notification_channels").unwrap(),json!({"kind":"webhook","enabled":true,"mode":"realtime","rate_per_min":0,"config":{},"filter":{"min_severity":"high"}})).unwrap();
        router.internal_commit(p.done(json!({}))).unwrap();
        (dir, router, finding)
    }
    #[test]
    fn durable_finding_event_fans_out_and_async_send_records_once() {
        let (_dir, mut router, finding) = seeded();
        let gateway = Arc::new(Gateway {
            calls: Mutex::new(Vec::new()),
        });
        router.gateway = Some(gateway.clone());
        assert_eq!(
            router
                .snapshot()
                .unwrap()
                .records("notification_events")
                .len(),
            1
        );
        router.process_notification_jobs().unwrap();
        for _ in 0..100 {
            std::thread::sleep(Duration::from_millis(2));
            router.process_notification_jobs().unwrap();
            if router.notification_work.is_none() {
                break;
            }
        }
        let s = router.snapshot().unwrap();
        let rows = s.records("notification_deliveries");
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].data["state"], "sent");
        assert_eq!(gateway.calls.lock().unwrap().len(), 1);
        let mut p = Planner::new(&s, "fixture.status", &json!({}));
        let mut d = s.get("findings", &finding).unwrap().data.clone();
        d["status"] = json!("confirmed");
        p.put("findings", &finding, d);
        router.internal_commit(p.done(json!({}))).unwrap();
        router.process_notification_jobs().unwrap();
        assert_eq!(
            router
                .snapshot()
                .unwrap()
                .records("notification_events")
                .len(),
            2
        );
        assert_eq!(
            router
                .snapshot()
                .unwrap()
                .records("notification_deliveries")
                .len(),
            1
        );
    }
    #[test]
    fn unconfirmed_restart_is_failed_without_automatic_external_repeat() {
        let (_dir, mut router, _finding) = seeded();
        router.process_notification_jobs().unwrap();
        let s = router.snapshot().unwrap();
        let row = s.records("notification_deliveries")[0];
        let mut p = Planner::new(&s, "fixture.unconfirmed", &json!({}));
        let mut d = row.data.clone();
        d["state"] = json!("sending");
        p.put("notification_deliveries", &row.header.id, d);
        router.internal_commit(p.done(json!({}))).unwrap();
        router.process_notification_jobs().unwrap();
        let s = router.snapshot().unwrap();
        assert_eq!(
            s.records("notification_deliveries")[0].data["state"],
            "failed"
        );
        assert!(
            text(&s.records("notification_deliveries")[0].data, "last_error")
                .contains("unconfirmed")
        );
    }
    #[test]
    fn source_filter_exclusion_status_and_scope() {
        let event = json!({"severity":"high","task_id":"task-a","asset_ids":["asset-a"],"vulnclass":"SQL injection"});
        assert!(matches_filter(
            &json!({"min_severity":"medium","vulnclass_include":["SQL"]}),
            &event,
            "finding_created"
        ));
        assert!(!matches_filter(
            &json!({"vulnclass_include":["sql"],"vulnclass_exclude":["inject"]}),
            &event,
            "finding_created"
        ));
        assert!(!matches_filter(
            &json!({}),
            &event,
            "finding_status_changed"
        ));
        assert!(!matches_filter(
            &json!({"task_ids":["task-other"]}),
            &event,
            "finding_created"
        ));
    }
}
fn setting<'a>(s: &'a Snapshot, key: &str, default: &'a str) -> &'a str {
    s.records("settings")
        .into_iter()
        .find(|r| r.data["key"] == key)
        .and_then(|r| r.data["value"].as_str())
        .unwrap_or(default)
}
fn matches_filter(filter: &Value, snapshot: &Value, kind: &str) -> bool {
    if kind == "finding_status_changed" && filter["on_status_change"] != true {
        return false;
    }
    fn rank(s: &str) -> u8 {
        match s {
            "critical" => 4,
            "high" => 3,
            "medium" => 2,
            "low" => 1,
            _ => 0,
        }
    }
    if rank(text(snapshot, "severity")) < rank(text(filter, "min_severity")) {
        return false;
    }
    if filter["task_ids"]
        .as_array()
        .is_some_and(|a| !a.is_empty() && !a.contains(&snapshot["task_id"]))
    {
        return false;
    }
    if filter["asset_ids"].as_array().is_some_and(|a| {
        !a.is_empty()
            && !a.iter().any(|id| {
                snapshot["asset_ids"]
                    .as_array()
                    .is_some_and(|b| b.contains(id))
            })
    }) {
        return false;
    }
    let class = text(snapshot, "vulnclass").to_lowercase();
    let contains = |key: &str| {
        filter[key].as_array().is_some_and(|a| {
            a.iter()
                .filter_map(Value::as_str)
                .map(str::trim)
                .filter(|w| !w.is_empty())
                .any(|w| class.contains(&w.to_lowercase()))
        })
    };
    if contains("vulnclass_exclude") {
        return false;
    }
    !filter["vulnclass_include"]
        .as_array()
        .is_some_and(|a| !a.is_empty())
        || contains("vulnclass_include")
}
fn date(v: &Value, key: &str) -> Option<DateTime<Utc>> {
    DateTime::parse_from_rfc3339(text(v, key))
        .ok()
        .map(|d| d.with_timezone(&Utc))
}
impl Router {
    pub(super) fn process_notification_jobs(&mut self) -> Result<()> {
        if self.read_only || self.store.is_none() {
            return Ok(());
        }
        if let Some(work) = self.notification_work.take() {
            if work.workspace != self.snapshot()?.workspace_id || work.epoch != self.engine_epoch {
                return Ok(());
            }
            let result = match work.receiver.try_recv() {
                Ok(r) => Some(r),
                Err(TryRecvError::Empty) => None,
                Err(TryRecvError::Disconnected) => Some(Err(Error::new(
                    "EXTERNAL_FAILURE",
                    "Notification outcome is unconfirmed",
                ))),
            };
            if let Some(result) = result {
                self.finish_notification(&work.ids, result)?;
            } else {
                self.notification_work = Some(work);
                return Ok(());
            }
        }
        let s = self.snapshot()?;
        // A process restart never retries an external effect whose outcome was not confirmed.
        let stale = s
            .records("notification_deliveries")
            .into_iter()
            .filter(|r| r.data["state"] == "sending")
            .collect::<Vec<_>>();
        if !stale.is_empty() {
            let mut p = Planner::new(&s, "notification.recover", &json!({}));
            for r in stale {
                let mut d = r.data.clone();
                d["state"] = json!("failed");
                d["last_error"] = json!("Delivery outcome is unconfirmed; review before retrying");
                p.put("notification_deliveries", &r.header.id, d);
            }
            self.internal_commit(p.done(json!({"recovered":true})))?;
        }
        let s = self.snapshot()?;
        if setting(&s, "notify_enabled", "true") != "true" {
            return Ok(());
        }
        let mut planner = Planner::new(&s, "notification.fanout", &json!({}));
        for event in s
            .records("notification_events")
            .into_iter()
            .filter(|r| r.data["fanned_out"] != true)
            .take(200)
        {
            for channel in s.records("notification_channels").into_iter().filter(|r| {
                r.data["enabled"] == true
                    && matches_filter(
                        &r.data["filter"],
                        &event.data["snapshot"],
                        text(&event.data, "kind"),
                    )
            }) {
                let mut next = Utc::now();
                if channel.data["mode"] == "digest" {
                    let minutes = setting(&s, "notify_digest_interval_min", "30")
                        .parse::<i64>()
                        .unwrap_or(30)
                        .clamp(1, 10080);
                    next = date(&event.data, "created_at").unwrap_or(next)
                        + chrono::Duration::minutes(minutes);
                }
                planner.insert("notification_deliveries",artex_core::collection_prefix("notification_deliveries")?,json!({"event_id":event.header.id,"channel_id":channel.header.id,"state":"pending","attempts":0,"next_attempt_at":next.to_rfc3339_opts(chrono::SecondsFormat::Millis,true)}))?;
            }
            let mut d = event.data.clone();
            d["fanned_out"] = json!(true);
            planner.put("notification_events", &event.header.id, d);
        }
        let batch = planner.done(json!({"fanned_out":true}));
        if !batch.changes.is_empty() {
            self.internal_commit(batch)?;
        }
        let Some(gateway) = self.gateway.clone() else {
            return Ok(());
        };
        let s = self.snapshot()?;
        let now = Utc::now();
        let candidate = s.records("notification_deliveries").into_iter().find(|r| {
            r.data["state"] == "pending"
                && date(&r.data, "next_attempt_at").is_some_and(|d| d <= now)
                && s.get("notification_channels", text(&r.data, "channel_id"))
                    .is_some_and(|c| c.data["enabled"] == true)
        });
        let Some(candidate) = candidate else {
            return Ok(());
        };
        let channel = record(
            &s,
            "notification_channels",
            text(&candidate.data, "channel_id"),
        )?;
        let rate = channel.data["rate_per_min"].as_u64().unwrap_or(0);
        if rate > 0 {
            // Spacing avoids burst doubling at minute boundaries and survives restart.
            let last = s
                .records("notification_deliveries")
                .into_iter()
                .filter(|r| r.data["channel_id"] == channel.header.id)
                .filter_map(|r| date(&r.data, "sent_at"))
                .max();
            if last.is_some_and(|t| {
                now.signed_duration_since(t).num_milliseconds() < 60000u64.div_ceil(rate) as i64
            }) {
                return Ok(());
            }
        }
        let digest = channel.data["mode"] == "digest";
        let rows = s
            .records("notification_deliveries")
            .into_iter()
            .filter(|r| {
                r.data["state"] == "pending"
                    && r.data["channel_id"] == channel.header.id
                    && date(&r.data, "next_attempt_at").is_some_and(|t| t <= now)
            })
            .take(if digest { 50 } else { 1 })
            .collect::<Vec<_>>();
        let ids = rows.iter().map(|r| r.header.id.clone()).collect::<Vec<_>>();
        let mut items = Vec::new();
        for row in &rows {
            let event = record(&s, "notification_events", text(&row.data, "event_id"))?;
            let mut item = event.data["snapshot"].clone();
            let finding = text(&item, "finding_id");
            let base = setting(&s, "notify_public_base_url", "").trim_end_matches('/');
            item["detail_url"] = json!(if base.is_empty() {
                String::new()
            } else {
                format!("{base}/findings/{finding}")
            });
            if item["summary"].is_object() {
                item["summary"] = json!(text(&item["summary"], "text"));
            }
            items.push(item);
        }
        let title = if digest {
            format!("ARTEX: {} findings", items.len())
        } else {
            format!("ARTEX: {}", text(&items[0], "name"))
        };
        let body = items
            .iter()
            .map(|i| {
                format!(
                    "{} [{}]\n{}\n{}",
                    text(i, "name"),
                    text(i, "severity"),
                    text(i, "summary"),
                    text(i, "detail_url")
                )
            })
            .collect::<Vec<_>>()
            .join("\n\n");
        let parameters = json!({"channel_id":channel.header.id,"kind":channel.data["kind"],"config":channel.data["config"],"message":{"title":title,"text":body,"markdown":body,"items":items,"batch":digest,"home_url":setting(&s,"notify_public_base_url","")},"test":false});
        let mut planner = Planner::new(&s, "notification.claim", &json!({"delivery_ids":ids}));
        for row in rows {
            let mut d = row.data.clone();
            d["state"] = json!("sending");
            d["attempts"] = json!(d["attempts"].as_u64().unwrap_or(0) + 1);
            planner.put("notification_deliveries", &row.header.id, d);
        }
        self.internal_commit(planner.done(json!({"claimed":true})))?;
        let workspace = s.workspace_id.clone();
        let context = Context {
            actor: Actor {
                kind: "user".into(),
                agent_id: None,
                run_id: None,
                engine_epoch: self.engine_epoch.clone(),
            },
            task_id: None,
            intent_id: None,
            conversation_id: None,
        };
        let (sender, receiver) = mpsc::sync_channel(1);
        std::thread::spawn(move || {
            let result = gateway.request(
                &workspace,
                &context,
                "notification.send",
                parameters,
                Duration::from_secs(60),
            );
            let _ = sender.send(result);
        });
        self.notification_work = Some(PendingSend {
            ids,
            receiver,
            workspace: s.workspace_id.clone(),
            epoch: self.engine_epoch.clone(),
        });
        Ok(())
    }
    fn finish_notification(&mut self, ids: &[String], result: Result<Value>) -> Result<()> {
        let s = self.snapshot()?;
        let mut planner = Planner::new(&s, "notification.complete", &json!({"delivery_ids":ids}));
        for (index, id) in ids.iter().enumerate() {
            let Some(row) = s.get("notification_deliveries", id) else {
                continue;
            };
            if row.data["state"] != "sending" {
                continue;
            }
            let mut d = row.data.clone();
            match &result {
                Ok(v) if v["ok"] == true => {
                    let kept = v["kept"].as_u64().unwrap_or(ids.len() as u64) as usize;
                    if kept == 0 || kept > ids.len() {
                        d["state"] = json!("failed");
                        d["last_error"] =
                            json!("Invalid notification packing result; review before retrying");
                    } else if index < kept {
                        d["state"] = json!("sent");
                        d["sent_at"] = json!(artex_core::now());
                        d["last_error"] = json!("");
                    } else {
                        d["state"] = json!("pending");
                        d["attempts"] =
                            json!(d["attempts"].as_u64().unwrap_or(1).saturating_sub(1));
                    }
                }
                Err(e) if e.retryable && d["attempts"].as_u64().unwrap_or(3) < 3 => {
                    let attempts = d["attempts"].as_u64().unwrap_or(3);
                    d["state"] = json!("pending");
                    d["next_attempt_at"] = json!(
                        (Utc::now()
                            + chrono::Duration::seconds(if attempts == 1 { 5 } else { 30 }))
                        .to_rfc3339_opts(chrono::SecondsFormat::Millis, true)
                    );
                    d["last_error"] = json!(format!("Notification delivery failed ({})", e.code));
                }
                _ => {
                    d["state"] = json!("failed");
                    d["last_error"] = json!("Notification delivery failed or is unconfirmed");
                }
            }
            planner.put("notification_deliveries", id, d);
        }
        self.internal_commit(planner.done(json!({"recorded":true})))?;
        Ok(())
    }
    pub(super) fn handle_notification_test(
        &mut self,
        r: &DispatchRequest,
        c: &Context,
    ) -> Result<Value> {
        if c.actor.kind != "user" {
            return Err(Error::new(
                "PERMISSION_DENIED",
                "Notification test requires the native user",
            ));
        }
        self.ensure_writable()?;
        self.require_command(r)?;
        if let Some(result) = self.repeated(r)? {
            return Ok(result);
        }
        let s = self.snapshot()?;
        let channel = record(
            &s,
            "notification_channels",
            artex_domain::required(&r.params, "id")?,
        )?;
        let Some(gateway) = self.gateway.clone() else {
            return Err(Error::new(
                "FEATURE_DISABLED",
                "Native notification gateway unavailable",
            ));
        };
        let parameters = json!({"channel_id":channel.header.id,"kind":channel.data["kind"],"config":channel.data["config"],"message":{"title":"ARTEX notification test","text":"ARTEX notification channel test.","markdown":"ARTEX notification channel test.","items":[],"batch":false},"test":true});
        drop(gateway);
        self.defer_ui(r, c, "notification.send", parameters)
    }
}
