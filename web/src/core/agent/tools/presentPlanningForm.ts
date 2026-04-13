import type {
  PlanningFormAnswers,
  PlanningFormField,
  PlanningFormFieldKind,
  PlanningFormSpec,
} from '../../llm/types';
import type { AgentActionContext } from '../ToolRegistry';

const MAX_FIELDS = 12;
const MAX_OPTION_LEN = 120;
const MAX_LABEL_LEN = 200;
const MAX_TITLE_LEN = 200;
const MAX_DESC_LEN = 1200;

const FIELD_KINDS = new Set<PlanningFormFieldKind>([
  'boolean',
  'single_select',
  'multi_select',
  'text',
  'textarea',
]);

function clip(s: string, max: number): string {
  if (s.length <= max) return s;
  return s.slice(0, max);
}

function isNonEmptyString(v: unknown): v is string {
  return typeof v === 'string' && v.trim().length > 0;
}

export function parsePlanningFormArgs(raw: unknown): PlanningFormSpec | null {
  if (!raw || typeof raw !== 'object') return null;
  const o = raw as Record<string, unknown>;

  const title =
    typeof o.title === 'string' && o.title.trim().length > 0
      ? clip(o.title.trim(), MAX_TITLE_LEN)
      : undefined;
  const description =
    typeof o.description === 'string' && o.description.trim().length > 0
      ? clip(o.description.trim(), MAX_DESC_LEN)
      : undefined;

  if (!Array.isArray(o.fields) || o.fields.length === 0 || o.fields.length > MAX_FIELDS) {
    return null;
  }

  const seenIds = new Set<string>();
  const fields: PlanningFormField[] = [];

  for (const item of o.fields) {
    if (!item || typeof item !== 'object') return null;
    const f = item as Record<string, unknown>;
    if (!isNonEmptyString(f.id) || !isNonEmptyString(f.label)) return null;
    const id = f.id.trim();
    if (seenIds.has(id)) return null;
    seenIds.add(id);

    const kind = f.kind;
    if (typeof kind !== 'string' || !FIELD_KINDS.has(kind as PlanningFormFieldKind)) return null;
    const k = kind as PlanningFormFieldKind;

    let options: string[] | undefined;
    if (k === 'single_select' || k === 'multi_select') {
      if (!Array.isArray(f.options) || f.options.length < 2) return null;
      options = [];
      for (const opt of f.options) {
        if (!isNonEmptyString(opt)) return null;
        options.push(clip(opt.trim(), MAX_OPTION_LEN));
      }
    } else if (Array.isArray(f.options) && f.options.length > 0) {
      return null;
    }

    const required = f.required === true;
    const helpText =
      typeof f.helpText === 'string' && f.helpText.trim().length > 0
        ? clip(f.helpText.trim(), 400)
        : undefined;

    fields.push({
      id,
      label: clip(f.label.trim(), MAX_LABEL_LEN),
      kind: k,
      ...(options ? { options } : {}),
      ...(required ? { required: true } : {}),
      ...(helpText ? { helpText } : {}),
    });
  }

  return { ...(title ? { title } : {}), ...(description ? { description } : {}), fields };
}

/** Build the user chat message sent after the form is submitted (human-readable + JSON for the model). */
export function buildPlanningAnswersUserMessage(spec: PlanningFormSpec, answers: PlanningFormAnswers): string {
  const lines: string[] = ['Planning answers (structured):', ''];
  for (const field of spec.fields) {
    const v = answers[field.id];
    let display: string;
    if (field.kind === 'multi_select' && Array.isArray(v)) {
      display = v.length ? v.join(', ') : '(none selected)';
    } else if (field.kind === 'boolean') {
      display = v === true ? 'Yes' : v === false ? 'No' : '(not set)';
    } else if (typeof v === 'string') {
      display = v.trim() || '(empty)';
    } else {
      display = String(v ?? '');
    }
    lines.push(`- **${field.label}** (\`${field.id}\`): ${display}`);
  }
  lines.push('');
  lines.push('```json');
  lines.push(JSON.stringify({ planningFormAnswers: answers }, null, 0));
  lines.push('```');
  return lines.join('\n');
}

export function presentPlanningForm(
  _agent: AgentActionContext,
  args: unknown,
): { ok: true; spec: PlanningFormSpec } | { ok: false } {
  const spec = parsePlanningFormArgs(args);
  if (!spec) {
    console.warn('[ToolRegistry] present_planning_form: invalid arguments');
    return { ok: false };
  }
  return { ok: true, spec };
}
