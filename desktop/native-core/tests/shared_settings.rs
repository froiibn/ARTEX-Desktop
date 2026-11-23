//! Shared application configuration follows public Service commands across
//! workspace lifetimes; workspace execution policy and data remain local.
//! Synthetic SecretRefs are never resolved and no OS credential is accessed.
#[path = "shared_settings/expanded.rs"]
mod expanded;
#[path = "shared_settings/recent_models.rs"]
mod recent_models;
use artex_core::{Change, CommandBatch, Expected, Result, default_data, new_id};
use artex_native_core::{
    NativeAction, NativeCommand, NativeQuery, NativeReply, NativeTicket, OpenMode, OpenOptions,
    Service, ServiceConfig, ServiceHandle,
};
use artex_store::Store;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
    thread,
    time::{Duration, Instant},
};

fn wait(handle: &ServiceHandle, ticket: NativeTicket) -> Result<NativeReply> {
    let until = Instant::now() + Duration::from_secs(60);
    loop {
        for done in handle.drain_completions() {
            assert_eq!(
                done.ticket, ticket,
                "Fixture submits one public action at a time"
            );
            return done.result;
        }
        assert!(
            Instant::now() < until,
            "Service action timed out: {:?}",
            handle.status()
        );
        thread::sleep(Duration::from_millis(5));
    }
}
fn action(handle: &ServiceHandle, action: NativeAction) -> NativeReply {
    let label = match &action {
        NativeAction::Command { command, .. } => command.method.clone(),
        NativeAction::Query {
            query: NativeQuery::Prepare { method, .. },
            ..
        } => format!("Prepare {method}"),
        _ => format!("{:?}", std::mem::discriminant(&action)),
    };
    wait(handle, handle.try_submit(action).unwrap())
        .unwrap_or_else(|error| panic!("{label}: {error:?}"))
}
fn query(handle: &ServiceHandle, method: &str, params: Value) -> Value {
    action(
        handle,
        NativeAction::Query {
            scope: handle.scope().unwrap(),
            query: NativeQuery::Feature {
                method: method.into(),
                params,
            },
        },
    )
    .value
}
fn command(handle: &ServiceHandle, method: &str, params: Value) -> Value {
    let prepared = action(
        handle,
        NativeAction::Query {
            scope: handle.scope().unwrap(),
            query: NativeQuery::Prepare {
                method: method.into(),
                params: params.clone(),
            },
        },
    );
    let expected: Vec<Expected> =
        serde_json::from_value(prepared.value["expected"].clone()).unwrap();
    action(
        handle,
        NativeAction::Command {
            scope: handle.scope().unwrap(),
            command: NativeCommand::new(method, params, expected),
        },
    )
    .value
}
fn open(handle: &ServiceHandle, path: &Path, mode: OpenMode, read_only: bool) -> String {
    let reply = action(
        handle,
        NativeAction::Open(OpenOptions {
            path: path.to_owned(),
            mode,
            read_only,
        }),
    );
    if !read_only {
        assert_eq!(
            reply.value["commit_seq"],
            reply.read_seq.to_string(),
            "Open must report the final persisted projection sequence"
        );
    }
    handle.scope().unwrap().workspace_id
}
fn close(handle: &ServiceHandle) {
    action(
        handle,
        NativeAction::Close {
            scope: handle.scope().unwrap(),
        },
    );
}
fn shutdown(service: Service) {
    action(&service.handle(), NativeAction::Shutdown);
    service.join().unwrap();
}
fn model_input(id: Option<&str>, model: &str, secret: Option<&str>) -> Value {
    let mut input = json!({"name":"Shared local fixture","format":"openai","model":model,"base_url":"http://127.0.0.1:9/v1","streaming":true,"priority":7,"pool_exclude":false,"rate_per_second":2.0,"context_window_k":128,"retry":{"connect":{"attempts":2,"interval_ms":25},"empty":{"attempts":1,"interval_ms":30},"stream":{"attempts":0,"interval_ms":0}}});
    if let Some(id) = id {
        input["id"] = json!(id);
    }
    if let Some(secret) = secret {
        input["api_key"] =
            json!({"secret_ref":secret,"hint":"synthetic reference; never resolved"});
    }
    input
}
fn profile<'a>(rows: &'a Value, id: &str) -> &'a Value {
    rows.as_array()
        .unwrap()
        .iter()
        .find(|r| r["id"] == id)
        .unwrap_or_else(|| panic!("Shared profile {id} missing from {rows}"))
}
fn active_setting(handle: &ServiceHandle) -> Option<String> {
    handle
        .view()
        .unwrap()
        .generation
        .snapshot
        .records("settings")
        .into_iter()
        .find(|r| r.data["key"] == "active_task_id")
        .and_then(|r| r.data["value"].as_str())
        .map(str::to_owned)
}

/// A paused fixture proves workspace-local selection without ui.createTask's
/// intentional scheduling behavior. Configuration and chat operations below
/// all pass through Prepare/Command; no provider or tool execution is admitted.
fn paused_workspace(path: &Path) -> String {
    let mut store = Store::create(path, "Paused local configuration fixture").unwrap();
    let exploration = new_id("exploration");
    let task = new_id("task");
    let setting = new_id("setting");
    let mut exp = default_data("explorations").unwrap();
    exp["id"] = json!(exploration);
    let mut data = default_data("tasks").unwrap();
    data["id"] = json!(task);
    data["description"] = json!("Workspace-owned paused task");
    data["goal"] = json!("Never launch work in configuration tests");
    data["exploration_id"] = json!(exploration);
    data["status"] = json!("paused");
    data["paused"] = json!(true);
    let mut selected = default_data("settings").unwrap();
    selected["key"] = json!("active_task_id");
    selected["value"] = json!(task);
    store
        .commit(CommandBatch {
            command_id: new_id("command"),
            method: "fixture.shared-settings".into(),
            normalized_params: json!({}),
            expected: vec![],
            changes: vec![
                Change::Put {
                    collection: "explorations".into(),
                    id: exploration,
                    data: exp,
                },
                Change::Put {
                    collection: "tasks".into(),
                    id: task.clone(),
                    data,
                },
                Change::Put {
                    collection: "settings".into(),
                    id: setting,
                    data: selected,
                },
            ],
            events: vec![],
            deliveries: vec![],
            result: json!({}),
        })
        .unwrap();
    store.checkpoint().unwrap();
    task
}
fn fingerprints(root: &Path) -> BTreeMap<PathBuf, (u64, String)> {
    fn visit(root: &Path, dir: &Path, out: &mut BTreeMap<PathBuf, (u64, String)>) {
        for entry in std::fs::read_dir(dir).unwrap() {
            let path = entry.unwrap().path();
            if path.is_dir() {
                visit(root, &path, out);
            } else {
                let bytes = std::fs::read(&path).unwrap();
                out.insert(
                    path.strip_prefix(root).unwrap().to_owned(),
                    (bytes.len() as u64, format!("{:x}", Sha256::digest(bytes))),
                );
            }
        }
    }
    let mut out = BTreeMap::new();
    visit(root, root, &mut out);
    out
}

#[test]
fn shared_models_and_general_settings_survive_workspace_switch_and_profile_service_restart() {
    let dir = tempfile::tempdir().unwrap();
    let app_profile = dir.path().join("profile");
    let workspace_a = dir.path().join("workspace-a");
    let workspace_b = dir.path().join("workspace-b");
    let task_a = paused_workspace(&workspace_a);
    let service = Service::spawn_with_profile(ServiceConfig::default(), &app_profile).unwrap();
    let handle = service.handle();
    let a_id = open(&handle, &workspace_a, OpenMode::Existing, false);
    let secret = new_id("secret");
    let created = command(
        &handle,
        "ui.saveLLMProfile",
        json!({"p":model_input(None,"fixture-original",Some(&secret))}),
    );
    let profile_id = created["id"].as_str().unwrap().to_owned();
    command(&handle, "ui.activateLLMProfile", json!({"id":profile_id}));
    command(
        &handle,
        "ui.setSettings",
        json!({"patch":{"workers":7,"global_proxy":"http://127.0.0.1:9","python_interpreter":"shared-python-fixture","llm_pool_enabled":true,"llm_pool_bind_fallback":true,"web_search_enabled":false,"notify_digest_interval_min":42,"approval_bypass":true}}),
    );
    command(
        &handle,
        "ui.saveLLMRetryPolicy",
        json!({"p":{"connect":{"attempts":2,"interval_ms":17},"empty":{"attempts":1,"interval_ms":19},"stream":{"attempts":0,"interval_ms":0},"breaker":{"attempts":4,"interval_ms":41},"intent":{"attempts":2,"interval_ms":23}}}),
    );
    command(
        &handle,
        "ui.saveAgentConfig",
        json!({"key":"auto","patch":{"llm_profile_id":profile_id}}),
    );
    let conversation_a=command(&handle,"ui.createConversation",json!({"agent_key":"auto","title":"Workspace A local conversation","llm_profile_id":profile_id}))["id"].as_str().unwrap().to_owned();
    assert_eq!(active_setting(&handle).as_deref(), Some(task_a.as_str()));
    assert!(
        handle
            .view()
            .unwrap()
            .generation
            .snapshot
            .records("agent_runs")
            .is_empty()
    );
    close(&handle);

    let b_id = open(
        &handle,
        &workspace_b,
        OpenMode::Create {
            name: "New independent workspace B".into(),
        },
        false,
    );
    assert_ne!(a_id, b_id);
    let profiles = query(&handle, "ui.llmProfiles", json!({}));
    assert_eq!(profile(&profiles, &profile_id)["model"], "fixture-original");
    assert_eq!(profile(&profiles, &profile_id)["is_default"], true);
    assert_eq!(profile(&profiles, &profile_id)["priority"], 7);
    assert_eq!(
        profile(&profiles, &profile_id)["retry"]["connect"]["attempts"],
        2
    );
    assert!(
        query(&handle, "ui.getLLM", json!({}))["configured"]
            .as_bool()
            .unwrap()
    );
    let settings = query(&handle, "ui.settings", json!({}));
    for (key, expected) in [
        ("workers", json!(7)),
        ("global_proxy", json!("http://127.0.0.1:9")),
        ("python_interpreter", json!("shared-python-fixture")),
        ("llm_pool_enabled", json!(true)),
        ("llm_pool_bind_fallback", json!(true)),
        ("notify_digest_interval_min", json!(42)),
    ] {
        assert_eq!(settings[key], expected, "Shared setting {key}");
    }
    assert_eq!(
        settings["approval_bypass"], false,
        "Workspace A bypass must not grant workspace B approval bypass"
    );
    assert!(
        active_setting(&handle).is_none(),
        "Workspace A active task must not enter the app catalog"
    );
    assert!(
        handle
            .view()
            .unwrap()
            .generation
            .snapshot
            .get("tasks", &task_a)
            .is_none()
    );
    assert!(
        handle
            .view()
            .unwrap()
            .generation
            .snapshot
            .get("conversations", &conversation_a)
            .is_none()
    );
    assert_eq!(
        query(&handle, "ui.getAgent", json!({"key":"auto"}))["agent"]["llm_profile_id"],
        profile_id,
        "Agent configuration follows the common app profile"
    );
    let conversation_b = command(
        &handle,
        "ui.createConversation",
        json!({"agent_key":"auto","title":"Workspace B model selection"}),
    )["id"]
        .as_str()
        .unwrap()
        .to_owned();
    command(
        &handle,
        "ui.updateConversationProfile",
        json!({"id":conversation_b,"llm_profile_id":profile_id}),
    );
    assert_eq!(
        handle
            .view()
            .unwrap()
            .generation
            .snapshot
            .get("conversations", &conversation_b)
            .unwrap()
            .data["llm_profile_id"],
        profile_id
    );
    command(
        &handle,
        "ui.saveLLMProfile",
        json!({"p":model_input(Some(&profile_id),"fixture-edited-in-b",None)}),
    );
    command(&handle, "ui.setSettings", json!({"patch":{"workers":9}}));
    close(&handle);

    assert_eq!(open(&handle, &workspace_a, OpenMode::Existing, false), a_id);
    let profiles = query(&handle, "ui.llmProfiles", json!({}));
    assert_eq!(
        profile(&profiles, &profile_id)["model"],
        "fixture-edited-in-b"
    );
    assert_eq!(profile(&profiles, &profile_id)["is_default"], true);
    assert_eq!(query(&handle, "ui.settings", json!({}))["workers"], 9);
    assert_eq!(
        query(&handle, "ui.settings", json!({}))["approval_bypass"],
        true
    );
    assert_eq!(active_setting(&handle).as_deref(), Some(task_a.as_str()));
    assert_eq!(
        query(&handle, "ui.getAgent", json!({"key":"auto"}))["agent"]["llm_profile_id"],
        profile_id
    );
    assert_eq!(
        handle
            .view()
            .unwrap()
            .generation
            .snapshot
            .get("conversations", &conversation_a)
            .unwrap()
            .data["llm_profile_id"],
        profile_id
    );
    assert!(
        handle
            .view()
            .unwrap()
            .generation
            .snapshot
            .get("conversations", &conversation_b)
            .is_none()
    );
    assert_eq!(
        query(&handle, "ui.llmRetryPolicy", json!({}))["breaker"]["attempts"],
        4
    );
    assert!(
        handle
            .view()
            .unwrap()
            .generation
            .snapshot
            .records("agent_runs")
            .is_empty()
    );
    close(&handle);
    shutdown(service);

    let restarted = Service::spawn_with_profile(ServiceConfig::default(), &app_profile).unwrap();
    let handle = restarted.handle();
    assert_eq!(open(&handle, &workspace_b, OpenMode::Existing, false), b_id);
    assert_eq!(
        profile(&query(&handle, "ui.llmProfiles", json!({})), &profile_id)["model"],
        "fixture-edited-in-b"
    );
    assert_eq!(query(&handle, "ui.settings", json!({}))["workers"], 9);
    assert_eq!(
        query(&handle, "ui.settings", json!({}))["approval_bypass"],
        false
    );
    assert!(
        query(&handle, "ui.getLLM", json!({}))["configured"]
            .as_bool()
            .unwrap(),
        "Saved SecretRef configuration must survive without resolving OS credentials"
    );
    let reference = handle
        .view()
        .unwrap()
        .generation
        .snapshot
        .get("llm_profiles", &profile_id)
        .unwrap()
        .data["api_key"]
        .clone();
    assert_eq!(reference["secret_ref"], secret);
    close(&handle);
    shutdown(restarted);
}

#[test]
fn readonly_existing_workspace_reads_shared_catalog_without_changing_any_input_file() {
    let dir = tempfile::tempdir().unwrap();
    let app_profile = dir.path().join("profile");
    let source = dir.path().join("configured");
    let readonly = dir.path().join("readonly");
    let task = paused_workspace(&readonly);
    let before = fingerprints(&readonly);
    let service = Service::spawn_with_profile(ServiceConfig::default(), &app_profile).unwrap();
    let handle = service.handle();
    open(
        &handle,
        &source,
        OpenMode::Create {
            name: "Configuration source".into(),
        },
        false,
    );
    let created = command(
        &handle,
        "ui.saveLLMProfile",
        json!({"p":model_input(None,"readonly-visible-model",Some(&new_id("secret")))}),
    );
    let profile_id = created["id"].as_str().unwrap().to_owned();
    command(&handle, "ui.activateLLMProfile", json!({"id":profile_id}));
    command(
        &handle,
        "ui.setSettings",
        json!({"patch":{"workers":8,"approval_bypass":true}}),
    );
    close(&handle);
    open(&handle, &readonly, OpenMode::Existing, true);
    assert!(handle.view().unwrap().workspace.read_only);
    assert_eq!(
        profile(&query(&handle, "ui.llmProfiles", json!({})), &profile_id)["model"],
        "readonly-visible-model"
    );
    assert_eq!(query(&handle, "ui.settings", json!({}))["workers"], 8);
    assert_eq!(
        query(&handle, "ui.settings", json!({}))["approval_bypass"],
        false
    );
    assert_eq!(active_setting(&handle).as_deref(), Some(task.as_str()));
    assert!(
        query(&handle, "ui.getLLM", json!({}))["configured"]
            .as_bool()
            .unwrap()
    );
    assert!(
        handle
            .view()
            .unwrap()
            .generation
            .snapshot
            .records("agent_runs")
            .is_empty()
    );
    close(&handle);
    shutdown(service);
    assert_eq!(
        fingerprints(&readonly),
        before,
        "Shared readonly projection must not migrate, rewrite, checkpoint or otherwise mutate the input workspace"
    );
}

#[test]
fn deleting_a_shared_nondefault_profile_does_not_resurrect_it_from_a_closed_workspace() {
    let dir = tempfile::tempdir().unwrap();
    let profile_dir = dir.path().join("profile");
    let a = dir.path().join("a");
    let b = dir.path().join("b");
    let service = Service::spawn_with_profile(ServiceConfig::default(), &profile_dir).unwrap();
    let handle = service.handle();
    open(
        &handle,
        &a,
        OpenMode::Create {
            name: "Profile owner A".into(),
        },
        false,
    );
    let primary = command(
        &handle,
        "ui.saveLLMProfile",
        json!({"p":model_input(None,"primary-default",Some(&new_id("secret")))}),
    )["id"]
        .as_str()
        .unwrap()
        .to_owned();
    command(&handle, "ui.activateLLMProfile", json!({"id":primary}));
    let mut secondary_input = model_input(None, "secondary-deleted", Some(&new_id("secret")));
    secondary_input["name"] = json!("Secondary shared model");
    let secondary = command(&handle, "ui.saveLLMProfile", json!({"p":secondary_input}))["id"]
        .as_str()
        .unwrap()
        .to_owned();
    let conv=command(&handle,"ui.createConversation",json!({"agent_key":"auto","title":"Old explicit model selection","llm_profile_id":secondary}))["id"].as_str().unwrap().to_owned();
    command(
        &handle,
        "ui.saveAgentConfig",
        json!({"key":"auto","patch":{"llm_profile_id":secondary}}),
    );
    command(
        &handle,
        "ui.interceptSetJudgeConfig",
        json!({"cfg":{"enabled":false,"profile_id":secondary,
        "prompt":"Unexecuted fixture", "timeout_seconds":15,"fail_action":"ask","ask_timeout_seconds":30,"ask_timeout_action":"deny"}}),
    );
    close(&handle);
    open(
        &handle,
        &b,
        OpenMode::Create {
            name: "Profile deletion B".into(),
        },
        false,
    );
    command(&handle, "ui.deleteLLMProfile", json!({"id":secondary}));
    assert!(query(&handle, "ui.interceptGetJudgeConfig", json!({}))["profile_id"].is_null());
    close(&handle);
    open(&handle, &a, OpenMode::Existing, false);
    let profiles = query(&handle, "ui.llmProfiles", json!({}));
    assert!(
        !profiles
            .as_array()
            .unwrap()
            .iter()
            .any(|r| r["id"] == secondary),
        "Deleted global ID must not be re-imported from stale workspace data"
    );
    assert_eq!(profile(&profiles, &primary)["is_default"], true);
    assert!(query(&handle, "ui.interceptGetJudgeConfig", json!({}))["profile_id"].is_null());
    assert!(
        query(&handle, "ui.getAgent", json!({"key":"auto"}))["agent"]["llm_profile_id"].is_null(),
        "Deleted global profile must preserve SET NULL semantics for local agent references"
    );
    assert!(
        handle
            .view()
            .unwrap()
            .generation
            .snapshot
            .get("conversations", &conv)
            .unwrap()
            .data["llm_profile_id"]
            .is_null()
    );
    close(&handle);
    shutdown(service);
}
