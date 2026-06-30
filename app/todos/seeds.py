"""Seed data: todo-board-ops action set + planner profiles."""

from __future__ import annotations

from action_orchestrator.models import Action, ActionSet
from action_orchestrator.schemas import ActionCreate, ActionSetCreate
from action_orchestrator.registry import create_action_set
from sqlmodel import Session, select

from todos.board_templates import get_board_template
from todos.models import PlannerAgentProfile, TodoBoard, TodoCategory

DEFAULT_MODEL = "gemma4:31b-cloud"

PLANNER_ACTION_SUFFIX = (
    " Use only todo-board-ops actions. Prefer ask_clarifying_questions when scope is unclear "
    "(always include a questions array with 2–4 specific strings); "
    "present_planning_form when profile_gaps for your domain is non-empty (see domain_form_templates); "
    "create_item, schedule_item, and set_due_date for concrete outcomes; "
    "break_down_task for step lists; export_ics_event when the user wants calendar blocks."
)

REVIEWER_ROLE_PROMPT = (
    "You are ProgressReviewer on a Personal Assistant planning team. "
    "Analyze completion stats, overdue items, habit consistency, and reported challenges. "
    "Propose concrete plan adjustments: reschedule overdue tasks, break down stuck items, adjust habits, "
    "or suggest focus areas. The user executes — you review direction and keep them on track. "
    "Be honest but supportive. Prioritize sustainable progress over perfection. "
    "Prefer propose_review, adjust_plan, log_completion, and break_down_task."
    + PLANNER_ACTION_SUFFIX
)


def _planner_prompt(role: str) -> str:
    return role + PLANNER_ACTION_SUFFIX

TODO_BOARD_OPS_ACTIONS: list[ActionCreate] = [
    ActionCreate(
        action_id="move_item_status",
        name="Move item status",
        description="Move a todo item to a different kanban column.",
        parameters={
            "type": "object",
            "properties": {
                "item_id": {"type": "integer"},
                "status": {
                    "type": "string",
                    "enum": ["plan", "backlog", "in_progress", "review", "done"],
                },
            },
            "required": ["item_id", "status"],
        },
        execution_mode="client",
    ),
    ActionCreate(
        action_id="update_item",
        name="Update item",
        description="Edit title, description, or priority of a todo item.",
        parameters={
            "type": "object",
            "properties": {
                "item_id": {"type": "integer"},
                "title": {"type": "string"},
                "description": {"type": "string"},
                "priority": {"type": "integer"},
            },
            "required": ["item_id"],
        },
        execution_mode="client",
    ),
    ActionCreate(
        action_id="add_subtask",
        name="Add subtask",
        description="Append a step to the item plan_json.",
        parameters={
            "type": "object",
            "properties": {
                "item_id": {"type": "integer"},
                "step": {"type": "string"},
                "done": {"type": "boolean"},
            },
            "required": ["item_id", "step"],
        },
        execution_mode="client",
    ),
    ActionCreate(
        action_id="suggest_next_steps",
        name="Suggest next steps",
        description="Return guidance text without mutating the board.",
        parameters={
            "type": "object",
            "properties": {
                "item_id": {"type": "integer"},
                "guidance": {"type": "string"},
            },
            "required": ["item_id", "guidance"],
        },
        execution_mode="client",
    ),
    ActionCreate(
        action_id="break_down_task",
        name="Break down task",
        description="Produce plan steps and write them to plan_json.",
        parameters={
            "type": "object",
            "properties": {
                "item_id": {"type": "integer"},
                "steps": {
                    "type": "array",
                    "items": {
                        "type": "object",
                        "properties": {
                            "step": {"type": "string"},
                            "done": {"type": "boolean"},
                        },
                    },
                },
            },
            "required": ["item_id", "steps"],
        },
        execution_mode="client",
    ),
    ActionCreate(
        action_id="ask_clarifying_questions",
        name="Ask clarifying questions",
        description=(
            "Ask the user specific questions before planning. "
            "Required: questions (array of 2–4 short strings). "
            "Optional: fields[] with id, label, kind (boolean|single_select|multi_select|text|textarea), "
            "options for selects — shown as yes/no, dropdowns, and inputs instead of free chat. "
            "Do not call without concrete questions."
        ),
        parameters={
            "type": "object",
            "properties": {
                "item_id": {"type": "integer"},
                "questions": {"type": "array", "items": {"type": "string"}},
                "fields": {
                    "type": "array",
                    "items": {
                        "type": "object",
                        "properties": {
                            "id": {"type": "string"},
                            "label": {"type": "string"},
                            "kind": {"type": "string"},
                            "options": {"type": "array", "items": {"type": "string"}},
                            "required": {"type": "boolean"},
                            "helpText": {"type": "string"},
                        },
                    },
                },
            },
            "required": ["item_id", "questions"],
        },
        execution_mode="client",
    ),
    ActionCreate(
        action_id="present_planning_form",
        name="Present planning form",
        description=(
            "Show a structured form to collect missing user details (fitness stats, travel dates, "
            "diet restrictions, etc.). Use when user_domain_profiles lacks required fields. "
            "Prefer domain_form_templates in context when available. Answers are saved automatically."
        ),
        parameters={
            "type": "object",
            "properties": {
                "item_id": {"type": "integer"},
                "domain": {
                    "type": "string",
                    "enum": ["fitness", "travel", "nutrition", "finance", "professional", "general"],
                },
                "form": {
                    "type": "object",
                    "description": "Planning form spec with title, description, fields[]",
                },
            },
            "required": ["form"],
        },
        execution_mode="client",
    ),
    ActionCreate(
        action_id="export_markdown_checklist",
        name="Export markdown checklist",
        description="Produce a markdown checklist the user can download.",
        parameters={
            "type": "object",
            "properties": {
                "item_id": {"type": "integer"},
                "title": {"type": "string"},
                "lines": {"type": "array", "items": {"type": "string"}},
            },
            "required": ["item_id"],
        },
        execution_mode="client",
    ),
    ActionCreate(
        action_id="export_ics_event",
        name="Export calendar event",
        description="Produce an iCalendar (.ics) event block for a scheduled item.",
        parameters={
            "type": "object",
            "properties": {
                "item_id": {"type": "integer"},
                "summary": {"type": "string"},
                "start": {"type": "string", "description": "YYYYMMDD or YYYYMMDDTHHMMSS"},
                "end": {"type": "string"},
                "description": {"type": "string"},
            },
            "required": ["item_id", "start"],
        },
        execution_mode="client",
    ),
    ActionCreate(
        action_id="trigger_webhook",
        name="Trigger webhook",
        description=(
            "POST a JSON payload to an external webhook URL (n8n, Zapier, Make). "
            "Runs on the server when the user approves the action."
        ),
        parameters={
            "type": "object",
            "properties": {
                "item_id": {"type": "integer"},
                "webhook_url": {
                    "type": "string",
                    "description": "HTTPS URL to POST to",
                },
                "payload": {
                    "type": "object",
                    "description": "JSON body sent to the webhook",
                },
            },
            "required": ["item_id", "webhook_url"],
        },
        execution_mode="server",
    ),
    ActionCreate(
        action_id="create_item",
        name="Create item",
        description="Create a new task, habit, goal, or chore on the assistant board.",
        parameters={
            "type": "object",
            "properties": {
                "title": {"type": "string"},
                "description": {"type": "string"},
                "status": {"type": "string", "enum": ["plan", "backlog", "in_progress", "review", "done"]},
                "category_id": {"type": "integer"},
                "item_kind": {"type": "string", "enum": ["task", "habit", "goal", "review", "chore"]},
                "time_horizon": {"type": "string", "enum": ["day", "week", "month", "goal"]},
                "due_at": {"type": "string", "description": "ISO8601 datetime"},
                "scheduled_at": {"type": "string", "description": "ISO8601 datetime"},
                "parent_item_id": {"type": "integer"},
                "priority": {"type": "integer"},
            },
            "required": ["title"],
        },
        execution_mode="client",
    ),
    ActionCreate(
        action_id="schedule_item",
        name="Schedule item",
        description="Set when the user should work on an item.",
        parameters={
            "type": "object",
            "properties": {
                "item_id": {"type": "integer"},
                "scheduled_at": {"type": "string", "description": "ISO8601 datetime"},
                "time_horizon": {"type": "string", "enum": ["day", "week", "month", "goal"]},
            },
            "required": ["item_id", "scheduled_at"],
        },
        execution_mode="client",
    ),
    ActionCreate(
        action_id="set_due_date",
        name="Set due date",
        description="Set or update the deadline for an item.",
        parameters={
            "type": "object",
            "properties": {
                "item_id": {"type": "integer"},
                "due_at": {"type": "string", "description": "ISO8601 datetime"},
            },
            "required": ["item_id", "due_at"],
        },
        execution_mode="client",
    ),
    ActionCreate(
        action_id="create_habit",
        name="Create habit",
        description="Create a recurring habit item with cadence.",
        parameters={
            "type": "object",
            "properties": {
                "title": {"type": "string"},
                "description": {"type": "string"},
                "category_id": {"type": "integer"},
                "recurrence": {
                    "type": "object",
                    "properties": {
                        "cadence": {"type": "string", "enum": ["daily", "weekly", "custom"]},
                        "days_of_week": {"type": "array", "items": {"type": "integer"}},
                    },
                },
                "time_horizon": {"type": "string", "enum": ["day", "week", "month", "goal"]},
            },
            "required": ["title"],
        },
        execution_mode="client",
    ),
    ActionCreate(
        action_id="log_completion",
        name="Log completion",
        description="Record task completion with time spent, difficulty, and notes.",
        parameters={
            "type": "object",
            "properties": {
                "item_id": {"type": "integer"},
                "time_spent_minutes": {"type": "integer"},
                "difficulty": {"type": "string", "enum": ["easy", "medium", "hard"]},
                "notes": {"type": "string"},
                "blockers": {"type": "string"},
            },
            "required": ["item_id"],
        },
        execution_mode="client",
    ),
    ActionCreate(
        action_id="create_subtask_item",
        name="Create subtask item",
        description="Create a child todo item linked to a parent task.",
        parameters={
            "type": "object",
            "properties": {
                "parent_item_id": {"type": "integer"},
                "title": {"type": "string"},
                "description": {"type": "string"},
                "due_at": {"type": "string"},
                "scheduled_at": {"type": "string"},
            },
            "required": ["parent_item_id", "title"],
        },
        execution_mode="client",
    ),
    ActionCreate(
        action_id="propose_review",
        name="Propose review",
        description="Suggest a progress review session for the user.",
        parameters={
            "type": "object",
            "properties": {
                "reason": {"type": "string"},
                "focus_areas": {"type": "array", "items": {"type": "string"}},
            },
            "required": ["reason"],
        },
        execution_mode="client",
    ),
    ActionCreate(
        action_id="adjust_plan",
        name="Adjust plan",
        description="Reschedule, reprioritize, or re-scope items based on progress.",
        parameters={
            "type": "object",
            "properties": {
                "item_id": {"type": "integer"},
                "title": {"type": "string"},
                "description": {"type": "string"},
                "due_at": {"type": "string"},
                "scheduled_at": {"type": "string"},
                "time_horizon": {"type": "string", "enum": ["day", "week", "month", "goal"]},
                "status": {"type": "string", "enum": ["plan", "backlog", "in_progress", "review", "done"]},
                "priority": {"type": "integer"},
            },
            "required": ["item_id"],
        },
        execution_mode="client",
    ),
    ActionCreate(
        action_id="store_user_profile",
        name="Store user profile",
        description=(
            "Persist user personal/domain data (fitness stats, travel prefs, etc.) to project memory. "
            "Use after collecting info via form or chat."
        ),
        parameters={
            "type": "object",
            "properties": {
                "domain": {
                    "type": "string",
                    "enum": ["fitness", "travel", "nutrition", "finance", "professional", "general"],
                },
                "data": {"type": "object", "description": "Key-value profile fields to merge"},
            },
            "required": ["domain", "data"],
        },
        execution_mode="client",
    ),
]

PLANNER_PROFILES: list[dict] = [
    {
        "slug": "sprint-planner",
        "name": "SprintPlanner",
        "requirement_type": "sprint",
        "system_prompt": _planner_prompt(
            "You are SprintPlanner. Break epics into sprint-sized, actionable items (1–3 day chunks). "
            "Use break_down_task and create_subtask_item for structured delivery plans."
        ),
    },
    {
        "slug": "research-scout",
        "name": "ResearchScout",
        "requirement_type": "research",
        "system_prompt": _planner_prompt(
            "You are ResearchScout. Turn vague goals into research plans with sources, questions, "
            "and next actions. Use ask_clarifying_questions when scope is unclear."
        ),
    },
    {
        "slug": "life-admin",
        "name": "LifeAdmin",
        "requirement_type": "life_admin",
        "system_prompt": _planner_prompt(
            "You are LifeAdmin. Handle errands, deadlines, and simple next actions. "
            "Keep plans short; use create_item, set_due_date, and break_down_task with few steps."
        ),
    },
    {
        "slug": "code-task-planner",
        "name": "CodeTaskPlanner",
        "requirement_type": "code_task",
        "system_prompt": _planner_prompt(
            "You are CodeTaskPlanner. Produce technical breakdowns, implementation steps, "
            "and review checklists for software tasks. Use create_subtask_item for multi-file work."
        ),
    },
    {
        "slug": "nutrition-coach",
        "name": "NutritionCoach",
        "requirement_type": "nutrition",
        "system_prompt": _planner_prompt(
            "You are NutritionCoach. Plan balanced meals and practical prep steps. "
            "Check user_domain_profiles.nutrition — if profile_gaps.nutrition is non-empty, "
            "call present_planning_form with the nutrition template before creating meal tasks. "
            "Respect dietary_requirements and allergies. Use store_user_profile when the user shares diet info."
        ),
    },
    {
        "slug": "fitness-coach",
        "name": "FitnessCoach",
        "requirement_type": "fitness",
        "system_prompt": _planner_prompt(
            "You are FitnessCoach. Design realistic workout plans and recovery habits. "
            "Before planning workouts you NEED: sex, age, height_cm, weight_kg, fitness_goal, "
            "experience_level, equipment — check user_domain_profiles.fitness and profile_gaps.fitness. "
            "If any required field is missing, use present_planning_form with domain=fitness. "
            "Never guess body stats. After profile is complete, use create_item and schedule_item."
        ),
    },
    {
        "slug": "travel-planner",
        "name": "TravelPlanner",
        "requirement_type": "travel",
        "system_prompt": _planner_prompt(
            "You are TravelPlanner. Break trips into research, bookings, itinerary, and packing. "
            "Check user_domain_profiles.travel — if profile_gaps.travel is non-empty, use "
            "present_planning_form with domain=travel before detailed planning. "
            "Use store_user_profile when user shares trip details."
        ),
    },
    {
        "slug": "shopping-planner",
        "name": "ShoppingPlanner",
        "requirement_type": "shopping",
        "system_prompt": _planner_prompt(
            "You are ShoppingPlanner. Organize shopping lists by store aisle or category. "
            "For grocery lists use break_down_task with grocery_groups: "
            '[{"category": "Produce", "items": ["apples", "spinach"]}, ...]. '
            "Keep lists concise; use create_item for separate shopping runs when helpful."
        ),
    },
    {
        "slug": "mentorship-coach",
        "name": "MentorshipCoach",
        "requirement_type": "mentorship",
        "system_prompt": _planner_prompt(
            "You are MentorshipCoach. Help set learning goals, milestones, and reflection prompts. "
            "Focus on sustainable growth — use create_item with item_kind=goal sparingly."
        ),
    },
    {
        "slug": "calendar-organizer",
        "name": "CalendarOrganizer",
        "requirement_type": "calendar",
        "system_prompt": _planner_prompt(
            "You are CalendarOrganizer. Propose time blocks and realistic schedules with buffer time. "
            "Prefer schedule_item, set_due_date, adjust_plan, and export_ics_event for timed commitments."
        ),
    },
    {
        "slug": "finance-planner",
        "name": "FinancePlanner",
        "requirement_type": "finance",
        "system_prompt": _planner_prompt(
            "You are FinancePlanner. Help with budgets, bill reminders, and savings goals. "
            "Check profile_gaps.finance — use present_planning_form when budget context is missing."
        ),
    },
    {
        "slug": "professional-planner",
        "name": "ProfessionalPlanner",
        "requirement_type": "professional",
        "system_prompt": _planner_prompt(
            "You are ProfessionalPlanner. Plan career milestones and skill development. "
            "Check profile_gaps.professional — use present_planning_form when role/goals unknown."
        ),
    },
    {
        "slug": "day-prioritizer",
        "name": "DayPrioritizer",
        "requirement_type": "day_plan",
        "system_prompt": _planner_prompt(
            "You are DayPrioritizer. Help the user choose what matters today given deadlines, "
            "energy, and backlog. Prefer adjust_plan and move_item_status; suggest a short top-3, "
            "not a huge new task list."
        ),
    },
    {
        "slug": "habit-coach",
        "name": "HabitCoach",
        "requirement_type": "habit",
        "system_prompt": _planner_prompt(
            "You are HabitCoach. Design sustainable routines and streak-friendly habits (not workout plans). "
            "Prefer create_habit and small break_down_task steps; use log_completion for check-ins."
        ),
    },
    {
        "slug": "progress-reviewer",
        "name": "ProgressReviewer",
        "requirement_type": "review",
        "system_prompt": REVIEWER_ROLE_PROMPT,
    },
    {
        "slug": "personal-assistant",
        "name": "PersonalAssistant",
        "requirement_type": "personal_assistant",
        "system_prompt": _planner_prompt(
            "You are the Personal Assistant. Triage goals and synthesize plans — the user executes, "
            "you organize. Check user_domain_profiles and profile_gaps. When a specialist needs details, "
            "use present_planning_form with the matching domain template before create_item. "
            "Use store_user_profile to save facts the user mentions in chat."
        ),
    },
]


def _default_categories() -> list[dict]:
    """Seed columns from the personal-assistant board template when available."""
    template = get_board_template("personal-assistant")
    if template:
        return [
            {"name": c.name, "color": c.color, "profile_slug": c.profile_slug}
            for c in template.categories
        ]
    return [
        {"name": "Work", "color": "#6366f1", "profile_slug": "code-task-planner"},
        {"name": "Personal", "color": "#10b981", "profile_slug": "life-admin"},
        {"name": "Learning", "color": "#f59e0b", "profile_slug": "research-scout"},
    ]


def _sync_todo_board_ops_actions(session: Session, action_set: ActionSet) -> None:
    """Upsert actions so existing DBs gain new todo-board-ops capabilities."""
    from action_orchestrator.models import Action

    existing = {
        a.action_id: a
        for a in session.exec(select(Action).where(Action.set_id == action_set.id)).all()
    }
    for spec in TODO_BOARD_OPS_ACTIONS:
        row = existing.get(spec.action_id)
        if row:
            row.name = spec.name
            row.description = spec.description
            row.execution_mode = spec.execution_mode
            row.set_parameters(spec.parameters)
            session.add(row)
        else:
            action = Action(
                set_id=action_set.id,
                action_id=spec.action_id,
                name=spec.name,
                description=spec.description,
                execution_mode=spec.execution_mode,
                endpoint=spec.endpoint,
            )
            action.set_parameters(spec.parameters)
            session.add(action)
    session.commit()


def _get_or_create_action_set(session: Session) -> ActionSet:
    existing = session.exec(
        select(ActionSet).where(ActionSet.name == "todo-board-ops")
    ).first()
    if existing:
        _sync_todo_board_ops_actions(session, existing)
        return existing
    return create_action_set(
        session,
        ActionSetCreate(
            name="todo-board-ops",
            description="Default actions for todo board agent operations",
            actions=TODO_BOARD_OPS_ACTIONS,
            metadata={"domain": "todos"},
        ),
    )


def seed_todo_domain(session: Session) -> None:
    """Idempotent seed: action set, planner profiles (upsert), default board (create once)."""
    action_set = _get_or_create_action_set(session)

    profiles_by_slug: dict[str, PlannerAgentProfile] = {}
    for spec in PLANNER_PROFILES:
        row = session.exec(
            select(PlannerAgentProfile).where(PlannerAgentProfile.slug == spec["slug"])
        ).first()
        if not row:
            row = PlannerAgentProfile(
                slug=spec["slug"],
                name=spec["name"],
                requirement_type=spec["requirement_type"],
                system_prompt=spec["system_prompt"],
                default_model=DEFAULT_MODEL,
                action_set_id=action_set.id,
            )
        else:
            row.system_prompt = spec["system_prompt"]
            row.name = spec["name"]
            row.requirement_type = spec["requirement_type"]
            row.action_set_id = action_set.id
        session.add(row)
        profiles_by_slug[spec["slug"]] = row
    session.commit()
    for row in profiles_by_slug.values():
        session.refresh(row)

    board = session.exec(select(TodoBoard)).first()
    if not board:
        board = TodoBoard(
            name="Personal planning",
            description="Daily planner board with domain categories",
            default_model=DEFAULT_MODEL,
        )
        session.add(board)
        session.flush()

        for i, cat_spec in enumerate(_default_categories()):
            profile = profiles_by_slug.get(cat_spec["profile_slug"])
            session.add(
                TodoCategory(
                    board_id=board.id,
                    name=cat_spec["name"],
                    color=cat_spec["color"],
                    sort_order=i,
                    planner_profile_id=profile.id if profile else None,
                )
            )
        session.commit()


def seed_todo_domain_if_empty(session: Session) -> None:
    """Backward-compatible alias for startup seed."""
    seed_todo_domain(session)


def get_todo_action_set_id(session: Session) -> int | None:
    row = session.exec(select(ActionSet).where(ActionSet.name == "todo-board-ops")).first()
    return row.id if row else None
