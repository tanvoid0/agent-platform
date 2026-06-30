"""Project-scoped domain profiles for Personal Assistant agents."""

from __future__ import annotations

from typing import Any

from sqlmodel import Session, select

from assistant.domain_forms import (
    DOMAIN_PROFILE_FIELDS,
    domain_for_profile_slug,
    get_domain_form_spec,
    missing_profile_fields,
)
from assistant.models import AssistantDomainProfile
from time_utils import utc_now_naive


def get_profile(session: Session, project_id: int, domain: str) -> dict[str, Any]:
    row = session.exec(
        select(AssistantDomainProfile).where(
            AssistantDomainProfile.project_id == project_id,
            AssistantDomainProfile.domain == domain,
        )
    ).first()
    return row.get_profile() if row else {}


def get_all_profiles(session: Session, project_id: int) -> dict[str, dict[str, Any]]:
    rows = session.exec(
        select(AssistantDomainProfile).where(
            AssistantDomainProfile.project_id == project_id,
        )
    ).all()
    return {r.domain: r.get_profile() for r in rows}


def merge_profile(
    session: Session,
    project_id: int,
    domain: str,
    patch: dict[str, Any],
) -> dict[str, Any]:
    now = utc_now_naive()
    row = session.exec(
        select(AssistantDomainProfile).where(
            AssistantDomainProfile.project_id == project_id,
            AssistantDomainProfile.domain == domain,
        )
    ).first()
    if not row:
        row = AssistantDomainProfile(
            project_id=project_id,
            domain=domain,
            created_at=now,
            updated_at=now,
        )
        row.set_profile({})
    current = row.get_profile()
    for k, v in patch.items():
        if v is not None and v != "":
            current[k] = v
    row.set_profile(current)
    row.updated_at = now
    session.add(row)
    session.commit()
    session.refresh(row)
    return row.get_profile()


def build_profile_context(session: Session, project_id: int, profile_slug: str) -> dict[str, Any]:
    """Context blob injected into agent step/chat."""
    domain = domain_for_profile_slug(profile_slug)
    all_profiles = get_all_profiles(session, project_id)
    gaps: dict[str, list[str]] = {}
    for dom, _fields in DOMAIN_PROFILE_FIELDS.items():
        missing = missing_profile_fields(dom, all_profiles.get(dom, {}))
        if missing:
            gaps[dom] = missing

    active_domain = domain if domain != "general" else None
    active_profile = all_profiles.get(active_domain, {}) if active_domain else {}
    active_gaps = gaps.get(active_domain, []) if active_domain else []

    form_templates: dict[str, Any] = {}
    if active_domain and active_gaps:
        spec = get_domain_form_spec(active_domain)
        if spec:
            form_templates[active_domain] = spec

    return {
        "user_domain_profiles": all_profiles,
        "profile_gaps": gaps,
        "active_domain": active_domain,
        "active_profile": active_profile,
        "active_profile_gaps": active_gaps,
        "domain_form_templates": form_templates,
    }


def format_answers_message(domain: str, answers: dict[str, Any]) -> str:
    lines = [f"Saved {domain} profile:"]
    for k, v in answers.items():
        if isinstance(v, list):
            val = ", ".join(str(x) for x in v)
        else:
            val = str(v)
        lines.append(f"- {k.replace('_', ' ')}: {val}")
    lines.append("Please continue planning using this information.")
    return "\n".join(lines)
