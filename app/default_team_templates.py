"""
Default team template seeds aligned with the-delegation `src/data/agents.ts` (AGENTIC_SETS).

Text-only templates are seeded into `teamtemplate` on fresh migrations.
Image / music / video presets are kept as Python data below but not seeded until the platform
can route outputs to the right modalities (see FIXME on each).
"""

from __future__ import annotations

from typing import Any

# --- Future: not seeded (need visual / audio / video support in agent-platform) ---
# Source roster trees: the-delegation AGENTIC_SETS entries photo-studio, music-studio, film-studio.

# FIXME(visual): Seed "Nano Banana Lab" when image-generation (or image handoff) is supported end-to-end.
_FUTURE_PHOTO_STUDIO_ROSTER: dict[str, Any] = {
    "roles": [
        {
            "id": "art-director",
            "name": "Art Director",
            "description": "Synthesizes descriptions into valid Nano Banana prompts.",
            "modality": "text",
            "parent_id": None,
            "accent_color": "#FBBF24",
        },
        {
            "id": "scene-designer",
            "name": "Scene Designer",
            "description": "Focuses on Subject and Action within the scene.",
            "modality": "text",
            "parent_id": "art-director",
            "accent_color": "#F59E0B",
        },
        {
            "id": "lighting-stylist",
            "name": "Lighting Stylist",
            "description": "Focuses on Composition, Lighting, and Style/Materiality.",
            "modality": "text",
            "parent_id": "art-director",
            "accent_color": "#E0E672",
        },
    ]
}

# FIXME(audio): Seed "Lyria Factory" when audio/music output pipeline is supported.
_FUTURE_MUSIC_STUDIO_ROSTER: dict[str, Any] = {
    "roles": [
        {
            "id": "master-producer",
            "name": "Master Producer",
            "description": "Orchestrates the 4 pillars of sound into a cohesive track.",
            "modality": "text",
            "parent_id": None,
            "accent_color": "#43E47C",
        },
        {
            "id": "genre-expert",
            "name": "Genre Expert",
            "description": "Defines style, mood, and global aesthetic (e.g., Synthwave, Lofi).",
            "modality": "text",
            "parent_id": "master-producer",
            "accent_color": "#74D295",
        },
        {
            "id": "tempo-architect",
            "name": "Tempo Architect",
            "description": "Specifies BPM, rhythmical complexity, and time signatures.",
            "modality": "text",
            "parent_id": "master-producer",
            "accent_color": "#92D540",
        },
        {
            "id": "instrumentalist",
            "name": "Instrumentalist",
            "description": "Selects timbres, arrangement, and orchestration layers.",
            "modality": "text",
            "parent_id": "master-producer",
            "accent_color": "#40D5AD",
        },
        {
            "id": "dynamics-engineer",
            "name": "Dynamics Engineer",
            "description": "Controls volume, texture, contrast, and emotional progression.",
            "modality": "text",
            "parent_id": "master-producer",
            "accent_color": "#50BB55",
        },
    ]
}

# FIXME(video): Seed "Veo Studio" when video/cinematic output pipeline is supported.
_FUTURE_FILM_STUDIO_ROSTER: dict[str, Any] = {
    "roles": [
        {
            "id": "film-director",
            "name": "Film Director",
            "description": "Orchestrates visuals and soundstage with global cinematic vision.",
            "modality": "text",
            "parent_id": None,
            "accent_color": "#E64347",
        },
        {
            "id": "visual-lead",
            "name": "Visual Lead",
            "description": "Manages cinematography and VFX direction.",
            "modality": "text",
            "parent_id": "film-director",
            "accent_color": "#F17DC5",
        },
        {
            "id": "cinematographer",
            "name": "Cinematographer",
            "description": "Defines camera work, shot composition, and subject action.",
            "modality": "text",
            "parent_id": "visual-lead",
            "accent_color": "#E643C5",
        },
        {
            "id": "audio-lead",
            "name": "Audio Lead",
            "description": "Manages the soundstage: Dialogue, SFX, and Ambience.",
            "modality": "text",
            "parent_id": "film-director",
            "accent_color": "#7CE630",
        },
        {
            "id": "sound-designer",
            "name": "Sound Designer",
            "description": 'Specifies SFX (SFX:), Ambient Noise (Ambient noise:), and Dialogue (" ").',
            "modality": "text",
            "parent_id": "audio-lead",
            "accent_color": "#50BB55",
        },
    ]
}


# --- Seeded templates (text output; matches agents.ts unboring-net, consultant-workshop, pr-agency, cv-reviewer-agency) ---

SEED_TEAM_TEMPLATES: list[dict[str, Any]] = [
    {
        "name": "Autonomous Product Engineering Team",
        "description": (
            "A fully autonomous software delivery team for planning, building, testing, "
            "documenting, and shipping production features."
        ),
        "color": "#4285F4",
        "roster": {
            "roles": [
                {
                    "id": "unboring-team-lead",
                    "name": "Team Lead",
                    "description": (
                        "Experience: 12+ years. Expertise: technical leadership, architecture decisions, "
                        "delivery planning, and stakeholder communication."
                    ),
                    "modality": "text",
                    "parent_id": None,
                    "accent_color": "#4285F4",
                },
                {
                    "id": "unboring-senior-fullstack",
                    "name": "Senior Full-Stack Developer",
                    "description": (
                        "Experience: 8+ years. Expertise: end-to-end architecture, API contracts, "
                        "database modeling, delivery decomposition, and mentoring."
                    ),
                    "modality": "text",
                    "parent_id": "unboring-team-lead",
                    "accent_color": "#34A853",
                },
                {
                    "id": "unboring-backend-developer",
                    "name": "Backend Developer",
                    "description": (
                        "Experience: 4+ years. Expertise: backend services, database migrations, "
                        "performance, reliability, and secure API implementation."
                    ),
                    "modality": "text",
                    "parent_id": "unboring-senior-fullstack",
                    "accent_color": "#FBBC05",
                },
                {
                    "id": "unboring-frontend-developer",
                    "name": "Frontend Developer",
                    "description": (
                        "Experience: 3+ years. Expertise: UI implementation, component architecture, "
                        "accessibility, client state, and integration testing."
                    ),
                    "modality": "text",
                    "parent_id": "unboring-backend-developer",
                    "accent_color": "#EA4335",
                },
                {
                    "id": "unboring-qa-docs-specialist",
                    "name": "QA Tester & Documentation Writer",
                    "description": (
                        "Experience: 4+ years. Expertise: test planning, regression validation, bug triage, "
                        "release notes, runbooks, and user/developer documentation."
                    ),
                    "modality": "text",
                    "parent_id": "unboring-team-lead",
                    "accent_color": "#7C4DFF",
                },
            ]
        },
    },
    {
        "name": "Consultant Workshop",
        "description": (
            "A normal text team with one extra capability: the Consultant can suggest execution-team "
            "layouts, save them as templates after you approve, and shape the project brief. With "
            "multi-project (delegation) enabled, they can also create a new empty server project when you agree."
        ),
        "color": "#0D9488",
        "roster": {
            "roles": [
                {
                    "id": "workshop-consultant",
                    "name": "Consultant",
                    "description": (
                        "Runs the same lead workflow as other teams: discuss goals, run the board, deliver "
                        "when done. Additionally proposes team structures (saved as templates after you approve), "
                        "captures the project brief when ready (including without a rigid 'start' phrase if scope "
                        "is clear), and can create a new delegation project when you want a fresh workspace."
                    ),
                    "modality": "text",
                    "parent_id": None,
                    "accent_color": "#0D9488",
                },
            ]
        },
    },
    {
        "name": "PR Agency",
        "description": "A sequential pipeline for media outreach: from strategy to press drafting.",
        "color": "#E34B99",
        "roster": {
            "roles": [
                {
                    "id": "pr-director",
                    "name": "PR Director",
                    "description": "Oversees media relations, strategic communications, and brand reputation.",
                    "modality": "text",
                    "parent_id": None,
                    "accent_color": "#E34B99",
                },
                {
                    "id": "media-strategist",
                    "name": "Media Strategist",
                    "description": "Identifies key media outlets and manages journalist outreach.",
                    "modality": "text",
                    "parent_id": "pr-director",
                    "accent_color": "#E6D979",
                },
                {
                    "id": "press-writer",
                    "name": "Press Writer",
                    "description": "Drafts press releases and media kits based on strategic goals.",
                    "modality": "text",
                    "parent_id": "media-strategist",
                    "accent_color": "#5E888E",
                },
            ]
        },
    },
    {
        "name": "CV Reviewer Agency",
        "description": (
            "A sequential review pipeline for résumés/CVs: structure and ATS alignment, then narrative "
            "and impact, with a lead synthesizing clear, actionable feedback."
        ),
        "color": "#4F46E5",
        "roster": {
            "roles": [
                {
                    "id": "cv-review-lead",
                    "name": "Career Review Lead",
                    "description": (
                        "Frames the review goals (role, seniority, geography), reconciles specialist input, "
                        "and delivers a prioritized, actionable summary for the candidate."
                    ),
                    "modality": "text",
                    "parent_id": None,
                    "accent_color": "#4F46E5",
                },
                {
                    "id": "cv-structure-ats",
                    "name": "Structure & ATS Specialist",
                    "description": (
                        "Checks layout, section order, headings, and keyword fit for applicant tracking systems; "
                        "flags parse risks, density issues, and missing standard sections."
                    ),
                    "modality": "text",
                    "parent_id": "cv-review-lead",
                    "accent_color": "#6366F1",
                },
                {
                    "id": "cv-narrative-impact",
                    "name": "Narrative & Impact Editor",
                    "description": (
                        "Improves bullets for outcomes and metrics, clarity and tone, and the story arc from "
                        "summary through experience; suggests concrete rewrites."
                    ),
                    "modality": "text",
                    "parent_id": "cv-structure-ats",
                    "accent_color": "#818CF8",
                },
            ]
        },
    },
]
