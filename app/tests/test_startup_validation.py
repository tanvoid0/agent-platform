from __future__ import annotations

import pytest

from startup_validation import assert_startup_config, collect_startup_validation_errors


def test_startup_validation_accepts_default_dev_env(monkeypatch, tmp_path):
    monkeypatch.delenv("AGENT_PLATFORM_ENV", raising=False)
    monkeypatch.delenv("AGENT_PLATFORM_MASTER_KEY", raising=False)
    monkeypatch.setenv("AGENT_PLATFORM_WORKSPACE_ROOT", str(tmp_path / "workspaces"))
    monkeypatch.setenv("CONFIG_DIR", str(tmp_path / "config"))

    assert collect_startup_validation_errors() == []


def test_startup_validation_requires_master_key_in_production(monkeypatch, tmp_path):
    monkeypatch.setenv("AGENT_PLATFORM_ENV", "production")
    monkeypatch.delenv("AGENT_PLATFORM_MASTER_KEY", raising=False)
    monkeypatch.setenv("AGENT_PLATFORM_WORKSPACE_ROOT", str(tmp_path / "workspaces"))
    monkeypatch.setenv("CONFIG_DIR", str(tmp_path / "config"))

    with pytest.raises(RuntimeError, match="AGENT_PLATFORM_MASTER_KEY is required"):
        assert_startup_config()


def test_startup_validation_rejects_invalid_yaml(monkeypatch, tmp_path):
    bad_yaml = tmp_path / "agent_platform.yaml"
    bad_yaml.write_text("env: [broken\n", encoding="utf-8")
    monkeypatch.setenv("AGENT_PLATFORM_CONFIG_YAML", str(bad_yaml))
    monkeypatch.setenv("AGENT_PLATFORM_WORKSPACE_ROOT", str(tmp_path / "workspaces"))
    monkeypatch.setenv("CONFIG_DIR", str(tmp_path / "config"))

    with pytest.raises(RuntimeError, match="agent_platform.yaml could not be parsed"):
        assert_startup_config()
