//! Joined configuration acceptance. Draft values are fixture inputs; every
//! qualified mutation still uses its real widget, Prepare/CAS and Service.
//! No projection, success result, OS credential or user workspace is injected.
use super::*;
use super::transport::*;

fn fixture() -> Fixture {
    Fixture::configured(|profile| ServiceConfig {
        host_factory: Some(factory(profile, BTreeMap::new(), Probe::default())),
        wake_ui: None,
    })
}

fn settle(f: &mut Fixture) {
    f.until(|app| app.pending.is_empty());
    assert!(f.app.error.is_none(), "{:?}", f.app.error);
}

fn row(f: &Fixture, collection: &str, id: &str) -> artex_core::Record {
    f.app.handle.view().unwrap().generation.get(collection,id).unwrap().clone()
}

fn keyed(f: &Fixture, collection: &str, field: &str, value: &str) -> artex_core::Record {
    f.app.handle.view().unwrap().generation.records(collection).into_iter()
        .find(|r|r.data[field] == value).unwrap_or_else(||panic!("Missing {collection}/{field}={value}" )).clone()
}

fn action(f: &mut Fixture, section: Section, method: &str, params: Value) -> Value {
    f.source_action(section,None,method,params)
}

fn query(f: &mut Fixture, section: Section, method: &str, params: Value, needle: &str) -> Value {
    f.source_query(section,None,method,params);
    let value=f.app.source_operations.query_result_for_app_test(method,None).unwrap();
    assert!(value.to_string().contains(needle),"{method} missing {needle}: {value}");
    f.source_result_visible(method,None,needle);
    value
}

fn form(f:&mut Fixture,section:Section,collection:&str,selected:Option<&str>,method:&str,
        params:Value,extras:Option<Value>,open:&[&str],button:&str) {
    select(f,section,selected);
    let view=f.app.handle.view().unwrap();
    f.app.forms.seed_editor_for_app_test(collection,selected,&view.generation.snapshot,params,extras);
    f.app.drawer.open=false;
    f.frame(vec![]);
    f.scroll_drawer_top();
    // Settings/Skill readiness depends on real automatic projection reads.
    // Wait for those callbacks before clicking the enabled save control.
    settle(f);
    for label in open {
        // Disclosures retain egui state across workspace opens. Reopen a
        // closed panel, but never close one whose target action is painted.
        if label.contains("확인") || !Fixture::texts(&f.frame(vec![])).iter().any(|text|text==button) {
            f.click_visible(label);
        }
    }
    f.click_visible(button);
    assert!(f.app.pending.values().any(|p|matches!(p,Purpose::Prepare{method:actual,..} if actual==method)),
        "Actual form {method} did not prepare: {:?}",Fixture::texts(&f.frame(vec![])));
    f.prepare(method);
    settle(f);
}

fn left_row(f:&mut Fixture,section:Section,id:&str,label:&str) {
    select(f,section,None);
    let output=f.frame(vec![]);
    let main_left=f.ctx.available_rect().left();
    let pos=output.shapes.iter().find_map(|shape| {
        let egui::epaint::Shape::Text(text)=&shape.shape else{return None;};
        let rect=text.visual_bounding_rect();
        (text.galley.text().contains(label)&&rect.right()<=main_left&&shape.clip_rect.contains_rect(rect)).then(||rect.center())
    }).unwrap_or_else(||panic!("Actual left list row missing {label}: {:?}",Fixture::texts(&output)));
    f.frame(vec![egui::Event::PointerMoved(pos),egui::Event::PointerButton{pos,button:egui::PointerButton::Primary,pressed:true,modifiers:Default::default()}]);
    f.frame(vec![egui::Event::PointerButton{pos,button:egui::PointerButton::Primary,pressed:false,modifiers:Default::default()}]);
    assert_eq!(f.app.selected[&section].as_deref(),Some(id));
}

fn trigger(message:&str)->Value {
    json!({"enabled":false,"interval_sec":0,"on_finding":false,"on_goal_met":false,"on_task_timeout":false,
        "on_tool_call":false,"on_task_create":true,"interval_message":"","finding_message":"","goal_message":"",
        "task_timeout_message":"","tool_call_message":"","task_create_message":message,"tool_names":[]})
}

#[test]
fn config_accept_custom_agent_metadata_config_and_trigger_save_reopen_edit_delete() {
    let mut f=fixture();
    form(&mut f,Section::Agents,"agents",None,"ui.createAgent",json!({"key":"accept_agent",
        "name":"ACCEPT AGENT CREATED","description":"ACCEPT AGENT DESCRIPTION"}),None,&[],"저장");
    let id=keyed(&f,"agents","key","accept_agent").header.id;
    f.reopen_acceptance(false);
    left_row(&mut f,Section::Agents,&id,"ACCEPT AGENT CREATED");
    f.click_visible("에이전트 이름과 설명");
    f.assert_drawer_texts(&["ACCEPT AGENT DESCRIPTION"]);
    form(&mut f,Section::Agents,"agents",Some(&id),"ui.updateAgent",Value::Null,
        Some(json!({"metadata":{"key":"accept_agent","name":"ACCEPT AGENT UPDATED","description":"ACCEPT UPDATED DESCRIPTION"}})),
        &["에이전트 이름과 설명"],"이름과 설명 저장");
    f.reopen_acceptance(false);
    assert_eq!(row(&f,"agents",&id).data["name"],"ACCEPT AGENT UPDATED");
    left_row(&mut f,Section::Agents,&id,"ACCEPT AGENT UPDATED");
    if !Fixture::texts(&f.frame(vec![])).iter().any(|s|s=="이름과 설명 저장") {f.click_visible("에이전트 이름과 설명");}
    f.assert_drawer_texts(&["ACCEPT UPDATED DESCRIPTION"]);
    form(&mut f,Section::Agents,"agents",Some(&id),"ui.saveAgentConfig",json!({"key":"accept_agent","patch":{
        "max_turns":7,"run_seconds":0,"web_search":false,"interactive_shell":false,
        "trigger_run_mode":"serial","trigger_merge_mode":"by_task","trigger_max_parallel":2}}),None,&[],"저장");
    f.reopen_acceptance(false);
    assert_eq!(row(&f,"agents",&id).data["max_turns"],7);
    select(&mut f,Section::Agents,Some(&id));assert_numeric_row(&mut f,"최대 턴","7");
    query(&mut f,Section::Agents,"ui.getAgent",json!({"key":"accept_agent"}),"ACCEPT AGENT UPDATED");
    form(&mut f,Section::Agents,"agents",Some(&id),"ui.createTrigger",Value::Null,
        Some(json!({"triggers":[{"t":trigger("ACCEPT TRIGGER CREATED") }]})),&["자동 실행 조건"],"조건 저장");
    let tid=keyed(&f,"agent_triggers","agent_key","accept_agent").header.id;
    f.reopen_acceptance(false);
    assert_eq!(row(&f,"agent_triggers",&tid).data["task_create_message"],"ACCEPT TRIGGER CREATED");
    select(&mut f,Section::Agents,Some(&id));
    // This is intentionally not seeded after reopen. The production editor
    // must find this agent's saved trigger through its real ownership key.
    if !Fixture::texts(&f.frame(vec![])).iter().any(|s|s=="조건 저장") {f.click_visible("자동 실행 조건");}
    f.assert_drawer_texts(&["ACCEPT TRIGGER CREATED"]);
    form(&mut f,Section::Agents,"agents",Some(&id),"ui.updateTrigger",Value::Null,
        Some(json!({"triggers":[{"id":tid,"t":trigger("ACCEPT TRIGGER UPDATED") }]})),&["자동 실행 조건"],"조건 저장");
    f.reopen_acceptance(false);
    assert_eq!(row(&f,"agent_triggers",&tid).data["task_create_message"],"ACCEPT TRIGGER UPDATED");
    select(&mut f,Section::Agents,Some(&id));
    let output=f.frame(vec![]);
    if !Fixture::texts(&output).iter().any(|s|s=="조건 삭제") {f.click_visible("자동 실행 조건");}
    f.assert_drawer_texts(&["ACCEPT TRIGGER UPDATED"]);
    // Delete the trigger loaded from storage, never a fabricated current row.
    f.click_visible("조건 삭제");f.prepare("ui.deleteTrigger");settle(&mut f);
    f.reopen_acceptance(false);
    assert!(f.app.handle.view().unwrap().generation.get("agent_triggers",&tid).is_none());
    select(&mut f,Section::Agents,Some(&id));
    let out=f.frame(vec![]);
    if !Fixture::texts(&out).iter().any(|s|s=="triggers") {f.click_visible("자동 실행 조건");}
    let displayed=Fixture::texts(&f.frame(vec![]));
    assert!(displayed.iter().any(|s|s=="항목 추가"));
    assert!(!displayed.iter().any(|s|s=="조건 삭제"||s.contains("ACCEPT TRIGGER UPDATED")),"Deleted trigger must disappear from actual editor");
    form(&mut f,Section::Agents,"agents",Some(&id),"ui.deleteAgent",Value::Null,None,
        &["이 항목을 삭제하는 것을 확인합니다."],"삭제");
    f.reopen_acceptance(true);
    assert!(f.app.handle.view().unwrap().generation.get("agents",&id).is_none());
    select(&mut f,Section::Agents,None);
    assert!(!Fixture::texts(&f.frame(vec![])).iter().any(|s|s=="ACCEPT AGENT UPDATED"));
    f.shutdown();
    emit("config_accept_custom_agent_metadata_config_and_trigger_save_reopen_edit_delete",&[
        "ui.createAgent","ui.updateAgent","ui.saveAgentConfig","ui.getAgent","ui.createTrigger","ui.updateTrigger","ui.deleteTrigger","ui.deleteAgent"],
        "Actual agent Forms Save/metadata/config/trigger/delete controls, stored typed identity and ownership, reopened unseeded trigger editor, confirmed deletion and native list/detail display. ui.agents and ui.agentTriggers are native snapshot projection replacements, not invented HTTP invocations.");
    println!("ACCEPT_CONFIG_NATIVE_ADAPTER {}",json!({"case":"config_accept_custom_agent_metadata_config_and_trigger_save_reopen_edit_delete","methods":["ui.agentTriggers"],"scope":"Actual selected agent and automatic-conditions disclosure -> Service stored agent_key ownership -> nonempty saved trigger input rendered after real same-space reopen; actual update/delete operates that loaded condition. Native snapshot form replaces standalone trigger-list RPC."}));
}

#[test]
fn config_repro_trigger_editor_reopen_uses_saved_agent_key() {
    let mut f=fixture();
    let id=f.setup("ui.createAgent",json!({"key":"accept_trigger_owner","name":"ACCEPT TRIGGER OWNER"}))["id"].as_str().unwrap().to_owned();
    f.setup("ui.createTrigger",json!({"key":"accept_trigger_owner","t":trigger("ACCEPT STORED TRIGGER") }));
    let before=keyed(&f,"agent_triggers","agent_key","accept_trigger_owner");
    f.reopen_acceptance(false);
    assert_eq!(row(&f,"agent_triggers",&before.header.id).data["task_create_message"],"ACCEPT STORED TRIGGER");
    select(&mut f,Section::Agents,Some(&id));
    f.click_visible("자동 실행 조건");
    f.assert_drawer_texts(&["ACCEPT STORED TRIGGER"]);
    f.shutdown();
}

#[test]
fn config_accept_agent_prompt_version_preview_and_builtin_reset_reopen() {
    let mut f=fixture();
    let agent=keyed(&f,"agents","key","auto");let id=agent.header.id.clone();
    let old_prompt=row(&f,"agent_prompts",agent.data["current_prompt_id"].as_str().unwrap()).data["template_text"].clone();
    form(&mut f,Section::Agents,"agents",Some(&id),"ui.saveAgentPrompt",Value::Null,
        Some(json!({"prompt":{"text":"ACCEPT PROMPT BODY","blob":null},"note":"ACCEPT PROMPT VERSION"})),
        &["에이전트 프롬프트와 이력"],"프롬프트 저장");
    f.reopen_acceptance(false);
    let saved=row(&f,"agents",&id);let prompt=row(&f,"agent_prompts",saved.data["current_prompt_id"].as_str().unwrap());
    assert_eq!(prompt.data["template_text"]["text"],"ACCEPT PROMPT BODY");
    select(&mut f,Section::Agents,Some(&id));
    f.assert_drawer_texts(&["ACCEPT PROMPT BODY"]);
    query(&mut f,Section::Agents,"ui.agentVariables",json!({"key":"auto"}),"Now");
    let preview=query(&mut f,Section::Agents,"ui.previewAgentPrompt",json!({"key":"auto","template":"ACCEPT PREVIEW {{.Now}}",
        "sample":{"Now":"ACCEPT SAMPLE"}}),"ACCEPT PREVIEW ACCEPT SAMPLE");
    assert!(preview.to_string().contains("ACCEPT SAMPLE"));
    form(&mut f,Section::Agents,"agents",Some(&id),"ui.resetAgentPrompt",Value::Null,None,
        &["에이전트 프롬프트와 이력"],"내장 프롬프트로 복원");
    f.reopen_acceptance(true);
    let reset=row(&f,"agents",&id);let reset_prompt=row(&f,"agent_prompts",reset.data["current_prompt_id"].as_str().unwrap());
    assert_eq!(reset_prompt.data["template_text"],old_prompt);
    assert_eq!(reset_prompt.data["note"],"Revert to built-in default");
    select(&mut f,Section::Agents,Some(&id));
    let history_label=format!("버전 {} · system",reset_prompt.data["version"]);
    if !Fixture::texts(&f.frame(vec![])).iter().any(|s|s==&history_label) {f.click_visible("에이전트 프롬프트와 이력");}
    f.click_visible(&history_label);f.assert_drawer_texts(&["Revert to built-in default"]);
    let retained_version=format!("버전 {} · ui",prompt.data["version"]);
    f.click_visible(&retained_version);f.assert_drawer_texts(&["ACCEPT PROMPT VERSION","ACCEPT PROMPT BODY"]);
    f.shutdown();
    emit("config_accept_agent_prompt_version_preview_and_builtin_reset_reopen",&[
        "ui.saveAgentPrompt","ui.agentVariables","ui.previewAgentPrompt","ui.resetAgentPrompt"],
        "Actual prompt Save/default-reset controls, appended prompt versions and original Body restored after readonly reopen; real variable/preview queries display supplied sample substitution. Native prompt history is read from the saved projection.");
    println!("ACCEPT_CONFIG_NATIVE_ADAPTER {}",json!({"case":"config_accept_agent_prompt_version_preview_and_builtin_reset_reopen","methods":["ui.agentPromptVersions"],"scope":"Actual agent prompt/history disclosure and version selection -> Service saved append-only prompt history -> original Body/default restoration recorded -> readonly reopen and actual selected version note/body displayed. Native snapshot history replaces standalone history-list RPC."}));
}

fn emit(case:&str,methods:&[&str],scope:&str) {
    println!("ACCEPT_CONFIG {}",json!({"case":case,"methods":methods,"connected_path":true,
        "stages":["input","admission","execution","storage","reopen","display"],"scope":scope}));
}

fn rule(name:&str,enabled:bool)->Value {
    json!({"name":name,"enabled":enabled,"priority":3,"match_target":"tool_name","match_type":"string",
        "pattern":"Read","action":"deny","message":"ACCEPT RULE MESSAGE","timeout_enabled":false,
        "timeout_seconds":0,"timeout_action":"deny"})
}

#[test]
fn config_accept_intercept_rule_crud_toggle_and_policy_reopen_display() {
    let mut f=fixture();
    f.setup("ui.createInterceptRule",json!({"rule":rule("ACCEPT PRESERVED RULE",true)}));
    let result=action(&mut f,Section::Approvals,"ui.createInterceptRule",json!({"rule":rule("ACCEPT RULE CREATED",true)}));
    let id=result["id"].as_str().unwrap().to_owned();
    assert_eq!(row(&f,"intercept_rules",&id).data["pattern"],"Read");
    f.reopen_acceptance(false);
    query(&mut f,Section::Approvals,"ui.interceptRules",json!({}),"ACCEPT RULE CREATED");
    action(&mut f,Section::Approvals,"ui.updateInterceptRule",json!({"id":id,"rule":rule("ACCEPT RULE UPDATED",true)}));
    f.reopen_acceptance(false);
    assert_eq!(row(&f,"intercept_rules",&id).data["name"],"ACCEPT RULE UPDATED");
    query(&mut f,Section::Approvals,"ui.interceptRules",json!({}),"ACCEPT RULE UPDATED");
    action(&mut f,Section::Approvals,"ui.toggleInterceptRule",json!({"id":id,"enabled":false}));
    f.reopen_acceptance(false);
    assert_eq!(row(&f,"intercept_rules",&id).data["enabled"],false);
    let displayed=query(&mut f,Section::Approvals,"ui.interceptRules",json!({}),"ACCEPT RULE UPDATED");
    let index=displayed.as_array().unwrap().iter().position(|r|r["id"]==id).unwrap().to_string();
    f.source_result_at_path_visible("ui.interceptRules",None,&[&index,"enabled"],"false");
    action(&mut f,Section::Approvals,"ui.interceptSetToolConfig",json!({"enabledTools":["Read","Write"]}));
    f.reopen_acceptance(false);
    query(&mut f,Section::Approvals,"ui.interceptGetToolConfig",json!({}),"Write");
    let judge=json!({"enabled":false,"profile_id":null,"prompt":"ACCEPT JUDGE CONFIG","timeout_seconds":15,
        "fail_action":"deny","ask_timeout_seconds":20,"ask_timeout_action":"deny"});
    action(&mut f,Section::Approvals,"ui.interceptSetJudgeConfig",json!({"cfg":judge}));
    f.reopen_acceptance(false);
    let loaded=query(&mut f,Section::Approvals,"ui.interceptGetJudgeConfig",json!({}),"ACCEPT JUDGE CONFIG");
    assert_eq!(loaded["enabled"],false);assert_eq!(loaded["fail_action"],"deny");
    assert!(loaded["profile_id"].is_null());
    f.source_result_at_path_visible("ui.interceptGetJudgeConfig",None,&["enabled"],"false");
    action(&mut f,Section::Approvals,"ui.deleteInterceptRule",json!({"id":id}));
    f.reopen_acceptance(true);
    assert!(f.app.handle.view().unwrap().generation.get("intercept_rules",&id).is_none());
    let rules=query(&mut f,Section::Approvals,"ui.interceptRules",json!({}),"ACCEPT PRESERVED RULE");
    assert!(!rules.to_string().contains("ACCEPT RULE UPDATED"));
    assert!(!Fixture::texts(&f.frame(vec![])).iter().any(|s|s.contains("ACCEPT RULE UPDATED")));
    f.shutdown();
    emit("config_accept_intercept_rule_crud_toggle_and_policy_reopen_display",&[
        "ui.createInterceptRule","ui.interceptRules","ui.updateInterceptRule","ui.toggleInterceptRule",
        "ui.interceptSetToolConfig","ui.interceptGetToolConfig","ui.interceptSetJudgeConfig","ui.interceptGetJudgeConfig",
        "ui.deleteInterceptRule"],"Configured tool-name rule and disabled judge settings; actual source-action widgets/Prepare, committed records, repeated actual list display after reopen. Rule deletion is confirmed in the saved snapshot; empty-rule list is not used as a nonempty query case.");
}

#[test]
fn config_accept_asset_rule_crud_toggle_reopen_and_display() {
    let mut f=fixture();
    f.setup("ui.createAssetInterceptRule",json!({"rule":{"kind":"exact_domain","enabled":true,
        "pattern":"preserved.fixture.invalid","note":"ACCEPT PRESERVED ASSET RULE"}}));
    let rule=json!({"kind":"exact_domain","enabled":true,"pattern":"fixture.invalid","note":"ACCEPT ASSET RULE"});
    let created=action(&mut f,Section::Approvals,"ui.createAssetInterceptRule",json!({"rule":rule}));
    let id=created["id"].as_str().unwrap().to_owned();
    f.reopen_acceptance(false);
    query(&mut f,Section::Approvals,"ui.assetInterceptRules",json!({}),"ACCEPT ASSET RULE");
    action(&mut f,Section::Approvals,"ui.updateAssetInterceptRule",json!({"id":id,"rule":{
        "kind":"exact_domain","enabled":true,"pattern":"updated.fixture.invalid","note":"ACCEPT ASSET UPDATED"}}));
    f.reopen_acceptance(false);
    query(&mut f,Section::Approvals,"ui.assetInterceptRules",json!({}),"updated.fixture.invalid");
    action(&mut f,Section::Approvals,"ui.toggleAssetInterceptRule",json!({"id":id,"enabled":false}));
    f.reopen_acceptance(false);
    assert_eq!(row(&f,"asset_intercept_rules",&id).data["enabled"],false);
    let displayed=query(&mut f,Section::Approvals,"ui.assetInterceptRules",json!({}),"ACCEPT ASSET UPDATED");
    let index=displayed.as_array().unwrap().iter().position(|r|r["id"]==id).unwrap().to_string();
    f.source_result_at_path_visible("ui.assetInterceptRules",None,&[&index,"enabled"],"false");
    let before_delete=f.app.handle.view().unwrap().generation.snapshot.clone();
    let deletion=std::panic::catch_unwind(std::panic::AssertUnwindSafe(||
        action(&mut f,Section::Approvals,"ui.deleteAssetInterceptRule",json!({"id":id}))));
    if let Err(error)=deletion {
        // A rejected post-planning result must not secretly remove the row or
        // write a success receipt. Preserve this proof before propagating the
        // original failure; the test never accepts that failure as success.
        let after=f.app.handle.view().unwrap().generation.snapshot.clone();
        assert_eq!(after.commit_seq,before_delete.commit_seq);
        assert_eq!(after.collections,before_delete.collections);
        println!("ACCEPT_REPRO asset-delete contract failure leaves exact Store generation/objects unchanged");
        std::panic::resume_unwind(error);
    }
    f.reopen_acceptance(true);
    assert!(f.app.handle.view().unwrap().generation.get("asset_intercept_rules",&id).is_none());
    let rules=query(&mut f,Section::Approvals,"ui.assetInterceptRules",json!({}),"ACCEPT PRESERVED ASSET RULE");
    assert!(!rules.to_string().contains("ACCEPT ASSET UPDATED"));
    assert!(!Fixture::texts(&f.frame(vec![])).iter().any(|s|s.contains("ACCEPT ASSET UPDATED")));
    f.shutdown();
    emit("config_accept_asset_rule_crud_toggle_reopen_and_display",&["ui.createAssetInterceptRule","ui.assetInterceptRules",
        "ui.updateAssetInterceptRule","ui.toggleAssetInterceptRule","ui.deleteAssetInterceptRule"],
        "Actual source-action rule CRUD/toggle, nonempty exact-domain/note display, durable updates and confirmed deletion after reopen; no external target contacted.");
}

#[test]
fn config_accept_retry_policy_source_editor_reopens_exact_layers() {
    let mut f=fixture();
    let layer=json!({"attempts":2,"interval_ms":25});
    let policy=json!({"breaker":layer,"intent":layer,"connect":layer,"empty":layer,"stream":layer});
    action(&mut f,Section::Models,"ui.saveLLMRetryPolicy",json!({"p":policy}));
    f.reopen_acceptance(true);
    let value=query(&mut f,Section::Models,"ui.llmRetryPolicy",json!({}),"25");
    for layer in ["breaker","intent","connect","empty","stream"] {
        assert_eq!(value[layer]["attempts"],2);assert_eq!(value[layer]["interval_ms"],25);
        f.source_result_at_path_visible("ui.llmRetryPolicy",None,&[layer,"attempts"],"2");
        f.source_result_at_path_visible("ui.llmRetryPolicy",None,&[layer,"interval_ms"],"25");
    }
    f.shutdown();
    emit("config_accept_retry_policy_source_editor_reopens_exact_layers",&["ui.saveLLMRetryPolicy","ui.llmRetryPolicy"],
        "Real source-action Save with all five named layers, CAS/Service/global settings persistence, read-only reopen and actual query rendering. Numeric values are isolated fixture configuration, not product defaults or timeout changes.");
}

#[test]
fn config_accept_builtin_tool_editor_save_reset_reopen() {
    let mut f=fixture();
    let tool=keyed(&f,"tools","key","Read");
    let original=tool.data.clone();
    form(&mut f,Section::Tools,"tools",Some(&tool.header.id),"ui.saveTool",json!({"key":"Read","patch":{
        "description":"ACCEPT READ DESCRIPTION","enabled":true,"schema":original["schema"],"agents":original["agents"]}}),None,&[],"저장");
    f.reopen_acceptance(false);
    assert_eq!(row(&f,"tools",&tool.header.id).data["description"],"ACCEPT READ DESCRIPTION");
    select(&mut f,Section::Tools,Some(&tool.header.id));
    f.assert_drawer_texts(&["ACCEPT READ DESCRIPTION"]);
    form(&mut f,Section::Tools,"tools",Some(&tool.header.id),"ui.resetTool",Value::Null,None,&[],"내장 도구 정의로 복원");
    f.reopen_acceptance(true);
    assert_eq!(row(&f,"tools",&tool.header.id).data["description"],original["description"]);
    select(&mut f,Section::Tools,Some(&tool.header.id));
    f.assert_drawer_texts(&[original["description"].as_str().unwrap()]);
    f.shutdown();
    emit("config_accept_builtin_tool_editor_save_reset_reopen",&["ui.saveTool","ui.resetTool"],
        "Actual built-in Read editor Save and reset buttons with original schema/bindings preserved, stored descriptions and read-only reopened native form display. This does not newly qualify Read execution.");
}

#[test]
fn config_accept_custom_tool_editor_crud_preserves_saved_definition() {
    let mut f=fixture();
    let definition=json!({"key":"accept_custom","kind":"command","enabled":true,"description":"ACCEPT CUSTOM CREATED",
        "agents":["auto"],"schema":{"type":"object","properties":{}},"exec":{"command":"Write-Output ACCEPT_CUSTOM_EFFECT"},"deferred":false});
    form(&mut f,Section::Tools,"tools",None,"ui.createCustomTool",json!({"t":definition}),None,&[],"저장");
    let id=keyed(&f,"tools","key","accept_custom").header.id;
    f.reopen_acceptance(false);
    select(&mut f,Section::Tools,Some(&id));f.assert_drawer_texts(&["ACCEPT CUSTOM CREATED"]);
    let mut changed=definition.clone();changed["description"]=json!("ACCEPT CUSTOM UPDATED");
    form(&mut f,Section::Tools,"tools",Some(&id),"ui.updateCustomTool",json!({"key":"accept_custom","t":changed}),None,&[],"저장");
    f.reopen_acceptance(false);
    assert_eq!(row(&f,"tools",&id).data["exec"],definition["exec"]);
    select(&mut f,Section::Tools,Some(&id));f.assert_drawer_texts(&["ACCEPT CUSTOM UPDATED"]);
    form(&mut f,Section::Tools,"tools",Some(&id),"ui.deleteCustomTool",Value::Null,None,
        &["이 항목을 삭제하는 것을 확인합니다."],"삭제");
    f.reopen_acceptance(true);
    assert!(f.app.handle.view().unwrap().generation.get("tools",&id).is_none());
    select(&mut f,Section::Tools,None);
    assert!(!Fixture::texts(&f.frame(vec![])).iter().any(|s|s=="accept_custom"||s=="ACCEPT CUSTOM UPDATED"));
    let preserved=keyed(&f,"tools","key","Read");left_row(&mut f,Section::Tools,&preserved.header.id,"Read");
    f.shutdown();
    emit("config_accept_custom_tool_editor_crud_preserves_saved_definition",&["ui.createCustomTool","ui.updateCustomTool","ui.deleteCustomTool"],
        "Actual custom-tool form Save/update/confirmed Delete, preserved command/schema/agent binding, read-only reopen and native form/list display. Command execution is not claimed by this definition-only case.");
}

struct McpFixture {
    url:String,
    seen:Arc<std::sync::Mutex<Vec<Value>>>,
    stop:Arc<std::sync::atomic::AtomicBool>,
    thread:Option<std::thread::JoinHandle<()>>,
}
impl McpFixture {
    fn new()->Self {
        use std::{io::{Read,Write},net::TcpListener,sync::{Mutex,atomic::{AtomicBool,Ordering}}};
        let listener=TcpListener::bind("127.0.0.1:0").unwrap();listener.set_nonblocking(true).unwrap();
        let url=format!("http://{}/mcp",listener.local_addr().unwrap());
        let seen=Arc::new(Mutex::new(Vec::new()));let captured=seen.clone();
        let stop=Arc::new(AtomicBool::new(false));let stopped=stop.clone();
        let thread=std::thread::spawn(move || {
            while !stopped.load(Ordering::Acquire) {
                let mut socket=match listener.accept() {
                    Ok((s,_))=>s,
                    Err(e) if e.kind()==std::io::ErrorKind::WouldBlock=>{std::thread::sleep(Duration::from_millis(3));continue;},
                    Err(e)=>panic!("{e}"),
                };
                socket.set_nonblocking(false).unwrap();socket.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
                let mut headers=Vec::new();let mut byte=[0];
                loop {
                    match socket.read_exact(&mut byte) {Ok(())=>headers.push(byte[0]),Err(_)=>break}
                    if headers.ends_with(b"\r\n\r\n") {break;}
                    assert!(headers.len()<32768);
                }
                if !headers.ends_with(b"\r\n\r\n") {continue;}
                let headers=String::from_utf8(headers).unwrap();
                let length=headers.lines().find_map(|line|line.to_ascii_lowercase().strip_prefix("content-length:")
                    .and_then(|v|v.trim().parse::<usize>().ok())).unwrap_or(0);
                assert!(length<2*1024*1024);
                let mut bytes=vec![0;length];socket.read_exact(&mut bytes).unwrap();
                let is_get=headers.starts_with("GET ");
                let request:Value=if is_get {json!({"http_path":headers.lines().next().unwrap().split_whitespace().nth(1).unwrap()})}
                    else {serde_json::from_slice(&bytes).unwrap()};
                captured.lock().unwrap().push(request.clone());
                let method=request["method"].as_str().unwrap_or("");
                let (status,result)=if is_get {
                    assert!(request["http_path"].as_str().unwrap().ends_with("/models"),"Only explicit model-discovery GET fixture is admitted");
                    assert!(headers.lines().any(|line|line.eq_ignore_ascii_case("authorization: Bearer accept-model-discovery-placeholder")),
                        "Model discovery must send the explicitly admitted synthetic bearer credential");
                    (200,json!({"data":[{"id":"ACCEPT_MODEL_DISCOVERED"},{"id":"ACCEPT_MODEL_SECONDARY"}]}))
                } else if request.get("jsonrpc").is_none() {
                    (200,json!({"echo":"ACCEPT CUSTOM HTTP RESULT","received":request}))
                } else if request.get("id").is_none() {
                    (202,Value::Null)
                } else {
                    let result=match method {
                        "initialize"=>json!({"protocolVersion":"2024-11-05","capabilities":{"tools":{}},
                            "serverInfo":{"name":"acceptance-fixture","version":"1"}}),
                        "tools/list"=>{
                            let tools=["accept_echo","list_projects_data","list_tasks","get_project","list_assets"].iter()
                                .map(|name|json!({"name":name,"description":"ACCEPT MCP TOOL","inputSchema":{"type":"object","properties":{}}})).collect::<Vec<_>>();
                            json!({"tools":tools})
                        },
                        "tools/call"=>{
                            let args=&request["params"]["arguments"];
                            let value=match request["params"]["name"].as_str().unwrap() {
                                "list_projects_data"=>json!({"result":{"All":[{"id":"accept-project","name":"ACCEPT PROJECT","unexpected":"must not leak"}]},"tag":{"all":1}}),
                                // Remote SSTask.status is numeric, unlike the
                                // local agent-run state strings.
                                "list_tasks"=>json!({"list":[{"id":"accept-task","name":"ACCEPT REMOTE TASK","status":2}]}),
                                "get_project"=>json!({"name":"ACCEPT SYNC COMPANY","target":"fixture.invalid"}),
                                "list_assets"=>if args["asset_type"]=="subdomain" {
                                    json!({"list":[{"host":"synced.fixture.invalid","type":"A","ip":[],"value":["203.0.113.7"]}]})
                                } else {json!({"list":[]})},
                                "accept_echo"=>json!({"message":"ACCEPT MCP EFFECT","arguments":args}),
                                other=>panic!("Unscripted actual MCP tool: {other}"),
                            };
                            json!({"content":[{"type":"text","text":value.to_string()}],"isError":false})
                        },
                        other=>panic!("Unscripted actual MCP method: {other}"),
                    };
                    (200,json!({"jsonrpc":"2.0","id":request["id"],"result":result}))
                };
                let bytes=if status==202 {Vec::new()} else {serde_json::to_vec(&result).unwrap()};
                let _=write!(socket,"HTTP/1.1 {status} Fixture\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",bytes.len());
                let _=socket.write_all(&bytes);
            }
        });
        Self{url,seen,stop,thread:Some(thread)}
    }
    fn finish(mut self)->Vec<Value> {
        self.stop.store(true,std::sync::atomic::Ordering::Release);
        self.thread.take().unwrap().join().unwrap();self.seen.lock().unwrap().clone()
    }
}
impl Drop for McpFixture {
    fn drop(&mut self) {
        self.stop.store(true,std::sync::atomic::Ordering::Release);
        if let Some(thread)=self.thread.take() {let _=thread.join();}
    }
}

fn mcp_definition(name:&str,url:&str)->Value {
    json!({"name":name,"transport":"http","url":url,"enabled":true,"env":{},"insecure":false})
}

#[test]
fn config_accept_mcp_forms_refresh_tools_visibility_and_delete_reopen() {
    let server=McpFixture::new();let mut f=fixture();
    let mut preserved=mcp_definition("ACCEPT MCP PRESERVED",&server.url);preserved["enabled"]=json!(false);
    let preserved_id=f.setup("ui.saveMcpServer",json!({"m":preserved}))["id"].as_str().unwrap().to_owned();
    let control_agent=keyed(&f,"agents","key","worker").header.id;
    f.setup("ui.toggleVisibility",json!({"agentId":control_agent,"kind":"mcp","resourceId":preserved_id,"visible":true}));
    form(&mut f,Section::Mcp,"mcp_servers",None,"ui.saveMcpServer",json!({"m":mcp_definition("ACCEPT MCP SERVER",&server.url)}),None,&[],"저장");
    let id=keyed(&f,"mcp_servers","name","ACCEPT MCP SERVER").header.id;
    f.reopen_acceptance(false);
    left_row(&mut f,Section::Mcp,&id,"ACCEPT MCP SERVER");
    f.assert_drawer_texts(&[&server.url]);
    let mut changed=mcp_definition("ACCEPT MCP UPDATED",&server.url);changed["id"]=json!(id);
    form(&mut f,Section::Mcp,"mcp_servers",Some(&id),"ui.saveMcpServer",json!({"m":changed}),None,&[],"저장");
    action(&mut f,Section::Mcp,"ui.refreshMcpServer",json!({"id":id}));
    f.reopen_acceptance(false);
    query(&mut f,Section::Mcp,"ui.mcpTools",json!({"id":id}),"accept_echo");
    let auto=keyed(&f,"agents","key","auto").header.id;
    let worker=keyed(&f,"agents","key","worker").header.id;
    f.setup("ui.toggleVisibility",json!({"agentId":worker,"kind":"mcp","resourceId":id,"visible":true}));
    action(&mut f,Section::Mcp,"ui.toggleVisibility",json!({"agentId":auto,"kind":"mcp","resourceId":id,"visible":true}));
    f.reopen_acceptance(false);
    query(&mut f,Section::Mcp,"ui.resourceVisibility",json!({"kind":"mcp","id":id}),&auto);
    action(&mut f,Section::Mcp,"ui.toggleVisibility",json!({"agentId":auto,"kind":"mcp","resourceId":id,"visible":false}));
    f.reopen_acceptance(false);
    let remaining=query(&mut f,Section::Mcp,"ui.resourceVisibility",json!({"kind":"mcp","id":id}),&worker);
    assert!(!remaining.as_array().unwrap().contains(&json!(auto)));
    action(&mut f,Section::Mcp,"ui.setAgentVisibility",json!({"key":"auto","mcp":[id],"skill":[]}));
    f.reopen_acceptance(false);
    let visibility=query(&mut f,Section::Mcp,"ui.getAgentVisibility",json!({"key":"auto"}),&id);
    assert!(visibility["mcp"].as_array().unwrap().contains(&json!(id)));
    form(&mut f,Section::Mcp,"mcp_servers",Some(&id),"ui.deleteMcpServer",Value::Null,None,
        &["이 항목을 삭제하는 것을 확인합니다."],"삭제");
    // A later shared-profile projection must not hide an orphan left by this
    // command. Inspect the real immediately published workspace generation.
    let immediate=f.app.handle.view().unwrap();
    let immediate_dangling=immediate.generation.records("agent_visibility").into_iter().filter(|r|r.data["resource_kind"]=="mcp"&&r.data["resource_id"]==id).collect::<Vec<_>>();
    assert!(immediate_dangling.is_empty(),"Immediate MCP deletion left dangling grants before reopen: {immediate_dangling:?}");
    assert!(immediate.generation.records("agent_visibility").iter().any(|r|r.data["resource_id"]==preserved_id&&r.data["agent_id"]==control_agent&&r.data["enabled"]==true));
    f.reopen_acceptance(true);
    assert!(f.app.handle.view().unwrap().generation.get("mcp_servers",&id).is_none());
    let view=f.app.handle.view().unwrap();
    assert!(view.generation.records("agent_visibility").iter().any(|r|r.data["resource_id"]==preserved_id&&r.data["agent_id"]==control_agent&&r.data["enabled"]==true),
        "Deleting one MCP must preserve the unrelated server grant");
    assert!(!view.generation.records("mcp_tools_cache").iter().any(|r|r.data["server_id"]==id),"Deleted MCP cache must cascade");
    let dangling=view.generation.records("agent_visibility").into_iter().filter(|r|r.data["resource_kind"]=="mcp"&&r.data["resource_id"]==id).collect::<Vec<_>>();
    assert!(dangling.is_empty(),"Deleted MCP must remove its polymorphic grants, found: {dangling:?}");
    select(&mut f,Section::Mcp,None);
    assert!(!Fixture::texts(&f.frame(vec![])).iter().any(|s|s.contains("ACCEPT MCP UPDATED")));
    let preserved=keyed(&f,"mcp_servers","name","ACCEPT MCP PRESERVED");
    left_row(&mut f,Section::Mcp,&preserved.header.id,"ACCEPT MCP PRESERVED");
    f.shutdown();
    let requests=server.finish();
    assert!(requests.iter().any(|r|r["method"]=="tools/list"));
    emit("config_accept_mcp_forms_refresh_tools_visibility_and_delete_reopen",&[
        "ui.saveMcpServer","ui.refreshMcpServer","ui.mcpTools","ui.toggleVisibility","ui.resourceVisibility",
        "ui.setAgentVisibility","ui.getAgentVisibility","ui.deleteMcpServer"],
        "Actual HTTP MCP protocol over loopback through NativeGateway, Forms Save/update/delete, cache refresh, independent agent visibility toggles/full replacement, persisted reopened configuration and actual tool/visibility display. No real external MCP server or user credential.");
}

#[test]
fn config_accept_native_configuration_lists_preserve_actual_selected_entities_after_reopen() {
    let server=McpFixture::new();let mut f=fixture();
    f.setup("ui.createAgent",json!({"key":"accept_list_agent","name":"ACCEPT LIST AGENT","description":"ACCEPT LIST AGENT DETAIL"}));
    f.setup("ui.createCustomTool",json!({"t":{"key":"accept_list_tool","kind":"command","enabled":true,
        "description":"ACCEPT LIST TOOL DETAIL","agents":["auto"],"schema":{"type":"object","properties":{}},
        "exec":{"command":"Write-Output ACCEPT_LIST_TOOL"},"deferred":false}}));
    let mut mcp=mcp_definition("ACCEPT LIST MCP",&server.url);mcp["enabled"]=json!(false);
    f.setup("ui.saveMcpServer",json!({"m":mcp}));
    f.setup("ui.saveLLMProfile",json!({"p":{"name":"ACCEPT LIST MODEL","model":"fixture","format":"openai",
        "base_url":server.url,"api_key":{"secret_ref":artex_core::new_id("secret"),"hint":"fixture only"}}}));
    f.setup("ui.notifyCreateChannel",json!({"payload":{"name":"ACCEPT LIST NOTIFICATION","kind":"email","enabled":false,
        "mode":"realtime","config":{"host":"127.0.0.1","port":25,"from":"from@fixture.invalid","to":["to@fixture.invalid"]},
        "filter":{},"rate_per_min":0}}));
    f.setup("ui.setSettings",json!({"patch":{"workers":3,"notify_public_base_url":"https://fixture.invalid/ACCEPT_SETTINGS"}}));
    let subjects=[
        (Section::Agents,"agents","key","accept_list_agent","ACCEPT LIST AGENT","ACCEPT LIST AGENT DETAIL"),
        (Section::Tools,"tools","key","accept_list_tool","accept_list_tool","ACCEPT LIST TOOL DETAIL"),
        (Section::Mcp,"mcp_servers","name","ACCEPT LIST MCP","ACCEPT LIST MCP",server.url.as_str()),
        (Section::Models,"llm_profiles","name","ACCEPT LIST MODEL","ACCEPT LIST MODEL","fixture"),
        (Section::Notifications,"notification_channels","name","ACCEPT LIST NOTIFICATION","ACCEPT LIST NOTIFICATION","127.0.0.1"),
    ];
    let stored=subjects.iter().map(|(_,col,field,key,_,_)|keyed(&f,col,field,key)).collect::<Vec<_>>();
    for reopening in [false,true] {
        if reopening {f.reopen_acceptance(true);}
        for ((section,col,_,_,label,detail),expected) in subjects.iter().zip(&stored) {
            assert_eq!(row(&f,col,&expected.header.id).data,expected.data);
            left_row(&mut f,*section,&expected.header.id,label);
            if *section==Section::Agents {
                let out=f.frame(vec![]);
                if !Fixture::texts(&out).iter().any(|s|s=="이름과 설명 저장") {f.click_visible("에이전트 이름과 설명");}
            }
            f.assert_drawer_texts(&[detail]);
        }
        select(&mut f,Section::Settings,None);
        settle(&mut f);
        assert_eq!(keyed(&f,"settings","key","workers").data["value"],"3");
        f.assert_drawer_texts(&["https://fixture.invalid/ACCEPT_SETTINGS"]);
    }
    f.shutdown();server.finish();
    // These are explicit native projection replacements: no absent REST-style
    // query button or ui.* RPC call is invented. The actual row click reads the
    // live Service generation and selects the persisted native editor.
    println!("ACCEPT_CONFIG_NATIVE_ADAPTER {}",json!({"case":"config_accept_native_configuration_lists_preserve_actual_selected_entities_after_reopen",
        "methods":["ui.agents","ui.tools","ui.mcpServers","ui.llmProfiles","ui.notifyChannels","ui.settings"],
        "scope":"Actual native list row input -> Service read generation -> correct stored entity/editor -> readonly reopen and actual list/detail/settings display. Query transport intentionally replaced by native read projection; ui.settings uses the production automatic settings projection. No legacy RPC invocation is asserted."}));
}

#[test]
fn config_accept_custom_tool_http_test_shows_actual_network_result_and_reopen() {
    let server=McpFixture::new();let mut f=fixture();
    let input=json!({"body":{"kind":"http","exec":{"url":server.url,"method":"POST","body":"{\"test\":\"ACCEPT HTTP INPUT\"}"},"params":{}}});
    let value=action(&mut f,Section::Tools,"ui.testCustomTool",input.clone());
    assert!(value.to_string().contains("ACCEPT CUSTOM HTTP RESULT"));
    f.source_result_visible("ui.testCustomTool",None,"ACCEPT CUSTOM HTTP RESULT");
    f.reopen_acceptance(false);
    let again=action(&mut f,Section::Tools,"ui.testCustomTool",input);
    assert!(again.to_string().contains("ACCEPT CUSTOM HTTP RESULT"));
    f.source_result_visible("ui.testCustomTool",None,"ACCEPT CUSTOM HTTP RESULT");
    f.shutdown();
    let requests=server.finish();assert_eq!(requests.iter().filter(|r|r["test"]=="ACCEPT HTTP INPUT").count(),2);
    emit("config_accept_custom_tool_http_test_shows_actual_network_result_and_reopen",&["ui.testCustomTool"],
        "Actual source Test input invokes native HTTP custom tool and loopback server receives intended body twice across reopen; real result shown. Does not pretend an arbitrary command or Python script executed.");
}

#[test]
fn config_accept_scopesentry_native_controls_import_real_mcp_assets_reopen() {
    let server=McpFixture::new();let mut f=fixture();
    let mut definition=mcp_definition("ScopeSentry",&server.url);
    // This is a credentialless local protocol fixture. The harmless header
    // makes the actual configured predicate true without touching an OS vault.
    definition["env"]=json!({"ACCEPT_HEADER":"fixture"});
    form(&mut f,Section::Mcp,"mcp_servers",None,"ui.saveMcpServer",json!({"m":definition}),None,&[],"저장");
    select(&mut f,Section::Sync,None);
    f.click_visible("연결 설정");
    f.click_visible("연결 주소 저장 / 데이터 소스 만들기");
    f.prepare("ui.ssDatasource");settle(&mut f);
    f.reopen_acceptance(false);
    let stored=keyed(&f,"mcp_servers","name","ScopeSentry");
    assert_eq!(stored.data["url"],server.url);assert_eq!(stored.data["enabled"],true);
    select(&mut f,Section::Sync,None);
    f.click_visible("연결 상태 조회");settle(&mut f);
    f.assert_drawer_texts(&["설정 true · 활성 true · 연결 true"]);
    f.click_visible("대상 조회");settle(&mut f);
    f.assert_drawer_texts(&["ACCEPT PROJECT · accept-project"]);
    assert!(!f.app.panes.projection["ui.ssProjects"].to_string().contains("unexpected"),"Remote project fields must pass the production allowlist");
    f.click_visible("project");f.click_visible("task");
    f.click_visible("대상 조회");settle(&mut f);
    f.assert_drawer_texts(&["ACCEPT REMOTE TASK · accept-task"]);
    assert_eq!(f.app.panes.projection["ui.ssTasks"][0]["status"],2);
    f.click_visible("task");f.click_visible("project");
    f.click_visible("대상 조회");settle(&mut f);
    f.click_visible("ACCEPT PROJECT · accept-project");
    f.assert_drawer_texts(&["선택한 대상 1개"]);
    f.click_visible("선택한 대상의 실제 자산 동기화 실행");
    f.prepare("ui.ssSync");
    f.until(|app|app.pending.is_empty()&&app.handle.view().unwrap().generation.records("assets").iter()
        .any(|r|r.data["type"]=="subdomain"&&r.data["domain"]=="synced.fixture.invalid"));
    assert!(f.app.error.is_none(),"{:?}",f.app.error);
    f.assert_drawer_texts(&["동기화 결과","subdomain: 1","생성된 회사: ACCEPT SYNC COMPANY"]);
    let asset=f.app.handle.view().unwrap().generation.records("assets").into_iter()
        .find(|r|r.data["type"]=="subdomain"&&r.data["domain"]=="synced.fixture.invalid").unwrap().clone();
    let company=keyed(&f,"companies","name","ACCEPT SYNC COMPANY");
    assert_eq!(asset.data["company_id"],company.header.id);
    f.reopen_acceptance(true);
    assert_eq!(row(&f,"assets",&asset.header.id).data,asset.data);
    left_row(&mut f,Section::Assets,&asset.header.id,"synced.fixture.invalid");
    f.assert_drawer_texts(&["synced.fixture.invalid"]);
    // Remote MCP tools may have effects, so readonly mode keeps denying them.
    // Prove saved imported evidence there, then explicitly reopen for queries.
    f.reopen_acceptance(false);
    select(&mut f,Section::Sync,None);f.click_visible("연결 상태 조회");settle(&mut f);
    f.assert_drawer_texts(&["설정 true · 활성 true · 연결 true"]);
    f.click_visible("대상 조회");settle(&mut f);
    f.assert_drawer_texts(&["ACCEPT PROJECT · accept-project"]);
    f.click_visible("project");f.click_visible("task");f.click_visible("대상 조회");settle(&mut f);
    f.assert_drawer_texts(&["ACCEPT REMOTE TASK · accept-task"]);
    f.shutdown();let requests=server.finish();
    for tool in ["list_projects_data","list_tasks","get_project","list_assets"] {
        assert!(requests.iter().any(|r|r["method"]=="tools/call"&&r["params"]["name"]==tool));
    }
    assert!(requests.iter().any(|r|r["params"]["name"]=="list_assets"&&r["params"]["arguments"]["filter"]["project"]==json!(["accept-project"])));
    emit("config_accept_scopesentry_native_controls_import_real_mcp_assets_reopen",&[
        "ui.ssDatasource","ui.ssStatus","ui.ssProjects","ui.ssTasks","ui.ssSync"],
        "Actual native Sync controls, original datasource/sync Prepare and CAS, real HTTP MCP protocol with project/task listing and normalized asset/company import, SQLite readonly reopen and actual imported asset/list/status display. Credentialless loopback fixture only; no third-party account/authentication assertion.");
}

fn open_skill(f:&mut Fixture,name:&str) {
    select(f,Section::Skills,None);settle(f);
    assert!(f.app.skill_catalog.iter().any(|v|v["name"]==name),"Actual native skill catalog lacks {name}");
    left_row(f,Section::Skills,name,name);settle(f);
    let out=f.frame(vec![]);
    if !Fixture::texts(&out).iter().any(|s|s=="파일 목록 다시 불러오기") {f.click_visible("스킬 파일");}
    settle(f);
}

fn save_actual_skill_file(f:&mut Fixture,content:&str) {
    f.app.forms.seed_skill_file_inputs_for_app_test(None,Some(content));
    f.click_visible("파일 저장");
    assert!(f.app.pending.values().any(|p|matches!(p,Purpose::SkillFileSave{..})),"Must submit native file CAS action");
    settle(f);
}

#[test]
fn config_accept_skill_native_create_file_cas_directories_metadata_visibility_delete_reopen() {
    let mut f=fixture();let name="accept-skill";
    f.setup("ui.createSkill",json!({"s":{"name":"accept-preserved-skill","description":"ACCEPT PRESERVED SKILL",
        "license":"fixture","compatibility":"native acceptance","mcps":[],"instructions":{"text":"ACCEPT PRESERVED INSTRUCTIONS","blob":null}}}));
    let preserved_agent=keyed(&f,"agents","key","worker").header.id;
    f.setup("ui.toggleSkillVisibility",json!({"agentId":preserved_agent,"skillName":"accept-preserved-skill","visible":true}));
    form(&mut f,Section::Skills,"skills",None,"ui.createSkill",json!({"s":{"name":name,
        "description":"ACCEPT SKILL DESCRIPTION","license":"fixture","compatibility":"native acceptance",
        "mcps":[],"instructions":{"text":"ACCEPT SKILL INSTRUCTIONS","blob":null}}}),None,&[],"저장");
    let root=f.app.handle.view().unwrap().workspace.resources_root.join("skills").join(name);
    let original=std::fs::read_to_string(root.join("SKILL.md")).unwrap();
    assert!(original.contains("ACCEPT SKILL INSTRUCTIONS"));
    f.reopen_acceptance(false);open_skill(&mut f,name);
    f.assert_drawer_texts(&["ACCEPT SKILL DESCRIPTION"]);
    f.click_visible("SKILL.md");settle(&mut f);
    f.assert_drawer_texts(&["ACCEPT SKILL INSTRUCTIONS"]);
    let updated=original.replace("ACCEPT SKILL INSTRUCTIONS","ACCEPT SKILL FILE UPDATED");
    save_actual_skill_file(&mut f,&updated);
    assert_eq!(std::fs::read_to_string(root.join("SKILL.md")).unwrap(),updated);
    f.reopen_acceptance(false);open_skill(&mut f,name);f.click_visible("SKILL.md");settle(&mut f);
    f.assert_drawer_texts(&["ACCEPT SKILL FILE UPDATED"]);
    action(&mut f,Section::Skills,"ui.createSkillDir",json!({"skill":name,"path":"notes"}));
    assert!(root.join("notes").is_dir());
    // Original SHA/create flags come from the actual file picker and read.
    // Only user-editable path/content inputs are supplied by the fixture.
    open_skill(&mut f,name);
    f.app.forms.seed_skill_file_inputs_for_app_test(Some("notes/entry.txt"),None);
    f.click_visible("새 파일 편집");
    save_actual_skill_file(&mut f,"ACCEPT SKILL NESTED FILE");
    assert_eq!(std::fs::read_to_string(root.join("notes/entry.txt")).unwrap(),"ACCEPT SKILL NESTED FILE");
    f.reopen_acceptance(false);
    query(&mut f,Section::Skills,"ui.skillFiles",json!({"name":name}),"notes/entry.txt");
    open_skill(&mut f,name);f.click_visible("notes/entry.txt");settle(&mut f);
    f.assert_drawer_texts(&["ACCEPT SKILL NESTED FILE"]);
    // Metadata is edited only after the native read supplies its immutable SHA.
    f.app.forms.patch_active_params_for_app_test(json!({"meta":{"description":"ACCEPT SKILL META UPDATED"}}));
    f.click_visible("저장");
    assert!(f.app.pending.values().any(|p|matches!(p,Purpose::FormsSave{..})),"Native metadata CAS must be submitted");
    settle(&mut f);
    assert!(std::fs::read_to_string(root.join("SKILL.md")).unwrap().contains("ACCEPT SKILL META UPDATED"));
    assert!(std::fs::read_to_string(root.join("SKILL.md")).unwrap().contains("ACCEPT SKILL FILE UPDATED"));
    f.reopen_acceptance(false);open_skill(&mut f,name);
    f.assert_drawer_texts(&["ACCEPT SKILL META UPDATED"]);
    let auto=keyed(&f,"agents","key","auto").header.id;
    let worker=keyed(&f,"agents","key","worker").header.id;
    f.setup("ui.toggleSkillVisibility",json!({"agentId":worker,"skillName":name,"visible":true}));
    action(&mut f,Section::Skills,"ui.toggleSkillVisibility",json!({"agentId":auto,"skillName":name,"visible":true}));
    f.reopen_acceptance(false);query(&mut f,Section::Skills,"ui.skillVisibility",json!({"name":name}),&auto);
    action(&mut f,Section::Skills,"ui.toggleSkillVisibility",json!({"agentId":auto,"skillName":name,"visible":false}));
    f.reopen_acceptance(false);
    let visible=query(&mut f,Section::Skills,"ui.skillVisibility",json!({"name":name}),&worker);
    assert!(!visible.as_array().unwrap().contains(&json!(auto)));
    action(&mut f,Section::Skills,"ui.deleteSkillPath",json!({"skill":name,"path":"notes"}));
    assert!(!root.join("notes").exists());
    f.reopen_acceptance(false);
    let files=query(&mut f,Section::Skills,"ui.skillFiles",json!({"name":name}),"SKILL.md");
    assert!(!files.to_string().contains("notes/entry.txt"));
    open_skill(&mut f,name);f.click_visible("이 항목을 삭제하는 것을 확인합니다.");f.click_visible("삭제");
    f.prepare("ui.deleteSkill");settle(&mut f);
    assert!(!root.exists());
    f.reopen_acceptance(true);select(&mut f,Section::Skills,None);settle(&mut f);
    assert!(!f.app.skill_catalog.iter().any(|v|v["name"]==name));
    let view=f.app.handle.view().unwrap();
    assert!(view.generation.records("agent_skill_visibility").iter().any(|r|r.data["skill_name"]=="accept-preserved-skill"&&r.data["agent_id"]==preserved_agent&&r.data["enabled"]==true),
        "Deleting one Skill must preserve the unrelated Skill grant");
    let dangling=view.generation.records("agent_skill_visibility").into_iter().filter(|r|r.data["skill_name"]==name).collect::<Vec<_>>();
    assert!(dangling.is_empty(),"Deleted Skill must remove every name-based grant, found: {dangling:?}");
    assert!(!Fixture::texts(&f.frame(vec![])).iter().any(|v|v==name||v.contains("ACCEPT SKILL META UPDATED")));
    open_skill(&mut f,"accept-preserved-skill");f.assert_drawer_texts(&["ACCEPT PRESERVED SKILL"]);
    f.shutdown();
    emit("config_accept_skill_native_create_file_cas_directories_metadata_visibility_delete_reopen",&[
        "ui.createSkill","ui.readSkillFile","ui.writeSkillFile","ui.createSkillDir","ui.skillFiles",
        "ui.updateSkillMeta","ui.skillVisibility","ui.toggleSkillVisibility","ui.deleteSkillPath","ui.deleteSkill"],
        "Actual Forms create, selected-file read, native SHA CAS write/create and metadata Save, real shared-profile files/directories, actual visibility controls, persisted readonly reopen and real native files/editor/list display. File/meta RPCs intentionally use native capability+SHA actions; no fabricated legacy RPC dispatch.");
    println!("ACCEPT_CONFIG_NATIVE_ADAPTER {}",json!({"case":"config_accept_skill_native_create_file_cas_directories_metadata_visibility_delete_reopen","methods":["ui.skills"],"scope":"Actual Skills navigation and selected catalog entry load -> native filesystem scan/decorated Service projection -> persisted nonempty created skill -> same-profile reopen and actual metadata/file selection display; deleted skill absent after actual confirmed deletion. Native automatic scan intentionally replaces standalone list RPC input."}));
}

fn notification(name:&str,url:Value,enabled:bool)->Value {
    json!({"name":name,"kind":"webhook","enabled":enabled,"mode":"realtime",
        "config":{"url":url,"method":"POST","body_template":"{\"proof\":\"ACCEPT NOTIFY NETWORK\",\"title\":{{json .Title}}}"},
        "filter":{},"rate_per_min":0})
}

fn email_notification(name:&str)->Value {
    json!({"name":name,"kind":"email","enabled":false,"mode":"realtime","config":{
        "host":"mail.fixture.invalid","port":2525,"from":"from@fixture.invalid","to":["to@fixture.invalid"]},
        "filter":{},"rate_per_min":0})
}

struct NotificationSecretBoundary {
    inner:Arc<dyn artex_runtime::HostGateway>,
    admitted:String,
    destination:String,
}
impl artex_runtime::HostGateway for NotificationSecretBoundary {
    fn request(&self,workspace:&str,context:&artex_core::Context,operation:&str,mut input:Value,timeout:Duration)->artex_core::Result<Value> {
        if operation=="notification.send" {
            // Webhook URLs are correctly SecretRefs in stored configuration.
            // Substitute only this admitted fixture's resolver; the real host
            // still enforces its local-endpoint opt-in and sends actual HTTP.
            assert_eq!(context.actor.kind,"user");assert!(context.actor.agent_id.is_none()&&context.actor.run_id.is_none());
            assert_eq!(input["config"]["url"]["secret_ref"],self.admitted);
            input["config"]["url"]=json!(self.destination);
        }
        self.inner.request(workspace,context,operation,input,timeout)
    }
    fn retire(&self,timeout:Duration)->artex_core::Result<()> {self.inner.retire(timeout)}
    fn retirement_report(&self,workspace:&str,contexts:&[artex_core::Context])->artex_core::Result<Option<Value>> {
        self.inner.retirement_report(workspace,contexts)
    }
}

#[test]
fn config_accept_notification_channel_forms_crud_meta_reopen() {
    let mut f=fixture();
    let preserved=f.setup("ui.notifyCreateChannel",json!({"payload":email_notification("ACCEPT NOTIFY PRESERVED")}))["id"].as_str().unwrap().to_owned();
    form(&mut f,Section::Notifications,"notification_channels",None,"ui.notifyCreateChannel",
        json!({"payload":email_notification("ACCEPT NOTIFY CREATED")}),None,&[],"저장");
    let id=keyed(&f,"notification_channels","name","ACCEPT NOTIFY CREATED").header.id;
    f.reopen_acceptance(false);left_row(&mut f,Section::Notifications,&id,"ACCEPT NOTIFY CREATED");
    f.assert_drawer_texts(&["mail.fixture.invalid","ACCEPT NOTIFY CREATED"]);
    form(&mut f,Section::Notifications,"notification_channels",Some(&id),"ui.notifyUpdateChannel",
        json!({"id":id,"payload":email_notification("ACCEPT NOTIFY UPDATED")}),None,&[],"저장");
    f.reopen_acceptance(false);left_row(&mut f,Section::Notifications,&id,"ACCEPT NOTIFY UPDATED");
    f.assert_drawer_texts(&["ACCEPT NOTIFY UPDATED","mail.fixture.invalid"]);
    let meta=query(&mut f,Section::Notifications,"ui.notifyMeta",json!({}),"webhook");
    assert_eq!(meta["stats"]["channels"],2);assert_eq!(meta["stats"]["channels_on"],0);
    f.source_result_at_path_visible("ui.notifyMeta",None,&["stats","channels"],"2");
    f.source_result_at_path_visible("ui.notifyMeta",None,&["stats","channels_on"],"0");
    form(&mut f,Section::Notifications,"notification_channels",Some(&id),"ui.notifyDeleteChannel",Value::Null,None,
        &["이 항목을 삭제하는 것을 확인합니다."],"삭제");
    f.reopen_acceptance(true);
    assert!(f.app.handle.view().unwrap().generation.get("notification_channels",&id).is_none());
    left_row(&mut f,Section::Notifications,&preserved,"ACCEPT NOTIFY PRESERVED");
    f.assert_drawer_texts(&["ACCEPT NOTIFY PRESERVED"]);
    assert!(!Fixture::texts(&f.frame(vec![])).iter().any(|s|s.contains("ACCEPT NOTIFY UPDATED")));
    let meta=query(&mut f,Section::Notifications,"ui.notifyMeta",json!({}),"webhook");assert_eq!(meta["stats"]["channels"],1);
    f.source_result_at_path_visible("ui.notifyMeta",None,&["stats","channels"],"1");
    assert!(f.app.handle.view().unwrap().generation.records("notification_deliveries").is_empty());
    f.shutdown();
    emit("config_accept_notification_channel_forms_crud_meta_reopen",&[
        "ui.notifyCreateChannel","ui.notifyUpdateChannel","ui.notifyDeleteChannel","ui.notifyMeta"],
        "Actual channel Forms save/update/confirmed delete, exact persisted endpoint/name/disabled state, readonly reopen and native selected row display plus real supported-kind/count query. Independent preserved channel proves deletion display; disabled config operations never contact endpoint.");
}

// The product intentionally blocks private network notification endpoints by
// default. Positive local delivery is a separate explicit-env run, not a silent
// security-policy relaxation inside the parallel Rust test process.
#[test]
#[ignore = "Requires explicit ARTEX_NOTIFY_ALLOW_LOCAL=1 in this isolated Cargo child"]
fn config_accept_notification_real_webhook_test_and_failed_outbox_retry_reopen() {
    assert_eq!(std::env::var("ARTEX_NOTIFY_ALLOW_LOCAL").as_deref(),Ok("1"));
    let server=McpFixture::new();let endpoint=server.url.clone();let secret=artex_core::new_id("secret");
    let admitted=secret.clone();let destination=endpoint.clone();
    let mut f=Fixture::configured_seeded(|profile|{
        let base=factory(profile,BTreeMap::new(),Probe::default());
        ServiceConfig {host_factory:Some(Arc::new(move|workspace|{
            Ok(Arc::new(NotificationSecretBoundary{inner:base(workspace)?,admitted:admitted.clone(),destination:destination.clone()})
                as Arc<dyn artex_runtime::HostGateway>)
        })),wake_ui:None}
    },|path|{
        let mut store=artex_store::Store::create(path,"Notification fixture").unwrap();
        let context=artex_core::Context{actor:artex_core::Actor{kind:"user".into(),agent_id:None,run_id:None,
            engine_epoch:artex_core::new_id("epoch")},task_id:None,intent_id:None,conversation_id:None};
        let mut bootstrap=artex_domain::bootstrap::bootstrap_plan(&store.snapshot(),&context).unwrap();
        bootstrap.command_id=artex_core::new_id("command");store.commit(bootstrap).unwrap();
        let snapshot=store.snapshot();let mut p=artex_domain::Planner::new(&snapshot,"fixture.failed_notification",&json!({}));
        let channel=p.insert("notification_channels",artex_core::collection_prefix("notification_channels").unwrap(),
            notification("ACCEPT NOTIFY RETRY",json!({"secret_ref":secret,"hint":"admitted loopback fixture URL"}),true)).unwrap();
        let finding=artex_core::new_id("finding");
        let event=p.insert("notification_events",artex_core::collection_prefix("notification_events").unwrap(),json!({
            "kind":"finding_created","finding_id":finding,"fanned_out":true,
            "snapshot":{"finding_id":finding,"name":"ACCEPT NOTIFICATION EVENT",
                "severity":"high","summary":"ACCEPT NOTIFICATION SUMMARY"}})).unwrap();
        p.insert("notification_deliveries",artex_core::collection_prefix("notification_deliveries").unwrap(),json!({
            "event_id":event,"channel_id":channel,"state":"failed","attempts":2,"last_error":"ACCEPT PRIOR FAILURE"})).unwrap();
        let mut batch=p.done(json!({}));batch.command_id=artex_core::new_id("command");store.commit(batch).unwrap();
        store.checkpoint().unwrap();
    });
    let channel=keyed(&f,"notification_channels","name","ACCEPT NOTIFY RETRY").header.id;
    let stored_destination=row(&f,"notification_channels",&channel).data["config"]["url"].clone();
    assert!(stored_destination["secret_ref"].is_string()&&!stored_destination.is_string());
    let failed=keyed(&f,"notification_deliveries","last_error","ACCEPT PRIOR FAILURE").header.id;
    let result=action(&mut f,Section::Notifications,"ui.notifyTestChannel",json!({"id":channel}));
    assert_eq!(result["ok"],true);f.source_result_visible("ui.notifyTestChannel",None,"true");
    assert_eq!(row(&f,"notification_deliveries",&failed).data["state"],"failed","Test button must not retry durable deliveries");
    f.reopen_acceptance(false);
    let failed_history=query(&mut f,Section::Notifications,"ui.notifyDeliveries",json!({"q":{"channelId":channel,"state":"failed","page":0,"pageSize":30}}),"ACCEPT PRIOR FAILURE");
    let index=failed_history["deliveries"].as_array().unwrap().iter().position(|r|r["id"]==failed).unwrap().to_string();
    f.source_result_at_path_visible("ui.notifyDeliveries",None,&["deliveries",&index,"state"],"failed");
    action(&mut f,Section::Notifications,"ui.notifyRetryDelivery",json!({"id":failed}));
    f.until(|app|app.handle.view().unwrap().generation.get("notification_deliveries",&failed).is_some_and(|r|r.data["state"]=="sent"));
    let delivered=row(&f,"notification_deliveries",&failed);
    assert_eq!(delivered.data["attempts"],1);assert_eq!(delivered.data["last_error"],"");assert!(delivered.data["sent_at"].is_string());
    f.reopen_acceptance(true);assert_eq!(row(&f,"notification_deliveries",&failed).data,delivered.data);
    assert_eq!(row(&f,"notification_channels",&channel).data["config"]["url"],stored_destination,
        "Resolving an admitted fixture secret must never rewrite persisted destination references");
    let history=query(&mut f,Section::Notifications,"ui.notifyDeliveries",json!({"q":{"channelId":channel,"state":"sent","page":0,"pageSize":30}}),"sent");
    assert!(history.to_string().contains(&failed));
    let index=history["deliveries"].as_array().unwrap().iter().position(|r|r["id"]==failed).unwrap().to_string();
    f.source_result_at_path_visible("ui.notifyDeliveries",None,&["deliveries",&index,"state"],"sent");
    f.shutdown();let requests=server.finish();assert_eq!(requests.len(),2,"One explicit test plus one explicit retry, no resend on reopen");
    assert!(requests.iter().all(|v|v["proof"]=="ACCEPT NOTIFY NETWORK"));
    assert!(requests.iter().any(|v|v["title"]=="ARTEX: ACCEPT NOTIFICATION EVENT"),"Retried message must carry the original event's semantic title");
    emit("config_accept_notification_real_webhook_test_and_failed_outbox_retry_reopen",&[
        "ui.notifyTestChannel","ui.notifyDeliveries","ui.notifyRetryDelivery"],
        "Explicit env-approved loopback webhook through real native notification host; actual user Test command, separately seeded failed durable delivery, actual retry UI Prepare/CAS, observed HTTP requests and exact sent state/attempts/timestamp, readonly reopen/history display, no automatic repeated external send. No real accounts/secrets/remote delivery claim.");
}

#[test]
fn config_accept_model_forms_save_test_activate_pool_delete_reopen() {
    let provider=LocalProvider::new(vec![Step::Reply(200,"application/json",anthropic("ACCEPT MODEL CONNECTION",false))]);
    let secret=artex_core::new_id("secret");let admitted=secret.clone();
    let mut f=Fixture::configured(|profile|ServiceConfig {
        host_factory:Some(factory(profile,BTreeMap::from([(admitted,"acceptance-memory-placeholder".into())]),Probe::default())),wake_ui:None,
    });
    let definition=|name:&str|json!({"name":name,"format":"anthropic","model":"fixture","base_url":provider.url,
        "streaming":false,"api_key":{"secret_ref":secret,"hint":"isolated memory fixture"},
        "retry":{"connect":{"attempts":-1,"interval_ms":0},"empty":{"attempts":-1,"interval_ms":0},"stream":{"attempts":-1,"interval_ms":0}}});
    let mut preserved_definition=definition("ACCEPT MODEL PRESERVED");preserved_definition["model"]=json!("fixture-control");
    let preserved=f.setup("ui.saveLLMProfile",json!({"p":preserved_definition}))["id"].as_str().unwrap().to_owned();
    form(&mut f,Section::Models,"llm_profiles",None,"ui.saveLLMProfile",json!({"p":definition("ACCEPT MODEL CREATED")}),None,&[],"모델 설정 저장");
    let id=keyed(&f,"llm_profiles","name","ACCEPT MODEL CREATED").header.id;
    f.reopen_acceptance(false);left_row(&mut f,Section::Models,&id,"ACCEPT MODEL CREATED");
    f.assert_drawer_texts(&["ACCEPT MODEL CREATED",&provider.url]);
    f.click_visible("저장된 모델 연결 시험");f.prepare("ui.testLLM");settle(&mut f);
    f.assert_drawer_texts(&["연결 시험 성공"]);
    assert_eq!(provider.requests().len(),1);
    f.click_visible("기본 모델로 설정");f.prepare("ui.activateLLMProfile");settle(&mut f);
    f.reopen_acceptance(false);
    assert_eq!(row(&f,"llm_profiles",&id).data["is_default"],true);
    left_row(&mut f,Section::Models,&id,"ACCEPT MODEL CREATED");
    f.assert_drawer_texts(&["ACCEPT MODEL CREATED","현재 기본 모델"]);
    let legacy=artex_domain::llm_ui::legacy_config(&f.app.handle.view().unwrap().generation.snapshot);
    assert_eq!(legacy["configured"],true);assert_eq!(legacy["provider"],"anthropic");assert_eq!(legacy["model"],"fixture");
    assert_eq!(legacy["base_url"],provider.url);assert_eq!(legacy["key_set"],true);
    assert!(legacy.get("api_key").is_none()&&!legacy.to_string().contains("secret_ref")&&!legacy.to_string().contains("acceptance-memory-placeholder"));
    let pool=query(&mut f,Section::Models,"ui.llmPool",json!({}),"ACCEPT MODEL CREATED");
    assert!(pool["chain"].as_array().unwrap().iter().any(|p|p["profile_id"]==id));
    left_row(&mut f,Section::Models,&preserved,"ACCEPT MODEL PRESERVED");
    f.click_visible("기본 모델로 설정");f.prepare("ui.activateLLMProfile");settle(&mut f);
    form(&mut f,Section::Models,"llm_profiles",Some(&id),"ui.deleteLLMProfile",Value::Null,None,
        &["이 항목을 삭제하는 것을 확인합니다."],"삭제");
    f.reopen_acceptance(true);
    assert!(f.app.handle.view().unwrap().generation.get("llm_profiles",&id).is_none());
    left_row(&mut f,Section::Models,&preserved,"ACCEPT MODEL PRESERVED");
    f.assert_drawer_texts(&["ACCEPT MODEL PRESERVED","현재 기본 모델"]);
    let legacy=artex_domain::llm_ui::legacy_config(&f.app.handle.view().unwrap().generation.snapshot);
    assert_eq!(legacy["model"],"fixture-control");assert_eq!(legacy["configured"],true);assert_eq!(legacy["key_set"],true);
    assert!(!Fixture::texts(&f.frame(vec![])).iter().any(|s|s.contains("ACCEPT MODEL CREATED")));
    f.shutdown();let requests=provider.finish();assert_eq!(requests.len(),1);
    assert_eq!(requests[0]["model"],"fixture");
    emit("config_accept_model_forms_save_test_activate_pool_delete_reopen",&[
        "ui.saveLLMProfile","ui.testLLM","ui.activateLLMProfile","ui.llmPool","ui.deleteLLMProfile"],
        "Actual model form Save, selected saved profile Test through real local Anthropic HTTP protocol/memory-only secret boundary, actual activation and default state/pool display, switch default to independent control before confirmed deletion, shared-profile persisted readonly reopen and preserved native model selection. No OS vault, live provider, user key or real model claimed.");
    println!("ACCEPT_CONFIG_NATIVE_ADAPTER {}",json!({"case":"config_accept_model_forms_save_test_activate_pool_delete_reopen","methods":["ui.getLLM","ui.setLLM"],"scope":"Explicit user-requested multiple shared LLM profiles/default-model replacement for legacy singleton settings. Actual model Form Save/Activate -> original profile CAS/native connection effect -> common-profile persistence/reopen -> actual current-default model display. Public legacy_config read cross-checks active model/provider/base URL/configured/key_set after both default choices and proves no plaintext/SecretRef exposed. Legacy getLLM/setLLM RPCs are not asserted as invoked from the new UI."}));
}

fn click_occurrence(f:&mut Fixture,label:&str,index:usize) {
    f.scroll_drawer_top();
    for _ in 0..40 {
        let out=f.frame(vec![]);
        let mut found=out.shapes.iter().filter_map(|shape|{
            let egui::epaint::Shape::Text(text)=&shape.shape else{return None;};let rect=text.visual_bounding_rect();
            (text.galley.text()==label&&shape.clip_rect.contains_rect(rect)).then(||rect.center())
        }).collect::<Vec<_>>();found.sort_by(|a,b|a.y.total_cmp(&b.y));
        if let Some(pos)=found.get(index).copied() {
            f.frame(vec![egui::Event::PointerMoved(pos),egui::Event::PointerButton{pos,button:egui::PointerButton::Primary,pressed:true,modifiers:Default::default()}]);
            f.frame(vec![egui::Event::PointerButton{pos,button:egui::PointerButton::Primary,pressed:false,modifiers:Default::default()}]);return;
        }
        f.scroll_drawer_down();
    }
    panic!("Actual {label} occurrence {index} missing");
}

fn assert_numeric_row(f:&mut Fixture,label:&str,value:&str) {
    f.scroll_drawer_top();
    for _ in 0..60 {
        let out=f.frame(vec![]);
        let painted=out.shapes.iter().filter_map(|shape|{
            let egui::epaint::Shape::Text(text)=&shape.shape else{return None;};let rect=text.visual_bounding_rect();
            shape.clip_rect.contains_rect(rect).then(||(text.galley.text(),rect))
        }).collect::<Vec<_>>();
        if painted.iter().filter(|(text,_)|*text==label).any(|(_,heading)|painted.iter().any(|(text,rect)|{
            *text==value&&rect.left()>heading.right()&&rect.left()-heading.right()<500.&&(rect.center().y-heading.center().y).abs()<16.
        })) {return;}
        f.scroll_drawer_down();
    }
    panic!("Actual numeric field row {label} did not paint {value}");
}

#[test]
fn config_accept_agent_normal_and_task_timeout_wrapup_save_reset_reopen() {
    let mut f=fixture();let original=keyed(&f,"agents","key","planner");let id=original.header.id.clone();
    for (index,field,turn_field,extra,extra_turn,save,reset,marker) in [
        (0,"wrapup_prompt","wrapup_max_turns","wrapup","wrapup_turns","ui.saveAgentWrapup","ui.resetAgentWrapup","ACCEPT NORMAL WRAPUP"),
        (1,"task_timeout_wrapup_prompt","task_timeout_wrapup_max_turns","timeout_wrapup","timeout_turns","ui.saveAgentTaskTimeoutWrapup","ui.resetAgentTaskTimeoutWrapup","ACCEPT TIMEOUT WRAPUP"),
    ] {
        select(&mut f,Section::Agents,Some(&id));let snapshot=f.app.handle.view().unwrap().generation.snapshot.clone();
        let mut extras=json!({});extras[extra]=json!({"text":marker,"blob":null});extras[extra_turn]=json!(3);
        f.app.forms.seed_editor_for_app_test("agents",Some(&id),&snapshot,Value::Null,Some(extras));
        let out=f.frame(vec![]);
        if !Fixture::texts(&out).iter().any(|s|s=="정리 프롬프트 저장") {f.click_visible("종료 정리 프롬프트");}
        click_occurrence(&mut f,"정리 프롬프트 저장",index);f.prepare(save);settle(&mut f);
        f.reopen_acceptance(false);
        let stored=row(&f,"agents",&id);assert_eq!(stored.data[field]["text"],marker);assert_eq!(stored.data[turn_field],3);
        select(&mut f,Section::Agents,Some(&id));f.assert_drawer_texts(&[marker]);
        assert_numeric_row(&mut f,extra_turn,"3");
        click_occurrence(&mut f,"기본값 복원",index);f.prepare(reset);settle(&mut f);
        f.reopen_acceptance(false);
        let restored=row(&f,"agents",&id);assert_eq!(restored.data[field],original.data[field]);assert_eq!(restored.data[turn_field],original.data[turn_field]);
        select(&mut f,Section::Agents,Some(&id));
        let original_body=original.data[field]["text"].as_str().expect("Builtin wrapup must be inline for native display");
        if original_body.is_empty() {
            // Empty is the original stored "inherit default" meaning. Do not
            // pretend an empty substring demonstrates rendered prompt text.
            f.assert_drawer_texts(&[if index==0 {"실행 종료 시 정리"} else {"작업 시간 만료 시 정리"},"정리 프롬프트 저장",
                &original.data[turn_field].to_string()]);
        } else {f.assert_drawer_texts(&[original_body]);}
        assert_numeric_row(&mut f,extra_turn,&original.data[turn_field].to_string());
    }
    f.reopen_acceptance(true);select(&mut f,Section::Agents,Some(&id));
    assert_eq!(row(&f,"agents",&id).data["wrapup_prompt"],original.data["wrapup_prompt"]);
    assert_eq!(row(&f,"agents",&id).data["task_timeout_wrapup_prompt"],original.data["task_timeout_wrapup_prompt"]);
    f.shutdown();
    emit("config_accept_agent_normal_and_task_timeout_wrapup_save_reset_reopen",&[
        "ui.saveAgentWrapup","ui.resetAgentWrapup","ui.saveAgentTaskTimeoutWrapup","ui.resetAgentTaskTimeoutWrapup"],
        "Actual distinct normal and timeout wrapup Save/default-reset buttons, original Prepare/CAS, exact Body and max-turn persistence, saved and restored actual editor display across repeated same-profile reopen. Both source meanings and native UI input paths retained.");
}

#[test]
fn config_accept_general_settings_form_save_reopen_auto_projection() {
    let mut f=fixture();
    form(&mut f,Section::Settings,"settings",None,"ui.setSettings",json!({"patch":{
        "workers":3,"notify_enabled":false,"notify_public_base_url":"https://fixture.invalid/ACCEPT_COMMON_SETTINGS"}}),None,&[],"저장");
    f.reopen_acceptance(true);select(&mut f,Section::Settings,None);settle(&mut f);
    assert_eq!(keyed(&f,"settings","key","workers").data["value"],"3");
    assert_eq!(keyed(&f,"settings","key","notify_enabled").data["value"],"false");
    f.assert_drawer_texts(&["https://fixture.invalid/ACCEPT_COMMON_SETTINGS"]);
    f.shutdown();
    emit("config_accept_general_settings_form_save_reopen_auto_projection",&["ui.setSettings"],
        "Actual settings Form Save, original Prepare/CAS, typed numeric/boolean/text settings persisted in common profile, same-space readonly reopen and real automatic ui.settings projection rendered in editor. No external request is made.");
}

#[test]
fn config_accept_skill_usage_and_missing_references_query_nonempty_reopen() {
    let mut f=Fixture::configured_seeded(|profile|ServiceConfig{
        host_factory:Some(factory(profile,BTreeMap::new(),Probe::default())),wake_ui:None,
    },|path|{
        let mut store=artex_store::Store::create(path,"Skill ledger fixture").unwrap();
        let snapshot=store.snapshot();let mut p=artex_domain::Planner::new(&snapshot,"fixture.skill_ledger",&json!({}));
        for (name,found,session,length) in [("accept-present-skill",true,"ACCEPT SKILL SESSION",41),("accept-missing-skill",false,"ACCEPT MISSING SESSION",17)] {
            p.insert("skill_usage","skillusage",json!({"skill":name,"found":found,"agent_key":"auto","session_id":session,"args_len":length})).unwrap();
        }
        let mut batch=p.done(json!({}));batch.command_id=artex_core::new_id("command");store.commit(batch).unwrap();store.checkpoint().unwrap();
    });
    let initial=f.app.handle.view().unwrap().generation.records("skill_usage").into_iter().map(|r|r.clone()).collect::<Vec<_>>();
    for reopened in [false,true] {
        if reopened {f.reopen_acceptance(true);}
        let usage=query(&mut f,Section::Skills,"ui.skillUsage",json!({"name":"accept-present-skill","limit":10}),"ACCEPT SKILL SESSION");
        assert_eq!(usage.as_array().unwrap().len(),1);assert_eq!(usage[0]["args_len"],41);assert_eq!(usage[0]["agent_key"],"auto");
        let missing=query(&mut f,Section::Skills,"ui.missingSkills",json!({"limit":10}),"accept-missing-skill");
        assert_eq!(missing.as_array().unwrap().len(),1);assert_eq!(missing[0]["calls"],1);
        assert!(!missing.to_string().contains("accept-present-skill"));
        for expected in &initial {assert_eq!(row(&f,"skill_usage",&expected.header.id).data,expected.data);}
    }
    f.shutdown();
    emit("config_accept_skill_usage_and_missing_references_query_nonempty_reopen",&["ui.skillUsage","ui.missingSkills"],
        "Actual query controls with specific skill and bounded limit, Service reads of initially seeded successful/missing call ledger, positive/nonmatching filter checks and numeric counts, exact persisted ledger readonly reopen, actual session/missing-name result leaf display. Seeded historical calls are query context, not claimed as actual skill execution.");
}

#[test]
fn config_accept_provider_model_discovery_real_http_reopen() {
    let server=McpFixture::new();let secret=artex_core::new_id("secret");let admitted=secret.clone();
    let mut f=Fixture::configured(|profile|ServiceConfig{
        host_factory:Some(factory(profile,BTreeMap::from([(admitted,"accept-model-discovery-placeholder".into())]),Probe::default())),wake_ui:None,
    });
    let profile=f.setup("ui.saveLLMProfile",json!({"p":{"name":"ACCEPT DISCOVERY PROFILE","format":"openai","model":"fixture",
        "base_url":server.url,"api_key":{"secret_ref":secret,"hint":"in-memory fixture"}}}))["id"].as_str().unwrap().to_owned();
    let params=json!({"provider":"openai","base_url":server.url,"api_key":null,"profile_id":profile});
    for reopened in [false,true] {
        // Provider probing is an explicit effectful POST-style UI command;
        // the native readonly policy intentionally keeps its button disabled.
        if reopened {f.reopen_acceptance(false);}
        let result=action(&mut f,Section::Models,"ui.fetchLLMModels",params.clone());
        f.source_result_visible("ui.fetchLLMModels",None,"ACCEPT_MODEL_DISCOVERED");
        assert_eq!(result["ok"],true);assert_eq!(result["models"],json!(["ACCEPT_MODEL_DISCOVERED","ACCEPT_MODEL_SECONDARY"]));
        assert_eq!(row(&f,"llm_profiles",&profile).data["base_url"],server.url);
    }
    f.shutdown();let requests=server.finish();assert_eq!(requests.len(),2);
    assert!(requests.iter().all(|r|r["http_path"]=="/mcp/models"));
    emit("config_accept_provider_model_discovery_real_http_reopen",&["ui.fetchLLMModels"],
        "Actual provider discovery command control selects persisted profile credentials, real authenticated HTTP GET /models and semantic two-ID response shown, saved endpoint/profile retained across explicit writable reopen and actual command repeated. Synthetic key stays memory-only; no live provider authentication claim.");
}

#[test]
fn config_accept_llm_pool_reset_only_selected_health_reopen() {
    let mut f=Fixture::configured_seeded(|_|ServiceConfig::default(),|path|{
        let mut store=artex_store::Store::create(path,"Pool health fixture").unwrap();
        let snapshot=store.snapshot();let mut p=artex_domain::Planner::new(&snapshot,"fixture.pool_health",&json!({}));
        for name in ["ACCEPT POOL RESET","ACCEPT POOL PRESERVED"] {
            let id=p.insert("llm_profiles","profile",json!({"name":name,"model":"fixture","format":"openai","base_url":"https://fixture.invalid/v1",
                "api_key":{"secret_ref":artex_core::new_id("secret"),"hint":"non-resolved fixture"}})).unwrap();
            p.insert("llm_profile_health","profilehealth",json!({"profile_id":id,"fails":2,"trips":1,"last_error":"ACCEPT POOL PRIOR FAILURE"})).unwrap();
        }
        let mut batch=p.done(json!({}));batch.command_id=artex_core::new_id("command");store.commit(batch).unwrap();store.checkpoint().unwrap();
    });
    let target=keyed(&f,"llm_profiles","name","ACCEPT POOL RESET").header.id;
    let preserved=keyed(&f,"llm_profiles","name","ACCEPT POOL PRESERVED").header.id;
    let before=query(&mut f,Section::Models,"ui.llmPool",json!({}),"ACCEPT POOL PRIOR FAILURE");
    assert!(before["chain"].as_array().unwrap().iter().all(|r|r["fails"]==2));
    action(&mut f,Section::Models,"ui.resetLLMPool",json!({"id":target}));
    f.reopen_acceptance(true);
    let result=query(&mut f,Section::Models,"ui.llmPool",json!({}),"ACCEPT POOL RESET");
    let chain=result["chain"].as_array().unwrap();
    let target_index=chain.iter().position(|r|r["profile_id"]==target).unwrap();let target=&chain[target_index];
    assert_eq!(target["state"],"ok");assert_eq!(target["fails"],0);
    let control_index=chain.iter().position(|r|r["profile_id"]==preserved).unwrap();let control=&chain[control_index];
    assert_eq!(control["state"],"degraded");assert_eq!(control["fails"],2);
    f.source_result_at_path_visible("ui.llmPool",None,&["chain",&target_index.to_string(),"state"],"ok");
    f.source_result_at_path_visible("ui.llmPool",None,&["chain",&target_index.to_string(),"fails"],"0");
    f.source_result_at_path_visible("ui.llmPool",None,&["chain",&control_index.to_string(),"fails"],"2");
    f.source_result_at_path_visible("ui.llmPool",None,&["chain",&control_index.to_string(),"last_error"],"ACCEPT POOL PRIOR FAILURE");
    assert_eq!(f.app.handle.view().unwrap().generation.records("llm_profile_health").len(),1);
    f.shutdown();
    emit("config_accept_llm_pool_reset_only_selected_health_reopen",&["ui.resetLLMPool"],
        "Actual pool-reset input and original destructive confirmation/Prepare/CAS; selected failed health removed while independent failure remains, exact healthy/degraded values and error shown after readonly reopen. Initially seeded health is historical context, not fabricated live failure.");
}

fn write_skill_zip(path:&std::path::Path,marker:&str) {
    use std::io::Write;
    let mut zip=zip::ZipWriter::new(std::fs::File::create(path).unwrap());
    let options=zip::write::SimpleFileOptions::default();
    zip.start_file("SKILL.md",options).unwrap();
    write!(zip,"---\nname: accept-zip-skill\ndescription: ACCEPT ZIP DESCRIPTION\n---\n\n{marker}\n").unwrap();
    zip.start_file("notes/proof.txt",options).unwrap();write!(zip,"{marker} NESTED").unwrap();zip.finish().unwrap();
}

#[test]
fn config_accept_skill_zip_picker_stage_upload_overwrite_reopen() {
    let mut f=fixture();let zip_path=f._dir.path().join("accepted-fixture.zip");
    write_skill_zip(&zip_path,"ACCEPT ZIP FIRST");
    select(&mut f,Section::Skills,None);
    TEST_INPUT_FILES.with(|q|q.borrow_mut().push_back(Some(zip_path.clone())));
    f.click_visible("스킬 ZIP 가져오기");
    assert!(f.app.pending.values().any(|p|matches!(p,Purpose::StageFile{..})),"Actual native staging must precede import");
    f.until(|app|app.pending.values().any(|p|matches!(p,Purpose::Prepare{method,..} if method=="ui.uploadSkill")));
    f.prepare("ui.uploadSkill");settle(&mut f);
    f.assert_drawer_texts(&["가져온 스킬: accept-zip-skill"]);
    let root=f.app.handle.view().unwrap().workspace.resources_root.join("skills/accept-zip-skill");
    assert!(std::fs::read_to_string(root.join("SKILL.md")).unwrap().contains("ACCEPT ZIP FIRST"));
    assert_eq!(std::fs::read_to_string(root.join("notes/proof.txt")).unwrap(),"ACCEPT ZIP FIRST NESTED");
    f.reopen_acceptance(false);open_skill(&mut f,"accept-zip-skill");f.click_visible("SKILL.md");settle(&mut f);
    f.assert_drawer_texts(&["ACCEPT ZIP FIRST"]);
    write_skill_zip(&zip_path,"ACCEPT ZIP OVERWRITE");
    f.click_visible("ZIP에 포함된 기존 스킬 덮어쓰기 허용");
    TEST_INPUT_FILES.with(|q|q.borrow_mut().push_back(Some(zip_path)));
    f.click_visible("스킬 ZIP 가져오기");
    f.until(|app|app.pending.values().any(|p|matches!(p,Purpose::Prepare{method,..} if method=="ui.uploadSkill")));
    f.prepare("ui.uploadSkill");settle(&mut f);
    f.reopen_acceptance(true);open_skill(&mut f,"accept-zip-skill");f.click_visible("SKILL.md");settle(&mut f);
    f.assert_drawer_texts(&["ACCEPT ZIP OVERWRITE"]);
    assert_eq!(std::fs::read_to_string(root.join("notes/proof.txt")).unwrap(),"ACCEPT ZIP OVERWRITE NESTED");
    assert_eq!(f.app.skill_catalog.iter().filter(|s|s["name"]=="accept-zip-skill").count(),1);
    TEST_INPUT_FILES.with(|q|assert!(q.borrow().is_empty(),"Actual picker must consume both supplied paths"));
    f.shutdown();
    emit("config_accept_skill_zip_picker_stage_upload_overwrite_reopen",&["ui.uploadSkill"],
        "Actual Skill import/overwrite controls, only OS picker selected-path boundary supplied by fixture, real native file staging/capability then original import Prepare, actual ZIP extraction and metadata/nested bytes, same-profile readonly reopen and actual SKILL.md read/display. No staged token/result is injected.");
}

struct SearchSecretBoundary {
    inner:Arc<dyn artex_runtime::HostGateway>,
    admitted:String,
    resolutions:Arc<std::sync::atomic::AtomicUsize>,
}
impl artex_runtime::HostGateway for SearchSecretBoundary {
    fn request(&self,workspace:&str,context:&artex_core::Context,operation:&str,mut input:Value,timeout:Duration)->artex_core::Result<Value> {
        // This substitutes only the fixture credential resolver, immediately
        // before the native host's equivalent resolver boundary. HTTP, SSE,
        // normalization and returned results remain real production code.
        if operation=="ui.execute"&&input["name"]=="ui.testWebSearch" {
            assert_eq!(context.actor.kind,"user");assert!(context.actor.agent_id.is_none()&&context.actor.run_id.is_none());
            assert_eq!(input["input"]["network_config"]["api_key"]["secret_ref"],self.admitted);
            input["input"]["network_config"]["api_key"]=json!("acceptance-search-memory-placeholder");
            self.resolutions.fetch_add(1,std::sync::atomic::Ordering::SeqCst);
        }
        self.inner.request(workspace,context,operation,input,timeout)
    }
    fn retire(&self,timeout:Duration)->artex_core::Result<()> {self.inner.retire(timeout)}
    fn retirement_report(&self,workspace:&str,contexts:&[artex_core::Context])->artex_core::Result<Option<Value>> {
        self.inner.retirement_report(workspace,contexts)
    }
}

#[test]
fn config_accept_web_search_actual_deepseek_http_sse_probe_reopen() {
    let event=format!("event: content_block_start\ndata: {}\n\n",json!({"type":"content_block_start","index":0,
        "content_block":{"type":"web_search_tool_result","content":[{"type":"web_search_result",
            "url":"https://fixture.invalid/ACCEPT_SEARCH_HIT","title":"ACCEPT SEARCH HIT","encrypted_content":"fixture"}]}}));
    let provider=LocalProvider::new(vec![Step::Reply(200,"text/event-stream",event.clone()),Step::Reply(200,"text/event-stream",event)]);
    let secret=artex_core::new_id("secret");let admitted=secret.clone();
    let resolutions=Arc::new(std::sync::atomic::AtomicUsize::new(0));let count=resolutions.clone();
    let mut f=Fixture::configured(|profile|{
        let base=factory(profile,BTreeMap::new(),Probe::default());
        ServiceConfig{wake_ui:None,host_factory:Some(Arc::new(move|workspace|{
            Ok(Arc::new(SearchSecretBoundary{inner:base(workspace)?,admitted:admitted.clone(),resolutions:count.clone()})
                as Arc<dyn artex_runtime::HostGateway>)
        }))}
    });
    let profile=f.setup("ui.saveLLMProfile",json!({"p":{"name":"ACCEPT SEARCH PROFILE","format":"anthropic","model":"fixture-search",
        "base_url":provider.url,"api_key":{"secret_ref":secret,"hint":"in-memory search boundary"}}}))["id"].as_str().unwrap().to_owned();
    f.setup("ui.activateLLMProfile",json!({"id":profile}));
    for reopen in [false,true] {
        if reopen {f.reopen_acceptance(false);}
        let result=action(&mut f,Section::Settings,"ui.testWebSearch",json!({"patch":{"web_search_backend":"deepseek"}}));
        assert_eq!(result["ok"],true);assert_eq!(result["backend"],"deepseek");assert_eq!(result["count"],1);
        f.source_result_visible("ui.testWebSearch",None,"deepseek");f.source_result_visible("ui.testWebSearch",None,"1");
        assert_eq!(row(&f,"llm_profiles",&profile).data["base_url"],provider.url);
    }
    f.shutdown();assert_eq!(resolutions.load(std::sync::atomic::Ordering::SeqCst),2);
    let requests=provider.finish();assert_eq!(requests.len(),2);
    assert!(requests.iter().all(|r|r["model"]=="fixture-search"&&r["tools"][0]["name"]=="web_search"&&r["stream"]==true));
    emit("config_accept_web_search_actual_deepseek_http_sse_probe_reopen",&["ui.testWebSearch"],
        "Actual test-search control against persisted active Anthropic-format profile, only admitted synthetic credential resolution substituted, real NativeGateway HTTP/SSE web_search_tool_result normalization and nonempty count/backend display before/after same-space reopen. No live search provider/site/API key, no endpoint/result injection.");
}

#[test]
fn config_accept_judge_usage_query_filters_worker_and_reopens_saved_counts() {
    let mut f=Fixture::configured_seeded(|_|ServiceConfig::default(),|path|{
        let mut store=artex_store::Store::create(path,"Judge usage fixture").unwrap();
        let snapshot=store.snapshot();let mut p=artex_domain::Planner::new(&snapshot,"fixture.judge_usage",&json!({}));
        // The daily grouping fixture stays on one UTC date even at midnight.
        let ts=artex_core::now();
        for (worker,input,output,read,write) in [("judge",17,4,5,2),("judge",20,5,7,3),("auto",99,99,99,99)] {
            p.insert("llm_usage",artex_core::collection_prefix("llm_usage").unwrap(),json!({"ts":ts,"worker":worker,"model":"fixture-model","profile_name":"ACCEPT JUDGE PROFILE",
                "latency_ms":13,"input_tokens":input,"output_tokens":output,"cache_read":read,"cache_write":write,"status":"completed"})).unwrap();
        }
        let mut batch=p.done(json!({}));batch.command_id=artex_core::new_id("command");store.commit(batch).unwrap();store.checkpoint().unwrap();
    });
    for reopened in [false,true] {
        if reopened {f.reopen_acceptance(true);}
        let result=query(&mut f,Section::Approvals,"ui.interceptJudgeUsage",json!({"days":1}),"37");
        assert_eq!(result["calls"],2);assert_eq!(result["input_tokens"],37);assert_eq!(result["output_tokens"],9);
        assert_eq!(result["cache_read_tokens"],12);assert_eq!(result["cache_write_tokens"],5);
        assert_eq!(result["daily"].as_array().unwrap().len(),1);assert_eq!(result["daily"][0]["calls"],2);
        for (path,value) in [("calls","2"),("input_tokens","37"),("output_tokens","9"),("cache_read_tokens","12"),("cache_write_tokens","5")] {
            f.source_result_at_path_visible("ui.interceptJudgeUsage",None,&[path],value);
        }
        assert_eq!(f.app.handle.view().unwrap().generation.records("llm_usage").len(),3);
    }
    f.shutdown();
    emit("config_accept_judge_usage_query_filters_worker_and_reopens_saved_counts",&["ui.interceptJudgeUsage"],
        "Actual judge-usage query input days=1, Service reads persisted historical ledger, two judge calls counted and unrelated agent excluded, exact input/output/cache and daily totals, readonly reopen and actual numeric result leaf display. Seeded historical usage is query context, not asserted as live model judging.");
}
