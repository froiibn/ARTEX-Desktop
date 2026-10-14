//! First-start defaults mirror the Go catalogs, preserving subsequent user edits.
use crate::*;
use sha2::{Digest, Sha256};
pub fn prompts() -> Value {
    serde_json::from_str(include_str!("../resources/builtin-prompts.v1.json"))
        .expect("bundled prompt catalog")
}
pub fn prompt_default(key: &str) -> Option<String> {
    prompts()[key].as_str().map(str::to_owned)
}
fn prompt_repairs() -> Value {
    serde_json::from_str(include_str!("../resources/builtin-prompt-repairs.v1.json"))
        .expect("bundled prompt repair identities")
}
fn repair_known_defaults(b: &mut Planner<'_>) -> Result<()> {
    let repairs = prompt_repairs();
    for repair in repairs["repairs"].as_array().unwrap() {
        let Some(agent) = b
            .state
            .records("agents")
            .into_iter()
            .find(|a| a.data["key"] == repair["agent_key"])
        else {
            continue;
        };
        let Some(current) = agent.data["current_prompt_id"]
            .as_str()
            .and_then(|id| b.state.get("agent_prompts", id))
        else {
            continue;
        };
        if current.data["agent_id"] != agent.header.id
            || current.data["updated_by"] != "system"
            || !matches!(
                text(&current.data, "note"),
                "Built-in default" | "Revert to built-in default"
            )
            || !current.data["template_text"]["blob"].is_null()
        {
            continue;
        }
        let Some(old) = current.data["template_text"]["text"].as_str() else {
            continue;
        };
        if old != text(repair, "old_template_text")
            || format!("{:x}", Sha256::digest(old.as_bytes())) != text(repair, "old_sha256")
        {
            continue;
        }
        let Some(template) = prompt_default(text(repair, "agent_key")) else {
            continue;
        };
        if format!("{:x}", Sha256::digest(template.as_bytes())) != text(repair, "new_sha256") {
            continue;
        }
        b.expect(agent);
        b.expect(current);
        let next = b
            .state
            .records("agent_prompts")
            .into_iter()
            .filter(|p| p.data["agent_id"] == agent.header.id)
            .filter_map(|p| p.data["version"].as_i64())
            .max()
            .unwrap_or(0)
            .checked_add(1)
            .ok_or_else(|| Error::new("SIZE_LIMIT", "Prompt version exhausted"))?;
        let id=b.insert("agent_prompts","prompt",json!({"agent_id":agent.header.id,"version":next,"template_text":artex_core::body(&template),"note":format!("Built-in default repair: {}",text(repair,"id")),"updated_by":"system"}))?;
        let mut data = b.data("agents", &agent.header.id)?;
        data["current_prompt_id"] = json!(id);
        b.put("agents", &agent.header.id, data);
    }
    Ok(())
}
fn seed_reporter(b: &mut Planner<'_>, catalog: &Value) -> Result<()> {
    let preset = prompt_repairs()["reporter_preset"].clone();
    let flag = text(&preset, "seed_flag");
    let prior = b
        .state
        .records("settings")
        .into_iter()
        .find(|r| r.data["key"] == flag);
    if prior.is_some_and(|r| r.data["value"] == "true") {
        return Ok(());
    }
    // The original one-time flag preserves a deliberate deletion and an already
    // occupied custom key. Existing tool rows and user bindings are never rewritten.
    if !b
        .state
        .records("agents")
        .iter()
        .any(|a| a.data["key"] == "reporter")
    {
        let aid=b.insert("agents","agent",json!({"key":"reporter","name":"보고서 작성","role":"assistant","description":"확인된 취약점의 증거와 실행 과정을 바탕으로 상세 Markdown 보고서를 작성하고 저장합니다.","builtin":false,"enabled":true,"trigger_run_mode":"parallel","trigger_merge_mode":"none","trigger_max_parallel":5}))?;
        let pid=b.insert("agent_prompts","prompt",json!({"agent_id":aid,"version":1,"template_text":artex_core::body(catalog["reporter"].as_str().unwrap()),"note":"Built-in default","updated_by":"system"}))?;
        let mut agent = b.data("agents", &aid)?;
        agent["current_prompt_id"] = json!(pid);
        b.put("agents", &aid, agent);
        b.insert("agent_triggers","trigger",json!({"agent_key":"reporter","enabled":true,"on_tool_call":true,"tool_names":"[\"report_finding\"]","tool_call_message":preset["tool_call_message"]}))?;
    }
    if let Some(prior) = prior {
        let mut d = prior.data.clone();
        d["value"] = json!("true");
        b.put("settings", &prior.header.id, d);
    } else {
        b.insert("settings", "setting", json!({"key":flag,"value":"true"}))?;
    }
    Ok(())
}
pub fn tool_seed(def: &Value) -> Value {
    json!({"key":def["name"],"system":true,"description":def["description_ko"],"schema":def["desktop_input_schema"],"agents":def["fresh_seed_agents"],"enabled":!text(def,"desktop_status").contains("default_off"),"kind":"builtin","exec":null,"deferred":false})
}
pub fn bootstrap_plan(s: &Snapshot, c: &Context) -> Result<CommandBatch> {
    let mut b = Planner::new(s, "workspace.bootstrap", &json!({}));
    let ps = prompts();
    for (key, name, role, desc, shell, seconds) in [
        (
            "goals",
            "목표 분해",
            "goals",
            "사용자가 요청한 최종 목표를 검증 가능한 하위 목표로 분해합니다.",
            false,
            1200,
        ),
        (
            "planner",
            "계획",
            "planner",
            "현재 증거와 진행 상황을 읽고 필요한 탐색 의도를 생성합니다.",
            true,
            1200,
        ),
        (
            "mainagent",
            "사용자 인터페이스",
            "main",
            "사용자와 대화하고 진행 상황과 요청을 조율합니다.",
            true,
            1200,
        ),
        (
            "worker",
            "실행",
            "worker",
            "할당된 탐색 의도를 수행하고 사실과 증거를 저장합니다.",
            true,
            1200,
        ),
        (
            "auto",
            "Auto",
            "assistant",
            "도구로 작업, 자산, 스킬 및 플랫폼 설정을 관리합니다.",
            true,
            1200,
        ),
        (
            "pentest",
            "독립 보안 테스트",
            "assistant",
            "허가된 범위에서 계획, 실행, 검증을 수행합니다.",
            true,
            0,
        ),
        (
            "retester",
            "취약점 재검증",
            "assistant",
            "원본 증거와 제약을 확인하고 독립적인 재검증 결론을 저장합니다.",
            true,
            1200,
        ),
    ] {
        let existing = s
            .records("agents")
            .into_iter()
            .find(|r| r.data["key"] == key);
        let id = if let Some(r) = existing {
            r.header.id.clone()
        } else {
            b.insert("agents","agent",json!({"key":key,"name":name,"role":role,"description":desc,"builtin":key!="retester","enabled":true,"interactive_shell":shell,"run_seconds":seconds}))?
        };
        let mut a = b.data("agents", &id)?;
        if a["current_prompt_id"].is_null() {
            let pid=b.insert("agent_prompts","prompt",json!({"agent_id":id,"version":1,"template_text":artex_core::body(ps[key].as_str().unwrap()),"note":"Built-in default","updated_by":"system"}))?;
            a["current_prompt_id"] = json!(pid);
            b.put("agents", &id, a);
        }
        let vars: Vec<(&str, &str, &str, &str)> = match key {
            "goals" => vec![(
                "EngagementDescription",
                "Task description",
                "Test example.com",
                "exploration",
            )],
            "planner" => vec![
                (
                    "Goal",
                    "Overall task goal",
                    "Verify authorization controls",
                    "exploration",
                ),
                (
                    "AssetSummary",
                    "Asset summary",
                    "domain:3 ip:5",
                    "distilled",
                ),
            ],
            "mainagent" => vec![
                (
                    "Goal",
                    "Task goal",
                    "Verify authorization controls",
                    "exploration",
                ),
                (
                    "AssetSummary",
                    "Asset summary",
                    "domain:3 ip:5",
                    "distilled",
                ),
                (
                    "FindingsSummary",
                    "Confirmed findings summary",
                    "high:1 medium:2",
                    "distilled",
                ),
            ],
            "worker" => vec![
                (
                    "ProxyAddr",
                    "Recording proxy address",
                    "127.0.0.1:8080",
                    "runtime",
                ),
                ("WorkerName", "Worker identity", "worker-1", "runtime"),
            ],
            _ => vec![],
        };
        for (var, description, example, source) in vars {
            if !s
                .records("agent_prompt_vars")
                .iter()
                .any(|r| r.data["agent_id"] == id && r.data["var_name"] == var)
            {
                b.insert("agent_prompt_vars","promptvar",json!({"agent_id":id,"var_name":var,"description":description,"example":example,"source":source}))?;
            }
        }
    }
    for def in artex_core::contracts::read("tools.v1.json")["tools"]
        .as_array()
        .unwrap()
    {
        if !s
            .records("tools")
            .iter()
            .any(|r| r.data["key"] == def["name"])
        {
            b.insert("tools", "toolconfig", tool_seed(def))?;
        }
    }
    seed_reporter(&mut b, &ps)?;
    repair_known_defaults(&mut b)?;
    if !s
        .records("mcp_servers")
        .iter()
        .any(|r| r.data["name"] == "browser")
    {
        b.insert("mcp_servers","mcp",json!({"name":"browser","transport":"stdio","command":"npx","args":["@playwright/mcp","--headless"],"env":{},"enabled":false}))?;
    }
    if let Some(Change::Put { collection, id, .. }) = b.batch.changes.first().cloned() {
        conversation::push_event(&mut b.batch, s, c, "ConfigurationSeeded", &collection, &id)?;
    }
    let mut result = b.done(json!({"ok":true}));
    // A replay with no new defaults must not create an event or a commit.
    if result.changes.is_empty() {
        result.events.clear();
    }
    augment_expected(&mut result, s)?;
    Ok(result)
}
