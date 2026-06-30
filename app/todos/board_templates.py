"""Board templates for practical planning domains."""

from __future__ import annotations

from dataclasses import dataclass
from typing import Any


@dataclass(frozen=True)
class BoardTemplateCategory:
    name: str
    color: str
    profile_slug: str


@dataclass(frozen=True)
class BoardTemplate:
    slug: str
    name: str
    description: str
    categories: tuple[BoardTemplateCategory, ...]


BOARD_TEMPLATES: tuple[BoardTemplate, ...] = (
    BoardTemplate(
        slug="life-weekly",
        name="This week",
        description="Personal errands, health habits, and day-to-day admin for the week ahead.",
        categories=(
            BoardTemplateCategory("Personal", "#10b981", "life-admin"),
            BoardTemplateCategory("Errands", "#6366f1", "life-admin"),
            BoardTemplateCategory("Health", "#f59e0b", "fitness-coach"),
        ),
    ),
    BoardTemplate(
        slug="meal-plan",
        name="Meal plan",
        description="Weekly meals, nutrition goals, and grocery shopping.",
        categories=(
            BoardTemplateCategory("Meals", "#22c55e", "nutrition-coach"),
            BoardTemplateCategory("Shopping", "#8b5cf6", "shopping-planner"),
        ),
    ),
    BoardTemplate(
        slug="travel-trip",
        name="Trip planner",
        description="Research, bookings, and packing for an upcoming trip.",
        categories=(
            BoardTemplateCategory("Research", "#0ea5e9", "travel-planner"),
            BoardTemplateCategory("Bookings", "#6366f1", "travel-planner"),
            BoardTemplateCategory("Packing", "#10b981", "life-admin"),
        ),
    ),
    BoardTemplate(
        slug="coding-sprint",
        name="Dev sprint",
        description="Features, bugs, and learning for a software sprint.",
        categories=(
            BoardTemplateCategory("Features", "#4285F4", "code-task-planner"),
            BoardTemplateCategory("Bugs", "#EA4335", "code-task-planner"),
            BoardTemplateCategory("Learning", "#FBBC05", "research-scout"),
        ),
    ),
    BoardTemplate(
        slug="mentorship",
        name="Growth",
        description="Goals, skills, and reflection with a mentorship coach.",
        categories=(
            BoardTemplateCategory("Goals", "#a855f7", "mentorship-coach"),
            BoardTemplateCategory("Skills", "#6366f1", "mentorship-coach"),
            BoardTemplateCategory("Reflection", "#10b981", "life-admin"),
        ),
    ),
    BoardTemplate(
        slug="personal-assistant",
        name="Personal Assistant",
        description=(
            "Daily planning board with domain categories for fitness, finance, professional "
            "growth, travel, health, life admin, and goals."
        ),
        categories=(
            BoardTemplateCategory("Fitness", "#f59e0b", "fitness-coach"),
            BoardTemplateCategory("Finance", "#10b981", "finance-planner"),
            BoardTemplateCategory("Professional", "#6366f1", "professional-planner"),
            BoardTemplateCategory("Travel", "#0ea5e9", "travel-planner"),
            BoardTemplateCategory("Health", "#22c55e", "nutrition-coach"),
            BoardTemplateCategory("Life Admin", "#64748b", "life-admin"),
            BoardTemplateCategory("Goals", "#a855f7", "mentorship-coach"),
        ),
    ),
)

_TEMPLATES_BY_SLUG: dict[str, BoardTemplate] = {t.slug: t for t in BOARD_TEMPLATES}


def get_board_template(slug: str) -> BoardTemplate | None:
    return _TEMPLATES_BY_SLUG.get(slug.strip())


def list_board_templates() -> list[dict[str, Any]]:
    return [
        {
            "slug": t.slug,
            "name": t.name,
            "description": t.description,
            "categories": [
                {
                    "name": c.name,
                    "color": c.color,
                    "profile_slug": c.profile_slug,
                }
                for c in t.categories
            ],
        }
        for t in BOARD_TEMPLATES
    ]
