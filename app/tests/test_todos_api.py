"""Tests for todo board CRUD and agent bridge context."""

from unittest.mock import AsyncMock, patch

import pytest
from sqlmodel import Session, select

from team_schema import ROSTER_ACCENT_PALETTE
from todos.models import PlannerAgentProfile, TodoBoard, TodoItem, TodoItemEvent
from todos.services.agent_bridge import build_item_context
from todos.seeds import seed_todo_domain_if_empty

pytestmark = pytest.mark.contract


def test_seed_creates_board_profiles_and_action_set(client, test_engine):
    c, _mock_cls, _mock_inst = client
    with Session(test_engine) as session:
        seed_todo_domain_if_empty(session)

    with Session(test_engine) as session:
        boards = session.exec(select(TodoBoard)).all()
        profiles = session.exec(select(PlannerAgentProfile)).all()
        assert len(boards) >= 1
        assert len(profiles) >= 16

    r = c.get("/api/v1/todos/boards")
    assert r.status_code == 200
    data = r.json()
    assert len(data["boards"]) >= 1


def test_board_scoped_by_project_id(client, test_engine):
    c, _mock_cls, _mock_inst = client
    pr = c.post("/api/v1/projects", json={"name": "Todo scope A"})
    assert pr.status_code == 201
    project_a = pr.json()["id"]
    pr2 = c.post("/api/v1/projects", json={"name": "Todo scope B"})
    project_b = pr2.json()["id"]

    r1 = c.post(f"/api/v1/todos/boards?project_id={project_a}", json={"name": "Board A"})
    r2 = c.post(f"/api/v1/todos/boards?project_id={project_b}", json={"name": "Board B"})
    assert r1.status_code == 201
    assert r2.status_code == 201

    list_a = c.get(f"/api/v1/todos/boards?project_id={project_a}")
    list_b = c.get(f"/api/v1/todos/boards?project_id={project_b}")
    assert list_a.status_code == 200
    assert list_b.status_code == 200
    names_a = {b["name"] for b in list_a.json()["boards"]}
    names_b = {b["name"] for b in list_b.json()["boards"]}
    assert names_a == {"Board A"}
    assert names_b == {"Board B"}


def test_create_category_without_color_defaults(client, test_engine):
    c, _mock_cls, _mock_inst = client
    board_id = c.post("/api/v1/todos/boards", json={"name": "Color board"}).json()["id"]
    with patch("todos.services.board_service.random_palette_color", return_value="#0ea5e9"):
        r = c.post(
            f"/api/v1/todos/boards/{board_id}/categories",
            json={"name": "Work"},
        )
    assert r.status_code == 201
    assert r.json()["color"] == "#0ea5e9"
    assert r.json()["color"] in ROSTER_ACCENT_PALETTE


def test_board_crud_and_item_status(client, test_engine):
    c, _mock_cls, _mock_inst = client

    r = c.post("/api/v1/todos/boards", json={"name": "Test Board", "default_model": "gemma4:31b-cloud"})
    assert r.status_code == 201
    board_id = r.json()["id"]

    r2 = c.get(f"/api/v1/todos/boards/{board_id}")
    assert r2.status_code == 200
    detail = r2.json()
    assert detail["name"] == "Test Board"
    assert "categories" in detail
    assert "items" in detail

    r3 = c.post(
        f"/api/v1/todos/boards/{board_id}/items",
        json={"title": "Write tests", "status": "plan", "description": "pytest + vitest"},
    )
    assert r3.status_code == 201
    item_id = r3.json()["id"]
    assert r3.json()["status"] == "plan"

    r4 = c.patch(f"/api/v1/todos/items/{item_id}", json={"status": "backlog"})
    assert r4.status_code == 200
    assert r4.json()["status"] == "backlog"

    r5 = c.patch(f"/api/v1/todos/items/{item_id}", json={"status": "invalid"})
    assert r5.status_code == 400


def test_build_item_context(client, test_engine):
    c, _mock_cls, _mock_inst = client
    r = c.post("/api/v1/todos/boards", json={"name": "Ctx Board"})
    board_id = r.json()["id"]
    r2 = c.post(
        f"/api/v1/todos/boards/{board_id}/items",
        json={"title": "Context item", "status": "plan"},
    )
    item_id = r2.json()["id"]

    with Session(test_engine) as session:
        item = session.get(TodoItem, item_id)
        ctx = build_item_context(session, item)
        assert ctx["item"]["title"] == "Context item"
        assert ctx["board"]["id"] == board_id


def test_planner_profiles_list(client, test_engine):
    c, _mock_cls, _mock_inst = client
    with Session(test_engine) as session:
        seed_todo_domain_if_empty(session)
    r = c.get("/api/v1/todos/planner-profiles")
    assert r.status_code == 200
    profiles = r.json()["profiles"]
    slugs = {p["slug"] for p in profiles}
    assert "sprint-planner" in slugs
    assert "code-task-planner" in slugs
    assert "nutrition-coach" in slugs
    assert "day-prioritizer" in slugs
    assert "habit-coach" in slugs
    assert "progress-reviewer" in slugs


def test_agent_step_endpoint_mocked(client, test_engine):
    c, _mock_cls, _mock_inst = client
    with Session(test_engine) as session:
        seed_todo_domain_if_empty(session)

    boards = c.get("/api/v1/todos/boards").json()["boards"]
    board_id = boards[0]["id"]
    r_item = c.post(
        f"/api/v1/todos/boards/{board_id}/items",
        json={"title": "Agent task", "status": "plan"},
    )
    item_id = r_item.json()["id"]

    from action_orchestrator.schemas import PlannedAction
    from chat_usage import LlmUsageOut

    mock_actions = [
        PlannedAction(
            action_id="suggest_next_steps",
            name="Suggest",
            parameters={"item_id": item_id, "guidance": "Start with a outline."},
        )
    ]

    with patch(
        "todos.services.agent_bridge.decide_actions",
        new=AsyncMock(return_value=(mock_actions, "Think step", LlmUsageOut())),
    ):
        r = c.post(
            f"/api/v1/todos/items/{item_id}/agent/step",
            json={"goal": "What next?", "model": "gemma4:31b-cloud"},
        )
    assert r.status_code == 200
    body = r.json()
    assert body["thought"] == "Think step"
    assert len(body["actions"]) == 1
    assert body["actions"][0]["action_id"] == "suggest_next_steps"


def test_board_templates_list(client):
    c, _mock_cls, _mock_inst = client
    r = c.get("/api/v1/todos/board-templates")
    assert r.status_code == 200
    templates = r.json()["templates"]
    slugs = {t["slug"] for t in templates}
    assert "life-weekly" in slugs
    assert "meal-plan" in slugs


def test_create_board_with_template(client, test_engine):
    c, _mock_cls, _mock_inst = client
    with Session(test_engine) as session:
        seed_todo_domain_if_empty(session)

    pr = c.post("/api/v1/projects", json={"name": "Template project"})
    project_id = pr.json()["id"]
    r = c.post(
        f"/api/v1/todos/boards?project_id={project_id}",
        json={"name": "My trip", "template_slug": "travel-trip"},
    )
    assert r.status_code == 201
    board_id = r.json()["id"]

    detail = c.get(f"/api/v1/todos/boards/{board_id}").json()
    cat_names = {c["name"] for c in detail["categories"]}
    assert cat_names == {"Research", "Bookings", "Packing"}


def test_project_planning_context_and_board_visit(client, test_engine):
    c, _mock_cls, _mock_inst = client
    with Session(test_engine) as session:
        seed_todo_domain_if_empty(session)

    pr = c.post("/api/v1/projects", json={"name": "Planning ctx"})
    project_id = pr.json()["id"]
    br = c.post(
        f"/api/v1/todos/boards?project_id={project_id}",
        json={"name": "Ctx board", "template_slug": "meal-plan"},
    )
    board_id = br.json()["id"]

    ctx0 = c.get(f"/api/v1/projects/{project_id}/planning-context")
    assert ctx0.status_code == 200
    assert ctx0.json()["last_todo_board_id"] is None

    c.get(f"/api/v1/todos/boards/{board_id}")
    ctx1 = c.get(f"/api/v1/projects/{project_id}/planning-context")
    assert ctx1.json()["last_todo_board_id"] == board_id


def test_server_apply_actions_and_events(client, test_engine):
    c, _mock_cls, _mock_inst = client
    with Session(test_engine) as session:
        seed_todo_domain_if_empty(session)

    boards = c.get("/api/v1/todos/boards").json()["boards"]
    board_id = boards[0]["id"]
    item = c.post(
        f"/api/v1/todos/boards/{board_id}/items",
        json={"title": "Apply me", "status": "plan"},
    ).json()
    item_id = item["id"]

    apply = c.post(
        f"/api/v1/todos/items/{item_id}/agent/apply",
        json={
            "actions": [
                {
                    "action_id": "break_down_task",
                    "name": "Break down",
                    "parameters": {
                        "item_id": item_id,
                        "steps": [{"step": "Step A", "done": False}],
                    },
                }
            ]
        },
    )
    assert apply.status_code == 200
    body = apply.json()
    assert body["item"]["plan"][0]["step"] == "Step A"

    events = c.get(f"/api/v1/todos/items/{item_id}/events")
    assert events.status_code == 200
    types = {e["event_type"] for e in events.json()["events"]}
    assert "actions_applied" in types


def test_planning_form_flow(client, test_engine):
    c, _mock_cls, _mock_inst = client
    with Session(test_engine) as session:
        seed_todo_domain_if_empty(session)

    boards = c.get("/api/v1/todos/boards").json()["boards"]
    board_id = boards[0]["id"]
    item_id = c.post(
        f"/api/v1/todos/boards/{board_id}/items",
        json={"title": "Form task", "status": "plan"},
    ).json()["id"]

    apply = c.post(
        f"/api/v1/todos/items/{item_id}/agent/apply",
        json={
            "actions": [
                {
                    "action_id": "present_planning_form",
                    "name": "Form",
                    "parameters": {
                        "item_id": item_id,
                        "form": {
                            "title": "Preferences",
                            "fields": [
                                {
                                    "id": "diet",
                                    "label": "Diet",
                                    "kind": "text",
                                    "required": True,
                                }
                            ],
                        },
                    },
                }
            ]
        },
    )
    assert apply.status_code == 200

    submit = c.post(
        f"/api/v1/todos/items/{item_id}/planning-form/submit",
        json={"form_index": 0, "answers": {"diet": "vegetarian"}},
    )
    assert submit.status_code == 200
    meta = submit.json()["metadata"]
    assert meta["planning_forms"][0]["status"] == "submitted"


def test_grocery_groups_break_down(client, test_engine):
    c, _mock_cls, _mock_inst = client
    with Session(test_engine) as session:
        seed_todo_domain_if_empty(session)

    board_id = c.get("/api/v1/todos/boards").json()["boards"][0]["id"]
    item_id = c.post(
        f"/api/v1/todos/boards/{board_id}/items",
        json={"title": "Groceries", "status": "plan"},
    ).json()["id"]

    apply = c.post(
        f"/api/v1/todos/items/{item_id}/agent/apply",
        json={
            "actions": [
                {
                    "action_id": "break_down_task",
                    "name": "Grocery list",
                    "parameters": {
                        "item_id": item_id,
                        "grocery_groups": [
                            {"category": "Produce", "items": ["apples"]},
                            {"category": "Dairy", "items": ["milk"]},
                        ],
                    },
                }
            ]
        },
    )
    assert apply.status_code == 200
    plan = apply.json()["item"]["plan"]
    assert plan[0]["category"] == "Produce"
    assert plan[0]["items"] == ["apples"]
    assert apply.json()["item"]["metadata"]["plan_kind"] == "grocery_list"


def test_trigger_webhook_apply(client, test_engine):
    c, _mock_cls, _mock_inst = client
    with Session(test_engine) as session:
        seed_todo_domain_if_empty(session)

    board_id = c.get("/api/v1/todos/boards").json()["boards"][0]["id"]
    item_id = c.post(
        f"/api/v1/todos/boards/{board_id}/items",
        json={"title": "Hook task", "status": "plan"},
    ).json()["id"]

    with patch(
        "todos.services.action_apply.execute_trigger_webhook",
        return_value={"status_code": 200, "ok": True, "body_preview": "ok"},
    ):
        apply = c.post(
            f"/api/v1/todos/items/{item_id}/agent/apply",
            json={
                "actions": [
                    {
                        "action_id": "trigger_webhook",
                        "name": "Webhook",
                        "parameters": {
                            "item_id": item_id,
                            "webhook_url": "https://example.com/hook",
                            "payload": {"event": "plan_ready"},
                        },
                    }
                ]
            },
        )
    assert apply.status_code == 200
    assert "Webhook 200" in apply.json()["applied"][0]


def test_merge_workspace_documents(test_engine):
    with Session(test_engine) as session:
        seed_todo_domain_if_empty(session)
        board = session.exec(select(TodoBoard)).first()
        board.project_id = 99
        session.add(board)
        session.commit()
        from todos.schemas import ItemCreate
        from todos.services.board_service import create_item

        item = create_item(
            session,
            board.id,
            ItemCreate(title="Meal plan task", status="plan"),
        )
        item_id = item.id

    from todos.services.agent_bridge import merge_workspace_documents

    with patch(
        "todos.services.agent_bridge.read_workspace_file_for_llm",
        return_value={
            "path": "documents/meal.txt",
            "content": "Weekly meal plan",
            "content_kind": "text",
        },
    ):
        ctx: dict = {"document_paths": ["documents/meal.txt"]}
        with Session(test_engine) as session:
            item = session.get(TodoItem, item_id)
            merge_workspace_documents(session, item, ctx)

    assert len(ctx["workspace_documents"]) == 1
    assert "Weekly meal plan" in ctx["workspace_documents"][0]["excerpt"]


def test_planning_context_onboarding(client):
    c, _mock_cls, _mock_inst = client
    pr = c.post("/api/v1/projects", json={"name": "Onboard"})
    pid = pr.json()["id"]
    ctx = c.get(f"/api/v1/projects/{pid}/planning-context").json()
    assert ctx["onboarding_dismissed"] is False

    patch = c.patch(
        f"/api/v1/projects/{pid}/planning-context",
        json={"onboarding_dismissed": True},
    )
    assert patch.status_code == 200
    assert patch.json()["onboarding_dismissed"] is True
