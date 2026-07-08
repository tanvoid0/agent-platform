"""Team template CRUD and processes with team_template_id."""

import json
from unittest.mock import patch

import pytest
from sqlmodel import Session, select

from models import Process, TeamTemplate

pytestmark = pytest.mark.contract


def _sample_roster():
    return {
        "roles": [
            {
                "id": "a",
                "name": "Alpha",
                "description": "Does A",
                "modality": "text",
                "parent_id": None,
            },
            {
                "id": "b",
                "name": "Beta",
                "description": "Does B",
                "modality": "text",
                "parent_id": "a",
                "accent_color": "#ff00aa",
            },
        ]
    }


def test_teams_list_includes_seed(client, test_engine):
    c, _mock_cls, _mock_inst = client
    r = c.get("/teams/")
    assert r.status_code == 200
    data = r.json()
    assert "teams" in data
    assert len(data["teams"]) >= 3
    names = {t["name"] for t in data["teams"]}
    assert "Autonomous Product Engineering Team" in names
    assert "CV Reviewer Agency" in names
    seed = next(
        t for t in data["teams"] if t["name"] == "Autonomous Product Engineering Team"
    )
    assert seed.get("role_count") == 5
    cv = next(t for t in data["teams"] if t["name"] == "CV Reviewer Agency")
    assert cv.get("role_count") == 5
    assert cv.get("category") == "Career"


def test_teams_create_without_color_or_accent_defaults(client, test_engine):
    c, _mock_cls, _mock_inst = client
    with patch(
        "team_schema.secrets.choice",
        side_effect=["#9333ea", "#16a34a"],
    ):
        r = c.post(
            "/teams/",
            json={
                "name": "Defaults Team",
                "roster": {
                    "roles": [
                        {"id": "lead", "name": "Lead", "parent_id": None},
                        {"id": "sub", "name": "Sub", "parent_id": "lead"},
                    ]
                },
            },
        )
    assert r.status_code == 201
    body = r.json()
    assert body["color"] == "#9333ea"
    roles = {role["id"]: role for role in body["roster"]["roles"]}
    assert roles["lead"]["accent_color"] == "#9333ea"
    assert roles["sub"]["accent_color"] == "#16a34a"


def test_teams_create_randomizes_team_color(client, test_engine):
    c, _mock_cls, _mock_inst = client
    roster = {
        "roles": [{"id": "lead", "name": "Lead", "parent_id": None}],
    }
    with patch("team_schema.secrets.choice", side_effect=["#2563eb", "#ca8a04"]):
        first = c.post("/teams/", json={"name": "Team A", "roster": roster}).json()
    with patch("team_schema.secrets.choice", side_effect=["#dc2626", "#dc2626"]):
        second = c.post("/teams/", json={"name": "Team B", "roster": roster}).json()
    assert first["color"] == "#2563eb"
    assert second["color"] == "#dc2626"
    assert first["color"] != second["color"]


def test_teams_crud(client, test_engine):
    c, _mock_cls, _mock_inst = client
    r = c.post(
        "/teams/",
        json={
            "name": "API Team",
            "description": "From test",
            "color": "#112233",
            "category": "QA",
            "roster": _sample_roster(),
        },
    )
    assert r.status_code == 201
    body = r.json()
    tid = body["id"]
    assert body["name"] == "API Team"
    assert body.get("category") == "QA"
    assert len(body["roster"]["roles"]) == 2
    assert body.get("role_count") == 2

    r2 = c.get(f"/teams/{tid}")
    assert r2.status_code == 200
    body_get = r2.json()
    assert body_get["roster"]["roles"][1]["parent_id"] == "a"
    assert body_get["roster"]["roles"][1]["accent_color"] == "#ff00aa"
    assert body_get.get("role_count") == 2
    assert body_get.get("category") == "QA"

    r3 = c.patch(f"/teams/{tid}", json={"name": "API Team Renamed", "category": None})
    assert r3.status_code == 200
    assert r3.json()["name"] == "API Team Renamed"
    assert r3.json().get("category") is None

    r4 = c.delete(f"/teams/{tid}")
    assert r4.status_code == 200
    r5 = c.get(f"/teams/{tid}")
    assert r5.status_code == 404


def test_teams_post_invalid_roster(client, test_engine):
    c, _mock_cls, _mock_inst = client
    r = c.post(
        "/teams/",
        json={
            "name": "Bad",
            "roster": {"roles": [{"id": "x", "name": "X", "parent_id": "missing"}]},
        },
    )
    assert r.status_code == 422


def test_teams_post_non_text_modality_rejected(client, test_engine):
    c, _mock_cls, _mock_inst = client
    r = c.post(
        "/teams/",
        json={
            "name": "Bad modality",
            "roster": {
                "roles": [
                    {
                        "id": "a",
                        "name": "A",
                        "description": "",
                        "modality": "image",
                        "parent_id": None,
                    }
                ]
            },
        },
    )
    assert r.status_code == 422


def test_post_runs_unknown_team_template(client, test_engine):
    c, _mock_cls, _mock_inst = client
    r = c.post("/processes", json={"goal": "g", "team_template_id": 999_999})
    assert r.status_code == 404


def test_post_process_requires_team_template_id(client, test_engine):
    c, _mock_cls, _mock_inst = client
    r = c.post("/processes", json={"goal": "g"})
    assert r.status_code == 422


def test_post_runs_with_team_template_passes_context_and_snapshot(client, test_engine):
    c, _mock_cls, mock_inst = client
    r = c.get("/teams/")
    tid = r.json()["teams"][0]["id"]

    r2 = c.post("/processes", json={"goal": "Ship feature X", "team_template_id": tid})
    assert r2.status_code == 200
    process_id = r2.json()["process_id"]

    mock_inst.plan.assert_called_once()
    args, _kwargs = mock_inst.plan.call_args
    assert args[0] == "Ship feature X"
    assert args[1] is not None
    assert "Autonomous Product Engineering Team" in args[1] or "Team template:" in args[1]

    r3 = c.get(f"/processes/{process_id}")
    assert r3.status_code == 200
    payload = r3.json()
    assert payload["process"]["team_template_id"] == tid
    snap = payload["process"].get("team_snapshot_json")
    assert snap
    parsed = json.loads(snap)
    assert parsed["team_template_id"] == tid
    assert "roster" in parsed


def test_delete_team_nullifies_run_fk(client, test_engine):
    c, _mock_cls, _mock_inst = client
    r = c.post(
        "/teams/",
        json={"name": "Tmp", "roster": _sample_roster()},
    )
    tid = r.json()["id"]
    r2 = c.post("/processes", json={"goal": "g", "team_template_id": tid})
    process_id = r2.json()["process_id"]

    with Session(test_engine) as session:
        proc = session.get(Process, process_id)
        assert proc.team_template_id == tid

    r3 = c.delete(f"/teams/{tid}")
    assert r3.status_code == 200

    with Session(test_engine) as session:
        proc = session.get(Process, process_id)
        session.refresh(proc)
        assert proc.team_template_id is None
        assert proc.team_snapshot_json is not None
        rows = session.exec(select(TeamTemplate).where(TeamTemplate.id == tid)).all()
        assert rows == []
