"""project table and process.project_id

Revision ID: k0l1m2n3o4p5
Revises: j4k5l6m7n8o9
Create Date: 2026-04-12

"""
from typing import Sequence, Union

from alembic import op
import sqlalchemy as sa
from sqlalchemy import inspect


revision: str = "k0l1m2n3o4p5"
down_revision: Union[str, Sequence[str], None] = "j4k5l6m7n8o9"
branch_labels: Union[str, Sequence[str], None] = None
depends_on: Union[str, Sequence[str], None] = None


def upgrade() -> None:
    bind = op.get_bind()
    insp = inspect(bind)
    tables = insp.get_table_names()
    if "project" not in tables:
        op.create_table(
            "project",
            sa.Column("id", sa.Integer(), nullable=False),
            sa.Column("name", sa.String(length=256), nullable=False),
            sa.Column("description", sa.String(length=4096), nullable=True),
            sa.Column("color", sa.String(length=32), nullable=True),
            sa.Column("created_at", sa.DateTime(), nullable=False),
            sa.Column("updated_at", sa.DateTime(), nullable=False),
            sa.PrimaryKeyConstraint("id"),
        )
    proc_cols = {c["name"] for c in insp.get_columns("process")} if "process" in insp.get_table_names() else set()
    if "project_id" not in proc_cols:
        with op.batch_alter_table("process") as batch:
            batch.add_column(sa.Column("project_id", sa.Integer(), nullable=True))
            batch.create_foreign_key(
                "fk_process_project_id_project",
                "project",
                ["project_id"],
                ["id"],
                ondelete="SET NULL",
            )


def downgrade() -> None:
    bind = op.get_bind()
    insp = inspect(bind)
    proc_cols = {c["name"] for c in insp.get_columns("process")} if "process" in insp.get_table_names() else set()
    if "project_id" in proc_cols:
        with op.batch_alter_table("process") as batch:
            batch.drop_constraint("fk_process_project_id_project", type_="foreignkey")
            batch.drop_column("project_id")
    if "project" in insp.get_table_names():
        op.drop_table("project")
