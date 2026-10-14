//! Native registry reads run off the writer thread; only approved metadata is persisted.
use super::*;
use artex_domain::{Planner, record, text};
use std::{collections::BTreeSet, time::Duration};

impl Router {
    pub(super) fn ensure_skill_catalog(&mut self, rid: &str) -> Result<bool> {
        let snapshot = self.snapshot()?;
        let run = record(&snapshot, "agent_runs", rid)?;
        let agent_id = text(&run.data, "agent_id");
        let names: Vec<String> = snapshot
            .records("agent_skill_visibility")
            .into_iter()
            .filter(|row| row.data["agent_id"] == agent_id && row.data["enabled"] == true)
            .filter_map(|row| row.data["skill_name"].as_str().map(str::to_owned))
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect();
        if names.is_empty() {
            return Ok(true);
        }
        let Some(capability) = self.resource_capability.clone() else {
            return Ok(true);
        };
        let Some(gateway) = self.gateway.clone() else {
            return Ok(true);
        };
        if run.data["context_snapshot"]["skill_catalog_names"] == json!(names)
            && run.data["context_snapshot"]["skill_catalog"].is_array()
        {
            return Ok(true);
        }
        if self
            .runtime
            .runs
            .get(rid)
            .is_none_or(|state| state.in_flight)
        {
            return Ok(false);
        }
        let context = artex_agent::context_for_run(&snapshot, rid, &self.engine_epoch)?;
        let workspace = snapshot.workspace_id.clone();
        let epoch = self.engine_epoch.clone();
        let generation = self.workspace_generation.clone();
        let run = rid.to_owned();
        let sender = self.runtime.tx.clone();
        self.runtime.runs.get_mut(rid).unwrap().in_flight = true;
        std::thread::spawn(move || {
            let result = gateway.request(
                &workspace,
                &context,
                "skill.catalog",
                json!({"directory_capability":capability}),
                Duration::from_secs(30),
            );
            let _ = sender.send(controller::Completion::SkillCatalog {
                run,
                epoch,
                generation,
                names,
                result,
            });
        });
        Ok(false)
    }

    pub(super) fn complete_skill_catalog(
        &mut self,
        rid: &str,
        epoch: &str,
        generation: &str,
        names: &[String],
        result: Result<Value>,
    ) -> Result<()> {
        if epoch != self.engine_epoch || generation != self.workspace_generation {
            return Ok(());
        }
        if let Some(state) = self.runtime.runs.get_mut(rid) {
            state.in_flight = false;
        }
        let snapshot = self.snapshot()?;
        let Some(run) = snapshot.get("agent_runs", rid) else {
            return Ok(());
        };
        if !matches!(text(&run.data, "state"), "running" | "waiting") {
            return Ok(());
        }
        let rows = match &result {
            Ok(value) => value
                .as_array()
                .or_else(|| value["skills"].as_array())
                .cloned()
                .ok_or_else(|| {
                    Error::new(
                        "INVALID_PARAMS",
                        "Native Skill catalog must contain metadata rows",
                    )
                })?,
            Err(_) => Vec::new(), // Failed registry reads never advertise or unlock a Skill.
        };
        let catalog: Vec<Value> = rows.iter().filter(|row| row["name"].as_str().is_some_and(|name| names.iter().any(|allowed| allowed == name)))
            .map(|row| json!({"name":row["name"],"description":row["description"],"mcps":row.get("mcps").or_else(||row.get("mcp")).cloned().unwrap_or(Value::Null)})).collect();
        let mut data = run.data.clone();
        if !data["context_snapshot"].is_object() {
            data["context_snapshot"] = json!({});
        }
        data["context_snapshot"]["skill_catalog_names"] = json!(names);
        data["context_snapshot"]["skill_catalog"] = json!(catalog);
        if let Err(error) = result {
            data["context_snapshot"]["skill_catalog_error"] = json!(error.code);
        } else {
            data["context_snapshot"]
                .as_object_mut()
                .unwrap()
                .remove("skill_catalog_error");
        }
        let mut planner = Planner::new(&snapshot, "run.skill.catalog", &json!({"run_id":rid}));
        planner.expect(run);
        planner.put("agent_runs", rid, data);
        self.internal_commit(planner.done(json!({"ready":true})))?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_visible(router: &mut Router, run: &str, name: &str) -> String {
        let snapshot = router.snapshot().unwrap();
        let agent = record(&snapshot, "agent_runs", run).unwrap().data["agent_id"].clone();
        let mut planner = Planner::new(&snapshot, "test.skill.visibility", &json!({}));
        let id = planner
            .insert(
                "agent_skill_visibility",
                "skillvisibility",
                json!({"agent_id":agent,"skill_name":name,"enabled":true}),
            )
            .unwrap();
        router.internal_commit(planner.done(json!({}))).unwrap();
        id
    }

    #[test]
    fn stale_skill_catalog_epoch_and_workspace_generation_do_not_touch_current_run_or_commit() {
        let (_dir, mut router, run, _context) = regression::seeded();
        let names = vec!["approved".to_owned()];
        let before = router.snapshot().unwrap();
        let before_run = record(&before, "agent_runs", &run).unwrap().clone();
        for (epoch, generation) in [
            (new_id("epoch"), router.workspace_generation.clone()),
            (router.engine_epoch.clone(), new_id("generation")),
        ] {
            router.complete_skill_catalog(&run,&epoch,&generation,&names,Ok(json!([{"name":"approved","description":"stale payload","mcps":["must-not-unlock"]}]))).unwrap();
            let after = router.snapshot().unwrap();
            let after_run = record(&after, "agent_runs", &run).unwrap();
            assert_eq!(after.commit_seq, before.commit_seq);
            assert_eq!(after_run.header.version, before_run.header.version);
            assert_eq!(after_run.data, before_run.data);
            assert_eq!(after_run.data["state"], "running");
        }
    }

    #[test]
    fn native_skill_catalog_persists_only_approved_metadata_and_visibility_revocation_is_immediate()
    {
        let (_dir, mut router, run, context) = regression::seeded();
        let visible = make_visible(&mut router, &run, "approved");
        let epoch = router.engine_epoch.clone();
        let generation = router.workspace_generation.clone();
        let names = vec!["approved".to_owned()];
        router.complete_skill_catalog(&run,&epoch,&generation,&names,Ok(json!({"skills":[{"name":"approved","description":"Inspect the authorized target","mcp":["scanner"],"instructions":"must-not-store-instructions","path":"must-not-store-path"},{"name":"hidden","description":"must-not-store-hidden","mcps":["hidden-scanner"]},{"description":"missing name"}]}))).unwrap();
        let snapshot = router.snapshot().unwrap();
        let data = &record(&snapshot, "agent_runs", &run).unwrap().data;
        assert_eq!(
            data["context_snapshot"]["skill_catalog"],
            json!([{"name":"approved","description":"Inspect the authorized target","mcps":["scanner"]}])
        );
        assert_eq!(
            data["context_snapshot"]["skill_catalog_names"],
            json!(names)
        );
        assert_eq!(data["state"], "running");
        assert!(!data.to_string().contains("must-not-store"));
        assert_eq!(
            artex_domain::sdk::visible_skills(&snapshot, &context).len(),
            1
        );
        let mut planner = Planner::new(&snapshot, "test.skill.revoke", &json!({}));
        let mut revoked = record(&snapshot, "agent_skill_visibility", &visible)
            .unwrap()
            .data
            .clone();
        revoked["enabled"] = json!(false);
        planner.put("agent_skill_visibility", &visible, revoked);
        router.internal_commit(planner.done(json!({}))).unwrap();
        let snapshot = router.snapshot().unwrap();
        assert_eq!(record(&snapshot,"agent_runs",&run).unwrap().data["context_snapshot"]["skill_catalog"].as_array().unwrap().len(),1);
        assert!(artex_domain::sdk::visible_skills(&snapshot, &context).is_empty());
    }

    #[test]
    fn native_skill_catalog_failure_clears_previous_metadata_and_retains_only_error_code() {
        let (_dir, mut router, run, context) = regression::seeded();
        make_visible(&mut router, &run, "approved");
        let names = vec!["approved".to_owned()];
        let epoch = router.engine_epoch.clone();
        let generation = router.workspace_generation.clone();
        router
            .complete_skill_catalog(
                &run,
                &epoch,
                &generation,
                &names,
                Ok(json!([{"name":"approved","description":"previous valid catalog","mcps":[]}])),
            )
            .unwrap();
        let before = router.snapshot().unwrap();
        assert_eq!(
            artex_domain::sdk::visible_skills(&before, &context).len(),
            1
        );
        router
            .complete_skill_catalog(
                &run,
                &epoch,
                &generation,
                &names,
                Err(Error::new(
                    "EXTERNAL_FAILURE",
                    "must-not-persist-private-host-error",
                )),
            )
            .unwrap();
        let snapshot = router.snapshot().unwrap();
        let data = &record(&snapshot, "agent_runs", &run).unwrap().data;
        assert_eq!(snapshot.commit_seq, before.commit_seq + 1);
        assert_eq!(data["state"], "running");
        assert_eq!(data["context_snapshot"]["skill_catalog"], json!([]));
        assert_eq!(
            data["context_snapshot"]["skill_catalog_error"],
            "EXTERNAL_FAILURE"
        );
        assert!(!data.to_string().contains("must-not-persist"));
        assert!(artex_domain::sdk::visible_skills(&snapshot, &context).is_empty());
        router
            .complete_skill_catalog(
                &run,
                &epoch,
                &generation,
                &names,
                Ok(json!([{"name":"approved","description":"recovered catalog","mcps":[]}])),
            )
            .unwrap();
        let snapshot = router.snapshot().unwrap();
        assert!(
            record(&snapshot, "agent_runs", &run).unwrap().data["context_snapshot"]
                .get("skill_catalog_error")
                .is_none()
        );
        assert_eq!(
            artex_domain::sdk::visible_skills(&snapshot, &context).len(),
            1
        );
    }
}
