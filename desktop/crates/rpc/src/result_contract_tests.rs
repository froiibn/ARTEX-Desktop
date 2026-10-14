use super::*;
use artex_core::body;
use artex_domain::Planner;

#[test]
fn failed_activity_query_identifies_method_and_schema_without_leaking_result_values() {
    let (_dir, mut router, run, _context) = regression::seeded();
    let snapshot = router.snapshot().unwrap();
    let conversation = snapshot.get("agent_runs", &run).unwrap().data["conversation_id"].clone();
    let private_marker = "private-credential-and-result-value";
    let mut batch = Planner::new(&snapshot, "test.invalid_activity", &json!({}));
    batch
        .insert(
            "conversation_activities",
            "conversationactivity",
            json!({
                "conversation_id":conversation,"worker":"mainagent","kind":private_marker,
                "summary":private_marker,"detail":body(private_marker),
            }),
        )
        .unwrap();
    router.internal_commit(batch.done(json!({}))).unwrap();
    let before = router.snapshot().unwrap();
    let method = "ui.conversationHistory";
    let response = router.response(json!({
        "jsonrpc":"2.0","protocol":"artex-rpc/1.0","id":new_id("request"),
        "workspace_id":before.workspace_id,"engine_epoch":router.engine_epoch,
        "command_id":null,"expected":[],
        "method":method,"params":{"id":conversation,"limit":100},"timeout_ms":5000,
    }));
    assert_eq!(response["error"]["data"]["code"], "EXTERNAL_FAILURE");
    assert!(
        response["error"]["message"]
            .as_str()
            .unwrap()
            .contains(method)
    );
    let details = &response["error"]["data"]["details"];
    assert_eq!(details["method"], method);
    assert_eq!(details["phase"], "result_validation");
    let paths = details["schema_paths"].as_array().unwrap();
    assert!(!paths.is_empty() && paths.len() <= 8);
    assert!(paths.iter().any(|p| p.as_str().unwrap().contains("kind")));
    assert!(!response.to_string().contains(private_marker));
    let after = router.snapshot().unwrap();
    assert_eq!(before.commit_seq, after.commit_seq);
    assert_eq!(
        serde_json::to_value(&before.collections).unwrap(),
        serde_json::to_value(&after.collections).unwrap()
    );
}
