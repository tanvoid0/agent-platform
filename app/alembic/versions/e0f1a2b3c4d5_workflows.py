"""workflows — user-authored automations with runs

Revision ID: e0f1a2b3c4d5
Revises: d9e0f1a2b3c4
Create Date: 2026-08-04
"""
from __future__ import annotations

from typing import Sequence, Union

from alembic import op
import sqlalchemy as sa


revision: str = "e0f1a2b3c4d5"
down_revision: Union[str, Sequence[str], None] = "d9e0f1a2b3c4"
branch_labels: Union[str, Sequence[str], None] = None
depends_on: Union[str, Sequence[str], None] = None


def upgrade() -> None:
    from sqlalchemy import inspect

    bind = op.get_context().bind
    inspector = inspect(bind)
    existing_tables = {t.lower() for t in inspector.get_table_names()}

    if "workflows" not in existing_tables:
        op.create_table(
            "workflows",
            sa.Column("id", sa.Integer(), nullable=False, primary_key=True),
            sa.Column("client_id", sa.String(), nullable=True, index=True),
            sa.Column("name", sa.String(), nullable=False, index=True),
            sa.Column("description", sa.String(), nullable=True),
            sa.Column("steps_json", sa.Text(), nullable=False, server_default="[]"),
            sa.Column("enabled", sa.Boolean(), nullable=False, server_default=sa.true()),
            sa.Column("interval_seconds", sa.Integer(), nullable=True),
            sa.Column("next_run_at", sa.DateTime(), nullable=True, index=True),
            sa.Column("created_at", sa.DateTime(), nullable=False),
            sa.Column("updated_at", sa.DateTime(), nullable=False),
        )

    if "workflow_runs" not in existing_tables:
        op.create_table(
            "workflow_runs",
            sa.Column("id", sa.Integer(), nullable=False, primary_key=True),
            sa.Column(
                "workflow_id",
                sa.Integer(),
                sa.ForeignKey("workflows.id"),
                nullable=False,
                index=True,
            ),
            sa.Column("trigger", sa.String(), nullable=False, server_default="manual"),
            sa.Column("status", sa.String(), nullable=False, server_default="running", index=True),
            sa.Column("input_json", sa.Text(), nullable=True),
            sa.Column("steps_json", sa.Text(), nullable=False, server_default="[]"),
            sa.Column("error", sa.Text(), nullable=True),
            sa.Column("started_at", sa.DateTime(), nullable=False),
            sa.Column("finished_at", sa.DateTime(), nullable=True),
        )


def downgrade() -> None:
    op.drop_table("workflow_runs")
    op.drop_table("workflows")
