//! The three lines every integration test opened with, in one place.
//!
//! Each `tests/*.rs` is its own binary, so this module is compiled into each of
//! them and every one uses a subset — hence the blanket `dead_code` allow. That
//! is the standard tax on a shared test harness in Cargo, and it is cheaper
//! than the three copies of `temp_db_path` that were here before.
#![allow(dead_code)]

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::Arc;

use agent_platform_server::{router, AppState};

pub const MASTER: &str = "master-key-under-test";

static SEQ: AtomicU32 = AtomicU32::new(0);

/// A database path no other test will pick. The pid separates concurrent
/// `cargo test` runs and the counter separates tests inside one binary — two
/// tests sharing a file is a flake that only shows up under load.
///
/// `tag` names the binary, so a leftover file says which test left it.
pub fn temp_db_path(tag: &str) -> PathBuf {
    let n = SEQ.fetch_add(1, Ordering::Relaxed);
    let pid = std::process::id();
    std::env::temp_dir().join(format!("agp-{tag}-{pid}-{n}.db"))
}

/// The real router on an ephemeral port. No schema: a caller that needs tables
/// either seeds its own or is testing something that never reaches one.
///
/// `master_key` is `None` for the tests that check what an unconfigured server
/// does.
pub async fn start_server(db: &Path, master_key: Option<&str>) -> String {
    let state = Arc::new(AppState::new(db, master_key.map(str::to_owned)));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let origin = format!("http://{}", listener.local_addr().unwrap());
    tokio::spawn(async move { axum::serve(listener, router(state)).await.unwrap() });
    origin
}
