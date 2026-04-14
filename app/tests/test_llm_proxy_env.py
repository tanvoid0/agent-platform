from unittest import mock

import pytest

from llm_proxy_env import (
    llm_proxy_base_url_v1,
    llm_proxy_http_timeout_seconds,
    llm_proxy_master_key,
)


@pytest.fixture(autouse=True)
def clear_docker_fix_env(monkeypatch):
    monkeypatch.delenv("AGENT_PLATFORM_ORCHESTRATOR_DOCKER_FIX", raising=False)


def test_default_localhost_when_not_docker(monkeypatch):
    monkeypatch.delenv("LLM_ORCHESTRATOR_BASE_URL", raising=False)
    with mock.patch("llm_proxy_env.os.path.exists", return_value=False):
        assert llm_proxy_base_url_v1() == "http://127.0.0.1:18410/v1"


def test_respects_explicit_url_when_not_docker(monkeypatch):
    monkeypatch.setenv("LLM_ORCHESTRATOR_BASE_URL", "http://10.0.0.5:9999/v1")
    with mock.patch("llm_proxy_env.os.path.exists", return_value=False):
        assert llm_proxy_base_url_v1() == "http://10.0.0.5:9999/v1"


def test_appends_v1_when_base_omits_path(monkeypatch):
    monkeypatch.setenv("LLM_ORCHESTRATOR_BASE_URL", "http://10.0.0.5:9999")
    with mock.patch("llm_proxy_env.os.path.exists", return_value=False):
        assert llm_proxy_base_url_v1() == "http://10.0.0.5:9999/v1"


def test_rewrites_loopback_in_docker(monkeypatch):
    monkeypatch.setenv("LLM_ORCHESTRATOR_BASE_URL", "http://127.0.0.1:18410/v1")
    with mock.patch("llm_proxy_env.os.path.exists", return_value=True):
        assert llm_proxy_base_url_v1() == "http://host.docker.internal:18410/v1"


def test_no_rewrite_non_loopback_in_docker(monkeypatch):
    monkeypatch.setenv("LLM_ORCHESTRATOR_BASE_URL", "http://llm-proxy:8080/v1")
    with mock.patch("llm_proxy_env.os.path.exists", return_value=True):
        assert llm_proxy_base_url_v1() == "http://llm-proxy:8080/v1"


def test_opt_out_rewrite(monkeypatch):
    monkeypatch.setenv("LLM_ORCHESTRATOR_BASE_URL", "http://127.0.0.1:18410/v1")
    monkeypatch.setenv("AGENT_PLATFORM_ORCHESTRATOR_DOCKER_FIX", "0")
    with mock.patch("llm_proxy_env.os.path.exists", return_value=True):
        assert llm_proxy_base_url_v1() == "http://127.0.0.1:18410/v1"


def test_llm_proxy_master_key_prefers_canonical_env(monkeypatch):
    monkeypatch.setenv("AGENT_PLATFORM_MASTER_KEY", "canonical")
    assert llm_proxy_master_key() == "canonical"


def test_llm_proxy_master_key_ignores_litellm(monkeypatch):
    monkeypatch.delenv("AGENT_PLATFORM_MASTER_KEY", raising=False)
    monkeypatch.setenv("LITELLM_MASTER_KEY", "legacy")
    assert llm_proxy_master_key() == ""


def test_llm_proxy_master_key_empty(monkeypatch):
    monkeypatch.delenv("AGENT_PLATFORM_MASTER_KEY", raising=False)
    monkeypatch.delenv("LITELLM_MASTER_KEY", raising=False)
    assert llm_proxy_master_key() == ""


def test_llm_proxy_http_timeout_default(monkeypatch):
    monkeypatch.delenv("AGENT_PLATFORM_ORCHESTRATOR_TIMEOUT_SECONDS", raising=False)
    assert llm_proxy_http_timeout_seconds() == 600.0


def test_llm_proxy_http_timeout_env(monkeypatch):
    monkeypatch.setenv("AGENT_PLATFORM_ORCHESTRATOR_TIMEOUT_SECONDS", "180")
    assert llm_proxy_http_timeout_seconds() == 180.0


def test_llm_proxy_http_timeout_invalid_falls_back(monkeypatch):
    monkeypatch.setenv("AGENT_PLATFORM_ORCHESTRATOR_TIMEOUT_SECONDS", "not-a-number")
    assert llm_proxy_http_timeout_seconds() == 600.0


def test_llm_proxy_http_timeout_clamped(monkeypatch):
    monkeypatch.setenv("AGENT_PLATFORM_ORCHESTRATOR_TIMEOUT_SECONDS", "5")
    assert llm_proxy_http_timeout_seconds() == 10.0
