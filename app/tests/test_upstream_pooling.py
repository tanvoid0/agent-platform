"""The upstream client pools connections instead of opening one per call.

This is the check that silently stops being true if someone reintroduces a
per-call ``httpx.AsyncClient()``. The suite would still pass without it —
per-call clients are correct, just slow — so assert the pooling directly.
"""

from __future__ import annotations

import asyncio
import http.server
import threading

from llm_proxy.services.upstream_http import UpstreamHttpClient


def _serve(handler_cls) -> http.server.HTTPServer:
    server = http.server.HTTPServer(("127.0.0.1", 0), handler_cls)
    threading.Thread(target=server.serve_forever, daemon=True).start()
    return server


def test_sequential_gets_reuse_one_connection():
    """Three requests, one TCP connection — the point of the whole change."""
    # socketserver builds one handler instance per *connection*; with keep-alive
    # its handle() loop serves many requests on that one instance. So counting
    # setup() calls counts connections, and do_GET calls counts requests.
    connections: list[int] = []
    requests: list[str] = []

    class Handler(http.server.BaseHTTPRequestHandler):
        protocol_version = "HTTP/1.1"  # required, or the server closes each connection

        def setup(self):
            super().setup()
            connections.append(id(self.connection))

        def do_GET(self):
            requests.append(self.path)
            self.send_response(200)
            self.send_header("Content-Length", "2")
            self.end_headers()
            self.wfile.write(b"{}")

        def log_message(self, *args):
            pass

    server = _serve(Handler)
    base = f"http://127.0.0.1:{server.server_port}"
    upstream = UpstreamHttpClient()

    async def run():
        try:
            for _ in range(3):
                r = await upstream.get(f"{base}/x", context="test")
                assert r.status_code == 200
        finally:
            await upstream.aclose()

    try:
        asyncio.run(run())
    finally:
        server.shutdown()

    assert len(requests) == 3, f"expected 3 requests to reach the server, saw {len(requests)}"
    assert len(connections) == 1, (
        f"expected all 3 requests on one pooled connection, saw {len(connections)} "
        "— has a per-call httpx.AsyncClient() come back?"
    )


def test_same_loop_reuses_the_client_and_a_new_loop_does_not():
    """The loop guard: one client per loop, rebuilt when the loop changes.

    Reusing a client across loops would hand out connections belonging to a dead
    loop — which is exactly what the test suite does, one loop per TestClient.
    """
    upstream = UpstreamHttpClient()

    async def grab():
        return upstream._shared()

    first = asyncio.run(grab())
    second_same_loop: list[object] = []

    async def twice():
        second_same_loop.append(upstream._shared())
        second_same_loop.append(upstream._shared())

    asyncio.run(twice())

    assert second_same_loop[0] is second_same_loop[1], "same loop must reuse one client"
    assert first is not second_same_loop[0], "a new loop must get a fresh client"

    asyncio.run(upstream.aclose())


def test_aclose_is_idempotent_and_survives_a_never_used_client():
    """Shutdown runs on paths where no upstream call ever happened."""
    upstream = UpstreamHttpClient()
    asyncio.run(upstream.aclose())
    asyncio.run(upstream.aclose())
