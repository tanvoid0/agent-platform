"""workspace tenant above project; workspace_id on project + api_tokens

Revision ID: z5a6b7c8d9e0
Revises: y4z5a6b7c8d9
Create Date: 2026-07-07

Introduces the Workspace tenant. A seed "Default" workspace absorbs every
existing project and token so nothing is orphaned. Order is deliberate so a
downgrade stays safe — api_tokens.project_id is only made nullable here, never
dropped.

SQLite note: SQLite cannot ``ALTER ... SET NOT NULL`` in place, so the NOT NULL
promotion runs only on PostgreSQL. On SQLite the columns stay nullable and the
non-null tenant is enforced in application code (see api_tokens/auth.py).
"""
from __future__ import annotations

from typing import Sequence, Union

from alembic import op
import sqlalchemy as sa
from sqlalchemy import inspect


revision: str = "z5a6b7c8d9e0"
down_revision: Union[str, Sequence[str], None] = "y4z5a6b7c8d9"
branch_labels: Union[str, Sequence[str], None] = None
depends_on: Union[str, Sequence[str], None] = None


def upgrade() -> None:
    bind = op.get_bind()
    insp = inspect(bind)
    is_pg = bind.dialect.name == "postgresql"
    tables = {t.lower() for t in insp.get_table_names()}

    # 1. workspace table
    if "workspace" not in tables:
        op.create_table(
            "workspace",
            sa.Column("id", sa.Integer(), nullable=False, primary_key=True),
            sa.Column("name", sa.String(length=256), nullable=False),
            sa.Column("slug", sa.String(length=128), nullable=False),
            sa.Column("description", sa.String(length=4096), nullable=True),
            sa.Column("created_at", sa.DateTime(), nullable=False, server_default=sa.text("CURRENT_TIMESTAMP")),
            sa.Column("updated_at", sa.DateTime(), nullable=False, server_default=sa.text("CURRENT_TIMESTAMP")),
        )
        op.create_index("ix_workspace_slug", "workspace", ["slug"], unique=True)

    # 2. seed Default workspace, capture id
    default_id = bind.execute(
        sa.text("SELECT id FROM workspace WHERE slug = 'default'")
    ).scalar()
    if default_id is None:
        bind.execute(
            sa.text(
                "INSERT INTO workspace (name, slug, description) "
                "VALUES ('Default', 'default', 'Auto-created tenant for existing projects')"
            )
        )
        default_id = bind.execute(
            sa.text("SELECT id FROM workspace WHERE slug = 'default'")
        ).scalar()

    # 3. project.workspace_id (nullable + FK + index)
    proj_cols = {c["name"] for c in insp.get_columns("project")} if "project" in tables else set()
    if "project" in tables and "workspace_id" not in proj_cols:
        with op.batch_alter_table("project") as batch:
            batch.add_column(sa.Column("workspace_id", sa.Integer(), nullable=True))
            batch.create_foreign_key(
                "fk_project_workspace_id_workspace", "workspace", ["workspace_id"], ["id"]
            )
        op.create_index("ix_project_workspace_id", "project", ["workspace_id"])
        # 4. backfill
        bind.execute(
            sa.text("UPDATE project SET workspace_id = :wid WHERE workspace_id IS NULL"),
            {"wid": default_id},
        )
        # 5. NOT NULL (PostgreSQL only)
        if is_pg:
            op.alter_column("project", "workspace_id", nullable=False)

    # 6. api_tokens.workspace_id (nullable + FK + index)
    tok_cols = {c["name"] for c in insp.get_columns("api_tokens")} if "api_tokens" in tables else set()
    if "api_tokens" in tables and "workspace_id" not in tok_cols:
        with op.batch_alter_table("api_tokens") as batch:
            batch.add_column(sa.Column("workspace_id", sa.Integer(), nullable=True))
            batch.create_foreign_key(
                "fk_api_tokens_workspace_id_workspace", "workspace", ["workspace_id"], ["id"]
            )
            # project_id is no longer required (workspace-only scoping model).
            batch.alter_column("project_id", existing_type=sa.Integer(), nullable=True)
        op.create_index("ix_api_tokens_workspace_id", "api_tokens", ["workspace_id"])
        # 7. backfill from each token's project (fall back to Default for null/dangling)
        bind.execute(
            sa.text(
                "UPDATE api_tokens SET workspace_id = "
                "(SELECT p.workspace_id FROM project p WHERE p.id = api_tokens.project_id) "
                "WHERE workspace_id IS NULL"
            )
        )
        bind.execute(
            sa.text("UPDATE api_tokens SET workspace_id = :wid WHERE workspace_id IS NULL"),
            {"wid": default_id},
        )
        # 8. workspace_id NOT NULL (PostgreSQL only; SQLite enforces in code)
        if is_pg:
            op.alter_column("api_tokens", "workspace_id", nullable=False)


def downgrade() -> None:
    bind = op.get_bind()
    insp = inspect(bind)
    is_pg = bind.dialect.name == "postgresql"
    tables = {t.lower() for t in insp.get_table_names()}

    if "api_tokens" in tables:
        tok_cols = {c["name"] for c in insp.get_columns("api_tokens")}
        if "workspace_id" in tok_cols:
            with op.batch_alter_table("api_tokens") as batch:
                batch.drop_constraint("fk_api_tokens_workspace_id_workspace", type_="foreignkey")
                batch.drop_column("workspace_id")
        if is_pg:
            op.alter_column("api_tokens", "project_id", nullable=False)

    if "project" in tables:
        proj_cols = {c["name"] for c in insp.get_columns("project")}
        if "workspace_id" in proj_cols:
            with op.batch_alter_table("project") as batch:
                batch.drop_constraint("fk_project_workspace_id_workspace", type_="foreignkey")
                batch.drop_column("workspace_id")

    if "workspace" in tables:
        op.drop_index("ix_workspace_slug", table_name="workspace")
        op.drop_table("workspace")
