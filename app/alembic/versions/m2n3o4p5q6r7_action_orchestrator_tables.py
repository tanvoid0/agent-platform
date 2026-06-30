"""action orchestrator tables for client action routing

Revision ID: m2n3o4p5q6r7
Revises: l1m2n3o4p5q6
Create Date: 2026-04-28

"""
from __future__ import annotations

from typing import Sequence, Union

from alembic import op
import sqlalchemy as sa


revision: str = "m2n3o4p5q6r7"
down_revision: Union[str, Sequence[str], None] = "l1m2n3o4p5q6"
branch_labels: Union[str, Sequence[str], None] = None
depends_on: Union[str, Sequence[str], None] = None


def upgrade() -> None:
    # FKs inlined for SQLite — separate op.create_foreign_key is unsupported for this dialect.
    from sqlalchemy import inspect, text

    # Check if tables already exist (for idempotency with existing DBs)
    bind = op.get_context().bind
    inspector = inspect(bind)
    existing_tables = {t.lower() for t in inspector.get_table_names()}

    if "action_sets" in existing_tables:
        return  # Tables already exist, skip

    op.create_table(
        "action_sets",
        sa.Column("id", sa.Integer(), nullable=False, primary_key=True),
        sa.Column("client_id", sa.String(length=256), nullable=True, index=True),
        sa.Column("name", sa.String(length=255), nullable=False, index=True),
        sa.Column("description", sa.Text(), nullable=True),
        sa.Column("metadata_json", sa.Text(), nullable=True),
        sa.Column("created_at", sa.DateTime(), nullable=False, server_default=sa.text("CURRENT_TIMESTAMP")),
        sa.Column("updated_at", sa.DateTime(), nullable=False, server_default=sa.text("CURRENT_TIMESTAMP")),
    )

    op.create_table(
        "actions",
        sa.Column("id", sa.Integer(), nullable=False, primary_key=True),
        sa.Column(
            "set_id",
            sa.Integer(),
            sa.ForeignKey("action_sets.id", ondelete="CASCADE"),
            nullable=False,
            index=True,
        ),
        sa.Column("action_id", sa.String(length=128), nullable=False, index=True),
        sa.Column("name", sa.String(length=255), nullable=False),
        sa.Column("description", sa.Text(), nullable=False),
        sa.Column("parameters_json", sa.Text(), nullable=False, server_default="{}"),
        sa.Column("execution_mode", sa.String(length=16), nullable=False, server_default="client"),
        sa.Column("endpoint", sa.String(length=512), nullable=True),
        sa.Column("created_at", sa.DateTime(), nullable=False, server_default=sa.text("CURRENT_TIMESTAMP")),
        sa.Column("updated_at", sa.DateTime(), nullable=False, server_default=sa.text("CURRENT_TIMESTAMP")),
    )
    op.create_index("ix_actions_set_action", "actions", ["set_id", "action_id"], unique=True)

    op.create_table(
        "action_sessions",
        sa.Column("id", sa.Integer(), nullable=False, primary_key=True),
        sa.Column("client_id", sa.String(length=256), nullable=True, index=True),
        sa.Column(
            "action_set_id",
            sa.Integer(),
            sa.ForeignKey("action_sets.id", ondelete="CASCADE"),
            nullable=False,
            index=True,
        ),
        sa.Column("goal", sa.Text(), nullable=False),
        sa.Column("context_json", sa.Text(), nullable=True),
        sa.Column("status", sa.String(length=32), nullable=False, server_default="active"),
        sa.Column("current_step", sa.Integer(), nullable=False, server_default="0"),
        sa.Column("max_steps", sa.Integer(), nullable=False, server_default="10"),
        sa.Column("execution_mode", sa.String(length=16), nullable=False, server_default="client"),
        sa.Column("created_at", sa.DateTime(), nullable=False, server_default=sa.text("CURRENT_TIMESTAMP")),
        sa.Column("updated_at", sa.DateTime(), nullable=False, server_default=sa.text("CURRENT_TIMESTAMP")),
        sa.Column("completed_at", sa.DateTime(), nullable=True),
    )

    op.create_table(
        "session_steps",
        sa.Column("id", sa.Integer(), nullable=False, primary_key=True),
        sa.Column(
            "session_id",
            sa.Integer(),
            sa.ForeignKey("action_sessions.id", ondelete="CASCADE"),
            nullable=False,
            index=True,
        ),
        sa.Column("step_number", sa.Integer(), nullable=False, index=True),
        sa.Column("thought", sa.Text(), nullable=True),
        sa.Column("actions_json", sa.Text(), nullable=False, server_default="[]"),
        sa.Column("status", sa.String(length=32), nullable=False, server_default="pending"),
        sa.Column("created_at", sa.DateTime(), nullable=False, server_default=sa.text("CURRENT_TIMESTAMP")),
        sa.Column("executed_at", sa.DateTime(), nullable=True),
    )
    op.create_index("ix_session_steps_session_step", "session_steps", ["session_id", "step_number"], unique=True)

    op.create_table(
        "session_results",
        sa.Column("id", sa.Integer(), nullable=False, primary_key=True),
        sa.Column(
            "session_id",
            sa.Integer(),
            sa.ForeignKey("action_sessions.id", ondelete="CASCADE"),
            nullable=False,
            index=True,
        ),
        sa.Column("step_number", sa.Integer(), nullable=False, index=True),
        sa.Column("action_id", sa.String(length=128), nullable=False),
        sa.Column("result_json", sa.Text(), nullable=True),
        sa.Column("error", sa.Text(), nullable=True),
        sa.Column("created_at", sa.DateTime(), nullable=False, server_default=sa.text("CURRENT_TIMESTAMP")),
    )


def downgrade() -> None:
    op.drop_table("session_results")
    op.drop_table("session_steps")
    op.drop_table("action_sessions")
    op.drop_table("actions")
    op.drop_table("action_sets")
