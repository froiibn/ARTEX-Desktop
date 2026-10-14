//! Durable summaries retain their original messages and content-addressed archives.
use super::*;
use artex_domain::{record, text};
use artex_runtime::Message;
use std::collections::BTreeSet;
#[path = "compression_policy.rs"]
mod policy;
pub(super) use policy::AutomaticCompaction;
use policy::{content_chars, hard_protected, repair_pairs, soft_protected};
#[derive(Clone)]
struct Entry {
    id: String,
    message: Message,
    skill_guidance: bool,
}

#[cfg(test)]
mod skill_guidance_regression {
    use super::*;

    #[test]
    fn blob_skill_guidance_survives_legacy_covered_ids_and_checkpoint_reopen() {
        let (dir, mut router, run, context) = crate::regression::seeded();
        router
            .append_transcript(
                &run,
                "text",
                "Compressible history",
                &Message {
                    role: "user".into(),
                    content: vec![json!({"type":"text","text":"ordinary covered history"})],
                },
                None,
            )
            .unwrap();
        let original = Message {
            role: "user".into(),
            content: vec![
                json!({"type":"text","text":format!("<system-reminder><invoked-skill name=\"inspect\">{}\n</invoked-skill></system-reminder>", "Follow the approved inspection procedure exactly. ".repeat(1800))}),
            ],
        };
        let s = router.snapshot().unwrap();
        let guidance = artex_domain::conversation::append_activity(
            &s, &context, &run, "skill_guidance", "Skill inspect loaded",
            &artex_core::body(&json!({"run_id":run,"skill_invocation":{"name":"inspect","success":true,"invocation_id":new_id("invocation")},"message":original}).to_string()), None,
        ).unwrap();
        drop(s);
        let guidance_id = router.internal_commit(guidance).unwrap()["activity_id"]
            .as_str()
            .unwrap()
            .to_owned();
        let s = router.snapshot().unwrap();
        assert!(
            s.get("conversation_activities", &guidance_id).unwrap().data["detail"]["blob"]
                .is_object(),
            "Exercise stored blob guidance, not only inline JSON"
        );
        drop(s);
        let (entries, _) = router.transcript_entries(&run).unwrap();
        assert!(hard_protected(&entries).contains(&guidance_id));
        let ordinary = entries
            .iter()
            .find(|e| e.id != guidance_id)
            .unwrap()
            .id
            .clone();
        let archive = router.store.as_mut().unwrap().blob_put(
            &serde_json::to_vec(&json!({"protocol":"artex-compression/1","run_id":run,"messages":entries.iter().map(|e|json!({"id":e.id,"message":e.message})).collect::<Vec<_>>()})).unwrap(),
            "application/json",
        ).unwrap();
        // Older compaction metadata can already list this ID as covered. The
        // current hard-protection rule must override that historical coverage.
        let s = router.snapshot().unwrap();
        let batch = artex_domain::conversation::append_activity(
            &s, &context, &run, "compression", "Legacy summary",
            &artex_core::body(&json!({"run_id":run,"compression_blocks":[{"block_id":"b1","tier":1,"first_id":ordinary,"last_id":guidance_id,"message_ids":[ordinary,guidance_id],"summary":"Historical summary without the Skill instructions","archive":archive,"direct_blocks":[]}]}).to_string()), None,
        ).unwrap();
        drop(s);
        router.internal_commit(batch).unwrap();
        for reopened in [false, true] {
            if reopened {
                router.store.as_mut().unwrap().checkpoint().unwrap();
                router.store = None;
                router.store = Some(Store::open(&dir.path().join("store")).unwrap());
            }
            let messages = router.compressed_messages(&run).unwrap();
            let matching = messages
                .iter()
                .filter(|m| m.role == original.role && m.content == original.content)
                .count();
            assert_eq!(
                matching, 1,
                "The complete original standing guidance must occur exactly once"
            );
            assert!(messages.iter().any(|m| m.content.iter().any(|b| {
                b["text"]
                    .as_str()
                    .is_some_and(|text| text.contains("Historical summary"))
            })));
            assert!(!messages.iter().any(|m| {
                m.content
                    .iter()
                    .any(|b| b["text"] == "ordinary covered history")
            }));
        }
    }
}
#[derive(Clone)]
struct Block {
    value: Value,
}
impl Router {
    fn transcript_entries(&self, rid: &str) -> Result<(Vec<Entry>, Vec<Block>)> {
        let s = self.snapshot()?;
        let run = record(&s, "agent_runs", rid)?;
        let mut activities = if let Some(cid) = run.data["conversation_id"].as_str() {
            s.records("conversation_activities")
                .into_iter()
                .filter(|a| a.data["conversation_id"] == cid)
                .collect::<Vec<_>>()
        } else {
            s.records("activity")
                .into_iter()
                .filter(|a| a.data["metadata"]["run_id"] == rid)
                .collect::<Vec<_>>()
        };
        activities.sort_by(|a, b| {
            a.header
                .created_at
                .cmp(&b.header.created_at)
                .then_with(|| {
                    (a.data["kind"] == "skill_guidance").cmp(&(b.data["kind"] == "skill_guidance"))
                })
                .then(
                    a.header
                        .commit_seq
                        .parse::<u64>()
                        .unwrap_or(0)
                        .cmp(&b.header.commit_seq.parse::<u64>().unwrap_or(0)),
                )
                .then(a.header.id.cmp(&b.header.id))
        });
        let mut entries = Vec::new();
        let mut blocks = Vec::new();
        for a in activities {
            let detail = self.body_text(&a.data["detail"])?;
            let v: Value = serde_json::from_str(&detail).unwrap_or(Value::Null);
            if let Some(list) = v["compression_blocks"].as_array() {
                blocks.extend(list.iter().cloned().map(|value| Block { value }));
                continue;
            }
            let message = if v["message"].is_object() {
                Some(serde_json::from_value(v["message"].clone()).map_err(|_| {
                    Error::new("CORRUPT_STORE", "Malformed durable transcript message")
                })?)
            } else if a.data["kind"] == "user" {
                Some(Message {
                    role: "user".into(),
                    content: vec![
                        json!({"type":"text","text":v["text"].as_str().unwrap_or(&detail)}),
                    ],
                })
            } else {
                None
            };
            if let Some(message) = message {
                entries.push(Entry {
                    id: a.header.id.clone(),
                    message,
                    skill_guidance: a.data["kind"] == "skill_guidance"
                        && v["skill_invocation"]["success"] == true,
                });
            }
        }
        if entries.len() > 99999 {
            return Err(Error::new(
                "SIZE_LIMIT",
                "Conversation reference space exhausted",
            ));
        }
        Ok((entries, blocks))
    }
    pub(super) fn compressed_messages(&self, rid: &str) -> Result<Vec<Message>> {
        self.render_compressed_messages(rid, true)
    }
    fn render_compressed_messages(&self, rid: &str, tags: bool) -> Result<Vec<Message>> {
        let (entries, blocks) = self.transcript_entries(rid)?;
        let inactive = blocks
            .iter()
            .flat_map(|b| {
                b.value["direct_blocks"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .filter_map(Value::as_str)
            })
            .collect::<BTreeSet<_>>();
        let active = blocks
            .iter()
            .filter(|b| !inactive.contains(text(&b.value, "block_id")))
            .collect::<Vec<_>>();
        let mut covered = active
            .iter()
            .flat_map(|b| {
                b.value["message_ids"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .filter_map(Value::as_str)
                    .map(str::to_owned)
            })
            .collect::<BTreeSet<_>>();
        let hard = hard_protected(&entries);
        covered.retain(|id| !hard.contains(id));
        repair_pairs(&entries, &mut covered);
        let mut out = Vec::new();
        for (index, entry) in entries.iter().enumerate() {
            for b in &active {
                if b.value["first_id"] == entry.id {
                    out.push(Message{role:"user".into(),content:vec![json!({"type":"text","text":format!("[Compressed conversation section]\n<noa-block id=\"{}\" tier=\"{}\" archive=\"blob:{}\">\n{}\n</noa-block>",text(&b.value,"block_id"),b.value["tier"].as_u64().unwrap_or(1),text(&b.value["archive"],"sha256"),text(&b.value,"summary"))})]});
                }
            }
            if covered.contains(entry.id.as_str()) {
                continue;
            }
            let mut message = entry.message.clone();
            if !tags || hard.contains(&entry.id) || message.role == "assistant" {
                out.push(message);
                continue;
            }
            let tag = format!("<noa-ref id=\"m{:05}\"/>", index + 1);
            if let Some(result) = message
                .content
                .iter_mut()
                .find(|b| b["type"] == "tool_result")
            {
                if let Some(content) = result["content"].as_str() {
                    result["content"] = json!(format!("{content}\n{tag}"));
                }
            } else {
                message.content.push(json!({"type":"text","text":tag}));
            }
            out.push(message);
        }
        Ok(out)
    }
    pub(super) fn compress_plan(
        &mut self,
        rid: &str,
        input: &Value,
        context: &Context,
    ) -> Result<CommandBatch> {
        if !self.noa_enabled(rid)? {
            return Err(Error::new(
                "FEATURE_DISABLED",
                "NOA compression is disabled for this run",
            ));
        }
        let (entries, blocks) = self.transcript_entries(rid)?;
        let inactive = blocks
            .iter()
            .flat_map(|b| {
                b.value["direct_blocks"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .filter_map(Value::as_str)
            })
            .collect::<BTreeSet<_>>();
        let active = blocks
            .iter()
            .filter(|b| !inactive.contains(text(&b.value, "block_id")))
            .collect::<Vec<_>>();
        let consumed = active
            .iter()
            .flat_map(|b| {
                b.value["message_ids"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .filter_map(Value::as_str)
                    .map(str::to_owned)
            })
            .collect::<BTreeSet<_>>();
        let hard = hard_protected(&entries);
        let soft = soft_protected(&entries, &consumed, &hard);
        let ranges = input["content"]
            .as_array()
            .ok_or_else(|| Error::new("INVALID_PARAMS", "Compression ranges required"))?;
        let mut prepared = Vec::new();
        let mut occupied = BTreeSet::new();
        let mut total_chars = 0usize;
        let mut block_boundary = false;
        let mut warnings = Vec::<String>::new();
        let mut errors = Vec::<String>::new();
        if ranges.len() > 100 {
            return Err(Error::new(
                "SIZE_LIMIT",
                "At most 100 compression ranges allowed",
            ));
        }
        let resolve = |value: &Value| -> Result<usize> {
            let r = value.as_str().unwrap_or("").trim().to_lowercase();
            if let Some(n) = r
                .strip_prefix('m')
                .and_then(|n| n.parse::<usize>().ok())
                .filter(|n| *n > 0 && *n <= entries.len())
            {
                if hard.contains(&entries[n - 1].id) {
                    return Err(Error::new(
                        "INVALID_PARAMS",
                        "Hard-protected tool exchange has no addressable reference",
                    ));
                }
                return Ok(n - 1);
            }
            if let Some(b) = active.iter().find(|b| b.value["block_id"] == r) {
                return entries
                    .iter()
                    .position(|e| b.value["first_id"] == e.id)
                    .ok_or_else(|| Error::new("CORRUPT_STORE", "Missing summary anchor"));
            }
            Err(Error::new(
                "INVALID_PARAMS",
                "Unknown or consumed compression reference",
            ))
        };
        let mut resolved = Vec::new();
        for range in ranges {
            let start = match resolve(&range["startId"]) {
                Ok(v) => v,
                Err(e) => {
                    errors.push(e.message);
                    continue;
                }
            };
            let mut end = match resolve(&range["endId"]) {
                Ok(v) => v,
                Err(e) => {
                    errors.push(e.message);
                    continue;
                }
            };
            for b in &active {
                if range["endId"]
                    .as_str()
                    .is_some_and(|v| b.value["block_id"] == v.trim().to_ascii_lowercase())
                {
                    end = entries
                        .iter()
                        .position(|e| b.value["last_id"] == e.id)
                        .unwrap_or(end);
                }
            }
            resolved.push((start.min(end), start.max(end), range));
        }
        resolved.sort_by_key(|r| r.0);
        for (mut start, mut end, range) in resolved {
            // Widen boundaries until every complete tool exchange survives together.
            loop {
                let mut calls = BTreeSet::new();
                for e in &entries[start..=end] {
                    for c in &e.message.content {
                        if c["type"] == "tool_use" {
                            calls.insert(text(c, "id").to_owned());
                        }
                        if c["type"] == "tool_result" {
                            calls.insert(text(c, "tool_use_id").to_owned());
                        }
                    }
                }
                let old = (start, end);
                for (i, e) in entries.iter().enumerate() {
                    if e.message.content.iter().any(|c| {
                        (c["type"] == "tool_use" && calls.contains(text(c, "id")))
                            || (c["type"] == "tool_result"
                                && calls.contains(text(c, "tool_use_id")))
                    }) {
                        start = start.min(i);
                        end = end.max(i);
                    }
                }
                if old == (start, end) {
                    break;
                }
            }
            let is_block = range["startId"]
                .as_str()
                .is_some_and(|r| r.trim().to_ascii_lowercase().starts_with('b'))
                || range["endId"]
                    .as_str()
                    .is_some_and(|r| r.trim().to_ascii_lowercase().starts_with('b'));
            if (start..=end).any(|i| occupied.contains(&i)) {
                warnings.push("Skipped overlapping range; earlier range takes precedence".into());
                continue;
            }
            occupied.extend(start..=end);
            let nested = active
                .iter()
                .filter(|b| {
                    entries[start..=end]
                        .iter()
                        .any(|e| b.value["first_id"] == e.id)
                })
                .collect::<Vec<_>>();
            let lowest = if is_block {
                nested
                    .iter()
                    .map(|b| b.value["tier"].as_u64().unwrap_or(1))
                    .min()
                    .unwrap_or(1)
            } else {
                1
            };
            let direct = if is_block {
                nested
                    .iter()
                    .filter(|b| b.value["tier"].as_u64().unwrap_or(1) == lowest)
                    .map(|b| b.value["block_id"].clone())
                    .collect::<Vec<_>>()
            } else {
                vec![]
            };
            let mut ids = entries[start..=end]
                .iter()
                .filter(|e| {
                    !hard.contains(&e.id) && !soft.contains(&e.id) && !consumed.contains(&e.id)
                })
                .map(|e| e.id.clone())
                .collect::<BTreeSet<_>>();
            repair_pairs(&entries, &mut ids);
            let raw_count = ids.len();
            for b in &nested {
                if direct.contains(&b.value["block_id"]) {
                    ids.extend(
                        b.value["message_ids"]
                            .as_array()
                            .into_iter()
                            .flatten()
                            .filter_map(Value::as_str)
                            .map(str::to_owned),
                    );
                } else if is_block {
                    warnings.push(format!(
                        "Higher tier block {} remains active",
                        text(&b.value, "block_id")
                    ));
                }
            }
            if ids.is_empty() {
                errors.push(
                    "Range contains no new unprotected content; target an active block".into(),
                );
                continue;
            }
            if is_block && lowest >= 3 && raw_count == 0 {
                errors.push(
                    "Tier 3 is terminal; rewriting terminal summaries reclaims nothing".into(),
                );
                continue;
            }
            let summary = text(range, "summary").trim();
            let max = range["summaryMaxChars"]
                .as_u64()
                .or_else(|| input["summaryMaxChars"].as_u64())
                .filter(|n| *n > 0)
                .unwrap_or(20000)
                .min(4 * 1024 * 1024) as usize;
            if summary.chars().count() < 50 || summary.chars().count() > max {
                errors.push("Summary must contain 50 characters and fit its explicit limit".into());
                continue;
            }
            if raw_count < end - start + 1 {
                warnings.push("Protected, already covered or split tool messages were excluded from the range".into());
            }
            total_chars += entries
                .iter()
                .filter(|e| ids.contains(&e.id) && !consumed.contains(&e.id))
                .map(|e| content_chars(&e.message))
                .sum::<usize>();
            block_boundary |= is_block;
            prepared.push((
                start,
                end,
                summary.to_owned(),
                range
                    .get("topic")
                    .or_else(|| input.get("topic"))
                    .cloned()
                    .unwrap_or(Value::Null),
                ids,
                direct,
                if is_block { (lowest + 1).min(3) } else { 1 },
            ));
        }
        if prepared.is_empty() {
            return Err(Error::new(
                "INVALID_PARAMS",
                errors
                    .first()
                    .map(String::as_str)
                    .unwrap_or("No valid compression ranges"),
            ));
        }
        if !prepared.is_empty() && !block_boundary && total_chars < 5000 {
            return Err(Error::new(
                "INVALID_PARAMS",
                "Compress at least 5000 characters in one batch",
            ));
        }
        let mut created = Vec::new();
        for (ordinal, (start, end, summary, topic, ids, direct, tier)) in
            prepared.into_iter().enumerate()
        {
            let archive = json!({"protocol":"artex-compression/1","run_id":rid,"messages":entries.iter().filter(|e|ids.contains(&e.id)).map(|e|json!({"id":e.id,"message":e.message})).collect::<Vec<_>>(),"prior_blocks":active.iter().filter(|b|direct.contains(&b.value["block_id"])).map(|b|b.value.clone()).collect::<Vec<_>>()});
            let archive = self
                .store
                .as_mut()
                .unwrap()
                .blob_put(&serde_json::to_vec(&archive)?, "application/json")?;
            let first = entries[start..=end]
                .iter()
                .find(|e| ids.contains(&e.id))
                .unwrap();
            let last = entries[start..=end]
                .iter()
                .rev()
                .find(|e| ids.contains(&e.id))
                .unwrap();
            created.push(json!({"block_id":format!("b{}",blocks.len()+ordinal+1),"tier":tier,"first_id":first.id,"last_id":last.id,"message_ids":ids,"summary":summary,"topic":topic,"archive":archive,"direct_blocks":direct}));
        }
        let s = self.snapshot()?;
        let mut batch = artex_domain::conversation::append_activity(
            &s,
            context,
            rid,
            "compression",
            "Context ranges archived and summarized",
            &artex_core::body(&json!({"run_id":rid,"compression_blocks":created,"warnings":warnings,"errors":errors}).to_string()),
            None,
        )?;
        batch.result = json!({"blocks":created,"originals_preserved":true,"warnings":warnings,"errors":errors});
        Ok(batch)
    }
}
