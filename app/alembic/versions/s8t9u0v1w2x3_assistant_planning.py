"""assistant planning — todo fields, chat threads, reviews, life team template

Revision ID: s8t9u0v1w2x3
Revises: r7s8t9u0v1w2
Create Date: 2026-05-25

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

revision: str = "s8t9u0v1w2x3"
down_revision: Union[str, Sequence[str], None] = "r7s8t9u0v1w2"
branch_labels: Union[str, Sequence[str], None] = None
depends_on: Union[str, Sequence[str], None] = None

_LIFE_TEAM_NAME = "Personal Life Assistant"


def upgrade() -> None:
    with op.batch_alter_table("todo_items") as batch:
        batch.add_column(sa.Column("parent_item_id", sa.Integer(), nullable=True))
        batch.add_column(sa.Column("due_at", sa.DateTime(), nullable=True))
        batch.add_column(sa.Column("scheduled_at", sa.DateTime(), nullable=True))
        batch.add_column(sa.Column("time_horizon", sa.String(length=16), nullable=True))
        batch.add_column(sa.Column("item_kind", sa.String(length=32), nullable=True))
        batch.add_column(sa.Column("recurrence_json", sa.String(), nullable=True))
        batch.add_column(sa.Column("completion_json", sa.String(), nullable=True))

    with op.batch_alter_table("project") as batch:
        batch.add_column(sa.Column("assistant_board_id", sa.Integer(), nullable=True))

    op.create_table(
        "assistant_chat_threads",
        sa.Column("id", sa.Integer(), nullable=False),
        sa.Column("project_id", sa.Integer(), nullable=False),
        sa.Column("messages_json", sa.String(), nullable=True),
        sa.Column("pending_actions_json", sa.String(), nullable=True),
        sa.Column("last_profile_slug", sa.String(length=64), nullable=True),
        sa.Column("created_at", sa.DateTime(), nullable=False),
        sa.Column("updated_at", sa.DateTime(), nullable=False),
        sa.ForeignKeyConstraint(["project_id"], ["project.id"]),
        sa.PrimaryKeyConstraint("id"),
    )
    op.create_index(
        "ix_assistant_chat_threads_project_id",
        "assistant_chat_threads",
        ["project_id"],
        unique=True,
    )

    op.create_table(
        "assistant_reviews",
        sa.Column("id", sa.Integer(), nullable=False),
        sa.Column("project_id", sa.Integer(), nullable=False),
        sa.Column("status", sa.String(length=32), nullable=False),
        sa.Column("summary", sa.String(), nullable=True),
        sa.Column("stats_json", sa.String(), nullable=True),
        sa.Column("proposed_actions_json", sa.String(), nullable=True),
        sa.Column("created_at", sa.DateTime(), nullable=False),
        sa.Column("updated_at", sa.DateTime(), nullable=False),
        sa.ForeignKeyConstraint(["project_id"], ["project.id"]),
        sa.PrimaryKeyConstraint("id"),
    )
    op.create_index(
        "ix_assistant_reviews_project_id",
        "assistant_reviews",
        ["project_id"],
    )

    tmpl = next(t for t in SEED_TEAM_TEMPLATES if t["name"] == _LIFE_TEAM_NAME)
    conn = op.get_bind()
    row = conn.execute(
        sa.text("SELECT id FROM teamtemplate WHERE name = :n"),
        {"n": _LIFE_TEAM_NAME},
    ).fetchone()
    if row is None:
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
        {"n": _LIFE_TEAM_NAME},
    )
    op.drop_index("ix_assistant_reviews_project_id", table_name="assistant_reviews")
    op.drop_table("assistant_reviews")
    op.drop_index("ix_assistant_chat_threads_project_id", table_name="assistant_chat_threads")
    op.drop_table("assistant_chat_threads")

    with op.batch_alter_table("project") as batch:
        batch.drop_column("assistant_board_id")

    with op.batch_alter_table("todo_items") as batch:
        batch.drop_column("completion_json")
        batch.drop_column("recurrence_json")
        batch.drop_column("item_kind")
        batch.drop_column("time_horizon")
        batch.drop_column("scheduled_at")
        batch.drop_column("due_at")
        batch.drop_column("parent_item_id")
