import type { TeamRoster } from "../api/types";

/** Curated examples; roster roles use output modality (text-only until the server routes others). */
export type TeamTemplatePreset = {
  id: string;
  name: string;
  description: string;
  color: string;
  /** Optional template category (library chip / filter). */
  category?: string;
  roster: TeamRoster;
};

export const TEAM_TEMPLATE_PRESETS: TeamTemplatePreset[] = [
  {
    id: "consultant-workshop",
    name: "Consultant workshop",
    description: "Single lead — quick experiments and written deliverables.",
    color: "#16a34a",
    category: "Workshop",
    roster: {
      roles: [
        {
          id: "lead",
          name: "Workshop lead",
          description: "Frames the ask, drafts the outcome, and hands off notes.",
          modality: "text",
          parent_id: null,
          accent_color: "#16a34a",
        },
      ],
    },
  },
  {
    id: "notepad-mentorship",
    name: "Notepad mentorship",
    description: "Lead plus implementer and reviewer — guided delivery.",
    color: "#2563eb",
    category: "Mentorship",
    roster: {
      roles: [
        {
          id: "lead",
          name: "Mentor",
          description: "Keeps scope clear and reviews direction.",
          modality: "text",
          parent_id: null,
          accent_color: "#2563eb",
        },
        {
          id: "junior",
          name: "Junior implementer",
          description: "Implements tasks and asks for checkpoints.",
          modality: "text",
          parent_id: "lead",
          accent_color: "#16a34a",
        },
        {
          id: "reviewer",
          name: "Reviewer",
          description: "Sanity-checks changes and suggests fixes.",
          modality: "text",
          parent_id: "lead",
          accent_color: "#9333ea",
        },
      ],
    },
  },
  {
    id: "content-sprint",
    name: "Content sprint",
    description: "Parallel writers with a coordinating editor.",
    color: "#ea580c",
    category: "Content",
    roster: {
      roles: [
        {
          id: "editor",
          name: "Editor",
          description: "Owns tone, deadlines, and final assembly.",
          modality: "text",
          parent_id: null,
          accent_color: "#ea580c",
        },
        {
          id: "a",
          name: "Writer A",
          description: "Drafts assigned sections.",
          modality: "text",
          parent_id: "editor",
          accent_color: "#2563eb",
        },
        {
          id: "b",
          name: "Writer B",
          description: "Drafts assigned sections.",
          modality: "text",
          parent_id: "editor",
          accent_color: "#16a34a",
        },
        {
          id: "fact",
          name: "Fact checker",
          description: "Traces claims to sources.",
          modality: "text",
          parent_id: "editor",
          accent_color: "#9333ea",
        },
      ],
    },
  },
  {
    id: "product-engineering",
    name: "Autonomous product engineering",
    description: "Software-style tree: lead → senior + QA; backend chain to frontend.",
    color: "#2563eb",
    category: "Engineering",
    roster: {
      roles: [
        {
          id: "lead",
          name: "Team lead",
          description: "Coordinates priorities, integrates work, requests human review when needed.",
          modality: "text",
          parent_id: null,
          accent_color: "#2563eb",
        },
        {
          id: "senior",
          name: "Senior full-stack developer",
          description: "Owns architecture and splits work across the stack.",
          modality: "text",
          parent_id: "lead",
          accent_color: "#16a34a",
        },
        {
          id: "qa",
          name: "QA & documentation",
          description: "Tests flows and keeps docs aligned.",
          modality: "text",
          parent_id: "lead",
          accent_color: "#9333ea",
        },
        {
          id: "backend",
          name: "Backend developer",
          description: "APIs, persistence, and integration points.",
          modality: "text",
          parent_id: "senior",
          accent_color: "#ca8a04",
        },
        {
          id: "frontend",
          name: "Frontend developer",
          description: "UI, accessibility, and client behavior.",
          modality: "text",
          parent_id: "backend",
          accent_color: "#dc2626",
        },
      ],
    },
  },
];
