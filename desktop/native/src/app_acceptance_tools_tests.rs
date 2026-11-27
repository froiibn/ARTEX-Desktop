//! Model-offered tools through the actual native composer, service and durable
//! result viewer. Historical graph/traffic below is fixture input, never a tool
//! result or a projection injected into the application under test.
use super::*;
use super::transport::*;
use artex_core::{Actor, Context, Snapshot};
use artex_domain::Planner;
use artex_store::Store;
use std::sync::Mutex;

#[derive(Clone, Default)]
struct Rich {
    task: String,
    other: String,
    company: String,
    asset: String,
    untested: String,
    fact: String,
    goal: String,
    intent: String,
    live: String,
    digest: String,
    finding: String,
    finding_node: String,
    traffic: String,
    new_traffic: String,
    hash: Value,
    mcp: String,
}

fn commit(store: &mut Store, mut batch: artex_core::CommandBatch) -> Value {
    batch.command_id = artex_core::new_id("command");
    artex_domain::augment_expected(&mut batch,&store.snapshot()).unwrap();
    store.commit(batch).unwrap().result
}
fn prep(store: &mut Store, method: &str, input: Value, context: &Context) -> Value {
    let batch=artex_domain::plan(method,&input,&store.snapshot(),context).unwrap();
    commit(store,batch)
}
fn seed_rich(path: &std::path::Path) -> Rich {
    let mut store=Store::create(path,"Actual model tool acceptance").unwrap();
    let mut context=Context { actor: Actor {kind:"user".into(),agent_id:None,run_id:None,engine_epoch:artex_core::new_id("epoch")},
        task_id:None,intent_id:None,conversation_id:None };
    let batch=artex_domain::bootstrap_plan(&store.snapshot(),&context).unwrap();
    commit(&mut store,batch);
    let task=prep(&mut store,"task.create",json!({"description":"TASK_MARKER","goal":"Only local fixture records","seed_first_intent":false}),&context)["task_id"].as_str().unwrap().to_owned();
    let other=prep(&mut store,"task.create",json!({"description":"OTHER_TASK_MARKER","goal":"Independent scope","seed_first_intent":false}),&context)["task_id"].as_str().unwrap().to_owned();
    context.task_id=Some(task.clone());
    let company=prep(&mut store,"add_company_scope",json!({"company":"Fixture Company","scope":"fixture.invalid","reason":"isolated preparation"}),&context)["company_id"].as_str().unwrap().to_owned();
    let asset=prep(&mut store,"insert_assets",json!({"assets":[{"type":"service","service_type":"http","url":"https://fixture.invalid/","company_id":company}]}),&context)["ids"][0].as_str().unwrap().to_owned();
    prep(&mut store,"add_task_scope",json!({"kind":"root_domain","value":"fixture.invalid","reason":"isolated preparation"}),&context);
    let fact=prep(&mut store,"record_fact",json!({"summary":"FACT_MARKER","detail":"confirmed local observation"}),&context)["ids"][0].as_str().unwrap().to_owned();
    let goal=prep(&mut store,"set_goals",json!({"text":"GOAL_MARKER"}),&context)["ids"][0].as_str().unwrap().to_owned();
    prep(&mut store,"set_goals",json!({"text":"SPARE_GOAL_MARKER"}),&context);
    let intent=prep(&mut store,"add_intent",json!({"summary":"TRACE_WORK_MARKER"}),&context)["ids"][0].as_str().unwrap().to_owned();
    let live=prep(&mut store,"add_intent",json!({"summary":"LIVE_WORK_MARKER"}),&context)["ids"][0].as_str().unwrap().to_owned();
    let finding_result=prep(&mut store,"report_finding",json!({"name":"FINDING_MARKER","summary":"FINDING_MARKER","vulnclass":"fixture-only","severity":"medium","evidence":"recorded fixture evidence"}),&context);
    let finding=finding_result["finding_id"].as_str().unwrap().to_owned();
    let finding_node=finding_result["finding_node_id"].as_str().unwrap().to_owned();
    let blank=store.blob_put(b"","application/octet-stream").unwrap();
    let response=store.blob_put(b"CAPTURED_MARKER","text/plain").unwrap();
    let head=store.blob_put(b"HTTP/1.1 200 OK\r\nContent-Type: text/plain\r\n","text/plain").unwrap();
    let snapshot=store.snapshot();
    let exp=snapshot.get("tasks",&task).unwrap().data["exploration_id"].clone();
    let aid=snapshot.records("agents").into_iter().find(|a|a.data["key"]=="mainagent").unwrap().header.id.clone();
    let mut p=Planner::new(&snapshot,"fixture.tool_inputs",&json!({}));
    for agent in snapshot.records("agents") {
        let mut d=agent.data.clone();
        d["enabled"]=json!(matches!(d["key"].as_str(),Some("mainagent"|"auto")));
        if d["enabled"]==true { d["interactive_shell"]=json!(true);d["web_search"]=json!(true); }
        p.put("agents",&agent.header.id,d);
        if matches!(agent.data["key"].as_str(),Some("mainagent"|"auto")) {
            p.insert("agent_features","agentfeature",json!({"agent_id":agent.header.id,"questions":true,"plan_mode":true,"memory":true})).unwrap();
        }
    }
    for tool in snapshot.records("tools") {
        let mut d=tool.data.clone();d["enabled"]=json!(true);d["agents"]=json!(["mainagent","auto"]);
        p.put("tools",&tool.header.id,d);
    }
    for trigger in snapshot.records("agent_triggers") {let mut d=trigger.data.clone();d["enabled"]=json!(false);p.put("agent_triggers",&trigger.header.id,d);}
    for task_id in [&task,&other] {
        let mut d=snapshot.get("tasks",task_id).unwrap().data.clone();d["paused"]=json!(true);d["queued"]=json!(false);d["status"]=json!("paused");p.put("tasks",task_id,d);
    }
    let mut done=snapshot.get("exploration_nodes",&intent).unwrap().data.clone();done["state"]=json!("done");p.put("exploration_nodes",&intent,done);
    let mut pending=snapshot.get("exploration_nodes",&live).unwrap().data.clone();pending["state"]=json!("paused");p.put("exploration_nodes",&live,pending);
    p.insert("agent_runs","run",json!({"agent_id":aid,"task_id":task,"intent_id":intent,"engine_epoch":artex_core::new_id("epoch"),"state":"completed"})).unwrap();
    p.insert("activity","activity",json!({"exploration_id":exp,"node_id":intent,"worker":"fixture-worker","kind":"result","summary":"TRACE_MARKER","detail":artex_core::body("TRACE_MARKER actual saved historical result"),"is_error":false})).unwrap();
    let digest=p.insert("exploration_nodes","node",json!({"exploration_id":exp,"kind":"digest","state":"active","payload":{"summary":"DIGEST_MARKER","body":artex_core::body("DIGEST_MARKER preserved context")}})).unwrap();
    p.insert("exploration_edges","edge",json!({"exploration_id":exp,"src_id":digest,"dst_id":fact,"rel":"covers"})).unwrap();
    let traffic=p.insert("exchanges","traffic",json!({"ts":chrono::Utc::now().timestamp().to_string(),"host":"fixture.invalid","url":"https://fixture.invalid/evidence","method":"GET","status":200,"req_len":"0","resp_len":b"CAPTURED_MARKER".len().to_string(),"content_type":"text/plain"})).unwrap();
    let new_traffic=p.insert("exchanges","traffic",json!({"ts":chrono::Utc::now().timestamp().to_string(),"host":"fixture.invalid","url":"https://fixture.invalid/second-evidence","method":"GET","status":200,"req_len":"0","resp_len":b"CAPTURED_MARKER".len().to_string(),"content_type":"text/plain"})).unwrap();
    let mut bodies=artex_core::default_data("exchange_bodies").unwrap();
    artex_domain::merge(&mut bodies,&json!({"id":traffic,"req_head":artex_core::body("GET /evidence HTTP/1.1\r\n"),"req_body":blank,"resp_head":{"text":null,"blob":head},"resp_body":response}));
    p.put("exchange_bodies",&artex_core::new_id(artex_core::collection_prefix("exchange_bodies").unwrap()),bodies);
    let mut second=artex_core::default_data("exchange_bodies").unwrap();
    artex_domain::merge(&mut second,&json!({"id":new_traffic,"req_head":artex_core::body("GET /second-evidence HTTP/1.1\r\n"),"req_body":blank,"resp_head":{"text":null,"blob":head},"resp_body":response}));
    p.put("exchange_bodies",&artex_core::new_id(artex_core::collection_prefix("exchange_bodies").unwrap()),second);
    p.insert("tools","toolconfig",json!({"key":"fixture_custom","kind":"command","system":false,"enabled":true,"deferred":true,"description":"CUSTOM_MARKER","agents":["mainagent","auto"],"exec":{"command":"echo EXTRA_EXECUTION_MARKER"},"schema":{"type":"object","properties":{}}})).unwrap();
    let mcp=p.insert("mcp_servers","mcp",json!({"name":"MCP_MARKER","transport":"stdio","command":"fixture-not-launched","enabled":false,"args":[],"env":{}})).unwrap();
    p.insert("memory_records","memory",json!({"agent_id":aid,"name":"MEMORY_MARKER","type":"project","description":"fixture recall","content":artex_core::body("MEMORY_MARKER preserved memory"),"version":"1"})).unwrap();
    for (key,value) in [("approval_bypass","true"),("agent_traffic_binding","true")] {
        if let Some(old)=snapshot.records("settings").into_iter().find(|r|r.data["key"]==key) {let mut d=old.data.clone();d["value"]=json!(value);p.put("settings",&old.header.id,d);} else {p.insert("settings","setting",json!({"key":key,"value":value})).unwrap();}
    }
    commit(&mut store,p.done(json!({})));
    prep(&mut store,"bind_finding_traffic",json!({"finding_id":finding,"traffic_refs":[{"traffic_id":traffic,"role":"proof","note":"TRAFFIC_BIND_MARKER"}]}),&context);
    let snapshot=store.snapshot();
    // insert_assets anchors the primary service to the task's origin fact,
    // which correctly counts it as tested. Its materialized root container is
    // the separate untested positive control; select by meaning, never UUID order.
    let roots=snapshot.records("assets").into_iter().filter(|a|a.data["type"]=="root_domain"&&a.data["domain"]=="fixture.invalid").collect::<Vec<_>>();
    assert_eq!(roots.len(),1);let untested=roots[0].header.id.clone();
    assert_eq!(snapshot.get("assets",&asset).unwrap().data["type"],"service");
    let (target,tested)=artex_domain::asset_graph_ui::scoped_coverage(&snapshot,&task).unwrap();
    assert!(target.contains(&untested)&&target.contains(&asset));assert!(tested.contains(&asset));assert!(!tested.contains(&untested));
    store.checkpoint().unwrap();
    Rich {task,other,company,asset,untested,fact,goal,intent,live,digest,finding,finding_node,traffic,new_traffic,hash:response["sha256"].clone(),mcp}
}

fn tool_input(r:&Rich,name:&str)->Value {
    match name {
        "add_company_scope"=>json!({"company":"NEW_COMPANY_MARKER","scope":"new.invalid","reason":"isolated recorded evidence"}),
        "add_hint"=>json!({"text":"NEW_HINT_MARKER"}),
        "add_task_hint"=>json!({"task_id":r.task,"text":"NEW_TASK_HINT_MARKER"}),
        "add_intent"=>json!({"summary":"NEW_INTENT_MARKER"}),
        "record_fact"=>json!({"summary":"NEW_FACT_MARKER","detail":"Confirmed isolated observation"}),
        "set_goals"=>json!({"text":"NEW_GOAL_MARKER"}),
        "set_constraints"=>json!({"text":"NEW_CONSTRAINT_MARKER","type":"deny"}),
        "add_task_scope"=>json!({"kind":"subdomain","value":"added.fixture.invalid","reason":"explicit isolated scope"}),
        "insert_assets"=>json!({"assets":[{"type":"service","service_type":"http","url":"https://unrelated.invalid/"}]}),
        "delete_assets_by_host"=>json!({"host":"fixture.invalid"}),
        "create_custom_tool"=>json!({"key":"new_fixture","kind":"command","exec":{"command":"echo fixture"},"schema":{"type":"object","properties":{}},"agents":["mainagent"],"description":"NEW_CUSTOM_MARKER"}),
        "update_custom_tool"=>json!({"key":"fixture_custom","kind":"command","exec":{"command":"echo fixture"},"schema":{"type":"object","properties":{}},"agents":["mainagent"],"description":"UPDATED_CUSTOM_MARKER"}),
        "create_mcp"=>json!({"name":"NEW_MCP_MARKER","transport":"stdio","command":"fixture-not-launched","enabled":false}),
        "update_mcp"=>json!({"id":r.mcp,"name":"UPDATED_MCP_MARKER","transport":"stdio","command":"fixture-not-launched","enabled":false}),
        "bind_finding_traffic"=>json!({"finding_id":r.finding,"traffic_refs":[{"traffic_id":r.new_traffic,"role":"proof","note":"NEW_BIND_MARKER"}]}),
        "prove_goal"=>json!({"goal_id":r.goal,"evidence_id":r.fact,"reason":"Recorded confirmed fixture observation"}),
        "kill_work"=>json!({"intent_id":r.live}),
        "spawn_task"=>json!({"description":"SPAWN_MARKER","goal":"Only the local fixture","seed_first_intent":false}),
        "RecordMemory"=>json!({"name":"NEW_MEMORY_MARKER","content":"NEW_MEMORY_CONTENT"}),
        "TodoWrite"=>json!({"todos":[{"content":"NEW_TODO_MARKER","status":"in_progress"}]}),
        "get_task_graph"|"list_task_findings"|"list_task_worker_traces"=>json!({"task_id":r.task}),
        "get_worker_output"|"get_worker_trace"=>json!({"intent_id":r.intent}),
        "get_task_worker_trace"=>json!({"task_id":r.task,"intent_id":r.intent}),
        "search_all_worker_traces"=>json!({"q":"TRACE_MARKER"}),
        "search_task_worker_traces"=>json!({"task_id":r.task,"q":"TRACE_MARKER"}),
        "node_detail"=>json!({"id":r.fact}),
        "get_task_node_detail"=>json!({"task_id":r.task,"id":r.fact}),
        "expand_digest"=>json!({"id":r.digest}),
        "list_assets"=>json!({"id":r.asset}),
        "get_finding_traffic"=>json!({"finding_id":r.finding}),
        "traffic_get"=>json!({"id":r.traffic}),
        "traffic_search"=>json!({"host":"fixture.invalid"}),
        "traffic_blob"=>json!({"hash":r.hash}),
        "RecallMemory"=>json!({"query":"MEMORY_MARKER"}),
        "SearchExtraTools"=>json!({"query":"select:fixture_custom"}),
        _=>json!({}),
    }
}
fn tool_marker(name:&str)->&str {
    match name {
        "add_company_scope"=>"company_id",
        "add_hint"|"add_task_hint"=>"hint added:",
        "add_intent"=>"intent created:",
        "record_fact"=>"fact recorded:",
        "set_goals"=>"goal added:",
        "set_constraints"=>"constraint added:",
        "add_task_scope"=>"added",
        "insert_assets"=>"results",
        "delete_assets_by_host"=>"deleted",
        "create_custom_tool"=>"custom tool created: new_fixture",
        "update_custom_tool"=>"custom tool updated: fixture_custom",
        "create_mcp"=>"NEW_MCP_MARKER",
        "update_mcp"=>"mcp updated:",
        "bind_finding_traffic"=>"NEW_BIND_MARKER",
        "prove_goal"=>"marked met",
        "kill_work"=>"Sent termination signal to intent",
        "spawn_task"=>"task created:",
        "RecordMemory"=>"NEW_MEMORY_MARKER",
        "TodoWrite"=>"NEW_TODO_MARKER",
        "graph_overview"|"get_task_graph"|"list_tasks"=>"TASK_MARKER",
        "list_companies"=>"Fixture Company",
        "list_assets"|"list_untested_assets"|"traffic_search"=>"fixture.invalid",
        "list_llm_profiles"=>"Acceptance model",
        "list_facts"|"node_detail"|"get_task_node_detail"=>"FACT_MARKER",
        "list_goals"=>"GOAL_MARKER",
        "list_findings"|"list_task_findings"=>"FINDING_MARKER",
        "expand_digest"=>"DIGEST_MARKER",
        "list_task_worker_traces"=>"TRACE_WORK_MARKER",
        "get_worker_output"|"get_worker_trace"|"get_task_worker_trace"|"search_all_worker_traces"|"search_task_worker_traces"=>"TRACE_MARKER",
        "traffic_get"|"traffic_blob"=>"CAPTURED_MARKER",
        "get_finding_traffic"=>"TRAFFIC_BIND_MARKER",
        "RecallMemory"=>"MEMORY_MARKER",
        "SearchExtraTools"=>"fixture_custom",
        _=>panic!("Explicit business oracle is missing for {name}"),
    }
}

fn verify_effect(snapshot:&Snapshot,r:&Rich,name:&str) {
    let has=|collection:&str,key:&str,expected:&str|snapshot.records(collection).iter().any(|row|row.data[key]==expected);
    match name {
        "add_company_scope"=>assert!(has("companies","name","NEW_COMPANY_MARKER")),
        "add_hint"|"add_task_hint"|"add_intent"|"record_fact"|"set_goals"=>{
            let (field,marker)=match name {"add_hint"=>("text","NEW_HINT_MARKER"),"add_task_hint"=>("text","NEW_TASK_HINT_MARKER"),"add_intent"=>("summary","NEW_INTENT_MARKER"),"record_fact"=>("summary","NEW_FACT_MARKER"),_=>("text","NEW_GOAL_MARKER")};
            let row=snapshot.records("exploration_nodes").into_iter().find(|row|row.data["payload"][field]==marker).unwrap_or_else(||panic!("Missing {name} stored payload"));
            assert_eq!(row.data["exploration_id"],snapshot.get("tasks",&r.task).unwrap().data["exploration_id"]);
        }
        "set_constraints"=>assert!(has("task_constraints","text","NEW_CONSTRAINT_MARKER")),
        "add_task_scope"=>assert!(has("task_scope","domain","added.fixture.invalid")),
        "insert_assets"=>assert!(has("assets","url","https://unrelated.invalid")),
        "delete_assets_by_host"=>{assert!(snapshot.get("assets",&r.asset).is_none());assert!(snapshot.get("companies",&r.company).is_some());assert!(has("assets","url","https://unrelated.invalid"),"Deleting fixture.invalid must preserve the separately inserted unrelated domain");},
        "create_custom_tool"=>assert!(has("tools","key","new_fixture")),
        "update_custom_tool"=>assert!(has("tools","description","UPDATED_CUSTOM_MARKER")),
        "create_mcp"=>assert!(has("mcp_servers","name","NEW_MCP_MARKER")),
        "update_mcp"=>assert_eq!(snapshot.get("mcp_servers",&r.mcp).unwrap().data["name"],"UPDATED_MCP_MARKER"),
        "bind_finding_traffic"=>{assert!(has("finding_traffic_bindings","note","NEW_BIND_MARKER"));assert_eq!(snapshot.get("findings",&r.finding).unwrap().data["evidence_version"],"2");},
        "prove_goal"=>{let row=snapshot.get("exploration_nodes",&r.goal).unwrap();assert_eq!(row.data["state"],"met");assert_eq!(row.data["payload"]["proof_node_id"],r.fact);},
        "kill_work"=>assert_eq!(snapshot.get("exploration_nodes",&r.live).unwrap().data["state"],"stopped"),
        "spawn_task"=>assert!(has("tasks","description","SPAWN_MARKER")),
        "RecordMemory"=>{let row=snapshot.records("memory_records").into_iter().find(|r|r.data["name"]=="NEW_MEMORY_MARKER").unwrap();assert_eq!(row.data["content"],artex_core::body("NEW_MEMORY_CONTENT"));},
        "TodoWrite"=>assert!(snapshot.records("agent_runs").iter().any(|r|r.data["todos"][0]["content"]=="NEW_TODO_MARKER")),
        _=>{},
    }
}
// An idle paused task is deliberately outside every claimed mutation scope.
// Preserve its full durable record, including identity and version, across
// execution and readonly reopen; checking only the intended effect is weaker.
fn unrelated_task(f:&Fixture,r:&Rich)->artex_core::Record {f.app.handle.view().unwrap().generation.get("tasks",&r.other).unwrap().clone()}
fn assert_unrelated_task(f:&Fixture,control:&artex_core::Record) {assert_eq!(f.app.handle.view().unwrap().generation.get("tasks",&control.header.id).unwrap(),control,"The unrelated idle task must remain byte-for-byte unchanged");}
fn reply_text(request:&Value,id:&str)->String {
    request["messages"].as_array().unwrap().iter().find(|m|m["role"]=="tool"&&m["tool_call_id"]==id)
        .unwrap_or_else(||panic!("Actual provider follow-up has no {id} result"))["content"].as_str().unwrap().to_owned()
}
fn tool_reply(id:&str,name:&str,input:Value)->Step {
    Step::Reply(200,"application/json",json!({"choices":[{"message":{"content":null,"tool_calls":[{"id":id,"type":"function","function":{"name":name,"arguments":input.to_string()}}]},"finish_reason":"tool_calls"}],"usage":{"prompt_tokens":4,"completion_tokens":2}}).to_string())
}
fn answer(text:&str)->Step {
    Step::Reply(200,"application/json",json!({"choices":[{"message":{"content":text},"finish_reason":"stop"}],"usage":{"prompt_tokens":6,"completion_tokens":2}}).to_string())
}
fn send_task(f:&mut Fixture,task:&str,message:&str) {
    f.app.drawer.open=false;select(f,Section::Chat,Some(task));
    let output=f.frame(vec![]);
    let pos=output.shapes.iter().find_map(|c| {
        let egui::epaint::Shape::Text(t)=&c.shape else{return None;};
        (t.galley.text().starts_with("메시지를 입력하세요.")&&c.clip_rect.contains_rect(t.visual_bounding_rect())).then(||t.visual_bounding_rect().center())
    }).expect("Task composer must be visible");
    f.frame(vec![egui::Event::PointerMoved(pos),egui::Event::PointerButton{pos,button:egui::PointerButton::Primary,pressed:true,modifiers:Default::default()}]);
    f.frame(vec![egui::Event::PointerButton{pos,button:egui::PointerButton::Primary,pressed:false,modifiers:Default::default()},egui::Event::Text(message.into())]);
    f.click_visible("보내기 · Ctrl+Enter");f.prepare("ui.chat");
}

fn show_tool_result(f:&mut Fixture,task:&str,name:&str,marker:&str) {
    show_tool_result_call(f,task,name,None,marker);
}
fn show_tool_result_call(f:&mut Fixture,task:&str,name:&str,call:Option<&str>,marker:&str) {
    f.app.drawer.open=false;select(f,Section::Chat,Some(task));
    let view=f.app.handle.view().unwrap();
    let conversation=view.generation.get("conversations",task).is_some();
    let exploration=view.generation.get("tasks",task).map(|r|r.data["exploration_id"].clone());
    let activity=view.generation.records(if conversation {"conversation_activities"} else {"activity"}).into_iter()
        .filter(|r|r.data["kind"]=="tool_result"&&r.data["tool"]==name&&call.is_none_or(|call|r.data["tool_use_id"]==call)&&if conversation {r.data["conversation_id"]==task} else {Some(&r.data["exploration_id"])==exploration.as_ref()})
        .max_by_key(|r|r.header.commit_seq.parse::<u64>().unwrap()).expect("Actual durable tool result activity").header.id.clone();
    let mut last_visible=Vec::new();let mut clicked=false;
    for _ in 0..30 {
        let output=f.frame(vec![]);
        last_visible=visible_texts(&output,egui::Rect::EVERYTHING);
        let observed=crate::panes::tool_result_observations().into_iter().find(|h|h.activity_id==activity&&h.name==name);
        // Navigate toward the actual target header, rather than assuming every
        // requested result is above the current scroll position.
        let scroll_direction=observed.as_ref().map(|h|if h.header.top()>=h.clip.bottom() {-1.0} else {1.0}).unwrap_or(1.0);
        if let Some(observed)=observed.filter(|h|h.clip.intersects(h.header)) {
            if let Some(body)=observed.body {
                // The actual response body belongs to this activity only.
                // Bind by the text layout origin, allowing glyph overhang only
                // inside the real viewport. A following standing-guidance
                // message has an origin outside this exact body allocation.
                if output.shapes.iter().any(|shape|match &shape.shape {
                    egui::epaint::Shape::Text(text)=>text.pos.y>=body.top()&&text.pos.y<body.bottom()
                        &&shape.clip_rect.contains_rect(text.visual_bounding_rect())
                        &&observed.clip.contains_rect(text.visual_bounding_rect())
                        &&text.galley.text().contains(marker),
                    _=>false,
                }) {return;}
            } else if !clicked {
                let pos=observed.header.intersect(observed.clip).center();
                f.frame(vec![egui::Event::PointerMoved(pos),egui::Event::PointerButton{pos,button:egui::PointerButton::Primary,pressed:true,modifiers:Default::default()}]);
                f.frame(vec![egui::Event::PointerButton{pos,button:egui::PointerButton::Primary,pressed:false,modifiers:Default::default()}]);
                clicked=true;
            }
            // Let the real egui disclosure finish its opening animation. Rapid
            // repeated toggles can keep its body clipped for every observation.
            std::thread::sleep(Duration::from_millis(10));
        } else {
            let area=f.ctx.content_rect();let pos=egui::pos2(area.center().x,area.center().y);
            f.frame(vec![egui::Event::PointerMoved(pos),egui::Event::MouseWheel{unit:egui::MouseWheelUnit::Point,delta:egui::vec2(0.,scroll_direction*area.height()*0.5),modifiers:Default::default()}]);
        }
    }
    panic!("The actual {name} activity {activity} result body never visibly rendered {marker}: {last_visible:?}");
}

fn query_group(names:&[&'static str]) {
    let rich=Arc::new(Mutex::new(Rich::default()));let seeded=rich.clone();let received=rich.clone();
    let names=names.to_vec();let offered=names.clone();
    let observed=Arc::new(Mutex::new(BTreeMap::<String,String>::new()));let seen=observed.clone();
    let provider=LocalProvider::from_handler(move |index,request| {
        let n=index/2;let name=offered[n];let id=format!("accept-{name}");
        if index%2==0 {
            assert!(request["tools"].as_array().unwrap().iter().any(|t|t["function"]["name"]==name),"Actual offer missing {name}");
            tool_reply(&id,name,tool_input(&received.lock().unwrap(),name))
        } else {
            let result=reply_text(request,&id);assert!(result.contains(tool_marker(name)),"{name}: {result}");
            if name=="list_tasks" {let r=received.lock().unwrap();assert!(result.contains(&r.task)&&result.contains(&r.other),"Both real task IDs must be present, independently of similar task names");}
            if matches!(name,"list_findings"|"list_task_findings") {assert!(result.contains(&received.lock().unwrap().finding_node));}
            if name=="list_assets" {assert!(result.contains(&received.lock().unwrap().asset),"The actual selected service must occur in list_assets: {result}");}
            if name=="list_untested_assets" {let r=received.lock().unwrap();let value:Value=serde_json::from_str(&result).unwrap();let rows=value["assets"].as_array().unwrap();assert!(rows.iter().any(|a|a["id"]==r.untested&&a["type"]=="root_domain"),"The actual untested root container must be returned: {result}");assert!(rows.iter().all(|a|a["id"]!=r.asset),"The service anchored to an origin fact must be excluded: {result}");}
            if name=="traffic_search" {let r=received.lock().unwrap();assert!(result.contains(&r.traffic)&&result.contains(&r.new_traffic),"A host echo alone cannot qualify traffic search; both actual saved exchanges must be returned");}
            seen.lock().unwrap().insert(name.into(),result);answer(&format!("ACCEPT_MODEL_DONE_{name}"))
        }
    });
    let secret=artex_core::new_id("secret");
    let mut f=Fixture::configured_seeded(|profile|ServiceConfig {host_factory:Some(factory(profile,BTreeMap::from([(secret.clone(),"isolated-tool-key".into())]),Probe::default())),wake_ui:None},move |path|*seeded.lock().unwrap()=seed_rich(path));
    let profile=provider::profile(&mut f,"Acceptance model","openai",&provider.url,&secret,false);
    f.setup("ui.activateLLMProfile",json!({"id":profile}));
    let r=rich.lock().unwrap().clone();let control=unrelated_task(&f,&r);
    for (index,name) in names.iter().enumerate() {
        send_task(&mut f,&r.task,&format!("Exercise only the local {name} fixture records."));
        let call=format!("accept-{name}");
        f.until(|app|app.pending.is_empty()&&app.handle.view().unwrap().generation.records("tool_invocations").iter().any(|i|i.data["tool_use_id"]==call&&i.data["state"]=="completed")
            &&provider.requests().len()==(index+1)*2&&app.handle.view().unwrap().generation.records("agent_runs").iter().filter(|a|a.data["task_id"]==r.task).all(|a|!matches!(a.data["state"].as_str(),Some("queued"|"running"|"waiting"))));
        f.assert_main_visible(&r.task,&format!("ACCEPT_MODEL_DONE_{name}"));
        show_tool_result(&mut f,&r.task,name,tool_marker(name));
        let invocation=f.app.handle.view().unwrap().generation.records("tool_invocations").into_iter().find(|i|i.data["tool_use_id"]==call).unwrap().clone();
        assert_eq!(invocation.data["name"],*name);assert!(invocation.data["result_ref"].is_object());
        verify_effect(&f.app.handle.view().unwrap().generation.snapshot,&r,name);assert_unrelated_task(&f,&control);
        assert!(observed.lock().unwrap().contains_key(*name));
        f.reopen_acceptance(true);
        assert_eq!(f.app.handle.view().unwrap().generation.get("tool_invocations",&invocation.header.id).unwrap(),&invocation);
        verify_effect(&f.app.handle.view().unwrap().generation.snapshot,&r,name);assert_unrelated_task(&f,&control);
        f.assert_main_visible(&r.task,&format!("ACCEPT_MODEL_DONE_{name}"));
        show_tool_result(&mut f,&r.task,name,tool_marker(name));
        println!("TOOL_ACCEPT {}",json!({"tool":name,"input":tool_input(&r,name),"semantic_marker":tool_marker(name),"model_received_actual_result":true,"display_and_readonly_reopen":true,"invocation":invocation.header.id}));
        if index+1<names.len(){f.reopen_acceptance(false);}
    }
    f.shutdown();assert_eq!(provider.finish().len(),names.len()*2);
}

#[test]
fn acceptance_tools_queries_companies_assets_tasks_profiles() {query_group(&["list_companies","list_assets","list_untested_assets","list_tasks","list_llm_profiles"]);}
#[test]
fn acceptance_tools_queries_graph_goals_facts_and_nodes() {query_group(&["graph_overview","list_goals","list_facts","node_detail","get_task_node_detail"]);}
#[test]
fn acceptance_tools_queries_graph_digest_findings_and_worker_index() {query_group(&["get_task_graph","expand_digest","list_findings","list_task_findings","list_task_worker_traces"]);}
#[test]
fn acceptance_tools_queries_worker_outputs_and_traces() {query_group(&["get_worker_output","get_worker_trace","get_task_worker_trace","search_all_worker_traces","search_task_worker_traces"]);}
#[test]
fn acceptance_tools_queries_saved_traffic_and_discovery() {query_group(&["traffic_search","traffic_get","traffic_blob","get_finding_traffic"]);}
#[test]
fn acceptance_tools_queries_memory_and_extra_discovery() {query_group(&["RecallMemory","SearchExtraTools"]);}

#[test]
fn acceptance_tools_mutate_company_scopes_hints_and_constraints() {query_group(&["add_company_scope","add_task_scope","add_hint","add_task_hint","set_constraints"]);}
#[test]
fn acceptance_tools_mutate_graph_facts_goals_and_pending_intents() {query_group(&["record_fact","set_goals","add_intent","prove_goal","kill_work"]);}
#[test]
fn acceptance_tools_mutate_assets_evidence_and_task_handover() {query_group(&["insert_assets","bind_finding_traffic","spawn_task","delete_assets_by_host"]);}
#[test]
fn acceptance_tools_mutate_custom_tool_and_mcp_definitions() {query_group(&["create_custom_tool","update_custom_tool","create_mcp","update_mcp"]);}
#[test]
fn acceptance_tools_mutate_memory_and_todos() {query_group(&["RecordMemory","TodoWrite"]);}

fn sdk_names(mode:&str)->Vec<&'static str> {
    match mode {
        "files"=>vec!["Edit","MultiEdit","Glob","Grep","LS"],
        "process"=>vec!["Bash","sleep"],
        "background"=>vec!["Monitor","TaskList","TaskOutput","TaskStop","TaskOutputAfterStop","TaskListAfterStop"],
        "extra"=>vec!["SearchExtraTools","ExecuteExtraTool"],
        "plan"=>vec!["EnterPlanMode","ExitPlanMode"],
        "pty"=>vec!["shell_open","shell_send","shell_close"],
        _=>panic!("Unknown SDK acceptance case"),
    }
}
// The final background step is a second real TaskOutput call, with its own
// call ID: acceptance of TaskStop does not prove asynchronous cleanup.
fn sdk_tool_name(stage:&str)->&str {match stage {"TaskOutputAfterStop"=>"TaskOutput","TaskListAfterStop"=>"TaskList",_=>stage}}
fn sequence_input(mode:&str,name:&str,observed:&BTreeMap<String,String>)->Value {
    let parsed=|name:&str|serde_json::from_str::<Value>(&observed[name]).unwrap();
    match name {
        "Edit"=>json!({"file_path":"sdk-acceptance.txt","old_string":"SDK_OLD_MARKER","new_string":"SDK_MIDDLE_MARKER"}),
        "MultiEdit"=>json!({"file_path":"sdk-acceptance.txt","edits":[{"old_string":"SDK_MIDDLE_MARKER","new_string":"SDK_FINAL_MARKER"},{"old_string":"UNCHANGED_LINE","new_string":"SECOND_EDIT_MARKER"}]}),
        "Glob"=>json!({"pattern":"*.txt"}),
        "Grep"=>json!({"pattern":"SDK_FINAL_MARKER","output_mode":"content"}),
        "LS"=>json!({"path":"."}),
        "Bash"=>json!({"command":"python -c \"print('SDK_BASH_MARKER')\""}),
        "sleep"=>json!({"seconds":1}),
        "Monitor"=>json!({"command":"python -u -c \"import time; print('SDK_MONITOR_OUTPUT', flush=True); time.sleep(30)\"","description":"SDK_MONITOR_MARKER"}),
        "TaskList"|"TaskListAfterStop"=>json!({}),
        "TaskOutput"=>json!({"task_id":parsed("Monitor")["execution_id"],"block":true,"timeout_ms":1000}),
        "TaskStop"=>json!({"task_id":parsed("Monitor")["execution_id"]}),
        "TaskOutputAfterStop"=>json!({"task_id":parsed("Monitor")["execution_id"],"block":true}),
        "SearchExtraTools"=>json!({"query":"select:fixture_custom"}),
        "ExecuteExtraTool"=>json!({"tool_name":"fixture_custom","params":{}}),
        "EnterPlanMode"=>json!({}),
        "ExitPlanMode"=>json!({"plan":"SDK_PLAN_MARKER: operate only on the isolated fixture"}),
        "shell_open"=>json!({"command":"python -u -c \"print('PTY_READY_MARKER', flush=True); value=input(); print('PTY_REPLY_'+value, flush=True)\"","mode":"interactive"}),
        "shell_send"=>json!({"session_id":parsed("shell_open")["session_id"],"text":"PTY_SEND_MARKER","submit":true,"wait":true,"prompt_regex":"PTY_REPLY_PTY_SEND_MARKER","timeout_ms":3000}),
        "shell_close"=>json!({"session_id":parsed("shell_open")["session_id"]}),
        _=>panic!("No real input for {mode}/{name}"),
    }
}
fn sequence_marker(name:&str)->&str {
    match name {
        "Edit"|"MultiEdit"=>"edited",
        "Glob"|"LS"=>"sdk-acceptance.txt",
        "Grep"=>"SDK_FINAL_MARKER",
        "Bash"=>"SDK_BASH_MARKER",
        "sleep"=>"slept_seconds",
        "Monitor"=>"execution_id",
        "TaskList"|"TaskListAfterStop"=>"execution_id",
        "TaskOutput"=>"SDK_MONITOR_OUTPUT",
        "TaskStop"=>"stop_requested",
        "TaskOutputAfterStop"=>"cleanup_confirmed",
        "SearchExtraTools"=>"fixture_custom",
        "ExecuteExtraTool"=>"EXTRA_EXECUTION_MARKER",
        "EnterPlanMode"=>"plan",
        "ExitPlanMode"=>"question_id",
        "shell_open"=>"session_id",
        "shell_send"=>"PTY_REPLY_PTY_SEND_MARKER",
        "shell_close"=>"closed",
        _=>panic!("No SDK semantic marker for {name}"),
    }
}
fn sdk_sequence(mode:&'static str) {
    let names=sdk_names(mode);let requested=names.clone();
    let rich=Arc::new(Mutex::new(Rich::default()));let seeded=rich.clone();
    let path=Arc::new(Mutex::new(None::<PathBuf>));let ready_path=path.clone();
    let actual=Arc::new(Mutex::new(BTreeMap::<String,String>::new()));let received=actual.clone();
    let provider=LocalProvider::from_handler(move |index,request| {
        if index>0 {
            let previous=requested[index-1];let result=reply_text(request,&format!("sdk-{previous}"));
            assert!(result.contains(sequence_marker(previous)),"SDK {previous}: {result}");
            if previous=="Edit" {assert!(std::fs::read_to_string(ready_path.lock().unwrap().as_ref().unwrap()).unwrap().contains("SDK_MIDDLE_MARKER"));}
            if previous=="MultiEdit" {assert_eq!(std::fs::read_to_string(ready_path.lock().unwrap().as_ref().unwrap()).unwrap(),"SDK_FINAL_MARKER\nSECOND_EDIT_MARKER\n");}
            if previous=="TaskList" {let listed:Value=serde_json::from_str(&result).unwrap();let monitor:Value=serde_json::from_str(&received.lock().unwrap()["Monitor"]).unwrap();assert!(listed["tasks"].as_array().unwrap().iter().any(|task|task["execution_id"]==monitor["execution_id"]&&task["state"]=="running"),"List the actual returned live Monitor execution");}
            if previous=="ExitPlanMode" {assert!(request["messages"].to_string().contains("approved"),"The separate actual user approval must reach the model after the waiting tool receipt");}
            if previous=="TaskStop" {let v:Value=serde_json::from_str(&result).unwrap();assert_eq!(v["stop_requested"],true);}
            if previous=="TaskOutputAfterStop" {let v:Value=serde_json::from_str(&result).unwrap();let monitor:Value=serde_json::from_str(&received.lock().unwrap()["Monitor"]).unwrap();let listed:Value=serde_json::from_str(&received.lock().unwrap()["TaskList"]).unwrap();let live=listed["tasks"].as_array().unwrap().iter().find(|t|t["execution_id"]==monitor["execution_id"]).unwrap();assert!(live["pid"].as_u64().unwrap()>0);assert_eq!(v["execution_id"],monitor["execution_id"]);assert_eq!(v["pid"],live["pid"]);assert_eq!(v["stop_requested"],true);assert_eq!(v["state"],"cancelled");assert_eq!(v["leader_exited"],true);assert_eq!(v["output_drained"],true);assert_eq!(v["cleanup_confirmed"],true);assert!(v["output"].as_str().unwrap().contains("SDK_MONITOR_OUTPUT"));}
            if previous=="TaskListAfterStop" {let listed:Value=serde_json::from_str(&result).unwrap();let monitor:Value=serde_json::from_str(&received.lock().unwrap()["Monitor"]).unwrap();let tasks=listed["tasks"].as_array().unwrap();assert_eq!(tasks.iter().filter(|t|t["state"]=="running").count(),0,"No live resources before the final model answer/run retirement");assert!(tasks.iter().any(|t|t["execution_id"]==monitor["execution_id"]&&t["cleanup_confirmed"]==true));}
            if previous=="shell_close" {let v:Value=serde_json::from_str(&result).unwrap();assert_eq!(v["accepted"],true);assert_eq!(v["cleanup_confirmed"],true);}
            received.lock().unwrap().insert(previous.into(),result);
        }
        if index==requested.len() {return answer(&format!("SDK_SEQUENCE_{mode}_COMPLETE"));}
        let stage=requested[index];let name=sdk_tool_name(stage);
        assert!(request["tools"].as_array().unwrap().iter().any(|t|t["function"]["name"]==name),"Actual SDK offer missing {name}");
        tool_reply(&format!("sdk-{stage}"),name,sequence_input(mode,stage,&received.lock().unwrap()))
    });
    let secret=artex_core::new_id("secret");let probe=Probe::default();
    let mut f=Fixture::configured_seeded(|profile|ServiceConfig{host_factory:Some(factory(profile,BTreeMap::from([(secret.clone(),"sdk-fixture-key".into())]),probe.clone())),wake_ui:None},move |workspace|*seeded.lock().unwrap()=seed_rich(workspace));
    let profile=provider::profile(&mut f,"Acceptance model","openai",&provider.url,&secret,false);
    f.setup("ui.activateLLMProfile",json!({"id":profile}));
    let r=rich.lock().unwrap().clone();let control=unrelated_task(&f,&r);
    let input=f.path.join("work").join("tasks").join(&r.task).join("sdk-acceptance.txt");
    std::fs::create_dir_all(input.parent().unwrap()).unwrap();let sibling=input.with_file_name("unrelated-control.txt");std::fs::write(&sibling,b"UNCHANGED_SDK_SIBLING").unwrap();std::fs::write(&input,"SDK_OLD_MARKER\nUNCHANGED_LINE\n").unwrap();*path.lock().unwrap()=Some(input.clone());
    send_task(&mut f,&r.task,&format!("Exercise only the local SDK {mode} sequence."));
    if mode=="plan" {
        f.until(|app|app.handle.view().unwrap().generation.records("user_questions").iter().any(|q|q.data["kind"]=="plan_approval"&&q.data["state"]=="pending"));
        f.click_visible("승인");f.prepare("user.answer");
    }
    f.until(|app|app.pending.is_empty()&&provider.requests().len()==names.len()+1&&app.handle.view().unwrap().generation.records("agent_runs").iter().filter(|r|r.data["task_id"]==rich.lock().unwrap().task).all(|r|!matches!(r.data["state"].as_str(),Some("queued"|"running"|"waiting"))));
    f.assert_main_visible(&r.task,&format!("SDK_SEQUENCE_{mode}_COMPLETE"));
    let snapshot=f.app.handle.view().unwrap().generation.snapshot.clone();
    let invocations=names.iter().map(|name| {
        let call=format!("sdk-{name}");let invocation=snapshot.records("tool_invocations").into_iter().find(|i|i.data["tool_use_id"]==call).unwrap();
        assert_eq!(invocation.data["state"],"completed");assert_eq!(invocation.data["name"],sdk_tool_name(name));assert!(invocation.data["result_ref"].is_object());invocation.clone()
    }).collect::<Vec<_>>();
    if mode=="files" {assert_eq!(std::fs::read_to_string(&input).unwrap(),"SDK_FINAL_MARKER\nSECOND_EDIT_MARKER\n");}
    if mode=="plan" {
        let question=snapshot.records("user_questions").into_iter().find(|q|q.data["kind"]=="plan_approval").unwrap();assert_eq!(question.data["state"],"answered");assert_eq!(question.data["answer"]["approved"],true);
        let run=snapshot.get("agent_runs",question.data["run_id"].as_str().unwrap()).unwrap();assert_ne!(run.data["permission_mode"],"plan");
    }
    assert_unrelated_task(&f,&control);assert_eq!(std::fs::read(&sibling).unwrap(),b"UNCHANGED_SDK_SIBLING");
    for name in &names {show_tool_result_call(&mut f,&r.task,sdk_tool_name(name),Some(&format!("sdk-{name}")),sequence_marker(name));}
    f.reopen_acceptance(true);assert_unrelated_task(&f,&control);assert_eq!(std::fs::read(&sibling).unwrap(),b"UNCHANGED_SDK_SIBLING");
    for invocation in &invocations {assert_eq!(f.app.handle.view().unwrap().generation.get("tool_invocations",&invocation.header.id).unwrap(),invocation);}
    for name in &names {
        show_tool_result_call(&mut f,&r.task,sdk_tool_name(name),Some(&format!("sdk-{name}")),sequence_marker(name));
        println!("TOOL_ACCEPT {}",json!({"tool":sdk_tool_name(name),"stage":name,"case":mode,"actual_follow_up_result":true,"actual_tool_result_display":true,"readonly_reopen":true,"input":sequence_input(mode,name,&actual.lock().unwrap())}));
    }
    if mode=="files" {assert_eq!(std::fs::read_to_string(&input).unwrap(),"SDK_FINAL_MARKER\nSECOND_EDIT_MARKER\n");}
    let effects=probe.results.lock().unwrap();
    for stage in &names {let name=sdk_tool_name(stage);
        if !["SearchExtraTools","EnterPlanMode","ExitPlanMode"].contains(&name) {
            assert!(effects.iter().any(|effect|effect["request"]["input"]["name"]==name&&effect.get("error").is_none()),"{name} must really reach NativeGateway");
        }
    }
    drop(effects);f.shutdown();assert_eq!(provider.finish().len(),names.len()+1);
}

#[test]
fn acceptance_tools_sdk_file_edits_search_and_listing() {sdk_sequence("files");}
#[test]
fn acceptance_tools_sdk_foreground_process_and_sleep() {sdk_sequence("process");}
#[test]
fn acceptance_tools_sdk_monitor_list_output_stop_real_child() {sdk_sequence("background");}
#[test]
fn acceptance_tools_sdk_deferred_discovery_and_actual_execution() {sdk_sequence("extra");}
#[test]
fn acceptance_tools_sdk_plan_entry_actual_user_approval_and_exit() {sdk_sequence("plan");}
#[test]
fn acceptance_tools_sdk_pty_send_and_close_returned_session() {sdk_sequence("pty");}

fn read_verified_body(f:&mut Fixture,body:Value)->String {
    assert!(f.app.pending.is_empty());
    let ticket=f.app.handle.try_submit(NativeAction::Query{scope:f.app.handle.scope().unwrap(),query:NativeQuery::Body{body}}).unwrap();
    let started=Instant::now();
    loop {
        for completion in f.app.handle.drain_completions() {
            assert_eq!(completion.ticket,ticket,"A fixture Body read must not steal another app request");
            return completion.result.unwrap().value.as_str().unwrap().to_owned();
        }
        assert!(started.elapsed()<Duration::from_secs(30));std::thread::sleep(Duration::from_millis(10));
    }
}

#[test]
fn acceptance_tools_compress_actual_read_pair_preserves_verified_archive_and_reopen() {
    let rich=Arc::new(Mutex::new(Rich::default()));let seeded=rich.clone();
    let block=Arc::new(Mutex::new(Value::Null));let observed=block.clone();
    let source_result=Arc::new(Mutex::new(String::new()));let actual_read=source_result.clone();
    let provider=LocalProvider::from_handler(move |index,request|match index {
        0=>{
            assert!(request["tools"].as_array().unwrap().iter().any(|t|t["function"]["name"]=="Compress"));
            tool_reply("compress-source","Read",json!({"file_path":"compress-source.txt"}))
        }
        1=>{
            let original=reply_text(request,"compress-source");assert!(original.contains("NOA_ORIGINAL_MARKER"));
            *actual_read.lock().unwrap()=original.split("\n<noa-ref").next().unwrap().to_owned();
            let reference=original.split("<noa-ref id=\"").nth(1).unwrap().split('"').next().unwrap();
            tool_reply("actual-compress","Compress",json!({"content":[{"startId":reference,"endId":reference,
                "summary":"NOA_SUMMARY_MARKER: preserved the complete isolated original Read exchange, its file path and repeated evidence; no additional safety conclusion was inferred."}]}))
        }
        2=>{
            let text=reply_text(request,"actual-compress");let result:Value=serde_json::from_str(&text).unwrap_or_else(|error|panic!("Compress actual result {text}: {error}"));
            assert_eq!(result["originals_preserved"],true);assert_eq!(result["blocks"].as_array().unwrap().len(),1);
            assert!(request.to_string().contains("NOA_SUMMARY_MARKER"));
            assert!(!request["messages"].to_string().contains("NOA_ORIGINAL_MARKER"),"The covered Read exchange must leave the active model context while its exact original remains archived");
            *observed.lock().unwrap()=result["blocks"][0].clone();answer("COMPRESS_ACTUAL_ARCHIVE_COMPLETE")
        }
        _=>panic!("Compress must not replay the source file work"),
    });
    let secret=artex_core::new_id("secret");
    let mut f=Fixture::configured_seeded(|profile|ServiceConfig {host_factory:Some(factory(profile,BTreeMap::from([(secret.clone(),"compression-fixture-key".into())]),Probe::default())),wake_ui:None},move |path|*seeded.lock().unwrap()=seed_rich(path));
    let profile=provider::profile(&mut f,"Acceptance model","openai",&provider.url,&secret,false);
    f.setup("ui.activateLLMProfile",json!({"id":profile}));f.setup("ui.setSettings",json!({"patch":{"noa_compaction":true}}));
    let task=rich.lock().unwrap().task.clone();let file=f.path.join("work").join("tasks").join(&task).join("compress-source.txt");
    // Read deliberately caps each long line at 2000 bytes. Multiple ordinary
    // lines exercise the 5000-character Compress contract without truncation.
    std::fs::create_dir_all(file.parent().unwrap()).unwrap();let original="NOA_ORIGINAL_MARKER preserved local observation\n".repeat(200);std::fs::write(&file,&original).unwrap();
    send_task(&mut f,&task,"Archive the actual local read exchange through the offered Compress tool.");
    f.until(|app|app.pending.is_empty()&&provider.requests().len()==3&&app.handle.view().unwrap().generation.records("agent_runs").iter().filter(|r|r.data["task_id"]==task).all(|r|!matches!(r.data["state"].as_str(),Some("queued"|"running"|"waiting"))));
    f.assert_main_visible(&task,"COMPRESS_ACTUAL_ARCHIVE_COMPLETE");show_tool_result(&mut f,&task,"Compress","originals_preserved");
    let actual=block.lock().unwrap().clone();let body=json!({"text":null,"blob":actual["archive"]});let archive=read_verified_body(&mut f,body.clone());
    let decoded:Value=serde_json::from_str(&archive).unwrap();assert_eq!(decoded["protocol"],"artex-compression/1");assert_eq!(decoded["messages"].as_array().unwrap().len(),2);
    assert!(decoded["messages"].as_array().unwrap().iter().any(|m|m["message"]["content"].as_array().unwrap().iter().any(|b|b["type"]=="tool_result"&&b["content"]==*source_result.lock().unwrap())),"The complete original Read result must survive byte for byte in the verified archive");
    let invocation=f.app.handle.view().unwrap().generation.records("tool_invocations").into_iter().find(|i|i.data["name"]=="Compress").unwrap().clone();assert_eq!(invocation.data["state"],"completed");
    f.reopen_acceptance(true);assert_eq!(read_verified_body(&mut f,body),archive);
    assert_eq!(f.app.handle.view().unwrap().generation.get("tool_invocations",&invocation.header.id).unwrap(),&invocation);
    show_tool_result(&mut f,&task,"Compress","originals_preserved");f.shutdown();assert_eq!(provider.finish().len(),3);
    println!("TOOL_ACCEPT {}",json!({"tool":"Compress","actual_model_reference":true,"complete_read_exchange_archived":true,"verified_archive_original_bytes":true,"display_and_readonly_reopen":true}));
}

fn seed_agent_patch(path:&std::path::Path,key:&str,patch:Value)->String {
    let mut store=Store::open(path).unwrap();let snapshot=store.snapshot();
    let agent=snapshot.records("agents").into_iter().find(|a|a.data["key"]==key).unwrap();
    let mut d=agent.data.clone();artex_domain::merge(&mut d,&patch);
    let mut p=Planner::new(&snapshot,"fixture.agent_config",&json!({}));p.put("agents",&agent.header.id,d);
    let id=agent.header.id.clone();commit(&mut store,p.done(json!({})));store.checkpoint().unwrap();id
}

#[test]
fn acceptance_tools_skill_create_update_and_scoped_load_actual_files_and_reopen() {
    let rich=Arc::new(Mutex::new(Rich::default()));let seeded=rich.clone();
    let provider=LocalProvider::from_handler(|index,request|match index {
        0=>tool_reply("skill-create","create_skill",json!({"name":"created-skill","description":"Created by the actual model tool","instructions":"CREATED_SKILL_MARKER"})),
        1=>{
            assert!(reply_text(request,"skill-create").contains("created-skill"));
            tool_reply("skill-update","update_skill_file",json!({"name":"starter-skill","file":"scripts/note.txt","content":"SKILL_RESOURCE_MARKER"}))
        }
        2=>{
            assert!(reply_text(request,"skill-update").contains("true"));
            assert!(request["tools"].as_array().unwrap().iter().any(|t|t["function"]["name"]=="Skill"));
            tool_reply("skill-load","Skill",json!({"name":"starter-skill"}))
        }
        3=>{assert!(reply_text(request,"skill-load").contains("SKILL_INSTRUCTIONS_MARKER"));answer("SKILL_SEQUENCE_COMPLETE")}
        _=>panic!("Unexpected Skill provider request"),
    });
    let secret=artex_core::new_id("secret");let probe=Probe::default();
    let mut f=Fixture::configured_seeded(|profile|{
        // An existing approved original is fixture input. Only created-skill
        // and scripts/note.txt are effects claimed for the model tools.
        let dir=profile.join("shared-resources/skills/starter-skill");std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("SKILL.md"),"---\nname: starter-skill\ndescription: Local acceptance original\n---\nSKILL_INSTRUCTIONS_MARKER: inspect only the local supplied note.\n").unwrap();
        ServiceConfig{host_factory:Some(factory(profile,BTreeMap::from([(secret.clone(),"skill-fixture-key".into())]),probe.clone())),wake_ui:None}
    },move |path|{
        *seeded.lock().unwrap()=seed_rich(path);
        let mut store=Store::open(path).unwrap();let snapshot=store.snapshot();let agent=snapshot.records("agents").into_iter().find(|a|a.data["key"]=="mainagent").unwrap();
        let mut p=Planner::new(&snapshot,"fixture.skill_approval",&json!({}));p.insert("agent_skill_visibility",artex_core::collection_prefix("agent_skill_visibility").unwrap(),json!({"agent_id":agent.header.id,"skill_name":"starter-skill","enabled":true})).unwrap();commit(&mut store,p.done(json!({})));store.checkpoint().unwrap();
    });
    let profile=provider::profile(&mut f,"Acceptance model","openai",&provider.url,&secret,false);f.setup("ui.activateLLMProfile",json!({"id":profile}));
    let r=rich.lock().unwrap().clone();let control=unrelated_task(&f,&r);let untouched=f.app.profile.join("shared-resources/skills/starter-skill/SKILL.md");let original_instructions=std::fs::read(&untouched).unwrap();let task=r.task.clone();send_task(&mut f,&task,"Create and update only the local approved Skill originals, then load the approved starter.");
    f.until(|app|app.pending.is_empty()&&provider.requests().len()==4&&app.handle.view().unwrap().generation.records("agent_runs").iter().filter(|r|r.data["task_id"]==task).all(|r|!matches!(r.data["state"].as_str(),Some("queued"|"running"|"waiting"))));
    f.assert_main_visible(&task,"SKILL_SEQUENCE_COMPLETE");
    let originals=f.app.profile.join("shared-resources/skills");let created=std::fs::read(originals.join("created-skill/SKILL.md")).unwrap();assert!(String::from_utf8_lossy(&created).contains("CREATED_SKILL_MARKER"));
    assert_eq!(std::fs::read(originals.join("starter-skill/scripts/note.txt")).unwrap(),b"SKILL_RESOURCE_MARKER");
    assert_unrelated_task(&f,&control);assert_eq!(std::fs::read(&untouched).unwrap(),original_instructions);
    let cases=[("create_skill","created-skill"),("update_skill_file","true"),("Skill","SKILL_INSTRUCTIONS_MARKER")];
    for (name,marker) in cases {show_tool_result(&mut f,&task,name,marker);}
    let invocations=f.app.handle.view().unwrap().generation.records("tool_invocations").into_iter().filter(|i|["skill-create","skill-update","skill-load"].contains(&i.data["tool_use_id"].as_str().unwrap_or(""))).cloned().collect::<Vec<_>>();assert_eq!(invocations.len(),3);
    for i in &invocations {assert_eq!(i.data["state"],"completed");}
    f.reopen_acceptance(true);assert_unrelated_task(&f,&control);assert_eq!(std::fs::read(&untouched).unwrap(),original_instructions);
    assert_eq!(std::fs::read(originals.join("created-skill/SKILL.md")).unwrap(),created);assert_eq!(std::fs::read(originals.join("starter-skill/scripts/note.txt")).unwrap(),b"SKILL_RESOURCE_MARKER");
    for invocation in &invocations {assert_eq!(f.app.handle.view().unwrap().generation.get("tool_invocations",&invocation.header.id).unwrap(),invocation);}
    for (name,marker) in cases {show_tool_result(&mut f,&task,name,marker);println!("TOOL_ACCEPT {}",json!({"tool":name,"real_original_resource_effects":true,"explicit_visibility":true,"display_and_readonly_reopen":true}));}
    let effects=probe.results.lock().unwrap();for (name,_) in cases {assert!(effects.iter().any(|e|e["request"]["input"]["name"]==name&&e.get("error").is_none()));}drop(effects);
    f.shutdown();assert_eq!(provider.finish().len(),4);
}

#[test]
fn acceptance_tools_retest_reads_bound_context_stages_then_seals_real_verdict_and_reopens() {
    use std::sync::atomic::{AtomicBool,Ordering};
    let rich=Arc::new(Mutex::new(Rich::default()));let seeded=rich.clone();
    let release=Arc::new(AtomicBool::new(false));let released=release.clone();
    let provider=LocalProvider::from_handler(move |index,request|match index {
        0=>tool_reply("retest-context","get_finding_retest_context",json!({})),
        1=>{let context=reply_text(request,"retest-context");assert!(context.contains("FINDING_MARKER")&&context.contains("TRAFFIC_BIND_MARKER"));tool_reply("retest-verdict","record_finding_retest_result",json!({"verdict":"fixed","summary":"RETEST_SAVED_MARKER","evidence":"RETEST_EVIDENCE_MARKER: targeted fixture verification recorded"}))}
        2=>{
            assert!(reply_text(request,"retest-verdict").contains("fixed"));let started=Instant::now();while !released.load(Ordering::Acquire){assert!(started.elapsed()<Duration::from_secs(30));std::thread::sleep(Duration::from_millis(10));}
            answer("RETEST_COMPLETED_VISIBLE")
        }
        _=>panic!("Unexpected retest request"),
    });
    let secret=artex_core::new_id("secret");
    let mut f=Fixture::configured_seeded(|profile|ServiceConfig{host_factory:Some(factory(profile,BTreeMap::from([(secret.clone(),"retest-fixture-key".into())]),Probe::default())),wake_ui:None},move |path|{
        *seeded.lock().unwrap()=seed_rich(path);seed_agent_patch(path,"retester",json!({"enabled":true}));
        let mut store=Store::open(path).unwrap();let snapshot=store.snapshot();let mut p=Planner::new(&snapshot,"fixture.retest_bindings",&json!({}));
        for key in ["get_finding_retest_context","record_finding_retest_result"] {let row=snapshot.records("tools").into_iter().find(|t|t.data["key"]==key).unwrap();let mut d=row.data.clone();d["agents"]=json!(["mainagent","auto","retester"]);p.put("tools",&row.header.id,d);}
        commit(&mut store,p.done(json!({})));store.checkpoint().unwrap();
    });
    let profile=provider::profile(&mut f,"Acceptance model","openai",&provider.url,&secret,false);f.setup("ui.activateLLMProfile",json!({"id":profile}));let r=rich.lock().unwrap().clone();let control=unrelated_task(&f,&r);
    let result=f.source_action(Section::Findings,Some(&r.finding),"ui.startFindingRetest",json!({"id":r.finding,"notes":"Use only the saved local fixture evidence"}));
    let cid=result["retest"]["conversation_id"].as_str().unwrap().to_owned();
    f.until(|app|app.handle.view().unwrap().generation.records("finding_retests").iter().any(|rt|rt.data["conversation_id"]==cid&&rt.data["summary"]=="RETEST_SAVED_MARKER"));
    let staged=f.app.handle.view().unwrap();let retest=staged.generation.records("finding_retests").into_iter().find(|rt|rt.data["conversation_id"]==cid).unwrap();assert_eq!(retest.data["status"],"running");assert_eq!(staged.generation.get("findings",&r.finding).unwrap().data["status"],"pending");
    let active=f.source_action(Section::Findings,Some(&r.finding),"ui.activeFindingRetests",json!({}));assert!(active.to_string().contains(&cid));assert!(active.to_string().contains("running"));f.source_result_visible("ui.activeFindingRetests",Some(&r.finding),&cid);
    let history=f.source_action(Section::Findings,Some(&r.finding),"ui.findingRetests",json!({"id":r.finding}));assert!(history.to_string().contains(&cid));f.source_result_visible("ui.findingRetests",Some(&r.finding),"RETEST_SAVED_MARKER");
    release.store(true,Ordering::Release);f.wait_conversation_terminal(&cid,"completed");f.assert_main_visible(&cid,"RETEST_COMPLETED_VISIBLE");
    let view=f.app.handle.view().unwrap();let rt=view.generation.records("finding_retests").into_iter().find(|rt|rt.data["conversation_id"]==cid).unwrap().clone();assert_eq!(rt.data["status"],"completed");assert_eq!(rt.data["verdict"],"fixed");assert_eq!(rt.data["evidence"],artex_core::body("RETEST_EVIDENCE_MARKER: targeted fixture verification recorded"));assert_eq!(view.generation.get("findings",&r.finding).unwrap().data["status"],"fixed");
    for (name,marker) in [("get_finding_retest_context","FINDING_MARKER"),("record_finding_retest_result","fixed")] {show_tool_result(&mut f,&cid,name,marker);}
    assert_unrelated_task(&f,&control);f.reopen_acceptance(true);assert_unrelated_task(&f,&control);assert_eq!(f.app.handle.view().unwrap().generation.get("finding_retests",&rt.header.id).unwrap(),&rt);assert_eq!(f.app.handle.view().unwrap().generation.get("findings",&r.finding).unwrap().data["status"],"fixed");
    let active=f.source_action(Section::Findings,Some(&r.finding),"ui.activeFindingRetests",json!({}));assert!(active.as_array().unwrap().is_empty(),"The completed retained retest must leave the active index");
    let history=f.source_action(Section::Findings,Some(&r.finding),"ui.findingRetests",json!({"id":r.finding}));assert!(history.to_string().contains("completed"));f.source_result_visible("ui.findingRetests",Some(&r.finding),"RETEST_SAVED_MARKER");
    for (name,marker) in [("get_finding_retest_context","FINDING_MARKER"),("record_finding_retest_result","fixed")] {show_tool_result(&mut f,&cid,name,marker);println!("TOOL_ACCEPT {}",json!({"tool":name,"actual_bound_retest":true,"staging_not_premature_fix":true,"terminal_seal":true,"display_and_readonly_reopen":true}));}
    f.shutdown();assert_eq!(provider.finish().len(),3);
    for method in ["ui.startFindingRetest","ui.findingRetests","ui.activeFindingRetests"] {println!("UI_ACCEPT {}",json!({"method":method,"actual_live_retest":true,"active_query_while_running":true,"saved_verdict_terminal_seal":true,"display_and_readonly_reopen":true}));}
}

#[test]
fn acceptance_tools_pause_own_task_cancels_live_run_without_an_extra_model_turn_and_reopens() {
    let rich=Arc::new(Mutex::new(Rich::default()));let seeded=rich.clone();let target=rich.clone();
    let provider=LocalProvider::from_handler(move |index,_|{assert_eq!(index,0,"Self-pause must not request another model turn");tool_reply("self-pause","pause_task",json!({"task_id":target.lock().unwrap().task}))});
    let secret=artex_core::new_id("secret");
    let mut f=Fixture::configured_seeded(|profile|ServiceConfig{host_factory:Some(factory(profile,BTreeMap::from([(secret.clone(),"pause-fixture-key".into())]),Probe::default())),wake_ui:None},move |path|*seeded.lock().unwrap()=seed_rich(path));
    let profile=provider::profile(&mut f,"Acceptance model","openai",&provider.url,&secret,false);f.setup("ui.activateLLMProfile",json!({"id":profile}));let r=rich.lock().unwrap().clone();let control=unrelated_task(&f,&r);let task=r.task.clone();
    send_task(&mut f,&task,"Pause this task from its own active model run.");
    f.until(|app|app.pending.is_empty()&&app.handle.view().unwrap().generation.records("tool_invocations").iter().any(|i|i.data["tool_use_id"]=="self-pause"&&i.data["state"]=="completed")&&app.handle.view().unwrap().generation.records("agent_runs").iter().filter(|r|r.data["task_id"]==task).all(|r|!matches!(r.data["state"].as_str(),Some("queued"|"running"|"waiting"))));
    assert_eq!(f.app.handle.view().unwrap().generation.get("tasks",&task).unwrap().data["paused"],true);show_tool_result(&mut f,&task,"pause_task","task paused:");
    let invocation=f.app.handle.view().unwrap().generation.records("tool_invocations").into_iter().find(|i|i.data["tool_use_id"]=="self-pause").unwrap().clone();
    assert_unrelated_task(&f,&control);f.reopen_acceptance(true);assert_unrelated_task(&f,&control);assert_eq!(f.app.handle.view().unwrap().generation.get("tasks",&task).unwrap().data["status"],"paused");assert_eq!(f.app.handle.view().unwrap().generation.get("tool_invocations",&invocation.header.id).unwrap(),&invocation);show_tool_result(&mut f,&task,"pause_task","task paused:");
    f.shutdown();assert_eq!(provider.finish().len(),1);println!("TOOL_ACCEPT {}",json!({"tool":"pause_task","actual_own_run_cancellation":true,"no_model_replay":true,"display_and_readonly_reopen":true}));
}

/// Resolve synthetic fixture credentials at the same trusted boundary as the
/// native host, while retaining the real HTTP execution and response parser.
struct SearchGateway {
    native: artex_native_core::gateway::NativeGateway,
    secrets: BTreeMap<String,String>,
}
impl artex_runtime::HostGateway for SearchGateway {
    fn request(&self,workspace:&str,context:&Context,op:&str,mut input:Value,timeout:Duration)->artex_core::Result<Value> {
        if op.starts_with("secret.") {
            return if op=="secret.resolve" {input["secret_ref"].as_str().and_then(|id|self.secrets.get(id)).map(|value|json!({"value":value})).ok_or_else(||artex_core::Error::new("PERMISSION_DENIED","Unknown fixture credential"))}
            else {Err(artex_core::Error::new("PERMISSION_DENIED","Fixture secret writes are forbidden"))};
        }
        if op=="tool.execute" && input["name"]=="web_search" {
            let id=input["input"]["network_config"]["api_key"]["secret_ref"].as_str().unwrap();
            input["input"]["network_config"]["api_key"]=json!(self.secrets.get(id).expect("Only admitted synthetic search key"));
        }
        self.native.request(workspace,context,op,input,timeout)
    }
    fn retire(&self,t:Duration)->artex_core::Result<()> {self.native.retire(t)}
    fn retirement_report(&self,w:&str,c:&[Context])->artex_core::Result<Option<Value>> {self.native.retirement_report(w,c)}
}

#[test]
fn acceptance_tools_web_search_actual_authenticated_loopback_sse_and_reopen() {
    use std::{io::{Read,Write},net::TcpListener};
    let listener=TcpListener::bind("127.0.0.1:0").unwrap();listener.set_nonblocking(true).unwrap();
    let endpoint=format!("http://{}",listener.local_addr().unwrap());
    let received=Arc::new(Mutex::new(None));let captured=received.clone();
    let search=std::thread::spawn(move || {
        let start=Instant::now();let mut socket=loop {match listener.accept(){Ok((socket,_))=>break socket,Err(e) if e.kind()==std::io::ErrorKind::WouldBlock=>{assert!(start.elapsed()<Duration::from_secs(60));std::thread::sleep(Duration::from_millis(5));},Err(e)=>panic!("{e}")}};
        // accept() is polled, but the HTTP reader must wait for reqwest's
        // header/body writes. Reset the accepted stream's nonblocking mode;
        // setting a read timeout alone does not reset it on Windows.
        socket.set_nonblocking(false).unwrap();
        socket.set_read_timeout(Some(Duration::from_secs(10))).unwrap();
        let mut bytes=Vec::new();let mut b=[0];while !bytes.ends_with(b"\r\n\r\n"){socket.read_exact(&mut b).unwrap();bytes.push(b[0]);}
        let header=String::from_utf8(bytes).unwrap();let length=header.lines().find_map(|line|line.split_once(':').filter(|(key,_)|key.eq_ignore_ascii_case("content-length")).map(|(_,value)|value.trim().parse::<usize>().unwrap())).unwrap();
        let mut body=vec![0;length];socket.read_exact(&mut body).unwrap();
        assert!(header.starts_with("POST /v1/messages "));assert!(header.to_lowercase().contains("x-api-key: search-fixture-credential"));
        let body:Value=serde_json::from_slice(&body).unwrap();assert_eq!(body["tools"][0]["type"],"web_search_20250305");assert!(body["messages"][0]["content"].as_str().unwrap().contains("LOCAL_SEARCH_QUERY"));
        *captured.lock().unwrap()=Some(body);
        let data=format!("data: {}\n\n",json!({"type":"content_block_start","content_block":{"type":"web_search_tool_result","content":[{"url":"https://fixture.invalid/result","title":"SEARCH_ACTUAL_MARKER"}]}}));
        write!(socket,"HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",data.len(),data).unwrap();
    });
    let model=LocalProvider::from_handler(|index,request|match index {0=>{assert!(request["tools"].as_array().unwrap().iter().any(|t|t["function"]["name"]=="web_search"));tool_reply("actual-search","web_search",json!({"query":"LOCAL_SEARCH_QUERY","limit":2}))},1=>{let text=reply_text(request,"actual-search");assert!(text.contains("SEARCH_ACTUAL_MARKER")&&text.contains("https://fixture.invalid/result"),"Actual web_search result returned to the model: {text}");answer("SEARCH_COMPLETE_MARKER")},_=>panic!("Extra model request")});
    let main_secret=artex_core::new_id("secret");let search_secret=artex_core::new_id("secret");
    let rich=Arc::new(Mutex::new(Rich::default()));let seeded=rich.clone();
    let mut f=Fixture::configured_seeded(|profile|{
        let profile=profile.to_owned();let vault=Arc::new(artex_host::SecretVault::default());let secrets=BTreeMap::from([(main_secret.clone(),"main-fixture-credential".into()),(search_secret.clone(),"search-fixture-credential".into())]);
        ServiceConfig{host_factory:Some(Arc::new(move |workspace|Ok(Arc::new(SearchGateway{native:artex_native_core::gateway::NativeGateway::open_with_profile(workspace,vault.clone(),&profile)?,secrets:secrets.clone()})))),wake_ui:None}
    },move |path|*seeded.lock().unwrap()=seed_rich(path));
    let main=provider::profile(&mut f,"Main local model","openai",&model.url,&main_secret,false);
    let searching=provider::profile(&mut f,"Search local model","anthropic",&endpoint,&search_secret,false);
    f.setup("ui.activateLLMProfile",json!({"id":searching}));f.setup("ui.saveAgentConfig",json!({"key":"mainagent","patch":{"llm_profile_id":main}}));
    f.setup("ui.setSettings",json!({"patch":{"web_search_enabled":true,"web_search_backend":"deepseek"}}));
    let task=rich.lock().unwrap().task.clone();send_task(&mut f,&task,"Search only the configured isolated loopback fixture.");
    f.until(|app|app.handle.view().unwrap().generation.records("tool_invocations").iter().any(|i|i.data["tool_use_id"]=="actual-search"&&i.data["state"]=="completed")&&app.handle.view().unwrap().generation.records("agent_runs").iter().filter(|r|r.data["task_id"]==task).all(|r|!matches!(r.data["state"].as_str(),Some("queued"|"running"|"waiting"))));
    f.assert_main_visible(&task,"SEARCH_COMPLETE_MARKER");show_tool_result(&mut f,&task,"web_search","SEARCH_ACTUAL_MARKER");
    let invocation=f.app.handle.view().unwrap().generation.records("tool_invocations").into_iter().find(|i|i.data["tool_use_id"]=="actual-search").unwrap().clone();
    assert!(!serde_json::to_string(&invocation).unwrap().contains("search-fixture-credential"));
    f.reopen_acceptance(true);assert_eq!(f.app.handle.view().unwrap().generation.get("tool_invocations",&invocation.header.id).unwrap(),&invocation);show_tool_result(&mut f,&task,"web_search","SEARCH_ACTUAL_MARKER");
    f.shutdown();search.join().unwrap();assert!(received.lock().unwrap().is_some());assert_eq!(model.finish().len(),2);
    println!("TOOL_ACCEPT {}",json!({"tool":"web_search","real_authenticated_loopback_request":true,"real_sse_parser":true,"os_vault_excluded":true,"display_and_readonly_reopen":true}));
}

#[test]
fn acceptance_tools_steer_live_worker_consumes_instruction_then_kill_cleans_run_and_reopens() {
    use std::sync::atomic::{AtomicBool,Ordering};
    let release=Arc::new(AtomicBool::new(false));let released=release.clone();
    let consumed=Arc::new(AtomicBool::new(false));let delivered=consumed.clone();
    let worker=LocalProvider::from_handler(move |index,request|match index {
        0=>{let start=Instant::now();while !released.load(Ordering::Acquire){assert!(start.elapsed()<Duration::from_secs(30));std::thread::sleep(Duration::from_millis(5));}tool_reply("steering-read","Read",json!({"file_path":"steer.txt"}))}
        1=>{assert!(reply_text(request,"steering-read").contains("STEER_FILE_MARKER"));assert!(request["messages"].to_string().contains("STEER_ACTIVE_MARKER"));assert!(request["messages"].to_string().contains("UI_STEER_ACTIVE_MARKER"));delivered.store(true,Ordering::Release);Step::Hold}
        _=>panic!("Unexpected extra worker turn"),
    });
    let rich=Arc::new(Mutex::new(Rich::default()));let seeded=rich.clone();let target=rich.clone();
    let main=LocalProvider::from_handler(move |index,request|match index {
        0=>tool_reply("steer-live","steer_work",json!({"intent_id":target.lock().unwrap().live,"message":"STEER_ACTIVE_MARKER"})),
        1=>{assert!(reply_text(request,"steer-live").contains("Injected corrective instructions"));release.store(true,Ordering::Release);let start=Instant::now();while !consumed.load(Ordering::Acquire){assert!(start.elapsed()<Duration::from_secs(30));std::thread::sleep(Duration::from_millis(5));}tool_reply("kill-live","kill_work",json!({"intent_id":target.lock().unwrap().live}))}
        2=>{assert!(reply_text(request,"kill-live").contains("termination signal"));answer("STEER_AND_KILL_COMPLETE")}
        _=>panic!("Unexpected main turn"),
    });
    let secret=artex_core::new_id("secret");let probe=Probe::default();
    let mut f=Fixture::configured_seeded(|profile|ServiceConfig{host_factory:Some(factory(profile,BTreeMap::from([(secret.clone(),"steer-fixture-key".into())]),probe.clone())),wake_ui:None},move |path|{
        let r=seed_rich(path);seed_agent_patch(path,"worker",json!({"enabled":true}));
        let mut store=Store::open(path).unwrap();let snapshot=store.snapshot();let mut p=Planner::new(&snapshot,"fixture.blocked_intent",&json!({}));let mut data=snapshot.get("exploration_nodes",&r.live).unwrap().data.clone();data["state"]=json!("blocked");p.put("exploration_nodes",&r.live,data);commit(&mut store,p.done(json!({})));store.checkpoint().unwrap();*seeded.lock().unwrap()=r;
    });
    let main_id=provider::profile(&mut f,"Steer main model","openai",&main.url,&secret,false);let worker_id=provider::profile(&mut f,"Steer worker model","openai",&worker.url,&secret,false);
    f.setup("ui.activateLLMProfile",json!({"id":main_id}));f.setup("ui.saveAgentConfig",json!({"key":"mainagent","patch":{"llm_profile_id":main_id}}));f.setup("ui.saveAgentConfig",json!({"key":"worker","patch":{"llm_profile_id":worker_id}}));
    let r=rich.lock().unwrap().clone();let control=unrelated_task(&f,&r);let directory=f.path.join("work/tasks").join(&r.task);std::fs::create_dir_all(&directory).unwrap();std::fs::write(directory.join("steer.txt"),"STEER_FILE_MARKER").unwrap();
    f.source_action(Section::Tasks,Some(&r.task),"ui.rerunIntent",json!({"taskId":r.task,"intentId":r.live}));
    f.until(|_|worker.requests().len()==1);
    f.source_action(Section::Tasks,Some(&r.task),"ui.sendWorkerMessage",json!({"taskId":r.task,"intentId":r.live,"message":"UI_STEER_ACTIVE_MARKER","requestId":artex_core::new_id("command")}));
    send_task(&mut f,&r.task,"Steer the active local worker, verify delivery, then stop it.");
    f.until(|app|app.handle.view().unwrap().generation.records("tool_invocations").iter().any(|i|i.data["tool_use_id"]=="kill-live"&&i.data["state"]=="completed")&&app.handle.view().unwrap().generation.records("agent_runs").iter().filter(|run|run.data["task_id"]==r.task).all(|run|!matches!(run.data["state"].as_str(),Some("queued"|"running"|"waiting"))));
    let view=f.app.handle.view().unwrap();let steering=view.generation.records("steering_messages").into_iter().find(|row|row.data["text"]=="STEER_ACTIVE_MARKER").unwrap().clone();assert_eq!(steering.data["state"],"consumed");
    let ui_steering=view.generation.records("steering_messages").into_iter().find(|row|row.data["text"]=="UI_STEER_ACTIVE_MARKER").unwrap().clone();assert_eq!(ui_steering.data["state"],"consumed");assert_eq!(ui_steering.data["target_run_id"],steering.data["target_run_id"]);
    let worker_run=view.generation.get("agent_runs",steering.data["target_run_id"].as_str().unwrap()).unwrap();assert_eq!(worker_run.data["intent_id"],r.live);assert_eq!(worker_run.data["state"],"cancelled");assert_eq!(view.generation.get("exploration_nodes",&r.live).unwrap().data["state"],"stopped");
    let invocations=view.generation.records("tool_invocations").into_iter().filter(|row|["steer-live","kill-live"].contains(&row.data["tool_use_id"].as_str().unwrap_or(""))).cloned().collect::<Vec<_>>();assert_eq!(invocations.len(),2);
    f.assert_main_visible(&r.task,"STEER_AND_KILL_COMPLETE");for (name,text) in [("steer_work","Injected corrective instructions"),("kill_work","termination signal")] {show_tool_result(&mut f,&r.task,name,text);}
    f.assert_main_visible(&r.task,"UI_STEER_ACTIVE_MARKER");
    assert_unrelated_task(&f,&control);f.reopen_acceptance(true);assert_unrelated_task(&f,&control);assert_eq!(f.app.handle.view().unwrap().generation.get("steering_messages",&steering.header.id).unwrap(),&steering);assert_eq!(f.app.handle.view().unwrap().generation.get("steering_messages",&ui_steering.header.id).unwrap(),&ui_steering);for row in &invocations {assert_eq!(f.app.handle.view().unwrap().generation.get("tool_invocations",&row.header.id).unwrap(),row);}
    f.assert_main_visible(&r.task,"UI_STEER_ACTIVE_MARKER");
    for (name,text) in [("steer_work","Injected corrective instructions"),("kill_work","termination signal")] {show_tool_result(&mut f,&r.task,name,text);println!("TOOL_ACCEPT {}",json!({"tool":name,"real_live_worker":true,"actual_next_model_turn_received_steering":true,"worker_cancelled":true,"display_and_readonly_reopen":true}));}
    f.shutdown();assert_eq!(main.finish().len(),3);assert_eq!(worker.finish().len(),2);
    for method in ["ui.rerunIntent","ui.sendWorkerMessage"] {println!("UI_ACCEPT {}",json!({"method":method,"real_worker_launch":true,"actual_next_model_turn_received_steering":true,"consumed_target_bound_message":true,"source_parity":method!="ui.sendWorkerMessage","native_live_extension":method=="ui.sendWorkerMessage","display_and_readonly_reopen":true}));}
}

fn select_pending_approval(f:&mut Fixture,id:&str) {
    f.app.drawer.open=false;select(f,Section::Approvals,None);
    let out=f.frame(vec![]);let label=out.shapes.iter().find_map(|s|match &s.shape {
        egui::epaint::Shape::Text(t) if t.galley.text().contains("Read · 승인 대기")&&s.clip_rect.contains_rect(t.visual_bounding_rect())=>Some(t.galley.text().to_owned()),_=>None,
    }).expect("Actual pending approval row must be visible");
    f.click_visible(&label);assert_eq!(f.app.selected.get(&Section::Approvals).and_then(|id|id.as_deref()),Some(id));
}

#[test]
fn acceptance_tools_approval_pending_allow_deny_real_host_pair_history_and_reopen() {
    let provider=LocalProvider::from_handler(|index,request|match index {
        0=>tool_reply("approval-allowed-read","Read",json!({"file_path":"approved.txt"})),
        1=>{assert!(reply_text(request,"approval-allowed-read").contains("ACTUAL_APPROVED_CONTENT"));tool_reply("approval-denied-read","Read",json!({"file_path":"denied.txt"}))}
        2=>{let result=reply_text(request,"approval-denied-read");assert!(!result.contains("DENIED_FILE_CONTENT"));assert!(result.to_lowercase().contains("denied"));answer("APPROVAL_FINISHED_MARKER")}
        3=>tool_reply("approval-other-pending-read","Read",json!({"file_path":"OTHER_TASK_APPROVAL_MARKER.txt"})),
        _=>panic!("Approval must not replay a decided invocation"),
    });
    let rich=Arc::new(Mutex::new(Rich::default()));let seeded=rich.clone();let secret=artex_core::new_id("secret");let probe=Probe::default();
    let mut f=Fixture::configured_seeded(|profile|ServiceConfig{host_factory:Some(factory(profile,BTreeMap::from([(secret.clone(),"approval-fixture-key".into())]),probe.clone())),wake_ui:None},move |path|*seeded.lock().unwrap()=seed_rich(path));
    let profile=provider::profile(&mut f,"Approval model","openai",&provider.url,&secret,false);f.setup("ui.activateLLMProfile",json!({"id":profile}));
    f.setup("ui.setSettings",json!({"patch":{"approval_bypass":false}}));f.setup("ui.interceptSetToolConfig",json!({"enabledTools":["Read"]}));
    f.setup("ui.createInterceptRule",json!({"rule":{"name":"Ask for fixture Read","enabled":true,"priority":100,"match_target":"tool_name","match_type":"string","pattern":"Read","action":"ask","message":"APPROVAL_POLICY_MARKER","timeout_enabled":false,"timeout_seconds":0,"timeout_action":"deny"}}));
    let r=rich.lock().unwrap().clone();let files=f.path.join("work/tasks").join(&r.task);std::fs::create_dir_all(&files).unwrap();std::fs::write(files.join("approved.txt"),"ACTUAL_APPROVED_CONTENT").unwrap();std::fs::write(files.join("denied.txt"),"DENIED_FILE_CONTENT").unwrap();
    send_task(&mut f,&r.task,"Read only the first fixture after approval, then request a second read for an explicit denial.");
    let mut approvals=Vec::new();
    for (call,decision,button) in [("approval-allowed-read","allowed","허용"),("approval-denied-read","denied","거부")] {
        f.until(|app|app.handle.view().unwrap().generation.records("intercept_pending").iter().any(|a|a.data["audit"]["tool_use_id"]==call&&a.data["status"]=="pending"));
        let approval=f.app.handle.view().unwrap().generation.records("intercept_pending").into_iter().find(|a|a.data["audit"]["tool_use_id"]==call).unwrap().header.id.clone();
        assert_eq!(probe.attempts.lock().unwrap().len(),usize::from(decision=="denied"),"No native read before the corresponding decision");
        select_pending_approval(&mut f,&approval);
        let one=f.source_action(Section::Approvals,Some(&approval),"ui.interceptGetOne",json!({"id":approval}));assert_eq!(one["status"],"pending");f.source_result_visible("ui.interceptGetOne",Some(&approval),"APPROVAL_POLICY_MARKER");
        // Return to the actual selected native approval pane for the user
        // decision; the Source query is not an alternate decision dispatcher.
        f.app.drawer.open=false;select(&mut f,Section::Approvals,Some(&approval));f.click_visible(button);f.prepare("ui.interceptDecide");
        f.until(|app|app.pending.is_empty()&&app.handle.view().unwrap().generation.get("intercept_pending",&approval).unwrap().data["status"]==decision);
        approvals.push(approval);
    }
    f.until(|app|app.handle.view().unwrap().generation.records("agent_runs").iter().filter(|run|run.data["task_id"]==r.task).all(|run|!matches!(run.data["state"].as_str(),Some("queued"|"running"|"waiting"))));f.assert_main_visible(&r.task,"APPROVAL_FINISHED_MARKER");
    assert_eq!(probe.attempts.lock().unwrap().len(),1);assert_eq!(probe.results.lock().unwrap()[0]["request"]["input"]["input"]["file_path"],"approved.txt");
    let rows=approvals.iter().map(|id|f.app.handle.view().unwrap().generation.get("intercept_pending",id).unwrap().clone()).collect::<Vec<_>>();
    // Produce the unrelated pending control through a second actual task chat,
    // rather than inserting a detached approval row that could never execute.
    send_task(&mut f,&r.other,"Request the unrelated fixture read, but leave its approval pending.");
    f.until(|app|app.handle.view().unwrap().generation.records("intercept_pending").iter().any(|a|a.data["audit"]["tool_use_id"]=="approval-other-pending-read"&&a.data["status"]=="pending"));
    let other_approval=f.app.handle.view().unwrap().generation.records("intercept_pending").into_iter().find(|a|a.data["audit"]["tool_use_id"]=="approval-other-pending-read").unwrap().header.id.clone();
    assert_eq!(f.app.handle.view().unwrap().generation.get("intercept_pending",&other_approval).unwrap().data["task_id"],r.other);assert_eq!(probe.attempts.lock().unwrap().len(),1);
    for reopened in [false,true] {
        if reopened {f.reopen_acceptance(true);}
        for row in &rows {assert_eq!(f.app.handle.view().unwrap().generation.get("intercept_pending",&row.header.id).unwrap(),row);}
        let a=&approvals[0];
        for (method,input,marker) in [
            ("ui.interceptDetail",json!({"id":a}),"APPROVAL_POLICY_MARKER"),
            ("ui.interceptExecution",json!({"id":a}),"approval-allowed-read"),
            ("ui.interceptHistoryPage",json!({"page":1,"size":10}),"APPROVAL_POLICY_MARKER"),
            ("ui.interceptTaskPage",json!({"taskId":r.task,"page":1,"size":10}),"APPROVAL_POLICY_MARKER"),
        ] {let result=f.source_action(Section::Approvals,Some(a),method,input);assert!(result.to_string().contains(marker),"{method}: {result}");
            // Source GetInterceptExecution is a metadata navigation pair, not
            // a body endpoint. Approved file bytes were checked in the real
            // model/host flow; qualify this query by exact original identities.
            if method=="ui.interceptExecution" {let items=result["items"].as_array().unwrap();assert_eq!(items.len(),2);assert_eq!(items[0]["kind"],"tool_use");assert_eq!(items[1]["kind"],"tool_result");for row in items {assert_eq!(row["tool"],"Read");assert_eq!(row["tool_use_id"],"approval-allowed-read");}assert_eq!(result["task_id"],r.task);assert_eq!(result["seq"],items[0]["seq"]);assert_ne!(items[0]["seq"],items[1]["seq"]);let view=f.app.handle.view().unwrap();for item in items {let actual=view.generation.get("activity",item["seq"].as_str().unwrap()).unwrap();assert_eq!(actual.data["kind"],item["kind"]);assert_eq!(actual.data["tool_use_id"],"approval-allowed-read");}}
            if method=="ui.interceptHistoryPage" {assert!(result.to_string().contains(&other_approval),"The unrelated actual pending control must exist in global history");}
            if method=="ui.interceptTaskPage" {assert!(!result.to_string().contains(&other_approval));assert!(!result.to_string().contains("OTHER_TASK_APPROVAL_MARKER"));assert_eq!(result["total"],2);for id in &approvals {assert!(result.to_string().contains(id));}}
            f.source_result_visible(method,Some(a),marker);}
        f.app.drawer.open=false;select(&mut f,Section::Approvals,Some(a));let out=f.frame(vec![]);let rendered=visible_texts(&out,egui::Rect::EVERYTHING).join("\n");assert!(rendered.contains("Read · 허용됨")&&rendered.contains("Read · 거부됨"),"Native history/task projection must render both decisions: {rendered}");
    }
    assert_eq!(probe.attempts.lock().unwrap().len(),1,"Reopen must not replay the denied or unrelated pending read");f.shutdown();assert_eq!(provider.finish().len(),4);
    for method in ["ui.interceptPending","ui.interceptGetOne","ui.interceptDecide","ui.interceptExecution","ui.interceptDetail","ui.interceptHistory","ui.interceptHistoryPage","ui.interceptTask","ui.interceptTaskPage"] {println!("UI_ACCEPT {}",json!({"method":method,"native_pending_row_selected":true,"allowed_real_read":true,"denied_host_effect_absent":true,"exact_tool_pair":true,"native_snapshot_replaces_pending_history_list_rpc":true,"interceptTask_replaced_by_actual_scoped_TaskPage":true,"unrelated_task_control_excluded":true,"old_list_wire_not_invoked":true,"display_and_readonly_reopen":true}));}
}
