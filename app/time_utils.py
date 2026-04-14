from __future__ import annotations

from datetime import datetime, timezone


def utc_now_naive() -> datetime:
    """Return current UTC time as naive datetime for existing DB shape."""
    return datetime.now(timezone.utc).replace(tzinfo=None)
