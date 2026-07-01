"""In-process fixed-window per-token rate limiter.

Single-instance only (counters live in this process's memory) — fine for the
current single-instance deployment. If the platform is later scaled
horizontally, this needs to move to a shared store (e.g. Redis) to be
effective across instances.
"""

from __future__ import annotations

import time
from threading import Lock

from api_tokens.exceptions import RateLimitExceededError

_lock = Lock()
_windows: dict[int, tuple[int, int]] = {}  # token_id -> (window_start_minute, count)


def check_and_increment(token_id: int, limit_per_minute: int | None) -> None:
    if not limit_per_minute:
        return
    minute = int(time.time() // 60)
    with _lock:
        window_start, count = _windows.get(token_id, (minute, 0))
        if window_start != minute:
            window_start, count = minute, 0
        count += 1
        _windows[token_id] = (window_start, count)
    if count > limit_per_minute:
        raise RateLimitExceededError(f"Rate limit exceeded ({limit_per_minute} requests/min).")
