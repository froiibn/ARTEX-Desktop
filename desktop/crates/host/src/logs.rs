//! Bounded native stderr spool, acknowledged only after engine persistence.
use artex_core::{Error, Result, new_id, valid_id};
use serde_json::{Value, json};
use std::{
    collections::BTreeMap,
    fs,
    path::PathBuf,
    sync::{
        Mutex,
        atomic::{AtomicU64, Ordering},
    },
};
#[derive(Default)]
pub struct NativeLogs {
    state: Mutex<State>,
    dropped: AtomicU64,
}
#[derive(Default)]
struct State {
    root: Option<PathBuf>,
    directory: Option<PathBuf>,
    items: BTreeMap<String, Value>,
    failure: Option<String>,
}
impl NativeLogs {
    pub(crate) fn capture_failure(&self) {
        self.state.lock().unwrap().failure = Some("Native log spool activation failed".into());
    }
    pub(crate) fn set_directory(&self, root: PathBuf) {
        self.state.lock().unwrap().root = Some(root);
    }
    pub(crate) fn activate(&self) -> Result<()> {
        let mut s = self.state.lock().unwrap();
        let Some(root) = s.root.clone() else {
            return Ok(());
        };
        let root = root.canonicalize().map_err(Error::io)?;
        let directory = root.join(".artex-host/logs");
        for path in [root.join(".artex-host"), directory.clone()] {
            if path.exists() && !path.canonicalize().map_err(Error::io)?.starts_with(&root) {
                return Err(Error::new(
                    "PERMISSION_DENIED",
                    "Log spool escaped execution root",
                ));
            }
            fs::create_dir_all(path).map_err(Error::io)?;
        }
        for entry in fs::read_dir(&directory).map_err(Error::io)? {
            let entry = entry.map_err(Error::io)?;
            let name = entry.file_name().to_string_lossy().into_owned();
            let id = name
                .strip_suffix(".json")
                .filter(|id| valid_id(id, "log"))
                .ok_or_else(|| Error::new("CORRUPT_STORE", "Unexpected native log spool file"))?;
            let meta = entry.metadata().map_err(Error::io)?;
            if !entry.file_type().map_err(Error::io)?.is_file() || meta.len() > 32768 {
                return Err(Error::new("CORRUPT_STORE", "Oversized native log entry"));
            }
            let value: Value = serde_json::from_slice(&fs::read(entry.path()).map_err(Error::io)?)?;
            if value["id"] != id {
                return Err(Error::new("CORRUPT_STORE", "Native log ID mismatch"));
            }
            s.items.insert(id.into(), value);
            if s.items.len() > 1000 {
                return Err(Error::new(
                    "SIZE_LIMIT",
                    "Native log spool requires ingestion",
                ));
            }
        }
        for (id, item) in &s.items {
            crate::tools::atomic_write(
                &directory.join(format!("{id}.json")),
                &serde_json::to_vec(item)?,
            )?;
        }
        s.directory = Some(directory);
        Ok(())
    }
    pub(crate) fn record(&self, text: &str) {
        let mut s = self.state.lock().unwrap();
        if s.items.len() >= 1000 {
            self.dropped.fetch_add(1, Ordering::SeqCst);
            return;
        }
        let mut text = text.trim_end().to_string();
        let truncated = text.len() > 16384;
        if truncated {
            let mut end = 16384;
            while !text.is_char_boundary(end) {
                end -= 1;
            }
            text.truncate(end);
        }
        if text.is_empty() {
            return;
        }
        let redact =
            regex::Regex::new(r#"(?i)(bearer\s+|api[_-]?key[=:]\s*|password[=:]\s*)[^\s,;]+"#)
                .unwrap();
        let text = redact.replace_all(&text, "${1}[redacted]").into_owned();
        let level = if text.to_ascii_lowercase().contains("error") || text.contains("stopped:") {
            "error"
        } else if text.to_ascii_lowercase().contains("warn") {
            "warn"
        } else {
            "info"
        };
        let id = new_id("log");
        let item = json!({"id":id,"ts":artex_core::now(),"level":level,"tag":"engine","text":text,"truncated":truncated});
        if let Some(directory) = s.directory.as_ref() {
            if crate::tools::atomic_write(
                &directory.join(format!("{id}.json")),
                &serde_json::to_vec(&item).unwrap(),
            )
            .is_err()
            {
                s.failure = Some("Native log persistence failed".into());
            }
        }
        s.items.insert(id, item);
    }
    pub fn poll(&self, limit: usize) -> Result<Value> {
        let s = self.state.lock().unwrap();
        let mut values = s.items.values().cloned().collect::<Vec<_>>();
        values.sort_by_key(|v| {
            (
                v["ts"].as_str().unwrap_or("").to_owned(),
                v["id"].as_str().unwrap_or("").to_owned(),
            )
        });
        let mut items = vec![];
        let mut size = 0;
        for value in values.into_iter().take(limit.clamp(1, 100)) {
            let bytes = serde_json::to_vec(&value)?.len();
            if size + bytes > 768000 {
                break;
            }
            size += bytes;
            items.push(value);
        }
        Ok(
            json!({"items":items,"dropped":self.dropped.load(Ordering::SeqCst).to_string(),"capture_error":s.failure}),
        )
    }
    pub fn acknowledge(&self, ids: &[String]) -> Result<Value> {
        if ids.len() > 100 {
            return Err(Error::new(
                "SIZE_LIMIT",
                "Native log acknowledgement budget exceeded",
            ));
        }
        let mut s = self.state.lock().unwrap();
        for id in ids {
            if !valid_id(id, "log") {
                return Err(Error::new("INVALID_PARAMS", "Invalid native log id"));
            }
            if s.items.contains_key(id) {
                if let Some(directory) = s.directory.as_ref() {
                    match fs::remove_file(directory.join(format!("{id}.json"))) {
                        Ok(()) => {}
                        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                        Err(e) => return Err(Error::io(e)),
                    }
                }
                s.items.remove(id);
            }
        }
        Ok(json!({"acknowledged":ids.len()}))
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn stderr_redaction_restart_and_ack() {
        let root = tempfile::tempdir().unwrap();
        let logs = NativeLogs::default();
        logs.set_directory(root.path().into());
        logs.record("warn api_key=fixture-secret bearer fixture-token password=fixture-password");
        logs.activate().unwrap();
        let first = logs.poll(100).unwrap();
        assert_eq!(first["items"].as_array().unwrap().len(), 1);
        let item = &first["items"][0];
        assert_eq!(item["level"], "warn");
        assert!(!item["text"].as_str().unwrap().contains("fixture-"));
        let id = item["id"].as_str().unwrap().to_owned();
        drop(logs);
        let restored = NativeLogs::default();
        restored.set_directory(root.path().into());
        restored.activate().unwrap();
        assert_eq!(restored.poll(100).unwrap()["items"][0]["id"], id);
        restored.acknowledge(&[id]).unwrap();
        drop(restored);
        let empty = NativeLogs::default();
        empty.set_directory(root.path().into());
        empty.activate().unwrap();
        assert_eq!(empty.poll(100).unwrap()["items"], json!([]));
    }
    #[test]
    fn oversized_output_is_bounded_before_ingestion() {
        let logs = NativeLogs::default();
        logs.record(&"한".repeat(6000));
        let value = logs.poll(100).unwrap();
        assert_eq!(value["items"][0]["truncated"], true);
        assert!(value["items"][0]["text"].as_str().unwrap().len() <= 16384);
        for _ in 0..1100 {
            logs.record("line");
        }
        assert!(
            logs.poll(100).unwrap()["dropped"]
                .as_str()
                .unwrap()
                .parse::<u64>()
                .unwrap()
                > 0
        );
    }
}
