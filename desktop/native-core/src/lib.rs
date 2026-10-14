#![forbid(unsafe_code)]
//! One-process desktop kernel. The UI only submits typed work and reads pinned
//! immutable generations; the writer owns the Router and all store mutations.
pub mod gateway;
pub mod lifecycle;
pub mod service;
pub use artex_rpc::native::*;
pub use service::{
    HostFactory, NativeEvent, NativeEventBatch, Service, ServiceConfig, ServiceHandle,
    ServicePhase, ServiceStatus,
};
