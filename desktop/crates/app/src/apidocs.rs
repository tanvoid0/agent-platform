//! API reference: the server's own OpenAPI document, rendered as copyable
//! recipes for whoever is writing a client against it.
//!
//! The endpoint list is fetched from `/openapi.json`, never typed out here.
//! That document is itself hand-maintained now — see `lib.rs::openapi` in the
//! server crate for why, and for the drift it can develop — but it is still one
//! place rather than two, and it is the server's own answer.
//!
//! [`RUST_ROUTES`] is gone with the proxy. It existed to badge each row with
//! "Rust" or "Python" while the two servers split the surface between them;
//! there is one server now, so every row would carry the same badge and the
//! column said nothing.

use agent_platform_client::Client;
use iced::Task;
use serde_json::{Map, Value};


/// The routes that answer without a bearer token. Everything else needs one —
/// `/api/v1/*` through the auth layer, `/v1/*` through each handler's own check.
const OPEN_ROUTES: &[&str] = &["/health", "/ready", "/v1/health", "/v1/health/readiness"];

/// Header parameters every route inherits from its auth dependency. They are
/// documented once, in the Connect card, not 192 times.
const BORING_HEADERS: [&str; 2] = ["authorization", "x-agent-platform-client"];

const METHODS: [&str; 5] = ["GET", "POST", "PUT", "PATCH", "DELETE"];

/// Which language the quickstart shows. Three, because they are three different
/// audiences: a shell check, a REST client, and an OpenAI SDK pointed here.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Snippet {
    #[default]
    Curl,
    Python,
    OpenAi,
}

impl Snippet {
    pub const ALL: [Snippet; 3] = [Snippet::Curl, Snippet::Python, Snippet::OpenAi];

    pub fn label(self) -> &'static str {
        match self {
            Snippet::Curl => "curl",
            Snippet::Python => "Python",
            Snippet::OpenAi => "OpenAI SDK",
        }
    }
}

#[derive(Debug, Clone)]
pub struct Param {
    pub name: String,
    /// `path` or `query`; header params are dropped at parse time.
    pub location: String,
    pub required: bool,
    pub kind: String,
}

#[derive(Debug, Clone)]
pub struct Endpoint {
    pub method: String,
    pub path: String,
    pub tag: String,
    pub summary: String,
    pub description: String,
    pub params: Vec<Param>,
    /// Request body skeleton, pretty-printed, built from the declared schema.
    pub body: Option<String>,
    /// The same value on one line, for a curl that survives being pasted into a
    /// PowerShell prompt (a multi-line `-d` does not).
    body_compact: Option<String>,
    pub auth: bool,
}

impl Endpoint {
    /// Stable key for the copy buttons and the expanded row.
    pub fn id(&self) -> String {
        format!("{} {}", self.method, self.path)
    }

    pub fn matches(&self, needle: &str) -> bool {
        if needle.is_empty() {
            return true;
        }
        let n = needle.to_lowercase();
        self.path.to_lowercase().contains(&n)
            || self.summary.to_lowercase().contains(&n)
            || self.tag.to_lowercase().contains(&n)
            || self.method.to_lowercase().contains(&n)
    }

    /// A runnable one-liner. Single line on purpose: a `\`-continued command is
    /// a bash-ism, and this app's own host pastes it into PowerShell.
    pub fn curl(&self, origin: &str, key: &str) -> String {
        let mut out = String::from("curl ");
        if self.method != "GET" {
            out.push_str(&format!("-X {} ", self.method));
        }
        let query: Vec<String> = self
            .params
            .iter()
            .filter(|p| p.location == "query" && p.required)
            .map(|p| format!("{}=<{}>", p.name, p.name))
            .collect();
        out.push_str(&format!("\"{origin}{}", self.path));
        if !query.is_empty() {
            out.push('?');
            out.push_str(&query.join("&"));
        }
        out.push('"');
        if self.auth {
            let shown = if key.is_empty() { "<key>" } else { key };
            out.push_str(&format!(" -H \"Authorization: Bearer {shown}\""));
        }
        if let Some(body) = &self.body_compact {
            out.push_str(&format!(" -H \"Content-Type: application/json\" -d '{body}'"));
        }
        out
    }
}

#[derive(Default)]
pub struct State {
    pub endpoints: Vec<Endpoint>,
    /// False until the first fetch settles, so a load in flight does not render
    /// as "this server has no API".
    pub loaded: bool,
    pub error: Option<String>,
    pub filter: String,
    /// Which row is expanded, by [`Endpoint::id`] — an index would point at a
    /// different endpoint the moment the filter changes.
    pub open: Option<String>,
    pub snippet: Snippet,
    /// Which copy button was last pressed, so it can say it worked.
    pub copied: Option<String>,
}

impl State {
    pub fn visible(&self) -> Vec<&Endpoint> {
        self.endpoints.iter().filter(|e| e.matches(&self.filter)).collect()
    }
}

#[derive(Debug, Clone)]
pub enum Message {
    /// "View logs" on a traced error banner — intercepted in `main::update`
    /// before it reaches here, so this arm exists only to satisfy exhaustiveness.
    TraceLogs(String),
    Refresh,
    Loaded(Result<Vec<Endpoint>, String>),
    FilterChanged(String),
    Toggle(String),
    SnippetChanged(Snippet),
    /// `(button id, text)` — the id is what turns that one button into
    /// "Copied!", so two buttons on the same row do not both claim it.
    Copy(String, String),
}

pub fn refresh(client: &Client) -> Task<Message> {
    let client = client.clone();
    Task::perform(
        // Parsed off the UI thread: the document is ~230 KB of JSON and every
        // frame would otherwise wait on it.
        async move {
            client.openapi().await.map(|spec| parse(&spec)).map_err(|e| e.to_string())
        },
        Message::Loaded,
    )
}

pub fn update(state: &mut State, client: &Client, message: Message) -> Task<Message> {
    match message {
        Message::TraceLogs(_) => Task::none(),
        Message::Refresh => {
            // Only once per session: the surface changes when the server is
            // rebuilt, not while it runs.
            if state.loaded && state.error.is_none() {
                return Task::none();
            }
            refresh(client)
        }
        Message::Loaded(Ok(endpoints)) => {
            state.endpoints = endpoints;
            state.error = None;
            state.loaded = true;
            Task::none()
        }
        Message::Loaded(Err(e)) => {
            state.error = Some(e);
            state.loaded = true;
            Task::none()
        }
        Message::FilterChanged(value) => {
            state.filter = value;
            Task::none()
        }
        Message::Toggle(id) => {
            state.open = if state.open.as_deref() == Some(id.as_str()) { None } else { Some(id) };
            Task::none()
        }
        Message::SnippetChanged(snippet) => {
            state.snippet = snippet;
            Task::none()
        }
        Message::Copy(id, text) => {
            state.copied = Some(id);
            iced::clipboard::write(text)
        }
    }
}

// ---------------------------------------------------------------------------
// OpenAPI → endpoints
// ---------------------------------------------------------------------------

/// Flattens the document into one sorted list. Sorted by tag then path so the
/// view can group by walking it once.
pub fn parse(spec: &Value) -> Vec<Endpoint> {
    let empty = Value::Object(Map::new());
    let components = spec.pointer("/components/schemas").unwrap_or(&empty);
    let Some(paths) = spec.get("paths").and_then(Value::as_object) else {
        return Vec::new();
    };

    let mut out = Vec::new();
    for (path, operations) in paths {
        let Some(operations) = operations.as_object() else { continue };
        for (method, op) in operations {
            let method = method.to_uppercase();
            if !METHODS.contains(&method.as_str()) {
                continue;
            }
            let body = op
                .pointer("/requestBody/content/application~1json/schema")
                .map(|schema| example(schema, components, 4))
                .filter(|v| !v.is_null());

            out.push(Endpoint {
                auth: !OPEN_ROUTES.contains(&path.as_str()),
                method,
                tag: op
                    .get("tags")
                    .and_then(Value::as_array)
                    .and_then(|t| t.first())
                    .and_then(Value::as_str)
                    .unwrap_or("other")
                    .to_string(),
                summary: op.get("summary").and_then(Value::as_str).unwrap_or("").to_string(),
                description: op
                    .get("description")
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .to_string(),
                params: params(op, components),
                body: body.as_ref().map(|v| serde_json::to_string_pretty(v).unwrap_or_default()),
                body_compact: body.as_ref().map(ToString::to_string),
                path: path.clone(),
            });
        }
    }
    out.sort_by(|a, b| {
        (&a.tag, &a.path, &a.method).cmp(&(&b.tag, &b.path, &b.method))
    });
    out
}


fn params(op: &Value, components: &Value) -> Vec<Param> {
    let Some(list) = op.get("parameters").and_then(Value::as_array) else {
        return Vec::new();
    };
    list.iter()
        .filter_map(|p| {
            let name = p.get("name").and_then(Value::as_str)?;
            let location = p.get("in").and_then(Value::as_str).unwrap_or("query");
            if location == "header" && BORING_HEADERS.contains(&name.to_lowercase().as_str()) {
                return None;
            }
            let schema = p.get("schema").unwrap_or(&Value::Null);
            Some(Param {
                name: name.to_string(),
                location: location.to_string(),
                required: p.get("required").and_then(Value::as_bool).unwrap_or(false),
                kind: type_name(schema, components),
            })
        })
        .collect()
}

/// `integer`, `string`, `integer | null` — what the caller has to send, not the
/// whole JSON-Schema node.
fn type_name(schema: &Value, components: &Value) -> String {
    let schema = resolve(schema, components);
    if let Some(branches) = schema.get("anyOf").or_else(|| schema.get("oneOf")).and_then(Value::as_array)
    {
        let names: Vec<String> = branches.iter().map(|b| type_name(b, components)).collect();
        return names.join(" | ");
    }
    match schema.get("type").and_then(Value::as_str) {
        Some(t) => t.to_string(),
        None => "object".to_string(),
    }
}

/// Follows `$ref` into `components.schemas`. Bounded rather than recursive: a
/// schema that refers to itself would otherwise spin here.
fn resolve<'a>(schema: &'a Value, components: &'a Value) -> &'a Value {
    let mut current = schema;
    for _ in 0..8 {
        let Some(reference) = current.get("$ref").and_then(Value::as_str) else {
            return current;
        };
        let name = reference.rsplit('/').next().unwrap_or_default();
        match components.get(name) {
            Some(next) => current = next,
            None => return current,
        }
    }
    current
}

/// A fillable example for a schema: the declared default or example when there
/// is one, else an empty value of the right type. `depth` bounds recursion —
/// these schemas nest, and one of them refers to itself.
fn example(schema: &Value, components: &Value, depth: u8) -> Value {
    let schema = resolve(schema, components);
    for key in ["default", "example"] {
        if let Some(v) = schema.get(key) {
            return v.clone();
        }
    }
    for key in ["examples", "enum"] {
        if let Some(first) = schema.get(key).and_then(Value::as_array).and_then(|v| v.first()) {
            return first.clone();
        }
    }
    if depth == 0 {
        return Value::Null;
    }
    if let Some(branches) = schema.get("anyOf").or_else(|| schema.get("oneOf")).and_then(Value::as_array)
    {
        // Optional fields are declared `anyOf: [T, null]`; the caller wants T.
        let pick = branches
            .iter()
            .find(|b| resolve(b, components).get("type").and_then(Value::as_str) != Some("null"));
        return match pick {
            Some(b) => example(b, components, depth - 1),
            None => Value::Null,
        };
    }
    match schema.get("type").and_then(Value::as_str) {
        Some("string") => Value::from(""),
        Some("integer") => Value::from(0),
        Some("number") => Value::from(0.0),
        Some("boolean") => Value::from(false),
        Some("null") => Value::Null,
        Some("array") => match schema.get("items") {
            Some(items) => Value::Array(vec![example(items, components, depth - 1)]),
            None => Value::Array(Vec::new()),
        },
        // No `type` and no `properties` is FastAPI's untyped `dict` body.
        _ => {
            let mut map = Map::new();
            if let Some(props) = schema.get("properties").and_then(Value::as_object) {
                for (name, prop) in props {
                    if resolve(prop, components).get("readOnly") == Some(&Value::Bool(true)) {
                        continue;
                    }
                    map.insert(name.clone(), example(prop, components, depth - 1));
                }
            }
            Value::Object(map)
        }
    }
}

// ---------------------------------------------------------------------------
// Quickstart
// ---------------------------------------------------------------------------

/// The three getting-started snippets, against this server's actual origin.
/// Every call in them is a real route with real field names — a sample that
/// does not run is worse than none.
pub fn snippet(kind: Snippet, origin: &str) -> String {
    match kind {
        Snippet::Curl => format!(
            "# liveness — no key needed\n\
             curl {origin}/health\n\n\
             # every other call carries the bearer token\n\
             curl -H \"Authorization: Bearer $KEY\" {origin}/api/v1/projects/\n\n\
             # start a multi-agent process; answers {{\"process_id\": N, \"status\": ...}}\n\
             curl -X POST \"{origin}/api/v1/processes\" -H \"Authorization: Bearer $KEY\" \
             -H \"Content-Type: application/json\" \
             -d '{{\"goal\": \"Draft a launch plan\", \"team_template_id\": 1}}'\n\n\
             # follow it (server-sent events; closes on a terminal status or a human gate)\n\
             curl -N -H \"Authorization: Bearer $KEY\" {origin}/api/v1/processes/1/stream"
        ),
        Snippet::Python => format!(
            "import requests\n\n\
             BASE = \"{origin}\"\n\
             KEY = \"<your key>\"\n\n\
             api = requests.Session()\n\
             api.headers[\"Authorization\"] = f\"Bearer {{KEY}}\"\n\n\
             # start a process against a team template\n\
             started = api.post(\n    \
                 f\"{{BASE}}/api/v1/processes\",\n    \
                 json={{\"goal\": \"Draft a launch plan\", \"team_template_id\": 1}},\n\
             )\n\
             started.raise_for_status()\n\
             process_id = started.json()[\"process_id\"]\n\n\
             # poll it: {{\"process\": {{...}}, \"tasks\": [...]}}\n\
             detail = api.get(f\"{{BASE}}/api/v1/processes/{{process_id}}\").json()\n\
             print(detail[\"process\"][\"status\"], len(detail[\"tasks\"]), \"tasks\")\n\n\
             # errors come back as {{\"error\": {{\"message\", \"code\", \"request_id\", ...}}}}\n\
             bad = api.get(f\"{{BASE}}/api/v1/processes/999999\")\n\
             print(bad.status_code, bad.json()[\"error\"][\"code\"])"
        ),
        Snippet::OpenAi => format!(
            "# The /v1 surface is OpenAI-compatible, so any OpenAI client works:\n\
             #   pip install openai\n\
             from openai import OpenAI\n\n\
             llm = OpenAI(base_url=\"{origin}/v1\", api_key=\"<your key>\")\n\n\
             # ids come from GET /v1/models; whichever providers are configured\n\
             answer = llm.chat.completions.create(\n    \
                 model=\"llama3\",\n    \
                 messages=[{{\"role\": \"user\", \"content\": \"ping\"}}],\n\
             )\n\
             print(answer.choices[0].message.content)\n\n\
             # streaming is the same call with stream=True\n\
             for chunk in llm.chat.completions.create(\n    \
                 model=\"llama3\",\n    \
                 messages=[{{\"role\": \"user\", \"content\": \"ping\"}}],\n    \
                 stream=True,\n\
             ):\n    \
                 print(chunk.choices[0].delta.content or \"\", end=\"\")"
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn spec() -> Value {
        json!({
            "paths": {
                "/api/v1/teams/": {
                    "get": {"tags": ["teams"], "summary": "List Teams", "parameters": [
                        {"name": "authorization", "in": "header", "required": false,
                         "schema": {"type": "string"}}
                    ]},
                    "post": {"tags": ["teams"], "summary": "Create Team", "requestBody": {
                        "content": {"application/json": {
                            "schema": {"$ref": "#/components/schemas/TeamBody"}}}}}
                },
                "/api/v1/coder/chat": {
                    "post": {"tags": ["coder"], "summary": "Chat"}
                },
                "/health": {"get": {"summary": "Health"}},
                "/api/v1/todos/boards/{board_id}/items": {
                    "get": {"tags": ["todos"], "summary": "List Items", "parameters": [
                        {"name": "board_id", "in": "path", "required": true,
                         "schema": {"type": "integer"}},
                        {"name": "q", "in": "query", "required": true,
                         "schema": {"anyOf": [{"type": "string"}, {"type": "null"}]}}
                    ]}
                }
            },
            "components": {"schemas": {
                "TeamBody": {"type": "object", "properties": {
                    "name": {"type": "string"},
                    "size": {"type": "integer", "default": 3},
                    "parent_id": {"anyOf": [{"type": "integer"}, {"type": "null"}]},
                    "roles": {"type": "array", "items": {"$ref": "#/components/schemas/Role"}},
                    "id": {"type": "integer", "readOnly": true}
                }},
                "Role": {"type": "object", "properties": {"title": {"type": "string"}}}
            }}
        })
    }

    fn find<'a>(all: &'a [Endpoint], method: &str, path: &str) -> &'a Endpoint {
        all.iter().find(|e| e.method == method && e.path == path).expect("endpoint")
    }

    #[test]
    fn every_operation_is_listed_and_grouped() {
        let all = parse(&spec());
        assert_eq!(all.len(), 5);
        // Sorted by tag, so the view groups by walking the list once.
        let tags: Vec<&str> = all.iter().map(|e| e.tag.as_str()).collect();
        assert_eq!(tags, ["coder", "other", "teams", "teams", "todos"]);
    }

    #[test]
    fn the_auth_header_every_route_inherits_is_not_a_parameter() {
        let all = parse(&spec());
        assert!(find(&all, "GET", "/api/v1/teams/").params.is_empty());
        let items = find(&all, "GET", "/api/v1/todos/boards/{board_id}/items");
        let names: Vec<&str> = items.params.iter().map(|p| p.name.as_str()).collect();
        assert_eq!(names, ["board_id", "q"]);
        assert_eq!(items.params[1].kind, "string | null");
    }

    #[test]
    fn a_body_skeleton_keeps_defaults_and_drops_read_only_fields() {
        let all = parse(&spec());
        let body: Value =
            serde_json::from_str(find(&all, "POST", "/api/v1/teams/").body.as_ref().unwrap())
                .unwrap();
        assert_eq!(body["name"], "");
        assert_eq!(body["size"], 3, "a declared default is the better example");
        assert_eq!(body["parent_id"], 0, "anyOf [T, null] asks the caller for T");
        assert_eq!(body["roles"], json!([{"title": ""}]), "$ref inside items resolves");
        assert!(body.get("id").is_none(), "server-assigned fields are not sent");
    }

    #[test]
    fn curl_is_one_pasteable_line() {
        let all = parse(&spec());
        let post = find(&all, "POST", "/api/v1/teams/").curl("http://127.0.0.1:18410", "k-1");
        assert_eq!(post.lines().count(), 1, "a \\-continued command breaks in PowerShell");
        assert!(post.starts_with("curl -X POST \"http://127.0.0.1:18410/api/v1/teams/\""));
        assert!(post.contains("-H \"Authorization: Bearer k-1\""));
        assert!(post.contains("-d '{\"name\":\"\""), "the body rides on one line");

        // Open routes carry no header, and a required query param is spelled out.
        assert!(!find(&all, "GET", "/health").curl("http://x", "k").contains("Authorization"));
        let items = find(&all, "GET", "/api/v1/todos/boards/{board_id}/items");
        assert!(items.curl("http://x", "k").contains("/items?q=<q>"));
    }

    /// The spec above is a model of the real document; this reads the real one —
    /// 150 paths and 142 schemas, some of them recursive. Needs the platform up:
    ///
    /// ```bash
    /// cargo test -p agent-platform-desktop -- --ignored live_document
    /// ```
    #[tokio::test]
    #[ignore]
    async fn the_live_document_parses() {
        let key = std::env::var("AGENT_PLATFORM_TEST_KEY").unwrap_or_default();
        let spec = Client::new("http://127.0.0.1:18410", key)
            .openapi()
            .await
            .expect("GET /openapi.json — is the server running?");

        let all = parse(&spec);
        assert!(all.len() > 100, "only {} operations", all.len());

        // One endpoint end to end: the schema behind a `$ref` became a body a
        // caller could actually post.
        let start = find(&all, "POST", "/api/v1/processes");
        let body: Value = serde_json::from_str(start.body.as_ref().expect("a body")).unwrap();
        assert_eq!(body["goal"], "");
        assert_eq!(body["team_template_id"], 0);
        assert_eq!(body["auto_approve"], false, "declared default");
    }

    #[test]
    fn the_filter_matches_what_a_reader_would_type() {
        let all = parse(&spec());
        let teams = find(&all, "GET", "/api/v1/teams/");
        assert!(teams.matches(""));
        assert!(teams.matches("TEAM"), "case-insensitive");
        assert!(teams.matches("list"), "summaries are searchable");
        assert!(!teams.matches("workflow"));
    }
}
