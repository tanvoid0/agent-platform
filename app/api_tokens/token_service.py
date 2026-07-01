"""Opaque API token generation/hashing (GitHub PAT / Stripe key style)."""

from __future__ import annotations

import hashlib
import secrets
from typing import Tuple

TOKEN_PREFIX_DISPLAY_LEN = 8  # chars of the secret shown in the public prefix


def hash_token(raw: str) -> str:
    return hashlib.sha256(raw.encode("utf-8")).hexdigest()


def generate_token(env: str = "live") -> Tuple[str, str, str]:
    """Returns (full_token, display_prefix, token_hash). Only the hash is persisted."""
    secret = secrets.token_urlsafe(32)
    full = f"agp_{env}_{secret}"
    prefix = f"agp_{env}_{secret[:TOKEN_PREFIX_DISPLAY_LEN]}"
    return full, prefix, hash_token(full)
