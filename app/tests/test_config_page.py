"""Lean config UI at `/config`."""

def test_root_redirects_to_config(client):
    c, _, _ = client
    r = c.get("/", follow_redirects=False)
    assert r.status_code in (307, 308)
    assert r.headers.get("location") == "/config"


def test_config_page_returns_html(client):
    c, _, _ = client
    r = c.get("/config")
    assert r.status_code == 200
    assert "text/html" in (r.headers.get("content-type") or "")
    assert "Default LLM" in r.text
