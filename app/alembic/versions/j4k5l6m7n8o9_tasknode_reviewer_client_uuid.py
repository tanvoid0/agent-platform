"""tasknode reviewer_client_uuid

Revision ID: j4k5l6m7n8o9
Revises: i3j4k5l6m7n8
Create Date: 2026-04-12

"""
from typing import Sequence, Union

from alembic import op
import sqlalchemy as sa


revision: str = "j4k5l6m7n8o9"
down_revision: Union[str, Sequence[str], None] = "i3j4k5l6m7n8"
branch_labels: Union[str, Sequence[str], None] = None
depends_on: Union[str, Sequence[str], None] = None


def upgrade() -> None:
    op.add_column(
        "tasknode",
        sa.Column("reviewer_client_uuid", sa.String(), nullable=True),
    )
    op.create_index(
        op.f("ix_tasknode_reviewer_client_uuid"),
        "tasknode",
        ["reviewer_client_uuid"],
        unique=False,
    )


def downgrade() -> None:
    op.drop_index(op.f("ix_tasknode_reviewer_client_uuid"), table_name="tasknode")
    op.drop_column("tasknode", "reviewer_client_uuid")
