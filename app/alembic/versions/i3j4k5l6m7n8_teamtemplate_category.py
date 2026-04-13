"""team template category for library grouping

Revision ID: i3j4k5l6m7n8
Revises: h2i3j4k5l6m7
Create Date: 2026-04-12

"""
from typing import Sequence, Union

from alembic import op
import sqlalchemy as sa


revision: str = "i3j4k5l6m7n8"
down_revision: Union[str, Sequence[str], None] = "h2i3j4k5l6m7"
branch_labels: Union[str, Sequence[str], None] = None
depends_on: Union[str, Sequence[str], None] = None


def upgrade() -> None:
    op.add_column(
        "teamtemplate",
        sa.Column("category", sa.String(length=128), nullable=True),
    )


def downgrade() -> None:
    op.drop_column("teamtemplate", "category")
