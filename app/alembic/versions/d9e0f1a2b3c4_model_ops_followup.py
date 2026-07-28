"""Model ops follow-up: process link, ollama job types."""

from __future__ import annotations

from typing import Sequence, Union

import sqlalchemy as sa
from alembic import op

revision: str = "d9e0f1a2b3c4"
down_revision: Union[str, Sequence[str], None] = "c8d9e0f1a2b3"
branch_labels: Union[str, Sequence[str], None] = None
depends_on: Union[str, Sequence[str], None] = None


def upgrade() -> None:
    from sqlalchemy import inspect

    bind = op.get_context().bind
    inspector = inspect(bind)
    cols = {c["name"] for c in inspector.get_columns("process")} if "process" in inspector.get_table_names() else set()
    if "model_build_job_id" not in cols:
        with op.batch_alter_table("process") as batch:
            batch.add_column(sa.Column("model_build_job_id", sa.Integer(), nullable=True))
        op.create_index("ix_process_model_build_job_id", "process", ["model_build_job_id"])

    job_cols = (
        {c["name"] for c in inspector.get_columns("model_build_jobs")}
        if "model_build_jobs" in inspector.get_table_names()
        else set()
    )
    if "job_type" not in job_cols:
        with op.batch_alter_table("model_build_jobs") as batch:
            batch.add_column(sa.Column("job_type", sa.String(length=32), nullable=False, server_default="pipeline"))
            batch.add_column(sa.Column("operation_json", sa.Text(), nullable=True))
        op.create_index("ix_model_build_jobs_job_type", "model_build_jobs", ["job_type"])

    if "model_build_jobs" in inspector.get_table_names():
        job_info = {c["name"]: c for c in inspector.get_columns("model_build_jobs")}
        if job_info.get("project_id", {}).get("nullable") is False:
            with op.batch_alter_table("model_build_jobs") as batch:
                batch.alter_column("project_id", existing_type=sa.Integer(), nullable=True)


def downgrade() -> None:
    op.drop_index("ix_process_model_build_job_id", table_name="process")
    with op.batch_alter_table("process") as batch:
        batch.drop_column("model_build_job_id")
    op.drop_index("ix_model_build_jobs_job_type", table_name="model_build_jobs")
    with op.batch_alter_table("model_build_jobs") as batch:
        batch.drop_column("operation_json")
        batch.drop_column("job_type")
