"""seed CV Reviewer Agency team template

Revision ID: l1m2n3o4p5q6
Revises: k0l1m2n3o4p5
Create Date: 2026-04-14

"""
from __future__ import annotations

import json
import sys
from pathlib import Path
from typing import Sequence, Union

from alembic import op
import sqlalchemy as sa

_app_dir = Path(__file__).resolve().parents[2]
if str(_app_dir) not in sys.path:
    sys.path.insert(0, str(_app_dir))

from default_team_templates import SEED_TEAM_TEMPLATES


revision: str = "l1m2n3o4p5q6"
down_revision: Union[str, Sequence[str], None] = "k0l1m2n3o4p5"
branch_labels: Union[str, Sequence[str], None] = None
depends_on: Union[str, Sequence[str], None] = None

_CV_NAME = "CV Reviewer Agency"


def upgrade() -> None:
    tmpl = next(t for t in SEED_TEAM_TEMPLATES if t["name"] == _CV_NAME)
    conn = op.get_bind()
    row = conn.execute(
        sa.text("SELECT id FROM teamtemplate WHERE name = :n"),
        {"n": _CV_NAME},
    ).fetchone()
    if row is not None:
        return
    conn.execute(
        sa.text(
            """
            INSERT INTO teamtemplate (
                name, description, color, category, roster_json, created_at, updated_at
            )
            VALUES (
                :name, :description, :color, :category, :roster_json,
                now(), now()
            )
            """
        ),
        {
            "name": tmpl["name"],
            "description": tmpl["description"],
            "color": tmpl["color"],
            "category": tmpl.get("category"),
            "roster_json": json.dumps(tmpl["roster"]),
        },
    )


def downgrade() -> None:
    bind = op.get_bind()
    bind.execute(
        sa.text("DELETE FROM teamtemplate WHERE name = :n"),
        {"n": _CV_NAME},
    )
