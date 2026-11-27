//! Connected core acceptance with actual egui input and the real SQLite service.
//! Picker paths and loopback credentials are isolated fixture inputs; no user
//! window, OS vault, external provider or operating website is manipulated.
use super::transport::*;
use super::*;

fn pointer(f: &mut Fixture, pos: egui::Pos2, button: egui::PointerButton) {
    f.frame(vec![egui::Event::PointerMoved(pos), egui::Event::PointerButton {
        pos, button, pressed: true, modifiers: Default::default(),
    }]);
    f.frame(vec![egui::Event::PointerButton {
        pos, button, pressed: false, modifiers: Default::default(),
    }]);
}

fn replace_visible_text(f: &mut Fixture, old: &str, new: &str) {
    let out = f.frame(vec![]);
    let pos = out.shapes.iter().rev().find_map(|clipped| {
        let egui::epaint::Shape::Text(text) = &clipped.shape else { return None; };
        let rect = text.visual_bounding_rect();
        (text.galley.text() == old && clipped.clip_rect.contains_rect(rect)).then(|| rect.center())
    }).unwrap_or_else(|| panic!("Editable text is not visible: {old}"));
    pointer(f, pos, egui::PointerButton::Primary);
    // Keyboard events edit the production TextEdit. No editor draft is patched.
    f.frame(vec![egui::Event::Key {
        key: egui::Key::A, physical_key: None, pressed: true, repeat: false,
        modifiers: egui::Modifiers { ctrl: true, command: true, ..Default::default() },
    }, egui::Event::Text(new.into())]);
}

fn settled(f: &mut Fixture) {
    f.until(|app| app.pending.is_empty());
    assert!(f.app.error.is_none(), "{:?}", f.app.error);
}

fn click_dialog_button(f: &mut Fixture, label: &str) {
    // egui measures a new Window before its first paint. Its title may also
    // equal its confirmation button; target the visible lower control, not
    // title-bar text, and never substitute a direct management invocation.
    for _ in 0..8 {
        let out = f.frame(vec![]);
        let pos = out.shapes.iter().filter_map(|clipped| {
            let egui::epaint::Shape::Text(text) = &clipped.shape else { return None; };
            let rect = text.visual_bounding_rect();
            (text.galley.text() == label && clipped.clip_rect.contains_rect(rect)).then(|| rect.center())
        }).max_by(|a,b| a.y.total_cmp(&b.y));
        if let Some(pos) = pos { pointer(f, pos, egui::PointerButton::Primary); return; }
    }
    panic!("Dialog control was not painted: {label}");
}

fn connected_conversation(f: &mut Fixture, title: &str) -> String {
    select(f, Section::Chat, None);
    f.app.panes.title = title.into();
    f.click_visible("대화 만들기");
    f.prepare("ui.createConversation");
    settled(f);
    f.app.selected[&Section::Chat].clone().unwrap()
}

fn switch(f: &mut Fixture, options: OpenOptions) {
    let old = f.app.handle.scope().unwrap();
    // This is the native file-picker result boundary. The confirmation, close,
    // host retirement and next Open all run through the real application UI.
    f.app.incoming_workspace = Some(options.clone());
    click_dialog_button(f, "현재 공간 유지");
    assert_eq!(f.app.handle.scope().as_ref(), Some(&old));
    assert!(f.app.incoming_workspace.is_none());
    f.app.incoming_workspace = Some(options.clone());
    click_dialog_button(f, "저장 후 전환");
    f.until(|app| app.handle.status().phase == ServicePhase::Ready
        && app.pending.is_empty()
        && app.handle.scope().is_some_and(|s| s.open_generation != old.open_generation));
    f.path = options.path;
    assert!(f.app.error.is_none(), "{:?}", f.app.error);
}

#[test]
fn actual_core_model_edit_activate_switch_and_chat_reopen() {
    let provider = LocalProvider::new(vec![
        Step::Reply(200, "application/json", anthropic("CORE_WORKSPACE_A", false)),
        Step::Reply(200, "application/json", anthropic("CORE_WORKSPACE_B", false)),
    ]);
    let secret = artex_core::new_id("secret");
    let mut f = Fixture::configured(|profile| ServiceConfig {
        host_factory: Some(factory(profile, BTreeMap::from([(secret.clone(), "isolated-core-value".into())]), Probe::default())),
        wake_ui: None,
    });
    let id = provider::profile(&mut f, "CORE_MODEL_BEFORE", "anthropic", &provider.url, &secret, false);
    select(&mut f, Section::Models, Some(&id));
    let before = f.app.handle.view().unwrap().generation.get("llm_profiles", &id).unwrap().clone();
    replace_visible_text(&mut f, "CORE_MODEL_BEFORE", "CORE_MODEL_SAVED");
    f.click_visible("모델 설정 저장");
    let (expected, _, _, _) = f.prepare("ui.saveLLMProfile");
    assert!(expected.contains(&crate::model::expected(&before)));
    settled(&mut f);
    assert_eq!(f.app.handle.view().unwrap().generation.get("llm_profiles", &id).unwrap().data["name"], "CORE_MODEL_SAVED");
    f.click_visible("기본 모델로 설정");
    f.prepare("ui.activateLLMProfile");
    settled(&mut f);
    assert_eq!(f.app.handle.view().unwrap().generation.get("llm_profiles", &id).unwrap().data["is_default"], true);
    assert!(Fixture::texts(&f.frame(vec![])).iter().any(|s| s.contains("CORE_MODEL_SAVED")));
    let a = connected_conversation(&mut f, "Core first workspace");
    f.type_composer_and_send(&a, "Local core A");
    f.wait_conversation_terminal(&a, "completed");
    f.assert_main_visible(&a, "CORE_WORKSPACE_A");
    let second = f._dir.path().join("second");
    switch(&mut f, OpenOptions { path: second, mode: OpenMode::Create { name: "Core second".into() }, read_only: false });
    assert!(f.app.handle.view().unwrap().generation.get("conversations", &a).is_none());
    let saved = f.app.handle.view().unwrap().generation.get("llm_profiles", &id).unwrap().data.clone();
    assert_eq!(saved["name"], "CORE_MODEL_SAVED");
    assert_eq!(saved["is_default"], true);
    let b = connected_conversation(&mut f, "Core common model");
    f.type_composer_and_send(&b, "Local core B");
    f.wait_conversation_terminal(&b, "completed");
    f.assert_main_visible(&b, "CORE_WORKSPACE_B");
    f.reopen_same_workspace();
    f.assert_main_visible(&b, "CORE_WORKSPACE_B");
    assert_eq!(f.app.handle.view().unwrap().generation.get("llm_profiles", &id).unwrap().data, saved);
    select(&mut f, Section::Models, Some(&id));
    assert!(Fixture::texts(&f.frame(vec![])).iter().any(|s| s.contains("CORE_MODEL_SAVED")));
    f.shutdown();
    assert_eq!(provider.finish().len(), 2);
    println!("CORE_ACCEPT model Save/Activate -> confirmed workspace switch -> chat -> SQLite reopen/display");
}

#[test]
fn actual_core_task_resume_stop_reopen_delete_preserves_other_task() {
    let provider = LocalProvider::from_handler(|_, _| Step::Hold);
    let secret = artex_core::new_id("secret");
    let mut f = Fixture::configured(|profile| ServiceConfig {
        host_factory: Some(factory(profile, BTreeMap::from([(secret.clone(), "isolated-task-value".into())]), Probe::default())),
        wake_ui: None,
    });
    let profile = provider::profile(&mut f, "Core task model", "anthropic", &provider.url, &secret, false);
    f.setup("ui.activateLLMProfile", json!({"id":profile}));
    let other = source_task(&mut f, "CORE_TASK_KEEP");
    let id = source_task(&mut f, "CORE_TASK_DELETE");
    select(&mut f, Section::Tasks, Some(&id));
    let count = provider.requests().len();
    f.click_visible("재개");
    f.prepare("ui.controlTask");
    f.until(|_| provider.requests().len() > count);
    f.click_visible("일시정지");
    f.prepare("ui.controlTask");
    settled(&mut f);
    assert_eq!(f.app.handle.view().unwrap().generation.get("tasks", &id).unwrap().data["paused"], true);
    f.click_visible("작업 중지·정리");
    settled(&mut f);
    let view = f.app.handle.view().unwrap();
    assert_eq!(view.generation.get("tasks", &id).unwrap().data["paused"], true);
    assert!(!view.generation.records("agent_runs").iter().any(|r| r.data["task_id"] == id
        && matches!(r.data["state"].as_str(), Some("queued" | "running" | "waiting_model" | "waiting_tool"))));
    let count = provider.requests().len();
    let path = f.path.clone();
    switch(&mut f, OpenOptions { path, mode: OpenMode::Existing, read_only: false });
    select(&mut f, Section::Tasks, Some(&id));
    for _ in 0..4 { f.frame(vec![]); }
    assert_eq!(provider.requests().len(), count, "Opening a stopped task must not request a model");
    assert_eq!(f.app.handle.view().unwrap().generation.get("tasks", &id).unwrap().data["paused"], true);
    let out = f.frame(vec![]);
    let pos = out.shapes.iter().find_map(|s| {
        let egui::epaint::Shape::Text(t) = &s.shape else { return None; };
        (t.galley.text().contains("CORE_TASK_DELETE") && t.visual_bounding_rect().left() < f.ctx.content_rect().center().x)
            .then(|| t.visual_bounding_rect().center())
    }).expect("Saved task list row must be visible");
    pointer(&mut f, pos, egui::PointerButton::Secondary);
    f.click_visible("작업 삭제…");
    settled(&mut f);
    f.click_visible("표시한 범위와 자료 영향을 확인했습니다.");
    click_dialog_button(&mut f, "작업 삭제");
    assert!(f.app.pending.values().any(|p| matches!(p, Purpose::NativeOperation { mutation: true, .. })),
        "Delete must submit its real mutation, not just paint a window: {:?}", Fixture::texts(&f.frame(vec![])));
    settled(&mut f);
    assert!(f.app.handle.view().unwrap().generation.get("tasks", &id).is_none());
    assert!(f.app.handle.view().unwrap().generation.get("tasks", &other).is_some());
    f.reopen_same_workspace();
    let view = f.app.handle.view().unwrap();
    assert!(view.generation.get("tasks", &id).is_none());
    assert!(view.generation.get("tasks", &other).is_some());
    select(&mut f, Section::Tasks, Some(&other));
    assert!(Fixture::texts(&f.frame(vec![])).iter().any(|s| s.contains("CORE_TASK_KEEP")));
    f.shutdown();
    provider.finish();
    println!("CORE_ACCEPT resume -> stop/cleanup -> reopen without replay -> actual delete -> reopen/display");
}

#[test]
fn actual_core_conversation_question_can_be_answered_without_leaving_chat() {
    let questions = json!({"questions":[{"question":"CORE_QUESTION_SCOPE?","options":[{"label":"Local only","description":"Use the isolated fixture"},{"label":"Cancel","description":"Do not continue"}]}]});
    let call = json!({"id":"core-question","type":"function","function":{"name":"AskUserQuestion","arguments":questions.to_string()}});
    let provider = LocalProvider::new(vec![
        Step::Reply(200,"application/json",json!({"choices":[{"message":{"content":null,"tool_calls":[call]},"finish_reason":"tool_calls"}],"usage":{"prompt_tokens":4,"completion_tokens":2}}).to_string()),
        Step::Reply(200,"application/json",json!({"choices":[{"message":{"content":"CORE_ANSWER_SAVED"},"finish_reason":"stop"}],"usage":{"prompt_tokens":6,"completion_tokens":2}}).to_string()),
    ]);
    let secret = artex_core::new_id("secret");
    let mut f = Fixture::configured(|profile| ServiceConfig {
        host_factory: Some(factory(profile, BTreeMap::from([(secret.clone(), "isolated-question-value".into())]), Probe::default())),
        wake_ui: None,
    });
    let id = provider::profile(&mut f, "Core question model", "openai", &provider.url, &secret, false);
    f.setup("ui.activateLLMProfile", json!({"id":id}));
    let agent = f.app.handle.view().unwrap().generation.records("agents").into_iter()
        .find(|a| a.data["key"] == "auto").unwrap().header.id.clone();
    // Questions are deliberately opt-in. Configure the supported public flag
    // for this isolated agent; never weaken default tool offer/permission gates.
    f.setup("agent.features.set", json!({"agent_id":agent,"questions":true,"plan_mode":false,"memory":false}));
    let tool = f.app.handle.view().unwrap().generation.records("tools").into_iter()
        .find(|t| t.data["key"] == "AskUserQuestion").unwrap().data.clone();
    f.setup("ui.saveTool", json!({"key":"AskUserQuestion","patch":{"enabled":true,
        "description":tool["description"],"schema":tool["schema"],"agents":tool["agents"]}}));
    let cid = connected_conversation(&mut f, "Core question conversation");
    f.type_composer_and_send(&cid, "Ask the local fixture question before continuing.");
    f.until(|_| !provider.requests().is_empty());
    assert!(provider.requests()[0]["tools"].as_array().unwrap().iter().any(|t| t["function"]["name"] == "AskUserQuestion"),
        "Only a genuinely offered, explicitly enabled tool is an acceptance input");
    f.until(|app| {
        let view = app.handle.view().unwrap();
        assert!(!view.generation.records("agent_runs").iter().any(|r| r.data["conversation_id"] == cid
            && matches!(r.data["state"].as_str(), Some("completed" | "failed"))),
            "Question run terminated before its user question: runs={:?}; features={:?}; activities={:?}",
            view.generation.records("agent_runs").iter().map(|r| &r.data).collect::<Vec<_>>(),
            view.generation.records("agent_features").iter().map(|r| &r.data).collect::<Vec<_>>(),
            view.generation.records("conversation_activities").iter().map(|r| &r.data).collect::<Vec<_>>());
        view.generation.records("user_questions").iter().any(|q| q.data["state"] == "pending")
    });
    let out = f.frame(vec![]);
    let visible = visible_texts(&out, egui::Rect::EVERYTHING);
    assert!(visible.iter().any(|s| s.contains("CORE_QUESTION_SCOPE?")),
        "An actual pending conversation question needs answer controls in Chat: {visible:?}");
    let run = f.app.handle.view().unwrap().generation.records("agent_runs").into_iter()
        .find(|r| r.data["conversation_id"] == cid).unwrap().header.id.clone();
    let other = connected_conversation(&mut f, "Unrelated question scope");
    assert!(!visible_texts(&f.frame(vec![]), egui::Rect::EVERYTHING).iter().any(|s| s.contains("CORE_QUESTION_SCOPE?")),
        "A pending question must never appear in another conversation");
    select(&mut f, Section::Sessions, Some(&run));
    assert!(visible_texts(&f.frame(vec![]), egui::Rect::EVERYTHING).iter().any(|s| s.contains("CORE_QUESTION_SCOPE?")),
        "The selected conversation run must expose its own question too");
    select(&mut f, Section::Chat, Some(&cid));
    f.click_visible("Local only — Use the isolated fixture");
    f.click_visible("답변 제출");
    f.prepare("user.answer");
    f.wait_conversation_terminal(&cid, "completed");
    f.assert_main_visible(&cid, "CORE_ANSWER_SAVED");
    let answer = f.app.handle.view().unwrap().generation.records("user_questions").into_iter().next().unwrap().data.clone();
    assert_eq!(answer["state"], "answered");
    assert_eq!(answer["answer"]["answers"]["CORE_QUESTION_SCOPE?"], "Local only");
    f.reopen_same_workspace();
    f.assert_main_visible(&cid, "CORE_ANSWER_SAVED");
    assert_eq!(f.app.handle.view().unwrap().generation.records("user_questions")[0].data, answer);
    assert!(f.app.handle.view().unwrap().generation.get("conversations", &other).is_some());
    f.shutdown();
    let requests = provider.finish();
    assert_eq!(requests.len(), 2);
    assert!(requests[1].to_string().contains("Local only"));
    println!("CORE_ACCEPT actual question offer -> answer widgets -> owned user.answer -> model -> SQLite reopen/display");
}

fn model_tool(name: &str, input: Value) -> Step {
    Step::Reply(200, "application/json", json!({"choices":[{"message":{"content":null,
        "tool_calls":[{"id":format!("core-{name}"),"type":"function","function":{"name":name,"arguments":input.to_string()}}]},
        "finish_reason":"tool_calls"}],"usage":{"prompt_tokens":4,"completion_tokens":2}}).to_string())
}

fn model_answer(text: &str) -> Step {
    Step::Reply(200,"application/json",json!({"choices":[{"message":{"content":text},"finish_reason":"stop"}],
        "usage":{"prompt_tokens":6,"completion_tokens":2}}).to_string())
}

fn bypass_value(f: &Fixture) -> bool {
    f.app.handle.view().unwrap().generation.records("settings").iter()
        .any(|r| r.data["key"] == "approval_bypass" && (r.data["value"] == true || r.data["value"] == "true"))
}

#[test]
fn actual_core_write_approval_and_bypass_deny_keep_effects_scoped_and_reopen() {
    use std::sync::Mutex;
    let provider = LocalProvider::new(vec![
        model_tool("Write", json!({"file_path":"core-write.txt","content":"CORE_WRITE_CONTENT"})),
        model_tool("Read", json!({"file_path":"core-write.txt"})),
        model_answer("CORE_APPROVED_WRITE_SAVED"),
        model_tool("Write", json!({"file_path":"core-denied.txt","content":"MUST_NOT_EXIST"})),
        model_answer("CORE_DENY_CONFIRMED"),
    ]);
    let secret = artex_core::new_id("secret");
    let probe = Probe::default();
    let work = Arc::new(Mutex::new(None::<PathBuf>));
    let opened = work.clone();
    let mut f = Fixture::configured(|profile| {
        let base = factory(profile, BTreeMap::from([(secret.clone(), "isolated-write-value".into())]), probe.clone());
        ServiceConfig { host_factory: Some(Arc::new(move |workspace| {
            *opened.lock().unwrap() = Some(workspace.work_root.clone());
            base(workspace)
        })), wake_ui: None }
    });
    let profile = provider::profile(&mut f, "Core write model", "openai", &provider.url, &secret, false);
    f.setup("ui.activateLLMProfile", json!({"id":profile}));
    let cid = connected_conversation(&mut f, "Core approved file");
    let file = work.lock().unwrap().as_ref().unwrap().join("conversations").join(&cid).join("core-write.txt");
    f.type_composer_and_send(&cid, "Write and verify only the isolated relative file.");
    f.until(|app| app.handle.view().unwrap().generation.records("user_questions").iter()
        .any(|q| q.data["state"] == "pending" && q.data["payload"]["type"] == "tool_approval"));
    assert!(!file.exists());
    assert!(probe.attempts.lock().unwrap().is_empty(), "Unapproved file effects must never reach HostBridge");
    f.click_visible("승인");
    f.prepare("user.answer");
    f.wait_conversation_terminal(&cid, "completed");
    assert_eq!(std::fs::read_to_string(&file).unwrap(), "CORE_WRITE_CONTENT");
    assert_eq!(probe.attempts.lock().unwrap().iter().filter(|r| r["input"]["name"] == "Write").count(), 1);
    f.assert_main_visible(&cid, "CORE_APPROVED_WRITE_SAVED");
    f.assert_tool_result_visible(&cid, "Read · 실행 결과", "CORE_WRITE_CONTENT");
    f.click_visible("Bypass 꺼짐");
    f.frame(vec![]);
    f.click_visible("Bypass 켜기");
    f.prepare("ui.setSettings");
    settled(&mut f);
    assert!(bypass_value(&f));
    // Explicit denials precede bypass. Configuration uses the same registered
    // user commands as the editor; only the execution/answer UI is qualified.
    f.setup("ui.interceptSetToolConfig", json!({"enabledTools":["Write"]}));
    f.setup("ui.createInterceptRule", json!({"rule":{"name":"Core explicit deny","enabled":true,
        "priority":100,"match_target":"tool_name","match_type":"string","pattern":"Write",
        "action":"deny","message":"CORE_EXPLICIT_DENY","timeout_enabled":false,"timeout_seconds":0,"timeout_action":"deny"}}));
    let denied = connected_conversation(&mut f, "Core denied file");
    let denied_file = work.lock().unwrap().as_ref().unwrap().join("conversations").join(&denied).join("core-denied.txt");
    f.type_composer_and_send(&denied, "The explicit deny must prevent this fixture write.");
    f.wait_conversation_terminal(&denied, "completed");
    f.assert_main_visible(&denied, "CORE_DENY_CONFIRMED");
    assert!(!denied_file.exists());
    assert_eq!(probe.attempts.lock().unwrap().iter().filter(|r| r["input"]["name"] == "Write").count(), 1);
    assert!(!f.app.handle.view().unwrap().generation.records("user_questions").iter().any(|q| q.data["state"] == "pending"));
    f.reopen_same_workspace();
    assert!(bypass_value(&f));
    assert_eq!(std::fs::read_to_string(&file).unwrap(), "CORE_WRITE_CONTENT");
    f.assert_main_visible(&denied, "CORE_DENY_CONFIRMED");
    f.assert_main_visible(&cid, "CORE_APPROVED_WRITE_SAVED");
    f.assert_tool_result_visible(&cid, "Read · 실행 결과", "CORE_WRITE_CONTENT");
    let next = f._dir.path().join("bypass-independent");
    switch(&mut f, OpenOptions { path: next, mode: OpenMode::Create { name: "Independent bypass".into() }, read_only: false });
    assert!(!bypass_value(&f), "Bypass is workspace-owned even when model/rule settings are common");
    f.shutdown();
    let requests = provider.finish();
    assert_eq!(requests.len(), 5);
    assert!(requests[2].to_string().contains("CORE_WRITE_CONTENT"));
    println!("CORE_ACCEPT actual approval -> one Write -> Read/result display -> bypass still denies -> SQLite reopen -> workspace isolation");
}

#[test]
fn actual_core_stop_conversation_cancels_approval_and_rejects_late_answer_after_reopen() {
    let provider = LocalProvider::new(vec![model_tool("Write", json!({"file_path":"never-approved.txt","content":"MUST_NOT_RUN"}))]);
    let secret = artex_core::new_id("secret");
    let probe = Probe::default();
    let mut f = Fixture::configured(|profile| ServiceConfig {
        host_factory: Some(factory(profile, BTreeMap::from([(secret.clone(), "isolated-stop-value".into())]), probe.clone())),
        wake_ui: None,
    });
    let profile = provider::profile(&mut f, "Core stop model", "openai", &provider.url, &secret, false);
    f.setup("ui.activateLLMProfile", json!({"id":profile}));
    let cid = connected_conversation(&mut f, "Core stopped approval");
    f.type_composer_and_send(&cid, "Wait for local approval; cancellation must prevent the write.");
    f.until(|app| app.handle.view().unwrap().generation.records("user_questions").iter().any(|q| q.data["state"] == "pending"));
    let question = f.app.handle.view().unwrap().generation.records("user_questions")[0].clone();
    assert!(probe.attempts.lock().unwrap().is_empty());
    f.click_visible("응답 중지");
    f.prepare("ui.stopConversation");
    settled(&mut f);
    let view = f.app.handle.view().unwrap();
    let stopped = view.generation.get("user_questions", &question.header.id).unwrap().data.clone();
    assert_eq!(stopped["state"], "cancelled");
    assert!(view.generation.records("agent_runs").iter().any(|r| r.data["conversation_id"] == cid && r.data["state"] == "cancelled"));
    assert!(visible_texts(&f.frame(vec![]), egui::Rect::EVERYTHING).iter().any(|s| s == "응답 중지·일시정지"));
    assert!(!visible_texts(&f.frame(vec![]), egui::Rect::EVERYTHING).iter().any(|s| s == "승인"));
    let params = json!({"question_id":question.header.id,"generation":question.data["generation"],"answer":{"approved":true}});
    let scope = f.app.handle.scope().unwrap();
    f.app.command("user.answer".into(), params.clone(), vec![], Purpose::Command {
        method:"user.answer".into(),params,section:Section::Chat,selected:Some(cid.clone()),scope,
    });
    f.until(|app| app.error.is_some() && app.pending.is_empty());
    // The question's cancelled state is checked before the run epoch; the
    // public user.answer contract reports that stale question as a CAS conflict.
    assert_eq!(f.app.error.as_ref().unwrap().code, "VERSION_CONFLICT");
    assert!(probe.attempts.lock().unwrap().is_empty());
    assert_eq!(f.app.handle.view().unwrap().generation.get("user_questions", &question.header.id).unwrap().data, stopped);
    f.reopen_same_workspace();
    select(&mut f, Section::Chat, Some(&cid));
    assert!(!visible_texts(&f.frame(vec![]), egui::Rect::EVERYTHING).iter().any(|s| s == "승인"));
    assert_eq!(f.app.handle.view().unwrap().generation.get("user_questions", &question.header.id).unwrap().data, stopped);
    assert!(probe.attempts.lock().unwrap().is_empty());
    f.shutdown();
    assert_eq!(provider.finish().len(), 1, "Stopping approval must never dispatch another model turn or replay work");
    println!("CORE_ACCEPT actual stop -> cancelled question -> late-answer rejection/no host effect -> readonly reopen/no replay");
}
