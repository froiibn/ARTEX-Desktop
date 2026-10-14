use crate::*;

fn sources(p: &Value, s: &Snapshot, own: Option<&str>) -> Result<Vec<String>> {
    let src = ids(p, "source_task_ids");
    if src.len() > 8 {
        return Err(Error::new(
            "INVALID_PARAMS",
            "maximum 8 direct source tasks",
        ));
    }
    let mut seen = BTreeSet::new();
    for id in &src {
        if own == Some(id) || !seen.insert(id) {
            return Err(Error::new("INVALID_PARAMS", "duplicate/self source task"));
        }
        let r = record(s, "tasks", id)?;
        if !r.data["deleted_at"].is_null() || !r.data["archived_at"].is_null() {
            return Err(Error::new(
                "INVALID_PARAMS",
                "source task is deleted or archived",
            ));
        }
    }
    Ok(src)
}
pub fn plan(m: &str, p: &Value, s: &Snapshot, c: &Context) -> Result<CommandBatch> {
    let mut b = Planner::new(s, m, p);
    if matches!(m, "task.create" | "spawn_task") {
        let goal = if m == "spawn_task" {
            required(p, "goal")?
        } else {
            text(p, "goal")
        };
        let desc = if text(p, "description").trim().is_empty() {
            "Unnamed task"
        } else {
            text(p, "description")
        };
        let src = sources(p, s, None)?;
        let mut profiles = ids(p, "llm_profile_ids");
        if profiles.is_empty() {
            if let Some(id) = p["llm_profile_id"].as_str() {
                profiles.push(id.into());
            }
        }
        let mut seen_profiles = BTreeSet::new();
        for id in &profiles {
            if !seen_profiles.insert(id) {
                return Err(Error::new("INVALID_PARAMS", "Duplicate LLM profile"));
            }
            let r = record(s, "llm_profiles", id)?;
            if r.data["api_key"].is_null() || text(&r.data, "model").is_empty() {
                return Err(Error::new(
                    "INVALID_PARAMS",
                    "Task profile is not configured",
                ));
            }
        }
        let companies: Vec<String> = ids(p, "company_ids")
            .into_iter()
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect();
        if companies.len() > 32 {
            return Err(Error::new(
                "INVALID_PARAMS",
                "Maximum 32 associated companies",
            ));
        }
        for id in &companies {
            record(s, "companies", id)?;
        }
        let rules = p["intercept_rules"].as_array().cloned().unwrap_or_default();
        for rule in &rules {
            validate_asset_rule(rule, true)?;
        }
        let parent = text(p, "parent_ref");
        if !parent.is_empty() {
            record(s, "tasks", parent)?;
        }
        let exp = b.insert(
            "explorations",
            "exploration",
            json!({"description":desc,"goal":goal,"status":"open","round_no":"0"}),
        )?;
        let heartbeat = p["plan_heartbeat_seconds"].as_i64().unwrap_or(0).max(600);
        let timeout = p["timeout_seconds"].as_i64().unwrap_or(0).max(0);
        let mut patch = json!({"description":desc,"goal":goal,"exploration_id":exp,"status":"created","plan_heartbeat_seconds":heartbeat,"timeout_seconds":timeout});
        if let Some(name) = p.get("name") {
            patch["name"] = name.clone();
        }
        if let Some(coverage) = p.get("coverage_enabled") {
            patch["coverage_enabled"] = coverage.clone();
        }
        if !parent.is_empty() {
            patch["parent_ref"] = json!(parent);
        }
        for field in ["company_id", "category_id", "llm_profile_id"] {
            if !p[field].is_null() {
                let collection = match field {
                    "company_id" => "companies",
                    "category_id" => "task_categories",
                    _ => "llm_profiles",
                };
                record(s, collection, required(p, field)?)?;
                patch[field] = p[field].clone();
            }
        }
        if p["llm_profile_id"].is_null() && !parent.is_empty() {
            patch["llm_profile_id"] = record(s, "tasks", parent)?.data["llm_profile_id"].clone();
        }
        let id = b.insert("tasks", "task", patch)?;
        if let Some(profile) = profiles.first() {
            let mut d = b.data("tasks", &id)?;
            d["llm_profile_id"] = json!(profile);
            d["active_llm_profile_id"] = json!(profile);

            b.put("tasks", &id, d);
        }
        for (position, profile) in profiles.iter().enumerate() {
            b.insert(
                "task_llm_profiles",
                "profilelink",
                json!({"task_id":id,"profile_id":profile,"position":position,"status":"ready"}),
            )?;
        }
        for company in &companies {
            b.insert("task_scope","scope",json!({"task_id":id,"kind":"company","company_id":company,"source":"manual","reason":"Associate the company when creating the task"}))?;
            for asset in s.records("assets") {
                if asset.data["company_id"] == company.as_str() {
                    let mut d = asset.data.clone();
                    let mut taskids = ids(&d, "task_ids");
                    if !taskids.contains(&id) {
                        taskids.push(id.clone());
                    }
                    d["task_ids"] = json!(taskids);
                    b.put("assets", &asset.header.id, d);
                    b.insert("task_asset_links","assetlink",json!({"task_id":id,"asset_id":asset.header.id,"source":"company","source_summary":format!("Associated company at task creation: {}",text(&record(s,"companies",company)?.data,"name"))}))?;
                }
            }
        }
        for rule in rules {
            let mut r = validate_asset_rule(&rule, true)?;
            r["task_id"] = json!(id);
            b.insert("task_intercept_rules", "taskrule", r)?;
        }
        for source in src {
            b.insert(
                "task_relations",
                "relation",
                json!({"task_id":id,"source_task_id":source}),
            )?;
        }
        b.insert("exploration_nodes","node",json!({"exploration_id":exp,"kind":"begin","state":"open","payload":{"description":desc}}))?;
        let origin=b.insert("exploration_nodes","node",json!({"exploration_id":exp,"kind":"fact","state":"origin","payload":{"summary":desc,"detail":goal}}))?;
        if p["seed_first_intent"] == true {
            let n=b.insert("exploration_nodes","node",json!({"exploration_id":exp,"kind":"intent","state":"open","priority":5,"payload":{"summary":format!("{desc} {goal}")}}))?;
            b.insert(
                "exploration_edges",
                "edge",
                json!({"exploration_id":exp,"src_id":origin,"dst_id":n,"rel":"spawns"}),
            )?;
        }
        let runtime = json!({"task_id":id,"timeout_policy":"active_elapsed","active_elapsed_ms":"0","remaining_timeout_ms":if timeout==0{Value::Null}else{json!((timeout as u64*1000).to_string())},"last_active_clock":null,"legacy_deadline_at":null,"accounting_state":"known"});
        b.insert("task_runtime", "taskruntime", runtime)?;
        if m == "spawn_task" {
            let limit = crate::configuration_ui::setting(s, "task_concurrency_limit")
                .parse::<usize>()
                .unwrap_or(5)
                .max(1);
            let running = s
                .records("tasks")
                .iter()
                .filter(|t| {
                    t.data["status"] == "running"
                        && t.data["queued"] != true
                        && t.data["paused"] != true
                        && t.data["deleted_at"].is_null()
                        && t.data["archived_at"].is_null()
                })
                .count();
            let queued = setting_bool(s, "task_concurrency_enabled", false) && running >= limit;
            let mut task = b.data("tasks", &id)?;
            task["status"] = json!(if queued { "created" } else { "running" });
            task["paused"] = json!(false);
            task["queued"] = json!(queued);
            task["queue_mode"] = json!("bootstrap");
            task["queued_at"] = if queued {
                json!(artex_core::now())
            } else {
                Value::Null
            };
            b.put("tasks", &id, task);
            for row in s.records("settings") {
                if matches!(
                    text(&row.data, "key"),
                    "task_concurrency_limit" | "task_concurrency_enabled"
                ) {
                    b.expect(row);
                }
            }
            conversation::push_event(&mut b.batch, s, c, "TaskCreated", "tasks", &id)?;
            return Ok(b.done(json!({"task_id":id,"exploration_id":exp,"status":if queued{"created"}else{"running"},"execution_scheduled":true})));
        }
        return Ok(b.done(json!({"task_id":id,"exploration_id":exp,"status":"created","execution_scheduled":false})));
    }
    let id = task_id(p, c)?;
    let mut d = record(s, "tasks", id)?.data.clone();
    match m {
        "task.recover_budget" => {
            if c.actor.kind != "user" {
                return Err(Error::new(
                    "PERMISSION_DENIED",
                    "Only a user can confirm recovered task time",
                ));
            }
            if !d["deleted_at"].is_null() || !d["archived_at"].is_null() {
                return Err(Error::new("NOT_FOUND", "Task is not live"));
            }
            if !matches!(text(&d, "status"), "paused" | "created") {
                return Err(Error::new(
                    "BUSY",
                    "Pause the task before confirming recovered time",
                ));
            }
            let runtime = s
                .records("task_runtime")
                .into_iter()
                .find(|r| r.data["task_id"] == id)
                .ok_or_else(|| Error::new("NOT_FOUND", "Task runtime is missing"))?;
            if runtime.data["accounting_state"] != "recovery_required" {
                return Err(Error::new(
                    "ALREADY_COMPLETED",
                    "Task time has already been confirmed",
                ));
            }
            let recorded = text(&runtime.data, "active_elapsed_ms")
                .parse::<u64>()
                .map_err(|_| Error::new("CORRUPT_STORE", "Invalid saved task elapsed time"))?;
            let elapsed = match required(p, "mode")? {
                "use_recorded" => {
                    if !p["elapsed_active_ms"].is_null() {
                        return Err(Error::new(
                            "INVALID_PARAMS",
                            "Recorded mode does not accept an override",
                        ));
                    }
                    recorded
                }
                "override" => required(p, "elapsed_active_ms")?
                    .parse::<u64>()
                    .map_err(|_| {
                        Error::new(
                            "INVALID_PARAMS",
                            "Elapsed time must be an unsigned decimal string",
                        )
                    })?,
                _ => {
                    return Err(Error::new(
                        "INVALID_PARAMS",
                        "Recovery mode must be use_recorded or override",
                    ));
                }
            };
            let timeout = d["timeout_seconds"]
                .as_u64()
                .unwrap_or(0)
                .checked_mul(1000)
                .ok_or_else(|| {
                    Error::new("SIZE_LIMIT", "Task timeout exceeds the supported range")
                })?;
            if elapsed < recorded || (timeout > 0 && elapsed > timeout) {
                return Err(Error::new(
                    "INVALID_PARAMS",
                    "Confirmed elapsed time must be at least the saved elapsed time and within the task timeout",
                ));
            }
            let mut next = runtime.data.clone();
            next["active_elapsed_ms"] = json!(elapsed.to_string());
            next["remaining_timeout_ms"] = if timeout == 0 {
                Value::Null
            } else {
                json!((timeout - elapsed).to_string())
            };
            next["accounting_state"] = json!("known");
            next["last_active_clock"] = Value::Null;
            // Imported absolute deadlines remain authoritative; confirmation cannot extend them.
            b.put("task_runtime", &runtime.header.id, next.clone());
            conversation::push_event(&mut b.batch, s, c, "TaskBudgetRecovered", "tasks", id)?;
            let mut out=b.done(json!({"task_id":id,"accounting_state":"known","active_elapsed_ms":next["active_elapsed_ms"],"remaining_timeout_ms":next["remaining_timeout_ms"]}));
            augment_expected(&mut out, s)?;
            return Ok(out);
        }
        "task.sources.replace" => {
            let src = sources(p, s, Some(id))?;
            for r in s.records("task_relations") {
                if text(&r.data, "task_id") == id {
                    b.delete("task_relations", &r.header.id);
                }
            }
            for source in src {
                b.insert(
                    "task_relations",
                    "relation",
                    json!({"task_id":id,"source_task_id":source}),
                )?;
            }
        }
        "task.pause" | "pause_task" => {
            if matches!(text(&d, "status"), "done" | "failed" | "timeout") {
                return Err(Error::new(
                    "INVALID_PARAMS",
                    "terminal task cannot be paused",
                ));
            }
            d["paused"] = json!(true);
            d["queued"] = json!(false);
            d["status"] = json!("paused");
            b.put("tasks", id, d);
            for runtime in s.records("task_runtime") {
                if runtime.data["task_id"] == id {
                    let mut rd = runtime.data.clone();
                    rd["last_active_clock"] = Value::Null;
                    b.put("task_runtime", &runtime.header.id, rd);
                }
            }
            for r in s.records("agent_runs") {
                if text(&r.data, "task_id") == id
                    && matches!(text(&r.data, "state"), "running" | "queued" | "waiting")
                {
                    conversation::cancel_run(&mut b, c, &r.header.id)?;
                    if let Some(intent) = r.data["intent_id"].as_str() {
                        if let Some(node) = s.get("exploration_nodes", intent) {
                            let mut nd = node.data.clone();
                            if matches!(text(&nd, "state"), "running" | "claimed" | "active") {
                                nd["state"] = json!("paused");
                                b.put("exploration_nodes", intent, nd);
                            }
                        }
                    }
                }
            }
        }
        "task.resume" => {
            if !matches!(text(&d, "status"), "paused" | "created") {
                return Err(Error::new(
                    "INVALID_PARAMS",
                    "Only paused or newly created tasks may be admitted",
                ));
            }
            // Resume is an explicit admission of new work. Interrupted runs belong
            // to the previous engine lifetime and are retired without replaying
            // their provider requests or uncertain host invocations.
            for run in s.records("agent_runs") {
                if run.data["task_id"] == id && run.data["state"] == "interrupted" {
                    let mut rd = run.data.clone();
                    rd["state"] = json!("cancelled");
                    rd["finished_at"] = json!(artex_core::now());
                    b.put("agent_runs", &run.header.id, rd);
                    conversation::push_event(
                        &mut b.batch,
                        s,
                        c,
                        "RunCancelled",
                        "agent_runs",
                        &run.header.id,
                    )?;
                }
            }
            let runtime = s
                .records("task_runtime")
                .into_iter()
                .find(|r| r.data["task_id"] == id);
            if let Some(runtime) = runtime {
                if runtime.data["accounting_state"] == "recovery_required" {
                    return Err(Error::new(
                        "RECOVERY_REQUIRED",
                        "Task elapsed time requires recovery confirmation",
                    ));
                }
                if runtime.data["remaining_timeout_ms"] == "0" {
                    return Err(Error::new(
                        "CANCELLED",
                        "Task active time budget is exhausted",
                    ));
                }
                if runtime.data["timeout_policy"] == "legacy_absolute"
                    && runtime.data["legacy_deadline_at"]
                        .as_str()
                        .is_some_and(|deadline| deadline <= artex_core::now().as_str())
                {
                    return Err(Error::new(
                        "CANCELLED",
                        "Imported task absolute deadline has expired",
                    ));
                }
            }
            let limit = configuration_ui::setting(s, "task_concurrency_limit")
                .parse::<usize>()
                .unwrap_or(5)
                .max(1);
            let running = s
                .records("tasks")
                .iter()
                .filter(|t| {
                    t.data["status"] == "running"
                        && t.data["queued"] != true
                        && t.data["paused"] != true
                        && t.data["deleted_at"].is_null()
                        && t.data["archived_at"].is_null()
                })
                .count();
            let queued = setting_bool(s, "task_concurrency_enabled", false) && running >= limit;
            d["paused"] = json!(false);
            d["queued"] = json!(queued);
            d["queue_mode"] =
                json!(if s
                    .records("exploration_nodes")
                    .iter()
                    .any(|n| n.data["exploration_id"] == d["exploration_id"]
                        && n.data["kind"] == "goal")
                {
                    "resume"
                } else {
                    "bootstrap"
                });
            d["queued_at"] = if queued {
                json!(artex_core::now())
            } else {
                Value::Null
            };
            if !queued {
                d["status"] = json!("running");
            }
            b.put("tasks", id, d);
            if !queued {
                for n in s.records("exploration_nodes") {
                    if n.data["exploration_id"] == record(s, "tasks", id)?.data["exploration_id"]
                        && n.data["kind"] == "intent"
                        && n.data["state"] == "paused"
                    {
                        let mut nd = n.data.clone();
                        nd["state"] = json!("open");
                        b.put("exploration_nodes", &n.header.id, nd);
                    }
                }
            }
            conversation::push_event(
                &mut b.batch,
                s,
                c,
                if queued { "TaskQueued" } else { "TaskResumed" },
                "tasks",
                id,
            )?;
        }
        _ => return Err(Error::new("METHOD_NOT_FOUND", m)),
    }
    Ok(b.done(json!({"task_id":id})))
}
pub fn validate_asset_rule(input: &Value, task: bool) -> Result<Value> {
    let pattern = required(input, "pattern")?.trim();
    let kind = required(input, "kind")?;
    match kind {
        "exact_domain" | "exact_url" | "fuzzy_domain" | "fuzzy_ip" | "fuzzy_url" => {}
        "exact_ip" => {
            pattern
                .parse::<std::net::IpAddr>()
                .map_err(|_| Error::new("INVALID_PARAMS", "Invalid exact IP"))?;
        }
        "cidr" => {
            pattern
                .parse::<ipnet::IpNet>()
                .map_err(|_| Error::new("INVALID_PARAMS", "Invalid CIDR"))?;
        }
        _ => {
            return Err(Error::new(
                "INVALID_PARAMS",
                "Invalid asset interception kind",
            ));
        }
    }
    let mut rule = json!({"kind":kind,"pattern":pattern,"enabled":input["enabled"].as_bool().unwrap_or(false),"note":text(input,"note")});
    if task {
        let action = input["action"].as_str().unwrap_or("block");
        if !matches!(action, "allow" | "block") {
            return Err(Error::new(
                "INVALID_PARAMS",
                "Invalid asset interception action",
            ));
        }
        rule["action"] = json!(action);
    }
    Ok(rule)
}
pub fn query(m: &str, p: &Value, s: &Snapshot, c: &Context) -> Result<Value> {
    match m {
        "task.get" => Ok(record(s, "tasks", task_id(p, c)?)?.data.clone()),
        "task.list" | "list_tasks" => Ok(json!(
            s.records("tasks")
                .iter()
                .filter(|r| r.data["deleted_at"].is_null())
                .map(|r| r.data.clone())
                .collect::<Vec<_>>()
        )),
        "list_llm_profiles" => Ok(
            json!({"profiles":s.records("llm_profiles").iter().map(|r|{let mut d=r.data.clone();if let Some(o)=d.as_object_mut(){o.remove("api_key");}d}).collect::<Vec<_>>() }),
        ),
        _ => Err(Error::new("METHOD_NOT_FOUND", m)),
    }
}
