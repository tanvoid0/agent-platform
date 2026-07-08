"""
Pydantic validation for team template rosters (flat roles + optional parent_id tree).

Stored as JSON in TeamTemplate.roster_json; Process.team_snapshot_json duplicates metadata + roster at plan time.
"""

from __future__ import annotations

import hashlib
import json
import secrets
from typing import Any, Literal

from pydantic import BaseModel, ConfigDict, Field, field_validator, model_validator

# Declared for API/docs; only ``text`` is accepted until the server resolves per-modality models.
RoleModality = Literal["text", "audio", "video", "image"]

DEFAULT_TEAM_COLOR = "#6366f1"

# Distinct accents when a role omits ``accent_color`` (matches web roster palette).
ROSTER_ACCENT_PALETTE: tuple[str, ...] = (
    "#2563eb",
    "#16a34a",
    "#9333ea",
    "#ca8a04",
    "#dc2626",
    "#0ea5e9",
)


class RosterRole(BaseModel):
    model_config = ConfigDict(extra="ignore")

    id: str = Field(min_length=1, max_length=128)
    name: str = Field(min_length=1, max_length=256)
    description: str = Field(default="", max_length=4096)
    modality: RoleModality = Field(default="text")
    parent_id: str | None = Field(default=None, max_length=128)
    # Optional CSS hex (e.g. #4f46e5) for roster map / UI; planner ignores.
    accent_color: str | None = Field(default=None, max_length=32)

    @field_validator("accent_color", mode="before")
    @classmethod
    def _normalize_accent_color(cls, v: Any) -> str | None:
        if v is None:
            return None
        if not isinstance(v, str):
            return v
        stripped = v.strip()
        return stripped if stripped else None

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


def primary_lead_role_id(roles: list[RosterRole]) -> str | None:
    """First root in roster order (invalid parent id => root)."""
    by_id = {r.id: r for r in roles}
    roots = [r for r in roles if r.parent_id is None or r.parent_id not in by_id]
    if roots:
        return roots[0].id
    return roles[0].id if roles else None


def random_palette_color(*, avoid: set[str] | None = None) -> str:
    """Pick a random accent from the roster palette, optionally skipping used colors."""
    blocked = {c.lower() for c in (avoid or set())}
    pool = [c for c in ROSTER_ACCENT_PALETTE if c.lower() not in blocked]
    if not pool:
        pool = list(ROSTER_ACCENT_PALETTE)
    return secrets.choice(pool)


def random_team_color() -> str:
    return random_palette_color()


def _stable_palette_color(seed: str) -> str:
    """Deterministic palette pick for legacy rows (stable across reads)."""
    digest = hashlib.sha256(seed.encode()).digest()
    idx = int.from_bytes(digest[:4], "big")
    return ROSTER_ACCENT_PALETTE[idx % len(ROSTER_ACCENT_PALETTE)]


def stable_palette_color(seed: str) -> str:
    return _stable_palette_color(seed)


def resolved_team_color(team_color: str | None, stable_key: str | None = None) -> str:
    explicit = (team_color or "").strip()
    if explicit:
        return explicit
    if stable_key:
        return stable_palette_color(f"team:{stable_key}")
    return DEFAULT_TEAM_COLOR


def assign_missing_accents(
    roster: TeamRoster,
    team_color: str | None,
) -> tuple[TeamRoster, str]:
    """Assign random team + per-role accent colors when omitted; persists on create/update."""
    resolved_team = (team_color or "").strip() or random_team_color()
    used: set[str] = {resolved_team.lower()}
    lead_id = primary_lead_role_id(roster.roles)
    roles: list[RosterRole] = []
    for role in roster.roles:
        if role.accent_color:
            roles.append(role)
            used.add(role.accent_color.lower())
            continue
        if role.id == lead_id:
            accent = resolved_team
        else:
            accent = random_palette_color(avoid=used)
        used.add(accent.lower())
        roles.append(role.model_copy(update={"accent_color": accent}))
    return TeamRoster(roles=roles), resolved_team


def resolve_role_accent_color(
    role: RosterRole,
    roles: list[RosterRole],
    team_color: str | None,
    *,
    stable_key: str | None = None,
) -> str:
    """Resolve accent for legacy/null rows. Stable when ``stable_key`` is set (read path)."""
    if role.accent_color and role.accent_color.strip():
        return role.accent_color.strip()
    team_accent = (team_color or "").strip()
    if not team_accent:
        team_accent = (
            _stable_palette_color(f"team:{stable_key}")
            if stable_key
            else DEFAULT_TEAM_COLOR
        )
    lead_id = primary_lead_role_id(roles)
    if role.id == lead_id:
        return team_accent
    if stable_key:
        return _stable_palette_color(f"{stable_key}:{role.id}")
    idx = next((i for i, r in enumerate(roles) if r.id == role.id), 0)
    return ROSTER_ACCENT_PALETTE[idx % len(ROSTER_ACCENT_PALETTE)]


def with_default_accents(
    roster: TeamRoster,
    team_color: str | None,
    *,
    stable_key: str | None = None,
) -> TeamRoster:
    """Fill missing per-agent accent colors for API responses; leaves explicit values unchanged."""
    resolved_team = (team_color or "").strip()
    if not resolved_team and stable_key:
        resolved_team = _stable_palette_color(f"team:{stable_key}")
    roles = [
        role
        if role.accent_color
        else role.model_copy(
            update={
                "accent_color": resolve_role_accent_color(
                    role,
                    roster.roles,
                    resolved_team or team_color,
                    stable_key=stable_key,
                )
            }
        )
        for role in roster.roles
    ]
    return TeamRoster(roles=roles)


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
