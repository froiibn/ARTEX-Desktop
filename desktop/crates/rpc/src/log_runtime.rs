//! Native stderr is acknowledged only after a durable object-graph commit.
use crate::*;
use artex_core::{Change, default_data, valid_id};
use std::{sync::mpsc, time::Duration};
pub(super) struct LogState {
    pending: Option<mpsc::Receiver<(String, String, Result<Value>)>>,
    last_poll: Instant,
    ack: bool,
}
impl Default for LogState {
    fn default() -> Self {
        Self {
            pending: None,
            last_poll: Instant::now(),
            ack: false,
        }
    }
}
impl Router {
    pub(super) fn process_logs(&mut self) -> Result<()> {
        if self.read_only || self.store.is_none() {
            self.logs = Default::default();
            return Ok(());
        }
        let Some(g) = self.gateway.clone() else {
            return Ok(());
        };
        if let Some(receiver) = self.logs.pending.take() {
            match receiver.try_recv() {
                Ok((workspace, epoch, result)) => {
                    if workspace != self.snapshot()?.workspace_id || epoch != self.engine_epoch {
                        self.logs = Default::default();
                        return Ok(());
                    }
                    if self.logs.ack {
                        self.logs.ack = false;
                    } else if let Ok(value) = result {
                        // Malformed native diagnostics must not stop the agent
                        // scheduler. Leave the spool unacknowledged for inspection.
                        let Ok(ids) = self.commit_native_logs(&value) else {
                            return Ok(());
                        };
                        if !ids.is_empty() {
                            let (tx, rx) = mpsc::channel();
                            let context = Context {
                                actor: Actor {
                                    kind: "user".into(),
                                    agent_id: None,
                                    run_id: None,
                                    engine_epoch: epoch.clone(),
                                },
                                task_id: None,
                                intent_id: None,
                                conversation_id: None,
                            };
                            self.logs.pending = Some(rx);
                            self.logs.ack = true;
                            std::thread::spawn(move || {
                                let result = g.request(
                                    &workspace,
                                    &context,
                                    "logs.ack",
                                    json!({"ids":ids}),
                                    Duration::from_secs(5),
                                );
                                let _ = tx.send((workspace, epoch, result));
                            });
                            return Ok(());
                        }
                    }
                }
                Err(mpsc::TryRecvError::Empty) => {
                    self.logs.pending = Some(receiver);
                    return Ok(());
                }
                Err(_) => {
                    self.logs.ack = false;
                }
            }
        }
        if self.logs.last_poll.elapsed() < Duration::from_millis(500) {
            return Ok(());
        }
        self.logs.last_poll = Instant::now();
        let workspace = self.snapshot()?.workspace_id.clone();
        let epoch = self.engine_epoch.clone();
        let context = Context {
            actor: Actor {
                kind: "user".into(),
                agent_id: None,
                run_id: None,
                engine_epoch: epoch.clone(),
            },
            task_id: None,
            intent_id: None,
            conversation_id: None,
        };
        let (tx, rx) = mpsc::channel();
        self.logs.pending = Some(rx);
        self.logs.ack = false;
        std::thread::spawn(move || {
            let result = g.request(
                &workspace,
                &context,
                "logs.poll",
                json!({"limit":100}),
                Duration::from_secs(5),
            );
            let _ = tx.send((workspace, epoch, result));
        });
        Ok(())
    }
    fn commit_native_logs(&mut self, source: &Value) -> Result<Vec<String>> {
        let items = source["items"]
            .as_array()
            .ok_or_else(|| Error::new("INVALID_PARAMS", "Native log items must be an array"))?;
        if items.len() > 100 || serde_json::to_vec(source)?.len() > 800000 {
            return Err(Error::new(
                "SIZE_LIMIT",
                "Native log polling boundary exceeded",
            ));
        }
        let s = self.snapshot()?;
        let mut batch = artex_domain::empty_batch(
            "migration.native_logs.ingest",
            &json!({"host_epoch":self.host_epoch}),
        );
        let mut timestamps = BTreeMap::new();
        let mut ids = BTreeMap::new();
        for item in items {
            let id = item["id"]
                .as_str()
                .filter(|id| valid_id(id, "log"))
                .ok_or_else(|| Error::new("INVALID_PARAMS", "Native log identifier invalid"))?;
            let ts = item["ts"]
                .as_str()
                .ok_or_else(|| Error::new("INVALID_PARAMS", "Native log timestamp missing"))?;
            artex_core::models::Timestamp::parse(ts.to_owned())?;
            let text = item["text"]
                .as_str()
                .filter(|s| s.len() <= 16384)
                .ok_or_else(|| Error::new("SIZE_LIMIT", "Native log text exceeds 16KiB"))?;
            let level = item["level"]
                .as_str()
                .filter(|s| matches!(*s, "info" | "warn" | "error" | "debug" | "trace"))
                .ok_or_else(|| Error::new("INVALID_PARAMS", "Native log level invalid"))?;
            let tag = item["tag"]
                .as_str()
                .filter(|s| s.len() <= 256)
                .ok_or_else(|| Error::new("INVALID_PARAMS", "Native log tag invalid"))?;
            let mut data = default_data("server_logs")?;
            data["id"] = json!(id);
            data["created_at"] = json!(ts);
            data["level"] = json!(level);
            data["tag"] = json!(tag);
            data["text"] = json!(text);
            if let Some(old) = s.get("server_logs", id) {
                if old.data != data {
                    return Err(Error::new(
                        "COMMAND_ID_REUSE",
                        "Native log ID was reused with changed content",
                    ));
                }
            } else if let Some(old) = ids.get(id) {
                if old != &data {
                    return Err(Error::new(
                        "COMMAND_ID_REUSE",
                        "Native log batch repeats ID with changed content",
                    ));
                }
            } else {
                batch.changes.push(Change::Put {
                    collection: "server_logs".into(),
                    id: id.into(),
                    data: data.clone(),
                });
                timestamps.insert(
                    ("server_logs".into(), id.into()),
                    artex_store::ImportMetadata {
                        legacy_key: Value::Null,
                        created_at: ts.into(),
                        updated_at: ts.into(),
                    },
                );
            }
            ids.insert(id.to_owned(), data);
        }
        if !batch.changes.is_empty() {
            batch.command_id = new_id("command");
            self.store
                .as_mut()
                .ok_or_else(|| Error::new("WORKSPACE_NOT_OPEN", "Workspace closed"))?
                .commit_import(batch, timestamps)?;
        }
        Ok(ids.into_keys().collect())
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    use artex_runtime::HostGateway;
    use std::sync::{
        Arc, Mutex,
        atomic::{AtomicUsize, Ordering},
    };
    #[test]
    fn ingestion_is_durable_duplicate_safe_and_never_acknowledges_invalid_entries() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("w");
        let mut router = Router::new(BTreeMap::new());
        router.store = Some(Store::create(&path, "logs").unwrap());
        let id = new_id("log");
        let entry = json!({"id":id,"ts":artex_core::now(),"level":"warn","tag":"engine","text":"bounded redacted entry"});
        assert_eq!(
            router
                .commit_native_logs(&json!({"items":[entry.clone(),entry.clone()]}))
                .unwrap(),
            vec![id.clone()]
        );
        assert_eq!(router.snapshot().unwrap().records("server_logs").len(), 1);
        let seq = router.snapshot().unwrap().commit_seq;
        router
            .commit_native_logs(&json!({"items":[entry.clone()]}))
            .unwrap();
        assert_eq!(router.snapshot().unwrap().commit_seq, seq);
        let mut changed = entry;
        changed["text"] = json!("changed");
        assert_eq!(
            router
                .commit_native_logs(&json!({"items":[changed]}))
                .unwrap_err()
                .code,
            "COMMAND_ID_REUSE"
        );
        drop(router);
        let reopened = Store::open(&path).unwrap();
        assert_eq!(reopened.snapshot().records("server_logs")[0].data["id"], id);
    }
    struct LogFixture {
        entry: Value,
        acknowledged: Mutex<Vec<String>>,
        attempts: AtomicUsize,
    }
    impl HostGateway for LogFixture {
        fn request(
            &self,
            _: &str,
            _: &Context,
            operation: &str,
            params: Value,
            _: Duration,
        ) -> Result<Value> {
            match operation {
                "logs.poll" => {
                    Ok(json!({"items":[self.entry.clone()],"dropped":"0","capture_error":null}))
                }
                "logs.ack" => {
                    let ids = params["ids"].as_array().unwrap();
                    assert_eq!(ids, &[self.entry["id"].clone()]);
                    if self.attempts.fetch_add(1, Ordering::SeqCst) == 0 {
                        return Err(Error::new(
                            "IO_FAILURE",
                            "Fixture acknowledgement interrupted",
                        ));
                    }
                    self.acknowledged
                        .lock()
                        .unwrap()
                        .push(ids[0].as_str().unwrap().into());
                    Ok(json!({"acknowledged":1}))
                }
                _ => Err(Error::new(
                    "METHOD_NOT_FOUND",
                    "Fixture unsupported operation",
                )),
            }
        }
    }
    #[test]
    fn lost_native_ack_repolls_without_duplicate_journal_entry_and_persists_after_restart() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("w");
        let mut router = Router::new(BTreeMap::new());
        router.store = Some(Store::create(&path, "Native logs").unwrap());
        let id = new_id("log");
        let host = Arc::new(LogFixture {
            entry: json!({"id":id,"ts":artex_core::now(),"level":"warn","tag":"engine","text":"safe stderr"}),
            acknowledged: Mutex::new(vec![]),
            attempts: AtomicUsize::new(0),
        });
        router.gateway = Some(host.clone());
        let end = Instant::now() + Duration::from_secs(5);
        while host.acknowledged.lock().unwrap().is_empty() {
            router.logs.last_poll = Instant::now() - Duration::from_secs(1);
            router.process_logs().unwrap();
            assert!(Instant::now() < end);
            std::thread::sleep(Duration::from_millis(2));
        }
        assert_eq!(host.attempts.load(Ordering::SeqCst), 2);
        assert_eq!(router.snapshot().unwrap().records("server_logs").len(), 1);
        assert_eq!(router.snapshot().unwrap().commit_seq, 1);
        drop(router);
        let reopened = Store::open(&path).unwrap();
        assert_eq!(reopened.snapshot().records("server_logs")[0].header.id, id);
        assert_eq!(reopened.snapshot().commit_seq, 1);
    }
}
