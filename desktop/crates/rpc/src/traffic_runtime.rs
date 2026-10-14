//! The native proxy owns capture spools; the writer acknowledges only durable captures.
use super::*;
use artex_domain::{Planner, text};
use std::{
    sync::mpsc::{self, Receiver, TryRecvError},
    time::Duration,
};
enum Reply {
    Configured { key: String, result: Result<Value> },
    Captures(Result<Value>),
}
pub(super) struct TrafficState {
    configured: String,
    pending: Option<Receiver<Reply>>,
    last_poll: Instant,
    pub(super) info: Value,
}

#[cfg(test)]
mod tests {
    use super::*;
    use artex_runtime::HostGateway;
    use std::sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    };
    struct Ack(AtomicUsize);
    impl HostGateway for Ack {
        fn request(
            &self,
            _: &str,
            _: &Context,
            operation: &str,
            _: Value,
            _: Duration,
        ) -> Result<Value> {
            assert_eq!(operation, "traffic.ack");
            self.0.fetch_add(1, Ordering::SeqCst);
            Ok(json!({"acknowledged":true}))
        }
    }
    #[test]
    fn capture_commit_verifies_spool_deduplicates_hash_and_acknowledges_after_commit() {
        let dir = tempfile::tempdir().unwrap();
        let work = dir.path().join("work");
        std::fs::create_dir(&work).unwrap();
        let path = work.join("empty.body");
        std::fs::write(&path, b"").unwrap();
        let mut router = Router::new(BTreeMap::from([("work".into(), work)]));
        router
            .set_execution_capabilities(Some("work".into()), None)
            .unwrap();
        router.store = Some(Store::create(&dir.path().join("store"), "Traffic").unwrap());
        let ack = Arc::new(Ack(AtomicUsize::new(0)));
        router.attach_host(ack.clone());
        let id = new_id("traffic");
        let digest = hex::encode(Sha256::digest(b""));
        let capture = json!({"id":id,"state":"completed","capture_error":null,"ts":"1700000000","host":"localhost","method":"GET","url":"https://localhost/x/123","status":200,"content_type":"text/plain","request_head":"GET /x/123 HTTP/1.1","response_head":"HTTP/1.1 200 OK","req_len":"0","resp_len":"0","request_file":path,"response_file":path,"request_sha256":digest,"response_sha256":digest,"request_captured_bytes":"0","response_captured_bytes":"0"});
        router
            .commit_captures(&json!({"exchanges":[capture.clone(),capture.clone()]}))
            .unwrap();
        let snapshot = router.snapshot().unwrap();
        assert_eq!(snapshot.records("exchanges").len(), 1);
        assert_eq!(snapshot.records("exchange_bodies").len(), 1);
        assert_eq!(snapshot.records("blob_refs").len(), 1);
        let bodies = snapshot.records("exchange_bodies")[0];
        assert_eq!(bodies.data["id"], id);
        assert!(artex_core::valid_id(&bodies.header.id, "trafficbody"));
        assert_eq!(
            router
                .read_blob_bytes(&bodies.data["resp_body"], 10)
                .unwrap(),
            b""
        );
        let seq = snapshot.commit_seq;
        router
            .commit_captures(&json!({"exchanges":[capture.clone()]}))
            .unwrap();
        assert_eq!(router.snapshot().unwrap().commit_seq, seq);
        let result=router.query_objects(&json!({"collection":"traffic_records","ids":[],"limit":100,"cursor":null,"at_seq":null})).unwrap();
        assert_eq!(
            result["items"][0]["header"]["collection"],
            "traffic_records"
        );
        assert_eq!(result["items"][0]["data"]["exchange"]["id"], id);
        assert_eq!(result["items"][0]["data"]["bodies"]["id"], id);
        let until = Instant::now() + Duration::from_secs(2);
        while ack.0.load(Ordering::SeqCst) < 2 {
            assert!(Instant::now() < until);
            std::thread::sleep(Duration::from_millis(5));
        }
        let mut invalid = capture;
        invalid["id"] = json!(new_id("traffic"));
        invalid["response_sha256"] = json!("a".repeat(64));
        assert_eq!(
            router
                .commit_captures(&json!({"exchanges":[invalid]}))
                .unwrap_err()
                .code,
            "CORRUPT_STORE"
        );
        assert_eq!(router.snapshot().unwrap().commit_seq, seq);
    }
}
impl Default for TrafficState {
    fn default() -> Self {
        Self {
            configured: String::new(),
            pending: None,
            last_poll: Instant::now(),
            info: json!({}),
        }
    }
}
impl Router {
    fn traffic_context(&self) -> Context {
        Context {
            actor: Actor {
                kind: "user".into(),
                agent_id: None,
                run_id: None,
                engine_epoch: self.engine_epoch.clone(),
            },
            task_id: None,
            intent_id: None,
            conversation_id: None,
        }
    }
    pub(super) fn process_traffic(&mut self) -> Result<()> {
        let Some(g) = self.gateway.clone() else {
            return Ok(());
        };
        let Some(cap) = self.execution_capability.clone() else {
            return Ok(());
        };
        if let Some(receiver) = self.traffic.pending.take() {
            match receiver.try_recv() {
                Ok(Reply::Configured { key, result }) => {
                    if let Ok(info) = result {
                        self.traffic.configured = key;
                        self.traffic.info = info;
                    } else {
                        self.traffic.last_poll = Instant::now();
                    }
                }
                Ok(Reply::Captures(result)) => {
                    if let Ok(result) = result {
                        self.commit_captures(&result)?;
                    }
                }
                Err(TryRecvError::Empty) => {
                    self.traffic.pending = Some(receiver);
                    return Ok(());
                }
                Err(TryRecvError::Disconnected) => {
                    self.traffic.configured.clear();
                }
            }
        }
        let snapshot = self.snapshot()?;
        let context = self.traffic_context();
        let workspace = snapshot.workspace_id.clone();
        let enabled = artex_domain::setting_bool(&snapshot, "traffic_capture", false);
        let proxy = artex_domain::configuration_ui::setting(&snapshot, "global_proxy");
        let config = json!({"directory_capability":cap,"enabled":enabled,"global_proxy":proxy});
        let key = hex::encode(Sha256::digest(serde_json::to_vec(&config)?));
        if self.traffic.configured != key {
            if self.traffic.last_poll.elapsed() < Duration::from_millis(250) {
                return Ok(());
            }
            let (tx, rx) = mpsc::channel();
            self.traffic.pending = Some(rx);
            self.traffic.last_poll = Instant::now();
            std::thread::spawn(move || {
                let result = g.request(
                    &workspace,
                    &context,
                    "traffic.configure",
                    config,
                    Duration::from_secs(10),
                );
                let _ = tx.send(Reply::Configured { key, result });
            });
        } else if self.traffic.last_poll.elapsed() >= Duration::from_millis(250) {
            let (tx, rx) = mpsc::channel();
            self.traffic.pending = Some(rx);
            self.traffic.last_poll = Instant::now();
            std::thread::spawn(move || {
                let result = g.request(
                    &workspace,
                    &context,
                    "traffic.poll",
                    json!({"limit":20}),
                    Duration::from_secs(5),
                );
                let _ = tx.send(Reply::Captures(result));
            });
        }
        Ok(())
    }
    fn sealed_capture_blob(&mut self, capture: &Value, side: &str) -> Result<Value> {
        let path = capture[format!("{side}_file")]
            .as_str()
            .ok_or_else(|| Error::new("INVALID_PARAMS", "Capture spool file missing"))?;
        let cap = self
            .execution_capability
            .as_ref()
            .and_then(|c| self.capabilities.get(c))
            .ok_or_else(|| Error::new("PERMISSION_DENIED", "Capture work capability missing"))?;
        let root = cap.canonicalize().map_err(Error::io)?;
        let path = PathBuf::from(path).canonicalize().map_err(Error::io)?;
        if !path.starts_with(&root) || path == root {
            return Err(Error::new(
                "PERMISSION_DENIED",
                "Capture spool escaped its workspace capability",
            ));
        }
        let media = if side == "response" {
            text(capture, "content_type")
        } else {
            "application/octet-stream"
        };
        let reference = self.store.as_mut().unwrap().blob_put_file(
            &path,
            if media.is_empty() {
                "application/octet-stream"
            } else {
                media
            },
        )?;
        let hash = &capture[format!("{side}_sha256")];
        let captured = &capture[format!("{side}_captured_bytes")];
        if hash.is_string() && *hash != reference["sha256"]
            || captured.is_string() && *captured != reference["length"]
        {
            return Err(Error::new(
                "CORRUPT_STORE",
                "Capture spool no longer matches its sealed digest and length",
            ));
        }
        Ok(reference)
    }
    fn commit_captures(&mut self, value: &Value) -> Result<()> {
        let captures = value["exchanges"]
            .as_array()
            .ok_or_else(|| Error::new("INVALID_PARAMS", "Capture list missing"))?;
        if captures.len() > 20 {
            return Err(Error::new("SIZE_LIMIT", "Capture poll exceeds queue limit"));
        }
        let s = self.snapshot()?;
        let mut p = Planner::new(&s, "traffic.capture.commit", &json!({}));
        let mut acknowledged = Vec::new();
        let mut seen = std::collections::BTreeSet::new();
        for capture in captures {
            let id = text(capture, "id");
            if !artex_core::valid_id(id, "traffic") {
                return Err(Error::new("INVALID_PARAMS", "Capture ID is not typed"));
            }
            if !seen.insert(id.to_owned()) {
                continue;
            }
            if s.get("exchanges", id).is_some() {
                acknowledged.push(id.to_owned());
                continue;
            }
            let request = self.sealed_capture_blob(capture, "request")?;
            let response = self.sealed_capture_blob(capture, "response")?;
            let mut data = artex_core::default_data("exchanges")?;
            data["id"] = json!(id);
            for key in [
                "ts",
                "host",
                "method",
                "url",
                "status",
                "content_type",
                "req_len",
                "resp_len",
            ] {
                if let Some(v) = capture.get(key) {
                    data[key] = v.clone();
                }
            }
            data["url_template"] = json!(
                url::Url::parse(text(capture, "url"))
                    .map(|u| u.path().to_owned())
                    .unwrap_or_default()
            );
            data["path"] = json!(format!("traffic/{id}"));
            p.put("exchanges", id, data);
            let mut data = artex_core::default_data("exchange_bodies")?;
            data["id"] = json!(id);
            data["req_head"] = artex_core::body(text(capture, "request_head"));
            data["resp_head"] = artex_core::body(text(capture, "response_head"));
            data["req_blob"] = request["sha256"].clone();
            data["resp_blob"] = response["sha256"].clone();
            data["req_body"] = request.clone();
            data["resp_body"] = response.clone();
            p.put(
                "exchange_bodies",
                &new_id(artex_core::collection_prefix("exchange_bodies")?),
                data,
            );
            let mut hashes = std::collections::BTreeSet::new();
            for reference in [&request, &response] {
                if !hashes.insert(reference["sha256"].as_str().unwrap().to_owned()) {
                    continue;
                }
                let mut data = artex_core::default_data("blob_refs")?;
                data["hash"] = reference["sha256"].clone();
                data["exchange_id"] = json!(id);
                p.put(
                    "blob_refs",
                    &new_id(artex_core::collection_prefix("blob_refs")?),
                    data,
                );
            }
            artex_domain::conversation::push_event(
                &mut p.batch,
                &s,
                &self.traffic_context(),
                "TrafficCaptured",
                "exchanges",
                id,
            )?;
            if let Some(event) = p.batch.events.last_mut() {
                event.data["capture"] = json!({"state":capture["state"],"error":capture["capture_error"],"request_captured_bytes":request["length"],"response_captured_bytes":response["length"]});
            }
            acknowledged.push(id.to_owned());
        }
        if !p.batch.changes.is_empty() {
            self.internal_commit(p.done(json!({"captured":acknowledged.len()})))?;
        }
        if !acknowledged.is_empty() {
            let g = self.gateway.clone().unwrap();
            let workspace = s.workspace_id.clone();
            let context = self.traffic_context();
            std::thread::spawn(move || {
                let _ = g.request(
                    &workspace,
                    &context,
                    "traffic.ack",
                    json!({"ids":acknowledged}),
                    Duration::from_secs(5),
                );
            });
        }
        Ok(())
    }
}
