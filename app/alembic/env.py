"""Alembic environment: uses the same SQLModel metadata and engine as the app."""

from logging.config import fileConfig

from alembic import context
from sqlmodel import SQLModel

# Register models on SQLModel.metadata
import models  # noqa: F401
from database import engine

config = context.config

if config.config_file_name is not None:
    fileConfig(config.config_file_name)

# Override ini placeholder so offline/CLI and app share the real URL
config.set_main_option("sqlalchemy.url", str(engine.url).replace("%", "%%"))

target_metadata = SQLModel.metadata


def run_migrations_offline() -> None:
    url = config.get_main_option("sqlalchemy.url")
    context.configure(
        url=url,
        target_metadata=target_metadata,
        literal_binds=True,
        dialect_opts={"paramstyle": "named"},
    )

    with context.begin_transaction():
        context.run_migrations()


def run_migrations_online() -> None:
    with engine.connect() as connection:
        context.configure(
            connection=connection,
            target_metadata=target_metadata,
            compare_type=True,
        )

        with context.begin_transaction():
            context.run_migrations()


if context.is_offline_mode():
    run_migrations_offline()
else:
    run_migrations_online()
