"""teamtemplate.workspace_id — optional workspace ownership (NULL = global)

Revision ID: a6b7c8d9e0f1
Revises: z5a6b7c8d9e0
Create Date: 2026-07-07

Global templates keep workspace_id NULL and stay shared across every workspace;
workspace-owned templates carry their workspace id. Existing rows are left NULL
(global), preserving current behavior.
"""
from __future__ import annotations

from typing import Sequence, Union

from alembic import op
import sqlalchemy as sa
from sqlalchemy import inspect


revision: str = "a6b7c8d9e0f1"
down_revision: Union[str, Sequence[str], None] = "z5a6b7c8d9e0"
branch_labels: Union[str, Sequence[str], None] = None
depends_on: Union[str, Sequence[str], None] = None


def upgrade() -> None:
    bind = op.get_bind()
    insp = inspect(bind)
    if "teamtemplate" not in insp.get_table_names():
        return
    cols = {c["name"] for c in insp.get_columns("teamtemplate")}
    if "workspace_id" in cols:
        return
    with op.batch_alter_table("teamtemplate") as batch:
        batch.add_column(sa.Column("workspace_id", sa.Integer(), nullable=True))
        batch.create_foreign_key(
            "fk_teamtemplate_workspace_id_workspace", "workspace", ["workspace_id"], ["id"]
        )
    op.create_index("ix_teamtemplate_workspace_id", "teamtemplate", ["workspace_id"])


def downgrade() -> None:
    bind = op.get_bind()
    insp = inspect(bind)
    if "teamtemplate" not in insp.get_table_names():
        return
    cols = {c["name"] for c in insp.get_columns("teamtemplate")}
    if "workspace_id" not in cols:
        return
    op.drop_index("ix_teamtemplate_workspace_id", table_name="teamtemplate")
    with op.batch_alter_table("teamtemplate") as batch:
        batch.drop_constraint("fk_teamtemplate_workspace_id_workspace", type_="foreignkey")
        batch.drop_column("workspace_id")
