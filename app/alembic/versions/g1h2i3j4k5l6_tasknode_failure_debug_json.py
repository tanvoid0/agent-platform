"""tasknode failure_debug_json

Revision ID: g1h2i3j4k5l6
Revises: f8e9d0a1b2c3
Create Date: 2026-04-11

"""
from typing import Sequence, Union

from alembic import op
import sqlalchemy as sa


revision: str = "g1h2i3j4k5l6"
down_revision: Union[str, Sequence[str], None] = "f8e9d0a1b2c3"
branch_labels: Union[str, Sequence[str], None] = None
depends_on: Union[str, Sequence[str], None] = None


def upgrade() -> None:
    op.add_column(
        "tasknode",
        sa.Column("failure_debug_json", sa.String(), nullable=True),
    )


def downgrade() -> None:
    op.drop_column("tasknode", "failure_debug_json")
