//! Saved conversations for the Chat and E.V. tabs — the sidebar of past chats.
//!
//! One plain JSON file next to `settings.json`, like `memories.json`: readable,
//! backupable and deletable without the app's help. The live thread is
//! autosaved into its conversation as each turn completes; there is no explicit
//! save button anywhere.

use agent_platform_client::types::ChatMessage;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::{Path, PathBuf};

const FILE: &str = "chats.json";

/// Conversations kept overall, oldest out first — the file rides along on every
/// autosave, so it must not grow without bound.
const MAX_ITEMS: usize = 200;

/// Characters of the first user message shown as a sidebar title.
const TITLE_LEN: usize = 42;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Conversation {
    pub id: u64,
    /// Which tab it belongs to: "Chat" or E.V.'s name.
    pub source: String,
    pub title: String,
    /// Local `YYYY-MM-DD HH:MM`, shown in the sidebar and used to sort it.
    pub updated: String,
    pub messages: Vec<ChatMessage>,
    /// Display-only chain-of-thought, same indices as `messages`.
    #[serde(default)]
    pub reasoning: Vec<String>,
    /// Provider/model the thread was answered on, so reopening it answers the
    /// same way. `None` is a conversation saved before this was recorded —
    /// distinct from `Some("")`, which is an explicit "the server's default".
    #[serde(default)]
    pub provider: Option<String>,
    #[serde(default)]
    pub model: Option<String>,
}

/// The persisted half of [`Store`] — which conversation each tab is on is
/// session state, not history, so it stays out of the file.
#[derive(Debug, Default, Serialize, Deserialize)]
#[serde(default)]
struct Saved {
    items: Vec<Conversation>,
    next_id: u64,
}

pub struct Store {
    pub items: Vec<Conversation>,
    next_id: u64,
    dir: PathBuf,
    /// Conversation each tab is currently on; absent = a fresh thread.
    current: HashMap<String, u64>,
}

#[derive(Debug, Clone)]
pub enum Message {
    /// Detach from the open conversation and start a fresh thread.
    New,
    /// Load a saved conversation into the open tab.
    Select(u64),
    Delete(u64),
}

impl Store {
    pub fn load(dir: &Path) -> Self {
        let mut saved: Saved = std::fs::read_to_string(dir.join(FILE))
            .ok()
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or_default();
        // The plain Chat tab was merged into E.V.; its saved threads move with
        // it, or they would sit in the file invisible to every sidebar.
        for c in saved.items.iter_mut().filter(|c| c.source == "Chat") {
            c.source = crate::assistant::NAME.to_string();
        }
        let next_id =
            saved.next_id.max(saved.items.iter().map(|c| c.id + 1).max().unwrap_or(1)).max(1);
        Self { items: saved.items, next_id, dir: dir.to_path_buf(), current: HashMap::new() }
    }

    /// Failures are silent, like the memory harvester's: chat history that nags
    /// about a read-only disk on every turn is worse than history with a gap.
    fn save(&self) {
        let saved = Saved { items: self.items.clone(), next_id: self.next_id };
        let _ = std::fs::create_dir_all(&self.dir).and_then(|_| {
            crate::shell::write_atomic(
                &self.dir.join(FILE),
                &serde_json::to_string_pretty(&saved).unwrap(),
            )
        });
    }

    pub fn current(&self, source: &str) -> Option<u64> {
        self.current.get(source).copied()
    }

    /// Detach the tab from its conversation — the next turn starts a new one.
    /// The saved conversation itself stays.
    pub fn close(&mut self, source: &str) {
        self.current.remove(source);
    }

    /// Write the tab's live thread into its conversation, creating one on the
    /// first turn. No-op for an empty thread.
    pub fn autosave(
        &mut self,
        source: &str,
        messages: &[ChatMessage],
        reasoning: &[String],
        provider: &str,
        model: &str,
    ) {
        if messages.is_empty() {
            return;
        }
        let title = title_of(messages);
        let updated = chrono::Local::now().format("%Y-%m-%d %H:%M").to_string();
        let open = self
            .current(source)
            .and_then(|id| self.items.iter_mut().find(|c| c.id == id));
        match open {
            Some(c) => {
                c.title = title;
                c.updated = updated;
                c.messages = messages.to_vec();
                c.reasoning = reasoning.to_vec();
                c.provider = Some(provider.to_string());
                c.model = Some(model.to_string());
            }
            None => {
                let id = self.next_id;
                self.next_id += 1;
                self.items.push(Conversation {
                    id,
                    source: source.to_string(),
                    title,
                    updated,
                    messages: messages.to_vec(),
                    reasoning: reasoning.to_vec(),
                    provider: Some(provider.to_string()),
                    model: Some(model.to_string()),
                });
                self.current.insert(source.to_string(), id);
                while self.items.len() > MAX_ITEMS {
                    let dropped = self.items.remove(0);
                    self.current.retain(|_, id| *id != dropped.id);
                }
            }
        }
        self.save();
    }

    /// Make `id` the tab's open conversation and hand back a copy to load.
    pub fn open(&mut self, source: &str, id: u64) -> Option<Conversation> {
        let found = self.items.iter().find(|c| c.id == id)?.clone();
        self.current.insert(source.to_string(), id);
        Some(found)
    }

    pub fn delete(&mut self, id: u64) {
        self.items.retain(|c| c.id != id);
        self.current.retain(|_, cur| *cur != id);
        self.save();
    }

    /// The sidebar's rows: this tab's conversations, most recently touched first.
    pub fn visible(&self, source: &str) -> Vec<&Conversation> {
        let mut rows: Vec<&Conversation> =
            self.items.iter().filter(|c| c.source == source).collect();
        rows.sort_by(|a, b| b.updated.cmp(&a.updated).then(b.id.cmp(&a.id)));
        rows
    }
}

/// A conversation is named after the first thing the user asked.
fn title_of(messages: &[ChatMessage]) -> String {
    let text = messages
        .iter()
        .find(|m| m.role == "user")
        .map(|m| m.content.as_str())
        .unwrap_or("");
    let mut title = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if title.chars().count() > TITLE_LEN {
        title = title.chars().take(TITLE_LEN).collect::<String>().trim_end().to_string();
        title.push('…');
    }
    if title.is_empty() {
        "New chat".to_string()
    } else {
        title
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn store() -> Store {
        Store::load(&std::env::temp_dir().join(format!(
            "ev-history-test-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        )))
    }

    fn thread(prompt: &str) -> Vec<ChatMessage> {
        vec![ChatMessage::text("user", prompt), ChatMessage::text("assistant", "hello")]
    }

    #[test]
    fn autosave_creates_then_updates_one_conversation() {
        let mut s = store();
        s.autosave("Chat", &thread("first question"), &[String::new(), String::new()], "", "");
        assert_eq!(s.items.len(), 1);
        let id = s.current("Chat").expect("the new conversation is the open one");

        let mut longer = thread("first question");
        longer.push(ChatMessage::text("user", "and another"));
        s.autosave("Chat", &longer, &[], "", "");
        assert_eq!(s.items.len(), 1, "same conversation, not a second one");
        assert_eq!(s.items[0].messages.len(), 3);
        assert_eq!(s.current("Chat"), Some(id));

        // A fresh thread after New lands in a new conversation.
        s.close("Chat");
        s.autosave("Chat", &thread("something else"), &[], "", "");
        assert_eq!(s.items.len(), 2);
        assert_ne!(s.current("Chat"), Some(id));

        // Empty threads are never saved.
        s.close("Chat");
        s.autosave("Chat", &[], &[], "", "");
        assert_eq!(s.items.len(), 2);
    }

    /// The plain Chat tab is gone; its saved threads must show up under E.V.
    /// rather than sitting in the file with no sidebar that lists them.
    #[test]
    fn old_chat_threads_move_to_the_assistant_on_load() {
        let dir = std::env::temp_dir().join(format!("ev-history-migrate-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let mut s = Store::load(&dir);
        s.autosave("Chat", &thread("plain chat"), &[], "", "");

        let reopened = Store::load(&dir);
        assert!(reopened.visible("Chat").is_empty());
        assert_eq!(reopened.visible(crate::assistant::NAME).len(), 1);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn conversations_survive_a_restart_and_tabs_stay_apart() {
        let dir = std::env::temp_dir()
            .join(format!("ev-history-persist-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let mut s = Store::load(&dir);
        s.autosave("run 7", &thread("plain chat"), &[], "ollama", "qwen2.5:7b");
        s.autosave("E.V.", &thread("suit check"), &[], "", "");

        let mut reopened = Store::load(&dir);
        assert_eq!(reopened.items.len(), 2);
        assert_eq!(reopened.current("run 7"), None, "which chat was open is session state");
        assert_eq!(reopened.visible("run 7").len(), 1);
        assert_eq!(reopened.visible("E.V.")[0].title, "suit check");

        let id = reopened.visible("run 7")[0].id;
        let loaded = reopened.open("run 7", id).expect("saved conversation loads");
        assert_eq!(loaded.messages[0].content, "plain chat");
        assert_eq!(loaded.provider.as_deref(), Some("ollama"), "the pair reopens with the thread");
        assert_eq!(loaded.model.as_deref(), Some("qwen2.5:7b"));
        assert_eq!(reopened.current("run 7"), Some(id));

        reopened.delete(id);
        assert_eq!(reopened.current("run 7"), None, "deleting the open chat closes it");
        assert_eq!(Store::load(&dir).items.len(), 1);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn titles_come_from_the_first_user_turn() {
        assert_eq!(title_of(&thread("  hello   world  ")), "hello world");
        assert_eq!(title_of(&[]), "New chat");
        let long = "x".repeat(100);
        assert!(title_of(&thread(&long)).chars().count() <= TITLE_LEN + 1);
        assert!(title_of(&thread(&long)).ends_with('…'));
    }
}
