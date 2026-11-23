//! Replay independent log appends without copying the entire historical graph.
use super::*;
use std::sync::OnceLock;

// Logs have only their typed primary key: no FK, secondary unique key, or graph
// semantics can change when a new log is inserted. Derive the eligibility from
// the frozen model, so future relational constraints retain full replay checks.
fn independent_logs() -> bool {
    static ELIGIBLE: OnceLock<bool> = OnceLock::new();
    *ELIGIBLE.get_or_init(|| {
        let model = artex_core::data_model();
        let Some(table) = model["tables"]
            .as_array()
            .and_then(|tables| tables.iter().find(|table| table["name"] == "server_logs"))
        else {
            return false;
        };
        table["constraints"].as_array().is_some_and(Vec::is_empty)
            && model["tables"].as_array().is_some_and(|tables| {
                // Receipts currently have native schema/command-ID uniqueness.
                // Adding a SQL table must revisit any new cross-record keys.
                tables
                    .iter()
                    .all(|table| table["name"] != "command_receipts")
            })
            && table["columns"].as_array().is_some_and(|columns| {
                columns.iter().all(|column| {
                    let declaration = column["declaration"]
                        .as_str()
                        .unwrap_or("")
                        .to_ascii_uppercase();
                    !declaration.contains("UNIQUE")
                        && (!declaration.contains("PRIMARY KEY") || column["name"] == "id")
                })
            })
            && model["additional_constraints"]
                .as_array()
                .is_some_and(|constraints| {
                    constraints.iter().all(|constraint| {
                        constraint["collection"] != "server_logs"
                            && constraint["collection"] != "command_receipts"
                    })
                })
            && artex_core::foreign_keys().iter().all(|key| {
                !["server_logs", "command_receipts"].contains(&key.collection.as_str())
                    && !["server_logs", "command_receipts"].contains(&key.target.as_str())
            })
    })
}

impl Store {
    pub(super) fn try_replay_log_append(&mut self, payload: &Payload, seq: u64) -> Result<bool> {
        if !payload.events.is_empty()
            || !payload.deliveries.is_empty()
            || payload.mutations.iter().any(|mutation| {
                mutation.collection != "server_logs"
                    || mutation.op != "put"
                    || mutation.expected_version != "0"
                    || mutation.new_version != "1"
            })
            || !independent_logs()
        {
            return Ok(false);
        }
        self.check_replay_command(payload)?;
        let mut seen = BTreeSet::new();
        for mutation in &payload.mutations {
            if !seen.insert(&mutation.id) {
                return Err(Error::new(
                    "CORRUPT_STORE",
                    "Duplicate/store-owned mutation",
                ));
            }
            if self.state.get("server_logs", &mutation.id).is_some() {
                return Err(Error::new(
                    "CORRUPT_STORE",
                    "Mutation version discontinuity",
                ));
            }
            let record = mutation
                .record
                .as_ref()
                .ok_or_else(|| Error::new("CORRUPT_STORE", "Put is missing record"))?;
            if record.header.collection != mutation.collection
                || record.header.id != mutation.id
                || record.header.version != mutation.new_version
                || parse_u64(&record.header.commit_seq)? != seq
                || self
                    .tombstones
                    .contains_key(&(mutation.collection.clone(), mutation.id.clone()))
            {
                return Err(Error::new(
                    "CORRUPT_STORE",
                    "Put header mismatch or tombstone reuse",
                ));
            }
            validate_record(record)?;
        }
        let receipt = receipt_record(payload, seq)?;
        // Finish *all* fallible checks before publishing any part of the frame.
        // Existing records were validated on checkpoint/previous-frame admission;
        // schema, identity, sequence, duplicate and tombstone checks still apply
        // to every new record. Other writes retain full graph validation.
        for mutation in &payload.mutations {
            self.state
                .collections
                .entry("server_logs".into())
                .or_default()
                .insert(
                    mutation.id.clone(),
                    mutation.record.as_ref().unwrap().clone(),
                );
        }
        self.state
            .collections
            .entry("command_receipts".into())
            .or_default()
            .insert(payload.command_id.clone(), receipt);
        self.state.commit_seq = seq;
        *self.read_cache.lock().unwrap() = None;
        Ok(true)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn log_payload(seq: u64, count: usize) -> Payload {
        let timestamp = now();
        Payload {
            format_version: 1, committed_at: timestamp.clone(), command_id: new_id("command"),
            method: "server_logs.append".into(), normalized_params: json!({}), expected: vec![],
            mutations: (0..count).map(|index| {
                let id = format!("log_{:032x}", seq * 100 + index as u64);
                Mutation { op: "put".into(), collection: "server_logs".into(), id: id.clone(),
                    expected_version: "0".into(), new_version: "1".into(),
                    record: Some(Record { header: Header {
                        id: id.clone(), collection: "server_logs".into(), version: "1".into(),
                        commit_seq: seq.to_string(), created_at: timestamp.clone(),
                        updated_at: timestamp.clone(), legacy_key: Value::Null,
                    }, data: json!({"id":id,"created_at":timestamp,"level":"info","tag":"fixture","text":"unchanged history"}) }) }
            }).collect(), events: vec![], deliveries: vec![], result: json!({"ok":true}),
        }
    }
    fn state(store: &Store) -> Value {
        json!({"snapshot":store.state,"tombstones":store.tombstones.values().collect::<Vec<_>>()})
    }
    #[test]
    fn log_replay_matches_full_validation_without_cloning_existing_history() {
        assert!(independent_logs());
        let temporary = tempfile::tempdir().unwrap();
        let mut fast = Store::create(&temporary.path().join("fast"), "fast").unwrap();
        let mut reference =
            Store::create(&temporary.path().join("reference"), "reference").unwrap();
        fast.apply_payload_full(&log_payload(1, 32), 1).unwrap();
        reference.state = fast.state.clone();
        let old_id = fast.state.records("server_logs")[0].header.id.clone();
        let original_text = fast.state.get("server_logs", &old_id).unwrap().data["text"]
            .as_str()
            .unwrap()
            .as_ptr();
        for seq in 2..18 {
            let payload = log_payload(seq, 2);
            fast.apply_payload(&payload, seq).unwrap();
            reference.apply_payload_full(&payload, seq).unwrap();
            assert_eq!(state(&fast), state(&reference));
        }
        // A full-graph clone allocates a different String before dropping the old
        // graph. Retaining this allocation guards against reintroducing O(history
        // size * pending log frames) work while values still compare equal.
        assert_eq!(
            original_text,
            fast.state.get("server_logs", &old_id).unwrap().data["text"]
                .as_str()
                .unwrap()
                .as_ptr()
        );
        let empty = log_payload(18, 0);
        fast.apply_payload(&empty, 18).unwrap();
        reference.apply_payload_full(&empty, 18).unwrap();
        assert_eq!(state(&fast), state(&reference));
    }
    #[test]
    fn malformed_log_frames_are_atomic_and_match_full_replay_rejections() {
        let temporary = tempfile::tempdir().unwrap();
        let mut fast = Store::create(&temporary.path().join("fast"), "fast").unwrap();
        let mut reference =
            Store::create(&temporary.path().join("reference"), "reference").unwrap();
        let seed = log_payload(1, 1);
        fast.apply_payload_full(&seed, 1).unwrap();
        reference.state = fast.state.clone();
        for case in 0..12 {
            let mut payload = log_payload(2, 2);
            match case {
                0 => payload.mutations[1] = payload.mutations[0].clone(),
                1 => {
                    payload.mutations[1]
                        .record
                        .as_mut()
                        .unwrap()
                        .data
                        .as_object_mut()
                        .unwrap()
                        .remove("text");
                }
                2 => payload.mutations[1].record.as_mut().unwrap().header.id = new_id("log"),
                3 => {
                    payload.mutations[1]
                        .record
                        .as_mut()
                        .unwrap()
                        .header
                        .commit_seq = "3".into()
                }
                4 => {
                    payload.mutations[1].record.as_mut().unwrap().data["id"] = json!(new_id("log"))
                }
                5 => payload.committed_at = "not-a-timestamp".into(),
                6 => payload.format_version = 2,
                7 => payload.normalized_params = json!({"api_key":"unsealed-fixture-secret"}),
                8 => payload.command_id = seed.command_id.clone(),
                9 => payload.mutations[1].record = None,
                10 => payload.mutations[1] = seed.mutations[0].clone(),
                11 => payload.mutations[1].record.as_mut().unwrap().header.version = "2".into(),
                _ => unreachable!(),
            }
            let before = state(&fast);
            let actual = fast.apply_payload(&payload, 2).unwrap_err();
            let expected = reference.apply_payload_full(&payload, 2).unwrap_err();
            assert_eq!(actual.code, expected.code, "case {case}");
            assert_eq!(state(&fast), before, "case {case}");
            assert_eq!(state(&reference), before, "case {case}");
        }
    }
    #[test]
    fn updates_deletes_and_relational_frames_keep_full_graph_replay() {
        let temporary = tempfile::tempdir().unwrap();
        let mut store = Store::create(&temporary.path().join("store"), "fallback").unwrap();
        let seed = log_payload(1, 1);
        store.apply_payload(&seed, 1).unwrap();
        let mut update = log_payload(2, 1);
        update.mutations[0] = seed.mutations[0].clone();
        let mutation = &mut update.mutations[0];
        mutation.expected_version = "1".into();
        mutation.new_version = "2".into();
        let record = mutation.record.as_mut().unwrap();
        record.header.version = "2".into();
        record.header.commit_seq = "2".into();
        record.data["text"] = json!("updated");
        let updated_id = mutation.id.clone();
        assert!(!store.try_replay_log_append(&update, 2).unwrap());
        store.apply_payload(&update, 2).unwrap();
        assert_eq!(
            store.state.get("server_logs", &updated_id).unwrap().data["text"],
            "updated"
        );
        let mut deletion = log_payload(3, 0);
        deletion.mutations.push(Mutation {
            op: "delete".into(),
            collection: "server_logs".into(),
            id: updated_id,
            expected_version: "2".into(),
            new_version: "3".into(),
            record: None,
        });
        assert!(!store.try_replay_log_append(&deletion, 3).unwrap());
        store.apply_payload(&deletion, 3).unwrap();
        let mut reuse = seed.clone();
        reuse.command_id = new_id("command");
        reuse.mutations[0]
            .record
            .as_mut()
            .unwrap()
            .header
            .commit_seq = "4".into();
        let before = state(&store);
        assert!(store.apply_payload(&reuse, 4).is_err());
        assert_eq!(state(&store), before);
        let mut relational = log_payload(4, 1);
        relational.mutations[0].collection = "companies".into();
        assert!(!store.try_replay_log_append(&relational, 4).unwrap());
        let mut event = log_payload(4, 0);
        event.events.push(seed.mutations[0].record.clone().unwrap());
        assert!(!store.try_replay_log_append(&event, 4).unwrap());
    }
    #[test]
    fn long_uncheckpointed_log_tail_reopens_and_rejects_bad_frame_without_partial_effects() {
        let temporary = tempfile::tempdir().unwrap();
        let root = temporary.path().join("history");
        let store = Store::create_legacy(&root, "long log tail").unwrap();
        let journal = store.journal_path.clone();
        let mut previous = store.last_hash;
        drop(store);
        let mut file = OpenOptions::new().append(true).open(&journal).unwrap();
        let mut commands = Vec::new();
        for seq in 1..=2000 {
            let payload = log_payload(seq, 1);
            commands.push(payload.command_id.clone());
            let frame = encode_frame(
                &serde_json::to_vec(&payload).unwrap(),
                seq,
                previous,
                [0; 16],
            )
            .unwrap();
            previous = hash(&frame);
            file.write_all(&frame).unwrap();
        }
        let mut bad = log_payload(2001, 2);
        bad.mutations[1]
            .record
            .as_mut()
            .unwrap()
            .data
            .as_object_mut()
            .unwrap()
            .remove("text");
        file.write_all(
            &encode_frame(&serde_json::to_vec(&bad).unwrap(), 2001, previous, [0; 16]).unwrap(),
        )
        .unwrap();
        file.sync_all().unwrap();
        drop(file);
        let journal_before = fs::read(&journal).unwrap();
        let lock_before = fs::read(root.join("locks/writer.lock")).unwrap();
        let reopened = Store::open_read_only(&root).unwrap();
        assert_eq!(reopened.state.commit_seq, 2000);
        assert_eq!(reopened.state.records("server_logs").len(), 2000);
        assert_eq!(reopened.state.records("command_receipts").len(), 2000);
        assert!(
            commands
                .iter()
                .all(|id| reopened.state.get("command_receipts", id).is_some())
        );
        assert!(
            reopened
                .state
                .get("server_logs", &bad.mutations[0].id)
                .is_none()
        );
        assert!(
            reopened
                .state
                .get("command_receipts", &bad.command_id)
                .is_none()
        );
        assert_eq!(
            reopened.corruption.as_ref().unwrap()["kind"],
            "invalid_mutation"
        );
        drop(reopened); // Release the Windows byte-range lock before inspecting its bytes.
        assert_eq!(fs::read(&journal).unwrap(), journal_before);
        assert_eq!(
            fs::read(root.join("locks/writer.lock")).unwrap(),
            lock_before
        );
    }
}
