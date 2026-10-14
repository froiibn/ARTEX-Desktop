//! Explicit validation mode uses a new workspace and a local provider fixture.
//! It exercises the same Rust application service used by the native window.
use artex_core::{Error, Expected, Result, new_id};
use artex_host::SecretVault;
use artex_native_core::{
    ConversationAction, DeletionOptions, DeletionTarget, MonitorTarget, NativeAction,
    NativeCommand, NativeFileArea, NativeQuery, NativeReply, NativeTicket, OpenMode, OpenOptions,
    RunControlAction, RunControlTarget, Service, ServiceConfig, ServiceHandle, ServicePhase,
    TaskDeleteOptions, gateway::NativeGateway,
};
use serde_json::{Value, json};
use std::{
    io::{Read, Write},
    net::TcpListener,
    path::PathBuf,
    sync::{Arc, Mutex},
    thread,
    time::{Duration, Instant},
};

pub fn wait(handle: &ServiceHandle, ticket: NativeTicket) -> Result<NativeReply> {
    let deadline = Instant::now() + Duration::from_secs(20);
    loop {
        for completion in handle.drain_completions() {
            if completion.ticket == ticket {
                return completion.result;
            }
        }
        if Instant::now() > deadline {
            return Err(Error::new("TIMEOUT", "Native validation ticket deadline"));
        }
        thread::sleep(Duration::from_millis(5));
    }
}
fn action(handle: &ServiceHandle, action: NativeAction) -> Result<NativeReply> {
    wait(handle, handle.try_submit(action)?)
}
fn command(
    handle: &ServiceHandle,
    method: &str,
    params: Value,
    mut expected: Vec<Expected>,
) -> Result<NativeReply> {
    let scope = handle
        .scope()
        .ok_or_else(|| Error::new("WORKSPACE_NOT_OPEN", "Validation workspace missing"))?;
    let prepared = action(
        handle,
        NativeAction::Query {
            scope: scope.clone(),
            query: NativeQuery::Prepare {
                method: method.into(),
                params: params.clone(),
            },
        },
    )?;
    for reference in serde_json::from_value::<Vec<Expected>>(prepared.value["expected"].clone())? {
        if !expected
            .iter()
            .any(|old| old.collection == reference.collection && old.id == reference.id)
        {
            expected.push(reference);
        }
    }
    action(
        handle,
        NativeAction::Command {
            scope,
            command: NativeCommand::new(method, params, expected),
        },
    )
    .map_err(|error| Error::new(&error.code, &format!("{method}: {}", error.message)))
}
fn expected(collection: &str, id: &str, handle: &ServiceHandle) -> Result<Expected> {
    let view = handle
        .view()
        .ok_or_else(|| Error::new("WORKSPACE_NOT_OPEN", "Validation workspace missing"))?;
    let record = view
        .generation
        .get(collection, id)
        .ok_or_else(|| Error::new("NOT_FOUND", "Validation object missing"))?;
    Ok(Expected {
        collection: collection.into(),
        id: id.into(),
        version: record.header.version.clone(),
    })
}
fn query(handle: &ServiceHandle, query: NativeQuery) -> Result<NativeReply> {
    action(
        handle,
        NativeAction::Query {
            scope: handle
                .scope()
                .ok_or_else(|| Error::new("WORKSPACE_NOT_OPEN", "Validation workspace missing"))?,
            query,
        },
    )
}
fn manage_conversation(
    handle: &ServiceHandle,
    id: &str,
    operation: ConversationAction,
) -> Result<Value> {
    let preview = query(
        handle,
        NativeQuery::DeletionPreview {
            target: DeletionTarget::Conversations {
                ids: vec![id.into()],
            },
            options: DeletionOptions::Conversations { action: operation },
        },
    )?;
    assert_eq!(
        preview.value["can_apply"], true,
        "isolated lifecycle preview is blocked: {}",
        preview.value
    );
    let expected = serde_json::from_value(preview.value["expected"].clone())?;
    let result = action(
        handle,
        NativeAction::ConversationManage {
            scope: handle.scope().unwrap(),
            command_id: new_id("command"),
            expected,
            ids: vec![id.into()],
            action: operation,
        },
    )?
    .value;
    assert_eq!(result["failed"], 0, "{}", result);
    assert_eq!(result["secure_erase"], false);
    Ok(result)
}
fn create_fixture_task(handle: &ServiceHandle, company: &str, name: &str) -> Result<String> {
    let created = command(
        handle,
        "ui.createTask",
        json!({"input":{"name":name,"description":"Isolated native lifecycle validation","goal":"Validate owned data lifecycle","companyIds":[company],"llmProfileIds":[],"sourceTaskIds":[]}}),
        vec![],
    )?;
    created.value["task_id"]
        .as_str()
        .or(created.value["id"].as_str())
        .map(str::to_owned)
        .ok_or_else(|| Error::new("EXTERNAL_FAILURE", "Validation task ID missing"))
}
fn original_transcript(handle: &ServiceHandle, conversation: &str) -> Vec<(String, Value)> {
    let mut rows = handle
        .view()
        .unwrap()
        .generation
        .records("conversation_activities")
        .into_iter()
        .filter(|row| row.data["conversation_id"] == conversation)
        .map(|row| (row.header.id.clone(), row.data.clone()))
        .collect::<Vec<_>>();
    rows.sort_by(|a, b| a.0.cmp(&b.0));
    rows
}
fn quarantine_has_marker(root: &std::path::Path, marker: &[u8]) -> Result<bool> {
    if !root.is_dir() {
        return Ok(false);
    }
    let mut pending = vec![root.to_path_buf()];
    let mut visited = 0usize;
    while let Some(directory) = pending.pop() {
        for entry in std::fs::read_dir(directory)? {
            visited += 1;
            if visited > 10000 {
                return Err(Error::new(
                    "SIZE_LIMIT",
                    "Validation quarantine inventory exceeds budget",
                ));
            }
            let entry = entry?;
            let metadata = std::fs::symlink_metadata(entry.path())?;
            if metadata.file_type().is_symlink() {
                return Err(Error::new(
                    "PERMISSION_DENIED",
                    "Validation quarantine contains a link",
                ));
            }
            if metadata.is_dir() {
                pending.push(entry.path());
            } else if metadata.len() == marker.len() as u64
                && std::fs::read(entry.path())? == marker
            {
                return Ok(true);
            }
        }
    }
    Ok(false)
}
fn lifecycle_fixture(
    handle: &ServiceHandle,
    path: &std::path::Path,
    profile: &std::path::Path,
    conversation: &str,
    company: &str,
    llm_profile: &str,
) -> Result<Value> {
    let transcript = original_transcript(handle, conversation);
    assert!(
        !transcript.is_empty(),
        "original local Read transcript must remain meaningful"
    );
    let view = handle.view().unwrap();
    let run = view
        .generation
        .records("agent_runs")
        .into_iter()
        .find(|run| run.data["conversation_id"] == conversation && run.data["state"] == "completed")
        .ok_or_else(|| Error::new("NOT_FOUND", "Completed local Read run missing"))?
        .header
        .id
        .clone();
    let mut controls = Vec::new();
    for operation in [RunControlAction::Stop, RunControlAction::ForceStop] {
        let monitor = query(
            handle,
            NativeQuery::AgentMonitor {
                target: MonitorTarget::Run {
                    run_id: run.clone(),
                },
            },
        )?;
        let row = monitor.value["runs"]
            .as_array()
            .and_then(|rows| rows.iter().find(|row| row["run_id"] == run))
            .ok_or_else(|| {
                Error::new("NOT_FOUND", "Native monitor did not return its exact run")
            })?;
        assert_eq!(
            row["cleanup_confirmed"], true,
            "completed Read has no external processes, but all provider/tool workers must be quiesced: {row}"
        );
        let controlled = action(
            handle,
            NativeAction::RunControl {
                scope: handle.scope().unwrap(),
                command_id: new_id("command"),
                expected: serde_json::from_value(row["expected"].clone())?,
                target: RunControlTarget::Run {
                    run_id: run.clone(),
                },
                action: operation,
            },
        )?
        .value;
        assert_eq!(controlled["accepted"], true);
        assert_eq!(controlled["cleanup_confirmed"], true, "{controlled}");
        assert!(controlled["secure_erase"] == false);
        controls.push(controlled);
    }
    let created = command(
        handle,
        "ui.createConversation",
        json!({"agent_key":"auto","title":"관리 검증 대화","llm_profile_id":llm_profile}),
        vec![],
    )?;
    let managed = created.value["id"]
        .as_str()
        .ok_or_else(|| Error::new("EXTERNAL_FAILURE", "Management conversation ID missing"))?
        .to_owned();
    // This deliberately empty-history conversation owns an actual attachment.
    // No extra model admission is needed, so the four original HTTP requests
    // and their complete transcripts stay untouched.
    let picked = profile.join("lifecycle-owned.txt");
    std::fs::write(&picked, "ISOLATED-CONVERSATION-FILE")?;
    let staged = action(
        handle,
        NativeAction::StageFile {
            scope: handle.scope().unwrap(),
            path: picked,
        },
    )?;
    let upload = command(
        handle,
        "ui.chatUpload",
        json!({"scope":"session","id":managed,"files":[staged.value["file_capability"]]}),
        vec![],
    )?;
    let relative = upload.value["attachments"][0]["path"]
        .as_str()
        .ok_or_else(|| Error::new("EXTERNAL_FAILURE", "Managed attachment missing"))?;
    let owned = path
        .join("work/conversations")
        .join(&managed)
        .join(relative);
    assert_eq!(
        std::fs::read_to_string(&owned)?,
        "ISOLATED-CONVERSATION-FILE"
    );
    let trashed = manage_conversation(handle, &managed, ConversationAction::Trash)?;
    assert!(owned.is_file(), "soft trash must retain files");
    let trash = query(handle, NativeQuery::ConversationTrash)?;
    assert!(
        trash.value["items"]
            .as_array()
            .unwrap()
            .iter()
            .any(|row| row["id"] == managed)
    );
    manage_conversation(handle, &managed, ConversationAction::Restore)?;
    assert!(owned.is_file());
    let cleared = manage_conversation(handle, &managed, ConversationAction::ClearHistory)?;
    assert_eq!(cleared["cleanup_confirmed"], true);
    assert_eq!(cleared["results"][0]["files_deleted"], true);
    assert!(
        handle
            .view()
            .unwrap()
            .generation
            .get("conversations", &managed)
            .is_some()
    );
    assert!(original_transcript(handle, &managed).is_empty());
    assert!(!owned.exists());
    // A new CWD is permitted after the clear's graph commit; the quarantined
    // old file remains retained and is never advertised as secure erasure.
    command(
        handle,
        "ui.workspaceMkdir",
        json!({"path":format!("conversations/{managed}")}),
        vec![],
    )?;
    action(
        handle,
        NativeAction::FileWrite {
            scope: handle.scope().unwrap(),
            area: NativeFileArea::Work,
            path: format!("conversations/{managed}/new-owned.txt"),
            content: "POST-CLEAR-OWNED-FILE".into(),
            expected_sha256: None,
            create: true,
            command_id: new_id("command"),
        },
    )?;
    manage_conversation(handle, &managed, ConversationAction::Trash)?;
    let deleted = manage_conversation(handle, &managed, ConversationAction::DeletePermanently)?;
    assert_eq!(deleted["cleanup_confirmed"], true);
    assert_eq!(deleted["results"][0]["files_deleted"], true);
    assert!(
        handle
            .view()
            .unwrap()
            .generation
            .get("conversations", &managed)
            .is_none()
    );
    assert!(!path.join("work/conversations").join(&managed).exists());
    let removed_task = create_fixture_task(handle, company, "영구 삭제 검증 작업")?;
    command(
        handle,
        "ui.workspaceMkdir",
        json!({"path":format!("tasks/{removed_task}")}),
        vec![],
    )?;
    action(
        handle,
        NativeAction::FileWrite {
            scope: handle.scope().unwrap(),
            area: NativeFileArea::Work,
            path: format!("tasks/{removed_task}/owned.txt"),
            content: "ISOLATED-TASK-FILE".into(),
            expected_sha256: None,
            create: true,
            command_id: new_id("command"),
        },
    )?;
    let options = TaskDeleteOptions {
        delete_files: true,
        ..Default::default()
    };
    let preview = query(
        handle,
        NativeQuery::DeletionPreview {
            target: DeletionTarget::Tasks {
                ids: vec![removed_task.clone()],
            },
            options: DeletionOptions::Tasks {
                options: options.clone(),
            },
        },
    )?;
    assert_eq!(preview.value["can_apply"], true);
    let task_deleted = action(
        handle,
        NativeAction::TaskDelete {
            scope: handle.scope().unwrap(),
            command_id: new_id("command"),
            expected: serde_json::from_value(preview.value["expected"].clone())?,
            ids: vec![removed_task.clone()],
            options,
        },
    )?
    .value;
    assert_eq!(task_deleted["failed"], 0);
    assert_eq!(task_deleted["cleanup_confirmed"], true);
    assert_eq!(task_deleted["results"][0]["files_deleted"], true);
    assert!(
        handle
            .view()
            .unwrap()
            .generation
            .get("tasks", &removed_task)
            .is_none()
    );
    assert!(!path.join("work/tasks").join(&removed_task).exists());
    let quarantine = path.join("work/.artex-host/quarantine");
    for marker in [
        b"ISOLATED-CONVERSATION-FILE".as_slice(),
        b"POST-CLEAR-OWNED-FILE".as_slice(),
        b"ISOLATED-TASK-FILE".as_slice(),
    ] {
        assert!(
            quarantine_has_marker(&quarantine, marker)?,
            "committed quarantine must retain its owned fixture contents; secure erasure was not requested or implemented"
        );
    }
    let retained = command(
        handle,
        "ui.createConversation",
        json!({"agent_key":"auto","title":"복원 가능한 휴지통 대화","llm_profile_id":llm_profile}),
        vec![],
    )?;
    let retained_id = retained.value["id"]
        .as_str()
        .ok_or_else(|| Error::new("EXTERNAL_FAILURE", "Trash fixture ID missing"))?
        .to_owned();
    manage_conversation(handle, &retained_id, ConversationAction::Trash)?;
    let archived_task = create_fixture_task(handle, company, "보관된 검증 작업")?;
    command(
        handle,
        "ui.controlTask",
        json!({"id":archived_task,"action":"pause"}),
        vec![expected("tasks", &archived_task, handle)?],
    )?;
    command(
        handle,
        "ui.archiveTask",
        json!({"id":archived_task}),
        vec![expected("tasks", &archived_task, handle)?],
    )?;
    let deadline = Instant::now() + Duration::from_secs(20);
    let archive_id = loop {
        let view = handle.view().unwrap();
        let archive = view
            .generation
            .records("task_archives")
            .into_iter()
            .find(|row| row.data["task_id"] == archived_task);
        if let Some(archive) = archive {
            if archive.data["state"] == "ready" {
                break archive.header.id.clone();
            }
            if archive.data["state"] == "archive_failed" {
                return Err(Error::new(
                    "EXTERNAL_FAILURE",
                    "Native isolated archive failed",
                ));
            }
        }
        if Instant::now() >= deadline {
            return Err(Error::new("TIMEOUT", "Native isolated archive deadline"));
        }
        thread::sleep(Duration::from_millis(10));
    };
    assert_eq!(
        original_transcript(handle, conversation),
        transcript,
        "management must preserve the original Read transcript"
    );
    assert!(
        handle
            .view()
            .unwrap()
            .generation
            .get("companies", company)
            .is_some()
    );
    assert!(
        handle
            .view()
            .unwrap()
            .generation
            .get("llm_profiles", llm_profile)
            .is_some()
    );
    assert_eq!(
        std::fs::read_to_string(path.join("work/native-validation.txt"))?,
        "confirmed native file edit"
    );
    let original_attachment = path.join("work/conversations").join(conversation);
    assert!(
        original_attachment.is_dir(),
        "original Read attachment directory was affected by unrelated cleanup"
    );
    thread::sleep(Duration::from_millis(3));
    command(
        handle,
        "ui.renameConversation",
        json!({"id":conversation,"title":"Native fixture conversation"}),
        vec![expected("conversations", conversation, handle)?],
    )?;
    let final_view = handle.view().unwrap();
    let original = final_view
        .generation
        .get("conversations", conversation)
        .unwrap();
    assert!(
        final_view
            .generation
            .records("conversations")
            .iter()
            .all(|row| row.header.id == conversation
                || row.header.updated_at < original.header.updated_at),
        "screen fixture must select the original completed Read conversation first"
    );
    Ok(
        json!({"preview_to_commit":true,"trash_query_restore":true,"clear_history_empty_case":true,"conversation_graph_deleted":true,"task_graph_deleted":true,"owned_files_removed_from_active_scope":true,"owned_files_retained_in_quarantine":true,"secure_erase":false,"shared_preserved":true,"original_transcript_unchanged":true,"monitor_stop_force_receipts":controls,"original_run_id":run,"retained_trash_conversation_id":retained_id,"retained_archive_task_id":archived_task,"retained_archive_id":archive_id,"trashed_receipt":trashed}),
    )
}

pub fn run(path: PathBuf) -> Result<()> {
    if path.exists() {
        return Err(Error::new(
            "VERSION_CONFLICT",
            "Validation requires a new directory; existing user data is never changed",
        ));
    }
    let vault = Arc::new(SecretVault::default());
    let bridge_vault = vault.clone();
    let host_profile = path.with_extension("native-profile");
    if host_profile.exists() {
        return Err(Error::new(
            "VERSION_CONFLICT",
            "Validation profile requires a new directory",
        ));
    }
    std::fs::create_dir_all(&host_profile)?;
    let factory_profile = host_profile.clone();
    let factory: artex_native_core::HostFactory = Arc::new(move |workspace| {
        Ok(Arc::new(NativeGateway::open_with_profile(
            workspace,
            bridge_vault.clone(),
            &factory_profile,
        )?))
    });
    let service = Service::spawn(ServiceConfig {
        host_factory: Some(factory),
        wake_ui: None,
    })?;
    let handle = service.handle();
    action(
        &handle,
        NativeAction::Open(OpenOptions {
            path: path.clone(),
            mode: OpenMode::Create {
                name: "Native validation".into(),
            },
            read_only: false,
        }),
    )?;
    let scope = handle.scope().unwrap();
    let snapshot = handle.view().unwrap();
    assert_eq!(snapshot.generation.records("tools").len(), 81);
    let company = command(
        &handle,
        "ui.createCompany",
        json!({"name":"Native fixture company","scope":[{"kind":"domain","value":"fixture.test"}]}),
        vec![],
    )?;
    let company_id = company.value["id"]
        .as_str()
        .map(str::to_owned)
        .unwrap_or_else(|| {
            handle.view().unwrap().generation.records("companies")[0]
                .header
                .id
                .clone()
        });
    let task = command(
        &handle,
        "ui.createTask",
        json!({"input":{"name":"Native fixture task","description":"Read the authorized local fixture only","goal":"Verify native app integration","companyIds":[company_id],"llmProfileIds":[],"sourceTaskIds":[]}}),
        vec![],
    )?;
    let task_id = task.value["task_id"]
        .as_str()
        .or_else(|| task.value["id"].as_str())
        .map(str::to_owned)
        .unwrap_or_else(|| {
            handle.view().unwrap().generation.records("tasks")[0]
                .header
                .id
                .clone()
        });
    let old_version = expected("tasks", &task_id, &handle)?;
    command(
        &handle,
        "ui.renameTask",
        json!({"id":task_id,"name":"Renamed native task"}),
        vec![old_version.clone()],
    )?;
    assert_eq!(
        command(
            &handle,
            "ui.renameTask",
            json!({"id":task_id,"name":"Stale edit must fail"}),
            vec![old_version]
        )
        .unwrap_err()
        .code,
        "VERSION_CONFLICT"
    );
    command(
        &handle,
        "ui.controlTask",
        json!({"id":task_id,"action":"pause"}),
        vec![expected("tasks", &task_id, &handle)?],
    )?;
    let file = "native-validation.txt".to_string();
    action(
        &handle,
        NativeAction::FileWrite {
            scope: scope.clone(),
            area: NativeFileArea::Work,
            path: file.clone(),
            content: "native read fixture".into(),
            expected_sha256: None,
            create: true,
            command_id: new_id("command"),
        },
    )?;
    let read = action(
        &handle,
        NativeAction::FileRead {
            scope: scope.clone(),
            area: NativeFileArea::Work,
            path: file.clone(),
        },
    )?;
    assert_eq!(read.value["content"], "native read fixture");
    let digest = read.value["sha256"].as_str().unwrap().to_string();
    action(
        &handle,
        NativeAction::FileWrite {
            scope: scope.clone(),
            area: NativeFileArea::Work,
            path: file.clone(),
            content: "confirmed native file edit".into(),
            expected_sha256: Some(digest.clone()),
            create: false,
            command_id: new_id("command"),
        },
    )?;
    assert_eq!(
        action(
            &handle,
            NativeAction::FileWrite {
                scope: scope.clone(),
                area: NativeFileArea::Work,
                path: file.clone(),
                content: "stale native file edit".into(),
                expected_sha256: Some(digest),
                create: false,
                command_id: new_id("command")
            }
        )
        .unwrap_err()
        .code,
        "VERSION_CONFLICT"
    );
    let (endpoint, provider, provider_path) = provider_fixture()?;
    let secret = action(
        &handle,
        NativeAction::SetSecret {
            scope: scope.clone(),
            command_id: new_id("command"),
            field: "api_key".into(),
            value: "native-validation-placeholder-key".into(),
        },
    )?;
    let reference = secret
        .value
        .get("reference")
        .cloned()
        .unwrap_or(secret.value);
    command(
        &handle,
        "ui.saveLLMProfile",
        json!({"p":{"name":"Native local provider","format":"openai","model":"fixture","base_url":endpoint,"api_key":reference,"streaming":false}}),
        vec![],
    )?;
    let profile_id = handle.view().unwrap().generation.records("llm_profiles")[0]
        .header
        .id
        .clone();
    let conversation = command(
        &handle,
        "ui.createConversation",
        json!({"agent_key":"auto","title":"Native fixture conversation","llm_profile_id":profile_id}),
        vec![],
    )?;
    let conversation_id = conversation.value["id"]
        .as_str()
        .map(str::to_owned)
        .unwrap_or_else(|| {
            handle.view().unwrap().generation.records("conversations")[0]
                .header
                .id
                .clone()
        });
    let picked_file = host_profile.join("picked-attachment.txt");
    std::fs::write(&picked_file, "actual native host Read result")?;
    let staged = action(
        &handle,
        NativeAction::StageFile {
            scope: scope.clone(),
            path: picked_file,
        },
    )?;
    let uploaded = command(
        &handle,
        "ui.chatUpload",
        json!({"scope":"session","id":conversation_id,"files":[staged.value["file_capability"]]}),
        vec![],
    )?;
    let attachments = uploaded.value["attachments"].clone();
    let relative = attachments[0]["path"]
        .as_str()
        .ok_or_else(|| Error::new("EXTERNAL_FAILURE", "Native attachment path missing"))?
        .to_string();
    *provider_path.lock().unwrap() = relative.clone();
    // Use the real native approval path with a local provider and local file.
    command(
        &handle,
        "ui.interceptSetToolConfig",
        json!({"enabledTools":["Read"]}),
        vec![],
    )?;
    command(
        &handle,
        "ui.createInterceptRule",
        json!({"rule":{"name":"Native approval fixture","enabled":true,"priority":1000,"match_target":"tool_name","match_type":"string","pattern":"Read","action":"ask","message":"Approve the local validation Read","timeout_enabled":false,"timeout_seconds":300,"timeout_action":"deny"}}),
        vec![],
    )?;
    command(
        &handle,
        "ui.sendConversationMessage",
        json!({"id":conversation_id,"message":format!("Read the authorized {relative} and report the result."),"attachments":attachments}),
        vec![expected("conversations", &conversation_id, &handle)?],
    )?;
    let deadline = Instant::now() + Duration::from_secs(25);
    let mut approval_clicked_at = None;
    let mut approval_to_tool_ms = None;
    loop {
        let view = handle.view().unwrap();
        if approval_clicked_at.is_none() {
            if let Some(pending) = view
                .generation
                .records("intercept_pending")
                .into_iter()
                .find(|row| row.data["status"] == "pending" && row.data["tool_name"] == "Read")
            {
                let clicked_at = Instant::now();
                command(
                    &handle,
                    "ui.interceptDecide",
                    json!({"id":pending.header.id,"decision":"allowed"}),
                    vec![Expected {
                        collection: "intercept_pending".into(),
                        id: pending.header.id.clone(),
                        version: pending.header.version.clone(),
                    }],
                )?;
                approval_clicked_at = Some(clicked_at);
            }
        }
        if approval_to_tool_ms.is_none()
            && view
                .generation
                .records("tool_invocations")
                .iter()
                .any(|row| row.data["name"] == "Read")
        {
            approval_to_tool_ms = approval_clicked_at.map(|at| at.elapsed().as_millis() as u64);
        }
        let runs = view.generation.records("agent_runs");
        if runs.iter().any(|run| {
            run.data["conversation_id"] == conversation_id && run.data["state"] == "completed"
        }) {
            break;
        }
        if let Some(run) = runs.iter().find(|run| {
            run.data["conversation_id"] == conversation_id && run.data["state"] == "failed"
        }) {
            return Err(Error::new(
                "EXTERNAL_FAILURE",
                &format!("Native provider run failed: {}", run.data),
            ));
        }
        if Instant::now() > deadline {
            return Err(Error::new("TIMEOUT", "Native provider run deadline"));
        }
        thread::sleep(Duration::from_millis(10));
    }
    assert!(
        approval_clicked_at.is_some(),
        "Fixture must actually await and approve Read"
    );
    assert!(
        approval_to_tool_ms.is_some(),
        "Approval must resume its actual tool invocation"
    );
    let snapshot = handle.view().unwrap();
    assert!(
        snapshot
            .generation
            .records("tool_invocations")
            .iter()
            .any(|tool| tool.data["name"] == "Read" && tool.data["state"] == "completed")
    );
    let bypass = command(
        &handle,
        "ui.setSettings",
        json!({"patch":{"approval_bypass":true}}),
        vec![],
    )?;
    assert_eq!(bypass.value["approval_bypass"], true);
    let prior_runs: std::collections::BTreeSet<_> = handle
        .view()
        .unwrap()
        .generation
        .records("agent_runs")
        .iter()
        .map(|run| run.header.id.clone())
        .collect();
    command(
        &handle,
        "ui.sendConversationMessage",
        json!({"id":conversation_id,"message":format!("Read the same authorized {relative} with workspace approval bypass."),"attachments":[]}),
        vec![expected("conversations", &conversation_id, &handle)?],
    )?;
    let deadline = Instant::now() + Duration::from_secs(25);
    loop {
        let view = handle.view().unwrap();
        if let Some(run) = view
            .generation
            .records("agent_runs")
            .into_iter()
            .find(|run| {
                run.data["conversation_id"] == conversation_id
                    && !prior_runs.contains(&run.header.id)
            })
        {
            assert!(
                !view
                    .generation
                    .records("intercept_pending")
                    .iter()
                    .any(|row| row.data["audit"]["run_id"] == run.header.id
                        && row.data["status"] == "pending"),
                "Bypass must not await internal Read approval"
            );
            if run.data["state"] == "completed" {
                break;
            }
            if run.data["state"] == "failed" {
                return Err(Error::new(
                    "EXTERNAL_FAILURE",
                    "Native bypass fixture failed",
                ));
            }
        }
        if Instant::now() > deadline {
            return Err(Error::new("TIMEOUT", "Native bypass fixture deadline"));
        }
        thread::sleep(Duration::from_millis(10));
    }
    assert!(
        handle
            .view()
            .unwrap()
            .generation
            .records("intercept_pending")
            .iter()
            .any(|row| row.data["status"] == "allowed"
                && row.data["audit"]["approval_bypass"] == true),
        "Actual bypass execution must retain its audit origin"
    );
    let requests = provider
        .join()
        .map_err(|_| Error::new("EXTERNAL_FAILURE", "Native provider fixture failed"))??;
    assert_eq!(requests.len(), 4);
    for index in [1, 3] {
        assert!(
            requests[index]["messages"]
                .to_string()
                .contains("actual native host Read result")
        );
    }
    let lifecycle = lifecycle_fixture(
        &handle,
        &path,
        &host_profile,
        &conversation_id,
        &company_id,
        &profile_id,
    )?;
    let persisted_transcript = original_transcript(&handle, &conversation_id);
    action(
        &handle,
        NativeAction::Close {
            scope: scope.clone(),
        },
    )?;
    assert_eq!(handle.status().phase, ServicePhase::Closed);
    action(
        &handle,
        NativeAction::Open(OpenOptions {
            path: path.clone(),
            mode: OpenMode::Existing,
            read_only: false,
        }),
    )?;
    let reopened = handle.scope().unwrap();
    let reopened_preview = query(
        &handle,
        NativeQuery::DeletionPreview {
            target: DeletionTarget::Conversations {
                ids: vec![conversation_id.clone()],
            },
            options: DeletionOptions::Conversations {
                action: ConversationAction::ClearHistory,
            },
        },
    )?;
    assert_eq!(
        reopened_preview.value["can_apply"], true,
        "Normal close must persist proof for every completed run, so reopening does not block normal deletion: {}",
        reopened_preview.value
    );
    assert_ne!(scope.engine_epoch, reopened.engine_epoch);
    assert_ne!(scope.open_generation, reopened.open_generation);
    assert_eq!(
        handle
            .try_submit(NativeAction::Query {
                scope,
                query: NativeQuery::WorkspaceInfo
            })
            .unwrap_err()
            .code,
        "STALE_EPOCH"
    );
    let view = handle.view().unwrap();
    let settings = action(
        &handle,
        NativeAction::Query {
            scope: reopened.clone(),
            query: NativeQuery::Feature {
                method: "ui.settings".into(),
                params: json!({}),
            },
        },
    )?;
    assert_eq!(
        settings.value["approval_bypass"], true,
        "Workspace bypass must persist across close/reopen"
    );
    assert!(
        view.generation
            .records("conversation_activities")
            .iter()
            .any(|row| row.data["kind"] == "tool_result")
    );
    assert_eq!(
        original_transcript(&handle, &conversation_id),
        persisted_transcript,
        "original transcript changed across checkpoint/reopen"
    );
    let persisted_monitor = query(
        &handle,
        NativeQuery::AgentMonitor {
            target: MonitorTarget::Run {
                run_id: lifecycle["original_run_id"].as_str().unwrap().into(),
            },
        },
    )?;
    assert_eq!(
        persisted_monitor.value["runs"][0]["cleanup_confirmed"], true,
        "previous epoch must use persisted confirmed proof rather than a new empty registry"
    );
    let persisted_trash = query(&handle, NativeQuery::ConversationTrash)?;
    assert!(
        persisted_trash.value["items"]
            .as_array()
            .unwrap()
            .iter()
            .any(|row| row["id"] == lifecycle["retained_trash_conversation_id"])
    );
    assert_eq!(
        handle
            .view()
            .unwrap()
            .generation
            .get(
                "task_archives",
                lifecycle["retained_archive_id"].as_str().unwrap()
            )
            .unwrap()
            .data["state"],
        "ready"
    );
    action(
        &handle,
        NativeAction::DeleteSecret {
            scope: reopened.clone(),
            reference: reference["secret_ref"].as_str().unwrap().into(),
        },
    )?;
    action(&handle, NativeAction::Close { scope: reopened })?;
    action(&handle, NativeAction::Shutdown)?;
    service.join()?;
    let report = json!({"rust_ui_runtime":"one process Rust application service","workspace":path,"tools":81,"object_cas":true,"file_cas":true,"actual_native_tool":true,"actual_provider_requests":requests.len(),"actual_manual_tool_approval":true,"approval_click_to_tool_ms":approval_to_tool_ms,"actual_bypass_tool_execution":true,"workspace_bypass_persisted":true,"persisted_transcript":true,"workspace_generation_fence":true,"lifecycle":lifecycle,"persisted_cleanup_proof":true,"persisted_trash_and_archive":true,"orderly_shutdown":true,"no_engine_sidecar":true,"passed":true});
    std::fs::write(
        path.join("native-self-test.json"),
        serde_json::to_vec_pretty(&report)?,
    )?;
    println!("Native integrated self-test PASS");
    Ok(())
}
fn provider_fixture() -> Result<(
    String,
    thread::JoinHandle<Result<Vec<Value>>>,
    Arc<Mutex<String>>,
)> {
    let listener = TcpListener::bind("127.0.0.1:0")?;
    listener.set_nonblocking(true)?;
    let address = listener.local_addr()?;
    let path = Arc::new(Mutex::new("fixture.txt".to_string()));
    let fixture_path = path.clone();
    let thread = thread::spawn(move || {
        let mut requests = Vec::new();
        let deadline = Instant::now() + Duration::from_secs(30);
        while requests.len() < 4 {
            if Instant::now() > deadline {
                return Err(Error::new("TIMEOUT", "Local validation provider deadline"));
            }
            let (mut socket, _) = match listener.accept() {
                Ok(pair) => pair,
                Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                    thread::sleep(Duration::from_millis(5));
                    continue;
                }
                Err(error) => return Err(error.into()),
            };
            socket.set_nonblocking(false)?;
            socket.set_read_timeout(Some(Duration::from_secs(5)))?;
            let mut header = Vec::new();
            let mut byte = [0];
            while !header.ends_with(b"\r\n\r\n") {
                socket.read_exact(&mut byte)?;
                header.push(byte[0]);
                if header.len() > 16 * 1024 {
                    return Err(Error::new(
                        "SIZE_LIMIT",
                        "Validation HTTP header exceeds budget",
                    ));
                }
            }
            let header = String::from_utf8(header)
                .map_err(|_| Error::new("INVALID_PARAMS", "Validation HTTP header is not text"))?;
            let length = header
                .lines()
                .find_map(|line| {
                    line.to_ascii_lowercase()
                        .strip_prefix("content-length:")
                        .and_then(|value| value.trim().parse::<usize>().ok())
                })
                .ok_or_else(|| Error::new("INVALID_PARAMS", "Validation HTTP length missing"))?;
            if length > 2 * 1024 * 1024 {
                return Err(Error::new(
                    "SIZE_LIMIT",
                    "Validation request exceeds budget",
                ));
            }
            let mut bytes = vec![0; length];
            socket.read_exact(&mut bytes)?;
            requests.push(serde_json::from_slice(&bytes)?);
            let response = if requests.len() % 2 == 1 {
                json!({"choices":[{"message":{"content":null,"tool_calls":[{"id":format!("native-read-fixture-{}",requests.len()),"type":"function","function":{"name":"Read","arguments":json!({"file_path":fixture_path.lock().unwrap().clone()}).to_string()}}]},"finish_reason":"tool_calls"}],"usage":{"prompt_tokens":5,"completion_tokens":2}})
            } else {
                json!({"choices":[{"message":{"content":"Native fixture read completed."},"finish_reason":"stop"}],"usage":{"prompt_tokens":8,"completion_tokens":3}})
            };
            let bytes = serde_json::to_vec(&response)?;
            write!(
                socket,
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                bytes.len()
            )?;
            socket.write_all(&bytes)?;
        }
        Ok(requests)
    });
    Ok((format!("http://{address}/v1"), thread, path))
}
