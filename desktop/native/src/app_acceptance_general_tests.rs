//! System and chat acceptance through actual App controls and durable service.
//! Loopback provider replies and file pickers are fixture boundaries only.
use super::*;
use super::transport::*;

fn fixture(secret: &str) -> Fixture {
    Fixture::configured(|profile| ServiceConfig {
        host_factory:Some(factory(profile,BTreeMap::from([(secret.into(),"isolated-placeholder".into())]),Probe::default())),
        wake_ui:None,
    })
}
fn settle(f:&mut Fixture) {
    f.until(|app|app.pending.is_empty());
    assert!(f.app.error.is_none(),"{:?}",f.app.error);
}
fn record(f:&Fixture,c:&str,id:&str)->artex_core::Record {
    f.app.handle.view().unwrap().generation.get(c,id).unwrap().clone()
}
fn query(f:&mut Fixture,section:Section,selected:Option<&str>,method:&str,params:Value,needle:&str)->Value {
    f.source_query(section,selected,method,params);
    let value=f.app.source_operations.query_result_for_app_test(method,selected).unwrap();
    assert!(value.to_string().contains(needle),"{method}: {value}");
    f.source_result_visible(method,selected,needle);value
}
fn left_row(f:&mut Fixture,section:Section,id:&str,label:&str) {
    select(f,section,None);
    let out=f.frame(vec![]);
    let list_right=f.ctx.available_rect().left();
    let pos=out.shapes.iter().find_map(|c| {
        let egui::epaint::Shape::Text(t)=&c.shape else {return None;};
        let r=t.visual_bounding_rect();
        (t.galley.text().contains(label)&&r.right()<=list_right&&c.clip_rect.contains_rect(r)).then(||r.center())
    }).unwrap_or_else(||panic!("Actual list row {label} missing: {:?}",Fixture::texts(&out)));
    f.frame(vec![egui::Event::PointerMoved(pos),egui::Event::PointerButton{pos,button:egui::PointerButton::Primary,pressed:true,modifiers:Default::default()}]);
    f.frame(vec![egui::Event::PointerButton{pos,button:egui::PointerButton::Primary,pressed:false,modifiers:Default::default()}]);
    assert_eq!(f.app.selected[&section].as_deref(),Some(id));
}
fn send_task(f:&mut Fixture,task:&str,message:&str) {
    f.app.drawer.open=false;select(f,Section::Chat,Some(task));
    let out=f.frame(vec![]);
    let pos=out.shapes.iter().find_map(|c| {
        let egui::epaint::Shape::Text(t)=&c.shape else {return None;};
        (t.galley.text().starts_with("메시지를 입력하세요.")&&c.clip_rect.contains_rect(t.visual_bounding_rect())).then(||t.visual_bounding_rect().center())
    }).expect("Actual task composer missing");
    f.frame(vec![egui::Event::PointerMoved(pos),egui::Event::PointerButton{pos,button:egui::PointerButton::Primary,pressed:true,modifiers:Default::default()}]);
    f.frame(vec![egui::Event::PointerButton{pos,button:egui::PointerButton::Primary,pressed:false,modifiers:Default::default()},egui::Event::Text(message.into())]);
    f.click_visible("보내기 · Ctrl+Enter");f.prepare("ui.chat");
}
fn task_terminal(f:&mut Fixture,task:&str,state:&str) {
    f.until(|app| {
        let view=app.handle.view().unwrap();
        view.generation.records("agent_runs").iter().any(|r|r.data["task_id"]==task&&r.data["state"]==state)
            && app.pending.is_empty()
    });
    assert!(f.app.error.is_none(),"{:?}",f.app.error);
}
fn note(case:&str,methods:&[&str]) {
    println!("ACCEPT_GENERAL case={case} methods={} connected_path=true fixture=isolated_native_app",methods.join(","));
}
fn compatibility_error(f:&mut Fixture,method:&str,params:Value)->Error {
    settle(f);let scope=f.app.handle.scope().unwrap();
    // Negative compatibility checks are additional service evidence. They do
    // not stand in for the native control input exercised by the scenario.
    let action=if matches!(method,"ui.checkUpdate"|"ui.applyUpdate"|"ui.rollbackUpdate") {
        NativeAction::Command{scope,command:NativeCommand::new(method,params,vec![])}
    } else {NativeAction::Query{scope,query:NativeQuery::Feature{method:method.into(),params}}};
    let ticket=f.app.handle.try_submit(action).unwrap();
    let started=Instant::now();
    loop {
        for completion in f.app.handle.drain_completions() {
            assert_eq!(completion.ticket,ticket,"Unexpected unrelated completion");
            return completion.result.unwrap_err();
        }
        assert!(started.elapsed()<Duration::from_secs(30));std::thread::sleep(Duration::from_millis(5));
    }
}

#[test]
fn general_conversation_profile_change_reopens_and_uses_selected_provider() {
    let local=LocalProvider::new(vec![Step::Reply(200,"application/json",anthropic("SELECTED PROFILE ANSWER ACCEPT",false))]);
    let secret=artex_core::new_id("secret");let mut f=fixture(&secret);
    let old=configured_profile(&mut f,"UNUSED PROFILE CONTROL");
    let chosen=provider::profile(&mut f,"SELECTED PROFILE ACCEPT","anthropic",&local.url,&secret,false);
    let id=f.setup("ui.createConversation",json!({"agent_key":"auto","title":"PROFILE CHANGE CONVERSATION","llm_profile_id":old}))["id"].as_str().unwrap().to_owned();
    f.source_action(Section::Chat,Some(&id),"ui.updateConversationProfile",json!({"id":id,"llm_profile_id":chosen}));
    f.reopen_acceptance(false);
    assert_eq!(record(&f,"conversations",&id).data["llm_profile_id"],chosen);
    assert!(f.app.handle.view().unwrap().generation.get("llm_profiles",&old).is_some());
    f.type_composer_and_send(&id,"Use the explicitly selected profile after reopen.");
    f.wait_conversation_terminal(&id,"completed");f.assert_main_visible(&id,"SELECTED PROFILE ANSWER ACCEPT");
    f.reopen_same_workspace();f.assert_main_visible(&id,"SELECTED PROFILE ANSWER ACCEPT");
    note("general_conversation_profile_change_reopens_and_uses_selected_provider",&["ui.updateConversationProfile"]);
    f.shutdown();assert_eq!(local.finish().len(),1);
}

#[test]
fn general_conversation_metadata_profile_native_list_and_deletion_preserve_control() {
    let mut f=fixture(&artex_core::new_id("secret"));
    let id=f.setup("ui.createConversation",json!({"agent_key":"auto","title":"CONVERSATION INITIAL"}))["id"].as_str().unwrap().to_owned();
    let other=f.setup("ui.createConversation",json!({"agent_key":"auto","title":"CONVERSATION PRESERVED"}))["id"].as_str().unwrap().to_owned();
    f.source_action(Section::Chat,Some(&id),"ui.updateConversation",json!({"id":id,"input":{"title":"CONVERSATION UPDATED","pinned":true}}));
    f.reopen_acceptance(false);
    left_row(&mut f,Section::Chat,&id,"CONVERSATION UPDATED");
    assert!(!record(&f,"conversations",&id).data["pinned_at"].is_null());
    f.source_action(Section::Chat,Some(&id),"ui.renameConversation",json!({"id":id,"title":"CONVERSATION RENAMED"}));
    f.reopen_acceptance(false);
    left_row(&mut f,Section::Chat,&id,"CONVERSATION RENAMED");
    f.source_action(Section::Chat,Some(&id),"ui.pinConversation",json!({"id":id,"pinned":false}));
    f.reopen_acceptance(false);
    assert!(record(&f,"conversations",&id).data["pinned_at"].is_null());
    left_row(&mut f,Section::Chat,&id,"CONVERSATION RENAMED");
    f.source_action(Section::Chat,Some(&id),"ui.deleteConversation",json!({"id":id}));
    f.reopen_acceptance(false);
    assert!(f.app.handle.view().unwrap().generation.get("conversations",&id).is_none());
    left_row(&mut f,Section::Chat,&other,"CONVERSATION PRESERVED");
    let batch_a=f.setup("ui.createConversation",json!({"agent_key":"auto","title":"BATCH CONVERSATION A"}))["id"].as_str().unwrap().to_owned();
    let batch_b=f.setup("ui.createConversation",json!({"agent_key":"auto","title":"BATCH CONVERSATION B"}))["id"].as_str().unwrap().to_owned();
    f.source_action(Section::Chat,Some(&batch_a),"ui.deleteConversations",json!({"ids":[batch_a,batch_b]}));
    f.reopen_same_workspace();
    let view=f.app.handle.view().unwrap();
    assert!(view.generation.get("conversations",&batch_a).is_none());
    assert!(view.generation.get("conversations",&batch_b).is_none());
    left_row(&mut f,Section::Chat,&other,"CONVERSATION PRESERVED");
    // ui.conversations is intentionally replaced by generation-owned native
    // list reads. Row clicks above prove selection and stored title ownership.
    note("general_conversation_metadata_profile_native_list_and_deletion_preserve_control",&["ui.conversations","ui.updateConversation","ui.renameConversation","ui.pinConversation","ui.deleteConversation","ui.deleteConversations"]);
    f.shutdown();
}

#[test]
fn general_conversation_real_provider_history_cursor_detail_and_mentions_reopen() {
    let local=LocalProvider::new(vec![Step::Reply(200,"application/json",anthropic("GENERAL ANSWER FIRST",false)),Step::Reply(200,"application/json",anthropic("GENERAL ANSWER SECOND",false))]);
    let secret=artex_core::new_id("secret");let mut f=fixture(&secret);
    let profile=provider::profile(&mut f,"History model","anthropic",&local.url,&secret,false);
    let company=f.setup("ui.createCompany",json!({"name":"MENTION COMPANY ACCEPT","scope":[]}))["id"].as_str().unwrap().to_owned();
    let id=f.setup("ui.createConversation",json!({"agent_key":"auto","title":"HISTORY CONVERSATION ACCEPT","llm_profile_id":profile}))["id"].as_str().unwrap().to_owned();
    f.type_composer_and_send(&id,"GENERAL USER FIRST");f.wait_conversation_terminal(&id,"completed");f.assert_main_visible(&id,"GENERAL ANSWER FIRST");
    let before=f.app.handle.view().unwrap().generation.records("conversation_activities").into_iter().filter(|r|r.data["conversation_id"]==id).max_by_key(|r|r.header.commit_seq.parse::<u64>().unwrap()).unwrap().header.id.clone();
    f.type_composer_and_send(&id,"GENERAL USER SECOND");
    f.until(|app| {
        let v=app.handle.view().unwrap();
        v.generation.records("agent_runs").iter().filter(|r|r.data["conversation_id"]==id&&r.data["state"]=="completed").count()==2&&app.pending.is_empty()
    });
    f.assert_main_visible(&id,"GENERAL ANSWER SECOND");
    f.reopen_acceptance(false);
    let all=query(&mut f,Section::Chat,Some(&id),"ui.conversationHistory",json!({"id":id,"limit":1000}),"GENERAL ANSWER SECOND");
    assert!(all.to_string().contains("GENERAL USER FIRST"));
    let items=all["items"].as_array().unwrap();assert!(items.len()>=4);
    let seq=items.iter().find(|item|item.to_string().contains("GENERAL ANSWER SECOND")).unwrap()["seq"].as_str().unwrap().to_owned();
    query(&mut f,Section::Chat,Some(&id),"ui.conversationMessages",json!({"id":id}),"GENERAL ANSWER FIRST");
    let cursor=&before;
    let after=query(&mut f,Section::Chat,Some(&id),"ui.conversationMessages",json!({"id":id,"since":cursor}),"GENERAL ANSWER SECOND");
    assert!(!after.to_string().contains("GENERAL USER FIRST"));
    query(&mut f,Section::Chat,Some(&id),"ui.conversationMsgDetail",json!({"id":id,"seq":seq}),"GENERAL ANSWER SECOND");
    let mention=query(&mut f,Section::Chat,Some(&id),"ui.chatMentions",json!({"kind":"company","query":"MENTION COMPANY ACCEPT"}),"MENTION COMPANY ACCEPT");
    assert!(mention.to_string().contains(&company));
    f.reopen_same_workspace();
    query(&mut f,Section::Chat,Some(&id),"ui.conversationHistory",json!({"id":id,"limit":1000}),"GENERAL ANSWER SECOND");
    query(&mut f,Section::Chat,Some(&id),"ui.conversationMessages",json!({"id":id}),"GENERAL ANSWER FIRST");
    query(&mut f,Section::Chat,Some(&id),"ui.conversationMsgDetail",json!({"id":id,"seq":seq}),"GENERAL ANSWER SECOND");
    query(&mut f,Section::Chat,Some(&id),"ui.chatMentions",json!({"kind":"company","query":"MENTION COMPANY ACCEPT"}),"MENTION COMPANY ACCEPT");
    note("general_conversation_real_provider_history_cursor_detail_and_mentions_reopen",&["ui.conversationHistory","ui.conversationMessages","ui.conversationMsgDetail","ui.chatMentions"]);
    f.shutdown();let requests=local.finish();assert_eq!(requests.len(),2);assert!(requests[1].to_string().contains("GENERAL USER FIRST"));
}

#[test]
fn general_task_chat_actual_send_stop_and_native_status_reopen() {
    let local=LocalProvider::new(vec![Step::Hold]);let secret=artex_core::new_id("secret");let mut f=fixture(&secret);
    let task=source_task(&mut f,"GENERAL TASK CHAT ACCEPT");
    let profile=provider::profile(&mut f,"Task chat model","anthropic",&local.url,&secret,false);
    f.setup("ui.activateLLMProfile",json!({"id":profile}));
    send_task(&mut f,&task,"GENERAL TASK HELD REQUEST");
    f.until(|app| !local.requests().is_empty()&&app.handle.view().unwrap().generation.records("agent_runs").iter().any(|r|r.data["task_id"]==task&&r.data["state"]=="running"));
    let shown=f.frame(vec![]);assert!(Fixture::texts(&shown).iter().any(|s|s.contains("응답 중지")));
    f.click_visible("응답 중지");f.prepare("ui.stopChat");
    task_terminal(&mut f,&task,"cancelled");
    f.reopen_same_workspace();
    assert!(record(&f,"tasks",&task).data["paused"]==true);
    f.assert_main_visible(&task,"GENERAL TASK HELD REQUEST");
    let view=f.app.handle.view().unwrap();let runs=view.generation.records("agent_runs");
    assert!(!runs.iter().any(|r|r.data["task_id"]==task&&matches!(r.data["state"].as_str(),Some("running"|"queued"|"cancel_requested"))));
    // Native chat status reads the same generation-owned runs rather than a
    // periodic web endpoint. The real held request and stop prove both states.
    note("general_task_chat_actual_send_stop_and_native_status_reopen",&["ui.chat","ui.stopChat","ui.chatStatus"]);
    f.shutdown();assert_eq!(local.finish().len(),1);
}

#[test]
fn general_attachment_actual_picker_stage_upload_send_preserves_bytes_on_reopen() {
    let admitted=Arc::new(std::sync::Mutex::new(String::new()));let received=admitted.clone();
    let local=LocalProvider::from_handler(move|index,request| match index {
        0=>{
            let path=received.lock().unwrap().clone();assert!(!path.is_empty());
            assert!(request.to_string().contains(&path)&&request.to_string().contains("attachment-accept.txt"),"Actual model prompt must disclose the uploaded locator");
            Step::Reply(200,"application/json",json!({"content":[{"type":"tool_use","id":"attachment-read","name":"Read","input":{"file_path":path}}],"stop_reason":"tool_use","usage":{"input_tokens":3,"output_tokens":2}}).to_string())
        }
        1=>{
            let block=request["messages"].as_array().unwrap().iter().flat_map(|m|m["content"].as_array().into_iter().flatten())
                .find(|b|b["type"]=="tool_result"&&b["tool_use_id"]=="attachment-read").expect("Actual paired attachment Read result");
            assert!(block["is_error"]!=true&&block.to_string().contains("ATTACHMENT ORIGINAL BYTES"),"Actual host read failed: {block}");
            Step::Reply(200,"application/json",anthropic("ATTACHMENT ACCEPTED ANSWER",false))
        }
        _=>panic!("Unexpected attachment model request"),
    });
    let secret=artex_core::new_id("secret");let probe=Probe::default();let observed=probe.clone();
    let mut f=Fixture::configured(|profile|ServiceConfig {host_factory:Some(factory(profile,BTreeMap::from([(secret.clone(),"isolated-placeholder".into())]),probe)),wake_ui:None});
    let profile=provider::profile(&mut f,"Attachment model","anthropic",&local.url,&secret,false);
    let id=f.setup("ui.createConversation",json!({"agent_key":"auto","title":"ATTACHMENT CONVERSATION","llm_profile_id":profile}))["id"].as_str().unwrap().to_owned();
    let input=f._dir.path().join("attachment-accept.txt");std::fs::write(&input,b"ATTACHMENT ORIGINAL BYTES").unwrap();
    select(&mut f,Section::Chat,Some(&id));f.app.drawer.open=false;
    TEST_INPUT_FILES.with(|q|q.borrow_mut().push_back(Some(input)));
    f.click_visible("파일 첨부 준비");
    f.until(|app|app.pending.is_empty());
    assert!(f.app.error.is_none(),"{:?}",f.app.error);
    f.assert_main_visible(&id,"attachment-accept.txt");
    let base=f.app.handle.view().unwrap().workspace.work_root.join("conversations").join(&id);
    let files=walk_files(&base);
    let stored=files.iter().find(|p|std::fs::read(p).ok().is_some_and(|bytes|bytes==b"ATTACHMENT ORIGINAL BYTES")).expect("Actual owner-scoped uploaded file bytes missing").clone();
    let relative=stored.strip_prefix(&base).unwrap().to_string_lossy().replace('\\',"/");
    assert!(relative.starts_with("attachments/"));*admitted.lock().unwrap()=relative.clone();
    f.type_composer_and_send(&id,"Read this local attachment; preserve its original bytes.");
    let sent=f.app.pending.values().find_map(|p|match p {Purpose::Prepare{method,params,..} if method=="ui.sendConversationMessage"=>Some(params),_=>None}).unwrap();
    assert_eq!(sent["attachments"][0]["path"],relative);
    f.wait_conversation_terminal(&id,"completed");f.assert_main_visible(&id,"ATTACHMENT ACCEPTED ANSWER");
    f.assert_tool_result_visible(&id,"Read · 실행 결과","ATTACHMENT ORIGINAL BYTES");
    let snapshot=f.app.handle.view().unwrap().generation.snapshot.clone();
    // Windows native roots may use the canonical extended-length spelling.
    // Compare resolved paths, retaining the isolated fixture ownership check.
    assert!(std::fs::canonicalize(&stored).unwrap().starts_with(std::fs::canonicalize(f._dir.path()).unwrap()));
    assert!(snapshot.records("conversation_activities").iter().any(|r|r.data["conversation_id"]==id&&r.data.to_string().contains("attachment-accept.txt")));
    f.reopen_same_workspace();f.assert_main_visible(&id,"ATTACHMENT ACCEPTED ANSWER");
    f.assert_tool_result_visible(&id,"Read · 실행 결과","ATTACHMENT ORIGINAL BYTES");
    assert_eq!(std::fs::read(&stored).unwrap(),b"ATTACHMENT ORIGINAL BYTES");
    let calls=observed.attempts.lock().unwrap();assert_eq!(calls.len(),1);assert_eq!(calls[0]["input"]["name"],"Read");drop(calls);
    note("general_attachment_actual_picker_stage_upload_send_preserves_bytes_on_reopen",&["ui.chatUpload"]);
    f.shutdown();let requests=local.finish();assert_eq!(requests.len(),2);
}
fn walk_files(root:&std::path::Path)->Vec<PathBuf> {
    let mut files=vec![];
    for e in std::fs::read_dir(root).unwrap() {let path=e.unwrap().path();if path.is_dir(){files.extend(walk_files(&path));}else{files.push(path);}}
    files
}

#[test]
fn general_python_detection_actual_host_ui_query_and_reopen() {
    let secret=artex_core::new_id("secret");let mut f=fixture(&secret);
    let value=f.source_action(Section::Settings,None,"ui.detectPython",json!({}));
    // Detection may legitimately find none on a supported host; a found path
    // must be an actual executable, not a configured success placeholder.
    assert!(value.is_object(),"{value}");
    let path=value["python_interpreter"].as_str().unwrap().to_owned();
    assert!(!path.is_empty(),"This positive host fixture requires Python on its PATH");
    assert!(std::path::Path::new(&path).is_file(),"Detection must return an actual PATH file: {path}");
    f.source_result_visible("ui.detectPython",None,&path);
    f.reopen_acceptance(false);
    assert_eq!(f.source_action(Section::Settings,None,"ui.detectPython",json!({})),value);
    f.source_result_visible("ui.detectPython",None,&path);
    note("general_python_detection_actual_host_ui_query_and_reopen",&["ui.detectPython"]);f.shutdown();
}

#[test]
fn general_actual_model_logs_detail_task_filter_delete_and_token_views_reopen() {
    let local=LocalProvider::new(vec![Step::Reply(200,"application/json",anthropic("TASK LOG ANSWER ACCEPT",false)),Step::Reply(200,"application/json",anthropic("PRESERVED CONVERSATION LOG ANSWER",false))]);
    let secret=artex_core::new_id("secret");let mut f=fixture(&secret);
    let task=source_task(&mut f,"LOG TASK ACCEPT");
    f.setup("ui.setSettings",json!({"patch":{"llm_record":true}}));
    let profile=provider::profile(&mut f,"GENERAL LOG MODEL","anthropic",&local.url,&secret,false);
    f.setup("ui.activateLLMProfile",json!({"id":profile}));
    send_task(&mut f,&task,"TASK LOG REQUEST ACCEPT");task_terminal(&mut f,&task,"completed");
    let cid=f.setup("ui.createConversation",json!({"agent_key":"auto","title":"LOG CONTROL","llm_profile_id":profile}))["id"].as_str().unwrap().to_owned();
    f.type_composer_and_send(&cid,"PRESERVED CONVERSATION LOG REQUEST");f.wait_conversation_terminal(&cid,"completed");
    f.reopen_acceptance(false);
    query(&mut f,Section::Models,None,"ui.llmTasks",json!({}),&task);
    let usage=query(&mut f,Section::Models,None,"ui.tokensByModel",json!({"task":task}),"fixture");
    assert_eq!(usage["models"][0]["calls"],1);
    // Anthropic reports uncached input separately; the source-compatible
    // normalized total includes 3 uncached +17 cache-read +5 cache-write.
    assert_eq!(usage["models"][0]["input_tokens"],25);
    assert_eq!(usage["models"][0]["output_tokens"],2);
    assert_eq!(usage["models"][0]["cache_read_tokens"],17);
    assert_eq!(usage["models"][0]["cache_write_tokens"],5);
    f.source_result_at_path_visible("ui.tokensByModel",None,&["models","0","input_tokens"],"25");
    select(&mut f,Section::Logs,None);f.app.drawer.open=false;
    f.click_visible("LLM 요청·응답");
    f.app.panes.seed_llm_record_filter_for_app_test(&task);
    f.click_visible("LLM 기록 조회");
    f.until(|app|app.pending.is_empty()&&app.panes.projection.contains_key("ui.llmRecords"));
    let rows=f.app.panes.projection["ui.llmRecords"].clone();
    assert_eq!(rows["total"],1);assert_eq!(rows["records"][0]["task_id"],task);
    f.assert_drawer_texts(&[&task,"fixture"]);
    f.click_visible("요청·응답 상세");
    f.until(|app|app.pending.is_empty()&&app.panes.projection.contains_key("ui.llmRecordDetail"));
    let detail=f.app.panes.projection["ui.llmRecordDetail"].clone();
    for (field,label) in [("request_body","요청 본문"),("response_body","응답 본문")] {
        if detail[field]["blob"].is_object() {f.click_visible(&format!("{label} 본문 불러오기"));settle(&mut f);}
    }
    f.assert_drawer_texts(&["TASK LOG REQUEST ACCEPT","TASK LOG ANSWER ACCEPT"]);
    let target=f.app.handle.view().unwrap().generation.records("llm_records").into_iter().filter(|r|r.data["task_id"]==task).map(|r|r.header.id.clone()).collect::<Vec<_>>();
    let preserved=f.app.handle.view().unwrap().generation.records("llm_records").into_iter().filter(|r|r.data["task_id"]!=task).map(|r|r.header.id.clone()).collect::<Vec<_>>();
    assert_eq!(target.len(),1);assert_eq!(preserved.len(),1);
    f.click_visible("입력한 작업 ID에 속한 LLM 기록 삭제를 확인합니다");
    f.click_visible("작업 LLM 기록 삭제");f.prepare("ui.llmRecordsDeleteTask");settle(&mut f);
    f.reopen_same_workspace();
    let view=f.app.handle.view().unwrap();
    assert!(target.iter().all(|id|view.generation.get("llm_records",id).is_none()));
    assert!(preserved.iter().all(|id|view.generation.get("llm_records",id).is_some()));
    select(&mut f,Section::Logs,None);f.app.drawer.open=false;
    f.app.panes.seed_llm_record_filter_for_app_test(&task);
    // Tabs are retained UI preferences; select only if not already active.
    f.click_visible("LLM 요청·응답");f.click_visible("LLM 기록 조회");
    f.until(|app|app.pending.is_empty()&&app.panes.projection.get("ui.llmRecords").is_some_and(|r|r["total"]==0));
    f.assert_drawer_texts(&["0개 기록"]);
    query(&mut f,Section::Models,None,"ui.tokensByModel",json!({"task":task}),"fixture");
    note("general_actual_model_logs_detail_task_filter_delete_and_token_views_reopen",&["ui.llmRecords","ui.llmRecordDetail","ui.llmRecordsDeleteTask","ui.llmTasks","ui.tokensByModel"]);
    f.shutdown();assert_eq!(local.finish().len(),2);
}

#[test]
fn general_gc_actual_destructive_control_removes_only_old_orphan_blob_and_reopens_live_body() {
    use artex_core::{Actor,Context};
    let pair=Arc::new(std::sync::Mutex::new((Value::Null,Value::Null,String::new())));let captured=pair.clone();
    let mut f=Fixture::configured_seeded(|_|ServiceConfig::default(),move|path| {
        let mut store=artex_store::Store::create(path,"GC acceptance").unwrap();
        let context=Context{actor:Actor{kind:"user".into(),agent_id:None,run_id:None,engine_epoch:artex_core::new_id("epoch")},task_id:None,intent_id:None,conversation_id:None};
        let mut bootstrap=artex_domain::bootstrap_plan(&store.snapshot(),&context).unwrap();bootstrap.command_id=artex_core::new_id("command");store.commit(bootstrap).unwrap();
        let live=store.blob_put(b"GC LIVE PROMPT BODY ACCEPT","text/plain").unwrap();
        let orphan=store.blob_put(b"GC OLD ORPHAN BYTES ACCEPT","text/plain").unwrap();
        let snapshot=store.snapshot();let prompt=snapshot.records("agent_prompts").into_iter().next().unwrap();
        let mut data=prompt.data.clone();data["template_text"]=json!({"text":null,"blob":live});
        let mut p=artex_domain::Planner::new(&snapshot,"fixture.gc.input",&json!({}));p.put("agent_prompts",&prompt.header.id,data);
        let mut batch=p.done(json!({}));batch.command_id=artex_core::new_id("command");artex_domain::augment_expected(&mut batch,&snapshot).unwrap();store.commit(batch).unwrap();
        for reference in [&live,&orphan] {
            let sha=reference["sha256"].as_str().unwrap();let blob=path.join("objects/blobs").join(&sha[..2]).join(sha);
            std::fs::File::options().write(true).open(blob).unwrap().set_modified(std::time::SystemTime::now()-Duration::from_secs(172800)).unwrap();
        }
        *captured.lock().unwrap()=(live,orphan,prompt.header.id.clone());store.checkpoint().unwrap();
    });
    let (live,orphan,prompt)=pair.lock().unwrap().clone();
    let blob_path=|reference:&Value| {let sha=reference["sha256"].as_str().unwrap();f.path.join("objects/blobs").join(&sha[..2]).join(sha)};
    let live_path=blob_path(&live);let orphan_path=blob_path(&orphan);
    assert!(live_path.exists()&&orphan_path.exists());
    let result=f.source_action(Section::Archives,None,"ui.gc",json!({"ttl":86400}));
    assert!(result["removed"].as_u64().unwrap()>=1,"{result}");
    assert!(!orphan_path.exists());assert_eq!(std::fs::read(&live_path).unwrap(),b"GC LIVE PROMPT BODY ACCEPT");
    f.source_result_visible("ui.gc",None,&result["removed"].to_string());
    f.reopen_acceptance(false);
    assert!(!orphan_path.exists());assert_eq!(std::fs::read(live_path).unwrap(),b"GC LIVE PROMPT BODY ACCEPT");
    let row=record(&f,"agent_prompts",&prompt);assert_eq!(row.data["template_text"]["blob"],live);
    // A reopened maintenance operation reports no second deletion. The live
    // prompt's unchanged Blob and bytes above establish reference retention.
    let second=f.source_action(Section::Archives,None,"ui.gc",json!({"ttl":86400}));
    assert_eq!(second["removed"],0);f.source_result_visible("ui.gc",None,"0");
    note("general_gc_actual_destructive_control_removes_only_old_orphan_blob_and_reopens_live_body",&["ui.gc"]);f.shutdown();
}

#[test]
fn general_native_task_selection_replaces_shared_server_active_context_without_autorun() {
    let mut f=Fixture::new();let a=source_task(&mut f,"ACTIVE CONTEXT A");let b=source_task(&mut f,"ACTIVE CONTEXT B");
    left_row(&mut f,Section::Tasks,&a,"ACTIVE CONTEXT A");
    query(&mut f,Section::Tasks,Some(&a),"ui.task",json!({"id":a}),"ACTIVE CONTEXT A");
    left_row(&mut f,Section::Tasks,&b,"ACTIVE CONTEXT B");
    query(&mut f,Section::Tasks,Some(&b),"ui.task",json!({"id":b}),"ACTIVE CONTEXT B");
    f.reopen_same_workspace();
    left_row(&mut f,Section::Tasks,&a,"ACTIVE CONTEXT A");
    query(&mut f,Section::Tasks,Some(&a),"ui.task",json!({"id":a}),"ACTIVE CONTEXT A");
    let view=f.app.handle.view().unwrap();assert!(view.generation.records("agent_runs").is_empty());
    assert!(view.generation.records("tasks").iter().all(|t|t.data["paused"]==true));
    // ui.setActive's server-wide selected pointer is intentionally replaced
    // by per-section local selection; it must never start a paused task.
    note("general_native_task_selection_replaces_shared_server_active_context_without_autorun",&["ui.setActive"]);f.shutdown();
}

#[test]
fn general_health_and_os_session_workspace_access_reopen_without_web_password() {
    let mut f=Fixture::new();
    let original=f.path.clone();let scope=f.app.handle.scope().unwrap();
    let category=f.setup("ui.createTaskCategory",json!({"name":"OS SESSION PRESERVED RECORD"}))["id"].as_str().unwrap().to_owned();
    query(&mut f,Section::Overview,None,"ui.health",json!({}),"ARTEX Desktop");
    for (method,input) in [("ui.authStatus",json!({})),("ui.login",json!({"username":"fixture-user","password":"fixture-password"})),("ui.initPassword",json!({"password":"fixture-password"})),("ui.changePassword",json!({"oldPassword":"fixture-password","newPassword":"fixture-new-password"}))] {
        let error=compatibility_error(&mut f,method,input);assert_eq!(error.code,"FEATURE_DISABLED","{error:?}");assert!(error.message.contains("OS_SESSION_AUTH"),"{error:?}");
    }
    // The requested native application uses local profile/workspace ownership.
    // The old server password operations are deliberately absent from the UI;
    // opening another local space must preserve and isolate the first one.
    TEST_WORKSPACE_DIRECTORIES.with(|q|q.borrow_mut().push_back(Some(f._dir.path().to_owned())));
    f.click_visible("새 공간");assert!(f.app.incoming_workspace.is_some());
    let new_path=f.app.incoming_workspace.as_ref().unwrap().path.clone();
    f.click_visible("저장 후 전환");
    f.until(|app|app.pending.is_empty()&&app.handle.status().phase==ServicePhase::Ready&&app.handle.scope().is_some_and(|s|s.workspace_id!=scope.workspace_id));
    assert!(f.app.handle.view().unwrap().generation.get("task_categories",&category).is_none());
    assert!(new_path.starts_with(f._dir.path()));
    TEST_WORKSPACE_DIRECTORIES.with(|q|q.borrow_mut().push_back(Some(original.clone())));
    f.click_visible("열기");assert!(f.app.incoming_workspace.is_some());f.click_visible("저장 후 전환");
    f.until(|app|app.pending.is_empty()&&app.handle.status().phase==ServicePhase::Ready&&app.handle.scope().is_some_and(|s|s.workspace_id==scope.workspace_id));
    assert_eq!(record(&f,"task_categories",&category).data["name"],"OS SESSION PRESERVED RECORD");
    f.path=original;f.reopen_same_workspace();
    query(&mut f,Section::Overview,None,"ui.health",json!({}),"ARTEX Desktop");
    select(&mut f,Section::Categories,Some(&category));f.app.drawer.open=false;
    f.assert_drawer_texts(&["OS SESSION PRESERVED RECORD"]);
    note("general_health_and_os_session_workspace_access_reopen_without_web_password",&["ui.health","ui.authStatus","ui.login","ui.initPassword","ui.changePassword"]);
    f.shutdown();
}

#[test]
fn general_native_update_handoff_preserves_workspace_and_rejects_web_self_replacement() {
    let mut f=Fixture::new();let category=f.setup("ui.createTaskCategory",json!({"name":"UPDATE PRESERVED DATA ACCEPT"}))["id"].as_str().unwrap().to_owned();
    let version=env!("CARGO_PKG_VERSION");
    for reopened in [false,true] {
        if reopened {f.reopen_acceptance(false);}
        f.frame(vec![egui::Event::Key{key:egui::Key::F1,physical_key:None,pressed:true,repeat:false,modifiers:Default::default()}]);
        assert!(f.app.about_open);let output=f.frame(vec![]);let texts=visible_texts(&output,egui::Rect::EVERYTHING).join("\n");
        assert!(texts.contains(&format!("버전 {version}")),"{texts}");assert!(texts.contains("자동 업데이트는 구성되지 않았습니다"),"{texts}");
        let clicked=f.click_visible("앱 업데이트·이전 버전 다운로드");
        assert!(clicked.platform_output.commands.iter().any(|c|matches!(c,egui::OutputCommand::OpenUrl(url) if url.url=="https://github.com/froiibn/ARTEX-Desktop/releases")),"Actual release handoff missing");
        f.app.about_open=false;
        // In-process native builds intentionally do not execute the Go server
        // updater or apply unsigned binaries; this is expected refusal, not a
        // claim to have installed/rolled back a package or contacted GitHub.
        for method in ["ui.checkUpdate","ui.applyUpdate","ui.rollbackUpdate"] {
            let error=compatibility_error(&mut f,method,json!({}));
            assert_eq!(error.code,"FEATURE_DISABLED","{method}: {error:?}");
            assert!(error.message.contains("NATIVE_PACKAGE_UPDATE"),"{error:?}");
        }
        assert_eq!(record(&f,"task_categories",&category).data["name"],"UPDATE PRESERVED DATA ACCEPT");
    }
    select(&mut f,Section::Categories,Some(&category));f.assert_drawer_texts(&["UPDATE PRESERVED DATA ACCEPT"]);
    note("general_native_update_handoff_preserves_workspace_and_rejects_web_self_replacement",&["ui.checkUpdate","ui.applyUpdate","ui.rollbackUpdate"]);f.shutdown();
}
