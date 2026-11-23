//! Agent network actions; callers must obtain run policy approval before dispatch.
use crate::HostRuntime;
use artex_core::{Context, Error, Result};
use futures_util::StreamExt;
use reqwest::{
    Url,
    blocking::{Client, Response},
};
use scraper::{Html, Selector};
use serde_json::{Value, json};
use std::{io::Read, path::Path, time::Duration};
const MAX_BYTES: u64 = 2 * 1024 * 1024;
fn invalid(message: &str) -> Error {
    Error::new("INVALID_PARAMS", message)
}
pub fn client(config: &Value) -> Result<Client> {
    let timeout = config["timeout_ms"]
        .as_u64()
        .unwrap_or(30000)
        .clamp(1, 120000);
    let mut builder = Client::builder()
        .no_proxy()
        .timeout(Duration::from_millis(timeout))
        .connect_timeout(Duration::from_secs(10))
        .redirect(reqwest::redirect::Policy::limited(10))
        .user_agent("ARTEX-Desktop/0.1");
    if let Some(proxy) = config["proxy"].as_str().filter(|s| !s.is_empty()) {
        builder = builder
            .proxy(reqwest::Proxy::all(proxy).map_err(|_| invalid("Invalid network proxy"))?);
    }
    if config["insecure_tls"].as_bool().unwrap_or(false) {
        builder = builder.danger_accept_invalid_certs(true);
    }
    if let Some(pem) = config["ca_pem"].as_str() {
        builder = builder.add_root_certificate(
            reqwest::Certificate::from_pem(pem.as_bytes())
                .map_err(|_| invalid("Invalid CA certificate"))?,
        );
    }
    builder
        .build()
        .map_err(|_| Error::new("EXTERNAL_FAILURE", "HTTP client unavailable"))
}
pub fn response_bytes(response: Response) -> Result<(String, String, Vec<u8>)> {
    if !response.status().is_success() {
        return Err(Error::new(
            "EXTERNAL_FAILURE",
            &format!("Remote HTTP status {}", response.status().as_u16()),
        ));
    }
    response_bytes_any(response)
}
pub fn response_bytes_any(mut response: Response) -> Result<(String, String, Vec<u8>)> {
    let final_url = response.url().to_string();
    let content_type = response
        .headers()
        .get(reqwest::header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("text/plain")
        .to_string();
    if response.content_length().is_some_and(|n| n > MAX_BYTES) {
        return Err(Error::new("SIZE_LIMIT", "Remote content exceeds 2 MiB"));
    }
    let mut bytes = vec![];
    response
        .by_ref()
        .take(MAX_BYTES + 1)
        .read_to_end(&mut bytes)
        .map_err(Error::io)?;
    if bytes.len() as u64 > MAX_BYTES {
        return Err(Error::new("SIZE_LIMIT", "Remote content exceeds 2 MiB"));
    }
    Ok((final_url, content_type, bytes))
}
fn page_url(raw: &str) -> Result<Url> {
    let url = Url::parse(raw).map_err(|_| invalid("Absolute HTTP(S) URL required"))?;
    if !matches!(url.scheme(), "http" | "https")
        || !url.username().is_empty()
        || url.password().is_some()
    {
        return Err(invalid("HTTP(S) URL without credentials required"));
    }
    let ext = Path::new(url.path())
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();
    if [
        "js", "mjs", "cjs", "css", "map", "png", "jpg", "jpeg", "gif", "svg", "webp", "avif",
        "ico", "bmp", "woff", "woff2", "ttf", "otf", "eot", "wasm", "mp4", "webm", "mp3", "wav",
        "ogg",
    ]
    .contains(&ext.as_str())
    {
        return Err(invalid(
            "WebFetch rejects static assets; fetch the owning page",
        ));
    }
    Ok(url)
}
pub fn execute(host: &HostRuntime, name: &str, input: &Value, context: &Context) -> Result<Value> {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(Error::io)?;
    runtime.block_on(execute_async(host, name, input, context))
}
async fn execute_async(
    host: &HostRuntime,
    name: &str,
    input: &Value,
    context: &Context,
) -> Result<Value> {
    let config = &input["network_config"];
    let http = async_client(config)?;
    if name == "WebFetch" {
        let url = page_url(
            input["url"]
                .as_str()
                .ok_or_else(|| invalid("URL required"))?,
        )?;
        // HTTP error pages are reconnaissance evidence, not transport failures.
        // Preserve norma's status and inexpensive technology fingerprints too.
        let response = owned(host, context, http.get(url).send())
            .await?
            .map_err(network_error)?;
        let status = response.status().as_u16();
        let header = |name: &str| {
            response
                .headers()
                .get(name)
                .and_then(|v| v.to_str().ok())
                .map(str::to_owned)
        };
        let server = header("server");
        let x_powered_by = header("x-powered-by");
        let (url, media_type, bytes) = read_response_async(host, context, response).await?;
        page_url(&url)?;
        // norma webFetchRunner compares MIME types without case sensitivity;
        // preserve the reported header while normalizing only its dispatch key.
        let normalized_media = media_type.to_ascii_lowercase();
        if normalized_media.starts_with("image/")
            || normalized_media.starts_with("audio/")
            || normalized_media.starts_with("video/")
            || normalized_media.contains("javascript")
            || normalized_media.contains("text/css")
        {
            return Err(invalid("WebFetch rejects static asset media types"));
        }
        let text = String::from_utf8(bytes).map_err(|_| invalid("Web response is not UTF-8"))?;
        let content = if normalized_media.contains("html") {
            render_html(&text, input["extract"].as_bool().unwrap_or(false))
        } else {
            text
        };
        return Ok(
            json!({"url":url,"status":status,"server":server,"x_powered_by":x_powered_by,"media_type":media_type,"content":content}),
        );
    }
    let query = input["query"]
        .as_str()
        .filter(|q| !q.trim().is_empty())
        .ok_or_else(|| invalid("Search query required"))?;
    let limit = input["limit"].as_u64().unwrap_or(5).clamp(1, 20) as usize;
    let backend = config["backend"].as_str().unwrap_or("ddgs");
    let hits = match backend {
        "ddgs" => {
            let endpoint = config["endpoint"]
                .as_str()
                .unwrap_or("https://html.duckduckgo.com/html/");
            let (_, _, bytes) = send_bytes_async(
                host,
                context,
                http.get(endpoint).query(&[("q", query)]),
                true,
            )
            .await?;
            ddgs_hits(&String::from_utf8_lossy(&bytes), limit)
        }
        "brave-free" => {
            let key = config["api_key"]
                .as_str()
                .ok_or_else(|| Error::new("FEATURE_DISABLED", "Brave search secret required"))?;
            let endpoint = config["endpoint"]
                .as_str()
                .unwrap_or("https://api.search.brave.com/res/v1/web/search");
            let (_, _, bytes) = send_bytes_async(
                host,
                context,
                http.get(endpoint)
                    .header("X-Subscription-Token", key)
                    .query(&[("q", query), ("count", &limit.to_string())]),
                true,
            )
            .await?;
            let v: Value = serde_json::from_slice(&bytes)?;
            normalize_hits(&v["web"]["results"], limit)
        }
        "tavily" => {
            let key = config["api_key"]
                .as_str()
                .ok_or_else(|| Error::new("FEATURE_DISABLED", "Tavily search secret required"))?;
            let endpoint = config["endpoint"]
                .as_str()
                .unwrap_or("https://api.tavily.com/search");
            let (_,_,bytes)=send_bytes_async(host,context,http.post(endpoint).json(&json!({"api_key":key,"query":query,"max_results":limit,"include_answer":false,"include_raw_content":false})),true).await?;
            let v: Value = serde_json::from_slice(&bytes)?;
            normalize_hits(&v["results"], limit)
        }
        "deepseek" => deepseek_hits(host, context, config, query, limit).await?,
        _ => return Err(invalid("Unknown search backend")),
    };
    Ok(json!({"backend":backend,"results":hits}))
}
fn normalize_hits(hits: &Value, limit: usize) -> Vec<Value> {
    hits.as_array().map(|h|h.iter().take(limit).enumerate().map(|(i,h)|json!({"title":h["title"],"url":h["url"],"description":h.get("description").or_else(||h.get("content")).unwrap_or(&Value::Null),"position":i+1})).collect()).unwrap_or_default()
}
async fn deepseek_hits(
    host: &HostRuntime,
    context: &Context,
    config: &Value,
    query: &str,
    limit: usize,
) -> Result<Vec<Value>> {
    let key = config["api_key"]
        .as_str()
        .or_else(|| config["deepseek_api_key"].as_str())
        .ok_or_else(|| Error::new("FEATURE_DISABLED", "DeepSeek search secret required"))?;
    let base = config["base_url"]
        .as_str()
        .or_else(|| config["deepseek_base_url"].as_str())
        .ok_or_else(|| invalid("DeepSeek Anthropic base URL required"))?
        .trim_end_matches('/');
    let endpoint = if base.ends_with("/v1/messages") {
        base.to_owned()
    } else if base.ends_with("/v1") {
        format!("{base}/messages")
    } else {
        format!("{base}/v1/messages")
    };
    let model = config["model"]
        .as_str()
        .or_else(|| config["deepseek_model"].as_str())
        .ok_or_else(|| invalid("DeepSeek search model required"))?;
    let mut settings = config.clone();
    settings["timeout_ms"] = json!(120000);
    let request=async_client(&settings)?.post(&endpoint).header("x-api-key",key).header("anthropic-version","2023-06-01").header("Accept","text/event-stream").json(&json!({"model":model,"max_tokens":1024,"stream":true,"messages":[{"role":"user","content":format!("Search the web for: {query}\n\nUse the web_search tool. Do not answer from memory; reply with at most one short sentence once the search is done.")}],"tools":[{"type":"web_search_20250305","name":"web_search","max_uses":limit.div_ceil(10).clamp(1,3)}]}));
    let response = owned(host, context, request.send())
        .await?
        .map_err(network_error)?;
    if !response.status().is_success() {
        return Err(Error::new(
            "EXTERNAL_FAILURE",
            "DeepSeek search HTTP status failed",
        ));
    }
    let mut stream = response.bytes_stream();
    let mut bytes = vec![];
    while let Some(chunk) = owned(host, context, stream.next()).await? {
        bytes.extend(chunk.map_err(network_error)?);
        if bytes.len() as u64 > MAX_BYTES {
            return Err(Error::new("SIZE_LIMIT", "Search stream exceeded 2 MiB"));
        }
        let hits = parse_deepseek(&bytes, limit)?;
        if !hits.is_empty() {
            return Ok(hits);
        }
    }
    parse_deepseek(&bytes, limit)
}
fn parse_deepseek(bytes: &[u8], limit: usize) -> Result<Vec<Value>> {
    let mut hits = vec![];
    let mut seen = std::collections::HashSet::new();
    let mut tool_error = None;
    for line in String::from_utf8_lossy(&bytes).lines() {
        let Some(data) = line.trim().strip_prefix("data:") else {
            continue;
        };
        let Ok(frame) = serde_json::from_str::<Value>(data.trim()) else {
            continue;
        };
        if frame["type"] == "error" {
            return Err(Error::new(
                "EXTERNAL_FAILURE",
                "DeepSeek search stream error",
            ));
        }
        if frame["type"] != "content_block_start"
            || frame["content_block"]["type"] != "web_search_tool_result"
        {
            continue;
        }
        if let Some(list) = frame["content_block"]["content"].as_array() {
            for hit in list {
                if let Some(error) = hit["error_code"].as_str() {
                    tool_error = Some(error.to_owned());
                    continue;
                }
                let Some(url) = hit["url"].as_str().filter(|s| !s.is_empty()) else {
                    continue;
                };
                if !seen.insert(url.to_owned()) {
                    continue;
                }
                let title = hit["title"]
                    .as_str()
                    .filter(|s| !s.is_empty())
                    .map(str::to_owned)
                    .unwrap_or_else(|| {
                        Url::parse(url)
                            .ok()
                            .and_then(|u| u.host_str().map(str::to_owned))
                            .unwrap_or_else(|| url.into())
                    });
                hits.push(
                    json!({"title":title,"url":url,"description":"","position":hits.len()+1}),
                );
                if hits.len() >= limit {
                    return Ok(hits);
                }
            }
        }
    }
    if hits.is_empty() && tool_error.is_some() {
        return Err(Error::new(
            "EXTERNAL_FAILURE",
            "DeepSeek web search tool failed",
        ));
    }
    Ok(hits)
}
fn ddgs_hits(text: &str, limit: usize) -> Vec<Value> {
    let doc = Html::parse_document(text);
    let rows = Selector::parse(".result").unwrap();
    let links = Selector::parse(".result__a").unwrap();
    let snippets = Selector::parse(".result__snippet").unwrap();
    doc.select(&rows).filter_map(|row|{let a=row.select(&links).next()?;let raw=a.value().attr("href")?;let parsed=if raw.starts_with("//"){Url::parse(&format!("https:{raw}"))}else{Url::parse(raw)}.ok()?;let url=parsed.query_pairs().find(|(k,_)|k=="uddg").map(|(_,v)|v.into_owned()).unwrap_or_else(||parsed.to_string());Some(json!({"title":a.text().collect::<Vec<_>>().join(" "),"url":url,"description":row.select(&snippets).next().map(|s|s.text().collect::<Vec<_>>().join(" ")).unwrap_or_default()}))}).take(limit).enumerate().map(|(i,mut h)|{h["position"]=json!(i+1);h}).collect()
}
// norma webFetchRunner truncates rendered page output at this byte budget.
const MAX_PAGE_TEXT_BYTES: usize = 50_000;
const PAGE_TRUNCATION_MARKER: &str = "\n... [truncated]";
#[derive(Default)]
struct PageText {
    text: String,
    truncated: bool,
}
impl PageText {
    fn push(&mut self, text: &str, preformatted: bool) {
        for mut character in text.chars() {
            // Match htmlToMarkdown's whitespace normalization before charging
            // the output budget. Empty layout containers carry no page content.
            // Preserve preformatted code indentation/newlines independently.
            if !preformatted {
                if self.text.is_empty() && character.is_whitespace() {
                    continue;
                }
                if matches!(character, ' ' | '\t' | '\r') {
                    character = ' ';
                }
                if character == '\n' {
                    while self.text.ends_with(' ') {
                        self.text.pop();
                    }
                    if self.text.is_empty() || self.text.ends_with('\n') {
                        continue;
                    }
                } else if character == ' '
                    && (self.text.is_empty() || self.text.ends_with([' ', '\n']))
                {
                    continue;
                }
            }
            if self.text.len() + character.len_utf8() > MAX_PAGE_TEXT_BYTES {
                self.truncated = true;
                break;
            }
            self.text.push(character);
        }
    }
    fn finish(mut self) -> String {
        if self.truncated {
            let mut end = self
                .text
                .len()
                .min(MAX_PAGE_TEXT_BYTES - PAGE_TRUNCATION_MARKER.len());
            while !self.text.is_char_boundary(end) {
                end -= 1;
            }
            self.text.truncate(end);
            self.text.push_str(PAGE_TRUNCATION_MARKER);
        } else {
            self.text.truncate(self.text.trim_end().len());
        }
        self.text
    }
}
fn page_text(out: &mut PageText, text: &str) {
    out.push(text, false);
}
fn render_html(text: &str, extract: bool) -> String {
    let doc = Html::parse_document(text);
    let mut out = PageText::default();
    // A node stack preserves the original visible-text/link traversal without
    // recursive Rust frames or repeated subtree copies for deeply nested pages.
    let mut stack = vec![(doc.tree.root(), false, false)];
    while let Some((node, closing, preformatted)) = stack.pop() {
        if out.truncated {
            break;
        }
        if let scraper::node::Node::Text(text) = node.value() {
            if !closing {
                if preformatted {
                    out.push(text, true);
                } else {
                    page_text(&mut out, &text.replace('\n', " "));
                }
            }
            continue;
        }
        let Some(element) = scraper::ElementRef::wrap(node) else {
            stack.extend(
                node.children()
                    .rev()
                    .map(|child| (child, false, preformatted)),
            );
            continue;
        };
        let tag = element.value().name();
        if matches!(tag, "script" | "style" | "noscript" | "svg" | "template") {
            continue;
        }
        let block = matches!(
            tag,
            "p" | "div"
                | "section"
                | "article"
                | "header"
                | "footer"
                | "main"
                | "ul"
                | "ol"
                | "table"
                | "tr"
                | "blockquote"
        );
        let heading = matches!(tag, "h1" | "h2" | "h3" | "h4" | "h5" | "h6");
        if closing {
            match tag {
                "pre" => out.push("\n```\n\n", true),
                "a" => {
                    if let Some(href) = element.value().attr("href") {
                        page_text(&mut out, &format!("]({href})"));
                    }
                }
                "strong" | "b" => page_text(&mut out, "**"),
                "em" | "i" => page_text(&mut out, "*"),
                "code" if !preformatted => page_text(&mut out, "`"),
                _ if block || heading || tag == "title" => page_text(&mut out, "\n\n"),
                _ => (),
            }
            continue;
        }
        match tag {
            "pre" => page_text(&mut out, "\n\n```\n"),
            "title" => page_text(&mut out, "\n\n# "),
            "li" => page_text(&mut out, "\n- "),
            "br" => page_text(&mut out, "\n"),
            "hr" => page_text(&mut out, "\n\n---\n\n"),
            "a" if element.value().attr("href").is_some() => page_text(&mut out, "["),
            "img" => {
                if let Some(src) = element.value().attr("src") {
                    page_text(
                        &mut out,
                        &format!("![{}]({src})", element.value().attr("alt").unwrap_or("")),
                    );
                }
            }
            "strong" | "b" => page_text(&mut out, "**"),
            "em" | "i" => page_text(&mut out, "*"),
            "code" if !preformatted => page_text(&mut out, "`"),
            _ if heading => page_text(
                &mut out,
                &format!("\n\n{} ", "#".repeat((tag.as_bytes()[1] - b'0') as usize)),
            ),
            _ if block => page_text(&mut out, "\n\n"),
            _ => (),
        }
        stack.push((node, true, preformatted));
        stack.extend(
            node.children()
                .rev()
                .map(|child| (child, false, preformatted || tag == "pre")),
        );
    }
    if extract && !out.truncated {
        render_recon(&doc, &mut out);
    }
    out.finish()
}
fn one_line(text: &str, max: usize) -> String {
    let text = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if text.len() <= max {
        return text;
    }
    let mut end = max;
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}…", &text[..end])
}
fn render_recon(doc: &Html, out: &mut PageText) {
    // Match norma extractRecon's compact, grouped signals. Serializing whole
    // scripts/forms can spend the entire page budget before an endpoint or CSRF
    // parameter is reached. Iterative traversal keeps deeply nested DOMs safe.
    static ENDPOINT: std::sync::OnceLock<regex::Regex> = std::sync::OnceLock::new();
    let endpoint = ENDPOINT.get_or_init(|| {
        regex::Regex::new(
            r#"["'`](https?://[^"'`\s]+|/[A-Za-z0-9._~$&'()*+,;=:@%/?#\[\]-]{2,})["'`]"#,
        )
        .unwrap()
    });
    let inputs = Selector::parse("input,textarea,select").unwrap();
    let mut comments = vec![];
    let mut scripts = vec![];
    let mut endpoints = vec![];
    let mut forms = vec![];
    let mut metas = vec![];
    let mut links = vec![];
    for node in doc.tree.root().descendants() {
        if let scraper::node::Node::Comment(comment) = node.value() {
            let comment = one_line(comment, 300);
            if !comment.is_empty() {
                comments.push(comment);
            }
        }
        let Some(element) = scraper::ElementRef::wrap(node) else {
            continue;
        };
        let attr = |name| element.value().attr(name).unwrap_or("");
        match element.value().name() {
            "script" if !attr("src").is_empty() => scripts.push(attr("src").to_owned()),
            "script" => {
                let body = element.text().collect::<String>();
                endpoints.extend(
                    endpoint
                        .captures_iter(&body)
                        .take(40)
                        .map(|m| m[1].to_owned()),
                );
            }
            "form" => {
                let fields = element
                    .select(&inputs)
                    .map(|input| {
                        let value = input.value();
                        let kind = value
                            .attr("type")
                            .filter(|s| !s.is_empty())
                            .unwrap_or(value.name());
                        let hidden = if kind == "hidden" {
                            value
                                .attr("value")
                                .filter(|v| !v.is_empty())
                                .map(|v| format!("={}", one_line(v, 60)))
                                .unwrap_or_default()
                        } else {
                            String::new()
                        };
                        format!("{}({kind}{hidden})", value.attr("name").unwrap_or(""))
                    })
                    .collect::<Vec<_>>()
                    .join(" ");
                let method = if attr("method").is_empty() {
                    "GET".into()
                } else {
                    attr("method").to_uppercase()
                };
                forms.push(format!("{method} {} → {fields}", attr("action")));
            }
            "meta" => {
                let name = if attr("name").is_empty() {
                    attr("property")
                } else {
                    attr("name")
                };
                if !name.is_empty() && !attr("content").is_empty() {
                    metas.push(format!("{name}={}", one_line(attr("content"), 120)));
                }
            }
            "a" | "link" if !attr("href").is_empty() => links.push(attr("href").to_owned()),
            _ => (),
        }
    }
    fn dedup(items: &mut Vec<String>) {
        let mut seen = std::collections::HashSet::new();
        items.retain(|item| seen.insert(item.clone()));
    }
    dedup(&mut scripts);
    dedup(&mut endpoints);
    dedup(&mut links);
    if links.len() > 100 {
        let remaining = links.len() - 100;
        links.truncate(100);
        links.push(format!("… ({remaining} more)"));
    }
    let sections = [
        ("COMMENTS", comments),
        ("SCRIPTS (src)", scripts),
        ("INLINE ENDPOINTS", endpoints),
        ("FORMS", forms),
        ("META", metas),
        ("LINKS", links),
    ];
    if sections.iter().all(|(_, items)| items.is_empty()) {
        return;
    }
    page_text(out, "\n\n=== RECON ===");
    for (title, items) in sections {
        if items.is_empty() {
            continue;
        }
        page_text(out, &format!("\n\n## {title}\n"));
        for item in items {
            if out.truncated {
                return;
            }
            page_text(out, &format!("- {item}\n"));
        }
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn page_rendering_uses_bounded_output_without_recursive_frames() {
        let deeply_nested = format!(
            "{}deep evidence{}",
            "<div>".repeat(4_000),
            "</div>".repeat(4_000)
        );
        assert!(render_html(&deeply_nested, false).contains("deep evidence"));
        let large = format!("<div>{}</div>", "한".repeat(MAX_PAGE_TEXT_BYTES));
        let rendered = render_html(&large, false);
        assert!(rendered.len() <= MAX_PAGE_TEXT_BYTES);
        assert!(rendered.contains('한'));
    }
    #[test]
    fn assets_and_search_normalization() {
        assert!(page_url("https://example.org/app.js?v=1").is_err());
        assert!(page_url("file:///x").is_err());
        let h = ddgs_hits(
            r#"<div class="result"><a class="result__a" href="//duckduckgo.com/l/?uddg=https%3A%2F%2Fexample.org">Example</a><span class="result__snippet">Actual snippet</span></div>"#,
            5,
        );
        assert_eq!(h[0]["url"], "https://example.org");
        assert!(
            render_html(
                "<h1>Title</h1><script src='/app.js'></script><!-- note -->",
                true
            )
            .contains("RECON")
        );
    }
}

// Dropping the in-flight future closes its owned connection rather than leaving
// a blocking HTTP thread alive after the user cancels the run.
pub fn async_client(config: &Value) -> Result<reqwest::Client> {
    let timeout = config["timeout_ms"]
        .as_u64()
        .unwrap_or(30000)
        .clamp(1, 120000);
    let mut builder = reqwest::Client::builder()
        .no_proxy()
        .timeout(Duration::from_millis(timeout))
        .connect_timeout(Duration::from_secs(10))
        .redirect(reqwest::redirect::Policy::limited(10))
        .user_agent("ARTEX-Desktop/0.1");
    if let Some(proxy) = config["proxy"].as_str().filter(|s| !s.is_empty()) {
        builder = builder
            .proxy(reqwest::Proxy::all(proxy).map_err(|_| invalid("Invalid network proxy"))?);
    }
    if config["insecure_tls"].as_bool().unwrap_or(false) {
        builder = builder.danger_accept_invalid_certs(true);
    }
    if let Some(pem) = config["ca_pem"].as_str() {
        builder = builder.add_root_certificate(
            reqwest::Certificate::from_pem(pem.as_bytes())
                .map_err(|_| invalid("Invalid CA certificate"))?,
        );
    }
    builder.build().map_err(network_error)
}
fn network_error(error: reqwest::Error) -> Error {
    Error::new(
        if error.is_timeout() {
            "TIMEOUT"
        } else {
            "EXTERNAL_FAILURE"
        },
        "Native HTTP operation failed",
    )
}
async fn owned<F: std::future::Future>(
    host: &HostRuntime,
    context: &Context,
    future: F,
) -> Result<F::Output> {
    tokio::pin!(future);
    let mut interval = tokio::time::interval(Duration::from_millis(20));
    loop {
        tokio::select! {value=&mut future=>{if host.context_cancelled(context){return Err(Error::new("CANCELLED","Native HTTP owner ended"));}return Ok(value)},_=interval.tick()=>{if host.context_cancelled(context){return Err(Error::new("CANCELLED","Native HTTP owner ended"));}}}
    }
}
pub fn send_bytes(
    host: &HostRuntime,
    context: &Context,
    request: reqwest::RequestBuilder,
    success: bool,
) -> Result<(String, String, Vec<u8>)> {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(Error::io)?
        .block_on(send_bytes_async(host, context, request, success))
}
async fn send_bytes_async(
    host: &HostRuntime,
    context: &Context,
    request: reqwest::RequestBuilder,
    success: bool,
) -> Result<(String, String, Vec<u8>)> {
    let response = owned(host, context, request.send())
        .await?
        .map_err(network_error)?;
    if success && !response.status().is_success() {
        return Err(Error::new("EXTERNAL_FAILURE", "Remote HTTP status failed"));
    }
    read_response_async(host, context, response).await
}
async fn read_response_async(
    host: &HostRuntime,
    context: &Context,
    response: reqwest::Response,
) -> Result<(String, String, Vec<u8>)> {
    let url = response.url().to_string();
    let media = response
        .headers()
        .get(reqwest::header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("text/plain")
        .to_string();
    if response.content_length().is_some_and(|n| n > MAX_BYTES) {
        return Err(Error::new("SIZE_LIMIT", "Remote content exceeds 2 MiB"));
    }
    let mut stream = response.bytes_stream();
    let mut bytes = vec![];
    while let Some(chunk) = owned(host, context, stream.next()).await? {
        bytes.extend(chunk.map_err(network_error)?);
        if bytes.len() as u64 > MAX_BYTES {
            return Err(Error::new("SIZE_LIMIT", "Remote content exceeds 2 MiB"));
        }
    }
    Ok((url, media, bytes))
}

pub fn send_custom(
    host: &HostRuntime,
    context: &Context,
    request: reqwest::RequestBuilder,
) -> Result<(u16, Vec<u8>)> {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(Error::io)?
        .block_on(async {
            let response = owned(host, context, request.send())
                .await?
                .map_err(network_error)?;
            let status = response.status().as_u16();
            let (_, _, bytes) = read_response_async(host, context, response).await?;
            Ok((status, bytes))
        })
}
