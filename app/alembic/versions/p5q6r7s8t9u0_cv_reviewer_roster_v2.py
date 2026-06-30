"""upgrade CV Reviewer Agency roster to v2 (5 roles, Career category)

Revision ID: p5q6r7s8t9u0
Revises: o4p5q6r7s8t9
Create Date: 2026-05-24

"""
from __future__ import annotations

import json
import sys
from datetime import datetime, timezone
from pathlib import Path
from typing import Sequence, Union

from alembic import op
import sqlalchemy as sa

_app_dir = Path(__file__).resolve().parents[2]
if str(_app_dir) not in sys.path:
    sys.path.insert(0, str(_app_dir))

from default_team_templates import SEED_TEAM_TEMPLATES


revision: str = "p5q6r7s8t9u0"
down_revision: Union[str, Sequence[str], None] = "o4p5q6r7s8t9"
branch_labels: Union[str, Sequence[str], None] = None
depends_on: Union[str, Sequence[str], None] = None

_CV_NAME = "CV Reviewer Agency"


def upgrade() -> None:
    tmpl = next(t for t in SEED_TEAM_TEMPLATES if t["name"] == _CV_NAME)
    conn = op.get_bind()
    conn.execute(
        sa.text(
            """
            UPDATE teamtemplate
            SET description = :description,
                color = :color,
                category = :category,
                roster_json = :roster_json,
                updated_at = :updated_at
            WHERE name = :name
            """
        ),
        {
            "name": _CV_NAME,
            "description": tmpl["description"],
            "color": tmpl["color"],
            "category": tmpl.get("category"),
            "roster_json": json.dumps(tmpl["roster"]),
            "updated_at": datetime.now(timezone.utc),
        },
    )


def downgrade() -> None:
    legacy_roster = {
        "roles": [
            {
                "id": "cv-review-lead",
                "name": "Career Review Lead",
                "description": (
                    "Frames the review goals (role, seniority, geography), reconciles specialist input, "
                    "and delivers a prioritized, actionable summary for the candidate."
                ),
                "modality": "text",
                "parent_id": None,
                "accent_color": "#4F46E5",
            },
            {
                "id": "cv-structure-ats",
                "name": "Structure & ATS Specialist",
                "description": (
                    "Checks layout, section order, headings, and keyword fit for applicant tracking systems; "
                    "flags parse risks, density issues, and missing standard sections."
                ),
                "modality": "text",
                "parent_id": "cv-review-lead",
                "accent_color": "#6366F1",
            },
            {
                "id": "cv-narrative-impact",
                "name": "Narrative & Impact Editor",
                "description": (
                    "Improves bullets for outcomes and metrics, clarity and tone, and the story arc from "
                    "summary through experience; suggests concrete rewrites."
                ),
                "modality": "text",
                "parent_id": "cv-structure-ats",
                "accent_color": "#818CF8",
            },
        ]
    }
    conn = op.get_bind()
    conn.execute(
        sa.text(
            """
            UPDATE teamtemplate
            SET description = :description,
                category = NULL,
                roster_json = :roster_json,
                updated_at = :updated_at
            WHERE name = :name
            """
        ),
        {
            "name": _CV_NAME,
            "description": (
                "A sequential review pipeline for résumés/CVs: structure and ATS alignment, then narrative "
                "and impact, with a lead synthesizing clear, actionable feedback."
            ),
            "roster_json": json.dumps(legacy_roster),
            "updated_at": datetime.now(timezone.utc),
        },
    )
