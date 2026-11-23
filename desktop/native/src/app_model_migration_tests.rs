//! Real App rendering after legacy model migration, including a hidden side list.
use super::*;
use artex_native_core::NativeReply;

fn service_action(handle: &ServiceHandle, action: NativeAction) -> NativeReply {
    let ticket = handle.try_submit(action).unwrap();
    let deadline = Instant::now() + Duration::from_secs(60);
    loop {
        for completed in handle.drain_completions() {
            assert_eq!(completed.ticket, ticket);
            return completed.result.unwrap();
        }
        assert!(Instant::now() < deadline, "Legacy fixture setup timed out");
        std::thread::sleep(Duration::from_millis(5));
    }
}
fn legacy_command(handle: &ServiceHandle, method: &str, params: Value) -> Value {
    let prepared = service_action(
        handle,
        NativeAction::Query {
            scope: handle.scope().unwrap(),
            query: NativeQuery::Prepare {
                method: method.into(),
                params: params.clone(),
            },
        },
    );
    service_action(
        handle,
        NativeAction::Command {
            scope: handle.scope().unwrap(),
            command: NativeCommand::new(
                method,
                params,
                serde_json::from_value(prepared.value["expected"].clone()).unwrap(),
            ),
        },
    )
    .value
}

fn navigate_to(fixture: &mut Fixture, label: &str) {
    let output = fixture.frame(vec![]);
    if !Fixture::texts(&output)
        .iter()
        .any(|text| text.trim() == label)
    {
        fixture.click("환경 설정");
    }
    let output = fixture.frame(vec![]);
    let rendered = Fixture::texts(&output)
        .into_iter()
        .find(|text| text.trim() == label)
        .expect("Actual navigation row must be visible");
    fixture.click(&rendered);
}

#[test]
fn actual_app_recovers_recent_models_shows_saved_fields_in_compact_window_and_preserves_new_draft()
{
    let mut recovered_id = None;
    let secret = artex_core::new_id("secret");
    let mut fixture = Fixture::configured(|profile| {
        let old = profile.parent().unwrap().join("previous-configured-space");
        let service = Service::spawn(ServiceConfig::default()).unwrap();
        let handle = service.handle();
        service_action(
            &handle,
            NativeAction::Open(OpenOptions {
                path: old.clone(),
                mode: OpenMode::Create {
                    name: "Previous settings".into(),
                },
                read_only: false,
            }),
        );
        let id = legacy_command(
            &handle,
            "ui.saveLLMProfile",
            json!({"p":{
            "name":"Recovered saved connection","model":"recovered-fixture-model","format":"openai",
            "api_key":{"secret_ref":secret,"hint":"synthetic fixture; never resolved"}}}),
        )["id"]
            .as_str()
            .unwrap()
            .to_owned();
        legacy_command(&handle, "ui.activateLLMProfile", json!({"id":id}));
        recovered_id = Some(id);
        service_action(
            &handle,
            NativeAction::Close {
                scope: handle.scope().unwrap(),
            },
        );
        service_action(&handle, NativeAction::Shutdown);
        service.join().unwrap();
        std::fs::write(
            profile.join("recent-workspaces.json"),
            serde_json::to_vec(&vec![old]).unwrap(),
        )
        .unwrap();
        ServiceConfig::default()
    });
    let id = recovered_id.unwrap();
    navigate_to(&mut fixture, "LLM 모델");
    let compact = |fixture: &mut Fixture| {
        fixture.ctx.run(
            egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(950., 1500.),
                )),
                ..Default::default()
            },
            |ctx| fixture.app.render(ctx),
        )
    };
    compact(&mut fixture);
    let output = compact(&mut fixture);
    let texts = Fixture::texts(&output);
    assert_eq!(
        fixture
            .app
            .selected
            .get(&Section::Models)
            .and_then(Option::as_deref),
        Some(id.as_str())
    );
    assert!(
        texts.iter().any(|text| text.contains("저장된 모델 1개")),
        "Saved model selector missing: {texts:?}"
    );
    assert!(
        texts
            .iter()
            .any(|text| text.contains("Recovered saved connection"))
    );
    assert!(
        texts
            .iter()
            .any(|text| text.contains("recovered-fixture-model")),
        "Saved model must hydrate the actual form"
    );
    assert!(
        !texts.iter().any(|text| text.contains("LLM 모델 목록")),
        "This reproduces the hidden compact list"
    );
    assert!(
        !texts.iter().any(|text| text == &secret),
        "Secret identities must not be rendered"
    );
    let view = fixture.app.handle.view().unwrap();
    assert_eq!(
        view.generation.get("llm_profiles", &id).unwrap().data["api_key"]["secret_ref"],
        secret
    );
    assert!(view.generation.records("agent_runs").is_empty());
    drop(view);
    fixture.click("모델 추가");
    let output = compact(&mut fixture);
    assert_eq!(fixture.app.selected.get(&Section::Models), Some(&None));
    assert!(
        Fixture::texts(&output)
            .iter()
            .any(|text| text == "새 모델 연결")
    );
    navigate_to(&mut fixture, "에이전트");
    compact(&mut fixture);
    navigate_to(&mut fixture, "LLM 모델");
    compact(&mut fixture);
    assert_eq!(
        fixture.app.selected.get(&Section::Models),
        Some(&None),
        "Navigation must not replace an intentional new draft"
    );
    fixture.reopen_same_workspace();
    navigate_to(&mut fixture, "LLM 모델");
    compact(&mut fixture);
    let output = compact(&mut fixture);
    assert_eq!(
        fixture
            .app
            .selected
            .get(&Section::Models)
            .and_then(Option::as_deref),
        Some(id.as_str())
    );
    assert!(
        Fixture::texts(&output)
            .iter()
            .any(|text| text.contains("recovered-fixture-model"))
    );
    fixture.shutdown();
}
