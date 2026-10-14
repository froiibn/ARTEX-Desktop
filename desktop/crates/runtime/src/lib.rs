#![forbid(unsafe_code)]
//! Provider-neutral requests and a private host boundary. Neither owns the writer.
pub mod permission;
pub mod provider;
use artex_core::{Context, Result};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};

pub trait HostGateway: Send + Sync {
    /// Called by the writer after controller cancellation, before recording
    /// native retirement proofs and the final storage checkpoint.
    /// Native implementations must confirm owned external work has stopped.
    fn retire(&self, _timeout: Duration) -> Result<()> {
        Ok(())
    }
    /// Optional native proof, produced only after retirement has actually
    /// completed. Absence is not evidence of process or request termination.
    fn retirement_report(&self, _workspace: &str, _contexts: &[Context]) -> Result<Option<Value>> {
        Ok(None)
    }
    fn request(
        &self,
        workspace: &str,
        context: &Context,
        operation: &str,
        parameters: Value,
        timeout: Duration,
    ) -> Result<Value>;
}
#[derive(Clone, Default)]
pub struct Cancellation(Arc<AtomicBool>);
impl Cancellation {
    pub fn cancel(&self) {
        self.0.store(true, Ordering::Release);
    }
    pub fn cancelled(&self) -> bool {
        self.0.load(Ordering::Acquire)
    }
    pub fn check(&self) -> Result<()> {
        if self.cancelled() {
            Err(artex_core::Error::new("CANCELLED", "Run cancelled"))
        } else {
            Ok(())
        }
    }
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ToolCall {
    pub id: String,
    pub name: String,
    pub input: Value,
}
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Turn {
    pub text: String,
    pub thinking: String,
    pub thinking_signature: String,
    pub tools: Vec<ToolCall>,
    pub usage: Value,
    pub stop_reason: String,
    pub raw_response: Value,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Message {
    pub role: String,
    pub content: Vec<Value>,
}
#[derive(Clone, Debug)]
pub struct Prompt {
    pub system: String,
    pub messages: Vec<Message>,
    pub tools: Vec<Value>,
    pub session_id: String,
}
