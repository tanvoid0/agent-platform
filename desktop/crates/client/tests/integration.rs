//! Integration tests against a live server.
//!
//! Skipped unless `AGENT_PLATFORM_TEST_URL` (+ optional `AGENT_PLATFORM_TEST_KEY`)
//! is set. Point it at a throwaway server, e.g.:
//!
//! ```text
//! AGENT_PLATFORM_PORT=18499 AGENT_PLATFORM_MASTER_KEY=test-key \
//!   python scripts/start.py --skip-build --no-browser
//! AGENT_PLATFORM_TEST_URL=http://127.0.0.1:18499 AGENT_PLATFORM_TEST_KEY=test-key \
//!   cargo test -p agent-platform-client -- --nocapture
//! ```

use agent_platform_client::sse::{process_stream, SseItem};
use agent_platform_client::types::*;
use agent_platform_client::Client;
use futures::StreamExt;

fn client() -> Option<Client> {
    let url = std::env::var("AGENT_PLATFORM_TEST_URL").ok()?;
    let key = std::env::var("AGENT_PLATFORM_TEST_KEY").unwrap_or_default();
    Some(Client::new(url, key))
}

macro_rules! require_server {
    () => {
        match client() {
            Some(c) => c,
            None => {
                eprintln!("skipped: AGENT_PLATFORM_TEST_URL not set");
                return;
            }
        }
    };
}

#[tokio::test]
async fn health_and_system_status() {
    let c = require_server!();
    c.health().await.expect("health");
    let status = c.system_status().await.expect("system_status");
    assert!(!status.service.is_empty());
    assert!(status.listening_on.port > 0);
}

#[tokio::test]
async fn system_logs_cursor() {
    let c = require_server!();
    let first = c.system_logs(0).await.expect("logs");
    // Cursor advances monotonically; polling from `next` returns only new lines.
    let second = c.system_logs(first.next).await.expect("logs cursor");
    assert!(second.next >= first.next);
}

#[tokio::test]
async fn projects_crud() {
    let c = require_server!();
    let created = c
        .create_project(&ProjectBody {
            name: format!("it-proj-{}", std::process::id()),
            description: Some("integration test".into()),
            color: Some("#123456".into()),
        })
        .await
        .expect("create project");

    let listed = c.projects().await.expect("list projects");
    assert!(listed.projects.iter().any(|p| p.id == created.id));

    let renamed = c
        .update_project(
            created.id,
            &ProjectBody { name: format!("{}-renamed", created.name), description: None, color: None },
        )
        .await
        .expect("update project");
    assert!(renamed.name.ends_with("-renamed"));

    c.delete_project(created.id).await.expect("delete project");
}

#[tokio::test]
async fn teams_crud_and_process_lifecycle() {
    let c = require_server!();
    let team = c
        .create_team(&TeamTemplateBody {
            name: format!("it-team-{}", std::process::id()),
            description: None,
            color: None,
            category: Some("integration".into()),
            roster: TeamRoster {
                roles: vec![RosterRole {
                    id: "r1".into(),
                    name: "Worker".into(),
                    description: None,
                    modality: None,
                    parent_id: None,
                    accent_color: None,
                }],
            },
        })
        .await
        .expect("create team");

    let detail = c.team_detail(team.id).await.expect("team detail");
    assert_eq!(detail.roster.roles.len(), 1);

    // Process: create -> cancel -> detail/events -> SSE terminal frame.
    let created = c
        .create_process(&CreateProcessBody {
            goal: "integration test goal (will be cancelled)".into(),
            team_template_id: team.id,
            auto_approve: Some(false),
            project_id: None,
        })
        .await
        .expect("create process");

    let cancelled = c.cancel_process(created.process_id).await.expect("cancel");
    assert!(!cancelled.status.is_empty());

    let detail = c.process_detail(created.process_id).await.expect("detail");
    assert_eq!(detail.process.id, created.process_id);
    // Cancel raced against planning: on a server with no LLM configured planning
    // may fail first. Either way the process must be terminal.
    assert!(
        matches!(detail.process.status, ProcessStatus::Cancelled | ProcessStatus::Failed),
        "unexpected status {:?}",
        detail.process.status
    );

    let events = c
        .process_events(created.process_id, None, 100)
        .await
        .expect("events");
    assert!(events.events.iter().all(|e| e.process_id == created.process_id));

    // Stream on a terminal process: with a backlog the server replays rows and
    // closes with NO sentinel (client then reports Reconnecting); without a
    // backlog it sends a terminal sentinel. Accept either — a consumer gates the
    // subscription on polled status.
    let mut saw_end_signal = false;
    let mut saw_event = false;
    let mut stream = Box::pin(process_stream(c.clone(), created.process_id));
    let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(10);
    while let Ok(Some(item)) = tokio::time::timeout_at(deadline, stream.next()).await {
        match item {
            SseItem::Event(ev) if ev.is_terminal() => {
                saw_end_signal = true;
                break;
            }
            SseItem::Event(_) | SseItem::Raw(_) => saw_event = true,
            SseItem::Reconnecting { .. } => {
                saw_end_signal = true;
                break;
            }
        }
    }
    assert!(
        saw_end_signal,
        "expected terminal sentinel or clean close (saw_event={saw_event})"
    );

    c.delete_team(team.id).await.expect("delete team");
}

#[tokio::test]
async fn model_ops_lists() {
    let c = require_server!();
    c.model_projects().await.expect("model projects");
    c.model_registry().await.expect("registry");
    // Ollama may or may not be running locally; both outcomes are valid.
    let _ = c.ollama_models().await;
}

#[tokio::test]
async fn model_project_file_upload() {
    let c = require_server!();
    let name = format!("it-model-{}", std::process::id());
    c.create_model_project(&ModelProjectBody {
        name: name.clone(),
        description: Some("integration test".into()),
        base_model: None,
        ollama_tag: None,
    })
    .await
    .expect("create model project");

    let uploaded = c
        .upload_project_file(&name, "datasets/train.jsonl", b"{}\n".to_vec())
        .await
        .expect("upload file");
    assert_eq!(uploaded.uploaded, 1);
}

#[tokio::test]
async fn process_list_requires_scope_and_filters() {
    let c = require_server!();
    let unassigned = c
        .processes(10, ProcessListFilter::Unassigned)
        .await
        .expect("unassigned list");
    assert!(unassigned.processes.len() <= 10);
}
