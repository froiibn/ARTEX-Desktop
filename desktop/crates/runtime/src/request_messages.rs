//! Provider request views preserve confirmed tool pairs without rewriting history.
use crate::Prompt;
use std::collections::BTreeSet;

pub(crate) fn normalize(prompt: &Prompt) -> Prompt {
    let mut uses = BTreeSet::new();
    let mut results = BTreeSet::new();
    for message in &prompt.messages {
        for block in &message.content {
            match block["type"].as_str() {
                Some("tool_use") => {
                    if let Some(id) = block["id"].as_str() {
                        uses.insert(id);
                    }
                }
                Some("tool_result") => {
                    if let Some(id) = block["tool_use_id"].as_str() {
                        results.insert(id);
                    }
                }
                _ => {}
            }
        }
    }
    // Like norma/llm/PairToolBlocks, repair only the outbound API view. A
    // deadline can leave a host invocation running: inventing a result here
    // would falsely assert an external outcome or cleanup proof in the ledger.
    let mut normalized = prompt.clone();
    for message in &mut normalized.messages {
        message
            .content
            .retain(|block| match block["type"].as_str() {
                Some("tool_use") => block["id"].as_str().is_some_and(|id| results.contains(id)),
                Some("tool_result") => block["tool_use_id"]
                    .as_str()
                    .is_some_and(|id| uses.contains(id)),
                _ => true,
            });
    }
    normalized
        .messages
        .retain(|message| !message.content.is_empty());
    normalized
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Message;
    use serde_json::json;

    #[test]
    fn request_view_preserves_pairs_and_text_without_mutating_durable_history() {
        let prompt = Prompt {
            system: "fixture".into(),
            session_id: "fixture".into(),
            tools: vec![],
            messages: vec![
                Message {
                    role: "assistant".into(),
                    content: vec![
                        json!({"type":"text","text":"Confirmed observation"}),
                        json!({"type":"tool_use","id":"done","name":"Read","input":{}}),
                        json!({"type":"tool_use","id":"pending","name":"Bash","input":{}}),
                    ],
                },
                Message {
                    role: "user".into(),
                    content: vec![
                        json!({"type":"tool_result","tool_use_id":"done","content":"Actual result"}),
                        json!({"type":"tool_result","tool_use_id":"missing","content":"Unpaired result"}),
                    ],
                },
                Message {
                    role: "assistant".into(),
                    content: vec![
                        json!({"type":"tool_use","id":"interrupted","name":"Read","input":{}}),
                    ],
                },
            ],
        };
        let before = serde_json::to_value(&prompt.messages).unwrap();
        let view = normalize(&prompt);
        assert_eq!(serde_json::to_value(&prompt.messages).unwrap(), before);
        assert_eq!(view.messages.len(), 2);
        assert_eq!(view.messages[0].content.len(), 2);
        assert_eq!(view.messages[1].content.len(), 1);
        assert_eq!(view.messages[0].content[1]["id"], "done");
        assert_eq!(view.messages[1].content[0]["tool_use_id"], "done");
        assert_eq!(
            serde_json::to_value(&normalize(&view).messages).unwrap(),
            serde_json::to_value(&view.messages).unwrap()
        );
    }
}
