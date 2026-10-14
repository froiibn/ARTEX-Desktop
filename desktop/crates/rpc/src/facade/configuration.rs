use artex_core::{CommandBatch, Context, Result, Snapshot};
use serde_json::Value;
pub fn query(m: &str, p: &Value, s: &Snapshot, c: &Context) -> Result<Value> {
    artex_domain::configuration_ui::query(m, p, s, c)
}
pub fn plan(m: &str, p: &Value, s: &Snapshot, c: &Context) -> Result<CommandBatch> {
    artex_domain::configuration_ui::plan(m, p, s, c)
}
