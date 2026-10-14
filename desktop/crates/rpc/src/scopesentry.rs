//! ScopeSentry network calls are native user jobs. Raw MCP results become a
//! single CAS domain transaction only after bounded collection has finished.
use crate::*;
use artex_domain::text;
use artex_runtime::{Cancellation, HostGateway};
use std::{sync::Arc, time::Duration};
struct ScopeClient {
    gateway: Arc<dyn HostGateway>,
    workspace: String,
    context: Context,
    server: Value,
    command: String,
    cancel: Cancellation,
    deadline: Instant,
}
impl ScopeClient {
    fn check(&self) -> Result<()> {
        if self.cancel.cancelled() {
            return Err(Error::new("CANCELLED", "ScopeSentry request cancelled"));
        }
        if Instant::now() >= self.deadline {
            return Err(Error::new(
                "TIMEOUT",
                "ScopeSentry collection deadline exceeded",
            ));
        }
        Ok(())
    }
    fn request(&self, operation: &str, tool: &str, args: Value) -> Result<Value> {
        self.check()?;
        let timeout = self
            .deadline
            .saturating_duration_since(Instant::now())
            .min(Duration::from_secs(60));
        let mut params = json!({"server":self.server,"command_id":self.command,"tool_name":tool,"arguments":args});
        params["operation"] = json!(operation);
        let result =
            self.gateway
                .request(&self.workspace, &self.context, operation, params, timeout)?;
        self.check()?;
        Ok(result)
    }
    fn call(&self, tool: &str, args: Value) -> Result<Value> {
        let result = self.request("mcp.call", tool, args)?;
        if result["isError"] == true {
            return Err(Error::new(
                "EXTERNAL_FAILURE",
                "ScopeSentry MCP tool rejected request",
            ));
        }
        if let Some(v) = result.get("structuredContent") {
            return Ok(v.clone());
        }
        let content = result["content"]
            .as_array()
            .ok_or_else(|| Error::new("PARSE_ERROR", "ScopeSentry MCP content is missing"))?;
        let mut text = String::new();
        for part in content {
            if let Some(t) = part["text"].as_str() {
                text.push_str(t);
            }
        }
        if text.len() > 1024 * 1024 {
            return Err(Error::new(
                "SIZE_LIMIT",
                "ScopeSentry tool result exceeds 1MiB",
            ));
        }
        serde_json::from_str(&text)
            .map_err(|_| Error::new("PARSE_ERROR", "ScopeSentry tool result is not JSON"))
    }
}
fn page_args(p: &Value) -> Value {
    let positive = |k: &str, default: u64| {
        p[k].as_f64()
            .filter(|n| n.is_finite() && *n >= 1.0 && n.fract() == 0.0 && *n <= 1000000.0)
            .map(|v| v as u64)
            .unwrap_or(default)
    };
    let mut out = json!({"pageIndex":positive("page",1),"pageSize":positive("size",50).min(500)});
    if !text(p, "search").is_empty() {
        out["search"] = p["search"].clone();
    }
    out
}
fn projected_list(raw: &Value, project: bool) -> Result<Value> {
    let values = if project {
        raw["result"]["All"].as_array()
    } else {
        raw["list"].as_array()
    };
    let values = values.cloned().unwrap_or_default();
    if values.len() > 5000 {
        return Err(Error::new(
            "SIZE_LIMIT",
            "ScopeSentry listing exceeds 5000 records",
        ));
    }
    let allowed = if project {
        &["id", "name", "logo", "AssetCount", "tag"][..]
    } else {
        &["id", "name", "status", "progress", "creatTime", "endTime"][..]
    };
    let mut out = Vec::new();
    for item in values {
        let mut row = json!({"id":item["id"].as_str().unwrap_or(""),"name":item["name"].as_str().unwrap_or("")});
        for key in allowed {
            if let Some(v) = item.get(*key).filter(|v| !v.is_null()) {
                row[*key] = v.clone();
            }
        }
        out.push(row);
    }
    Ok(json!(out))
}
fn collect_sync(client: &ScopeClient, p: &Value) -> Result<Value> {
    artex_domain::sync_scopesentry::validate_sync(p)?;
    let p = &p["body"];
    let project = p["dimension"] == "project";
    let targets = p["targets"].as_array().unwrap();
    let default_types = json!(["subdomain", "service", "app"]);
    let types = if p["asset_types"].as_array().unwrap().is_empty() {
        default_types.as_array().unwrap()
    } else {
        p["asset_types"].as_array().unwrap()
    };
    let page_size = p["page_size"]
        .as_u64()
        .filter(|n| *n > 0 && *n <= 500)
        .unwrap_or(100);
    let mut projects = Vec::new();
    let mut groups = Vec::new();
    let mut warnings = Vec::new();
    let mut errors = Vec::new();
    let mut collected_bytes = 0usize;
    for target in targets {
        client.check()?;
        if project && p["create_company"] == true {
            match client.call("get_project", json!({"id":target})) {
                Ok(value) => {
                    if !text(&value, "name").is_empty() {
                        projects.push(json!({"name":value["name"],"target":value["target"].as_str().unwrap_or("")}));
                    }
                }
                Err(e) => {
                    if matches!(e.code.as_str(), "CANCELLED" | "TIMEOUT") {
                        return Err(e);
                    }
                    warnings.push(json!(format!("Project details unavailable ({})", e.code)));
                }
            }
        }
        for kind in types {
            client.check()?;
            let kind = kind.as_str().unwrap();
            let ss_type = match kind {
                "subdomain" => "subdomain",
                "service" => "asset",
                "app" => "app",
                _ => {
                    warnings.push(json!("Unknown asset type was excluded"));
                    continue;
                }
            };
            let mut items = Vec::new();
            let mut page = 1u64;
            loop {
                client.check()?;
                let mut filter = json!({});
                filter[if project { "project" } else { "task" }] = json!([target]);
                let result=client.call("list_assets",json!({"asset_type":ss_type,"pageIndex":page,"pageSize":page_size,"filter":filter}));
                let raw = match result {
                    Ok(v) => v,
                    Err(e) => {
                        if matches!(e.code.as_str(), "CANCELLED" | "TIMEOUT") {
                            return Err(e);
                        }
                        errors.push(json!(format!("{kind} collection failed ({})", e.code)));
                        break;
                    }
                };
                let batch = raw["list"].as_array().cloned().unwrap_or_default();
                let count = batch.len();
                if count == 0 {
                    break;
                }
                for item in batch.into_iter().take(5000 - items.len()) {
                    collected_bytes = collected_bytes
                        .checked_add(serde_json::to_vec(&item)?.len())
                        .ok_or_else(|| {
                            Error::new("SIZE_LIMIT", "ScopeSentry collection size overflow")
                        })?;
                    if collected_bytes > 8 * 1024 * 1024 {
                        return Err(Error::new(
                            "SIZE_LIMIT",
                            "ScopeSentry sync exceeds 8MiB staging boundary; select fewer targets",
                        ));
                    }
                    items.push(item);
                }
                if items.len() >= 5000 {
                    warnings.push(json!(format!(
                        "{kind} reached 5000-record protection limit"
                    )));
                    break;
                }
                if count < (page_size as usize) {
                    break;
                }
                page += 1;
            }
            groups.push(json!({"kind":kind,"items":items}));
        }
    }
    Ok(json!({"projects":projects,"groups":groups,"warnings":warnings,"errors":errors}))
}
impl Router {
    pub(super) fn handle_scopesentry(&mut self, r: &DispatchRequest, c: &Context) -> Result<Value> {
        if c.actor.kind != "user" {
            return Err(Error::new(
                "PERMISSION_DENIED",
                "ScopeSentry synchronization requires native user",
            ));
        }
        if r.method == "ui.ssDatasource" {
            self.ensure_writable()?;
            self.require_command(r)?;
            if let Some(v) = self.repeated(r)? {
                return Ok(v);
            }
            let mut b = artex_domain::sync_scopesentry::plan(
                &r.method,
                &r.params,
                self.snapshot()?.as_ref(),
                c,
            )?;
            return self.commit(r, &mut b);
        }
        let s = self.snapshot()?;
        let row = artex_domain::sync_scopesentry::server(&s);
        let configured = row.is_some_and(|r| {
            artex_domain::sync_scopesentry::configured(r) && r.data["enabled"] == true
        });
        let status = artex_domain::sync_scopesentry::status(&s);
        if r.method == "ui.ssStatus" && !configured {
            return Ok(status);
        }
        let server = row
            .filter(|_| configured)
            .ok_or_else(|| {
                Error::new(
                    "INVALID_PARAMS",
                    "ScopeSentry data source is not enabled and configured",
                )
            })?
            .data
            .clone();
        let gateway = self
            .gateway
            .clone()
            .ok_or_else(|| Error::new("FEATURE_DISABLED", "Native MCP host unavailable"))?;
        if r.method == "ui.ssSync" {
            self.ensure_writable()?;
            self.require_command(r)?;
            if let Some(v) = self.repeated(r)? {
                return Ok(v);
            }
            artex_domain::sync_scopesentry::plan(&r.method, &r.params, &s, c)?;
        }
        let method = r.method.clone();
        let p = r.params.clone();
        let context = c.clone();
        let workspace = s.workspace_id.clone();
        let command = r.command_id.clone().unwrap_or_else(|| new_id("command"));
        let timeout = Duration::from_millis(r.timeout_ms).min(if r.method == "ui.ssSync" {
            Duration::from_secs(600)
        } else {
            Duration::from_secs(60)
        });
        let owner = r.clone();
        self.defer_read_worker(&owner, move |cancel| {
            let client = ScopeClient {
                gateway,
                workspace,
                context,
                server,
                command,
                cancel,
                deadline: Instant::now() + timeout,
            };
            match method.as_str() {
                "ui.ssStatus" => {
                    let mut status = status;
                    match client.request("mcp.refresh", "", json!({})) {
                        Ok(tools) => {
                            status["reachable"] = json!(true);
                            let _ = tools;
                        }
                        Err(e) => {
                            if e.code == "CANCELLED" {
                                return Err(e);
                            }
                            status["reachable"] = json!(false);
                        }
                    }
                    Ok(status)
                }
                "ui.ssProjects" => {
                    let raw = client.call("list_projects_data", page_args(&p))?;
                    let mut tag = json!({});
                    if let Some(o) = raw["tag"].as_object() {
                        for (k, v) in o {
                            if v.is_number() {
                                tag[k] = v.clone();
                            }
                        }
                    }
                    Ok(json!({"projects":projected_list(&raw,true)?,"tag":tag}))
                }
                "ui.ssTasks" => projected_list(&client.call("list_tasks", page_args(&p))?, false),
                "ui.ssSync" => collect_sync(&client, &p),
                _ => Err(Error::new("METHOD_NOT_FOUND", &method)),
            }
        })
    }
    pub(super) fn complete_scopesentry_sync(
        &mut self,
        r: &DispatchRequest,
        raw: &Value,
    ) -> Result<Value> {
        if let Some(v) = self.repeated(r)? {
            return Ok(v);
        }
        let c = Context {
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
        let mut b = artex_domain::sync_scopesentry::sync_plan(
            &r.params,
            raw,
            self.snapshot()?.as_ref(),
            &c,
        )?;
        self.commit(r, &mut b)
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};
    struct Fixture(AtomicUsize);
    impl HostGateway for Fixture {
        fn request(&self, _: &str, c: &Context, op: &str, p: Value, _: Duration) -> Result<Value> {
            assert_eq!(c.actor.kind, "user");
            self.0.fetch_add(1, Ordering::SeqCst);
            if op == "mcp.refresh" {
                return Ok(json!([]));
            }
            assert_eq!(op, "mcp.call");
            let value = match p["tool_name"].as_str().unwrap() {
                "list_projects_data" => {
                    json!({"result":{"All":[{"id":"opaque-project","name":"Project","unexpected":"excluded"}]},"tag":{"all":1}})
                }
                "list_tasks" => {
                    json!({"list":[{"id":"opaque-task","name":"Task","unexpected":"excluded"}]})
                }
                "get_project" => json!({"name":"Company","target":"example.test"}),
                "list_assets" => {
                    json!({"list":[{"host":"www.example.test","type":"A","ip":[],"value":["203.0.113.1"]}]})
                }
                _ => panic!("Unexpected fixture MCP tool"),
            };
            Ok(json!({"content":[{"type":"text","text":value.to_string()}]}))
        }
    }
    fn seed() -> (tempfile::TempDir, Router, Context) {
        let dir = tempfile::tempdir().unwrap();
        let mut router = Router::new(BTreeMap::new());
        router.store = Some(Store::create(&dir.path().join("w"), "SS fixture").unwrap());
        router.gateway = Some(Arc::new(Fixture(AtomicUsize::new(0))));
        let context = Context {
            actor: Actor {
                kind: "user".into(),
                agent_id: None,
                run_id: None,
                engine_epoch: router.engine_epoch.clone(),
            },
            task_id: None,
            intent_id: None,
            conversation_id: None,
        };
        let b=artex_domain::sync_scopesentry::plan("ui.ssDatasource",&json!({"body":{"url":"https://example.test/mcp","api_key":{"secret_ref":"secret_00000000000000000000000000000001","hint":"configured"}}}),&router.snapshot().unwrap(),&context).unwrap();
        router.internal_commit(b).unwrap();
        (dir, router, context)
    }
    fn make_request(router: &Router, method: &str, params: Value) -> DispatchRequest {
        Request {
            jsonrpc: "2.0".into(),
            protocol: "artex-rpc/1.0".into(),
            id: new_id("request"),
            method: method.into(),
            workspace_id: Some(router.snapshot().unwrap().workspace_id.clone()),
            engine_epoch: Some(router.engine_epoch.clone()),
            command_id: if method == "ui.ssSync" {
                Some(new_id("command"))
            } else {
                None
            },
            expected: vec![],
            timeout_ms: 2000,
            params,
        }
        .into()
    }
    fn completed(router: &mut Router) -> Value {
        for _ in 0..500 {
            router.runtime_tick().unwrap();
            if let Some(v) = router.drain_responses().pop() {
                return v;
            }
            std::thread::sleep(Duration::from_millis(2));
        }
        panic!("SS worker timed out")
    }
    #[test]
    fn listings_project_only_source_fields_and_status_checks_native_mcp() {
        let (_d, mut r, c) = seed();
        for (method, params) in [
            ("ui.ssStatus", json!({})),
            ("ui.ssProjects", json!({})),
            ("ui.ssTasks", json!({})),
        ] {
            let request = make_request(&r, method, params);
            assert_eq!(
                r.handle_scopesentry(&request, &c).unwrap()["__deferred"],
                true
            );
            let value = completed(&mut r);
            assert!(value["error"].is_null(), "{value:?}");
            let result = &value["result"];
            if method == "ui.ssStatus" {
                assert_eq!(result["reachable"], true);
            } else if method == "ui.ssProjects" {
                assert_eq!(result["projects"][0]["id"], "opaque-project");
                assert!(result["projects"][0].get("unexpected").is_none());
            } else {
                assert_eq!(result[0]["id"], "opaque-task");
            }
        }
    }
    #[test]
    fn sync_collects_natively_then_commits_cas_and_cancelled_job_creates_no_assets() {
        let (_d, mut r, c) = seed();
        let p = json!({"body":{"dimension":"project","targets":["opaque-project"],"asset_types":["subdomain"],"create_company":true}});
        let mut request = make_request(&r, "ui.ssSync", p);
        request.expected = artex_domain::sync_scopesentry::plan(
            "ui.ssSync",
            &request.params,
            &r.snapshot().unwrap(),
            &c,
        )
        .unwrap()
        .expected;
        r.handle_scopesentry(&request, &c).unwrap();
        let result = completed(&mut r);
        assert!(result["error"].is_null(), "{result:?}");
        assert_eq!(r.snapshot().unwrap().records("assets").len(), 3);
        assert_eq!(result["result"]["synced"]["subdomain"], 1);
        assert!(
            r.snapshot()
                .unwrap()
                .records("command_receipts")
                .iter()
                .any(|receipt| receipt.data["command_id"] == *request.command_id.as_ref().unwrap())
        );
        let (_d, mut r, c) = seed();
        let mut req = make_request(&r, "ui.ssSync", request.params.clone());
        req.expected = artex_domain::sync_scopesentry::plan(
            "ui.ssSync",
            &req.params,
            &r.snapshot().unwrap(),
            &c,
        )
        .unwrap()
        .expected;
        r.handle_scopesentry(&req, &c).unwrap();
        r.cancel_request(&req.id);
        let _ = completed(&mut r);
        for _ in 0..20 {
            r.runtime_tick().unwrap();
            std::thread::sleep(Duration::from_millis(2));
        }
        assert!(r.snapshot().unwrap().records("assets").is_empty());
        assert!(
            !r.snapshot()
                .unwrap()
                .records("command_receipts")
                .iter()
                .any(|receipt| receipt.data["command_id"] == *req.command_id.as_ref().unwrap())
        );
    }
}
