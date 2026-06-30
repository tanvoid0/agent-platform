"""Background model catalog cache. Prevents health checks from blocking on upstream queries."""

from __future__ import annotations

import asyncio
import logging
import time
from typing import Any

from llm_proxy.core.errors import LlmProxyError
from llm_proxy.core.provider_config import (
    lm_studio_api_base,
    lm_studio_api_key,
    ollama_api_base,
)
from llm_proxy.services.upstream_http import get_with_retry

logger = logging.getLogger("llm_proxy")


class ModelCatalogCache:
    """Thread-safe cache for Ollama tags and LM Studio models. Updates in background."""

    def __init__(self, refresh_interval_sec: float = 30.0):
        self._refresh_interval = refresh_interval_sec
        self._ollama_tags: list[str] = []
        self._ollama_tags_updated_at: float = 0
        self._lm_studio_models: list[str] = []
        self._lm_studio_models_updated_at: float = 0
        self._background_task: asyncio.Task[None] | None = None

    async def start_background_refresh(self) -> None:
        """Launch background refresh loop. Safe to call multiple times."""
        if self._background_task is not None:
            return
        self._background_task = asyncio.create_task(self._refresh_loop())
        logger.info("Started model catalog background refresh (interval=%.1fs)", self._refresh_interval)

    async def stop_background_refresh(self) -> None:
        """Gracefully stop background refresh."""
        if self._background_task:
            self._background_task.cancel()
            try:
                await self._background_task
            except asyncio.CancelledError:
                pass
            self._background_task = None
            logger.info("Stopped model catalog background refresh")

    async def _refresh_loop(self) -> None:
        """Periodically refresh both Ollama and LM Studio catalogs."""
        while True:
            try:
                await asyncio.sleep(self._refresh_interval)
                await asyncio.gather(
                    self._refresh_ollama_tags(),
                    self._refresh_lm_studio_models(),
                    return_exceptions=True,
                )
            except asyncio.CancelledError:
                break
            except Exception as e:
                logger.warning("Model catalog refresh error: %s", e)

    async def _refresh_ollama_tags(self) -> None:
        """Fetch Ollama model tags. Updates cache on success, preserves on error."""
        base = ollama_api_base().strip()
        if not base:
            return
        try:
            r = await get_with_retry(
                f"{base.rstrip('/')}/api/tags",
                timeout=8.0,
                context="catalog_cache_ollama_tags",
            )
            if r.status_code != 200:
                return
            payload = r.json()
            names: list[str] = []
            for item in payload.get("models") or []:
                if isinstance(item, dict) and isinstance(item.get("name"), str):
                    n = item["name"].strip()
                    if n:
                        names.append(n)
            self._ollama_tags = names
            self._ollama_tags_updated_at = time.time()
            logger.debug("Updated Ollama tags cache: %d models", len(names))
        except LlmProxyError as e:
            logger.debug("Failed to refresh Ollama tags: %s", e.message)

    async def _refresh_lm_studio_models(self) -> None:
        """Fetch LM Studio models. Updates cache on success, preserves on error."""
        base = lm_studio_api_base().strip()
        if not base:
            return
        headers: dict[str, str] = {}
        key = lm_studio_api_key()
        if key:
            headers["Authorization"] = f"Bearer {key}"
        try:
            r = await get_with_retry(
                f"{base.rstrip('/')}/v1/models",
                headers=headers,
                timeout=8.0,
                context="catalog_cache_lm_studio_models",
            )
            if r.status_code != 200:
                return
            payload = r.json()
            ids: list[str] = []
            for item in payload.get("data") or []:
                if isinstance(item, dict) and isinstance(item.get("id"), str):
                    mid = item["id"].strip()
                    if mid:
                        ids.append(mid)
            self._lm_studio_models = ids
            self._lm_studio_models_updated_at = time.time()
            logger.debug("Updated LM Studio models cache: %d models", len(ids))
        except LlmProxyError as e:
            logger.debug("Failed to refresh LM Studio models: %s", e.message)

    def get_ollama_tags(self) -> list[str]:
        """Return cached Ollama tags. Empty if never fetched or failed."""
        return self._ollama_tags.copy()

    def get_lm_studio_models(self) -> list[str]:
        """Return cached LM Studio models. Empty if never fetched or failed."""
        return self._lm_studio_models.copy()

    def ollama_tag_age_sec(self) -> float:
        """Seconds since last successful Ollama tags fetch. 0 if never fetched."""
        if not self._ollama_tags_updated_at:
            return 0
        return time.time() - self._ollama_tags_updated_at

    def lm_studio_models_age_sec(self) -> float:
        """Seconds since last successful LM Studio models fetch. 0 if never fetched."""
        if not self._lm_studio_models_updated_at:
            return 0
        return time.time() - self._lm_studio_models_updated_at


# Global singleton
_catalog_cache = ModelCatalogCache()


def get_catalog_cache() -> ModelCatalogCache:
    """Get the global model catalog cache."""
    return _catalog_cache
