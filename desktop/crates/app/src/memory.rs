//! Long-term memory for the assistants — the ChatGPT-style kind: durable facts
//! about the user, picked out of ordinary conversations and carried into unrelated
//! ones later.
//!
//! Two halves, deliberately separate:
//!
//! * **Harvest.** After a turn finishes, one cheap non-streamed call asks the
//!   model what — if anything — in that exchange is worth keeping. It sees what
//!   is already remembered, so it returns only what is new. Failures are silent:
//!   background memory that nags about a dropped connection is worse than
//!   memory that occasionally misses something.
//! * **Recall.** Everything remembered is rendered into one system block and
//!   prepended to every conversation. No retrieval, no embeddings — a few
//!   hundred words of facts costs less than the machinery to rank them.
//!
//! The store is a plain JSON file next to `settings.json`, so a user can read,
//! back up or delete their memories without the app's help.

use agent_platform_client::sse::ChatChunk;
use agent_platform_client::types::{ChatCompletionBody, ChatMessage};
use agent_platform_client::Client;
use futures::StreamExt;
use iced::Task;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

const FILE: &str = "memories.json";

/// Hard ceiling on remembered facts. Every one of them rides along in the system
/// prompt of every turn, so the cost of the collection is paid continuously.
/// Past this the oldest is dropped — memory that never forgets stops being
/// memory and becomes a transcript.
const MAX_ITEMS: usize = 200;

/// Most a single harvest may add. A long exchange that "learns" eight things
/// about you has almost certainly learned one thing and paraphrased it.
const MAX_PER_HARVEST: usize = 3;

/// Longest a single memory may be. Anything longer is a summary of the
/// conversation, which is not what this is for.
const MAX_LEN: usize = 240;

/// Turns of context handed to the harvester — the exchange that just closed.
const HARVEST_TURNS: usize = 2;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Memory {
    pub id: u64,
    pub text: String,
    /// Which assistant the fact came from, or "You" when typed by hand.
    pub source: String,
    /// Local date, `YYYY-MM-DD`. Shown in the dashboard so a stale fact is
    /// recognisable as one.
    pub created: String,
}

/// The persisted half of [`Store`]. Kept separate from the live struct so the
/// dashboard's transient fields (search box, open editor) never reach the disk.
#[derive(Debug, Default, Serialize, Deserialize)]
#[serde(default)]
struct Saved {
    items: Vec<Memory>,
    enabled: bool,
    next_id: u64,
}

pub struct Store {
    pub items: Vec<Memory>,
    /// Master switch. Off means nothing is harvested *and* nothing is recalled;
    /// what is already stored stays stored, so turning it back on restores it.
    pub enabled: bool,
    next_id: u64,
    dir: PathBuf,
    /// Last write error, surfaced by the dashboard — a read-only data directory
    /// would otherwise drop memories silently at every mutation.
    pub error: Option<String>,

    // --- dashboard state, never persisted ---
    pub search: String,
    /// The "add a memory" composer.
    pub draft: String,
    /// The row being edited and its working copy. One at a time: two open
    /// editors is a save-order question nobody wants to answer.
    pub editing: Option<(u64, String)>,
    /// A harvest is in flight; the dashboard says so rather than looking idle
    /// while new facts are on their way.
    pub harvesting: bool,
    /// "Forget all" is armed: the next click really wipes everything. Anything
    /// else the user does disarms it.
    pub confirm_forget: bool,
    /// What the last harvest just saved, surfaced as a banner in the chat it
    /// came from so the user can discard a wrong guess on the spot.
    pub notice: Option<Notice>,
}

/// The facts one harvest added: `(id, text)` pairs, so Discard can delete
/// exactly those rows and nothing typed or edited since.
#[derive(Debug, Clone)]
pub struct Notice {
    pub source: String,
    pub facts: Vec<(u64, String)>,
}

impl Store {
    pub fn load(dir: &Path) -> Self {
        let saved: Saved = std::fs::read_to_string(dir.join(FILE))
            .ok()
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or(Saved { items: Vec::new(), enabled: true, next_id: 1 });
        // A file written before `next_id` existed (or hand-edited) must not hand
        // out an id that is already taken.
        let next_id =
            saved.next_id.max(saved.items.iter().map(|m| m.id + 1).max().unwrap_or(1)).max(1);
        Self {
            items: saved.items,
            enabled: saved.enabled,
            next_id,
            dir: dir.to_path_buf(),
            error: None,
            search: String::new(),
            draft: String::new(),
            editing: None,
            harvesting: false,
            confirm_forget: false,
            notice: None,
        }
    }

    fn save(&mut self) {
        let saved =
            Saved { items: self.items.clone(), enabled: self.enabled, next_id: self.next_id };
        let write = std::fs::create_dir_all(&self.dir).and_then(|_| {
            std::fs::write(self.dir.join(FILE), serde_json::to_string_pretty(&saved).unwrap())
        });
        self.error = write.err().map(|e| format!("Could not save memories: {e}"));
    }

    /// Everything remembered, as the system message that leads every
    /// conversation. `None` when memory is off or empty — callers pass it
    /// straight into a system slot, and an empty header is worse than nothing.
    pub fn system_block(&self) -> Option<String> {
        if !self.enabled || self.items.is_empty() {
            return None;
        }
        // Each fact carries its id: without one, "forget the thing about Rust"
        // makes the model guess which row that is, and a live run watched it
        // guess wrong and delete the user's address instead.
        let facts = self
            .items
            .iter()
            .map(|m| format!("- [{}] {}", m.id, m.text))
            .collect::<Vec<_>>()
            .join("\n");
        Some(format!(
            "What you remember about the user from previous conversations. Use it \
             when it is relevant and never recite it back unprompted. The bracketed \
             number is the memory's id for update_memory and forget — never say an \
             id out loud:\n{facts}"
        ))
    }

    fn add(&mut self, text: String, source: &str) -> bool {
        let text = clean(&text);
        if text.is_empty() || self.duplicate(&text) {
            return false;
        }
        let id = self.next_id;
        self.next_id += 1;
        self.items.push(Memory {
            id,
            text,
            source: source.to_string(),
            created: chrono::Local::now().format("%Y-%m-%d").to_string(),
        });
        // Oldest out first: the collection is a window on who the user is now.
        while self.items.len() > MAX_ITEMS {
            self.items.remove(0);
        }
        true
    }

    /// Is this fact already known? Compared on letters and digits alone, so
    /// "User prefers Rust." and "user prefers rust" are one memory, not two.
    fn duplicate(&self, text: &str) -> bool {
        let key = fingerprint(text);
        self.items.iter().any(|m| fingerprint(&m.text) == key)
    }

    /// Rows the dashboard shows, newest first and filtered by the search box.
    pub fn visible(&self) -> Vec<&Memory> {
        let needle = self.search.trim().to_lowercase();
        self.items
            .iter()
            .rev()
            .filter(|m| needle.is_empty() || m.text.to_lowercase().contains(&needle))
            .collect()
    }

    /// Ask the model what in the exchange that just closed is worth keeping.
    ///
    /// Returns [`Task::none`] when there is nothing to do — memory off, a
    /// harvest already running, or no complete exchange yet — so callers can
    /// fire it after every turn without checking anything themselves.
    pub fn harvest(&mut self, client: &Client, messages: &[ChatMessage], source: &str) -> Task<Message> {
        if !self.enabled || self.harvesting || messages.len() < HARVEST_TURNS {
            return Task::none();
        }
        let exchange: String = messages[messages.len() - HARVEST_TURNS..]
            .iter()
            .map(|m| format!("{}: {}\n", role_label(&m.role), m.content.trim()))
            .collect();
        if exchange.trim().is_empty() {
            return Task::none();
        }
        let known = if self.items.is_empty() {
            "(nothing yet)".to_string()
        } else {
            self.items.iter().map(|m| format!("- {}\n", m.text)).collect()
        };

        self.harvesting = true;
        let source = source.to_string();
        let body = ChatCompletionBody {
            messages: vec![
                ChatMessage::text("system", HARVESTER),
                ChatMessage::text(
                    "user",
                    format!("Already remembered:\n{known}\nExchange:\n{exchange}"),
                ),
            ],
            model: None,
            provider: None,
            // Extraction, not writing: the same exchange should yield the same
            // facts twice in a row.
            temperature: Some(0.0),
            max_tokens: Some(200),
            tools: None,
            stream: Some(true),
        };
        let client = client.clone();
        Task::perform(
            async move {
                let reply = crate::inference::chat_stream(client, body)
                    .fold(String::new(), |mut acc, chunk| async move {
                        if let ChatChunk::Delta(text) = chunk {
                            acc.push_str(&text);
                        }
                        acc
                    })
                    .await;
                (parse_harvest(&reply), source)
            },
            |(facts, source)| Message::Harvested(facts, source),
        )
    }
}

// --- The toolkit ------------------------------------------------------------
// Harvest is the passive half: it guesses. These are the active half — what the
// assistant reaches for when the user *says* "remember this", "that's wrong,
// change it" or "forget that". Without them the only honest answer to "forget
// my address" was to open the dashboard and do it by hand.

/// Tool names handled here. The assistant checks this before dispatching a call
/// to the terminal.
pub const TOOLS: [&str; 4] = ["list_memories", "remember", "update_memory", "forget"];

/// The memory half of the assistant's tool spec, in OpenAI function form.
pub fn tools_spec() -> Vec<serde_json::Value> {
    let tool = |name: &str, description: &str, properties: serde_json::Value, required: &[&str]| {
        serde_json::json!({
            "type": "function",
            "function": {
                "name": name,
                "description": description,
                "parameters": {
                    "type": "object",
                    "properties": properties,
                    "required": required,
                }
            }
        })
    };
    let text = |description: &str| serde_json::json!({ "type": "string", "description": description });
    let id = serde_json::json!({
        "type": "integer",
        "description": "The id from list_memories."
    });
    vec![
        tool(
            "list_memories",
            "List every long-term memory with its id. Call this before updating or \
             forgetting one — the id is the only way to name it.",
            serde_json::json!({}),
            &[],
        ),
        tool(
            "remember",
            "Save one durable fact about the user to long-term memory. Use it when the \
             user asks you to remember something.",
            serde_json::json!({
                "text": text("The fact, as a short third-person statement. \"Lives in London.\"")
            }),
            &["text"],
        ),
        tool(
            "update_memory",
            "Replace the text of one remembered fact, when the user corrects it.",
            serde_json::json!({ "id": id, "text": text("The corrected fact.") }),
            &["id", "text"],
        ),
        tool(
            "forget",
            "Delete one remembered fact for good, when the user asks you to forget it.",
            serde_json::json!({ "id": id }),
            &["id"],
        ),
    ]
}

/// Run one memory tool call. `None` means the call was for some other tool.
///
/// Synchronous and infallible on purpose: the store is right here in memory, and
/// every problem the model can cause (a bad id, a blank fact, memory switched
/// off) is answered in the tool result so it can correct itself.
pub fn run_tool(store: &mut Store, name: &str, arguments: &str) -> Option<String> {
    if !TOOLS.contains(&name) {
        return None;
    }
    if !store.enabled {
        return Some(
            "error: long-term memory is switched off. Tell the user to turn it back on \
             in the Memory dashboard."
                .into(),
        );
    }
    let args: serde_json::Value = serde_json::from_str(arguments).unwrap_or_default();
    let text = clean(args.get("text").and_then(|v| v.as_str()).unwrap_or_default());
    // Models write an id as a number or as a string, depending on the day.
    let id = args.get("id").and_then(|v| {
        v.as_u64().or_else(|| v.as_str().and_then(|s| s.trim().parse().ok()))
    });
    let missing = |id: u64| format!("error: no memory with id {id}. Call list_memories first.");

    Some(match name {
        "list_memories" if store.items.is_empty() => "(nothing remembered yet)".into(),
        "list_memories" => store.items.iter().map(|m| format!("{}: {}\n", m.id, m.text)).collect(),
        "remember" if text.is_empty() => {
            "error: remember needs {\"text\": \"the fact\"}".into()
        }
        "remember" => {
            if store.add(text, crate::assistant::NAME) {
                let id = store.items.last().expect("add() just pushed").id;
                store.save();
                format!("Remembered, id {id}.")
            } else {
                "Already remembered — nothing added.".into()
            }
        }
        "update_memory" | "forget" if id.is_none() => {
            format!("error: {name} needs {{\"id\": <number>}} — the id from list_memories.")
        }
        "update_memory" if text.is_empty() => {
            "error: update_memory needs the new {\"text\": \"…\"}; use forget to delete."
                .into()
        }
        // Both echo what they hit, so a wrong id is visible in the transcript
        // instead of being a silent deletion the user finds out about later.
        "update_memory" => match store.items.iter_mut().find(|m| m.id == id.unwrap()) {
            Some(item) => {
                let was = std::mem::replace(&mut item.text, text);
                store.save();
                format!("Updated memory {}: was \"{was}\".", id.unwrap())
            }
            None => missing(id.unwrap()),
        },
        "forget" => {
            let id = id.unwrap();
            match store.items.iter().find(|m| m.id == id).map(|m| m.text.clone()) {
                // Through the same path as the dashboard's delete, so a row open
                // in the editor is closed rather than left editing a ghost.
                Some(was) => {
                    let _ = update(store, Message::Delete(id));
                    format!("Forgotten: \"{was}\".")
                }
                None => missing(id),
            }
        }
        _ => unreachable!("TOOLS and this match are the same list"),
    })
}

const HARVESTER: &str = "You maintain an AI assistant's long-term memory of its user. \
From the exchange you are given, extract only durable facts that would still be \
useful months later in a completely unrelated conversation: the user's identity, \
role, preferences, ongoing projects, tools, constraints and working style. \
When the user explicitly asks to remember, save or note something about \
themselves, that is always worth keeping — never answer NONE to such a request. \
Ignore one-off task details, questions, transient state, and anything already \
listed as remembered. Never keep anything the user asked you to forget or keep \
private. Most other exchanges contain nothing worth keeping — that is the \
normal answer. Reply with one memory per line, each a short third-person \
statement (\"Prefers Rust over Go for CLI tools.\"). No numbering, no bullets, \
no commentary. Reply with exactly NONE if there is nothing durable.";

/// Pull the memories out of the harvester's reply, defensively: models add
/// bullets, numbering and a closing remark no matter how the prompt is phrased.
fn parse_harvest(reply: &str) -> Vec<String> {
    reply
        .lines()
        .map(clean)
        .filter(|line| {
            !line.is_empty()
                && !line.eq_ignore_ascii_case("none")
                // A model that decides to explain itself does it in a sentence
                // about memory, not in a fact about the user.
                && line.len() > 3
                // A bare label like "Remembered:" or "Note:" is the model
                // narrating the act of saving rather than stating a fact — a
                // real memory is a full sentence, never just a trailing colon.
                && !line.ends_with(':')
        })
        .take(MAX_PER_HARVEST)
        .collect()
}

/// Normalize one line into a storable memory: leading list markers stripped,
/// whitespace collapsed, length capped.
fn clean(line: &str) -> String {
    let line = line.trim();
    // "- ", "* ", "1. ", "2) " — every way a model writes a list.
    let line = line.trim_start_matches(['-', '*', '•', ' ']);
    let line = match line.find(['.', ')']) {
        Some(i) if i <= 2 && line[..i].chars().all(|c| c.is_ascii_digit()) && i > 0 => {
            &line[i + 1..]
        }
        _ => line,
    };
    let mut out: String = line.split_whitespace().collect::<Vec<_>>().join(" ");
    if out.chars().count() > MAX_LEN {
        out = out.chars().take(MAX_LEN).collect::<String>().trim_end().to_string();
    }
    out
}

/// Letters and digits only, lowercased — the identity used for de-duplication.
fn fingerprint(text: &str) -> String {
    text.chars().filter(|c| c.is_alphanumeric()).flat_map(char::to_lowercase).collect()
}

fn role_label(role: &str) -> &str {
    if role == "user" {
        "User"
    } else {
        "Assistant"
    }
}

#[derive(Debug, Clone)]
pub enum Message {
    /// A harvest came back: the facts it found, and which assistant they came from.
    Harvested(Vec<String>, String),
    SearchChanged(String),
    DraftChanged(String),
    Add,
    StartEdit(u64),
    EditChanged(String),
    SaveEdit,
    CancelEdit,
    Delete(u64),
    ToggleEnabled,
    /// Arms, then performs, the wipe — see the handler.
    ForgetAll,
    /// Disarm "Forget all" without wiping anything.
    CancelForget,
    DismissError,
    /// Close the "memory updated" banner, keeping the facts.
    NoticeKeep,
    /// Close the banner and delete the facts it announced.
    NoticeDiscard,
}

pub fn update(store: &mut Store, message: Message) -> Task<Message> {
    // Doing anything else — searching, editing, navigating away — disarms the
    // wipe, so a confirmation can never be left lying around to be hit later.
    if !matches!(message, Message::ForgetAll) {
        store.confirm_forget = false;
    }
    match message {
        Message::Harvested(facts, source) => {
            store.harvesting = false;
            // Memory may have been switched off while the call was in flight.
            if !store.enabled {
                return Task::none();
            }
            let mut added = Vec::new();
            for f in facts {
                if store.add(f, &source) {
                    let m = store.items.last().expect("add() just pushed");
                    added.push((m.id, m.text.clone()));
                }
            }
            if !added.is_empty() {
                store.save();
                store.notice = Some(Notice { source, facts: added });
            }
            Task::none()
        }
        Message::SearchChanged(v) => {
            store.search = v;
            Task::none()
        }
        Message::DraftChanged(v) => {
            store.draft = v;
            Task::none()
        }
        Message::Add => {
            let text = std::mem::take(&mut store.draft);
            if store.add(text, "You") {
                store.save();
            }
            Task::none()
        }
        Message::StartEdit(id) => {
            store.editing =
                store.items.iter().find(|m| m.id == id).map(|m| (id, m.text.clone()));
            Task::none()
        }
        Message::EditChanged(v) => {
            if let Some((_, text)) = store.editing.as_mut() {
                *text = v;
            }
            Task::none()
        }
        Message::SaveEdit => {
            if let Some((id, text)) = store.editing.take() {
                let text = clean(&text);
                match (text.is_empty(), store.items.iter_mut().find(|m| m.id == id)) {
                    // Emptying a memory is how you delete it from the editor.
                    (true, _) => store.items.retain(|m| m.id != id),
                    (false, Some(item)) => item.text = text,
                    (false, None) => {}
                }
                store.save();
            }
            Task::none()
        }
        Message::CancelEdit => {
            store.editing = None;
            Task::none()
        }
        Message::Delete(id) => {
            store.items.retain(|m| m.id != id);
            if store.editing.as_ref().is_some_and(|(e, _)| *e == id) {
                store.editing = None;
            }
            store.save();
            Task::none()
        }
        Message::ToggleEnabled => {
            store.enabled = !store.enabled;
            store.save();
            Task::none()
        }
        Message::ForgetAll => {
            // First click only arms the button; the second one wipes.
            if !store.confirm_forget {
                store.confirm_forget = true;
                return Task::none();
            }
            store.confirm_forget = false;
            store.items.clear();
            store.editing = None;
            store.notice = None;
            store.save();
            Task::none()
        }
        Message::CancelForget => Task::none(),
        Message::DismissError => {
            store.error = None;
            Task::none()
        }
        Message::NoticeKeep => {
            store.notice = None;
            Task::none()
        }
        Message::NoticeDiscard => {
            if let Some(n) = store.notice.take() {
                store.items.retain(|m| !n.facts.iter().any(|(id, _)| *id == m.id));
                store.save();
            }
            Task::none()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn store() -> Store {
        Store::load(&std::env::temp_dir().join(format!(
            "ev-memory-test-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        )))
    }

    #[test]
    fn harvest_replies_are_parsed_into_bare_facts() {
        assert_eq!(
            parse_harvest("- Prefers Rust.\n* Works on an agent platform.\n"),
            vec!["Prefers Rust.", "Works on an agent platform."]
        );
        assert_eq!(parse_harvest("1. Lives in London.\n2) Uses Windows."), vec![
            "Lives in London.",
            "Uses Windows."
        ]);
        assert!(parse_harvest("NONE").is_empty());
        assert!(parse_harvest("  none  \n").is_empty());
        assert!(parse_harvest("").is_empty());
        // A chatty model must not push out real facts.
        assert_eq!(parse_harvest("a\nb\nOne.\nTwo.\nThree.\nFour.").len(), MAX_PER_HARVEST);
        // A bare "Remembered:" is the model narrating the save, not a fact —
        // seen live, stored as a memory with no content.
        assert_eq!(parse_harvest("Remembered:\nPrefers Rust."), vec!["Prefers Rust."]);
    }

    #[test]
    fn the_same_fact_is_only_remembered_once() {
        let mut s = store();
        assert!(s.add("Prefers Rust.".into(), "Chat"));
        assert!(!s.add("prefers rust".into(), "Chat"), "punctuation and case are not new facts");
        assert!(s.add("Prefers Go.".into(), "Chat"));
        assert_eq!(s.items.len(), 2);
        assert!(!s.add("   ".into(), "Chat"));
    }

    #[test]
    fn recall_is_one_system_block_and_the_switch_silences_it() {
        let mut s = store();
        assert_eq!(s.system_block(), None, "nothing remembered, nothing to say");
        s.add("Prefers Rust.".into(), "Chat");
        let block = s.system_block().expect("a fact makes a block");
        // The id rides along: it is what update_memory and forget are given.
        assert!(block.contains(&format!("- [{}] Prefers Rust.", s.items[0].id)));

        s.enabled = false;
        assert_eq!(s.system_block(), None, "off means off, without losing the facts");
        assert_eq!(s.items.len(), 1);
    }

    #[test]
    fn memories_survive_a_restart_and_ids_keep_climbing() {
        let dir = std::env::temp_dir().join(format!("ev-memory-persist-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let mut s = Store::load(&dir);
        s.add("Prefers Rust.".into(), "Chat");
        s.add("Uses Windows.".into(), "E.V.");
        let first = s.items[0].id;
        let _ = update(&mut s, Message::Delete(first));
        s.save();

        let reopened = Store::load(&dir);
        assert_eq!(reopened.items.len(), 1);
        assert_eq!(reopened.items[0].text, "Uses Windows.");
        assert!(
            reopened.next_id > reopened.items[0].id,
            "a reused id would make the dashboard edit the wrong row"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn forget_all_needs_a_second_click_and_anything_else_disarms_it() {
        let mut s = store();
        s.add("Prefers Rust.".into(), "Chat");
        let _ = update(&mut s, Message::ForgetAll);
        assert!(s.confirm_forget && s.items.len() == 1, "the first click only arms");
        let _ = update(&mut s, Message::SearchChanged("r".into()));
        assert!(!s.confirm_forget, "doing anything else disarms the wipe");

        let _ = update(&mut s, Message::ForgetAll);
        let _ = update(&mut s, Message::ForgetAll);
        assert!(s.items.is_empty() && !s.confirm_forget);
    }

    #[test]
    fn editing_to_blank_deletes_the_memory() {
        let mut s = store();
        s.add("Prefers Rust.".into(), "Chat");
        let id = s.items[0].id;
        let _ = update(&mut s, Message::StartEdit(id));
        let _ = update(&mut s, Message::EditChanged("Prefers Go.".into()));
        let _ = update(&mut s, Message::SaveEdit);
        assert_eq!(s.items[0].text, "Prefers Go.");

        let _ = update(&mut s, Message::StartEdit(id));
        let _ = update(&mut s, Message::EditChanged("   ".into()));
        let _ = update(&mut s, Message::SaveEdit);
        assert!(s.items.is_empty());
    }

    /// The whole point of the toolkit: what the user asks for out loud —
    /// remember, correct, forget — lands in the store on that turn.
    #[test]
    fn the_toolkit_reads_writes_corrects_and_deletes() {
        let mut s = store();
        let run = |s: &mut Store, name: &str, args: &str| {
            run_tool(s, name, args).expect("a memory tool")
        };

        assert!(run(&mut s, "list_memories", "{}").contains("nothing remembered"));
        assert!(run(&mut s, "remember", r#"{"text":"Lives in London."}"#).contains("id 1"));
        assert_eq!(s.items[0].source, crate::assistant::NAME);
        // A second identical fact is not a second memory.
        assert!(run(&mut s, "remember", r#"{"text":"lives in london"}"#).contains("Already"));
        assert_eq!(s.items.len(), 1);

        run(&mut s, "remember", r#"{"text":"Prefers Rust."}"#);
        let listed = run(&mut s, "list_memories", "{}");
        assert!(listed.contains("1: Lives in London.") && listed.contains("2: Prefers Rust."));

        // Ids come back as strings about as often as numbers.
        assert!(run(&mut s, "update_memory", r#"{"id":"1","text":"Lives in Leeds."}"#)
            .contains("Updated"));
        assert_eq!(s.items[0].text, "Lives in Leeds.");
        assert!(run(&mut s, "forget", r#"{"id":2}"#).contains("Forgotten"));
        assert_eq!(s.items.len(), 1);

        // Everything written is on disk, not just in this Store.
        assert_eq!(Store::load(&s.dir).items.len(), 1);

        // Bad calls answer the model instead of doing something arbitrary.
        assert!(run(&mut s, "forget", r#"{"id":99}"#).contains("no memory with id 99"));
        assert!(run(&mut s, "update_memory", "{}").starts_with("error:"));
        assert!(run(&mut s, "remember", r#"{"text":"  "}"#).starts_with("error:"));
        assert!(run(&mut s, "remember", "not json").starts_with("error:"));
        assert_eq!(s.items.len(), 1, "nothing was written by a broken call");

        // Not a memory tool → not this module's business.
        assert!(run_tool(&mut s, "run_command", "{}").is_none());

        // Memory off means off for the model too, in both directions.
        s.enabled = false;
        assert!(run(&mut s, "remember", r#"{"text":"Uses Windows."}"#).contains("switched off"));
        assert!(run(&mut s, "list_memories", "{}").contains("switched off"));
        assert_eq!(s.items.len(), 1);
    }

    /// The model is only told about tools it can actually reach.
    #[test]
    fn every_advertised_tool_is_a_handled_one() {
        let spec = tools_spec();
        let names: Vec<String> = spec
            .iter()
            .map(|t| t["function"]["name"].as_str().unwrap().to_string())
            .collect();
        assert_eq!(names, TOOLS);
        let mut s = store();
        for name in &names {
            assert!(run_tool(&mut s, name, "{}").is_some(), "{name} is advertised but unhandled");
        }
    }

    #[test]
    fn harvest_is_skipped_when_it_could_only_waste_a_call() {
        let client = Client::new("http://127.0.0.1:1", "k");
        let exchange = vec![
            ChatMessage::text("user", "hi"),
            ChatMessage::text("assistant", "hello"),
        ];

        let mut s = store();
        s.enabled = false;
        let _ = s.harvest(&client, &exchange, "Chat");
        assert!(!s.harvesting, "memory is off");

        let mut s = store();
        let _ = s.harvest(&client, &exchange[..1], "Chat");
        assert!(!s.harvesting, "no assistant turn to learn from yet");

        let mut s = store();
        let _ = s.harvest(&client, &exchange, "Chat");
        assert!(s.harvesting);
        let _ = s.harvest(&client, &exchange, "Chat");
        assert!(s.harvesting, "the second call is a no-op, not a second request");

        let _ = update(&mut s, Message::Harvested(vec!["Prefers Rust.".into()], "Chat".into()));
        assert!(!s.harvesting);
        assert_eq!(s.items.len(), 1);
        assert_eq!(s.items[0].source, "Chat");
    }

    #[test]
    fn a_harvest_raises_a_notice_and_discard_undoes_exactly_it() {
        let mut s = store();
        s.add("Prefers Rust.".into(), "Chat");
        let _ = update(
            &mut s,
            Message::Harvested(vec!["Name is Tan.".into(), "Uses Windows.".into()], "E.V.".into()),
        );
        let notice = s.notice.clone().expect("new facts raise a banner");
        assert_eq!(notice.source, "E.V.");
        assert_eq!(notice.facts.len(), 2);

        let _ = update(&mut s, Message::NoticeDiscard);
        assert!(s.notice.is_none());
        assert_eq!(s.items.len(), 1, "only the harvested facts are gone");
        assert_eq!(s.items[0].text, "Prefers Rust.");

        // A harvest that adds nothing (all duplicates) raises no banner.
        let _ = update(&mut s, Message::Harvested(vec!["Prefers Rust.".into()], "Chat".into()));
        assert!(s.notice.is_none());

        let _ = update(&mut s, Message::Harvested(vec!["Likes Go.".into()], "Chat".into()));
        assert!(s.notice.is_some());
        let _ = update(&mut s, Message::NoticeKeep);
        assert!(s.notice.is_none());
        assert_eq!(s.items.len(), 2, "keep closes the banner without deleting");
    }
}
