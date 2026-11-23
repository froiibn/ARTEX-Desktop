//! Actual public RPC admission, durable reopen and query coverage for V01/V02/V13.
use super::*;
use artex_domain::Planner;
use chrono::{Duration as ChronoDuration, Utc};

fn fixture() -> (tempfile::TempDir, Router) {
    let dir = tempfile::tempdir().unwrap();
    let mut router = Router::new(BTreeMap::new());
    router.store =
        Some(Store::create(&dir.path().join("workspace"), "public domain UI fixture").unwrap());
    (dir, router)
}
fn request(router: &mut Router, method: &str, params: Value, expected: bool) -> Value {
    if router.host_epoch.is_none() {
        let hello=router.response(json!({"jsonrpc":"2.0","protocol":"artex-rpc/1.0","id":new_id("request"),"method":"system.hello","workspace_id":null,"engine_epoch":null,"command_id":null,"expected":[],"timeout_ms":5000,"params":{"versions":["artex-rpc/1.0"],"host_epoch":new_id("epoch"),"capabilities":[]}}));
        assert!(hello.get("error").is_none(), "{hello}");
    }
    let s = router.snapshot().unwrap();
    let versions = if expected {
        s.collections.values().flat_map(|rows| rows.values()).map(|row| json!({"collection":row.header.collection,"id":row.header.id,"version":row.header.version})).collect::<Vec<_>>()
    } else {
        vec![]
    };
    let before = s.commit_seq;
    let response=router.response(json!({"jsonrpc":"2.0","protocol":"artex-rpc/1.0","id":new_id("request"),"method":method,"workspace_id":s.workspace_id,"engine_epoch":router.engine_epoch,"command_id":new_id("command"),"expected":versions,"timeout_ms":5000,"params":params}));
    println!(
        "E2E {}",
        json!({"method":method,"params":params,"before":before,"after":router.snapshot().unwrap().commit_seq,"response":response})
    );
    response
}
fn ok(router: &mut Router, method: &str, params: Value) -> Value {
    let response = request(router, method, params, true);
    assert!(response.get("error").is_none(), "{method}: {response}");
    let result = response["result"].clone();
    artex_core::validate_contract(
        &router.contract["results"][method],
        &router.contract["$defs"],
        &result,
    )
    .unwrap_or_else(|e| panic!("{method}: {e:?}; {result}"));
    result
}
fn create_task(router: &mut Router, name: &str) -> String {
    ok(
        router,
        "ui.createTask",
        json!({"input":{"name":name,"description":name,"goal":"Local fixture only"}}),
    )["id"]
        .as_str()
        .unwrap()
        .to_owned()
}
fn reopen(router: &mut Router, root: &std::path::Path, readonly: bool) {
    router.store.take().unwrap().checkpoint().unwrap();
    let mut next = Router::new(BTreeMap::new());
    next.store = Some(if readonly {
        Store::open_read_only(root).unwrap()
    } else {
        Store::open(root).unwrap()
    });
    next.read_only = readonly;
    *router = next;
}
fn drain(router: &mut Router, archive: &str, restoring: bool) {
    let end = Instant::now() + Duration::from_secs(10);
    loop {
        router.process_archive_jobs().unwrap();
        let s = router.snapshot().unwrap();
        let complete = if restoring {
            s.get("task_archives", archive).is_none()
        } else {
            s.get("task_archives", archive)
                .is_some_and(|a| a.data["state"] == "ready")
        };
        if complete && router.archive_work.is_none() {
            return;
        }
        assert!(Instant::now() < end, "Archive workflow did not finish");
        std::thread::sleep(Duration::from_millis(2));
    }
}
#[test]
fn public_profile_delete_preserves_typed_identity_cas_and_reopen() {
    let (dir, mut router) = fixture();
    let profile = ok(
        &mut router,
        "ui.saveLLMProfile",
        json!({"p":{"name":"Disposable fixture profile","model":"fixture","format":"openai","api_key":{"secret_ref":new_id("secret"),"hint":"fixture-only reference"}}}),
    )["id"]
        .as_str()
        .unwrap()
        .to_owned();
    assert!(
        ok(&mut router, "ui.llmProfiles", json!({}))
            .as_array()
            .unwrap()
            .iter()
            .any(|p| p["id"] == profile)
    );
    let linked_task=ok(&mut router,"ui.createTask",json!({"input":{"description":"Linked profile fixture","goal":"Local fixture only","llmProfileIds":[profile]}}))["id"].as_str().unwrap().to_owned();
    assert_eq!(
        router
            .snapshot()
            .unwrap()
            .get("tasks", &linked_task)
            .unwrap()
            .data["llm_profile_id"],
        profile
    );
    let before = router.snapshot().unwrap();
    assert_eq!(
        request(
            &mut router,
            "ui.deleteLLMProfile",
            json!({"id":profile}),
            false
        )["error"]["data"]["code"],
        "VERSION_CONFLICT"
    );
    assert_eq!(before.collections, router.snapshot().unwrap().collections);
    let deleted = ok(&mut router, "ui.deleteLLMProfile", json!({"id":profile}));
    assert_eq!(deleted["deleted"], profile);
    assert!(
        router
            .snapshot()
            .unwrap()
            .get("tasks", &linked_task)
            .unwrap()
            .data["llm_profile_id"]
            .is_null()
    );
    assert!(
        router
            .snapshot()
            .unwrap()
            .records("task_llm_profiles")
            .iter()
            .all(|r| r.data["profile_id"] != profile)
    );
    for invalid in [
        json!({"deleted":1}),
        json!({"deleted":new_id("task")}),
        json!({"deleted":"profile_invalid"}),
    ] {
        assert!(
            artex_core::validate_contract(
                &router.contract["results"]["ui.deleteLLMProfile"],
                &router.contract["$defs"],
                &invalid
            )
            .is_err()
        );
    }
    reopen(&mut router, &dir.path().join("workspace"), false);
    assert!(
        router
            .snapshot()
            .unwrap()
            .get("tasks", &linked_task)
            .unwrap()
            .data["llm_profile_id"]
            .is_null()
    );
    assert!(
        router
            .snapshot()
            .unwrap()
            .records("task_llm_profiles")
            .iter()
            .all(|r| r.data["profile_id"] != profile)
    );
    assert!(
        !ok(&mut router, "ui.llmProfiles", json!({}))
            .as_array()
            .unwrap()
            .iter()
            .any(|p| p["id"] == profile)
    );
    assert_eq!(
        request(
            &mut router,
            "ui.deleteLLMProfile",
            json!({"id":profile}),
            true
        )["error"]["data"]["code"],
        "NOT_FOUND"
    );
}
#[test]
fn public_archive_restore_projects_source_totals_and_keeps_rich_statistics_through_reopen() {
    let (dir, mut router) = fixture();
    let task = create_task(&mut router, "Public archive fixture");
    let now = Utc::now();
    let recent = now - ChronoDuration::hours(23);
    let old = now - ChronoDuration::hours(25);
    let s = router.snapshot().unwrap();
    let exp = s.get("tasks", &task).unwrap().data["exploration_id"].clone();
    let mut p = Planner::new(&s, "migration.fixture.usage", &json!({}));
    let mut metadata = BTreeMap::new();
    for (at, tokens) in [(now, 1), (recent, 2), (old, 4)] {
        p.insert("llm_usage","llmusage",json!({"task_id":task,"exploration_id":exp,"profile_name":"fixture","ts":at.to_rfc3339_opts(chrono::SecondsFormat::Millis,true),"input_tokens":tokens,"output_tokens":1,"cache_read":2,"cache_write":3})).unwrap();
        let activity=p.insert("activity","activity",json!({"exploration_id":exp,"kind":"result","worker":"fixture","input_tokens":tokens})).unwrap();
        // Store preserves creation time through import metadata, not arbitrary
        // writes to created_at. Seed genuine historical rows through that path.
        let timestamp = at.to_rfc3339_opts(chrono::SecondsFormat::Millis, true);
        metadata.insert(
            ("activity".into(), activity),
            artex_store::ImportMetadata {
                legacy_key: Value::Null,
                created_at: timestamp.clone(),
                updated_at: timestamp,
            },
        );
    }
    let mut task_data = s.get("tasks", &task).unwrap().data.clone();
    task_data["paused"] = json!(true);
    p.put("tasks", &task, task_data);
    let mut batch = p.done(json!({}));
    batch.command_id = new_id("command");
    router
        .store
        .as_mut()
        .unwrap()
        .commit_import(batch, metadata)
        .unwrap();
    let sum = |rows: &Value| {
        rows.as_array()
            .unwrap()
            .iter()
            .map(|r| r["input_tokens"].as_u64().unwrap())
            .sum::<u64>()
    };
    let hot = ok(&mut router, "ui.usageStats", json!({"days":1}));
    assert_eq!(sum(&hot["daily"]), 3);
    assert_eq!(
        sum(&ok(&mut router, "ui.usageStats", json!({"days":0}))["daily"]),
        7
    );
    assert_eq!(
        sum(&ok(&mut router, "ui.usageStats", json!({}))["daily"]),
        7
    );
    assert_eq!(sum(&ok(&mut router, "ui.tokenDaily", json!({"days":1}))), 3);
    assert_eq!(sum(&ok(&mut router, "ui.tokenDaily", json!({"days":0}))), 7);
    let archive = ok(&mut router, "ui.archiveTask", json!({"id":task}))["id"]
        .as_str()
        .unwrap()
        .to_owned();
    drain(&mut router, &archive, false);
    let raw = router
        .snapshot()
        .unwrap()
        .get("task_archives", &archive)
        .unwrap()
        .data["aggregate_stats"]
        .clone();
    assert_eq!(
        raw["native_statistics_version"],
        artex_domain::auxiliary_ui::ARCHIVE_STATISTICS_VERSION
    );
    assert_eq!(raw["token_profiles"][0]["input_tokens"], 7);
    let detail = ok(&mut router, "ui.taskArchive", json!({"id":archive}));
    assert_eq!(detail["aggregate_stats"]["tokens"]["input_tokens"], 7);
    assert_eq!(detail["aggregate_stats"]["tokens"]["calls"], 3);
    assert_eq!(detail["aggregate_stats"]["tokens"]["cache_write_tokens"], 9);
    assert!(detail["aggregate_stats"].get("token_profiles").is_none());
    assert_eq!(ok(&mut router, "ui.taskArchives", json!({}))["total"], 1);
    assert_eq!(
        sum(&ok(&mut router, "ui.usageStats", json!({"days":0}))["daily"]),
        7
    );
    // Source cold archives merge complete UTC-day buckets, while live history
    // uses the precise rolling timestamp cutoff. Never discard either ledger.
    assert!(sum(&ok(&mut router, "ui.usageStats", json!({"days":1}))["daily"]) >= 3);
    reopen(&mut router, &dir.path().join("workspace"), true);
    assert_eq!(
        ok(&mut router, "ui.taskArchive", json!({"id":archive}))["aggregate_stats"],
        detail["aggregate_stats"]
    );
    let before = router.snapshot().unwrap();
    assert_eq!(
        request(
            &mut router,
            "ui.restoreTaskArchive",
            json!({"id":archive}),
            true
        )["error"]["data"]["code"],
        "RECOVERY_REQUIRED"
    );
    assert_eq!(before.collections, router.snapshot().unwrap().collections);
    drop(before);
    // A read-only handle cannot checkpoint. Reopen writable without issuing a repair.
    router.store.take();
    router = Router::new(BTreeMap::new());
    router.store = Some(Store::open(&dir.path().join("workspace")).unwrap());
    assert_eq!(
        router
            .snapshot()
            .unwrap()
            .get("task_archives", &archive)
            .unwrap()
            .data["aggregate_stats"],
        raw
    );
    assert_eq!(
        ok(&mut router, "ui.restoreTaskArchive", json!({"id":archive}))["state"],
        "restore_queued"
    );
    drain(&mut router, &archive, true);
    reopen(&mut router, &dir.path().join("workspace"), false);
    assert_eq!(ok(&mut router, "ui.taskArchives", json!({}))["total"], 0);
    assert_eq!(
        ok(&mut router, "ui.task", json!({"id":task}))["paused"],
        true
    );
    let restored = ok(&mut router, "ui.usageStats", json!({"days":1}));
    assert_eq!(sum(&restored["daily"]), 3);
    assert_eq!(restored["by_profile"][0]["input_tokens"], 7);
    assert_eq!(sum(&ok(&mut router, "ui.tokenDaily", json!({"days":1}))), 3);
}
#[test]
fn public_notification_creation_does_not_require_a_nonexistent_old_secret_but_update_remains_guarded()
 {
    let (dir, mut router) = fixture();
    // Source email.Validate requires delivery coordinates, not SMTP authentication.
    // A new channel has no existing credential that could be redirected.
    let config = json!({"host":"smtp.example.test","port":25,"from":"from@example.test","to":["to@example.test"]});
    let channel=ok(&mut router,"ui.notifyCreateChannel",json!({"payload":{"name":"Unauthenticated SMTP fixture","kind":"email","enabled":false,"config":config}}))["id"].as_str().unwrap().to_owned();
    ok(
        &mut router,
        "ui.notifyUpdateChannel",
        json!({"id":channel,"payload":{"name":"Renamed fixture"}}),
    );
    let channels = ok(&mut router, "ui.notifyChannels", json!({}));
    let row = channels
        .as_array()
        .unwrap()
        .iter()
        .find(|r| r["id"] == channel)
        .unwrap();
    assert_eq!(row["config"], config);
    assert_eq!(row["name"], "Renamed fixture");
    reopen(&mut router, &dir.path().join("workspace"), false);
    assert_eq!(ok(&mut router, "ui.notifyChannels", json!({})), channels);
    // Existing destinations keep the same explicit credential replacement guard.
    let secret = json!({"secret_ref":new_id("secret"),"hint":"fixture-only reference"});
    ok(
        &mut router,
        "ui.notifyUpdateChannel",
        json!({"id":channel,"payload":{"config":{"password":secret}}}),
    );
    for config in [
        json!({"host":"different.example.test"}),
        json!({"host":"different.example.test","password":"__masked__"}),
    ] {
        let before = router.snapshot().unwrap();
        assert_eq!(
            request(
                &mut router,
                "ui.notifyUpdateChannel",
                json!({"id":channel,"payload":{"config":config}}),
                true
            )["error"]["data"]["code"],
            "PERMISSION_DENIED"
        );
        assert_eq!(before.collections, router.snapshot().unwrap().collections);
    }
    ok(
        &mut router,
        "ui.notifyUpdateChannel",
        json!({"id":channel,"payload":{"config":{"host":"different.example.test","password":""}}}),
    );
    let before = router.snapshot().unwrap();
    assert_eq!(
        request(
            &mut router,
            "ui.notifyCreateChannel",
            json!({"payload":{"name":"Invalid mask fixture","kind":"email","enabled":false,"config":{"host":"smtp.example.test","port":25,"from":"from@example.test","to":["to@example.test"],"password":"__masked__"}}}),
            true
        )["error"]["data"]["code"],
        "INVALID_PARAMS"
    );
    assert_eq!(before.collections, router.snapshot().unwrap().collections);
    reopen(&mut router, &dir.path().join("workspace"), false);
    let channels = ok(&mut router, "ui.notifyChannels", json!({}));
    let row = channels
        .as_array()
        .unwrap()
        .iter()
        .find(|r| r["id"] == channel)
        .unwrap();
    assert_eq!(row["config"]["host"], "different.example.test");
    assert_eq!(row["config"]["password"], "");
}

#[test]
fn broad_public_crud_preserves_business_values_and_reopen() {
    let (dir, mut router) = fixture();
    let root = dir.path().join("workspace");
    // Every step crosses the public schema/admission boundary, including CAS.
    let steps: Value = serde_json::from_str(
        r###"[
 {"method":"ui.createCompany","params":{"name":"Fixture company","scope":[]},"save":{"company":"id"}},
 {"method":"ui.addCompanyScope","params":{"id":"@company","scope":[{"kind":"domain","value":"fixture.example"}],"reason":"fixture"}},
 {"method":"ui.updateCompanyScope","params":{"id":"@company","scope":[{"kind":"domain","value":"fixture.example"}],"reason":"updated"}},
 {"method":"ui.companies","params":{},"capture":"companies","assert":{"0.name":"Fixture company"}},
 {"method":"ui.createTask","params":{"input":{"name":"Broad CRUD fixture","description":"Broad description","goal":"Local fixture only"}},"save":{"task":"id"}},
 {"method":"ui.updateTask","params":{"id":"@task","input":{"name":"Updated fixture","pinned":true}}},
 {"method":"ui.renameTask","params":{"id":"@task","name":"Renamed fixture"},"assert":{"name":"Renamed fixture"}},
 {"method":"ui.registerTaskAssetScopes","params":{"taskId":"@task","scope":[{"kind":"domain","value":"fixture.example"}]},"assert":{"assets_linked":1}},
 {"method":"ui.assets","params":{"limit":50,"offset":0,"type":""},"save":{"asset":"assets.0.id"},"capture":"assets","assert":{"total":1,"assets.0.domain":"fixture.example","assets.0.company_id":"@company","assets.0.task_ids.0":"@task"}},
 {"method":"ui.taskAssets","params":{"taskId":"@task","type":"","limit":50,"offset":0},"capture":"taskAssets","assert":{"total":1,"assets.0.id":"@asset","assets.0.task_ids.0":"@task"}},
 {"method":"ui.assetCounts","params":{},"capture":"assetCounts"},
 {"method":"ui.createTaskCategory","params":{"name":"Fixture category"},"save":{"category":"id"}},
 {"method":"ui.renameTaskCategory","params":{"id":"@category","name":"Updated category"}},
 {"method":"ui.updateTaskCategory","params":{"taskId":"@task","categoryId":"@category"}},
 {"method":"ui.taskCategories","params":{},"capture":"categories","assert":{"0.name":"Updated category","0.task_count":1}},
 {"method":"ui.createTaskTemplate","params":{"input":{"name":"Fixture template","description":"Fixture description","goal":"Fixture goal","category_id":"@category","intercept_rules":[]}},"save":{"template":"id"}},
 {"method":"ui.updateTaskTemplate","params":{"id":"@template","input":{"name":"Updated template","description":"Fixture description","goal":"Fixture goal","category_id":"@category","intercept_rules":[]}}},
 {"method":"ui.taskTemplates","params":{},"capture":"templates","assert":{"0.name":"Updated template","0.category_id":"@category"}},
 {"method":"ui.addGoal","params":{"id":"@task","text":"Fixture goal","vulnclass":""},"save":{"goal":"id"}},
 {"method":"ui.updateGoal","params":{"id":"@task","goalId":"@goal","text":"Updated goal","vulnclass":""}},
 {"method":"ui.taskGoals","params":{"id":"@task"},"capture":"goals","assert":{"goals.0.text":"Updated goal","goals.0.id":"@goal"}},
 {"method":"ui.addConstraint","params":{"id":"@task","text":"Fixture constraint","kind":"deny"},"save":{"constraint":"id"}},
 {"method":"ui.updateConstraint","params":{"id":"@task","constraintId":"@constraint","text":"Updated constraint","kind":"allow"}},
 {"method":"ui.taskConstraints","params":{"id":"@task"},"capture":"constraints","assert":{"constraints.0.text":"Updated constraint","constraints.0.kind":"allow"}},
 {"method":"ui.createAgent","params":{"key":"fixture_custom","name":"Fixture agent","description":"local"}},
 {"method":"ui.updateAgent","params":{"key":"fixture_custom","name":"Updated agent","description":"updated"}},
 {"method":"ui.agents","params":{},"capture":"agents","assert":{"0.key":"fixture_custom","0.name":"Updated agent","0.description":"updated"}},
 {"method":"ui.setSettings","params":{"patch":{"approval_bypass":false}}},
 {"method":"ui.settings","params":{},"capture":"settings","assert":{"approval_bypass":false}},
 {"method":"ui.createInterceptRule","params":{"rule":{"name":"Fixture rule","enabled":false,"priority":0,"match_target":"tool_name","match_type":"string","pattern":"FixtureTool","action":"deny","message":"local fixture","timeout_enabled":false,"timeout_seconds":0,"timeout_action":"deny"}},"save":{"rule":"id"}},
 {"method":"ui.updateInterceptRule","params":{"id":"@rule","rule":{"name":"Updated rule","enabled":false,"priority":0,"match_target":"tool_name","match_type":"string","pattern":"FixtureTool","action":"deny","message":"local fixture","timeout_enabled":false,"timeout_seconds":0,"timeout_action":"deny"}}},
 {"method":"ui.interceptRules","params":{},"capture":"rules","assert":{"0.name":"Updated rule","0.pattern":"FixtureTool","0.enabled":false}},
 {"method":"ui.notifyCreateChannel","params":{"payload":{"name":"Fixture notification","kind":"email","enabled":false,"mode":"realtime","config":{"host":"smtp.example.test","port":25,"from":"from@example.test","to":["to@example.test"]},"filter":{},"rate_per_min":0}},"save":{"channel":"id"}},
 {"method":"ui.notifyUpdateChannel","params":{"id":"@channel","payload":{"name":"Updated notification","kind":"email","enabled":false,"mode":"realtime","config":{"host":"smtp.example.test","password":""},"filter":{},"rate_per_min":0}}},
 {"method":"ui.notifyUpdateChannel","params":{"id":"@channel","payload":{"name":"Updated notification"}}},
 {"method":"ui.notifyChannels","params":{},"capture":"channels","assert":{"0.name":"Updated notification","0.enabled":false,"0.config.host":"smtp.example.test"}},
 {"method":"ui.task","params":{"id":"@task"},"capture":"task","assert":{"name":"Renamed fixture","description":"Broad description","category_id":"@category"}},
 {"method":"ui.companies","params":{},"capture":"companies"},
 {"method":"ui.assets","params":{"limit":50,"offset":0,"type":""},"capture":"assets"},
 {"method":"ui.taskAssets","params":{"taskId":"@task","type":"","limit":50,"offset":0},"capture":"taskAssets","assert":{"total":1,"assets.0.id":"@asset","assets.0.task_ids.0":"@task"}},
 {"method":"ui.assetCounts","params":{},"capture":"assetCounts"},
 {"method":"ui.taskCategories","params":{},"capture":"categories"},
 {"method":"ui.taskTemplates","params":{},"capture":"templates"},
 {"method":"ui.taskGoals","params":{"id":"@task"},"capture":"goals","assert":{"goals.0.text":"Updated goal","goals.0.id":"@goal"}},
 {"method":"ui.taskConstraints","params":{"id":"@task"},"capture":"constraints"},
 {"method":"ui.agents","params":{},"capture":"agents","assert":{"0.key":"fixture_custom","0.name":"Updated agent","0.description":"updated"}},
 {"method":"ui.settings","params":{},"capture":"settings","assert":{"approval_bypass":false}},
 {"method":"ui.interceptRules","params":{},"capture":"rules","assert":{"0.name":"Updated rule","0.pattern":"FixtureTool","0.enabled":false}},
 {"method":"ui.notifyChannels","params":{},"capture":"channels","assert":{"0.name":"Updated notification","0.enabled":false,"0.config.host":"smtp.example.test"}},
 {"method":"ui.task","params":{"id":"@task"},"capture":"task"},
 {"reopen":true},
 {"method":"ui.companies","params":{},"same":"companies"},
 {"method":"ui.task","params":{"id":"@task"},"same":"task"},
 {"method":"ui.taskCategories","params":{},"same":"categories"},
 {"method":"ui.taskTemplates","params":{},"same":"templates"},
 {"method":"ui.taskGoals","params":{"id":"@task"},"same":"goals"},
 {"method":"ui.taskConstraints","params":{"id":"@task"},"same":"constraints"},
 {"method":"ui.agents","params":{},"same":"agents"},
 {"method":"ui.settings","params":{},"same":"settings"},
 {"method":"ui.interceptRules","params":{},"same":"rules"},
 {"method":"ui.notifyChannels","params":{},"same":"channels"},
 {"method":"ui.assets","params":{"limit":50,"offset":0,"type":""},"same":"assets"},
 {"method":"ui.taskAssets","params":{"taskId":"@task","type":"","limit":50,"offset":0},"same":"taskAssets"},
 {"method":"ui.assetCounts","params":{},"same":"assetCounts"},
 {"method":"ui.renameTask","params":{"id":"@task","name":"must not commit"},"without_cas":true,"error":"VERSION_CONFLICT"},
 {"method":"ui.createCompany","params":{"name":"bad scope","scope":[{"kind":"cidr","value":"not/cidr"}]},"save":{"badCompany":"id"},"assert":{"scope_invalid":1,"scope_added":0}},
 {"method":"ui.deleteCompany","params":{"id":"@badCompany","deleteAssets":false}},
 {"method":"ui.renameTask","params":{"id":"task_00000000000000000000000000000000","name":"Missing task"},"error":"NOT_FOUND"},
 {"method":"ui.notifyDeleteChannel","params":{"id":"@channel"}},
 {"method":"ui.deleteInterceptRule","params":{"id":"@rule"}},
 {"method":"ui.deleteAgent","params":{"key":"fixture_custom"}},
 {"method":"ui.deleteGoal","params":{"id":"@task","goalId":"@goal"}},
 {"method":"ui.deleteConstraint","params":{"id":"@task","constraintId":"@constraint"}},
 {"method":"ui.deleteTaskTemplate","params":{"id":"@template"}},
 {"method":"ui.deleteTaskCategory","params":{"id":"@category"}},
 {"method":"ui.deleteCompany","params":{"id":"@company","deleteAssets":false}},
 {"method":"ui.deleteAssets","params":{"ids":["@asset"]}},
 {"reopen":true},
 {"method":"ui.companies","params":{},"assert":{"":[]}},
 {"method":"ui.taskTemplates","params":{},"assert":{"":[]}},
 {"method":"ui.taskCategories","params":{},"assert":{"":[]}},
 {"method":"ui.notifyChannels","params":{},"assert":{"":[]}},
 {"method":"ui.taskConstraints","params":{"id":"@task"},"assert":{"constraints":[]}},
 {"method":"ui.assets","params":{"limit":50,"offset":0,"type":""},"assert":{"total":0,"assets":[]}},
 {"method":"ui.task","params":{"id":"@task"},"assert":{"name":"Renamed fixture","description":"Broad description"}}
]"###,
    )
    .unwrap();
    let mut vars = BTreeMap::<String, Value>::new();
    let mut reads = BTreeMap::<String, Value>::new();
    fn expand(v: &Value, vars: &BTreeMap<String, Value>) -> Value {
        match v {
            Value::String(s) if s.starts_with('@') => vars
                .get(&s[1..])
                .unwrap_or_else(|| panic!("Unknown fixture variable {s}"))
                .clone(),
            Value::Array(a) => Value::Array(a.iter().map(|v| expand(v, vars)).collect()),
            Value::Object(o) => Value::Object(
                o.iter()
                    .map(|(k, v)| (k.clone(), expand(v, vars)))
                    .collect(),
            ),
            _ => v.clone(),
        }
    }
    fn part<'a>(v: &'a Value, path: &str) -> &'a Value {
        if path.is_empty() {
            return v;
        }
        path.split('.').fold(v, |v, k| {
            if let Ok(i) = k.parse::<usize>() {
                &v[i]
            } else {
                &v[k]
            }
        })
    }
    for (index, step) in steps.as_array().unwrap().iter().enumerate() {
        if step["reopen"] == true {
            reopen(&mut router, &root, false);
            println!("PHASE checkpoint_reopen step={index}");
            continue;
        }
        let method = step["method"].as_str().unwrap();
        let params = expand(&step["params"], &vars);
        let before = router.snapshot().unwrap();
        println!("PHASE step={index} {method}");
        let response = request(&mut router, method, params, step["without_cas"] != true);
        if let Some(code) = step["error"].as_str() {
            assert_eq!(
                response["error"]["data"]["code"], code,
                "step {index}: {response}"
            );
            assert_eq!(before.collections, router.snapshot().unwrap().collections);
            continue;
        }
        assert!(response.get("error").is_none(), "step {index}: {response}");
        let result = &response["result"];
        artex_core::validate_contract(
            &router.contract["results"][method],
            &router.contract["$defs"],
            result,
        )
        .unwrap();
        if let Some(save) = step["save"].as_object() {
            for (k, p) in save {
                let value = part(result, p.as_str().unwrap());
                assert!(!value.is_null(), "Cannot save {k}");
                vars.insert(k.clone(), value.clone());
            }
        }
        if let Some(check) = step["assert"].as_object() {
            for (path, expected) in check {
                assert_eq!(
                    part(result, path),
                    &expand(expected, &vars),
                    "step {index} {method} path {path}"
                );
            }
        }
        if let Some(name) = step["capture"].as_str() {
            reads.insert(name.into(), result.clone());
        }
        if let Some(name) = step["same"].as_str() {
            assert_eq!(result, &reads[name], "Reopen changed {name}");
        }
    }
}
#[test]
fn public_graph_edges_project_source_endpoints_and_nonempty_lineage_through_reopen() {
    let (dir, mut router) = fixture();
    let task=ok(&mut router,"ui.createTask",json!({"input":{"description":"Graph fixture","goal":"Local graph","seedFirstIntent":true}}))["id"].as_str().unwrap().to_owned();
    let goal = ok(
        &mut router,
        "ui.addGoal",
        json!({"id":task,"text":"Graph goal"}),
    )["id"]
        .as_str()
        .unwrap()
        .to_owned();
    let s = router.snapshot().unwrap();
    let exp = s.get("tasks", &task).unwrap().data["exploration_id"].clone();
    let intent = s
        .records("exploration_nodes")
        .into_iter()
        .find(|n| n.data["exploration_id"] == exp && n.data["kind"] == "intent")
        .unwrap()
        .header
        .id
        .clone();
    let mut p = Planner::new(&s, "fixture.graph.agent_run", &json!({}));
    let agent = p
        .insert("agents", "agent", json!({"key":"mainagent","enabled":true}))
        .unwrap();
    let run=p.insert("agent_runs","run",json!({"agent_id":agent,"task_id":task,"intent_id":intent,"engine_epoch":router.engine_epoch,"state":"running"})).unwrap();
    p.insert(
        "tools",
        "toolconfig",
        json!({"key":"report_finding","system":true,"enabled":true,"agents":["mainagent"]}),
    )
    .unwrap();
    router.internal_commit(p.done(json!({}))).unwrap();
    // Only the execution owner is fixture setup. Finding/node/edge creation
    // crosses the actual public tool contract and writes its invocation ledger.
    let context =
        artex_agent::context_for_run(&router.snapshot().unwrap(), &run, &router.engine_epoch)
            .unwrap();
    let s = router.snapshot().unwrap();
    let versions=s.collections.values().flat_map(|rows|rows.values()).map(|row|json!({"collection":row.header.collection,"id":row.header.id,"version":row.header.version})).collect::<Vec<_>>();
    let params = json!({"run_id":run,"tool_use_id":new_id("tooluse"),"name":"report_finding","input":{"severity":"low","summary":"Graph finding","vulnclass":"fixture","evidence":"Local fixture only"},"raw_input":null});
    let reply=router.response_as(json!({"jsonrpc":"2.0","protocol":"artex-rpc/1.0","id":new_id("request"),"method":"tool.invoke","workspace_id":s.workspace_id,"engine_epoch":router.engine_epoch,"command_id":new_id("command"),"expected":versions,"timeout_ms":5000,"params":params}),Some(&context));
    println!(
        "E2E {}",
        json!({"method":"tool.invoke","params":params,"response":reply})
    );
    assert!(reply.get("error").is_none(), "{reply}");
    assert_eq!(reply["result"]["state"], "completed");
    assert_eq!(reply["result"]["result"]["is_error"], false);
    let finding = router.snapshot().unwrap().records("findings")[0]
        .header
        .id
        .clone();
    let raw = router
        .snapshot()
        .unwrap()
        .records("exploration_edges")
        .into_iter()
        .map(|e| e.data.clone())
        .collect::<Vec<_>>();
    assert!(raw.iter().any(|e| e["dst_id"] == goal));
    let mut expected = raw
        .iter()
        .map(|e| json!({"src":e["src_id"],"dst":e["dst_id"],"rel":e["rel"]}))
        .collect::<Vec<_>>();
    expected.sort_by_key(Value::to_string);
    let mut before = BTreeMap::new();
    for method in [
        "ui.explorationGraph",
        "ui.explorationNodes",
        "ui.findingLineage",
    ] {
        let params = if method == "ui.findingLineage" {
            json!({"id":finding})
        } else {
            json!({"task":task})
        };
        let value = ok(&mut router, method, params);
        let edges = value["edges"].as_array().unwrap();
        assert!(!edges.is_empty());
        for edge in edges {
            assert_eq!(edge.as_object().unwrap().len(), 3);
            assert!(edge["src"].as_str().unwrap().starts_with("node_"));
            assert!(edge["dst"].as_str().unwrap().starts_with("node_"));
        }
        if method != "ui.findingLineage" {
            let mut actual = edges.clone();
            actual.sort_by_key(Value::to_string);
            assert_eq!(actual, expected);
        } else {
            assert!(
                edges
                    .iter()
                    .any(|edge| edge["src"] == intent && edge["rel"] == "yields")
            );
        }
        before.insert(method, value);
    }
    assert_eq!(
        router
            .snapshot()
            .unwrap()
            .records("exploration_edges")
            .into_iter()
            .map(|e| e.data.clone())
            .collect::<Vec<_>>(),
        raw
    );
    // Mentions share the same three actual native record identities. The asset
    // selector is an input alias; its returned kind remains the source subtype.
    let company = ok(
        &mut router,
        "ui.createCompany",
        json!({"name":"Graph mention company","scope":[]}),
    )["id"]
        .as_str()
        .unwrap()
        .to_owned();
    ok(
        &mut router,
        "ui.registerTaskAssetScopes",
        json!({"taskId":task,"scope":[{"kind":"domain","value":"mention.example"}]}),
    );
    let asset = ok(&mut router, "ui.taskAssets", json!({"taskId":task}))["assets"][0]["id"]
        .as_str()
        .unwrap()
        .to_owned();
    let mut mention_results = vec![];
    for (kind, id, expected_kind) in [
        ("company", company.as_str(), "company"),
        ("finding", finding.as_str(), "finding"),
        ("asset", asset.as_str(), "root_domain"),
    ] {
        let params = json!({"kind":kind,"query":id});
        let result = ok(&mut router, "ui.chatMentions", params.clone());
        assert_eq!(result["items"].as_array().unwrap().len(), 1);
        assert_eq!(result["items"][0]["id"], id);
        assert_eq!(result["items"][0]["kind"], expected_kind);
        for wrong in [
            json!(new_id("task")),
            json!(new_id("profile")),
            json!("company_bad"),
            json!(42),
            json!(""),
        ] {
            let mut invalid = result.clone();
            invalid["items"][0]["id"] = wrong;
            assert!(
                artex_core::validate_contract(
                    &router.contract["results"]["ui.chatMentions"],
                    &router.contract["$defs"],
                    &invalid
                )
                .is_err()
            );
        }
        let mut wrong_kind = result.clone();
        wrong_kind["items"][0]["id"] = json!(if kind == "company" { &asset } else { &company });
        assert!(
            artex_core::validate_contract(
                &router.contract["results"]["ui.chatMentions"],
                &router.contract["$defs"],
                &wrong_kind
            )
            .is_err()
        );
        mention_results.push((params, result));
    }
    for index in 0..21 {
        ok(
            &mut router,
            "ui.createCompany",
            json!({"name":format!("Paging fixture company {index}"),"scope":[]}),
        );
    }
    let first_params = json!({"kind":"company","query":"Paging fixture company"});
    let first = ok(&mut router, "ui.chatMentions", first_params.clone());
    assert_eq!(first["items"].as_array().unwrap().len(), 20);
    let cursor = first["next_cursor"].as_str().unwrap();
    let second_params = json!({"kind":"company","query":"Paging fixture company","cursor":cursor});
    let second = ok(&mut router, "ui.chatMentions", second_params.clone());
    assert_eq!(second["items"].as_array().unwrap().len(), 1);
    assert!(second["next_cursor"].is_null());
    let mut unique = BTreeSet::new();
    for page in [&first, &second] {
        for item in page["items"].as_array().unwrap() {
            assert!(unique.insert(item["id"].as_str().unwrap().to_owned()));
        }
    }
    assert_eq!(unique.len(), 21);
    mention_results.push((first_params, first));
    mention_results.push((second_params, second));
    reopen(&mut router, &dir.path().join("workspace"), false);
    for method in [
        "ui.explorationGraph",
        "ui.explorationNodes",
        "ui.findingLineage",
    ] {
        let params = if method == "ui.findingLineage" {
            json!({"id":finding})
        } else {
            json!({"task":task})
        };
        assert_eq!(ok(&mut router, method, params), before[method]);
    }
    assert_eq!(
        router
            .snapshot()
            .unwrap()
            .records("exploration_edges")
            .into_iter()
            .map(|e| e.data.clone())
            .collect::<Vec<_>>(),
        raw
    );
    for (params, expected) in mention_results {
        assert_eq!(ok(&mut router, "ui.chatMentions", params), expected);
    }
}
