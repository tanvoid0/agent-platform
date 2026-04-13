"""
Pydantic validation for team template rosters (flat roles + optional parent_id tree).

Stored as JSON in TeamTemplate.roster_json; Process.team_snapshot_json duplicates metadata + roster at plan time.
"""

from __future__ import annotations

import json
from typing import Any, Literal

from pydantic import BaseModel, ConfigDict, Field, field_validator, model_validator

# Declared for API/docs; only ``text`` is accepted until the server resolves per-modality models.
RoleModality = Literal["text", "audio", "video", "image"]


class RosterRole(BaseModel):
    model_config = ConfigDict(extra="ignore")

    id: str = Field(min_length=1, max_length=128)
    name: str = Field(min_length=1, max_length=256)
    description: str = Field(default="", max_length=4096)
    modality: RoleModality = Field(default="text")
    parent_id: str | None = Field(default=None, max_length=128)
    # Optional CSS hex (e.g. #4f46e5) for roster map / UI; planner ignores.
    accent_color: str | None = Field(default=None, max_length=32)

    @field_validator("modality")
    @classmethod
    def _modality_text_only_for_now(cls, v: str) -> str:
        if v != "text":
            raise ValueError(
                "Only modality 'text' is supported until the server resolves "
                "audio, video, and image routing."
            )
        return v

    @model_validator(mode="before")
    @classmethod
    def _drop_legacy_default_model(cls, data: Any) -> Any:
        if not isinstance(data, dict):
            return data
        cleaned = {k: v for k, v in data.items() if k != "default_model"}
        return cleaned


class TeamRoster(BaseModel):
    """Flat list of roles; parent_id references another role's id for optional hierarchy."""

    model_config = ConfigDict(extra="ignore")

    roles: list[RosterRole] = Field(min_length=1, max_length=64)

    @model_validator(mode="after")
    def _validate_parent_graph(self) -> TeamRoster:
        ids = {r.id for r in self.roles}
        if len(ids) != len(self.roles):
            raise ValueError("Duplicate role id")
        parent_by_id = {r.id: r.parent_id for r in self.roles}
        for r in self.roles:
            if r.parent_id is not None:
                if r.parent_id not in ids:
                    raise ValueError(f"Unknown parent_id {r.parent_id!r} for role {r.id!r}")
                if r.parent_id == r.id:
                    raise ValueError("Role cannot be its own parent")
        for start in ids:
            seen: set[str] = set()
            cur: str | None = start
            steps = 0
            while cur is not None and steps <= len(self.roles) + 1:
                if cur in seen:
                    raise ValueError("Cycle in role parent graph")
                seen.add(cur)
                cur = parent_by_id.get(cur)
                steps += 1
            if steps > len(self.roles) + 1:
                raise ValueError("Cycle in role parent graph")
        return self


def parse_team_roster_dict(data: dict[str, Any]) -> TeamRoster:
    return TeamRoster.model_validate(data)


def parse_team_roster_json(roster_json: str) -> TeamRoster:
    try:
        raw = json.loads(roster_json)
    except json.JSONDecodeError as e:
        raise ValueError(f"Invalid roster JSON: {e}") from e
    if not isinstance(raw, dict):
        raise ValueError("Roster JSON must be an object")
    return parse_team_roster_dict(raw)


def roster_to_json(roster: TeamRoster) -> str:
    return roster.model_dump_json()


def role_depth(role_id: str, parent_by_id: dict[str, str | None]) -> int:
    """Number of ancestor edges to a root (root depth = 0)."""
    d = 0
    cur: str | None = role_id
    seen: set[str] = set()
    while cur is not None:
        if cur in seen:
            return d
        seen.add(cur)
        parent = parent_by_id.get(cur)
        if parent is None:
            break
        d += 1
        cur = parent
    return d


def render_team_context_for_planner(
    name: str,
    description: str | None,
    color: str | None,
    roster: TeamRoster,
) -> str:
    lines: list[str] = [f"Team template: {name}"]
    if description and description.strip():
        lines.append(f"Team description: {description.strip()}")
    if color and color.strip():
        lines.append(f"Team color (UI hint): {color.strip()}")
    lines.append(
        "Preferred team roster (map subagent `role` names and responsibilities to these where sensible):"
    )
    parent_by_id = {r.id: r.parent_id for r in roster.roles}
    ordered = sorted(
        roster.roles,
        key=lambda r: (role_depth(r.id, parent_by_id), r.name.lower()),
    )
    for r in ordered:
        depth = role_depth(r.id, parent_by_id)
        indent = "  " * max(depth, 0)
        parts = [f"{indent}- {r.name} (id={r.id})"]
        if r.description.strip():
            parts.append(f": {r.description.strip()}")
        if r.modality != "text":
            parts.append(f" [modality: {r.modality}]")
        lines.append("".join(parts))
    return "\n".join(lines)


def build_process_team_snapshot(
    team_template_id: int,
    name: str,
    description: str | None,
    color: str | None,
    roster: TeamRoster,
) -> str:
    payload = {
        "team_template_id": team_template_id,
        "name": name,
        "description": description,
        "color": color,
        "roster": roster.model_dump(),
    }
    return json.dumps(payload, separators=(",", ":"))


def team_context_from_snapshot_json(snapshot_json: str | None) -> str | None:
    if not snapshot_json or not snapshot_json.strip():
        return None
    try:
        data = json.loads(snapshot_json)
    except json.JSONDecodeError:
        return None
    if not isinstance(data, dict):
        return None
    name = str(data.get("name") or "").strip() or "Team"
    desc = data.get("description")
    col = data.get("color")
    roster_raw = data.get("roster")
    if not isinstance(roster_raw, dict):
        return None
    try:
        roster = parse_team_roster_dict(roster_raw)
    except ValueError:
        return None
    return render_team_context_for_planner(
        name,
        desc if isinstance(desc, str) else None,
        col if isinstance(col, str) else None,
        roster,
    )
