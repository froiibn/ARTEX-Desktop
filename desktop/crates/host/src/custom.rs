use crate::{ExecutionRequest, HostRuntime};
use artex_core::{Context, Error, Result, new_id};
use serde_json::{Value, json};
use std::{
    collections::BTreeMap,
    fs, thread,
    time::{Duration, Instant},
};
fn render(template: &str, params: &Value, quote: bool) -> String {
    let mut result = template.to_string();
    if let Some(params) = params.as_object() {
        for (key, value) in params {
            let text = value.as_str().map(str::to_owned).unwrap_or_else(|| {
                if value.is_null() {
                    String::new()
                } else {
                    value.to_string()
                }
            });
            let text = if quote {
                if cfg!(windows) {
                    format!("'{}'", text.replace('\'', "''"))
                } else {
                    format!("'{}'", text.replace('\'', "'\\''"))
                }
            } else {
                text
            };
            result = result.replace(&format!("{{{key}}}"), &text);
        }
    }
    result
}
pub fn test(host: &HostRuntime, input: &Value, context: &Context) -> Result<Value> {
    execute(host, &input["body"], input, context)
}
pub fn execute(
    host: &HostRuntime,
    spec: &Value,
    input: &Value,
    context: &Context,
) -> Result<Value> {
    let kind = spec["kind"]
        .as_str()
        .ok_or_else(|| Error::new("INVALID_PARAMS", "Custom tool kind required"))?;
    let params = &spec["params"];
    let exec = &spec["exec"];
    let cap = input["directory_capability"].as_str().ok_or_else(|| {
        Error::new(
            "PERMISSION_DENIED",
            "Custom tool directory capability required",
        )
    })?;
    if kind == "http" {
        let http = crate::network::async_client(
            &json!({"proxy":input["network_config"].get("proxy").unwrap_or(&exec["proxy"]),"insecure_tls":input["network_config"]["insecure_tls"],"ca_pem":input["network_config"]["ca_pem"]}),
        )?;
        let url = render(
            exec["url"]
                .as_str()
                .ok_or_else(|| Error::new("INVALID_PARAMS", "Custom URL required"))?,
            params,
            false,
        );
        let parsed = reqwest::Url::parse(&url)
            .map_err(|_| Error::new("INVALID_PARAMS", "Invalid custom URL"))?;
        if !matches!(parsed.scheme(), "http" | "https") {
            return Err(Error::new("INVALID_PARAMS", "Custom HTTP URL required"));
        }
        let method =
            reqwest::Method::from_bytes(exec["method"].as_str().unwrap_or("GET").as_bytes())
                .map_err(|_| Error::new("INVALID_PARAMS", "Invalid HTTP method"))?;
        let mut request = http.request(method, parsed).timeout(Duration::from_millis(
            exec["timeout_ms"]
                .as_u64()
                .unwrap_or(30000)
                .clamp(1, 600000),
        ));
        if let Some(headers) = exec["headers"].as_object() {
            for (name, value) in headers {
                let value = render(
                    value.as_str().ok_or_else(|| {
                        Error::new("INVALID_PARAMS", "HTTP header must be a string")
                    })?,
                    params,
                    false,
                );
                request = request.header(name, &value);
            }
        }
        if let Some(body) = exec["body"].as_str() {
            request = request.body(render(body, params, false));
        }
        let (status, bytes) = crate::network::send_custom(host, context, request)?;
        return Ok(
            json!({"output":format!("HTTP {status}\n{}",String::from_utf8_lossy(&bytes)),"is_error":status>=400}),
        );
    }
    let cwd = host.capabilities.root(cap)?;
    let execution_id = input["execution_id"]
        .as_str()
        .map(str::to_owned)
        .unwrap_or_else(|| new_id("exec"));
    let owner = context.actor.run_id.clone().unwrap_or_else(|| {
        format!(
            "ui_{}",
            input["command_id"].as_str().unwrap_or(&execution_id)
        )
    });
    let mut environment = input["native_environment"]
        .as_object()
        .map(|map| {
            map.iter()
                .filter_map(|(k, v)| v.as_str().map(|v| (k.clone(), v.to_string())))
                .collect::<BTreeMap<_, _>>()
        })
        .unwrap_or_default();
    let mut stdin = None;
    let script_directory;
    let (executable, args) = match kind {
        "command" => {
            let command = render(
                exec["command"]
                    .as_str()
                    .filter(|s| !s.is_empty())
                    .ok_or_else(|| Error::new("INVALID_PARAMS", "Custom command required"))?,
                params,
                true,
            );
            if cfg!(windows) {
                (
                    "powershell.exe".into(),
                    vec![
                        "-NoProfile".into(),
                        "-NonInteractive".into(),
                        "-Command".into(),
                        command,
                    ],
                )
            } else {
                ("bash".into(), vec!["-c".into(), command])
            }
        }
        "script" => {
            let code = exec["code"]
                .as_str()
                .filter(|s| !s.trim().is_empty())
                .ok_or_else(|| Error::new("INVALID_PARAMS", "Python code required"))?;
            script_directory = tempfile::tempdir_in(&cwd).map_err(Error::io)?;
            let path = script_directory.path().join("tool.py");
            fs::write(&path, code).map_err(Error::io)?;
            stdin = Some(serde_json::to_vec(params)?);
            if let Some(params) = params.as_object() {
                for (k, v) in params {
                    if v.is_string() || v.is_number() || v.is_boolean() || v.is_null() {
                        environment.insert(
                            format!("TOOL_{}", k.to_uppercase()),
                            v.as_str().map(str::to_owned).unwrap_or_else(|| {
                                if v.is_null() {
                                    String::new()
                                } else {
                                    v.to_string()
                                }
                            }),
                        );
                    }
                }
            }
            let python = input["python_interpreter"]
                .as_str()
                .filter(|s| !s.is_empty())
                .unwrap_or(if cfg!(windows) {
                    "python.exe"
                } else {
                    "python3"
                });
            (python.into(), vec![path.to_string_lossy().into_owned()])
        }
        _ => {
            return Err(Error::new(
                "INVALID_PARAMS",
                "Custom tool kind must be command, script or http",
            ));
        }
    };
    let timeout = exec["timeout_ms"]
        .as_u64()
        .unwrap_or(120000)
        .clamp(1, 600000);
    host.executions.execute_with_input(
        ExecutionRequest {
            execution_id: execution_id.clone(),
            run_id: owner.clone(),
            engine_epoch: context.actor.engine_epoch.clone(),
            executable,
            args,
            cwd,
            timeout_ms: timeout,
        },
        stdin,
        &environment,
    )?;
    let end = Instant::now() + Duration::from_millis(timeout + 2000);
    while Instant::now() < end {
        let state = host
            .executions
            .status(&execution_id, &owner, &context.actor.engine_epoch)?;
        if state.state != "running" {
            return Ok(json!({"output":state.output,"is_error":state.state!="completed"}));
        }
        thread::sleep(Duration::from_millis(10));
    }
    host.executions
        .stop(&execution_id, &owner, &context.actor.engine_epoch)?;
    Err(Error::new(
        "TIMEOUT",
        "Custom tool termination deadline exceeded",
    ))
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn quoting_preserves_values() {
        let rendered = render("echo {x}", &json!({"x":"hello'; $value"}), true);
        if cfg!(windows) {
            assert_eq!(rendered, "echo 'hello''; $value'");
        } else {
            assert_eq!(rendered, "echo 'hello'\\''; $value'");
        }
    }
}
