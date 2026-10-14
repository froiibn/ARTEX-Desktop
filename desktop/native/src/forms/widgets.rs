use super::{FormSubmission, schema};
use eframe::egui::{self, Ui};
use serde_json::{Value, json};
use std::collections::{BTreeMap, BTreeSet};

pub(super) struct WidgetState<'a> {
    pub token: &'a str,
    pub collection: &'a str,
    pub selected: Option<&'a str>,
    pub references: &'a [schema::Reference],
    pub secrets: &'a mut BTreeMap<String, String>,
    pub pending_secrets: &'a mut BTreeSet<String>,
    pub pending_reads: &'a mut BTreeSet<String>,
    pub submissions: &'a mut Vec<FormSubmission>,
    pub requests: &'a mut Vec<(String, Value)>,
    pub can_secret: bool,
    pub invalid_secret: bool,
}
fn path_key(path: &[String]) -> String {
    serde_json::to_string(path).unwrap()
}
pub(super) fn sensitive(key: &str) -> bool {
    let key = key.to_ascii_lowercase().replace('-', "_");
    matches!(
        key.as_str(),
        "api_key"
            | "password"
            | "token"
            | "secret"
            | "authorization"
            | "cookie"
            | "webhook"
            | "bot_token"
    ) || key.ends_with("_api_key")
        || key.ends_with("_password")
        || key.ends_with("_secret")
        || key.ends_with("_token") && !key.ends_with("_tokens")
        || key.ends_with("_cookie")
        || key.ends_with("_authorization")
        || key.contains("apikey")
}
fn is_long(key: &str) -> bool {
    matches!(
        key,
        "description"
            | "goal"
            | "text"
            | "instructions"
            | "template"
            | "prompt"
            | "report"
            | "code"
            | "body"
            | "body_template"
            | "message"
            | "compatibility"
    )
}
fn target(state: &WidgetState<'_>, path: &[String]) -> Value {
    json!({"form_token":state.token,"collection":state.collection,"id":state.selected,"path":path})
}

fn secret(ui: &mut Ui, key: &str, value: &mut Value, path: &[String], state: &mut WidgetState<'_>) {
    crate::theme::field_row(ui, schema::label(key), |ui| {
        secret_controls(ui, key, value, path, state);
    });
}
fn secret_controls(
    ui: &mut Ui,
    key: &str,
    value: &mut Value,
    path: &[String],
    state: &mut WidgetState<'_>,
) {
    let id = path_key(path);
    if value.get("secret_ref").is_some() {
        let hint = value["hint"].as_str().unwrap_or("비밀 값");
        if hint == "configured" {
            ui.label("비밀 값 등록됨");
        } else {
            ui.label(format!("저장됨: {hint}"));
        }
    } else if !value.is_null() && value.as_str() != Some("") {
        ui.colored_label(
            ui.visuals().warn_fg_color,
            "안전한 참조 형식이 아닙니다. 교체가 필요합니다.",
        );
        state.invalid_secret = true;
    } else {
        ui.weak("저장된 비밀 값 없음");
    }
    let waiting = state.pending_secrets.contains(&id);
    let secret_target = target(state, path);
    let buffer = state.secrets.entry(id.clone()).or_default();
    ui.horizontal_wrapped(|ui| {
        ui.add_enabled(
            !waiting && state.can_secret,
            egui::TextEdit::singleline(buffer)
                .password(true)
                .desired_width(ui.available_width().min(280.0))
                .hint_text("새 비밀 값 (로컬 보관소에 저장)"),
        );
        if ui
            .add_enabled(
                !waiting && state.can_secret && !buffer.is_empty(),
                egui::Button::new("비밀 값 저장"),
            )
            .clicked()
        {
            let plaintext = std::mem::take(buffer);
            state.pending_secrets.insert(id);
            state.submissions.push(FormSubmission {
                operation: "native.secret.set".into(),
                params: json!({"field":path.join("."),"value":plaintext,"target":secret_target}),
                expected: vec![],
            });
        }
    });
    if waiting {
        ui.weak("안전한 참조를 기다리는 중…");
    }
    if state.collection == "settings"
        && matches!(key, "brave_search_api_key" | "tavily_search_api_key")
        && ui
            .add_enabled(
                !waiting && state.can_secret,
                egui::Button::new("저장된 키 제거"),
            )
            .clicked()
    {
        *value = json!("");
    }
}
fn body(ui: &mut Ui, value: &mut Value, path: &[String], state: &mut WidgetState<'_>) {
    if value["blob"].is_object() && value["text"].is_null() {
        ui.weak("큰 본문은 확인된 저장 데이터에서 불러옵니다.");
        let id = path_key(path);
        if ui
            .add_enabled(
                !state.pending_reads.contains(&id),
                egui::Button::new("본문 불러오기"),
            )
            .clicked()
        {
            state.pending_reads.insert(id);
            state.requests.push((
                "native.body.read".into(),
                json!({"reference":value["blob"],"_form_token":state.token,"_form_path":path}),
            ));
        }
        return;
    }
    if !value.is_object() {
        *value = json!({"text":"","blob":null});
    }
    if !value["text"].is_string() {
        value["text"] = json!("");
    }
    if let Value::String(text) = &mut value["text"] {
        ui.add(
            egui::TextEdit::multiline(text)
                .desired_rows(8)
                .desired_width(f32::INFINITY),
        );
    }
}
fn dictionary(
    ui: &mut Ui,
    value: &mut Value,
    path: &[String],
    state: &mut WidgetState<'_>,
    secret_entries: bool,
) {
    if value.get("secret_ref").is_some() {
        secret(ui, "env", value, path, state);
        if ui.button("목록 전체를 새로 작성").clicked() {
            *value = json!({});
        }
        return;
    }
    if !value.is_object() {
        *value = json!({});
    }
    let keys = value
        .as_object()
        .unwrap()
        .keys()
        .cloned()
        .collect::<Vec<_>>();
    let mut remove = None;
    for key in keys {
        ui.horizontal_wrapped(|ui| {
            ui.label(&key);
            if ui.small_button("삭제").clicked() {
                remove = Some(key.clone());
            }
        });
        let mut p = path.to_vec();
        p.push(key.clone());
        if secret_entries || sensitive(&key) || value[&key].get("secret_ref").is_some() {
            secret(ui, &key, &mut value[&key], &p, state);
        } else {
            field(
                ui,
                &key,
                &mut value[&key],
                &json!({"type":"string"}),
                &p,
                state,
                0,
            );
        }
    }
    if let Some(key) = remove {
        value.as_object_mut().unwrap().remove(&key);
    }
    let buffer_key = format!("new-key:{}", path_key(path));
    let new = state.secrets.entry(buffer_key).or_default();
    ui.horizontal_wrapped(|ui| {
        ui.add(egui::TextEdit::singleline(new).hint_text("새 키 이름"));
        if ui.button("항목 추가").clicked() && !new.trim().is_empty() {
            value
                .as_object_mut()
                .unwrap()
                .entry(new.trim().to_owned())
                .or_insert(json!(""));
            new.clear();
        }
    });
}
fn exec_schema(value: &Value) -> Value {
    if value.get("url").is_some() {
        json!({"type":"object","properties":{"url":{"type":"string"},"method":{"type":"string","enum":["GET","POST","PUT","PATCH","DELETE","HEAD"]},"headers":{"type":"object"},"body":{"type":"string"},"timeout_ms":{"type":"integer"},"proxy":{"type":"string"}}})
    } else {
        json!({"type":"object","properties":{"command":{"type":"string"},"code":{"type":"string"},"cwd":{"type":"string"},"env":{"type":"object"},"timeout_ms":{"type":"integer"}}})
    }
}
pub(super) fn notification_schema(kind: &str) -> Value {
    let secret = schema::secret_schema();
    let s = json!({"type":"string"});
    let properties = match kind {
        "email" => {
            json!({"host":s,"port":{"type":"integer"},"username":s,"password":secret,"from":s,"to":{"type":"array","items":s},"tls":{"type":"boolean"}})
        }
        "telegram" => json!({"bot_token":secret,"chat_id":s,"base_url":s}),
        "dingtalk" | "feishu" => json!({"webhook":secret,"secret":secret}),
        "wecom" => json!({"webhook":secret}),
        _ => {
            json!({"url":secret,"headers":{"type":"object"},"method":{"type":"string","enum":["POST","PUT","PATCH","GET"]},"body_template":s,"timeout_seconds":{"type":"integer"}})
        }
    };
    let required = match kind {
        "email" => vec!["host", "port", "from", "to"],
        "telegram" => vec!["bot_token", "chat_id"],
        "dingtalk" | "feishu" | "wecom" => vec!["webhook"],
        _ => vec!["url"],
    };
    json!({"type":"object","properties":properties,"required":required})
}
fn schema_builder(ui: &mut Ui, value: &mut Value, path: &[String], state: &mut WidgetState<'_>) {
    if path.len() > 18 {
        ui.weak("깊은 매개변수 스키마의 기존 값을 유지합니다.");
        return;
    }
    if !value.is_object() {
        ui.weak("기존 복합 스키마를 유지합니다.");
        if ui.button("매개변수 목록 새로 만들기").clicked() {
            *value = json!({"type":"object","properties":{}});
        }
        return;
    }
    if !value["properties"].is_object() {
        if value.as_object().unwrap().is_empty() {
            *value = json!({"type":"object","properties":{}});
        } else {
            ui.weak("복합 스키마의 기존 정의를 보존합니다.");
            return;
        }
    }
    let keys = value["properties"]
        .as_object()
        .unwrap()
        .keys()
        .cloned()
        .collect::<Vec<_>>();
    let mut removed = None;
    for key in keys {
        egui::CollapsingHeader::new(format!("매개변수: {key}"))
            .id_salt((state.token, path_key(path), &key))
            .show(ui, |ui| {
                let required = value["required"]
                    .as_array()
                    .is_some_and(|a| a.iter().any(|v| v == &key));
                let mut required_now = required;
                ui.checkbox(&mut required_now, "필수 입력");
                if required_now != required {
                    if !value["required"].is_array() {
                        value["required"] = json!([]);
                    }
                    let array = value["required"].as_array_mut().unwrap();
                    array.retain(|v| v != &key);
                    if required_now {
                        array.push(json!(key));
                    }
                }
                let property = &mut value["properties"][&key];
                if !property.is_object() {
                    ui.weak("기존 복합 타입을 유지합니다.");
                    return;
                }
                if property["$ref"].is_string()
                    || property.get("anyOf").is_some()
                    || property.get("oneOf").is_some()
                {
                    ui.weak("복합·참조 타입의 기존 정의를 보존합니다.");
                } else {
                    let current = property["type"].as_str().unwrap_or("string").to_owned();
                    let mut selected = current.clone();
                    egui::ComboBox::from_id_salt((path_key(path), &key, "type"))
                        .selected_text(&selected)
                        .show_ui(ui, |ui| {
                            for t in ["string", "integer", "number", "boolean", "object", "array"] {
                                ui.selectable_value(&mut selected, t.into(), t);
                            }
                        });
                    if selected != current {
                        property["type"] = json!(selected);
                        property.as_object_mut().unwrap().remove("default");
                    }
                    if selected == "array" && !property["items"].is_object() {
                        property["items"] = json!({"type":"string"});
                    }
                    if selected == "object" {
                        if !property["properties"].is_object() {
                            property["properties"] = json!({});
                        }
                        let mut p = path.to_vec();
                        p.extend(["properties".into(), key.clone()]);
                        ui.indent((&key, "object-fields"), |ui| {
                            schema_builder(ui, property, &p, state);
                        });
                    }
                    if selected == "array" {
                        let mut item_type = property["items"]["type"]
                            .as_str()
                            .unwrap_or("string")
                            .to_owned();
                        let before = item_type.clone();
                        ui.horizontal_wrapped(|ui| {
                            ui.label("목록 항목 타입");
                            egui::ComboBox::from_id_salt((path_key(path), &key, "items"))
                                .selected_text(&item_type)
                                .show_ui(ui, |ui| {
                                    for t in ["string", "integer", "number", "boolean", "object"] {
                                        ui.selectable_value(&mut item_type, t.into(), t);
                                    }
                                });
                        });
                        if item_type != before {
                            property["items"] = json!({"type":item_type});
                        }
                        if item_type == "object" {
                            if !property["items"]["properties"].is_object() {
                                property["items"]["properties"] = json!({});
                            }
                            let mut p = path.to_vec();
                            p.extend(["properties".into(), key.clone(), "items".into()]);
                            ui.indent((&key, "item-fields"), |ui| {
                                schema_builder(ui, &mut property["items"], &p, state);
                            });
                        }
                    }
                    if matches!(
                        selected.as_str(),
                        "string" | "integer" | "number" | "boolean"
                    ) {
                        let mut enabled = property.get("default").is_some();
                        if ui.checkbox(&mut enabled, "기본값 사용").changed() {
                            if enabled {
                                property["default"] = schema::empty(&json!({"type":selected}));
                            } else {
                                property.as_object_mut().unwrap().remove("default");
                            }
                        }
                        if enabled {
                            let mut p = path.to_vec();
                            p.extend(["properties".into(), key.clone(), "default".into()]);
                            field(
                                ui,
                                "기본값",
                                &mut property["default"],
                                &json!({"type":selected}),
                                &p,
                                state,
                                0,
                            );
                        }
                    }
                }
                if !property["description"].is_string() {
                    property["description"] = json!("");
                }
                if let Value::String(s) = &mut property["description"] {
                    ui.add(egui::TextEdit::singleline(s).hint_text("매개변수 설명"));
                }
                if ui.button("이 매개변수 삭제").clicked() {
                    removed = Some(key.clone());
                }
            });
    }
    if let Some(key) = removed {
        value["properties"].as_object_mut().unwrap().remove(&key);
        if let Some(a) = value["required"].as_array_mut() {
            a.retain(|v| v != &key);
        }
    }
    let key = format!("schema-key:{}", path_key(path));
    let draft = state.secrets.entry(key).or_default();
    ui.horizontal_wrapped(|ui| {
        ui.add(egui::TextEdit::singleline(draft).hint_text("새 매개변수 이름"));
        if ui.button("매개변수 추가").clicked() && !draft.trim().is_empty() {
            value["properties"]
                .as_object_mut()
                .unwrap()
                .entry(draft.trim().to_owned())
                .or_insert(json!({"type":"string","description":""}));
            draft.clear();
        }
    });
}
pub(super) fn field(
    ui: &mut Ui,
    key: &str,
    value: &mut Value,
    schema_value: &Value,
    path: &[String],
    state: &mut WidgetState<'_>,
    depth: usize,
) {
    if depth > 8 {
        ui.weak("깊은 구조의 기존 값을 보존합니다.");
        return;
    }
    let schema = schema::actual(schema_value, value);
    if schema::is_secret(&schema)
        || sensitive(key) && !matches!(key, "session_header_key" | "secret_ref")
    {
        secret(ui, key, value, path, state);
        return;
    }
    if schema::is_body(&schema) {
        crate::theme::group(ui, schema::label(key), |ui| {
            ui.set_min_width(ui.available_width());
            body(ui, value, path, state);
        });
        return;
    }
    if key == "schema" {
        ui.label("도구 입력 매개변수");
        schema_builder(ui, value, path, state);
        return;
    }
    if matches!(key, "env" | "headers") {
        ui.label(schema::label(key));
        dictionary(ui, value, path, state, false);
        return;
    }
    if matches!(key, "id" | "constraintId" | "ruleId" | "key") && state.selected.is_some() {
        ui.label(format!(
            "{}: {}",
            schema::label(key),
            value.as_str().unwrap_or("")
        ));
        return;
    }
    if value.is_boolean()
        || schema["type"] == "boolean"
        || schema::choices(schema_value).iter().any(Value::is_boolean)
    {
        let mut checked = value.as_bool().unwrap_or(false);
        ui.scope(|ui| {
            ui.style_mut().wrap_mode = Some(egui::TextWrapMode::Wrap);
            let response = ui.checkbox(&mut checked, schema::label(key));
            if key == "approval_bypass" {
                response.clone().on_hover_text("작업 공간의 내부 실행 승인과 모델 심사를 생략합니다. 명시적 거부·계획 승인·범위 제한·사용자 질문은 유지합니다. 상단 Bypass 도구 모음과 같은 저장 설정입니다.");
            }
            if response.changed() {
                *value = json!(checked);
            }
        });
        return;
    }
    let mut choices = schema::choices(schema_value);
    if choices.is_empty() {
        let strings: &[&str] = match key {
            "format" => &["anthropic", "openai", "openai-responses"],
            "thinking_type" => &["", "disabled", "enabled"],
            "reasoning_effort" => &[
                "", "low", "medium", "high", "xhigh", "minimal", "none", "off",
            ],
            "max_tokens_field" => &["", "max_completion_tokens"],
            "match_target" => &["tool_name", "tool_input"],
            "match_type" => &["string", "regex"],
            "action" => &["allow", "deny", "ask", "judge"],
            "timeout_action" => &["deny", "allow"],
            "web_search_backend" => &["ddgs", "brave-free", "tavily", "deepseek"],
            "kind" if state.collection == "notification_channels" => &[
                "email", "webhook", "telegram", "dingtalk", "feishu", "wecom",
            ],
            "kind" if state.collection == "tools" => &["command", "script", "http"],
            "mode" if state.collection == "notification_channels" => &["realtime", "digest"],
            "trigger_run_mode" => &["serial", "parallel"],
            "trigger_merge_mode" => &["by_task", "all", "none"],
            "min_severity" => &["", "low", "medium", "high", "critical"],
            _ => &[],
        };
        choices = strings.iter().map(|s| json!(s)).collect();
    }
    if !choices.is_empty() {
        crate::theme::field_row(ui, &schema::label(key), |ui| {
            egui::ComboBox::from_id_salt((state.token, path_key(path)))
                .width(ui.available_width())
                .selected_text(schema::enum_label(value.as_str().unwrap_or("선택")))
                .show_ui(ui, |ui| {
                    for option in choices {
                        let label = schema::enum_label(option.as_str().unwrap_or("선택"));
                        ui.selectable_value(value, option.clone(), label);
                    }
                });
        });
        return;
    }
    if let Some(pattern) = schema["pattern"]
        .as_str()
        .filter(|p| p.contains("[0-9a-f]{32}"))
    {
        let prefix = pattern
            .strip_prefix('^')
            .unwrap_or(pattern)
            .split('_')
            .next()
            .unwrap_or("");
        let collection = match prefix {
            "agent" => "agents",
            "profile" => "llm_profiles",
            "mcp" => "mcp_servers",
            "task" => "tasks",
            "category" => "task_categories",
            "asset" => "assets",
            _ => "",
        };
        crate::theme::field_row(ui, &schema::label(key), |ui| {
            let selected = state
                .references
                .iter()
                .find(|r| value == &json!(r.id))
                .map(|r| r.label.as_str())
                .unwrap_or("선택 안 함");
            egui::ComboBox::from_id_salt((state.token, path_key(path)))
                .width(ui.available_width())
                .selected_text(selected)
                .show_ui(ui, |ui| {
                    ui.selectable_value(value, Value::Null, "선택 안 함");
                    for r in state
                        .references
                        .iter()
                        .filter(|r| r.collection == collection)
                    {
                        ui.selectable_value(value, json!(r.id), &r.label);
                    }
                });
        });
        return;
    }
    if schema["type"] == "boolean" {
        let mut checked = value.as_bool().unwrap_or(false);
        if ui.checkbox(&mut checked, schema::label(key)).changed() {
            *value = json!(checked);
        }
        return;
    }
    if schema.get("x-max").is_some() {
        let mut number = value
            .as_str()
            .and_then(|s| s.parse::<u64>().ok())
            .unwrap_or(0);
        crate::theme::field_row(ui, &schema::label(key), |ui| {
            if ui.add(egui::DragValue::new(&mut number).speed(1)).changed() {
                *value = json!(number.to_string());
            }
        });
        return;
    }
    if matches!(schema["type"].as_str(), Some("integer" | "number")) {
        let mut number = value.as_f64().unwrap_or(0.0);
        crate::theme::field_row(ui, &schema::label(key), |ui| {
            if ui.add(egui::DragValue::new(&mut number).speed(1)).changed() {
                *value = if number.fract() == 0.0 {
                    json!(number as i64)
                } else {
                    json!(number)
                };
            }
        });
        return;
    }
    if schema["type"] == "array" || value.is_array() {
        ui.label(schema::label(key));
        if !value.is_array() {
            *value = json!([]);
        }
        if matches!(key, "agents" | "mcp" | "mcps") {
            let references = state
                .references
                .iter()
                .filter(|r| {
                    if key == "agents" {
                        r.collection == "agents"
                    } else {
                        r.collection == "mcp_servers"
                    }
                })
                .cloned()
                .collect::<Vec<_>>();
            for r in references {
                let selected = if key == "agents" {
                    json!(r.key)
                } else if key == "mcps" {
                    json!(r.label)
                } else {
                    json!(r.id)
                };
                let mut on = value.as_array().unwrap().contains(&selected);
                if ui.checkbox(&mut on, &r.label).changed() {
                    let items = value.as_array_mut().unwrap();
                    items.retain(|v| v != &selected);
                    if on {
                        items.push(selected);
                    }
                }
            }
            return;
        }
        let mut removed = None;
        for (index, item) in value.as_array_mut().unwrap().iter_mut().enumerate() {
            let mut p = path.to_vec();
            p.push(index.to_string());
            ui.vertical(|ui| {
                ui.set_min_width(ui.available_width());
                if ui
                    .small_button(format!("항목 {} 삭제", index + 1))
                    .clicked()
                {
                    removed = Some(index);
                }
                field(ui, "", item, &schema["items"], &p, state, depth + 1);
            });
        }
        if let Some(index) = removed {
            value.as_array_mut().unwrap().remove(index);
        }
        if ui.button("항목 추가").clicked() {
            value
                .as_array_mut()
                .unwrap()
                .push(schema::empty(&schema["items"]));
        }
        return;
    }
    let effective = if key == "exec" {
        exec_schema(value)
    } else {
        schema.clone()
    };
    if effective["type"] == "object" || value.is_object() {
        if !value.is_object() {
            *value = json!({});
        }
        if let Some(props) = effective["properties"]
            .as_object()
            .filter(|p| !p.is_empty())
        {
            if depth == 1
                && path.len() == 1
                && ((state.collection == "llm_profiles" && path[0] == "p")
                    || (state.collection == "settings" && path[0] == "patch"))
            {
                let groups: &[&str] = if state.collection == "llm_profiles" {
                    &["연결 설정", "응답 설정", "고급 설정"]
                } else {
                    &["일반", "연결", "에이전트", "고급 설정"]
                };
                for (index, title) in groups.iter().enumerate() {
                    let fields = props
                        .iter()
                        .filter(|(name, _)| schema::field_group(state.collection, name) == index)
                        .map(|(name, field)| (name.clone(), field.clone()))
                        .collect::<serde_json::Map<_, _>>();
                    if fields.is_empty() {
                        continue;
                    }
                    let group_schema = json!({"type":"object","properties":fields,"required":effective["required"]});
                    if index == groups.len() - 1 {
                        egui::CollapsingHeader::new(*title)
                            .id_salt((state.token, path_key(path), "advanced"))
                            .show(ui, |ui| {
                                field(ui, "", value, &group_schema, path, state, depth + 1);
                                ui.collapsing("원문 필드 이름·값 확인", |ui| {
                                    for name in props.keys() {
                                        if !sensitive(name) && value.get(name).is_some() {
                                            ui.horizontal_wrapped(|ui| {
                                                ui.monospace(name);
                                                ui.label(value[name].to_string());
                                            });
                                        }
                                    }
                                });
                            });
                    } else {
                        crate::theme::group(ui, title, |ui| {
                            field(ui, "", value, &group_schema, path, state, depth + 1);
                        });
                    }
                }
                return;
            }
            let provider = value["kind"].as_str().map(str::to_owned);
            ui.vertical(|ui| {
                if !key.is_empty() && !matches!(key, "p" | "patch" | "input") {
                    ui.add_space(6.0);
                    ui.strong(schema::label(key));
                    ui.add_space(4.0);
                }
                let mut ordered = props.iter().collect::<Vec<_>>();
                if state.collection == "llm_profiles" && path.first().is_some_and(|p| p == "p") {
                    let order = ["name", "format", "base_url", "model", "api_key"];
                    ordered.sort_by_key(|(name, _)| {
                        order
                            .iter()
                            .position(|key| *key == name.as_str())
                            .unwrap_or(order.len())
                    });
                }
                for (name, child_schema) in ordered {
                    if matches!(name.as_str(), "brave_key_set" | "tavily_key_set" | "tools") {
                        if value.get(name).is_some() {
                            if name == "tools" {
                                ui.weak(format!("{}: {}", schema::label(name), value[name]));
                                continue;
                            }
                            let status = if value[name] == true {
                                "등록됨"
                            } else if value[name] == false {
                                "등록되지 않음"
                            } else {
                                "저장 정보 확인"
                            };
                            ui.weak(format!("{}: {status}", schema::label(name)));
                        }
                        continue;
                    }
                    if name == "id" && value.get(name).is_none() {
                        continue;
                    }
                    if value.get(name).is_none()
                        && !effective["required"]
                            .as_array()
                            .is_some_and(|r| r.iter().any(|v| v == name))
                    {
                        let mut activate = false;
                        crate::theme::field_row(ui, schema::label(name), |ui| {
                            activate = ui.small_button("설정 추가").clicked();
                        });
                        if !activate {
                            continue;
                        }
                    }
                    if value.get(name).is_none() {
                        value[name] = schema::empty(child_schema);
                    }
                    if name == "exec"
                        && provider.as_deref() == Some("http")
                        && value[name].get("url").is_none()
                    {
                        value[name]["url"] = json!("");
                    }
                    let mut p = path.to_vec();
                    p.push(name.clone());
                    let child_schema = if name == "config" {
                        notification_schema(provider.as_deref().unwrap_or("email"))
                    } else {
                        child_schema.clone()
                    };
                    field(
                        ui,
                        if state.collection == "llm_profiles" && name == "name" {
                            "표시 이름"
                        } else {
                            name
                        },
                        &mut value[name],
                        &child_schema,
                        &p,
                        state,
                        depth + 1,
                    );
                }
            });
        } else {
            dictionary(ui, value, path, state, false);
        }
        return;
    }
    if !value.is_string() {
        *value = json!("");
    }
    if let Value::String(text) = value {
        if is_long(key) {
            crate::theme::group(ui, &schema::label(key), |ui| {
                ui.add(
                    egui::TextEdit::multiline(text)
                        .desired_rows(4)
                        .desired_width(f32::INFINITY),
                );
            });
        } else {
            crate::theme::field_row(ui, &schema::label(key), |ui| {
                ui.add(egui::TextEdit::singleline(text).desired_width(f32::INFINITY));
            });
        }
    }
}
