//! Domain acceptance follows real NativeApp controls and callbacks. Seeded
//! records are initial context only; their creation is never counted as input
//! evidence for the public UI command under test.
use super::*;
use artex_core::{Actor, Context, Record, new_id};
use artex_domain::Planner;
use artex_store::Store;
use std::collections::BTreeSet;

fn seed_apply(store: &mut Store, context: &Context, method: &str, params: Value) -> Value {
    let snapshot=store.snapshot();
    let mut plan = if artex_domain::assets_ui::COMMANDS.contains(&method) {artex_domain::assets_ui::plan(method,&params,&snapshot,context)}
        else if artex_domain::task_ui::COMMANDS.contains(&method) {artex_domain::task_ui::plan(method,&params,&snapshot,context)}
        else if artex_domain::task_control_ui::COMMANDS.contains(&method) {artex_domain::task_control_ui::plan(method,&params,&snapshot,context)}
        else {artex_domain::plan(method,&params,&snapshot,context)}.unwrap_or_else(|e|panic!("Initial context {method}: {e:?}"));
    artex_domain::augment_expected(&mut plan, &store.snapshot()).unwrap();
    plan.command_id = new_id("command");
    store.commit(plan).unwrap().result
}

fn seeded_domain(path: &std::path::Path) {
    let mut store = Store::create(path,"Domain acceptance context").unwrap();
    let mut context = Context { actor: Actor {kind:"user".into(),agent_id:None,run_id:None,engine_epoch:new_id("epoch")},
        task_id:None,intent_id:None,conversation_id:None };
    let mut bootstrap = artex_domain::bootstrap::bootstrap_plan(&store.snapshot(),&context).unwrap();
    bootstrap.command_id = new_id("command");
    store.commit(bootstrap).unwrap();
    let task = seed_apply(&mut store,&context,"task.create",json!({"description":"DOMAIN TASK CONTEXT","goal":"Only isolated fixture records","seed_first_intent":false}))["task_id"].as_str().unwrap().to_owned();
    context.task_id = Some(task.clone());
    let company = seed_apply(&mut store,&context,"add_company_scope",json!({"company":"DOMAIN COMPANY CONTEXT","scope":"domain-fixture.invalid","reason":"isolated fixture"}))["company_id"].as_str().unwrap().to_owned();
    let asset = seed_apply(&mut store,&context,"insert_assets",json!({"assets":[{"type":"service","service_type":"http","url":"https://domain-fixture.invalid/evidence","page_title":"DOMAIN ASSET CONTEXT","company_id":company}]}))["ids"][0].as_str().unwrap().to_owned();
    seed_apply(&mut store,&context,"ui.detachTaskAsset",json!({"taskId":task,"assetId":asset}));
    seed_apply(&mut store,&context,"insert_assets",json!({"assets":[{"type":"service","service_type":"http","url":"https://preserved-fixture.invalid/","page_title":"PRESERVED ASSET CONTEXT","company_id":company}]}));
    seed_apply(&mut store,&context,"ui.createTaskCategory",json!({"name":"PRESERVED CATEGORY CONTEXT"}));
    seed_apply(&mut store,&context,"ui.createTaskTemplate",json!({"input":{"name":"PRESERVED TEMPLATE CONTEXT","description":"Preserve unrelated template","goal":"fixture"}}));
    seed_apply(&mut store,&context,"set_goals",json!({"text":"DOMAIN GOAL CONTEXT"}));
    seed_apply(&mut store,&context,"add_task_scope",json!({"kind":"root_domain","value":"preserved-scope.invalid","reason":"Preserve unrelated scope"}));
    seed_apply(&mut store,&context,"set_constraints",json!({"constraints":[{"type":"deny","text":"PRESERVED CONSTRAINT CONTEXT"}]}));
    seed_apply(&mut store,&context,"ui.createTaskInterceptRule",json!({"id":task,"rule":{"action":"block","kind":"exact_domain","pattern":"preserved-rule.invalid","note":"PRESERVED TASK RULE CONTEXT","enabled":true}}));
    let intent = seed_apply(&mut store,&context,"add_intent",json!({"summary":"DOMAIN INTENT CONTEXT","asset_ids":[asset]}))["ids"][0].as_str().unwrap().to_owned();
    let fact = seed_apply(&mut store,&context,"record_fact",json!({"summary":"DOMAIN FACT CONTEXT","detail":"DOMAIN FACT EVIDENCE","intent_id":intent}))["ids"][0].as_str().unwrap().to_owned();
    let finding = seed_apply(&mut store,&context,"report_finding",json!({"name":"DOMAIN FINDING CONTEXT","severity":"high","summary":"DOMAIN FINDING SUMMARY","vulnclass":"fixture","evidence":"DOMAIN FINDING EVIDENCE","intent_id":intent,"asset_ids":[asset]}));
    seed_apply(&mut store,&context,"update_finding_report",json!({"finding_node_id":finding["finding_node_id"],"evidence_version":"0","report":"DOMAIN FINDING REPAIR"}));
    seed_apply(&mut store,&context,"report_finding",json!({"name":"PRESERVED FINDING CONTEXT","severity":"low","summary":"Preserve unrelated finding","vulnclass":"fixture","evidence":"PRESERVED EVIDENCE"}));
    let blank = store.blob_put(b"","application/octet-stream").unwrap();
    let response = store.blob_put(b"DOMAIN CAPTURED BODY","text/plain").unwrap();
    let snapshot = store.snapshot();
    let exp = snapshot.get("tasks",&task).unwrap().data["exploration_id"].clone();
    let agent = snapshot.records("agents").into_iter().find(|a|a.data["key"]=="mainagent").unwrap().header.id.clone();
    let mut p = Planner::new(&snapshot,"fixture.domain.rich",&json!({}));
    let mut node = snapshot.get("exploration_nodes",&intent).unwrap().data.clone();
    node["state"] = json!("done");p.put("exploration_nodes",&intent,node);
    let historical_run=p.insert("agent_runs","run",json!({"agent_id":agent,"task_id":task,"intent_id":intent,"engine_epoch":context.actor.engine_epoch,"state":"completed","started_at":artex_core::now(),"finished_at":artex_core::now()})).unwrap();
    p.insert("activity","activity",json!({"exploration_id":exp,"node_id":intent,"worker":"fixture-worker","kind":"result","summary":"DOMAIN ACTIVITY CONTEXT","detail":artex_core::body("DOMAIN ACTIVITY BODY"),"is_error":false,"input_tokens":13,"output_tokens":5,"cache_read_tokens":7,"cache_write_tokens":3})).unwrap();
    p.insert("activity","activity",json!({"exploration_id":exp,"node_id":intent,"worker":"fixture-worker","kind":"tool_use","tool":"Bash","tool_use_id":"domain-command","summary":"DOMAIN COMMAND INPUT","detail":artex_core::body("echo DOMAIN COMMAND INPUT"),"is_error":false})).unwrap();
    p.insert("activity","activity",json!({"exploration_id":exp,"node_id":intent,"worker":"fixture-worker","kind":"tool_result","tool":"Bash","tool_use_id":"domain-command","summary":"DOMAIN COMMAND RESULT","detail":artex_core::body("DOMAIN COMMAND RESULT"),"is_error":false})).unwrap();
    p.insert("activity","activity",json!({"exploration_id":exp,"node_id":intent,"worker":"fixture-worker","kind":"guard","summary":"DOMAIN GUARD CONTEXT","detail":artex_core::body(&json!({"guard_audit":{"ts":chrono::Utc::now().timestamp(),"tool":"Read","action":"allow","reason":"DOMAIN GUARD ALLOW REASON","command":"Read isolated fixture"},"guard_attribution":"DOMAIN GUARD ATTRIBUTION"}).to_string()),"is_error":false})).unwrap();
    // Reserved runtime activity must remain a valid protocol packet even when
    // its only asserted purpose is exclusion from the public history view.
    p.insert("activity","activity",json!({"exploration_id":exp,"node_id":intent,"worker":"fixture-worker","kind":"context_config","summary":"DOMAIN INTERNAL CONTEXT CONFIG","detail":artex_core::body(&json!({"run_id":historical_run,"context_config":{"noa_enabled":false,"protocol":"artex-context/1"}}).to_string()),"is_error":false})).unwrap();
    let conversation=p.insert("conversations",artex_core::collection_prefix("conversations").unwrap(),json!({"agent_key":"mainagent","title":"DOMAIN TOKEN CONVERSATION"})).unwrap();
    p.insert("conversation_activities",artex_core::collection_prefix("conversation_activities").unwrap(),json!({"conversation_id":conversation,"worker":"mainagent","kind":"result","summary":"DOMAIN CONVERSATION TOKEN RESULT","detail":artex_core::body("DOMAIN CONVERSATION TOKEN BODY"),"input_tokens":421,"output_tokens":23,"cache_read_tokens":17,"cache_write_tokens":11,"is_error":false})).unwrap();
    p.insert("llm_usage",artex_core::collection_prefix("llm_usage").unwrap(),json!({"ts":artex_core::now(),"task_id":task,"exploration_id":exp,"worker":"fixture-worker","model":"fixture-model","profile_name":"DOMAIN USAGE PROFILE","latency_ms":17,"input_tokens":13,"output_tokens":5,"cache_read":7,"cache_write":3,"status":"completed"})).unwrap();
    let digest = p.insert("exploration_nodes","node",json!({"exploration_id":exp,"kind":"digest","state":"active","payload":{"summary":"DOMAIN DIGEST CONTEXT","body":artex_core::body("DOMAIN DIGEST BODY")}})).unwrap();
    p.insert("exploration_edges","edge",json!({"exploration_id":exp,"src_id":digest,"dst_id":fact,"rel":"covers"})).unwrap();
    let traffic=p.insert("exchanges",artex_core::collection_prefix("exchanges").unwrap(),json!({"ts":chrono::Utc::now().timestamp().to_string(),"host":"domain-fixture.invalid","url":"https://domain-fixture.invalid/evidence","method":"GET","status":200,"req_len":"0","resp_len":"20","content_type":"text/plain"})).unwrap();
    let mut data=artex_core::default_data("exchange_bodies").unwrap();
    artex_domain::merge(&mut data,&json!({"id":traffic,"req_head":artex_core::body("GET /evidence HTTP/1.1\r\n"),"req_body":blank,"resp_head":artex_core::body("HTTP/1.1 200 OK\r\nContent-Type: text/plain\r\n"),"resp_body":response}));
    p.put("exchange_bodies",&new_id(artex_core::collection_prefix("exchange_bodies").unwrap()),data);
    // Distinct captures make host filters, batch deletion, and evidence order
    // observable. They share immutable bytes, not mutable record identities.
    for host in ["secondary-fixture.invalid","preserved-traffic.invalid","batch-traffic.invalid"] {
        let capture=p.insert("exchanges",artex_core::collection_prefix("exchanges").unwrap(),json!({"ts":chrono::Utc::now().timestamp().to_string(),"host":host,"url":format!("https://{host}/evidence"),"method":"GET","status":201,"req_len":"0","resp_len":"20","content_type":"text/plain"})).unwrap();
        let mut data=artex_core::default_data("exchange_bodies").unwrap();
        artex_domain::merge(&mut data,&json!({"id":capture,"req_head":artex_core::body("GET /evidence HTTP/1.1\r\n"),"req_body":blank,"resp_head":artex_core::body("HTTP/1.1 201 Created\r\n"),"resp_body":response}));
        p.put("exchange_bodies",&new_id(artex_core::collection_prefix("exchange_bodies").unwrap()),data);
    }
    let mut batch=p.done(json!({}));batch.command_id=new_id("command");store.commit(batch).unwrap();
    seed_apply(&mut store,&context,"task.pause",json!({"task_id":task}));
    store.checkpoint().unwrap();
}

fn domain_fixture() -> Fixture {
    Fixture::configured_seeded(|_| ServiceConfig::default(),seeded_domain)
}
fn row(f: &Fixture, collection: &str, field: &str, value: &str) -> Record {
    f.app.handle.view().unwrap().generation.records(collection).into_iter()
        .find(|r|r.data[field]==value).unwrap_or_else(||panic!("Missing fixture {collection}.{field}={value}")).clone()
}
fn task(f: &Fixture) -> String { row(f,"tasks","description","DOMAIN TASK CONTEXT").header.id }
fn settled(f: &mut Fixture) {f.until(|app|app.pending.is_empty());assert!(f.app.error.is_none(),"{:?}",f.app.error);}
fn stop(f: &mut Fixture,id: &str) {
    select(f,Section::Tasks,Some(id));f.click_visible("작업 중지·정리");
    f.until(|app|app.pending.is_empty() && app.handle.view().unwrap().generation.get("tasks",id)
        .is_some_and(|t|t.data["paused"]==true&&t.data["queued"]==false));
    assert!(f.app.error.is_none(),"{:?}",f.app.error);
}
fn action(f:&mut Fixture,section:Section,selected:Option<&str>,method:&str,params:Value)->Value {
    f.source_action(section,selected,method,params)
}
fn query(f:&mut Fixture,section:Section,selected:Option<&str>,method:&str,params:Value,marker:&str)->Value {
    let value=action(f,section,selected,method,params);
    assert!(value.to_string().contains(marker),"{method} did not return {marker}: {value}");
    f.source_result_visible(method,selected,marker);
    value
}
fn roundtrip_query(f:&mut Fixture,section:Section,selected:Option<&str>,method:&str,params:Value,marker:&str)->Value {
    query(f,section,selected,method,params.clone(),marker);
    f.reopen_acceptance(false);
    query(f,section,selected,method,params,marker)
}
fn accepted(methods:&[&str],case:&str) {
    println!("DOMAIN_ACCEPT {}",json!({"case":case,"methods":methods,"scope":"Isolated headless NativeApp actual Source controls/Prepare/Service/SQLite/query/reopen/display; prepared valid context records are not counted as operation input"}));
}
fn company_scope_visible(f:&mut Fixture,name:&str,marker:&str) {
    select(f,Section::Assets,None);f.app.drawer.open=false;f.scroll_drawer_top();
    let out=f.frame(vec![]);
    // Hide the management result so only the real company expander can satisfy
    // this assertion; a correctly rendered source JSON must not mask its bug.
    if visible_texts(&out,f.source_area()).iter().any(|s|s=="조회"||s=="변경 실행") {
        f.click_visible("추가 관리");
    }
    let out=f.frame(vec![]);
    if !visible_texts(&out,f.source_area()).iter().any(|s|s=="자산 범위 추가") { f.click_visible(name); }
    f.assert_drawer_texts(&[marker]);
}
fn constraint_form(f:&mut Fixture,selected:Option<&str>,params:Value,delete:bool) {
    select(f,Section::Constraints,selected);
    let view=f.app.handle.view().unwrap();
    f.app.forms.seed_editor_for_app_test("task_constraints",selected,&view.generation.snapshot,params,None);
    f.app.drawer.open=false;f.frame(vec![]);
    if delete {f.click_visible("이 항목을 삭제하는 것을 확인합니다.");}
    f.click_visible(if delete {"삭제"} else {"저장"});
    f.prepare(if delete {"ui.deleteConstraint"} else if selected.is_some() {"ui.updateConstraint"} else {"ui.addConstraint"});
    settled(f);
}

fn edit_visible_text(f:&mut Fixture,old:&str,new:&str) {
    let out=f.frame(vec![]);
    let pos=out.shapes.iter().rev().find_map(|s| {
        let egui::epaint::Shape::Text(t)=&s.shape else{return None;};
        (t.galley.text()==old&&s.clip_rect.contains_rect(t.visual_bounding_rect())).then(||t.visual_bounding_rect().center())
    }).unwrap_or_else(||panic!("Actual editable text missing {old}"));
    f.frame(vec![egui::Event::PointerMoved(pos),egui::Event::PointerButton{pos,button:egui::PointerButton::Primary,pressed:true,modifiers:Default::default()}]);
    f.frame(vec![egui::Event::PointerButton{pos,button:egui::PointerButton::Primary,pressed:false,modifiers:Default::default()}]);
    f.frame(vec![egui::Event::Key{key:egui::Key::A,physical_key:None,pressed:true,repeat:false,modifiers:egui::Modifiers{ctrl:true,command:true,..Default::default()}},egui::Event::Text(new.into())]);
}
fn navigate(f:&mut Fixture,section:Section) {
    let out=f.frame(vec![]);let main_left=f.ctx.available_rect().left();let pos=out.shapes.iter().find_map(|shape|match &shape.shape {
        egui::epaint::Shape::Text(t) if t.galley.text().trim()==section.label()&&t.visual_bounding_rect().right()<=main_left&&shape.clip_rect.contains_rect(t.visual_bounding_rect())=>Some(t.visual_bounding_rect().center()),_=>None}).expect("Actual native navigation control must be painted");
    f.frame(vec![egui::Event::PointerMoved(pos),egui::Event::PointerButton{pos,button:egui::PointerButton::Primary,pressed:true,modifiers:Default::default()}]);
    f.frame(vec![egui::Event::PointerButton{pos,button:egui::PointerButton::Primary,pressed:false,modifiers:Default::default()}]);assert_eq!(f.app.section,section);
}
fn left_record(f:&mut Fixture,section:Section,id:&str,label:&str) {
    select(f,section,None);let out=f.frame(vec![]);let main_left=f.ctx.available_rect().left();
    // The same title may also appear in the central editor. Observe the real
    // rendered panel boundary so only an actual object-list row is clicked.
    let pos=out.shapes.iter().find_map(|shape|match &shape.shape {
        egui::epaint::Shape::Text(t) if t.galley.text().contains(label)&&t.visual_bounding_rect().right()<=main_left&&shape.clip_rect.contains_rect(t.visual_bounding_rect())=>Some(t.visual_bounding_rect().center()),_=>None}).unwrap_or_else(||panic!("Actual left object row missing {label}"));
    f.frame(vec![egui::Event::PointerMoved(pos),egui::Event::PointerButton{pos,button:egui::PointerButton::Primary,pressed:true,modifiers:Default::default()}]);
    f.frame(vec![egui::Event::PointerButton{pos,button:egui::PointerButton::Primary,pressed:false,modifiers:Default::default()}]);assert_eq!(f.app.selected[&section].as_deref(),Some(id));
}

#[test]
fn domain_categories_templates_task_metadata_have_real_controls_persistence_and_reopen() {
    let mut f=domain_fixture();let task=task(&f);
    let keep_category=row(&f,"task_categories","name","PRESERVED CATEGORY CONTEXT").header.id;
    let cat=action(&mut f,Section::Categories,None,"ui.createTaskCategory",json!({"name":"DOMAIN CATEGORY CREATED"}))["id"].as_str().unwrap().to_owned();
    assert_eq!(f.app.handle.view().unwrap().generation.get("task_categories",&cat).unwrap().data["name"],"DOMAIN CATEGORY CREATED");
    roundtrip_query(&mut f,Section::Categories,None,"ui.taskCategories",json!({}),"DOMAIN CATEGORY CREATED");
    action(&mut f,Section::Categories,Some(&cat),"ui.renameTaskCategory",json!({"id":cat,"name":"DOMAIN CATEGORY RENAMED"}));
    roundtrip_query(&mut f,Section::Categories,None,"ui.taskCategories",json!({}),"DOMAIN CATEGORY RENAMED");
    action(&mut f,Section::Tasks,Some(&task),"ui.updateTaskCategory",json!({"taskId":task,"categoryId":cat}));
    f.reopen_acceptance(false);
    assert_eq!(f.app.handle.view().unwrap().generation.get("tasks",&task).unwrap().data["category_id"],cat);
    query(&mut f,Section::Tasks,Some(&task),"ui.task",json!({"id":task}),"DOMAIN TASK CONTEXT");
    f.source_result_visible("ui.task",Some(&task),&cat);
    action(&mut f,Section::Categories,Some(&cat),"ui.updateTasksCategory",json!({"taskIds":[task],"categoryId":keep_category}));
    f.reopen_acceptance(false);
    assert_eq!(f.app.handle.view().unwrap().generation.get("tasks",&task).unwrap().data["category_id"],keep_category);
    query(&mut f,Section::Tasks,Some(&task),"ui.task",json!({"id":task}),"DOMAIN TASK CONTEXT");
    f.source_result_visible("ui.task",Some(&task),&keep_category);
    let template=action(&mut f,Section::Templates,None,"ui.createTaskTemplate",json!({"input":{"name":"DOMAIN TEMPLATE CREATED","description":"DOMAIN TEMPLATE DESCRIPTION","goal":"DOMAIN TEMPLATE GOAL","category_id":cat}}))["id"].as_str().unwrap().to_owned();
    roundtrip_query(&mut f,Section::Tasks,Some(&task),"ui.taskTemplates",json!({}),"DOMAIN TEMPLATE CREATED");
    action(&mut f,Section::Templates,Some(&template),"ui.updateTaskTemplate",json!({"id":template,"input":{"name":"DOMAIN TEMPLATE RENAMED","goal":"DOMAIN TEMPLATE UPDATED GOAL"}}));
    roundtrip_query(&mut f,Section::Tasks,Some(&task),"ui.taskTemplates",json!({}),"DOMAIN TEMPLATE UPDATED GOAL");
    action(&mut f,Section::Tasks,Some(&task),"ui.renameTask",json!({"id":task,"name":"DOMAIN TASK RENAMED"}));
    roundtrip_query(&mut f,Section::Tasks,Some(&task),"ui.task",json!({"id":task}),"DOMAIN TASK RENAMED");
    action(&mut f,Section::Tasks,Some(&task),"ui.pinTask",json!({"id":task,"pinned":true}));
    f.reopen_acceptance(false);assert!(!f.app.handle.view().unwrap().generation.get("tasks",&task).unwrap().data["pinned_at"].is_null());
    query(&mut f,Section::Tasks,Some(&task),"ui.task",json!({"id":task}),"DOMAIN TASK RENAMED");
    let pinned=f.app.handle.view().unwrap().generation.get("tasks",&task).unwrap().data["pinned_at"].as_str().unwrap().to_owned();
    f.source_result_visible("ui.task",Some(&task),&pinned);
    action(&mut f,Section::Tasks,Some(&task),"ui.updateTask",json!({"id":task,"input":{"name":"DOMAIN TASK UPDATED","pinned":false}}));
    let updated=roundtrip_query(&mut f,Section::Tasks,Some(&task),"ui.task",json!({"id":task}),"DOMAIN TASK UPDATED");assert!(updated.get("pinned_at").is_none());
    assert!(f.app.handle.view().unwrap().generation.get("tasks",&task).unwrap().data["pinned_at"].is_null());
    action(&mut f,Section::Templates,Some(&template),"ui.deleteTaskTemplate",json!({"id":template}));
    f.reopen_acceptance(false);assert!(f.app.handle.view().unwrap().generation.get("task_templates",&template).is_none());
    let remaining=query(&mut f,Section::Tasks,Some(&task),"ui.taskTemplates",json!({}),"PRESERVED TEMPLATE CONTEXT");assert!(!remaining.to_string().contains("DOMAIN TEMPLATE RENAMED"));
    action(&mut f,Section::Categories,Some(&cat),"ui.deleteTaskCategory",json!({"id":cat}));
    f.reopen_acceptance(true);assert!(f.app.handle.view().unwrap().generation.get("task_categories",&cat).is_none());
    let remaining=query(&mut f,Section::Categories,None,"ui.taskCategories",json!({}),"PRESERVED CATEGORY CONTEXT");assert!(!remaining.to_string().contains("DOMAIN CATEGORY RENAMED"));
    f.shutdown();
    accepted(&["ui.createTaskCategory","ui.renameTaskCategory","ui.deleteTaskCategory","ui.taskCategories","ui.updateTaskCategory","ui.updateTasksCategory","ui.createTaskTemplate","ui.updateTaskTemplate","ui.deleteTaskTemplate","ui.taskTemplates","ui.renameTask","ui.pinTask","ui.updateTask","ui.task"],"domain_categories_templates_task_metadata_have_real_controls_persistence_and_reopen");
}

#[test]
fn domain_goals_use_returned_node_ids_survive_edits_and_delete_after_reopen() {
    let mut f=domain_fixture();let task=task(&f);
    let goal=action(&mut f,Section::Tasks,Some(&task),"ui.addGoal",json!({"id":task,"text":"DOMAIN UI GOAL CREATED","vulnclass":"fixture"}))["id"].as_str().unwrap().to_owned();
    stop(&mut f,&task);
    roundtrip_query(&mut f,Section::Tasks,Some(&task),"ui.taskGoals",json!({"id":task}),"DOMAIN UI GOAL CREATED");
    action(&mut f,Section::Tasks,Some(&task),"ui.updateGoal",json!({"id":task,"goalId":goal,"text":"DOMAIN UI GOAL UPDATED"}));
    stop(&mut f,&task);
    roundtrip_query(&mut f,Section::Tasks,Some(&task),"ui.taskGoals",json!({"id":task}),"DOMAIN UI GOAL UPDATED");
    action(&mut f,Section::Tasks,Some(&task),"ui.deleteGoal",json!({"id":task,"goalId":goal}));
    f.reopen_acceptance(true);
    let result=query(&mut f,Section::Tasks,Some(&task),"ui.taskGoals",json!({"id":task}),"DOMAIN GOAL CONTEXT");
    assert!(!result.to_string().contains("DOMAIN UI GOAL UPDATED"));assert!(f.app.handle.view().unwrap().generation.get("exploration_nodes",&goal).is_none());
    f.shutdown();accepted(&["ui.addGoal","ui.updateGoal","ui.deleteGoal","ui.taskGoals"],"domain_goals_use_returned_node_ids_survive_edits_and_delete_after_reopen");
}

#[test]
fn domain_companies_scope_replacement_and_asset_attribution_survive_reopen() {
    let mut f=domain_fixture();
    let id=action(&mut f,Section::Assets,None,"ui.createCompany",json!({"name":"DOMAIN COMPANY CREATED","scope":[{"kind":"domain","value":"new-company.invalid"}]}))["id"].as_str().unwrap().to_owned();
    roundtrip_query(&mut f,Section::Assets,None,"ui.companies",json!({}),"DOMAIN COMPANY CREATED");
    company_scope_visible(&mut f,"DOMAIN COMPANY CREATED","new-company.invalid");
    action(&mut f,Section::Assets,None,"ui.addCompanyScope",json!({"id":id,"scope":[{"kind":"domain","value":"new-scope.invalid"},{"kind":"ip","value":"192.0.2.7"},{"kind":"cidr","value":"192.0.2.0/24"}],"reason":"DOMAIN COMPANY ADD REASON"}));
    roundtrip_query(&mut f,Section::Assets,None,"ui.companies",json!({}),"DOMAIN COMPANY CREATED");
    assert!(f.app.handle.view().unwrap().generation.records("company_scope").iter().any(|r|r.data["company_id"]==id&&r.data["domain"]=="new-scope.invalid"));
    company_scope_visible(&mut f,"DOMAIN COMPANY CREATED","new-scope.invalid");
    f.assert_drawer_texts(&["192.0.2.7","192.0.2.0/24"]);
    action(&mut f,Section::Assets,None,"ui.updateCompanyScope",json!({"id":id,"scope":[{"kind":"domain","value":"replaced-scope.invalid"}],"reason":"DOMAIN COMPANY REPLACE REASON"}));
    roundtrip_query(&mut f,Section::Assets,None,"ui.companies",json!({}),"DOMAIN COMPANY CREATED");
    let scopes=f.app.handle.view().unwrap().generation.records("company_scope").into_iter().filter(|r|r.data["company_id"]==id).map(|r|r.data.clone()).collect::<Vec<_>>();
    assert_eq!(scopes.len(),1);assert_eq!(scopes[0]["domain"],"replaced-scope.invalid");
    company_scope_visible(&mut f,"DOMAIN COMPANY CREATED","replaced-scope.invalid");
    action(&mut f,Section::Assets,None,"ui.deleteCompany",json!({"id":id,"deleteAssets":false}));
    f.reopen_acceptance(true);assert!(f.app.handle.view().unwrap().generation.get("companies",&id).is_none());
    let value=query(&mut f,Section::Assets,None,"ui.companies",json!({}),"DOMAIN COMPANY CONTEXT");assert!(!value.to_string().contains("DOMAIN COMPANY CREATED"));
    f.shutdown();accepted(&["ui.createCompany","ui.addCompanyScope","ui.updateCompanyScope","ui.deleteCompany","ui.companies"],"domain_companies_scope_replacement_and_asset_attribution_survive_reopen");
}

#[test]
fn domain_assets_search_task_links_detach_and_delete_preserve_owned_scope() {
    let mut f=domain_fixture();let task=task(&f);let asset=row(&f,"assets","page_title","DOMAIN ASSET CONTEXT").header.id;
    roundtrip_query(&mut f,Section::Assets,None,"ui.searchAssets",json!({"dsl":"url=domain-fixture.invalid","limit":25,"offset":0}),"DOMAIN ASSET CONTEXT");
    action(&mut f,Section::Assets,None,"ui.attachTaskAssets",json!({"taskId":task,"assetIds":[asset],"sourceSummary":"DOMAIN ATTACH SOURCE"}));
    roundtrip_query(&mut f,Section::Assets,None,"ui.taskAssets",json!({"taskId":task,"limit":25,"offset":0}),"DOMAIN ATTACH SOURCE");
    roundtrip_query(&mut f,Section::Assets,None,"ui.searchTaskAssets",json!({"taskId":task,"dsl":"url=domain-fixture.invalid","limit":25,"offset":0}),"DOMAIN ASSET CONTEXT");
    let counts=roundtrip_query(&mut f,Section::Assets,None,"ui.assetCounts",json!({"taskId":task}),"2");
    assert_eq!(counts["service"],2);f.source_result_at_path_visible("ui.assetCounts",None,&["service"],"2");
    roundtrip_query(&mut f,Section::Assets,None,"ui.taskAssetRefs",json!({"id":task,"assetId":asset}),"DOMAIN INTENT CONTEXT");
    action(&mut f,Section::Assets,None,"ui.detachTaskAsset",json!({"taskId":task,"assetId":asset}));
    f.reopen_acceptance(false);
    assert!(!f.app.handle.view().unwrap().generation.records("task_asset_links").iter().any(|l|l.data["task_id"]==task&&l.data["asset_id"]==asset));
    let remaining=query(&mut f,Section::Assets,None,"ui.taskAssets",json!({"taskId":task}),"PRESERVED ASSET CONTEXT");assert!(!remaining["assets"].as_array().unwrap().iter().any(|r|r["id"]==asset));
    // The asset itself remains available after removing this task's link.
    query(&mut f,Section::Assets,None,"ui.searchAssets",json!({"dsl":"url=domain-fixture.invalid"}),"DOMAIN ASSET CONTEXT");
    action(&mut f,Section::Assets,None,"ui.deleteAssets",json!({"ids":[asset]}));
    f.reopen_acceptance(true);assert!(f.app.handle.view().unwrap().generation.get("assets",&asset).is_none());
    let empty=action(&mut f,Section::Assets,None,"ui.searchAssets",json!({"dsl":"url=domain-fixture.invalid"}));
    assert_eq!(empty["total"],0);assert!(empty["assets"].as_array().unwrap().is_empty());f.source_result_visible("ui.searchAssets",None,"0");
    query(&mut f,Section::Assets,None,"ui.searchAssets",json!({"dsl":"url=preserved-fixture.invalid"}),"PRESERVED ASSET CONTEXT");
    f.shutdown();accepted(&["ui.searchAssets","ui.taskAssets","ui.searchTaskAssets","ui.assetCounts","ui.attachTaskAssets","ui.detachTaskAsset","ui.taskAssetRefs","ui.deleteAssets"],"domain_assets_search_task_links_detach_and_delete_preserve_owned_scope");
}

#[test]
fn domain_graph_queries_render_nonempty_nodes_edges_lineage_and_coverage_after_reopen() {
    let mut f=domain_fixture();let task=task(&f);
    for (section,method,params,marker) in [
        (Section::Templates,"ui.explorationGraph",json!({"task":task}),"DOMAIN FACT CONTEXT"),
        (Section::Templates,"ui.explorationNodes",json!({"task":task,"query":{"page":1,"size":25,"kinds":["fact"]}}),"DOMAIN FACT CONTEXT"),
        (Section::Tasks,"ui.intentsPage",json!({"task":task,"limit":25}),"DOMAIN INTENT CONTEXT"),
        (Section::Tasks,"ui.taskCoverage",json!({"id":task}),"service"),
        (Section::Tasks,"ui.taskCoverageGraph",json!({"id":task}),"domain-fixture.invalid"),
        (Section::Assets,"ui.taskIntentAssets",json!({"taskId":task}),"domain-fixture.invalid"),
    ] {
        let value=roundtrip_query(&mut f,section,if section==Section::Tasks {Some(task.as_str())} else {None},method,params,marker);
        if method=="ui.taskCoverage" {assert_eq!(value["enabled"],true);assert!(value["denominator"].as_u64().unwrap()>0);assert!(value["tested"].as_u64().unwrap()>0);} else {assert!(!value.is_null());}
        if matches!(method,"ui.explorationGraph"|"ui.taskCoverageGraph") {let edges=value["edges"].as_array().unwrap();assert!(!edges.is_empty());assert!(edges.iter().all(|e|e["src"].is_string()&&e["dst"].is_string()));}
    }
    f.shutdown();accepted(&["ui.explorationGraph","ui.explorationNodes","ui.intentsPage","ui.taskCoverage","ui.taskCoverageGraph","ui.taskIntentAssets"],"domain_graph_queries_render_nonempty_nodes_edges_lineage_and_coverage_after_reopen");
}

#[test]
fn domain_findings_metadata_pages_groups_lineage_and_delete_are_durable() {
    let mut f=domain_fixture();let task=task(&f);let finding=row(&f,"findings","name","DOMAIN FINDING CONTEXT").header.id;
    let finding_query=json!({"page":1,"pageSize":25,"task":task,"query":"DOMAIN FINDING"});
    roundtrip_query(&mut f,Section::Findings,Some(&finding),"ui.getFinding",json!({"id":finding,"contextTaskId":task}),"DOMAIN FINDING EVIDENCE");
    roundtrip_query(&mut f,Section::Findings,None,"ui.findingsPage",json!({"q":finding_query}),"DOMAIN FINDING CONTEXT");
    roundtrip_query(&mut f,Section::Findings,None,"ui.findingGroups",json!({"q":finding_query}),"DOMAIN TASK CONTEXT");
    roundtrip_query(&mut f,Section::Findings,None,"ui.findingAssetTree",json!({"q":{"task":task}}),"domain-fixture.invalid");
    roundtrip_query(&mut f,Section::Findings,Some(&finding),"ui.findingLineage",json!({"id":finding}),"DOMAIN INTENT CONTEXT");
    action(&mut f,Section::Findings,Some(&finding),"ui.updateFinding",json!({"id":finding,"fields":{"name":"DOMAIN FINDING RENAMED","vulnclass":"fixture-updated"}}));
    roundtrip_query(&mut f,Section::Findings,Some(&finding),"ui.getFinding",json!({"id":finding}),"DOMAIN FINDING RENAMED");
    action(&mut f,Section::Findings,Some(&finding),"ui.setFindingStatus",json!({"id":finding,"status":"confirmed"}));
    let result=roundtrip_query(&mut f,Section::Findings,Some(&finding),"ui.getFinding",json!({"id":finding}),"confirmed");assert_eq!(result["status"],"confirmed");
    action(&mut f,Section::Findings,Some(&finding),"ui.setFindingSeverity",json!({"id":finding,"severity":"critical"}));
    let result=roundtrip_query(&mut f,Section::Findings,Some(&finding),"ui.getFinding",json!({"id":finding}),"critical");assert_eq!(result["severity"],"critical");
    let stats=roundtrip_query(&mut f,Section::Findings,None,"ui.findingStats",json!({}),"1");assert_eq!(stats["critical"],1);f.source_result_at_path_visible("ui.findingStats",None,&["critical"],"1");
    action(&mut f,Section::Findings,Some(&finding),"ui.deleteFinding",json!({"id":finding}));
    f.reopen_acceptance(true);assert!(f.app.handle.view().unwrap().generation.get("findings",&finding).is_none());
    let result=query(&mut f,Section::Findings,None,"ui.findingsPage",json!({"q":{"page":1,"pageSize":25,"task":task}}),"PRESERVED FINDING CONTEXT");assert_eq!(result["total"],1);assert!(!result.to_string().contains("DOMAIN FINDING RENAMED"));
    f.shutdown();accepted(&["ui.getFinding","ui.findingsPage","ui.findingGroups","ui.findingAssetTree","ui.findingLineage","ui.updateFinding","ui.setFindingStatus","ui.setFindingSeverity","ui.findingStats","ui.deleteFinding"],"domain_findings_metadata_pages_groups_lineage_and_delete_are_durable");
}

#[test]
fn domain_finding_traffic_binding_uses_saved_versions_and_preserves_body_after_capture_removal() {
    let mut f=domain_fixture();let task=task(&f);let finding=row(&f,"findings","name","DOMAIN FINDING CONTEXT").header.id;
    let traffic=row(&f,"exchanges","host","domain-fixture.invalid").header.id;
    let second=row(&f,"exchanges","host","secondary-fixture.invalid").header.id;
    let bound=action(&mut f,Section::Findings,Some(&finding),"ui.bindFindingTraffic",json!({"id":finding,"contextTask":task,"traffic_refs":[{"traffic_id":traffic,"role":"proof","note":"DOMAIN EVIDENCE BINDING"},{"traffic_id":second,"role":"supporting","note":"PRESERVED EVIDENCE BINDING"}]}));
    assert!(bound.to_string().contains("DOMAIN EVIDENCE BINDING"));
    let saved=row(&f,"finding_traffic_bindings","note","DOMAIN EVIDENCE BINDING");let mut binding=saved.header.id.clone();
    let preserved=row(&f,"finding_traffic_bindings","note","PRESERVED EVIDENCE BINDING").header.id;
    roundtrip_query(&mut f,Section::Findings,Some(&finding),"ui.findingTraffic",json!({"id":finding,"contextTask":task}),"DOMAIN EVIDENCE BINDING");
    roundtrip_query(&mut f,Section::Findings,Some(&finding),"ui.findingTrafficDetail",json!({"id":finding,"bindingId":binding,"contextTask":task}),"DOMAIN CAPTURED BODY");
    roundtrip_query(&mut f,Section::Findings,Some(&finding),"ui.findingTrafficBody",json!({"id":finding,"bindingId":binding,"side":"response","offset":0,"contextTask":task}),"DOMAIN CAPTURED BODY");
    let version=f.app.handle.view().unwrap().generation.get("findings",&finding).unwrap().data["evidence_version"].clone();
    action(&mut f,Section::Findings,Some(&finding),"ui.editFindingTraffic",json!({"id":finding,"bindingId":binding,"version":version,"fields":{"role":"supporting","note":"DOMAIN EVIDENCE EDITED"},"contextTask":task}));
    roundtrip_query(&mut f,Section::Findings,Some(&finding),"ui.findingTraffic",json!({"id":finding}),"DOMAIN EVIDENCE EDITED");
    let version=f.app.handle.view().unwrap().generation.get("findings",&finding).unwrap().data["evidence_version"].clone();
    action(&mut f,Section::Findings,Some(&finding),"ui.orderFindingTraffic",json!({"id":finding,"binding_ids":[preserved,binding],"version":version,"contextTask":task}));
    let ordered=roundtrip_query(&mut f,Section::Findings,Some(&finding),"ui.findingTraffic",json!({"id":finding}),"DOMAIN EVIDENCE EDITED");
    assert_eq!(ordered["bindings"][0]["id"],preserved);assert_eq!(ordered["bindings"][1]["id"],binding);
    let version=f.app.handle.view().unwrap().generation.get("findings",&finding).unwrap().data["evidence_version"].clone();
    action(&mut f,Section::Findings,Some(&finding),"ui.removeFindingTraffic",json!({"id":finding,"bindingId":binding,"version":version,"contextTask":task}));
    roundtrip_query(&mut f,Section::Findings,Some(&finding),"ui.findingTraffic",json!({"id":finding}),"PRESERVED EVIDENCE BINDING");
    let snapshot_id=saved.data["snapshot_id"].as_str().unwrap();assert!(f.app.handle.view().unwrap().generation.get("traffic_evidence_snapshots",snapshot_id).unwrap().data["unreferenced_at"].is_string());
    action(&mut f,Section::Findings,Some(&finding),"ui.bindFindingTraffic",json!({"id":finding,"traffic_refs":[{"traffic_id":traffic,"role":"proof","note":"DOMAIN EVIDENCE REBOUND"}]}));
    let rebound=row(&f,"finding_traffic_bindings","note","DOMAIN EVIDENCE REBOUND");binding=rebound.header.id;assert_eq!(rebound.data["snapshot_id"],snapshot_id);
    assert!(f.app.handle.view().unwrap().generation.get("traffic_evidence_snapshots",snapshot_id).unwrap().data["unreferenced_at"].is_null());
    roundtrip_query(&mut f,Section::Findings,Some(&finding),"ui.findingTraffic",json!({"id":finding}),"DOMAIN EVIDENCE REBOUND");
    // Removing the mutable capture must not invalidate immutable evidence.
    action(&mut f,Section::Traffic,None,"ui.trafficDeleteHosts",json!({"hosts":["domain-fixture.invalid"]}));
    f.reopen_acceptance(false);assert!(f.app.handle.view().unwrap().generation.get("exchanges",&traffic).is_none());
    query(&mut f,Section::Findings,Some(&finding),"ui.findingTrafficBody",json!({"id":finding,"bindingId":binding,"side":"response","offset":0}),"DOMAIN CAPTURED BODY");
    let version=f.app.handle.view().unwrap().generation.get("findings",&finding).unwrap().data["evidence_version"].clone();
    action(&mut f,Section::Findings,Some(&finding),"ui.removeFindingTraffic",json!({"id":finding,"bindingId":binding,"version":version,"contextTask":task}));
    f.reopen_acceptance(true);assert!(f.app.handle.view().unwrap().generation.get("finding_traffic_bindings",&binding).is_none());
    let result=query(&mut f,Section::Findings,Some(&finding),"ui.findingTraffic",json!({"id":finding}),"PRESERVED EVIDENCE BINDING");assert_eq!(result["bindings"].as_array().unwrap().len(),1);assert!(!result.to_string().contains("DOMAIN EVIDENCE REBOUND"));
    f.shutdown();accepted(&["ui.bindFindingTraffic","ui.findingTraffic","ui.findingTrafficDetail","ui.findingTrafficBody","ui.editFindingTraffic","ui.orderFindingTraffic","ui.removeFindingTraffic"],"domain_finding_traffic_binding_uses_saved_versions_and_preserves_body_after_capture_removal");
}

#[test]
fn domain_constraint_editor_creates_updates_and_deletes_owned_constraint_with_reopened_display() {
    let mut f=domain_fixture();let task=task(&f);
    constraint_form(&mut f,None,json!({"id":task,"text":"DOMAIN CONSTRAINT CREATED","kind":"deny"}),false);
    let id=row(&f,"task_constraints","text","DOMAIN CONSTRAINT CREATED").header.id;
    f.reopen_acceptance(false);select(&mut f,Section::Constraints,Some(&id));
    f.assert_drawer_texts(&["DOMAIN CONSTRAINT CREATED","차단"]);
    assert_eq!(f.app.handle.view().unwrap().generation.get("task_constraints",&id).unwrap().data["kind"],"deny");
    constraint_form(&mut f,Some(&id),json!({"id":task,"constraintId":id,"text":"DOMAIN CONSTRAINT UPDATED","kind":"allow"}),false);
    f.reopen_acceptance(false);select(&mut f,Section::Constraints,Some(&id));
    f.assert_drawer_texts(&["DOMAIN CONSTRAINT UPDATED","허용"]);
    assert_eq!(f.app.handle.view().unwrap().generation.get("task_constraints",&id).unwrap().data["kind"],"allow");
    constraint_form(&mut f,Some(&id),Value::Null,true);
    f.reopen_acceptance(true);select(&mut f,Section::Constraints,None);
    assert!(f.app.handle.view().unwrap().generation.get("task_constraints",&id).is_none());
    assert!(!Fixture::texts(&f.frame(vec![])).iter().any(|s|s.contains("DOMAIN CONSTRAINT UPDATED")));
    let preserved=row(&f,"task_constraints","text","PRESERVED CONSTRAINT CONTEXT").header.id;
    left_record(&mut f,Section::Constraints,&preserved,"PRESERVED CONSTRAINT CONTEXT");
    f.assert_drawer_texts(&["PRESERVED CONSTRAINT CONTEXT"]);
    f.shutdown();accepted(&["ui.addConstraint","ui.updateConstraint","ui.deleteConstraint"],"domain_constraint_editor_creates_updates_and_deletes_owned_constraint_with_reopened_display");
}

#[test]
fn domain_task_rules_create_update_toggle_delete_through_source_actions_and_reopen() {
    let mut f=domain_fixture();let task=task(&f);
    let id=action(&mut f,Section::Templates,None,"ui.createTaskInterceptRule",json!({"id":task,"rule":{"action":"block","kind":"exact_domain","pattern":"rule-created.invalid","note":"DOMAIN TASK RULE CREATED","enabled":true}}))["id"].as_str().unwrap().to_owned();
    roundtrip_query(&mut f,Section::Templates,None,"ui.taskInterceptRules",json!({"id":task}),"rule-created.invalid");
    action(&mut f,Section::Templates,None,"ui.updateTaskInterceptRule",json!({"id":task,"ruleId":id,"rule":{"action":"allow","kind":"exact_domain","pattern":"rule-updated.invalid","note":"DOMAIN TASK RULE UPDATED","enabled":true}}));
    roundtrip_query(&mut f,Section::Templates,None,"ui.taskInterceptRules",json!({"id":task}),"rule-updated.invalid");
    action(&mut f,Section::Templates,None,"ui.toggleTaskInterceptRule",json!({"id":task,"ruleId":id,"enabled":false}));
    let value=roundtrip_query(&mut f,Section::Templates,None,"ui.taskInterceptRules",json!({"id":task}),"false");
    assert_eq!(value.as_array().unwrap().iter().find(|v|v["id"]==id).unwrap()["enabled"],false);
    let index=value.as_array().unwrap().iter().position(|v|v["id"]==id).unwrap().to_string();f.source_result_at_path_visible("ui.taskInterceptRules",None,&[&index,"enabled"],"false");
    action(&mut f,Section::Templates,None,"ui.deleteTaskInterceptRule",json!({"id":task,"ruleId":id}));
    f.reopen_acceptance(true);assert!(f.app.handle.view().unwrap().generation.get("task_intercept_rules",&id).is_none());
    let result=query(&mut f,Section::Templates,None,"ui.taskInterceptRules",json!({"id":task}),"PRESERVED TASK RULE CONTEXT");assert_eq!(result.as_array().unwrap().len(),1);assert!(!result.to_string().contains("DOMAIN TASK RULE UPDATED"));
    f.shutdown();accepted(&["ui.createTaskInterceptRule","ui.updateTaskInterceptRule","ui.toggleTaskInterceptRule","ui.deleteTaskInterceptRule","ui.taskInterceptRules"],"domain_task_rules_create_update_toggle_delete_through_source_actions_and_reopen");
}

#[test]
fn domain_workspace_files_use_native_cas_controls_and_original_full_download_after_reopen() {
    let mut f=Fixture::configured(|profile|ServiceConfig{host_factory:Some(transport::factory(profile,BTreeMap::new(),transport::Probe::default())),wake_ui:None});
    select(&mut f,Section::Files,None);f.app.drawer.open=false;
    f.app.panes.new_directory="domain-files".into();
    f.click_visible("폴더 만들기");f.prepare("ui.workspaceMkdir");settled(&mut f);
    let root=f.app.handle.view().unwrap().workspace.work_root.clone();assert!(root.join("domain-files").is_dir());
    f.reopen_acceptance(false);select(&mut f,Section::Files,None);
    f.app.panes.file_path="domain-files".into();f.click_visible("폴더 목록");
    f.until(|app|app.pending.is_empty()&&app.panes.projection.contains_key("ui.workspaceList"));
    assert!(root.join("domain-files").is_dir());
    f.app.panes.file_path="domain-files/note.txt".into();f.app.panes.file_text="DOMAIN FILE BEFORE INPUT".into();
    edit_visible_text(&mut f,"DOMAIN FILE BEFORE INPUT","DOMAIN FILE CREATED 한글");
    f.click_visible("새 파일 만들기");
    assert!(f.app.pending.values().any(|p|matches!(p,Purpose::FileWrite{..})),"Actual native FileWrite must be submitted");
    settled(&mut f);assert_eq!(std::fs::read_to_string(root.join("domain-files/note.txt")).unwrap(),"DOMAIN FILE CREATED 한글");
    f.reopen_acceptance(false);select(&mut f,Section::Files,None);f.app.panes.file_path="domain-files/note.txt".into();
    f.click_visible("파일 읽기");assert!(f.app.pending.values().any(|p|matches!(p,Purpose::FileRead{..})));
    f.until(|app|app.pending.is_empty()&&app.panes.file_loaded.as_deref()==Some("domain-files/note.txt"));
    f.assert_drawer_texts(&["DOMAIN FILE CREATED 한글"]);
    edit_visible_text(&mut f,"DOMAIN FILE CREATED 한글","DOMAIN FILE UPDATED 한글");
    f.click_visible("변경 저장");settled(&mut f);
    f.reopen_acceptance(false);select(&mut f,Section::Files,None);f.app.panes.file_path="domain-files/note.txt".into();
    f.click_visible("파일 읽기");f.until(|app|app.pending.is_empty()&&app.panes.file_loaded.as_deref()==Some("domain-files/note.txt"));
    f.assert_drawer_texts(&["DOMAIN FILE UPDATED 한글"]);
    f.click_visible("선택 경로 파일 내보내기 준비");
    f.until(|app|app.pending.is_empty()&&app.panes.projection.contains_key("ui.workspaceDownload"));
    let destination=f._dir.path().join("downloaded-note.txt");
    TEST_EXPORT_DESTINATIONS.with(|queue|queue.borrow_mut().push_back(Some(destination.clone())));
    f.click_visible("파일로 저장");settled(&mut f);
    assert_eq!(std::fs::read_to_string(&destination).unwrap(),"DOMAIN FILE UPDATED 한글");
    f.app.panes.file_path="domain-files".into();f.click_visible("폴더 목록");
    f.until(|app|app.pending.is_empty());f.assert_drawer_texts(&["note.txt"]);
    let input=f._dir.path().join("domain-upload.txt");std::fs::write(&input,"DOMAIN UPLOADED FILE 한글").unwrap();
    TEST_INPUT_FILES.with(|queue|queue.borrow_mut().push_back(Some(input.clone())));
    f.click_visible("선택한 로컬 파일을 이 폴더에 복사");
    assert!(f.app.pending.values().any(|p|matches!(p,Purpose::StageFile{..})));
    f.until(|app|app.pending.values().any(|p|matches!(p,Purpose::Prepare{method,..} if method=="ui.workspaceUpload")));
    f.prepare("ui.workspaceUpload");settled(&mut f);
    assert_eq!(std::fs::read(root.join("domain-files/domain-upload.txt")).unwrap(),std::fs::read(&input).unwrap());
    f.reopen_acceptance(false);select(&mut f,Section::Files,None);f.app.panes.file_path="domain-files".into();f.click_visible("폴더 목록");f.until(|app|app.pending.is_empty());f.assert_drawer_texts(&["domain-upload.txt"]);
    f.app.panes.file_path="domain-files/domain-upload.txt".into();f.click_visible("파일 읽기");f.until(|app|app.pending.is_empty()&&app.panes.file_loaded.as_deref()==Some("domain-files/domain-upload.txt"));f.assert_drawer_texts(&["DOMAIN UPLOADED FILE 한글"]);
    action(&mut f,Section::Files,None,"ui.workspaceDelete",json!({"path":"domain-files/note.txt"}));
    f.reopen_acceptance(true);select(&mut f,Section::Files,None);f.app.panes.file_path="domain-files".into();
    f.click_visible("폴더 목록");f.until(|app|app.pending.is_empty());
    assert!(!root.join("domain-files/note.txt").exists());assert!(root.join("domain-files").is_dir());
    assert!(!f.app.panes.projection["ui.workspaceList"].to_string().contains("note.txt"));
    f.assert_drawer_texts(&["domain-upload.txt"]);assert_eq!(std::fs::read_to_string(root.join("domain-files/domain-upload.txt")).unwrap(),"DOMAIN UPLOADED FILE 한글");
    f.shutdown();accepted(&["ui.workspaceMkdir","ui.workspaceList","ui.workspaceRead","ui.workspaceWrite","ui.workspaceDownload","ui.workspaceDelete","ui.workspaceUpload"],"domain_workspace_files_use_native_cas_controls_and_original_full_download_after_reopen");
    println!("DOMAIN_NATIVE_ADAPTER {}",json!({"methods":["ui.workspaceRead","ui.workspaceWrite"],"scope":"Approved integrated native file boundary: actual file-read/create/save buttons submit typed NativeAction FileRead/FileWrite with service-owned SHA revision CAS; no legacy text-write RPC is fabricated. Isolated exact UTF-8 bytes are read, changed, reopened and painted; upload/download use original staged file capabilities."}));
}

#[test]
fn domain_usage_commands_and_task_statistics_read_saved_nonempty_dimensions_after_reopen() {
    let mut f=domain_fixture();let task=task(&f);
    let stats=roundtrip_query(&mut f,Section::Overview,None,"ui.stats",json!({"task":task}),"DOMAIN TASK CONTEXT");
    assert!(stats["assets"].as_u64().unwrap()>=2);assert_eq!(stats["active_task"]["id"],task);
    let tokens=roundtrip_query(&mut f,Section::Overview,None,"ui.tokenStats",json!({"task":task}),"fixture-worker");
    assert_eq!(tokens["total"]["input_tokens"],13);assert_eq!(tokens["total"]["output_tokens"],5);assert_eq!(tokens["total"]["cache_read_tokens"],7);
    f.source_result_at_path_visible("ui.tokenStats",None,&["total","input_tokens"],"13");
    let today=artex_core::now()[..10].to_owned();
    let days=roundtrip_query(&mut f,Section::Overview,None,"ui.tokenDaily",json!({"days":30}),&today);
    assert_eq!(days.as_array().unwrap().iter().find(|r|r["date"]==today).unwrap()["output_tokens"],5);
    let usage=roundtrip_query(&mut f,Section::Overview,None,"ui.usageStats",json!({"days":30}),"DOMAIN USAGE PROFILE");
    assert!(usage.to_string().contains("fixture-model")||usage.to_string().contains("input_tokens"));
    let commands=roundtrip_query(&mut f,Section::Logs,None,"ui.commands",json!({"params":{"task":task,"q":"DOMAIN COMMAND","page":0,"size":25}}),"DOMAIN COMMAND RESULT");
    assert_eq!(commands["commands"].as_array().unwrap().len(),1);assert_eq!(commands["commands"][0]["tool"],"Bash");
    let aggregate=roundtrip_query(&mut f,Section::Overview,None,"ui.commandStats",json!({"params":{"task":task,"q":"DOMAIN COMMAND"}}),"Bash");
    assert_eq!(aggregate["stats"][0]["total"],1);assert_eq!(aggregate["stats"][0]["errors"],0);
    f.shutdown();accepted(&["ui.stats","ui.tokenStats","ui.tokenDaily","ui.usageStats","ui.commands","ui.commandStats"],"domain_usage_commands_and_task_statistics_read_saved_nonempty_dimensions_after_reopen");
}

#[test]
fn domain_new_main_session_creates_durable_segment_without_running_a_model() {
    let mut f=domain_fixture();let task=task(&f);
    let result=action(&mut f,Section::Tasks,Some(&task),"ui.newMainSession",json!({"task":task}));
    let seq=result["seq"].as_u64().unwrap();assert!(seq>0);
    let value=roundtrip_query(&mut f,Section::Tasks,Some(&task),"ui.mainSessions",json!({"task":task}),&seq.to_string());
    assert_eq!(value["current"],seq);assert!(value["sessions"].as_array().unwrap().iter().any(|v|v["seq"]==seq));
    assert!(f.app.handle.view().unwrap().generation.records("agent_runs").iter().all(|r|r.data["state"]=="completed"));
    f.shutdown();accepted(&["ui.newMainSession","ui.mainSessions"],"domain_new_main_session_creates_durable_segment_without_running_a_model");
}

#[test]
fn domain_register_asset_scopes_creates_scoped_asset_and_task_link_visible_after_reopen() {
    let mut f=domain_fixture();let task=task(&f);
    let result=action(&mut f,Section::Assets,None,"ui.registerTaskAssetScopes",json!({"taskId":task,"scope":[{"kind":"domain","value":"registered-scope.invalid"}]}));
    assert_eq!(result["requested"],1);assert_eq!(result["assets_linked"],1);
    roundtrip_query(&mut f,Section::Assets,None,"ui.taskAssets",json!({"taskId":task}),"registered-scope.invalid");
    let scope=f.app.handle.view().unwrap().generation.records("task_scope").into_iter().find(|s|s.data["task_id"]==task&&s.data["domain"]=="registered-scope.invalid").cloned();
    assert!(scope.is_some());
    select(&mut f,Section::Tasks,Some(&task));f.click_visible("개요");f.click_visible("목표·허용 범위");f.assert_drawer_texts(&["registered-scope.invalid"]);
    f.shutdown();accepted(&["ui.registerTaskAssetScopes"],"domain_register_asset_scopes_creates_scoped_asset_and_task_link_visible_after_reopen");
}

#[test]
fn domain_native_task_scope_add_query_delete_preserves_other_scope_and_reopened_display() {
    let mut f=domain_fixture();let task=task(&f);
    select(&mut f,Section::Tasks,Some(&task));f.app.drawer.open=false;f.click_visible("개요");f.click_visible("목표·허용 범위");
    f.app.panes.scope_kind="root_domain".into();f.app.panes.scope_value="domain-added-scope.invalid".into();
    f.app.panes.scope_reason="DOMAIN SCOPE REASON".into();
    f.click_visible("범위 추가");f.prepare("ui.addTaskScope");settled(&mut f);
    let scope=f.app.handle.view().unwrap().generation.records("task_scope").into_iter()
        .find(|s|s.data["task_id"]==task&&s.data["domain"]=="domain-added-scope.invalid").unwrap().header.id.clone();
    f.reopen_acceptance(false);
    roundtrip_query(&mut f,Section::Tasks,Some(&task),"ui.taskScope",json!({"id":task}),"domain-added-scope.invalid");
    select(&mut f,Section::Tasks,Some(&task));f.click_visible("개요");f.click_visible("목표·허용 범위");f.assert_drawer_texts(&["domain-added-scope.invalid"]);
    // Both native rows have Delete controls. Select the control adjacent to
    // the actual target scope text, never an arbitrary same-label button.
    let out=f.frame(vec![]);let text=out.shapes.iter().find_map(|shape|match &shape.shape {
        egui::epaint::Shape::Text(t) if t.galley.text().contains("domain-added-scope.invalid")=>Some(t.visual_bounding_rect()),_=>None}).unwrap();
    let pos=out.shapes.iter().find_map(|shape|match &shape.shape {
        egui::epaint::Shape::Text(t) if t.galley.text()=="삭제"&&(t.visual_bounding_rect().center().y-text.center().y).abs()<12.=>Some(t.visual_bounding_rect().center()),_=>None}).unwrap();
    f.frame(vec![egui::Event::PointerMoved(pos),egui::Event::PointerButton{pos,button:egui::PointerButton::Primary,pressed:true,modifiers:Default::default()}]);
    f.frame(vec![egui::Event::PointerButton{pos,button:egui::PointerButton::Primary,pressed:false,modifiers:Default::default()}]);
    f.prepare("ui.deleteTaskScope");settled(&mut f);f.reopen_acceptance(true);
    let value=query(&mut f,Section::Tasks,Some(&task),"ui.taskScope",json!({"id":task}),"preserved-scope.invalid");
    assert!(!value.to_string().contains("domain-added-scope.invalid"));assert!(f.app.handle.view().unwrap().generation.get("task_scope",&scope).is_none());
    f.shutdown();accepted(&["ui.addTaskScope","ui.taskScope","ui.deleteTaskScope"],"domain_native_task_scope_add_query_delete_preserves_other_scope_and_reopened_display");
}

#[test]
fn domain_activity_filters_cursor_history_and_full_body_reopen_without_internal_noise() {
    let mut f=domain_fixture();let task=task(&f);let intent=row(&f,"exploration_nodes","kind","intent").header.id;
    let result=roundtrip_query(&mut f,Section::Logs,None,"ui.activity",json!({"task":task,"opts":{"intent":intent,"limit":20}}),"DOMAIN ACTIVITY CONTEXT");
    assert_eq!(result["items"].as_array().unwrap().len(),4);
    assert!(!result.to_string().contains("DOMAIN INTERNAL CONTEXT CONFIG"));
    assert_eq!(result["items"].as_array().unwrap().iter().find(|r|r["summary"]=="DOMAIN GUARD CONTEXT").unwrap()["kind"],"text");
    let activity=row(&f,"activity","summary","DOMAIN ACTIVITY CONTEXT").header.id;
    roundtrip_query(&mut f,Section::Logs,None,"ui.activityDetail",json!({"task":task,"id":activity}),"DOMAIN ACTIVITY BODY");
    let history=roundtrip_query(&mut f,Section::Logs,None,"ui.activityHistory",json!({"task":task,"session":format!("intent:{intent}"),"limit":20}),"DOMAIN COMMAND RESULT");
    assert_eq!(history["items"].as_array().unwrap().len(),4);assert_eq!(history["hasMore"],false);
    roundtrip_query(&mut f,Section::Tasks,Some(&task),"ui.taskConstraints",json!({"id":task}),"PRESERVED CONSTRAINT CONTEXT");
    let audit=roundtrip_query(&mut f,Section::Logs,None,"ui.audit",json!({"task":task}),"DOMAIN GUARD ALLOW REASON");assert_eq!(audit["entries"].as_array().unwrap().len(),1);assert_eq!(audit["entries"][0]["action"],"allow");assert_eq!(audit["attributions"]["DOMAIN GUARD ATTRIBUTION"],1);
    let tokens=roundtrip_query(&mut f,Section::Models,None,"ui.conversationTokens",json!({}),"421");assert_eq!(tokens.as_array().unwrap().len(),1);assert_eq!(tokens[0]["input_tokens"],421);assert_eq!(tokens[0]["output_tokens"],23);assert_eq!(tokens[0]["cache_read_tokens"],17);assert_eq!(tokens[0]["cache_write_tokens"],11);
    f.shutdown();accepted(&["ui.activity","ui.activityDetail","ui.activityHistory","ui.taskConstraints","ui.audit","ui.conversationTokens"],"domain_activity_filters_cursor_history_and_full_body_reopen_without_internal_noise");
}

fn native_traffic(f:&mut Fixture)->Value {
    select(f,Section::Traffic,None);f.app.drawer.open=false;f.scroll_drawer_top();
    let out=f.frame(vec![]);
    if visible_texts(&out,f.source_area()).iter().any(|s|s=="변경 실행") {f.click_visible("추가 관리");}
    f.click_visible("조회");f.until(|app|app.pending.is_empty()&&app.panes.projection.contains_key("ui.traffic"));
    assert!(f.app.error.is_none(),"{:?}",f.app.error);f.app.panes.projection["ui.traffic"].clone()
}

#[test]
fn domain_native_traffic_host_filters_full_exchange_batch_and_single_deletes_survive_reopen() {
    let mut f=domain_fixture();let result=native_traffic(&mut f);assert_eq!(result["total"],4);
    f.assert_drawer_texts(&["https://domain-fixture.invalid/evidence","https://preserved-traffic.invalid/evidence"]);
    f.click_visible("https://domain-fixture.invalid/evidence");
    f.until(|app|app.pending.is_empty()&&app.panes.projection.contains_key("ui.trafficExchange"));
    assert!(f.app.panes.projection["ui.trafficExchange"]["resp"]["text"].as_str().unwrap().contains("DOMAIN CAPTURED BODY"));
    f.assert_drawer_texts(&["DOMAIN CAPTURED BODY"]);
    f.reopen_acceptance(false);native_traffic(&mut f);f.click_visible("호스트 목록");
    f.until(|app|app.pending.is_empty()&&app.panes.projection.contains_key("ui.trafficHosts"));
    assert_eq!(f.app.panes.projection["ui.trafficHosts"]["hosts"].as_array().unwrap().len(),4);
    f.click_visible("캡처 호스트");f.assert_drawer_texts(&["domain-fixture.invalid (1)"]);f.click_visible("domain-fixture.invalid (1)");
    f.click_visible("조회");f.until(|app|app.pending.is_empty());
    assert_eq!(f.app.panes.projection["ui.traffic"]["total"],1);f.assert_drawer_texts(&["https://domain-fixture.invalid/evidence"]);
    f.click_visible("캡처 삭제를 확인합니다 (발견 사항에 고정된 증거는 유지)");f.click_visible("선택 호스트 캡처 삭제");f.prepare("ui.trafficDeleteHost");settled(&mut f);
    f.reopen_acceptance(false);let result=native_traffic(&mut f);assert_eq!(result["total"],3);assert!(!result.to_string().contains("https://domain-fixture.invalid/evidence"));f.assert_drawer_texts(&["https://preserved-traffic.invalid/evidence"]);
    let deleted=action(&mut f,Section::Traffic,None,"ui.trafficDeleteHosts",json!({"hosts":["secondary-fixture.invalid","batch-traffic.invalid"]}));assert_eq!(deleted["deleted"],2);
    f.reopen_acceptance(false);let result=native_traffic(&mut f);assert_eq!(result["total"],1);assert_eq!(result["exchanges"][0]["host"],"preserved-traffic.invalid");f.assert_drawer_texts(&["https://preserved-traffic.invalid/evidence"]);
    f.click_visible("캡처 삭제를 확인합니다 (발견 사항에 고정된 증거는 유지)");f.click_visible("전체 캡처 삭제");f.prepare("ui.trafficDeleteAll");settled(&mut f);
    f.reopen_acceptance(true);let result=native_traffic(&mut f);assert_eq!(result["total"],0);assert!(result["exchanges"].as_array().unwrap().is_empty());f.assert_drawer_texts(&["검색 0건 / 전체 0건"]);
    f.shutdown();accepted(&["ui.traffic","ui.trafficExchange","ui.trafficHosts","ui.trafficDeleteHost","ui.trafficDeleteHosts","ui.trafficDeleteAll"],"domain_native_traffic_host_filters_full_exchange_batch_and_single_deletes_survive_reopen");
}

fn source_export(f:&mut Fixture,method:&str,selected:Option<&str>,params:Value,path:&std::path::Path)->Value {
    let result=action(f,Section::Findings,selected,method,params);
    assert!(result["filename"].is_string(),"Expected original file metadata: {result}");
    TEST_EXPORT_DESTINATIONS.with(|queue|queue.borrow_mut().push_back(Some(path.to_path_buf())));
    f.click_visible("결과를 파일로 저장");settled(f);
    assert!(path.is_file(),"Native export must create the selected destination");result
}

#[test]
fn domain_finding_export_formats_and_binary_evidence_download_use_real_file_capabilities() {
    let mut f=Fixture::configured_seeded(|profile|ServiceConfig{host_factory:Some(transport::factory(profile,BTreeMap::new(),transport::Probe::default())),wake_ui:None},seeded_domain);let finding=row(&f,"findings","name","DOMAIN FINDING CONTEXT").header.id;
    let traffic=row(&f,"exchanges","host","domain-fixture.invalid").header.id;
    action(&mut f,Section::Findings,Some(&finding),"ui.bindFindingTraffic",json!({"id":finding,"traffic_refs":[{"traffic_id":traffic,"role":"proof","note":"DOMAIN EXPORT EVIDENCE"}]}));
    let binding=row(&f,"finding_traffic_bindings","note","DOMAIN EXPORT EVIDENCE").header.id;
    f.reopen_acceptance(true);
    for format in ["json","csv","md-single","md-zip"] {
        let path=f._dir.path().join(format!("finding-export-{format}"));
        source_export(&mut f,"ui.exportFindings",Some(&finding),json!({"opts":{"format":format,"scope":"selected","ids":[finding]}}),&path);
        let bytes=std::fs::read(&path).unwrap();assert!(!bytes.is_empty());
        if format=="md-zip" {
            use std::io::Read;
            let mut archive=zip::ZipArchive::new(std::io::Cursor::new(bytes)).unwrap();
            let names=archive.file_names().map(str::to_owned).collect::<Vec<_>>();
            let markdown=names.iter().find(|n|n.ends_with(".md")).unwrap();let mut text=String::new();archive.by_name(markdown).unwrap().read_to_string(&mut text).unwrap();
            assert!(text.contains("DOMAIN FINDING CONTEXT"));assert!(text.contains("DOMAIN EXPORT EVIDENCE"));assert!(!text.contains("PRESERVED FINDING CONTEXT"));
            let response=names.iter().find(|n|n.ends_with("/response.http")).unwrap();let mut text=String::new();archive.by_name(response).unwrap().read_to_string(&mut text).unwrap();assert!(text.contains("DOMAIN CAPTURED BODY"));
            assert!(names.iter().any(|n|n.ends_with("/manifest.json")));assert!(names.iter().any(|n|n.ends_with("/request.http")));
        }
        else {let text=String::from_utf8(bytes).unwrap();assert!(text.contains("DOMAIN FINDING CONTEXT"));assert!(!text.contains("PRESERVED FINDING CONTEXT"));if format=="json" {let rows:Value=serde_json::from_str(&text).unwrap();assert_eq!(rows.as_array().unwrap().len(),1);assert_eq!(rows[0]["id"],finding);}else if format=="csv" {assert!(text.starts_with('\u{feff}'));assert!(text.contains("Number of traffic evidence"));}else {assert!(text.contains("DOMAIN EXPORT EVIDENCE"));}}
    }
    let path=f._dir.path().join("evidence-response.bin");
    source_export(&mut f,"ui.downloadFindingTrafficBody",Some(&finding),json!({"id":finding,"bindingId":binding,"side":"response"}),&path);
    assert_eq!(std::fs::read(path).unwrap(),b"DOMAIN CAPTURED BODY");
    query(&mut f,Section::Findings,Some(&finding),"ui.findingTraffic",json!({"id":finding}),"DOMAIN EXPORT EVIDENCE");
    f.shutdown();accepted(&["ui.exportFindings","ui.downloadFindingTrafficBody"],"domain_finding_export_formats_and_binary_evidence_download_use_real_file_capabilities");
}

#[test]
fn domain_native_snapshot_task_finding_intent_lists_select_durable_records_after_reopen() {
    let mut f=domain_fixture();let task=task(&f);let finding=row(&f,"findings","name","DOMAIN FINDING CONTEXT").header.id;
    for reopen in [false,true] {
        if reopen {f.reopen_acceptance(true);}
        select(&mut f,Section::Overview,None);f.app.drawer.open=false;navigate(&mut f,Section::Tasks);
        left_record(&mut f,Section::Tasks,&task,"DOMAIN TASK CONTEXT");
        f.assert_drawer_texts(&["DOMAIN TASK CONTEXT","Only isolated fixture records"]);
        f.click_visible("개요");f.click_visible("탐색 노드");f.assert_drawer_texts(&["DOMAIN INTENT CONTEXT","DOMAIN FACT CONTEXT"]);
        select(&mut f,Section::Overview,None);navigate(&mut f,Section::Findings);
        left_record(&mut f,Section::Findings,&finding,"DOMAIN FINDING CONTEXT");f.assert_drawer_texts(&["DOMAIN FINDING CONTEXT","DOMAIN FINDING SUMMARY"]);
        query(&mut f,Section::Assets,None,"ui.assets",json!({"type":"service"}),"DOMAIN ASSET CONTEXT");
    }
    f.shutdown();accepted(&["ui.tasks","ui.findings","ui.intents","ui.assets"],"domain_native_snapshot_task_finding_intent_lists_select_durable_records_after_reopen");
    println!("DOMAIN_NATIVE_ADAPTER {}",json!({"methods":["ui.tasks","ui.findings","ui.intents"],"scope":"Approved integrated NativeApp snapshot read generations replace standalone list RPCs: actual navigation and task/finding object-row clicks select persistent records; native task exploration tab paints saved intent and fact; identical native actions after readonly reopen. No legacy RPC dispatch is fabricated."}));
}

#[test]
fn domain_intent_control_rejects_unrelated_task_scope_without_mutating_saved_evidence() {
    let mut f=domain_fixture();let finding=row(&f,"findings","name","DOMAIN FINDING CONTEXT");let intent=row(&f,"exploration_nodes","kind","intent");let other=source_task(&mut f,"DOMAIN UNRELATED OWNER");
    select(&mut f,Section::Tasks,Some(&other));f.app.drawer.open=false;
    let view=f.app.handle.view().unwrap();f.app.source_operations.seed_action_for_app_test(&view,Section::Tasks,Some(&other),"ui.controlIntent",json!({"taskId":other,"intentId":intent.header.id,"action":"cancel","reason":"DOMAIN UNRELATED CANCEL","mode":"soft"}));
    let out=f.frame(vec![]);if !visible_texts(&out,f.source_area()).iter().any(|s|s=="변경 실행") {f.click_visible("추가 관리");}
    f.click_visible("선택한 대상의 삭제·연결 해제·초기화를 확인했습니다.");f.click_visible("변경 실행");f.prepare("ui.controlIntent");f.until(|app|app.pending.is_empty());
    assert_eq!(f.app.error.as_ref().map(|e|e.code.as_str()),Some("PERMISSION_DENIED"));f.assert_drawer_texts(&["PERMISSION_DENIED"]);
    assert_eq!(f.app.handle.view().unwrap().generation.get("findings",&finding.header.id).unwrap(),&finding);
    assert_eq!(f.app.handle.view().unwrap().generation.get("exploration_nodes",&intent.header.id).unwrap(),&intent);
    // Dismiss the real displayed error before reopening; no failed mutation is
    // reinterpreted as success or used as a shortcut for the positive path.
    f.app.error=None;f.reopen_acceptance(true);
    let value=query(&mut f,Section::Findings,Some(&finding.header.id),"ui.getFinding",json!({"id":finding.header.id}),"DOMAIN FINDING EVIDENCE");assert_eq!(value["severity"],"high");
    f.shutdown();println!("DOMAIN_BOUNDARY {}",json!({"case":"domain_intent_control_rejects_unrelated_task_scope_without_mutating_saved_evidence","method":"ui.controlIntent","rejection":"PERMISSION_DENIED","unchanged_record":true,"readonly_reopen_original_evidence":true}));
}

#[test]
fn domain_task_model_chain_edits_show_actual_role_resolution_after_reopen() {
    let mut f=domain_fixture();let task=task(&f);
    // Profiles are contextual setup only. The qualified operation selects and
    // orders real profile identities through the task's public source editor.
    let first=configured_profile(&mut f,"DOMAIN CHAIN FIRST");
    let second=configured_profile(&mut f,"DOMAIN CHAIN SELECTED");
    action(&mut f,Section::Tasks,Some(&task),"ui.updateTaskLLMProfiles",json!({"id":task,"llmProfileIds":[first,second],"activeLLMProfileId":second}));
    let result=roundtrip_query(&mut f,Section::Tasks,Some(&task),"ui.taskLLMResolution",json!({"id":task}),"DOMAIN CHAIN SELECTED");
    for role in ["mainagent","planner","worker"] {assert_eq!(result[role]["profile_id"],second);assert_eq!(result[role]["source"],"task_chain");assert_eq!(result[role]["available"],true);}
    let links=f.app.handle.view().unwrap().generation.records("task_llm_profiles").into_iter().filter(|r|r.data["task_id"]==task).cloned().collect::<Vec<_>>();
    assert_eq!(links.len(),2);assert!(links.iter().any(|r|r.data["profile_id"]==first&&r.data["position"]==0));assert!(links.iter().any(|r|r.data["profile_id"]==second&&r.data["position"]==1));
    action(&mut f,Section::Tasks,Some(&task),"ui.updateTaskLLMProfiles",json!({"id":task,"llmProfileIds":[second,first],"activeLLMProfileId":first}));
    let result=roundtrip_query(&mut f,Section::Tasks,Some(&task),"ui.taskLLMResolution",json!({"id":task}),"DOMAIN CHAIN FIRST");
    assert_eq!(result["worker"]["profile_id"],first);assert_eq!(result["worker"]["available"],true);
    f.shutdown();accepted(&["ui.updateTaskLLMProfiles","ui.taskLLMResolution"],"domain_task_model_chain_edits_show_actual_role_resolution_after_reopen");
}

#[test]
fn domain_frontier_filters_completed_intent_and_keeps_open_priority_after_reopen() {
    let mut f=Fixture::configured_seeded(|_|ServiceConfig::default(),|path| {
        seeded_domain(path);let mut store=Store::open(path).unwrap();let s=store.snapshot();
        let task=s.records("tasks")[0];let mut p=Planner::new(&s,"fixture.domain.frontier",&json!({}));
        p.insert("exploration_nodes","node",json!({"exploration_id":task.data["exploration_id"],"kind":"intent","state":"open","payload":{"summary":"DOMAIN FRONTIER CONTEXT"},"priority":2})).unwrap();
        let mut batch=p.done(json!({}));batch.command_id=new_id("command");store.commit(batch).unwrap();store.checkpoint().unwrap();
    });
    let task=task(&f);let result=roundtrip_query(&mut f,Section::Tasks,Some(&task),"ui.frontier",json!({"task":task}),"DOMAIN FRONTIER CONTEXT");
    assert_eq!(result.as_array().unwrap().len(),1);assert_eq!(result[0]["state"],"open");assert_eq!(result[0]["priority"],2);assert!(!result.to_string().contains("DOMAIN INTENT CONTEXT"));
    f.shutdown();accepted(&["ui.frontier"],"domain_frontier_filters_completed_intent_and_keeps_open_priority_after_reopen");
}

fn archive_ready(f:&mut Fixture,task_id:&str)->String {
    f.until(|app|app.handle.view().unwrap().generation.records("task_archives").iter().any(|r|r.data["task_id"]==task_id&&r.data["state"]=="ready"));
    row(f,"task_archives","task_id",task_id).header.id
}
fn archive_native(f:&mut Fixture,task_id:&str)->String {
    select(f,Section::Tasks,Some(task_id));f.app.drawer.open=false;f.click_visible("보관");f.prepare("ui.archiveTask");archive_ready(f,task_id)
}
fn task_graph_payloads(f:&Fixture,task_id:&str)->Vec<Value> {
    let view=f.app.handle.view().unwrap();let exp=&view.generation.get("tasks",task_id).unwrap().data["exploration_id"];
    let mut values=view.generation.records("exploration_nodes").into_iter().filter(|r|r.data["exploration_id"]==*exp).map(|r|json!({"kind":r.data["kind"],"state":r.data["state"],"payload":r.data["payload"]})).collect::<Vec<_>>();values.sort_by_key(Value::to_string);values
}

#[test]
fn domain_archives_native_single_and_batch_preserve_graph_restore_then_delete_saved_packages() {
    let mut f=Fixture::configured_seeded(|_|ServiceConfig::default(),|path| {
        seeded_domain(path);let mut store=Store::open(path).unwrap();let snapshot=store.snapshot();let task=snapshot.records("tasks")[0];let finding=snapshot.records("findings").into_iter().find(|r|r.data["name"]=="DOMAIN FINDING CONTEXT").unwrap();
        // A legitimate pre-existing follow-up is prepared through its real
        // domain plan; only archive/restore controls are qualified in this case.
        let context=Context{actor:Actor{kind:"user".into(),agent_id:None,run_id:None,engine_epoch:new_id("epoch")},task_id:Some(task.header.id.clone()),intent_id:None,conversation_id:None};
        let mut plan=artex_domain::findings_ui::plan("ui.deepenFinding",&json!({"id":finding.header.id,"description":"DOMAIN ARCHIVE FOLLOWUP CONTEXT"}),&snapshot,&context).unwrap();artex_domain::augment_expected(&mut plan,&snapshot).unwrap();plan.command_id=new_id("command");store.commit(plan).unwrap();seed_apply(&mut store,&context,"task.pause",json!({"task_id":task.header.id}));store.checkpoint().unwrap();
    });let task=task(&f);let original=row(&f,"findings","name","DOMAIN FINDING CONTEXT");let finding=original.header.id.clone();
    let original_follow=f.app.handle.view().unwrap().generation.records("exploration_nodes").into_iter().find(|r|r.data["payload"]["summary"]=="DOMAIN ARCHIVE FOLLOWUP CONTEXT").unwrap().clone();assert_eq!(original_follow.data["payload"]["source_finding_id"],finding);assert_eq!(original_follow.data["payload"]["source_finding_node_id"],original.data["node_id"]);
    let second=source_task(&mut f,"DOMAIN ARCHIVE SECOND");let third=source_task(&mut f,"DOMAIN ARCHIVE THIRD");
    let original_second_graph=task_graph_payloads(&f,&second);let original_third_graph=task_graph_payloads(&f,&third);
    let main_archive=archive_native(&mut f,&task);
    let detail=roundtrip_query(&mut f,Section::Archives,Some(&main_archive),"ui.taskArchive",json!({"id":main_archive}),"DOMAIN TASK CONTEXT");
    assert_eq!(detail["state"],"ready");assert!(detail["compressed_size"].as_str().unwrap().parse::<u64>().unwrap()>0);
    assert!(f.app.handle.view().unwrap().generation.get("findings",&finding).is_none());
    // Native single restore and batch restore invoke distinct production
    // paths; both must finish import, not merely return queued=true.
    select(&mut f,Section::Archives,Some(&main_archive));f.app.drawer.open=false;f.click_visible("보관 작업 복구");f.prepare("ui.restoreTaskArchive");
    f.until(|app|app.handle.view().unwrap().generation.get("task_archives",&main_archive).is_none());f.reopen_acceptance(false);
    assert_eq!(f.app.handle.view().unwrap().generation.get("tasks",&task).unwrap().data["archived_at"],Value::Null);
    // The approved typed-store anti-ABA design forbids resurrecting a deleted
    // ID (implementation spec: tombstones). Go restored original graph IDs;
    // native import maps fresh graph identities and must preserve every
    // relationship and business byte. This qualifies a design replacement.
    let restored=row(&f,"findings","name","DOMAIN FINDING CONTEXT");let finding=restored.header.id.clone();
    assert_eq!(restored.data["task_id"],task);assert_eq!(restored.data["evidence"],artex_core::body("DOMAIN FINDING EVIDENCE"));
    assert_eq!(restored.data["report"],artex_core::body("DOMAIN FINDING REPAIR"));assert_eq!(restored.data["severity"],"high");assert_eq!(restored.data["summary"],"DOMAIN FINDING SUMMARY");
    let view=f.app.handle.view().unwrap();let restored_node=view.generation.get("exploration_nodes",restored.data["node_id"].as_str().unwrap()).unwrap();assert_eq!(restored_node.data["exploration_id"],view.generation.get("tasks",&task).unwrap().data["exploration_id"]);assert_eq!(view.generation.records("findings").len(),2);
    assert_ne!(finding,original.header.id);assert_ne!(restored.data["node_id"],original.data["node_id"]);
    let restored_follow=view.generation.records("exploration_nodes").into_iter().find(|r|r.data["payload"]["summary"]=="DOMAIN ARCHIVE FOLLOWUP CONTEXT").unwrap().clone();assert_ne!(restored_follow.header.id,original_follow.header.id);assert_eq!(restored_follow.data["payload"]["source_finding_id"],finding);assert_eq!(restored_follow.data["payload"]["source_finding_node_id"],restored.data["node_id"]);assert_eq!(restored_follow.data["payload"]["asset_ids"],restored.data["asset_ids"]);assert!(!restored.data["asset_ids"].as_array().unwrap().is_empty());
    let anchors=view.generation.records("exploration_anchors").iter().filter(|a|a.data["node_id"]==restored_follow.header.id).map(|a|a.data["asset_id"].clone()).collect::<Vec<_>>();assert_eq!(json!(anchors),restored.data["asset_ids"]);assert!(view.generation.records("exploration_edges").iter().any(|e|e.data["src_id"]==restored.data["node_id"]&&e.data["dst_id"]==restored_follow.header.id&&e.data["rel"]=="derived_from"));
    query(&mut f,Section::Findings,Some(&finding),"ui.getFinding",json!({"id":finding}),"DOMAIN FINDING EVIDENCE");
    let intents=query(&mut f,Section::Tasks,Some(&task),"ui.intentsPage",json!({"task":task,"limit":30}),"DOMAIN ARCHIVE FOLLOWUP CONTEXT");let index=intents["items"].as_array().unwrap().iter().position(|r|r["id"]==restored_follow.header.id).unwrap().to_string();f.source_result_at_path_visible("ui.intentsPage",Some(&task),&["items",&index,"payload"],&finding);
    let admission=action(&mut f,Section::Archives,None,"ui.archiveTasks",json!({"taskIds":[second,third]}));
    assert_eq!(admission["items"].as_array().unwrap().len(),2);assert!(admission["items"].as_array().unwrap().iter().all(|r|r["ok"]==true));
    let a2=archive_ready(&mut f,&second);let a3=archive_ready(&mut f,&third);
    let listed=roundtrip_query(&mut f,Section::Archives,None,"ui.taskArchives",json!({"input":{"q":"DOMAIN ARCHIVE","state":"ready","page":0,"size":30}}),"DOMAIN ARCHIVE SECOND");assert_eq!(listed["total"],2);
    action(&mut f,Section::Archives,None,"ui.restoreTaskArchives",json!({"archiveIds":[a2,a3]}));
    f.until(|app|app.handle.view().unwrap().generation.records("task_archives").is_empty());f.reopen_acceptance(false);
    assert_eq!(task_graph_payloads(&f,&second),original_second_graph);assert_eq!(task_graph_payloads(&f,&third),original_third_graph);
    assert!(f.app.handle.view().unwrap().generation.records("tasks").iter().all(|r|r.data["paused"]==true&&r.data["queued"]==false));assert!(f.app.handle.view().unwrap().generation.records("agent_runs").iter().all(|r|!matches!(r.data["state"].as_str(),Some("queued"|"running"|"waiting"))));
    query(&mut f,Section::Tasks,Some(&second),"ui.task",json!({"id":second}),"DOMAIN ARCHIVE SECOND");query(&mut f,Section::Tasks,Some(&third),"ui.task",json!({"id":third}),"DOMAIN ARCHIVE THIRD");
    let main_archive=archive_native(&mut f,&task);action(&mut f,Section::Archives,None,"ui.archiveTasks",json!({"taskIds":[second,third]}));
    let a2=archive_ready(&mut f,&second);let a3=archive_ready(&mut f,&third);
    action(&mut f,Section::Archives,Some(&main_archive),"ui.deleteTaskArchive",json!({"id":main_archive}));
    f.until(|app|app.handle.view().unwrap().generation.get("task_archives",&main_archive).is_none());f.reopen_acceptance(false);
    let preserved=query(&mut f,Section::Archives,None,"ui.taskArchives",json!({"input":{"page":0,"size":30}}),"DOMAIN ARCHIVE SECOND");assert_eq!(preserved["total"],2);assert!(!preserved.to_string().contains("DOMAIN TASK CONTEXT"));
    action(&mut f,Section::Archives,None,"ui.deleteTaskArchives",json!({"archiveIds":[a2,a3]}));
    f.until(|app|app.handle.view().unwrap().generation.records("task_archives").is_empty());f.reopen_acceptance(true);
    let empty=action(&mut f,Section::Archives,None,"ui.taskArchives",json!({"input":{"page":0,"size":30}}));assert_eq!(empty["total"],0);assert!(empty["items"].as_array().unwrap().is_empty());f.source_result_at_path_visible("ui.taskArchives",None,&["total"],"0");
    f.shutdown();accepted(&["ui.archiveTask","ui.taskArchive","ui.taskArchives","ui.restoreTaskArchive","ui.archiveTasks","ui.restoreTaskArchives","ui.deleteTaskArchive","ui.deleteTaskArchives"],"domain_archives_native_single_and_batch_preserve_graph_restore_then_delete_saved_packages");
    println!("DOMAIN_NATIVE_ADAPTER {}",json!({"methods":["ui.restoreTaskArchive","ui.restoreTaskArchives"],"scope":"Approved typed-store tombstone/anti-ABA design replacement (desktop-implementation-spec and sqlite-storage): original Go restores original graph IDs; native importer maps fresh compacted graph identities, keeps task stubs, exact finding evidence/ownership and node/exploration references, and exact batch task graph payloads. This is business/reference equivalence under changed identity semantics, never original-ID equivalence."}));
}

fn worker_only_context(path:&std::path::Path) {
    seeded_domain(path);let mut store=Store::open(path).unwrap();let s=store.snapshot();let mut p=Planner::new(&s,"fixture.domain.worker_enabled",&json!({}));
    for agent in s.records("agents") {let mut d=agent.data.clone();d["enabled"]=json!(agent.data["key"]=="worker");p.put("agents",&agent.header.id,d);}
    for trigger in s.records("agent_triggers") {let mut d=trigger.data.clone();d["enabled"]=json!(false);p.put("agent_triggers",&trigger.header.id,d);}
    let node=s.records("exploration_nodes").into_iter().find(|n|n.data["kind"]=="intent").unwrap();let mut d=node.data.clone();d["state"]=json!("blocked");d["blocked_reason"]=json!("fixture isolated blocked work");p.put("exploration_nodes",&node.header.id,d);
    let mut batch=p.done(json!({}));batch.command_id=new_id("command");store.commit(batch).unwrap();store.checkpoint().unwrap();
}
fn no_live_intent_runs(app:&NativeApp,id:&str)->bool {
    app.handle.view().unwrap().generation.records("agent_runs").iter().filter(|r|r.data["intent_id"]==id).all(|r|!matches!(r.data["state"].as_str(),Some("queued"|"running"|"waiting")))
}
fn worker_until(f:&mut Fixture,provider:&transport::LocalProvider,stage:&str,condition:impl Fn(&NativeApp)->bool) {
    let last=std::cell::RefCell::new(String::new());
    let failure=std::cell::RefCell::new(Vec::<Value>::new());
    f.until(|app| {
        let view=app.handle.view().unwrap();let seq=view.generation.snapshot.commit_seq;
        let failed=view.generation.records("agent_runs").into_iter().filter(|r|r.data["state"]=="failed"&&r.data["engine_epoch"]==view.workspace.scope.engine_epoch).collect::<Vec<_>>();
        if !failed.is_empty() {
            *failure.borrow_mut()=view.generation.records("activity").into_iter().filter(|row|row.data["is_error"]==true&&failed.iter().any(|run|row.data["node_id"]==run.data["intent_id"])).map(|row|json!({"summary":row.data["summary"],"detail":row.data["detail"]})).collect();
            assert!(!failure.borrow().is_empty(),"Failed worker must retain an error result activity: {failed:?}");return true;
        }
        let signature=json!({"requests":provider.requests().len(),"error":format!("{:?}",app.handle.status().last_error),"runs":view.generation.records("agent_runs").iter().map(|r|json!({"id":r.header.id,"state":r.data["state"]})).collect::<Vec<_>>(),"intents":view.generation.records("exploration_nodes").iter().filter(|r|r.data["kind"]=="intent").map(|r|r.data["state"].clone()).collect::<Vec<_>>()}).to_string();
        let changed=*last.borrow()!=signature;
        if changed {
            *last.borrow_mut()=signature;
            println!("DOMAIN_WORKER_STATE {}",json!({"stage":stage,"requests":provider.requests().len(),"seq":seq,"service_status":format!("{:?}",app.handle.status()),
                "tasks":view.generation.records("tasks").iter().map(|r|json!({"id":r.header.id,"status":r.data["status"],"paused":r.data["paused"],"queued":r.data["queued"]})).collect::<Vec<_>>(),
                "intents":view.generation.records("exploration_nodes").iter().filter(|r|r.data["kind"]=="intent").map(|r|json!({"id":r.header.id,"state":r.data["state"],"blocked_reason":r.data["blocked_reason"]})).collect::<Vec<_>>(),
                "runs":view.generation.records("agent_runs").iter().map(|r|json!({"id":r.header.id,"intent":r.data["intent_id"],"state":r.data["state"],"error":r.data["error"]})).collect::<Vec<_>>(),
                "agents":view.generation.records("agents").iter().filter(|r|r.data["enabled"]==true).map(|r|json!({"key":r.data["key"],"profile":r.data["llm_profile_id"]})).collect::<Vec<_>>() }));
        }
        condition(app)
    });
    let mut failure=failure.into_inner();
    if !failure.is_empty() {
        // Preserve the real terminal failure before TempDir teardown. Closing
        // first releases the writer; diagnostic blob reads retain Store's hash
        // verification and never steal an outstanding app callback.
        f.app.close();f.until(|app|app.handle.status().phase==ServicePhase::Closed&&app.pending.is_empty());
        let store=Store::open_read_only(&f.path).unwrap();
        for row in &mut failure {if row["detail"]["blob"].is_object() {let reference=row["detail"]["blob"].clone();let length=reference["length"].as_str().unwrap().parse::<usize>().unwrap();row["verified_detail"]=json!(String::from_utf8(store.blob_read(&reference,0,length).unwrap()).unwrap());}}
        panic!("Actual worker terminal failure at {stage}, provider requests {}: {}",provider.requests().len(),json!(failure));
    }
}
fn resume_native_task_if_paused(f:&mut Fixture,task:&str) {
    if f.app.handle.view().unwrap().generation.get("tasks",task).unwrap().data["paused"]!=true {return;}
    select(f,Section::Tasks,Some(task));f.app.drawer.open=false;f.scroll_drawer_top();let out=f.frame(vec![]);
    if visible_texts(&out,f.source_area()).iter().any(|s|s=="변경 실행"||s=="조회") {f.click_visible("추가 관리");}
    f.click_visible("재개");f.prepare("ui.controlTask");settled(f);
    assert_eq!(f.app.handle.view().unwrap().generation.get("tasks",task).unwrap().data["paused"],false);
}
fn local_answer(text:&str)->transport::Step {
    transport::Step::Reply(200,"application/json",json!({"choices":[{"message":{"content":text},"finish_reason":"stop"}],"usage":{"prompt_tokens":6,"completion_tokens":2}}).to_string())
}

#[test]
fn domain_live_intent_pause_resume_and_finding_followup_soft_hard_cancel_finish_real_runs() {
    let provider=transport::LocalProvider::from_handler(|index,_|match index {0|2|3=>transport::Step::Hold,1=>local_answer("DOMAIN WORKER RESUMED COMPLETE"),_=>panic!("Unexpected domain worker request {index}")});
    let secret=new_id("secret");
    let mut f=Fixture::configured_seeded(|profile|ServiceConfig{host_factory:Some(transport::factory(profile,BTreeMap::from([(secret.clone(),"domain-worker-fixture-key".into())]),transport::Probe::default())),wake_ui:None},worker_only_context);
    let profile=provider::profile(&mut f,"DOMAIN WORKER PROFILE","openai",&provider.url,&secret,false);
    f.setup("ui.saveAgentConfig",json!({"key":"worker","patch":{"llm_profile_id":profile}}));
    let task=task(&f);let intent=row(&f,"exploration_nodes","kind","intent").header.id;let finding=row(&f,"findings","name","DOMAIN FINDING CONTEXT").header.id;
    action(&mut f,Section::Tasks,Some(&task),"ui.rerunIntent",json!({"taskId":task,"intentId":intent}));
    worker_until(&mut f,&provider,"first worker request",|app|provider.requests().len()==1&&app.handle.view().unwrap().generation.get("exploration_nodes",&intent).unwrap().data["state"]=="running");
    action(&mut f,Section::Tasks,Some(&task),"ui.controlIntent",json!({"taskId":task,"intentId":intent,"action":"pause"}));
    f.until(|app|no_live_intent_runs(app,&intent)&&app.handle.view().unwrap().generation.get("exploration_nodes",&intent).unwrap().data["state"]=="paused");
    query(&mut f,Section::Tasks,Some(&task),"ui.intentsPage",json!({"task":task}),"paused");
    f.reopen_acceptance(false);let value=query(&mut f,Section::Tasks,Some(&task),"ui.intentsPage",json!({"task":task}),"paused");assert_eq!(value["items"].as_array().unwrap().iter().find(|r|r["id"]==intent).unwrap()["state"],"paused");
    action(&mut f,Section::Tasks,Some(&task),"ui.controlIntent",json!({"taskId":task,"intentId":intent,"action":"resume"}));
    resume_native_task_if_paused(&mut f,&task);
    worker_until(&mut f,&provider,"resumed worker complete",|app|provider.requests().len()==2&&no_live_intent_runs(app,&intent)&&app.handle.view().unwrap().generation.get("exploration_nodes",&intent).unwrap().data["state"]=="done");
    f.reopen_acceptance(false);query(&mut f,Section::Tasks,Some(&task),"ui.intentsPage",json!({"task":task}),"done");
    let original_finding=f.app.handle.view().unwrap().generation.get("findings",&finding).unwrap().clone();
    let source_node=original_finding.data["node_id"].as_str().unwrap().to_owned();
    let source_assets=f.app.handle.view().unwrap().generation.records("exploration_anchors").iter().filter(|a|a.data["node_id"]==source_node).map(|a|a.data["asset_id"].as_str().unwrap().to_owned()).collect::<BTreeSet<_>>();
    assert!(!source_assets.is_empty(),"The actual report_finding preparation must retain affected-asset anchors");
    for (mode,description,requests) in [("soft","DOMAIN SOFT FOLLOWUP",3),("hard","DOMAIN HARD FOLLOWUP",4)] {
        // Reopening alone must not execute a Worker. The explicit follow-up
        // action below is responsible for reviving its task and starting work.
        assert_eq!(f.app.handle.view().unwrap().generation.get("tasks",&task).unwrap().data["paused"],true);
        for _ in 0..4 {f.frame(vec![]);}
        assert_eq!(provider.requests().len(),requests-1,"Reopening must preserve the no-automatic-resume policy");
        let result=action(&mut f,Section::Findings,Some(&finding),"ui.deepenFinding",json!({"id":finding,"description":description}));let follow=result["intent_id"].as_str().unwrap().to_owned();
        f.until(|app|provider.requests().len()==requests&&app.handle.view().unwrap().generation.get("exploration_nodes",&follow).unwrap().data["state"]=="running");
        let snapshot=f.app.handle.view().unwrap();
        assert!(snapshot.generation.records("exploration_edges").iter().any(|e|e.data["src_id"]==source_node&&e.data["dst_id"]==follow&&e.data["rel"]=="derived_from"));
        let copied=snapshot.generation.records("exploration_anchors").iter().filter(|a|a.data["node_id"]==follow).map(|a|a.data["asset_id"].as_str().unwrap().to_owned()).collect::<BTreeSet<_>>();assert_eq!(copied,source_assets);
        let payload=&snapshot.generation.get("exploration_nodes",&follow).unwrap().data["payload"];assert_eq!(payload["source_finding_id"],finding);assert_eq!(payload["source_finding_node_id"],source_node);assert_eq!(payload["asset_ids"],json!(source_assets));
        let outbound=provider.requests()[requests-1].to_string();assert!(outbound.contains(description)&&outbound.contains(&finding)&&outbound.contains(&source_node));for asset in &source_assets {assert!(outbound.contains(asset),"The real worker request must retain assigned asset context");}
        query(&mut f,Section::Tasks,Some(&task),"ui.intentsPage",json!({"task":task}),description);
        action(&mut f,Section::Tasks,Some(&task),"ui.controlIntent",json!({"taskId":task,"intentId":follow,"action":"cancel","reason":"DOMAIN USER CANCEL REASON","mode":mode}));f.until(|app|no_live_intent_runs(app,&follow));
        f.reopen_acceptance(false);
        if mode=="soft" {let value=query(&mut f,Section::Tasks,Some(&task),"ui.intentsPage",json!({"task":task}),"DOMAIN USER CANCEL REASON");let rows=value["items"].as_array().unwrap();let index=rows.iter().position(|r|r["id"]==follow).unwrap();let row=&rows[index];assert_eq!(row["state"],"deleted");assert_eq!(row["delete_reason"],"DOMAIN USER CANCEL REASON");f.source_result_at_path_visible("ui.intentsPage",Some(&task),&["items",&index.to_string(),"payload"],"DOMAIN SOFT FOLLOWUP");}
        else {assert!(f.app.handle.view().unwrap().generation.get("exploration_nodes",&follow).is_none());let value=query(&mut f,Section::Tasks,Some(&task),"ui.intentsPage",json!({"task":task}),"DOMAIN INTENT CONTEXT");assert!(!value.to_string().contains("DOMAIN HARD FOLLOWUP"));}
        assert_eq!(f.app.handle.view().unwrap().generation.get("findings",&finding).unwrap(),&original_finding,"Follow-up controls preserve the original finding and evidence after reopen");
        query(&mut f,Section::Findings,Some(&finding),"ui.getFinding",json!({"id":finding}),"DOMAIN FINDING EVIDENCE");
    }
    f.shutdown();assert_eq!(provider.finish().len(),4);accepted(&["ui.controlIntent","ui.deepenFinding"],"domain_live_intent_pause_resume_and_finding_followup_soft_hard_cancel_finish_real_runs");
}

#[test]
fn domain_rerun_all_blocked_executes_each_worker_and_batch_task_pause_resume_reports_each_target() {
    let provider=transport::LocalProvider::from_handler(|index,_| {assert!(index<2);local_answer("DOMAIN BLOCKED WORK COMPLETED")});let secret=new_id("secret");
    let mut f=Fixture::configured_seeded(|profile|ServiceConfig{host_factory:Some(transport::factory(profile,BTreeMap::from([(secret.clone(),"domain-batch-fixture-key".into())]),transport::Probe::default())),wake_ui:None},|path| {
        worker_only_context(path);let mut store=Store::open(path).unwrap();let s=store.snapshot();let task=s.records("tasks")[0];let mut p=Planner::new(&s,"fixture.domain.second_blocked",&json!({}));
        p.insert("exploration_nodes","node",json!({"exploration_id":task.data["exploration_id"],"kind":"intent","state":"blocked","blocked_reason":"fixture isolated second blocked work","payload":{"summary":"DOMAIN SECOND BLOCKED INTENT"}})).unwrap();let mut batch=p.done(json!({}));batch.command_id=new_id("command");store.commit(batch).unwrap();store.checkpoint().unwrap();
    });
    let profile=provider::profile(&mut f,"DOMAIN BATCH WORKER","openai",&provider.url,&secret,false);f.setup("ui.saveAgentConfig",json!({"key":"worker","patch":{"llm_profile_id":profile}}));
    let task=task(&f);let result=action(&mut f,Section::Tasks,Some(&task),"ui.rerunBlocked",json!({"taskId":task}));assert_eq!(result["reopened"],2);
    worker_until(&mut f,&provider,"both blocked workers complete",|app|provider.requests().len()==2&&app.handle.view().unwrap().generation.records("exploration_nodes").iter().filter(|r|r.data["kind"]=="intent").all(|r|r.data["state"]=="done"));
    f.reopen_acceptance(false);let result=query(&mut f,Section::Tasks,Some(&task),"ui.intentsPage",json!({"task":task}),"DOMAIN SECOND BLOCKED INTENT");assert!(result["items"].as_array().unwrap().iter().all(|r|r["state"]=="done"));assert!(result.to_string().contains("DOMAIN INTENT CONTEXT"));
    let second=source_task(&mut f,"DOMAIN TASK BATCH SECOND");stop(&mut f,&task);
    let resumed=action(&mut f,Section::Tasks,Some(&task),"ui.controlTasksBatch",json!({"taskIds":[task,second],"action":"resume"}));assert_eq!(resumed["items"].as_array().unwrap().len(),2);assert!(resumed["items"].as_array().unwrap().iter().all(|r|r["ok"]==true));
    for id in [&task,&second] {let value=query(&mut f,Section::Tasks,Some(id),"ui.task",json!({"id":id}),"running");assert_eq!(value["paused"],false);}
    // Workspace close deliberately pauses admitted tasks. Reopen must retain
    // their real admission evidence and display paused, never auto-resume.
    f.reopen_acceptance(false);
    for id in [&task,&second] {assert!(f.app.handle.view().unwrap().generation.records("domain_events").iter().any(|r|r.data["kind"]=="TaskResumed"&&r.data["subject_id"]==*id));let value=query(&mut f,Section::Tasks,Some(id),"ui.task",json!({"id":id}),"paused");assert_eq!(value["paused"],true);}
    action(&mut f,Section::Tasks,Some(&task),"ui.controlTasksBatch",json!({"taskIds":[task,second],"action":"resume"}));
    let invalid=new_id("task");let paused=action(&mut f,Section::Tasks,Some(&task),"ui.controlTasksBatch",json!({"taskIds":[task,second,invalid],"action":"pause"}));
    assert_eq!(paused["items"].as_array().unwrap().len(),3);assert_eq!(paused["items"].as_array().unwrap().iter().filter(|r|r["ok"]==true).count(),2);assert_eq!(paused["items"].as_array().unwrap().iter().find(|r|r["id"]==invalid).unwrap()["ok"],false);
    f.reopen_acceptance(true);
    for id in [&task,&second] {let value=query(&mut f,Section::Tasks,Some(id),"ui.task",json!({"id":id}),"paused");assert_eq!(value["paused"],true);}
    f.shutdown();assert_eq!(provider.finish().len(),2);accepted(&["ui.rerunBlocked","ui.controlTasksBatch"],"domain_rerun_all_blocked_executes_each_worker_and_batch_task_pause_resume_reports_each_target");
}

#[test]
fn domain_paused_worker_message_starts_dedicated_continuations_with_exact_history_and_no_replay_after_reopen() {
    let provider=transport::LocalProvider::from_handler(|index,_|match index {0|1=>transport::Step::Hold,2=>local_answer("DOMAIN HUMAN CONTINUATIONS COMPLETE"),_=>panic!("Unexpected continuation request {index}")});let secret=new_id("secret");
    let mut f=Fixture::configured_seeded(|profile|ServiceConfig{host_factory:Some(transport::factory(profile,BTreeMap::from([(secret.clone(),"domain-human-fixture-key".into())]),transport::Probe::default())),wake_ui:None},|path| {
        worker_only_context(path);let mut store=Store::open(path).unwrap();let snapshot=store.snapshot();let task=snapshot.records("tasks")[0];let mut p=Planner::new(&snapshot,"fixture.domain.human_intent",&json!({}));
        p.insert("exploration_nodes","node",json!({"exploration_id":task.data["exploration_id"],"kind":"intent","state":"blocked","payload":{"summary":"DOMAIN HUMAN CONTINUATION INTENT"}})).unwrap();let mut b=p.done(json!({}));b.command_id=new_id("command");store.commit(b).unwrap();store.checkpoint().unwrap();
    });
    let profile=provider::profile(&mut f,"DOMAIN HUMAN WORKER PROFILE","openai",&provider.url,&secret,false);f.setup("ui.saveAgentConfig",json!({"key":"worker","patch":{"llm_profile_id":profile}}));
    let task=task(&f);let intent=f.app.handle.view().unwrap().generation.records("exploration_nodes").into_iter().find(|r|r.data["payload"]["summary"]=="DOMAIN HUMAN CONTINUATION INTENT").unwrap().header.id.clone();
    action(&mut f,Section::Tasks,Some(&task),"ui.rerunIntent",json!({"taskId":task,"intentId":intent}));worker_until(&mut f,&provider,"first human-controlled worker",|app|provider.requests().len()==1&&app.handle.view().unwrap().generation.get("exploration_nodes",&intent).unwrap().data["state"]=="running");
    let mut admitted=Vec::new();
    for (index,message,request) in [(1,"DOMAIN HUMAN FIRST 지시 🧪","human:first"),(2,"DOMAIN HUMAN SECOND 지시 🧪","human:second")] {
        action(&mut f,Section::Tasks,Some(&task),"ui.controlIntent",json!({"taskId":task,"intentId":intent,"action":"pause"}));f.until(|app|no_live_intent_runs(app,&intent));
        let before=f.app.handle.view().unwrap();assert_eq!(before.generation.get("exploration_nodes",&intent).unwrap().data["state"],"paused");assert_eq!(before.generation.get("tasks",&task).unwrap().data["paused"],false);let old=before.generation.records("agent_runs").iter().map(|r|r.header.id.clone()).collect::<BTreeSet<_>>();
        let result=action(&mut f,Section::Tasks,Some(&task),"ui.sendWorkerMessage",json!({"taskId":task,"intentId":intent,"requestId":request,"message":format!("  {message}  ")}));assert_eq!(result["accepted"],true);assert_eq!(result["id"],intent);assert_eq!(result["request_id"],request);
        worker_until(&mut f,&provider,"human continuation reached actual provider",|app|provider.requests().len()==index+1&&(index==1||no_live_intent_runs(app,&intent)));
        let view=f.app.handle.view().unwrap();let created=view.generation.records("agent_runs").into_iter().filter(|r|!old.contains(&r.header.id)&&r.data["intent_id"]==intent).collect::<Vec<_>>();assert_eq!(created.len(),1);let run=created[0];assert_eq!(run.data["task_id"],task);assert_eq!(run.data["engine_epoch"],view.workspace.scope.engine_epoch);admitted.push(run.header.id.clone());
        let turns=view.generation.records("activity").into_iter().filter(|a|a.data["node_id"]==intent&&a.data["metadata"]["worker_message_request_id"]==request).collect::<Vec<_>>();assert_eq!(turns.len(),1);let packet:Value=serde_json::from_str(turns[0].data["detail"]["text"].as_str().unwrap()).unwrap();assert_eq!(packet["text"],message);assert_eq!(packet["display_text"],message);assert_eq!(packet["run_id"],run.header.id);
        let wire=provider.requests()[index].clone();let users=wire["messages"].as_array().unwrap().iter().filter(|m|m["role"]=="user").map(|m|m["content"].to_string()).collect::<Vec<_>>();assert_eq!(users.iter().filter(|s|s.contains(message)).count(),1);
        if index==2 {assert_eq!(users.iter().filter(|s|s.contains("DOMAIN HUMAN FIRST")).count(),1);assert!(users.iter().position(|s|s.contains("DOMAIN HUMAN FIRST")).unwrap()<users.iter().position(|s|s.contains("DOMAIN HUMAN SECOND")).unwrap());assert_eq!(run.data["parent_run_id"],admitted[0]);}
    }
    f.reopen_acceptance(true);for _ in 0..4 {f.frame(vec![]);}assert_eq!(provider.requests().len(),3);
    let history=query(&mut f,Section::Logs,None,"ui.activityHistory",json!({"task":task,"session":format!("intent:{intent}"),"limit":30}),"DOMAIN HUMAN FIRST");assert!(history.to_string().contains("DOMAIN HUMAN SECOND"));f.source_result_visible("ui.activityHistory",None,"DOMAIN HUMAN SECOND");
    let view=f.app.handle.view().unwrap();for id in &admitted {assert!(view.generation.get("agent_runs",id).is_some());}assert_eq!(view.generation.records("activity").iter().filter(|a|a.data["node_id"]==intent&&a.data["metadata"]["worker_message_request_id"].is_string()).count(),2);
    f.shutdown();assert_eq!(provider.finish().len(),3);accepted(&["ui.sendWorkerMessage"],"domain_paused_worker_message_starts_dedicated_continuations_with_exact_history_and_no_replay_after_reopen");
}
