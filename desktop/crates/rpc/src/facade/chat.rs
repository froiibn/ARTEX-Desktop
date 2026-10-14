use artex_core::{CommandBatch, Context, Result, Snapshot};
use serde_json::{Value, json};
pub const QUERIES: &[&str] = &[
    "ui.conversations",
    "ui.conversationMessages",
    "ui.conversationHistory",
    "ui.conversationMsgDetail",
    "ui.chatStatus",
];
pub const COMMANDS: &[&str] = &[
    "ui.createConversation",
    "ui.updateConversation",
    "ui.renameConversation",
    "ui.pinConversation",
    "ui.updateConversationProfile",
    "ui.deleteConversation",
    "ui.deleteConversations",
    "ui.sendConversationMessage",
    "ui.stopConversation",
    "ui.chat",
    "ui.stopChat",
];
pub fn query(m: &str, p: &Value, s: &Snapshot, c: &Context) -> Result<Value> {
    let (method, params) = match m {
        "ui.conversations" => ("conversation.list", json!({})),
        "ui.conversationMessages" => (
            "conversation.messages",
            json!({"id":p["id"],"since":p["since"]}),
        ),
        "ui.conversationHistory" => (
            "conversation.messages",
            json!({"id":p["id"],"before":p["before"],"limit":p["limit"],"history":true}),
        ),
        "ui.conversationMsgDetail" => ("conversation.detail", p.clone()),
        "ui.chatStatus" => ("chat.status", json!({"task_id":p["taskId"]})),
        _ => return Err(artex_core::Error::new("METHOD_NOT_FOUND", m)),
    };
    artex_domain::query(method, &params, s, c)
}
pub fn plan(m: &str, p: &Value, s: &Snapshot, c: &Context) -> Result<CommandBatch> {
    let (method, params) = match m {
        "ui.createConversation" => ("conversation.create", p.clone()),
        "ui.updateConversation" => (
            "conversation.update",
            json!({"id":p["id"],"patch":p["input"]}),
        ),
        "ui.renameConversation" => (
            "conversation.update",
            json!({"id":p["id"],"patch":{"title":p["title"]}}),
        ),
        "ui.pinConversation" => (
            "conversation.update",
            json!({"id":p["id"],"patch":{"pinned":p["pinned"]}}),
        ),
        "ui.updateConversationProfile" => ("conversation.profile", p.clone()),
        "ui.deleteConversation" => ("conversation.delete", p.clone()),
        "ui.deleteConversations" => ("conversation.delete_batch", p.clone()),
        "ui.sendConversationMessage" => ("conversation.send", p.clone()),
        "ui.stopConversation" => ("conversation.stop", p.clone()),
        "ui.chat" => {
            let mut parameters = p.clone();
            if let Some(task) = p.get("task") {
                parameters["task_id"] = task.clone();
            }
            ("chat.send", parameters)
        }
        "ui.stopChat" => ("chat.stop", json!({"task_id":p["taskId"]})),
        _ => return Err(artex_core::Error::new("METHOD_NOT_FOUND", m)),
    };
    let mut b = artex_domain::plan(method, &params, s, c)?;
    if m == "ui.deleteConversation" {
        b.result = json!({"deleted":p["id"]});
    }
    Ok(b)
}
