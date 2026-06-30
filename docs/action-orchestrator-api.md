# Action Orchestrator API

The Action Orchestrator API allows clients to register their executable capabilities (actions) and use AI to decide which actions to perform based on goals and context. The server returns action recommendations that clients execute themselves.

**Base URL:** `http://127.0.0.1:18410/api/v1`

---

## Authentication

This API uses the **same authentication** as the main Agent Platform API. You need the `AGENT_PLATFORM_MASTER_KEY` from your `.env` file.

### Where to find your API key

```bash
# In your agent-platform/.env file:
AGENT_PLATFORM_MASTER_KEY=sk-agent-platform-xxxxxxxx
```

### How to authenticate

Include the key as a Bearer token in the `Authorization` header:

```
Authorization: Bearer sk-agent-platform-xxxxxxxx
```

### Example

```bash
curl -X POST http://127.0.0.1:18410/api/v1/action-sets \
  -H "Authorization: Bearer $AGENT_PLATFORM_MASTER_KEY" \
  -H "Content-Type: application/json" \
  -d '{...}'
```

---

## Quick Start

```bash
# 1. Register your actions
curl -X POST http://127.0.0.1:18410/api/v1/action-sets \
  -H "Authorization: Bearer $AGENT_PLATFORM_MASTER_KEY" \
  -H "Content-Type: application/json" \
  -d '{
    "name": "my-service",
    "description": "My app capabilities",
    "actions": [
      {
        "action_id": "send_email",
        "name": "Send Email",
        "description": "Send an email to a recipient",
        "parameters": {
          "type": "object",
          "properties": {
            "to": {"type": "string", "description": "Recipient email"},
            "subject": {"type": "string"},
            "body": {"type": "string"}
          },
          "required": ["to", "subject"]
        },
        "execution_mode": "client"
      }
    ]
  }'
# Response: {"id": 1, "name": "my-service", ...}

# 2. Get AI decision (one-shot)
curl -X POST http://127.0.0.1:18410/api/v1/decide \
  -H "Authorization: Bearer $AGENT_PLATFORM_MASTER_KEY" \
  -H "Content-Type: application/json" \
  -d '{
    "action_set_id": 1,
    "goal": "Notify team about deployment failure",
    "context": {"deployment_id": "dep-123", "error": "timeout"}
  }'
# Response: {"thought": "...", "actions": [{"action_id": "send_email", ...}]}

# 3. Execute the action locally in your app
# (Send the email using your own infrastructure)

# 4. (Optional) Create a session for multi-step flows
```

---

## Concepts

### Action
An action represents a capability your application can perform. Each action has:
- **action_id** — Unique identifier within your action set
- **name** — Human-readable name
- **description** — What the action does (used by AI)
- **parameters** — JSON Schema defining expected inputs
- **execution_mode** — `"client"` (you execute) or `"server"` (API calls your endpoint)

### Action Set
A collection of related actions. Organize actions by service or domain.

### Session
A multi-step conversation with the AI. Use sessions for complex workflows where:
- Multiple actions may be needed
- Action results influence subsequent decisions
- Context accumulates across steps

---

## API Endpoints

### Action Sets

#### Create Action Set
```http
POST /action-sets
```

**Request:**
```json
{
  "name": "email-service",
  "description": "Email sending capabilities",
  "actions": [
    {
      "action_id": "send_email",
      "name": "Send Email",
      "description": "Send an email to a recipient",
      "parameters": {
        "type": "object",
        "properties": {
          "to": {"type": "string", "description": "Recipient email address"},
          "subject": {"type": "string"},
          "body": {"type": "string"}
        },
        "required": ["to", "subject"]
      },
      "execution_mode": "client"
    }
  ],
  "metadata": {"version": "1.0"}
}
```

**Response:**
```json
{
  "id": 1,
  "name": "email-service",
  "description": "Email sending capabilities",
  "metadata": {"version": "1.0"},
  "actions": [...]
}
```

#### List Action Sets
```http
GET /action-sets?limit=50
```

**Response:**
```json
{
  "action_sets": [
    {
      "id": 1,
      "name": "email-service",
      "description": "...",
      "actions": [...]
    }
  ]
}
```

#### Get Action Set
```http
GET /action-sets/{set_id}
```

#### Update Action Set
```http
PUT /action-sets/{set_id}
```

**Request:**
```json
{
  "name": "new-name",
  "description": "Updated description",
  "metadata": {"version": "1.1"}
}
```

#### Delete Action Set
```http
DELETE /action-sets/{set_id}
```

---

### Actions

#### Add Action to Set
```http
POST /action-sets/{set_id}/actions
```

**Request:**
```json
{
  "action_id": "get_user",
  "name": "Get User",
  "description": "Fetch user details from database",
  "parameters": {
    "type": "object",
    "properties": {
      "user_id": {"type": "string", "description": "User ID to look up"}
    },
    "required": ["user_id"]
  },
  "execution_mode": "client"
}
```

#### List Actions in Set
```http
GET /action-sets/{set_id}/actions
```

#### Get Action
```http
GET /action-sets/{set_id}/actions/{action_id}
```

#### Update Action
```http
PUT /action-sets/{set_id}/actions/{action_id}
```

#### Delete Action
```http
DELETE /action-sets/{set_id}/actions/{action_id}
```

---

### Sessions (Multi-Step)

#### Create Session
```http
POST /sessions
```

**Request:**
```json
{
  "action_set_id": 1,
  "goal": "Onboard new user john@example.com",
  "context": {"user_email": "john@example.com", "plan": "basic"},
  "execution_mode": "client",
  "max_steps": 5
}
```

**Response:**
```json
{
  "id": 1,
  "action_set_id": 1,
  "goal": "Onboard new user john@example.com",
  "context": {"user_email": "john@example.com", "plan": "basic"},
  "status": "active",
  "current_step": 0,
  "max_steps": 5,
  "execution_mode": "client"
}
```

#### Get Session
```http
GET /sessions/{session_id}
```

#### Request Next Step
```http
POST /sessions/{session_id}/steps
```

**Request:**
```json
{
  "context": {"previous_result": "user_created"}
}
```

**Response:**
```json
{
  "session_id": 1,
  "step_number": 1,
  "thought": "User created successfully. Now I need to send a welcome email to complete onboarding.",
  "actions": [
    {
      "action_id": "send_email",
      "name": "Send Email",
      "parameters": {
        "to": "john@example.com",
        "subject": "Welcome to our platform!",
        "body": "Hi John, welcome aboard..."
      },
      "confidence": 0.95,
      "reasoning": "Sending welcome email is the final step in user onboarding"
    }
  ],
  "status": "awaiting_execution",
  "execution_mode": "client",
  "is_final": false
}
```

#### Submit Action Result
```http
POST /sessions/{session_id}/results
```

**Request (Success):**
```json
{
  "step_number": 1,
  "action_id": "send_email",
  "result": {"sent": true, "message_id": "msg_123"}
}
```

**Request (Failure):**
```json
{
  "step_number": 1,
  "action_id": "send_email",
  "error": "SMTP connection failed"
}
```

**Response:**
```json
{
  "session_id": 1,
  "step_number": 1,
  "action_id": "send_email",
  "status": "success",
  "next_step_available": true
}
```

#### Complete Session
```http
POST /sessions/{session_id}/complete
```

**Request:**
```json
{
  "summary": "User successfully onboarded"
}
```

#### Get Session History
```http
GET /sessions/{session_id}/history
```

**Response:**
```json
{
  "session": {...},
  "steps": [...],
  "results": [...]
}
```

---

### One-Shot Decision (Simple Mode)

For simple use cases where you don't need multi-step state, use the `/decide` endpoint:

```http
POST /decide
```

**Request:**
```json
{
  "action_set_id": 1,
  "goal": "What should I do about deployment failure?",
  "context": {
    "deployment_id": "dep-456",
    "error": "timeout",
    "retry_count": 2
  },
  "execution_mode": "client"
}
```

**Response:**
```json
{
  "thought": "The deployment has failed due to timeout after 2 retries. I should notify the on-call team immediately.",
  "actions": [
    {
      "action_id": "page_oncall",
      "name": "Page On-Call",
      "parameters": {
        "service": "api",
        "severity": "high",
        "message": "Deployment dep-456 failed: timeout"
      },
      "confidence": 0.92,
      "reasoning": "Multiple timeouts indicate a service issue requiring human attention"
    }
  ],
  "execution_mode": "client"
}
```

---

## Execution Modes

### Client Mode (Default)
The API returns action recommendations. Your application:
1. Receives the action(s) with parameters
2. Executes them using your own infrastructure
3. (Optional) Submits results back to continue a session

Use this when:
- Actions require local resources (databases, internal APIs)
- You want full control over execution
- Actions have side effects you manage yourself

### Server Mode (Experimental)
The API calls your provided endpoint directly. Configure with:
```json
{
  "execution_mode": "server",
  "endpoint": "https://your-app.example.com/actions/execute"
}
```

Your endpoint receives:
```json
{
  "action_id": "send_email",
  "parameters": {...},
  "session_id": 123,
  "step_number": 1
}
```

---

## Best Practices

1. **Action Descriptions Matter** — Write clear descriptions. The AI uses these to understand capabilities.

2. **Use JSON Schema** — Well-defined parameters help the AI choose correct values.

3. **Start Simple** — Use `/decide` for one-off decisions before implementing multi-step sessions.

4. **Handle Failures** — Always submit action results, especially failures. The AI can adapt its plan.

5. **Add Context** — More context leads to better decisions. Include relevant state in `context`.

6. **Action Naming** — Use `verb_noun` pattern: `send_email`, `create_user`, `fetch_data`.

---

## Error Handling

| Status | Meaning |
|--------|---------|
| 400 | Bad request — invalid parameters |
| 403 | Access denied — wrong client_id |
| 404 | Resource not found |
| 422 | Validation error — check your JSON |

**Error Response:**
```json
{
  "detail": "Action set not found"
}
```

---

## Client SDK Example (Python)

```python
import os
import requests

# Use the same key from your .env file
AGENT_PLATFORM_MASTER_KEY = os.getenv("AGENT_PLATFORM_MASTER_KEY", "your-master-key-here")

class ActionOrchestratorClient:
    def __init__(self, base_url: str, api_key: str = None):
        self.base_url = base_url.rstrip("/")
        # Default to the environment variable if not provided
        key = api_key or AGENT_PLATFORM_MASTER_KEY
        self.headers = {
            "Authorization": f"Bearer {key}",
            "Content-Type": "application/json",
        }

    def create_action_set(self, name: str, actions: list) -> dict:
        resp = requests.post(
            f"{self.base_url}/action-sets",
            headers=self.headers,
            json={"name": name, "actions": actions},
        )
        resp.raise_for_status()
        return resp.json()

    def decide(self, action_set_id: int, goal: str, context: dict = None) -> dict:
        resp = requests.post(
            f"{self.base_url}/decide",
            headers=self.headers,
            json={
                "action_set_id": action_set_id,
                "goal": goal,
                "context": context or {},
            },
        )
        resp.raise_for_status()
        return resp.json()

    def create_session(self, action_set_id: int, goal: str, **kwargs) -> dict:
        resp = requests.post(
            f"{self.base_url}/sessions",
            headers=self.headers,
            json={
                "action_set_id": action_set_id,
                "goal": goal,
                **kwargs,
            },
        )
        resp.raise_for_status()
        return resp.json()

    def get_step(self, session_id: int, context: dict = None) -> dict:
        resp = requests.post(
            f"{self.base_url}/sessions/{session_id}/steps",
            headers=self.headers,
            json={"context": context or {}},
        )
        resp.raise_for_status()
        return resp.json()

    def submit_result(self, session_id: int, step: int, action_id: str, result: dict = None, error: str = None):
        resp = requests.post(
            f"{self.base_url}/sessions/{session_id}/results",
            headers=self.headers,
            json={
                "step_number": step,
                "action_id": action_id,
                "result": result,
                "error": error,
            },
        )
        resp.raise_for_status()
        return resp.json()


# Usage - API key is read from AGENT_PLATFORM_MASTER_KEY env var by default
client = ActionOrchestratorClient("http://127.0.0.1:18410/api/v1")

# Or pass the key explicitly (must match your .env AGENT_PLATFORM_MASTER_KEY)
# client = ActionOrchestratorClient("http://127.0.0.1:18410/api/v1", "sk-agent-platform-xxxxx")

# One-shot decision
decision = client.decide(
    action_set_id=1,
    goal="Notify team about critical alert",
    context={"alert_level": "critical", "service": "api"},
)
print(decision["thought"])
for action in decision["actions"]:
    print(f"Execute: {action['action_id']} with {action['parameters']}")

# Multi-step session
session = client.create_session(1, "Deploy new feature", max_steps=3)
while True:
    step = client.get_step(session["id"])
    if step["status"] == "completed":
        break

    for action in step["actions"]:
        # Execute locally
        result = execute_locally(action)  # Your implementation
        client.submit_result(
            session["id"],
            step["step_number"],
            action["action_id"],
            result={"success": True, "data": result},
        )
```
