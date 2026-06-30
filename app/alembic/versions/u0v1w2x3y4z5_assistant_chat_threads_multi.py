"""assistant chat — multiple threads per project

Revision ID: u0v1w2x3y4z5
Revises: t9u0v1w2x3y4
Create Date: 2026-05-25

"""
from __future__ import annotations

from typing import Sequence, Union

from alembic import op
import sqlalchemy as sa


revision: str = "u0v1w2x3y4z5"
down_revision: Union[str, Sequence[str], None] = "t9u0v1w2x3y4"
branch_labels: Union[str, Sequence[str], None] = None
depends_on: Union[str, Sequence[str], None] = None


def upgrade() -> None:
    with op.batch_alter_table("assistant_chat_threads") as batch:
        batch.add_column(sa.Column("title", sa.String(length=128), nullable=True))

    op.drop_index("ix_assistant_chat_threads_project_id", table_name="assistant_chat_threads")
    op.create_index(
        "ix_assistant_chat_threads_project_id",
        "assistant_chat_threads",
        ["project_id"],
        unique=False,
    )


def downgrade() -> None:
    op.drop_index("ix_assistant_chat_threads_project_id", table_name="assistant_chat_threads")
    op.create_index(
        "ix_assistant_chat_threads_project_id",
        "assistant_chat_threads",
        ["project_id"],
        unique=True,
    )
    with op.batch_alter_table("assistant_chat_threads") as batch:
        batch.drop_column("title")
