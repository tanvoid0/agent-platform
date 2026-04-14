"""Tests for optional `config/agent_platform.yaml` defaults."""

from __future__ import annotations

import os

import pytest


def test_yaml_setdefault_applies(tmp_path, monkeypatch):
    monkeypatch.delenv("AGENT_PLATFORM_TEST_FOO", raising=False)
    y = tmp_path / "agent_platform.yaml"
    y.write_text(
        "version: 1\nenv:\n  AGENT_PLATFORM_TEST_FOO: xyzzy\n",
        encoding="utf-8",
    )
    monkeypatch.setenv("AGENT_PLATFORM_CONFIG_YAML", str(y))

    import importlib

    import platform_config

    importlib.reload(platform_config)
    platform_config.apply_platform_yaml_defaults()
    assert os.environ.get("AGENT_PLATFORM_TEST_FOO") == "xyzzy"


def test_env_wins_over_yaml(tmp_path, monkeypatch):
    y = tmp_path / "agent_platform.yaml"
    y.write_text(
        "version: 1\nenv:\n  AGENT_PLATFORM_TEST_BAR: from-yaml\n",
        encoding="utf-8",
    )
    monkeypatch.setenv("AGENT_PLATFORM_CONFIG_YAML", str(y))
    monkeypatch.setenv("AGENT_PLATFORM_TEST_BAR", "from-env")

    import importlib

    import platform_config

    importlib.reload(platform_config)
    platform_config.apply_platform_yaml_defaults()
    assert os.environ.get("AGENT_PLATFORM_TEST_BAR") == "from-env"


def test_secret_keys_never_applied_from_yaml(tmp_path, monkeypatch):
    y = tmp_path / "agent_platform.yaml"
    y.write_text(
        'version: 1\nenv:\n  AGENT_PLATFORM_MASTER_KEY: "from-yaml"\n',
        encoding="utf-8",
    )
    monkeypatch.setenv("AGENT_PLATFORM_CONFIG_YAML", str(y))
    monkeypatch.delenv("AGENT_PLATFORM_MASTER_KEY", raising=False)

    import importlib

    import platform_config

    importlib.reload(platform_config)
    platform_config.apply_platform_yaml_defaults()
    assert os.environ.get("AGENT_PLATFORM_MASTER_KEY") is None
