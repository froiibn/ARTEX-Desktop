//! Bidirectional framed RPC. Pending replies bypass the bounded UI request queue.
use artex_core::{Context, Error, Result, new_id};
use artex_runtime::HostGateway;
use serde_json::{Value, json};
use std::{
    collections::HashMap,
    sync::{Mutex, mpsc},
    time::{Duration, Instant},
};
pub struct PipeGateway {
    output: mpsc::SyncSender<Option<Value>>,
    pending: Mutex<HashMap<String, mpsc::SyncSender<Value>>>,
}
impl PipeGateway {
    pub fn new(output: mpsc::SyncSender<Option<Value>>) -> Self {
        Self {
            output,
            pending: Mutex::new(HashMap::new()),
        }
    }
    pub fn receive(&self, value: &Value) -> bool {
        let Some(id) = value["id"].as_str().filter(|s| s.starts_with("hostreq_")) else {
            return false;
        };
        if value["method"].is_string() {
            return false;
        }
        if let Some(tx) = self.pending.lock().unwrap().remove(id) {
            let _ = tx.try_send(value.clone());
        }
        true
    }
    pub fn disconnect(&self) {
        self.pending.lock().unwrap().clear();
    }
}
impl HostGateway for PipeGateway {
    fn request(
        &self,
        workspace: &str,
        context: &Context,
        operation: &str,
        parameters: Value,
        timeout: Duration,
    ) -> Result<Value> {
        let started = Instant::now();
        let input = serde_json::to_vec(&parameters)?;
        let parameters = if !operation.starts_with("host.input.") && input.len() > 768000 {
            use base64::Engine;
            use sha2::{Digest, Sha256};
            if input.len() > 64 * 1024 * 1024 {
                return Err(Error::new("SIZE_LIMIT", "Native input exceeds 64 MiB"));
            }
            let remaining = || {
                timeout
                    .checked_sub(started.elapsed())
                    .filter(|t| !t.is_zero())
                    .ok_or_else(|| Error::new("TIMEOUT", "Native input staging deadline exceeded"))
            };
            let digest = hex::encode(Sha256::digest(&input));
            let begin = self.request(
                workspace,
                context,
                "host.input.begin",
                json!({"length":input.len().to_string(),"sha256":digest}),
                remaining()?,
            )?;
            let token = begin["token"]
                .as_str()
                .filter(|s| artex_core::valid_id(s, "transfer"))
                .ok_or_else(|| {
                    Error::new("INVALID_REQUEST", "Invalid native input transfer token")
                })?;
            for (offset, chunk) in input.chunks(65536).enumerate() {
                let written=self.request(workspace,context,"host.input.write",json!({"token":token,"offset":(offset*65536).to_string(),"data_base64":base64::engine::general_purpose::STANDARD.encode(chunk)}),remaining()?)?;
                if written["length"]
                    .as_str()
                    .and_then(|s| s.parse::<usize>().ok())
                    != Some(offset * 65536 + chunk.len())
                {
                    return Err(Error::new(
                        "INVALID_REQUEST",
                        "Native input chunk ACK mismatch",
                    ));
                }
            }
            let committed = self.request(
                workspace,
                context,
                "host.input.commit",
                json!({"token":token}),
                remaining()?,
            )?;
            if committed["token"] != token
                || committed["length"] != input.len().to_string()
                || committed["sha256"] != digest
            {
                return Err(Error::new(
                    "CORRUPT_STORE",
                    "Native input commit identity mismatch",
                ));
            }
            json!({"host_input":{"token":token}})
        } else {
            parameters
        };
        let id = new_id("hostreq");
        let (tx, rx) = mpsc::sync_channel(1);
        {
            let mut pending = self.pending.lock().unwrap();
            if pending.len() >= 32 {
                return Err(Error::new("BUSY", "Host request queue is full"));
            }
            pending.insert(id.clone(), tx);
        }
        let mut params = parameters;
        if !params.is_object() {
            params = json!({});
        }
        params["operation"] = json!(operation);
        params["context"] = json!(context);
        let frame = json!({"jsonrpc":"2.0","protocol":"artex-rpc/1.0","id":id,"method":"host.request","engine_epoch":context.actor.engine_epoch,"workspace_id":workspace,"params":params});
        if serde_json::to_vec(&frame)?.len() > crate::MAX_FRAME {
            self.pending.lock().unwrap().remove(&id);
            return Err(Error::new("SIZE_LIMIT", "Host request exceeds frame limit"));
        }
        if self.output.try_send(Some(frame)).is_err() {
            self.pending.lock().unwrap().remove(&id);
            return Err(Error::new("BUSY", "Host pipe writer queue unavailable"));
        }
        let result = rx.recv_timeout(timeout.saturating_sub(started.elapsed()));
        self.pending.lock().unwrap().remove(&id);
        let v = result.map_err(|e| {
            Error::new(
                if matches!(e, mpsc::RecvTimeoutError::Timeout) {
                    "TIMEOUT"
                } else {
                    "IO_FAILURE"
                },
                "Unconfirmed host request; automatic repetition is forbidden",
            )
        })?;
        if v["jsonrpc"] != "2.0" {
            return Err(Error::new("INVALID_REQUEST", "Invalid host reply"));
        }
        if let Some(error) = v.get("error") {
            let d = &error["data"];
            return Err(Error {
                code: d["code"].as_str().unwrap_or("EXTERNAL_FAILURE").into(),
                message: error["message"]
                    .as_str()
                    .unwrap_or("Host operation failed")
                    .into(),
                retryable: d["retryable"] == true,
                recovery_required: d["recovery_required"] == true,
                details: d["details"].clone(),
            });
        }
        let value = v
            .get("result")
            .cloned()
            .ok_or_else(|| Error::new("INVALID_REQUEST", "Host reply requires result"))?;
        if !operation.starts_with("host.result.")
            && value
                .as_object()
                .is_some_and(|v| v.len() == 1 && v.contains_key("host_result"))
        {
            let result = (|| {
                use base64::Engine;
                use sha2::{Digest, Sha256};
                let reference = &value["host_result"];
                let token = reference["token"]
                    .as_str()
                    .filter(|s| artex_core::valid_id(s, "transfer"))
                    .ok_or_else(|| {
                        Error::new("INVALID_REQUEST", "Invalid native result transfer token")
                    })?;
                let length = reference["length"]
                    .as_str()
                    .and_then(|s| s.parse::<usize>().ok())
                    .filter(|n| *n > 0 && *n <= 64 * 1024 * 1024)
                    .ok_or_else(|| {
                        Error::new("SIZE_LIMIT", "Native result length exceeds 64 MiB")
                    })?;
                let digest = reference["sha256"]
                    .as_str()
                    .filter(|s| s.len() == 64 && s.bytes().all(|b| b.is_ascii_hexdigit()))
                    .ok_or_else(|| Error::new("INVALID_REQUEST", "Invalid native result hash"))?;
                let mut bytes = Vec::with_capacity(length);
                while bytes.len() < length {
                    let remaining = timeout
                        .checked_sub(started.elapsed())
                        .filter(|t| !t.is_zero())
                        .ok_or_else(|| {
                            Error::new("TIMEOUT", "Native result transfer deadline exceeded")
                        })?;
                    let chunk = self.request(
                        workspace,
                        context,
                        "host.result.read",
                        json!({"token":token,"offset":bytes.len().to_string(),"length":65536}),
                        remaining,
                    )?;
                    let data = base64::engine::general_purpose::STANDARD
                        .decode(chunk["data_base64"].as_str().ok_or_else(|| {
                            Error::new("INVALID_REQUEST", "Native result chunk missing")
                        })?)
                        .map_err(|_| {
                            Error::new("INVALID_REQUEST", "Invalid native result base64")
                        })?;
                    if data.is_empty()
                        || data.len() > 65536
                        || bytes.len() + data.len() > length
                        || chunk["length"]
                            .as_str()
                            .and_then(|s| s.parse::<usize>().ok())
                            != Some(data.len())
                        || chunk["eof"].as_bool() != Some(bytes.len() + data.len() == length)
                        || chunk["sha256"] != digest
                    {
                        return Err(Error::new(
                            "CORRUPT_STORE",
                            "Native result chunk bounds or identity mismatch",
                        ));
                    }
                    bytes.extend(data);
                }
                if hex::encode(Sha256::digest(&bytes)) != digest {
                    return Err(Error::new("CORRUPT_STORE", "Native result hash mismatch"));
                }
                let result = serde_json::from_slice::<Value>(&bytes).map_err(|_| {
                    Error::new(
                        "INVALID_REQUEST",
                        "Native result transfer contains invalid JSON",
                    )
                })?;
                // ACK only after complete hash and JSON validation. On failure,
                // expiry cleans the spool and the controller reconciles effects.
                let remaining = timeout
                    .checked_sub(started.elapsed())
                    .filter(|t| !t.is_zero())
                    .ok_or_else(|| Error::new("TIMEOUT", "Native result ACK deadline exceeded"))?;
                let ack = self.request(
                    workspace,
                    context,
                    "host.result.ack",
                    json!({"token":token}),
                    remaining,
                )?;
                if ack["acknowledged"] != true {
                    return Err(Error::new(
                        "INVALID_REQUEST",
                        "Native result ACK unconfirmed",
                    ));
                }
                Ok(result)
            })();
            return result.map_err(|mut error: Error| {
                error.recovery_required = true;
                error
            });
        }
        Ok(value)
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    #[test]
    fn oversized_bidirectional_json_preserves_context_and_validates_every_frame() {
        use base64::Engine;
        use sha2::{Digest, Sha256};
        let (tx, rx) = mpsc::sync_channel(16);
        let gateway = Arc::new(PipeGateway::new(tx));
        let worker = gateway.clone();
        let context = Context {
            actor: artex_core::Actor {
                kind: "user".into(),
                agent_id: None,
                run_id: None,
                engine_epoch: new_id("epoch"),
            },
            task_id: None,
            intent_id: None,
            conversation_id: None,
        };
        let owner = context.clone();
        let input = json!({"content":"x".repeat(2*1024*1024)});
        let expected = input.clone();
        let task = std::thread::spawn(move || {
            worker.request(
                "workspace_transfer",
                &owner,
                "ui.execute",
                input,
                Duration::from_secs(10),
            )
        });
        let token = new_id("transfer");
        let result = json!({"content":"한".repeat(500000)});
        let bytes = serde_json::to_vec(&result).unwrap();
        let digest = hex::encode(Sha256::digest(&bytes));
        let mut staged = vec![];
        let mut input_digest = String::new();
        let mut acked = false;
        loop {
            let frame = rx.recv_timeout(Duration::from_secs(10)).unwrap().unwrap();
            assert!(serde_json::to_vec(&frame).unwrap().len() <= crate::MAX_FRAME);
            assert_eq!(frame["params"]["context"], json!(context));
            assert_eq!(frame["workspace_id"], "workspace_transfer");
            let params = &frame["params"];
            let reply = match params["operation"].as_str().unwrap() {
                "host.input.begin" => {
                    input_digest = params["sha256"].as_str().unwrap().into();
                    json!({"token":token})
                }
                "host.input.write" => {
                    assert_eq!(params["offset"], staged.len().to_string());
                    staged.extend(
                        base64::engine::general_purpose::STANDARD
                            .decode(params["data_base64"].as_str().unwrap())
                            .unwrap(),
                    );
                    json!({"length":staged.len().to_string()})
                }
                "host.input.commit" => {
                    assert_eq!(serde_json::from_slice::<Value>(&staged).unwrap(), expected);
                    assert_eq!(hex::encode(Sha256::digest(&staged)), input_digest);
                    json!({"token":token,"length":staged.len().to_string(),"sha256":input_digest})
                }
                "ui.execute" => {
                    assert_eq!(params["host_input"]["token"], token);
                    assert!(params.get("content").is_none());
                    json!({"host_result":{"token":token,"length":bytes.len().to_string(),"sha256":digest}})
                }
                "host.result.read" => {
                    let offset = params["offset"].as_str().unwrap().parse::<usize>().unwrap();
                    let end = (offset + 65536).min(bytes.len());
                    json!({"data_base64":base64::engine::general_purpose::STANDARD.encode(&bytes[offset..end]),"length":(end-offset).to_string(),"eof":end==bytes.len(),"sha256":digest})
                }
                "host.result.ack" => {
                    acked = true;
                    json!({"acknowledged":true})
                }
                _ => panic!("Unexpected frame {frame}"),
            };
            gateway.receive(&json!({"jsonrpc":"2.0","id":frame["id"],"result":reply}));
            if acked {
                break;
            }
        }
        assert_eq!(task.join().unwrap().unwrap(), result);
        assert!(gateway.pending.lock().unwrap().is_empty());
    }
    #[test]
    fn multiplexes_private_reply_and_rejects_request_as_reply() {
        let (tx, rx) = mpsc::sync_channel(16);
        let g = Arc::new(PipeGateway::new(tx));
        let worker = g.clone();
        let context = Context {
            actor: artex_core::Actor {
                kind: "agent".into(),
                agent_id: Some(new_id("agent")),
                run_id: Some(new_id("run")),
                engine_epoch: new_id("epoch"),
            },
            task_id: None,
            intent_id: None,
            conversation_id: None,
        };
        let t = std::thread::spawn(move || {
            worker.request(
                "workspace_x",
                &context,
                "tool.execute",
                json!({"name":"Read","input":{}}),
                Duration::from_secs(2),
            )
        });
        let f = rx.recv().unwrap().unwrap();
        assert!(!g.receive(&f));
        assert!(g.receive(&json!({"jsonrpc":"2.0","id":f["id"],"result":{"text":"ok"}})));
        assert_eq!(t.join().unwrap().unwrap()["text"], "ok");
    }
}
