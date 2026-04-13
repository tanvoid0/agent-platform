"""One-off: insert default team templates if teamtemplate is empty (after partial migration).

Prefer normal startup: `create_db_and_tables()` runs Alembic and `_ensure_team_template_schema_and_seed()`.
Use this script only for manual repair; DB path matches `AGENT_PLATFORM_DB_PATH` (default data/agent_platform.db).
"""
import json
import os
import sqlite3
import sys
from pathlib import Path

_root = Path(__file__).resolve().parent.parent
_app = _root / "app"
if str(_app) not in sys.path:
    sys.path.insert(0, str(_app))

from dotenv import load_dotenv

load_dotenv(_root / ".env")

from default_team_templates import SEED_TEAM_TEMPLATES

_db_raw = (os.getenv("AGENT_PLATFORM_DB_PATH") or "data/agent_platform.db").strip()
_db = Path(_db_raw)
if not _db.is_file():
    print("no db at", _db, file=sys.stderr)
    sys.exit(1)

c = sqlite3.connect(_db)
existing = {row[0] for row in c.execute("select name from teamtemplate").fetchall()}
inserted = 0
for tmpl in SEED_TEAM_TEMPLATES:
    if tmpl["name"] in existing:
        continue
    c.execute(
        """
        insert into teamtemplate (name, description, color, category, roster_json, created_at, updated_at)
        values (?, ?, ?, ?, ?, datetime('now'), datetime('now'))
        """,
        (
            tmpl["name"],
            tmpl["description"],
            tmpl["color"],
            tmpl.get("category"),
            json.dumps(tmpl["roster"]),
        ),
    )
    inserted += 1
c.commit()
if inserted:
    print("inserted", inserted, "missing seed teamtemplate row(s)")
else:
    print("all", len(SEED_TEAM_TEMPLATES), "seed templates already present")
