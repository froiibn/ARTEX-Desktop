//! Configured-channel delivery; credentials are resolved only inside the native host.
use crate::SecretVault;
use artex_core::{Error, Result};
use base64::{Engine as _, engine::general_purpose::STANDARD};
use hmac::{Hmac, Mac};
use serde_json::{Value, json};
use sha2::Sha256;
use std::{
    collections::HashMap,
    io::{Read, Write},
    net::{IpAddr, SocketAddr, ToSocketAddrs},
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};
fn text<'a>(v: &'a Value, k: &str) -> &'a str {
    v[k].as_str().unwrap_or("")
}
fn failure(message: &str, retryable: bool) -> Error {
    let mut e = Error::new("EXTERNAL_FAILURE", message);
    e.retryable = retryable;
    e
}
fn invalid(message: &str) -> Error {
    Error::new("INVALID_PARAMS", message)
}
fn resolve(value: &mut Value, vault: &SecretVault, depth: usize, structured: bool) -> Result<()> {
    if depth > 32 {
        return Err(invalid("Notification configuration nesting exceeded"));
    }
    if let Some(id) = value["secret_ref"].as_str() {
        let secret = vault.resolve(id)?;
        *value = if structured {
            let decoded: Value = serde_json::from_str(&secret)
                .map_err(|_| invalid("Notification secret headers must be JSON object"))?;
            if !decoded.is_object() {
                return Err(invalid("Notification secret headers must be JSON object"));
            }
            decoded
        } else {
            json!(secret)
        };
        return Ok(());
    }
    match value {
        Value::Object(o) => {
            for (key, v) in o.iter_mut() {
                resolve(v, vault, depth + 1, key == "headers")?
            }
        }
        Value::Array(a) => {
            for v in a {
                resolve(v, vault, depth + 1, false)?
            }
        }
        _ => {}
    }
    Ok(())
}
fn blocked(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(v) => {
            v.is_loopback() || v.is_link_local() || v.is_unspecified() || v.is_multicast()
        }
        IpAddr::V6(v) => v
            .to_ipv4_mapped()
            .map(|v| blocked(IpAddr::V4(v)))
            .unwrap_or_else(|| {
                v.is_loopback()
                    || v.is_unspecified()
                    || v.is_multicast()
                    || (v.segments()[0] & 0xffc0) == 0xfe80
            }),
    }
}
fn resolved(host: &str, port: u16, allow_local: bool) -> Result<Vec<SocketAddr>> {
    let addresses = (host, port)
        .to_socket_addrs()
        .map_err(|_| failure("Notification DNS resolution failed", true))?
        .collect::<Vec<_>>();
    if addresses.is_empty() || addresses.len() > 32 {
        return Err(failure("Notification destination unavailable", true));
    }
    if !allow_local && addresses.iter().any(|a| blocked(a.ip())) {
        return Err(invalid(
            "Notification loopback or link-local destination is blocked",
        ));
    }
    Ok(addresses)
}
fn endpoint(raw: &str) -> Result<reqwest::Url> {
    let url = reqwest::Url::parse(raw).map_err(|_| invalid("Notification URL is invalid"))?;
    if !matches!(url.scheme(), "http" | "https")
        || url.host_str().is_none()
        || !url.username().is_empty()
        || url.password().is_some()
    {
        return Err(invalid("Notification URL must be HTTP(S) without userinfo"));
    }
    Ok(url)
}
fn post(
    url: reqwest::Url,
    method: &str,
    headers: &Value,
    payload: &Value,
    allow_local: bool,
) -> Result<Value> {
    let host = url.host_str().unwrap().to_owned();
    let port = url.port_or_known_default().unwrap();
    let addresses = resolved(&host, port, allow_local)?;
    let expected_host = host.clone();
    let expected_port = port;
    let initial_scheme = url.scheme().to_owned();
    let client = reqwest::blocking::Client::builder()
        .no_proxy()
        .timeout(Duration::from_secs(15))
        .connect_timeout(Duration::from_secs(10))
        .resolve_to_addrs(&host, &addresses)
        .redirect(reqwest::redirect::Policy::custom(move |attempt| {
            if attempt.previous().len() >= 5
                || attempt.url().host_str() != Some(&expected_host)
                || attempt.url().port_or_known_default() != Some(expected_port)
                || initial_scheme == "https" && attempt.url().scheme() != "https"
            {
                attempt.error("Notification redirect blocked")
            } else {
                attempt.follow()
            }
        }))
        .build()
        .map_err(|_| failure("Notification transport initialization failed", false))?;
    let method = reqwest::Method::from_bytes(method.as_bytes())
        .map_err(|_| invalid("Notification HTTP method invalid"))?;
    let mut request = client.request(method.clone(), url);
    if method != reqwest::Method::GET {
        request = request.json(payload);
    }
    if let Some(o) = headers.as_object() {
        for (k, v) in o {
            let value = v
                .as_str()
                .ok_or_else(|| invalid("Notification header must be a string"))?;
            let name = reqwest::header::HeaderName::from_bytes(k.as_bytes())
                .map_err(|_| invalid("Notification header name invalid"))?;
            let value = reqwest::header::HeaderValue::from_str(value)
                .map_err(|_| invalid("Notification header value invalid"))?;
            request = request.header(name, value);
        }
    }
    let response = request.send().map_err(|_| {
        failure(
            "Notification transport failed; delivery may be unconfirmed",
            false,
        )
    })?;
    let status = response.status().as_u16();
    let mut bytes = Vec::new();
    response.take(8193).read_to_end(&mut bytes).map_err(|_| {
        failure(
            "Notification response read failed; delivery may be unconfirmed",
            false,
        )
    })?;
    if !(200..300).contains(&status) {
        return Err(failure(
            &format!("Notification HTTP status {status}"),
            status >= 500 || status == 429 || status == 408,
        ));
    }
    if bytes.len() > 8192 {
        return Err(failure("Notification response exceeds 8 KiB", false));
    }
    Ok(serde_json::from_slice(&bytes).unwrap_or(Value::Null))
}
fn sign(key: &str, data: &str) -> String {
    let mut m =
        Hmac::<Sha256>::new_from_slice(key.as_bytes()).expect("HMAC accepts every key size");
    m.update(data.as_bytes());
    STANDARD.encode(m.finalize().into_bytes())
}
fn truncate(s: &str, max: usize) -> String {
    let mut n = s.len().min(max);
    while !s.is_char_boundary(n) {
        n -= 1;
    }
    s[..n].into()
}
fn html(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}
fn template_value(v: &Value) -> gtmpl::Value {
    match v {
        Value::Null => gtmpl::Value::Nil,
        Value::Bool(v) => (*v).into(),
        Value::String(v) => v.clone().into(),
        Value::Number(v) => {
            if let Some(n) = v.as_i64() {
                n.into()
            } else if let Some(n) = v.as_u64() {
                n.into()
            } else {
                v.as_f64().unwrap_or(0.0).into()
            }
        }
        Value::Array(a) => gtmpl::Value::Array(a.iter().map(template_value).collect()),
        Value::Object(o) => gtmpl::Value::Object(
            o.iter()
                .map(|(k, v)| (k.clone(), template_value(v)))
                .collect(),
        ),
    }
}
fn from_template(v: &gtmpl::Value) -> std::result::Result<Value, gtmpl::FuncError> {
    Ok(match v {
        gtmpl::Value::Nil => Value::Null,
        gtmpl::Value::Bool(v) => json!(v),
        gtmpl::Value::String(v) => json!(v),
        gtmpl::Value::Number(n) => {
            if let Some(n) = n.as_i64() {
                json!(n)
            } else if let Some(n) = n.as_u64() {
                json!(n)
            } else {
                json!(n.as_f64())
            }
        }
        gtmpl::Value::Array(a) => json!(
            a.iter()
                .map(from_template)
                .collect::<std::result::Result<Vec<_>, _>>()?
        ),
        gtmpl::Value::Object(o) | gtmpl::Value::Map(o) => json!(
            o.iter()
                .map(|(k, v)| Ok((k.clone(), from_template(v)?)))
                .collect::<std::result::Result<HashMap<_, _>, gtmpl::FuncError>>()?
        ),
        _ => {
            return Err(gtmpl::FuncError::Generic(
                "Unsupported template value".into(),
            ));
        }
    })
}
fn json_func(a: &[gtmpl::Value]) -> std::result::Result<gtmpl::Value, gtmpl::FuncError> {
    if a.len() != 1 {
        return Err(gtmpl::FuncError::ExactlyXArgs("json".into(), 1));
    }
    Ok(from_template(&a[0])?.to_string().into())
}
fn jsons_func(a: &[gtmpl::Value]) -> std::result::Result<gtmpl::Value, gtmpl::FuncError> {
    let inner = json_func(a)?;
    let quoted = json!(inner.to_string()).to_string();
    Ok(quoted[1..quoted.len() - 1].to_owned().into())
}
struct Bounded(Vec<u8>);
impl Write for Bounded {
    fn write(&mut self, b: &[u8]) -> std::io::Result<usize> {
        if self.0.len() + b.len() > 1024 * 1024 {
            return Err(std::io::Error::other("Template size limit"));
        }
        self.0.extend_from_slice(b);
        Ok(b.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}
fn webhook_body(template: &str, message: &Value) -> Result<Value> {
    let items = message["items"].as_array().cloned().unwrap_or_default();
    if template.trim().is_empty() {
        return Ok(
            json!({"title":message["title"],"batch":message["batch"]==true,"count":items.len(),"items":items}),
        );
    }
    if template.len() > 65536
        || template.matches("range").count() > 3
        || ["define", "template", "block"].iter().any(|token| {
            template
                .split("{{")
                .skip(1)
                .any(|s| s.trim_start_matches('-').trim_start().starts_with(token))
        })
    {
        return Err(invalid("Notification template exceeds execution boundary"));
    }
    let items=items.iter().map(|item|json!({"FindingID":item["finding_id"],"Name":item["name"],"VulnClass":item["vulnclass"],"Severity":item["severity"],"SeverityLabel":item["severity_label"],"Summary":item["summary"],"Assets":item["assets"],"DetailURL":item["detail_url"],"FromStatus":item["from_status"],"ToStatus":item["to_status"],"StatusLabel":item["status_label"]})).collect::<Vec<_>>();
    let data = json!({"Title":message["title"],"Batch":message["batch"]==true,"Count":items.len(),"Items":items,"HomeURL":message["home_url"],"SentAt":artex_core::now()});
    let mut t = gtmpl::Template::default();
    t.add_func("json", json_func);
    t.add_func("jsons", jsons_func);
    t.parse(template)
        .map_err(|_| invalid("Notification template syntax invalid"))?;
    let mut result = Bounded(Vec::new());
    t.execute(&mut result, &gtmpl::Context::from(template_value(&data)))
        .map_err(|_| invalid("Notification template execution failed"))?;
    serde_json::from_slice(&result.0)
        .map_err(|_| invalid("Notification template must render valid JSON"))
}
fn smtp(cfg: &Value, message: &Value, allow_local: bool) -> Result<()> {
    use lettre::{
        Message, SmtpTransport, Transport,
        message::{Mailbox, header::ContentType},
        transport::smtp::{
            authentication::Credentials,
            client::{Tls, TlsParameters},
        },
    };
    let host = text(cfg, "host");
    let port = cfg["port"]
        .as_u64()
        .or_else(|| cfg["port"].as_str().and_then(|s| s.parse().ok()))
        .filter(|p| *p > 0 && *p <= 65535)
        .ok_or_else(|| invalid("SMTP port must be 1..65535"))? as u16;
    let addresses = resolved(host, port, allow_local)?;
    let from: Mailbox = text(cfg, "from")
        .parse()
        .map_err(|_| invalid("SMTP sender address invalid"))?;
    let mut builder = Message::builder()
        .from(from)
        .subject(text(message, "title"));
    let recipients = cfg["to"]
        .as_array()
        .map(|a| {
            a.iter()
                .filter_map(Value::as_str)
                .map(str::to_owned)
                .collect::<Vec<_>>()
        })
        .unwrap_or_else(|| {
            text(cfg, "to")
                .split(',')
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .map(str::to_owned)
                .collect()
        });
    if recipients.is_empty() || recipients.len() > 100 {
        return Err(invalid("SMTP requires 1..100 recipients"));
    }
    for recipient in recipients {
        builder = builder.to(recipient
            .parse()
            .map_err(|_| invalid("SMTP recipient address invalid"))?);
    }
    let body = if text(message, "html").is_empty() {
        format!(
            "<h2>{}</h2><pre>{}</pre>",
            html(text(message, "title")),
            html(text(message, "text"))
        )
    } else {
        text(message, "html").to_owned()
    };
    let email = builder
        .header(ContentType::TEXT_HTML)
        .body(body)
        .map_err(|_| invalid("SMTP message invalid"))?;
    let tls = TlsParameters::new(host.to_owned()).map_err(|_| invalid("SMTP TLS host invalid"))?;
    let mode = if cfg["tls"] == true {
        Tls::Wrapper(tls)
    } else if !text(cfg, "username").is_empty() {
        Tls::Required(tls)
    } else {
        Tls::Opportunistic(tls)
    };
    // Pin the checked address while retaining the configured domain for certificate/SNI.
    let mut transport = SmtpTransport::builder_dangerous(addresses[0].ip().to_string())
        .port(port)
        .timeout(Some(Duration::from_secs(45)))
        .tls(mode);
    if !text(cfg, "username").is_empty() {
        transport = transport.credentials(Credentials::new(
            text(cfg, "username").into(),
            text(cfg, "password").into(),
        ));
    }
    transport
        .build()
        .send(&email)
        .map_err(|e| failure("SMTP delivery failed", e.is_transient()))?;
    Ok(())
}
fn pack_message(kind: &str, message: &Value) -> (Value, usize) {
    let Some(items) = message["items"].as_array().filter(|a| !a.is_empty()) else {
        return (message.clone(), 0);
    };
    let limit = match kind {
        "wecom" => 3900,
        "telegram" => 3000,
        "feishu" => 19000,
        "dingtalk" => 19000,
        _ => usize::MAX,
    };
    if limit == usize::MAX {
        return (message.clone(), items.len());
    }
    let mut body = String::new();
    let mut kept = 0;
    for item in items {
        let line = format!(
            "{} [{}]\n{}\n{}\n\n",
            text(item, "name"),
            text(item, "severity"),
            text(item, "summary"),
            text(item, "detail_url")
        );
        if kept > 0 && body.len().saturating_add(line.len()) > limit {
            break;
        }
        body.push_str(&truncate(&line, limit.saturating_sub(body.len())));
        kept += 1;
        if body.len() >= limit {
            break;
        }
    }
    let mut packed = message.clone();
    packed["items"] = json!(&items[..kept]);
    packed["text"] = json!(body);
    packed["markdown"] = packed["text"].clone();
    (packed, kept)
}
pub fn send(parameters: &Value, vault: &SecretVault) -> Result<Value> {
    let mut config = parameters["config"].clone();
    resolve(&mut config, vault, 0, false)?;
    // Escape hatch has exactly the source application's explicit environment semantics.
    let allow_local = std::env::var("ARTEX_NOTIFY_ALLOW_LOCAL")
        .is_ok_and(|s| s == "1" || s.eq_ignore_ascii_case("true"));
    send_resolved(parameters, &config, allow_local)
}
fn send_resolved(parameters: &Value, cfg: &Value, allow_local: bool) -> Result<Value> {
    let start = Instant::now();
    let kind = text(parameters, "kind");
    let message = &parameters["message"];
    if serde_json::to_vec(message)?.len() > 1024 * 1024
        || message["items"].as_array().is_some_and(|a| a.len() > 100)
    {
        return Err(invalid("Notification message exceeds delivery boundary"));
    }
    let (packed, kept) = pack_message(kind, message);
    let message = &packed;
    if kind == "email" {
        smtp(cfg, message, allow_local)?;
        let mut result = json!({"ok":true,"latency_ms":start.elapsed().as_millis() as u64});
        if parameters["test"] != true {
            result["kept"] = json!(kept);
        }
        return Ok(result);
    }
    let title = text(message, "title");
    let markdown = if text(message, "markdown").is_empty() {
        text(message, "text")
    } else {
        text(message, "markdown")
    };
    let (mut url, mut payload, method, mut headers) = match kind {
        "webhook" => {
            let method = if text(cfg, "method").is_empty() {
                "POST"
            } else {
                text(cfg, "method")
            };
            if !["GET", "POST", "PUT", "PATCH"].contains(&method) {
                return Err(invalid("Webhook method unsupported"));
            }
            (
                endpoint(text(cfg, "url"))?,
                webhook_body(text(cfg, "body_template"), message)?,
                method.to_owned(),
                cfg["headers"].clone(),
            )
        }
        "dingtalk" => (
            endpoint(text(cfg, "webhook"))?,
            json!({"msgtype":"markdown","markdown":{"title":title,"text":truncate(markdown,20000)}}),
            "POST".into(),
            json!({}),
        ),
        "feishu" => (
            endpoint(text(cfg, "webhook"))?,
            json!({"msg_type":"interactive","card":{"config":{"wide_screen_mode":true},"header":{"title":{"tag":"plain_text","content":title}},"elements":[{"tag":"div","text":{"tag":"lark_md","content":truncate(markdown,20000)}}]}}),
            "POST".into(),
            json!({}),
        ),
        "wecom" => (
            endpoint(text(cfg, "webhook"))?,
            json!({"msgtype":"markdown","markdown":{"content":truncate(markdown,4096)}}),
            "POST".into(),
            json!({}),
        ),
        "telegram" => {
            let base = if text(cfg, "base_url").is_empty() {
                "https://api.telegram.org"
            } else {
                text(cfg, "base_url")
            };
            let token = text(cfg, "bot_token");
            if token.is_empty()
                || token
                    .chars()
                    .any(|c| c == '/' || c == '?' || c == '#' || c.is_whitespace())
                || text(cfg, "chat_id").is_empty()
            {
                return Err(invalid("Telegram token or chat ID invalid"));
            }
            (
                endpoint(&format!(
                    "{}/bot{token}/sendMessage",
                    base.trim_end_matches('/')
                ))?,
                json!({"chat_id":cfg["chat_id"],"text":html(&text(message,"text").chars().take(3500).collect::<String>()),"parse_mode":"HTML","disable_web_page_preview":false}),
                "POST".into(),
                json!({}),
            )
        }
        _ => return Err(invalid("Unknown notification channel kind")),
    };
    let secret = text(cfg, "secret");
    let millis = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|_| invalid("Notification clock before epoch"))?
        .as_millis();
    if kind == "dingtalk" && !secret.is_empty() {
        let timestamp = millis.to_string();
        let signature = sign(secret, &format!("{timestamp}\n{secret}"));
        url.query_pairs_mut()
            .append_pair("timestamp", &timestamp)
            .append_pair("sign", &signature);
    }
    if kind == "feishu" && !secret.is_empty() {
        let timestamp = (millis / 1000).to_string();
        payload["timestamp"] = json!(timestamp);
        payload["sign"] = json!(sign(&format!("{timestamp}\n{secret}"), ""));
    }
    if kind == "webhook" && !text(cfg, "content_type").is_empty() {
        if !headers.is_object() {
            headers = json!({});
        }
        headers["Content-Type"] = cfg["content_type"].clone();
    }
    let response = post(url, &method, &headers, &payload, allow_local)?;
    let business_error = match kind {
        "dingtalk" | "wecom" => response["errcode"].as_i64().filter(|c| *c != 0),
        "feishu" => response["code"]
            .as_i64()
            .filter(|c| *c != 0)
            .or(response["StatusCode"].as_i64().filter(|c| *c != 0)),
        "telegram" => {
            if response["ok"] == true {
                None
            } else {
                Some(response["error_code"].as_i64().unwrap_or(-1))
            }
        }
        _ => None,
    };
    if kind != "webhook" && response.is_null() {
        return Err(failure("Notification provider response is invalid", false));
    }
    if let Some(code) = business_error {
        return Err(failure(
            &format!("Notification provider rejected message ({code})"),
            code == 429 || kind == "wecom" && code == 45009,
        ));
    }
    let mut result = json!({"ok":true,"latency_ms":start.elapsed().as_millis() as u64});
    if parameters["test"] != true {
        result["kept"] = json!(kept);
    }
    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{io::BufRead, net::TcpListener};
    fn receiver(response: &str) -> (String, std::thread::JoinHandle<String>) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let response = response.to_owned();
        let worker = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            stream
                .set_read_timeout(Some(Duration::from_secs(5)))
                .unwrap();
            let mut reader = std::io::BufReader::new(stream.try_clone().unwrap());
            let mut request = String::new();
            let mut size = 0usize;
            loop {
                let mut line = String::new();
                reader.read_line(&mut line).unwrap();
                request.push_str(&line);
                if line.to_lowercase().starts_with("content-length:") {
                    size = line.split(':').nth(1).unwrap().trim().parse().unwrap();
                }
                if line == "\r\n" {
                    break;
                }
            }
            let mut body = vec![0; size];
            reader.read_exact(&mut body).unwrap();
            request.push_str(&String::from_utf8(body).unwrap());
            write!(
                stream,
                "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                response.len(),
                response
            )
            .unwrap();
            request
        });
        (format!("http://{address}"), worker)
    }
    #[test]
    fn packing_preserves_finding_boundaries_and_reports_the_unsent_tail() {
        let msg = json!({"title":"digest","items":[{"name":"first","summary":"가".repeat(850)},{"name":"second","summary":"나".repeat(850)},{"name":"third","summary":"tail"}]});
        let (packed, kept) = pack_message("telegram", &msg);
        assert_eq!(kept, 1);
        assert_eq!(packed["items"].as_array().unwrap().len(), 1);
        assert!(text(&packed, "text").contains("first"));
        assert!(!text(&packed, "text").contains("second"));
        assert!(text(&packed, "text").len() <= 3000);
        assert_eq!(pack_message("webhook", &msg).1, 3);
    }
    #[test]
    fn numeric_password_secret_stays_a_string_and_headers_are_explicit_json() {
        let vault = SecretVault::default();
        let key = uuid::Uuid::new_v4().to_string();
        let password = vault.set(&key, "password", "12345").unwrap();
        let headers = vault
            .set(&key, "headers", r#"{"Authorization":"Bearer fixture"}"#)
            .unwrap();
        let mut config = json!({"password":password,"headers":headers});
        let result = resolve(&mut config, &vault, 0, false);
        vault
            .delete(password["secret_ref"].as_str().unwrap())
            .unwrap();
        vault
            .delete(headers["secret_ref"].as_str().unwrap())
            .unwrap();
        result.unwrap();
        assert_eq!(config["password"], json!("12345"));
        assert_eq!(config["headers"]["Authorization"], "Bearer fixture");
    }
    #[test]
    fn provider_signatures_payloads_and_business_errors() {
        for kind in ["dingtalk", "feishu", "wecom", "telegram", "webhook"] {
            let (url, worker) = receiver(if kind == "telegram" {
                "{\"ok\":true}"
            } else {
                "{\"errcode\":0,\"code\":0}"
            });
            let cfg = json!({"url":url,"webhook":url,"base_url":url,"bot_token":"fixture-token","secret":"fixture-signing-secret","chat_id":"fixture","headers":{"Authorization":"fixture-header"}});
            let p = json!({"kind":kind,"message":{"title":"한국어","text":"test","markdown":"proof","items":[]}});
            assert_eq!(send_resolved(&p, &cfg, true).unwrap()["ok"], true);
            let request = worker.join().unwrap();
            assert!(request.contains("POST"));
            if kind == "dingtalk" {
                assert!(request.contains("timestamp=") && request.contains("sign="));
            }
            if kind == "feishu" {
                assert!(request.contains("\"sign\"") && request.contains("interactive"));
            }
            if kind == "webhook" {
                assert!(
                    request
                        .to_lowercase()
                        .contains("authorization: fixture-header")
                );
            }
            if kind == "telegram" {
                assert!(request.contains("/botfixture-token/sendMessage"));
            }
        }
        let (url, worker) = receiver("{\"errcode\":93000,\"errmsg\":\"credential-secret\"}");
        let e = send_resolved(
            &json!({"kind":"wecom","message":{"text":"x"}}),
            &json!({"webhook":url}),
            true,
        )
        .unwrap_err();
        assert!(!e.retryable);
        assert!(!e.message.contains("credential-secret"));
        worker.join().unwrap();
        assert_eq!(
            sign("secret", "1700000000000\nsecret"),
            "OuzzJR5+xZ4/EYwqtNt6sMYZQMTa/HEGvc9miJe7XzY="
        );
        assert_eq!(
            sign("1700000000\nsecret", ""),
            "fiWS2+gh28DOydAv7hzONH/mDn9+b1Y4Y5ivXWXy8vA="
        );
    }
    #[test]
    fn address_guard_and_response_redaction() {
        for ip in [
            "127.0.0.1",
            "0.0.0.0",
            "169.254.169.254",
            "224.0.0.1",
            "::1",
            "::ffff:127.0.0.1",
            "fe80::1",
        ] {
            assert!(blocked(ip.parse().unwrap()));
        }
        assert!(!blocked("10.0.0.1".parse().unwrap()));
        assert!(endpoint("file:///private").is_err());
        assert!(endpoint("https://user:secret@example.com").is_err());
        let e = post(
            endpoint("http://127.0.0.1:1/path?token=private-secret").unwrap(),
            "POST",
            &json!({}),
            &json!({}),
            false,
        )
        .unwrap_err();
        assert!(!e.message.contains("private-secret"));
    }
    #[test]
    fn template_pure_data_json_range_printf_and_errors() {
        let msg = json!({"title":"quote\"\n","items":[{"name":"SQL inject","severity":"high","finding_id":"finding_fixture"}],"home_url":""});
        assert_eq!(webhook_body("{\"title\":{{json .Title}},\"items\":[{{range $i,$it := .Items}}{{if $i}},{{end}}{{json $it.Name}}{{end}}]}",&msg).unwrap()["title"],msg["title"]);
        assert_eq!(
            webhook_body("{\"title\":{{json (printf \"%d items\" .Count)}}}", &msg).unwrap()["title"],
            "1 items"
        );
        let error = webhook_body("{\"env\":{{.Environment}}}", &msg).unwrap_err();
        assert!(!error.message.contains("SQL inject"));
        assert!(webhook_body("{{ readFile \"secret\" }}", &msg).is_err());
        assert!(webhook_body("{{define \"x\"}}{{template \"x\"}}{{end}}", &msg).is_err());
    }
    #[test]
    fn smtp_local_fixture_delivers_rfc_message() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let worker = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            stream
                .set_read_timeout(Some(Duration::from_secs(10)))
                .unwrap();
            stream.write_all(b"220 fixture ESMTP\r\n").unwrap();
            let mut reader = std::io::BufReader::new(stream.try_clone().unwrap());
            let mut body = String::new();
            let mut data = false;
            loop {
                let mut line = String::new();
                if reader.read_line(&mut line).unwrap() == 0 {
                    break;
                }
                if data {
                    if line == ".\r\n" {
                        data = false;
                        stream.write_all(b"250 delivered\r\n").unwrap();
                    } else {
                        body.push_str(&line);
                    }
                    continue;
                }
                if line.starts_with("EHLO") {
                    stream
                        .write_all(b"250-fixture\r\n250 8BITMIME\r\n")
                        .unwrap();
                } else if line.starts_with("DATA") {
                    data = true;
                    stream.write_all(b"354 proceed\r\n").unwrap();
                } else if line.starts_with("QUIT") {
                    stream.write_all(b"221 bye\r\n").unwrap();
                    break;
                } else {
                    stream.write_all(b"250 okay\r\n").unwrap();
                }
            }
            body
        });
        smtp(&json!({"host":"127.0.0.1","port":addr.port(),"from":"sender@example.test","to":["receiver@example.test"]}),&json!({"title":"한국어 test","text":"evidence"}),true).unwrap();
        let body = worker.join().unwrap();
        assert!(
            body.contains("Subject:") && body.contains("text/html") && body.contains("evidence")
        );
    }
}
