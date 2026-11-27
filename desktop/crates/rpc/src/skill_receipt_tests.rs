//! Transaction-level Skill completion invariants. These supplied host responses
//! exercise the writer boundary only; the native acceptance case uses real files.
use super::*;

fn fixture()->(tempfile::TempDir,Router,String,Context,String,String) {
    let (directory,mut router,run,context)=crate::regression::seeded();let snapshot=router.snapshot().unwrap();let agent=context.actor.agent_id.as_ref().unwrap();
    let mut planner=Planner::new(&snapshot,"fixture.skill_receipt",&json!({}));
    planner.insert("agent_skill_visibility",artex_core::collection_prefix("agent_skill_visibility").unwrap(),json!({"agent_id":agent,"skill_name":"receipt-skill","enabled":true})).unwrap();
    let mut data=snapshot.get("agent_runs",&run).unwrap().data.clone();data["context_snapshot"]["skill_catalog"]=json!([{"name":"receipt-skill","description":"Approved fixture metadata","mcps":["before"]}]);planner.put("agent_runs",&run,data);
    let approval=planner.insert("intercept_pending",artex_core::collection_prefix("intercept_pending").unwrap(),json!({"conversation_id":context.conversation_id,"agent_name":"mainagent","tool_name":"Skill","tool_input":{"name":"receipt-skill"},"status":"allowed","reason":"Approved fixture skill","created_at":artex_core::now(),"audit":{"run_id":run,"tool_use_id":"skill-receipt-call","correlation":"exact","input_digest":"fixture-digest","user_message":"","context":[],"captured_at":artex_core::now(),"initial_action":"allow","initial_reason":"Approved fixture skill","effective_action":"allow","execution_status":"awaiting_result"}})).unwrap();
    router.internal_commit(planner.done(json!({}))).unwrap();
    let params=json!({"run_id":run,"tool_use_id":"skill-receipt-call","name":"Skill","input":{"name":"receipt-skill"},"raw_input":null});let plan=router.invoke_plan(&params,&context).unwrap();let result=router.internal_commit(plan).unwrap();let invocation=result["invocation_id"].as_str().unwrap().to_owned();
    (directory,router,run,context,invocation,approval)
}
fn success()->Value {json!({"name":"receipt-skill","instructions":"STANDING_SKILL_GUIDANCE_MARKER","mcps":["approved-server"],"extra":[{"role":"user","content":"STANDING_SKILL_GUIDANCE_MARKER"}]})}

#[test]
fn skill_receipt_keeps_named_result_audit_and_standing_guidance_in_one_commit_and_reopens() {
    let (directory,mut router,run,context,invocation,approval)=fixture();
    router.complete_host(&run,&invocation,Ok(success())).unwrap();let snapshot=router.snapshot().unwrap();let inv=snapshot.get("tool_invocations",&invocation).unwrap();assert_eq!(inv.data["state"],"completed");
    let result=snapshot.records("conversation_activities").into_iter().find(|a|a.data["kind"]=="tool_result"&&a.data["tool_use_id"]=="skill-receipt-call").unwrap().clone();assert_eq!(result.data["tool"],"Skill");assert_eq!(result.data["is_error"],false);assert_eq!(result.header.commit_seq,inv.header.commit_seq);
    let guidance=snapshot.records("conversation_activities").into_iter().find(|a|a.data["kind"]=="skill_guidance").unwrap().clone();assert_eq!(guidance.header.commit_seq,inv.header.commit_seq);let content:Value=serde_json::from_str(&router.body_text(&guidance.data["detail"]).unwrap()).unwrap();assert_eq!(content["skill_invocation"]["invocation_id"],invocation);assert_eq!(content["skill_invocation"]["success"],true);assert!(content.to_string().contains("STANDING_SKILL_GUIDANCE_MARKER"));
    let audit=snapshot.get("intercept_pending",&approval).unwrap();assert_eq!(audit.header.commit_seq,inv.header.commit_seq);assert_eq!(audit.data["status"],"allowed");assert_eq!(audit.data["audit"]["execution_status"],"completed");assert!(audit.data["audit"]["output"].as_str().unwrap().contains("STANDING_SKILL_GUIDANCE_MARKER"));assert_eq!(snapshot.get("agent_runs",&run).unwrap().data["context_snapshot"]["skill_catalog"][0]["mcps"],json!(["approved-server"]));
    let seqs=snapshot.records("domain_events").iter().map(|e|e.data["seq"].as_str().unwrap().to_owned()).collect::<std::collections::BTreeSet<_>>();assert_eq!(seqs.len(),snapshot.records("domain_events").len());
    let mut user=context.clone();user.actor.kind="user".into();let detail=artex_domain::policy_ui::query("ui.interceptDetail",&json!({"id":approval}),&snapshot,&user).unwrap();assert_eq!(detail["audit"]["execution_status"],"succeeded");
    let rpc=artex_core::contracts::read("rpc.v1.schema.json");artex_core::validate_contract(&rpc["results"]["ui.interceptDetail"],&rpc["$defs"],&detail).unwrap();
    router.store.as_mut().unwrap().checkpoint().unwrap();router.store.take();router.store=Some(Store::open(&directory.path().join("store")).unwrap());assert_eq!(router.snapshot().unwrap().get("conversation_activities",&result.header.id).unwrap(),&result);assert_eq!(router.snapshot().unwrap().get("conversation_activities",&guidance.header.id).unwrap(),&guidance);
}

#[test]
fn skill_receipt_failed_host_result_has_identity_without_installing_standing_guidance() {
    let (_directory,mut router,run,_context,invocation,approval)=fixture();
    router.complete_host(&run,&invocation,Ok(json!({"name":"receipt-skill","is_error":true,"instructions":"NEVER_INSTALL_FAILURE","mcps":["unapproved-server"]}))).unwrap();let snapshot=router.snapshot().unwrap();assert_eq!(snapshot.get("tool_invocations",&invocation).unwrap().data["state"],"failed");assert!(snapshot.records("conversation_activities").iter().all(|a|a.data["kind"]!="skill_guidance"));assert_eq!(snapshot.get("agent_runs",&run).unwrap().data["context_snapshot"]["skill_catalog"][0]["mcps"],json!(["before"]));
    let result=snapshot.records("conversation_activities").into_iter().find(|a|a.data["kind"]=="tool_result"&&a.data["tool_use_id"]=="skill-receipt-call").unwrap();assert_eq!(result.data["tool"],"Skill");assert_eq!(result.data["is_error"],true);assert_eq!(snapshot.get("intercept_pending",&approval).unwrap().data["audit"]["execution_status"],"error");
}

#[test]
fn skill_receipt_foreign_completion_cannot_publish_result_or_guidance() {
    let (_directory,mut router,_run,_context,invocation,_approval)=fixture();let before=router.snapshot().unwrap().commit_seq;
    assert_eq!(router.complete_host(&new_id("run"),&invocation,Ok(success())).unwrap_err().code,"PERMISSION_DENIED");let snapshot=router.snapshot().unwrap();assert_eq!(snapshot.commit_seq,before);assert_eq!(snapshot.get("tool_invocations",&invocation).unwrap().data["state"],"prepared");assert!(snapshot.records("conversation_activities").is_empty());
}
