//! Actual headless App input -> local provider/host -> store -> reopen -> UI.
//! These fixture listeners never use user keys, external sites or the OS vault.
use super::transport::*;
use super::*;

#[test]
fn actual_app_shared_default_model_completes_chat_after_switch_without_reconfiguration() {
    let provider = LocalProvider::new(vec![
        Step::Reply(200, "application/json", anthropic("SHARED_MODEL_IN_A", false)),
        Step::Reply(200, "application/json", anthropic("SHARED_MODEL_IN_B", false)),
    ]);
    let secret = artex_core::new_id("secret");
    let mut f = fixture(Probe::default(), BTreeMap::from([(secret.clone(), "local-shared-placeholder".into())]));
    let profile_id = profile(&mut f, "Common model fixture", "anthropic", &provider.url, &secret, false);
    f.setup("ui.activateLLMProfile", json!({"id":profile_id}));
    let a = f.setup("ui.createConversation", json!({"agent_key":"auto","title":"First workspace"}))["id"].as_str().unwrap().to_owned();
    f.type_composer_and_send(&a, "Local common model check A");
    f.wait_conversation_terminal(&a, "completed");
    f.assert_main_visible(&a, "SHARED_MODEL_IN_A");
    let old_scope = f.app.handle.scope().unwrap();
    f.app.close();
    f.until(|app| app.handle.status().phase == ServicePhase::Closed && app.pending.is_empty());
    f.path = f._dir.path().join("second-workspace");
    f.app.submit(NativeAction::Open(OpenOptions { path: f.path.clone(),
        mode: OpenMode::Create { name: "Second shared-model workspace".into() }, read_only: false }), Purpose::Open(f.path.clone()));
    f.until(|app| app.handle.status().phase == ServicePhase::Ready && app.pending.is_empty());
    assert_ne!(f.app.handle.scope().unwrap().workspace_id, old_scope.workspace_id);
    assert!(f.app.handle.view().unwrap().generation.get("conversations", &a).is_none());
    assert_eq!(f.app.handle.view().unwrap().generation.get("llm_profiles", &profile_id).unwrap().data["is_default"], true);
    // Neither a model binding nor another Save/Activate is submitted in B.
    let b = f.setup("ui.createConversation", json!({"agent_key":"auto","title":"Uses common default"}))["id"].as_str().unwrap().to_owned();
    f.type_composer_and_send(&b, "Local common model check B");
    f.wait_conversation_terminal(&b, "completed");
    f.assert_main_visible(&b, "SHARED_MODEL_IN_B");
    f.reopen_same_workspace();
    f.assert_main_visible(&b, "SHARED_MODEL_IN_B");
    f.shutdown();
    let requests = provider.finish();
    assert_eq!(requests.len(), 2);
    assert!(requests[1].to_string().contains("Local common model check B"));
}

fn fixture(probe: Probe, secrets: BTreeMap<String,String>) -> Fixture {
    Fixture::new_with_config(|profile|ServiceConfig {
        host_factory:Some(factory(profile,secrets,probe)),wake_ui:None,
    })
}
fn profile(f:&mut Fixture,name:&str,format:&str,url:&str,secret:&str,streaming:bool)->String {
    f.setup("ui.saveLLMProfile",json!({"p":{"name":name,"format":format,"model":"fixture","base_url":url,"streaming":streaming,"api_key":{"secret_ref":secret,"hint":"memory fixture"},"retry":{"connect":{"attempts":-1,"interval_ms":0},"empty":{"attempts":-1,"interval_ms":0},"stream":{"attempts":-1,"interval_ms":0}}}}))["id"].as_str().unwrap().into()
}
fn conversation(f:&mut Fixture,profile:&str,title:&str)->String {
    f.setup("ui.createConversation",json!({"agent_key":"auto","title":title,"llm_profile_id":profile}))["id"].as_str().unwrap().into()
}
fn usage(f:&Fixture)->Vec<Value> {
    f.app.handle.view().unwrap().generation.records("llm_usage").into_iter().map(|r|r.data.clone()).collect()
}
fn exact_usage(f:&Fixture,name:&str,calls:usize,input:i64,output:i64,read:i64,write:i64) {
    let rows=usage(f).into_iter().filter(|r|r["profile_name"]==name).collect::<Vec<_>>();
    assert_eq!(rows.len(),calls,"{rows:?}");
    for row in rows {
        assert_eq!(row["status"],"completed");
        assert_eq!(row["input_tokens"],input);assert_eq!(row["output_tokens"],output);
        assert_eq!(row["cache_read"],read);assert_eq!(row["cache_write"],write);
    }
}
fn click_first(f:&mut Fixture,label:&str) {
    f.scroll_drawer_top();
    for _ in 0..40 {
        let out=f.frame(vec![]);
        let pos=out.shapes.iter().find_map(|clipped| {
            let egui::epaint::Shape::Text(text)=&clipped.shape else {return None;};
            let rect=text.visual_bounding_rect();
            (text.galley.text()==label && clipped.clip_rect.contains_rect(rect)).then(||rect.center())
        });
        if let Some(pos)=pos {
            f.frame(vec![egui::Event::PointerMoved(pos),egui::Event::PointerButton{pos,button:egui::PointerButton::Primary,pressed:true,modifiers:Default::default()}]);
            f.frame(vec![egui::Event::PointerButton{pos,button:egui::PointerButton::Primary,pressed:false,modifiers:Default::default()}]);return;
        }
        f.scroll_drawer_down();
    }
    panic!("Actual first visible {label} missing");
}
fn assert_drawer(f:&mut Fixture,needles:&[&str]) { f.assert_drawer_texts(needles); }
fn scalar_visible(f:&mut Fixture,label:&str,value:&str)->bool {
    let out=f.frame(vec![]);let area=f.source_area();
    let texts=out.shapes.iter().filter_map(|clipped| {
        let egui::epaint::Shape::Text(text)=&clipped.shape else{return None;};
        let rect=text.visual_bounding_rect();
        clipped.clip_rect.intersect(area).contains_rect(rect).then(||(text.galley.text(),rect))
    }).collect::<Vec<_>>();
    texts.iter().filter(|(text,_)|*text==label).any(|(_,heading)|texts.iter().any(|(text,rect)|*text==value && rect.center().y>heading.center().y && rect.top()-heading.bottom()<40. && rect.left()>=heading.left()-4.))
}
fn open_scalar(f:&mut Fixture,label:&str,value:&str) {
    if !scalar_visible(f,label,value) {f.click_visible(label);}
    assert!(scalar_visible(f,label,value),"Actual expanded {label} did not render its value {value}");
}
fn usage_period_ui(f:&mut Fixture) {
    f.source_query(Section::Overview,None,"ui.usageStats",json!({"days":1}));
    let actual=f.app.source_operations.query_result_for_app_test("ui.usageStats",None).unwrap();
    assert_eq!(actual["by_profile"].as_array().unwrap().len(),1);
    let p=&actual["by_profile"][0];
    assert_eq!(p["profile_name"],"Cap fixture");assert_eq!(p["input_tokens"],50);
    assert_eq!(p["output_tokens"],4);assert_eq!(p["cache_read_tokens"],34);assert_eq!(p["cache_write_tokens"],10);
    // Both by_profile and daily have item 1; choose the first rendered one.
    // Expand the real result tree, never write a synthesized display value.
    let out=f.frame(vec![]);
    if !visible_texts(&out,f.source_area()).iter().any(|s|s=="profile_name") {click_first(f,"항목 1");}
    for (label,value) in [("profile_name","Cap fixture"),("input_tokens","50"),("output_tokens","4"),("cache_read_tokens","34"),("cache_write_tokens","10")] {open_scalar(f,label,value);}
    assert_drawer(f,&["Cap fixture","input_tokens","output_tokens","cache_read_tokens","cache_write_tokens","50","34","10"]);
}
fn activity_seq(f:&Fixture,cid:&str,needle:&str)->String {
    f.app.handle.view().unwrap().generation.records("conversation_activities").into_iter()
        .find(|r|r.data["conversation_id"]==cid && r.data["detail"]["text"].as_str().is_some_and(|s|s.contains(needle)))
        .unwrap_or_else(||panic!("No actual persisted activity contains {needle}")).header.id.clone()
}
fn history_text(f:&Fixture,cid:&str)->String {
    f.app.handle.view().unwrap().generation.records("conversation_activities").into_iter()
        .filter(|r|r.data["conversation_id"]==cid)
        .map(|r|r.data["detail"]["text"].as_str().expect("Small fixture detail stays inline").to_owned())
        .collect::<Vec<_>>().join("\n")
}
fn health_absent(f:&Fixture,ids:&[&str]) {
    let view=f.app.handle.view().unwrap();
    let rows=view.generation.records("llm_profile_health");
    assert!(!rows.iter().any(|r|ids.iter().any(|id|r.data["profile_id"]==*id)),"{rows:?}");
}
fn no_child(f:&Fixture,cid:&str) {
    let view=f.app.handle.view().unwrap();
    let runs=view.generation.records("agent_runs").into_iter().filter(|r|r.data["conversation_id"]==cid).collect::<Vec<_>>();
    assert_eq!(runs.len(),1,"Explicit zero wrapup must not create a settlement child");
    assert_eq!(runs[0].data["state"],"failed");
    assert!(runs[0].data["parent_run_id"].is_null());
}
fn pool_ui(f:&mut Fixture,names:&[&str],failed:Option<&str>) {
    f.source_query(Section::Models,None,"ui.llmPool",json!({}));
    let result=f.app.source_operations.query_result_for_app_test("ui.llmPool",None).unwrap();
    let chain=result["chain"].as_array().unwrap();assert_eq!(chain.len(),names.len());
    for (index,row) in chain.iter().enumerate() {
        let name=row["name"].as_str().unwrap();assert!(names.contains(&name));
        assert_eq!(row["fails"],if failed==Some(name){1}else{0});
        let state=if failed==Some(name){"degraded"}else{"ok"};assert_eq!(row["state"],state);
        let label=format!("항목 {}",index+1);f.click_visible(&label);
        open_scalar(f,"이름",name);open_scalar(f,"fails",if failed==Some(name){"1"}else{"0"});open_scalar(f,"state",state);
        assert_drawer(f,&[name,"fails",state]);
        // Close this actual row before opening the next so identical field
        // headings cannot select another profile's result.
        f.click_visible(&label);
    }
}

#[test]
fn actual_app_composer_cap_cache_usage_period_display_history_reopen() {
    let provider=LocalProvider::new(vec![Step::Reply(200,"application/json",anthropic("PARTIAL_EVIDENCE",true)),Step::Reply(200,"application/json",anthropic("FINAL_EVIDENCE",false))]);
    let secret=artex_core::new_id("secret");
    let mut f=fixture(Probe::default(),BTreeMap::from([(secret.clone(),"local-cap-placeholder".into())]));
    let profile=profile(&mut f,"Cap fixture","anthropic",&provider.url,&secret,false);
    f.setup("ui.saveAgentConfig",json!({"key":"auto","patch":{"max_turns":1}}));
    let cid=conversation(&mut f,&profile,"Cap UI fixture");
    f.type_composer_and_send(&cid,"Use the local fixture only.");f.wait_conversation_terminal(&cid,"completed");
    let requests=provider.requests();assert_eq!(requests.len(),2);
    assert!(requests[0].to_string().contains("Use the local fixture only."));
    assert!(requests[1].to_string().contains("PARTIAL_EVIDENCE"));assert!(requests[1].to_string().contains("Resume directly"));
    assert!(!requests[1].to_string().contains("Turn budget wrap-up"));
    exact_usage(&f,"Cap fixture",2,25,2,17,5);
    f.assert_main_visible(&cid,"FINAL_EVIDENCE");usage_period_ui(&mut f);
    let seq=activity_seq(&f,&cid,"PARTIAL_EVIDENCE");
    let out=f.source_body_detail(&cid,&seq);
    let _=out;f.assert_drawer_texts(&["PARTIAL_EVIDENCE"]);
    let before=usage(&f);f.reopen_same_workspace();
    assert_eq!(usage(&f),before);f.assert_main_visible(&cid,"FINAL_EVIDENCE");usage_period_ui(&mut f);
    let out=f.source_body_detail(&cid,&seq);
    let _=out;f.assert_drawer_texts(&["PARTIAL_EVIDENCE"]);
    f.shutdown();assert_eq!(provider.finish().len(),2);
}

#[test]
fn actual_app_composer_responses_named_terminal_empty_args_real_host_display_reopen() {
    for output in [None,Some(json!([]))] {
        let provider=LocalProvider::new(vec![Step::Reply(200,"text/event-stream",responses(output.clone(),true)),Step::Reply(200,"text/event-stream",responses(output,false))]);
        let secret=artex_core::new_id("secret");let probe=Probe::default();
        let mut f=fixture(probe.clone(),BTreeMap::from([(secret.clone(),"local-responses-placeholder".into())]));
        let profile=profile(&mut f,"Responses fixture","openai-responses",&provider.url,&secret,true);
        f.setup("ui.setSettings",json!({"patch":{"approval_bypass":true}}));
        f.setup("ui.saveAgentConfig",json!({"key":"auto","patch":{"interactive_shell":true}}));
        let cid=conversation(&mut f,&profile,"Responses UI fixture");
        f.type_composer_and_send(&cid,"List only the local fixture sessions.");f.wait_conversation_terminal(&cid,"completed");
        let requests=provider.requests();assert_eq!(requests.len(),2);
        assert!(requests[0].to_string().contains("List only the local fixture sessions."));
        let attempts=probe.attempts.lock().unwrap().clone();assert_eq!(attempts.len(),1);
        assert_eq!(attempts[0]["input"]["name"],"shell_list");
        // The model/ledger args are {}, while production host admission adds
        // its actual directory capability and attachment scope before dispatch.
        let scoped=&attempts[0]["input"]["input"];
        assert_eq!(scoped.as_object().unwrap().len(),2);
        assert_eq!(scoped["attachment_paths"],json!([]));
        assert!(artex_core::valid_id(scoped["directory_capability"].as_str().unwrap(),"capability"));
        let view=f.app.handle.view().unwrap();let invocations=view.generation.records("tool_invocations");
        assert_eq!(invocations.len(),1);let invocation=invocations[0].clone();
        assert_eq!(invocation.data["name"],"shell_list");assert_eq!(invocation.data["input"],json!({}));
        assert_eq!(invocation.data["state"],"completed");assert!(invocation.data["result_ref"]["sha256"].as_str().is_some_and(|s|s.len()==64));
        let outputs=requests[1]["input"].as_array().unwrap().iter().filter(|v|v["type"]=="function_call_output" && v["call_id"]=="responses-shell-list").collect::<Vec<_>>();
        assert_eq!(outputs.len(),1);let actual:Value=serde_json::from_str(outputs[0]["output"].as_str().unwrap()).unwrap();assert_eq!(actual["sessions"],json!([]));
        exact_usage(&f,"Responses fixture",2,9,4,0,0);
        assert_eq!(history_text(&f,&cid).matches("RESPONSES_OBSERVED_SENTINEL").count(),1);
        f.assert_main_visible(&cid,"RESPONSES_FINAL_SENTINEL");
        f.assert_tool_result_visible(&cid,"shell_list · 실행 결과","\"sessions\"");
        let seq=f.app.handle.view().unwrap().generation.records("conversation_activities").into_iter().find(|r|r.data["conversation_id"]==cid && r.data["kind"]=="tool_result" && r.data["tool_use_id"]=="responses-shell-list").expect("Actual typed tool-result activity").header.id.clone();
        let out=f.source_body_detail(&cid,&seq);let _=out;
        let body=f.app.source_operations.query_result_for_app_test("ui.conversationMsgDetail",Some(&cid)).unwrap();
        println!("APP_RESPONSES actual typed tool result detail {body}");
        let projected:Value=serde_json::from_str(body["text"].as_str().unwrap()).expect("Actual tool-result detail must retain JSON sessions");assert_eq!(projected,actual);
        f.assert_drawer_texts(&["sessions"]);
        let before=usage(&f);f.reopen_same_workspace();assert_eq!(usage(&f),before);
        assert_eq!(f.app.handle.view().unwrap().generation.get("tool_invocations",&invocation.header.id).unwrap().data,invocation.data);
        f.assert_main_visible(&cid,"RESPONSES_FINAL_SENTINEL");f.assert_tool_result_visible(&cid,"shell_list · 실행 결과","\"sessions\"");
        assert_eq!(probe.attempts.lock().unwrap().len(),1);f.shutdown();assert_eq!(provider.finish().len(),2);
    }
}

#[test]
fn actual_app_composer_responses_malformed_nonempty_arguments_do_not_dispatch() {
    let wire=responses(None,true).replace("\"arguments\":\"\"","\"arguments\":\"{\\\"path\\\":\"");
    let provider=LocalProvider::new(vec![Step::Reply(200,"text/event-stream",wire)]);
    let secret=artex_core::new_id("secret");let probe=Probe::default();
    let mut f=fixture(probe.clone(),BTreeMap::from([(secret.clone(),"local-invalid-placeholder".into())]));
    let profile=profile(&mut f,"Malformed fixture","openai-responses",&provider.url,&secret,true);
    f.setup("ui.setSettings",json!({"patch":{"approval_bypass":true}}));f.setup("ui.saveAgentConfig",json!({"key":"auto","patch":{"interactive_shell":true}}));
    let cid=conversation(&mut f,&profile,"Malformed UI fixture");
    f.type_composer_and_send(&cid,"Use only the malformed local fixture.");f.wait_conversation_terminal(&cid,"failed");
    assert_eq!(provider.requests().len(),1);assert!(provider.requests()[0].to_string().contains("Use only the malformed local fixture."));assert!(probe.attempts.lock().unwrap().is_empty());
    assert!(f.app.handle.view().unwrap().generation.records("tool_invocations").is_empty());
    assert_eq!(history_text(&f,&cid).matches("RESPONSES_OBSERVED_SENTINEL").count(),1);
    let seq=activity_seq(&f,&cid,"RESPONSES_OBSERVED_SENTINEL");let out=f.source_body_detail(&cid,&seq);
    let _=out;f.assert_drawer_texts(&["RESPONSES_OBSERVED_SENTINEL"]);
    f.reopen_same_workspace();assert_eq!(history_text(&f,&cid).matches("RESPONSES_OBSERVED_SENTINEL").count(),1);
    let out=f.source_body_detail(&cid,&seq);let _=out;f.assert_drawer_texts(&["RESPONSES_OBSERVED_SENTINEL"]);
    f.shutdown();assert_eq!(provider.finish().len(),1);
}

#[test]
fn actual_app_composer_own_deadline_zero_wrapup_keeps_pool_health_display_reopen() {
    let primary=LocalProvider::new(vec![Step::Hold]);
    let backup=LocalProvider::new(vec![Step::Reply(200,"application/json",anthropic("UNEXPECTED_BACKUP",false))]);
    let a=artex_core::new_id("secret");let b=artex_core::new_id("secret");
    let mut f=fixture(Probe::default(),BTreeMap::from([(a.clone(),"primary-placeholder".into()),(b.clone(),"backup-placeholder".into())]));
    let primary_id=profile(&mut f,"Deadline primary","anthropic",&primary.url,&a,false);
    let backup_id=profile(&mut f,"Deadline backup","anthropic",&backup.url,&b,false);
    f.setup("ui.setSettings",json!({"patch":{"llm_pool_enabled":true,"llm_pool_bind_fallback":true}}));
    f.setup("ui.saveAgentConfig",json!({"key":"auto","patch":{"run_seconds":5}}));
    // A successful child could hide poison by resetting primary health. Public
    // zero-wrapup finishes the parent and returns before creating that child.
    f.setup("ui.saveAgentWrapup",json!({"key":"auto","prompt":"","maxTurns":0}));
    let cid=conversation(&mut f,&primary_id,"Deadline UI fixture");
    f.type_composer_and_send(&cid,"Wait only on the local deadline fixture.");
    f.until(|_|!primary.requests().is_empty());f.wait_conversation_terminal(&cid,"failed");
    assert_eq!(primary.requests().len(),1);assert!(primary.requests()[0].to_string().contains("Wait only on the local deadline fixture."));assert!(backup.requests().is_empty());
    no_child(&f,&cid);health_absent(&f,&[&primary_id,&backup_id]);
    pool_ui(&mut f,&["Deadline primary","Deadline backup"],None);f.assert_main_visible(&cid,"Run deadline reached");
    f.reopen_same_workspace();no_child(&f,&cid);health_absent(&f,&[&primary_id,&backup_id]);
    f.assert_main_visible(&cid,"Run deadline reached");pool_ui(&mut f,&["Deadline primary","Deadline backup"],None);
    f.shutdown();assert_eq!(primary.finish().len(),1);assert!(backup.finish().is_empty());
}

#[test]
fn actual_app_composer_real_failure_uses_backup_keeps_health_usage_display_reopen() {
    let primary=LocalProvider::new(vec![Step::Reply(500,"application/json",json!({"error":{"message":"local fixture failure"}}).to_string())]);
    let backup=LocalProvider::new(vec![Step::Reply(200,"application/json",anthropic("FALLBACK_FINAL_EVIDENCE",false))]);
    let a=artex_core::new_id("secret");let b=artex_core::new_id("secret");
    let mut f=fixture(Probe::default(),BTreeMap::from([(a.clone(),"primary-placeholder".into()),(b.clone(),"backup-placeholder".into())]));
    let primary_id=profile(&mut f,"Failure primary","anthropic",&primary.url,&a,false);
    let backup_id=profile(&mut f,"Failure backup","anthropic",&backup.url,&b,false);
    f.setup("ui.setSettings",json!({"patch":{"llm_pool_enabled":true,"llm_pool_bind_fallback":true}}));
    f.setup("ui.saveAgentConfig",json!({"key":"auto","patch":{"run_seconds":30}}));
    f.setup("ui.saveAgentWrapup",json!({"key":"auto","prompt":"","maxTurns":0}));
    let cid=conversation(&mut f,&primary_id,"Failure UI fixture");
    f.type_composer_and_send(&cid,"Use local failure/backup fixtures only.");f.wait_conversation_terminal(&cid,"completed");
    assert_eq!(primary.requests().len(),1);assert_eq!(backup.requests().len(),1);assert!(primary.requests()[0].to_string().contains("Use local failure/backup fixtures only."));assert!(backup.requests()[0].to_string().contains("Use local failure/backup fixtures only."));
    let rows=usage(&f);assert_eq!(rows.len(),2);
    let failed=rows.iter().find(|r|r["profile_name"]=="Failure primary").unwrap();
    assert_eq!(failed["status"],"failed");assert_eq!(failed["input_tokens"],0);assert_eq!(failed["output_tokens"],0);
    exact_usage(&f,"Failure backup",1,25,2,17,5);health_absent(&f,&[&backup_id]);
    f.assert_main_visible(&cid,"FALLBACK_FINAL_EVIDENCE");pool_ui(&mut f,&["Failure primary","Failure backup"],Some("Failure primary"));
    f.reopen_same_workspace();assert_eq!(usage(&f),rows);f.assert_main_visible(&cid,"FALLBACK_FINAL_EVIDENCE");
    pool_ui(&mut f,&["Failure primary","Failure backup"],Some("Failure primary"));f.shutdown();assert_eq!(primary.finish().len(),1);assert_eq!(backup.finish().len(),1);
}
