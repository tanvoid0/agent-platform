//! Docs — a read-only browser over a project's file sandbox (the Confluence
//! surface of the company workspace). The server exposes the project files on
//! `/projects/{id}/files/list` and `/files/file`; ingested PDFs show up here as
//! their derived markdown, so a dropped document is readable without leaving the
//! app.
//!
//! One directory at a time: `path` is the directory being shown (""=root), the
//! left rail lists it (folders first), and selecting a file loads its text into
//! the reader on the right. Folder-based, no `project_id` beyond scoping the
//! sandbox — switching project reloads from the root.

use crate::domain::err_string;
use agent_platform_client::types::{WorkspaceEntry, WorkspaceFile};
use agent_platform_client::Client;
use iced::widget::markdown;
use iced::Task;

#[derive(Debug, Default)]
pub struct State {
    pub project: Option<i64>,
    /// The directory currently listed; "" is the project root.
    pub path: String,
    pub entries: Vec<WorkspaceEntry>,
    /// The file whose content is in the reader, if any.
    pub selected: Option<String>,
    /// Parsed on load, not per frame — the same discipline as the chat panes.
    pub md: Vec<markdown::Item>,
    /// `"text"` or `"pdf_derived_markdown"`, for the caption.
    pub content_kind: Option<String>,
    pub loading: bool,
    pub error: Option<String>,
}

#[derive(Debug, Clone)]
pub enum Message {
    /// "View logs" on a traced error banner — intercepted in `main::update`
    /// before it reaches here, like the other modules' `TraceLogs`.
    TraceLogs(String),
    EntriesLoaded(Result<Vec<WorkspaceEntry>, String>),
    /// A row was clicked: a folder descends, a file loads into the reader.
    Open(WorkspaceEntry),
    FileLoaded(Result<Box<WorkspaceFile>, String>),
    /// Up to the parent directory.
    Up,
    LinkClicked(String),
    DismissError,
}

fn load_list(client: &Client, project: i64, path: &str) -> Task<Message> {
    let (c, path) = (client.clone(), path.to_string());
    Task::perform(
        async move { err_string(c.project_files_list(project, &path).await).map(|r| r.entries) },
        Message::EntriesLoaded,
    )
}

fn load_file(client: &Client, project: i64, path: &str) -> Task<Message> {
    let (c, path) = (client.clone(), path.to_string());
    Task::perform(
        async move { err_string(c.project_file_read(project, &path).await).map(Box::new) },
        Message::FileLoaded,
    )
}

/// Point the browser at a project and list its root. A no-op if already there,
/// so re-entering the tab does not reload the user out from under their reading.
pub fn set_project(state: &mut State, client: &Client, project: Option<i64>) -> Task<Message> {
    if state.project == project {
        return Task::none();
    }
    state.project = project;
    state.path = String::new();
    state.entries.clear();
    state.selected = None;
    state.md.clear();
    state.content_kind = None;
    state.error = None;
    match project {
        Some(id) => {
            state.loading = true;
            load_list(client, id, "")
        }
        None => Task::none(),
    }
}

pub fn update(state: &mut State, client: &Client, message: Message) -> Task<Message> {
    let Some(project) = state.project else { return Task::none() };
    match message {
        Message::TraceLogs(_) => Task::none(),
        Message::EntriesLoaded(Ok(entries)) => {
            state.loading = false;
            state.error = None;
            state.entries = entries;
            Task::none()
        }
        Message::Open(entry) if entry.kind == "dir" => {
            state.path = entry.path;
            state.loading = true;
            load_list(client, project, &state.path)
        }
        Message::Open(entry) => {
            state.selected = Some(entry.path.clone());
            state.loading = true;
            load_file(client, project, &entry.path)
        }
        Message::FileLoaded(Ok(file)) => {
            state.loading = false;
            state.error = None;
            state.md = markdown::parse(&file.content).collect();
            state.content_kind = Some(file.content_kind.clone());
            Task::none()
        }
        Message::Up => {
            // Strip the last segment; the root ("") stays the root.
            state.path = match state.path.rsplit_once('/') {
                Some((parent, _)) => parent.to_string(),
                None => String::new(),
            };
            state.loading = true;
            load_list(client, project, &state.path)
        }
        Message::LinkClicked(url) => {
            if url.starts_with("http://") || url.starts_with("https://") {
                crate::shell::reveal_path(&url);
            }
            Task::none()
        }
        Message::DismissError => {
            state.error = None;
            Task::none()
        }
        Message::EntriesLoaded(Err(e)) | Message::FileLoaded(Err(e)) => {
            state.loading = false;
            state.error = Some(e);
            Task::none()
        }
    }
}

#[cfg(test)]
mod tests {
    use agent_platform_client::types::WorkspaceListResponse;

    /// The list endpoint's shape: folders and files with a `type` tag. This is
    /// the one net-new parse in the Docs tab, so it gets the one check.
    #[test]
    fn list_response_parses_entries() {
        let body = r#"{"entries":[
            {"name":"documents","path":"documents","type":"dir"},
            {"name":"notes.md","path":"notes.md","type":"file"}
        ]}"#;
        let parsed: WorkspaceListResponse = serde_json::from_str(body).unwrap();
        assert_eq!(parsed.entries.len(), 2);
        assert_eq!(parsed.entries[0].kind, "dir");
        assert_eq!(parsed.entries[1].name, "notes.md");
        assert_eq!(parsed.entries[1].path, "notes.md");
    }
}
