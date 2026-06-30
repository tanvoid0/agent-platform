"""task_review_fields

Revision ID: a1b2c3d4e5f6
Revises: 8bf04992de78
Create Date: 2026-04-10

"""
from typing import Sequence, Union

from alembic import op
import sqlalchemy as sa


revision: str = "a1b2c3d4e5f6"
down_revision: Union[str, Sequence[str], None] = "8bf04992de78"
branch_labels: Union[str, Sequence[str], None] = None
depends_on: Union[str, Sequence[str], None] = None


def upgrade() -> None:
    op.add_column(
        "tasknode",
        sa.Column("requires_review", sa.Boolean(), nullable=False, server_default=sa.text("false")),
    )
    op.add_column("tasknode", sa.Column("review_feedback", sa.String(), nullable=True))
    op.add_column(
        "tasknode",
        sa.Column("revision_count", sa.Integer(), nullable=False, server_default="0"),
    )
    op.add_column("tasknode", sa.Column("draft_output", sa.String(), nullable=True))


def downgrade() -> None:
    op.drop_column("tasknode", "draft_output")
    op.drop_column("tasknode", "revision_count")
    op.drop_column("tasknode", "review_feedback")
    op.drop_column("tasknode", "requires_review")
