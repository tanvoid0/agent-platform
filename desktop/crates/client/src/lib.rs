//! Typed Rust client for the agent-platform API (`/api/v1`).
//!
//! Contract mirrors the frozen web client (`web/src/api/*`); enums are generated
//! from `app/shared_enums.py` by `scripts/sync_contract_enums.py`.

pub mod client;
pub mod dag;
pub mod enums;
pub mod sse;
pub mod types;

pub use client::{Client, DorkRequest, Error, Result};
