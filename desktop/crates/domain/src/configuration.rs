use crate::*;

pub fn plan(m: &str, p: &Value, s: &Snapshot, _: &Context) -> Result<CommandBatch> {
    let mut b = Planner::new(s, m, p);
    match m {
        "create_custom_tool" | "update_custom_tool" => {
            let key = required(p, "key")?.trim();
            if !key.chars().next().is_some_and(|c| c.is_ascii_lowercase())
                || !key
                    .chars()
                    .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_')
            {
                return Err(Error::new("INVALID_PARAMS", "invalid custom tool key"));
            }
            let existing = s
                .records("tools")
                .into_iter()
                .find(|r| text(&r.data, "key") == key);
            let id = if m == "create_custom_tool" {
                if existing.is_some() {
                    return Err(Error::new("VERSION_CONFLICT", "tool already exists"));
                }
                new_id("toolconfig")
            } else {
                let r = existing.ok_or_else(|| Error::new("NOT_FOUND", "tool does not exist"))?;
                if r.data["system"] == true {
                    return Err(Error::new(
                        "PERMISSION_DENIED",
                        "system tool cannot become custom tool",
                    ));
                }
                r.header.id.clone()
            };
            let mut data = existing
                .map(|r| r.data.clone())
                .unwrap_or(default_data("tools")?);
            if m == "update_custom_tool" {
                let defaults = default_data("tools")?;
                for key in ["description", "schema", "agents", "exec", "deferred"] {
                    data[key] = defaults[key].clone();
                }
                data["enabled"] = json!(true);
            }
            merge(&mut data, p);
            data["key"] = json!(key);
            data["system"] = json!(false);
            if m == "create_custom_tool" && p["enabled"].is_null() {
                data["enabled"] = json!(true);
            }
            let kind = required(&data, "kind")?.to_owned();
            if !["shell", "command", "script", "http"].contains(&kind.as_str()) {
                return Err(Error::new("INVALID_PARAMS", "invalid custom tool kind"));
            }
            if kind != "shell" && !data["exec"].is_object() {
                return Err(Error::new(
                    "INVALID_PARAMS",
                    "execution specification required",
                ));
            }
            if kind == "shell" {
                data["deferred"] = json!(false);
            }
            if kind == "http"
                && data["schema"]["properties"]
                    .as_object()
                    .is_none_or(|p| p.is_empty())
            {
                return Err(Error::new(
                    "INVALID_PARAMS",
                    "HTTP tools require a schema containing properties",
                ));
            }
            b.put("tools", &id, data);
            Ok(b.done(json!({"key":key,"id":id})))
        }
        "create_mcp" | "update_mcp" => {
            let existing = if m == "update_mcp" {
                let id = p["id"]
                    .as_str()
                    .or(p["mcp_id"].as_str())
                    .ok_or_else(|| Error::new("INVALID_PARAMS", "id required"))?;
                Some(record(s, "mcp_servers", id)?)
            } else {
                None
            };
            let mut data = existing
                .map(|r| r.data.clone())
                .unwrap_or(default_data("mcp_servers")?);
            if m == "update_mcp" {
                let defaults = default_data("mcp_servers")?;
                for key in ["command", "args", "env", "url", "insecure"] {
                    data[key] = defaults[key].clone();
                }
                data["enabled"] = json!(true);
            }
            merge(&mut data, p);
            required(&data, "name")?;
            let transport = required(&data, "transport")?;
            if !["stdio", "http", "sse"].contains(&transport) {
                return Err(Error::new("INVALID_PARAMS", "invalid MCP transport"));
            }
            if transport == "stdio" {
                required(&data, "command")?;
            } else {
                url::Url::parse(required(&data, "url")?)
                    .map_err(|_| Error::new("INVALID_PARAMS", "invalid MCP URL"))?;
            }
            if !data["env"].is_object() {
                return Err(Error::new(
                    "INVALID_PARAMS",
                    "MCP environment must be an object",
                ));
            }
            let env = data["env"].as_object().unwrap();
            if env.contains_key("secret_ref") {
                if env.len() != 2
                    || !env
                        .get("secret_ref")
                        .and_then(Value::as_str)
                        .is_some_and(|id| artex_core::valid_id(id, "secret"))
                    || !env.get("hint").is_some_and(Value::is_string)
                {
                    return Err(Error::new(
                        "INVALID_PARAMS",
                        "Invalid sealed MCP environment SecretRef",
                    ));
                }
            } else {
                for (key, value) in env {
                    if !value.is_string() && value["secret_ref"].as_str().is_none() {
                        return Err(Error::new(
                            "INVALID_PARAMS",
                            "environment value requires string or SecretRef",
                        ));
                    }
                    let key = key.to_lowercase();
                    if value.is_string()
                        && [
                            "password",
                            "token",
                            "secret",
                            "api_key",
                            "apikey",
                            "authorization",
                        ]
                        .iter()
                        .any(|hint| key.contains(hint))
                    {
                        return Err(Error::new(
                            "PERMISSION_DENIED",
                            "secret environment literals require host SecretRef normalization",
                        ));
                    }
                }
            }
            let id = existing
                .map(|r| r.header.id.clone())
                .unwrap_or_else(|| new_id("mcp"));
            data["id"] = json!(id);
            if m == "create_mcp" && p["enabled"].is_null() {
                data["enabled"] = json!(true);
            }
            b.put("mcp_servers", &id, data);
            Ok(b.done(json!({"id":id,"connected":false})))
        }
        _ => Err(Error::new("METHOD_NOT_FOUND", m)),
    }
}
