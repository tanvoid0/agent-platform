"""
Validated planner DAG schema (Pydantic) + acyclicity checks.
Matches the JSON contract described in `llm_client.generate_planner_dag`.
"""

from __future__ import annotations

import logging
import re
from collections import deque

from pydantic import BaseModel, ConfigDict, Field, ValidationError, field_validator

logger = logging.getLogger(__name__)

# Planners sometimes put role/skill slugs in `model` (e.g. typescript-expert, react-scaffolder).
# Single-token prefix + hyphen + role suffix; real aliases like gemini-flash have two hyphens / more structure.
_HALLUCINATED_ROLE_SLUG = re.compile(
    r"^[a-z][a-z0-9]{0,48}-(?:expert|scaffolder)$"
)

_UNICODE_HYPHENS = str.maketrans(
    {
        "\u2010": "-",
        "\u2011": "-",
        "\u2012": "-",
        "\u2013": "-",
        "\u2014": "-",
        "\u2015": "-",
    }
)


def sanitize_llm_model_alias(value: str | None) -> str | None:
    """
    Return a model name safe to send to the embedded LLM proxy, or None to use the server default.

    Strips common planner mistakes: skill-style slugs that are not real proxy aliases.
    """
    if value is None:
        return None
    s = value.strip()
    if not s:
        return None
    s = s.translate(_UNICODE_HYPHENS)
    if _HALLUCINATED_ROLE_SLUG.fullmatch(s.lower()):
        logger.warning(
            "Ignoring llm model %r (looks like a role/skill slug, not a proxy model alias)",
            s,
        )
        return None
    return s


class SubagentSpec(BaseModel):
    model_config = ConfigDict(extra="ignore")

    client_uuid: str
    role: str
    system_prompt: str
    instructions: str
    dependencies: list[str] = Field(default_factory=list)
    # JSON key `model` matches OpenAI chat completions; value must be a proxy alias (GET /v1/models), not a role/skill.
    llm_model: str | None = Field(
        default=None,
        alias="model",
        description=(
            "Optional OpenAI `model` alias for this task's chat/completions call (embedded LLM proxy). "
            "Omit to use SUBAGENT_MODEL / PLANNER_MODEL / server default. "
            "Not an agent persona or skill label (use `role` and prompts for that)."
        ),
    )
    subdecompose: bool = Field(
        default=False,
        description=(
            "If true, after this node completes the executor may append child tasks from its output "
            "(within AGENT_PLATFORM_SUBDECOMP_* limits; defaults allow expansion unless disabled)."
        ),
    )
    requires_review: bool = Field(
        default=False,
        description="If true, task pauses in awaiting_review after LLM output until approve/reject/request_changes.",
    )

    @field_validator("llm_model", mode="before")
    @classmethod
    def _coerce_llm_model(cls, v: object) -> object:
        if v is None:
            return None
        if not isinstance(v, str):
            return v
        return sanitize_llm_model_alias(v)


class PlannerDag(BaseModel):
    model_config = ConfigDict(extra="ignore")

    team_name: str
    goal_restatement: str
    subagents: list[SubagentSpec] = Field(min_length=1)


class SubagentsOnly(BaseModel):
    """Partial planner output used for sub-DAG expansion (new nodes only)."""

    model_config = ConfigDict(extra="ignore")

    subagents: list[SubagentSpec] = Field(min_length=1)


def _assert_unique_uuids(planner: PlannerDag) -> None:
    seen: set[str] = set()
    for a in planner.subagents:
        if a.client_uuid in seen:
            raise ValueError(f"Duplicate client_uuid: {a.client_uuid!r}")
        seen.add(a.client_uuid)


def _assert_dependency_refs(planner: PlannerDag) -> None:
    ids = {a.client_uuid for a in planner.subagents}
    for a in planner.subagents:
        for d in a.dependencies:
            if d not in ids:
                raise ValueError(
                    f"Unknown dependency {d!r} referenced by subagent {a.client_uuid!r}"
                )


def _assert_acyclic(planner: PlannerDag) -> None:
    """Kahn topological sort; if not all nodes are processed, the graph has a cycle."""
    ids = [a.client_uuid for a in planner.subagents]
    id_set = set(ids)
    indegree: dict[str, int] = {u: 0 for u in id_set}
    adj: dict[str, list[str]] = {u: [] for u in id_set}
    for a in planner.subagents:
        for d in a.dependencies:
            adj[d].append(a.client_uuid)
            indegree[a.client_uuid] += 1

    q: deque[str] = deque(u for u in id_set if indegree[u] == 0)
    processed = 0
    while q:
        u = q.popleft()
        processed += 1
        for v in adj[u]:
            indegree[v] -= 1
            if indegree[v] == 0:
                q.append(v)

    if processed != len(id_set):
        raise ValueError("DAG contains a cycle (cyclic dependencies)")


def validate_planner_dag(raw: dict) -> PlannerDag:
    """
    Parse and validate planner output. Raises ValueError with a human-readable message
    on structural or graph errors; wraps Pydantic ValidationError the same way.
    """
    try:
        planner = PlannerDag.model_validate(raw)
    except ValidationError as e:
        raise ValueError(f"Invalid planner DAG: {e}") from e

    _assert_unique_uuids(planner)
    _assert_dependency_refs(planner)
    _assert_acyclic(planner)
    return planner


def planner_dag_to_json_dict(planner: PlannerDag) -> dict:
    """Serialize for persistence / API; keep `model` key for subagents."""
    return planner.model_dump(mode="json", by_alias=True)


def merge_planner_with_new_subagents(
    base: PlannerDag,
    new_subagents_raw: list[dict],
) -> PlannerDag:
    """
    Append planner-validated subagents to an existing DAG and re-run full validation
    (unique UUIDs, dependency refs, acyclicity).
    """
    new_specs = [SubagentSpec.model_validate(x) for x in new_subagents_raw]
    base_dump = planner_dag_to_json_dict(base)
    merged = {
        "team_name": base_dump["team_name"],
        "goal_restatement": base_dump["goal_restatement"],
        "subagents": [*base_dump["subagents"], *[s.model_dump(mode="json", by_alias=True) for s in new_specs]],
    }
    return validate_planner_dag(merged)
