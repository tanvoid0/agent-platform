"""todo planning authority — project last board, item metadata, item events

Revision ID: r7s8t9u0v1w2
Revises: q6r7s8t9u0v1
Create Date: 2026-05-24

"""
from __future__ import annotations

from typing import Sequence, Union

from alembic import op
import sqlalchemy as sa


revision: str = "r7s8t9u0v1w2"
down_revision: Union[str, Sequence[str], None] = "q6r7s8t9u0v1"
branch_labels: Union[str, Sequence[str], None] = None
depends_on: Union[str, Sequence[str], None] = None


def upgrade() -> None:
    with op.batch_alter_table("project") as batch:
        batch.add_column(sa.Column("last_todo_board_id", sa.Integer(), nullable=True))
        batch.add_column(sa.Column("planning_prefs_json", sa.String(), nullable=True))

    with op.batch_alter_table("todo_items") as batch:
        batch.add_column(sa.Column("metadata_json", sa.String(), nullable=True))

    op.create_table(
        "todo_item_events",
        sa.Column("id", sa.Integer(), nullable=False),
        sa.Column("item_id", sa.Integer(), nullable=False),
        sa.Column("event_type", sa.String(length=64), nullable=False),
        sa.Column("content_json", sa.String(), nullable=False),
        sa.Column("created_at", sa.DateTime(), nullable=False),
        sa.ForeignKeyConstraint(["item_id"], ["todo_items.id"]),
        sa.PrimaryKeyConstraint("id"),
    )
    op.create_index("ix_todo_item_events_item_id", "todo_item_events", ["item_id"])


def downgrade() -> None:
    op.drop_index("ix_todo_item_events_item_id", table_name="todo_item_events")
    op.drop_table("todo_item_events")

    with op.batch_alter_table("todo_items") as batch:
        batch.drop_column("metadata_json")

    with op.batch_alter_table("project") as batch:
        batch.drop_column("planning_prefs_json")
        batch.drop_column("last_todo_board_id")
