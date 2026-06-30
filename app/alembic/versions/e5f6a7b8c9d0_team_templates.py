"""team templates and run team snapshot

Revision ID: e5f6a7b8c9d0
Revises: c3d4e5f6a7b8
Create Date: 2026-04-10

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


revision: str = "e5f6a7b8c9d0"
down_revision: Union[str, Sequence[str], None] = "c3d4e5f6a7b8"
branch_labels: Union[str, Sequence[str], None] = None
depends_on: Union[str, Sequence[str], None] = None


def upgrade() -> None:
    op.create_table(
        "teamtemplate",
        sa.Column("id", sa.Integer(), nullable=False),
        sa.Column("name", sa.String(length=256), nullable=False),
        sa.Column("description", sa.String(length=4096), nullable=True),
        sa.Column("color", sa.String(length=32), nullable=True),
        sa.Column("roster_json", sa.String(), nullable=False),
        sa.Column("created_at", sa.DateTime(), nullable=False),
        sa.Column("updated_at", sa.DateTime(), nullable=False),
        sa.PrimaryKeyConstraint("id"),
    )
    with op.batch_alter_table("run") as batch:
        batch.add_column(sa.Column("team_template_id", sa.Integer(), nullable=True))
        batch.add_column(sa.Column("team_snapshot_json", sa.String(), nullable=True))
        batch.create_foreign_key(
            "fk_run_team_template_id",
            "teamtemplate",
            ["team_template_id"],
            ["id"],
            ondelete="SET NULL",
        )

    conn = op.get_bind()
    for tmpl in SEED_TEAM_TEMPLATES:
        conn.execute(
            sa.text(
                """
                INSERT INTO teamtemplate (name, description, color, roster_json, created_at, updated_at)
                VALUES (:name, :description, :color, :roster_json, now(), now())
                """
            ),
            {
                "name": tmpl["name"],
                "description": tmpl["description"],
                "color": tmpl["color"],
                "roster_json": json.dumps(tmpl["roster"]),
            },
        )


def downgrade() -> None:
    with op.batch_alter_table("run") as batch:
        batch.drop_constraint("fk_run_team_template_id", type_="foreignkey")
        batch.drop_column("team_snapshot_json")
        batch.drop_column("team_template_id")
    op.drop_table("teamtemplate")
