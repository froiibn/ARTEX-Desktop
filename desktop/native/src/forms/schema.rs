use artex_core::{Snapshot, contracts};
use serde_json::{Value, json};
use std::sync::OnceLock;

pub(super) fn rpc() -> &'static Value {
    static RPC: OnceLock<Value> = OnceLock::new();
    RPC.get_or_init(|| contracts::read("rpc.v1.schema.json"))
}
pub(super) fn resolve(schema: &Value) -> Value {
    if let Some(reference) = schema["$ref"].as_str() {
        if let Some(name) = reference.strip_prefix("#/$defs/") {
            return rpc()["$defs"][name].clone();
        }
    }
    schema.clone()
}
pub(super) fn method(name: &str) -> Value {
    resolve(&rpc()["methods"][name])
}
pub(super) fn choices(schema: &Value) -> Vec<Value> {
    let schema = resolve(schema);
    if let Some(values) = schema["enum"].as_array() {
        return values.clone();
    }
    schema["anyOf"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|s| s.get("const").cloned())
        .collect()
}
pub(super) fn actual(schema: &Value, value: &Value) -> Value {
    let schema = resolve(schema);
    if let Some(options) = schema["anyOf"]
        .as_array()
        .or_else(|| schema["oneOf"].as_array())
    {
        if let Some(found) = options
            .iter()
            .map(resolve)
            .find(|s| match s["type"].as_str() {
                Some("null") => value.is_null(),
                Some("string") => value.is_string(),
                Some("number" | "integer") => value.is_number(),
                Some("boolean") => value.is_boolean(),
                Some("array") => value.is_array(),
                Some("object") => value.is_object(),
                _ => false,
            })
        {
            return found;
        }
        return options
            .iter()
            .map(resolve)
            .find(|s| s["type"] != "null")
            .unwrap_or(schema);
    }
    schema
}
pub(super) fn empty(schema: &Value) -> Value {
    let options = choices(schema);
    if let Some(first) = options.first() {
        return first.clone();
    }
    let resolved = resolve(schema);
    if resolved["anyOf"]
        .as_array()
        .is_some_and(|a| a.iter().any(|v| resolve(v)["type"] == "null"))
    {
        return Value::Null;
    }
    let schema = actual(schema, &Value::Null);
    if is_body(&schema) {
        return json!({"text":"","blob":null});
    }
    if is_secret(&schema) {
        return Value::Null;
    }
    match schema["type"].as_str() {
        Some("boolean") => json!(false),
        Some("number" | "integer") => json!(0),
        Some("array") => json!([]),
        Some("object") => json!({}),
        Some("string") if schema.get("x-max").is_some() => json!("0"),
        _ => json!(""),
    }
}
pub(super) fn is_body(schema: &Value) -> bool {
    schema["properties"].get("text").is_some() && schema["properties"].get("blob").is_some()
}
pub(super) fn is_secret(schema: &Value) -> bool {
    schema["properties"].get("secret_ref").is_some()
}
pub(super) fn secret_schema() -> Value {
    resolve(&json!({"$ref":"#/$defs/SecretRef"}))
}
pub(super) fn shape(value: &Value, schema: &Value) -> Value {
    let schema = actual(schema, value);
    if is_body(&schema) && value.is_string() {
        return json!({"text":value,"blob":null});
    }
    if is_secret(&schema) {
        return value.clone();
    }
    if schema["type"] == "string" && schema.get("x-max").is_some() && value.is_number() {
        return json!(value.to_string());
    }
    if let Some(properties) = schema["properties"].as_object().filter(|p| !p.is_empty()) {
        let required = schema["required"].as_array().cloned().unwrap_or_default();
        let mut out = serde_json::Map::new();
        for (key, s) in properties {
            if let Some(v) = value.get(key) {
                let optional = !required.iter().any(|v| v == key);
                let nullable = resolve(s)["type"] == "null"
                    || resolve(s)["anyOf"]
                        .as_array()
                        .is_some_and(|a| a.iter().any(|s| resolve(s)["type"] == "null"));
                if optional
                    && ((v.is_null() && !nullable) || (key == "id" && v.as_str() == Some("")))
                {
                    continue;
                }
                out.insert(key.clone(), shape(v, s));
            } else if required.iter().any(|v| v == key) {
                out.insert(key.clone(), empty(s));
            }
        }
        return Value::Object(out);
    }
    if let Some(items) = value.as_array() {
        if schema.get("items").is_some() {
            return json!(
                items
                    .iter()
                    .map(|v| shape(v, &schema["items"]))
                    .collect::<Vec<_>>()
            );
        }
    }
    value.clone()
}

#[derive(Clone, Debug)]
pub(super) struct Reference {
    pub collection: String,
    pub id: String,
    pub key: String,
    pub label: String,
}
pub(super) fn references(snapshot: &Snapshot) -> Vec<Reference> {
    let mut result = Vec::new();
    for collection in [
        "agents",
        "llm_profiles",
        "mcp_servers",
        "tasks",
        "task_categories",
        "assets",
    ] {
        for row in snapshot.records(collection) {
            if collection == "tasks"
                && (!row.data["deleted_at"].is_null() || !row.data["archived_at"].is_null())
            {
                continue;
            }
            let label = row.data["name"]
                .as_str()
                .filter(|s| !s.is_empty())
                .or_else(|| row.data["description"].as_str())
                .or_else(|| row.data["key"].as_str())
                .unwrap_or(&row.header.id);
            result.push(Reference {
                collection: collection.into(),
                id: row.header.id.clone(),
                key: row.data["key"].as_str().unwrap_or("").into(),
                label: label.into(),
            });
        }
    }
    result
}
pub(super) fn label(key: &str) -> &str {
    match key {
        "p" | "input" => "입력 설정",
        "patch" => "변경할 설정",
        "rule" => "규칙 설정",
        "cfg" => "연결 설정",
        "name" => "이름",
        "description" => "설명",
        "goal" => "목표",
        "format" => "API 형식",
        "model" => "모델",
        "base_url" | "url" => "주소",
        "proxy" | "global_proxy" => "프록시",
        "api_key" => "API 키",
        "max_tokens" => "출력 토큰 한도",
        "max_tokens_field" => "출력 토큰 매개변수",
        "session_header_key" => "세션 헤더 이름",
        "context_window_k" => "문맥 크기 (천 토큰)",
        "streaming" => "스트리밍",
        "enabled" => "활성화",
        "approval_bypass" => "Bypass · 도구 실행 승인 생략",
        "traffic_capture" => "트래픽 기록 저장",
        "agent_traffic_binding" => "에이전트 실행과 트래픽 연결",
        "llm_record" => "모델 요청·응답 기록 저장",
        "web_search_enabled" => "웹 검색 사용",
        "web_search_backend" => "웹 검색 제공자",
        "brave_key_set" => "Brave 검색 키 저장 여부",
        "tavily_key_set" => "Tavily 검색 키 저장 여부",
        "brave_search_api_key" => "Brave 검색 API 키",
        "tavily_search_api_key" => "Tavily 검색 API 키",
        "web_search_proxy" => "웹 검색 프록시",
        "python_interpreter" => "Python 실행 파일 경로",
        "workers" => "작업자 수",
        "task_concurrency_enabled" => "여러 작업 동시 실행",
        "task_concurrency_limit" => "최대 동시 작업 수",
        "llm_pool_enabled" => "모델 풀 사용",
        "llm_pool_bind_fallback" => "지정 모델 실패 시 모델 풀로 전환",
        "constraints_inject_planner" => "계획 에이전트에 제약 조건 전달",
        "constraints_inject_worker" => "작업자에 제약 조건 전달",
        "noa_compaction" => "대화 문맥 자동 압축",
        "notify_enabled" => "알림 사용",
        "notify_public_base_url" => "알림 링크의 공개 주소",
        "notify_digest_interval_min" => "묶음 알림 간격 (분)",
        "schema" => "입력 매개변수",
        "agents" => "사용 에이전트",
        "kind" => "종류",
        "transport" => "전송 방식",
        "command" => "실행 명령",
        "args" => "인수",
        "env" => "환경 변수",
        "headers" => "HTTP 헤더",
        "key" => "식별 이름",
        "script" => "실행 코드",
        "instructions" => "스킬 지침",
        "mcps" | "mcp" => "MCP 서버",
        "skill" => "스킬",
        "text" => "내용",
        "category_id" => "작업 분류",
        "intercept_rules" => "자산 차단 규칙",
        "pattern" => "일치 패턴",
        "action" => "동작",
        "priority" => "우선순위",
        "message" => "안내 메시지",
        "note" => "메모",
        "timeout_seconds" => "시간 제한 (초)",
        "timeout_enabled" => "시간 제한 사용",
        "timeout_action" => "시간 만료 동작",
        "match_target" => "검사 대상",
        "match_type" => "일치 방식",
        "rate_per_min" => "분당 전송 한도",
        "mode" => "모드",
        "filter" => "알림 조건",
        "config" => "연결 설정",
        "llm_profile_id" => "모델 프로필",
        "max_turns" => "최대 턴",
        "run_seconds" => "실행 시간 (초)",
        "web_search" => "웹 검색",
        "interactive_shell" => "대화형 터미널",
        "trigger_run_mode" => "호출 실행 방식",
        "trigger_merge_mode" => "이벤트 병합",
        "trigger_max_parallel" => "동시 실행 수",
        "insecure" => "TLS 검증 생략",
        "deferred" => "검색 후에만 호출",
        "thinking_type" => "사고 모드",
        "reasoning_effort" => "추론 강도",
        "thinking" => "사고",
        "rate_per_second" => "초당 요청 한도",
        "rate_per_minute" => "분당 요청 한도",
        "pool_exclude" => "모델 풀에서 제외",
        "retry" => "재시도 정책",
        "connect" => "연결 실패",
        "empty" => "빈 응답",
        "stream" => "스트림 실패",
        "attempts" => "재시도 횟수 (-1: 사용 안 함)",
        "interval_ms" => "재시도 간격 (밀리초)",
        _ => key,
    }
}
pub(super) fn enum_label(value: &str) -> &str {
    match value {
        "" => "보내지 않음 (기본값)",
        "allow" => "허용",
        "deny" | "block" => "차단",
        "ask" => "사용자 확인",
        "realtime" => "즉시 전송",
        "digest" => "모아 전송",
        "parallel" => "동시 실행",
        "serial" => "순차 실행",
        "none" => "병합 안 함",
        "all" => "모두 병합",
        "by_task" => "작업별 병합",
        "stdio" => "프로세스 입출력",
        "http" => "HTTP",
        "sse" => "HTTP 이벤트",
        "command" => "명령",
        "script" => "Python 코드",
        "email" => "이메일",
        "webhook" => "HTTP 알림",
        "telegram" => "Telegram",
        "openai" => "OpenAI 호환",
        "anthropic" => "Anthropic",
        "default" => "기본값",
        "configured" => "설정됨",
        "disabled" => "사용 안 함",
        "enabled" => "사용",
        "false" => "사용 안 함",
        "true" => "사용",
        "openai-responses" => "OpenAI Responses 호환",
        _ => value,
    }
}

/// Presentation groups do not reshape or remove the source payload.
pub(super) fn field_group(collection: &str, key: &str) -> usize {
    match collection {
        "llm_profiles" => match key {
            "name" | "format" | "base_url" | "model" | "api_key" => 0,
            "max_tokens" | "context_window_k" | "streaming" | "thinking_type"
            | "reasoning_effort" => 1,
            _ => 2,
        },
        "settings" => match key {
            "approval_bypass" | "traffic_capture" | "llm_record" | "notify_enabled" => 0,
            "global_proxy"
            | "python_interpreter"
            | "web_search_enabled"
            | "web_search_backend"
            | "brave_key_set"
            | "tavily_key_set"
            | "brave_search_api_key"
            | "tavily_search_api_key"
            | "web_search_proxy"
            | "notify_public_base_url" => 1,
            "workers"
            | "agent_traffic_binding"
            | "task_concurrency_enabled"
            | "task_concurrency_limit"
            | "llm_pool_enabled"
            | "llm_pool_bind_fallback"
            | "constraints_inject_planner"
            | "constraints_inject_worker"
            | "noa_compaction" => 2,
            _ => 3,
        },
        _ => 0,
    }
}
