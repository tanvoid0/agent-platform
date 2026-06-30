"""Rename legacy ``run`` table to ``process`` and ``run_id`` columns.

Revision ID: f8e9d0a1b2c3
Revises: e5f6a7b8c9d0
Create Date: 2026-04-11

"""
from __future__ import annotations

import sys
from pathlib import Path
from typing import Sequence, Union

from alembic import op
import sqlalchemy as sa
from sqlalchemy import inspect, text

_app_dir = Path(__file__).resolve().parents[1]
if str(_app_dir) not in sys.path:
    sys.path.insert(0, str(_app_dir))


revision: str = "f8e9d0a1b2c3"
down_revision: Union[str, Sequence[str], None] = "e5f6a7b8c9d0"
branch_labels: Union[str, Sequence[str], None] = None
depends_on: Union[str, Sequence[str], None] = None


def upgrade() -> None:
    bind = op.get_bind()
    if bind.dialect.name == "sqlite":
        from process_table_sqlite import apply_process_table_sqlite  # noqa: E402
        apply_process_table_sqlite(bind)
    elif bind.dialect.name == "postgresql":
        # PostgreSQL: rename run -> process, run_id -> process_id
        try:
            op.rename_table("run", "process")
            op.alter_column("tasknode", "run_id", new_column_name="process_id")
            op.alter_column("eventlog", "run_id", new_column_name="process_id")
        except Exception:
            # Tables may not exist yet or already renamed
            pass


def downgrade() -> None:
    raise NotImplementedError("Downgrade to legacy run/run_id names is not supported.")
