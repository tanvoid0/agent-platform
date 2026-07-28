"""Thin async wrapper around Ollama native /api/* endpoints."""

from __future__ import annotations

import json
from typing import Any, AsyncIterator

import httpx

from llm_proxy.core.provider_config import ollama_api_base
from llm_proxy.services.model_catalog_cache import get_catalog_cache
from llm_proxy.services.upstream_http import get_with_retry, post_with_retry


def _base() -> str:
    base = ollama_api_base().strip()
    if not base:
        raise RuntimeError("OLLAMA_API_BASE is not configured")
    return base.rstrip("/")


async def list_models() -> dict[str, Any]:
    r = await get_with_retry(f"{_base()}/api/tags", timeout=15.0, context="model_ops_tags")
    if r.status_code != 200:
        raise RuntimeError(f"Ollama /api/tags returned {r.status_code}")
    return r.json()


async def show_model(name: str) -> dict[str, Any]:
    r = await post_with_retry(
        f"{_base()}/api/show",
        json={"name": name},
        timeout=30.0,
        context="model_ops_show",
    )
    if r.status_code != 200:
        raise RuntimeError(f"Ollama /api/show returned {r.status_code}: {r.text[:200]}")
    return r.json()


async def delete_model(name: str) -> None:
    r = await post_with_retry(
        f"{_base()}/api/delete",
        json={"name": name},
        timeout=60.0,
        context="model_ops_delete",
    )
    if r.status_code != 200:
        raise RuntimeError(f"Ollama /api/delete returned {r.status_code}: {r.text[:200]}")
    await get_catalog_cache().refresh_ollama_now()


async def create_model(name: str, modelfile: str | None = None, **fields: Any) -> AsyncIterator[dict[str, Any]]:
    body: dict[str, Any] = {"model": name, "stream": True}
    if modelfile:
        body["modelfile"] = modelfile
    else:
        body.update(fields)

    async with httpx.AsyncClient(timeout=httpx.Timeout(600.0, connect=10.0)) as client:
        async with client.stream("POST", f"{_base()}/api/create", json=body) as resp:
            if resp.status_code != 200:
                text = (await resp.aread()).decode(errors="replace")[:500]
                raise RuntimeError(f"Ollama /api/create returned {resp.status_code}: {text}")
            async for line in resp.aiter_lines():
                if not line.strip():
                    continue
                try:
                    yield json.loads(line)
                except json.JSONDecodeError:
                    yield {"status": line.strip()}

    await get_catalog_cache().refresh_ollama_now()


async def create_model_sync(name: str, modelfile: str) -> dict[str, Any]:
    last: dict[str, Any] = {"status": "unknown"}
    async for event in create_model(name, modelfile=modelfile):
        last = event
    return last


async def pull_model(name: str) -> AsyncIterator[dict[str, Any]]:
    async with httpx.AsyncClient(timeout=httpx.Timeout(600.0, connect=10.0)) as client:
        async with client.stream(
            "POST", f"{_base()}/api/pull", json={"name": name, "stream": True}
        ) as resp:
            if resp.status_code != 200:
                text = (await resp.aread()).decode(errors="replace")[:500]
                raise RuntimeError(f"Ollama /api/pull returned {resp.status_code}: {text}")
            async for line in resp.aiter_lines():
                if not line.strip():
                    continue
                try:
                    yield json.loads(line)
                except json.JSONDecodeError:
                    yield {"status": line.strip()}

    await get_catalog_cache().refresh_ollama_now()


async def pull_model_sync(name: str) -> dict[str, Any]:
    last: dict[str, Any] = {"status": "unknown"}
    async for event in pull_model(name):
        last = event
    return last


async def copy_model(source: str, destination: str) -> AsyncIterator[dict[str, Any]]:
    async with httpx.AsyncClient(timeout=httpx.Timeout(600.0, connect=10.0)) as client:
        async with client.stream(
            "POST",
            f"{_base()}/api/copy",
            json={"source": source, "destination": destination, "stream": True},
        ) as resp:
            if resp.status_code != 200:
                text = (await resp.aread()).decode(errors="replace")[:500]
                raise RuntimeError(f"Ollama /api/copy returned {resp.status_code}: {text}")
            async for line in resp.aiter_lines():
                if not line.strip():
                    continue
                try:
                    yield json.loads(line)
                except json.JSONDecodeError:
                    yield {"status": line.strip()}

    await get_catalog_cache().refresh_ollama_now()


async def copy_model_sync(source: str, destination: str) -> dict[str, Any]:
    last: dict[str, Any] = {"status": "unknown"}
    async for event in copy_model(source, destination):
        last = event
    return last
