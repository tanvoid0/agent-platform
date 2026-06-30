"""Tests for assistant message → planner profile routing."""

from assistant.services.assistant_router import route_profile_slug, reviewer_profile_slug


def test_explicit_delegate_wins():
    assert route_profile_slug("buy groceries", explicit="finance-planner") == "finance-planner"


def test_shopping_routes_grocery_message():
    assert route_profile_slug("Build my grocery list for Costco this week") == "shopping-planner"


def test_code_task_routes_software_message():
    assert route_profile_slug("Debug the failing API unit test in Python") == "code-task-planner"


def test_research_scout_routes_learning_message():
    assert route_profile_slug("Research and compare options for note-taking apps") == "research-scout"


def test_day_prioritizer_routes_today_focus():
    assert route_profile_slug("What should I focus on today? I'm overwhelmed") == "day-prioritizer"


def test_progress_reviewer_routes_weekly_review():
    assert route_profile_slug("Run my weekly review — how am I doing?") == "progress-reviewer"


def test_habit_coach_routes_routine():
    assert route_profile_slug("Help me build a morning routine streak") == "habit-coach"


def test_professional_not_triggered_by_homework():
    assert route_profile_slug("Finish math homework tonight") != "professional-planner"


def test_default_when_no_keywords():
    assert route_profile_slug("Hello there") == "personal-assistant"


def test_reviewer_profile_slug():
    assert reviewer_profile_slug() == "progress-reviewer"
