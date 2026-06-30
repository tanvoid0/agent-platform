"""Route user messages to domain planner profiles."""

from __future__ import annotations

import re

DOMAIN_KEYWORDS: dict[str, list[str]] = {
    "code-task-planner": [
        "bug",
        "feature",
        "code",
        "coding",
        "implement",
        "refactor",
        "api",
        "pull request",
        "github",
        "typescript",
        "python",
        "debug",
        "deploy",
        "software",
        "dev sprint",
        "unit test",
        "lint",
    ],
    "research-scout": [
        "research",
        "read up",
        "sources",
        "literature",
        "survey",
        "compare options",
        "investigate",
        "learn about",
        "notes on",
    ],
    "sprint-planner": [
        "sprint",
        "epic",
        "story points",
        "backlog grooming",
        "iteration plan",
    ],
    "shopping-planner": [
        "shop",
        "shopping",
        "grocery",
        "groceries",
        "costco",
        "supermarket",
        "shopping list",
        "buy list",
        "aisle",
        "pantry",
    ],
    "fitness-coach": [
        "workout",
        "workouts",
        "exercise",
        "exercises",
        "gym",
        "fitness",
        "run",
        "running",
        "lift",
        "lifting",
        "cardio",
        "recovery",
        "stretch",
        "weights",
    ],
    "finance-planner": [
        "budget",
        "finance",
        "money",
        "savings",
        "bill",
        "expense",
        "invest",
        "debt",
        "paycheck",
        "subscription",
    ],
    "professional-planner": [
        "career",
        "professional",
        "promotion",
        "interview",
        "resume",
        "cv",
        "networking",
        "salary",
        "performance review",
    ],
    "travel-planner": [
        "travel",
        "trip",
        "flight",
        "hotel",
        "vacation",
        "itinerary",
        "packing",
        "booking",
        "passport",
    ],
    "nutrition-coach": [
        "meal",
        "meals",
        "nutrition",
        "diet",
        "food",
        "cook",
        "recipe",
        "calorie",
        "breakfast",
        "lunch",
        "dinner",
    ],
    "habit-coach": [
        "habit",
        "routine",
        "streak",
        "daily practice",
        "morning routine",
        "evening routine",
        "consistency",
    ],
    "mentorship-coach": [
        "mentor",
        "milestone",
        "growth plan",
        "reflect",
        "reflection",
        "long-term goal",
        "personal growth",
    ],
    "calendar-organizer": [
        "schedule",
        "calendar",
        "time block",
        "appointment",
        "meeting",
        "block time",
        "availability",
    ],
    "day-prioritizer": [
        "prioritize",
        "priority",
        "what should i do",
        "focus today",
        "top 3",
        "today's plan",
        "overwhelmed",
        "pick three",
        "daily plan",
    ],
    "progress-reviewer": [
        "weekly review",
        "progress review",
        "retrospective",
        "how am i doing",
        "catch up",
        "review my week",
        "what did i finish",
    ],
    "life-admin": [
        "errand",
        "chore",
        "admin",
        "clean",
        "laundry",
        "dry cleaning",
        "renew",
        "paperwork",
    ],
}

# Short tokens matched on word boundaries to reduce false positives (e.g. "work" in homework).
_WORD_BOUNDARY_KEYWORDS: dict[str, list[str]] = {
    "day-prioritizer": ["today", "tomorrow"],
    "life-admin": ["todo", "task"],
}

DEFAULT_PROFILE = "personal-assistant"


def _keyword_matches(text: str, keyword: str) -> bool:
    kw = keyword.lower().strip()
    if not kw:
        return False
    if " " in kw or len(kw) > 8:
        return kw in text
    return bool(re.search(rf"\b{re.escape(kw)}\b", text))


def route_profile_slug(message: str, *, explicit: str | None = None) -> str:
    if explicit and explicit.strip():
        return explicit.strip()
    text = message.lower()
    scores: dict[str, int] = {}
    for slug, keywords in DOMAIN_KEYWORDS.items():
        score = sum(1 for kw in keywords if _keyword_matches(text, kw))
        if score:
            scores[slug] = score
    for slug, keywords in _WORD_BOUNDARY_KEYWORDS.items():
        extra = sum(1 for kw in keywords if _keyword_matches(text, kw))
        if extra:
            scores[slug] = scores.get(slug, 0) + extra
    if not scores:
        return DEFAULT_PROFILE
    return max(scores, key=scores.get)


def reviewer_profile_slug() -> str:
    return "progress-reviewer"
