"""process client_id for logical client scope

Revision ID: h2i3j4k5l6m7
Revises: g1h2i3j4k5l6
Create Date: 2026-04-11

"""
from typing import Sequence, Union

from alembic import op
import sqlalchemy as sa


revision: str = "h2i3j4k5l6m7"
down_revision: Union[str, Sequence[str], None] = "g1h2i3j4k5l6"
branch_labels: Union[str, Sequence[str], None] = None
depends_on: Union[str, Sequence[str], None] = None


def upgrade() -> None:
    op.add_column(
        "process",
        sa.Column("client_id", sa.String(length=256), nullable=True),
    )
    op.create_index("ix_process_client_id", "process", ["client_id"], unique=False)


def downgrade() -> None:
    op.drop_index("ix_process_client_id", table_name="process")
    op.drop_column("process", "client_id")
