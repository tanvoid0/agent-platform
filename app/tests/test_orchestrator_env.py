from unittest import mock

import pytest

from orchestrator_env import (
    orchestrator_base_url_v1,
    orchestrator_http_timeout_seconds,
    orchestrator_master_key,
)


@pytest.fixture(autouse=True)
def clear_docker_fix_env(monkeypatch):
    monkeypatch.delenv("AGENT_PLATFORM_ORCHESTRATOR_DOCKER_FIX", raising=False)


def test_default_localhost_when_not_docker(monkeypatch):
    monkeypatch.delenv("LLM_ORCHESTRATOR_BASE_URL", raising=False)
    with mock.patch("orchestrator_env.os.path.exists", return_value=False):
        assert orchestrator_base_url_v1() == "http://127.0.0.1:18408/v1"


def test_respects_explicit_url_when_not_docker(monkeypatch):
    monkeypatch.setenv("LLM_ORCHESTRATOR_BASE_URL", "http://10.0.0.5:9999/v1")
    with mock.patch("orchestrator_env.os.path.exists", return_value=False):
        assert orchestrator_base_url_v1() == "http://10.0.0.5:9999/v1"


def test_appends_v1_when_base_omits_path(monkeypatch):
    monkeypatch.setenv("LLM_ORCHESTRATOR_BASE_URL", "http://10.0.0.5:9999")
    with mock.patch("orchestrator_env.os.path.exists", return_value=False):
        assert orchestrator_base_url_v1() == "http://10.0.0.5:9999/v1"


def test_rewrites_loopback_in_docker(monkeypatch):
    monkeypatch.setenv("LLM_ORCHESTRATOR_BASE_URL", "http://127.0.0.1:18408/v1")
    with mock.patch("orchestrator_env.os.path.exists", return_value=True):
        assert orchestrator_base_url_v1() == "http://host.docker.internal:18408/v1"


def test_no_rewrite_non_loopback_in_docker(monkeypatch):
    monkeypatch.setenv("LLM_ORCHESTRATOR_BASE_URL", "http://orchestrator:8080/v1")
    with mock.patch("orchestrator_env.os.path.exists", return_value=True):
        assert orchestrator_base_url_v1() == "http://orchestrator:8080/v1"


def test_opt_out_rewrite(monkeypatch):
    monkeypatch.setenv("LLM_ORCHESTRATOR_BASE_URL", "http://127.0.0.1:18408/v1")
    monkeypatch.setenv("AGENT_PLATFORM_ORCHESTRATOR_DOCKER_FIX", "0")
    with mock.patch("orchestrator_env.os.path.exists", return_value=True):
        assert orchestrator_base_url_v1() == "http://127.0.0.1:18408/v1"


def test_orchestrator_master_key_reads_orchestrator_env(monkeypatch):
    monkeypatch.setenv("ORCHESTRATOR_MASTER_KEY", "a")
    assert orchestrator_master_key() == "a"


def test_orchestrator_master_key_ignores_litellm(monkeypatch):
    monkeypatch.delenv("ORCHESTRATOR_MASTER_KEY", raising=False)
    monkeypatch.setenv("LITELLM_MASTER_KEY", "legacy")
    assert orchestrator_master_key() == ""


def test_orchestrator_master_key_empty(monkeypatch):
    monkeypatch.delenv("ORCHESTRATOR_MASTER_KEY", raising=False)
    monkeypatch.delenv("LITELLM_MASTER_KEY", raising=False)
    assert orchestrator_master_key() == ""


def test_orchestrator_http_timeout_default(monkeypatch):
    monkeypatch.delenv("AGENT_PLATFORM_ORCHESTRATOR_TIMEOUT_SECONDS", raising=False)
    assert orchestrator_http_timeout_seconds() == 600.0


def test_orchestrator_http_timeout_env(monkeypatch):
    monkeypatch.setenv("AGENT_PLATFORM_ORCHESTRATOR_TIMEOUT_SECONDS", "180")
    assert orchestrator_http_timeout_seconds() == 180.0


def test_orchestrator_http_timeout_invalid_falls_back(monkeypatch):
    monkeypatch.setenv("AGENT_PLATFORM_ORCHESTRATOR_TIMEOUT_SECONDS", "not-a-number")
    assert orchestrator_http_timeout_seconds() == 600.0


def test_orchestrator_http_timeout_clamped(monkeypatch):
    monkeypatch.setenv("AGENT_PLATFORM_ORCHESTRATOR_TIMEOUT_SECONDS", "5")
    assert orchestrator_http_timeout_seconds() == 10.0
