"""process.token_id — attribute a run to the API token that started it

Revision ID: w2x3y4z5a6b7
Revises: v1w2x3y4z5a6
Create Date: 2026-07-01

"""
from __future__ import annotations

from typing import Sequence, Union

from alembic import op
import sqlalchemy as sa


revision: str = "w2x3y4z5a6b7"
down_revision: Union[str, Sequence[str], None] = "v1w2x3y4z5a6"
branch_labels: Union[str, Sequence[str], None] = None
depends_on: Union[str, Sequence[str], None] = None


def upgrade() -> None:
    with op.batch_alter_table("process") as batch:
        batch.add_column(sa.Column("token_id", sa.Integer(), nullable=True))
    op.create_index("ix_process_token_id", "process", ["token_id"], unique=False)


def downgrade() -> None:
    op.drop_index("ix_process_token_id", table_name="process")
    with op.batch_alter_table("process") as batch:
        batch.drop_column("token_id")
