"""Workspace-scoped external API token issuance, auth, usage tracking, and revocation."""

# Catalog of scopes enforced by `require_scope(...)` across the API. Keep in sync
# with the string literals passed to require_scope / verify_project_api_token.
# `*` (all scopes) is granted implicitly and is not listed here.
AVAILABLE_SCOPES: list[dict[str, str]] = [
    {"scope": "process:read", "description": "Read processes and their runs"},
    {"scope": "process:write", "description": "Create, start, and mutate processes"},
    {"scope": "todos:read", "description": "Read todo boards and items"},
    {"scope": "todos:write", "description": "Create and update todo boards and items"},
    {"scope": "chat:write", "description": "Send messages to chat and the coder agent"},
    {"scope": "model:read", "description": "Read model-ops projects, jobs, registry, and Ollama models"},
    {"scope": "model:write", "description": "Create/train models, run build jobs, and manage Ollama lifecycle"},
]
