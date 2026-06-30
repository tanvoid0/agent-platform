"""todo board tables

Revision ID: n3o4p5q6r7s8
Revises: m2n3o4p5q6r7
Create Date: 2026-05-24

"""
from __future__ import annotations

from typing import Sequence, Union

from alembic import op
import sqlalchemy as sa


revision: str = "n3o4p5q6r7s8"
down_revision: Union[str, Sequence[str], None] = "m2n3o4p5q6r7"
branch_labels: Union[str, Sequence[str], None] = None
depends_on: Union[str, Sequence[str], None] = None


def upgrade() -> None:
    op.create_table(
        "planner_agent_profiles",
        sa.Column("id", sa.Integer(), nullable=False, primary_key=True),
        sa.Column("slug", sa.String(length=64), nullable=False),
        sa.Column("name", sa.String(length=256), nullable=False),
        sa.Column("requirement_type", sa.String(length=64), nullable=False),
        sa.Column("system_prompt", sa.Text(), nullable=False, server_default=""),
        sa.Column("default_model", sa.String(length=128), nullable=True),
        sa.Column(
            "action_set_id",
            sa.Integer(),
            sa.ForeignKey("action_sets.id", ondelete="SET NULL"),
            nullable=True,
        ),
        sa.Column("skill_paths_json", sa.Text(), nullable=True),
        sa.Column("created_at", sa.DateTime(), nullable=False, server_default=sa.text("CURRENT_TIMESTAMP")),
        sa.Column("updated_at", sa.DateTime(), nullable=False, server_default=sa.text("CURRENT_TIMESTAMP")),
    )
    op.create_index("ix_planner_agent_profiles_slug", "planner_agent_profiles", ["slug"], unique=True)
    op.create_index("ix_planner_agent_profiles_requirement_type", "planner_agent_profiles", ["requirement_type"])

    op.create_table(
        "todo_boards",
        sa.Column("id", sa.Integer(), nullable=False, primary_key=True),
        sa.Column("name", sa.String(length=256), nullable=False),
        sa.Column("description", sa.Text(), nullable=True),
        sa.Column("default_model", sa.String(length=128), nullable=True),
        sa.Column("created_at", sa.DateTime(), nullable=False, server_default=sa.text("CURRENT_TIMESTAMP")),
        sa.Column("updated_at", sa.DateTime(), nullable=False, server_default=sa.text("CURRENT_TIMESTAMP")),
    )

    op.create_table(
        "todo_categories",
        sa.Column("id", sa.Integer(), nullable=False, primary_key=True),
        sa.Column(
            "board_id",
            sa.Integer(),
            sa.ForeignKey("todo_boards.id", ondelete="CASCADE"),
            nullable=False,
            index=True,
        ),
        sa.Column("name", sa.String(length=128), nullable=False),
        sa.Column("color", sa.String(length=32), nullable=True),
        sa.Column("sort_order", sa.Integer(), nullable=False, server_default="0"),
        sa.Column(
            "planner_profile_id",
            sa.Integer(),
            sa.ForeignKey("planner_agent_profiles.id", ondelete="SET NULL"),
            nullable=True,
        ),
        sa.Column("created_at", sa.DateTime(), nullable=False, server_default=sa.text("CURRENT_TIMESTAMP")),
        sa.Column("updated_at", sa.DateTime(), nullable=False, server_default=sa.text("CURRENT_TIMESTAMP")),
    )

    op.create_table(
        "todo_items",
        sa.Column("id", sa.Integer(), nullable=False, primary_key=True),
        sa.Column(
            "board_id",
            sa.Integer(),
            sa.ForeignKey("todo_boards.id", ondelete="CASCADE"),
            nullable=False,
            index=True,
        ),
        sa.Column(
            "category_id",
            sa.Integer(),
            sa.ForeignKey("todo_categories.id", ondelete="SET NULL"),
            nullable=True,
        ),
        sa.Column("title", sa.String(length=512), nullable=False),
        sa.Column("description", sa.Text(), nullable=False, server_default=""),
        sa.Column("status", sa.String(length=32), nullable=False, server_default="plan", index=True),
        sa.Column("priority", sa.Integer(), nullable=False, server_default="0"),
        sa.Column("tags_json", sa.Text(), nullable=True),
        sa.Column("plan_json", sa.Text(), nullable=True),
        sa.Column(
            "assigned_profile_id",
            sa.Integer(),
            sa.ForeignKey("planner_agent_profiles.id", ondelete="SET NULL"),
            nullable=True,
        ),
        sa.Column(
            "linked_process_id",
            sa.Integer(),
            sa.ForeignKey("process.id", ondelete="SET NULL"),
            nullable=True,
        ),
        sa.Column("created_at", sa.DateTime(), nullable=False, server_default=sa.text("CURRENT_TIMESTAMP")),
        sa.Column("updated_at", sa.DateTime(), nullable=False, server_default=sa.text("CURRENT_TIMESTAMP")),
    )


def downgrade() -> None:
    op.drop_table("todo_items")
    op.drop_table("todo_categories")
    op.drop_table("todo_boards")
    op.drop_table("planner_agent_profiles")
