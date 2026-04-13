"""Project CRUD and processes with project_id."""

from sqlmodel import Session, select

from models import Process, Project


def test_projects_crud(client, test_engine):
    c, _mock_cls, _mock_inst = client
    r = c.post(
        "/projects/",
        json={
            "name": "Alpha",
            "description": "First project",
            "color": "#aabbcc",
        },
    )
    assert r.status_code == 201
    body = r.json()
    pid = body["id"]
    assert body["name"] == "Alpha"
    assert body.get("description") == "First project"

    r_list = c.get("/projects/")
    assert r_list.status_code == 200
    assert any(p["id"] == pid for p in r_list.json()["projects"])

    r2 = c.get(f"/projects/{pid}")
    assert r2.status_code == 200
    assert r2.json()["name"] == "Alpha"

    r3 = c.patch(f"/projects/{pid}", json={"name": "Alpha Renamed"})
    assert r3.status_code == 200
    assert r3.json()["name"] == "Alpha Renamed"

    r4 = c.delete(f"/projects/{pid}")
    assert r4.status_code == 200
    r5 = c.get(f"/projects/{pid}")
    assert r5.status_code == 404


def test_post_process_unknown_project(client, test_engine):
    c, _mock_cls, _mock_inst = client
    tr = c.get("/teams/")
    tid = tr.json()["teams"][0]["id"]
    r = c.post(
        "/processes",
        json={"goal": "g", "team_template_id": tid, "project_id": 999_999},
    )
    assert r.status_code == 404


def test_post_process_with_project_and_list_filter(client, test_engine):
    c, _mock_cls, _mock_inst = client
    tr = c.get("/teams/")
    tid = tr.json()["teams"][0]["id"]
    pr = c.post("/projects/", json={"name": "P1"})
    assert pr.status_code == 201
    project_id = pr.json()["id"]

    r2 = c.post(
        "/processes",
        json={"goal": "g", "team_template_id": tid, "project_id": project_id},
    )
    assert r2.status_code == 200
    process_id = r2.json()["process_id"]

    r3 = c.get(f"/processes/{process_id}")
    assert r3.status_code == 200
    assert r3.json()["process"]["project_id"] == project_id

    r4 = c.get("/processes", params={"project_id": project_id, "limit": 50})
    assert r4.status_code == 200
    ids = {p["id"] for p in r4.json()["processes"]}
    assert process_id in ids

    r5 = c.get("/processes", params={"project_id": project_id + 9999, "limit": 50})
    assert r5.status_code == 200
    assert r5.json()["processes"] == []


def test_list_processes_unassigned_only(client, test_engine):
    c, _mock_cls, _mock_inst = client
    tr = c.get("/teams/")
    tid = tr.json()["teams"][0]["id"]
    pr = c.post("/projects/", json={"name": "U1"})
    project_id = pr.json()["id"]
    c.post("/processes", json={"goal": "with proj", "team_template_id": tid, "project_id": project_id})
    c.post("/processes", json={"goal": "no proj", "team_template_id": tid})

    r = c.get("/processes", params={"unassigned_only": "true", "limit": 50})
    assert r.status_code == 200
    goals = [p["goal"] for p in r.json()["processes"]]
    assert "no proj" in goals
    assert "with proj" not in goals


def test_delete_project_nullifies_process_fk(client, test_engine):
    c, _mock_cls, _mock_inst = client
    tr = c.get("/teams/")
    tid = tr.json()["teams"][0]["id"]
    pr = c.post("/projects/", json={"name": "Tmp"})
    project_id = pr.json()["id"]
    r2 = c.post(
        "/processes",
        json={"goal": "g", "team_template_id": tid, "project_id": project_id},
    )
    process_id = r2.json()["process_id"]

    with Session(test_engine) as session:
        proc = session.get(Process, process_id)
        assert proc.project_id == project_id

    r3 = c.delete(f"/projects/{project_id}")
    assert r3.status_code == 200

    with Session(test_engine) as session:
        proc = session.get(Process, process_id)
        session.refresh(proc)
        assert proc.project_id is None
        rows = session.exec(select(Project).where(Project.id == project_id)).all()
        assert rows == []


def test_project_workspace_roundtrip(client, test_engine, tmp_path, monkeypatch):
    """List / write / read / delete sandbox files under AGENT_PLATFORM_WORKSPACE_ROOT."""
    monkeypatch.setenv("AGENT_PLATFORM_WORKSPACE_ROOT", str(tmp_path / "ws"))
    c, _mock_cls, _mock_inst = client
    pr = c.post("/projects/", json={"name": "WS"})
    assert pr.status_code == 201
    pid = pr.json()["id"]

    r0 = c.get(f"/projects/{pid}/workspace/list")
    assert r0.status_code == 200
    assert r0.json()["entries"] == []

    w = c.put(
        f"/projects/{pid}/workspace/file",
        json={"path": "notes/hello.txt", "content": "hello world"},
    )
    assert w.status_code == 200

    r1 = c.get(f"/projects/{pid}/workspace/list", params={"path": "notes"})
    assert r1.status_code == 200
    names = [e["name"] for e in r1.json()["entries"]]
    assert "hello.txt" in names

    r2 = c.get(f"/projects/{pid}/workspace/file", params={"path": "notes/hello.txt"})
    assert r2.status_code == 200
    assert r2.json()["content"] == "hello world"

    d = c.delete(f"/projects/{pid}/workspace/file", params={"path": "notes/hello.txt"})
    assert d.status_code == 200

    r3 = c.get(f"/projects/{pid}/workspace/file", params={"path": "notes/hello.txt"})
    assert r3.status_code == 404


def test_project_workspace_info_path(client, test_engine, tmp_path, monkeypatch):
    monkeypatch.setenv("AGENT_PLATFORM_WORKSPACE_ROOT", str(tmp_path / "ws"))
    c, _mock_cls, _mock_inst = client
    pr = c.post("/projects/", json={"name": "Info"})
    pid = pr.json()["id"]
    c.put(f"/projects/{pid}/workspace/file", json={"path": "a/b.txt", "content": "x"})

    r0 = c.get(f"/projects/{pid}/workspace/info", params={"path": "a"})
    assert r0.status_code == 200
    body = r0.json()
    assert "absolute_path" in body
    assert body["relative_prefix"] == "a"
    assert "a" in body["absolute_path"].replace("\\", "/")


def test_workspace_info_validates_process_folder(client, test_engine, tmp_path, monkeypatch):
    monkeypatch.setenv("AGENT_PLATFORM_WORKSPACE_ROOT", str(tmp_path / "ws"))
    c, _mock_cls, _mock_inst = client
    tr = c.get("/teams/")
    tid = tr.json()["teams"][0]["id"]
    pr = c.post("/projects/", json={"name": "WP"})
    project_id = pr.json()["id"]
    r2 = c.post(
        "/processes",
        json={"goal": "goal", "team_template_id": tid, "project_id": project_id},
    )
    process_id = r2.json()["process_id"]

    r_ok = c.get(
        f"/projects/{project_id}/workspace/info",
        params={"path": f"processes/{process_id}"},
    )
    assert r_ok.status_code == 200
    assert "absolute_path" in r_ok.json()

    r_bad = c.get(f"/projects/{project_id}/workspace/info", params={"path": "processes/999999999"})
    assert r_bad.status_code == 404
