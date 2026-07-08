"""workspace.archived_at — soft-delete tenant with cascade archive

Revision ID: b7c8d9e0f1a2
Revises: a6b7c8d9e0f1
Create Date: 2026-07-07
"""
from __future__ import annotations

from typing import Sequence, Union

from alembic import op
import sqlalchemy as sa
from sqlalchemy import inspect


revision: str = "b7c8d9e0f1a2"
down_revision: Union[str, Sequence[str], None] = "a6b7c8d9e0f1"
branch_labels: Union[str, Sequence[str], None] = None
depends_on: Union[str, Sequence[str], None] = None


def upgrade() -> None:
    bind = op.get_bind()
    insp = inspect(bind)
    tables = {t.lower() for t in insp.get_table_names()}
    if "workspace" not in tables:
        return
    cols = {c["name"] for c in insp.get_columns("workspace")}
    if "archived_at" in cols:
        return
    with op.batch_alter_table("workspace") as batch:
        batch.add_column(sa.Column("archived_at", sa.DateTime(), nullable=True))
    op.create_index("ix_workspace_archived_at", "workspace", ["archived_at"])


def downgrade() -> None:
    bind = op.get_bind()
    insp = inspect(bind)
    tables = {t.lower() for t in insp.get_table_names()}
    if "workspace" not in tables:
        return
    cols = {c["name"] for c in insp.get_columns("workspace")}
    if "archived_at" not in cols:
        return
    op.drop_index("ix_workspace_archived_at", table_name="workspace")
    with op.batch_alter_table("workspace") as batch:
        batch.drop_column("archived_at")
