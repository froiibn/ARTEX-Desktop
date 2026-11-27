//! Supplied native completion outcomes exercise writer/CAS boundaries only.
//! Real filesystem deletion and native input/display/reopen are covered by the
//! separate config acceptance case; these are not whole-application evidence.
use super::*;

fn fixture()->(tempfile::TempDir,Router,DispatchRequest,Vec<String>,String) {
    let (directory,mut router,_run,context)=crate::regression::seeded();let snapshot=router.snapshot().unwrap();let agent=context.actor.agent_id.unwrap();
    let mut p=Planner::new(&snapshot,"fixture.skill_delete",&json!({}));
    let second=p.insert("agents","agent",json!({"key":"worker","enabled":true})).unwrap();
    let mut ids=Vec::new();for (index,enabled) in [true,false].into_iter().enumerate() {ids.push(p.insert("agent_skill_visibility",artex_core::collection_prefix("agent_skill_visibility").unwrap(),json!({"agent_id":if index==0 {&agent} else {&second},"skill_name":"delete-me","enabled":enabled})).unwrap());}
    let other=p.insert("agent_skill_visibility",artex_core::collection_prefix("agent_skill_visibility").unwrap(),json!({"agent_id":agent,"skill_name":"retain-me","enabled":true})).unwrap();router.internal_commit(p.done(json!({}))).unwrap();
    let prepared=router.response(json!({"jsonrpc":"2.0","protocol":"artex-rpc/1.0","id":new_id("request"),"workspace_id":router.snapshot().unwrap().workspace_id,"engine_epoch":router.engine_epoch,"command_id":null,"expected":[],"method":"command.prepare","params":{"method":"ui.deleteSkill","params":{"name":"delete-me"}},"timeout_ms":5000}));
    assert!(prepared.get("error").is_none(),"{prepared}");let expected:Vec<Expected>=serde_json::from_value(prepared["result"]["expected"].clone()).unwrap();assert_eq!(expected.len(),2,"Both enabled and disabled grant identities are pinned before host I/O");
    let request=DispatchRequest{id:new_id("request"),method:"ui.deleteSkill".into(),workspace_id:Some(router.snapshot().unwrap().workspace_id.clone()),engine_epoch:Some(router.engine_epoch.clone()),command_id:Some(new_id("command")),expected,timeout_ms:5000,params:json!({"name":"delete-me"})};
    (directory,router,request,ids,other)
}
fn stage(router:&mut Router,request:&DispatchRequest) {
    router.preflight_skill_delete(request).unwrap();router.prepare_external_ui(request).unwrap();
    router.runtime.pending_ui.insert(request.id.clone(),request.clone());router.runtime.ui_generations.insert(request.id.clone(),router.workspace_generation.clone());
}
fn response(router:&mut Router)->Value {router.drain_responses().pop().unwrap()}

#[test]
fn skill_delete_success_cleans_all_exact_name_grants_retains_other_and_replays_without_host() {
    let (directory,mut router,request,ids,other)=fixture();let control=router.snapshot().unwrap().get("agent_skill_visibility",&other).unwrap().clone();stage(&mut router,&request);
    router.complete_ui(&request,Ok(json!({"deleted":"delete-me"}))).unwrap();assert_eq!(response(&mut router)["result"]["deleted"],"delete-me");let snapshot=router.snapshot().unwrap();for id in &ids {assert!(snapshot.get("agent_skill_visibility",id).is_none());}assert_eq!(snapshot.get("agent_skill_visibility",&other).unwrap(),&control);
    // A confirmed command must not perform filesystem deletion again, even
    // without a current gateway/capability. It returns the durable receipt.
    let user=Context {actor:Actor{kind:"user".into(),agent_id:None,run_id:None,engine_epoch:router.engine_epoch.clone()},task_id:None,intent_id:None,conversation_id:None};let before=snapshot.commit_seq;assert_eq!(router.native_ui(&request,&user).unwrap()["deleted"],"delete-me");assert_eq!(router.snapshot().unwrap().commit_seq,before);
    router.store.as_mut().unwrap().checkpoint().unwrap();router.store.take();router.store=Some(Store::open(&directory.path().join("store")).unwrap());for id in &ids {assert!(router.snapshot().unwrap().get("agent_skill_visibility",id).is_none());}assert_eq!(router.snapshot().unwrap().get("agent_skill_visibility",&other).unwrap(),&control);
}
#[test]
fn skill_delete_host_failure_keeps_grants_and_unconfirmed_prepared_receipt() {
    let (_directory,mut router,request,ids,_other)=fixture();stage(&mut router,&request);let before=router.snapshot().unwrap();router.complete_ui(&request,Err(Error::new("IO_ERROR","Fixture host deletion failed"))).unwrap();assert_eq!(response(&mut router)["error"]["data"]["code"],"IO_ERROR");let snapshot=router.snapshot().unwrap();assert_eq!(snapshot.commit_seq,before.commit_seq);for id in ids {assert_eq!(snapshot.get("agent_skill_visibility",&id),before.get("agent_skill_visibility",&id));}assert!(!snapshot.records("command_receipts").iter().any(|r|r.data["command_id"]==*request.command_id.as_ref().unwrap()));assert!(snapshot.records("command_receipts").iter().any(|r|r.data["method"]=="ui.deleteSkill.prepared"));
}
#[test]
fn skill_delete_stale_or_missing_grant_versions_fail_before_host_and_are_not_rebased_after_effect() {
    let (_directory,mut router,request,ids,_other)=fixture();let mut missing=request.clone();missing.expected.clear();assert_eq!(router.preflight_skill_delete(&missing).unwrap_err().code,"VERSION_CONFLICT");stage(&mut router,&request);
    let snapshot=router.snapshot().unwrap();let mut p=Planner::new(&snapshot,"fixture.concurrent_grant_edit",&json!({}));let mut data=snapshot.get("agent_skill_visibility",&ids[0]).unwrap().data.clone();data["enabled"]=json!(false);p.put("agent_skill_visibility",&ids[0],data);router.internal_commit(p.done(json!({}))).unwrap();assert_eq!(router.preflight_skill_delete(&request).unwrap_err().code,"VERSION_CONFLICT");let before=router.snapshot().unwrap();router.complete_ui(&request,Ok(json!({"deleted":"delete-me"}))).unwrap();assert_eq!(response(&mut router)["error"]["data"]["code"],"VERSION_CONFLICT");assert_eq!(router.snapshot().unwrap().commit_seq,before.commit_seq);for id in ids {assert_eq!(router.snapshot().unwrap().get("agent_skill_visibility",&id),before.get("agent_skill_visibility",&id));}
}
#[test]
fn skill_delete_foreign_generation_or_mismatched_resource_completion_cannot_remove_grants() {
    for stale in [false,true] {
        let (_directory,mut router,request,ids,_other)=fixture();stage(&mut router,&request);if stale {router.runtime.ui_generations.insert(request.id.clone(),new_id("epoch"));}let before=router.snapshot().unwrap();router.complete_ui(&request,Ok(json!({"deleted":if stale {"delete-me"} else {"retain-me"}}))).unwrap();assert_eq!(response(&mut router)["error"]["data"]["code"],if stale {"STALE_EPOCH"} else {"EXTERNAL_FAILURE"});assert_eq!(router.snapshot().unwrap().commit_seq,before.commit_seq);for id in ids {assert_eq!(router.snapshot().unwrap().get("agent_skill_visibility",&id),before.get("agent_skill_visibility",&id));}
    }
}
