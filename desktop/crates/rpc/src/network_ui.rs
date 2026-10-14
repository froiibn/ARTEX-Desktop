//! Interactive provider probes run outside the single writer with sealed credentials.
use super::*;
use artex_domain::record;
use artex_runtime::{Message, Prompt};
use std::time::Duration;
impl Router {
    pub(super) fn mcp_cache_plan(
        &self,
        r: &DispatchRequest,
        result: &Value,
    ) -> Result<CommandBatch> {
        let s = self.snapshot()?;
        let id = artex_domain::required(&r.params, "id")?;
        let server = record(&s, "mcp_servers", id)?;
        let list = result
            .as_array()
            .ok_or_else(|| Error::new("EXTERNAL_FAILURE", "MCP discovery returned no tool list"))?;
        let mut p = artex_domain::Planner::new(&s, &r.method, &r.params);
        p.expect(server);
        for old in s
            .records("mcp_tools_cache")
            .into_iter()
            .filter(|t| t.data["server_id"] == id)
        {
            p.delete("mcp_tools_cache", &old.header.id);
        }
        let mut names = std::collections::BTreeSet::new();
        let mut wire = Vec::new();
        for tool in list {
            let name = artex_domain::required(tool, "name")?;
            if !names.insert(name.to_owned()) {
                return Err(Error::new(
                    "EXTERNAL_FAILURE",
                    "MCP discovery contains duplicate tool names",
                ));
            }
            p.insert("mcp_tools_cache","mcptool",json!({"server_id":id,"tool_name":name,"description":tool["description"].as_str().unwrap_or(""),"schema":tool.get("input_schema").cloned().unwrap_or(json!({"type":"object"}))}))?;
            wire.push(
                json!({"name":name,"description":tool["description"].as_str().unwrap_or("")}),
            );
        }
        Ok(p.done(json!(wire)))
    }
    pub(super) fn handle_llm_network(&mut self, r: &DispatchRequest, c: &Context) -> Result<Value> {
        if c.actor.kind != "user" {
            return Err(Error::new(
                "PERMISSION_DENIED",
                "Provider configuration tests require the native user",
            ));
        }
        let s = self.snapshot()?;
        let saved = r.params["profile_id"]
            .as_str()
            .map(|id| record(&s, "llm_profiles", id))
            .transpose()?;
        let credential = if r.params["api_key"]["secret_ref"].is_string() {
            r.params["api_key"].clone()
        } else {
            saved
                .map(|p| p.data["api_key"].clone())
                .unwrap_or(Value::Null)
        };
        if credential.is_null() {
            return Ok(json!({"ok":false,"error":"API key required"}));
        }
        let gateway = self
            .gateway
            .clone()
            .ok_or_else(|| Error::new("FEATURE_DISABLED", "Native credential vault unavailable"))?;
        let mut profile = saved.map(|p| p.data.clone()).unwrap_or_else(|| json!({}));
        for key in [
            "model",
            "base_url",
            "proxy",
            "thinking_type",
            "reasoning_effort",
            "streaming",
            "session_header_key",
        ] {
            if let Some(v) = r.params.get(key) {
                profile[key] = v.clone();
            }
        }
        profile["format"] = r.params["provider"].clone();
        profile["api_key"] = credential;
        if r.method == "ui.testLLM" {
            profile["max_tokens"] = json!(256);
        }
        let workspace = s.workspace_id.clone();
        let context = c.clone();
        let method = r.method.clone();
        let timeout = Duration::from_millis(r.timeout_ms.min(30000));
        self.defer_read_worker(r,move|cancel|{
            let start=Instant::now();
            let result=(||{
                let secret=gateway.request(&workspace,&context,"secret.resolve_user",json!({"secret_ref":profile["api_key"]["secret_ref"]}),timeout)?;
                cancel.check()?;
                let key=secret["value"].as_str().ok_or_else(||Error::new("SECRET_UNAVAILABLE","Provider secret is unavailable"))?;
                let left=timeout.saturating_sub(start.elapsed());if left.is_zero(){return Err(Error::new("TIMEOUT","Provider probe deadline reached"));}
                if method=="ui.fetchLLMModels"{return artex_runtime::provider::models(&profile,key,&cancel,left).map(|models|json!({"ok":true,"models":models}));}
                let prompt=Prompt{system:"Respond briefly in Korean or English.".into(),messages:vec![Message{role:"user".into(),content:vec![json!({"type":"text","text":"Reply with one short greeting to confirm this connection."})]}],tools:vec![],session_id:new_id("probe")};
                let turn=artex_runtime::provider::generate(&profile,key,&prompt,&cancel,left)?;
                Ok(json!({"ok":true,"latency_ms":start.elapsed().as_millis() as u64,"model":profile["model"],"reply":turn.text}))
            })();
            match result{Ok(v)=>Ok(v),Err(e)if e.code=="CANCELLED"=>Err(e),Err(e)=>Ok(json!({"ok":false,"error":format!("{}: {}",e.code,e.message)}))}
        })
    }
}
