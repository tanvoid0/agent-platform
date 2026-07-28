#!/usr/bin/env python3
"""Example: create a model-ops project, start a build job, poll until done."""

from __future__ import annotations

import os
import sys
import time

import httpx

BASE = os.environ.get("AGENT_PLATFORM_BASE_URL", "http://127.0.0.1:18410").rstrip("/")
TOKEN = os.environ.get("AGENT_PLATFORM_MASTER_KEY") or os.environ.get("AGENT_PLATFORM_TOKEN")
if not TOKEN:
    print("Set AGENT_PLATFORM_MASTER_KEY or AGENT_PLATFORM_TOKEN", file=sys.stderr)
    sys.exit(1)

H = {"Authorization": f"Bearer {TOKEN}"}
PROJECT = os.environ.get("MODEL_OPS_PROJECT", "demo-coach")


def main() -> int:
    with httpx.Client(base_url=BASE, headers=H, timeout=120) as c:
        c.post(
            "/api/v1/model-ops/projects",
            json={"name": PROJECT, "ollama_tag": PROJECT, "description": "demo"},
        )
        job = c.post(
            "/api/v1/model-ops/jobs",
            json={
                "project": PROJECT,
                "stages": ["prepare"],
                "offline_eval": True,
            },
        ).json()
        print("started job", job["id"], job["poll_url"])
        while job["status"] in ("pending", "running"):
            time.sleep(1)
            job = c.get(f"/api/v1/model-ops/jobs/{job['id']}").json()
        print("finished:", job["status"], job.get("error_message"))
        return 0 if job["status"] == "succeeded" else 1


if __name__ == "__main__":
    raise SystemExit(main())
