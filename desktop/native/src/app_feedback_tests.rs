//! Actual create-button -> Prepare -> Service -> durable conversation -> reopen.
use super::*;

#[test]
fn create_conversation_button_preserves_validation_and_separates_previous_name_error() {
    let mut f = Fixture::new();
    // Obtain a real name validation failure through the same public pipeline.
    let params = json!({"key":"feedback_fixture_agent","name":""});
    let scope = f.app.handle.scope().unwrap();
    f.app.command(
        "ui.createAgent".into(),
        params.clone(),
        vec![],
        Purpose::Command {
            method: "ui.createAgent".into(),
            params,
            section: Section::Agents,
            selected: None,
            scope,
        },
    );
    f.until(|app| app.error.is_some() && app.pending.is_empty());
    let error = f.app.error.as_ref().unwrap();
    assert_eq!(error.code, "INVALID_PARAMS");
    assert_eq!(error.message, "name is required");
    assert_eq!(error.details["native_operation"], "ui.createAgent");
    assert!(
        f.app
            .handle
            .view()
            .unwrap()
            .generation
            .records("conversations")
            .is_empty()
    );
    for title in ["", "   ", "첫 대화 버튼 회귀"] {
        select(&mut f, Section::Chat, None);
        f.app.drawer.open = false;
        f.app.panes.title = title.into();
        f.click_visible("대화 만들기");
        let (_, _, _, purpose) = f.prepare("ui.createConversation");
        assert!(
            matches!(purpose,Purpose::Command { method,.. } if method=="ui.createConversation")
        );
        assert!(
            f.app.error.is_none(),
            "An earlier name error must not be attributed to the pending conversation"
        );
        f.until(|app| {
            app.selected
                .get(&Section::Chat)
                .and_then(|s| s.as_ref())
                .is_some()
                && app.pending.is_empty()
        });
        assert!(f.app.error.is_none(), "{:?}", f.app.error);
        let id = f.app.selected[&Section::Chat].as_ref().unwrap();
        let view = f.app.handle.view().unwrap();
        let row = view.generation.get("conversations", id).unwrap();
        assert_eq!(
            row.data["title"],
            if title.trim().is_empty() {
                "new conversation"
            } else {
                title
            }
        );
        assert!(
            view.generation.records("agent_runs").is_empty(),
            "Create must not run a model"
        );
    }
    assert!(
        f.app
            .notices
            .iter()
            .any(|n| n.contains("이전 오류 기록") && n.contains("name is required")),
        "Earlier validation failure must be retained in Activity"
    );
    f.reopen_same_workspace();
    assert_eq!(
        f.app
            .handle
            .view()
            .unwrap()
            .generation
            .records("conversations")
            .len(),
        3
    );
    let id = f
        .app
        .handle
        .view()
        .unwrap()
        .generation
        .records("conversations")
        .into_iter()
        .find(|row| row.data["title"] == "첫 대화 버튼 회귀")
        .unwrap()
        .header
        .id
        .clone();
    select(&mut f, Section::Chat, Some(&id));
    let output = f.frame(vec![]);
    assert!(
        Fixture::texts(&output)
            .iter()
            .any(|text| text == "첫 대화 버튼 회귀")
    );
    assert!(
        Fixture::texts(&output)
            .iter()
            .any(|text| text == "응답 대기")
    );
    f.shutdown();
}

#[test]
fn actual_composer_displays_running_completed_failed_and_local_notice_without_reopen_replay() {
    use super::transport::{LocalProvider, Probe, Step, factory};
    use std::sync::atomic::{AtomicBool, Ordering};
    let released = Arc::new(AtomicBool::new(false));
    let release = released.clone();
    let provider = LocalProvider::from_handler(move |index, _| {
        if index == 0 {
            let start = Instant::now();
            while !release.load(Ordering::Acquire) {
                assert!(
                    start.elapsed() < Duration::from_secs(20),
                    "Fixture reply release missing"
                );
                std::thread::sleep(Duration::from_millis(5));
            }
            Step::Reply(200,"application/json",json!({"id":"feedback-reply","choices":[{"message":{"role":"assistant","content":"FEEDBACK_SAVED_RESPONSE"},"finish_reason":"stop"}],"usage":{"prompt_tokens":3,"completion_tokens":2}}).to_string())
        } else {
            Step::Reply(
                500,
                "application/json",
                json!({"error":{"message":"Local fixture failure"}}).to_string(),
            )
        }
    });
    let secret = artex_core::new_id("secret");
    let mut f = Fixture::new_with_config(|profile| ServiceConfig {
        host_factory: Some(factory(
            profile,
            BTreeMap::from([(secret.clone(), "isolated-memory-placeholder".into())]),
            Probe::default(),
        )),
        wake_ui: None,
    });
    let profile=f.setup("ui.saveLLMProfile",json!({"p":{"name":"Feedback loopback","format":"openai","model":"fixture","base_url":provider.url,"streaming":false,"api_key":{"secret_ref":secret,"hint":"memory fixture"},"retry":{"connect":{"attempts":-1,"interval_ms":0},"empty":{"attempts":-1,"interval_ms":0},"stream":{"attempts":-1,"interval_ms":0}}}}))["id"].as_str().unwrap().to_owned();
    select(&mut f, Section::Chat, None);
    f.app.panes.title = "Feedback actual response".into();
    f.app.panes.profile_id = Some(profile);
    f.click_visible("대화 만들기");
    f.until(|app| {
        app.selected
            .get(&Section::Chat)
            .and_then(|id| id.as_ref())
            .is_some()
            && app.pending.is_empty()
    });
    let cid = f.app.selected[&Section::Chat].clone().unwrap();
    f.type_composer_and_send(&cid, "Respond with the isolated fixture only.");
    f.until(|_| !provider.requests().is_empty());
    let out = f.frame(vec![]);
    assert!(
        Fixture::texts(&out).iter().any(|t| t == "응답 실행 중"),
        "Running response must be visible before fixture release"
    );
    released.store(true, Ordering::Release);
    f.wait_conversation_terminal(&cid, "completed");
    f.until(|app| {
        app.analysis_notice
            .as_ref()
            .is_some_and(|n| n.contains("응답 완료"))
    });
    f.assert_main_visible(&cid, "FEEDBACK_SAVED_RESPONSE");
    let out = f.frame(vec![]);
    assert!(Fixture::texts(&out).iter().any(|t| t == "응답 완료"));
    assert!(
        Fixture::texts(&out)
            .iter()
            .any(|t| t.contains("Feedback actual response: 응답 완료")),
        "Actual local notification banner missing"
    );
    f.type_composer_and_send(&cid, "Return the next isolated failure.");
    f.wait_conversation_terminal(&cid, "failed");
    f.until(|app| {
        app.analysis_notice
            .as_ref()
            .is_some_and(|n| n.contains("응답 실패"))
    });
    let out = f.frame(vec![]);
    assert!(Fixture::texts(&out).iter().any(|t| t == "응답 실패"));
    assert_eq!(provider.requests().len(), 2);
    f.reopen_same_workspace();
    select(&mut f, Section::Chat, Some(&cid));
    let out = f.frame(vec![]);
    assert!(
        f.app.analysis_notice.is_none(),
        "Opening history must not replay completion notifications"
    );
    assert!(Fixture::texts(&out).iter().any(|t| t == "응답 실패"));
    assert!(f.app.error.is_none(), "{:?}", f.app.error);
    f.shutdown();
    assert_eq!(provider.finish().len(), 2);
}
