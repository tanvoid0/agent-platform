"""Rename legacy ``run`` table to ``process`` and ``run_id`` columns (SQLite).

Revision ID: f8e9d0a1b2c3
Revises: e5f6a7b8c9d0
Create Date: 2026-04-11

"""
from __future__ import annotations

import sys
from pathlib import Path
from typing import Sequence, Union

from alembic import op

_app_dir = Path(__file__).resolve().parents[1]
if str(_app_dir) not in sys.path:
    sys.path.insert(0, str(_app_dir))

from process_table_sqlite import apply_process_table_sqlite  # noqa: E402


revision: str = "f8e9d0a1b2c3"
down_revision: Union[str, Sequence[str], None] = "e5f6a7b8c9d0"
branch_labels: Union[str, Sequence[str], None] = None
depends_on: Union[str, Sequence[str], None] = None


def upgrade() -> None:
    bind = op.get_bind()
    if bind.dialect.name != "sqlite":
        return
    apply_process_table_sqlite(bind)


def downgrade() -> None:
    raise NotImplementedError("Downgrade to legacy run/run_id names is not supported.")
