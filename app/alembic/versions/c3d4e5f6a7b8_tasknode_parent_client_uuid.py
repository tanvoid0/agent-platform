"""tasknode parent_client_uuid

Revision ID: c3d4e5f6a7b8
Revises: a1b2c3d4e5f6
Create Date: 2026-04-10

"""
from typing import Sequence, Union

from alembic import op
import sqlalchemy as sa


revision: str = "c3d4e5f6a7b8"
down_revision: Union[str, Sequence[str], None] = "a1b2c3d4e5f6"
branch_labels: Union[str, Sequence[str], None] = None
depends_on: Union[str, Sequence[str], None] = None


def upgrade() -> None:
    op.add_column(
        "tasknode",
        sa.Column("parent_client_uuid", sa.String(), nullable=True),
    )
    op.create_index(
        op.f("ix_tasknode_parent_client_uuid"),
        "tasknode",
        ["parent_client_uuid"],
        unique=False,
    )


def downgrade() -> None:
    op.drop_index(op.f("ix_tasknode_parent_client_uuid"), table_name="tasknode")
    op.drop_column("tasknode", "parent_client_uuid")
