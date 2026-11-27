//! Self-cancellation must commit its truthful receipt without granting terminal
//! actors another write capability. The native composer case covers its UI path.
use super::*;
use artex_domain::Planner;
use artex_runtime::{Message, ToolCall};

fn fixture() -> (tempfile::TempDir, Router, String, Context, String) {
    let (dir, mut router, run, old_context)=regression::seeded();
    let mut user=old_context.clone();user.actor.kind="user".into();
    let bootstrap=artex_domain::bootstrap_plan(&router.snapshot().unwrap(),&user).unwrap();router.internal_commit(bootstrap).unwrap();
    let plan=artex_domain::plan("task.create",&json!({"description":"Atomic pause fixture","goal":"Only isolated records","seed_first_intent":false}),&router.snapshot().unwrap(),&user).unwrap();
    let task=router.internal_commit(plan).unwrap()["task_id"].as_str().unwrap().to_owned();
    let snapshot=router.snapshot().unwrap();let original=snapshot.get("agent_runs",&run).unwrap();let mut data=original.data.clone();data["conversation_id"]=Value::Null;data["task_id"]=json!(task);data["state"]=json!("running");
    let mut p=Planner::new(&snapshot,"fixture.task_run",&json!({}));p.put("agent_runs",&run,data);
    let tool=snapshot.records("tools").into_iter().find(|r|r.data["key"]=="pause_task").unwrap();let mut data=tool.data.clone();data["enabled"]=json!(true);data["agents"]=json!(["mainagent"]);p.put("tools",&tool.header.id,data);
    p.insert("intercept_pending",artex_core::collection_prefix("intercept_pending").unwrap(),json!({"task_id":task,"tool_name":"pause_task","status":"allowed","reason":"Confirmed user grant","audit":{"run_id":run,"tool_use_id":"atomic-pause","correlation":"exact"}})).unwrap();
    router.internal_commit(p.done(json!({}))).unwrap();
    let context=artex_agent::context_for_run(&router.snapshot().unwrap(),&run,&router.engine_epoch).unwrap();
    (dir,router,run,context,task)
}
fn params(run:&str,task:&str)->Value {json!({"run_id":run,"tool_use_id":"atomic-pause","name":"pause_task","input":{"task_id":task},"raw_input":null})}
fn call(task:&str)->ToolCall {ToolCall{id:"atomic-pause".into(),name:"pause_task".into(),input:json!({"task_id":task})}}

#[test]
fn self_pause_receipt_and_cancel_share_one_commit_and_do_not_grant_terminal_append_authority() {
    let (dir,mut router,run,context,task)=fixture();
    let before=router.snapshot().unwrap();let plan=router.invoke_plan(&params(&run,&task),&context).unwrap();
    assert_eq!(router.snapshot().unwrap().commit_seq,before.commit_seq,"Planning may not publish a partial pause or receipt");
    let result=router.internal_commit(plan).unwrap();let snapshot=router.snapshot().unwrap();
    let actor=snapshot.get("agent_runs",&run).unwrap();assert_eq!(actor.data["state"],"cancelled");assert_eq!(snapshot.get("tasks",&task).unwrap().data["status"],"paused");
    let invocation=snapshot.get("tool_invocations",result["invocation_id"].as_str().unwrap()).unwrap();
    let receipt=snapshot.records("activity").into_iter().find(|a|a.data["kind"]=="tool_result"&&a.data["tool_use_id"]=="atomic-pause").unwrap();
    assert_eq!(receipt.data["tool"],"pause_task");assert_eq!(receipt.data["is_error"],false);
    assert_eq!(actor.header.commit_seq,invocation.header.commit_seq);assert_eq!(receipt.header.commit_seq,actor.header.commit_seq);
    let saved:Value=serde_json::from_str(&router.body_text(&receipt.data["detail"]).unwrap()).unwrap();assert_eq!(saved["atomic_invocation"]["id"],invocation.header.id);assert_eq!(saved["atomic_invocation"]["result_ref"],invocation.data["result_ref"]);
    assert!(saved["message"]["content"][0]["content"].as_str().unwrap().contains("task paused:"));
    let audit=snapshot.records("intercept_pending").into_iter().find(|a|a.data["audit"]["tool_use_id"]=="atomic-pause").unwrap();assert_eq!(audit.data["status"],"allowed");assert_eq!(audit.data["audit"]["execution_status"],"completed");assert!(audit.data["audit"]["output"].as_str().unwrap().contains("task paused:"));
    let mut seqs=snapshot.records("domain_events").iter().map(|e|e.data["seq"].as_str().unwrap().parse::<u64>().unwrap()).collect::<Vec<_>>();let count=seqs.len();seqs.sort_unstable();seqs.dedup();assert_eq!(seqs.len(),count,"Joined events must preserve unique sequence ordering");
    assert!(router.atomic_self_pause_receipt_present(&run,&call(&task),&result).unwrap());
    let message=Message{role:"user".into(),content:vec![json!({"type":"tool_result","tool_use_id":"atomic-pause","content":"forged later terminal write"})]};
    assert_eq!(router.append_transcript(&run,"tool_result","forged later terminal write",&message,None).unwrap_err().code,"CANCELLED");assert_eq!(router.snapshot().unwrap().commit_seq,snapshot.commit_seq);
    let original=receipt.clone();router.store.as_mut().unwrap().checkpoint().unwrap();router.store.take();router.store=Some(Store::open(&dir.path().join("store")).unwrap());
    assert_eq!(router.snapshot().unwrap().get("activity",&original.header.id).unwrap(),&original);assert!(router.atomic_self_pause_receipt_present(&run,&call(&task),&result).unwrap());assert!(artex_agent::context_for_run(&router.snapshot().unwrap(),&run,&router.engine_epoch).is_err());
}

#[test]
fn self_pause_atomic_receipt_rejects_failed_foreign_epoch_and_forged_results_without_commit() {
    let (_dir,mut router,run,context,task)=fixture();let before=router.snapshot().unwrap();
    let plan=router.invoke_plan(&params(&run,&task),&context).unwrap();
    let invocation=plan.changes.iter().find_map(|change|match change {artex_core::Change::Put{collection,data,..} if collection=="tool_invocations"=>Some(data.clone()),_=>None}).unwrap();
    let actual=router.tool_result_text(&plan.result["result"]).unwrap();let message=Message{role:"user".into(),content:vec![json!({"type":"tool_result","tool_use_id":"atomic-pause","content":actual,"is_error":false})]};
    for patch in [json!({"state":"failed"}),json!({"run_id":new_id("run")}),json!({"name":"kill_work"}),json!({"input":{"task_id":new_id("task")}})] {
        let mut foreign=invocation.clone();artex_domain::merge(&mut foreign,&patch);
        assert_eq!(router.transcript_batch(&before,&context,&run,"tool_result","invalid atomic receipt",&message,None,Some(&foreign)).unwrap_err().code,"PERMISSION_DENIED");
    }
    let mut stale=context.clone();stale.actor.engine_epoch=new_id("epoch");assert_eq!(router.transcript_batch(&before,&stale,&run,"tool_result","stale receipt",&message,None,Some(&invocation)).unwrap_err().code,"STALE_EPOCH");
    let mut forged=message.clone();forged.content[0]["content"]=json!("fabricated successful result");assert_eq!(router.transcript_batch(&before,&context,&run,"tool_result","forged receipt",&forged,None,Some(&invocation)).unwrap_err().code,"PERMISSION_DENIED");assert_eq!(router.snapshot().unwrap().commit_seq,before.commit_seq);
    let result=router.internal_commit(plan).unwrap();
    let mut failed=result.clone();failed["state"]=json!("failed");assert!(!router.atomic_self_pause_receipt_present(&run,&call(&task),&failed).unwrap());
    let mut foreign=result.clone();foreign["invocation_id"]=json!(new_id("invocation"));assert!(!router.atomic_self_pause_receipt_present(&run,&call(&task),&foreign).unwrap());
    let mut different=call(&task);different.id="foreign-call".into();assert!(!router.atomic_self_pause_receipt_present(&run,&different,&result).unwrap());
    let original_epoch=router.engine_epoch.clone();router.engine_epoch=new_id("epoch");assert!(!router.atomic_self_pause_receipt_present(&run,&call(&task),&result).unwrap());router.engine_epoch=original_epoch;
    let delayed=router.transcript_batch(&before,&context,&run,"tool_result","delayed pre-cancel plan",&message,None,Some(&invocation)).unwrap();let seq=router.snapshot().unwrap().commit_seq;
    assert_eq!(router.internal_commit(delayed).unwrap_err().code,"VERSION_CONFLICT");assert_eq!(router.snapshot().unwrap().commit_seq,seq);
}

#[test]
fn self_pause_stale_cas_cannot_publish_either_cancellation_or_receipt() {
    let (_dir,mut router,run,context,task)=fixture();let plan=router.invoke_plan(&params(&run,&task),&context).unwrap();
    let snapshot=router.snapshot().unwrap();let mut p=Planner::new(&snapshot,"fixture.concurrent_run_change",&json!({}));let mut data=snapshot.get("agent_runs",&run).unwrap().data.clone();data["permission_mode"]=json!("acceptEdits");p.put("agent_runs",&run,data);router.internal_commit(p.done(json!({}))).unwrap();
    let seq=router.snapshot().unwrap().commit_seq;assert_eq!(router.internal_commit(plan).unwrap_err().code,"VERSION_CONFLICT");let state=router.snapshot().unwrap();assert_eq!(state.commit_seq,seq);assert_eq!(state.get("agent_runs",&run).unwrap().data["state"],"running");assert!(state.records("tool_invocations").is_empty());assert!(!state.records("activity").iter().any(|a|a.data["tool_use_id"]=="atomic-pause"));
}

#[test]
fn self_pause_foreign_task_failure_cannot_publish_a_successful_receipt() {
    let (_dir,mut router,run,context,_task)=fixture();let before=router.snapshot().unwrap().commit_seq;
    let error=router.invoke_plan(&params(&run,&new_id("task")),&context).unwrap_err();assert_eq!(error.code,"PERMISSION_DENIED");
    let state=router.snapshot().unwrap();assert_eq!(state.commit_seq,before);assert_eq!(state.get("agent_runs",&run).unwrap().data["state"],"running");assert!(state.records("tool_invocations").is_empty());assert!(!state.records("activity").iter().any(|a|a.data["tool_use_id"]=="atomic-pause"));
}
