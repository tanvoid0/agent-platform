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
        "category": "Career",
        "description": (
            "Multi-specialist CV and job-prep team: HR lens, ATS structure, domain technical depth, and "
            "interview coaching. The planner maps these roles into parallel or sequential process tasks; "
            "the lead synthesizes prioritized edits, gap analysis vs target roles, and a prep plan."
        ),
        "color": "#4F46E5",
        "roster": {
            "roles": [
                {
                    "id": "cv-review-lead",
                    "name": "Career Review Lead",
                    "description": (
                        "Clarifies target role, seniority, industry, and geography; assigns focus areas; "
                        "reconciles HR, technical, ATS, and interview-prep input; delivers one prioritized "
                        "action list (quick wins vs deeper rewrites) and suggested next process goals."
                    ),
                    "modality": "text",
                    "parent_id": None,
                    "accent_color": "#4F46E5",
                },
                {
                    "id": "cv-hr-lens",
                    "name": "HR & Recruiter Lens",
                    "description": (
                        "Reviews like a hiring manager: role fit, employment gaps, title inflation, summary "
                        "positioning, culture signals, and what recruiters skim in the first pass; flags "
                        "credibility risks and missing hooks for the stated job family."
                    ),
                    "modality": "text",
                    "parent_id": "cv-review-lead",
                    "accent_color": "#7C3AED",
                },
                {
                    "id": "cv-structure-ats",
                    "name": "ATS & Structure Specialist",
                    "description": (
                        "Checks layout, section order, headings, file/parse friendliness, keyword alignment "
                        "to the target JD, density, and standard sections; proposes ATS-safe structure changes."
                    ),
                    "modality": "text",
                    "parent_id": "cv-hr-lens",
                    "accent_color": "#6366F1",
                },
                {
                    "id": "cv-technical-domain",
                    "name": "Technical Domain Reviewer",
                    "description": (
                        "Deep review of skills, projects, and impact for the target discipline (e.g. software, "
                        "data, product, design): stack credibility, scope/seniority signals, weak bullets, "
                        "jargon balance, and portfolio/GitHub pointers; suggests metric-led rewrites."
                    ),
                    "modality": "text",
                    "parent_id": "cv-review-lead",
                    "accent_color": "#0EA5E9",
                },
                {
                    "id": "cv-interview-prep",
                    "name": "Interview & Job Prep Coach",
                    "description": (
                        "Turns CV gaps into a prep plan: likely interview themes, stories to rehearse (STAR), "
                        "questions to expect, skills to upsell, and follow-up materials (cover letter angles, "
                        "LinkedIn headline); optional mock Q&A outline tied to the target role."
                    ),
                    "modality": "text",
                    "parent_id": "cv-review-lead",
                    "accent_color": "#10B981",
                },
            ]
        },
    },
    {
        "name": "Personal Life Assistant",
        "category": "Personal",
        "description": (
            "Hierarchical personal planning team: a Personal Assistant triages user goals and "
            "delegates to domain managers (fitness, finance, professional, travel, health, life admin). "
            "Specialists under each manager produce actionable tasks for the user to execute. "
            "A Progress Reviewer validates direction, deadlines, and habit consistency."
        ),
        "color": "#8B5CF6",
        "roster": {
            "roles": [
                {
                    "id": "personal-assistant",
                    "name": "Personal Assistant",
                    "description": (
                        "Primary intake and triage. Understands user goals, routes to the right domain "
                        "manager, synthesizes daily/weekly/monthly plans, and keeps the user focused on "
                        "what to do next — the user executes; you plan and organize."
                    ),
                    "modality": "text",
                    "parent_id": None,
                    "accent_color": "#8B5CF6",
                },
                {
                    "id": "fitness-planner",
                    "name": "Fitness Planner",
                    "description": (
                        "Plans workouts, recovery, and fitness goals with realistic schedules and "
                        "progressive overload."
                    ),
                    "modality": "text",
                    "parent_id": "personal-assistant",
                    "accent_color": "#F59E0B",
                },
                {
                    "id": "workout-coach",
                    "name": "Workout Coach",
                    "description": "Designs specific workout sessions and exercise progressions.",
                    "modality": "text",
                    "parent_id": "fitness-planner",
                    "accent_color": "#FB923C",
                },
                {
                    "id": "recovery-advisor",
                    "name": "Recovery Advisor",
                    "description": "Plans rest days, mobility, sleep, and recovery habits.",
                    "modality": "text",
                    "parent_id": "fitness-planner",
                    "accent_color": "#FBBF24",
                },
                {
                    "id": "finance-planner",
                    "name": "Finance Planner",
                    "description": (
                        "Budgeting, savings goals, bill reminders, and financial task breakdowns."
                    ),
                    "modality": "text",
                    "parent_id": "personal-assistant",
                    "accent_color": "#10B981",
                },
                {
                    "id": "budget-tracker",
                    "name": "Budget Tracker",
                    "description": "Tracks spending categories and suggests budget adjustments.",
                    "modality": "text",
                    "parent_id": "finance-planner",
                    "accent_color": "#34D399",
                },
                {
                    "id": "savings-advisor",
                    "name": "Savings Advisor",
                    "description": "Plans savings milestones and financial habit building.",
                    "modality": "text",
                    "parent_id": "finance-planner",
                    "accent_color": "#6EE7B7",
                },
                {
                    "id": "professional-planner",
                    "name": "Professional Planner",
                    "description": (
                        "Career goals, skill development, and professional milestone planning."
                    ),
                    "modality": "text",
                    "parent_id": "personal-assistant",
                    "accent_color": "#6366F1",
                },
                {
                    "id": "career-coach",
                    "name": "Career Coach",
                    "description": "Career moves, networking, and professional development tasks.",
                    "modality": "text",
                    "parent_id": "professional-planner",
                    "accent_color": "#818CF8",
                },
                {
                    "id": "skills-developer",
                    "name": "Skills Developer",
                    "description": "Learning paths, practice schedules, and skill milestones.",
                    "modality": "text",
                    "parent_id": "professional-planner",
                    "accent_color": "#A5B4FC",
                },
                {
                    "id": "travel-planner-mgr",
                    "name": "Travel Planner",
                    "description": "Trip planning, itineraries, bookings, and packing lists.",
                    "modality": "text",
                    "parent_id": "personal-assistant",
                    "accent_color": "#0EA5E9",
                },
                {
                    "id": "research-scout-travel",
                    "name": "Research Scout",
                    "description": "Researches destinations, options, and travel logistics.",
                    "modality": "text",
                    "parent_id": "travel-planner-mgr",
                    "accent_color": "#38BDF8",
                },
                {
                    "id": "itinerary-builder",
                    "name": "Itinerary Builder",
                    "description": "Builds day-by-day itineraries and booking checklists.",
                    "modality": "text",
                    "parent_id": "travel-planner-mgr",
                    "accent_color": "#7DD3FC",
                },
                {
                    "id": "health-nutrition-mgr",
                    "name": "Health & Nutrition",
                    "description": "Meal planning, wellness habits, and nutrition goals.",
                    "modality": "text",
                    "parent_id": "personal-assistant",
                    "accent_color": "#22C55E",
                },
                {
                    "id": "nutrition-coach-mgr",
                    "name": "Nutrition Coach",
                    "description": "Meal plans, dietary goals, and grocery lists.",
                    "modality": "text",
                    "parent_id": "health-nutrition-mgr",
                    "accent_color": "#4ADE80",
                },
                {
                    "id": "wellness-monitor",
                    "name": "Wellness Monitor",
                    "description": "Tracks wellness habits, sleep, hydration, and health routines.",
                    "modality": "text",
                    "parent_id": "health-nutrition-mgr",
                    "accent_color": "#86EFAC",
                },
                {
                    "id": "life-admin-mgr",
                    "name": "Life Admin",
                    "description": "Errands, chores, appointments, and day-to-day admin tasks.",
                    "modality": "text",
                    "parent_id": "personal-assistant",
                    "accent_color": "#64748B",
                },
                {
                    "id": "errands-organizer",
                    "name": "Errands Organizer",
                    "description": "Groups errands efficiently and sets realistic deadlines.",
                    "modality": "text",
                    "parent_id": "life-admin-mgr",
                    "accent_color": "#94A3B8",
                },
                {
                    "id": "calendar-coordinator",
                    "name": "Calendar Coordinator",
                    "description": "Time blocks, scheduling, and calendar-aware planning.",
                    "modality": "text",
                    "parent_id": "life-admin-mgr",
                    "accent_color": "#CBD5E1",
                },
                {
                    "id": "progress-reviewer",
                    "name": "Progress Reviewer",
                    "description": (
                        "Reviews completion rates, deadline adherence, habit streaks, and reported "
                        "challenges. Proposes plan corrections and ensures the user stays on track "
                        "toward their goals."
                    ),
                    "modality": "text",
                    "parent_id": "personal-assistant",
                    "accent_color": "#EC4899",
                },
            ]
        },
    },
]
