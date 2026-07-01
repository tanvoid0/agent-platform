"""Per-token usage rollup: daily aggregate row + lifetime counters on ApiToken."""

from __future__ import annotations

from datetime import datetime

from sqlmodel import Session, select

from models import ApiToken, ApiTokenUsageDaily


def record_api_token_usage(
    session: Session,
    token_id: int | None,
    *,
    tokens: int = 0,
    cost: float = 0.0,
    is_error: bool = False,
) -> None:
    """No-op for master-key callers (token_id is None) — only project tokens are tracked."""
    if token_id is None:
        return

    today = datetime.utcnow().strftime("%Y-%m-%d")

    row = session.exec(
        select(ApiTokenUsageDaily).where(
            ApiTokenUsageDaily.token_id == token_id,
            ApiTokenUsageDaily.usage_date == today,
        )
    ).first()
    if row is None:
        row = ApiTokenUsageDaily(token_id=token_id, usage_date=today)
    row.request_count += 1
    row.error_count += 1 if is_error else 0
    row.total_tokens += tokens
    row.total_cost += cost
    session.add(row)

    token = session.get(ApiToken, token_id)
    if token is not None:
        token.total_requests += 1
        token.total_errors += 1 if is_error else 0
        token.total_tokens += tokens
        token.total_cost += cost
        session.add(token)

    session.commit()
