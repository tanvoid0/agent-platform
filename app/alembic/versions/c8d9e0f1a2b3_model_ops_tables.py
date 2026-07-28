"""Alembic migration: model_ops tables."""

from __future__ import annotations

from typing import Sequence, Union

import sqlalchemy as sa
from alembic import op

revision: str = "c8d9e0f1a2b3"
down_revision: Union[str, Sequence[str], None] = "b7c8d9e0f1a2"
branch_labels: Union[str, Sequence[str], None] = None
depends_on: Union[str, Sequence[str], None] = None


def upgrade() -> None:
    from sqlalchemy import inspect

    bind = op.get_context().bind
    inspector = inspect(bind)
    existing = {t.lower() for t in inspector.get_table_names()}

    if "model_projects" not in existing:
        op.create_table(
            "model_projects",
            sa.Column("id", sa.Integer(), nullable=False),
            sa.Column("name", sa.String(length=128), nullable=False),
            sa.Column("description", sa.String(length=512), nullable=True),
            sa.Column("manifest_json", sa.Text(), nullable=True),
            sa.Column("workspace_id", sa.Integer(), nullable=True),
            sa.Column("created_at", sa.DateTime(), nullable=False),
            sa.Column("updated_at", sa.DateTime(), nullable=False),
            sa.PrimaryKeyConstraint("id"),
        )
        op.create_index("ix_model_projects_name", "model_projects", ["name"], unique=True)
        op.create_index("ix_model_projects_workspace_id", "model_projects", ["workspace_id"])

    if "model_build_jobs" not in existing:
        op.create_table(
            "model_build_jobs",
            sa.Column("id", sa.Integer(), nullable=False),
            sa.Column("project_id", sa.Integer(), nullable=False),
            sa.Column("stages_json", sa.Text(), nullable=False),
            sa.Column("status", sa.String(length=32), nullable=False),
            sa.Column("current_stage", sa.String(length=32), nullable=True),
            sa.Column("log_path", sa.String(length=1024), nullable=True),
            sa.Column("result_json", sa.Text(), nullable=True),
            sa.Column("register_alias", sa.String(length=128), nullable=True),
            sa.Column("error_message", sa.String(length=2048), nullable=True),
            sa.Column("process_id", sa.Integer(), nullable=True),
            sa.Column("created_at", sa.DateTime(), nullable=False),
            sa.Column("started_at", sa.DateTime(), nullable=True),
            sa.Column("finished_at", sa.DateTime(), nullable=True),
            sa.ForeignKeyConstraint(["project_id"], ["model_projects.id"]),
            sa.PrimaryKeyConstraint("id"),
        )
        op.create_index("ix_model_build_jobs_project_id", "model_build_jobs", ["project_id"])
        op.create_index("ix_model_build_jobs_status", "model_build_jobs", ["status"])
        op.create_index("ix_model_build_jobs_process_id", "model_build_jobs", ["process_id"])

    if "model_registry_entries" not in existing:
        op.create_table(
            "model_registry_entries",
            sa.Column("id", sa.Integer(), nullable=False),
            sa.Column("project_id", sa.Integer(), nullable=False),
            sa.Column("version", sa.String(length=32), nullable=False),
            sa.Column("ollama_tag", sa.String(length=128), nullable=False),
            sa.Column("base_model", sa.String(length=256), nullable=True),
            sa.Column("adapter_path", sa.String(length=512), nullable=True),
            sa.Column("gguf_path", sa.String(length=512), nullable=True),
            sa.Column("eval_score", sa.Float(), nullable=True),
            sa.Column("is_active", sa.Boolean(), nullable=False, server_default=sa.false()),
            sa.Column("metadata_json", sa.Text(), nullable=True),
            sa.Column("created_at", sa.DateTime(), nullable=False),
            sa.ForeignKeyConstraint(["project_id"], ["model_projects.id"]),
            sa.PrimaryKeyConstraint("id"),
        )
        op.create_index("ix_model_registry_entries_project_id", "model_registry_entries", ["project_id"])
        op.create_index("ix_model_registry_entries_ollama_tag", "model_registry_entries", ["ollama_tag"])
        op.create_index("ix_model_registry_entries_is_active", "model_registry_entries", ["is_active"])


def downgrade() -> None:
    op.drop_table("model_registry_entries")
    op.drop_table("model_build_jobs")
    op.drop_table("model_projects")
