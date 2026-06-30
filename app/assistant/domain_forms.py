"""Domain intake form templates and required profile fields."""

from __future__ import annotations

from typing import Any

# Fields agents expect per domain (used for gap detection).
DOMAIN_PROFILE_FIELDS: dict[str, list[str]] = {
    "fitness": [
        "sex",
        "age",
        "height_cm",
        "weight_kg",
        "fitness_goal",
        "experience_level",
        "equipment",
    ],
    "travel": [
        "destination",
        "departure_date",
        "return_date",
        "travelers",
        "budget",
        "travel_style",
    ],
    "nutrition": [
        "diet_style",
        "meals_per_day",
        "cooking_time_minutes",
    ],
    "finance": ["monthly_budget", "savings_goal", "primary_focus"],
    "professional": ["current_role", "target_role", "growth_focus"],
}

PROFILE_SLUG_TO_DOMAIN: dict[str, str] = {
    "fitness-coach": "fitness",
    "travel-planner": "travel",
    "nutrition-coach": "nutrition",
    "finance-planner": "finance",
    "professional-planner": "professional",
    "mentorship-coach": "professional",
    "personal-assistant": "general",
    "life-admin": "general",
    "shopping-planner": "nutrition",
    "calendar-organizer": "general",
    "sprint-planner": "general",
    "code-task-planner": "general",
    "research-scout": "general",
    "day-prioritizer": "general",
    "habit-coach": "general",
    "progress-reviewer": "general",
}


def domain_for_profile_slug(slug: str) -> str:
    return PROFILE_SLUG_TO_DOMAIN.get(slug, "general")


def get_domain_form_spec(domain: str) -> dict[str, Any] | None:
    return _DOMAIN_FORMS.get(domain)


def list_profile_domains() -> list[str]:
    """Stable UI order for profile settings."""
    order = ["general", "fitness", "nutrition", "travel", "finance", "professional"]
    known = set(_DOMAIN_FORMS)
    return [d for d in order if d in known] + sorted(known - set(order))


def list_domain_form_specs() -> dict[str, dict[str, Any]]:
    return {d: _DOMAIN_FORMS[d] for d in list_profile_domains()}


def missing_profile_fields(domain: str, profile: dict[str, Any]) -> list[str]:
    required = DOMAIN_PROFILE_FIELDS.get(domain, [])
    missing: list[str] = []
    for key in required:
        val = profile.get(key)
        if val is None or val == "" or val == []:
            missing.append(key)
    return missing


_DOMAIN_FORMS: dict[str, dict[str, Any]] = {
    "general": {
        "title": "About you",
        "description": "Basics shared across assistants — name, location, and free-form notes.",
        "domain": "general",
        "fields": [
            {
                "id": "display_name",
                "label": "Preferred name",
                "kind": "text",
                "required": False,
                "placeholder": "e.g. Alex",
            },
            {
                "id": "pronouns",
                "label": "Pronouns",
                "kind": "single_select",
                "options": ["she/her", "he/him", "they/them", "Prefer not to say"],
                "required": False,
            },
            {
                "id": "timezone",
                "label": "Timezone",
                "kind": "text",
                "required": False,
                "placeholder": "e.g. America/New_York",
            },
            {
                "id": "home_location",
                "label": "Home base",
                "kind": "text",
                "required": False,
                "placeholder": "City or region",
            },
            {
                "id": "personal_notes",
                "label": "Anything else assistants should know",
                "kind": "textarea",
                "required": False,
                "placeholder": "Schedule constraints, dependents, accessibility needs, etc.",
            },
        ],
    },
    "fitness": {
        "title": "Fitness profile",
        "description": "A few details so your workout plan fits you — saved for future sessions.",
        "domain": "fitness",
        "fields": [
            {
                "id": "sex",
                "label": "Sex",
                "kind": "single_select",
                "options": ["Female", "Male", "Non-binary", "Prefer not to say"],
                "required": True,
                "helpText": "Used for training volume and recovery guidance.",
            },
            {
                "id": "age",
                "label": "Age",
                "kind": "text",
                "required": True,
                "placeholder": "e.g. 32",
            },
            {
                "id": "height_cm",
                "label": "Height (cm)",
                "kind": "text",
                "required": True,
                "placeholder": "e.g. 175",
            },
            {
                "id": "weight_kg",
                "label": "Weight (kg)",
                "kind": "text",
                "required": True,
                "placeholder": "e.g. 70",
            },
            {
                "id": "fitness_goal",
                "label": "Primary goal",
                "kind": "single_select",
                "options": [
                    "Lose weight",
                    "Build muscle",
                    "General fitness",
                    "Train for event",
                    "Mobility & recovery",
                ],
                "required": True,
            },
            {
                "id": "experience_level",
                "label": "Experience",
                "kind": "single_select",
                "options": ["Beginner", "Intermediate", "Advanced"],
                "required": True,
            },
            {
                "id": "equipment",
                "label": "Equipment available",
                "kind": "multi_select",
                "options": [
                    "Gym full access",
                    "Home dumbbells",
                    "Resistance bands",
                    "Bodyweight only",
                    "Outdoor running",
                ],
                "required": True,
            },
            {
                "id": "injuries",
                "label": "Injuries or limits (optional)",
                "kind": "text",
                "required": False,
                "placeholder": "e.g. bad knee, lower back",
            },
        ],
    },
    "travel": {
        "title": "Trip details",
        "description": "Tell me about the trip — I'll remember this for packing and bookings.",
        "domain": "travel",
        "fields": [
            {
                "id": "destination",
                "label": "Destination",
                "kind": "text",
                "required": True,
                "placeholder": "City, country, or region",
            },
            {
                "id": "departure_date",
                "label": "Departure date",
                "kind": "text",
                "required": True,
                "placeholder": "YYYY-MM-DD",
            },
            {
                "id": "return_date",
                "label": "Return date",
                "kind": "text",
                "required": False,
                "placeholder": "YYYY-MM-DD",
            },
            {
                "id": "travelers",
                "label": "Travelers",
                "kind": "single_select",
                "options": ["Just me", "Couple", "Family with kids", "Group of friends"],
                "required": True,
            },
            {
                "id": "budget",
                "label": "Budget (approx.)",
                "kind": "single_select",
                "options": ["Budget", "Mid-range", "Comfort", "Luxury", "Flexible"],
                "required": True,
            },
            {
                "id": "travel_style",
                "label": "Trip style",
                "kind": "multi_select",
                "options": [
                    "Sightseeing",
                    "Food & culture",
                    "Relaxation",
                    "Adventure",
                    "Business",
                ],
                "required": True,
            },
            {
                "id": "notes",
                "label": "Must-haves or constraints",
                "kind": "text",
                "required": False,
                "placeholder": "Dietary needs, mobility, visa, etc.",
            },
        ],
    },
    "nutrition": {
        "title": "Nutrition preferences",
        "description": "Saved to personalize meal plans and shopping lists.",
        "domain": "nutrition",
        "fields": [
            {
                "id": "diet_style",
                "label": "Diet style",
                "kind": "single_select",
                "options": [
                    "Omnivore",
                    "Vegetarian",
                    "Vegan",
                    "Pescatarian",
                    "Keto",
                    "Mediterranean",
                    "Other",
                ],
                "required": True,
            },
            {
                "id": "dietary_requirements",
                "label": "Religious / cultural dietary rules",
                "kind": "multi_select",
                "options": ["Halal", "Kosher", "No pork", "No beef"],
                "required": False,
                "helpText": "Applied on top of your diet style when planning meals and shopping lists.",
            },
            {
                "id": "allergies",
                "label": "Allergies / avoid",
                "kind": "text",
                "required": False,
                "placeholder": "e.g. nuts, dairy, shellfish",
            },
            {
                "id": "meals_per_day",
                "label": "Meals per day",
                "kind": "single_select",
                "options": ["2", "3", "4+"],
                "required": True,
            },
            {
                "id": "cooking_time_minutes",
                "label": "Typical cooking time",
                "kind": "single_select",
                "options": ["15 min or less", "30 min", "45+ min", "Meal prep batches"],
                "required": True,
            },
        ],
    },
    "finance": {
        "title": "Finance snapshot",
        "description": "Helps tailor budgets and savings tasks — stored on your project.",
        "domain": "finance",
        "fields": [
            {
                "id": "monthly_budget",
                "label": "Monthly budget focus",
                "kind": "single_select",
                "options": ["Tight", "Moderate", "Comfortable", "Not sure"],
                "required": True,
            },
            {
                "id": "savings_goal",
                "label": "Savings goal",
                "kind": "text",
                "required": False,
                "placeholder": "e.g. emergency fund, vacation",
            },
            {
                "id": "primary_focus",
                "label": "Primary focus",
                "kind": "multi_select",
                "options": ["Bills", "Debt payoff", "Saving", "Investing", "Tracking spending"],
                "required": True,
            },
        ],
    },
    "professional": {
        "title": "Career & growth",
        "description": "Saved to personalize professional development plans.",
        "domain": "professional",
        "fields": [
            {
                "id": "current_role",
                "label": "Current role",
                "kind": "text",
                "required": True,
            },
            {
                "id": "target_role",
                "label": "Target (6–12 months)",
                "kind": "text",
                "required": False,
            },
            {
                "id": "growth_focus",
                "label": "Growth focus",
                "kind": "multi_select",
                "options": ["Skills", "Leadership", "Networking", "Certifications", "Job search"],
                "required": True,
            },
        ],
    },
}
