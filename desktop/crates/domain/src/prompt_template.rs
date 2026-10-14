//! Go text/template evaluation with the same catalog inputs as the source editor.
use crate::*;
use std::{
    collections::HashMap,
    io::{self, Write},
};
fn json_value(v: &Value) -> gtmpl::Value {
    match v {
        Value::Null => gtmpl::Value::Nil,
        Value::Bool(b) => (*b).into(),
        Value::Number(n) => {
            if let Some(i) = n.as_i64() {
                i.into()
            } else {
                n.as_f64().unwrap_or(0.0).into()
            }
        }
        Value::String(s) => s.clone().into(),
        Value::Array(a) => gtmpl::Value::Array(a.iter().map(json_value).collect()),
        Value::Object(o) => {
            gtmpl::Value::Object(o.iter().map(|(k, v)| (k.clone(), json_value(v))).collect())
        }
    }
}
struct Limited(Vec<u8>);
impl Write for Limited {
    fn write(&mut self, b: &[u8]) -> io::Result<usize> {
        if self.0.len().saturating_add(b.len()) > 4 * 1024 * 1024 {
            return Err(io::Error::other("Rendered prompt exceeds 4 MiB"));
        }
        self.0.extend_from_slice(b);
        Ok(b.len())
    }
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}
fn actions(text: &str) -> Result<Vec<String>> {
    let mut out = vec![];
    let mut tail = text;
    while let Some(start) = tail.find("{{") {
        tail = &tail[start + 2..];
        let mut quote = None;
        let mut escaped = false;
        let mut end = None;
        let bytes = tail.as_bytes();
        for (i, ch) in tail.char_indices() {
            if let Some(q) = quote {
                if escaped {
                    escaped = false;
                } else if ch == '\\' && q != '`' {
                    escaped = true;
                } else if ch == q {
                    quote = None;
                }
                continue;
            }
            if ['\"', '\'', '`'].contains(&ch) {
                quote = Some(ch);
                continue;
            }
            if ch == '}' && bytes.get(i + 1) == Some(&b'}') {
                end = Some(i);
                break;
            }
        }
        let end =
            end.ok_or_else(|| Error::new("INVALID_PARAMS", "Unterminated template action"))?;
        out.push(tail[..end].trim().trim_matches('-').trim().into());
        tail = &tail[end + 2..];
    }
    Ok(out)
}
fn check(template: &str, vars: &[Value]) -> Result<()> {
    if template.len() > 1024 * 1024 {
        return Err(Error::new("SIZE_LIMIT", "Prompt template exceeds 1 MiB"));
    }
    let fields = regex::Regex::new(r"(?:^|[\s(])\.([A-Za-z_][A-Za-z0-9_]*)").unwrap();
    let mut nesting = 0i32;
    let mut scope = String::new();
    let mut scopes: Vec<(String, i32)> = vec![];
    let mut calls = BTreeMap::<String, Vec<String>>::new();
    for action in actions(template)? {
        if action.starts_with("/*") {
            continue;
        }
        let mut scrubbed = String::new();
        let mut quote = None;
        let mut escaped = false;
        for ch in action.chars() {
            if let Some(q) = quote {
                scrubbed.push(' ');
                if escaped {
                    escaped = false;
                } else if ch == '\\' && q != '`' {
                    escaped = true;
                } else if ch == q {
                    quote = None;
                }
            } else if ['\"', '\'', '`'].contains(&ch) {
                quote = Some(ch);
                scrubbed.push(' ');
            } else {
                scrubbed.push(ch);
            }
        }
        for cap in fields.captures_iter(&scrubbed) {
            if !vars.iter().any(|v| v["name"] == cap[1]) {
                return Err(Error::new(
                    "INVALID_PARAMS",
                    &format!("Variable {} is outside the agent catalog", &cap[1]),
                ));
            }
        }
        let first = action.split_whitespace().next().unwrap_or("");
        if first == "template" || first == "block" {
            let target = quoted_name(&action)
                .ok_or_else(|| Error::new("INVALID_PARAMS", "Template name must be a constant"))?;
            calls.entry(scope.clone()).or_default().push(target);
        }
        if ["if", "with", "range", "define", "block"].contains(&first) {
            nesting += 1;
            if nesting > 64 {
                return Err(Error::new("SIZE_LIMIT", "Template nesting exceeds 64"));
            }
            if first == "define" || first == "block" {
                scopes.push((scope.clone(), nesting));
                scope = quoted_name(&action).ok_or_else(|| {
                    Error::new("INVALID_PARAMS", "Template name must be a string")
                })?;
            }
        } else if first == "end" {
            if scopes.last().is_some_and(|(_, level)| *level == nesting) {
                scope = scopes.pop().unwrap().0;
            }
            nesting -= 1;
            if nesting < 0 {
                return Err(Error::new("INVALID_PARAMS", "Unexpected template end"));
            }
        }
    }
    if nesting != 0 {
        return Err(Error::new("INVALID_PARAMS", "Unclosed template block"));
    }
    fn visit(
        name: &str,
        calls: &BTreeMap<String, Vec<String>>,
        stack: &mut BTreeSet<String>,
        depth: usize,
    ) -> Result<()> {
        if depth > 64 || !stack.insert(name.into()) {
            return Err(Error::new(
                "SIZE_LIMIT",
                "Recursive template calls are not supported",
            ));
        }
        for next in calls.get(name).into_iter().flatten() {
            visit(next, calls, stack, depth + 1)?;
        }
        stack.remove(name);
        Ok(())
    }
    for name in calls.keys() {
        visit(name, &calls, &mut BTreeSet::new(), 0)?;
    }
    Ok(())
}
use std::collections::BTreeMap;
fn quoted_name(action: &str) -> Option<String> {
    let start = action.find('"')?;
    let mut escaped = false;
    for (i, ch) in action[start + 1..].char_indices() {
        if escaped {
            escaped = false;
        } else if ch == '\\' {
            escaped = true;
        } else if ch == '"' {
            return serde_json::from_str(&action[start..start + i + 2]).ok();
        }
    }
    None
}
fn joined(args: &[gtmpl::Value]) -> std::result::Result<String, gtmpl::FuncError> {
    gtmpl::funcs::print(args).map(|v| v.to_string())
}
fn html(args: &[gtmpl::Value]) -> std::result::Result<gtmpl::Value, gtmpl::FuncError> {
    Ok(joined(args)?
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('\'', "&#39;")
        .replace('"', "&#34;")
        .into())
}
fn js(args: &[gtmpl::Value]) -> std::result::Result<gtmpl::Value, gtmpl::FuncError> {
    let input = joined(args)?;
    let mut out = String::new();
    for ch in input.chars() {
        match ch {
            '\\' => out.push_str("\\\\"),
            '\'' => out.push_str("\\'"),
            '"' => out.push_str("\\\""),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            '<' => out.push_str("\\u003C"),
            '>' => out.push_str("\\u003E"),
            '&' => out.push_str("\\u0026"),
            _ if ch.is_control() => out.push_str(&format!("\\u{:04X}", ch as u32)),
            _ => out.push(ch),
        }
    }
    Ok(out.into())
}
pub fn render(template: &str, vars: &[Value], sample: &Value) -> Result<String> {
    check(template, vars)?;
    let mut data = HashMap::new();
    for var in vars {
        let name = text(var, "name");
        data.insert(
            name.to_owned(),
            json_value(sample.get(name).unwrap_or(&var["example"])),
        );
    }
    let mut t = gtmpl::Template::with_name("p");
    t.add_func("html", html);
    t.add_func("js", js);
    t.parse(template)
        .map_err(|e| Error::new("INVALID_PARAMS", &e.to_string()))?;
    let mut output = Limited(vec![]);
    t.execute(
        &mut output,
        &gtmpl::Context::from(gtmpl::Value::Object(data)),
    )
    .map_err(|e| Error::new("INVALID_PARAMS", &e.to_string()))?;
    String::from_utf8(output.0)
        .map_err(|_| Error::new("INVALID_PARAMS", "Rendered prompt is not UTF-8"))
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn source_controls_pipelines_templates_and_catalog_validation() {
        let vars = vec![
            json!({"name":"Goal","example":"Verify"}),
            json!({"name":"Now","example":"Today"}),
        ];
        assert_eq!(render(r#"{{if .Goal}}{{with .Goal}}{{printf "%s" .}}{{end}}{{else}}empty{{end}}:{{define "footer"}}{{.Now}}{{end}}{{template "footer" .}}"#,&vars,&json!({})).unwrap(),"Verify:Today");
        assert_eq!(
            render(
                "{{range $k,$v := .}}{{$k}}={{$v}};{{end}}",
                &vars,
                &json!({})
            )
            .unwrap()
            .split(';')
            .filter(|s| !s.is_empty())
            .count(),
            2
        );
        assert!(render("{{if false}}{{.Secret}}{{end}}", &vars, &json!({})).is_err());
        assert!(
            render(
                r#"{{define "loop"}}{{template "loop" .}}{{end}}{{template "loop" .}}"#,
                &vars,
                &json!({})
            )
            .is_err()
        );
        assert_eq!(
            render(r#"{{printf "{{.Unknown}}"}}"#, &vars, &json!({})).unwrap(),
            "{{.Unknown}}"
        );
    }
}
