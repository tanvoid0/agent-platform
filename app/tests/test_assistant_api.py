"""Tests for Personal Assistant API."""

from datetime import datetime, timedelta
from unittest.mock import AsyncMock, patch

from sqlmodel import Session, select

from chat_usage import LlmUsageOut
from assistant.models import AssistantChatThread, AssistantDomainProfile, AssistantReview
from models import Project
from todos.models import TodoBoard, TodoItem
from todos.seeds import seed_todo_domain_if_empty


def test_assistant_dashboard_auto_creates_board(client, test_engine):
    c, _mock_cls, _mock_inst = client
    with Session(test_engine) as session:
        seed_todo_domain_if_empty(session)

    pr = c.post("/api/v1/projects", json={"name": "Assistant project"})
    assert pr.status_code == 201
    project_id = pr.json()["id"]

    r = c.get(f"/api/v1/assistant/dashboard?project_id={project_id}&horizon=day")
    assert r.status_code == 200
    data = r.json()
    assert data["project_id"] == project_id
    assert data["board_id"] > 0
    assert data["horizon"] == "day"
    assert "stats" in data
    assert "categories" in data
    assert len(data["categories"]) >= 1

    with Session(test_engine) as session:
        proj = session.get(Project, project_id)
        assert proj.assistant_board_id == data["board_id"]


def test_assistant_create_item_with_schedule(client, test_engine):
    c, _mock_cls, _mock_inst = client
    with Session(test_engine) as session:
        seed_todo_domain_if_empty(session)

    pr = c.post("/api/v1/projects", json={"name": "Schedule project"})
    project_id = pr.json()["id"]
    dash = c.get(f"/api/v1/assistant/dashboard?project_id={project_id}").json()
    board_id = dash["board_id"]

    due = (datetime.utcnow() + timedelta(days=1)).isoformat()
    r = c.post(
        f"/api/v1/todos/boards/{board_id}/items",
        json={
            "title": "Morning run",
            "status": "backlog",
            "time_horizon": "week",
            "due_at": due,
            "item_kind": "task",
        },
    )
    assert r.status_code == 201
    item = r.json()
    assert item["title"] == "Morning run"
    assert item["time_horizon"] == "week"
    assert item["due_at"] is not None

    dash2 = c.get(f"/api/v1/assistant/dashboard?project_id={project_id}&horizon=week")
    assert dash2.status_code == 200
    titles = {i["title"] for i in dash2.json()["items"]}
    assert "Morning run" in titles


def test_assistant_chat_thread_persists(client, test_engine):
    c, _mock_cls, _mock_inst = client
    with Session(test_engine) as session:
        seed_todo_domain_if_empty(session)

    pr = c.post("/api/v1/projects", json={"name": "Chat project"})
    project_id = pr.json()["id"]

    r1 = c.get(f"/api/v1/assistant/chat/thread?project_id={project_id}")
    assert r1.status_code == 200
    assert r1.json()["messages"] == []
    assert r1.json()["thread_id"] > 0

    with patch(
        "assistant.services.assistant_chat.decide_actions",
        new_callable=AsyncMock,
    ) as mock_decide:
        from action_orchestrator.schemas import PlannedAction

        mock_decide.return_value = (
            [
                PlannedAction(
                    action_id="suggest_next_steps",
                    name="Suggest",
                    parameters={"item_id": 1, "guidance": "Try a 20 min walk."},
                    confidence=1.0,
                )
            ],
            "Here is a plan for your week.",
            LlmUsageOut(),
        )
        r2 = c.post(
            f"/api/v1/assistant/chat/send?project_id={project_id}",
            json={"message": "Help me plan workouts this week"},
        )
    assert r2.status_code == 200
    body = r2.json()
    assert body["profile_slug"]
    assert len(body["messages"]) >= 2

    r3 = c.get(f"/api/v1/assistant/chat/thread?project_id={project_id}")
    assert r3.status_code == 200
    assert len(r3.json()["messages"]) >= 2

    with Session(test_engine) as session:
        threads = session.exec(
            select(AssistantChatThread).where(AssistantChatThread.project_id == project_id)
        ).all()
        assert len(threads) >= 1


def test_assistant_chat_retry_truncates_and_regenerates(client, test_engine):
    c, _mock_cls, _mock_inst = client
    with Session(test_engine) as session:
        seed_todo_domain_if_empty(session)

    pr = c.post("/api/v1/projects", json={"name": "Retry chat project"})
    project_id = pr.json()["id"]

    with patch(
        "assistant.services.assistant_chat.decide_actions",
        new_callable=AsyncMock,
    ) as mock_decide:
        from action_orchestrator.schemas import PlannedAction

        mock_decide.side_effect = [
            (
                [
                    PlannedAction(
                        action_id="suggest_next_steps",
                        name="Suggest",
                        parameters={"guidance": "First reply."},
                        confidence=1.0,
                    )
                ],
                "First assistant turn.",
                LlmUsageOut(),
            ),
            (
                [
                    PlannedAction(
                        action_id="suggest_next_steps",
                        name="Suggest",
                        parameters={"guidance": "Second reply."},
                        confidence=1.0,
                    )
                ],
                "Second assistant turn.",
                LlmUsageOut(),
            ),
            (
                [
                    PlannedAction(
                        action_id="suggest_next_steps",
                        name="Suggest",
                        parameters={"guidance": "Retried reply."},
                        confidence=1.0,
                    )
                ],
                "Retried assistant turn.",
                LlmUsageOut(),
            ),
        ]
        first = c.post(
            f"/api/v1/assistant/chat/send?project_id={project_id}",
            json={"message": "Plan workouts"},
        )
        assert first.status_code == 200
        thread_id = first.json()["thread_id"]
        msgs = first.json()["messages"]
        user_idx = next(i for i, m in enumerate(msgs) if m["role"] == "user")

        second = c.post(
            f"/api/v1/assistant/chat/send?project_id={project_id}",
            json={"message": "Add a rest day", "thread_id": thread_id},
        )
        assert second.status_code == 200
        assert len(second.json()["messages"]) >= 4

        retry = c.post(
            f"/api/v1/assistant/chat/retry?project_id={project_id}",
            json={"thread_id": thread_id, "message_index": user_idx},
        )
    assert retry.status_code == 200
    retry_body = retry.json()
    assert len(retry_body["messages"]) == 2
    assert retry_body["messages"][0]["role"] == "user"
    assert retry_body["messages"][0]["content"] == "Plan workouts"
    assert retry_body["messages"][1]["role"] == "assistant"
    assert "Retried" in retry_body["content"]


def test_assistant_multiple_chat_threads(client, test_engine):
    c, _mock_cls, _mock_inst = client
    with Session(test_engine) as session:
        seed_todo_domain_if_empty(session)

    pr = c.post("/api/v1/projects", json={"name": "Multi chat project"})
    project_id = pr.json()["id"]

    r_new = c.post(f"/api/v1/assistant/chat/threads?project_id={project_id}", json={})
    assert r_new.status_code == 200
    thread_a = r_new.json()["thread_id"]

    with patch(
        "assistant.services.assistant_chat.decide_actions",
        new_callable=AsyncMock,
    ) as mock_decide:
        from action_orchestrator.schemas import PlannedAction

        mock_decide.return_value = (
            [PlannedAction(action_id="suggest_next_steps", name="Suggest", parameters={}, confidence=1.0)],
            "First thread reply.",
            LlmUsageOut(),
        )
        c.post(
            f"/api/v1/assistant/chat/send?project_id={project_id}",
            json={"message": "Plan week one", "thread_id": thread_a},
        )

    r_new2 = c.post(f"/api/v1/assistant/chat/threads?project_id={project_id}", json={})
    thread_b = r_new2.json()["thread_id"]
    assert thread_b != thread_a

    t_b = c.get(
        f"/api/v1/assistant/chat/thread?project_id={project_id}&thread_id={thread_b}"
    ).json()
    assert t_b["messages"] == []

    t_a = c.get(
        f"/api/v1/assistant/chat/thread?project_id={project_id}&thread_id={thread_a}"
    ).json()
    assert len(t_a["messages"]) >= 2

    list_r = c.get(f"/api/v1/assistant/chat/threads?project_id={project_id}")
    assert list_r.status_code == 200
    ids = {t["id"] for t in list_r.json()["threads"]}
    assert thread_a in ids and thread_b in ids


def test_assistant_complete_item(client, test_engine):
    c, _mock_cls, _mock_inst = client
    with Session(test_engine) as session:
        seed_todo_domain_if_empty(session)

    pr = c.post("/api/v1/projects", json={"name": "Complete project"})
    project_id = pr.json()["id"]
    board_id = c.get(f"/api/v1/assistant/dashboard?project_id={project_id}").json()["board_id"]

    item = c.post(
        f"/api/v1/todos/boards/{board_id}/items",
        json={"title": "Drink water", "item_kind": "habit"},
    ).json()

    r = c.post(
        f"/api/v1/assistant/items/{item['id']}/complete",
        json={"difficulty": "easy", "notes": "Done"},
    )
    assert r.status_code == 200
    done = r.json()
    assert done["status"] == "done"
    assert done["completion"].get("difficulty") == "easy"


def test_assistant_apply_create_item(client, test_engine):
    c, _mock_cls, _mock_inst = client
    with Session(test_engine) as session:
        seed_todo_domain_if_empty(session)

    pr = c.post("/api/v1/projects", json={"name": "Apply project"})
    project_id = pr.json()["id"]
    dash = c.get(f"/api/v1/assistant/dashboard?project_id={project_id}").json()
    board_id = dash["board_id"]
    category_id = dash["categories"][0]["id"]

    due = (datetime.utcnow() + timedelta(days=2)).isoformat()
    r = c.post(
        f"/api/v1/assistant/chat/apply?project_id={project_id}",
        json={
            "actions": [
                {
                    "action_id": "create_item",
                    "name": "Create item",
                    "parameters": {
                        "title": "Plan meals",
                        "time_horizon": "week",
                        "due_at": due,
                        "category_id": category_id,
                    },
                }
            ]
        },
    )
    assert r.status_code == 200
    assert r.json()["created_items"]
    assert r.json()["created_items"][0]["title"] == "Plan meals"

    with Session(test_engine) as session:
        items = session.exec(select(TodoItem).where(TodoItem.board_id == board_id)).all()
        assert any(i.title == "Plan meals" for i in items)


def test_personal_life_team_template_seeded(client, test_engine):
    c, _mock_cls, _mock_inst = client
    r = c.get("/api/v1/teams")
    assert r.status_code == 200
    names = {t["name"] for t in r.json()["teams"]}
    assert "Personal Life Assistant" in names


def test_domain_profile_patch_and_context(client, test_engine):
    c, _mock_cls, _mock_inst = client
    with Session(test_engine) as session:
        seed_todo_domain_if_empty(session)

    pr = c.post("/api/v1/projects", json={"name": "Profile project"})
    project_id = pr.json()["id"]

    r = c.patch(
        f"/api/v1/assistant/profile/fitness?project_id={project_id}",
        json={
            "profile": {
                "sex": "Female",
                "age": "30",
                "height_cm": "165",
                "weight_kg": "60",
                "fitness_goal": "General fitness",
                "experience_level": "Beginner",
                "equipment": ["Home dumbbells"],
            }
        },
    )
    assert r.status_code == 200
    assert r.json()["profile"]["fitness_goal"] == "General fitness"

    listed = c.get(f"/api/v1/assistant/profile?project_id={project_id}")
    assert listed.status_code == 200
    assert "fitness" in listed.json()["profiles"]

    forms = c.get(f"/api/v1/assistant/profile/forms?project_id={project_id}")
    assert forms.status_code == 200
    body = forms.json()
    assert body["project_id"] == project_id
    assert "fitness" in body["forms"]
    assert "nutrition" in body["forms"]
    assert "general" in body["forms"]
    assert body["forms"]["fitness"]["fields"]


def test_form_submit_saves_profile_and_continues(client, test_engine):
    c, _mock_cls, _mock_inst = client
    with Session(test_engine) as session:
        seed_todo_domain_if_empty(session)

    pr = c.post("/api/v1/projects", json={"name": "Form project"})
    project_id = pr.json()["id"]

    with patch(
        "assistant.services.assistant_chat.decide_actions",
        new_callable=AsyncMock,
    ) as mock_decide:
        from action_orchestrator.schemas import PlannedAction

        mock_decide.return_value = (
            [
                PlannedAction(
                    action_id="create_item",
                    name="Create item",
                    parameters={"title": "Easy run", "time_horizon": "week"},
                    confidence=1.0,
                )
            ],
            "Scheduled an easy run for you.",
            LlmUsageOut(),
        )
        r = c.post(
            f"/api/v1/assistant/chat/submit-form?project_id={project_id}",
            json={
                "domain": "fitness",
                "answers": {
                    "sex": "Male",
                    "age": "28",
                    "height_cm": "180",
                    "weight_kg": "75",
                    "fitness_goal": "Build muscle",
                    "experience_level": "Intermediate",
                    "equipment": ["Gym full access"],
                },
                "auto_continue": True,
            },
        )
    assert r.status_code == 200
    prof = c.get(f"/api/v1/assistant/profile/fitness?project_id={project_id}")
    assert prof.status_code == 200
    assert prof.json()["profile"]["fitness_goal"] == "Build muscle"


def test_form_submit_no_duplicate_user_messages(client, test_engine):
    c, _mock_cls, _mock_inst = client
    with Session(test_engine) as session:
        seed_todo_domain_if_empty(session)

    pr = c.post("/api/v1/projects", json={"name": "No dup project"})
    project_id = pr.json()["id"]

    with patch(
        "assistant.services.assistant_chat.decide_actions",
        new_callable=AsyncMock,
    ) as mock_decide:
        from action_orchestrator.schemas import PlannedAction

        mock_decide.return_value = (
            [
                PlannedAction(
                    action_id="create_item",
                    name="Create item",
                    parameters={"title": "Meal prep Sunday", "time_horizon": "week"},
                    confidence=1.0,
                )
            ],
            "I'll add a meal prep task.",
            LlmUsageOut(),
        )
        r = c.post(
            f"/api/v1/assistant/chat/submit-form?project_id={project_id}",
            json={
                "domain": "nutrition",
                "answers": {
                    "diet_style": "Omnivore",
                    "meals_per_day": "3",
                    "cooking_time_minutes": "Meal prep batches",
                },
                "auto_continue": True,
            },
        )
    assert r.status_code == 200
    messages = r.json()["messages"]
    user_contents = [m["content"] for m in messages if m["role"] == "user"]
    assert len(user_contents) == 1
    assert "Saved nutrition profile" in user_contents[0]
    assert r.json().get("pending_form") is None


def test_nutrition_profile_complete_without_allergies(client, test_engine):
    c, _mock_cls, _mock_inst = client
    with Session(test_engine) as session:
        seed_todo_domain_if_empty(session)

    pr = c.post("/api/v1/projects", json={"name": "Nutrition gap project"})
    project_id = pr.json()["id"]

    c.patch(
        f"/api/v1/assistant/profile/nutrition?project_id={project_id}",
        json={
            "profile": {
                "diet_style": "Omnivore",
                "meals_per_day": "3",
                "cooking_time_minutes": "Meal prep batches",
            }
        },
    )

    with (
        patch(
            "assistant.services.assistant_chat.decide_actions",
            new_callable=AsyncMock,
        ) as mock_decide,
        patch(
            "assistant.services.assistant_chat._chat_only",
            new_callable=AsyncMock,
            return_value=("Here is a balanced plan for the week.", LlmUsageOut()),
        ) as mock_chat,
    ):
        mock_decide.return_value = ([], None, LlmUsageOut())
        r = c.post(
            f"/api/v1/assistant/chat/send?project_id={project_id}",
            json={"message": "Plan my meals this week", "delegate_slug": "nutrition-coach"},
        )
    assert r.status_code == 200
    assert r.json().get("pending_form") is None
    mock_chat.assert_awaited_once()


def test_strip_redundant_profile_save_after_form():
    from assistant.services.assistant_chat import _strip_redundant_profile_saves
    from todos.schemas import PlannedActionOut

    msg = "Saved nutrition profile:\n- diet style: Omnivore\nPlease continue planning."
    planned = [
        PlannedActionOut(
            action_id="store_user_profile",
            name="Store",
            parameters={"domain": "nutrition", "data": {"diet_style": "Omnivore"}},
        ),
        PlannedActionOut(
            action_id="create_item",
            name="Create",
            parameters={"title": "Meal prep"},
        ),
    ]
    filtered = _strip_redundant_profile_saves(planned, msg)
    assert len(filtered) == 1
    assert filtered[0].action_id == "create_item"


def test_strip_profile_save_even_when_wrong_domain_in_message():
    """Form saved under general by mistake — still drop duplicate profile proposals."""
    from assistant.services.assistant_chat import _strip_redundant_profile_saves
    from todos.schemas import PlannedActionOut

    msg = "Saved general profile:\n- diet style: Omnivore\nPlease continue planning."
    planned = [
        PlannedActionOut(
            action_id="store_user_profile",
            name="Store",
            parameters={"domain": "nutrition", "data": {"diet_style": "Omnivore"}},
        ),
    ]
    assert _strip_redundant_profile_saves(planned, msg) == []


def test_form_submit_resolves_domain_from_pending_form(client, test_engine):
    c, _mock_cls, _mock_inst = client
    with Session(test_engine) as session:
        seed_todo_domain_if_empty(session)

    pr = c.post("/api/v1/projects", json={"name": "Domain resolve project"})
    project_id = pr.json()["id"]

    from assistant.domain_forms import get_domain_form_spec
    from assistant.models import AssistantChatThread
    from todos.schemas import PlannedActionOut

    spec = get_domain_form_spec("nutrition")
    pending = [
        PlannedActionOut(
            action_id="present_planning_form",
            name="Form",
            parameters={"domain": "nutrition", "form": spec},
        ).model_dump()
    ]
    with Session(test_engine) as session:
        thread = session.exec(
            select(AssistantChatThread).where(AssistantChatThread.project_id == project_id)
        ).first()
        if not thread:
            thread = AssistantChatThread(project_id=project_id, title="New chat")
            session.add(thread)
            session.commit()
            session.refresh(thread)
        thread.last_profile_slug = "nutrition-coach"
        thread.set_pending_actions(pending)
        session.add(thread)
        session.commit()
        thread_id = thread.id

    with (
        patch(
            "assistant.services.assistant_chat.decide_actions",
            new_callable=AsyncMock,
        ) as mock_decide,
        patch(
            "assistant.services.assistant_chat._chat_only",
            new_callable=AsyncMock,
            return_value=("Thanks, planning next.", LlmUsageOut()),
        ),
    ):
        mock_decide.return_value = ([], None, LlmUsageOut())
        r = c.post(
            f"/api/v1/assistant/chat/submit-form?project_id={project_id}",
            json={
                "domain": "general",
                "answers": {
                    "diet_style": "Omnivore",
                    "meals_per_day": "3",
                    "cooking_time_minutes": "Meal prep batches",
                },
                "thread_id": thread_id,
                "auto_continue": True,
            },
        )
    assert r.status_code == 200
    prof = c.get(f"/api/v1/assistant/profile/nutrition?project_id={project_id}")
    assert prof.status_code == 200
    assert prof.json()["profile"]["diet_style"] == "Omnivore"
    user_contents = [m["content"] for m in r.json()["messages"] if m["role"] == "user"]
    assert any("Saved nutrition profile" in c for c in user_contents)


def test_chat_fallback_when_no_planned_actions(client, test_engine):
    c, _mock_cls, _mock_inst = client
    with Session(test_engine) as session:
        seed_todo_domain_if_empty(session)

    pr = c.post("/api/v1/projects", json={"name": "Chat fallback project"})
    project_id = pr.json()["id"]

    c.patch(
        f"/api/v1/assistant/profile/nutrition?project_id={project_id}",
        json={
            "profile": {
                "diet_style": "Omnivore",
                "meals_per_day": "3",
                "cooking_time_minutes": "60",
            }
        },
    )

    with (
        patch(
            "assistant.services.assistant_chat.decide_actions",
            new_callable=AsyncMock,
        ) as mock_decide,
        patch(
            "assistant.services.assistant_chat._chat_only",
            new_callable=AsyncMock,
        ) as mock_chat,
    ):
        mock_decide.return_value = ([], None, LlmUsageOut())
        mock_chat.return_value = (
            "Here is a 7-day meal prep outline with Sunday batch cooking.",
            LlmUsageOut(),
        )
        r = c.post(
            f"/api/v1/assistant/chat/send?project_id={project_id}",
            json={
                "message": "What is your plan for meal prep?",
                "delegate_slug": "nutrition-coach",
            },
        )
    assert r.status_code == 200
    assert "7-day meal prep" in r.json()["content"]
    mock_chat.assert_awaited_once()


def test_decide_actions_receives_conversation_history(client, test_engine):
    c, _mock_cls, _mock_inst = client
    with Session(test_engine) as session:
        seed_todo_domain_if_empty(session)

    pr = c.post("/api/v1/projects", json={"name": "Conv history project"})
    project_id = pr.json()["id"]

    with patch(
        "assistant.services.assistant_chat._chat_only",
        new_callable=AsyncMock,
        return_value=("I'll help with meal prep.", LlmUsageOut()),
    ):
        c.post(
            f"/api/v1/assistant/chat/send?project_id={project_id}",
            json={"message": "Help me meal prep", "propose_actions": False},
        )

    with patch(
        "assistant.services.assistant_chat.decide_actions",
        new_callable=AsyncMock,
    ) as mock_decide:
        from action_orchestrator.schemas import PlannedAction

        mock_decide.return_value = (
            [
                PlannedAction(
                    action_id="break_down_task",
                    name="Break down",
                    parameters={"steps": ["Shop Sunday", "Prep proteins"]},
                )
            ],
            None,
            LlmUsageOut(),
        )
        c.post(
            f"/api/v1/assistant/chat/send?project_id={project_id}",
            json={"message": "go ahead with the plan"},
        )
        assert mock_decide.await_count == 1
        ctx = mock_decide.await_args.kwargs["context"]
        conv = ctx.get("conversation_history")
        assert isinstance(conv, list)
        assert any("meal prep" in t.get("content", "").lower() for t in conv)


def test_assistant_reply_for_clarifying_questions():
    from assistant.services.assistant_chat import _assistant_reply_for_actions
    from todos.schemas import PlannedActionOut

    actions = [
        PlannedActionOut(
            action_id="ask_clarifying_questions",
            name="Ask clarifying questions",
            parameters={
                "item_id": 2,
                "questions": ["Do you have any dietary restrictions?", "How many meals per day?"],
            },
        )
    ]
    text = _assistant_reply_for_actions(actions)
    assert "prepared" not in text.lower()
    assert "action(s)" not in text
    assert "dietary restrictions" in text
    assert "meals per day" in text
    assert "see the questions below" not in text.lower()

    actions_with_form = [
        PlannedActionOut(
            action_id="ask_clarifying_questions",
            name="Ask clarifying questions",
            parameters={
                "questions": ["Do you have any dietary restrictions?"],
                "form": {
                    "purpose": "clarifying",
                    "title": "Quick questions",
                    "fields": [
                        {
                            "id": "restrictions",
                            "label": "Dietary restrictions",
                            "kind": "textarea",
                        }
                    ],
                },
            },
        )
    ]
    form_reply = _assistant_reply_for_actions(actions_with_form)
    assert "form below" in form_reply.lower()

    text_with_thought = _assistant_reply_for_actions(
        actions,
        thought="I've prepared 1 action(s) for your review.",
    )
    assert "prepared" not in text_with_thought.lower()
    assert "details" in text_with_thought.lower() or "questions" in text_with_thought.lower()


def test_chat_stores_proposal_snapshot_and_resolves_on_apply(client, test_engine):
    c, _mock_cls, _mock_inst = client
    with Session(test_engine) as session:
        seed_todo_domain_if_empty(session)

    pr = c.post("/api/v1/projects", json={"name": "Proposal snapshot project"})
    project_id = pr.json()["id"]

    from action_orchestrator.schemas import PlannedAction

    with patch(
        "assistant.services.assistant_chat.decide_actions",
        new_callable=AsyncMock,
    ) as mock_decide:
        mock_decide.return_value = (
            [
                PlannedAction(
                    action_id="create_item",
                    name="Create item",
                    parameters={"title": "Meal prep"},
                )
            ],
            "I've prepared 1 action(s) for your review.",
            LlmUsageOut(),
        )
        send = c.post(
            f"/api/v1/assistant/chat/send?project_id={project_id}",
            json={"message": "Help me meal prep"},
        )
    assert send.status_code == 200
    body = send.json()
    assistant_msgs = [m for m in body["messages"] if m["role"] == "assistant"]
    assert assistant_msgs
    last = assistant_msgs[-1]
    assert last.get("proposed_actions")
    assert last.get("proposal_status") == "pending"
    assert "prepared" not in last["content"].lower()

    thread_id = body["thread_id"]
    apply = c.post(
        f"/api/v1/assistant/chat/apply?project_id={project_id}",
        json={
            "thread_id": thread_id,
            "actions": body["pending_actions"],
        },
    )
    assert apply.status_code == 200

    thread = c.get(
        f"/api/v1/assistant/chat/thread?project_id={project_id}&thread_id={thread_id}"
    ).json()
    resolved = [m for m in thread["messages"] if m.get("proposal_status") == "approved"]
    assert len(resolved) == 1


def test_build_clarifying_form_infers_controls():
    from assistant.clarifying_form import build_clarifying_form

    form = build_clarifying_form(
        [
            "Do you have any specific calorie or macro goals (e.g., high protein, low carb)?",
            "Are there any specific ingredients you love or want to avoid?",
            "How many days of the week do you typically meal prep for?",
            "Do you prefer a variety of different meals or the same few meals repeated?",
        ]
    )
    assert form is not None
    assert form.get("purpose") == "clarifying"
    fields = form["fields"]
    assert len(fields) == 4
    assert fields[0]["kind"] == "single_select"
    assert "high protein" in fields[0]["options"][0]
    assert fields[1]["kind"] == "textarea"
    assert fields[2]["kind"] == "text"
    assert fields[3]["kind"] == "single_select"


def test_chat_clarifying_questions_inline_without_pending(client, test_engine):
    c, _mock_cls, _mock_inst = client
    with Session(test_engine) as session:
        seed_todo_domain_if_empty(session)

    pr = c.post("/api/v1/projects", json={"name": "Inline clarify project"})
    project_id = pr.json()["id"]

    from action_orchestrator.schemas import PlannedAction

    with patch(
        "assistant.services.assistant_chat.decide_actions",
        new_callable=AsyncMock,
    ) as mock_decide:
        mock_decide.return_value = (
            [
                PlannedAction(
                    action_id="ask_clarifying_questions",
                    name="ask_clarifying_questions",
                    parameters={
                        "questions": [
                            "Which day do you want to prep?",
                            "Any foods to avoid this week?",
                        ],
                    },
                )
            ],
            None,
            LlmUsageOut(),
        )
        send = c.post(
            f"/api/v1/assistant/chat/send?project_id={project_id}",
            json={"message": "now plan"},
        )
    assert send.status_code == 200
    body = send.json()
    assert body.get("pending_form") is not None
    assert body["pending_form"].get("purpose") == "clarifying"
    fields = body["pending_form"]["fields"]
    assert len(fields) >= 2
    assert "form below" in body["content"].lower()
    assert len(body["pending_actions"]) == 1
    assert body["pending_actions"][0]["action_id"] == "ask_clarifying_questions"


def test_normalize_drops_empty_clarifying_questions():
    from assistant.services.assistant_chat import _normalize_planned_actions
    from todos.schemas import PlannedActionOut

    raw = [
        PlannedActionOut(
            action_id="ask_clarifying_questions",
            name="ask_clarifying_questions",
            parameters={},
        ),
        PlannedActionOut(
            action_id="create_item",
            name="Create item",
            parameters={"title": "Meal prep"},
        ),
    ]
    out = _normalize_planned_actions(raw)
    assert len(out) == 1
    assert out[0].action_id == "create_item"


def test_send_clears_stale_informational_pending(client, test_engine):
    c, _mock_cls, _mock_inst = client
    with Session(test_engine) as session:
        seed_todo_domain_if_empty(session)

    pr = c.post("/api/v1/projects", json={"name": "Clear stale pending project"})
    project_id = pr.json()["id"]

    with Session(test_engine) as session:
        now = datetime.utcnow()
        chat = AssistantChatThread(
            project_id=project_id,
            title="Stale pending chat",
            created_at=now,
            updated_at=now,
        )
        chat.set_messages(
            [
                {"role": "user", "content": "Plan meals"},
                {
                    "role": "assistant",
                    "content": "A few questions below.",
                    "proposed_actions": [
                        {
                            "action_id": "ask_clarifying_questions",
                            "name": "Ask clarifying questions",
                            "parameters": {"questions": ["Any allergies?"]},
                        }
                    ],
                    "proposal_status": "pending",
                },
            ]
        )
        chat.set_pending_actions(
            [
                {
                    "action_id": "ask_clarifying_questions",
                    "name": "Ask clarifying questions",
                    "parameters": {"questions": ["Any allergies?"]},
                }
            ]
        )
        session.add(chat)
        session.commit()
        thread_id = chat.id

    from action_orchestrator.schemas import PlannedAction

    with patch(
        "assistant.services.assistant_chat.decide_actions",
        new_callable=AsyncMock,
    ) as mock_decide:
        mock_decide.return_value = (
            [
                PlannedAction(
                    action_id="suggest_next_steps",
                    name="suggest_next_steps",
                    parameters={"guidance": "Start with proteins."},
                )
            ],
            None,
            LlmUsageOut(),
        )
        send = c.post(
            f"/api/v1/assistant/chat/send?project_id={project_id}",
            json={"message": "no allergies", "thread_id": thread_id},
        )
    assert send.status_code == 200
    assert send.json()["pending_actions"] == []


def test_assistant_apply_clarifying_questions_notes_guidance(client, test_engine):
    c, _mock_cls, _mock_inst = client
    with Session(test_engine) as session:
        seed_todo_domain_if_empty(session)

    pr = c.post("/api/v1/projects", json={"name": "Clarify project"})
    project_id = pr.json()["id"]

    r = c.post(
        f"/api/v1/assistant/chat/apply?project_id={project_id}",
        json={
            "actions": [
                {
                    "action_id": "ask_clarifying_questions",
                    "name": "Ask clarifying questions",
                    "parameters": {"questions": ["Any allergies?"]},
                }
            ]
        },
    )
    assert r.status_code == 200
    data = r.json()
    assert "Questions noted" in data["applied"]
    assert "Any allergies?" in data["guidance"]


def test_fitness_gaps_inject_form_on_chat(client, test_engine):
    c, _mock_cls, _mock_inst = client
    with Session(test_engine) as session:
        seed_todo_domain_if_empty(session)

    pr = c.post("/api/v1/projects", json={"name": "Gap project"})
    project_id = pr.json()["id"]

    with patch(
        "assistant.services.assistant_chat.decide_actions",
        new_callable=AsyncMock,
    ) as mock_decide:
        mock_decide.return_value = ([], None, LlmUsageOut())
        r = c.post(
            f"/api/v1/assistant/chat/send?project_id={project_id}",
            json={"message": "Plan my workouts this week"},
        )
    assert r.status_code == 200
    body = r.json()
    assert body.get("pending_form") is not None
    assert body["pending_form"].get("title") == "Fitness profile"


def test_assistant_reset_requires_confirm(client, test_engine):
    c, _mock_cls, _mock_inst = client
    with Session(test_engine) as session:
        seed_todo_domain_if_empty(session)

    pr = c.post("/api/v1/projects", json={"name": "Reset confirm project"})
    project_id = pr.json()["id"]

    bad = c.post(f"/api/v1/assistant/reset?project_id={project_id}", json={"confirm": False})
    assert bad.status_code == 400


def test_assistant_reset_clears_board_and_chat(client, test_engine):
    c, _mock_cls, _mock_inst = client
    with Session(test_engine) as session:
        seed_todo_domain_if_empty(session)

    pr = c.post("/api/v1/projects", json={"name": "Reset workspace project"})
    project_id = pr.json()["id"]
    dash = c.get(f"/api/v1/assistant/dashboard?project_id={project_id}").json()
    board_id = dash["board_id"]

    c.post(
        f"/api/v1/todos/boards/{board_id}/items",
        json={"title": "Old task", "status": "backlog"},
    )

    with Session(test_engine) as session:
        now = datetime.utcnow()
        chat = AssistantChatThread(
            project_id=project_id,
            title="Old chat",
            created_at=now,
            updated_at=now,
        )
        chat.set_messages(
            [
                {"role": "user", "content": "Hello"},
                {"role": "assistant", "content": "Hi there"},
            ]
        )
        session.add(chat)
        profile = AssistantDomainProfile(
            project_id=project_id,
            domain="fitness",
            created_at=datetime.utcnow(),
            updated_at=datetime.utcnow(),
        )
        profile.set_profile({"weight_kg": 70})
        session.add(profile)
        session.add(
            AssistantReview(
                project_id=project_id,
                status="pending",
                summary="Stale review",
                created_at=now,
                updated_at=now,
            )
        )
        session.commit()

    reset = c.post(
        f"/api/v1/assistant/reset?project_id={project_id}",
        json={"confirm": True},
    )
    assert reset.status_code == 200
    data = reset.json()
    assert data["project_id"] == project_id
    assert data["board_id"] > 0
    assert data["thread_id"] > 0

    dash2 = c.get(f"/api/v1/assistant/dashboard?project_id={project_id}").json()
    assert dash2["board_id"] == data["board_id"]
    assert dash2["stats"]["total_items"] == 0

    threads = c.get(f"/api/v1/assistant/chat/threads?project_id={project_id}").json()
    assert len(threads["threads"]) == 1
    assert threads["threads"][0]["id"] == data["thread_id"]
    assert threads["threads"][0]["message_count"] == 0

    with Session(test_engine) as session:
        stale_items = session.exec(
            select(TodoItem).where(TodoItem.title == "Old task")
        ).all()
        assert stale_items == []
        old_chats = session.exec(
            select(AssistantChatThread).where(AssistantChatThread.title == "Old chat")
        ).all()
        assert old_chats == []
        chats = session.exec(
            select(AssistantChatThread).where(AssistantChatThread.project_id == project_id)
        ).all()
        assert len(chats) == 1
        profiles = session.exec(
            select(AssistantDomainProfile).where(
                AssistantDomainProfile.project_id == project_id
            )
        ).all()
        assert len(profiles) == 1
        assert profiles[0].domain == "fitness"
        assert profiles[0].get_profile().get("weight_kg") == 70
        reviews = session.exec(
            select(AssistantReview).where(AssistantReview.project_id == project_id)
        ).all()
        assert reviews == []


def test_dismiss_review_persists(client, test_engine):
    c, _mock_cls, _mock_inst = client
    with Session(test_engine) as session:
        seed_todo_domain_if_empty(session)

    pr = c.post("/api/v1/projects", json={"name": "Review dismiss project"})
    project_id = pr.json()["id"]

    from assistant.models import AssistantReview
    from time_utils import utc_now_naive

    with Session(test_engine) as session:
        now = utc_now_naive()
        row = AssistantReview(
            project_id=project_id,
            status="pending",
            summary="Progress review complete.",
            created_at=now,
            updated_at=now,
        )
        row.set_proposed_actions([])
        session.add(row)
        session.commit()
        session.refresh(row)
        review_id = row.id

    pending = c.get(f"/api/v1/assistant/reviews/pending?project_id={project_id}")
    assert pending.status_code == 200
    assert len(pending.json()["reviews"]) == 1

    dismissed = c.post(f"/api/v1/assistant/reviews/{review_id}/dismiss")
    assert dismissed.status_code == 200
    assert dismissed.json()["status"] == "dismissed"

    pending2 = c.get(f"/api/v1/assistant/reviews/pending?project_id={project_id}")
    assert pending2.status_code == 200
    assert pending2.json()["reviews"] == []
