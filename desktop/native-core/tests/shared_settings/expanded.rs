//! Public service commands exercise portable configuration and the real shared
//! file capability; no external model/MCP connection or OS secret is resolved.
use super::*;
use artex_host::SecretVault;
use artex_native_core::{NativeFileArea, gateway::NativeGateway};
use std::sync::Arc;

fn with_host(profile: &Path) -> Service {
    let host_profile = profile.to_path_buf();
    let secrets = Arc::new(SecretVault::default());
    Service::spawn_with_profile(
        ServiceConfig {
            host_factory: Some(Arc::new(move |workspace| {
                Ok(Arc::new(NativeGateway::open_with_profile(
                    workspace,
                    secrets.clone(),
                    &host_profile,
                )?))
            })),
            wake_ui: None,
        },
        profile,
    )
    .unwrap()
}
fn agent_record(handle: &ServiceHandle, key: &str) -> artex_core::Record {
    handle
        .view()
        .unwrap()
        .generation
        .records("agents")
        .into_iter()
        .find(|r| r.data["key"] == key)
        .unwrap()
        .clone()
}
fn prompt_body(handle: &ServiceHandle, key: &str) -> Value {
    let agent = agent_record(handle, key);
    handle
        .view()
        .unwrap()
        .generation
        .get(
            "agent_prompts",
            agent.data["current_prompt_id"].as_str().unwrap(),
        )
        .unwrap()
        .data["template_text"]
        .clone()
}
fn read_body(handle: &ServiceHandle, body: Value) -> String {
    action(
        handle,
        NativeAction::Query {
            scope: handle.scope().unwrap(),
            query: NativeQuery::Body { body },
        },
    )
    .value
    .as_str()
    .unwrap()
    .to_owned()
}
fn skill_read(handle: &ServiceHandle) -> Value {
    action(
        handle,
        NativeAction::FileRead {
            scope: handle.scope().unwrap(),
            area: NativeFileArea::Resources,
            path: "skills/shared-inspect/SKILL.md".into(),
        },
    )
    .value
}

#[test]
fn shared_configuration_public_save_switch_restart_and_readonly_prompt_blobs_survive_source_removal()
 {
    let dir = tempfile::tempdir().unwrap();
    let profile = dir.path().join("profile");
    let a = dir.path().join("a");
    let b = dir.path().join("b");
    let c = dir.path().join("c");
    paused_workspace(&a);
    paused_workspace(&b);
    paused_workspace(&c);
    let instructions = "---\nname: shared-inspect\ndescription: shared fixture\ncustom_field: preserved\n---\nInspect only local fixture files.\n";
    let skill = a.join("resources/skills/shared-inspect");
    std::fs::create_dir_all(skill.join("empty")).unwrap();
    std::fs::write(skill.join("SKILL.md"), instructions).unwrap();
    std::fs::write(skill.join("helper.bin"), [0, 255, 10, 42]).unwrap();
    // An independently bootstrapped older workspace proves destination IDs and
    // prior prompt versions are retained, rather than replacing the entire graph.
    let old = Service::spawn(ServiceConfig::default()).unwrap();
    let old_handle = old.handle();
    open(&old_handle, &b, OpenMode::Existing, false);
    let b_agent = agent_record(&old_handle, "auto");
    let b_prompt = b_agent.data["current_prompt_id"]
        .as_str()
        .unwrap()
        .to_owned();
    close(&old_handle);
    shutdown(old);

    let service = with_host(&profile);
    let handle = service.handle();
    open(&handle, &a, OpenMode::Existing, false);
    let a_agent = agent_record(&handle, "auto");
    let large = "Shared prompt without workspace ownership.\n".repeat(2_000);
    command(
        &handle,
        "ui.saveAgentPrompt",
        json!({"key":"auto","template":large,"note":"shared fixture"}),
    );
    assert!(prompt_body(&handle, "auto")["blob"].is_object());
    command(
        &handle,
        "ui.saveAgentConfig",
        json!({"key":"auto","patch":{"max_turns":17,"web_search":true}}),
    );
    command(
        &handle,
        "ui.saveAgentWrapup",
        json!({"key":"auto","prompt":"Common settlement instructions.","maxTurns":2}),
    );
    let secret = new_id("secret");
    command(
        &handle,
        "ui.createCustomTool",
        json!({"t":{"key":"shared_fixture_command","description":"Never executed fixture",
        "kind":"command","enabled":true,"schema":{"type":"object","properties":{}},"agents":["auto"],
        "exec":{"argv":["fixture-command-not-executed"],"env":{"API_TOKEN":{"secret_ref":secret,"hint":"fixture"}}},"deferred":false}}),
    );
    let mcp=command(&handle,"ui.saveMcpServer",json!({"m":{"name":"shared-fixture-server","transport":"http",
        "url":"http://127.0.0.1:9/mcp","enabled":true,"env":{"X-API-Key":{"secret_ref":secret,"hint":"fixture"}}}}))["id"].as_str().unwrap().to_owned();
    command(
        &handle,
        "ui.setAgentVisibility",
        json!({"key":"auto","mcp":[mcp],"skill":["shared-inspect"]}),
    );
    let original_skill = skill_read(&handle);
    assert_eq!(original_skill["content"], instructions);
    assert!(
        handle
            .view()
            .unwrap()
            .generation
            .records("agent_runs")
            .is_empty()
    );
    close(&handle);

    open(&handle, &b, OpenMode::Existing, false);
    let actual = agent_record(&handle, "auto");
    assert_eq!(actual.header.id, b_agent.header.id);
    assert_ne!(actual.header.id, a_agent.header.id);
    assert_eq!(actual.data["max_turns"], 17);
    assert_eq!(actual.data["web_search"], true);
    assert_eq!(read_body(&handle, prompt_body(&handle, "auto")), large);
    assert!(
        handle
            .view()
            .unwrap()
            .generation
            .get("agent_prompts", &b_prompt)
            .is_some()
    );
    let view = handle.view().unwrap();
    let server = view
        .generation
        .records("mcp_servers")
        .into_iter()
        .find(|r| r.data["name"] == "shared-fixture-server")
        .unwrap();
    assert_ne!(server.header.id, mcp);
    assert_eq!(server.data["env"]["X-API-Key"]["secret_ref"], secret);
    assert!(
        view.generation
            .records("agent_visibility")
            .iter()
            .any(|r| r.data["agent_id"] == actual.header.id
                && r.data["resource_id"] == server.header.id
                && r.data["enabled"] == true)
    );
    assert!(
        view.generation
            .records("agent_skill_visibility")
            .iter()
            .any(|r| r.data["agent_id"] == actual.header.id
                && r.data["skill_name"] == "shared-inspect"
                && r.data["enabled"] == true)
    );
    assert!(
        view.generation
            .records("tools")
            .iter()
            .any(|r| r.data["key"] == "shared_fixture_command"
                && r.data["exec"]["env"]["API_TOKEN"]["secret_ref"] == secret)
    );
    assert!(view.generation.records("agent_runs").is_empty());
    drop(view);
    let changed = "---\nname: shared-inspect\ndescription: changed once\ncustom_field: preserved\n---\nChanged common instructions.\n";
    action(
        &handle,
        NativeAction::FileWrite {
            scope: handle.scope().unwrap(),
            area: NativeFileArea::Resources,
            path: "skills/shared-inspect/SKILL.md".into(),
            content: changed.into(),
            expected_sha256: Some(original_skill["sha256"].as_str().unwrap().into()),
            create: false,
            command_id: new_id("command"),
        },
    );
    command(
        &handle,
        "ui.saveAgentConfig",
        json!({"key":"auto","patch":{"max_turns":23}}),
    );
    close(&handle);
    open(&handle, &a, OpenMode::Existing, false);
    assert_eq!(agent_record(&handle, "auto").data["max_turns"], 23);
    assert_eq!(skill_read(&handle)["content"], changed);
    let stale = handle
        .try_submit(NativeAction::FileWrite {
            scope: handle.scope().unwrap(),
            area: NativeFileArea::Resources,
            path: "skills/shared-inspect/SKILL.md".into(),
            content: "stale overwrite".into(),
            expected_sha256: Some(original_skill["sha256"].as_str().unwrap().into()),
            create: false,
            command_id: new_id("command"),
        })
        .unwrap();
    assert_eq!(wait(&handle, stale).unwrap_err().code, "VERSION_CONFLICT");
    close(&handle);
    shutdown(service);

    // Delete only our canonical temporary fixture folder. Common configuration
    // and binary Skill helpers must no longer depend on this migration source.
    assert!(
        a.canonicalize()
            .unwrap()
            .starts_with(dir.path().canonicalize().unwrap())
    );
    std::fs::remove_dir_all(&a).unwrap();
    let before = fingerprints(&c);
    let service = with_host(&profile);
    let handle = service.handle();
    open(&handle, &c, OpenMode::Existing, true);
    assert_eq!(read_body(&handle, prompt_body(&handle, "auto")), large);
    assert_eq!(agent_record(&handle, "auto").data["max_turns"], 23);
    assert_eq!(skill_read(&handle)["content"], changed);
    assert_eq!(
        std::fs::read(
            handle
                .view()
                .unwrap()
                .workspace
                .resources_root
                .join("skills/shared-inspect/helper.bin")
        )
        .unwrap(),
        [0, 255, 10, 42]
    );
    let blocked = handle
        .try_submit(NativeAction::FileWrite {
            scope: handle.scope().unwrap(),
            area: NativeFileArea::Resources,
            path: "skills/shared-inspect/SKILL.md".into(),
            content: "forbidden".into(),
            expected_sha256: None,
            create: false,
            command_id: new_id("command"),
        })
        .unwrap();
    assert_eq!(
        wait(&handle, blocked).unwrap_err().code,
        "RECOVERY_REQUIRED"
    );
    close(&handle);
    shutdown(service);
    assert_eq!(fingerprints(&c), before);
}

#[test]
fn deleted_common_mcp_custom_tool_and_agent_do_not_reappear_in_old_workspace_or_bootstrap() {
    let dir = tempfile::tempdir().unwrap();
    let profile = dir.path().join("profile");
    let a = dir.path().join("a");
    let b = dir.path().join("b");
    let service = Service::spawn_with_profile(ServiceConfig::default(), &profile).unwrap();
    let handle = service.handle();
    open(
        &handle,
        &a,
        OpenMode::Create {
            name: "Source configuration".into(),
        },
        false,
    );
    command(
        &handle,
        "ui.createAgent",
        json!({"key":"shared_helper","name":"Shared helper","description":"Fixture"}),
    );
    command(
        &handle,
        "ui.createCustomTool",
        json!({"t":{"key":"shared_unused_tool","description":"Fixture","kind":"shell","enabled":true,"schema":{},"agents":["shared_helper"]}}),
    );
    let browser = handle
        .view()
        .unwrap()
        .generation
        .records("mcp_servers")
        .into_iter()
        .next()
        .unwrap()
        .data["name"]
        .as_str()
        .unwrap()
        .to_owned();
    close(&handle);
    open(
        &handle,
        &b,
        OpenMode::Create {
            name: "Destination configuration".into(),
        },
        false,
    );
    let server = handle
        .view()
        .unwrap()
        .generation
        .records("mcp_servers")
        .into_iter()
        .find(|r| r.data["name"] == browser)
        .unwrap()
        .header
        .id
        .clone();
    command(&handle, "ui.deleteMcpServer", json!({"id":server}));
    command(
        &handle,
        "ui.deleteCustomTool",
        json!({"key":"shared_unused_tool"}),
    );
    command(&handle, "ui.deleteAgent", json!({"key":"shared_helper"}));
    close(&handle);
    open(&handle, &a, OpenMode::Existing, false);
    assert!(
        !handle
            .view()
            .unwrap()
            .generation
            .records("mcp_servers")
            .iter()
            .any(|r| r.data["name"] == browser)
    );
    assert!(
        !handle
            .view()
            .unwrap()
            .generation
            .records("agents")
            .iter()
            .any(|r| r.data["key"] == "shared_helper")
    );
    assert!(
        !handle
            .view()
            .unwrap()
            .generation
            .records("tools")
            .iter()
            .any(|r| r.data["key"] == "shared_unused_tool")
    );
    let seq = handle.view().unwrap().generation.commit_seq;
    close(&handle);
    open(&handle, &a, OpenMode::Existing, false);
    assert_eq!(
        handle.view().unwrap().generation.commit_seq,
        seq,
        "Global deletions must not be seeded/deleted repeatedly"
    );
    close(&handle);
    shutdown(service);
}

#[test]
fn readonly_zero_generation_workspace_uses_valid_common_view_without_persisting_a_generation() {
    let dir = tempfile::tempdir().unwrap();
    let profile = dir.path().join("profile");
    let source = dir.path().join("configured");
    let empty = dir.path().join("empty");
    drop(Store::create(&empty, "No committed records").unwrap());
    let before = fingerprints(&empty);
    let service = with_host(&profile);
    let handle = service.handle();
    open(
        &handle,
        &source,
        OpenMode::Create {
            name: "Common fixture".into(),
        },
        false,
    );
    close(&handle);
    open(&handle, &empty, OpenMode::Existing, true);
    let view = handle.view().unwrap();
    artex_core::validate_snapshot(&view.generation).unwrap();
    assert!(!view.generation.records("agents").is_empty());
    assert_eq!(
        view.generation.commit_seq, 1,
        "Derived read provenance is separate from source storage"
    );
    assert!(view.workspace.read_only);
    drop(view);
    let info = action(
        &handle,
        NativeAction::Query {
            scope: handle.scope().unwrap(),
            query: NativeQuery::WorkspaceInfo,
        },
    )
    .value;
    assert_eq!(
        info["commit_seq"], "0",
        "The actual disk generation must remain zero"
    );
    close(&handle);
    shutdown(service);
    assert_eq!(fingerprints(&empty), before);
    assert_eq!(
        Store::open_read_only(&empty).unwrap().snapshot().commit_seq,
        0
    );
}

#[test]
fn shared_notification_channel_and_global_intercept_settings_follow_public_commands_without_deliveries()
 {
    let dir = tempfile::tempdir().unwrap();
    let profile = dir.path().join("profile");
    let a = dir.path().join("a");
    let b = dir.path().join("b");
    let service = Service::spawn_with_profile(ServiceConfig::default(), &profile).unwrap();
    let handle = service.handle();
    open(
        &handle,
        &a,
        OpenMode::Create {
            name: "Global policy source".into(),
        },
        false,
    );
    let secret = new_id("secret");
    let channel = command(&handle, "ui.notifyCreateChannel", json!({"payload":{
        "name":"Shared disabled notification", "kind":"email", "enabled":false,
        "mode":"realtime", "config":{"host":"mail.example.test","password":{"secret_ref":secret,"hint":"fixture"}},
        "filter":{},"rate_per_min":0}}))["id"].as_str().unwrap().to_owned();
    let rule = command(&handle, "ui.createInterceptRule", json!({"rule":{
        "name":"Shared disabled rule","enabled":false,"priority":10,"match_target":"tool_name",
        "match_type":"string","pattern":"FixtureNeverInvoked","action":"deny","message":"Fixture only",
        "timeout_enabled":false,"timeout_seconds":0,"timeout_action":"deny"}}))["id"].as_str().unwrap().to_owned();
    command(
        &handle,
        "ui.interceptSetToolConfig",
        json!({"enabledTools":["FixtureNeverInvoked"]}),
    );
    command(
        &handle,
        "ui.interceptSetJudgeConfig",
        json!({"cfg":{"enabled":false,"profile_id":null,"prompt":"Shared judgement fixture",
        "timeout_seconds":19,"fail_action":"ask","ask_timeout_seconds":45,"ask_timeout_action":"deny"}}),
    );
    close(&handle);
    open(
        &handle,
        &b,
        OpenMode::Create {
            name: "Global policy destination".into(),
        },
        false,
    );
    let view = handle.view().unwrap();
    assert_eq!(
        view.generation
            .get("notification_channels", &channel)
            .unwrap()
            .data["config"]["password"]["secret_ref"],
        secret
    );
    assert_eq!(
        view.generation.get("intercept_rules", &rule).unwrap().data["action"],
        "deny"
    );
    assert!(
        view.generation
            .records("notification_deliveries")
            .is_empty()
    );
    assert!(view.generation.records("intercept_approvals").is_empty());
    drop(view);
    let settings = query(&handle, "ui.interceptGetToolConfig", json!({}));
    assert_eq!(settings["enabled_tools"], json!(["FixtureNeverInvoked"]));
    let judge = query(&handle, "ui.interceptGetJudgeConfig", json!({}));
    assert_eq!(judge["prompt"], "Shared judgement fixture");
    assert_eq!(judge["timeout_seconds"], 19);
    command(&handle, "ui.notifyDeleteChannel", json!({"id":channel}));
    command(&handle, "ui.deleteInterceptRule", json!({"id":rule}));
    close(&handle);
    open(&handle, &a, OpenMode::Existing, false);
    let view = handle.view().unwrap();
    assert!(
        view.generation
            .get("notification_channels", &channel)
            .is_none()
    );
    assert!(view.generation.get("intercept_rules", &rule).is_none());
    assert!(
        view.generation
            .records("notification_deliveries")
            .is_empty()
    );
    drop(view);
    close(&handle);
    shutdown(service);
}
