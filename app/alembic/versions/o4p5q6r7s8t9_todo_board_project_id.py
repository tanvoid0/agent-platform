"""todo board project_id

Revision ID: o4p5q6r7s8t9
Revises: n3o4p5q6r7s8
Create Date: 2026-05-24

"""
from __future__ import annotations

from typing import Sequence, Union

from alembic import op
import sqlalchemy as sa


revision: str = "o4p5q6r7s8t9"
down_revision: Union[str, Sequence[str], None] = "n3o4p5q6r7s8"
branch_labels: Union[str, Sequence[str], None] = None
depends_on: Union[str, Sequence[str], None] = None


def upgrade() -> None:
    with op.batch_alter_table("todo_boards") as batch:
        batch.add_column(sa.Column("project_id", sa.Integer(), nullable=True))
        batch.create_foreign_key(
            "fk_todo_boards_project_id",
            "project",
            ["project_id"],
            ["id"],
            ondelete="CASCADE",
        )
    op.create_index("ix_todo_boards_project_id", "todo_boards", ["project_id"])


def downgrade() -> None:
    op.drop_index("ix_todo_boards_project_id", table_name="todo_boards")
    with op.batch_alter_table("todo_boards") as batch:
        batch.drop_constraint("fk_todo_boards_project_id", type_="foreignkey")
        batch.drop_column("project_id")
