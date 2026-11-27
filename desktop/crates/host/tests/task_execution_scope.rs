//! SDK `task_id` is an execution handle. Admission must reach the host, whose
//! run/epoch boundary still owns status, stop-one, and stop-all operations.
//! Windows confirms descendant cleanup; Unix retains its explicit unconfirmed
//! descendant status after the leader exits rather than claiming that proof.
use artex_core::{Actor,Context,new_id};
use artex_host::HostRuntime;
use serde_json::{Value,json};
use std::time::{Duration,Instant};

#[test]
fn sdk_task_output_and_stop_enforce_real_execution_run_and_epoch_ownership() {
    let directory=tempfile::tempdir().unwrap();let host=HostRuntime::default();let cap=host.capabilities.grant_directory(directory.path()).unwrap();
    let owner=Context {actor:Actor{kind:"agent".into(),agent_id:Some(new_id("agent")),run_id:Some(new_id("run")),engine_epoch:new_id("epoch")},task_id:Some(new_id("task")),intent_id:None,conversation_id:None};
    let mut other=owner.clone();other.actor.run_id=Some(new_id("run"));other.task_id=Some(new_id("task"));
    let start=|context:&Context,marker:&str|host.execute_tool("Monitor",&json!({"directory_capability":cap,"command":format!("python -u -c \"import time; print('{marker}', flush=True); time.sleep(30)\""),"description":marker}),context).unwrap()["execution_id"].as_str().unwrap().to_owned();
    let own_id=start(&owner,"OWNED_SDK_EXECUTION_MARKER");let other_id=start(&other,"OTHER_SDK_EXECUTION_MARKER");
    let mut stale=owner.clone();stale.actor.engine_epoch=new_id("epoch");
    for foreign in [&other,&stale] {
        for name in ["TaskOutput","TaskStop"] {assert_eq!(host.execute_tool(name,&json!({"task_id":own_id}),foreign).unwrap_err().code,"PERMISSION_DENIED","{name} may not access a different run or epoch");}
        assert_eq!(host.execute_tool("TaskOutput",&json!({"task_id":own_id}),&owner).unwrap()["state"],"running","A foreign stop attempt must leave the owned process alive");
    }
    let output=host.execute_tool("TaskOutput",&json!({"task_id":own_id,"block":true,"timeout_ms":1000}),&owner).unwrap();assert!(output["output"].as_str().unwrap().contains("OWNED_SDK_EXECUTION_MARKER"));
    assert_eq!(host.execute_tool("TaskStop",&json!({"task_id":""}),&owner).unwrap()["accepted"],true);
    let deadline=Instant::now()+Duration::from_secs(5);loop {
        let status=host.execute_tool("TaskOutput",&json!({"task_id":own_id}),&owner).unwrap();
        if status["state"]!="running"&&status["leader_exited"]==true&&status["output_drained"]==true {assert_eq!(status["state"],"cancelled");assert_eq!(status["cleanup_confirmed"],cfg!(windows));if !cfg!(windows) {assert_eq!(status["cleanup_state"],"unconfirmed");}break;}
        assert!(Instant::now()<deadline,"Owned stop-all cleanup did not settle: {status}");std::thread::sleep(Duration::from_millis(20));
    }
    assert_eq!(host.execute_tool("TaskOutput",&json!({"task_id":other_id}),&other).unwrap()["state"],"running","Stop-all is scoped to its caller's run");
    let stopped:Value=host.execute_tool("TaskStop",&json!({"task_id":other_id}),&other).unwrap();assert_eq!(stopped["stop_requested"],true);
    let deadline=Instant::now()+Duration::from_secs(5);loop {
        let status=host.execute_tool("TaskOutput",&json!({"task_id":other_id}),&other).unwrap();
        if status["state"]!="running"&&status["leader_exited"]==true&&status["output_drained"]==true {assert_eq!(status["state"],"cancelled");assert_eq!(status["cleanup_confirmed"],cfg!(windows));if !cfg!(windows) {assert_eq!(status["cleanup_state"],"unconfirmed");}break;}
        assert!(Instant::now()<deadline,"Other owner cleanup did not settle: {status}");std::thread::sleep(Duration::from_millis(20));
    }
}
