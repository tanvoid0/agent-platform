"""api tokens — project-scoped external credentials + daily usage rollup

Revision ID: v1w2x3y4z5a6
Revises: u0v1w2x3y4z5
Create Date: 2026-07-01

Note: file-based SQLite dev setups skip auto-migration at startup (existing
platform limitation, see database.py). Run `alembic upgrade head` manually
after pulling this migration if developing against SQLite.
"""
from __future__ import annotations

from typing import Sequence, Union

from alembic import op
import sqlalchemy as sa


revision: str = "v1w2x3y4z5a6"
down_revision: Union[str, Sequence[str], None] = "u0v1w2x3y4z5"
branch_labels: Union[str, Sequence[str], None] = None
depends_on: Union[str, Sequence[str], None] = None


def upgrade() -> None:
    from sqlalchemy import inspect

    bind = op.get_context().bind
    inspector = inspect(bind)
    existing_tables = {t.lower() for t in inspector.get_table_names()}

    if "api_tokens" in existing_tables:
        return  # Tables already exist, skip

    op.create_table(
        "api_tokens",
        sa.Column("id", sa.Integer(), nullable=False, primary_key=True),
        sa.Column(
            "project_id",
            sa.Integer(),
            sa.ForeignKey("project.id", ondelete="CASCADE"),
            nullable=False,
            index=True,
        ),
        sa.Column("name", sa.String(length=256), nullable=False),
        sa.Column("prefix", sa.String(length=32), nullable=False, index=True),
        sa.Column("token_hash", sa.String(length=64), nullable=False, unique=True, index=True),
        sa.Column("scopes_json", sa.Text(), nullable=False, server_default="[]"),
        sa.Column("status", sa.String(length=16), nullable=False, server_default="active", index=True),
        sa.Column("rate_limit_per_minute", sa.Integer(), nullable=True),
        sa.Column("expires_at", sa.DateTime(), nullable=True),
        sa.Column("last_used_at", sa.DateTime(), nullable=True),
        sa.Column("revoked_at", sa.DateTime(), nullable=True),
        sa.Column("revoked_reason", sa.String(length=512), nullable=True),
        sa.Column("held_reason", sa.String(length=512), nullable=True),
        sa.Column("total_requests", sa.Integer(), nullable=False, server_default="0"),
        sa.Column("total_errors", sa.Integer(), nullable=False, server_default="0"),
        sa.Column("total_tokens", sa.Integer(), nullable=False, server_default="0"),
        sa.Column("total_cost", sa.Float(), nullable=False, server_default="0.0"),
        sa.Column("created_at", sa.DateTime(), nullable=False, server_default=sa.text("CURRENT_TIMESTAMP")),
        sa.Column("updated_at", sa.DateTime(), nullable=False, server_default=sa.text("CURRENT_TIMESTAMP")),
    )

    op.create_table(
        "api_token_usage_daily",
        sa.Column("id", sa.Integer(), nullable=False, primary_key=True),
        sa.Column(
            "token_id",
            sa.Integer(),
            sa.ForeignKey("api_tokens.id", ondelete="CASCADE"),
            nullable=False,
            index=True,
        ),
        sa.Column("usage_date", sa.String(length=10), nullable=False, index=True),
        sa.Column("request_count", sa.Integer(), nullable=False, server_default="0"),
        sa.Column("error_count", sa.Integer(), nullable=False, server_default="0"),
        sa.Column("total_tokens", sa.Integer(), nullable=False, server_default="0"),
        sa.Column("total_cost", sa.Float(), nullable=False, server_default="0.0"),
    )
    op.create_index(
        "ix_api_token_usage_daily_token_date",
        "api_token_usage_daily",
        ["token_id", "usage_date"],
        unique=True,
    )


def downgrade() -> None:
    op.drop_index("ix_api_token_usage_daily_token_date", table_name="api_token_usage_daily")
    op.drop_table("api_token_usage_daily")
    op.drop_table("api_tokens")
