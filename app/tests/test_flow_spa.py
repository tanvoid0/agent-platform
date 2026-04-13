"""Deep-link support for the Vite `/flow` SPA."""

from pathlib import Path

import pytest
from fastapi.testclient import TestClient

from main import app

_DIST_INDEX = Path(__file__).resolve().parent.parent / "static" / "dist" / "index.html"


@pytest.mark.skipif(not _DIST_INDEX.is_file(), reason="web dist not built (cd web && npm run build)")
def test_flow_deep_link_returns_index_html():
    with TestClient(app) as client:
        r = client.get("/flow/graph/1")
    assert r.status_code == 200
    assert "text/html" in (r.headers.get("content-type") or "")
